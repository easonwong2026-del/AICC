//! 授权写入服务（M2）。
//!
//! 所有会改凭据的路径都收敛到这里：扫码登录、Magpie 导入 / 重同步、刷新、删除单份授权。
//! 三条规矩：
//!
//! 1. **一份授权只写自己的键**：own 写 `own/access|refresh`，magpie 只写 `magpie/access`，
//!    两边互不覆盖（G1）。Magpie 的 refresh 在类型上就不存在 —— 没有可写的键名。
//! 2. **写之前拍快照，失败就回滚**：钥匙串没有事务，写到一半失败会留下"新 access + 旧 refresh"。
//!    快照拍不出来（`Denied`）就中止，不拿未知状态赌博。
//! 3. **写完同步兼容镜像**：`accounts.*` 与旧别名键必须指向同一份授权（G6）。
//!    别名同步失败不算登录失败，但要记 `last_error` 并如实报 `degraded`。

use serde_json::{json, Value};

use crate::app::App;
use crate::rpc::{RpcError, RpcResult};
use crate::securestore::SecretErrorKind;
use crate::storage::authz::KEY_STATE_READY;
use crate::storage::{AccountInput, AccountRecord, AuthorizationRecord};

use super::{broker, ids};

/// 两份凭据的键名。`refresh` 对 magpie 恒为 None。
pub struct Keys {
    pub access: String,
    pub refresh: Option<String>,
}

pub fn own_keys(account_id: &str) -> Keys {
    Keys {
        access: ids::access_key(account_id, ids::KIND_OWN),
        refresh: Some(ids::refresh_key(account_id, ids::KIND_OWN)),
    }
}

pub fn magpie_keys(account_id: &str) -> Keys {
    Keys {
        access: ids::access_key(account_id, ids::KIND_MAGPIE),
        // 没有这个键，也不允许有。
        refresh: None,
    }
}

pub fn keys_for(account_id: &str, kind: &str) -> Keys {
    if kind == ids::KIND_MAGPIE {
        magpie_keys(account_id)
    } else {
        own_keys(account_id)
    }
}

// ---------------------------------------------------------------------------
// 快照 / 回滚
// ---------------------------------------------------------------------------

/// 写凭据前的快照。**只有 `NotFound` 才算"本来就没有"**。
pub struct Snapshot {
    entries: Vec<(String, Option<Vec<u8>>)>,
}

impl Snapshot {
    pub fn capture(app: &App, keys: &[String]) -> RpcResult<Self> {
        let mut entries = Vec::with_capacity(keys.len());
        for key in keys {
            match app.secure.get(key) {
                Ok(bytes) => entries.push((key.clone(), Some(bytes))),
                Err(err) if err.kind == SecretErrorKind::NotFound => {
                    entries.push((key.clone(), None))
                }
                Err(err) => {
                    return Err(RpcError::new(
                        crate::rpc::E_SECURESTORE,
                        format!("读取旧凭据失败，为避免覆盖未知状态已中止：{}", err.message),
                    )
                    .with_data(json!({ "kind": "snapshot_unreadable", "key": key })))
                }
            }
        }
        Ok(Self { entries })
    }

    /// 恢复。返回 false 表示没恢复干净（旧值也写不回去），必须留痕。
    pub fn restore(&self, app: &App) -> bool {
        let mut ok = true;
        for (key, old) in &self.entries {
            let restored = match old {
                Some(bytes) => app.secure.set(key, bytes).is_ok(),
                None => match app.secure.delete(key) {
                    Ok(_) => true,
                    Err(err) => err.kind == SecretErrorKind::NotFound,
                },
            };
            ok &= restored;
        }
        if !ok {
            crate::handlers::log_note("凭据回滚没有完全成功：钥匙串里可能留有半套凭据，请重新登录");
        }
        ok
    }
}

pub fn credential_error(err: crate::securestore::SecretError, stage: &str) -> RpcError {
    RpcError::new(
        crate::rpc::E_SECURESTORE,
        format!("写入凭据失败（{stage}）：{}", err.message),
    )
    .with_data(json!({ "kind": "credential_write_failed", "stage": stage }))
}

// ---------------------------------------------------------------------------
// 行管理
// ---------------------------------------------------------------------------

