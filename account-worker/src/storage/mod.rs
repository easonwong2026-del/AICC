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
pub mod qoder_credits;
pub mod qoder_profiles;
pub mod qoder_switch;
pub mod quota;
pub mod reward_ops;
pub mod rotation;
pub mod travel;
pub mod wb_switch;

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

/// v14：额度快照缓存（M4 额度看板）。
///
/// 只建一张新表，不动任何已有表：额度是**观测数据**，不是业务状态，
/// 因此不写进 `accounts`，也不改 `credit_snapshots`（那张是上游 wb-switch 的账，
/// 由 vendored 模块自己维护文件版快照）。
///
/// `payload` 存整行 JSON 而不是拆列：四平台的计量单位不同（credits / tokens /
/// unknown），拆成 `total/used/remaining` 三列会诱导"跨单位相加"这种错误。
/// 时间范围查询走 `captured_at` 索引。
///
/// **`account_id` 刻意没有外键**：四平台里 WorkBuddy 账号在 `accounts`，
/// Qoder 服务账号在 `qoder_accounts`，一张缓存表没法同时对两张表建外键。
/// 代价是删除账号时要显式清快照（`delete_account` / `delete_qoder_account`
/// 已各加一条 DELETE），另外 90 天裁剪兜底。
const MIGRATION_V14_SQL: &str = "\
CREATE TABLE IF NOT EXISTS quota_snapshots (\
    id          TEXT PRIMARY KEY,\
    account_id  TEXT NOT NULL,\
    provider    TEXT NOT NULL,\
    captured_at TEXT NOT NULL,\
    status      TEXT NOT NULL,\
    payload     TEXT NOT NULL\
);\
CREATE INDEX IF NOT EXISTS idx_quota_snapshots_account ON quota_snapshots (account_id, captured_at);";

/// v22：CRED-4 迁移记录。
///
/// 它回答一个 SQL 表本身回答不了的问题：**这一行是我迁过来的，还是它本来就是
/// JSON？** 没有这张表，"回滚"只能按"当前是 localjson"筛选，会把 CRED-3a/3b
/// 正常登录得到的有效授权一起切回可能已经失效的旧钥匙串条目。
///
/// 记录里**只有引用与指纹**，没有令牌：
///
/// | 列 | 用途 |
/// | --- | --- |
/// | `original_ref` | 迁移前的钥匙串引用，回滚要回到它 |
/// | `original_fingerprint` | 迁移时读到的 access 指纹：回滚前核对"还是那一份" |
/// | `original_refresh_fingerprint` | 同上，refresh 槽 |
/// | `migrated_ref` | 迁移后应该仍在用的本地文件 |
/// | `migrated_version` | 迁移后的行版本：行被重新授权过就不能回滚 |
const MIGRATION_V22_SQL: &str = "\
CREATE TABLE IF NOT EXISTS credential_migrations (\
    id                         TEXT PRIMARY KEY,\
    domain                     TEXT NOT NULL,\
    account_id                 TEXT NOT NULL,\
    source                     TEXT NOT NULL,\
    original_ref               TEXT NOT NULL,\
    original_fingerprint       TEXT,\
    original_refresh_fingerprint TEXT,\
    migrated_ref               TEXT NOT NULL,\
    migrated_version           INTEGER NOT NULL,\
    created_at                 TEXT NOT NULL\
);\
CREATE INDEX IF NOT EXISTS idx_cred_migrations_account \
    ON credential_migrations (domain, account_id);";

/// v24：奖励操作账本（PR #95，复审 P0-1/P0-2 后收紧）。
///
/// 兑换 / 抽奖 / 补签都会消耗账号的一次机会，所以"这次发没发过"必须活过崩溃与并发。
/// 三件事都由这一张表兜住：
///
/// | 约束 | 作用 |
/// | --- | --- |
/// | `ux_reward_ops_open` | 同一 `(账号, 动作, 目标)` 同时只允许一条未决记录 |
/// | `ux_reward_ops_token` | 同一个 `client_token` 不能落在两条记录上 |
/// | `client_token` 列 | 重启后回查/重发都用**同一个**幂等键 |
///
/// 阶段里 `ready` 是"服务端明确没执行"（404 / tier 参数不认识），允许用同一 token
/// 再发一次；`unknown` / `frozen` 仍然占着目标 —— 这两者的区别是"再查一次"和
/// "等人工"，共同点是**都不许生成新 token**。
///
/// 只建表、不改任何已有表，也不动 `growth_runs`（那张记的是"接单/领奖"的最终结果，
/// 这张记的是"每一次可能消耗机会的尝试"，两者互补而不是替代）。
const MIGRATION_V24_SQL: &str = "\
CREATE TABLE IF NOT EXISTS reward_operations (\
    id           TEXT PRIMARY KEY,\
    account_id   TEXT NOT NULL,\
    action       TEXT NOT NULL CHECK (action IN ('redeem','lottery_draw','makeup_card')),\
    target       TEXT NOT NULL DEFAULT '',\
    client_token TEXT,\
    phase        TEXT NOT NULL CHECK (phase IN (\
                     'pending','ready','sent','confirmed','rejected','failed','unknown','frozen')),\
    attempt      INTEGER NOT NULL DEFAULT 0,\
    outcome      TEXT,\
    grant_json   TEXT,\
    server_state TEXT,\
    detail       TEXT,\
    created_at   TEXT NOT NULL,\
    updated_at   TEXT NOT NULL,\
    verified_at  TEXT\
);\
CREATE UNIQUE INDEX IF NOT EXISTS ux_reward_ops_open \
    ON reward_operations (account_id, action, target) \
    WHERE phase IN ('pending','ready','sent','unknown','frozen');\
CREATE UNIQUE INDEX IF NOT EXISTS ux_reward_ops_token \
    ON reward_operations (account_id, action, client_token) \
    WHERE client_token IS NOT NULL;\
CREATE INDEX IF NOT EXISTS idx_reward_ops_account \
    ON reward_operations (account_id, created_at);\
CREATE INDEX IF NOT EXISTS idx_reward_ops_phase \
    ON reward_operations (phase);";

