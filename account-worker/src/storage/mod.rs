//! SQLite 存储层。
//!
//! 只用 `rusqlite` 的 bundled SQLite（与上游核心同一份），避免 CGO / 系统库差异。
//! 迁移靠 `PRAGMA user_version` 递增，且每次迁移都在事务里，失败不留半个库。

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::clock::now_rfc3339;
use crate::rpc::{RpcError, RpcResult};

pub mod authz;
pub mod checkin;
pub mod growth;
pub mod qoder_accounts;
pub mod qoder_profiles;
pub mod qoder_switch;
pub mod travel;

pub use authz::AuthorizationRecord;

const SCHEMA_V1: &str = include_str!("schema.sql");

/// v2：登录态需要的两列。SQLite 的 ALTER TABLE 只能追加列，正好够用。
const MIGRATION_V2: &str = "\
ALTER TABLE accounts ADD COLUMN token_expires_at TEXT;\
ALTER TABLE accounts ADD COLUMN auth_domain TEXT;";

/// v3：登录来源。`own` = AICC 自己扫码/导入的独立授权；`magpie` = 从 Magpie 借用
/// 的 access token（不再刷新，过期后重新同步）。
const MIGRATION_V3: &str = "\
ALTER TABLE accounts ADD COLUMN auth_source TEXT NOT NULL DEFAULT 'own';";

/// v4：Buddy 旅行（PR1c）。开关放在账号上（每账号独立、默认关闭），
/// 运行态单独一张表，事件另记一张，方便回答「谁在什么时候派出/领取了什么」。
const MIGRATION_V4: &str = "\
ALTER TABLE accounts ADD COLUMN travel_enabled INTEGER NOT NULL DEFAULT 0;\
CREATE TABLE IF NOT EXISTS travel_states (\
    account_id           TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,\
    label                TEXT NOT NULL DEFAULT 'unknown',\
    server_state         TEXT NOT NULL DEFAULT '',\
    location_id          INTEGER,\
    location_name        TEXT NOT NULL DEFAULT '',\
    depart_at            INTEGER NOT NULL DEFAULT 0,\
    arrive_at            INTEGER NOT NULL DEFAULT 0,\
    record_id            INTEGER NOT NULL DEFAULT 0,\
    daily_limit_reached  INTEGER NOT NULL DEFAULT 0,\
    claimed              INTEGER NOT NULL DEFAULT 0,\
    reward_credits       INTEGER,\
    claimed_at           TEXT,\
    claimed_record_id    INTEGER NOT NULL DEFAULT 0,\
    depart_date          TEXT,\
    last_checked_at      TEXT,\
    last_depart_attempt_at TEXT,\
    failures             INTEGER NOT NULL DEFAULT 0,\
    retry_after          TEXT,\
    last_error           TEXT,\
    conflict             TEXT,\
    updated_at           TEXT NOT NULL\
);\
CREATE TABLE IF NOT EXISTS travel_events (\
    id         INTEGER PRIMARY KEY AUTOINCREMENT,\
    account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,\
    kind       TEXT NOT NULL,\
    status     TEXT NOT NULL,\
    message    TEXT,\
    credits    INTEGER,\
    created_at TEXT NOT NULL\
);\
CREATE INDEX IF NOT EXISTS idx_travel_events_account ON travel_events (account_id, created_at DESC);";

/// v5：凭据版本号（PR #65 第二轮）。
///
/// 后台任务（自动旅行）可能拿着几秒前的账号快照去做 401 重同步，而这期间用户
/// 扫码把账号换成了自有授权。写凭据之前先比对这个版本号：对不上就说明
/// "我读到的账号已经不是现在这个账号了"，直接放弃，不许覆盖。
///
/// 每次 `upsert_account` / `set_auth_state` 都会 +1 —— 宁可保守地让在途操作作废，
/// 也不能让旧快照把新授权写坏。
const MIGRATION_V5: &str = "\
ALTER TABLE accounts ADD COLUMN auth_epoch INTEGER NOT NULL DEFAULT 0;";

/// v6：一账号多来源授权（PR #66）。
///
/// 只建表。回填是 Rust 步骤（见 `authz::backfill_v6`），因为它要算确定性授权 ID、
/// 按 kind 决定 `refresh_ref` 是 NULL 还是引用 —— 这些属于业务规则，不属于 DDL。
///
/// `refresh_ref` 对 magpie 恒为 NULL 由 `CHECK` 保证：表比"记得别写"可靠。
const MIGRATION_V6_SQL: &str = "\
CREATE TABLE IF NOT EXISTS account_authorizations (\
    id                  TEXT PRIMARY KEY,\
    account_id          TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,\
    kind                TEXT NOT NULL CHECK (kind IN ('own', 'magpie')),\
    access_ref          TEXT NOT NULL,\
    refresh_ref         TEXT,\
    auth_domain         TEXT,\
    token_expires_at    TEXT,\
    status              TEXT NOT NULL DEFAULT 'unknown'\
                        CHECK (status IN ('unknown', 'valid', 'expired', 'invalid', 'denied')),\
    version             INTEGER NOT NULL DEFAULT 0,\
    key_migration_state TEXT NOT NULL DEFAULT 'pending'\
                        CHECK (key_migration_state IN ('pending', 'migrating', 'ready', 'failed')),\
    source_key          TEXT,\
    last_used_at        TEXT,\
    last_error          TEXT,\
    created_at          TEXT NOT NULL,\
    updated_at          TEXT NOT NULL,\
    CHECK (kind <> 'magpie' OR refresh_ref IS NULL),\
    UNIQUE (account_id, kind)\
);\
CREATE INDEX IF NOT EXISTS idx_auth_account ON account_authorizations (account_id, status);\
CREATE INDEX IF NOT EXISTS idx_auth_pending ON account_authorizations (key_migration_state);";

