//! Qoder Personal Access Token 授权（PR #73 的**备用**入口）。
//!
//! 主入口仍是浏览器设备授权（`qoder_auth`）。PAT 存在的意义：设备授权被网络策略
//! 挡住、或者用户就想用一张长期凭据时，还有第二条能走通的路。两条路互不替代，
//! 也互不覆盖——它们是 `qoder_authorizations` 里两行独立的来源。
//!
//! ## 几条不能妥协的规则
//!
//! 1. **PAT 只在钥匙串里**。SQLite 只有 `secret_ref`、指纹、状态；日志、RPC 响应、
//!    错误信息里都不出现明文（错误信息只带 HTTP 状态码与归一化后的原因）。
//! 2. **不刷新**。PAT 是用户在自己账户页生成的长期凭据，AICC 没有轮换它的能力；
//!    失效（401）只能提示用户重新生成，不能假装还能用。
//! 3. **"保存成功" ≠ "登录已验证"**。落库前会真的打一次 `userinfo` 换身份：
//!    拿不到 uid 就不入库。但 `userinfo` 通得过**不代表** Credits 接口通得过，
//!    所以返回体里把 `identity` 与 `credits` 分开报，后者恒为 `unknown`，
//!    由 #75 的探测接口单独回答。
//! 4. **两个站点不通用**。CN 的 PAT 发到 International 的 `userinfo` 只会 401；
//!    这里不做任何"失败了换个域名再试"的回落——那会把令牌发到错误的站点。
//!
//! 官方文档：<https://docs.qoder.com/cli/authentication>；
//! 生成入口：`https://qoder.com/account/integrations`（国际）/
//! `https://qoder.cn/account/integrations`（国内）。

use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};

use crate::app::App;
use crate::credentials::localjson::CredentialKind;
use crate::credentials::CredentialErrorKind;
use crate::providers::qoder_auth::{parse_identity, userinfo_url, Identity};
use crate::providers::qoder_edition::QoderEdition;
use crate::providers::qoder_secret;
use crate::rpc::{RpcError, RpcResult};
use crate::securestore::fingerprint;
use crate::storage::qoder_accounts::{
    QoderAccountRecord, QoderAuthRecord, QoderLifecycle, SOURCE_OWN_PAT,
};

/// `userinfo` 单次请求上限。换身份只是一次只读请求，12 秒足够。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(12);

pub const KIND_MALFORMED: &str = "malformed";
pub const KIND_REJECTED: &str = "rejected";
pub const KIND_FORBIDDEN: &str = "forbidden";
pub const KIND_NETWORK: &str = "network";
pub const KIND_PROFILE_MISSING: &str = "profile_missing";
pub const KIND_UNKNOWN_ACCOUNT: &str = "unknown_account";
pub const KIND_NO_SECRET: &str = "no_secret";

/// PAT 最短长度。真实的 PAT 远长于此；这里只是挡住"把邮箱/空串粘进来"这类误操作，
/// **不假装知道**官方格式（格式随时可能变，硬编码正则只会在某天误杀合法令牌）。
const MIN_LEN: usize = 20;

/// 错误只带**类别与归一化文案**，从不构造含令牌的字符串，所以 `Debug` 是安全的。
#[derive(Debug)]
pub struct PatError {
    pub kind: &'static str,
    pub message: String,
}

impl PatError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn to_json(&self) -> Value {
        json!({ "kind": self.kind, "message": self.message })
    }
}

/// 归一化：去掉首尾空白，拒绝空串、含空白的串、过短的串。
///
/// 不做大小写/前后缀改写：PAT 是**不透明**凭据，任何"聪明"的处理都可能改坏它。
pub fn normalize(raw: &str) -> Result<String, PatError> {
    let token = raw.trim();
    if token.is_empty() {
        return Err(PatError::new(KIND_MALFORMED, "PAT 不能为空"));
    }
    if token.len() < MIN_LEN {
        return Err(PatError::new(
            KIND_MALFORMED,
            "PAT 长度明显不对（看起来不是完整令牌）",
        ));
    }
    if token.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(PatError::new(
            KIND_MALFORMED,
            "PAT 中不应包含空格或换行，请重新复制",
        ));
    }
    Ok(token.to_string())
}

/// 钥匙串键：`qoder/<account_id>/pat/token`。
///
/// 与浏览器授权的 `.../own/access` 分开：两条来源各有自己的凭据，
/// 删掉其中一条永远不该影响另一条。
pub fn pat_key(account_id: &str) -> String {
    format!("qoder/{account_id}/pat/token")
}

fn client() -> Result<&'static reqwest::Client, PatError> {
    static CLIENT: OnceLock<Result<reqwest::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| PatError::new(KIND_NETWORK, format!("HTTP 客户端创建失败：{e}")))
}

