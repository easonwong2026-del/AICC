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

const SCHEMA_V1: &str = include_str!("schema.sql");

/// 迁移清单：按 version 升序，只追加不修改。
const MIGRATIONS: &[(i64, &str)] = &[(1, SCHEMA_V1)];

pub const LATEST_VERSION: i64 = 1;

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
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInput {
    pub platform: String,
    pub uid: String,
    #[serde(default)]
    pub nickname: String,
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
        let auth_status = input
            .auth_status
            .clone()
            .unwrap_or_else(|| "unknown".into());
        if !["unknown", "valid", "expired", "invalid", "denied"].contains(&auth_status.as_str()) {
            return Err(RpcError::invalid_params(format!(
                "auth_status 取值非法：{auth_status}"
            )));
        }
        let checkin_enabled = input.checkin_enabled.unwrap_or(false);

        self.conn.execute(
            "INSERT INTO accounts (
                 id, platform, uid, nickname, email, group_id, auth_ref, profile_ref,
                 auth_status, checkin_enabled, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)
             ON CONFLICT (platform, uid) DO UPDATE SET
                 nickname = excluded.nickname,
                 email = excluded.email,
                 group_id = excluded.group_id,
                 auth_ref = excluded.auth_ref,
                 profile_ref = excluded.profile_ref,
                 auth_status = excluded.auth_status,
                 checkin_enabled = excluded.checkin_enabled,
                 updated_at = excluded.updated_at",
            params![
                id,
                input.platform,
                input.uid,
                input.nickname,
                input.email,
                input.group_id,
                input.auth_ref,
                input.profile_ref,
                auth_status,
                if checkin_enabled { 1 } else { 0 },
                now,
            ],
        )?;

        self.get_account(&id)?
            .ok_or_else(|| RpcError::new(crate::rpc::E_STORAGE, "写入后读不到账号"))
    }

    pub fn get_account(&self, id: &str) -> RpcResult<Option<AccountRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, platform, uid, nickname, email, group_id, auth_status,
                    checkin_enabled, last_checkin_at, last_result, updated_at
             FROM accounts WHERE id = ?1",
        )?;
        let row = stmt.query_row(params![id], map_account).optional()?;
        Ok(row)
    }

    pub fn list_accounts(&self, filter: &AccountFilter) -> RpcResult<Vec<AccountRecord>> {
        let mut sql = String::from(
            "SELECT id, platform, uid, nickname, email, group_id, auth_status,
                    checkin_enabled, last_checkin_at, last_result, updated_at
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
            nickname: "WB".into(),
            ..Default::default()
        };
        let qd = AccountInput {
            platform: "qoder_cn".into(),
            uid: "42".into(),
            nickname: "QD".into(),
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
            nickname: "旧名字".into(),
            ..Default::default()
        };
        let first = db.upsert_account(&input).unwrap();
        input.nickname = "新名字".into();
        let second = db.upsert_account(&input).unwrap();
        assert_eq!(first.id, second.id);
        assert_eq!(second.nickname, "新名字");
        assert_eq!(
            db.list_accounts(&AccountFilter::default()).unwrap().len(),
            1
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
                nickname: nickname.into(),
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