/// v7：镜像指针（双写起点）。
///
/// `auth_mirror_auth_id` 记录兼容镜像来自**哪一行授权**。没有它就无法证明
/// "`auth_status = valid` 说的是 own 那份还是 magpie 那份"（G6）。
const MIGRATION_V7_SQL: &str = "\
ALTER TABLE accounts ADD COLUMN auth_mirror_auth_id TEXT;";

/// v9：成长中心执行记录（PR #68）。
///
/// 只记"AICC 做了什么、服务端怎么回的"，**不缓存任务状态**：任务是否完成
/// 永远以 `/tasks` 为准，这张表只用于界面上的"最近执行结果"与事后排查。
///
/// 为什么不复用 `task_executions`：那张表一条记录对应"一次任务运行"，
/// 没有 `task_code` 与 `energy` 两列，塞进去要么丢信息要么靠拼接字符串。
///
/// v8 是 #66 规划的"删旧钥匙串键"，刻意还没做，所以这里直接跳到 9。
const MIGRATION_V9_SQL: &str = "\
CREATE TABLE IF NOT EXISTS growth_runs (\
    id           INTEGER PRIMARY KEY AUTOINCREMENT,\
    account_id   TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,\
    task_code    TEXT NOT NULL,\
    action       TEXT NOT NULL CHECK (action IN ('accept', 'claim')),\
    outcome      TEXT NOT NULL CHECK (outcome IN ('ok', 'already', 'rejected', 'unknown', 'failed')),\
    credit       INTEGER,\
    energy       INTEGER,\
    error_kind   TEXT,\
    error_detail TEXT,\
    executed_at  TEXT NOT NULL\
);\
CREATE INDEX IF NOT EXISTS idx_growth_runs_account ON growth_runs (account_id, executed_at DESC);\
CREATE INDEX IF NOT EXISTS idx_growth_runs_task ON growth_runs (account_id, task_code, executed_at DESC);";

/// v10：签到结果的两列诊断字段（PR #69）。
///
/// `checkin_records` 原本只有 `status` / `credits` / `checked_at`，够回答"今天签了没"，
/// 但答不了"为什么没签、是谁触发的"。两列都为 NULL 即旧行为，**旧二进制读到多出来的
/// 列会直接忽略**，回退不需要脚本。
///
/// 没有新增表：每次尝试的细节走已有的 `task_executions`（`task_kind = 'daily_checkin'`），
/// 它本来就有 `error_kind` / `error_detail` / `next_retry_at` 三列。
const MIGRATION_V10_SQL: &str = "\
ALTER TABLE checkin_records ADD COLUMN error_kind TEXT;\
ALTER TABLE checkin_records ADD COLUMN execution_source TEXT;";

/// v11：Qoder 双版本多账号 Profile（PR #70）。
///
/// **只新增两张表，不碰任何已有表。**
///
/// 表结构按 M0 实测结论设计：Qoder 客户端忽略 `--user-data-dir`，一个 OS 用户
/// 只有一份 `userData`，所以
///
/// - `live_dir` 是**客户端真实目录**（同版本的多个 Profile 共享同一行值，不做唯一约束）；
/// - `snapshot_dir` 是 AICC 管理的**认证快照**（每个 Profile 独占，唯一）；
/// - 切换账号靠"退出客户端 → 存回当前账号 → 写回目标快照 → 再启动"。
///
/// 为什么不把 `qoder_global` 塞进 `accounts.platform`：见 `storage/qoder_profiles.rs`
/// 顶部注释——那需要重建挂着四组外键的主表，风险与收益不成比例。
const MIGRATION_V11_SQL: &str = "\
CREATE TABLE IF NOT EXISTS qoder_profiles (\
    profile_id       TEXT PRIMARY KEY,\
    edition          TEXT NOT NULL CHECK (edition IN ('qoder_cn', 'qoder_global')),\
    display_name     TEXT NOT NULL,\
    account_uid      TEXT,\
    email            TEXT,\
    app_bundle_id    TEXT,\
    app_path         TEXT,\
    client_version   TEXT,\
    live_dir         TEXT NOT NULL,\
    snapshot_dir     TEXT NOT NULL,\
    auth_file        TEXT,\
    auth_fingerprint TEXT,\
    auth_status      TEXT NOT NULL DEFAULT 'missing'\
                     CHECK (auth_status IN ('unknown','valid','expired','invalid','denied','missing')),\
    expires_at       TEXT,\
    managed          INTEGER NOT NULL DEFAULT 1 CHECK (managed IN (0, 1)),\
    last_opened_at   TEXT,\
    created_at       TEXT NOT NULL,\
    updated_at       TEXT NOT NULL\
);\
CREATE UNIQUE INDEX IF NOT EXISTS ux_qoder_profiles_snapshot ON qoder_profiles (snapshot_dir);\
CREATE UNIQUE INDEX IF NOT EXISTS ux_qoder_profiles_identity \
    ON qoder_profiles (edition, account_uid) WHERE account_uid IS NOT NULL;\
CREATE INDEX IF NOT EXISTS idx_qoder_profiles_edition ON qoder_profiles (edition);\
CREATE INDEX IF NOT EXISTS idx_qoder_profiles_live ON qoder_profiles (edition, live_dir);\
CREATE TABLE IF NOT EXISTS qoder_runtime (\
    profile_id  TEXT PRIMARY KEY REFERENCES qoder_profiles(profile_id) ON DELETE CASCADE,\
    pid         INTEGER NOT NULL,\
    executable  TEXT NOT NULL,\
    launched_at TEXT NOT NULL\
);";