/// 用一次 `userinfo` 换身份。**这是唯一能证明"这张 PAT 属于这个账号"的动作。**
pub async fn verify(edition: QoderEdition, pat: &str) -> Result<Identity, PatError> {
    let client = client()?;
    let resp = client
        .get(userinfo_url(edition))
        .header("Authorization", format!("Bearer {pat}"))
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|e| PatError::new(KIND_NETWORK, network_detail(&e)))?;

    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    match status {
        200..=299 => {
            // `data` 包裹只是兼容分支：官方 userinfo 目前是顶层字段。
            let source = body
                .get("data")
                .filter(|data| data.is_object())
                .unwrap_or(&body);
            match parse_identity(&json!({}), Some(source)) {
                Some(identity) if !identity.uid.trim().is_empty() => Ok(identity),
                _ => Err(PatError::new(
                    KIND_PROFILE_MISSING,
                    "服务端没有返回账号 uid，无法确认这张 PAT 属于谁，未保存",
                )),
            }
        }
        401 => Err(PatError::new(
            KIND_REJECTED,
            "PAT 被服务端拒绝（401）：可能已撤销、已过期，或不属于这个站点",
        )),
        403 => Err(PatError::new(
            KIND_FORBIDDEN,
            "PAT 没有权限访问身份接口（403）：请检查生成时勾选的权限范围",
        )),
        other => Err(PatError::new(
            KIND_NETWORK,
            format!("身份接口返回 HTTP {other}"),
        )),
    }
}

/// 网络错误归一化：reqwest 的文本可能带 URL / 代理地址，不需要出现在界面上，
/// 但**也不会**带出 Bearer 令牌——这里只保留错误类别。
fn network_detail(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        "请求超时，请检查网络后重试".to_string()
    } else if err.is_connect() {
        "连接失败，请检查网络后重试".to_string()
    } else {
        "网络请求失败".to_string()
    }
}

pub struct Persisted {
    pub account_id: String,
    pub edition: &'static str,
    pub uid: String,
    pub nickname: String,
    pub email: Option<String>,
    pub fingerprint: String,
    /// 这次 PAT 落在哪：`keychain` / `localjson`（CRED-3b）。
    pub credential_storage: &'static str,
}

impl Persisted {
    /// 对外 JSON。**刻意**把 `identity` 与 `credits` 分开：
    /// 前者是本次真实验证过的（`userinfo` 换到了 uid），后者一律 `unknown`。
    pub fn to_json(&self) -> Value {
        json!({
            "accountId": self.account_id,
            "edition": self.edition,
            "uid": self.uid,
            "nickname": self.nickname,
            "email": self.email,
            "fingerprint": self.fingerprint,
            "source": SOURCE_OWN_PAT,
            "hasSecret": true,
            "credentialStorage": self.credential_storage,
            "identity": "verified",
            "credits": "unknown",
            "warning": "PAT 已保存并通过身份验证；它是否能领取 Credits 需要单独检测，两者不是同一件事",
        })
    }
}

/// 落库：PAT 按**这一行现在是什么**落到钥匙串或 JSON，账号与授权行进 SQLite。
///
/// 顺序与浏览器授权一致：写凭据（唯一需要显式回滚的一步）→ 条件提交授权行。
/// PAT 与设备授权是两个文件 / 两组键，任何一步失败都不会动到另一份。
pub fn persist(
    app: &App,
    edition: QoderEdition,
    pat: &str,
    identity: &Identity,
) -> RpcResult<Persisted> {
    // 与浏览器授权一致：整个"观察 → 写 → 提交 → 失败清理"在同一凭据事务里。
    let _txn = qoder_secret::begin(app)?;

    // 现在看这是不是一行新账号：决定失败时要不要连行一起清掉。
    let created = app
        .with_db(|db| db.find_qoder_account(edition.as_str(), &identity.uid))?
        .is_none();

    let account = app.with_db(|db| {
        db.upsert_qoder_account(
            edition.as_str(),
            &identity.uid,
            identity.name.as_deref(),
            identity.email.as_deref(),
        )
    })?;
    // 只有"本次真的新建"才记下它的 generation：失败清理必须限定在这一代，
    // 否则同一个 UID 删了又建时会把别人刚建的账号删掉（审计 P1-1）。
    let created_generation = created.then(|| account.generation.clone());

    // **只观察一次**：写凭据的落点与条件提交的预期都来自这一份快照（复审 P0-1）。
    let observed = match qoder_secret::observe(app, &account.id, SOURCE_OWN_PAT) {
        Ok(observed) => observed,
        Err(err) => {
            qoder_secret::cleanup_created_account(app, &account.id, created_generation.as_deref());
            return Err(err);
        }
    };

    // PAT 是长期凭据：不带 refresh，也没有到期时间。
    let write = match qoder_secret::write(app, &account, SOURCE_OWN_PAT, pat, None, None, &observed)
    {
        Ok(write) => write,
        Err(err) => {
            qoder_secret::cleanup_created_account(app, &account.id, created_generation.as_deref());
            return Err(err);
        }
    };

    let digest = fingerprint(pat.as_bytes());
    let expectation = qoder_secret::expectation_of(&observed);
    let committed = qoder_secret::commit(
        app,
        &account.id,
        SOURCE_OWN_PAT,
        &write,
        "valid",
        None,
        Some(&digest),
        &expectation,
    );

    match committed {
        Ok(Some(_)) => Ok(Persisted {
            account_id: account.id,
            edition: edition.as_str(),
            uid: identity.uid.clone(),
            nickname: account.nickname,
            email: account.email,
            fingerprint: digest,
            credential_storage: write.storage().as_str(),
        }),
        Ok(None) => {
            write.discard_unreferenced(app);
            Err(qoder_secret::conflict_error())
        }
        Err(err) => {
            write.restore(app);
            qoder_secret::cleanup_created_account(app, &account.id, created_generation.as_deref());
            Err(err)
        }
    }
}

