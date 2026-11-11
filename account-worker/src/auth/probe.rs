//! 授权有效性探测（M2）。
//!
//! 为什么不能直接用 `get_checkin_status`：它把失败压成 `ok:false` + 一句文案，
//! 而且 404 会回落到第二个路径 —— 连"这个状态码属于哪次请求"都说不清。
//! 用它驱动授权切换，会把 429、超时、网关拦截全都当成"授权失效"，把还能用的凭据标死。
//!
//! 这里只做一件事：发一次轻量鉴权请求，保留 HTTP 语义，分成三类：
//!
//! | 结果 | 含义 | 能否切源 |
//! | --- | --- | --- |
//! | `Valid` | 授权有效 | — |
//! | `Unauthorized` | **明确的**认证失败 | 是（唯一触发条件） |
//! | `Transient` | 429 / 5xx / 超时 / 403 无法归类 | 否，只记错误 |
//!
//! 403 必须依据 WorkBuddy 的真实响应语义分类：它既可能是令牌失效，也可能是网关指纹拦截
//! （业务码 `10085`）或活动未开放。判不出来时归 `Transient` —— 宁可这次不切源，
//! 也不能把一份还能用的授权标成失效。

use serde_json::Value;
use wb_switch_core::modules::account::build_auth_headers;
use wb_switch_core::modules::config::{http_request_raw, CHECKIN_API_PREFIX};
use wb_switch_core::modules::variant::WbVariant;

use crate::rpc::sanitize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Valid,
    Unauthorized,
    Transient,
}

impl Probe {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Valid => "valid",
            Self::Unauthorized => "unauthorized",
            Self::Transient => "transient",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProbeReport {
    pub outcome: Probe,
    pub http_status: Option<u16>,
    pub business_code: Option<i64>,
    pub detail: Option<String>,
}

impl ProbeReport {
    fn new(outcome: Probe, status: Option<u16>, code: Option<i64>, detail: Option<&str>) -> Self {
        Self {
            outcome,
            http_status: status,
            business_code: code,
            detail: detail.map(sanitize),
        }
    }
}

/// 发一次探测。`access` 只在这里短暂存在，不写库、不进日志。
pub async fn probe(variant: WbVariant, access: &str, uid: &str, domain: &str) -> ProbeReport {
    let account = serde_json::json!({
        "access_token": access,
        "uid": uid,
        "domain": domain,
        "variant": "cn",
    });
    let headers = build_auth_headers(&account);
    let paths = variant.billing_paths(&format!("{CHECKIN_API_PREFIX}/checkin-activity-status"));
    let Some(path) = paths.first() else {
        return ProbeReport::new(Probe::Transient, None, None, Some("没有可用的探测路径"));
    };
    let url = format!("{}{path}", variant.api_endpoint());

    // 只试第一个路径：探测要的是"这份令牌能不能用"，不是"路径对不对"。
    // 回落会把 404 之后的成功混进来，反而看不清授权状态。
    let (status, _headers, body) = http_request_raw(
        &url,
        "POST",
        Some(serde_json::json!({})),
        Some(&headers),
        None,
        true,
    )
    .await;

    classify(status, &body)
}

/// 纯函数：HTTP 状态 + 响应体 → 分类。抽出来是为了能离线覆盖每个分支。
pub fn classify(status: u16, body: &str) -> ProbeReport {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let code = parsed
        .as_ref()
        .and_then(|v| v.get("code"))
        .and_then(Value::as_i64);
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("message").or_else(|| v.get("msg")))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_lowercase();

    // 传输层失败：`http_request_raw` 用 0 表示没拿到响应。
    if status == 0 {
        return ProbeReport::new(Probe::Transient, None, code, Some("网络错误或超时"));
    }

    // 业务码有时比 HTTP 码更准（网关会回 200 + code=401）。
    if code == Some(401) {
        return ProbeReport::new(Probe::Unauthorized, Some(status), code, Some("业务码 401"));
    }
    if code == Some(10085) {
        // 网关指纹拦截：不是授权问题，重试或换网络才有用。
        return ProbeReport::new(
            Probe::Transient,
            Some(status),
            code,
            Some("网关拦截（10085）"),
        );
    }

    match status {
        200..=299 => {
            if matches!(code, Some(0) | Some(200)) || code.is_none() {
                ProbeReport::new(Probe::Valid, Some(status), code, None)
            } else {
                // 2xx 但业务码异常：语义不明，保守归暂时性。
                ProbeReport::new(
                    Probe::Transient,
                    Some(status),
                    code,
                    Some("2xx 但业务码异常"),
                )
            }
        }
        401 => ProbeReport::new(Probe::Unauthorized, Some(status), code, Some("HTTP 401")),
        403 => {
            // 403 不等于"令牌失效"。只有响应本身在说"未授权 / 登录失效"时才算。
            let auth_like = ["unauthorized", "forbidden token", "登录", "失效", "token"]
                .iter()
                .any(|needle| message.contains(needle));
            if auth_like {
                ProbeReport::new(
                    Probe::Unauthorized,
                    Some(status),
                    code,
                    Some("HTTP 403 + 鉴权文案"),
                )
            } else {
                ProbeReport::new(
                    Probe::Transient,
                    Some(status),
                    code,
                    Some("HTTP 403（语义不足以判定为认证失败）"),
                )
            }
        }
        429 => ProbeReport::new(Probe::Transient, Some(status), code, Some("限流 429")),
        500..=599 => ProbeReport::new(Probe::Transient, Some(status), code, Some("服务端 5xx")),
        _ => ProbeReport::new(
            Probe::Transient,
            Some(status),
            code,
            Some("其它状态码，保守归暂时性"),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn success_is_valid() {
        let report = classify(200, r#"{"code":0,"data":{}}"#);
        assert_eq!(report.outcome, Probe::Valid);
        assert_eq!(report.http_status, Some(200));
    }

    #[test]
    fn http_401_is_unauthorized() {
        assert_eq!(
            classify(401, r#"{"code":401,"message":"unauthorized"}"#).outcome,
            Probe::Unauthorized
        );
    }

    #[test]
    fn body_code_401_on_http_200_is_unauthorized() {
        assert_eq!(
            classify(200, r#"{"code":401,"message":"token 失效"}"#).outcome,
            Probe::Unauthorized
        );
    }

    #[test]
    fn gateway_403_is_transient_not_auth_failure() {
        let report = classify(403, r#"{"code":10085,"message":"fingerprint blocked"}"#);
        assert_eq!(report.outcome, Probe::Transient, "{report:?}");
    }

    #[test]
    fn auth_like_403_is_unauthorized() {
        assert_eq!(
            classify(403, r#"{"code":403,"message":"登录状态已失效"}"#).outcome,
            Probe::Unauthorized
        );
    }

    #[test]
    fn unclassifiable_403_is_transient() {
        // 活动未开放这类 403 不该把授权标死。
        assert_eq!(
            classify(403, r#"{"code":403,"message":"activity not open"}"#).outcome,
            Probe::Transient
        );
    }

    #[test]
    fn rate_limit_and_server_errors_are_transient() {
        assert_eq!(classify(429, "{}").outcome, Probe::Transient);
        assert_eq!(
            classify(502, "<html>bad gateway</html>").outcome,
            Probe::Transient
        );
        assert_eq!(classify(0, "").outcome, Probe::Transient);
    }

    #[test]
    fn transport_failure_has_no_status() {
        let report = classify(0, "");
        assert_eq!(report.http_status, None, "传输失败不该伪造状态码");
    }
}
