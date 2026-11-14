//! WorkBuddy 官方 OAuth 扫码登录（PR1b-1）。
//!
//! 只复用上游的**协议**（端点、参数、请求头），不复用它的落盘方式：
//! `wb-switch-core::oauth` 会把 access/refresh token 明文写进
//! `~/.wb-switch/accounts.json`，而 AICC 的凭据必须进钥匙串、账号进 SQLite。
//!
//! 安全性约定：
//! - 令牌只在本模块与 `securestore` 之间流动；
//! - `TokenPayload` 故意不实现 `Serialize`/`Debug`，避免顺手打进日志或 RPC；
//! - 对外只给指纹（`fingerprint`）。
//!
//! 本流程**不读写** `~/.wb-switch`，也不碰 WorkBuddy 客户端自己的认证文件。

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use wb_switch_core::modules::config::{http_request, norm_ts, now_ms, OAUTH_TIMEOUT_SECONDS};
use wb_switch_core::modules::variant::WbVariant;

/// 单次 HTTP 的最长等待。上游客户端是 30s，界面轮询等不了那么久。
const HTTP_TIMEOUT: Duration = Duration::from_secs(12);
/// 登录会话存活时间，与上游一致（10 分钟）。
const SESSION_TTL_SECONDS: i64 = OAUTH_TIMEOUT_SECONDS;

pub const KIND_NETWORK: &str = "network";
pub const KIND_EXPIRED: &str = "expired";
pub const KIND_CANCELLED: &str = "cancelled";
pub const KIND_DOMAIN_MISMATCH: &str = "domain_mismatch";
pub const KIND_PROFILE_MISSING: &str = "profile_missing";
pub const KIND_UNKNOWN_LOGIN: &str = "unknown_login";
/// 会话已经结束（成功过），界面应改为刷新账号列表。
pub const KIND_FINISHED: &str = "finished";
/// 服务端给的登录地址不在本档位可信域名内，拒绝交给系统浏览器打开。
pub const KIND_UNTRUSTED_URL: &str = "untrusted_url";

/// 解密后的账号资料。令牌单独放在 `TokenPayload` 里，不和资料混在一起。
#[derive(Clone)]
pub struct Profile {
    pub uid: String,
    pub nickname: Option<String>,
    pub email: Option<String>,
}

/// 登录成功后拿到的一切。**不要**给它加 Serialize/Debug。
pub struct TokenPayload {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub domain: String,
    pub expires_at_ms: Option<i64>,
    pub profile: Profile,
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
    state: String,
    variant: WbVariant,
    created: Instant,
    phase: Phase,
    result: Option<Value>,
    error: Option<Value>,
}

static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

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

// ---------------------------------------------------------------------------
// 端点（与官方客户端一致，全部按档位拼）
// ---------------------------------------------------------------------------

pub fn state_url(variant: WbVariant) -> String {
    format!(
        "{}{}/auth/state?platform={}",
        variant.api_endpoint(),
        variant.api_prefix(),
        variant.oauth_platform()
    )
}

pub fn token_url(variant: WbVariant, state: &str) -> String {
    format!(
        "{}{}/auth/token?state={state}",
        variant.api_endpoint(),
        variant.api_prefix()
    )
}

pub fn account_url(variant: WbVariant, state: &str) -> String {
    format!(
        "{}{}/login/account?state={state}",
        variant.api_endpoint(),
        variant.api_prefix()
    )
}

pub fn refresh_url(variant: WbVariant) -> String {
    format!(
        "{}{}/auth/token/refresh",
        variant.api_endpoint(),
        variant.api_prefix()
    )
}

// ---------------------------------------------------------------------------
// 响应解析（纯函数，便于单测）
// ---------------------------------------------------------------------------

