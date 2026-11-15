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

/// 调用方手上那份 Magpie 授权快照。给了它，这次写入就必须建立在
/// "这一行还是我读到的样子"之上（跨进程 CAS）。
#[derive(Debug, Clone)]
pub struct MagpieGuard {
    pub version: i64,
    pub access_ref: String,
}

impl MagpieGuard {
    pub fn of(row: &AuthorizationRecord) -> Self {
        Self {
            version: row.version,
            access_ref: row.access_ref.clone(),
        }
    }
}

/// 登记借用授权：**只写引用，不复制任何令牌**（PR-CRED-2 / PRD §3.2）。
///
/// 旧实现会把 access 复制进 AICC 钥匙串，于是每次重建二进制都可能弹一次授权框，
/// 并且 Magpie 刷新之后 AICC 那份立刻过期。现在只落一条 `magpie:v1:<source_key>`
/// 引用，用的时候按引用现读当前文件。
///
/// `source_key` 是硬要求：没有它就无法确定"这个账号对应文件里哪条登录态"，
/// 宁可让调用方报错，也不猜（PRD §3.3）。
pub fn save_magpie(
    app: &App,
    account_id: &str,
    uid: &str,
    source_key: &str,
    expires_at: Option<&str>,
) -> RpcResult<Saved> {
    save_magpie_guarded(app, account_id, uid, source_key, expires_at, None)
}

/// 同 [`save_magpie`]，但可以带一份"我读到的是这个版本"的守卫。
///
/// resync / 自动任务都是"先读行、再读 Magpie 文件、最后写"的三步流程；
/// 中间用户可能重新导入、另一个 Worker 可能刚 relink 过。`auth_write_guard`
/// 只是**进程内** Mutex，挡不住跨进程的迟到写入，所以写之前必须用
/// `version + access_ref` 做一次 CAS；对不上就 `superseded`，不重试、不盲写。
pub fn save_magpie_guarded(
    app: &App,
    account_id: &str,
    uid: &str,
    source_key: &str,
    expires_at: Option<&str>,
    guard: Option<MagpieGuard>,
) -> RpcResult<Saved> {
    if source_key.trim().is_empty() {
        return Err(
            RpcError::new(crate::rpc::E_AUTH, "缺少 Magpie 来源键，无法建立动态关联")
                .with_data(json!({ "kind": "magpie_source_key_missing" })),
        );
    }
    let reference = crate::credentials::reference::magpie_ref(source_key)
        .map_err(|err| RpcError::new(crate::rpc::E_AUTH, err.message))?;

    let mut row = ensure_row(app, account_id, ids::KIND_MAGPIE, uid)?;
    // 有守卫就按守卫的版本写；没有就按刚读到的这一份写（至少能挡住本函数执行期间的变更）。
    let expectation = guard.unwrap_or_else(|| MagpieGuard::of(&row));
    row.access_ref = reference;
    // 借用授权永远不持有刷新令牌，引用也一起清掉（表上的 CHECK 同样不允许）。
    row.refresh_ref = None;
    row.source_key = Some(source_key.to_string());
    row.token_expires_at = expires_at.map(str::to_string).or(row.token_expires_at);
    row.status = "valid".to_string();
    row.key_migration_state = KEY_STATE_READY.to_string();
    row.last_error = None;
    let applied = app.with_db(|db| {
        db.replace_authorization_cas(&row, expectation.version, &expectation.access_ref)
    })?;
    if !applied {
        // 有人在这中间改了这一行（重新导入 / relink / 另一个 Worker）：
        // 我这份结论已经过时，放弃，绝不覆盖。
        return Err(crate::handlers::superseded("magpie_source_changed"));
    }

    let degraded = sync_mirror(app, account_id).ok().and_then(|r| r.error);
    let fresh = app
        .with_db(|db| db.find_authorization(account_id, ids::KIND_MAGPIE))?
        .unwrap_or(row);
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

/// 读某一份授权的 access。**按引用分派**，不再假设钥匙串。
///
/// 只有"根本没有授权行"时才退到旧别名键 —— 那是最早期的安装（v5 之前）。
/// 有授权行就一定按它的 `access_ref` 读：magpie 现读文件、localjson 读本地文件、
/// keychain 才碰钥匙串。这样 `read_access` 与 Broker 的结论永远一致。
pub fn read_access(app: &App, account: &AccountRecord, kind: &str) -> RpcResult<Option<String>> {
    let row = app.with_db(|db| db.find_authorization(&account.id, kind))?;
    let Some(row) = row else {
        // 无授权行：只可能是旧库，读一次旧别名。
        let (legacy_access, _) = ids::legacy_keys(&account.uid);
        return match app.secure.get(&legacy_access) {
            Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).to_string())),
            Err(err) if err.kind == SecretErrorKind::NotFound => Ok(None),
            Err(err) => Err(err.into()),
        };
    };
    match broker::access_for(app, account, &row) {
        Ok(token) => Ok(Some(token)),
        // "没有"与"读不了"必须分开：这里把 NotFound 折成 None 是调用方的语义，
        // 其余分类原样上抛，避免把拒绝 / 串号 / 文件坏掉当成"没凭据"。
        Err(err) if err.kind == crate::credentials::CredentialErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
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
        // 一份授权都不剩：镜像列必须清掉。留着旧值会让界面显示"来源 magpie、状态 valid"，
        // 而实际上一份可用授权都没有 —— 镜像的全部意义就是可追溯。
        app.with_db(|db| db.clear_account_mirror(account_id))?;
        return Ok(MirrorReport { error: None });
    }
    let preferred = broker::preferred_source(app);
    // 用 Broker 的判据，不要在这里另写一份：动态 Magpie 引用的有效性在文件里，
    // 库里的 status 是旧快照。各写一份的话，镜像会一直跳过"其实已经刷新"的那一份。
    let usable = |row: &AuthorizationRecord| broker::is_usable(row);
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

    // 2) 旧别名键：**只给仍然指向钥匙串的授权写**。
    //
    // magpie / localjson 引用如果也写别名，就等于把令牌又复制进钥匙串 ——
    // 那正是 PRD §8 点名的隐性钥匙串写入，迁移完还会偶发授权弹窗。
    // 代价是 PR #65 的二进制读不到借用授权的当前值；回退要用迁移前的数据库备份
    // （PRD §7.4 已经接受这一点）。
    let mut error = None;
    if chosen.storage_kind() == crate::storage::authz::STORAGE_KEYCHAIN {
        error = mirror_legacy_alias(app, account_id, &chosen)?;
    }

    if let Some(message) = &error {
        app.with_db(|db| db.set_authorization_error(&chosen.id, message))?;
    }

    Ok(MirrorReport { error })
}