/// 读回已存的 PAT。**只在 worker 内部使用**，不经过 RPC。
///
/// 按授权行的 `secret_ref` 分派：`localjson:v1:` 读文件、`keychain:` 读钥匙串。
/// 未知引用、归属不符、文件不可读都不回落钥匙串 —— 那会让"迁移后还偶尔弹窗"
/// 这件事重新出现，也会掩盖串号。
pub fn read_secret(
    app: &App,
    account: &QoderAccountRecord,
    row: &QoderAuthRecord,
) -> Result<String, PatError> {
    qoder_secret::read(app, account, row, CredentialKind::Pat).map_err(|err| {
        let kind = match err.kind {
            CredentialErrorKind::NotFound => KIND_NO_SECRET,
            other => other.as_str(),
        };
        PatError::new(
            kind,
            format!("读不到 PAT：{}", crate::rpc::sanitize(&err.message)),
        )
    })
}

/// 重新检查一份已存的 PAT（界面上的「检测」按钮）。
///
/// 结论会写回授权状态：**只改状态，不推进 `auth_epoch`**——状态是观测结论，
/// 不是新的凭据事实。
pub async fn recheck(
    app: &App,
    edition: QoderEdition,
    account_id: &str,
) -> Result<Identity, PatError> {
    // 请求前的生命周期：网络回来时要确认"还是这个账号、这份授权"，否则不写状态。
    // 账号 id 由 `(edition, uid)` 决定，删号重建后 id 相同、`auth_epoch` 也归零，
    // 所以必须带上 `generation`。
    let account = app
        .with_db(|db| db.get_qoder_account(account_id))
        .map_err(|err| PatError::new(KIND_UNKNOWN_ACCOUNT, err.message))?
        .ok_or_else(|| PatError::new(KIND_UNKNOWN_ACCOUNT, "Qoder 账号不存在"))?;
    let lifecycle = app
        .with_db(|db| db.qoder_lifecycle(account_id))
        .map_err(|err| PatError::new(KIND_NO_SECRET, err.message))?
        .ok_or_else(|| PatError::new(KIND_UNKNOWN_ACCOUNT, "Qoder 账号不存在"))?;
    let row = app
        .with_db(|db| db.get_qoder_authorization(account_id, SOURCE_OWN_PAT))
        .map_err(|err| PatError::new(KIND_NO_SECRET, err.message))?
        .ok_or_else(|| PatError::new(KIND_NO_SECRET, "这个账号还没有保存过 PAT"))?;

    let pat = read_secret(app, &account, &row)?;
    match verify(edition, &pat).await {
        Ok(identity) => {
            write_status_if_current(app, account_id, "valid", &lifecycle);
            Ok(identity)
        }
        Err(err) => {
            // **只有 401 才说明"这份凭据不被接受了"**。403 是权限问题：
            // 把它写成 invalid，用户会以为令牌坏了去重新生成，其实只是没勾权限。
            if err.kind == KIND_REJECTED {
                write_status_if_current(app, account_id, "invalid", &lifecycle);
            }
            Err(err)
        }
    }
}

/// 带生命周期核验地写授权状态。
///
/// 用户在请求返回期间换了 PAT（`auth_epoch` 前进），这次结果说的就是**旧令牌**，
/// 写进去只会把新授权标脏；若期间账号被删掉又用同一个 UID 登录，写进去就是把
/// 另一个账号的凭据标脏。所以两者都对不上就什么都不写。
fn write_status_if_current(app: &App, account_id: &str, status: &str, lifecycle: &QoderLifecycle) {
    let _ = app.with_db(|db| {
        db.set_qoder_auth_status_if_current(account_id, SOURCE_OWN_PAT, status, lifecycle)
    });
}

#[derive(Debug)]
pub struct Removed {
    pub removed: bool,
    pub secret_deleted: bool,
    /// 移除期间这一行被别的流程改过（例如另一个窗口刚保存了新 PAT）。
    /// 这种时候**什么都没删**，让用户刷新后重试。
    pub authorization_changed: bool,
    /// 行已经摘掉了，但凭据没能删干净 —— 已把行以 `missing` 放回，等待重试。
    /// 这时候**绝不能**报"已移除"：那份令牌还在盘上，只是暂时没有有效引用。
    pub pending_cleanup: bool,
}

/// 删掉 PAT 这一份来源。
///
/// ## 顺序：先摘引用，再销毁凭据
///
/// 反过来（先删凭据、再删行）会有一个真实的数据丢失路径：用户删 PAT 的同一秒
/// 另一个窗口保存了新 PAT，新令牌写进**同一个文件**（覆盖写不换引用），
/// 而移除流程正在按旧快照删那个文件 —— 结果行还显示 `valid`，令牌却没了。
///
/// 现在改成：
///
/// ```text
/// ① 读一次快照（行 + 生命周期）并记下凭据版本（JSON revision / 钥匙串字节）
/// ② 带核验删行：`secret_ref` + `auth_version` + `generation` 任一变了就整次放弃
/// ③ 删凭据：版本对不上（期间被重写）就**不删**，把行以 `missing` 放回
/// ④ 其他删除失败同样把行放回 `missing`，返回可重试的错误
/// ```
///
/// 整个 ①→④ 还在**同一个凭据事务**里（`qoder_secret::begin`）：另一个进程的
/// "重新保存 PAT"必须等这次移除做完，否则它可能在 ② 与 ③ 之间插进来插入新行，
/// 再被 ③ 的旧 guard 删掉（审计 P0-3）。
///
/// ## 为什么 ③ 冲突时不能报"已移除"（复审 P0-A）
///
/// ② 成功之后这一行就没了。若此时才发现文件已被重写成新令牌（`revision` 变了），
/// 直接返回 `removed: true` 会把一份**有效的、没有任何行引用的令牌**留在盘上，
/// 而且再点一次移除也找不到它（行已经没了）。所以这个分支必须：
///
/// - 把行以 `missing` 放回（可定位、可重试）；
/// - `missing` 状态下 `credits` 不会使用它（见 `credits::resolve_token`）；
/// - 返回 `removed: false` + `pending_cleanup: true`，绝不假报成功。
///
/// ## 崩溃窗口
///
/// ② 与 ③ 之间进程被强杀会留下一个没有行引用的文件。这是**有意**选的：
/// 宁可留一份可被 CRED-4/5 核对扫到的 `0600` 孤儿文件，也不能删掉用户刚保存的
/// 新令牌。
pub fn remove(app: &App, account_id: &str) -> RpcResult<Removed> {
    remove_with(app, account_id, || {}, || {})
}