/// `auth/state` 的响应 →（state, 浏览器/二维码地址）。
pub fn parse_state_response(resp: &Value) -> Result<(String, String), String> {
    let code = resp.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code == -1 {
        let message = resp
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("网络请求失败");
        return Err(message.to_string());
    }
    let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
    let state = data
        .get("state")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "auth/state 响应缺少 state".to_string())?
        .to_string();
    let auth_url = data
        .get("authUrl")
        .and_then(Value::as_str)
        .or_else(|| data.get("auth_url").and_then(Value::as_str))
        .or_else(|| data.get("url").and_then(Value::as_str))
        .map(str::to_string)
        .unwrap_or_default();
    Ok((state, auth_url))
}

pub enum Polled {
    /// 还没扫码 / 还没确认。
    Pending,
    Token(Box<TokenPayload>),
    Failed(AuthError),
}

pub fn parse_token_response(resp: &Value) -> Polled {
    let code = resp.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code == -1 {
        let message = resp
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("网络请求失败");
        return Polled::Failed(AuthError::new(KIND_NETWORK, message.to_string()));
    }
    let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
    let access_token = data
        .get("accessToken")
        .and_then(Value::as_str)
        .or_else(|| data.get("access_token").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    if access_token.is_empty() {
        // 未扫码 / 未确认时官方也是 200 空数据，这里只能算 pending。
        return Polled::Pending;
    }
    // token 接口不一定带 uid；缺失时先留空，等 login/account 补。
    let profile = parse_profile(&data).unwrap_or(Profile {
        uid: String::new(),
        nickname: None,
        email: None,
    });
    Polled::Token(Box::new(TokenPayload {
        access_token,
        refresh_token: data
            .get("refreshToken")
            .and_then(Value::as_str)
            .or_else(|| data.get("refresh_token").and_then(Value::as_str))
            .map(str::to_string),
        domain: data
            .get("domain")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        expires_at_ms: expires_at_ms(&data, "expiresAt", "expiresIn"),
        profile,
    }))
}

/// `login/account` 的响应 → 资料。缺 uid 视为失败，不能入库。
pub fn parse_profile(data: &Value) -> Result<Profile, AuthError> {
    let uid = data
        .get("uid")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AuthError::new(KIND_PROFILE_MISSING, "登录响应缺少 uid，无法入库"))?;
    Ok(Profile {
        uid: uid.to_string(),
        nickname: data
            .get("nickname")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
        email: data
            .get("email")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string),
    })
}

fn expires_at_ms(data: &Value, absolute: &str, relative: &str) -> Option<i64> {
    norm_ts(data.get(absolute)).or_else(|| {
        data.get(relative)
            .and_then(Value::as_i64)
            .map(|secs| now_ms() + secs * 1000)
    })
}

/// AICC 的 `accounts.provider` 值。
///
/// 两个站点共用 `platform = "workbuddy"`，靠 provider 区分：这是 v13 定下的模型，
/// 签到 / 成长 / 旅行 / 授权检查都按它判断能不能走国内版接口。
pub fn provider_of(variant: WbVariant) -> &'static str {
    match variant {
        WbVariant::Cn => "workbuddy",
        WbVariant::Ai => "workbuddy_ai",
    }
}

/// 严格解析 RPC 参数里的 `variant`（`cn` / `ai`）。
///
/// 不用 `WbVariant::parse`：它对未知值一律回落国内版，那会让"写错参数"
/// 变成"悄悄用国内版登录国际账号"。
pub fn parse_variant(raw: &str) -> Option<WbVariant> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "cn" => Some(WbVariant::Cn),
        "ai" | "intl" | "global" => Some(WbVariant::Ai),
        _ => None,
    }
}

/// 登录地址是否可信：**必须 https**，且主机属于该档位的产品域。
///
/// 这个字符串是服务端给的，界面会直接丢给系统浏览器，因此不能只做格式检查：
/// `javascript:` / 第三方域名 / 主域名后缀仿冒（`www.codebuddy.cn.evil.com`）
/// 都必须拦在这里。落库前会话里的档位是唯一判据，这里也按档位限制域名。
pub fn is_trusted_login_url(variant: WbVariant, raw: &str) -> bool {
    let url = raw.trim();
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    // authority 截止到 path / query / fragment 前的第一个分隔符。
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // userinfo 形式（`https://a@b/`）里真正的目标是 `@` 之后那一段，取最后一段。
    let host_port = authority.rsplit('@').next().unwrap_or("");
    let host = host_port
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    if host.is_empty() || host.contains('\\') || host.chars().any(char::is_whitespace) {
        return false;
    }
    let domains: &[&str] = match variant {
        WbVariant::Cn => &["codebuddy.cn", "workbuddy.cn"],
        WbVariant::Ai => &["codebuddy.ai", "workbuddy.ai"],
    };
    domains
        .iter()
        .any(|root| host == *root || host.ends_with(&format!(".{root}")))
}

