//! Qoder 平台服务账号与授权来源（v13）。
//!
//! ## 和 `qoder_profiles` 的区别（别混）
//!
//! | 表 | 是什么 | 依赖客户端吗 |
//! | --- | --- | --- |
//! | `qoder_profiles` | **本机客户端**的认证快照，用于识别"本机登录的是谁" | 依赖 |
//! | `qoder_accounts` | **平台服务**上的账号身份（UID） | 不依赖 |
//! | `qoder_authorizations` | 某个账号的一份授权来源（magpie / 浏览器） | 不依赖 |
//!
//! 所以：**不装 Qoder 客户端也能把账号导进来**。网页授权、Magpie 导入都只写后两张表。
//!
//! ## 授权来源的所有权
//!
//! - `magpie`：只登记关联（`source_key`），**不复制令牌**——Magpie 自己的可轮换
//!   refresh token 由 Magpie 刷新，AICC 复制一份再刷新只会互相踢下线。
//!   `secret_ref` 在这类行里恒为空。
//! - `own_browser`：令牌在钥匙串，这里只有 `secret_ref`（钥匙串键）与指纹。
//!
//! 同一账号可以有多个来源并存：两行分别存在，互不覆盖。

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::clock::now_rfc3339;
use crate::rpc::{RpcError, RpcResult};

pub const SOURCE_MAGPIE: &str = "magpie";
pub const SOURCE_OWN_BROWSER: &str = "own_browser";
pub const SOURCES: &[&str] = &[SOURCE_MAGPIE, SOURCE_OWN_BROWSER];
pub const EDITIONS: &[&str] = &["qoder_cn", "qoder_global"];
pub const AUTH_STATUSES: &[&str] = &[
    "unknown", "valid", "expired", "invalid", "denied", "missing",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderAccountRecord {
    pub id: String,
    pub edition: String,
    pub uid: String,
    pub nickname: String,
    pub email: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderAuthRecord {
    pub id: String,
    pub account_id: String,
    pub source: String,
    pub source_key: Option<String>,
    pub secret_ref: Option<String>,
    pub auth_status: String,
    pub token_expires_at: Option<String>,
    pub fingerprint: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl QoderAuthRecord {
    /// 给 RPC / 界面用：**不含令牌**，只有"有没有凭据"。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "accountId": self.account_id,
            "source": self.source,
            "sourceKey": self.source_key,
            "hasSecret": self.secret_ref.is_some(),
            "authStatus": self.auth_status,
            "tokenExpiresAt": self.token_expires_at,
            "fingerprint": self.fingerprint,
            "updatedAt": self.updated_at,
        })
    }
}

impl QoderAccountRecord {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "edition": self.edition,
            "uid": self.uid,
            "nickname": self.nickname,
            "email": self.email,
            "updatedAt": self.updated_at,
        })
    }
}

