//! 成长中心的传输层（PR #67：**只有 GET**）。
//!
//! 两个不确定点在这里被隔离掉：域名路径形态和错误语义。
//!
//! 1. **路径形态**。社区资料里同时存在 `/v2/activity/growth/...` 与
//!    `/activity/growth/...`（上游 travel 用的就是后者）。我们不去猜哪个对，
//!    而是按序试：先 v2，路由不存在（404）再退无 v2。401 / 403 / 429 / 5xx
//!    **绝不**触发退路径 —— 那些是业务答案，换个路径再问一次只会把错误藏起来。
//! 2. **错误语义**。`http_request` 把非 2xx 压成 `{code: <status>, message}`，
//!    和业务码共用一个字段。所以这里统一按数字分类，并明确区分
//!    `Unauthorized`（要换授权）与 `Transient`（等一会儿再来）——
//!    PR #66 的整套换源逻辑都建立在这个区分上。
//!
//! 不写操作：本文件里没有 POST。接受任务、领奖在 #68，接口形态同样要实测。

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::rpc::sanitize;

use wb_switch_core::modules::config::{http_request, is_route_missing, WORKBUDDY_API_ENDPOINT};

/// 单次请求上限。成长中心一次扫描要串 9 个接口，单个卡 30 秒会让整轮看不见进度。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 成长中心在 API 域名下的两种路径形态，按优先级排列。
const PREFIXES: [&str; 2] = ["/v2/activity/growth", "/activity/growth"];

/// 失败分类。**只有 `Unauthorized` 允许触发换源**，其余一律不动授权状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 明确 401：这份授权确实失效了。
    Unauthorized,
    /// 403：业务拒绝（例如"连登天数不足"），授权本身没问题。
    Forbidden,
    /// 429：被限流，等一会儿。
    RateLimited,
    /// 5xx / 网关错误。
    Server,
    /// 网络错误或超时。
    Network,
    /// 路径不存在：可以换另一种形态再试。
    RouteMissing,
    /// 其它业务码。
    Business,
}

impl ErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::RateLimited => "rate_limited",
            Self::Server => "server",
            Self::Network => "network",
            Self::RouteMissing => "route_missing",
            Self::Business => "business",
        }
    }

    /// 值得等下一轮再试的错误。授权类错误不在此列：重试只会继续 401。
    ///
    /// #68 的重试策略直接用它；#67 只读阶段还没有调用方，但语义要现在钉住，
    /// 免得将来某处手写 `kind != Unauthorized` 又把 5xx 当成登录失效。
    #[allow(dead_code)]
    pub fn retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::Server | Self::Network)
    }
}

#[derive(Debug, Clone)]
pub struct GrowthError {
    pub kind: ErrorKind,
    pub http: Option<i64>,
    pub code: Option<i64>,
    pub message: String,
}

impl GrowthError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            http: None,
            code: None,
            message: sanitize(&message.into()),
        }
    }
}

fn code_of(response: &Value) -> Option<i64> {
    let raw = response
        .get("code")
        .or_else(|| response.get("errno"))
        .or_else(|| response.get("statusCode"))?;
    if let Some(n) = raw.as_i64() {
        return Some(n);
    }
    raw.as_str()?.trim().parse().ok()
}

fn message_of(response: &Value) -> String {
    response
        .get("message")
        .or_else(|| response.get("msg"))
        .or_else(|| response.get("error"))
        .and_then(Value::as_str)
        .map(|text| text.chars().take(200).collect())
        .unwrap_or_default()
}

