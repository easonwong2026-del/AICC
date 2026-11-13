//! Qoder Profile 存储层（v11）。
//!
//! ## 为什么要"快照"而不是"独立目录"
//!
//! M0 实测：Qoder 客户端**完全忽略** `--user-data-dir`（进程守着默认目录，
//! 传入的目录一直是空的，见 `docs/QODER_MULTI_PROFILE_SPIKE.md` §4）。
//! 一个 OS 用户只有一份 `userData`，不可能靠启动参数并行多个隔离实例。
//!
//! 所以一个 Profile = **一份认证快照**：
//!
//! | 概念 | 位置 | 谁拥有 |
//! | --- | --- | --- |
//! | `live_dir` | 客户端真实数据目录（`com.qoder.app.stable` 等） | 客户端 |
//! | `snapshot_dir` | AICC 数据目录下 `qoder/profiles/<profile_id>/` | AICC |
//!
//! 切换账号 = 先退出客户端 → 把当前 live 认证存回它所属的 Profile →
//! 把目标 Profile 的快照写回 live → 再启动。Token 明文始终不落这个库，
//! 库里只有指纹（sha256 前 8 位）与到期时间。
//!
//! 之所以不复用 `accounts` 表：`accounts.platform` 的 CHECK 只有
//! `workbuddy` / `qoder_cn`，而它背后挂着授权、签到、成长、旅行四组外键，
//! 为了加一个平台值去重建主表，风险不可接受。

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::clock::now_rfc3339;
use crate::rpc::RpcResult;

pub const AUTH_STATUSES: &[&str] = &[
    "unknown", "valid", "expired", "invalid", "denied", "missing",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileRecord {
    pub profile_id: String,
    pub edition: String,
    pub display_name: String,
    pub account_uid: Option<String>,
    pub email: Option<String>,
    pub app_bundle_id: Option<String>,
    pub app_path: Option<String>,
    pub client_version: Option<String>,
    /// 客户端真实使用的数据目录（每个版本一份，同版本的多个 Profile 共享）。
    pub live_dir: String,
    /// AICC 管理的快照目录（每个 Profile 独占）。
    pub snapshot_dir: String,
    /// 快照里的认证文件路径。
    pub auth_file: Option<String>,
    pub auth_fingerprint: Option<String>,
    /// 快照文件的整文件 sha256，切换时用来判断"快照有没有被改坏"。
    pub snapshot_hash: Option<String>,
    pub auth_status: String,
    pub expires_at: Option<String>,
    pub managed: bool,
    pub last_opened_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone)]
pub struct ProfileInsert {
    pub profile_id: String,
    pub edition: String,
    pub display_name: String,
    pub live_dir: String,
    pub snapshot_dir: String,
    pub app_bundle_id: Option<String>,
    pub app_path: Option<String>,
    pub client_version: Option<String>,
    pub auth_file: Option<String>,
    pub snapshot_hash: Option<String>,
    pub auth_status: String,
    pub managed: bool,
}

