//! Qoder 设备授权（PR #73）：PKCE + device token 轮询 + 凭据落钥匙串。
//!
//! 协议照抄客户端自己的 CLI（`@qoder-ai/qodercli` / `@qodercn-ai/qoderclicn`），
//! 细节见 `docs/QODER_DEVICE_AUTH.md`：
//!
//! 1. **发起**：本地生成 `verifier` / `challenge`(S256) / `nonce` / `machine_id`，
//!    把用户送到 `{web}/device/selectAccounts?...`。这一步**不联网**，
//!    所以助手离线也能把登录页开出来。
//! 2. **轮询**：`GET {openapi}/api/v1/deviceToken/poll?nonce&verifier&challenge_method=S256`。
//!    只有 **HTTP 200 且 `token` 非空**才算完成；其它情况（含 4xx、网络抖动）
//!    一律继续等，由 TTL 兜底——这与 CLI 的行为一致。
//! 3. **身份**：`GET {openapi}/api/v1/userinfo`（Bearer device token）→ `name` / `email`；
//!    uid 取轮询响应里的 `user_id`，缺失才回落到 userinfo。
//!
//! 本阶段**只取 device token**：
//! - 它是 #74 领取 Credits 要用的凭据；
//! - `/api/v1/me/jobToken` 换出来的是 chat token，AICC 不聊天，不做交换；
//! - 刷新单独评审，不在本 PR 的隐含交付项里（`refresh` 只存不用）。
//!
//! 两个版本**互不共用**：域名、client id、钥匙串键全部按 edition 分开，
//! 同一 uid 在两个站是两行账号（`qoder_accounts` 的唯一键是 `(edition, uid)`）。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use base64::Engine;
use chrono::DateTime;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::auth::service::{credential_error, Snapshot};
use crate::providers::qoder_edition::QoderEdition;
use crate::rpc::RpcResult;
use crate::securestore::fingerprint;
use crate::storage::qoder_accounts::SOURCE_OWN_BROWSER;

/// 与浏览器登录同一套错误分类，界面上的状态文案可以复用。
pub const KIND_NETWORK: &str = "network";
pub const KIND_EXPIRED: &str = "expired";
pub const KIND_CANCELLED: &str = "cancelled";
pub const KIND_UNKNOWN_LOGIN: &str = "unknown_login";
pub const KIND_FINISHED: &str = "finished";
pub const KIND_PROFILE_MISSING: &str = "profile_missing";

/// 登录会话存活时间，与 CLI 的 15 分钟一致。
pub const SESSION_TTL_SECONDS: i64 = 15 * 60;

pub struct AuthError {
    pub kind: &'static str,
    pub message: String,
}

impl AuthError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// 授权拿到的账号身份。令牌单独放在 `DeviceToken` 里，不混进资料。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Identity {
    pub uid: String,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// 轮询成功后拿到的一切。**不要**给它加 Serialize/Debug。
pub struct DeviceToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at_ms: Option<i64>,
    pub identity: Identity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    Pending,
    /// 已通过提交门禁（`begin_commit`），正在落库。此时取消不再生效。
    Committing,
    Success,
    Cancelled,
    Expired,
    Failed,
}

struct Session {
    nonce: String,
    verifier: String,
    edition: QoderEdition,
    created: Instant,
    phase: Phase,
    error: Option<Value>,
}

static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------------------------------------------------------------------------
// PKCE 与登录地址（纯函数，便于单测）
// ---------------------------------------------------------------------------

/// 43～128 字符的 base64url 随机串。64 字节随机 → 86 字符，与 CLI 一致。
pub fn new_verifier() -> String {
    b64url(&random_bytes(64))
}

/// `challenge = base64url(sha256(verifier))`，`challenge_method=S256`。
pub fn challenge_for(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    b64url(&hasher.finalize())
}

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// 取 `len` 字节随机数。没有单独的 `rand` 依赖：uuid v4 本身就取系统熵，
/// 拼几段就够 PKCE 用的（64 字节 = 4 段）。
fn random_bytes(len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len + 16);
    while out.len() < len {
        out.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    out.truncate(len);
    out
}