/// 钥匙串里的键名。按 uid 隔离，换账号不会互相覆盖。
pub fn ms_to_rfc3339(ms: i64) -> Option<String> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
}

// ---------------------------------------------------------------------------
// 会话状态机
// ---------------------------------------------------------------------------

pub struct StartInfo {
    pub login_id: String,
    pub verification_uri: String,
    pub expires_in: i64,
}

/// 发起登录：拿到 state，写进内存会话表。
pub async fn start(variant: WbVariant) -> Result<StartInfo, AuthError> {
    let resp = call(&state_url(variant), "POST", Some(json!({})), None).await?;
    let (state, auth_url) =
        parse_state_response(&resp).map_err(|message| AuthError::new(KIND_NETWORK, message))?;
    let login_id = format!("wb1b_{}", uuid::Uuid::new_v4().simple());
    let auth_url = if auth_url.is_empty() {
        format!("{}/login?state={state}", variant.api_endpoint())
    } else {
        auth_url
    };
    // 界面会把这个地址交给系统浏览器打开：先按档位校验协议与域名。
    // 失败时直接拒绝发起会话，不留一个"能轮询但链接不可信"的账号。
    if !is_trusted_login_url(variant, &auth_url) {
        return Err(AuthError::new(
            KIND_UNTRUSTED_URL,
            format!(
                "服务端返回的登录地址不在 {} 的可信域名内，已拒绝打开",
                variant.api_endpoint()
            ),
        ));
    }

    let mut map = sessions().lock().expect("sessions");
    map.insert(
        login_id.clone(),
        Session {
            state,
            variant,
            created: Instant::now(),
            phase: Phase::Pending,
            result: None,
            error: None,
        },
    );
    Ok(StartInfo {
        login_id,
        verification_uri: auth_url,
        expires_in: SESSION_TTL_SECONDS,
    })
}

/// 当前会话的可读快照（终态直接返回缓存，避免重复请求）。
#[cfg(test)] // 只有用例需要直接读会话表，生产走 `poll`
pub fn snapshot(login_id: &str) -> Option<(Phase, Option<Value>, Option<Value>)> {
    let map = sessions().lock().expect("sessions");
    map.get(login_id)
        .map(|s| (s.phase.clone(), s.result.clone(), s.error.clone()))
}

pub fn cancel(login_id: &str) -> bool {
    let mut map = sessions().lock().expect("sessions");
    match map.get_mut(login_id) {
        // 只对 `Pending` 生效：已经进入 `Committing` 的会话正在落库，取消必须落空，
        // 否则界面会显示"已取消"而账号其实已经写进去了。
        Some(session) if session.phase == Phase::Pending => {
            session.phase = Phase::Cancelled;
            true
        }
        _ => false,
    }
}

