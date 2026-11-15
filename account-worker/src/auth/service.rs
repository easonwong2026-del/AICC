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

/// 组装一行授权。
///
/// `refs` 给了就用它当引用（新登录会带一份 `localjson:v1:`），没给就按钥匙串默认
/// ——后者只服务 CRED-4 之前的老账号：那类行的凭据确实还在钥匙串里。
fn build_authorization_row(
    account_id: &str,
    kind: &str,
    uid: &str,
    refs: Option<(&str, Option<&str>)>,
) -> AuthorizationRecord {
    let keys = keys_for(account_id, kind);
    let now = crate::clock::now_rfc3339();
    let (access_ref, refresh_ref) = match refs {
        Some((access, refresh)) => (access.to_string(), refresh.map(str::to_string)),
        None => (
            ids::keychain_ref(&keys.access),
            keys.refresh.as_deref().map(ids::keychain_ref),
        ),
    };
    AuthorizationRecord {
        id: ids::auth_id(account_id, kind),
        account_id: account_id.to_string(),
        kind: kind.to_string(),
        access_ref,
        refresh_ref,
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
    }
}

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
    let row = build_authorization_row(account_id, kind, uid, None);
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

/// 一次自有授权写入的结果。
///
/// JSON 化之后，"凭据写在哪"必须跟着这一次写入走：新登录落一个随机 UUID 文件，
/// 引用要一起交给落库那一步。回滚也跟着走 —— 钥匙串按快照回滚，JSON 按
/// 「新建 → 删掉」/「复用 → 写回旧值」两种方式回滚。
pub struct OwnWrite {
    access_ref: String,
    refresh_ref: Option<String>,
    /// 这次写入是基于"库当时是什么样"的观察结果，落库时要用它做条件提交。
    /// 单独记下来是因为 `write_own` 和 `record_grant` 之间隔着一次 Magpie / 网络
    /// 之外的窗口，别的进程完全可能已经把这行改掉了。
    expectation: GrantExpectation,
    undo: OwnUndo,
}

/// 落库时的预期。**不能只是"再读一次库然后更新"**：那就是"谁最后写谁赢"，
/// 而最后的往往正是那个最慢返回的旧快照。
#[derive(Debug, Clone)]
pub enum GrantExpectation {
    /// 写凭据时这一行还不存在：只允许 `INSERT`，别人先建了就放弃。
    Absent,
    /// 写凭据时看到的是这一行：`UPDATE` 必须命中 `version + access_ref`。
    Present { version: i64, access_ref: String },
}

enum OwnUndo {
    Keychain(Snapshot),
    /// 新建的凭据文件：回滚 = 删掉它。之前（如果有行）还指着别的引用。
    Created {
        secret_id: String,
    },
    /// 复用了已有文件：回滚 = 把旧令牌写回去，带 revision 检查，不会盖掉并发写入。
    Replaced {
        secret_id: String,
        revision: u64,
        previous: crate::credentials::localjson::Credentials,
        previous_expires: Option<String>,
    },
}

impl OwnWrite {
    pub fn access_ref(&self) -> &str {
        &self.access_ref
    }

    pub fn refresh_ref(&self) -> Option<&str> {
        self.refresh_ref.as_deref()
    }

    pub fn expectation(&self) -> &GrantExpectation {
        &self.expectation
    }

    /// 竞争失败（`superseded`）时用：只丢掉**这次新建、没有任何行引用**的文件。
    ///
    /// 复用已有文件的那次写入**刻意不回滚** —— 那是刚拿到的有效令牌，回滚只会把账号
    /// 退回到服务端已经作废的旧值，而且那个文件本来就归这条授权所有。
    pub fn discard_unreferenced(&self, app: &App) {
        let secret_id = match &self.undo {
            OwnUndo::Created { secret_id } | OwnUndo::Replaced { secret_id, .. } => secret_id,
            // 钥匙串路径不在这里处理：键名是按账号确定的，不存在"孤儿文件"，
            // 而且删掉可能正好是别人正在用的那份。
            OwnUndo::Keychain(_) => return,
        };
        let reference = match crate::credentials::reference::local_json_ref(secret_id) {
            Ok(reference) => reference,
            Err(_) => return,
        };
        let referenced = app
            .with_db(|db| db.count_authorizations_referencing(&reference))
            .unwrap_or(1);
        if referenced > 0 {
            // 还有行指着它（很可能是刚刚赢下竞争的那一份）：**不动**。
            return;
        }
        let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
        if let Err(err) = store.delete(secret_id) {
            crate::handlers::log_note(&format!(
                "竞争失败后清理未引用凭据失败（{}）：{}",
                err.kind.as_str(),
                crate::rpc::sanitize(&err.message)
            ));
        }
    }