/// 补丁语义：`None` = 不动这一列。
#[derive(Debug, Default, Clone)]
pub struct ProfilePatch {
    pub display_name: Option<String>,
    pub account_uid: Option<String>,
    pub email: Option<String>,
    pub app_bundle_id: Option<String>,
    pub app_path: Option<String>,
    pub client_version: Option<String>,
    pub auth_file: Option<String>,
    pub auth_fingerprint: Option<String>,
    pub snapshot_hash: Option<String>,
    pub auth_status: Option<String>,
    pub expires_at: Option<String>,
    pub managed: Option<bool>,
    pub last_opened_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeRecord {
    pub profile_id: String,
    pub pid: i64,
    pub executable: String,
    pub launched_at: String,
}

const PROFILE_COLUMNS: &str =
    "profile_id, edition, display_name, account_uid, email, app_bundle_id, \
     app_path, client_version, live_dir, snapshot_dir, auth_file, auth_fingerprint, \
     snapshot_hash, auth_status, expires_at, managed, last_opened_at, created_at, updated_at";

fn map_profile(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProfileRecord> {
    Ok(ProfileRecord {
        profile_id: row.get(0)?,
        edition: row.get(1)?,
        display_name: row.get(2)?,
        account_uid: row.get(3)?,
        email: row.get(4)?,
        app_bundle_id: row.get(5)?,
        app_path: row.get(6)?,
        client_version: row.get(7)?,
        live_dir: row.get(8)?,
        snapshot_dir: row.get(9)?,
        auth_file: row.get(10)?,
        auth_fingerprint: row.get(11)?,
        snapshot_hash: row.get(12)?,
        auth_status: row.get(13)?,
        expires_at: row.get(14)?,
        managed: row.get::<_, i64>(15)? != 0,
        last_opened_at: row.get(16)?,
        created_at: row.get(17)?,
        updated_at: row.get(18)?,
    })
}

impl ProfileRecord {
    /// 给 RPC / Swift 的展示形态：**永远不带令牌**。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_else(|_| serde_json::json!({}))
    }
}

impl crate::storage::Db {
    pub fn insert_profile(&mut self, input: &ProfileInsert) -> RpcResult<ProfileRecord> {
        let now = now_rfc3339();
        self.conn.execute(
            "INSERT INTO qoder_profiles (
                 profile_id, edition, display_name, account_uid, email, app_bundle_id, app_path,
                 client_version, live_dir, snapshot_dir, auth_file, auth_fingerprint,
                 snapshot_hash, auth_status, expires_at, managed, last_opened_at,
                 created_at, updated_at
             ) VALUES (?1, ?2, ?3, NULL, NULL, ?4, ?5, ?6, ?7, ?8, ?9, NULL, ?10, ?11, NULL, ?12, NULL, ?13, ?13)",
            params![
                input.profile_id,
                input.edition,
                input.display_name,
                input.app_bundle_id,
                input.app_path,
                input.client_version,
                input.live_dir,
                input.snapshot_dir,
                input.auth_file,
                input.snapshot_hash,
                input.auth_status,
                i64::from(input.managed),
                now,
            ],
        )?;
        self.get_profile(&input.profile_id)?
            .ok_or_else(|| crate::rpc::RpcError::storage("写入后读不到 profile"))
    }

    pub fn list_profiles(&self, edition: Option<&str>) -> RpcResult<Vec<ProfileRecord>> {
        let mut sql = format!("SELECT {PROFILE_COLUMNS} FROM qoder_profiles WHERE 1 = 1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(edition) = edition.filter(|e| !e.is_empty()) {
            sql.push_str(" AND edition = ?");
            args.push(Box::new(edition.to_string()));
        }
        sql.push_str(" ORDER BY edition ASC, created_at ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), map_profile)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_profile(&self, profile_id: &str) -> RpcResult<Option<ProfileRecord>> {
        let sql = format!("SELECT {PROFILE_COLUMNS} FROM qoder_profiles WHERE profile_id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![profile_id], map_profile)
            .optional()?)
    }

    pub fn find_profile_by_snapshot_dir(&self, dir: &str) -> RpcResult<Option<ProfileRecord>> {
        let sql = format!("SELECT {PROFILE_COLUMNS} FROM qoder_profiles WHERE snapshot_dir = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt.query_row(params![dir], map_profile).optional()?)
    }

    /// 同版本下"已验证的 UID"唯一：不同版本同一 UID 是两个人，不能合并。
    pub fn find_profile_by_identity(
        &self,
        edition: &str,
        account_uid: &str,
    ) -> RpcResult<Option<ProfileRecord>> {
        let sql = format!(
            "SELECT {PROFILE_COLUMNS} FROM qoder_profiles WHERE edition = ?1 AND account_uid = ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![edition, account_uid], map_profile)
            .optional()?)
    }

    pub fn update_profile(
        &mut self,
        profile_id: &str,
        patch: &ProfilePatch,
    ) -> RpcResult<ProfileRecord> {
        if let Some(status) = patch.auth_status.as_deref() {
            if !AUTH_STATUSES.contains(&status) {
                return Err(crate::rpc::RpcError::invalid_params(format!(
                    "authStatus 取值非法：{status}"
                )));
            }
        }
        let mut sets: Vec<&str> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        macro_rules! set_text {
            ($field:expr, $column:literal) => {
                if let Some(value) = &$field {
                    sets.push(concat!($column, " = ?"));
                    args.push(Box::new(value.clone()));
                }
            };
        }

        set_text!(patch.display_name, "display_name");
        set_text!(patch.account_uid, "account_uid");
        set_text!(patch.email, "email");
        set_text!(patch.app_bundle_id, "app_bundle_id");
        set_text!(patch.app_path, "app_path");
        set_text!(patch.client_version, "client_version");
        set_text!(patch.auth_file, "auth_file");
        set_text!(patch.auth_fingerprint, "auth_fingerprint");
        set_text!(patch.snapshot_hash, "snapshot_hash");
        set_text!(patch.auth_status, "auth_status");
        set_text!(patch.expires_at, "expires_at");
        set_text!(patch.last_opened_at, "last_opened_at");
        if let Some(value) = patch.managed {
            sets.push("managed = ?");
            args.push(Box::new(i64::from(value)));
        }
        sets.push("updated_at = ?");
        args.push(Box::new(now_rfc3339()));

        args.push(Box::new(profile_id.to_string()));
        let sql = format!(
            "UPDATE qoder_profiles SET {} WHERE profile_id = ?",
            sets.join(", ")
        );
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let changed = self.conn.execute(&sql, refs.as_slice())?;
        if changed == 0 {
            return Err(crate::rpc::RpcError::not_found(format!(
                "profile 不存在：{profile_id}"
            )));
        }
        self.get_profile(profile_id)?
            .ok_or_else(|| crate::rpc::RpcError::storage("更新后读不到 profile"))
    }

    pub fn delete_profile(&mut self, profile_id: &str) -> RpcResult<bool> {
        let changed = self.conn.execute(
            "DELETE FROM qoder_profiles WHERE profile_id = ?1",
            params![profile_id],
        )?;
        Ok(changed > 0)
    }

    // ---- 运行记录 ----

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_runtime(&mut self, record: &RuntimeRecord) -> RpcResult<()> {
        self.conn.execute(
            "INSERT INTO qoder_runtime (profile_id, pid, executable, launched_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (profile_id) DO UPDATE SET
                 pid = excluded.pid,
                 executable = excluded.executable,
                 launched_at = excluded.launched_at",
            params![
                record.profile_id,
                record.pid,
                record.executable,
                record.launched_at
            ],
        )?;
        Ok(())
    }

    pub fn get_runtime(&self, profile_id: &str) -> RpcResult<Option<RuntimeRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT profile_id, pid, executable, launched_at FROM qoder_runtime WHERE profile_id = ?1",
        )?;
        Ok(stmt
            .query_row(params![profile_id], map_runtime)
            .optional()?)
    }