/// 把镜像所选的那份写进旧别名键（PR #65 的二进制读这里）。
///
/// 只在 `storage_kind == keychain` 时调用：引用已经不是钥匙串时，任何写入都会把
/// 令牌重新复制回钥匙串。返回失败原因（已脱敏），凭据本身不受影响。
fn mirror_legacy_alias(
    app: &App,
    account_id: &str,
    chosen: &AuthorizationRecord,
) -> RpcResult<Option<String>> {
    let mut error = None;
    let uid = app
        .with_db(|db| db.get_account(account_id))?
        .map(|account| account.uid)
        .unwrap_or_default();
    match read_by_row(app, chosen) {
        Ok(Some(access)) => {
            let (legacy_access, legacy_refresh) = ids::legacy_keys(&uid);
            if let Err(err) = app.secure.set(&legacy_access, access.as_bytes()) {
                error = Some(format!(
                    "旧别名写入失败：{}",
                    crate::rpc::sanitize(&err.message)
                ));
            }
            if chosen.kind == ids::KIND_OWN {
                match read_own_refresh_by_row(app, chosen) {
                    Ok(Some(refresh)) => {
                        if let Err(err) = app.secure.set(&legacy_refresh, refresh.as_bytes()) {
                            error = Some(format!(
                                "旧别名刷新令牌写入失败：{}",
                                crate::rpc::sanitize(&err.message)
                            ));
                        }
                    }
                    Ok(None) => {
                        // 这份 own 授权没有 refresh：旧别名下也不能留旧的，
                        // 否则库里"无 refresh"与钥匙串里"有 refresh"长期不一致。
                        if let Err(err) = app.secure.delete(&legacy_refresh) {
                            if err.kind != SecretErrorKind::NotFound {
                                error = Some(format!(
                                    "旧的刷新令牌清理失败：{}",
                                    crate::rpc::sanitize(&err.message)
                                ));
                            }
                        }
                    }
                    Err(err) => {
                        error = Some(format!("旧别名刷新令牌同步失败：{}", err.message));
                    }
                }
            } else {
                // 镜像是借用授权：旧别名下不能留 refresh，否则就是"状态 magpie、凭据 own"。
                if let Err(err) = app.secure.delete(&legacy_refresh) {
                    if err.kind != SecretErrorKind::NotFound {
                        error = Some(format!(
                            "旧的刷新令牌清理失败：{}",
                            crate::rpc::sanitize(&err.message)
                        ));
                    }
                }
            }
        }
        Ok(None) => {
            // 这份授权还没有凭据（例如刚建行），别名保持不动。
        }
        Err(err) => error = Some(format!("读取授权凭据失败：{}", err.message)),
    }
    Ok(error)
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
    /// 删完之后兼容镜像没同步成功的原因（记录已经删掉了，只提示不阻断）。
    pub mirror_error: Option<String>,
}

