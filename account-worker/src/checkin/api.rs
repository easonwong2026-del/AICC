//! 签到接口层。
//!
//! 三条路径，全部在 `www.codebuddy.cn` 上（与成长中心同域，`/v2` 前缀）：
//!
//! | 用途 | 路径 | 方法 |
//! | --- | --- | --- |
//! | 查今日状态（首个候选） | `/v2/billing/meter/checkin-activity-status` | POST `{}` |
//! | 查今日状态（旧候选） | `/v2/billing/meter/checkin-status` | POST `{}` |
//! | 执行签到 | `/v2/billing/meter/daily-checkin` | POST `{}` |
//!
//! 两条状态路径都是 POST，但**只读语义**：服务端不因它改签到状态。这是上游
//! `wb-switch-core` 与 `changexbc/workbuddy-switch` 一致的做法，AICC 沿用。
//! 证据等级见 `docs/WORKBUDDY_CHECKIN_API.md`。
//!
//! 三个不可妥协的判定：
//!
//! 1. **缺 `today_checked_in` 就是 `unknown`**，不是 `false`。`wb-switch-core` 的
//!    `status_from_response` 用 `.unwrap_or(false)` 兜底 —— 那会让"接口改结构"
//!    变成"每天重复签到一次"。这里返回 `Option<bool>`，`None` 一律不下发写请求。
//! 2. **只在 404 时换路径**。401 / 403 / 429 / 5xx / 超时都是服务端的真实回答，
//!    换个路径重问只会把错误藏起来。
//! 3. **HTTP 200 不等于签到成功**。写请求的成功只看 `code` 是否 0/200，而且
//!    调用方必须再查一次状态才算 `claimed`。

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::rpc::sanitize;
use wb_switch_core::modules::account::build_auth_headers;
use wb_switch_core::modules::config::{http_request, is_route_missing, WORKBUDDY_API_ENDPOINT};

/// 单个请求上限。签到是交互式操作，卡 15 秒就该给用户一个结论。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 状态查询的两条候选，按优先级排列。只在 404 时退让。
pub const STATUS_PATHS: [&str; 2] = [
    "/v2/billing/meter/checkin-activity-status",
    "/v2/billing/meter/checkin-status",
];

pub const CHECKIN_PATH: &str = "/v2/billing/meter/daily-checkin";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    /// 明确 401：这份授权失效了，允许换源。
    Unauthorized,
    /// 403：可能是网关拦截（10085）也可能是业务拒绝，**不当作授权失效**。
    Forbidden,
    /// 429：被限流。这一轮不再重试。
    RateLimited,
    /// 5xx / 网关错误。
    Server,
    /// 网络错误或超时。**写请求超时后必须回查，不能直接重发**。
    Network,
    /// 404：只有它允许换路径。
    RouteMissing,
    /// 接口成功但**答不出今日状态**（缺 `today_checked_in`）。
    ///
    /// 单独一个类型而不是塞进 `Business`：这类情况下正确的动作是"什么都别写，
    /// 等下一轮重查"，与"业务明确拒绝"完全不同。
    UnknownStatus,
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
            Self::UnknownStatus => "unknown_status",
            Self::Business => "business",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckinError {
    pub kind: ErrorKind,
    pub http: Option<i64>,
    pub code: Option<i64>,
    pub message: String,
}

impl CheckinError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            http: None,
            code: None,
            message: sanitize(&message.into()),
        }
    }
}

/// 服务端今日状态。
///
/// `checked_in` 是 `Option` 而不是 `bool` —— 见模块注释第 1 条。
#[derive(Debug, Clone)]
pub struct StatusReply {
    pub checked_in: Option<bool>,
    pub streak_days: Option<i64>,
    pub daily_credit: Option<i64>,
    pub today_credit: Option<i64>,
    /// 实到的那条路径。线上出问题时不用再猜。
    pub path: &'static str,
}

/// 签到写请求的回应。
#[derive(Debug, Clone)]
pub struct SubmitReply {
    /// 幂等回执：服务端说"今天已经签过了"。既不是失败，也不能算新增积分。
    pub already: bool,
    /// 服务端明确给出的奖励。**没有就 None**，不猜 100。
    pub credits: Option<i64>,
}

/// 传输层抽象。唯一的存在理由：让签到流程能在 Mock 上跑测试。
///
/// CI 里绝不允许对真实账号发 `daily-checkin`，"提交成功但回查没确认"这种分支
/// 又只有注入才能覆盖 —— 所以这层不是为好看。
#[allow(async_fn_in_trait)]
pub trait Transport {
    async fn status(&self) -> Result<StatusReply, CheckinError>;
    async fn submit(&self) -> Result<SubmitReply, CheckinError>;
}

