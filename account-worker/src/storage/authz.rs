//! 多来源授权表（v6）与镜像列（v7）。
//!
//! 职责边界：
//! * 这里只做**数据库**的事：建表、回填、增删改查。钥匙串搬运在 `crate::auth::migrate`，
//!   因为钥匙串没有事务，塞进 SQL 迁移会留下"库回滚了、钥匙串写了一半"（G4）。
//! * 回填时直接算最终授权 ID（G5），不留 `seed-*` 占位；幂等靠 `UNIQUE (account_id, kind)`。

use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;

use crate::auth::ids;
use crate::clock::now_rfc3339;
use crate::rpc::{RpcError, RpcResult};

pub const KEY_STATE_PENDING: &str = "pending";
pub const KEY_STATE_MIGRATING: &str = "migrating";
pub const KEY_STATE_READY: &str = "ready";
pub const KEY_STATE_FAILED: &str = "failed";

pub const STATUSES: &[&str] = &["unknown", "valid", "expired", "invalid", "denied"];

/// 一份授权。`kind` 只有 `own` / `magpie` 两种取值。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorizationRecord {
    pub id: String,
    pub account_id: String,
    pub kind: String,
    pub access_ref: String,
    pub refresh_ref: Option<String>,
    pub auth_domain: Option<String>,
    pub token_expires_at: Option<String>,
    pub status: String,
    /// 单份授权的写版本。任何一次凭据 / 状态写入都 +1，在途的旧快照据此作废。
    pub version: i64,
    /// `pending` / `migrating` / `ready` / `failed`，见 `crate::auth::migrate`。
    pub key_migration_state: String,
    pub source_key: Option<String>,
    pub last_used_at: Option<String>,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl AuthorizationRecord {
    /// 钥匙串键（去掉 `keychain:` 前缀）。引用坏了返回 None。
    pub fn access_key(&self) -> Option<String> {
        ids::ref_to_key(&self.access_ref).map(str::to_string)
    }

    pub fn refresh_key(&self) -> Option<String> {
        self.refresh_ref
            .as_deref()
            .and_then(ids::ref_to_key)
            .map(str::to_string)
    }

    /// 凭据是不是已经搬完。`ready` 之前读旧键（G4）。
    pub fn credentials_migrated(&self) -> bool {
        self.key_migration_state == KEY_STATE_READY
    }
}

const AUTH_COLUMNS: &str = "id, account_id, kind, access_ref, refresh_ref, auth_domain,
    token_expires_at, status, version, key_migration_state, source_key,
    last_used_at, last_error, created_at, updated_at";