/// 把一次响应翻成成功值或分类错误。
///
/// 成功判定：没有 `code` 字段，或 `code` 为 0 / "0" / 200。缺 `code` 也算成功，
/// 因为有的接口直接返回裸对象 —— 这不是宽容，是它本来就这样。
pub fn classify(response: &Value) -> Result<(), GrowthError> {
    if is_route_missing(response) {
        return Err(GrowthError::new(ErrorKind::RouteMissing, "路由不存在"));
    }
    let Some(code) = code_of(response) else {
        return Ok(());
    };
    if code == 0 || code == 200 {
        return Ok(());
    }
    let kind = match code {
        401 => ErrorKind::Unauthorized,
        403 => ErrorKind::Forbidden,
        429 => ErrorKind::RateLimited,
        -1 => ErrorKind::Network,
        500..=599 => ErrorKind::Server,
        _ => ErrorKind::Business,
    };
    let message = message_of(response);
    let message = if message.is_empty() {
        format!("成长中心返回 code {code}")
    } else {
        message
    };
    Err(GrowthError {
        kind,
        http: Some(code),
        code: Some(code),
        message: sanitize(&message),
    })
}

/// 构造请求头。复用官方对齐的那套，只补一个 UA 与来源页。
///
/// `payload` 只放 access / uid / domain，**不放 refresh token** ——
/// 借用来的授权不刷新，这是 PR #66 定下的边界。
pub fn headers(payload: &Value) -> HashMap<String, String> {
    let mut headers = wb_switch_core::modules::account::build_auth_headers(payload);
    headers.insert("User-Agent".to_string(), "WorkBuddy".to_string());
    headers.insert("x-client-platform".to_string(), "web".to_string());
    headers.insert("origin".to_string(), WORKBUDDY_API_ENDPOINT.to_string());
    headers.insert(
        "referer".to_string(),
        format!("{WORKBUDDY_API_ENDPOINT}/profile/growth-center"),
    );
    headers
}

/// 试一条路径。返回响应原文（成功与业务错误都返回，由调用方分类）。
///
/// 超时被单独识别成 `Network`：调用方必须据此区分"请求可能已经在服务端生效"与
/// "请求根本没发出去"。写操作（accept / claim）尤其依赖这个区分。
async fn send_once(
    payload: &Value,
    prefix: &str,
    suffix: &str,
    method: &str,
    body: Option<Value>,
) -> Result<Value, GrowthError> {
    let url = format!("{WORKBUDDY_API_ENDPOINT}{prefix}{suffix}");
    let headers = headers(payload);
    match tokio::time::timeout(
        REQUEST_TIMEOUT,
        http_request(&url, method, body, Some(&headers)),
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(_) => Err(GrowthError {
            kind: ErrorKind::Network,
            http: None,
            code: None,
            message: format!("请求超时（{}s）", REQUEST_TIMEOUT.as_secs()),
        }),
    }
}

/// 令牌为空的请求一律拒绝：发出去就是一个空 `Bearer`，服务端回 401，
/// 然后可能被误判成"授权失效"去换源 —— 一次构造错误变成一次状态污染。
fn guard_token(payload: &Value) -> Result<(), GrowthError> {
    let empty = payload
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .is_empty();
    if empty {
        return Err(GrowthError::new(
            ErrorKind::Unauthorized,
            "没有可用的访问令牌",
        ));
    }
    Ok(())
}