/// `before_row_delete` / `after_row_delete` 是测试用的确定性交错点：
/// 前者在"读完快照"之后、"删行"之前执行；后者在"行已删、凭据未删"之间执行。
pub(crate) fn remove_with(
    app: &App,
    account_id: &str,
    before_row_delete: impl FnOnce(),
    after_row_delete: impl FnOnce(),
) -> RpcResult<Removed> {
    // 观测、删行、删凭据三步必须在同一个凭据事务里：否则"删行之后、删凭据之前"
    // 另一个进程可以重新插入同一来源的授权，随后被旧 guard 删掉（审计 P0-3）。
    let _txn = qoder_secret::begin(app)?;

    let account = app
        .with_db(|db| db.get_qoder_account(account_id))?
        .ok_or_else(|| RpcError::not_found(format!("Qoder 账号不存在：{account_id}")))?;

    // ① 只观察一次：行、生命周期、凭据版本都来自这一刻。
    let observed = qoder_secret::observe(app, account_id, SOURCE_OWN_PAT)?;
    let Some(row) = observed.row.clone() else {
        return Ok(Removed {
            removed: false,
            secret_deleted: true,
            authorization_changed: false,
            pending_cleanup: false,
        });
    };
    let guard = match qoder_secret::observe_for_delete(app, &account, &row, CredentialKind::Pat) {
        Ok(guard) => guard,
        Err(err) => return Err(secret_action_error(&err, "读取 PAT 凭据")),
    };

    before_row_delete();

    // ② 带核验删行：比它新的授权不能被这次移除带走。
    let removed = app.with_db(|db| {
        db.delete_qoder_authorization_if_current(
            account_id,
            SOURCE_OWN_PAT,
            row.secret_ref.as_deref(),
            row.auth_version,
            &observed.lifecycle.generation,
        )
    })?;
    if !removed {
        return Ok(Removed {
            removed: false,
            secret_deleted: false,
            authorization_changed: true,
            pending_cleanup: false,
        });
    }

    after_row_delete();

    // ②b **删凭据前再确认一次这一行没有被重建**（审计 P0-3）。
    //
    // 上面删掉的是"我观察到的那一行"。如果这之后有人重新插入同一来源的授权
    // （Keychain 键名固定、PAT 还可能是同一串字节），它的 guard 会与旧快照
    // 逐字节相等 —— 直接删就会删掉新授权正在用的凭据。锁域已经让这种交错
    // 不可能来自另一个进程，但同进程重入与未来的锁粒度调整仍需要这道防线。
    if qoder_secret::row_of(app, account_id, SOURCE_OWN_PAT).is_some() {
        return Ok(Removed {
            removed: false,
            secret_deleted: false,
            authorization_changed: true,
            pending_cleanup: false,
        });
    }

    // ③ 删凭据。本来就不在（`None`）算删干净。
    let Some(guard) = guard else {
        return Ok(Removed {
            removed: true,
            secret_deleted: true,
            authorization_changed: false,
            pending_cleanup: false,
        });
    };
    match qoder_secret::delete_guarded(app, &account, &row, CredentialKind::Pat, &guard) {
        Ok(_) => Ok(Removed {
            removed: true,
            secret_deleted: true,
            authorization_changed: false,
            pending_cleanup: false,
        }),
        // 文件在 ② 之后被重写：**不删**这份可能已是新令牌的内容，
        // 把行以 `missing` 放回，交给用户重试（不能假报已移除）。
        Err(err) if err.kind == CredentialErrorKind::Conflict => {
            if !restore_missing(app, &row, &observed.lifecycle) {
                crate::handlers::log_note(
                    "PAT 凭据在移除期间被重写，且授权行未能放回：请重新保存 PAT 或删除该账号",
                );
            }
            Ok(Removed {
                removed: false,
                secret_deleted: false,
                authorization_changed: false,
                pending_cleanup: true,
            })
        }
        // ④ 删不掉：把行放回去（`missing`），让用户能再点一次。
        Err(err) => {
            let restored = restore_missing(app, &row, &observed.lifecycle);
            if !restored {
                crate::handlers::log_note(
                    "移除 PAT 失败且授权行未能放回：请重新保存一次 PAT 或删除该账号",
                );
            }
            Err(secret_action_error(&err, "删除 PAT 凭据"))
        }
    }
}

