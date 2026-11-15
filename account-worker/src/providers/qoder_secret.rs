//! Qoder 自有凭据的**落点决策、归属校验与回滚**（PR-CRED-3b）。
//!
//! Qoder 有两条自有授权来源，各占 `qoder_authorizations` 的一行：
//!
//! | 来源 | 凭据类型 | 钥匙串键（旧） | JSON 文件（新） |
//! | --- | --- | --- | --- |
//! | `own_browser` | OAuth（access + 可选 refresh） | `qoder/<id>/own/access`、`.../refresh` | 一个独立文件 |
//! | `own_pat` | PAT（只有 access） | `qoder/<id>/pat/token` | **另一个**独立文件 |
//!
//! 两条来源的凭据必须**互不覆盖**：删掉 PAT 不该影响设备授权，反之亦然。
//! 所以它们各自一个文件（`LocalJsonStore` 一份授权一个文件），各自一个引用。
//!
//! # 落点只看这一行现在是什么
//!
//! | 行的状态 | 新令牌写到哪里 |
//! | --- | --- |
//! | 还没有这一行，且用户**没同意**明文 | 钥匙串（与 CRED-3b 之前完全一致） |
//! | 还没有这一行，且已同意明文 | 新建 JSON 文件 |
//! | 已是 `localjson:v1:` | 覆盖同一个文件（`revision` 校验） |
//! | 仍是 `keychain:` | 继续写钥匙串，**不隐式迁移**（迁移是 CRED-4 的显式动作） |
//! | 未知前缀 / magpie / 引用指向别人的键 | 显式拒绝，不猜、不回退 |
//!
//! # 归属校验是读路径的一部分
//!
//! `localjson:v1:<uuid>` 只是一个文件名。文件里写着 `authorizationId` /
//! `platform` / `source` / `credentialKind`，**每次读、写、删之前都要与授权行
//! 逐项比对**。引用被写错（迁移出错、库损坏、并发覆盖）时，没有这道校验就会把
//! 别人的令牌当成自己的返回，或者删掉别人的凭据。
//!
//! `platform` 用的是**实际 edition**（`qoder_cn` / `qoder_global`）：CN 与
//! International 是两个产品，同一个 UID 在两边是两行账号，凭据也不能互通。

use chrono::{DateTime, Utc};

use crate::app::App;
use crate::auth::consent::json_storage_enabled;
use crate::auth::service::Snapshot;
use crate::credentials::localjson::{
    CredentialKind, Credentials, LocalJsonStore, SecretDraft, SecretSource, StoredSecret,
};
use crate::credentials::reference::{self, CredentialRef, CredentialSlot};
use crate::credentials::{CredentialError, CredentialErrorKind};
use crate::providers::qoder_edition::QoderEdition;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::qoder_accounts::{
    qoder_auth_id, QoderAccountRecord, QoderAuthExpectation, QoderAuthRecord, QoderLifecycle,
    SOURCE_OWN_BROWSER, SOURCE_OWN_PAT,
};

/// 自有来源的两种存储形态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Storage {
    /// 旧钥匙串（CRED-4 之前写的行，或用户还没同意明文）。
    Keychain,
    /// AICC 自有 JSON 文件。
    Json,
}

impl Storage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keychain => "keychain",
            Self::Json => "localjson",
        }
    }
}

/// Qoder edition → 凭据文件里的 `platform`。与 `qoder_accounts.edition` 同一套字面值。
pub fn platform_of(edition: QoderEdition) -> &'static str {
    edition.as_str()
}

/// 授权行 ID。`qoder_auth_id` 是确定性哈希，所以**建文件之前**就能算出文件里的
/// `authorizationId` —— 不必等数据库那一行先存在（首次授权的顺序刚好相反）。
pub fn authorization_id(account_id: &str, source: &str) -> String {
    qoder_auth_id(account_id, source)
}

/// 该来源的凭据类型。只有 PAT 是 `Pat`，其它（含浏览器设备授权）都是 `Oauth`。
pub fn kind_of(source: &str) -> CredentialKind {
    if source == SOURCE_OWN_PAT {
        CredentialKind::Pat
    } else {
        CredentialKind::Oauth
    }
}

/// 该来源在钥匙串里**全部**可能的键。
///
/// `own_browser` 是两条（access + refresh）：老代码只把 access 的引用写进
/// `secret_ref`，refresh 的键由账号 id 派生，所以删除时必须两条一起清。
pub fn keychain_keys(account_id: &str, source: &str) -> Vec<String> {
    if source == SOURCE_OWN_PAT {
        vec![crate::providers::qoder_pat::pat_key(account_id)]
    } else {
        vec![
            crate::providers::qoder_auth::access_key(account_id),
            crate::providers::qoder_auth::refresh_key(account_id),
        ]
    }
}

/// 该来源的"主"钥匙串键：引用里记的就是这一条。
fn keychain_primary_key(account_id: &str, source: &str) -> String {
    if source == SOURCE_OWN_PAT {
        crate::providers::qoder_pat::pat_key(account_id)
    } else {
        crate::providers::qoder_auth::access_key(account_id)
    }
}

/// `keychain:qoder/<id>/<own|pat>/...` 形状的引用串。
pub fn keychain_reference(account_id: &str, source: &str) -> String {
    crate::providers::qoder_auth::keychain_ref(&keychain_primary_key(account_id, source))
}

/// 钥匙串操作的跨进程排他锁。
///
/// macOS 的钥匙串**没有原子 compare-and-delete**：`get` 与 `delete` 是两次调用。
/// 中间没有锁，就可能"比对的是旧令牌 A、删掉的却是刚写进来的新令牌 B"。
/// 所以钥匙串的写入、读-比-删、以及回滚都必须持同一把锁 —— 这里直接复用凭据目录的
/// `.lock`，JSON 写入与钥匙串写入因此在同一把锁上排队。
struct KeychainLock {
    _lock: crate::credentials::localjson::FileLock,
}

fn keychain_lock(app: &App) -> Result<KeychainLock, CredentialError> {
    let store = LocalJsonStore::for_paths(&app.paths);
    Ok(KeychainLock {
        _lock: store.lock_exclusive()?,
    })
}

/// 一次**凭据事务**：同一来源的「读观察 → 写凭据 → 数据库条件提交 → 失败清理」
/// 全部在这把锁内完成。
///
/// ## 为什么需要它（审计 P0-1 / P0-2 / P0-3）
///
/// 之前的实现每次操作只锁各自的片段：`write_keychain` 锁住"写"，`commit` 单独提交
/// SQLite，中间的间隙足够另一个进程插进来：
///
/// ```text
/// A: 写 Keychain = A-TOKEN
/// B: 写 Keychain = B-TOKEN，提交成功          ← 行归 B
/// A: 提交（基于旧快照）失败                     ← 失败路径什么都不补偿
/// 结果：行记 B 的 fingerprint，Keychain 里是 A 的令牌
/// ```
///
/// JSON 原地覆盖同理（文件被写成 B，行是 A）。现在四条 mutate 路径
/// （`qoder_auth::persist` / `qoder_pat::persist` / `qoder_pat::remove` /
/// `qoder_admin::delete`）整段持锁，写-提交-回滚不可分割，上面的交错不可能出现。
///
/// ## 锁顺序
///
/// 只允许 `.lock` → SQLite。持锁期间**不做**任何会反向取锁的事；`LocalJsonStore`
/// 的每个方法会重入同一把锁（`FileLock` 支持同线程重入），不会自我阻塞。
///
/// ## 读路径为什么不进来
///
/// `credits::resolve_token` 只读令牌、不写任何状态，而写入顺序永远是
/// "先写凭据、后提交行"，所以它读到的最坏情况是**上一版令牌**，不会读到半份
/// （钥匙串的 access/refresh 是两份值，但请求只需要 access）。让它也排队只会
/// 把一次网络请求压到凭据写入后面，没有收益。
pub struct SecretTxn {
    _lock: crate::credentials::localjson::FileLock,
}

/// 开始一次凭据事务。**调用方必须把它持有到提交/回滚都结束。**
pub fn begin(app: &App) -> RpcResult<SecretTxn> {
    let store = LocalJsonStore::for_paths(&app.paths);
    let lock = store
        .lock_exclusive()
        .map_err(|err| to_rpc(err, "qoder/lock"))?;
    Ok(SecretTxn { _lock: lock })
}

/// 这一行现在把凭据放在哪里。
///
/// `secret_ref = None` 视为钥匙串：那是 CRED-3b 之前写下的老行，凭据确实在钥匙串里。
pub fn storage_of(row: &QoderAuthRecord) -> Result<Storage, CredentialError> {
    let Some(raw) = row.secret_ref.as_deref() else {
        return Ok(Storage::Keychain);
    };
    match CredentialRef::parse(raw)? {
        CredentialRef::Keychain { .. } => Ok(Storage::Keychain),
        CredentialRef::LocalJson { .. } => Ok(Storage::Json),
        // 自有授权指向 Magpie = 库被改脏。不猜它想指谁。
        CredentialRef::Magpie { .. } => Err(CredentialError::new(
            CredentialErrorKind::Unsupported,
            "自有授权的引用指向 Magpie，已拒绝（不读取、不写入、不删除）",
        )),
    }
}

/// 引用 → 本地文件 ID。
fn json_secret_id(row: &QoderAuthRecord) -> Result<String, CredentialError> {
    match CredentialRef::parse(row.secret_ref.as_deref().unwrap_or_default())? {
        CredentialRef::LocalJson { secret_id } => Ok(secret_id),
        _ => Err(CredentialError::new(
            CredentialErrorKind::Unsupported,
            "这条授权的引用不是本地 JSON",
        )),
    }
}

/// 钥匙串行里的引用必须**正好**指向该来源的主键。
///
/// 少了这一道，一条指向 `qoder/别的账号/own/access` 的脏行会让删除流程去清
/// 另一个账号的令牌。
pub fn verify_keychain_ref(row: &QoderAuthRecord, account_id: &str) -> Result<(), CredentialError> {
    let expected = keychain_primary_key(account_id, &row.source);
    let Some(raw) = row.secret_ref.as_deref() else {
        // 老行没有引用：键名由账号 id + 来源派生，视为一致。
        return Ok(());
    };
    match CredentialRef::parse(raw)? {
        CredentialRef::Keychain { key } if key == expected => Ok(()),
        CredentialRef::Keychain { .. } => Err(CredentialError::new(
            CredentialErrorKind::Conflict,
            "钥匙串引用指向别的键，已拒绝（不读取、不删除）",
        )),
        _ => Err(CredentialError::new(
            CredentialErrorKind::Unsupported,
            "这条授权的引用不是钥匙串",
        )),
    }
}