/// v12：快照切换日志（PR #70 安全收尾）。
///
/// 切换是"多文件多步骤"的操作，进程中途崩溃后必须能在重启时把 live 目录
/// 恢复成原账号。日志落在库里而不是内存里，就是为了活过崩溃。
///
/// 唯一索引只约束"同一版本同时最多一条未完成切换"，是并发切换的闸门。
const MIGRATION_V12_SQL: &str = "\
CREATE TABLE IF NOT EXISTS qoder_switch_journal (\
    id          TEXT PRIMARY KEY,\
    profile_id  TEXT NOT NULL,\
    edition     TEXT NOT NULL,\
    phase       TEXT NOT NULL CHECK (phase IN (\
                    'prepared','staged','committed','rolled_back','cleared')),\
    live_dir    TEXT NOT NULL,\
    backup_dir  TEXT NOT NULL,\
    files_json  TEXT NOT NULL,\
    created_at  TEXT NOT NULL,\
    updated_at  TEXT NOT NULL\
);\
CREATE UNIQUE INDEX IF NOT EXISTS ux_qoder_switch_active \
    ON qoder_switch_journal (edition) WHERE phase IN ('prepared', 'staged');\
ALTER TABLE qoder_profiles ADD COLUMN snapshot_hash TEXT;";

/// v13：四平台统一导入（PR #71）。
///
/// 两件事，**都只追加**，不重建任何已有表：
///
/// 1. `accounts.provider`：WorkBuddy 国内版与国际版复用同一张表、同一套授权 /
///    签到 / 成长 / 旅行能力，用 `provider` 区分服务（`workbuddy` / `workbuddy_ai`）。
///    身份仍然是 `(platform, uid)`——同一个 UID 在两个站点都存在时，第二个导入
///    会被明确拒绝（`uid_conflict`），**不做自动合并**，也不覆盖已有账号。
/// 2. `qoder_accounts` / `qoder_authorizations`：Qoder CN / Global 的平台服务账号
///    与它的授权来源。`qoder_profiles` 依旧是本地客户端快照，两者互不依赖——
///    导入 Qoder 账号**不需要**先装客户端。
const MIGRATION_V13_SQL: &str = "\
ALTER TABLE accounts ADD COLUMN provider TEXT NOT NULL DEFAULT 'workbuddy';\
CREATE INDEX IF NOT EXISTS idx_accounts_provider ON accounts (platform, provider);\
CREATE TABLE IF NOT EXISTS qoder_accounts (\
    id         TEXT PRIMARY KEY,\
    edition    TEXT NOT NULL CHECK (edition IN ('qoder_cn', 'qoder_global')),\
    uid        TEXT NOT NULL,\
    nickname   TEXT NOT NULL DEFAULT '',\
    email      TEXT,\
    created_at TEXT NOT NULL,\
    updated_at TEXT NOT NULL,\
    UNIQUE (edition, uid)\
);\
CREATE INDEX IF NOT EXISTS idx_qoder_accounts_edition ON qoder_accounts (edition);\
CREATE TABLE IF NOT EXISTS qoder_authorizations (\
    id               TEXT PRIMARY KEY,\
    account_id       TEXT NOT NULL REFERENCES qoder_accounts(id) ON DELETE CASCADE,\
    source           TEXT NOT NULL CHECK (source IN ('magpie', 'own_browser')),\
    source_key       TEXT,\
    secret_ref       TEXT,\
    auth_status      TEXT NOT NULL DEFAULT 'unknown'\
                     CHECK (auth_status IN ('unknown','valid','expired','invalid','denied','missing')),\
    token_expires_at TEXT,\
    fingerprint      TEXT,\
    created_at       TEXT NOT NULL,\
    updated_at       TEXT NOT NULL,\
    UNIQUE (account_id, source)\
);\
CREATE INDEX IF NOT EXISTS idx_qoder_auth_account ON qoder_authorizations (account_id);";