/// 删一份授权：只删它自己的键与记录，不碰另一份、不碰账号历史。
///
/// 删的是**最后一份**时，旧别名键（`workbuddy/<uid>/access|refresh`）也要一起清：
/// 那是 PR #65 及更早版本的读取路径，留着就是一份没有主人的令牌，下次这个 uid 重新
/// 导入时会读到上一轮的凭据。
///
/// 删除失败时**保留记录**：记录留着才能重试，删掉就再也找不到那次失败了。
/// 也正因如此，别名键必须在删记录**之前**删 —— 记录没了，重试就没有抓手了。
pub fn delete_authorization(app: &App, account_id: &str, kind: &str) -> RpcResult<DeleteReport> {
    if !ids::is_known_kind(kind) {
        return Err(RpcError::invalid_params(format!("授权类型非法：{kind}")));
    }
    let Some(row) = app.with_db(|db| db.find_authorization(account_id, kind))? else {
        return Err(RpcError::not_found(format!(
            "账号 {account_id} 没有 {kind} 授权"
        )));
    };
    let account = app.with_db(|db| db.get_account(account_id))?;

    // 动态 Magpie 引用**没有任何 AICC 凭据**要删：令牌在 Magpie 自己的文件里，
    // 我们只持有一条引用。这类删除只清授权行与镜像，**不碰钥匙串**——
    // 既包括它自己的键，也包括当年为回滚保留的旧别名（清理是后续的显式用户操作）。
    let dynamic = row.storage_kind() == crate::storage::authz::STORAGE_MAGPIE;

    let mut targets: Vec<String> = Vec::new();
    if !dynamic {
        let keys = keys_for(account_id, kind);
        targets.push(keys.access);
        if let Some(refresh) = keys.refresh {
            targets.push(refresh);
        }

        // 最后一份？是的话旧别名键属于这次删除的范围。
        let is_last = app
            .with_db(|db| db.list_authorizations(account_id))?
            .iter()
            .all(|other| other.id == row.id);
        if is_last {
            if let Some(account) = &account {
                let (legacy_access, legacy_refresh) = ids::legacy_keys(&account.uid);
                targets.push(legacy_access);
                targets.push(legacy_refresh);
            }
        }
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

    // 镜像可能指向刚被删掉的那份：重新选一次（没有授权了就连镜像一起清空）。
    let mut mirror_error = None;
    if record_deleted {
        if let Ok(report) = sync_mirror(app, account_id) {
            mirror_error = report.error;
        }
    }

    Ok(DeleteReport {
        record_deleted,
        removed_keys: removed,
        failed_keys: failed,
        mirror_error,
    })
}

/// 改完全局首选来源后，把所有账号的兼容镜像重新对齐一遍。
///
/// 不做这件事的后果很具体：界面上写着"首选 own"，PR #65 的二进制还在用旧别名键里的
/// magpie 令牌 —— 两套读法给出两个答案。返回每个没同步成功的账号及原因（已脱敏）。
pub fn resync_all_mirrors(app: &App) -> RpcResult<Vec<(String, String)>> {
    let accounts = app.with_db(|db| db.list_accounts(&Default::default()))?;
    let mut failures = Vec::new();
    for account in accounts {
        match sync_mirror(app, &account.id) {
            Ok(report) => {
                if let Some(error) = report.error {
                    failures.push((account.id, error));
                }
            }
            Err(err) => failures.push((account.id, err.message)),
        }
    }
    Ok(failures)
}

/// 账号删除要清的钥匙串键。**按实际引用算，不按"账号大概有哪几种授权"猜**。
///
/// * `keychain:` 引用：只清它自己记录的那几个键（access + 可选 refresh）；
/// * 动态引用（`magpie:v1:` / `localjson:v1:`）：**一个键都不产生** —— 令牌不在我们这里；
/// * 旧别名键：只在"确实还有钥匙串凭据"或"连授权行都没有"（v6 之前的安装，
///   别名是唯一凭据）时才清。全是动态引用的账号保留别名，那是回滚用的。
///
/// 结果排序去重，便于日志与测试比较。
pub fn account_purge_plan(rows: &[AuthorizationRecord], account: &AccountRecord) -> Vec<String> {
    let has_keychain = rows
        .iter()
        .any(|row| row.storage_kind() == crate::storage::authz::STORAGE_KEYCHAIN);
    let mut targets = Vec::new();
    for row in rows {
        if row.storage_kind() != crate::storage::authz::STORAGE_KEYCHAIN {
            continue;
        }
        if let Some(key) = row.access_key() {
            targets.push(key);
        }
        if let Some(key) = row.refresh_key() {
            targets.push(key);
        }
    }
    if rows.is_empty() || has_keychain {
        let (legacy_access, legacy_refresh) = ids::legacy_keys(&account.uid);
        targets.push(legacy_access);
        targets.push(legacy_refresh);
    }
    targets.sort();
    targets.dedup();
    targets
}

/// 执行清理计划，返回删不掉的键名（不含值）。
pub fn purge_keys(app: &App, keys: &[String]) -> Vec<String> {
    let mut failed = Vec::new();
    for key in keys {
        match app.secure.delete(key) {
            Ok(_) => {}
            Err(err) if err.kind == SecretErrorKind::NotFound => {}
            Err(err) => {
                crate::handlers::log_note(&format!(
                    "清理凭据失败 {key}：{}",
                    crate::rpc::sanitize(&err.message)
                ));
                failed.push(key.clone());
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