// ---------------------------------------------------------------------------
// 读
// ---------------------------------------------------------------------------

/// 读一份自有凭据，返回 access 令牌。**读不到就是读不到**，不换来源、不回落钥匙串。
pub fn read(
    app: &App,
    account: &QoderAccountRecord,
    row: &QoderAuthRecord,
    kind: CredentialKind,
) -> Result<String, CredentialError> {
    match storage_of(row)? {
        Storage::Json => {
            let secret = load_json(app, account, row, kind)?;
            Ok(secret.token(CredentialSlot::Access)?.to_string())
        }
        Storage::Keychain => {
            verify_keychain_ref(row, &account.id)?;
            let key = keychain_primary_key(&account.id, &row.source);
            let bytes = app.secure.get(&key).map_err(|err| {
                let kind = match err.kind {
                    crate::securestore::SecretErrorKind::NotFound => CredentialErrorKind::NotFound,
                    crate::securestore::SecretErrorKind::Denied => CredentialErrorKind::Denied,
                    _ => CredentialErrorKind::Io,
                };
                let mut out =
                    CredentialError::new(kind, format!("读不到钥匙串凭据：{}", err.message));
                out.os_status = err.os_status;
                out
            })?;
            String::from_utf8(bytes).map_err(|_| {
                CredentialError::new(CredentialErrorKind::Corrupt, "钥匙串里的凭据不是合法 UTF-8")
            })
        }
    }
}

/// 读并校验归属。四个判据缺一不可。
pub fn load_json(
    app: &App,
    account: &QoderAccountRecord,
    row: &QoderAuthRecord,
    kind: CredentialKind,
) -> Result<StoredSecret, CredentialError> {
    let secret_id = json_secret_id(row)?;
    let secret = LocalJsonStore::for_paths(&app.paths).load(&secret_id)?;
    verify_json(&secret, account, row, kind)?;
    Ok(secret)
}

fn verify_json(
    secret: &StoredSecret,
    account: &QoderAccountRecord,
    row: &QoderAuthRecord,
    kind: CredentialKind,
) -> Result<(), CredentialError> {
    if secret.authorization_id != row.id {
        return Err(mismatch("凭据文件属于另一条授权"));
    }
    if secret.platform != account.edition {
        return Err(mismatch("凭据文件的平台与账号不一致"));
    }
    if secret.source != SecretSource::Own {
        return Err(mismatch("凭据文件的来源不是 AICC 自有授权"));
    }
    if secret.credential_kind != kind {
        return Err(mismatch("凭据文件的类型与授权来源不一致"));
    }
    Ok(())
}

fn mismatch(what: &str) -> CredentialError {
    CredentialError::new(
        CredentialErrorKind::Conflict,
        format!("{what}（已拒绝，没有读取、写入或删除任何令牌）"),
    )
}

// ---------------------------------------------------------------------------
// 写
// ---------------------------------------------------------------------------

/// 一次写入要记住的"怎么撤回"。三种形态对应三条落点。
enum Undo {
    /// 钥匙串：按快照恢复，**但只在盘上还是我写进去的那一版时才恢复**。
    ///
    /// 只加锁是不够的：A 写完等数据库提交，B 随后写入并提交成功，A 的提交失败 ——
    /// A 拿到锁后无条件写回快照，就把 B 的成功授权覆盖成了旧值（复审 P0-B）。
    /// `written` 是这次写入后的值，回滚前逐键比对，对不上就放弃回滚。
    Keychain {
        snapshot: Snapshot,
        written: Vec<(String, Vec<u8>)>,
    },
    /// 新建的 JSON 文件：撤回 = 删掉它。
    Created { secret_id: String },
    /// 覆盖已有 JSON 文件：撤回 = 把旧令牌写回去（带 revision 校验）。
    Replaced {
        secret_id: String,
        revision: u64,
        previous: Credentials,
        previous_expires: Option<String>,
    },
}

/// 一次自有凭据写入的结果。`reference` 就是落库时要写进 `secret_ref` 的值。
pub struct QoderWrite {
    reference: String,
    storage: Storage,
    undo: Undo,
}

impl QoderWrite {
    pub fn reference(&self) -> &str {
        &self.reference
    }

    pub fn storage(&self) -> Storage {
        self.storage
    }

    /// 竞争失败时用：只丢掉**这次新建、没有任何行引用**的文件。
    ///
    /// 覆盖已有文件的那次写入**刻意不回滚**：盘上那份本来就是这条授权的，
    /// 里面是刚拿到的有效令牌；回滚只会把它退回到服务端可能已作废的旧值。
    pub fn discard_unreferenced(&self, app: &App) {
        let secret_id = match &self.undo {
            Undo::Created { secret_id } => secret_id,
            _ => return,
        };
        let Ok(reference) = reference::local_json_ref(secret_id) else {
            return;
        };
        let referenced = app
            .with_db(|db| db.qoder_authorizations_referencing(&reference))
            .unwrap_or(1);
        if referenced > 0 {
            return;
        }
        let store = LocalJsonStore::for_paths(&app.paths);
        if let Err(err) = store.delete(secret_id) {
            crate::handlers::log_note(&format!(
                "竞争失败后清理未引用的 Qoder 凭据失败（{}）：{}",
                err.kind.as_str(),
                crate::rpc::sanitize(&err.message)
            ));
        }
    }