/// 一次迁移里的一步：纯 SQL，或者需要 Rust 参与的数据迁移。
enum MigrationStep {
    Sql(&'static str),
    Rust(fn(&rusqlite::Transaction<'_>) -> RpcResult<()>),
}

struct Migration {
    version: i64,
    steps: &'static [MigrationStep],
}

/// 迁移清单：按 version 升序，只追加不修改。
///
/// **v6 与 v7 都在本版本里，`Db::open()` 会连续执行它们。** 拆号是为了 diff 可评审、
/// 每一步能单独回滚，不是为了让运行时观察到"只有 v6"的中间态 —— 任何 RPC 都看不到。
/// v8（删旧钥匙串键、下线镜像列）属于后续独立 PR，这里刻意没有。
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        steps: &[MigrationStep::Sql(SCHEMA_V1)],
    },
    Migration {
        version: 2,
        steps: &[MigrationStep::Sql(MIGRATION_V2)],
    },
    Migration {
        version: 3,
        steps: &[MigrationStep::Sql(MIGRATION_V3)],
    },
    Migration {
        version: 4,
        steps: &[MigrationStep::Sql(MIGRATION_V4)],
    },
    Migration {
        version: 5,
        steps: &[MigrationStep::Sql(MIGRATION_V5)],
    },
    Migration {
        version: 6,
        steps: &[
            MigrationStep::Sql(MIGRATION_V6_SQL),
            MigrationStep::Rust(authz::backfill_v6),
        ],
    },
    Migration {
        version: 7,
        steps: &[
            MigrationStep::Sql(MIGRATION_V7_SQL),
            MigrationStep::Rust(authz::seed_mirror_v7),
        ],
    },
    // v8 预留（#66 的旧钥匙串清理），刻意未实施，所以版本号直接排到 9。
    Migration {
        version: 9,
        steps: &[MigrationStep::Sql(MIGRATION_V9_SQL)],
    },
    // v10：签到诊断列。只追加列，不改 v1～v9 里的任何一个字。
    Migration {
        version: 10,
        steps: &[MigrationStep::Sql(MIGRATION_V10_SQL)],
    },
    // v11：Qoder 双版本多账号 Profile。只建新表，不重建 accounts。
    Migration {
        version: 11,
        steps: &[MigrationStep::Sql(MIGRATION_V11_SQL)],
    },
    // v12：快照切换日志（崩溃恢复用）。同样只建新表。
    Migration {
        version: 12,
        steps: &[MigrationStep::Sql(MIGRATION_V12_SQL)],
    },
    // v13：accounts.provider + Qoder 平台账号表（PR #71）。只加列、只建新表。
    Migration {
        version: 13,
        steps: &[MigrationStep::Sql(MIGRATION_V13_SQL)],
    },
];

pub const LATEST_VERSION: i64 = 13;

pub struct Db {
    conn: Connection,
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRecord {
    pub id: String,
    pub platform: String,
    pub uid: String,
    pub nickname: String,
    pub email: Option<String>,
    pub group_id: Option<String>,
    pub auth_status: String,
    pub checkin_enabled: bool,
    pub last_checkin_at: Option<String>,
    pub last_result: Option<String>,
    pub updated_at: String,
    /// 访问令牌到期时间（RFC3339）。令牌本身在钥匙串，这里只有时间。
    pub token_expires_at: Option<String>,
    /// 登录时响应里的 domain，刷新令牌要带上。
    pub auth_domain: Option<String>,
    /// `own`（独立授权）或 `magpie`（借用 Magpie 的 access token）。
    ///
    /// v7 起这是**派生值**：真实状态在 `account_authorizations`，这里只是兼容镜像（G6）。
    pub auth_source: String,
    /// 该账号是否允许自动派 Buddy 旅行。默认关闭，与签到开关互不影响。
    pub travel_enabled: bool,
    /// 凭据版本号。任何一次账号/授权写入都会 +1，用来让在途的旧快照作废。
    pub auth_epoch: i64,
    /// 兼容镜像来自哪一行授权（v7）。为空表示还没镜像过。
    pub auth_mirror_auth_id: Option<String>,
    /// 服务标识（v13）：`workbuddy`（国内版）或 `workbuddy_ai`（国际版）。
    ///
    /// `platform` 仍然是 `workbuddy`——两个站点共用同一套授权 / 签到 / 成长 /
    /// 旅行能力，差别只在 API 域名与 OAuth 档位。新平台（Qoder）不进来。
    pub provider: String,
}

/// 账号写入参数。**所有可选字段都是"补丁"语义**：
/// 没出现的字段保持库里原值，不会被重置成默认值。
///
/// 注意 `None` 同时表示"没传"和"要清空"——本期只支持前者；
/// 清空字段留到 PR2，用单独的显式方法，避免一次 typo 洗掉用户的备注或凭据引用。
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInput {
    pub platform: String,
    pub uid: String,
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub auth_ref: Option<String>,
    #[serde(default)]
    pub profile_ref: Option<String>,
    #[serde(default)]
    pub auth_status: Option<String>,
    #[serde(default)]
    pub checkin_enabled: Option<bool>,
    #[serde(default)]
    pub token_expires_at: Option<String>,
    #[serde(default)]
    pub auth_domain: Option<String>,
    #[serde(default)]
    pub auth_source: Option<String>,
    #[serde(default)]
    pub travel_enabled: Option<bool>,
    /// 只有 WorkBuddy 用：`workbuddy` / `workbuddy_ai`。不传按 `workbuddy`。
    #[serde(default)]
    pub provider: Option<String>,
}

/// `accounts.provider` 的合法取值。
pub const PROVIDERS: &[&str] = &["workbuddy", "workbuddy_ai"];

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountFilter {
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
    #[serde(default)]
    pub auth_status: Option<String>,
    #[serde(default)]
    pub query: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub offset: Option<u32>,
}

pub const PLATFORMS: &[&str] = &["workbuddy", "qoder_cn"];

impl Db {
    pub fn open(path: &Path) -> RpcResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // journal_mode 会返回一行结果，不能用 pragma_update（会报 "Execute returned results"）。
        let _mode: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let mut db = Self {
            conn,
            path: path.to_path_buf(),
        };
        db.migrate()?;
        Ok(db)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn migrate(&mut self) -> RpcResult<i64> {
        let mut current: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        for migration in MIGRATIONS {
            if migration.version <= current {
                continue;
            }
            // 一个版本里的所有步骤在同一个事务里：SQL 与 Rust 数据迁移要么都成，要么都不留痕。
            let tx = self.conn.transaction()?;
            for step in migration.steps {
                match step {
                    MigrationStep::Sql(sql) => tx.execute_batch(sql)?,
                    MigrationStep::Rust(run) => run(&tx)?,
                }
            }
            tx.execute(
                "INSERT OR REPLACE INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![migration.version, now_rfc3339()],
            )?;
            tx.pragma_update(None, "user_version", migration.version)?;
            tx.commit()?;
            current = migration.version;
        }
        Ok(current)
    }

    /// 仅测试用：直接执行一段 SQL，用来注入"写一定会失败"的约束
    /// （例如给 accounts 加一个 BEFORE UPDATE 触发器），生产构建里不存在。
    #[cfg(test)]
    pub fn exec_raw(&mut self, sql: &str) -> RpcResult<()> {
        self.conn.execute_batch(sql)?;
        Ok(())
    }

    pub fn schema_version(&self) -> RpcResult<i64> {
        Ok(self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?)
    }

    pub fn integrity_ok(&self) -> bool {
        self.conn
            .query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))
            .map(|v| v == "ok")
            .unwrap_or(false)
    }

    /// 各表行数，`health` / `storage.status` 用。
    pub fn counts(&self) -> RpcResult<Value> {
        let mut out = serde_json::Map::new();
        for table in [
            "accounts",
            "account_groups",
            "task_definitions",
            "task_executions",
            "checkin_records",
            "credit_snapshots",
            "app_settings",
            "travel_states",
            "travel_events",
            "account_authorizations",
            "qoder_accounts",
            "qoder_authorizations",
        ] {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            let n: i64 = self.conn.query_row(&sql, [], |row| row.get(0))?;
            out.insert(table.to_string(), json!(n));
        }
        Ok(Value::Object(out))
    }