/// v25：WorkBuddy **客户端**账号切换日志（不是 Qoder Profile 那张表）。
///
/// 切换要写客户端的 `workbuddy-desktop.info` 并重启它，中途失败必须能把文件
/// 逐字节还原。所以每次切换先存一份快照，再在库里记一条 journal：
///
/// - `open_slot`：**同一时刻只允许一条未完成切换**。完成 / 失败 / 回滚后置 NULL，
///   SQLite 的唯一索引把 NULL 当互不相等，正好表达"历史可以有很多条，未完成只能一条"。
/// - `snapshot_*`：快照路径与 sha256。恢复前必须核对 sha，快照被改过就拒绝恢复，
///   不能用一份不知道是什么的文件覆盖客户端登录态。
/// - `activity_before` / `activity_after`：切换前后目标账号的服务端活跃状态
///   （只读快照，JSON）。**只作证据**，不参与任何判定。
const MIGRATION_V25_SQL: &str = "\
CREATE TABLE IF NOT EXISTS workbuddy_switch_journal (\
    id                TEXT PRIMARY KEY,\
    target_account_id TEXT NOT NULL,\
    target_uid        TEXT NOT NULL,\
    previous_uid      TEXT,\
    previous_running  INTEGER NOT NULL DEFAULT 1 CHECK (previous_running IN (0, 1)),\
    previous_marker   INTEGER NOT NULL DEFAULT 0 CHECK (previous_marker IN (0, 1)),\
    phase             TEXT NOT NULL CHECK (phase IN (\
                          'prepared','written','launched','verified','unconfirmed',                          'failed','restored')),\
    open_slot         INTEGER,\
    snapshot_path     TEXT NOT NULL,\
    snapshot_sha256   TEXT NOT NULL,\
    token_form        TEXT CHECK (token_form IN ('plaintext','envelope')),\
    detail            TEXT,\
    activity_before   TEXT,\
    activity_after    TEXT,\
    created_at        TEXT NOT NULL,\
    updated_at        TEXT NOT NULL\
);\
CREATE UNIQUE INDEX IF NOT EXISTS ux_wb_switch_open \
    ON workbuddy_switch_journal (open_slot);\
CREATE INDEX IF NOT EXISTS idx_wb_switch_created \
    ON workbuddy_switch_journal (created_at DESC);";