/// 账号主键 = `qa_` + sha256(edition + NUL + uid) 前 12 hex。
///
/// 用哈希而不是自增：同一份 Magpie 数据重复导入必须落到同一行，不能靠"先查再写"。
pub fn qoder_account_id(edition: &str, uid: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(edition.as_bytes());
    hasher.update([0u8]);
    hasher.update(uid.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("qa_{hex}")
}

/// 授权主键 = `qz_` + sha256(account_id + NUL + source) 前 12 hex。
pub fn qoder_auth_id(account_id: &str, source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(account_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(source.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("qz_{hex}")
}

const ACCOUNT_COLUMNS: &str = "id, edition, uid, nickname, email, created_at, updated_at";

const AUTH_COLUMNS: &str = "id, account_id, source, source_key, secret_ref, auth_status, \
     token_expires_at, fingerprint, created_at, updated_at";

fn map_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<QoderAccountRecord> {
    Ok(QoderAccountRecord {
        id: row.get(0)?,
        edition: row.get(1)?,
        uid: row.get(2)?,
        nickname: row.get(3)?,
        email: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn map_auth(row: &rusqlite::Row<'_>) -> rusqlite::Result<QoderAuthRecord> {
    Ok(QoderAuthRecord {
        id: row.get(0)?,
        account_id: row.get(1)?,
        source: row.get(2)?,
        source_key: row.get(3)?,
        secret_ref: row.get(4)?,
        auth_status: row.get(5)?,
        token_expires_at: row.get(6)?,
        fingerprint: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

impl crate::storage::Db {
    /// 建/接管一个 Qoder 平台账号。
    ///
    /// 语义与 `upsert_account` 一致：**只补空**。昵称只在为空时写入，
    /// 邮箱只在为空时写入——用户改过的名字不会被下一次导入冲掉。
    pub fn upsert_qoder_account(
        &mut self,
        edition: &str,
        uid: &str,
        nickname: Option<&str>,
        email: Option<&str>,
    ) -> RpcResult<QoderAccountRecord> {
        if !EDITIONS.contains(&edition) {
            return Err(RpcError::invalid_params(format!(
                "edition 只支持 qoder_cn / qoder_global，收到 {edition}"
            )));
        }
        if uid.trim().is_empty() {
            return Err(RpcError::invalid_params("uid 不能为空"));
        }
        let id = qoder_account_id(edition, uid);
        let now = now_rfc3339();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO qoder_accounts (id, edition, uid, nickname, email, created_at, updated_at)
             VALUES (?1, ?2, ?3, '', NULL, ?4, ?4)
             ON CONFLICT (edition, uid) DO NOTHING",
            params![id, edition, uid, now],
        )?;
        let mut sets: Vec<&str> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(name) = nickname.map(str::trim).filter(|s| !s.is_empty()) {
            sets.push("nickname = CASE WHEN nickname = '' THEN ? ELSE nickname END");
            args.push(Box::new(name.to_string()));
        }
        if let Some(mail) = email.map(str::trim).filter(|s| !s.is_empty()) {
            sets.push("email = COALESCE(email, ?)");
            args.push(Box::new(mail.to_string()));
        }
        sets.push("updated_at = ?");
        args.push(Box::new(now));
        args.push(Box::new(edition.to_string()));
        args.push(Box::new(uid.to_string()));
        let sql = format!(
            "UPDATE qoder_accounts SET {} WHERE edition = ? AND uid = ?",
            sets.join(", ")
        );
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        tx.execute(&sql, refs.as_slice())?;
        tx.commit()?;

        self.find_qoder_account(edition, uid)?
            .ok_or_else(|| RpcError::storage("写入后读不到 Qoder 账号"))
    }

    pub fn get_qoder_account(&self, id: &str) -> RpcResult<Option<QoderAccountRecord>> {
        let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM qoder_accounts WHERE id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt.query_row(params![id], map_account).optional()?)
    }

    pub fn find_qoder_account(
        &self,
        edition: &str,
        uid: &str,
    ) -> RpcResult<Option<QoderAccountRecord>> {
        let sql =
            format!("SELECT {ACCOUNT_COLUMNS} FROM qoder_accounts WHERE edition = ?1 AND uid = ?2");
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![edition, uid], map_account)
            .optional()?)
    }

    pub fn list_qoder_accounts(&self, edition: Option<&str>) -> RpcResult<Vec<QoderAccountRecord>> {
        let mut sql = format!("SELECT {ACCOUNT_COLUMNS} FROM qoder_accounts WHERE 1 = 1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(edition) = edition.filter(|e| !e.is_empty()) {
            sql.push_str(" AND edition = ?");
            args.push(Box::new(edition.to_string()));
        }
        sql.push_str(" ORDER BY edition ASC, created_at ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), map_account)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 写一份授权来源。`(account_id, source)` 唯一：同一来源重复导入是**更新**
    /// 元数据，不会多出一行；另一个来源的行完全不受影响。
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_qoder_authorization(
        &mut self,
        account_id: &str,
        source: &str,
        source_key: Option<&str>,
        secret_ref: Option<&str>,
        auth_status: &str,
        token_expires_at: Option<&str>,
        fingerprint: Option<&str>,
    ) -> RpcResult<QoderAuthRecord> {
        if !SOURCES.contains(&source) {
            return Err(RpcError::invalid_params(format!(
                "source 只支持 magpie / own_browser，收到 {source}"
            )));
        }
        if !AUTH_STATUSES.contains(&auth_status) {
            return Err(RpcError::invalid_params(format!(
                "authStatus 取值非法：{auth_status}"
            )));
        }
        // Magpie 来源不许带凭据引用：Magpie 的令牌由 Magpie 刷新，AICC 只借用。
        if source == SOURCE_MAGPIE && secret_ref.is_some() {
            return Err(RpcError::invalid_params(
                "magpie 来源不能持有 AICC 自己的凭据引用",
            ));
        }
        if self.get_qoder_account(account_id)?.is_none() {
            return Err(RpcError::not_found(format!(
                "Qoder 账号不存在：{account_id}"
            )));
        }
        let id = qoder_auth_id(account_id, source);
        let now = now_rfc3339();
        self.conn.execute(
            "INSERT INTO qoder_authorizations (
                 id, account_id, source, source_key, secret_ref, auth_status,
                 token_expires_at, fingerprint, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT (account_id, source) DO UPDATE SET
                 source_key = COALESCE(excluded.source_key, qoder_authorizations.source_key),
                 secret_ref = COALESCE(excluded.secret_ref, qoder_authorizations.secret_ref),
                 auth_status = excluded.auth_status,
                 token_expires_at = COALESCE(excluded.token_expires_at, qoder_authorizations.token_expires_at),
                 fingerprint = COALESCE(excluded.fingerprint, qoder_authorizations.fingerprint),
                 updated_at = excluded.updated_at",
            params![
                id,
                account_id,
                source,
                source_key,
                secret_ref,
                auth_status,
                token_expires_at,
                fingerprint,
                now
            ],
        )?;
        self.get_qoder_authorization(account_id, source)?
            .ok_or_else(|| RpcError::storage("写入后读不到 Qoder 授权"))
    }

    pub fn get_qoder_authorization(
        &self,
        account_id: &str,
        source: &str,
    ) -> RpcResult<Option<QoderAuthRecord>> {
        let sql = format!(
            "SELECT {AUTH_COLUMNS} FROM qoder_authorizations WHERE account_id = ?1 AND source = ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![account_id, source], map_auth)
            .optional()?)
    }

    pub fn list_qoder_authorizations(&self, account_id: &str) -> RpcResult<Vec<QoderAuthRecord>> {
        let sql = format!(
            "SELECT {AUTH_COLUMNS} FROM qoder_authorizations WHERE account_id = ?1 ORDER BY source ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![account_id], map_auth)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn delete_qoder_authorization(
        &mut self,
        account_id: &str,
        source: &str,
    ) -> RpcResult<bool> {
        let changed = self.conn.execute(
            "DELETE FROM qoder_authorizations WHERE account_id = ?1 AND source = ?2",
            params![account_id, source],
        )?;
        Ok(changed > 0)
    }

    pub fn delete_qoder_account(&mut self, id: &str) -> RpcResult<bool> {
        // 与 accounts 一样：快照表无外键，删账号时显式清快照。
        self.purge_account_quota_snapshots(id)?;
        let changed = self
            .conn
            .execute("DELETE FROM qoder_accounts WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    fn db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("accounts.sqlite")).unwrap();
        (db, dir)
    }

    #[test]
    fn same_uid_on_two_editions_is_two_accounts() {
        let (mut db, _dir) = db();
        let cn = db
            .upsert_qoder_account("qoder_cn", "u-1", Some("CN 账号"), None)
            .unwrap();
        let global = db
            .upsert_qoder_account("qoder_global", "u-1", Some("Global 账号"), None)
            .unwrap();
        assert_ne!(cn.id, global.id);
        assert_eq!(db.list_qoder_accounts(None).unwrap().len(), 2);
        assert_eq!(db.list_qoder_accounts(Some("qoder_cn")).unwrap().len(), 1);
    }

    #[test]
    fn reimporting_keeps_user_edited_nickname() {
        let (mut db, _dir) = db();
        db.upsert_qoder_account("qoder_global", "u-1", Some("我改的名字"), None)
            .unwrap();
        let again = db
            .upsert_qoder_account("qoder_global", "u-1", Some("Magpie 的名字"), None)
            .unwrap();
        assert_eq!(again.nickname, "我改的名字");
        assert_eq!(db.list_qoder_accounts(None).unwrap().len(), 1);
    }

    #[test]
    fn two_sources_coexist_and_do_not_overwrite_each_other() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_global", "u-1", None, None)
            .unwrap();
        db.upsert_qoder_authorization(
            &account.id,
            SOURCE_MAGPIE,
            Some("qoder#2ntprc"),
            None,
            "valid",
            Some("2026-12-12T03:39:39Z"),
            None,
        )
        .unwrap();
        db.upsert_qoder_authorization(
            &account.id,
            SOURCE_OWN_BROWSER,
            None,
            Some("qoder/global/u-1"),
            "valid",
            None,
            Some("688d92f2"),
        )
        .unwrap();

        let rows = db.list_qoder_authorizations(&account.id).unwrap();
        assert_eq!(rows.len(), 2);
        let magpie = rows.iter().find(|r| r.source == SOURCE_MAGPIE).unwrap();
        assert_eq!(magpie.source_key.as_deref(), Some("qoder#2ntprc"));
        assert!(magpie.secret_ref.is_none(), "magpie 来源不许有凭据引用");
        // 再导一次 magpie 不会动 own_browser 那行。
        db.upsert_qoder_authorization(
            &account.id,
            SOURCE_MAGPIE,
            Some("qoder#2ntprc"),
            None,
            "expired",
            None,
            None,
        )
        .unwrap();
        let rows = db.list_qoder_authorizations(&account.id).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter()
                .find(|r| r.source == SOURCE_MAGPIE)
                .unwrap()
                .auth_status,
            "expired"
        );
        assert_eq!(
            rows.iter()
                .find(|r| r.source == SOURCE_OWN_BROWSER)
                .unwrap()
                .auth_status,
            "valid"
        );
    }

    #[test]
    fn magpie_authorization_cannot_carry_a_secret() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        let err = db
            .upsert_qoder_authorization(
                &account.id,
                SOURCE_MAGPIE,
                Some("qoder-cn"),
                Some("qoder/cn/u-1"),
                "valid",
                None,
                None,
            )
            .unwrap_err();
        assert!(err.message.contains("magpie"));
    }

    #[test]
    fn authorization_needs_an_existing_account() {
        let (mut db, _dir) = db();
        assert!(db
            .upsert_qoder_authorization(
                "qa_missing",
                SOURCE_MAGPIE,
                None,
                None,
                "valid",
                None,
                None
            )
            .is_err());
    }

    #[test]
    fn deleting_an_account_cascades_its_authorizations() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_global", "u-1", None, None)
            .unwrap();
        db.upsert_qoder_authorization(
            &account.id,
            SOURCE_OWN_BROWSER,
            None,
            Some("ref"),
            "valid",
            None,
            None,
        )
        .unwrap();
        assert!(db.delete_qoder_account(&account.id).unwrap());
        assert!(db
            .list_qoder_authorizations(&account.id)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn token_columns_do_not_exist() {
        let (db, _dir) = db();
        let mut stmt = db
            .conn
            .prepare("PRAGMA table_info(qoder_authorizations)")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            !cols
                .iter()
                .any(|c| c.contains("token") && c != "token_expires_at"),
            "cols={cols:?}"
        );
    }
}