/// 取或建一行授权。新建的行 `key_migration_state = ready`：
/// 它的凭据本来就是按新键写的，没有旧键需要搬。
pub fn ensure_row(
    app: &App,
    account_id: &str,
    kind: &str,
    uid: &str,
) -> RpcResult<AuthorizationRecord> {
    if !ids::is_known_kind(kind) {
        return Err(RpcError::invalid_params(format!("授权类型非法：{kind}")));
    }
    if let Some(row) = app.with_db(|db| db.find_authorization(account_id, kind))? {
        return Ok(row);
    }
    let id = ids::auth_id(account_id, kind);
    let keys = keys_for(account_id, kind);
    let now = crate::clock::now_rfc3339();
    let row = AuthorizationRecord {
        id,
        account_id: account_id.to_string(),
        kind: kind.to_string(),
        access_ref: ids::keychain_ref(&keys.access),
        refresh_ref: keys.refresh.as_deref().map(ids::keychain_ref),
        auth_domain: None,
        token_expires_at: None,
        status: "unknown".to_string(),
        version: 0,
        key_migration_state: KEY_STATE_READY.to_string(),
        source_key: (kind == ids::KIND_MAGPIE).then(|| format!("workbuddy#{uid}")),
        last_used_at: None,
        last_error: None,
        created_at: now.clone(),
        updated_at: now,
    };
    app.with_db(|db| db.upsert_authorization(&row))?;
    Ok(row)
}

/// 把一份授权的状态写到库里，**保留调用方读到的版本号做校验**。
/// 返回 false 表示这份授权在途期间被改过（旧快照），什么都没写。
pub fn record_status(
    app: &App,
    row: &AuthorizationRecord,
    status: &str,
    expires: Option<&str>,
    error: Option<&str>,
) -> RpcResult<bool> {
    // Magpie 行的 refresh 引用必须一直是 NULL：这里先兜一道，防止将来有人从别处塞进来。
    if row.kind == ids::KIND_MAGPIE && row.refresh_ref.is_some() {
        return Err(RpcError::internal("magpie 授权不允许持有 refresh 引用"));
    }
    let applied = app
        .with_db(|db| db.set_authorization_status(&row.id, row.version, status, expires, error))?;
    if applied {
        sync_mirror(app, &row.account_id)?;
    }
    Ok(applied)
}

// ---------------------------------------------------------------------------
// 写入
// ---------------------------------------------------------------------------

/// 只写自有授权的钥匙串条目，**不碰数据库**。
///
/// 拆出来的原因：登录时账号行还不存在，而授权行的外键要求它先存在。
/// 顺序必须是「写凭据 → 建账号行 → 记授权行」，中途任何一步失败都要能回滚凭据。
pub fn write_own(
    app: &App,
    account_id: &str,
    access: &str,
    refresh: Option<&str>,
) -> RpcResult<Snapshot> {
    let keys = own_keys(account_id);
    let snapshot = Snapshot::capture(
        app,
        &[
            keys.access.clone(),
            keys.refresh.clone().expect("own 一定有 refresh 键"),
        ],
    )?;

    if let Err(err) = app.secure.set(&keys.access, access.as_bytes()) {
        return Err(rollback(
            app,
            &snapshot,
            credential_error(err, "own/access"),
        ));
    }
    let refresh_key = keys.refresh.as_deref().expect("own 一定有 refresh 键");
    match refresh {
        Some(token) => {
            if let Err(err) = app.secure.set(refresh_key, token.as_bytes()) {
                return Err(rollback(
                    app,
                    &snapshot,
                    credential_error(err, "own/refresh"),
                ));
            }
        }
        None => match app.secure.delete(refresh_key) {
            Ok(_) => {}
            Err(err) if err.kind == SecretErrorKind::NotFound => {}
            Err(err) => {
                return Err(rollback(
                    app,
                    &snapshot,
                    RpcError::new(
                        crate::rpc::E_SECURESTORE,
                        format!("旧的刷新令牌删不掉（{}），本次登录已回滚", err.message),
                    )
                    .with_data(json!({ "kind": "stale_refresh_not_cleared" })),
                ));
            }
        },
    }
    Ok(snapshot)
}

/// 凭据写完之后：把这份授权记成 valid、刷新到期时间、推进版本，再同步镜像。
///
/// 调用前账号行必须已经存在（授权行有外键）。
pub fn record_grant(
    app: &App,
    account_id: &str,
    uid: &str,
    kind: &str,
    expires_at: Option<&str>,
) -> RpcResult<Saved> {
    ensure_row(app, account_id, kind, uid)?;
    let row = finish_row(app, account_id, kind, "valid", expires_at, None)?;
    let degraded = sync_mirror(app, account_id).ok().and_then(|r| r.error);
    let fresh = app
        .with_db(|db| db.find_authorization(account_id, kind))?
        .unwrap_or(row);
    Ok(Saved {
        authorization: fresh,
        degraded,
    })
}