pub fn new_nonce() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// 浏览器登录页。参数全部是 base64url / uuid，天然 URL 安全，不做额外编码。
pub fn authorize_url(
    edition: QoderEdition,
    challenge: &str,
    nonce: &str,
    machine_id: &str,
) -> String {
    let redirect = edition.redirect_uri();
    let mut url = format!(
        "{}/device/selectAccounts?challenge={challenge}&challenge_method=S256&nonce={nonce}\
         &machine_id={machine_id}&client_id={}",
        edition.web_base(),
        edition.client_id()
    );
    if !redirect.is_empty() {
        url.push_str("&redirect_uri=");
        url.push_str(redirect);
    }
    url
}

pub fn poll_url(edition: QoderEdition, nonce: &str, verifier: &str) -> String {
    format!(
        "{}/api/v1/deviceToken/poll?nonce={nonce}&verifier={verifier}&challenge_method=S256",
        edition.openapi_base()
    )
}

pub fn userinfo_url(edition: QoderEdition) -> String {
    format!("{}/api/v1/userinfo", edition.openapi_base())
}

// ---------------------------------------------------------------------------
// 响应解析
// ---------------------------------------------------------------------------

/// 轮询响应 → 令牌 JSON。只有 200 且带非空 `token` 才算完成，其余都是"还没授权"。
pub fn device_token_payload(status: u16, body: &Value) -> Option<Value> {
    if status != 200 {
        return None;
    }
    // 官方响应是顶层的 `{token, refresh_token, user_id}`；`data` 包裹只是兼容。
    let source = body
        .get("token")
        .map(|_| body)
        .or_else(|| body.get("data").filter(|d| d.get("token").is_some()))?;
    let token = source.get("token").and_then(Value::as_str)?.trim();
    if token.is_empty() {
        return None;
    }
    Some(source.clone())
}

/// 身份：uid 优先取轮询响应，缺失才回落到 userinfo；都没有就不许入库。
pub fn parse_identity(token: &Value, user: Option<&Value>) -> Option<Identity> {
    let pick = |value: Option<&Value>, keys: &[&str]| -> Option<String> {
        let value = value?;
        for key in keys {
            if let Some(text) = value.get(*key).and_then(Value::as_str) {
                let text = text.trim();
                if !text.is_empty() {
                    return Some(text.to_string());
                }
            }
            // `id` 可能是数字。
            if let Some(number) = value.get(*key).and_then(Value::as_i64) {
                return Some(number.to_string());
            }
        }
        None
    };
    let uid = pick(Some(token), &["user_id", "userId", "uid"])
        .or_else(|| pick(user, &["id", "user_id", "userId", "uid"]))?;
    let name = pick(user, &["name", "nickname", "display_name"]);
    let email = pick(user, &["email", "mail"]);
    Some(Identity { uid, name, email })
}

/// `expires_at`（日期，或秒/毫秒时间戳）优先，其次 `expires_in`（秒）。
pub fn expires_at_ms(token: &Value) -> Option<i64> {
    for key in ["expires_at", "expire_time"] {
        if let Some(value) = token.get(key) {
            if let Some(ms) = when_said(value) {
                return Some(ms);
            }
        }
    }
    token
        .get("expires_in")
        .and_then(Value::as_i64)
        .filter(|secs| *secs > 0)
        .map(|secs| now_ms() + secs * 1000)
}