/// 一次性提交门禁：只有**仍处于 `Pending`** 的会话才允许落库，并把会话里
/// 不可变的档位一并带出。
///
/// `poll()` 要等网络，期间 `cancel()` 可能把会话改成取消、TTL 也可能刚好到期。
/// 落库前必须在这里做原子判定并抢占到 `Committing`：抢不到就绝不写库。
/// 这也是档位的唯一取法——以前调用方用 `session_variant(..).unwrap_or(Cn)`
/// 兜底，会把取不到版本的会话悄悄记成国内版账号。
pub fn begin_commit(login_id: &str) -> Result<WbVariant, AuthError> {
    let mut map = sessions().lock().expect("sessions");
    let Some(session) = map.get_mut(login_id) else {
        return Err(AuthError::new(KIND_UNKNOWN_LOGIN, "登录请求不存在或已结束"));
    };
    if session.created.elapsed().as_secs() as i64 > SESSION_TTL_SECONDS {
        session.phase = Phase::Expired;
        return Err(AuthError::new(KIND_EXPIRED, "二维码已过期，请重新获取"));
    }
    match session.phase {
        Phase::Pending => {
            session.phase = Phase::Committing;
            Ok(session.variant)
        }
        Phase::Cancelled => Err(AuthError::new(KIND_CANCELLED, "登录已取消")),
        Phase::Expired => Err(AuthError::new(KIND_EXPIRED, "二维码已过期，请重新获取")),
        Phase::Failed => Err(failure_of(session)),
        Phase::Committing | Phase::Success => {
            Err(AuthError::new(KIND_FINISHED, "该登录请求已完成"))
        }
    }
}

/// 会话失败时对外暴露的稳定错误。只保留界面需要区分的两类原因，
/// 其余一律归到网络错误（与改造前一致）。
fn failure_of(session: &Session) -> AuthError {
    let error = session.error.clone().unwrap_or_default();
    let kind = error
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or(KIND_NETWORK);
    let kind: &'static str = if kind == KIND_DOMAIN_MISMATCH {
        KIND_DOMAIN_MISMATCH
    } else if kind == KIND_PROFILE_MISSING {
        KIND_PROFILE_MISSING
    } else if kind == KIND_CANCELLED {
        KIND_CANCELLED
    } else if kind == KIND_EXPIRED {
        KIND_EXPIRED
    } else {
        KIND_NETWORK
    };
    AuthError::new(
        kind,
        error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("登录失败"),
    )
}

/// 会话仍在等待吗？`poll` 等待网络期间可能被 `cancel()` 结束。
fn ensure_pending(login_id: &str) -> Result<(), AuthError> {
    let map = sessions().lock().expect("sessions");
    let Some(session) = map.get(login_id) else {
        return Err(AuthError::new(KIND_UNKNOWN_LOGIN, "登录请求不存在或已结束"));
    };
    match session.phase {
        Phase::Pending => Ok(()),
        Phase::Cancelled => Err(AuthError::new(KIND_CANCELLED, "登录已取消")),
        Phase::Expired => Err(AuthError::new(KIND_EXPIRED, "二维码已过期，请重新获取")),
        Phase::Failed => Err(failure_of(session)),
        Phase::Committing | Phase::Success => {
            Err(AuthError::new(KIND_FINISHED, "该登录请求已完成"))
        }
    }
}

/// 落库成功。只有通过门禁（或仍 `Pending` 的单测路径）的会话能被标记成功，
/// 已取消 / 已过期的会话不会被"抢救"回来。
pub fn complete(login_id: &str, result: Value) {
    let mut map = sessions().lock().expect("sessions");
    if let Some(session) = map.get_mut(login_id) {
        if matches!(session.phase, Phase::Committing | Phase::Pending) {
            session.phase = Phase::Success;
            session.result = Some(result);
        }
    }
}

/// 落库失败 / 网络失败。只覆盖还在流程中的会话，不会把已取消改写成失败。
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