/// v27：夜间账号轮换。
///
/// 三张表彼此独立，**没有外键**：几轮记录被清理时不该连带删掉另一张表里的历史。
/// 关键约束只有两条，且都放在数据库里：
///
/// - `UNIQUE(plan_id, plan_date)` —— 同一天最多一轮。重复心跳、两个 worker、
///   AICC 重启都不可能凭空多跑一轮；插入冲突就是"今天已经跑过"。
/// - `UNIQUE(run_id, seq)` —— 一轮里每个位置只有一个步骤。重复 tick 撞唯一键，
///   调用方按"已存在"处理。
///
/// `origin_*` 五列是这一轮**最初**的登录态（不是"上一站"）。它必须能独立于
/// `workbuddy_switch_journal` 存在：中途切了几次、那些 journal 怎么结算，
/// 都不影响"回到 A"。
const MIGRATION_V27_SQL: &str = "\
CREATE TABLE IF NOT EXISTS workbuddy_rotation_plan (\
    id             TEXT PRIMARY KEY,\
    enabled        INTEGER NOT NULL DEFAULT 0 CHECK (enabled IN (0, 1)),\
    timezone       TEXT NOT NULL DEFAULT 'Asia/Shanghai',\
    weekdays       TEXT NOT NULL DEFAULT '1,2,3,4,5,6,7',\
    start_minutes  INTEGER NOT NULL DEFAULT 1350 CHECK (start_minutes BETWEEN 0 AND 1439),\
    end_minutes    INTEGER NOT NULL DEFAULT 420 CHECK (end_minutes BETWEEN 0 AND 1439),\
    account_ids    TEXT NOT NULL DEFAULT '[]',\
    dwell_seconds  INTEGER NOT NULL DEFAULT 3600,\
    created_at     TEXT NOT NULL,\
    updated_at     TEXT NOT NULL\
);\
CREATE TABLE IF NOT EXISTS workbuddy_rotation_run (\
    id                     TEXT PRIMARY KEY,\
    plan_id                TEXT NOT NULL,\
    plan_date              TEXT NOT NULL,\
    timezone               TEXT NOT NULL,\
    window_start           TEXT NOT NULL,\
    window_end             TEXT NOT NULL,\
    origin_account_id      TEXT,\
    origin_uid             TEXT,\
    origin_snapshot_path   TEXT,\
    origin_snapshot_sha256 TEXT,\
    origin_marker          INTEGER NOT NULL DEFAULT 0 CHECK (origin_marker IN (0, 1)),\
    origin_running         INTEGER NOT NULL DEFAULT 0 CHECK (origin_running IN (0, 1)),\
    state                  TEXT NOT NULL CHECK (state IN (\
                             'scheduled','prechecking','switching','dwelling','restoring',\
                             'completed','skipped','failed','recovery_required')),\
    cursor                 INTEGER NOT NULL DEFAULT 0,\
    current_journal_id     TEXT,\
    last_target_account_id TEXT,\
    last_target_uid        TEXT,\
    restore_attempts       INTEGER NOT NULL DEFAULT 0,\
    restore_result         TEXT,\
    detail                 TEXT,\
    created_at             TEXT NOT NULL,\
    updated_at             TEXT NOT NULL\
);\
CREATE UNIQUE INDEX IF NOT EXISTS ux_rotation_run_per_day \
    ON workbuddy_rotation_run (plan_id, plan_date);\
CREATE INDEX IF NOT EXISTS idx_rotation_run_state \
    ON workbuddy_rotation_run (state, window_end);\
CREATE TABLE IF NOT EXISTS workbuddy_rotation_step (\
    id              TEXT PRIMARY KEY,\
    run_id          TEXT NOT NULL,\
    seq             INTEGER NOT NULL,\
    account_id      TEXT NOT NULL,\
    uid             TEXT NOT NULL,\
    mode            TEXT,\
    planned_at      TEXT,\
    started_at      TEXT,\
    ended_at        TEXT,\
    status          TEXT NOT NULL,\
    journal_id      TEXT,\
    skip_reason     TEXT,\
    detail          TEXT,\
    auth_precheck   TEXT,\
    activity_before TEXT,\
    activity_after  TEXT,\
    created_at      TEXT NOT NULL,\
    updated_at      TEXT NOT NULL\
);\
CREATE UNIQUE INDEX IF NOT EXISTS ux_rotation_step_seq \
    ON workbuddy_rotation_step (run_id, seq);";