/// 把行以 `missing` 放回，**限定在观察到的那个账号生命周期**（复审 P1-D）。
fn restore_missing(app: &App, row: &QoderAuthRecord, observed: &QoderLifecycle) -> bool {
    app.with_db(|db| db.restore_qoder_authorization(row, "missing", observed))
        .unwrap_or(false)
}

/// 凭据层错误 → RPC 错误，**带 `可重试` 字样**（界面靠它决定是否让用户再点一次）。
fn secret_action_error(err: &crate::credentials::CredentialError, what: &str) -> RpcError {
    RpcError::new(
        crate::rpc::E_SECURESTORE,
        format!("{what}失败：{}。可重试", crate::rpc::sanitize(&err.message)),
    )
    .with_data(json!({
        "kind": "secret_delete_failed",
        "reason": err.kind.as_str(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;
    use crate::storage::qoder_accounts::{SOURCE_MAGPIE, SOURCE_OWN_BROWSER};

    fn test_app() -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let paths = Paths {
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data,
            wb_home_owned: true,
        };
        (App::with_secure(paths, SecureStore::memory()), dir)
    }

    fn identity(uid: &str) -> Identity {
        Identity {
            uid: uid.to_string(),
            name: Some("PAT 账号".to_string()),
            email: Some("pat@e.com".to_string()),
        }
    }

    #[test]
    fn normalize_rejects_the_obvious_mistakes_and_keeps_the_token_intact() {
        assert!(normalize("   ").is_err());
        assert!(normalize("short").is_err());
        assert!(normalize("has space inside the token value").is_err());
        assert!(normalize("line\nbreak").is_err());
        // 前后空白去掉，内容一字不改。
        let pat = "  qp_abcdefghijklmnopqrstuvwxyz012345  ";
        assert_eq!(
            normalize(pat).unwrap(),
            "qp_abcdefghijklmnopqrstuvwxyz012345"
        );
    }

    #[test]
    fn keys_are_scoped_per_account_and_differ_from_browser_keys() {
        assert_eq!(pat_key("qa_1"), "qoder/qa_1/pat/token");
        assert_eq!(
            qoder_secret::keychain_reference("qa_1", SOURCE_OWN_PAT),
            "keychain:qoder/qa_1/pat/token"
        );
        assert_ne!(
            pat_key("qa_1"),
            crate::providers::qoder_auth::access_key("qa_1")
        );
    }

    #[test]
    fn persist_puts_the_pat_in_the_keychain_only() {
        let (app, _dir) = test_app();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let saved = persist(&app, QoderEdition::Cn, pat, &identity("u-pat")).unwrap();
        assert_eq!(saved.edition, "qoder_cn");
        assert_eq!(
            app.secure.get(&pat_key(&saved.account_id)).unwrap(),
            pat.as_bytes()
        );

        let rows = app
            .with_db(|db| db.list_qoder_authorizations(&saved.account_id))
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, SOURCE_OWN_PAT);
        assert_eq!(rows[0].auth_status, "valid");
        // PAT 不声明到期时间：不能凭空造一个。
        assert!(rows[0].token_expires_at.is_none());
        let shown = rows[0].to_json();
        assert_eq!(shown["hasSecret"], json!(true));
        assert!(shown.get("token").is_none());

        // 响应里"身份已验证"与"能否领 Credits"必须分开。
        let payload = saved.to_json();
        assert_eq!(payload["identity"], "verified");
        assert_eq!(payload["credits"], "unknown");
    }

    #[test]
    fn pat_and_browser_and_magpie_are_three_independent_rows() {
        let (app, _dir) = test_app();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let saved = persist(&app, QoderEdition::Global, pat, &identity("u-1")).unwrap();
        let id = saved.account_id.clone();

        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &id,
                SOURCE_OWN_BROWSER,
                None,
                Some("keychain:qoder/own/access"),
                "valid",
                None,
                None,
            )
        })
        .unwrap();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &id,
                SOURCE_MAGPIE,
                Some("qoder#1"),
                None,
                "valid",
                None,
                None,
            )
        })
        .unwrap();

        let rows = app.with_db(|db| db.list_qoder_authorizations(&id)).unwrap();
        assert_eq!(rows.len(), 3);

        // 删掉 PAT 不动另外两份。
        let removed = remove(&app, &id).unwrap();
        assert!(removed.removed);
        assert!(removed.secret_deleted);
        assert!(app.secure.get(&pat_key(&id)).is_err());
        let rows = app.with_db(|db| db.list_qoder_authorizations(&id)).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.source != SOURCE_OWN_PAT));
    }

    #[test]
    fn removing_a_pat_that_is_not_there_is_not_an_error() {
        let (app, _dir) = test_app();
        let account = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-x", None, None))
            .unwrap();
        let removed = remove(&app, &account.id).unwrap();
        assert!(!removed.removed);
        assert!(removed.secret_deleted, "本来就没有凭据 → 视为删干净");
    }

    #[test]
    fn reauthorizing_with_a_pat_advances_the_auth_epoch() {
        let (app, _dir) = test_app();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let saved = persist(&app, QoderEdition::Cn, pat, &identity("u-epoch")).unwrap();
        let before = app
            .with_db(|db| db.qoder_lifecycle(&saved.account_id))
            .unwrap()
            .unwrap();
        persist(&app, QoderEdition::Cn, pat, &identity("u-epoch")).unwrap();
        let after = app
            .with_db(|db| db.qoder_lifecycle(&saved.account_id))
            .unwrap()
            .unwrap();
        assert!(
            after.auth_epoch > before.auth_epoch,
            "重新授权必须推进版本号：{} → {}",
            before.auth_epoch,
            after.auth_epoch
        );
        assert_eq!(after.generation, before.generation, "换授权不换生命周期");
    }

    fn app_with(secure: SecureStore) -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let paths = Paths {
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data,
            wb_home_owned: true,
        };
        (App::with_secure(paths, secure), dir)
    }

    fn faulty_app(fail_set: &[&str], fail_delete: &[&str]) -> (App, tempfile::TempDir) {
        app_with(SecureStore::faulty(fail_set, fail_delete))
    }

    /// 钥匙串写失败：**不能留下一个没有授权的空账号**。
    /// 那种账号在界面上就是一个删不掉、也认不出的空卡片。
    #[test]
    fn a_failed_keychain_write_leaves_no_orphan_account() {
        let (app, _dir) = faulty_app(&["qoder/"], &[]);
        app.secure.arm();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let err = persist(&app, QoderEdition::Cn, pat, &identity("u-orphan"))
            .err()
            .expect("钥匙串写失败必须报错");
        assert!(err.message.contains("凭据"), "{}", err.message);
        assert!(
            app.with_db(|db| db.list_qoder_accounts(None))
                .unwrap()
                .is_empty(),
            "账号必须被回滚掉"
        );
    }

    /// 回滚链上的**第一步**也可能失败：读不到旧凭据时 `Snapshot::capture` 直接返回，
    /// 同样不能留下刚建的空账号。
    #[test]
    fn a_failed_snapshot_capture_also_rolls_the_new_account_back() {
        let (app, _dir) = app_with(SecureStore::faulty_on_read_keys(&["qoder/"]));
        app.secure.arm();
        let err = persist(
            &app,
            QoderEdition::Cn,
            "qp_abcdefghijklmnopqrstuvwxyz012345",
            &identity("u-capture"),
        )
        .err()
        .expect("读不到旧凭据必须中止保存");
        assert!(
            err.message.contains("凭据") || err.message.contains("读取"),
            "{}",
            err.message
        );
        assert!(
            app.with_db(|db| db.list_qoder_accounts(None))
                .unwrap()
                .is_empty(),
            "capture 失败也必须把新建账号回滚掉"
        );
    }

    /// 账号本来就有（例如先 Magpie 导入过）：失败时**不许**把它删掉。
    #[test]
    fn a_failed_keychain_write_keeps_a_preexisting_account() {
        let (app, _dir) = faulty_app(&["qoder/"], &[]);
        let existing = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-old", Some("老账号"), None))
            .unwrap();
        // 先把账号种进去，再武装故障注入：顺序反了就连账号都建不出来。
        app.secure.arm();
        let err = persist(
            &app,
            QoderEdition::Cn,
            "qp_abcdefghijklmnopqrstuvwxyz012345",
            &identity("u-old"),
        )
        .err()
        .expect("钥匙串写失败必须报错");
        assert!(err.message.contains("凭据"));
        let rows = app.with_db(|db| db.list_qoder_accounts(None)).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, existing.id);
    }

    // -- 复审 P0-4：并发重新保存 PAT -----------------------------------

    /// 旧快照的"移除"撞上"另一个窗口刚保存了新 PAT"：**什么都没删**，
    /// 新令牌仍然可用、行还在，并明确告诉界面"授权变了，刷新重试"。
    #[test]
    fn a_remove_racing_a_new_pat_keeps_the_new_one() {
        let (app, _dir) = test_app();
        let old_pat = "qp_oldoldoldoldoldoldoldoldoldold01";
        let new_pat = "qp_newnewnewnewnewnewnewnewnewnew02";
        let saved = persist(&app, QoderEdition::Cn, old_pat, &identity("u-race")).unwrap();

        let outcome = remove_with(
            &app,
            &saved.account_id,
            || {
                // 另一个窗口保存了新的 PAT（同一个来源 → 覆盖写 + epoch 前进）。
                persist(&app, QoderEdition::Cn, new_pat, &identity("u-race"))
                    .expect("新 PAT 必须保存成功");
            },
            || {},
        )
        .unwrap();

        assert!(outcome.authorization_changed, "旧快照的移除必须放弃");
        assert!(!outcome.removed, "行不许被删");
        assert!(!outcome.secret_deleted, "凭据不许被删");

        let rows = app
            .with_db(|db| db.list_qoder_authorizations(&saved.account_id))
            .unwrap();
        assert_eq!(rows.len(), 1, "新授权必须还在");
        assert_eq!(rows[0].auth_status, "valid");
        let token = read_secret(
            &app,
            &app.with_db(|db| db.get_qoder_account(&saved.account_id))
                .unwrap()
                .unwrap(),
            &rows[0],
        )
        .unwrap();
        assert_eq!(token, new_pat, "最新 PAT 必须原样可读");
    }

    /// 同一个交错，凭据在本地 JSON 文件里：一样要保住新令牌。
    #[test]
    fn a_remove_racing_a_new_json_pat_keeps_the_new_file_contents() {
        let (app, _dir) = test_app();
        crate::auth::consent::set_json_storage(&app, true).unwrap();
        let old_pat = "qp_oldoldoldoldoldoldoldoldoldold01";
        let new_pat = "qp_newnewnewnewnewnewnewnewnewnew02";
        let saved = persist(&app, QoderEdition::Cn, old_pat, &identity("u-json")).unwrap();

        // 交错点在"删行之前"：另一个窗口把新 PAT 覆盖写进同一个文件并提交，
        // 于是我们的条件删行看到 epoch 变了，整次放弃（P0-4 的核心）。
        let outcome = remove_with(
            &app,
            &saved.account_id,
            || {
                persist(&app, QoderEdition::Cn, new_pat, &identity("u-json")).unwrap();
            },
            || {},
        )
        .unwrap();

        assert!(outcome.authorization_changed);
        assert!(!outcome.removed && !outcome.secret_deleted);
        let row = app
            .with_db(|db| db.get_qoder_authorization(&saved.account_id, SOURCE_OWN_PAT))
            .unwrap()
            .expect("行必须还在");
        let secret_id = row
            .secret_ref
            .as_deref()
            .unwrap()
            .trim_start_matches("localjson:v1:");
        let secret = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
            .load(secret_id)
            .expect("文件必须还在");
        assert_eq!(
            secret
                .token(crate::credentials::reference::CredentialSlot::Access)
                .unwrap(),
            new_pat
        );
        assert_eq!(secret.revision, 1, "只被新保存写过一次");
    }

    /// **复审 P0-A 的确定性交错**：行已摘掉之后才发现文件被重写（`revision` 变了）。
    ///
    /// 这时不能报"已移除"——那会把一份有效、却没有任何行引用的令牌留在盘上，
    /// 而且再点一次也找不到它。正确结果是：行以 `missing` 放回（可定位、可重试、
    /// Credits 不会用），返回 `removed: false` + `pending_cleanup: true`。
    #[test]
    fn a_rewritten_secret_leaves_a_retryable_record_and_never_reports_success() {
        let (app, _dir) = test_app();
        crate::auth::consent::set_json_storage(&app, true).unwrap();
        let old_pat = "qp_oldoldoldoldoldoldoldoldoldold01";
        let new_pat = "qp_newnewnewnewnewnewnewnewnewnew02";
        let saved = persist(&app, QoderEdition::Cn, old_pat, &identity("u-strand")).unwrap();

        let row = app
            .with_db(|db| db.get_qoder_authorization(&saved.account_id, SOURCE_OWN_PAT))
            .unwrap()
            .expect("PAT 行");
        let secret_id = row
            .secret_ref
            .as_deref()
            .unwrap()
            .trim_start_matches("localjson:v1:")
            .to_string();

        let outcome = remove_with(
            &app,
            &saved.account_id,
            || {},
            || {
                // 行已摘掉、凭据还没删：另一个进程把新令牌写进了同一个文件。
                crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
                    .replace_credentials(
                        &secret_id,
                        0,
                        crate::credentials::localjson::Credentials::access_only(new_pat),
                        None,
                    )
                    .expect("模拟并发写入");
            },
        )
        .unwrap();

        assert!(!outcome.removed, "不能假报已移除");
        assert!(!outcome.secret_deleted, "文件还在");
        assert!(outcome.pending_cleanup, "必须留下可重试的清理入口");
        assert!(!outcome.authorization_changed);

        // 行回来了，标成 missing，并且指着那份文件。
        let restored = app
            .with_db(|db| db.get_qoder_authorization(&saved.account_id, SOURCE_OWN_PAT))
            .unwrap()
            .expect("行必须以 missing 恢复");
        assert_eq!(restored.auth_status, "missing");
        assert_eq!(
            restored.secret_ref.as_deref(),
            Some(row.secret_ref.as_deref().unwrap())
        );

        // **没有任何无主有效令牌**：那份文件现在有行引用，而且行是停用状态。
        let account = app
            .with_db(|db| db.get_qoder_account(&saved.account_id))
            .unwrap()
            .unwrap();
        let err = crate::credits::resolve_token(&app, &account)
            .err()
            .expect("missing 行不许被 Credits 使用");
        assert!(err.contains("missing"), "{err}");

        // 再点一次 remove 就能收尾（文件按新 revision 删掉、行删掉）。
        let retried = remove(&app, &saved.account_id).unwrap();
        assert!(
            retried.removed && retried.secret_deleted,
            "removed={} deleted={}",
            retried.removed,
            retried.secret_deleted
        );
        assert!(app
            .with_db(|db| db.list_qoder_authorizations(&saved.account_id))
            .unwrap()
            .is_empty());
        assert!(
            crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
                .load(&secret_id)
                .is_err()
        );
    }

    /// **审计 P0-3 的确定性交错**：行删掉之后、凭据还没删，另一个流程重新保存了
    /// **完全相同**的一串 PAT。
    ///
    /// 这时旧 guard 的逐字节比对会通过（键名固定、值也一样），直接删就会删掉新
    /// 授权正在用的凭据，而新行还显示 `valid`。修法：删凭据前再读一次行，发现
    /// 被重建就停手。
    #[test]
    fn a_rebuilt_pat_row_is_not_deleted_by_the_older_removal() {
        let (app, _dir) = test_app();
        let pat = "qp_samesamesamesamesamesamesame01";
        let saved = persist(&app, QoderEdition::Cn, pat, &identity("u-rebuild")).unwrap();

        let outcome = remove_with(
            &app,
            &saved.account_id,
            || {},
            || {
                // 行已删、凭据未删：同一个窗口里重新保存同一串 PAT。
                persist(&app, QoderEdition::Cn, pat, &identity("u-rebuild"))
                    .expect("重新保存必须成功");
            },
        )
        .unwrap();

        assert!(outcome.authorization_changed, "发现新行就必须停手");
        assert!(!outcome.removed && !outcome.secret_deleted);

        // 新行还在、仍有效，PAT 原样可读 —— 没有被旧移除删掉。
        let rows = app
            .with_db(|db| db.list_qoder_authorizations(&saved.account_id))
            .unwrap();
        assert_eq!(rows.len(), 1, "新行必须还在");
        assert_eq!(rows[0].auth_status, "valid");
        let account = app
            .with_db(|db| db.get_qoder_account(&saved.account_id))
            .unwrap()
            .unwrap();
        assert_eq!(read_secret(&app, &account, &rows[0]).unwrap(), pat);
        assert_eq!(
            crate::credits::resolve_token(&app, &account).unwrap().token,
            pat,
            "Credits 必须还能用这份 PAT"
        );
    }

    /// 幂等重试：凭据已经不在（上次删到一半），再点一次照样把行删掉。
    #[test]
    fn removing_after_a_half_finished_delete_still_converges() {
        let (app, _dir) = test_app();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let saved = persist(&app, QoderEdition::Cn, pat, &identity("u-half")).unwrap();
        // 模拟"凭据已被删掉，行还在"。
        app.secure.delete(&pat_key(&saved.account_id)).unwrap();

        let outcome = remove(&app, &saved.account_id).unwrap();
        assert!(outcome.removed);
        assert!(outcome.secret_deleted, "本来就没有 → 视为删干净");
        assert!(app
            .with_db(|db| db.list_qoder_authorizations(&saved.account_id))
            .unwrap()
            .is_empty());
    }

    /// 钥匙串删不掉：**保留授权行并报错**，不能静默留下孤儿凭据。
    #[test]
    fn a_failed_secret_delete_keeps_the_authorization_for_retry() {
        let (app, _dir) = faulty_app(&[], &["qoder/"]);
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let saved = persist(&app, QoderEdition::Cn, pat, &identity("u-del")).unwrap();
        // 写入成功之后再武装：要测的是"删失败"，不是"写失败"。
        app.secure.arm();

        let err = remove(&app, &saved.account_id)
            .err()
            .expect("删除失败必须有错误");
        assert!(err.message.contains("可重试"), "{}", err.message);

        // 授权行还在，凭据也还在：这是一个可以重试的中间态。
        app.secure.disarm();
        let rows = app
            .with_db(|db| db.list_qoder_authorizations(&saved.account_id))
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].source, SOURCE_OWN_PAT);
        assert!(app.secure.get(&pat_key(&saved.account_id)).is_ok());
    }

    /// 网络返回期间用户换了 PAT：旧结果不许把新授权标脏。
    #[test]
    fn a_status_write_from_a_replaced_authorization_is_rejected() {
        let (app, _dir) = test_app();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let saved = persist(&app, QoderEdition::Cn, pat, &identity("u-race")).unwrap();
        let old_epoch = app
            .with_db(|db| db.qoder_lifecycle(&saved.account_id))
            .unwrap()
            .unwrap();

        // 用户重新保存了一次 PAT → auth_epoch 前进。
        persist(&app, QoderEdition::Cn, pat, &identity("u-race")).unwrap();

        let written = app
            .with_db(|db| {
                db.set_qoder_auth_status_if_current(
                    &saved.account_id,
                    SOURCE_OWN_PAT,
                    "invalid",
                    &old_epoch,
                )
            })
            .unwrap();
        assert!(!written, "旧版本的状态写入必须被拒");

        let rows = app
            .with_db(|db| db.list_qoder_authorizations(&saved.account_id))
            .unwrap();
        assert_eq!(rows[0].auth_status, "valid", "新授权不能被旧结果标脏");
    }

    #[test]
    fn a_status_write_with_the_current_epoch_goes_through() {
        let (app, _dir) = test_app();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let saved = persist(&app, QoderEdition::Cn, pat, &identity("u-ok")).unwrap();
        let epoch = app
            .with_db(|db| db.qoder_lifecycle(&saved.account_id))
            .unwrap()
            .unwrap();
        let written = app
            .with_db(|db| {
                db.set_qoder_auth_status_if_current(
                    &saved.account_id,
                    SOURCE_OWN_PAT,
                    "invalid",
                    &epoch,
                )
            })
            .unwrap();
        assert!(written);
        let rows = app
            .with_db(|db| db.list_qoder_authorizations(&saved.account_id))
            .unwrap();
        assert_eq!(rows[0].auth_status, "invalid");
    }

    #[test]
    fn status_updates_do_not_advance_the_epoch() {
        let (app, _dir) = test_app();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let saved = persist(&app, QoderEdition::Cn, pat, &identity("u-status")).unwrap();
        let before = app
            .with_db(|db| db.qoder_lifecycle(&saved.account_id))
            .unwrap()
            .unwrap();
        app.with_db(|db| db.set_qoder_auth_status(&saved.account_id, SOURCE_OWN_PAT, "invalid"))
            .unwrap();
        let after = app
            .with_db(|db| db.qoder_lifecycle(&saved.account_id))
            .unwrap()
            .unwrap();
        assert_eq!(before, after, "状态是观测结论，不是凭据变更");
    }
}