/// 轮询一次。只有 `Pending` 才会真的打网络，终态直接返回缓存。
pub async fn poll(login_id: &str) -> Polled {
    let (state, variant, expired) = {
        let map = sessions().lock().expect("sessions");
        let Some(session) = map.get(login_id) else {
            return Polled::Failed(AuthError::new(KIND_UNKNOWN_LOGIN, "登录请求不存在或已结束"));
        };
        match session.phase {
            Phase::Pending => {}
            Phase::Cancelled => {
                return Polled::Failed(AuthError::new(KIND_CANCELLED, "登录已取消"));
            }
            Phase::Expired => {
                return Polled::Failed(AuthError::new(KIND_EXPIRED, "二维码已过期，请重新获取"));
            }
            Phase::Failed => {
                return Polled::Failed(failure_of(session));
            }
            Phase::Committing => {
                return Polled::Failed(AuthError::new(KIND_FINISHED, "该登录请求已完成"));
            }
            Phase::Success => {
                return Polled::Failed(AuthError::new(KIND_FINISHED, "该登录请求已完成"));
            }
        }
        let elapsed = session.created.elapsed().as_secs() as i64;
        (
            session.state.clone(),
            session.variant,
            elapsed > SESSION_TTL_SECONDS,
        )
    };

    if expired {
        fail(login_id, KIND_EXPIRED, "二维码已过期，请重新获取");
        return Polled::Failed(AuthError::new(KIND_EXPIRED, "二维码已过期，请重新获取"));
    }

    let resp = match call(&token_url(variant, &state), "GET", None, None).await {
        Ok(resp) => resp,
        Err(err) => return Polled::Failed(err),
    };
    let mut payload = match parse_token_response(&resp) {
        Polled::Pending => return Polled::Pending,
        Polled::Failed(err) => return Polled::Failed(err),
        Polled::Token(payload) => payload,
    };

    // 上面 `call` 是 await 点：等待期间用户可能已经点了取消，会话可能已成终态。
    // 这里先确认会话还在等待，避免为一次已取消的登录继续拉资料、更避免把令牌交出去
    // （真正的落库门禁在 `begin_commit`，这里只是尽早止损）。
    if let Err(err) = ensure_pending(login_id) {
        return Polled::Failed(err);
    }

    // 档位一致性：拿国内版的 state 换到国际版令牌会让后续请求被网关拒绝。
    if !variant.matches_domain(&payload.domain) {
        return Polled::Failed(AuthError::new(
            KIND_DOMAIN_MISMATCH,
            format!(
                "登录响应的 domain（{}）与当前档位不符，已拒绝入库",
                payload.domain
            ),
        ));
    }

    // 补拉资料：token 接口本身也带 uid，但没有昵称/邮箱。
    match fetch_profile(variant, &state, &payload).await {
        Ok(profile) => payload.profile = profile,
        Err(err) => {
            if payload.profile.uid.is_empty() {
                return Polled::Failed(err);
            }
        }
    }
    if payload.profile.uid.is_empty() {
        return Polled::Failed(AuthError::new(
            KIND_PROFILE_MISSING,
            "拿不到账号 uid，无法入库",
        ));
    }

    Polled::Token(payload)
}

async fn fetch_profile(
    variant: WbVariant,
    state: &str,
    payload: &TokenPayload,
) -> Result<Profile, AuthError> {
    use std::collections::HashMap;
    let mut headers = HashMap::new();
    headers.insert(
        "Authorization".to_string(),
        format!("Bearer {}", payload.access_token),
    );
    if !payload.domain.is_empty() {
        headers.insert("X-Domain".to_string(), payload.domain.clone());
    }
    let resp = call(&account_url(variant, state), "GET", None, Some(&headers)).await?;
    let code = resp.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code == -1 {
        let message = resp
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("网络请求失败");
        return Err(AuthError::new(KIND_NETWORK, message.to_string()));
    }
    let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
    // 资料接口缺 uid 时退回 token 里的 uid（官方在部分企业账号上如此）。
    match parse_profile(&data) {
        Ok(profile) => Ok(profile),
        Err(_) if !payload.profile.uid.is_empty() => Ok(payload.profile.clone()),
        Err(err) => Err(err),
    }
}

/// 带超时的 HTTP 调用，把上游的 `{"code":-1}` 约定翻译成 `Result`。
async fn call(
    url: &str,
    method: &str,
    body: Option<Value>,
    headers: Option<&std::collections::HashMap<String, String>>,
) -> Result<Value, AuthError> {
    match tokio::time::timeout(HTTP_TIMEOUT, http_request(url, method, body, headers)).await {
        Err(_) => Err(AuthError::new(
            KIND_NETWORK,
            format!("请求超时（{}s），请检查网络后重试", HTTP_TIMEOUT.as_secs()),
        )),
        Ok(resp) => {
            let code = resp.get("code").and_then(Value::as_i64).unwrap_or(-1);
            if code == -1 {
                let message = resp
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("网络请求失败");
                return Err(AuthError::new(KIND_NETWORK, message.to_string()));
            }
            Ok(resp)
        }
    }
}