fn map_authorization(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuthorizationRecord> {
    Ok(AuthorizationRecord {
        id: row.get(0)?,
        account_id: row.get(1)?,
        kind: row.get(2)?,
        access_ref: row.get(3)?,
        refresh_ref: row.get(4)?,
        auth_domain: row.get(5)?,
        token_expires_at: row.get(6)?,
        status: row.get(7)?,
        version: row.get(8)?,
        key_migration_state: row.get(9)?,
        source_key: row.get(10)?,
        last_used_at: row.get(11)?,
        last_error: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

impl super::Db {
    pub fn list_authorizations(&self, account_id: &str) -> RpcResult<Vec<AuthorizationRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {AUTH_COLUMNS} FROM account_authorizations
              WHERE account_id = ?1 ORDER BY kind ASC"
        ))?;
        let rows = stmt.query_map(params![account_id], map_authorization)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn get_authorization(&self, id: &str) -> RpcResult<Option<AuthorizationRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {AUTH_COLUMNS} FROM account_authorizations WHERE id = ?1"
        ))?;
        Ok(stmt.query_row(params![id], map_authorization).optional()?)
    }

    pub fn find_authorization(
        &self,
        account_id: &str,
        kind: &str,
    ) -> RpcResult<Option<AuthorizationRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {AUTH_COLUMNS} FROM account_authorizations
              WHERE account_id = ?1 AND kind = ?2"
        ))?;
        Ok(stmt
            .query_row(params![account_id, kind], map_authorization)
            .optional()?)
    }

    /// 写一行授权（存在则更新元数据），`version` 不前进：用于元数据修正。
    pub fn upsert_authorization(&self, record: &AuthorizationRecord) -> RpcResult<()> {
        if !ids::is_known_kind(&record.kind) {
            return Err(RpcError::invalid_params(format!(
                "授权类型非法：{}",
                record.kind
            )));
        }
        put_authorization(self, record, false)
    }

    /// 凭据写入后调用：更新字段并把 `version` 推进一格（在途旧快照就此作废）。
    pub fn replace_authorization(&self, record: &AuthorizationRecord) -> RpcResult<()> {
        put_authorization(self, record, true)
    }

    /// 带版本校验的状态写入。返回 false 表示版本对不上（旧快照，一个字节都没写）。
    pub fn set_authorization_status(
        &self,
        id: &str,
        expected_version: i64,
        status: &str,
        token_expires_at: Option<&str>,
        error: Option<&str>,
    ) -> RpcResult<bool> {
        if !STATUSES.contains(&status) {
            return Err(RpcError::invalid_params(format!("授权状态非法：{status}")));
        }
        let changed = self.conn.execute(
            "UPDATE account_authorizations
                SET status = ?1,
                    token_expires_at = COALESCE(?2, token_expires_at),
                    last_error = ?3,
                    version = version + 1,
                    updated_at = ?4
              WHERE id = ?5 AND version = ?6",
            params![
                status,
                token_expires_at,
                error,
                now_rfc3339(),
                id,
                expected_version
            ],
        )?;
        Ok(changed > 0)
    }

    /// 迁移状态推进。**不动 `version`**：它不是凭据变更，旧快照不该因此作废。
    pub fn set_key_migration_state(
        &self,
        id: &str,
        state: &str,
        error: Option<&str>,
    ) -> RpcResult<()> {
        if ![
            KEY_STATE_PENDING,
            KEY_STATE_MIGRATING,
            KEY_STATE_READY,
            KEY_STATE_FAILED,
        ]
        .contains(&state)
        {
            return Err(RpcError::invalid_params(format!("迁移状态非法：{state}")));
        }
        self.conn.execute(
            "UPDATE account_authorizations
                SET key_migration_state = ?1, last_error = ?2, updated_at = ?3
              WHERE id = ?4",
            params![state, error, now_rfc3339(), id],
        )?;
        Ok(())
    }

    /// 同步账号的兼容镜像列（G6）。
    ///
    /// `auth_mirror_auth_id` 与四个字段一起写：镜像必须能追溯回具体的授权行，
    /// 否则"来源 own、凭据却指向 magpie"这种组合就无法被发现。
    /// 这里**不推进 `auth_epoch`**：镜像是派生值，不是新的授权事实。
    pub fn set_account_mirror(
        &self,
        account_id: &str,
        auth_id: &str,
        kind: &str,
        status: &str,
        token_expires_at: Option<&str>,
        auth_ref: Option<&str>,
    ) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE accounts
                SET auth_mirror_auth_id = ?1,
                    auth_source = ?2,
                    auth_status = ?3,
                    token_expires_at = COALESCE(?4, token_expires_at),
                    auth_ref = COALESCE(?5, auth_ref),
                    updated_at = ?6
              WHERE id = ?7",
            params![
                auth_id,
                kind,
                status,
                token_expires_at,
                auth_ref,
                now_rfc3339(),
                account_id
            ],
        )?;
        Ok(())
    }

    /// 记一条授权级错误（迁移 / 别名同步 / 删除失败）。内容必须已脱敏。
    pub fn set_authorization_error(&self, id: &str, message: &str) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE account_authorizations SET last_error = ?1, updated_at = ?2 WHERE id = ?3",
            params![message, now_rfc3339(), id],
        )?;
        Ok(())
    }

    pub fn delete_authorization(&self, id: &str) -> RpcResult<bool> {
        let n = self.conn.execute(
            "DELETE FROM account_authorizations WHERE id = ?1",
            params![id],
        )?;
        Ok(n > 0)
    }

    /// `health` 用：各迁移状态各有几行。
    pub fn authorization_migration_counts(&self) -> RpcResult<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT key_migration_state, COUNT(*) FROM account_authorizations
             GROUP BY key_migration_state",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// `health` 用：各来源 / 状态各有几行。
    pub fn authorization_status_counts(&self) -> RpcResult<Vec<(String, String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT kind, status, COUNT(*) FROM account_authorizations
             GROUP BY kind, status ORDER BY kind ASC, status ASC",
        )?;
        let rows = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// `health` 用：最近一条迁移失败的原因（写入前已脱敏）。
    pub fn last_migration_error(&self) -> RpcResult<Option<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT last_error FROM account_authorizations
              WHERE key_migration_state = 'failed' AND last_error IS NOT NULL
              ORDER BY updated_at DESC LIMIT 1",
        )?;
        Ok(stmt.query_row([], |row| row.get(0)).optional()?)
    }

    /// 需要搬运的授权（`pending` / `migrating` / `failed`）。
    pub fn authorizations_pending_key_migration(&self) -> RpcResult<Vec<AuthorizationRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {AUTH_COLUMNS} FROM account_authorizations
              WHERE key_migration_state <> 'ready' ORDER BY account_id ASC, kind ASC"
        ))?;
        let rows = stmt.query_map([], map_authorization)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 读设置项。M1 只需要它来定首选来源；RPC 面在 M3 才开。
    pub fn get_setting(&self, key: &str) -> RpcResult<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT value FROM app_settings WHERE key = ?1")?;
        Ok(stmt.query_row(params![key], |row| row.get(0)).optional()?)
    }

    pub fn set_setting(&self, key: &str, value: &str) -> RpcResult<()> {
        self.conn.execute(
            "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![key, value, now_rfc3339()],
        )?;
        Ok(())
    }
}