    /// 回滚这次写入。返回 false = 没恢复干净，调用方必须留痕。
    pub fn restore(&self, app: &App) -> bool {
        let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
        match &self.undo {
            OwnUndo::Keychain(snapshot) => snapshot.restore(app),
            OwnUndo::Created { secret_id } => match store.delete(secret_id) {
                Ok(_) => true,
                Err(err) => {
                    crate::handlers::log_note(&format!(
                        "新凭据文件删不掉（{}）：{}",
                        err.kind.as_str(),
                        crate::rpc::sanitize(&err.message)
                    ));
                    false
                }
            },
            OwnUndo::Replaced {
                secret_id,
                revision,
                previous,
                previous_expires,
            } => {
                let expires = previous_expires.as_deref().and_then(parse_rfc3339);
                match store.replace_credentials(secret_id, *revision, previous.clone(), expires) {
                    Ok(_) => true,
                    Err(err) => {
                        crate::handlers::log_note(&format!(
                            "旧凭据写不回去（{}）：{}",
                            err.kind.as_str(),
                            crate::rpc::sanitize(&err.message)
                        ));
                        false
                    }
                }
            }
        }
    }
}

/// RFC3339 → UTC。解析不了就当没有到期时间（不猜）。
pub fn parse_rfc3339(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|at| at.with_timezone(&chrono::Utc))
}

/// 写自有授权的凭据，**不碰数据库**。
///
/// 拆出来的原因：登录时账号行还不存在，而授权行的外键要求它先存在。
/// 顺序必须是「写凭据 → 建账号行 → 记授权行」，中途任何一步失败都要能回滚凭据。
///
/// 落点只看这一行现在是什么，**不做隐式迁移**：
/// * **还没有行** → 新建一个 JSON 文件（CRED-3 的默认）；
/// * **已经是 `localjson:`** → 覆盖同一个文件（一份授权一个文件，不留孤儿）；
/// * **还是 `keychain:`**（CRED-4 之前的老账号）→ 维持钥匙串写入，
///   等用户显式迁移再换存储，不给迁移中的账号制造半新半旧的状态。
pub fn write_own(
    app: &App,
    account_id: &str,
    platform: &str,
    access: &str,
    refresh: Option<&str>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) -> RpcResult<OwnWrite> {
    let existing = app.with_db(|db| db.find_authorization(account_id, ids::KIND_OWN))?;

    if let Some(row) = existing.as_ref() {
        match row.storage_kind() {
            crate::storage::authz::STORAGE_KEYCHAIN => {
                let snapshot = write_own_keychain(app, account_id, access, refresh)?;
                return Ok(OwnWrite {
                    access_ref: row.access_ref.clone(),
                    refresh_ref: row.refresh_ref.clone(),
                    expectation: GrantExpectation::Present {
                        version: row.version,
                        access_ref: row.access_ref.clone(),
                    },
                    undo: OwnUndo::Keychain(snapshot),
                });
            }
            crate::storage::authz::STORAGE_LOCALJSON => {
                return write_own_json_in_place(app, account_id, row, access, refresh, expires_at);
            }
            other => {
                // own 行出现 magpie / unknown 引用 = 库被外力改脏或串号。
                // **不静默重建**：那会把"数据损坏"藏起来，还会顺手把引用指向一份新文件，
                // 让原本指错的那一份再也查不出来。要求显式处理（CRED-4 的一致性检查）。
                return Err(RpcError::new(
                    crate::rpc::E_SECURESTORE,
                    format!("自有授权的引用不可用（{other}），请先修复凭据状态再登录"),
                )
                .with_data(json!({ "kind": "credential_ref_unusable", "storage": other })));
            }
        }
    }

    // 还没有自有授权 = 这是"第一次把令牌写成明文"。
    // 用户没确认过风险之前，继续走钥匙串（与 #84 行为一致）——
    // 不能在产品默认明文落盘之后才补告知。
    if !crate::auth::consent::json_storage_enabled(app) {
        let snapshot = write_own_keychain(app, account_id, access, refresh)?;
        let keys = own_keys(account_id);
        return Ok(OwnWrite {
            access_ref: ids::keychain_ref(&keys.access),
            refresh_ref: keys.refresh.as_deref().map(ids::keychain_ref),
            expectation: GrantExpectation::Absent,
            undo: OwnUndo::Keychain(snapshot),
        });
    }

    write_own_json_new(app, account_id, platform, access, refresh, expires_at)
}