pub struct Saved {
    pub authorization: AuthorizationRecord,
    /// 兼容别名没同步成功时的原因（凭据本身已经写好）。
    pub degraded: Option<String>,
}

/// 写借用授权：只写 `magpie/access`。refresh 的键名都不出现。
pub fn save_magpie(
    app: &App,
    account_id: &str,
    uid: &str,
    access: &str,
    source_key: Option<&str>,
    expires_at: Option<&str>,
) -> RpcResult<Saved> {
    let mut row = ensure_row(app, account_id, ids::KIND_MAGPIE, uid)?;
    let keys = magpie_keys(account_id);
    let snapshot = Snapshot::capture(app, &[keys.access.clone()])?;

    if let Err(err) = app.secure.set(&keys.access, access.as_bytes()) {
        return Err(rollback(
            app,
            &snapshot,
            credential_error(err, "magpie/access"),
        ));
    }
    row.source_key = source_key.map(str::to_string).or(row.source_key);
    row.token_expires_at = expires_at.map(str::to_string).or(row.token_expires_at);
    row.status = "valid".to_string();
    row.key_migration_state = KEY_STATE_READY.to_string();
    row.last_error = None;
    app.with_db(|db| db.replace_authorization(&row))?;

    let degraded = sync_mirror(app, account_id).ok().and_then(|r| r.error);
    let fresh = app
        .with_db(|db| db.find_authorization(account_id, ids::KIND_MAGPIE))?
        .unwrap_or(row);
    let _ = snapshot; // 写入失败时已经在上面回滚过了，成功路径不需要它
    Ok(Saved {
        authorization: fresh,
        degraded,
    })
}

/// 回滚并合成最终错误。回滚不干净必须**如实**说出来 —— 不能一边恢复失败，
/// 一边告诉调用方"已经完整回滚"。
fn rollback(app: &App, snapshot: &Snapshot, cause: RpcError) -> RpcError {
    if snapshot.restore(app) {
        return cause;
    }
    RpcError::new(
        cause.code,
        format!("{}（并且旧凭据未能完整恢复，请重新登录）", cause.message),
    )
    .with_data(json!({ "kind": "rollback_incomplete", "cause": cause.data }))
}

/// 写完凭据后的行更新：状态置 valid、刷新到期时间、版本 +1。
fn finish_row(
    app: &App,
    account_id: &str,
    kind: &str,
    status: &str,
    expires: Option<&str>,
    error: Option<&str>,
) -> RpcResult<AuthorizationRecord> {
    let mut row = app
        .with_db(|db| db.find_authorization(account_id, kind))?
        .ok_or_else(|| RpcError::internal(format!("授权行不存在：{account_id}/{kind}")))?;
    row.status = status.to_string();
    row.token_expires_at = expires.map(str::to_string).or(row.token_expires_at);
    row.key_migration_state = KEY_STATE_READY.to_string();
    row.last_error = error.map(str::to_string);
    // `replace_authorization` 会把 version 推进一格：在途的旧快照就此作废。
    app.with_db(|db| db.replace_authorization(&row))?;
    app.with_db(|db| db.find_authorization(account_id, kind))?
        .ok_or_else(|| RpcError::internal("授权行写入后读不到"))
}

// ---------------------------------------------------------------------------
// 读取
// ---------------------------------------------------------------------------