/// 线上实现。
pub struct LiveTransport {
    payload: Value,
}

impl LiveTransport {
    /// `payload` 只放 access / uid / domain，**不放 refresh token**（PRD §9.5）。
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
}

impl Transport for LiveTransport {
    async fn status(&self) -> Result<StatusReply, CheckinError> {
        query_status(&self.payload).await
    }

    async fn submit(&self) -> Result<SubmitReply, CheckinError> {
        submit_checkin(&self.payload).await
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

/// 把一次响应翻成成功或分类错误。
///
/// 成功判定：没有 `code` 字段，或 `code` 为 0 / 200。缺 `code` 也算成功，
/// 因为有的网关直接返回裸对象 —— 这不是宽容，是它本来就这样。
pub fn classify(response: &Value) -> Result<(), CheckinError> {
    if is_route_missing(response) {
        return Err(CheckinError::new(ErrorKind::RouteMissing, "路由不存在"));
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
        404 => ErrorKind::RouteMissing,
        429 => ErrorKind::RateLimited,
        -1 => ErrorKind::Network,
        500..=599 => ErrorKind::Server,
        _ => ErrorKind::Business,
    };
    let message = message_of(response);
    let message = if message.is_empty() {
        format!("签到接口返回 code {code}")
    } else {
        message
    };
    Err(CheckinError {
        kind,
        http: Some(code),
        code: Some(code),
        message: sanitize(&message),
    })
}

fn guard_token(payload: &Value) -> Result<(), CheckinError> {
    let empty = payload
        .get("access_token")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or("")
        .is_empty();
    if empty {
        // 发出去就是一个空 Bearer，服务端 401，然后可能被误判成"授权失效"去换源。
        return Err(CheckinError::new(
            ErrorKind::Unauthorized,
            "没有可用的访问令牌",
        ));
    }
    Ok(())
}

async fn send_once(payload: &Value, path: &str) -> Result<Value, CheckinError> {
    guard_token(payload)?;
    let url = format!("{WORKBUDDY_API_ENDPOINT}{path}");
    let headers: HashMap<String, String> = build_auth_headers(payload);
    match tokio::time::timeout(
        REQUEST_TIMEOUT,
        http_request(&url, "POST", Some(json!({})), Some(&headers)),
    )
    .await
    {
        Ok(response) => Ok(response),
        Err(_) => Err(CheckinError {
            kind: ErrorKind::Network,
            http: None,
            code: None,
            message: format!("请求超时（{}s）", REQUEST_TIMEOUT.as_secs()),
        }),
    }
}

/// 从状态响应里取今日状态。
///
/// **只认 `data.today_checked_in` / `data.todayCheckedIn` 两个键**。取不到就是
/// `None`，绝不默认 `false`。
pub fn status_from_response(response: &Value, path: &'static str) -> StatusReply {
    let data = response.get("data").cloned().unwrap_or_else(|| json!({}));
    let checked = data
        .get("today_checked_in")
        .or_else(|| data.get("todayCheckedIn"))
        .and_then(Value::as_bool);
    StatusReply {
        checked_in: checked,
        streak_days: int_of(&data, &["streak_days", "streakDays"]),
        daily_credit: int_of(&data, &["daily_credit", "dailyCredit"]),
        today_credit: int_of(&data, &["today_credit", "todayCredit"]),
        path,
    }
}

fn int_of(value: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        match value.get(*key) {
            Some(v) => {
                if let Some(n) = v.as_i64() {
                    return Some(n);
                }
                if let Some(text) = v.as_str() {
                    if let Ok(n) = text.trim().parse::<i64>() {
                        return Some(n);
                    }
                }
            }
            None => continue,
        }
    }
    None
}

/// 查一次今日状态：先 activity-status，只在 404 时退旧路径。
async fn query_status(payload: &Value) -> Result<StatusReply, CheckinError> {
    let mut last: Option<CheckinError> = None;
    for path in STATUS_PATHS {
        let response = send_once(payload, path).await?;
        match classify(&response) {
            Ok(()) => {
                let reply = status_from_response(&response, path);
                if reply.checked_in.is_none() {
                    // 成功但没有布尔值：结构变了。这不是"没签到"，是"不知道"。
                    return Err(CheckinError {
                        kind: ErrorKind::UnknownStatus,
                        http: Some(200),
                        code: code_of(&response),
                        message: "签到状态响应里没有 today_checked_in，状态未知".to_string(),
                    });
                }
                return Ok(reply);
            }
            Err(err) if err.kind == ErrorKind::RouteMissing => {
                last = Some(err);
                continue;
            }
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or_else(|| CheckinError::new(ErrorKind::RouteMissing, "路由不存在")))
}

/// 提交签到。**不重试**，除"404 换路径"外没有任何自动动作。
///
/// 幂等回执的处理：服务端对重复签到可能回非 0 业务码 + "已签到"文案，
/// 那是**成功语义**（今天确实签过了），必须记成 `already` 而不是 `failed`。
pub async fn submit_checkin(payload: &Value) -> Result<SubmitReply, CheckinError> {
    let response = send_once(payload, CHECKIN_PATH).await?;
    if let Err(err) = classify(&response) {
        if err.kind == ErrorKind::Business && looks_already_reported(&response) {
            return Ok(SubmitReply {
                already: true,
                credits: None,
            });
        }
        return Err(err);
    }
    let data = response.get("data").cloned().unwrap_or_else(|| json!({}));
    let already = data
        .get("already_checked_in")
        .or_else(|| data.get("alreadyCheckedIn"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || looks_already_reported(&response);
    let credits = int_of(
        &data,
        &[
            "credit",
            "credits",
            "today_credit",
            "todayCredit",
            "reward_credit",
        ],
    );
    Ok(SubmitReply {
        // 服务端说"已经签过了"，奖励就不是这次拿到的。
        credits: if already { None } else { credits },
        already,
    })
}

fn looks_already_reported(response: &Value) -> bool {
    let msg = message_of(response).to_lowercase();
    ["已签到", "重复", "repeat", "already"]
        .iter()
        .any(|keyword| msg.contains(keyword))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_boolean_is_unknown_not_false() {
        let reply = status_from_response(&json!({"code": 0, "data": {"streak_days": 12}}), "p");
        assert_eq!(reply.checked_in, None, "缺字段必须是未知");
    }

    #[test]
    fn both_spellings_are_accepted() {
        let snake = status_from_response(&json!({"data": {"today_checked_in": true}}), "p");
        assert_eq!(snake.checked_in, Some(true));
        let camel = status_from_response(&json!({"data": {"todayCheckedIn": false}}), "p");
        assert_eq!(camel.checked_in, Some(false));
    }

    #[test]
    fn string_numbers_are_read_as_numbers() {
        let reply = status_from_response(
            &json!({"data": {"today_checked_in": true, "streak_days": "42", "daily_credit": 100}}),
            "p",
        );
        assert_eq!(reply.streak_days, Some(42));
        assert_eq!(reply.daily_credit, Some(100));
    }

    #[test]
    fn http_codes_are_classified_without_guessing() {
        assert_eq!(
            classify(&json!({"code": 401})).unwrap_err().kind,
            ErrorKind::Unauthorized
        );
        assert_eq!(
            classify(&json!({"code": 403})).unwrap_err().kind,
            ErrorKind::Forbidden
        );
        assert_eq!(
            classify(&json!({"code": 429})).unwrap_err().kind,
            ErrorKind::RateLimited
        );
        assert_eq!(
            classify(&json!({"code": 502})).unwrap_err().kind,
            ErrorKind::Server
        );
        assert_eq!(
            classify(&json!({"code": -1})).unwrap_err().kind,
            ErrorKind::Network
        );
        assert_eq!(
            classify(&json!({"code": 404})).unwrap_err().kind,
            ErrorKind::RouteMissing
        );
        assert_eq!(
            classify(&json!({"code": 10085})).unwrap_err().kind,
            ErrorKind::Business
        );
        assert!(classify(&json!({"code": 0})).is_ok());
        assert!(classify(&json!({"code": 200})).is_ok());
    }

    #[test]
    fn a_403_is_never_read_as_an_expired_token() {
        let err = classify(&json!({"code": 403, "message": "blocked by gateway"})).unwrap_err();
        assert_ne!(err.kind, ErrorKind::Unauthorized);
        assert!(!err.kind.as_str().is_empty());
    }

    #[test]
    fn credits_are_only_taken_when_the_server_states_them() {
        let reply = SubmitReply {
            already: false,
            credits: int_of(&json!({"credit": 100}), &["credit", "credits"]),
        };
        assert_eq!(reply.credits, Some(100));
        assert_eq!(int_of(&json!({}), &["credit"]), None, "没有就是 None，不猜");
    }

    #[test]
    fn an_empty_token_never_reaches_the_network() {
        let err = guard_token(&json!({"access_token": "  "})).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Unauthorized);
        assert!(guard_token(&json!({"access_token": "abc"})).is_ok());
    }
}