/// 在两种路径形态之间退让的通用发送器。
///
/// **只对 `RouteMissing` 退让**。写操作也走这里：404 意味着这条路径什么都没发生，
/// 换一条再试是安全的；而 401 / 403 / 429 / 5xx 是真实答复，重发只会重复提交。
async fn send(
    payload: &Value,
    suffix: &str,
    method: &str,
    body: Option<Value>,
) -> Result<(Value, &'static str), GrowthError> {
    guard_token(payload)?;

    let mut last: Option<GrowthError> = None;
    for prefix in PREFIXES {
        let response = send_once(payload, prefix, suffix, method, body.clone()).await?;
        match classify(&response) {
            Ok(()) => return Ok((response, prefix)),
            Err(err) if err.kind == ErrorKind::RouteMissing => {
                last = Some(err);
                continue;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or_else(|| GrowthError::new(ErrorKind::RouteMissing, "路由不存在")))
}

/// GET 一个成长中心接口，自动在两种路径形态之间退让。
///
/// 返回 `(响应, 生效前缀)`。带上前缀是为了让扫描结果能记录"这个账号实际走哪条路"，
/// 线上出问题时不用再猜一遍。
pub async fn get(payload: &Value, suffix: &str) -> Result<(Value, &'static str), GrowthError> {
    send(payload, suffix, "GET", None).await
}

/// POST 一个成长中心写接口。
///
/// 会改服务端状态，所以调用方必须自己保证幂等：**不要在这里重试**。
/// 唯一的自动退让是 `RouteMissing`（404 表示那条路径什么都没发生）。
pub async fn post(
    payload: &Value,
    suffix: &str,
    body: Value,
) -> Result<(Value, &'static str), GrowthError> {
    send(payload, suffix, "POST", Some(body)).await
}

// ---------------------------------------------------------------------------
// 具体接口（全部 GET）
// ---------------------------------------------------------------------------

pub struct ReadOnlyEndpoints {
    pub payload: Value,
}

impl ReadOnlyEndpoints {
    pub fn new(access: &str, uid: &str, domain: &str) -> Self {
        Self {
            payload: json!({
                "access_token": access,
                "uid": uid,
                "domain": domain,
                "variant": "cn",
            }),
        }
    }

    pub async fn tasks(&self) -> Result<(Value, &'static str), GrowthError> {
        get(&self.payload, "/tasks").await
    }
    pub async fn profile(&self) -> Result<(Value, &'static str), GrowthError> {
        get(&self.payload, "/profile").await
    }
    pub async fn energy(&self) -> Result<(Value, &'static str), GrowthError> {
        get(&self.payload, "/energy").await
    }
    pub async fn streak(&self) -> Result<(Value, &'static str), GrowthError> {
        get(&self.payload, "/streak").await
    }
    pub async fn heatmap(&self) -> Result<(Value, &'static str), GrowthError> {
        get(&self.payload, "/heatmap").await
    }
    pub async fn redeem_summary(&self) -> Result<(Value, &'static str), GrowthError> {
        get(&self.payload, "/redeem/summary").await
    }
    pub async fn lottery_chances(&self) -> Result<(Value, &'static str), GrowthError> {
        get(&self.payload, "/lottery/chances").await
    }
    pub async fn buddy_quota(&self) -> Result<(Value, &'static str), GrowthError> {
        get(&self.payload, "/buddy/quota").await
    }
    /// 旅行状态由 `travel` 模块负责调度，这里只借它的只读接口填界面。
    pub async fn buddy_travel_status(&self) -> Result<(Value, &'static str), GrowthError> {
        get(&self.payload, "/buddy/travel/status").await
    }
}

// ---------------------------------------------------------------------------
// 写接口（只有接单与领奖）
// ---------------------------------------------------------------------------

/// task_code 会直接进 URL 路径。只放行字母数字与 `_ . -`，
/// 别的一律拒绝 —— 少一个编码依赖，也堵住路径注入。
fn safe_task_code(code: &str) -> Option<&str> {
    let trimmed = code.trim();
    if trimmed.is_empty() || trimmed.len() > 128 {
        return None;
    }
    trimmed
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
        .then_some(trimmed)
}

/// 一批最多 20 条：服务端上限，超了会被拒或拆开，我们按上限自己分批。
pub const ACCEPT_BATCH: usize = 20;

/// 会改服务端状态的接口。**这就是全部两个**：接单、领奖。
pub struct WriteEndpoints {
    pub payload: Value,
}

impl WriteEndpoints {
    pub fn new(access: &str, uid: &str, domain: &str) -> Self {
        Self {
            payload: json!({
                "access_token": access,
                "uid": uid,
                "domain": domain,
                "variant": "cn",
            }),
        }
    }

    /// 批量接单。请求体是**复数** `task_codes`（旧的单数形态服务端一律 400）。
    pub async fn accept(&self, codes: &[String]) -> Result<(Value, &'static str), GrowthError> {
        if codes.is_empty() {
            return Err(GrowthError::new(ErrorKind::Business, "没有待接单的任务"));
        }
        if codes.len() > ACCEPT_BATCH {
            return Err(GrowthError::new(
                ErrorKind::Business,
                format!("一次最多接 {ACCEPT_BATCH} 个任务"),
            ));
        }
        post(
            &self.payload,
            "/tasks/accept",
            json!({ "task_codes": codes }),
        )
        .await
    }

    /// 单个任务领奖。路径带 code，请求体是空对象。
    pub async fn claim(&self, code: &str) -> Result<(Value, &'static str), GrowthError> {
        let Some(code) = safe_task_code(code) else {
            return Err(GrowthError::new(
                ErrorKind::Business,
                "task_code 含有不允许的字符",
            ));
        };
        post(&self.payload, &format!("/tasks/{code}/claim"), json!({})).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_is_code_zero_or_no_code_at_all() {
        assert!(classify(&json!({"code": 0, "data": {}})).is_ok());
        assert!(classify(&json!({"code": "0"})).is_ok());
        assert!(classify(&json!({"tasks": []})).is_ok(), "裸对象也算成功");
        assert!(classify(&json!({"code": 200})).is_ok());
    }

    #[test]
    fn auth_errors_are_distinguished_from_transient_ones() {
        let unauthorized = classify(&json!({"code": 401, "message": "unauthorized"})).unwrap_err();
        assert_eq!(unauthorized.kind, ErrorKind::Unauthorized);
        assert!(!unauthorized.kind.retryable(), "401 重试没有意义");

        let forbidden = classify(&json!({"code": 403, "msg": "连登天数不足"})).unwrap_err();
        assert_eq!(forbidden.kind, ErrorKind::Forbidden);
        assert!(!forbidden.kind.retryable(), "业务拒绝不是授权失效");

        for code in [429, 500, 502, 503] {
            let err = classify(&json!({"code": code})).unwrap_err();
            assert!(err.kind.retryable(), "{code} 应该等下一轮");
        }
        assert_eq!(
            classify(&json!({"code": 429})).unwrap_err().kind,
            ErrorKind::RateLimited
        );
        assert_eq!(
            classify(&json!({"code": 502})).unwrap_err().kind,
            ErrorKind::Server
        );
    }

    #[test]
    fn network_failure_is_transient_not_unauthorized() {
        let err = classify(&json!({"code": -1, "message": "connection reset"})).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Network);
        assert!(err.kind.retryable());
        assert_ne!(
            err.kind,
            ErrorKind::Unauthorized,
            "网络抖动不能当成登录失效"
        );
    }

    #[test]
    fn route_missing_is_recognised_at_either_level() {
        assert_eq!(
            classify(&json!({"code": 404})).unwrap_err().kind,
            ErrorKind::RouteMissing
        );
        assert_eq!(
            classify(&json!({"data": {"code": 404}})).unwrap_err().kind,
            ErrorKind::RouteMissing
        );
    }

    #[test]
    fn business_error_keeps_the_message_but_no_secrets() {
        let err = classify(&json!({"code": 10086, "msg": "活动已结束"})).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Business);
        assert_eq!(err.message, "活动已结束");
        assert_eq!(err.code, Some(10086));
        assert_eq!(err.http, Some(10086));
    }

    /// 空令牌必须在发请求之前就被拒：真发出去就是一个 `Bearer ` 空串，
    /// 服务端回 401，然后我们可能误判成"授权失效"去换源 —— 一次构造错误
    /// 变成一次状态污染。
    #[tokio::test]
    async fn empty_token_is_refused_before_any_request() {
        let empty = json!({"access_token": "  ", "uid": "u"});
        let err = get(&empty, "/tasks").await.expect_err("空令牌必须被拒");
        assert_eq!(err.kind, ErrorKind::Unauthorized);
    }
}