/// 新授权：建一个随机 UUID 文件。失败的话目标文件要么不存在、要么完整，没有半份。
fn write_own_json_new(
    app: &App,
    account_id: &str,
    platform: &str,
    access: &str,
    refresh: Option<&str>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) -> RpcResult<OwnWrite> {
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let mut draft = crate::credentials::localjson::SecretDraft::oauth(
        ids::auth_id(account_id, ids::KIND_OWN),
        platform,
        access,
    );
    if let Some(token) = refresh {
        draft = draft.with_refresh_token(token);
    }
    if let Some(at) = expires_at {
        draft = draft.with_expires_at(at);
    }
    let secret = store
        .create(draft)
        .map_err(|err| credential_store_error(err, "own/json"))?;
    let reference = crate::credentials::reference::local_json_ref(&secret.secret_id)
        .map_err(|err| RpcError::internal(err.message))?;
    Ok(OwnWrite {
        // 同一个文件、两个槽位：读 access 走 Access，读 refresh 走 Refresh。
        refresh_ref: refresh.map(|_| reference.clone()),
        access_ref: reference,
        expectation: GrantExpectation::Absent,
        undo: OwnUndo::Created {
            secret_id: secret.secret_id,
        },
    })
}

/// 已有 JSON 授权重新登录：覆盖同一个文件，access + refresh 一次替换。
///
/// 不复用「新建再删旧的」：那会在崩溃时留下一个没人引用的文件，而凭据文件
/// 只要还在盘上就还是有效令牌。覆盖写有 `revision` 校验，并发的刷新不会被盖掉。
fn write_own_json_in_place(
    app: &App,
    account_id: &str,
    row: &AuthorizationRecord,
    access: &str,
    refresh: Option<&str>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
) -> RpcResult<OwnWrite> {
    use crate::credentials::localjson::{Credentials, LocalJsonStore};

    let account = app
        .with_db(|db| db.get_account(account_id))?
        .ok_or_else(|| RpcError::internal("授权行存在但账号行不存在"))?;
    let secret_id = crate::auth::owned::secret_id_of(row)
        .map_err(|err| credential_store_error(err, "own/json"))?;
    // 覆盖写之前先确认这份文件确实属于这一行：否则一次错误的引用会把别人的令牌改写掉。
    let current = crate::auth::owned::load(app, &account, row)
        .map_err(|err| credential_store_error(err, "own/json"))?;
    let store = LocalJsonStore::for_paths(&app.paths);
    let credentials = match refresh {
        Some(token) => Credentials::oauth(access, token),
        // 服务端这次没给 refresh：和钥匙串路径一样，旧的作废，不留半套。
        None => Credentials::access_only(access),
    };
    let updated = store
        .replace_credentials(&secret_id, current.revision, credentials, expires_at)
        .map_err(|err| credential_store_error(err, "own/json"))?;
    Ok(OwnWrite {
        access_ref: row.access_ref.clone(),
        refresh_ref: refresh.map(|_| row.access_ref.clone()),
        expectation: GrantExpectation::Present {
            version: row.version,
            access_ref: row.access_ref.clone(),
        },
        undo: OwnUndo::Replaced {
            secret_id,
            revision: updated.revision,
            previous: current.credentials,
            previous_expires: current.expires_at,
        },
    })
}