fn when_said(value: &Value) -> Option<i64> {
    if let Some(number) = value.as_i64() {
        if number <= 0 {
            return None;
        }
        return Some(if number < 1_000_000_000_000 {
            number * 1000
        } else {
            number
        });
    }
    let text = value.as_str()?.trim();
    if text.is_empty() {
        return None;
    }
    if text.chars().all(|c| c.is_ascii_digit()) {
        return when_said(&json!(text.parse::<i64>().ok()?));
    }
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn ms_to_rfc3339(ms: i64) -> Option<String> {
    DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

// ---------------------------------------------------------------------------
// 钥匙串键名
// ---------------------------------------------------------------------------

/// `qoder/<account_id>/own/access|refresh`。
///
/// 和 WorkBuddy 的 `workbuddy/...` 前缀分开：两套凭据不能互相读到，
/// 也不会因为 uid 相同而覆盖。
pub fn secret_key(account_id: &str, which: &str) -> String {
    format!("qoder/{account_id}/own/{which}")
}

pub fn access_key(account_id: &str) -> String {
    secret_key(account_id, "access")
}

pub fn refresh_key(account_id: &str) -> String {
    secret_key(account_id, "refresh")
}

pub fn keychain_ref(key: &str) -> String {
    format!("keychain:{key}")
}

// ---------------------------------------------------------------------------
// 会话状态机
// ---------------------------------------------------------------------------

pub struct StartInfo {
    pub login_id: String,
    pub verification_uri: String,
    pub expires_in: i64,
}

/// 发起授权。**不联网**：只生成 PKCE 并登记会话。
pub fn start(edition: QoderEdition) -> StartInfo {
    let verifier = new_verifier();
    let challenge = challenge_for(&verifier);
    let nonce = new_nonce();
    let machine_id = new_nonce();
    let login_id = format!("qd1_{}", uuid::Uuid::new_v4().simple());
    let verification_uri = authorize_url(edition, &challenge, &nonce, &machine_id);

    let mut map = sessions().lock().expect("sessions");
    map.insert(
        login_id.clone(),
        Session {
            nonce,
            verifier,
            edition,
            created: Instant::now(),
            phase: Phase::Pending,
            error: None,
        },
    );
    StartInfo {
        login_id,
        verification_uri,
        expires_in: SESSION_TTL_SECONDS,
    }
}

pub enum Polled {
    Pending,
    Failed(AuthError),
    Token(DeviceToken),
}

/// 轮询一次。只有 `Pending` 才会真的打网络。
pub async fn poll(login_id: &str) -> Polled {
    let (nonce, verifier, edition, expired) = {
        let map = sessions().lock().expect("sessions");
        let Some(session) = map.get(login_id) else {
            return Polled::Failed(AuthError::new(KIND_UNKNOWN_LOGIN, "登录请求不存在或已结束"));
        };
        match session.phase {
            Phase::Pending => {}
            Phase::Cancelled => {
                return Polled::Failed(AuthError::new(KIND_CANCELLED, "登录已取消"))
            }
            Phase::Expired => {
                return Polled::Failed(AuthError::new(KIND_EXPIRED, "授权已过期，请重新发起"))
            }
            Phase::Failed => return Polled::Failed(failure_of(session)),
            Phase::Committing | Phase::Success => {
                return Polled::Failed(AuthError::new(KIND_FINISHED, "该登录请求已完成"))
            }
        }
        let elapsed = session.created.elapsed().as_secs() as i64;
        (
            session.nonce.clone(),
            session.verifier.clone(),
            session.edition,
            elapsed > SESSION_TTL_SECONDS,
        )
    };

    if expired {
        fail(login_id, KIND_EXPIRED, "授权已过期，请重新发起");
        return Polled::Failed(AuthError::new(KIND_EXPIRED, "授权已过期，请重新发起"));
    }

    let (status, body) = get_json(&poll_url(edition, &nonce, &verifier), None).await;
    let Some(token) = device_token_payload(status, &body) else {
        // 还没点授权，或者一次网络抖动：继续等，TTL 兜底（与 CLI 行为一致）。
        if let Err(err) = ensure_pending(login_id) {
            return Polled::Failed(err);
        }
        return Polled::Pending;
    };

    // 上面是 await 点：等待期间用户可能已经取消。
    if let Err(err) = ensure_pending(login_id) {
        return Polled::Failed(err);
    }

    let access = token
        .get("token")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let refresh = token
        .get("refresh_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let expires_at_ms = expires_at_ms(&token);

    let user = fetch_userinfo(edition, &access).await;
    let Some(identity) = parse_identity(&token, user.as_ref()) else {
        return Polled::Failed(AuthError::new(
            KIND_PROFILE_MISSING,
            "拿不到账号 uid，无法入库",
        ));
    };

    Polled::Token(DeviceToken {
        access_token: access,
        refresh_token: refresh,
        expires_at_ms,
        identity,
    })
}

async fn fetch_userinfo(edition: QoderEdition, access: &str) -> Option<Value> {
    let (status, body) = get_json(&userinfo_url(edition), Some(access)).await;
    if (200..300).contains(&status) {
        Some(body)
    } else {
        None
    }
}

async fn get_json(url: &str, token: Option<&str>) -> (u16, Value) {
    let mut headers = HashMap::new();
    headers.insert("Accept".to_string(), "application/json".to_string());
    if let Some(token) = token {
        headers.insert("Authorization".to_string(), format!("Bearer {token}"));
    }
    let (status, _headers, body) = wb_switch_core::modules::config::http_request_raw(
        url,
        "GET",
        None,
        Some(&headers),
        None,
        true,
    )
    .await;
    let parsed = serde_json::from_str(&body).unwrap_or(Value::Null);
    (status, parsed)
}

/// 轮询用的是 2 秒一次的网络请求，超时由 HTTP 客户端兜住。

// ---------------------------------------------------------------------------
// 门禁与终态
// ---------------------------------------------------------------------------

pub fn cancel(login_id: &str) -> bool {
    let mut map = sessions().lock().expect("sessions");
    match map.get_mut(login_id) {
        // 只对 `Pending` 生效：已进入 `Committing` 的会话正在落库，取消必须落空。
        Some(session) if session.phase == Phase::Pending => {
            session.phase = Phase::Cancelled;
            true
        }
        _ => false,
    }
}

/// 一次性提交门禁：只有仍 `Pending` 的会话能落库，并带出会话里不可变的版本。
///
/// 与 WorkBuddy 登录同一套纪律（#72 的教训）：`poll` 等网络期间可能被取消、
/// TTL 也可能刚好到期，落库前必须原子判定，**不允许"取不到版本就默认某个站"**。
pub fn begin_commit(login_id: &str) -> Result<QoderEdition, AuthError> {
    let mut map = sessions().lock().expect("sessions");
    let Some(session) = map.get_mut(login_id) else {
        return Err(AuthError::new(KIND_UNKNOWN_LOGIN, "登录请求不存在或已结束"));
    };
    if session.created.elapsed().as_secs() as i64 > SESSION_TTL_SECONDS {
        session.phase = Phase::Expired;
        return Err(AuthError::new(KIND_EXPIRED, "授权已过期，请重新发起"));
    }
    match session.phase {
        Phase::Pending => {
            session.phase = Phase::Committing;
            Ok(session.edition)
        }
        Phase::Cancelled => Err(AuthError::new(KIND_CANCELLED, "登录已取消")),
        Phase::Expired => Err(AuthError::new(KIND_EXPIRED, "授权已过期，请重新发起")),
        Phase::Failed => Err(failure_of(session)),
        Phase::Committing | Phase::Success => {
            Err(AuthError::new(KIND_FINISHED, "该登录请求已完成"))
        }
    }
}

fn failure_of(session: &Session) -> AuthError {
    let error = session.error.clone().unwrap_or_default();
    let kind = error
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or(KIND_NETWORK);
    let kind: &'static str = match kind {
        KIND_PROFILE_MISSING => KIND_PROFILE_MISSING,
        KIND_CANCELLED => KIND_CANCELLED,
        KIND_EXPIRED => KIND_EXPIRED,
        _ => KIND_NETWORK,
    };
    AuthError::new(
        kind,
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("授权失败"),
    )
}

fn ensure_pending(login_id: &str) -> Result<(), AuthError> {
    let map = sessions().lock().expect("sessions");
    let Some(session) = map.get(login_id) else {
        return Err(AuthError::new(KIND_UNKNOWN_LOGIN, "登录请求不存在或已结束"));
    };
    match session.phase {
        Phase::Pending => Ok(()),
        Phase::Cancelled => Err(AuthError::new(KIND_CANCELLED, "登录已取消")),
        Phase::Expired => Err(AuthError::new(KIND_EXPIRED, "授权已过期，请重新发起")),
        Phase::Failed => Err(failure_of(session)),
        Phase::Committing | Phase::Success => {
            Err(AuthError::new(KIND_FINISHED, "该登录请求已完成"))
        }
    }
}

pub fn complete(login_id: &str) {
    let mut map = sessions().lock().expect("sessions");
    if let Some(session) = map.get_mut(login_id) {
        if matches!(session.phase, Phase::Committing | Phase::Pending) {
            session.phase = Phase::Success;
        }
    }
}

pub fn fail(login_id: &str, kind: &'static str, message: impl Into<String>) {
    let mut map = sessions().lock().expect("sessions");
    if let Some(session) = map.get_mut(login_id) {
        if !matches!(session.phase, Phase::Pending | Phase::Committing) {
            return;
        }
        session.phase = if kind == KIND_EXPIRED {
            Phase::Expired
        } else {
            Phase::Failed
        };
        session.error = Some(json!({ "kind": kind, "message": message.into() }));
    }
}

#[cfg(test)]
pub fn snapshot(login_id: &str) -> Option<(Phase, Option<Value>)> {
    let map = sessions().lock().expect("sessions");
    map.get(login_id)
        .map(|session| (session.phase.clone(), session.error.clone()))
}

// ---------------------------------------------------------------------------
// 落库
// ---------------------------------------------------------------------------

pub struct Persisted {
    pub account_id: String,
    pub edition: &'static str,
    pub nickname: String,
    pub email: Option<String>,
    pub token_expires_at: Option<String>,
    pub fingerprint: String,
    pub has_refresh: bool,
}

impl Persisted {
    pub fn to_json(&self) -> Value {
        json!({
            "accountId": self.account_id,
            "edition": self.edition,
            "nickname": self.nickname,
            "email": self.email,
            "tokenExpiresAt": self.token_expires_at,
            "fingerprint": self.fingerprint,
            "hasRefresh": self.has_refresh,
            "source": SOURCE_OWN_BROWSER,
        })
    }
}

/// 令牌进钥匙串、账号与授权进 SQLite。
///
/// 顺序：先写钥匙串（唯一需要显式回滚的一步），再写库；库写失败就把旧凭据恢复回去，
/// 不留"钥匙串里有令牌但没有授权行"的半套状态。
pub fn persist(app: &App, edition: QoderEdition, token: &DeviceToken) -> RpcResult<Persisted> {
    let account = app.with_db(|db| {
        db.upsert_qoder_account(
            edition.as_str(),
            &token.identity.uid,
            token.identity.name.as_deref(),
            token.identity.email.as_deref(),
        )
    })?;

    let access = access_key(&account.id);
    let refresh = refresh_key(&account.id);
    let snapshot = Snapshot::capture(app, &[access.clone(), refresh.clone()])?;

    if let Err(err) = app.secure.set(&access, token.access_token.as_bytes()) {
        return Err(credential_error(err, "qoder/access"));
    }
    if let Some(value) = token.refresh_token.as_deref() {
        if let Err(err) = app.secure.set(&refresh, value.as_bytes()) {
            snapshot.restore(app);
            return Err(credential_error(err, "qoder/refresh"));
        }
    }

    let expires_at = token.expires_at_ms.and_then(ms_to_rfc3339);
    let digest = fingerprint(token.access_token.as_bytes());
    let authorization = app.with_db(|db| {
        db.upsert_qoder_authorization(
            &account.id,
            SOURCE_OWN_BROWSER,
            None,
            Some(&keychain_ref(&access)),
            "valid",
            expires_at.as_deref(),
            Some(&digest),
        )
    });
    if let Err(err) = authorization {
        // 授权行没写成功：钥匙串里不该留一份没人认领的令牌。
        snapshot.restore(app);
        return Err(err);
    }

    Ok(Persisted {
        account_id: account.id,
        edition: edition.as_str(),
        nickname: account.nickname,
        email: account.email,
        token_expires_at: expires_at,
        fingerprint: digest,
        has_refresh: token.refresh_token.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;
    use std::time::Duration;

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

    fn token(uid: &str, access: &str) -> DeviceToken {
        DeviceToken {
            access_token: access.to_string(),
            refresh_token: Some("rt-1".to_string()),
            expires_at_ms: Some(1_795_354_934_000),
            identity: Identity {
                uid: uid.to_string(),
                name: Some("测试账号".to_string()),
                email: Some("q@e.com".to_string()),
            },
        }
    }

    fn seed(login_id: &str, edition: QoderEdition, phase: Phase, age: Duration) {
        sessions().lock().unwrap().insert(
            login_id.to_string(),
            Session {
                nonce: "n".into(),
                verifier: "v".into(),
                edition,
                created: Instant::now() - age,
                phase,
                error: None,
            },
        );
    }

    #[test]
    fn pkce_matches_the_cli_shape() {
        let verifier = new_verifier();
        assert_eq!(verifier.len(), 86, "64 字节 base64url 无填充应是 86 字符");
        assert!(verifier
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        // challenge 固定 32 字节 sha256 → 43 字符。
        let challenge = challenge_for(&verifier);
        assert_eq!(challenge.len(), 43);
        // 已知向量：同一 verifier 必须得到同一 challenge。
        assert_eq!(challenge_for("abc"), challenge_for("abc"));
        assert_ne!(challenge_for("abc"), challenge_for("abd"));
        // 与 RFC 7636 的官方样例一致（S256）。
        assert_eq!(
            challenge_for("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn authorize_url_carries_every_parameter_and_the_right_site() {
        let url = authorize_url(QoderEdition::Global, "ch", "no", "mid");
        assert!(url.starts_with("https://qoder.com/device/selectAccounts?"));
        for part in [
            "challenge=ch",
            "challenge_method=S256",
            "nonce=no",
            "machine_id=mid",
            "client_id=732aef47-9cf2-46a2-95fe-4cebb5d0d1fa",
        ] {
            assert!(url.contains(part), "缺 {part}: {url}");
        }
        // 两站参数互不通用，也不该带 redirect_uri。
        assert!(!url.contains("redirect_uri"));
        let cn = authorize_url(QoderEdition::Cn, "ch", "no", "mid");
        assert!(cn.starts_with("https://qoder.cn/device/selectAccounts?"));
        assert!(cn.contains("e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb"));
        assert!(poll_url(QoderEdition::Cn, "n", "v")
            .starts_with("https://openapi.qoder.com.cn/api/v1/deviceToken/poll?"));
        assert!(poll_url(QoderEdition::Global, "n", "v")
            .starts_with("https://openapi.qoder.sh/api/v1/deviceToken/poll?"));
    }

    #[test]
    fn only_a_200_with_a_token_counts_as_signed_in() {
        let ok = json!({"token": "dt-1", "refresh_token": "rt-1", "user_id": "u-1"});
        assert!(device_token_payload(200, &ok).is_some());

        // 未授权：200 但没有 token，或 4xx，或网络错误（status 0）。
        assert!(device_token_payload(200, &json!({"code": 0})).is_none());
        assert!(device_token_payload(200, &json!({"token": "   "})).is_none());
        assert!(device_token_payload(400, &ok).is_none());
        assert!(device_token_payload(0, &json!({"code": -1, "message": "timeout"})).is_none());

        // `data` 包裹的兼容形态。
        let wrapped = json!({"code": 0, "data": {"token": "dt-2"}});
        assert_eq!(
            device_token_payload(200, &wrapped)
                .unwrap()
                .get("token")
                .unwrap(),
            "dt-2"
        );
    }

    #[test]
    fn identity_prefers_the_poll_uid_and_falls_back_to_userinfo() {
        let poll_uid = json!({"token": "dt", "user_id": "u-from-poll"});
        let user = json!({"id": "u-from-info", "name": "名字", "email": "a@b.c"});
        let identity = parse_identity(&poll_uid, Some(&user)).unwrap();
        assert_eq!(identity.uid, "u-from-poll");
        assert_eq!(identity.name.as_deref(), Some("名字"));
        assert_eq!(identity.email.as_deref(), Some("a@b.c"));

        // 轮询没给 uid：回落到 userinfo。
        let identity = parse_identity(&json!({"token": "dt"}), Some(&user)).unwrap();
        assert_eq!(identity.uid, "u-from-info");

        // 数字 id 也认。
        let identity = parse_identity(&json!({"token": "dt"}), Some(&json!({"id": 42}))).unwrap();
        assert_eq!(identity.uid, "42");

        // 都没有 uid：不许入库。
        assert!(parse_identity(&json!({"token": "dt"}), None).is_none());
        assert!(parse_identity(&json!({"token": "dt"}), Some(&json!({"name": "无名"}))).is_none());
        // userinfo 挂了但轮询有 uid：仍然可以入库，只是没有昵称。
        let identity = parse_identity(&json!({"token": "dt", "uid": "u-9"}), None).unwrap();
        assert_eq!(identity.uid, "u-9");
        assert!(identity.email.is_none());
    }

    #[test]
    fn expiry_reads_dates_epochs_and_relative_seconds() {
        // `expires_in` 是相对秒：只断言落在一个合理区间，不把 now() 钉死。
        let relative = expires_at_ms(&json!({"expires_in": 3600})).unwrap();
        assert!(relative > now_ms() + 3_500_000);
        assert!(relative < now_ms() + 3_700_000);
        let abs = expires_at_ms(&json!({"expires_at": "2027-01-01T00:00:00Z"})).unwrap();
        assert_eq!(ms_to_rfc3339(abs).unwrap(), "2027-01-01T00:00:00Z");
        // 秒级 epoch。
        assert_eq!(
            expires_at_ms(&json!({"expire_time": 1_795_354_934i64})).unwrap(),
            1_795_354_934_000
        );
        // 毫秒级 epoch 不再乘 1000。
        assert_eq!(
            expires_at_ms(&json!({"expires_at": 1_795_354_934_000i64})).unwrap(),
            1_795_354_934_000
        );
        assert!(expires_at_ms(&json!({})).is_none());
        assert!(expires_at_ms(&json!({"expires_in": 0})).is_none());
    }

    #[test]
    fn keys_are_scoped_by_account_and_never_shared() {
        assert_eq!(access_key("qa_1"), "qoder/qa_1/own/access");
        assert_eq!(refresh_key("qa_1"), "qoder/qa_1/own/refresh");
        assert_eq!(
            keychain_ref("qoder/qa_1/own/access"),
            "keychain:qoder/qa_1/own/access"
        );
        // 与 WorkBuddy 的键空间分开。
        assert!(!access_key("qa_1").starts_with("workbuddy/"));
    }

    #[test]
    fn commit_gate_is_one_shot_and_returns_the_edition() {
        seed(
            "q73-ok",
            QoderEdition::Global,
            Phase::Pending,
            Duration::ZERO,
        );
        assert!(matches!(begin_commit("q73-ok"), Ok(QoderEdition::Global)));
        assert_eq!(snapshot("q73-ok").unwrap().0, Phase::Committing);
        assert_eq!(begin_commit("q73-ok").unwrap_err().kind, KIND_FINISHED);
        assert!(!cancel("q73-ok"), "Committing 会话不能再被取消");
        sessions().lock().unwrap().remove("q73-ok");
    }

    #[test]
    fn cancelled_or_expired_sessions_can_never_commit() {
        seed(
            "q73-cancel",
            QoderEdition::Cn,
            Phase::Cancelled,
            Duration::ZERO,
        );
        assert_eq!(begin_commit("q73-cancel").unwrap_err().kind, KIND_CANCELLED);
        seed(
            "q73-expired",
            QoderEdition::Cn,
            Phase::Pending,
            Duration::from_secs(SESSION_TTL_SECONDS as u64 + 1),
        );
        assert_eq!(begin_commit("q73-expired").unwrap_err().kind, KIND_EXPIRED);
        assert_eq!(snapshot("q73-expired").unwrap().0, Phase::Expired);
        assert_eq!(
            begin_commit("q73-missing").unwrap_err().kind,
            KIND_UNKNOWN_LOGIN
        );
        sessions().lock().unwrap().remove("q73-cancel");
        sessions().lock().unwrap().remove("q73-expired");
    }

    #[test]
    fn persist_stores_the_tokens_outside_sqlite() {
        let (app, _dir) = test_app();
        let saved = persist(&app, QoderEdition::Global, &token("u-1", "dt-secret")).unwrap();
        assert_eq!(saved.edition, "qoder_global");
        assert!(saved.has_refresh);

        let key = access_key(&saved.account_id);
        assert_eq!(app.secure.get(&key).unwrap(), b"dt-secret");
        assert_eq!(
            app.secure.get(&refresh_key(&saved.account_id)).unwrap(),
            b"rt-1"
        );

        let row = app
            .with_db(|db| db.list_qoder_authorizations(&saved.account_id))
            .unwrap();
        assert_eq!(row.len(), 1);
        assert_eq!(row[0].source, SOURCE_OWN_BROWSER);
        assert_eq!(
            row[0].secret_ref.as_deref(),
            Some("keychain:qoder/".to_string() + &saved.account_id + "/own/access").as_deref()
        );
        assert_eq!(row[0].auth_status, "valid");
        assert_eq!(
            row[0].token_expires_at.as_deref(),
            Some("2026-11-22T13:42:14Z")
        );
        assert!(row[0].fingerprint.is_some());

        // 令牌不在对外 JSON 里：只有 `hasSecret` 与指纹。
        let shown = row[0].to_json();
        assert_eq!(shown.get("hasSecret").and_then(Value::as_bool), Some(true));
        assert!(shown.get("accessToken").is_none() && shown.get("token").is_none());
    }

    #[test]
    fn same_uid_on_both_sites_gets_two_accounts_and_two_secrets() {
        let (app, _dir) = test_app();
        let cn = persist(&app, QoderEdition::Cn, &token("u-same", "dt-cn")).unwrap();
        let global = persist(&app, QoderEdition::Global, &token("u-same", "dt-global")).unwrap();
        assert_ne!(cn.account_id, global.account_id);
        assert_eq!(
            app.secure.get(&access_key(&cn.account_id)).unwrap(),
            b"dt-cn"
        );
        assert_eq!(
            app.secure.get(&access_key(&global.account_id)).unwrap(),
            b"dt-global"
        );
        assert_eq!(
            app.with_db(|db| db.list_qoder_accounts(None))
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn reauthorizing_updates_in_place_and_keeps_one_row() {
        let (app, _dir) = test_app();
        let first = persist(&app, QoderEdition::Global, &token("u-1", "dt-old")).unwrap();
        let second = persist(&app, QoderEdition::Global, &token("u-1", "dt-new")).unwrap();
        assert_eq!(first.account_id, second.account_id);
        assert_eq!(
            app.secure.get(&access_key(&first.account_id)).unwrap(),
            b"dt-new"
        );
        let rows = app
            .with_db(|db| db.list_qoder_authorizations(&first.account_id))
            .unwrap();
        assert_eq!(rows.len(), 1, "同一来源重复授权不应多出一行");
    }
}