/// 一次迁移里的一步：纯 SQL，或者需要 Rust 参与的数据迁移。
///
/// v16 起的 Qoder 迁移都是 Rust 步骤：它们要先检测现状再动手，重复执行必须是
/// 空操作——人工把 `user_version` 回拨、或者迁移事务已提交但版本号没写下去时，
/// 再来一次不能炸在 `duplicate column name` 上。具体步骤见各版本自己的 `migrate_*`。
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
    // v14：额度快照缓存（M4）。只建新表。
    Migration {
        version: 14,
        steps: &[MigrationStep::Sql(MIGRATION_V14_SQL)],
    },
    // v15：把 v14 原来那条只指向 `accounts` 的外键去掉——一张缓存表要同时装
    // WorkBuddy 与 Qoder 服务账号，外键在这里是错的。v14 也曾带过这条外键，
    // 所以这一步同时负责**修复已经跑过旧 v14 的库**（新建库上是空操作）。
    Migration {
        version: 15,
        steps: &[MigrationStep::Rust(quota::rebuild_v15)],
    },
    // v16：Qoder PAT 来源 + 账号凭据版本号。重建一张子表、追加一列。
    Migration {
        version: 16,
        steps: &[MigrationStep::Rust(qoder_accounts::rebuild_v16)],
    },
    // v17：Qoder 每日 Credits 领取记录 + 每账号开关。只追加列、只建新表。
    Migration {
        version: 17,
        steps: &[MigrationStep::Rust(qoder_credits::migrate_v17)],
    },
    // v18：领取账加活动周期两列，让去重键包含周期（服务端会复用 campaignId）。
    Migration {
        version: 18,
        steps: &[MigrationStep::Rust(qoder_credits::migrate_v18)],
    },
    // v19：账号加生命周期标识 `generation`。删号后用同一个 UID 重新登录会复用
    // 账号 id，`auth_epoch` 也回到 0；只有 generation 能区分"这两次不是同一个账号"。
    Migration {
        version: 19,
        steps: &[MigrationStep::Rust(qoder_accounts::migrate_v19)],
    },
    // v20：凭据存储种类（PR-CRED-2）。只加列 + 按引用前缀回填，**不改任何 access_ref**；
    // 旧 Magpie 授权改成动态引用要走显式的重新关联。
    Migration {
        version: 20,
        steps: &[MigrationStep::Rust(authz::migrate_v20)],
    },
    // v21：授权行加**授权版本号**（PR-CRED-3b 复审 P1-C）。只加一列。
    //
    // 之前用 `fingerprint`（令牌的 SHA-256 前 8 位）代替版本：重新保存**同一个**
    // PAT 或同一串 access 时摘要不变，于是"这一行还是不是我删掉的那一份"判不出来，
    // 删号失败后的收尾会把别人刚保存成功的新授权标成 `missing`。
    // 计数器每次提交必然前进，才是"这一份凭据的第几次授权"。
    Migration {
        version: 21,
        steps: &[MigrationStep::Rust(qoder_accounts::migrate_v21)],
    },
    // v22：CRED-4 的迁移记录表（只建新表，无数据迁移）。
    Migration {
        version: 22,
        steps: &[MigrationStep::Sql(MIGRATION_V22_SQL)],
    },
    // v23：回滚后的副本清理要留痕（CRED-4 复审 P1-E）。
    //
    // 之前回滚是"先删记录、再尽力删文件"：文件删失败只剩一行日志，记录已经没了，
    // 谁也不知道哪个副本该清、该不该重试。加一列标记，删文件的成败就有地方记。
    Migration {
        version: 23,
        steps: &[MigrationStep::Rust(authz::migrate_v23)],
    },
    // v24：奖励操作账本（PR #95）。只建新表 + 索引，不碰 accounts / growth_runs。
    //
    // 部分唯一索引（`WHERE phase IN (...)`）是这张表的核心：它让"同一账号同一目标
    // 同时只能有一条未决记录"变成数据库约束，而不是"代码记得先查一下"。
    Migration {
        version: 24,
        steps: &[MigrationStep::Sql(MIGRATION_V24_SQL)],
    },
    // v25：WorkBuddy 客户端账号切换日志。只建新表 + 索引，不碰 accounts。
    Migration {
        version: 25,
        steps: &[MigrationStep::Sql(MIGRATION_V25_SQL)],
    },
    // v26：临时授权（Magpie access-only）的切换记录。只加列 + 一个索引。
    //
    // 「切换正在执行」由 `open_slot` 管，「临时授权还没还原」由 `restore_state` 管。
    // 两者分开，成功后的临时登录才不会一直占着并发槽位。
    Migration {
        version: 26,
        steps: &[MigrationStep::Rust(wb_switch::migrate_v26)],
    },
    // v27：夜间账号轮换（计划 / 轮次 / 步骤）。只建新表 + 两个唯一索引。
    //
    // 「一轮最初登录的是谁」必须有自己的列：`switch_journal` 的语义是"回到上一站"，
    // A→B→C→D 之后它只能带回 C。夜间计划要回 A，所以 origin 快照与 journal 分开存。
    Migration {
        version: 27,
        steps: &[MigrationStep::Sql(MIGRATION_V27_SQL)],
    },
    // v28：轮次记住"谁发起的"。关闭自动计划只该收尾自动轮次，
    // 不能把用户手动跑的那一轮也当成遗留清掉。
    Migration {
        version: 28,
        steps: &[MigrationStep::Rust(rotation::migrate_v28)],
    },
    // v29：显式停止请求要落库，否则 `stop()` 遇到忙就丢了。
    Migration {
        version: 29,
        steps: &[MigrationStep::Rust(rotation::migrate_v29)],
    },
];

pub const LATEST_VERSION: i64 = 29;

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

impl AccountRecord {
    /// 这个账号能不能走 **WorkBuddy 国内版**的接口。
    ///
    /// 签到、成长、旅行、授权检查全部只实现了国内版协议（端点写死在
    /// `copilot.tencent.com` / `www.codebuddy.cn`）。WorkBuddy AI 是**另一个站点**，
    /// 用国内版接口去请求它只会得到 401/404，更糟的是可能把国际版令牌发到国内域。
    ///
    /// 所以这些入口一律先问这个方法：**只看 `platform` 不够**，必须看 `provider`。
    pub fn supports_workbuddy_cn_apis(&self) -> bool {
        self.platform == "workbuddy" && self.provider == "workbuddy"
    }
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

