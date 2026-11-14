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

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::app::App;
use crate::auth::service::{credential_error, Snapshot};
use crate::providers::qoder_auth::{parse_identity, userinfo_url, Identity};
use crate::providers::qoder_edition::QoderEdition;
use crate::rpc::{RpcError, RpcResult};
use crate::securestore::fingerprint;
use crate::storage::qoder_accounts::SOURCE_OWN_PAT;

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

pub fn keychain_ref(account_id: &str) -> String {
    format!("keychain:{}", pat_key(account_id))
}

/// 用一次 `userinfo` 换身份。**这是唯一能证明"这张 PAT 属于这个账号"的动作。**
pub async fn verify(edition: QoderEdition, pat: &str) -> Result<Identity, PatError> {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| PatError::new(KIND_NETWORK, format!("HTTP 客户端创建失败：{e}")))?;
    let mut headers = HashMap::new();
    headers.insert("Accept".to_string(), "application/json".to_string());
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
            "identity": "verified",
            "credits": "unknown",
            "warning": "PAT 已保存并通过身份验证；它是否能领取 Credits 需要单独检测，两者不是同一件事",
        })
    }
}

/// 落库：PAT 进钥匙串，账号与授权进 SQLite。
///
/// 顺序与浏览器授权一致：先写钥匙串（唯一需要显式回滚的一步），再写库。
pub fn persist(
    app: &App,
    edition: QoderEdition,
    pat: &str,
    identity: &Identity,
) -> RpcResult<Persisted> {
    let account = app.with_db(|db| {
        db.upsert_qoder_account(
            edition.as_str(),
            &identity.uid,
            identity.name.as_deref(),
            identity.email.as_deref(),
        )
    })?;

    let key = pat_key(&account.id);
    let snapshot = Snapshot::capture(app, std::slice::from_ref(&key))?;
    if let Err(err) = app.secure.set(&key, pat.as_bytes()) {
        return Err(credential_error(err, "qoder/pat"));
    }

    let digest = fingerprint(pat.as_bytes());
    let written = app.with_db(|db| {
        db.upsert_qoder_authorization(
            &account.id,
            SOURCE_OWN_PAT,
            None,
            Some(&keychain_ref(&account.id)),
            "valid",
            // PAT 不随请求下发到期时间：有就是有，失效靠 401 发现。
            None,
            Some(&digest),
        )
    });
    if let Err(err) = written {
        // 库没写成功：钥匙串里不该留一份没人认领的令牌。
        snapshot.restore(app);
        return Err(err);
    }

    Ok(Persisted {
        account_id: account.id,
        edition: edition.as_str(),
        uid: identity.uid.clone(),
        nickname: account.nickname,
        email: account.email,
        fingerprint: digest,
    })
}

/// 读回已存的 PAT。**只在 worker 内部使用**，不经过 RPC。
pub fn read_secret(app: &App, account_id: &str) -> Result<String, PatError> {
    let key = pat_key(account_id);
    let bytes = app
        .secure
        .get(&key)
        .map_err(|err| PatError::new(KIND_NO_SECRET, format!("读不到 PAT：{}", err.message)))?;
    String::from_utf8(bytes)
        .map_err(|_| PatError::new(KIND_NO_SECRET, "钥匙串里的 PAT 不是合法 UTF-8"))
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
    let pat = read_secret(app, account_id)?;
    match verify(edition, &pat).await {
        Ok(identity) => {
            app.with_db(|db| db.set_qoder_auth_status(account_id, SOURCE_OWN_PAT, "valid"))
                .ok();
            Ok(identity)
        }
        Err(err) => {
            let status = match err.kind {
                KIND_REJECTED | KIND_FORBIDDEN => Some("invalid"),
                _ => None,
            };
            if let Some(status) = status {
                app.with_db(|db| db.set_qoder_auth_status(account_id, SOURCE_OWN_PAT, status))
                    .ok();
            }
            Err(err)
        }
    }
}

pub struct Removed {
    pub removed: bool,
    pub secret_deleted: bool,
}

/// 删掉 PAT 这一份来源：先删库（用户在界面上看到的立刻消失），再删钥匙串。
///
/// 顺序与写入相反，理由也相反：写入失败要回滚成"什么都没有"，
/// 删除失败要保证"库里没有、钥匙串里也别留"。
pub fn remove(app: &App, account_id: &str) -> RpcResult<Removed> {
    if app
        .with_db(|db| db.get_qoder_account(account_id))?
        .is_none()
    {
        return Err(RpcError::not_found(format!(
            "Qoder 账号不存在：{account_id}"
        )));
    }
    let removed = app.with_db(|db| db.delete_qoder_authorization(account_id, SOURCE_OWN_PAT))?;
    let key = pat_key(account_id);
    let secret_deleted = match app.secure.delete(&key) {
        Ok(_) => true,
        Err(err) => {
            // 没有这份凭据本来就算删干净了。
            err.kind == crate::securestore::SecretErrorKind::NotFound
        }
    };
    Ok(Removed {
        removed,
        secret_deleted,
    })
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
        assert_eq!(keychain_ref("qa_1"), "keychain:qoder/qa_1/pat/token");
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
            .with_db(|db| db.qoder_auth_epoch(&saved.account_id))
            .unwrap()
            .unwrap();
        persist(&app, QoderEdition::Cn, pat, &identity("u-epoch")).unwrap();
        let after = app
            .with_db(|db| db.qoder_auth_epoch(&saved.account_id))
            .unwrap()
            .unwrap();
        assert!(after > before, "重新授权必须推进版本号：{before} → {after}");
    }

    #[test]
    fn status_updates_do_not_advance_the_epoch() {
        let (app, _dir) = test_app();
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let saved = persist(&app, QoderEdition::Cn, pat, &identity("u-status")).unwrap();
        let before = app
            .with_db(|db| db.qoder_auth_epoch(&saved.account_id))
            .unwrap()
            .unwrap();
        app.with_db(|db| db.set_qoder_auth_status(&saved.account_id, SOURCE_OWN_PAT, "invalid"))
            .unwrap();
        let after = app
            .with_db(|db| db.qoder_auth_epoch(&saved.account_id))
            .unwrap()
            .unwrap();
        assert_eq!(before, after, "状态是观测结论，不是凭据变更");
    }
}
