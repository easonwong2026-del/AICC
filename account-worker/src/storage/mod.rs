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

pub mod travel;

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

/// 迁移清单：按 version 升序，只追加不修改。
const MIGRATIONS: &[(i64, &str)] = &[
    (1, SCHEMA_V1),
    (2, MIGRATION_V2),
    (3, MIGRATION_V3),
    (4, MIGRATION_V4),
];

pub const LATEST_VERSION: i64 = 4;

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
    pub auth_source: String,
    /// 该账号是否允许自动派 Buddy 旅行。默认关闭，与签到开关互不影响。
    pub travel_enabled: bool,
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
}

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
        for (version, sql) in MIGRATIONS {
            if *version <= current {
                continue;
            }
            let tx = self.conn.transaction()?;
            tx.execute_batch(sql)?;
            tx.execute(
                "INSERT OR REPLACE INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![version, now_rfc3339()],
            )?;
            tx.pragma_update(None, "user_version", *version)?;
            tx.commit()?;
            current = *version;
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
                 auth_status, checkin_enabled, created_at, updated_at
             ) VALUES (?1, ?2, ?3, '', NULL, NULL, NULL, NULL, 'unknown', 0, ?4, ?4)
             ON CONFLICT (platform, uid) DO NOTHING",
            params![id, input.platform, input.uid, now],
        )?;

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
                    token_expires_at, auth_domain, auth_source, travel_enabled
             FROM accounts WHERE id = ?1",
        )?;
        let row = stmt.query_row(params![id], map_account).optional()?;
        Ok(row)
    }

    /// 按 platform + uid 查账号。登录去重与刷新都用它。
    pub fn find_account(&self, platform: &str, uid: &str) -> RpcResult<Option<AccountRecord>> {
        self.get_account(&account_id(platform, uid))
    }

    /// 只更新登录态相关字段，不动用户填的备注/分组。
    pub fn set_auth_state(
        &self,
        id: &str,
        status: &str,
        token_expires_at: Option<&str>,
    ) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE accounts
                SET auth_status = ?1,
                    token_expires_at = COALESCE(?2, token_expires_at),
                    updated_at = ?3
              WHERE id = ?4",
            params![status, token_expires_at, now_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn list_accounts(&self, filter: &AccountFilter) -> RpcResult<Vec<AccountRecord>> {
        let mut sql = String::from(
            "SELECT id, platform, uid, nickname, email, group_id, auth_status,
                    checkin_enabled, last_checkin_at, last_result, updated_at,
                    token_expires_at, auth_domain, auth_source, travel_enabled
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