    pub fn status(&self) -> RpcResult<Value> {
        Ok(json!({
            "path": self.path.to_string_lossy(),
            "schemaVersion": self.schema_version()?,
            "latestSchemaVersion": LATEST_VERSION,
            "integrityOk": self.integrity_ok(),
            "walMode": self.conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))?,
            "counts": self.counts()?,
        }))
    }

    // -----------------------------------------------------------------
    // 账号
    // -----------------------------------------------------------------

    pub fn upsert_account(&mut self, input: &AccountInput) -> RpcResult<AccountRecord> {
        if !PLATFORMS.contains(&input.platform.as_str()) {
            return Err(RpcError::invalid_params(format!(
                "platform 只支持 workbuddy / qoder_cn，收到 {}",
                input.platform
            )));
        }
        if input.uid.trim().is_empty() {
            return Err(RpcError::invalid_params("uid 不能为空"));
        }
        let provider = input.provider.as_deref().unwrap_or("workbuddy");
        if !PROVIDERS.contains(&provider) {
            return Err(RpcError::invalid_params(format!(
                "provider 只支持 workbuddy / workbuddy_ai，收到 {provider}"
            )));
        }
        let now = now_rfc3339();
        let id = account_id(&input.platform, &input.uid);
        if let Some(status) = input.auth_status.as_deref() {
            if !["unknown", "valid", "expired", "invalid", "denied"].contains(&status) {
                return Err(RpcError::invalid_params(format!(
                    "auth_status 取值非法：{status}"
                )));
            }
        }

        // 两步写入必须原子：只建行不写字段会留下一条"半成品"账号，
        // 而调用方已经收到错误、不知道它存在。用同一个事务兜住。
        let tx = self.conn.transaction()?;

        // 第一步：不存在就建行（默认值）。存在则什么都不改。
        tx.execute(
            "INSERT INTO accounts (
                 id, platform, uid, nickname, email, group_id, auth_ref, profile_ref,
                 auth_status, checkin_enabled, created_at, updated_at, provider
             ) VALUES (?1, ?2, ?3, '', NULL, NULL, NULL, NULL, 'unknown', 0, ?4, ?4, ?5)
             ON CONFLICT (platform, uid) DO NOTHING",
            params![id, input.platform, input.uid, now, provider],
        )?;

        // 同一 UID 已经在**另一个服务**下存在：WorkBuddy 国内版与国际版是两个
        // 账号，不能因为 UID 相同就共用一行（那会把国际版的令牌塞进国内版）。
        // 主表的 UNIQUE(platform, uid) 是 v1 的既有约束，这里不做重建，
        // 而是明确报冲突，让调用方决定怎么处理。
        let existing_provider: Option<String> = tx
            .query_row(
                "SELECT provider FROM accounts WHERE platform = ?1 AND uid = ?2",
                params![input.platform, input.uid],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(existing) = existing_provider {
            if existing != provider {
                return Err(RpcError::invalid_params(format!(
                    "同一 UID 已属于 {existing}，不能再作为 {provider} 导入；\
                     两个服务是不同账号，不会自动合并"
                ))
                .with_data(json!({
                    "code": "uid_conflict",
                    "existingProvider": existing,
                    "requestedProvider": provider,
                    "uid": input.uid,
                })));
            }
        }

        // 第二步：只更新调用方显式给的字段。
        // 早前用的是 `ON CONFLICT DO UPDATE SET x = excluded.x`，没传
        // checkinEnabled 会把签到开关重置成 false、没传 authRef 会把凭据引用清空，
        // 属于"改备注顺手把账号弄坏"。
        let mut sets: Vec<&str> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(value) = &input.nickname {
            sets.push("nickname = ?");
            args.push(Box::new(value.clone()));
        }
        if let Some(value) = &input.email {
            sets.push("email = ?");
            args.push(Box::new(value.clone()));
        }
        if let Some(value) = &input.group_id {
            sets.push("group_id = ?");
            args.push(Box::new(value.clone()));
        }
        if let Some(value) = &input.auth_ref {
            sets.push("auth_ref = ?");
            args.push(Box::new(value.clone()));
        }
        if let Some(value) = &input.profile_ref {
            sets.push("profile_ref = ?");
            args.push(Box::new(value.clone()));
        }
        if let Some(value) = &input.auth_status {
            sets.push("auth_status = ?");
            args.push(Box::new(value.clone()));
        }
        if let Some(value) = input.checkin_enabled {
            sets.push("checkin_enabled = ?");
            args.push(Box::new(if value { 1i64 } else { 0i64 }));
        }
        if let Some(value) = &input.token_expires_at {
            sets.push("token_expires_at = ?");
            args.push(Box::new(value.clone()));
        }
        if let Some(value) = &input.auth_domain {
            sets.push("auth_domain = ?");
            args.push(Box::new(value.clone()));
        }
        if let Some(value) = &input.auth_source {
            sets.push("auth_source = ?");
            args.push(Box::new(value.clone()));
        }
        if let Some(value) = input.travel_enabled {
            sets.push("travel_enabled = ?");
            args.push(Box::new(i64::from(value)));
        }
        // 任何字段的写入都推进凭据版本：在途的旧快照就此作废（见 MIGRATION_V5）。
        sets.push("auth_epoch = auth_epoch + 1");
        sets.push("updated_at = ?");
        args.push(Box::new(now));

        args.push(Box::new(input.platform.clone()));
        args.push(Box::new(input.uid.clone()));

        let sql = format!(
            "UPDATE accounts SET {} WHERE platform = ? AND uid = ?",
            sets.join(", ")
        );
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        tx.execute(&sql, refs.as_slice())?;
        tx.commit()?;

        self.get_account(&id)?
            .ok_or_else(|| RpcError::new(crate::rpc::E_STORAGE, "写入后读不到账号"))
    }

    pub fn get_account(&self, id: &str) -> RpcResult<Option<AccountRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, platform, uid, nickname, email, group_id, auth_status,
                    checkin_enabled, last_checkin_at, last_result, updated_at,
                    token_expires_at, auth_domain, auth_source, travel_enabled, auth_epoch,
                    auth_mirror_auth_id, provider
             FROM accounts WHERE id = ?1",
        )?;
        let row = stmt.query_row(params![id], map_account).optional()?;
        Ok(row)
    }

    /// 按 platform + uid 查账号。登录去重与刷新都用它。
    pub fn find_account(&self, platform: &str, uid: &str) -> RpcResult<Option<AccountRecord>> {
        self.get_account(&account_id(platform, uid))
    }

    pub fn list_accounts(&self, filter: &AccountFilter) -> RpcResult<Vec<AccountRecord>> {
        let mut sql = String::from(
            "SELECT id, platform, uid, nickname, email, group_id, auth_status,
                    checkin_enabled, last_checkin_at, last_result, updated_at,
                    token_expires_at, auth_domain, auth_source, travel_enabled, auth_epoch,
                    auth_mirror_auth_id, provider
             FROM accounts WHERE 1 = 1",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(platform) = filter.platform.as_deref().filter(|p| !p.is_empty()) {
            sql.push_str(" AND platform = ?");
            args.push(Box::new(platform.to_string()));
        }
        if let Some(group) = filter.group_id.as_deref().filter(|g| !g.is_empty()) {
            sql.push_str(" AND group_id = ?");
            args.push(Box::new(group.to_string()));
        }
        if let Some(status) = filter.auth_status.as_deref().filter(|s| !s.is_empty()) {
            sql.push_str(" AND auth_status = ?");
            args.push(Box::new(status.to_string()));
        }
        if let Some(query) = filter.query.as_deref().filter(|q| !q.is_empty()) {
            sql.push_str(" AND (nickname LIKE ? OR uid LIKE ? OR IFNULL(email, '') LIKE ?)");
            let like = format!("%{query}%");
            args.push(Box::new(like.clone()));
            args.push(Box::new(like.clone()));
            args.push(Box::new(like));
        }

        sql.push_str(" ORDER BY platform ASC, updated_at DESC");
        sql.push_str(" LIMIT ? OFFSET ?");
        args.push(Box::new(filter.limit.unwrap_or(500).min(5000) as i64));
        args.push(Box::new(filter.offset.unwrap_or(0) as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), map_account)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn delete_account(&mut self, id: &str) -> RpcResult<bool> {
        let n = self
            .conn
            .execute("DELETE FROM accounts WHERE id = ?1", params![id])?;
        Ok(n > 0)
    }

    #[allow(dead_code)] // PR2 接线时使用
    pub fn set_auth_status(&self, id: &str, status: &str) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE accounts SET auth_status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn list_groups(&self) -> RpcResult<Vec<Value>> {
        let mut stmt = self.conn.prepare(
            "SELECT g.id, g.name, g.sort_order, COUNT(a.id)
             FROM account_groups g LEFT JOIN accounts a ON a.group_id = g.id
             GROUP BY g.id ORDER BY g.sort_order ASC, g.name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(json!({
                "id": row.get::<_, String>(0)?,
                "name": row.get::<_, String>(1)?,
                "sortOrder": row.get::<_, i64>(2)?,
                "accountCount": row.get::<_, i64>(3)?,
            }))
        })?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r?);
        }
        Ok(out)
    }

    pub fn upsert_group(&self, id: &str, name: &str, sort_order: i64) -> RpcResult<()> {
        self.conn.execute(
            "INSERT INTO account_groups (id, name, sort_order, created_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (id) DO UPDATE SET name = excluded.name, sort_order = excluded.sort_order",
            params![id, name, sort_order, now_rfc3339()],
        )?;
        Ok(())
    }
}