/// 读某一份授权的 access。迁移没完成时退回旧键（G4）。
pub fn read_access(app: &App, account: &AccountRecord, kind: &str) -> RpcResult<Option<String>> {
    let row = app.with_db(|db| db.find_authorization(&account.id, kind))?;
    let keys = keys_for(&account.id, kind);
    let legacy = ids::legacy_keys(&account.uid);
    let mut order: Vec<String> = Vec::new();
    match &row {
        Some(row) if row.credentials_migrated() => {
            order.push(keys.access.clone());
            order.push(legacy.0);
        }
        Some(_) => {
            order.push(legacy.0);
            order.push(keys.access.clone());
        }
        None => order.push(legacy.0),
    }
    for key in order {
        match app.secure.get(&key) {
            Ok(bytes) => {
                let token =
                    String::from_utf8(bytes).map_err(|_| RpcError::internal("凭据不是 UTF-8"))?;
                if !token.trim().is_empty() {
                    return Ok(Some(token));
                }
            }
            Err(err) if err.kind == SecretErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(None)
}

/// 读自有授权的 refresh。**没有 magpie 分支**：借来的授权不刷新。
pub fn read_own_refresh(app: &App, account: &AccountRecord) -> RpcResult<Option<String>> {
    let row = app.with_db(|db| db.find_authorization(&account.id, ids::KIND_OWN))?;
    let keys = own_keys(&account.id);
    let legacy = ids::legacy_keys(&account.uid);
    let refresh_key = keys.refresh.expect("own 一定有 refresh 键");
    let order = match &row {
        Some(row) if row.credentials_migrated() => vec![refresh_key, legacy.1],
        _ => vec![legacy.1, refresh_key],
    };
    for key in order {
        match app.secure.get(&key) {
            Ok(bytes) => return Ok(Some(String::from_utf8_lossy(&bytes).to_string())),
            Err(err) if err.kind == SecretErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// 兼容镜像
// ---------------------------------------------------------------------------

pub struct MirrorReport {
    pub error: Option<String>,
}

/// 把账号的兼容镜像（`accounts.*` 四个字段 + 旧别名键）同步到**同一份授权**。
///
/// 选谁：首选来源里可用的那份 → 任意可用的那份 → 第一行。这样镜像与
/// `auth_preferred_source` 的语义一致，也不会因为两份都失效就停止同步。
pub fn sync_mirror(app: &App, account_id: &str) -> RpcResult<MirrorReport> {
    let rows = app.with_db(|db| db.list_authorizations(account_id))?;
    if rows.is_empty() {
        return Ok(MirrorReport { error: None });
    }
    let preferred = broker::preferred_source(app);
    let usable = |row: &AuthorizationRecord| {
        !["invalid", "expired", "denied"].contains(&row.status.as_str())
    };
    let chosen = rows
        .iter()
        .find(|row| row.kind == preferred && usable(row))
        .or_else(|| rows.iter().find(|row| usable(row)))
        .unwrap_or(&rows[0])
        .clone();

    // 1) 账号镜像列：四件套 + 来源授权 ID（G6：镜像必须可追溯）
    app.with_db(|db| {
        db.set_account_mirror(
            account_id,
            &chosen.id,
            &chosen.kind,
            &chosen.status,
            chosen.token_expires_at.as_deref(),
            Some(&chosen.access_ref),
        )
    })?;

    // 2) 旧别名键：让 PR #65 的二进制仍能读到"当前有效的那份"
    let mut error = None;
    let uid = app
        .with_db(|db| db.get_account(account_id))?
        .map(|account| account.uid)
        .unwrap_or_default();
    match read_by_row(app, &chosen) {
        Ok(Some(access)) => {
            let (legacy_access, legacy_refresh) = ids::legacy_keys(&uid);
            if let Err(err) = app.secure.set(&legacy_access, access.as_bytes()) {
                error = Some(format!(
                    "旧别名写入失败：{}",
                    crate::rpc::sanitize(&err.message)
                ));
            }
            if chosen.kind == ids::KIND_OWN {
                match read_own_refresh_by_row(app, &chosen) {
                    Ok(Some(refresh)) => {
                        if let Err(err) = app.secure.set(&legacy_refresh, refresh.as_bytes()) {
                            error = Some(format!(
                                "旧别名刷新令牌写入失败：{}",
                                crate::rpc::sanitize(&err.message)
                            ));
                        }
                    }
                    Ok(None) => {
                        let _ = app.secure.delete(&legacy_refresh);
                    }
                    Err(err) => {
                        error = Some(format!("旧别名刷新令牌同步失败：{}", err.message));
                    }
                }
            } else {
                // 镜像是借用授权：旧别名下不能留 refresh，否则就是"状态 magpie、凭据 own"的组合。
                match app.secure.delete(&legacy_refresh) {
                    Ok(_) => {}
                    Err(err) if err.kind == SecretErrorKind::NotFound => {}
                    Err(err) => {
                        error = Some(format!(
                            "旧的刷新令牌清理失败：{}",
                            crate::rpc::sanitize(&err.message)
                        ))
                    }
                }
            }
        }
        Ok(None) => {
            // 这份授权还没有凭据（例如刚建行），别名保持不动。
        }
        Err(err) => error = Some(format!("读取授权凭据失败：{}", err.message)),
    }

    if let Some(message) = &error {
        app.with_db(|db| db.set_authorization_error(&chosen.id, message))?;
    }

    Ok(MirrorReport { error })
}

fn read_by_row(app: &App, row: &AuthorizationRecord) -> RpcResult<Option<String>> {
    let key = row
        .access_key()
        .ok_or_else(|| RpcError::internal("授权记录的 access 引用不可用"))?;
    match app.secure.get(&key) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).to_string())),
        Err(err) if err.kind == SecretErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn read_own_refresh_by_row(app: &App, row: &AuthorizationRecord) -> RpcResult<Option<String>> {
    let Some(key) = row.refresh_key() else {
        return Ok(None);
    };
    match app.secure.get(&key) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).to_string())),
        Err(err) if err.kind == SecretErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

// ---------------------------------------------------------------------------
// 删除
// ---------------------------------------------------------------------------

pub struct DeleteReport {
    pub record_deleted: bool,
    pub removed_keys: Vec<String>,
    pub failed_keys: Vec<String>,
}

/// 删一份授权：只删它自己的键与记录，不碰另一份、不碰账号历史。
///
/// 删除失败时**保留记录**：记录留着才能重试，删掉就再也找不到那次失败了。
pub fn delete_authorization(app: &App, account_id: &str, kind: &str) -> RpcResult<DeleteReport> {
    if !ids::is_known_kind(kind) {
        return Err(RpcError::invalid_params(format!("授权类型非法：{kind}")));
    }
    let Some(row) = app.with_db(|db| db.find_authorization(account_id, kind))? else {
        return Err(RpcError::not_found(format!(
            "账号 {account_id} 没有 {kind} 授权"
        )));
    };

    let keys = keys_for(account_id, kind);
    let mut targets = vec![keys.access];
    if let Some(refresh) = keys.refresh {
        targets.push(refresh);
    }

    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for key in targets {
        match app.secure.delete(&key) {
            Ok(true) => removed.push(key),
            Ok(false) => {}
            Err(err) if err.kind == SecretErrorKind::NotFound => {}
            Err(err) => {
                crate::handlers::log_note(&format!(
                    "删除凭据失败 {key}：{}",
                    crate::rpc::sanitize(&err.message)
                ));
                failed.push(key);
            }
        }
    }

    let record_deleted = if failed.is_empty() {
        app.with_db(|db| db.delete_authorization(&row.id))?
    } else {
        app.with_db(|db| db.set_authorization_error(&row.id, "credential_delete_failed"))?;
        false
    };

    // 镜像可能指向刚被删掉的那份：重新选一次。
    if record_deleted {
        let _ = sync_mirror(app, account_id);
    }

    Ok(DeleteReport {
        record_deleted,
        removed_keys: removed,
        failed_keys: failed,
    })
}

/// 完整账号删除时清 5 条键：3 条新键 + 2 条旧别名。返回删不掉的键名（不含值）。
pub fn purge_account_keys(app: &App, account: &AccountRecord) -> Vec<String> {
    let own = own_keys(&account.id);
    let magpie = magpie_keys(&account.id);
    let (legacy_access, legacy_refresh) = ids::legacy_keys(&account.uid);
    let mut targets = vec![
        own.access.clone(),
        magpie.access,
        legacy_access,
        legacy_refresh,
    ];
    if let Some(refresh) = own.refresh {
        targets.push(refresh);
    }
    let mut failed = Vec::new();
    for key in targets {
        match app.secure.delete(&key) {
            Ok(_) => {}
            Err(err) if err.kind == SecretErrorKind::NotFound => {}
            Err(err) => {
                crate::handlers::log_note(&format!(
                    "清理凭据失败 {key}：{}",
                    crate::rpc::sanitize(&err.message)
                ));
                failed.push(key);
            }
        }
    }
    failed
}

/// 建账号时如果已经存在授权行（例如回填过），把行也带上：返回给 RPC 的账号视图。
pub fn account_view(app: &App, account: &AccountRecord) -> RpcResult<Value> {
    let mut value = json!(account);
    value["authorizations"] = json!(broker::describe(app, &account.id)?);
    value["authorizationSummary"] = broker::summary(app, account);
    Ok(value)
}

/// 让 `AccountInput` 只写"用户可编辑"的字段，授权相关一律由服务写（G6）。
pub fn account_input_without_auth(input: &AccountInput) -> AccountInput {
    AccountInput {
        platform: input.platform.clone(),
        uid: input.uid.clone(),
        nickname: input.nickname.clone(),
        email: input.email.clone(),
        group_id: input.group_id.clone(),
        checkin_enabled: input.checkin_enabled,
        travel_enabled: input.travel_enabled,
        ..Default::default()
    }
}