fn put_authorization(
    db: &super::Db,
    record: &AuthorizationRecord,
    bump_version: bool,
) -> RpcResult<()> {
    let now = now_rfc3339();
    db.conn.execute(
        "INSERT INTO account_authorizations (
             id, account_id, kind, access_ref, refresh_ref, auth_domain, token_expires_at,
             status, version, key_migration_state, source_key, last_used_at, last_error,
             created_at, updated_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?14)
         ON CONFLICT (id) DO UPDATE SET
             access_ref = excluded.access_ref,
             refresh_ref = excluded.refresh_ref,
             auth_domain = excluded.auth_domain,
             token_expires_at = excluded.token_expires_at,
             status = excluded.status,
             version = account_authorizations.version + ?15,
             key_migration_state = excluded.key_migration_state,
             source_key = excluded.source_key,
             last_used_at = COALESCE(excluded.last_used_at, account_authorizations.last_used_at),
             last_error = excluded.last_error,
             updated_at = excluded.updated_at",
        params![
            record.id,
            record.account_id,
            record.kind,
            record.access_ref,
            record.refresh_ref,
            record.auth_domain,
            record.token_expires_at,
            record.status,
            record.version,
            record.key_migration_state,
            record.source_key,
            record.last_used_at,
            record.last_error,
            now,
            i64::from(bump_version),
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 迁移步骤
// ---------------------------------------------------------------------------

/// v6 的 Rust 部分：按现有 `accounts.auth_source` 给每个 WorkBuddy 账号回填一行。
///
/// 只读 `accounts`，不写它：账号 ID 与所有签到 / 旅行历史必须一行不改。
pub(super) fn backfill_v6(tx: &Transaction<'_>) -> RpcResult<()> {
    let now = now_rfc3339();

    // (account_id, kind, status, token_expires_at, auth_domain)
    let rows: Vec<(String, String, String, Option<String>, Option<String>)> = {
        let mut stmt = tx.prepare(
            "SELECT id, COALESCE(NULLIF(auth_source, ''), 'own'),
                    auth_status, token_expires_at, auth_domain
               FROM accounts WHERE platform = 'workbuddy'",
        )?;
        let mapped = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
            ))
        })?;
        let mut collected = Vec::new();
        for row in mapped {
            collected.push(row?);
        }
        collected
    };

    for (account_id, source, status, expires, domain) in rows {
        let kind = ids::kind_from_source(&source);
        // G5：直接算最终 ID。v6/v7 连续执行，临时 ID 连一个观察窗口都换不到。
        let id = ids::auth_id(&account_id, kind);
        let access_ref = ids::keychain_ref(&ids::access_key(&account_id, kind));
        // magpie 永远没有 refresh 引用；own 的 refresh 引用先写上，值由搬运补齐。
        let refresh_ref = if kind == ids::KIND_OWN {
            Some(ids::keychain_ref(&ids::refresh_key(&account_id, kind)))
        } else {
            None
        };
        let status = if STATUSES.contains(&status.as_str()) {
            status
        } else {
            "unknown".to_string()
        };
        tx.execute(
            "INSERT INTO account_authorizations (
                 id, account_id, kind, access_ref, refresh_ref, auth_domain, token_expires_at,
                 status, version, key_migration_state, source_key, last_used_at, last_error,
                 created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 'pending', NULL, NULL, NULL, ?9, ?9)
             ON CONFLICT (account_id, kind) DO NOTHING",
            params![
                id,
                account_id,
                kind,
                access_ref,
                refresh_ref,
                domain,
                expires,
                status,
                now
            ],
        )?;
    }
    Ok(())
}