// ---------------------------------------------------------------------------
// 令牌刷新（认证失效处理）
// ---------------------------------------------------------------------------

pub enum Refreshed {
    Valid {
        access_token: String,
        refresh_token: Option<String>,
        expires_at_ms: Option<i64>,
    },
    Invalid(AuthError),
}

/// 用 refresh token 换新令牌。失败即视为需要重新登录。
pub async fn refresh(
    variant: WbVariant,
    access_token: &str,
    refresh_token: &str,
    uid: &str,
    domain: &str,
) -> Refreshed {
    use std::collections::HashMap;
    let mut headers = HashMap::new();
    headers.insert("Accept".to_string(), "application/json".to_string());
    headers.insert("Content-Type".to_string(), "application/json".to_string());
    headers.insert(
        "Authorization".to_string(),
        format!("Bearer {access_token}"),
    );
    headers.insert("X-Refresh-Token".to_string(), refresh_token.to_string());
    // 两个档位的官方客户端都发这个头；缺了会被判定成另一个 client 来源。
    headers.insert("X-Auth-Refresh-Source".to_string(), "plugin".to_string());
    if !uid.is_empty() {
        headers.insert("X-User-Id".to_string(), uid.to_string());
    }
    if !domain.is_empty() {
        headers.insert("X-Domain".to_string(), domain.to_string());
    }

    let resp = match call(
        &refresh_url(variant),
        "POST",
        Some(json!({})),
        Some(&headers),
    )
    .await
    {
        Ok(resp) => resp,
        Err(err) => return Refreshed::Invalid(err),
    };
    let code = resp.get("code").and_then(Value::as_i64).unwrap_or(-1);
    if code != 0 && code != 200 {
        return Refreshed::Invalid(AuthError::new(
            KIND_NETWORK,
            "刷新令牌失败，请重新登录".to_string(),
        ));
    }
    let data = resp.get("data").cloned().unwrap_or_else(|| json!({}));
    let access = data
        .get("accessToken")
        .and_then(Value::as_str)
        .or_else(|| data.get("access_token").and_then(Value::as_str))
        .unwrap_or("")
        .trim()
        .to_string();
    if access.is_empty() {
        return Refreshed::Invalid(AuthError::new(KIND_NETWORK, "刷新响应没有新令牌"));
    }
    Refreshed::Valid {
        access_token: access,
        refresh_token: data
            .get("refreshToken")
            .and_then(Value::as_str)
            .or_else(|| data.get("refresh_token").and_then(Value::as_str))
            .map(str::to_string),
        expires_at_ms: expires_at_ms(&data, "expiresAt", "expiresIn"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_follow_the_variant() {
        let cn = WbVariant::Cn;
        assert!(state_url(cn).ends_with("/v2/plugin/auth/state?platform=workbuddy"));
        assert!(token_url(cn, "st-1").ends_with("/v2/plugin/auth/token?state=st-1"));
        assert!(account_url(cn, "st-1").ends_with("/v2/plugin/login/account?state=st-1"));
        assert!(refresh_url(cn).ends_with("/v2/plugin/auth/token/refresh"));
    }

    #[test]
    fn state_response_requires_state() {
        let ok = json!({"code": 0, "data": {"state": "st", "authUrl": "https://x/y"}});
        assert_eq!(
            parse_state_response(&ok).unwrap(),
            ("st".to_string(), "https://x/y".to_string())
        );

        let missing = json!({"code": 0, "data": {}});
        assert!(parse_state_response(&missing).is_err());

        let network = json!({"code": -1, "message": "boom"});
        assert_eq!(parse_state_response(&network).unwrap_err(), "boom");
    }

    #[test]
    fn pending_when_no_access_token() {
        assert!(matches!(
            parse_token_response(&json!({"code": 0, "data": {}})),
            Polled::Pending
        ));
    }

    #[test]
    fn token_response_is_parsed_without_leaking_into_debug() {
        let resp = json!({
            "code": 0,
            "data": {
                "accessToken": "at-123",
                "refreshToken": "rt-456",
                "domain": "www.codebuddy.cn",
                "expiresIn": 3600,
                "uid": "u-1",
                "nickname": "工作号",
                "email": "a@b.c",
            }
        });
        match parse_token_response(&resp) {
            Polled::Token(payload) => {
                assert_eq!(payload.access_token, "at-123");
                assert_eq!(payload.refresh_token.as_deref(), Some("rt-456"));
                assert_eq!(payload.profile.uid, "u-1");
                assert_eq!(payload.profile.nickname.as_deref(), Some("工作号"));
                assert!(payload.expires_at_ms.unwrap() > now_ms());
            }
            _ => panic!("应解析出令牌"),
        }
    }

    #[test]
    fn profile_without_uid_fails() {
        match parse_profile(&json!({"nickname": "x"})) {
            Err(err) => assert_eq!(err.kind, KIND_PROFILE_MISSING),
            Ok(_) => panic!("缺 uid 不应解析成功"),
        }
    }

    #[test]
    fn ms_to_rfc3339_is_stable() {
        assert_eq!(
            ms_to_rfc3339(1_762_000_000_000).as_deref(),
            Some("2025-11-01T12:26:40Z")
        );
    }

    #[test]
    fn cancel_only_affects_pending_sessions() {
        let id = format!("test-{}", uuid::Uuid::new_v4().simple());
        sessions().lock().unwrap().insert(
            id.clone(),
            Session {
                state: "st".into(),
                variant: WbVariant::Cn,
                created: Instant::now(),
                phase: Phase::Pending,
                result: None,
                error: None,
            },
        );
        assert!(cancel(&id));
        assert!(!cancel(&id), "重复取消应是幂等的 false");
        assert_eq!(snapshot(&id).unwrap().0, Phase::Cancelled);
        sessions().lock().unwrap().remove(&id);
    }

    fn seed(login_id: &str, variant: WbVariant, phase: Phase, age: Duration) {
        sessions().lock().unwrap().insert(
            login_id.to_string(),
            Session {
                state: "st".into(),
                variant,
                created: Instant::now() - age,
                phase,
                result: None,
                error: None,
            },
        );
    }

    #[test]
    fn commit_gate_returns_the_session_variant_and_is_one_shot() {
        let id = "commit-ai";
        seed(id, WbVariant::Ai, Phase::Pending, Duration::ZERO);
        // 档位只能从会话里取：这是 provider=workbuddy_ai 的唯一来源。
        assert!(matches!(begin_commit(id), Ok(WbVariant::Ai)));
        assert_eq!(snapshot(id).unwrap().0, Phase::Committing);
        // 第二次提交拿不到门禁，落库不会重复发生。
        assert_eq!(begin_commit(id).unwrap_err().kind, KIND_FINISHED);
        // 进入提交后取消必须落空，否则界面说"已取消"而账号其实已写进去。
        assert!(!cancel(id), "Committing 会话不能再被取消");
        sessions().lock().unwrap().remove(id);
    }

    #[test]
    fn cancelled_or_expired_sessions_can_never_commit() {
        // 取消发生在 `poll` 等网络期间：令牌回来了也不许落库。
        seed(
            "commit-cancelled",
            WbVariant::Ai,
            Phase::Cancelled,
            Duration::ZERO,
        );
        assert_eq!(
            begin_commit("commit-cancelled").unwrap_err().kind,
            KIND_CANCELLED
        );

        // TTL 在等待期间走完：同样是终态，不许落库。
        seed(
            "commit-expired",
            WbVariant::Cn,
            Phase::Pending,
            Duration::from_secs(SESSION_TTL_SECONDS as u64 + 1),
        );
        assert_eq!(
            begin_commit("commit-expired").unwrap_err().kind,
            KIND_EXPIRED
        );
        assert_eq!(snapshot("commit-expired").unwrap().0, Phase::Expired);

        // 会话不存在（进程重启 / 已被清理）：报 unknown_login，而不是默认国内版。
        assert_eq!(
            begin_commit("commit-missing").unwrap_err().kind,
            KIND_UNKNOWN_LOGIN
        );

        for id in ["commit-cancelled", "commit-expired"] {
            sessions().lock().unwrap().remove(id);
        }
    }

    #[test]
    fn pending_recheck_rejects_a_session_cancelled_during_the_await() {
        seed(
            "poll-cancelled",
            WbVariant::Ai,
            Phase::Cancelled,
            Duration::ZERO,
        );
        assert_eq!(
            ensure_pending("poll-cancelled").unwrap_err().kind,
            KIND_CANCELLED
        );
        seed(
            "poll-pending",
            WbVariant::Ai,
            Phase::Pending,
            Duration::ZERO,
        );
        assert!(ensure_pending("poll-pending").is_ok());
        for id in ["poll-cancelled", "poll-pending"] {
            sessions().lock().unwrap().remove(id);
        }
    }

    #[test]
    fn complete_and_fail_never_revive_a_terminal_session() {
        // 已取消的会话收到落库成功回执：仍必须留在 Cancelled。
        seed(
            "done-cancelled",
            WbVariant::Cn,
            Phase::Cancelled,
            Duration::ZERO,
        );
        complete("done-cancelled", json!({"ok": true}));
        assert_eq!(snapshot("done-cancelled").unwrap().0, Phase::Cancelled);
        fail("done-cancelled", KIND_NETWORK, "boom");
        assert_eq!(snapshot("done-cancelled").unwrap().0, Phase::Cancelled);

        // 门禁之后落库失败：Committing → Failed，可以正常报错。
        seed(
            "done-committing",
            WbVariant::Cn,
            Phase::Pending,
            Duration::ZERO,
        );
        assert!(begin_commit("done-committing").is_ok());
        fail("done-committing", KIND_NETWORK, "boom");
        assert_eq!(snapshot("done-committing").unwrap().0, Phase::Failed);

        for id in ["done-cancelled", "done-committing"] {
            sessions().lock().unwrap().remove(id);
        }
    }

    #[test]
    fn login_url_must_be_https_on_the_variant_domain() {
        // 正常形态：官方站点，可带端口与路径参数。
        assert!(is_trusted_login_url(
            WbVariant::Cn,
            "https://www.codebuddy.cn/login?state=st-1"
        ));
        assert!(is_trusted_login_url(
            WbVariant::Ai,
            "https://www.workbuddy.ai/login?state=st-1"
        ));
        assert!(is_trusted_login_url(
            WbVariant::Cn,
            "https://sso.codebuddy.cn:8443/login"
        ));
        assert!(
            !is_trusted_login_url(WbVariant::Cn, "https://HTTPS://X"),
            "协议名大小写要在正确的位置，不能把 host 当协议"
        );

        // 协议不是 https：`javascript:` / 明文 http 一律拒绝。
        assert!(!is_trusted_login_url(WbVariant::Cn, "javascript:alert(1)"));
        assert!(!is_trusted_login_url(
            WbVariant::Cn,
            "http://www.codebuddy.cn/login"
        ));
        assert!(!is_trusted_login_url(WbVariant::Cn, "codebuddy.cn/login"));

        // 域名仿冒与跨档位域名。
        assert!(!is_trusted_login_url(
            WbVariant::Cn,
            "https://www.codebuddy.cn.evil.com/login"
        ));
        assert!(!is_trusted_login_url(
            WbVariant::Cn,
            "https://evil.com/?codebuddy.cn"
        ));
        assert!(!is_trusted_login_url(
            WbVariant::Cn,
            "https://www.codebuddy.cn@evil.com/login"
        ));
        assert!(!is_trusted_login_url(
            WbVariant::Ai,
            "https://www.codebuddy.cn/login"
        ));
        assert!(!is_trusted_login_url(WbVariant::Ai, ""));
    }
}