fn map_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<AccountRecord> {
    Ok(AccountRecord {
        id: row.get(0)?,
        platform: row.get(1)?,
        uid: row.get(2)?,
        nickname: row.get(3)?,
        email: row.get(4)?,
        group_id: row.get(5)?,
        auth_status: row.get(6)?,
        checkin_enabled: row.get::<_, i64>(7)? != 0,
        last_checkin_at: row.get(8)?,
        last_result: row.get(9)?,
        updated_at: row.get(10)?,
        token_expires_at: row.get(11)?,
        auth_domain: row.get(12)?,
        auth_source: row
            .get::<_, Option<String>>(13)?
            .unwrap_or_else(|| "own".into()),
        travel_enabled: row.get::<_, Option<i64>>(14)?.unwrap_or(0) != 0,
        auth_epoch: row.get::<_, Option<i64>>(15)?.unwrap_or(0),
        auth_mirror_auth_id: row.get(16)?,
        provider: row
            .get::<_, Option<String>>(17)?
            .unwrap_or_else(|| "workbuddy".into()),
    })
}

/// 账号主键 = sha256(platform + NUL + uid) 前 16 hex。
///
/// 确定性：同一 platform+uid 永远算出同一个 id，重复添加天然幂等；
/// 同时把 platform 掺进去，避免"UID 相同但不是同一账号"（PRD §3.2）。
pub fn account_id(platform: &str, uid: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(platform.as_bytes());
    hasher.update([0u8]);
    hasher.update(uid.as_bytes());
    let digest = hasher.finalize();
    digest.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("accounts.sqlite")).expect("open");
        (db, dir)
    }

    /// 造一个"旧版本"的库：逐条跑 v1..=target，再把 user_version 钉住。
    ///
    /// 不用 `Db::open`，因为它会把库一路迁到最新；这里要的是真实的旧库形态。
    fn legacy_db(dir: &std::path::Path, target: i64) -> std::path::PathBuf {
        let path = dir.join("accounts.sqlite");
        let conn = Connection::open(&path).expect("open legacy");
        for migration in MIGRATIONS {
            if migration.version > target {
                break;
            }
            for step in migration.steps {
                if let MigrationStep::Sql(sql) = step {
                    conn.execute_batch(sql).expect("legacy step");
                }
            }
        }
        conn.pragma_update(None, "user_version", target)
            .expect("pin version");
        path
    }

    fn seed_history(path: &std::path::Path) {
        let conn = Connection::open(path).expect("open");
        conn.execute_batch(
            "INSERT INTO accounts (id, platform, uid, nickname, auth_status, checkin_enabled,
                                   created_at, updated_at, auth_source)
             VALUES ('acc-own', 'workbuddy', 'uid-own', '自有', 'valid', 1, '2026-01-01T00:00:00Z',
                     '2026-01-01T00:00:00Z', 'own'),
                    ('acc-magpie', 'workbuddy', 'uid-magpie', '借用', 'expired', 0,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'magpie'),
                    ('acc-qoder', 'qoder_cn', 'uid-q', 'Qoder', 'valid', 0,
                     '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'own');
             INSERT INTO checkin_records (account_id, platform, local_date, status, checked_at)
             VALUES ('acc-own', 'workbuddy', '2026-11-10', 'claimed', '2026-11-10T01:00:00Z');
             INSERT INTO travel_states (account_id, updated_at) VALUES ('acc-own', '2026-11-10T01:00:00Z');
             INSERT INTO travel_events (account_id, kind, status, created_at)
             VALUES ('acc-own', 'depart', 'ok', '2026-11-10T01:00:00Z');",
        )
        .expect("seed history");
    }

    fn count(db: &Db, table: &str) -> i64 {
        db.conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .expect("count")
    }

    /// v5 → 最新：账号 ID、签到、旅行事件一行不改，授权行按确定公式生成，
    /// 镜像指向同一行（G2 / G3 / G5 / G6）。
    #[test]
    fn v5_upgrade_keeps_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = legacy_db(dir.path(), 5);
        seed_history(&path);

        let db = Db::open(&path).expect("open");
        assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);

        assert_eq!(count(&db, "accounts"), 3, "账号一行都不能少");
        assert_eq!(count(&db, "checkin_records"), 1);
        assert_eq!(count(&db, "travel_events"), 1);
        assert_eq!(count(&db, "travel_states"), 1);

        // 只为 WorkBuddy 建授权行，Qoder 不进这张表
        assert_eq!(count(&db, "account_authorizations"), 2);

        let own = db
            .find_authorization("acc-own", "own")
            .unwrap()
            .expect("own row");
        assert_eq!(
            own.id,
            crate::auth::ids::auth_id("acc-own", "own"),
            "G5：ID 必须可重算"
        );
        assert_eq!(own.key_migration_state, "pending");
        assert!(own.refresh_ref.is_some(), "own 必须保留 refresh 引用");
        assert_eq!(own.status, "valid");

        let magpie = db
            .find_authorization("acc-magpie", "magpie")
            .unwrap()
            .expect("magpie row");
        assert!(magpie.refresh_ref.is_none(), "magpie 不允许有 refresh 引用");
        assert_eq!(magpie.status, "expired");

        // 镜像：来自同一行、来源一致
        let account = db.get_account("acc-own").unwrap().expect("account");
        assert_eq!(account.auth_source, "own");
        assert_eq!(
            account.auth_mirror_auth_id.as_deref(),
            Some(own.id.as_str())
        );
        let mirrored_ref: Option<String> = db
            .conn
            .query_row(
                "SELECT auth_ref FROM accounts WHERE id = 'acc-own'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            mirrored_ref.as_deref(),
            Some(own.access_ref.as_str()),
            "镜像必须指向同一来源的 access"
        );

        let qoder = db.get_account("acc-qoder").unwrap().expect("qoder");
        assert!(qoder.auth_mirror_auth_id.is_none(), "Qoder 不参与多授权");
    }

    /// v4 → 最新：跨多级升级同样不丢数据。
    #[test]
    fn v4_upgrade_lands_on_latest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = legacy_db(dir.path(), 4);
        seed_history(&path);
        let db = Db::open(&path).expect("open");
        assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);
        assert_eq!(count(&db, "accounts"), 3);
        assert_eq!(count(&db, "account_authorizations"), 2);
    }

    /// 空库直接建到最新版本：不产生任何授权行，也不报错。
    #[test]
    fn empty_database_lands_on_latest_without_rows() {
        let (db, _dir) = temp_db();
        assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);
        assert_eq!(count(&db, "account_authorizations"), 0);
    }

    /// 反复打开（重复启动）不产生重复授权行（G3）。
    #[test]
    fn reopening_does_not_duplicate_authorizations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = legacy_db(dir.path(), 5);
        seed_history(&path);
        let mut first = Db::open(&path).expect("first");
        assert_eq!(count(&first, "account_authorizations"), 2);
        first.migrate().unwrap();
        drop(first);
        let second = Db::open(&path).expect("second");
        assert_eq!(count(&second, "account_authorizations"), 2);
        assert_eq!(second.schema_version().unwrap(), LATEST_VERSION);
    }

    /// 删账号连带删授权行（外键级联），不留孤儿。
    #[test]
    fn deleting_an_account_cascades_authorizations() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = legacy_db(dir.path(), 5);
        seed_history(&path);
        let mut db = Db::open(&path).expect("open");
        assert!(db.delete_account("acc-own").unwrap());
        assert!(db.find_authorization("acc-own", "own").unwrap().is_none());
        assert_eq!(count(&db, "account_authorizations"), 1);
    }

    #[test]
    fn migration_is_idempotent() {
        let (mut db, _dir) = temp_db();
        assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);
        assert_eq!(db.migrate().unwrap(), LATEST_VERSION);
        assert!(db.integrity_ok());
    }

    #[test]
    fn account_id_is_deterministic_and_platform_scoped() {
        assert_eq!(account_id("workbuddy", "u1"), account_id("workbuddy", "u1"));
        assert_ne!(account_id("workbuddy", "u1"), account_id("qoder_cn", "u1"));
    }

    #[test]
    fn same_uid_on_two_platforms_are_two_accounts() {
        let (mut db, _dir) = temp_db();
        let wb = AccountInput {
            platform: "workbuddy".into(),
            uid: "42".into(),
            nickname: Some("WB".into()),
            ..Default::default()
        };
        let qd = AccountInput {
            platform: "qoder_cn".into(),
            uid: "42".into(),
            nickname: Some("QD".into()),
            ..Default::default()
        };
        let a = db.upsert_account(&wb).unwrap();
        let b = db.upsert_account(&qd).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(
            db.list_accounts(&AccountFilter::default()).unwrap().len(),
            2
        );
    }

    #[test]
    fn upsert_updates_in_place() {
        let (mut db, _dir) = temp_db();
        let mut input = AccountInput {
            platform: "workbuddy".into(),
            uid: "u1".into(),
            nickname: Some("旧名字".into()),
            ..Default::default()
        };
        let first = db.upsert_account(&input).unwrap();
        input.nickname = Some("新名字".into());
        let second = db.upsert_account(&input).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.nickname, "新名字");
        assert_eq!(
            db.list_accounts(&AccountFilter::default()).unwrap().len(),
            1
        );
    }

    /// 回归：只改备注时，签到开关与凭据引用必须保持原值。
    /// 早前用 `DO UPDATE SET x = excluded.x`，没传的字段会被重置。
    #[test]
    fn patch_semantics_preserve_unspecified_fields() {
        let (mut db, _dir) = temp_db();
        let created = db
            .upsert_account(&AccountInput {
                platform: "workbuddy".into(),
                uid: "u-patch".into(),
                nickname: Some("原名".into()),
                auth_ref: Some("keychain://wb/u-patch".into()),
                profile_ref: Some("/tmp/profile-a".into()),
                auth_status: Some("valid".into()),
                checkin_enabled: Some(true),
                ..Default::default()
            })
            .unwrap();
        assert!(created.checkin_enabled);
        assert_eq!(created.auth_status, "valid");

        // 只改备注
        let patched = db
            .upsert_account(&AccountInput {
                platform: "workbuddy".into(),
                uid: "u-patch".into(),
                nickname: Some("新备注".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(patched.nickname, "新备注");
        assert!(patched.checkin_enabled, "签到开关不能因为没传就被重置");
        assert_eq!(patched.auth_status, "valid", "认证状态不能因为没传就被重置");

        // 显式关掉签到必须生效
        let disabled = db
            .upsert_account(&AccountInput {
                platform: "workbuddy".into(),
                uid: "u-patch".into(),
                checkin_enabled: Some(false),
                ..Default::default()
            })
            .unwrap();
        assert!(!disabled.checkin_enabled);
        assert_eq!(disabled.nickname, "新备注", "没传备注时不能被清空");
    }

    /// 回归：外键写失败时，第一步的 INSERT 不能留在库里。
    /// 早前 INSERT 与 UPDATE 不在同一个事务，调用方收到错误但账号已经被创建。
    #[test]
    fn failed_write_rolls_back_the_insert() {
        let (mut db, _dir) = temp_db();
        let result = db.upsert_account(&AccountInput {
            platform: "workbuddy".into(),
            uid: "u-fk".into(),
            nickname: Some("半条账号".into()),
            group_id: Some("no-such-group".into()),
            ..Default::default()
        });
        assert!(result.is_err(), "不存在的 group_id 必须报错");
        assert_eq!(
            db.list_accounts(&AccountFilter::default()).unwrap().len(),
            0,
            "写入失败不能留下半条账号"
        );
        assert!(
            db.get_account(&account_id("workbuddy", "u-fk"))
                .unwrap()
                .is_none(),
            "失败的写入不能按 id 查到"
        );
    }

    #[test]
    fn rejects_unknown_platform() {
        let (mut db, _dir) = temp_db();
        let input = AccountInput {
            platform: "codex".into(),
            uid: "u1".into(),
            ..Default::default()
        };
        assert!(db.upsert_account(&input).is_err());
    }

    #[test]
    fn filter_by_platform_and_query() {
        let (mut db, _dir) = temp_db();
        for (platform, uid, nickname) in [
            ("workbuddy", "a", "alpha"),
            ("workbuddy", "b", "beta"),
            ("qoder_cn", "c", "gamma"),
        ] {
            db.upsert_account(&AccountInput {
                platform: platform.into(),
                uid: uid.into(),
                nickname: Some(nickname.into()),
                ..Default::default()
            })
            .unwrap();
        }
        let wb = AccountFilter {
            platform: Some("workbuddy".into()),
            ..Default::default()
        };
        assert_eq!(db.list_accounts(&wb).unwrap().len(), 2);
        let hit = AccountFilter {
            query: Some("bet".into()),
            ..Default::default()
        };
        assert_eq!(db.list_accounts(&hit).unwrap().len(), 1);
    }

    #[test]
    fn delete_removes_row() {
        let (mut db, _dir) = temp_db();
        let acc = db
            .upsert_account(&AccountInput {
                platform: "qoder_cn".into(),
                uid: "z".into(),
                ..Default::default()
            })
            .unwrap();
        assert!(db.delete_account(&acc.id).unwrap());
        assert!(!db.delete_account(&acc.id).unwrap());
    }
}