/// 老路径：钥匙串写入（CRED-4 之前的老账号继续走这里）。
fn write_own_keychain(
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

/// 凭据层错误 → RPC 错误。保留分类，别把 `unsafe` / `conflict` 压成一句话。
fn credential_store_error(err: crate::credentials::CredentialError, stage: &str) -> RpcError {
    RpcError::new(
        crate::rpc::E_SECURESTORE,
        format!(
            "写入凭据失败（{stage}）：{}",
            crate::rpc::sanitize(&err.message)
        ),
    )
    .with_data(json!({ "kind": err.kind.as_str(), "stage": stage }))
}

/// 把一次成功授权的结论落到库里：**引用、状态、有效期、存储种类一次提交**。
///
/// 为什么必须是一条语句：`refs` 指向的凭据（JSON 文件 / 钥匙串键）是在这之前写好的。
/// 若先写引用再写状态，中途失败会留下「新凭据 + 旧状态」或「新状态 + 旧引用」——
/// 后者尤其糟，库会一直指着一个不是这次登录写出来的文件。`put_authorization`
/// 本身就是一条 UPSERT（SQLite 单语句即事务），所以只要把该改的字段一次填好。
///
/// 调用前账号行必须已经存在（授权行有外键）。
pub fn record_grant(
    app: &App,
    account_id: &str,
    uid: &str,
    kind: &str,
    expires_at: Option<&str>,
    refs: Option<(&str, Option<&str>)>,
    expectation: &GrantExpectation,
) -> RpcResult<Saved> {
    if !ids::is_known_kind(kind) {
        return Err(RpcError::invalid_params(format!("授权类型非法：{kind}")));
    }
    // 没有行时直接按这次写入的引用建行：不先落一行 `keychain:` 占位再改 ——
    // 那个中间态会被并发读路径看到，而且真的会去敲钥匙串。
    let existing = app.with_db(|db| db.find_authorization(account_id, kind))?;
    let mut row = match &existing {
        Some(row) => row.clone(),
        None => build_authorization_row(account_id, kind, uid, refs),
    };
    if let Some((access_ref, refresh_ref)) = refs {
        row.access_ref = access_ref.to_string();
        row.refresh_ref = refresh_ref.map(str::to_string);
    }
    row.status = "valid".to_string();
    row.token_expires_at = expires_at.map(str::to_string).or(row.token_expires_at);
    row.key_migration_state = KEY_STATE_READY.to_string();
    row.last_error = None;

    // 跨进程条件提交（`auth_write_guard` 只是进程内的 Mutex）：
    // * 写凭据时没有这一行 → `INSERT ... DO NOTHING`：另一个 Worker 先建了就放弃，
    //   调用方会把那份没人引用的文件删掉；
    // * 写凭据时有这一行 → `version + access_ref` 双条件 CAS：谁先把引用改掉，
    //   后到的旧快照就写不进去。
    //
    // 条件来自 `write_own` 当时的观察，**不是**这里重新读一次库 —— 重读只会把
    // "我看到的"替换成"现在是什么"，于是最后一次写入永远成功，CAS 等于没有。
    let applied = match expectation {
        GrantExpectation::Absent => app.with_db(|db| db.insert_authorization_if_absent(&row))?,
        GrantExpectation::Present {
            version,
            access_ref,
        } => app.with_db(|db| db.replace_authorization_cas(&row, *version, access_ref))?,
    };
    if !applied {
        return Err(crate::handlers::superseded("own_authorization_changed"));
    }

    let degraded = sync_mirror(app, account_id).ok().and_then(|r| r.error);
    let fresh = app
        .with_db(|db| db.find_authorization(account_id, kind))?
        .ok_or_else(|| RpcError::internal("授权行写入后读不到"))?;
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

    if let Some(row) = &row {
        match row.storage_kind() {
            // JSON 行：同一个文件里的 Refresh 槽位，**先验归属**。
            // 读不到就是没有，绝不退到钥匙串 —— 退过去会读到旧值，甚至再弹一次授权框。
            crate::storage::authz::STORAGE_LOCALJSON => {
                return match crate::auth::owned::token(
                    app,
                    account,
                    row,
                    crate::credentials::reference::CredentialSlot::Refresh,
                ) {
                    Ok(token) => Ok(Some(token)),
                    Err(err) if err.kind == crate::credentials::CredentialErrorKind::NotFound => {
                        Ok(None)
                    }
                    Err(err) => Err(err.into()),
                };
            }
            // 只有**明确**写着 keychain 的老行才走钥匙串。未知 / 错种类的引用必须显式
            // 失败：悄悄去读钥匙串会把"库坏了"变成"偶发弹窗 + 读到一份不是它的令牌"。
            crate::storage::authz::STORAGE_KEYCHAIN => {}
            other => {
                return Err(RpcError::new(
                    crate::rpc::E_SECURESTORE,
                    format!("自有授权的引用不可用（{other}），请重新登录或检查凭据"),
                )
                .with_data(json!({ "kind": "credential_ref_unusable", "storage": other })));
            }
        }
    }

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
    /// 删掉的本地凭据文件 ID（PR-CRED-3）。钥匙串路径永远为空。
    pub removed_secrets: Vec<String>,
    pub failed_keys: Vec<String>,
    /// 没删掉的本地凭据文件 ID。
    pub failed_secrets: Vec<String>,
    /// 凭据归属校验失败时的错误码（`conflict` / `account_missing`）。
    /// 出现它意味着**一个字节都没删**：那可能是别人的凭据。
    pub secret_error: Option<String>,
    /// 凭据已经清掉、但授权行没删掉时的原因码（可重试）。
    pub record_error: Option<String>,
    /// 删完之后兼容镜像没同步成功的原因（记录已经删掉了，只提示不阻断）。
    pub mirror_error: Option<String>,
}

/// 删一份授权：只删它自己的键与记录，不碰另一份、不碰账号历史。
///
/// 删的是**最后一份**时，旧别名键（`workbuddy/<uid>/access|refresh`）也要一起清：
/// 那是 PR #65 及更早版本的读取路径，留着就是一份没有主人的令牌，下次这个 uid 重新
/// 导入时会读到上一轮的凭据。**动态引用（magpie / localjson）不清别名**：那两种来源
/// 从来没往别名里写过东西，而 Magpie 的别名是 CRED-4 迁移的回滚依据。
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
    let account = app.with_db(|db| db.get_account(account_id))?;

    // 三类引用，三种清理方式：
    // * `magpie:v1:`   → 令牌在 Magpie 文件里，**什么都不删**（也不碰旧别名，回滚要用）；
    // * `localjson:v1:` → 只删 AICC 自己的那个凭据文件，**不碰钥匙串**；
    // * `keychain:`     → 老规则：自己的键 + 最后一份时的旧别名。
    let storage = row.storage_kind();
    let dynamic = storage == crate::storage::authz::STORAGE_MAGPIE;
    let local_json = storage == crate::storage::authz::STORAGE_LOCALJSON;

    let mut removed_secrets: Vec<String> = Vec::new();
    let failed_secrets: Vec<String> = Vec::new();
    let mut secret_error: Option<String> = None;
    if local_json {
        // 删别人的文件 = 直接毁掉另一份有效凭据，所以**先验归属**再删。
        // 归属不符一律拒绝（不删文件、也不删这一行），把问题留给一致性检查去暴露。
        match &account {
            Some(account) => {
                let secret_id = crate::auth::owned::secret_id_of(&row)
                    .map_err(|err| RpcError::internal(err.message))?;
                match crate::auth::owned::delete(app, account, &row) {
                    // `delete` 对"文件已经不在"也返回 Ok（幂等），所以统一记成已删除。
                    Ok(()) => removed_secrets.push(secret_id),
                    Err(err) => {
                        crate::handlers::log_note(&format!(
                            "拒绝删除凭据文件（{}）：{}",
                            err.kind.as_str(),
                            crate::rpc::sanitize(&err.message)
                        ));
                        secret_error = Some(err.kind.as_str().to_string());
                    }
                }
            }
            None => secret_error = Some("account_missing".to_string()),
        }
    }

    let mut targets: Vec<String> = Vec::new();
    if !dynamic && !local_json {
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

    // 顺序固定为「先凭据、后记录」，两个方向都有代价，这里选可重试的那个：
    //
    // * 凭据先删：若记录删除失败，行会指着一份不存在的文件 —— 读的时候是 `not_found`，
    //   界面提示重新登录；而**删除本身可以原样重试**（删文件、删行都幂等）。
    // * 记录先删：文件名是随机 UUID，行一没就再也定位不到那个文件，只能扫目录找孤儿，
    //   而孤儿里躺着的还是有效令牌。
    //
    // 所以不是"顺手这么写"，而是刻意选的可恢复性更差的那一侧留给不存在的状态。
    let mut record_error: Option<String> = None;
    let record_deleted = if failed.is_empty() && failed_secrets.is_empty() && secret_error.is_none()
    {
        match app.with_db(|db| db.delete_authorization(&row.id)) {
            Ok(deleted) => deleted,
            Err(err) => {
                // 凭据已经删了，行因数据库错误留下：不假装成功，也不掩盖。
                crate::handlers::log_note(&format!(
                    "授权记录删除失败（{}）：{}",
                    row.id,
                    crate::rpc::sanitize(&err.message)
                ));
                record_error = Some("record_delete_failed".to_string());
                false
            }
        }
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
        removed_secrets,
        failed_keys: failed,
        failed_secrets,
        secret_error,
        record_error,
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

/// 账号删除要删的本地凭据文件 ID（PR-CRED-3）。动态 Magpie 行没有文件。
#[derive(Debug)]
pub struct SecretPlan {
    /// 确认属于这个账号、可以删的文件 ID（已去重）。
    pub owned: Vec<String>,
    /// 归属校验没过的行引用的文件 ID（**不会删**）。
    pub refused: Vec<String>,
}

/// 整账号删除要删的本地凭据文件。
///
/// 只做 UUID 去重是不够的：引用可能指错（迁移错、并发覆盖、库损坏），那样"删账号"
/// 就会顺手把**另一个账号**的有效凭据删掉。所以逐行验归属，不通过的单独列出来，
/// 让调用方停下来报错，而不是静默跳过。
pub fn account_secret_plan(
    app: &App,
    account: &AccountRecord,
    rows: &[AuthorizationRecord],
) -> SecretPlan {
    let mut owned = Vec::new();
    let mut refused = Vec::new();
    for row in rows {
        if row.storage_kind() != crate::storage::authz::STORAGE_LOCALJSON {
            continue;
        }
        let Ok(secret_id) = crate::auth::owned::secret_id_of(row) else {
            refused.push(row.access_ref.clone());
            continue;
        };
        match crate::auth::owned::load(app, account, row) {
            // 文件已经不在：删除是幂等的，算"有它但不用再删"。
            Err(err) if err.kind == crate::credentials::CredentialErrorKind::NotFound => {
                owned.push(secret_id)
            }
            Err(_) => refused.push(secret_id),
            Ok(_) => owned.push(secret_id),
        }
    }
    owned.sort();
    owned.dedup();
    refused.sort();
    refused.dedup();
    SecretPlan { owned, refused }
}

/// 删除本地凭据文件，返回删不掉的 ID。文件不存在算成功（幂等）。
pub fn purge_secrets(app: &App, ids: &[String]) -> Vec<String> {
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let mut failed = Vec::new();
    for id in ids {
        match store.delete(id) {
            Ok(_) => {}
            Err(err) => {
                crate::handlers::log_note(&format!(
                    "删除凭据文件失败 {id}：{}",
                    crate::rpc::sanitize(&err.message)
                ));
                failed.push(id.clone());
            }
        }
    }
    failed
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
