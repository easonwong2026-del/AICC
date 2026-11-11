//! 旅行接口的传输层。
//!
//! 真正的请求封装全部复用上游 `wb_switch_core::modules::travel`
//! （见 `vendor/VENDOR.md` 补丁 2），这里只做两件事：
//!
//! 1. 给每次调用加**硬超时**（上游 client 是 30 秒，调度器一轮 4 个账号会拖很久）；
//! 2. 提供一个可以注入的 `Transport`，让状态机能在 Mock 下跑真实分支。
//!
//! 注意：payload 只有 `access_token` / `uid` / `domain` / `variant`。
//! 上游 `travel_request` 只在 payload 里出现非空 `refresh_token` 时才尝试刷新，
//! 我们不构造这个字段，所以「绝不调用 refresh」是构造上的保证，而不是靠约定。

use std::time::Duration;

use serde_json::{json, Value};

use crate::rpc::sanitize;

/// 单次请求上限。超过就当网络超时处理，走退避，不重试。
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[allow(async_fn_in_trait)]
pub trait Transport {
    async fn config(&self, payload: &Value) -> Value;
    async fn status(&self, payload: &Value) -> Value;
    async fn depart(&self, payload: &Value, location_id: &Value) -> Value;
    async fn claim(&self, payload: &Value, record_id: i64) -> Value;
}

/// 线上实现：转调上游封装 + 超时。
pub struct LiveTransport;

fn timeout_value() -> Value {
    json!({"ok": false, "code": -1, "error": "请求超时（20s）"})
}

impl Transport for LiveTransport {
    async fn config(&self, payload: &Value) -> Value {
        let fut = wb_switch_core::modules::travel::fetch_travel_config(payload);
        match tokio::time::timeout(REQUEST_TIMEOUT, fut).await {
            Ok(value) => value,
            Err(_) => timeout_value(),
        }
    }

    async fn status(&self, payload: &Value) -> Value {
        let fut = wb_switch_core::modules::travel::fetch_travel_status(payload);
        match tokio::time::timeout(REQUEST_TIMEOUT, fut).await {
            Ok(value) => value,
            Err(_) => timeout_value(),
        }
    }

    async fn depart(&self, payload: &Value, location_id: &Value) -> Value {
        let fut = wb_switch_core::modules::travel::depart_travel(payload, location_id);
        match tokio::time::timeout(REQUEST_TIMEOUT, fut).await {
            Ok(value) => value,
            Err(_) => timeout_value(),
        }
    }

    async fn claim(&self, payload: &Value, record_id: i64) -> Value {
        let fut = wb_switch_core::modules::travel::claim_travel(payload, record_id);
        match tokio::time::timeout(REQUEST_TIMEOUT, fut).await {
            Ok(value) => value,
            Err(_) => timeout_value(),
        }
    }
}

/// 构造请求体。
///
/// **刻意不包含 `refresh_token`**：`payload_never_carries_refresh_token` 用例盯着这一点。
pub fn build_payload(uid: &str, domain: &str, access: &str) -> Value {
    json!({
        "access_token": access,
        "uid": uid,
        "domain": domain,
        "variant": "cn",
    })
}

/// 请求层的错误文案统一脱敏后再落库 / 上报。
pub fn clean_message(raw: &str) -> String {
    sanitize(raw).chars().take(200).collect()
}

/// 解析 `/config` 的地点列表。返回 `(id, name)`，顺序即官方给的顺序。
pub fn parse_locations(resp: &Value) -> Result<Vec<(Value, String)>, String> {
    let ok = resp.get("ok").and_then(Value::as_bool) == Some(true);
    if !ok {
        return Err(clean_message(&crate::travel::state::error_text(resp)));
    }
    let enabled = resp.get("enabled").and_then(Value::as_bool).unwrap_or(true);
    let locations: Vec<(Value, String)> = resp
        .get("locations")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| {
                    let id = item.get("id")?.clone();
                    if id.is_null() {
                        return None;
                    }
                    let name = item
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    Some((id, name))
                })
                .collect()
        })
        .unwrap_or_default();
    if !enabled || locations.is_empty() {
        return Err("没有可用的旅行地点".to_string());
    }
    Ok(locations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_never_carries_refresh_token() {
        let payload = build_payload("uid-1", "www.workbuddy.cn", "access-xyz");
        let text = payload.to_string();
        assert!(payload.get("refresh_token").is_none());
        assert!(!text.contains("refresh"), "请求体里不得出现 refresh 字段");
        assert_eq!(payload["access_token"], "access-xyz");
        assert_eq!(payload["variant"], "cn");
    }

    #[test]
    fn parses_locations_and_rejects_empty() {
        let resp = json!({"ok":true,"enabled":true,"locations":[
            {"id":1,"name":"咖啡馆"},{"id":2,"name":"健身房"}]});
        let locations = parse_locations(&resp).expect("ok");
        assert_eq!(locations.len(), 2);
        assert_eq!(locations[1].1, "健身房");

        let empty = json!({"ok":true,"enabled":true,"locations":[]});
        assert!(parse_locations(&empty).is_err());
        let failed = json!({"ok":false,"error":"boom"});
        assert!(parse_locations(&failed).is_err());
    }
}