/// v7 的 Rust 部分：给每个账号定下"镜像来自哪一行"，并把兼容镜像同步一次。
///
/// 迁移时每个账号只有一行授权（v6 刚回填），所以选它就是选"当前唯一的那份凭据"。
/// 排序仍写成"own 优先、显式 ORDER BY"，将来有第二行时这段不会选错。
///
/// 这里**不推进 `auth_epoch`**：迁移发生在启动时，没有任何在途快照；而每次启动都推进
/// 版本号会让"版本没变"这个信号失去意义。镜像本身是派生值，不是新的授权事实。
pub(super) fn seed_mirror_v7(tx: &Transaction<'_>) -> RpcResult<()> {
    let now = now_rfc3339();

    struct Mirror {
        account_id: String,
        auth_id: String,
        kind: String,
        status: String,
        token_expires_at: Option<String>,
        access_ref: String,
    }

    let rows: Vec<Mirror> = {
        let mut stmt = tx.prepare(
            "SELECT z.account_id, z.id, z.kind, z.status, z.token_expires_at, z.access_ref
               FROM account_authorizations z
               JOIN accounts a ON a.id = z.account_id
              WHERE a.platform = 'workbuddy'
              ORDER BY z.account_id ASC, CASE z.kind WHEN 'own' THEN 0 ELSE 1 END ASC",
        )?;
        let mapped = stmt.query_map([], |row| {
            Ok(Mirror {
                account_id: row.get(0)?,
                auth_id: row.get(1)?,
                kind: row.get(2)?,
                status: row.get(3)?,
                token_expires_at: row.get(4)?,
                access_ref: row.get(5)?,
            })
        })?;
        let mut collected = Vec::new();
        for row in mapped {
            collected.push(row?);
        }
        collected
    };

    let mut seen: Vec<String> = Vec::new();
    for mirror in rows {
        if seen.contains(&mirror.account_id) {
            continue; // 每个账号只镜像第一行
        }
        seen.push(mirror.account_id.clone());
        tx.execute(
            "UPDATE accounts
                SET auth_mirror_auth_id = ?1,
                    auth_source = ?2,
                    auth_status = ?3,
                    token_expires_at = ?4,
                    auth_ref = ?5,
                    updated_at = ?6
              WHERE id = ?7",
            params![
                mirror.auth_id,
                mirror.kind,
                mirror.status,
                mirror.token_expires_at,
                mirror.access_ref,
                now,
                mirror.account_id
            ],
        )?;
    }
    Ok(())
}