    /// 回滚这次写入。返回 false = 没恢复干净（或**故意没恢复**），调用方必须留痕。
    ///
    /// 三种情况都会拒绝回滚，而不是硬写：
    ///
    /// - 拿不到锁（另一个进程正在写）；
    /// - 盘上已经不是这次写入的值：别人在我之后写入并提交成功了，把它的令牌
    ///   覆盖回旧值等于毁掉一份有效授权（复审 P0-B）；
    /// - 回滚本身失败。
    pub fn restore(&self, app: &App) -> bool {
        let store = LocalJsonStore::for_paths(&app.paths);
        match &self.undo {
            // 回滚也是"写钥匙串"：必须与删除/比对在同一把锁下排队，否则回滚可能盖掉
            // 别人刚写进来的新令牌（复审要求：写入、比较、删除、回滚统一协调）。
            Undo::Keychain { snapshot, written } => match keychain_lock(app) {
                Ok(_lock) => {
                    // 锁内再看一眼：盘上还是不是我写进去的那一版？
                    for (key, mine) in written {
                        match app.secure.get(key) {
                            Ok(current) if current == *mine => {}
                            Ok(_) => {
                                crate::handlers::log_note(
                                    "Qoder 凭据已被另一个进程更新：本次不回滚，以免覆盖对方刚保存的令牌",
                                );
                                return false;
                            }
                            Err(err)
                                if err.kind == crate::securestore::SecretErrorKind::NotFound =>
                            {
                                // 有人把它删了：旧值也没有恢复的意义，保持删除状态。
                                crate::handlers::log_note(
                                    "Qoder 凭据已被另一个进程删除：本次不回滚，不复活旧令牌",
                                );
                                return false;
                            }
                            Err(err) => {
                                crate::handlers::log_note(&format!(
                                    "Qoder 凭据回滚前读不到当前值（{:?}）：未回滚，请重新授权",
                                    err.kind
                                ));
                                return false;
                            }
                        }
                    }
                    snapshot.restore(app)
                }
                Err(err) => {
                    crate::handlers::log_note(&format!(
                        "Qoder 凭据回滚拿不到锁（{}）：未回滚，请重新授权",
                        err.kind.as_str()
                    ));
                    false
                }
            },
            Undo::Created { secret_id } => {
                // 新建的文件按说没人知道它的 id；但万一提交其实成功了（或另一个
                // 进程恰好引用了它），删掉就是毁一份有效凭据。删前确认没人引用。
                if let Ok(reference) = reference::local_json_ref(secret_id) {
                    if app
                        .with_db(|db| db.qoder_authorizations_referencing(&reference))
                        .unwrap_or(1)
                        > 0
                    {
                        crate::handlers::log_note(
                            "Qoder 新凭据文件已被授权行引用：本次不回滚，保留它",
                        );
                        return false;
                    }
                }
                match store.delete(secret_id) {
                    Ok(_) => true,
                    Err(err) => {
                        crate::handlers::log_note(&format!(
                            "Qoder 新凭据文件删不掉（{}）：{}",
                            err.kind.as_str(),
                            crate::rpc::sanitize(&err.message)
                        ));
                        false
                    }
                }
            }
            Undo::Replaced {
                secret_id,
                revision,
                previous,
                previous_expires,
            } => {
                let expires = previous_expires
                    .as_deref()
                    .and_then(crate::auth::service::parse_rfc3339);
                match store.replace_credentials(secret_id, *revision, previous.clone(), expires) {
                    Ok(_) => true,
                    Err(err) => {
                        crate::handlers::log_note(&format!(
                            "Qoder 旧凭据写不回去（{}）：{}",
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

/// 写凭据之前的**那一次**观察：授权行 + 账号生命周期。
///
/// 只读一次、只信这一份，是 CRED-3b 复审 P0-1 的修法。之前的实现让 `write()`
/// 自己再查一次授权行来决定"写哪里"，于是两次观察可以不同：
///
/// ```text
/// A: 看到 own_browser 不存在 → 准备新建（Absent）
/// B: 建好文件 F、插入 own_browser 行
/// A: write() 重查 → 看到 Present(F) → 直接覆盖 F     ← 赢家的令牌被悄悄改写
/// A: commit(Absent) 撞唯一键 → 报冲突                 ← 但 F 已经毁了
/// ```
///
/// 现在"写哪里"和"怎么提交"来自同一次观察，落点与提交预期不可能错位。
pub struct Observation {
    pub row: Option<QoderAuthRecord>,
    pub lifecycle: QoderLifecycle,
}

/// 读一次观察。行与生命周期在同一次数据库调用里取，调用方之后必须只信它。
pub fn observe(app: &App, account_id: &str, source: &str) -> RpcResult<Observation> {
    app.with_db(|db| {
        let lifecycle = db
            .qoder_lifecycle(account_id)?
            .ok_or_else(|| RpcError::internal("账号不存在，读不到生命周期"))?;
        let row = db.get_qoder_authorization(account_id, source)?;
        Ok(Observation { row, lifecycle })
    })
}

/// 写一份自有凭据（**不碰数据库**）。
///
/// 落点只看 `observed`：调用方之后必须用 [`QoderWrite::reference`] 和同一份观察
/// 走一次条件提交；提交失败时按 [`QoderWrite::restore`] /
/// [`QoderWrite::discard_unreferenced`] 收尾。
#[allow(clippy::too_many_arguments)]
pub fn write(
    app: &App,
    account: &QoderAccountRecord,
    source: &str,
    access: &str,
    refresh: Option<&str>,
    expires_at: Option<DateTime<Utc>>,
    observed: &Observation,
) -> RpcResult<QoderWrite> {
    let kind = kind_of(source);
    if kind == CredentialKind::Pat && refresh.is_some() {
        // PAT 是长期凭据，带着 refresh 只能是调用方搞错了字段。
        return Err(RpcError::invalid_params("PAT 不允许携带刷新令牌"));
    }
    if access.trim().is_empty() {
        return Err(RpcError::invalid_params("凭据不能为空"));
    }

    if let Some(row) = observed.row.as_ref() {
        return match storage_of(row).map_err(|err| to_rpc(err, "qoder/ref"))? {
            // 老行继续写钥匙串：不隐式迁移，避免迁移中的账号出现半新半旧的状态。
            Storage::Keychain => {
                verify_keychain_ref(row, &account.id).map_err(|err| to_rpc(err, "qoder/ref"))?;
                let (snapshot, written) =
                    write_keychain(app, &account.id, source, access, refresh)?;
                Ok(QoderWrite {
                    reference: row
                        .secret_ref
                        .clone()
                        .unwrap_or_else(|| keychain_reference(&account.id, source)),
                    storage: Storage::Keychain,
                    undo: Undo::Keychain { snapshot, written },
                })
            }
            Storage::Json => write_json_in_place(app, account, row, access, refresh, expires_at),
        };
    }

    // 还没有这一行 = 这是"第一次把这份令牌交给 AICC"。
    // 用户没确认过明文风险之前继续走钥匙串（与 #84/#86 的行为一致）。
    if !json_storage_enabled(app) {
        let (snapshot, written) = write_keychain(app, &account.id, source, access, refresh)?;
        return Ok(QoderWrite {
            reference: keychain_reference(&account.id, source),
            storage: Storage::Keychain,
            undo: Undo::Keychain { snapshot, written },
        });
    }

    write_json_new(app, account, source, access, refresh, expires_at)
}

fn write_json_new(
    app: &App,
    account: &QoderAccountRecord,
    source: &str,
    access: &str,
    refresh: Option<&str>,
    expires_at: Option<DateTime<Utc>>,
) -> RpcResult<QoderWrite> {
    let kind = kind_of(source);
    let edition = QoderEdition::parse(&account.edition)
        .ok_or_else(|| RpcError::invalid_params("账号的 edition 非法"))?;
    let authorization_id = authorization_id(&account.id, source);
    let platform = platform_of(edition);
    let mut draft = if kind == CredentialKind::Pat {
        SecretDraft::pat(authorization_id, platform, access)
    } else {
        let mut draft = SecretDraft::oauth(authorization_id, platform, access);
        if let Some(token) = refresh {
            draft = draft.with_refresh_token(token);
        }
        draft
    };
    if let Some(at) = expires_at {
        draft = draft.with_expires_at(at);
    }

    let secret = LocalJsonStore::for_paths(&app.paths)
        .create(draft)
        .map_err(|err| to_rpc(err, "qoder/json"))?;
    let reference = reference::local_json_ref(&secret.secret_id)
        .map_err(|err| RpcError::internal(err.message))?;
    Ok(QoderWrite {
        reference,
        storage: Storage::Json,
        undo: Undo::Created {
            secret_id: secret.secret_id,
        },
    })
}

/// 已有 JSON 授权再次写入：覆盖同一个文件，access + refresh 一次替换。
///
/// 不复用"新建再删旧的"：崩溃窗口里会多出一份没人引用、但**仍然有效**的令牌。
fn write_json_in_place(
    app: &App,
    account: &QoderAccountRecord,
    row: &QoderAuthRecord,
    access: &str,
    refresh: Option<&str>,
    expires_at: Option<DateTime<Utc>>,
) -> RpcResult<QoderWrite> {
    let kind = kind_of(&row.source);
    let secret_id = json_secret_id(row).map_err(|err| to_rpc(err, "qoder/json"))?;
    // 覆盖写等于"改写这份文件"：先验归属，否则一次错误的引用会覆盖别人的令牌。
    let current = match load_json(app, account, row, kind) {
        Ok(secret) => secret,
        // 文件已经不在，但行还在引用它 —— 上一次删除删掉了文件、没删掉行
        // （删号被并发打断就是这种状态）。这时按"新建一份"处理，否则用户
        // 重新授权会永远失败在"凭据不存在"，等于把重新登录功能弄坏了。
        Err(err) if err.kind == CredentialErrorKind::NotFound => {
            return write_json_new(app, account, &row.source, access, refresh, expires_at);
        }
        Err(err) => return Err(to_rpc(err, "qoder/json")),
    };

    let credentials = match (kind, refresh) {
        (CredentialKind::Pat, _) => Credentials::access_only(access),
        // 服务端这次没下发 refresh：保留盘上那份（钥匙串路径同样保留旧 refresh 键）。
        (CredentialKind::Oauth, None) => match current.credentials.refresh_token.as_deref() {
            Some(previous) => Credentials::oauth(access, previous),
            None => Credentials::access_only(access),
        },
        (CredentialKind::Oauth, Some(token)) => Credentials::oauth(access, token),
    };

    let store = LocalJsonStore::for_paths(&app.paths);
    let updated = store
        .replace_credentials(&secret_id, current.revision, credentials, expires_at)
        .map_err(|err| to_rpc(err, "qoder/json"))?;
    Ok(QoderWrite {
        reference: row.secret_ref.clone().unwrap_or_default(),
        storage: Storage::Json,
        undo: Undo::Replaced {
            secret_id,
            revision: updated.revision,
            previous: current.credentials,
            previous_expires: current.expires_at,
        },
    })
}

type KeychainWrite = (Snapshot, Vec<(String, Vec<u8>)>);

fn write_keychain(
    app: &App,
    account_id: &str,
    source: &str,
    access: &str,
    refresh: Option<&str>,
) -> RpcResult<KeychainWrite> {
    // 整个"拍快照 → 写 access → 写 refresh"必须在同一把锁里完成：它和
    // `delete_guarded` 的"读-比-删"共用这把锁，才谈得上原子。
    let _lock = keychain_lock(app).map_err(|err| to_rpc(err, "qoder/lock"))?;
    let keys = keychain_keys(account_id, source);
    let snapshot = Snapshot::capture(app, &keys)?;
    let primary = keychain_primary_key(account_id, source);
    if let Err(err) = app.secure.set(&primary, access.as_bytes()) {
        snapshot.restore(app);
        return Err(rpc_secret_error(err, "qoder/access"));
    }
    // 写进去的值要记住：提交失败回滚时用它判断"盘上还是不是我这一版"。
    let mut written = vec![(primary, access.as_bytes().to_vec())];
    // refresh 只属于浏览器授权；PAT 没有这个槽位。
    if let (Some(token), Some(refresh_key)) = (refresh, keys.get(1)) {
        if let Err(err) = app.secure.set(refresh_key, token.as_bytes()) {
            snapshot.restore(app);
            return Err(rpc_secret_error(err, "qoder/refresh"));
        }
        written.push((refresh_key.clone(), token.as_bytes().to_vec()));
    }
    Ok((snapshot, written))
}

/// 钥匙串错误 → RPC 错误。**带上"凭据"字样与阶段**：界面与验收都靠它区分
/// "没写进去"和"读不到"。
fn rpc_secret_error(err: crate::securestore::SecretError, stage: &str) -> RpcError {
    RpcError::new(
        crate::rpc::E_SECURESTORE,
        format!("写入凭据失败（{stage}）：{}", err.message),
    )
    .with_data(serde_json::json!({
        "kind": "credential_write_failed",
        "stage": stage,
    }))
}

// ---------------------------------------------------------------------------
// 删
// ---------------------------------------------------------------------------

/// 删除前的观察：**这一份凭据现在到底是什么版本**。
///
/// 单独删一份授权（例如移除 PAT）与"重新保存同一来源"可以重叠。先读、再删、
/// 最后按行删，中间那段窗口里新令牌可能已经落盘 —— 不加这道校验就会删掉
/// 刚刚保存的那一份，而授权行还显示 `valid`。
///
/// JSON 有 `revision`，可以和写入做真正的 CAS；钥匙串条目没有版本号，退而记录
/// **整组键**的取值，删除前在同一把锁里逐键比对（见 [`delete_guarded`]）。
///
/// **刻意不实现 `Debug`**：`Keychain` 分支里装的是令牌原文。
pub enum DeleteGuard {
    Json {
        secret_id: String,
        revision: u64,
    },
    /// **整组键**的取值快照（`None` = 观察时该键不存在）。
    ///
    /// 设备授权是 access + refresh 两份值，必须当成同一份凭据一起比对、一起删：
    /// 只看 access 的话，refresh 被单独更新过也会被一并删掉。
    Keychain {
        values: Vec<(String, Option<Vec<u8>>)>,
    },
}

/// 读一次凭据，验归属，并记下版本。`Ok(None)` = 本来就不在（删除幂等）。
pub fn observe_for_delete(
    app: &App,
    account: &QoderAccountRecord,
    row: &QoderAuthRecord,
    kind: CredentialKind,
) -> Result<Option<DeleteGuard>, CredentialError> {
    match storage_of(row)? {
        Storage::Json => {
            let secret = match load_json(app, account, row, kind) {
                Ok(secret) => secret,
                Err(err) if err.kind == CredentialErrorKind::NotFound => return Ok(None),
                Err(err) => return Err(err),
            };
            Ok(Some(DeleteGuard::Json {
                secret_id: secret.secret_id,
                revision: secret.revision,
            }))
        }
        Storage::Keychain => {
            verify_keychain_ref(row, &account.id)?;
            // 整组键都要读：access 与 refresh 是同一份凭据的两半。
            let mut values = Vec::new();
            for key in keychain_keys(&account.id, &row.source) {
                match app.secure.get(&key) {
                    Ok(bytes) => values.push((key, Some(bytes))),
                    Err(err) if err.kind == crate::securestore::SecretErrorKind::NotFound => {
                        values.push((key, None))
                    }
                    Err(err) => return Err(secure_read_error(err)),
                }
            }
            // 所有键都不在 = 本来就没有（删除幂等）。
            if values.iter().all(|(_, value)| value.is_none()) {
                return Ok(None);
            }
            Ok(Some(DeleteGuard::Keychain { values }))
        }
    }
}

/// 只有"还是观察到的那一份"才删。版本对不上返回 `conflict`，**一个字节都不删**。
pub fn delete_guarded(
    app: &App,
    account: &QoderAccountRecord,
    row: &QoderAuthRecord,
    kind: CredentialKind,
    guard: &DeleteGuard,
) -> Result<bool, CredentialError> {
    match guard {
        DeleteGuard::Json {
            secret_id,
            revision,
        } => {
            // 删除前再验一次归属：引用没变也可能被别人改成了另一条授权。
            let secret = match load_json(app, account, row, kind) {
                Ok(secret) => secret,
                Err(err) if err.kind == CredentialErrorKind::NotFound => return Ok(false),
                Err(err) => return Err(err),
            };
            if secret.secret_id != *secret_id || secret.revision != *revision {
                return Err(mismatch("凭据在这期间被重新写过"));
            }
            LocalJsonStore::for_paths(&app.paths).delete_at_revision(secret_id, *revision)
        }
        DeleteGuard::Keychain { values } => {
            verify_keychain_ref(row, &account.id)?;
            // **比对与删除必须在同一把跨进程锁里**：钥匙串没有原子 compare-and-delete，
            // 中间放开锁就可能把刚写进来的新令牌删掉。
            let _lock = keychain_lock(app)?;
            for (key, expected) in values {
                match (app.secure.get(key), expected) {
                    (Ok(current), Some(expected)) if current == *expected => {}
                    // 观察时不在、现在出现了：有人刚写过，同样算变化。
                    (Ok(_), _) => return Err(mismatch("凭据在这期间被重新写过")),
                    (Err(err), None)
                        if err.kind == crate::securestore::SecretErrorKind::NotFound => {}
                    (Err(err), _) if err.kind == crate::securestore::SecretErrorKind::NotFound => {
                        return Err(mismatch("凭据在这期间被删掉了"))
                    }
                    (Err(err), _) => return Err(secure_read_error(err)),
                }
            }
            let mut removed = 0usize;
            for (key, expected) in values {
                if expected.is_none() {
                    continue;
                }
                match app.secure.delete(key) {
                    Ok(true) => removed += 1,
                    Ok(false) => {}
                    Err(err) if err.kind == crate::securestore::SecretErrorKind::NotFound => {}
                    Err(err) => {
                        // **已经删掉一部分**：剩下半份凭据（例如没有 access 的 refresh）
                        // 发不出请求。调用方必须把行标成不可用，否则界面显示 `valid`、
                        // 而 Credits 永远失败（复审 P1-D）。
                        let out = secure_delete_error(err);
                        return Err(if removed > 0 { out.with_partial() } else { out });
                    }
                }
            }
            Ok(removed > 0)
        }
    }
}

/// 按"观察到的值"删**一组**钥匙串键：整组比对 + 整组删除在同一把锁里完成。
///
/// 用途：删账号时兜底清"没有任何授权行"的旧安装（`qoder/<id>/own/access|refresh`
/// 与 `qoder/<id>/pat/token`）。这些键没有引用可依，但一样会被同一来源的重新登录
/// 覆盖，所以同样需要版本守卫 —— 而一组键（access + refresh）必须一起判、一起删。
///
/// 返回真正删掉的键数；任何一键的值与观察不符就返回 `conflict`，一个字节都不删。
pub fn delete_keychain_group(
    app: &App,
    values: &[(String, Option<Vec<u8>>)],
) -> Result<usize, CredentialError> {
    if values.is_empty() {
        return Ok(0);
    }
    // 与 `delete_guarded` 同一把锁：比对和删除不能被"写新令牌"插队。
    let _lock = keychain_lock(app)?;
    // **在不在也要比对**（复审 P0-A）：旧安装原来可能只有 access，重新登录补上了
    // refresh；如果计划只记"当时存在的键"，就会把这次新增整组忽略掉，照样删掉
    // 新授权正在用的 access。
    for (key, expected) in values {
        match (app.secure.get(key), expected) {
            (Ok(current), Some(expected)) if current == *expected => {}
            (Ok(_), None) => return Err(mismatch("钥匙串这期间新增了这一份凭据的一部分")),
            (Ok(_), Some(_)) => return Err(mismatch("钥匙串凭据在这期间被重新写过")),
            (Err(err), None) if err.kind == crate::securestore::SecretErrorKind::NotFound => {}
            (Err(err), _) if err.kind == crate::securestore::SecretErrorKind::NotFound => {
                return Err(mismatch("钥匙串凭据在这期间被删掉了"))
            }
            (Err(err), _) => return Err(secure_read_error(err)),
        }
    }
    let mut removed = 0usize;
    for (key, expected) in values {
        if expected.is_none() {
            continue;
        }
        match app.secure.delete(key) {
            Ok(true) => removed += 1,
            Ok(false) => {}
            Err(err) if err.kind == crate::securestore::SecretErrorKind::NotFound => {}
            Err(err) => {
                let out = secure_delete_error(err);
                return Err(if removed > 0 { out.with_partial() } else { out });
            }
        }
    }
    Ok(removed)
}

fn secure_read_error(err: crate::securestore::SecretError) -> CredentialError {
    let kind = match err.kind {
        crate::securestore::SecretErrorKind::NotFound => CredentialErrorKind::NotFound,
        crate::securestore::SecretErrorKind::Denied => CredentialErrorKind::Denied,
        _ => CredentialErrorKind::Io,
    };
    let mut out = CredentialError::new(kind, format!("读不到钥匙串凭据：{}", err.message));
    out.os_status = err.os_status;
    out
}

fn secure_delete_error(err: crate::securestore::SecretError) -> CredentialError {
    let mut out = CredentialError::new(
        CredentialErrorKind::Io,
        format!("删除钥匙串凭据失败：{}", err.message),
    );
    out.os_status = err.os_status;
    out
}

/// 删除一份自有凭据，**只在归属校验通过时动手**。
///
/// 返回 `true` 表示确实删掉了一份；`false` 表示本来就没有（删除是幂等的，
/// 重试要能收敛）。归属不符一律 `conflict`，一个字节都不删。
///
/// **生产路径一律走 [`delete_guarded`]**：无条件删会在"同一来源刚被重新授权"
/// 时删掉新令牌（复审第三轮 P0）。这里保留给"已经确认没人会写"的场景与测试。
#[allow(dead_code)]
pub fn delete(
    app: &App,
    account: &QoderAccountRecord,
    row: &QoderAuthRecord,
    kind: CredentialKind,
) -> Result<bool, CredentialError> {
    match storage_of(row)? {
        Storage::Json => {
            let secret_id = json_secret_id(row)?;
            let store = LocalJsonStore::for_paths(&app.paths);
            match store.load(&secret_id) {
                Ok(secret) => {
                    verify_json(&secret, account, row, kind)?;
                    store.delete(&secret_id)
                }
                Err(err) if err.kind == CredentialErrorKind::NotFound => Ok(false),
                Err(err) => Err(err),
            }
        }
        Storage::Keychain => {
            verify_keychain_ref(row, &account.id)?;
            let mut removed = false;
            for key in keychain_keys(&account.id, &row.source) {
                match app.secure.delete(&key) {
                    Ok(true) => removed = true,
                    Ok(false) => {}
                    Err(err) if err.kind == crate::securestore::SecretErrorKind::NotFound => {}
                    Err(err) => {
                        return Err(CredentialError::new(
                            CredentialErrorKind::Io,
                            format!("删除钥匙串凭据失败：{}", err.message),
                        )
                        .with_status(err.os_status.unwrap_or(0)))
                    }
                }
            }
            Ok(removed)
        }
    }
}

// ---------------------------------------------------------------------------
// 条件提交
// ---------------------------------------------------------------------------

/// 把一次观察翻成条件提交的预期。**必须与 `write` 用的是同一份观察。**
pub fn expectation_of(observed: &Observation) -> QoderAuthExpectation {
    match &observed.row {
        Some(row) => QoderAuthExpectation::Present {
            secret_ref: row.secret_ref.clone(),
            lifecycle: observed.lifecycle.clone(),
            auth_version: row.auth_version,
        },
        // 首次授权也要带上生命周期：删号重建后 epoch 归零，只有 generation 挡得住。
        None => QoderAuthExpectation::Absent {
            lifecycle: observed.lifecycle.clone(),
        },
    }
}

/// 条件提交：引用、状态、有效期、指纹一次落库，同一事务里推进 `auth_epoch`。
///
/// 返回 `None` 表示"这一行在我写凭据期间被别人改过"——调用方必须按竞争失败处理
/// （`Created` 的新文件要清掉，`Replaced` 的保持不动），而不是覆盖对方。
pub fn commit(
    app: &App,
    account_id: &str,
    source: &str,
    write: &QoderWrite,
    auth_status: &str,
    token_expires_at: Option<&str>,
    fingerprint: Option<&str>,
    expectation: &QoderAuthExpectation,
) -> RpcResult<Option<QoderAuthRecord>> {
    app.with_db(|db| {
        db.commit_qoder_authorization(
            account_id,
            source,
            Some(write.reference()),
            auth_status,
            token_expires_at,
            fingerprint,
            expectation,
        )
    })
}

/// 凭据层错误 → RPC 错误。保留分类，别把 `conflict` / `unsafe` 压成一句话。
pub fn to_rpc(err: CredentialError, stage: &str) -> RpcError {
    RpcError::new(
        crate::rpc::E_SECURESTORE,
        format!(
            "凭据不可用（{stage}）：{}",
            crate::rpc::sanitize(&err.message)
        ),
    )
    .with_data(serde_json::json!({ "kind": err.kind.as_str(), "stage": stage }))
}

/// 竞争失败的统一错误。**不**说"保存成功"，也不删账号行（可能是胜者建的）。
pub fn conflict_error() -> RpcError {
    RpcError::new(
        crate::rpc::E_SECURESTORE,
        "另一个 AICC 进程刚刚更新了这份授权，本次结果已丢弃，请刷新后重试",
    )
    .with_data(serde_json::json!({
        "kind": "authorization_conflict",
        "stage": "qoder/commit",
    }))
}

/// 回滚"刚新建的账号"。
///
/// 三个条件缺一不可（审计 P1-1）：
///
/// 1. 本次调用**确实新建**了账号（`created` 是那句 upsert 之前观察到的）；
/// 2. 账号还是**我建的那一代**（`generation` 相同）—— 同一个 UID 删了又建会
///    得到同一个 account_id，只比 id 会把别人刚建的账号删掉；
/// 3. 它**一条授权行都没有** —— 有就说明别人已经写进来了（例如同一账号的另一个
///    来源），那不是"我建的空账号"。
///
/// 三条判断与删除在同一条 SQL 里，没有"查完再删"的窗口。
pub fn cleanup_created_account(app: &App, account_id: &str, created: Option<&str>) {
    let Some(generation) = created else {
        return;
    };
    let removed = app
        .with_db(|db| db.delete_qoder_account_if_empty(account_id, generation))
        .unwrap_or(false);
    if !removed {
        // 清不掉要留痕：账号页会看到一个没有来源的账号，得知道为什么。
        crate::handlers::log_note(
            "Qoder 保存失败后未能清掉本次新建的账号（可能已被其他流程使用），请手动移除",
        );
    }
}

/// 只读：某个来源现在的授权行。
///
/// 用于"删除凭据之前确认这一行没有被重建"：删除动作是按**旧快照**做的，
/// 一旦行被重新插入，旧 guard 的字节比对可能正好命中新凭据。
pub fn row_of(app: &App, account_id: &str, source: &str) -> Option<QoderAuthRecord> {
    app.with_db(|db| db.get_qoder_authorization(account_id, source))
        .ok()
        .flatten()
}

/// 授权来源是否属于"自有"（只有这两种才可能持有 AICC 自己的凭据）。
pub fn is_own_source(source: &str) -> bool {
    matches!(source, SOURCE_OWN_BROWSER | SOURCE_OWN_PAT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::localjson::Credentials;
    use crate::paths::Paths;
    use crate::providers::qoder_auth::{DeviceToken, Identity};
    use crate::securestore::SecureStore;
    use crate::storage::qoder_accounts::{QoderLifecycle, SOURCE_MAGPIE};

    fn paths(dir: &std::path::Path) -> Paths {
        Paths {
            db_path: dir.join("accounts.sqlite"),
            backups_dir: dir.join("backups"),
            logs_dir: dir.join("logs"),
            wb_switch_home: dir.join("wb-switch-home"),
            data_dir: dir.to_path_buf(),
            wb_home_owned: true,
        }
    }

    fn app_with(secure: SecureStore) -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = App::with_secure(paths(dir.path()), secure);
        (app, dir)
    }

    fn json_app() -> (App, tempfile::TempDir) {
        let (app, dir) = app_with(SecureStore::memory());
        crate::auth::consent::set_json_storage(&app, true).unwrap();
        (app, dir)
    }

    fn account(app: &App, edition: &str, uid: &str) -> QoderAccountRecord {
        app.with_db(|db| db.upsert_qoder_account(edition, uid, Some("昵称"), None))
            .unwrap()
    }

    fn token(uid: &str, access: &str) -> DeviceToken {
        DeviceToken {
            access_token: access.to_string(),
            refresh_token: Some("rt-1".to_string()),
            expires_at_ms: Some(1_795_354_934_000),
            identity: Identity {
                uid: uid.to_string(),
                name: Some("测试账号".to_string()),
                email: None,
            },
        }
    }

    fn secret_files(app: &App) -> usize {
        LocalJsonStore::for_paths(&app.paths)
            .scan()
            .map(|report| report.secrets.len())
            .unwrap_or(0)
    }

    fn row(app: &App, account_id: &str, source: &str) -> QoderAuthRecord {
        app.with_db(|db| db.get_qoder_authorization(account_id, source))
            .unwrap()
            .expect("授权行必须存在")
    }

    fn lifecycle(app: &App, account_id: &str) -> QoderLifecycle {
        app.with_db(|db| db.qoder_lifecycle(account_id))
            .unwrap()
            .expect("生命周期")
    }

    fn observation(app: &App, account_id: &str, source: &str) -> Observation {
        observe(app, account_id, source).expect("观察")
    }

    // -- 存储形态判定 ---------------------------------------------------

    #[test]
    fn a_row_without_a_reference_is_a_legacy_keychain_row() {
        let (app, _dir) = json_app();
        let account = account(&app, "qoder_cn", "u-1");
        let mut row = QoderAuthRecord {
            id: authorization_id(&account.id, SOURCE_OWN_BROWSER),
            account_id: account.id.clone(),
            source: SOURCE_OWN_BROWSER.to_string(),
            source_key: None,
            secret_ref: None,
            auth_status: "valid".to_string(),
            token_expires_at: None,
            fingerprint: None,
            created_at: String::new(),
            updated_at: String::new(),
            auth_version: 1,
        };
        assert_eq!(storage_of(&row).unwrap(), Storage::Keychain);

        row.secret_ref = Some(keychain_reference(&account.id, SOURCE_OWN_BROWSER));
        assert_eq!(storage_of(&row).unwrap(), Storage::Keychain);

        row.secret_ref = Some(format!("localjson:v1:{}", uuid::Uuid::new_v4()));
        assert_eq!(storage_of(&row).unwrap(), Storage::Json);

        // 自有行指向 Magpie：库被改脏，不许猜。
        row.secret_ref = Some("magpie:v1:qoder-cn#1".to_string());
        assert_eq!(
            storage_of(&row).unwrap_err().kind,
            CredentialErrorKind::Unsupported
        );

        // 未知前缀同样拒绝（不回退钥匙串）。
        row.secret_ref = Some("secret-store:foo".to_string());
        assert_eq!(
            storage_of(&row).unwrap_err().kind,
            CredentialErrorKind::Unsupported
        );
    }

    #[test]
    fn a_keychain_row_pointing_elsewhere_is_refused() {
        let (app, _dir) = json_app();
        let account = account(&app, "qoder_cn", "u-1");
        let mut row = QoderAuthRecord {
            id: "qz_x".to_string(),
            account_id: account.id.clone(),
            source: SOURCE_OWN_BROWSER.to_string(),
            source_key: None,
            secret_ref: Some("keychain:qoder/another-account/own/access".to_string()),
            auth_status: "valid".to_string(),
            token_expires_at: None,
            fingerprint: None,
            created_at: String::new(),
            updated_at: String::new(),
            auth_version: 1,
        };
        assert_eq!(
            verify_keychain_ref(&row, &account.id).unwrap_err().kind,
            CredentialErrorKind::Conflict
        );
        row.source = SOURCE_OWN_PAT.to_string();
        row.secret_ref = Some(keychain_reference(&account.id, SOURCE_OWN_PAT));
        assert!(verify_keychain_ref(&row, &account.id).is_ok());
    }

    // -- 写入落点 -------------------------------------------------------

    /// 没同意明文 → 新授权仍然写钥匙串，且**一个 JSON 文件都不建**。
    #[test]
    fn without_consent_a_new_authorization_stays_in_the_keychain() {
        let (app, _dir) = app_with(SecureStore::memory());
        let account = account(&app, "qoder_cn", "u-1");
        let write = write(
            &app,
            &account,
            SOURCE_OWN_BROWSER,
            "dt-1",
            Some("rt-1"),
            None,
            &observation(&app, &account.id, SOURCE_OWN_BROWSER),
        )
        .unwrap();
        assert_eq!(write.storage(), Storage::Keychain);
        assert!(write.reference().starts_with("keychain:qoder/"));
        assert_eq!(secret_files(&app), 0);
        assert_eq!(
            app.secure
                .get(&crate::providers::qoder_auth::access_key(&account.id))
                .unwrap(),
            b"dt-1"
        );
    }

    /// 已同意 → 新授权落 JSON，钥匙串一次都不碰（这里用"全部拒绝"的后端证明）。
    #[test]
    fn with_consent_a_new_authorization_never_touches_the_keychain() {
        let (app, _dir) = app_with(SecureStore::failing());
        crate::auth::consent::set_json_storage(&app, true).unwrap();
        let account = account(&app, "qoder_cn", "u-1");
        let write = write(
            &app,
            &account,
            SOURCE_OWN_BROWSER,
            "dt-1",
            Some("rt-1"),
            None,
            &observation(&app, &account.id, SOURCE_OWN_BROWSER),
        )
        .unwrap();
        assert_eq!(write.storage(), Storage::Json);
        assert!(write.reference().starts_with("localjson:v1:"));
        assert_eq!(secret_files(&app), 1);

        let stored = LocalJsonStore::for_paths(&app.paths)
            .token(
                write.reference().trim_start_matches("localjson:v1:"),
                CredentialSlot::Access,
            )
            .unwrap();
        assert_eq!(stored, "dt-1");
    }

    /// CN 与 International 同一个 UID：两份文件、两个 platform、互不覆盖。
    #[test]
    fn the_two_editions_never_share_a_secret() {
        let (app, _dir) = json_app();
        for (uid, edition, token_value) in [
            ("u-same", "qoder_cn", "dt-cn"),
            ("u-same", "qoder_global", "dt-global"),
        ] {
            crate::providers::qoder_auth::persist(
                &app,
                QoderEdition::parse(edition).unwrap(),
                &token(uid, token_value),
            )
            .unwrap();
        }
        assert_eq!(secret_files(&app), 2);

        let scan = LocalJsonStore::for_paths(&app.paths).scan().unwrap();
        let platforms: Vec<&str> = scan.secrets.iter().map(|s| s.platform.as_str()).collect();
        assert!(platforms.contains(&"qoder_cn"));
        assert!(platforms.contains(&"qoder_global"));
    }

    /// 设备授权与 PAT：两份凭据、两个文件，互不覆盖。
    #[test]
    fn device_auth_and_pat_live_in_two_files() {
        let (app, _dir) = json_app();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-1", "dt-1"))
            .unwrap();
        crate::providers::qoder_pat::persist(
            &app,
            QoderEdition::Cn,
            pat,
            &Identity {
                uid: "u-1".into(),
                name: None,
                email: None,
            },
        )
        .unwrap();

        assert_eq!(secret_files(&app), 2);
        let id = app
            .with_db(|db| db.list_qoder_accounts(None))
            .unwrap()
            .remove(0)
            .id;
        let browser = row(&app, &id, SOURCE_OWN_BROWSER);
        let pat_row = row(&app, &id, SOURCE_OWN_PAT);
        assert_ne!(
            browser.secret_ref, pat_row.secret_ref,
            "两个来源不能共用一个文件"
        );

        assert_eq!(
            read(
                &app,
                &account_of(&app, &id),
                &browser,
                CredentialKind::Oauth
            )
            .unwrap(),
            "dt-1"
        );
        assert_eq!(
            read(&app, &account_of(&app, &id), &pat_row, CredentialKind::Pat).unwrap(),
            pat
        );
        // PAT 文件里没有 refresh 槽位。
        let secret =
            load_json(&app, &account_of(&app, &id), &pat_row, CredentialKind::Pat).unwrap();
        assert!(secret.credentials.refresh_token.is_none());
    }

    fn account_of(app: &App, id: &str) -> QoderAccountRecord {
        app.with_db(|db| db.get_qoder_account(id))
            .unwrap()
            .expect("账号")
    }

    /// 覆盖写：同一个文件、revision +1，令牌整体替换。
    #[test]
    fn rewriting_a_json_authorization_keeps_one_file() {
        let (app, _dir) = json_app();
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-1", "dt-old"))
                .unwrap();
        let before = row(&app, &saved.account_id, SOURCE_OWN_BROWSER);
        assert_eq!(secret_files(&app), 1);

        crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-1", "dt-new"))
            .unwrap();
        let after = row(&app, &saved.account_id, SOURCE_OWN_BROWSER);
        assert_eq!(secret_files(&app), 1, "同一份授权不允许留两个文件");
        assert_eq!(before.secret_ref, after.secret_ref, "覆盖写不换引用");

        let secret = load_json(
            &app,
            &account_of(&app, &saved.account_id),
            &after,
            CredentialKind::Oauth,
        )
        .unwrap();
        assert_eq!(
            secret.secret_id,
            before
                .secret_ref
                .unwrap()
                .trim_start_matches("localjson:v1:")
        );
        assert_eq!(secret.revision, 1);
        assert_eq!(secret.token(CredentialSlot::Access).unwrap(), "dt-new");
    }

    /// 关掉明文开关：**已有 JSON 行照旧读写**（文件早就落盘了，退回钥匙串既不减少
    /// 暴露还会把一份授权分裂成两处），但新账号重新走钥匙串。
    #[test]
    fn turning_consent_off_keeps_existing_json_rows_and_sends_new_ones_to_the_keychain() {
        let (app, _dir) = json_app();
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-1", "dt-old"))
                .unwrap();
        let before = row(&app, &saved.account_id, SOURCE_OWN_BROWSER);

        crate::auth::consent::set_json_storage(&app, false).unwrap();
        crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-1", "dt-new"))
            .unwrap();
        let after = row(&app, &saved.account_id, SOURCE_OWN_BROWSER);
        assert_eq!(
            before.secret_ref, after.secret_ref,
            "已有 JSON 行不许隐式迁移"
        );
        assert_eq!(secret_files(&app), 1);

        // 新账号 → 钥匙串。
        crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-2", "dt-kc"))
            .unwrap();
        let other = app
            .with_db(|db| db.find_qoder_account("qoder_cn", "u-2"))
            .unwrap()
            .expect("账号");
        let row = row(&app, &other.id, SOURCE_OWN_BROWSER);
        assert!(row.secret_ref.unwrap().starts_with("keychain:"));
        assert_eq!(secret_files(&app), 1, "关闭后不再新建明文文件");
    }

    // -- 钥匙串：比对与删除必须原子（复审 P0） ---------------------------

    /// 比对之后、删除之前另一个进程写入新令牌：旧 guard 必须被拒绝，新令牌完好。
    #[test]
    fn a_keychain_secret_rewritten_after_the_guard_is_refused() {
        let (app, _dir) = app_with(SecureStore::memory());
        let saved = crate::providers::qoder_auth::persist(
            &app,
            QoderEdition::Cn,
            &token("u-kc", "A-TOKEN"),
        )
        .unwrap();
        let account = account_of(&app, &saved.account_id);
        let row = row(&app, &saved.account_id, SOURCE_OWN_BROWSER);
        let guard = observe_for_delete(&app, &account, &row, CredentialKind::Oauth)
            .unwrap()
            .expect("钥匙串里有凭据");

        // 另一个进程对同一来源重新授权（钥匙串键名固定，引用不变）。
        crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-kc", "B-TOKEN"))
            .unwrap();

        let err = delete_guarded(&app, &account, &row, CredentialKind::Oauth, &guard)
            .expect_err("值变了必须拒绝");
        assert_eq!(err.kind, CredentialErrorKind::Conflict);
        assert_eq!(
            app.secure
                .get(&crate::providers::qoder_auth::access_key(&saved.account_id))
                .unwrap(),
            b"B-TOKEN",
            "新令牌必须完整保留"
        );
    }

    /// access 没变、只有 refresh 被更新：同样算变化，**两把键都不许删**。
    ///
    /// 旧实现只比对主键（access），然后连 refresh 一起删 —— 会删掉没纳入快照的新值。
    #[test]
    fn a_refresh_only_update_also_blocks_the_delete() {
        let (app, _dir) = app_with(SecureStore::memory());
        let saved = crate::providers::qoder_auth::persist(
            &app,
            QoderEdition::Cn,
            &token("u-rf", "A-TOKEN"),
        )
        .unwrap();
        let account = account_of(&app, &saved.account_id);
        let row = row(&app, &saved.account_id, SOURCE_OWN_BROWSER);
        let guard = observe_for_delete(&app, &account, &row, CredentialKind::Oauth)
            .unwrap()
            .expect("钥匙串里有凭据");

        // 只动 refresh。
        app.secure
            .set(
                &crate::providers::qoder_auth::refresh_key(&saved.account_id),
                b"R-NEW",
            )
            .unwrap();

        let err = delete_guarded(&app, &account, &row, CredentialKind::Oauth, &guard)
            .expect_err("整组值变了必须拒绝");
        assert_eq!(err.kind, CredentialErrorKind::Conflict);
        assert_eq!(
            app.secure
                .get(&crate::providers::qoder_auth::access_key(&saved.account_id))
                .unwrap(),
            b"A-TOKEN"
        );
        assert_eq!(
            app.secure
                .get(&crate::providers::qoder_auth::refresh_key(
                    &saved.account_id
                ))
                .unwrap(),
            b"R-NEW",
            "新 refresh 不许被误删"
        );
    }

    /// 删除操作**必须真的持有跨进程锁**：锁被占着时它只能报 `lock_timeout`，
    /// 不能"没锁也照着比对照着删"。
    #[test]
    fn the_keychain_delete_actually_waits_for_the_cross_process_lock() {
        let (app, _dir) = app_with(SecureStore::memory());
        let saved = crate::providers::qoder_auth::persist(
            &app,
            QoderEdition::Cn,
            &token("u-lk", "A-TOKEN"),
        )
        .unwrap();
        let account = account_of(&app, &saved.account_id);
        let row = row(&app, &saved.account_id, SOURCE_OWN_BROWSER);
        let guard = observe_for_delete(&app, &account, &row, CredentialKind::Oauth)
            .unwrap()
            .expect("钥匙串里有凭据");

        // 模拟**另一个进程**握着锁：真的起一个线程持有同一目录的锁。
        // （同一线程拿锁是可重入的，那样测不出"删除必须持锁"。）
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let lock_root = app.paths.data_dir.clone();
        let holder = std::thread::spawn(move || {
            let paths = paths(&lock_root);
            let store = LocalJsonStore::for_paths(&paths);
            let _held = store.lock_exclusive().expect("拿到锁");
            ready_tx.send(()).unwrap();
            let _ = release_rx.recv();
        });
        ready_rx.recv().unwrap();

        let err = delete_guarded(&app, &account, &row, CredentialKind::Oauth, &guard)
            .expect_err("拿不到锁就不能继续");
        assert_eq!(err.kind, CredentialErrorKind::LockTimeout, "{err}");
        assert_eq!(
            app.secure
                .get(&crate::providers::qoder_auth::access_key(&saved.account_id))
                .unwrap(),
            b"A-TOKEN",
            "没拿到锁时一个字节都不许动"
        );

        release_tx.send(()).unwrap();
        holder.join().unwrap();
        assert!(delete_guarded(&app, &account, &row, CredentialKind::Oauth, &guard).unwrap());
    }

    // -- 回滚的条件（复审 P0-B） -----------------------------------------

    /// A 写完、提交失败，期间 B 已经写入并**提交成功**：A 的回滚必须放弃。
    ///
    /// 只加锁不够 —— 锁保护的是"同一时刻只有一个在写"，不是"盘上还是我那一版"。
    /// 无条件写回快照会把 B 刚保存的有效令牌覆盖成旧值，而 B 的授权行还是 `valid`。
    #[test]
    fn a_late_rollback_never_overwrites_a_newer_write() {
        let (app, _dir) = app_with(SecureStore::memory());
        let account = account(&app, "qoder_cn", "u-rb");

        // A：观察（Absent）→ 写进去。
        let observed = observation(&app, &account.id, SOURCE_OWN_BROWSER);
        let mine = write(
            &app,
            &account,
            SOURCE_OWN_BROWSER,
            "A-TOKEN",
            Some("A-REFRESH"),
            None,
            &observed,
        )
        .unwrap();

        // B：完整的保存流程（写 + 提交）成功。
        crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-rb", "B-TOKEN"))
            .unwrap();

        // A 的提交落空（那一行已经存在），随后尝试回滚。
        let lost = commit(
            &app,
            &account.id,
            SOURCE_OWN_BROWSER,
            &mine,
            "valid",
            None,
            None,
            &expectation_of(&observed),
        )
        .unwrap();
        assert!(lost.is_none(), "Absent 撞上已存在的行必须落空");

        assert!(!mine.restore(&app), "盘上已经是 B 的那一版，回滚必须放弃");
        assert_eq!(
            app.secure
                .get(&crate::providers::qoder_auth::access_key(&account.id))
                .unwrap(),
            b"B-TOKEN",
            "赢家的 access 不许被旧快照覆盖"
        );
        assert_eq!(
            app.secure
                .get(&crate::providers::qoder_auth::refresh_key(&account.id))
                .unwrap(),
            b"rt-1",
            "赢家的 refresh 同样不许被覆盖"
        );
        let row = row(&app, &account.id, SOURCE_OWN_BROWSER);
        assert_eq!(row.auth_status, "valid");
        assert_eq!(
            read(
                &app,
                &account_of(&app, &account.id),
                &row,
                CredentialKind::Oauth
            )
            .unwrap(),
            "B-TOKEN"
        );
    }

    /// 反过来：盘上还是我写进去的那一版，回滚必须真的发生。
    ///
    /// 没有这条，"拒绝回滚"可能变成"永远不回滚"，而调用方是靠返回值判断的。
    #[test]
    fn a_rollback_still_restores_when_the_token_is_still_mine() {
        let (app, _dir) = app_with(SecureStore::memory());
        let account = account(&app, "qoder_cn", "u-rb2");
        // 先有一份旧令牌，才能观察"回滚到旧值"。
        crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-rb2", "OLD"))
            .unwrap();

        let observed = observation(&app, &account.id, SOURCE_OWN_BROWSER);
        let mine = write(
            &app,
            &account,
            SOURCE_OWN_BROWSER,
            "NEW-TOKEN",
            Some("NEW-REFRESH"),
            None,
            &observed,
        )
        .unwrap();
        assert!(mine.restore(&app), "还是我那一版，必须回滚");
        assert_eq!(
            app.secure
                .get(&crate::providers::qoder_auth::access_key(&account.id))
                .unwrap(),
            b"OLD",
            "旧令牌必须回来"
        );
    }

    // -- 竞争失败的收尾 -------------------------------------------------

    /// 提交落空时：**新建**的文件删掉（没人引用）。
    #[test]
    fn a_lost_commit_discards_a_new_file() {
        let (app, _dir) = json_app();
        let account = account(&app, "qoder_cn", "u-1");

        // 写文件之后才发现这一行已经被别人建好了。
        let first = write(
            &app,
            &account,
            SOURCE_OWN_BROWSER,
            "dt-1",
            None,
            None,
            &observation(&app, &account.id, SOURCE_OWN_BROWSER),
        )
        .unwrap();
        assert_eq!(secret_files(&app), 1);
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                None,
                Some(&keychain_reference(&account.id, SOURCE_OWN_BROWSER)),
                "valid",
                None,
                None,
            )
        })
        .unwrap();
        let lost = commit(
            &app,
            &account.id,
            SOURCE_OWN_BROWSER,
            &first,
            "valid",
            None,
            None,
            &QoderAuthExpectation::Absent {
                lifecycle: lifecycle(&app, &account.id),
            },
        )
        .unwrap();
        assert!(lost.is_none(), "别人先建好了行，本次提交必须落空");
        first.discard_unreferenced(&app);
        assert_eq!(secret_files(&app), 0, "没有人引用的新文件必须清掉");
    }

    /// 提交落空时：**覆盖写**的那份保持不动（盘上那份属于这条授权，
    /// 里面是刚拿到的有效令牌；回滚只会退回服务端可能已作废的旧值）。
    #[test]
    fn a_lost_commit_keeps_the_overwritten_file() {
        let (app, _dir) = json_app();
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-1", "dt-a"))
                .unwrap();
        let account = account_of(&app, &saved.account_id);
        let before = row(&app, &account.id, SOURCE_OWN_BROWSER);
        let overwritten = write(
            &app,
            &account,
            SOURCE_OWN_BROWSER,
            "dt-b",
            None,
            None,
            &observation(&app, &account.id, SOURCE_OWN_BROWSER),
        )
        .unwrap();

        let lost = commit(
            &app,
            &account.id,
            SOURCE_OWN_BROWSER,
            &overwritten,
            "valid",
            None,
            None,
            // 故意给一个过期的引用：模拟"引用被迁走了"。
            &QoderAuthExpectation::Present {
                secret_ref: Some("localjson:v1:moved".to_string()),
                lifecycle: lifecycle(&app, &account.id),
                auth_version: 1,
            },
        )
        .unwrap();
        assert!(lost.is_none());
        overwritten.discard_unreferenced(&app);

        assert_eq!(secret_files(&app), 1, "覆盖写的那份不许删");
        let secret = load_json(&app, &account, &before, CredentialKind::Oauth).unwrap();
        assert_eq!(
            secret.token(CredentialSlot::Access).unwrap(),
            "dt-b",
            "刚拿到的有效令牌要留在盘上"
        );
    }

    /// **审计 P0-1 / P0-2 的跨进程证据**：多个进程同时覆盖**同一来源**的凭据。
    ///
    /// 锁域把整段"观察 → 写 → 提交"串起来之后，最终必须满足：
    /// **行的 fingerprint == 盘上令牌的 fingerprint == 读路径拿到的令牌**。
    /// 之前只锁"写"的那一版，允许出现"行记 A、文件是 B"。
    #[test]
    fn concurrent_rewrites_keep_the_row_and_the_file_in_sync() {
        let (app, _dir) = json_app();
        // 先有一份，让子进程全部走"覆盖写"这条路径。
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-rw", "BASE"))
                .unwrap();
        let account = account_of(&app, &saved.account_id);

        let start_at = now_ms() + 1_500;
        let children: Vec<_> = (0..4)
            .map(|index| {
                child_command(&app, "u-rw", &format!("REWRITE-{index}"), start_at)
                    .spawn()
                    .expect("spawn child")
            })
            .collect();

        let mut wins = 0;
        for child in children {
            let output = child.wait_with_output().expect("wait child");
            if String::from_utf8_lossy(&output.stdout).contains("CHILD-RESULT=ok") {
                wins += 1;
            }
        }
        assert!(wins >= 1, "至少有一个子进程要成功");

        let rows = app
            .with_db(|db| db.list_qoder_authorizations(&account.id))
            .unwrap();
        assert_eq!(rows.len(), 1, "同一来源只能有一行");

        let row = &rows[0];
        let reference = row.secret_ref.as_deref().expect("引用");
        let secret_id = reference.trim_start_matches("localjson:v1:");
        let secret = LocalJsonStore::for_paths(&app.paths)
            .load(secret_id)
            .expect("文件必须存在");
        let on_disk = secret
            .token(CredentialSlot::Access)
            .expect("文件里必须有令牌")
            .to_string();
        assert!(
            on_disk.starts_with("REWRITE-"),
            "盘上应当是某个子进程写进去的令牌：{on_disk}"
        );

        // 三者必须是同一个胜者。
        assert_eq!(
            row.fingerprint.as_deref(),
            Some(crate::securestore::fingerprint(on_disk.as_bytes()).as_str()),
            "行的指纹必须对应盘上的令牌"
        );
        let account = account_of(&app, &account.id);
        assert_eq!(
            read(&app, &account, row, CredentialKind::Oauth).unwrap(),
            on_disk,
            "读路径拿到的必须就是盘上那一份"
        );
        assert_eq!(secret_files(&app), 1, "不许出现第二份文件");
    }

    // -- 归属与错误分类 -------------------------------------------------
    #[test]
    fn a_secret_of_another_authorization_is_refused_everywhere() {
        let (app, _dir) = json_app();
        let account = account(&app, "qoder_cn", "u-1");
        // 文件里的 authorizationId 是"另一条授权"的。
        let store = LocalJsonStore::for_paths(&app.paths);
        let secret = store
            .create(SecretDraft::oauth(
                "qz_someone_else",
                "qoder_cn",
                "TOKEN-OF-B",
            ))
            .unwrap();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                None,
                Some(&format!("localjson:v1:{}", secret.secret_id)),
                "valid",
                None,
                None,
            )
        })
        .unwrap();
        let row = row(&app, &account.id, SOURCE_OWN_BROWSER);

        let err = read(&app, &account, &row, CredentialKind::Oauth).unwrap_err();
        assert_eq!(err.kind, CredentialErrorKind::Conflict, "{err}");
        assert!(!err.message.contains("TOKEN-OF-B"), "错误消息不许带令牌");
        // 删除同样拒绝：不能删别人的文件。
        assert_eq!(
            delete(&app, &account, &row, CredentialKind::Oauth)
                .unwrap_err()
                .kind,
            CredentialErrorKind::Conflict
        );
        assert!(store.load(&secret.secret_id).is_ok(), "文件必须还在");
    }

    #[test]
    fn a_platform_or_kind_mismatch_is_refused() {
        let (app, _dir) = json_app();
        let cn = account(&app, "qoder_cn", "u-1");
        let store = LocalJsonStore::for_paths(&app.paths);
        // 文件是 global 的，行却挂在 CN 账号上。
        let secret = store
            .create(SecretDraft::oauth(
                authorization_id(&cn.id, SOURCE_OWN_BROWSER),
                "qoder_global",
                "TOKEN",
            ))
            .unwrap();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &cn.id,
                SOURCE_OWN_BROWSER,
                None,
                Some(&format!("localjson:v1:{}", secret.secret_id)),
                "valid",
                None,
                None,
            )
        })
        .unwrap();
        let row = row(&app, &cn.id, SOURCE_OWN_BROWSER);
        assert_eq!(
            read(&app, &cn, &row, CredentialKind::Oauth)
                .unwrap_err()
                .kind,
            CredentialErrorKind::Conflict
        );
        // 类型不符（PAT 文件冒充设备授权）同样拒绝。
        assert_eq!(
            read(&app, &cn, &row, CredentialKind::Pat).unwrap_err().kind,
            CredentialErrorKind::Conflict
        );
    }

    #[test]
    fn a_missing_file_is_not_found_and_never_falls_back_to_the_keychain() {
        let (app, _dir) = json_app();
        let account = account(&app, "qoder_cn", "u-1");
        let missing = uuid::Uuid::new_v4().to_string();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                None,
                Some(&format!("localjson:v1:{missing}")),
                "valid",
                None,
                None,
            )
        })
        .unwrap();
        // 钥匙串里放一份同名令牌：读路径也不能去拿它。
        app.secure
            .set(
                &crate::providers::qoder_auth::access_key(&account.id),
                b"keychain-token",
            )
            .unwrap();
        let row = row(&app, &account.id, SOURCE_OWN_BROWSER);
        assert_eq!(
            read(&app, &account, &row, CredentialKind::Oauth)
                .unwrap_err()
                .kind,
            CredentialErrorKind::NotFound
        );
    }

    // -- 删除 -----------------------------------------------------------

    #[test]
    fn deleting_one_source_leaves_the_other_alone() {
        let (app, _dir) = json_app();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-1", "dt-1"))
                .unwrap();
        crate::providers::qoder_pat::persist(
            &app,
            QoderEdition::Cn,
            pat,
            &Identity {
                uid: "u-1".into(),
                name: None,
                email: None,
            },
        )
        .unwrap();
        let account = account_of(&app, &saved.account_id);
        let pat_row = row(&app, &saved.account_id, SOURCE_OWN_PAT);
        assert!(delete(&app, &account, &pat_row, CredentialKind::Pat).unwrap());
        assert_eq!(secret_files(&app), 1, "只删掉 PAT 那一份");
        assert!(
            !delete(&app, &account, &pat_row, CredentialKind::Pat).unwrap(),
            "删除幂等"
        );
    }

    // -- 真实跨进程竞争 -------------------------------------------------

    /// 起跑时刻：给所有子进程一个共同的"发令枪"。
    ///
    /// 没有它，第一个子进程往往在其余进程还没启动时就跑完了，竞争窗口根本不存在
    /// —— 测试会一直"通过"，直到 CI 上真的撞上一次。
    fn child_command(
        app: &App,
        uid: &str,
        token_value: &str,
        start_at_ms: i64,
    ) -> std::process::Command {
        let exe = std::env::current_exe().expect("current_exe");
        let mut command = std::process::Command::new(exe);
        command
            .args([
                "--exact",
                "providers::qoder_secret::tests::child_process_grant",
                "--nocapture",
            ])
            .env("AICC_TEST_QODER_ROOT", &app.paths.data_dir)
            .env("AICC_TEST_QODER_UID", uid)
            .env("AICC_TEST_QODER_TOKEN", token_value)
            .env("AICC_TEST_QODER_START_AT", start_at_ms.to_string())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        command
    }

    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    }

    fn wait_for_go() {
        let Ok(raw) = std::env::var("AICC_TEST_QODER_START_AT") else {
            return;
        };
        let Ok(at) = raw.parse::<i64>() else { return };
        while now_ms() < at {
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    /// 子进程侧：启动一次设备授权落库。父进程用环境变量唤醒，正常跑测试时是空跑。
    #[test]
    fn child_process_grant() {
        let Ok(root) = std::env::var("AICC_TEST_QODER_ROOT") else {
            return;
        };
        let uid = std::env::var("AICC_TEST_QODER_UID").expect("uid");
        let access = std::env::var("AICC_TEST_QODER_TOKEN").expect("token");
        let app = App::with_secure(paths(std::path::Path::new(&root)), SecureStore::memory());
        // 明文同意写在数据库里，父进程已经设好；子进程只读它。
        wait_for_go();
        let token = DeviceToken {
            access_token: access,
            refresh_token: None,
            expires_at_ms: None,
            identity: Identity {
                uid,
                name: None,
                email: None,
            },
        };
        match crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token) {
            Ok(_) => println!("CHILD-RESULT=ok"),
            Err(err) => println!(
                "CHILD-RESULT=err:{}",
                err.data
                    .as_ref()
                    .and_then(|data| data.get("kind"))
                    .and_then(|kind| kind.as_str())
                    .unwrap_or("unknown")
            ),
        }
    }

    /// 四（N）个进程同时给同一个账号做首次设备授权。
    ///
    /// 断言的是**最终状态**，不是"几个成功"：后启动的子进程可能看到胜者刚建好的
    /// 行，于是走"覆盖写"这条正常路径（同一份授权换了新令牌），它也是成功的。
    /// 真正不许出现的是：
    ///
    /// 1. 两行授权（唯一键兜住了）；
    /// 2. **没有任何行引用的有效令牌文件**（文件里是明文令牌，删不掉就是长期暴露）；
    /// 3. 行指着一份不存在的文件。
    #[test]
    fn concurrent_first_grants_leave_exactly_one_secret_and_one_row() {
        let (app, _dir) = json_app();
        // 账号先建好，让竞争只发生在授权行上。
        let account = account(&app, "qoder_cn", "u-race");

        // 给子进程 1.5 秒热启动，然后一起放行：竞争窗口必须真的存在。
        let start_at = now_ms() + 1_500;
        let children: Vec<_> = (0..4)
            .map(|index| {
                child_command(&app, "u-race", &format!("CHILD-TOKEN-{index}"), start_at)
                    .spawn()
                    .expect("spawn child")
            })
            .collect();

        let mut wins = 0;
        let mut failures = 0;
        for child in children {
            let output = child.wait_with_output().expect("wait child");
            let stdout = String::from_utf8_lossy(&output.stdout);
            if stdout.contains("CHILD-RESULT=ok") {
                wins += 1;
            } else if stdout.contains("CHILD-RESULT=err:") {
                failures += 1;
            } else {
                panic!(
                    "子进程没有给出预期结果：stdout={stdout} stderr={}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
        }
        assert!(wins >= 1, "至少有一个子进程要成功");
        assert_eq!(wins + failures, 4, "每个子进程都要给出结论");

        let rows = app
            .with_db(|db| db.list_qoder_authorizations(&account.id))
            .unwrap();
        assert_eq!(rows.len(), 1, "同一来源只能有一行授权");

        let store = LocalJsonStore::for_paths(&app.paths);
        let scan = store.scan().unwrap();
        assert_eq!(scan.secrets.len(), 1, "不许留下没有任何行引用的令牌文件");
        assert!(scan.broken.is_empty());
        assert_eq!(
            rows[0].secret_ref.as_deref(),
            Some(format!("localjson:v1:{}", scan.secrets[0].secret_id).as_str()),
            "那一行必须指着留下的这份文件"
        );

        // 不只是"文件数量对"：文件里的令牌必须真是某个子进程写进去的，
        // 而且与读路径拿到的一致 —— 否则可能是被交错写坏的半份内容。
        let account = account_of(&app, &account.id);
        let token = read(&app, &account, &rows[0], CredentialKind::Oauth).unwrap();
        assert!(
            token.starts_with("CHILD-TOKEN-"),
            "令牌内容必须来自某个子进程：{token}"
        );
        let secret = store
            .load(
                rows[0]
                    .secret_ref
                    .as_deref()
                    .unwrap()
                    .trim_start_matches("localjson:v1:"),
            )
            .unwrap();
        // revision 只统计"覆盖写"的次数：赢家是新建（0），后来者若走覆盖写会 +1。
        // 4 个子进程最多再加 3 次；再多就说明有没必要的重写。
        assert!(secret.revision <= 3, "revision={}", secret.revision);
        assert_eq!(
            secret.token(CredentialSlot::Access).unwrap(),
            token,
            "文件里的令牌与读路径必须一致"
        );
    }

    // -- 辅助结构 -------------------------------------------------------

    #[test]
    fn expectation_tracks_absence_and_the_observed_reference() {
        let (app, _dir) = json_app();
        let account = account(&app, "qoder_cn", "u-1");
        let absent = observation(&app, &account.id, SOURCE_OWN_BROWSER);
        assert!(matches!(
            expectation_of(&absent),
            QoderAuthExpectation::Absent { .. }
        ));
        assert!(expectation_of(&absent).is_absent());
        // 首次授权的预期也必须带生命周期（复审 P0-2）。
        assert_eq!(expectation_of(&absent).lifecycle(), &absent.lifecycle);

        crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-1", "dt-1"))
            .unwrap();
        let row = row(&app, &account.id, SOURCE_OWN_BROWSER);
        match expectation_of(&observation(&app, &account.id, SOURCE_OWN_BROWSER)) {
            QoderAuthExpectation::Present { secret_ref, .. } => {
                assert_eq!(secret_ref, row.secret_ref)
            }
            _ => panic!("已有行必须是 Present"),
        }
    }

    /// **复审 P0-1 的确定性复现**：A 观察到 Absent、B 抢先建好行与文件之后，
    /// A 的写入必须仍然走"新建"，**不能**被 B 的行带成"覆盖写"。
    ///
    /// 旧实现里 `write()` 会自己再查一次行，于是直接覆盖 B 的文件；随后 A 的
    /// `Absent` 提交撞唯一键报冲突 —— 而 B 的令牌已经被悄悄改写，且行还是 valid。
    #[test]
    fn a_stale_absent_observation_never_overwrites_the_winner() {
        let (app, _dir) = json_app();
        let account = account(&app, "qoder_cn", "u-race");

        // A：读到"没有这一行"。
        let stale = observation(&app, &account.id, SOURCE_OWN_BROWSER);
        assert!(stale.row.is_none());

        // B：完成一次授权，建好文件与行。
        let winner = crate::providers::qoder_auth::persist(
            &app,
            QoderEdition::Cn,
            &token("u-race", "B-TOKEN"),
        )
        .unwrap();
        let winner_row = row(&app, &winner.account_id, SOURCE_OWN_BROWSER);
        let winner_ref = winner_row.secret_ref.clone().expect("引用");
        let store = LocalJsonStore::for_paths(&app.paths);
        let winner_secret_id = winner_ref.trim_start_matches("localjson:v1:").to_string();
        let winner_before = store.load(&winner_secret_id).unwrap();

        // A 继续写：**必须**走新建（同一次观察），而不是覆盖 B 的文件。
        let write = write(
            &app,
            &account,
            SOURCE_OWN_BROWSER,
            "A-TOKEN",
            None,
            None,
            &stale,
        )
        .unwrap();
        assert_eq!(write.reference().starts_with("localjson:v1:"), true);
        assert_ne!(write.reference(), winner_ref, "不许写到赢家的文件上");

        // A 的条件提交必须落空（那一行已经存在），并清掉自己的新文件。
        let lost = commit(
            &app,
            &account.id,
            SOURCE_OWN_BROWSER,
            &write,
            "valid",
            None,
            None,
            &expectation_of(&stale),
        )
        .unwrap();
        assert!(lost.is_none(), "首次授权的预期撞上已存在的行");
        write.discard_unreferenced(&app);

        // 赢家的文件：内容与 revision 一字未动，令牌仍原样可读。
        let after = store.load(&winner_secret_id).unwrap();
        assert_eq!(after.revision, winner_before.revision, "revision 不许变");
        assert_eq!(
            after.token(CredentialSlot::Access).unwrap(),
            "B-TOKEN",
            "赢家令牌必须原样可读"
        );
        assert_eq!(secret_files(&app), 1, "只留赢家那一份");
        let row = row(&app, &winner.account_id, SOURCE_OWN_BROWSER);
        assert_eq!(row.secret_ref.as_deref(), Some(winner_ref.as_str()));
        assert_eq!(row.auth_status, "valid");
    }

    #[test]
    fn a_pat_cannot_be_written_with_a_refresh_token() {
        let (app, _dir) = json_app();
        let account = account(&app, "qoder_cn", "u-1");
        let err = write(
            &app,
            &account,
            SOURCE_OWN_PAT,
            "qp_abcdefghijklmnopqrstuvwxyz012345",
            Some("rt"),
            None,
            &observation(&app, &account.id, SOURCE_OWN_PAT),
        )
        .err()
        .expect("PAT 带 refresh 必须报错");
        assert_eq!(err.code, RpcError::invalid_params("").code);
    }

    #[test]
    fn own_sources_are_the_only_ones_holding_our_credentials() {
        assert!(is_own_source(SOURCE_OWN_BROWSER));
        assert!(is_own_source(SOURCE_OWN_PAT));
        assert!(!is_own_source(SOURCE_MAGPIE));
        // 两个来源的键必须不同，否则删除会互相误伤。
        assert_ne!(
            keychain_keys("qa_1", SOURCE_OWN_BROWSER),
            keychain_keys("qa_1", SOURCE_OWN_PAT)
        );
        assert_eq!(keychain_keys("qa_1", SOURCE_OWN_PAT).len(), 1);
    }

    #[test]
    fn the_json_draft_uses_the_real_edition_and_the_row_id() {
        let (app, _dir) = json_app();
        let account = account(&app, "qoder_global", "u-1");
        let write = write(
            &app,
            &account,
            SOURCE_OWN_BROWSER,
            "dt-1",
            None,
            None,
            &observation(&app, &account.id, SOURCE_OWN_BROWSER),
        )
        .unwrap();
        let secret_id = write.reference().trim_start_matches("localjson:v1:");
        let secret = LocalJsonStore::for_paths(&app.paths)
            .load(secret_id)
            .unwrap();
        assert_eq!(secret.platform, "qoder_global");
        assert_eq!(
            secret.authorization_id,
            authorization_id(&account.id, SOURCE_OWN_BROWSER)
        );
        assert_eq!(secret.source, SecretSource::Own);
        assert_eq!(secret.credential_kind, CredentialKind::Oauth);
        // 服务端没给 refresh 时文件里就没有 refresh 槽位。
        assert!(secret.credentials.refresh_token.is_none());
        let _ = Credentials::access_only("x");
    }
}