    /// 把当前库完整复制到 `path`（`VACUUM INTO`）。
    ///
    /// 用它而不是直接拷 `accounts.sqlite`：WAL 模式下还有内容没 checkpoint，
    /// 裸拷贝会漏掉最近的事务。`VACUUM INTO` 在线进行、产出单文件、不留 WAL。
    /// 目标必须不存在（SQLite 的硬要求，也顺便防覆盖旧备份）。
    pub fn backup_to(&self, path: &Path) -> RpcResult<()> {
        if path.exists() {
            return Err(RpcError::invalid_params(format!(
                "备份目标已存在：{}",
                path.display()
            )));
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        self.conn
            .execute("VACUUM INTO ?1", params![path.to_string_lossy()])?;
        // 备份里有授权引用与账号信息，按凭据文件的规矩收权限。
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// 只读检查一个库文件（备份快照）：能打开、`integrity_check` 通过。
    ///
    /// 备份之后一定要跑一次：`VACUUM INTO` 失败或磁盘满时可能留下一个**能打开
    /// 但内容不全**的文件，"有备份"和"备份可用"是两件事。
    pub fn snapshot_is_sound(path: &Path) -> RpcResult<bool> {
        let conn = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let result: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        Ok(result == "ok")
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
            "qoder_credit_runs",
            "quota_snapshots",
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
        // 没传 provider 时**继承已有账号的 provider**：改备注这类补丁不该把
        // 国际版账号重新判成国内版，更不该因此报 uid_conflict。
        let existing_provider: Option<String> = self
            .conn
            .query_row(
                "SELECT provider FROM accounts WHERE platform = ?1 AND uid = ?2",
                params![input.platform, input.uid],
                |row| row.get(0),
            )
            .optional()?;
        let provider: &str = match input.provider.as_deref() {
            Some(explicit) => explicit,
            None => existing_provider.as_deref().unwrap_or("workbuddy"),
        };
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
        if let Some(existing) = existing_provider.as_deref() {
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
        // 三张表都没有指向 accounts 的外键（额度快照要同时装 Qoder 服务账号，
        // 奖励账本按账号 id 记账），所以这里必须显式清，而且要在**同一个事务**里：
        // 删了账号行、留下未决的奖励记录，下次用同一个 UID 重新导入会复用同一个
        // 账号 id（id 是 uid 的确定性哈希），旧记录立刻变成"这个账号还有一个没查完的
        // 抽奖"，新账号一进界面就被卡住。
        //
        // 为什么不用 `auth_epoch` 做隔离：它每次重新授权都会 +1，而**重新授权不该
        // 释放一个可能已经发出去的写操作** —— 那正好是重复消耗的入口。
        let tx = self.conn.transaction()?;
        tx.execute(
            "DELETE FROM quota_snapshots WHERE account_id = ?1",
            params![id],
        )?;
        tx.execute(
            "DELETE FROM reward_operations WHERE account_id = ?1",
            params![id],
        )?;
        let n = tx.execute("DELETE FROM accounts WHERE id = ?1", params![id])?;
        tx.commit()?;
        Ok(n > 0)
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
    fn a_patch_without_provider_inherits_the_existing_one() {
        let (mut db, _dir) = temp_db();
        let ai = db
            .upsert_account(&AccountInput {
                platform: "workbuddy".into(),
                uid: "u-ai".into(),
                provider: Some("workbuddy_ai".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(ai.provider, "workbuddy_ai");

        // 改备注时没传 provider：必须继承，不能降级成国内版，也不能报冲突。
        let patched = db
            .upsert_account(&AccountInput {
                platform: "workbuddy".into(),
                uid: "u-ai".into(),
                nickname: Some("新备注".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(patched.nickname, "新备注");
        assert_eq!(patched.provider, "workbuddy_ai");
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

    /// v19：给已有账号补 `generation`。**每行各自不同**，重复执行不改写。
    ///
    /// 旧库的真实形状是"没有这一列"，所以这里先把列删掉再回拨版本号，
    /// 而不是只把值清空——那会把"加列"这一步测不到。
    #[test]
    fn v19_backfills_a_generation_for_existing_accounts() {
        let (mut db, _dir) = temp_db();
        let a = db
            .upsert_qoder_account("qoder_cn", "u-a", Some("甲"), None)
            .unwrap();
        let b = db
            .upsert_qoder_account("qoder_global", "u-b", Some("乙"), None)
            .unwrap();
        assert_eq!(a.generation.len(), 32, "新建账号就该带着生命周期");

        db.exec_raw("ALTER TABLE qoder_accounts DROP COLUMN generation;")
            .expect("退回 v18 的形状");
        db.conn.pragma_update(None, "user_version", 18).unwrap();

        assert_eq!(db.migrate().unwrap(), LATEST_VERSION);
        let ga = db.get_qoder_account(&a.id).unwrap().unwrap().generation;
        let gb = db.get_qoder_account(&b.id).unwrap().unwrap().generation;
        assert_eq!(ga.len(), 32);
        assert_ne!(ga, gb, "每行必须各自不同，不能都写成同一个值");

        // 回拨再跑一次：空操作，不能把已经有的值改掉。
        db.conn.pragma_update(None, "user_version", 18).unwrap();
        db.migrate().unwrap();
        assert_eq!(db.get_qoder_account(&a.id).unwrap().unwrap().generation, ga);
    }

    /// v22：迁移记录表。只建表，重复执行是空操作。
    #[test]
    fn v22_adds_the_credential_migration_table_idempotently() {
        let (mut db, _dir) = temp_db();
        let record = crate::storage::authz::CredentialMigrationRecord {
            id: "workbuddy:acc:own".to_string(),
            domain: "workbuddy".to_string(),
            account_id: "acc".to_string(),
            source: "own".to_string(),
            original_ref: "keychain:workbuddy/acc/own/access".to_string(),
            original_fingerprint: Some("fp".to_string()),
            original_refresh_fingerprint: None,
            migrated_ref: "localjson:v1:abc".to_string(),
            migrated_version: 2,
            created_at: "2026-11-15T00:00:00Z".to_string(),
            cleanup_pending: false,
        };
        db.upsert_credential_migration(&record).unwrap();
        assert_eq!(
            db.find_credential_migration(&record.id).unwrap(),
            Some(record.clone())
        );

        // 退回 v21 的形状：没有这张表。
        db.exec_raw("DROP TABLE credential_migrations;").unwrap();
        db.conn.pragma_update(None, "user_version", 21).unwrap();
        assert_eq!(db.migrate().unwrap(), LATEST_VERSION);
        // 表回来了，且是空的（旧库本来就没有迁移记录）。
        assert!(db.list_credential_migrations().unwrap().is_empty());
        db.upsert_credential_migration(&record).unwrap();
        assert_eq!(db.list_credential_migrations().unwrap().len(), 1);

        // 回拨重跑：空操作，不报错、不清数据。
        db.conn.pragma_update(None, "user_version", 21).unwrap();
        db.migrate().unwrap();
        assert_eq!(db.list_credential_migrations().unwrap().len(), 1);
    }

    /// v23：`cleanup_pending`。老记录默认 0（没有待清理副本），新值能读回，
    /// 回拨版本号再跑一次是空操作。
    #[test]
    fn v23_adds_cleanup_pending_defaulting_to_zero() {
        let (mut db, _dir) = temp_db();
        let mut record = crate::storage::authz::CredentialMigrationRecord {
            id: "workbuddy:acc:own".to_string(),
            domain: "workbuddy".to_string(),
            account_id: "acc".to_string(),
            source: "own".to_string(),
            original_ref: "keychain:workbuddy/acc/own/access".to_string(),
            original_fingerprint: Some("fp".to_string()),
            original_refresh_fingerprint: None,
            migrated_ref: "localjson:v1:abc".to_string(),
            migrated_version: 2,
            created_at: "2026-11-15T00:00:00Z".to_string(),
            cleanup_pending: false,
        };
        db.upsert_credential_migration(&record).unwrap();
        assert!(
            !db.find_credential_migration(&record.id)
                .unwrap()
                .unwrap()
                .cleanup_pending
        );

        record.cleanup_pending = true;
        db.upsert_credential_migration(&record).unwrap();
        assert_eq!(db.pending_cleanup_migrations().unwrap().len(), 1);

        // 回拨到 v22 再跑：加列幂等，数据不动。
        db.conn.pragma_update(None, "user_version", 22).unwrap();
        assert_eq!(db.migrate().unwrap(), LATEST_VERSION);
        assert!(
            db.find_credential_migration(&record.id)
                .unwrap()
                .unwrap()
                .cleanup_pending
        );
    }

    /// v24：奖励操作账本。只建表 + 索引，重复执行是空操作。
    ///
    /// 老库的真实形状是"没有这张表"，所以先 DROP 再把版本号回拨 —— 只清数据
    /// 会把"建表"这一步测不到。回拨后必须还能补回来，且不产生半张表。
    #[test]
    fn v24_adds_the_reward_ledger_idempotently() {
        let (mut db, _dir) = temp_db();
        let opened = db
            .open_reward_op(&crate::storage::reward_ops::NewRewardOp {
                account_id: "acc",
                action: "lottery_draw",
                target: "",
                client_token: Some("u-1"),
            })
            .unwrap();
        assert!(opened.was_created());
        let id = opened.op().id.clone();

        // 退回 v23 的形状：没有这张表。
        db.exec_raw("DROP TABLE reward_operations;").unwrap();
        db.conn.pragma_update(None, "user_version", 23).unwrap();
        assert_eq!(db.migrate().unwrap(), LATEST_VERSION);
        assert!(
            db.reward_ops("acc", 10).unwrap().is_empty(),
            "旧库本来就没有账本，迁移后必须是空表"
        );

        let again = db
            .open_reward_op(&crate::storage::reward_ops::NewRewardOp {
                account_id: "acc",
                action: "lottery_draw",
                target: "",
                client_token: Some("u-2"),
            })
            .unwrap();
        assert!(again.was_created());
        assert_ne!(again.op().id, id);

        // 回拨重跑：空操作，不报错、不清数据。
        db.conn.pragma_update(None, "user_version", 23).unwrap();
        db.migrate().unwrap();
        assert_eq!(db.reward_ops("acc", 10).unwrap().len(), 1);
    }

    /// v21：给授权行补 `auth_version`，老行从 0 起，重复执行不改写。    ///
    /// 旧库的真实形状是"没有这一列"，所以先把列删掉再回拨版本号 —— 只清空值
    /// 会把"加列"这一步测不到。
    #[test]
    fn v21_adds_an_authorization_version_without_touching_existing_rows() {
        let (mut db, _dir) = temp_db();
        let acc = db
            .upsert_qoder_account("qoder_cn", "u-v21", Some("甲"), None)
            .unwrap();
        let saved = db
            .commit_qoder_authorization(
                &acc.id,
                "own_browser",
                Some("localjson:v1:abc"),
                "valid",
                None,
                Some("0123456789abcdef"),
                &crate::storage::qoder_accounts::QoderAuthExpectation::Absent {
                    lifecycle: db.qoder_lifecycle(&acc.id).unwrap().unwrap(),
                },
            )
            .unwrap()
            .expect("首次授权");
        assert_eq!(saved.auth_version, 1, "首次提交就是第 1 版");

        // 一次覆盖写必须让版本前进。
        let again = db
            .commit_qoder_authorization(
                &acc.id,
                "own_browser",
                Some("localjson:v1:def"),
                "valid",
                None,
                Some("0123456789abcdef"),
                &crate::storage::qoder_accounts::QoderAuthExpectation::Present {
                    secret_ref: Some("localjson:v1:abc".to_string()),
                    lifecycle: db.qoder_lifecycle(&acc.id).unwrap().unwrap(),
                    auth_version: 1,
                },
            )
            .unwrap()
            .expect("覆盖写");
        assert_eq!(again.auth_version, 2, "重新授权必须前进");

        // 退回 v20 的形状：没有这一列。
        db.exec_raw("ALTER TABLE qoder_authorizations DROP COLUMN auth_version;")
            .expect("退回 v20 的形状");
        db.conn.pragma_update(None, "user_version", 20).unwrap();
        assert_eq!(db.migrate().unwrap(), LATEST_VERSION);
        let row = db
            .get_qoder_authorization(&acc.id, "own_browser")
            .unwrap()
            .expect("行还在");
        assert_eq!(row.auth_version, 0, "老行从 0 起，不是猜出来的版本");
        assert_eq!(row.secret_ref.as_deref(), Some("localjson:v1:def"));

        // 回拨再跑一次：空操作。
        db.conn.pragma_update(None, "user_version", 20).unwrap();
        db.migrate().unwrap();
        assert_eq!(
            db.get_qoder_authorization(&acc.id, "own_browser")
                .unwrap()
                .unwrap()
                .auth_version,
            0
        );
    }

    /// v20：只加列 + 按引用前缀回填，**不改任何 access_ref**。
    ///
    /// 旧库里的借用授权还指着钥匙串；要改成动态关联必须走显式的重新关联，
    /// 迁移本身不许猜归属（PRD §3.3）。
    #[test]
    fn v20_records_the_storage_kind_without_rewriting_references() {
        let (mut db, _dir) = temp_db();
        let acc = db
            .upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: "uid-v20".to_string(),
                ..Default::default()
            })
            .unwrap();
        let id = crate::auth::ids::auth_id(&acc.id, "magpie");
        db.exec_raw(&format!(
            "INSERT INTO account_authorizations (
                 id, account_id, kind, access_ref, refresh_ref, auth_domain, token_expires_at,
                 status, version, key_migration_state, source_key, last_used_at, last_error,
                 created_at, updated_at
             ) VALUES ('{id}', '{}', 'magpie', 'keychain:workbuddy/{}/magpie/access', NULL,
                       NULL, NULL, 'valid', 0, 'ready', 'workbuddy#uid-v20', NULL, NULL,
                       '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z')",
            acc.id, acc.id
        ))
        .expect("退回 v19 的形状");
        db.exec_raw("ALTER TABLE account_authorizations DROP COLUMN storage_kind;")
            .unwrap();
        db.exec_raw("ALTER TABLE account_authorizations DROP COLUMN ref_updated_at;")
            .unwrap();
        db.conn.pragma_update(None, "user_version", 19).unwrap();

        assert_eq!(db.migrate().unwrap(), LATEST_VERSION);
        let row = db.find_authorization(&acc.id, "magpie").unwrap().unwrap();
        assert!(
            row.access_ref.starts_with("keychain:"),
            "迁移不许改引用：{}",
            row.access_ref
        );
        assert_eq!(row.storage_kind(), "keychain");
        assert_eq!(
            db.authorization_storage_counts().unwrap(),
            vec![("keychain".to_string(), 1)],
            "回填要按前缀算"
        );

        // 重新关联之后才变成 magpie，并且引用一起换掉。
        let before = db.find_authorization(&acc.id, "magpie").unwrap().unwrap();
        assert!(db
            .rebind_authorization_ref(
                &id,
                before.version,
                &before.access_ref,
                "magpie:v1:workbuddy#uid-v20",
                authz::STORAGE_MAGPIE,
                Some("workbuddy#uid-v20"),
            )
            .unwrap());
        let row = db.find_authorization(&acc.id, "magpie").unwrap().unwrap();
        assert_eq!(row.storage_kind(), "magpie");
        assert_eq!(
            db.authorization_storage_counts().unwrap(),
            vec![("magpie".to_string(), 1)]
        );

        // 回拨再跑一次：幂等，不能把已经是 magpie 的改回 keychain。
        db.conn.pragma_update(None, "user_version", 19).unwrap();
        db.migrate().unwrap();
        let row = db.find_authorization(&acc.id, "magpie").unwrap().unwrap();
        assert_eq!(row.storage_kind(), "magpie");
        assert_eq!(row.access_ref, "magpie:v1:workbuddy#uid-v20");
    }

    /// P0-3：重新关联带 CAS —— 拿旧快照去写必须失败，且不许改一个字。
    #[test]
    fn rebind_refuses_a_stale_snapshot() {
        let (mut db, _dir) = temp_db();
        let acc = db
            .upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: "uid-cas".to_string(),
                ..Default::default()
            })
            .unwrap();
        let id = crate::auth::ids::auth_id(&acc.id, "magpie");
        db.upsert_authorization(&crate::storage::AuthorizationRecord {
            id: id.clone(),
            account_id: acc.id.clone(),
            kind: "magpie".to_string(),
            access_ref: format!("keychain:workbuddy/{}/magpie/access", acc.id),
            refresh_ref: None,
            auth_domain: None,
            token_expires_at: None,
            status: "valid".to_string(),
            version: 0,
            key_migration_state: "ready".to_string(),
            source_key: Some("workbuddy#uid-cas".to_string()),
            last_used_at: None,
            last_error: None,
            created_at: "2026-11-01T00:00:00Z".to_string(),
            updated_at: "2026-11-01T00:00:00Z".to_string(),
        })
        .unwrap();
        let stale = db.find_authorization(&acc.id, "magpie").unwrap().unwrap();

        // 另一条流程（重新导入 / 另一个 Worker）先把它改成动态关联。
        assert!(db
            .rebind_authorization_ref(
                &stale.id,
                stale.version,
                &stale.access_ref,
                "magpie:v1:workbuddy#uid-cas",
                authz::STORAGE_MAGPIE,
                Some("workbuddy#uid-cas"),
            )
            .unwrap());

        // 旧快照再来一次：版本 + 引用都对不上，必须拒绝。
        assert!(
            !db.rebind_authorization_ref(
                &stale.id,
                stale.version,
                &stale.access_ref,
                "magpie:v1:workbuddy#someone-else",
                authz::STORAGE_MAGPIE,
                Some("workbuddy#someone-else"),
            )
            .unwrap(),
            "旧快照不许覆盖新引用"
        );
        let after = db.find_authorization(&acc.id, "magpie").unwrap().unwrap();
        assert_eq!(after.access_ref, "magpie:v1:workbuddy#uid-cas");
        assert_eq!(after.source_key.as_deref(), Some("workbuddy#uid-cas"));

        // 只改 version、引用不变也要拒绝（引用被别处改回去过的情况）。
        assert!(!db
            .rebind_authorization_ref(
                &stale.id,
                stale.version,
                "magpie:v1:workbuddy#uid-cas",
                "magpie:v1:workbuddy#x",
                authz::STORAGE_MAGPIE,
                Some("workbuddy#x"),
            )
            .unwrap());
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

    /// v15 → v16：**旧 Qoder 授权一行不丢**，重建后的表接受 `own_pat`，
    /// 仍然拒绝表外的来源，账号多了 `auth_epoch` 且默认 0。
    ///
    /// 这是本轮唯一一次"重建已有表"的迁移，必须在真实旧库形态上验证，
    /// 而不是新建一个库再看它长得对不对。
    #[test]
    fn v15_upgrade_keeps_qoder_authorizations_and_accepts_pat() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = legacy_db(dir.path(), 15);
        {
            let conn = Connection::open(&path).expect("open legacy");
            conn.execute_batch(
                "INSERT INTO qoder_accounts (id, edition, uid, nickname, email, created_at, updated_at)
                 VALUES ('qa_old', 'qoder_cn', 'u-old', '旧账号', 'old@e.com',
                         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');
                 INSERT INTO qoder_authorizations
                     (id, account_id, source, source_key, secret_ref, auth_status,
                      token_expires_at, fingerprint, created_at, updated_at)
                 VALUES ('qz_old', 'qa_old', 'own_browser', NULL, 'keychain:qoder/qa_old/own/access',
                         'valid', '2027-01-01T00:00:00Z', 'fp-old',
                         '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z');",
            )
            .expect("seed qoder");
        }

        let mut db = Db::open(&path).expect("open");
        assert_eq!(db.schema_version().unwrap(), LATEST_VERSION);

        let account = db.get_qoder_account("qa_old").unwrap().expect("账号还在");
        assert_eq!(account.nickname, "旧账号");
        assert_eq!(account.email.as_deref(), Some("old@e.com"));
        assert_eq!(account.auth_epoch, 0, "旧账号从 0 起算");

        let rows = db.list_qoder_authorizations("qa_old").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, "own_browser");
        assert_eq!(rows[0].auth_status, "valid");
        assert_eq!(rows[0].fingerprint.as_deref(), Some("fp-old"));

        // 新来源可写，且与旧来源并存。
        db.upsert_qoder_authorization(
            "qa_old",
            "own_pat",
            None,
            Some("keychain:qoder/qa_old/pat/token"),
            "valid",
            None,
            Some("fp-pat"),
        )
        .expect("own_pat 必须被 v16 接受");
        assert_eq!(db.list_qoder_authorizations("qa_old").unwrap().len(), 2);
        assert!(db.qoder_lifecycle("qa_old").unwrap().unwrap().auth_epoch > 0);

        // CHECK 仍然生效：表外来源一律拒绝。
        assert!(db
            .upsert_qoder_authorization("qa_old", "curl", None, None, "valid", None, None)
            .is_err());

        // 级联：删账号把两份授权一起带走。
        assert!(db.delete_qoder_account("qa_old").unwrap());
        assert!(db.list_qoder_authorizations("qa_old").unwrap().is_empty());
    }
}