    pub fn list_runtime(&self) -> RpcResult<Vec<RuntimeRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT profile_id, pid, executable, launched_at FROM qoder_runtime ORDER BY launched_at ASC",
        )?;
        let rows = stmt.query_map([], map_runtime)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn clear_runtime(&mut self, profile_id: &str) -> RpcResult<()> {
        self.conn.execute(
            "DELETE FROM qoder_runtime WHERE profile_id = ?1",
            params![profile_id],
        )?;
        Ok(())
    }
}

fn map_runtime(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuntimeRecord> {
    Ok(RuntimeRecord {
        profile_id: row.get(0)?,
        pid: row.get(1)?,
        executable: row.get(2)?,
        launched_at: row.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    fn temp_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("accounts.sqlite")).unwrap();
        (db, dir)
    }

    fn sample(id: &str, edition: &str, snapshot: &str) -> ProfileInsert {
        ProfileInsert {
            profile_id: id.into(),
            edition: edition.into(),
            display_name: "账号".into(),
            live_dir: "/live/qoder".into(),
            snapshot_dir: snapshot.into(),
            app_bundle_id: Some("com.qoder.app".into()),
            app_path: Some("/Applications/Qoder.app".into()),
            client_version: Some("0.5.7".into()),
            auth_file: Some(format!("{snapshot}/auth.v1.dat")),
            snapshot_hash: None,
            auth_status: "unknown".into(),
            managed: true,
        }
    }

    #[test]
    fn inserts_and_lists_profiles() {
        let (mut db, _dir) = temp_db();
        db.insert_profile(&sample("qp_1", "qoder_cn", "/snap/a"))
            .unwrap();
        db.insert_profile(&sample("qp_2", "qoder_global", "/snap/b"))
            .unwrap();

        assert_eq!(db.list_profiles(None).unwrap().len(), 2);
        assert_eq!(db.list_profiles(Some("qoder_cn")).unwrap().len(), 1);
        assert!(db.list_profiles(Some("qoder")).unwrap().is_empty());
    }

    #[test]
    fn same_live_dir_is_allowed_for_multiple_profiles() {
        // 快照模型下多个 Profile 共享同一个 live 目录，这是正常的。
        let (mut db, _dir) = temp_db();
        db.insert_profile(&sample("qp_1", "qoder_global", "/snap/a"))
            .unwrap();
        db.insert_profile(&sample("qp_2", "qoder_global", "/snap/b"))
            .unwrap();
        assert_eq!(db.list_profiles(Some("qoder_global")).unwrap().len(), 2);
    }

    #[test]
    fn snapshot_dir_must_be_unique() {
        let (mut db, _dir) = temp_db();
        db.insert_profile(&sample("qp_1", "qoder_global", "/snap/a"))
            .unwrap();
        assert!(db
            .insert_profile(&sample("qp_2", "qoder_global", "/snap/a"))
            .is_err());
    }

    #[test]
    fn identity_is_unique_per_edition_only() {
        let (mut db, _dir) = temp_db();
        db.insert_profile(&sample("qp_1", "qoder_cn", "/snap/a"))
            .unwrap();
        db.insert_profile(&sample("qp_2", "qoder_global", "/snap/b"))
            .unwrap();

        let patch = ProfilePatch {
            account_uid: Some("u-1".into()),
            ..Default::default()
        };
        db.update_profile("qp_1", &patch).unwrap();
        db.update_profile("qp_2", &patch).unwrap();
        assert!(db
            .find_profile_by_identity("qoder_cn", "u-1")
            .unwrap()
            .is_some());
        assert!(db
            .find_profile_by_identity("qoder_global", "u-1")
            .unwrap()
            .is_some());

        db.insert_profile(&sample("qp_3", "qoder_cn", "/snap/c"))
            .unwrap();
        assert!(db
            .update_profile(
                "qp_3",
                &ProfilePatch {
                    account_uid: Some("u-1".into()),
                    ..Default::default()
                }
            )
            .is_err());
    }

    #[test]
    fn patch_touches_only_given_fields() {
        let (mut db, _dir) = temp_db();
        db.insert_profile(&sample("qp_1", "qoder_cn", "/snap/a"))
            .unwrap();
        let updated = db
            .update_profile(
                "qp_1",
                &ProfilePatch {
                    display_name: Some("工作账号".into()),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(updated.display_name, "工作账号");
        assert_eq!(updated.auth_status, "unknown");
        assert_eq!(updated.client_version.as_deref(), Some("0.5.7"));
    }

    #[test]
    fn rejects_unknown_auth_status() {
        let (mut db, _dir) = temp_db();
        db.insert_profile(&sample("qp_1", "qoder_cn", "/snap/a"))
            .unwrap();
        let err = db
            .update_profile(
                "qp_1",
                &ProfilePatch {
                    auth_status: Some("bogus".into()),
                    ..Default::default()
                },
            )
            .unwrap_err();
        assert!(err.message.contains("authStatus"));
    }

    #[test]
    fn runtime_rows_survive_and_clear() {
        let (mut db, _dir) = temp_db();
        db.insert_profile(&sample("qp_1", "qoder_cn", "/snap/a"))
            .unwrap();
        db.set_runtime(&RuntimeRecord {
            profile_id: "qp_1".into(),
            pid: 4242,
            executable: "/Applications/Qoder.app/Contents/MacOS/Qoder".into(),
            launched_at: "2026-01-01T00:00:00Z".into(),
        })
        .unwrap();
        assert_eq!(db.get_runtime("qp_1").unwrap().unwrap().pid, 4242);
        db.clear_runtime("qp_1").unwrap();
        assert!(db.get_runtime("qp_1").unwrap().is_none());
    }

    #[test]
    fn deleting_profile_removes_runtime_row() {
        let (mut db, _dir) = temp_db();
        db.insert_profile(&sample("qp_1", "qoder_cn", "/snap/a"))
            .unwrap();
        db.set_runtime(&RuntimeRecord {
            profile_id: "qp_1".into(),
            pid: 1,
            executable: "/x".into(),
            launched_at: "t".into(),
        })
        .unwrap();
        assert!(db.delete_profile("qp_1").unwrap());
        assert!(db.list_runtime().unwrap().is_empty());
    }

    #[test]
    fn token_never_has_a_column() {
        let (db, _dir) = temp_db();
        let mut stmt = db
            .conn
            .prepare("PRAGMA table_info(qoder_profiles)")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(!cols.iter().any(|c| c.contains("token")), "cols={cols:?}");
    }
}
