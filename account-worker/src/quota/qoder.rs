//! Qoder 两版在额度看板上的行。
//!
//! ## 为什么这里只有"奖励"，没有"余额"
//!
//! WorkBuddy 的看板行有 `total / used / remaining`（资源额度）。Qoder 这边
//! **没有任何已证实的余额接口**：能拿到的是"每天的活动里有没有可领的 Credits"。
//! 把每日奖励塞进 `remaining` 会让用户以为账户里只剩 100，这是最坏的误导。
//!
//! 所以 Qoder 行的形状是：
//!
//! | 字段 | 含义 |
//! | --- | --- |
//! | `unit = "credits"` | 计量单位，与 WorkBuddy 的 credits/tokens **不相加** |
//! | `total / used / remaining = null` | 余额未知，**不是 0** |
//! | `credits` | 当日活动的真实状态（可领 / 已领 / 无活动 / 未验证） |
//!
//! ## 三种"没有数"，三种说法
//!
//! 1. `unverified`：这个版本没有适配器（International），或本机没有账号级凭据
//!    ——**零请求**；
//! 2. `unauthorized`：请求发了，401/403 被拒；
//! 3. `error`：网络 / 5xx / 结构不认识。
//!
//! 把它们混成一句"暂不可用"会让用户永远不知道该去重新授权还是等一会儿。

use serde_json::{json, Value};

/// Qoder 账号的标识方式：`qoder_cn` / `qoder_global`。
pub const PROVIDERS: &[&str] = &["qoder_cn", "qoder_global"];

/// 未验证的能力行。`status = unverified`、`quota = null`，并附上阻塞原因。
pub fn unverified_row(
    account_id: &str,
    provider: &str,
    nickname: &str,
    source: &str,
    detail: &str,
) -> Value {
    json!({
        "accountId": account_id,
        "provider": provider,
        "nickname": nickname,
        "source": source,
        "status": "unverified",
        "unit": Value::Null,
        "total": Value::Null,
        "used": Value::Null,
        "remaining": Value::Null,
        "plan": Value::Null,
        "isPaid": Value::Null,
        "usage": Value::Null,
        "syncAt": crate::clock::now_rfc3339(),
        "error": detail,
        // Qoder 没有"余额"这个概念，`credits` 明确为 null 而不是空对象。
        "credits": Value::Null,
        "cached": false,
        "stale": false,
    })
}

/// 把 `credits::status` 的结论折成看板的一行。
///
/// `status` 与 `credits.state` 是两件事：行状态回答"这一格能不能信"，
/// `credits.state` 回答"今天的活动是什么情况"。
pub fn row_from_status(
    account_id: &str,
    provider: &str,
    nickname: &str,
    source: &str,
    status: &Value,
) -> Value {
    let state = status
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("error");
    let (row_status, reason) = match state {
        "claimable" | "already" | "nothing" => ("ok", "ok"),
        "unsupported" => ("unverified", "unsupported"),
        "no_credential" => ("unverified", "no_credential"),
        "unauthorized" => ("unauthorized", "unauthorized"),
        "error" => ("error", "error"),
        other => ("error", other),
    };
    let error = status
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(crate::rpc::sanitize)
        .unwrap_or_default();
    // 同步时间用服务端这次查询的时间；没有就退到"现在"。
    let sync_at = status
        .get("checkedAt")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let sync_at = if sync_at.is_empty() {
        crate::clock::now_rfc3339()
    } else {
        sync_at
    };

    json!({
        "accountId": account_id,
        "provider": provider,
        "nickname": nickname,
        "source": source,
        "status": row_status,
        "reason": reason,
        // 余额未知：null，不是 0。
        "unit": json!("credits"),
        "total": Value::Null,
        "used": Value::Null,
        "remaining": Value::Null,
        "plan": Value::Null,
        "isPaid": Value::Null,
        "usage": Value::Null,
        "credits": {
            "state": state,
            "claimable": status.get("claimable").cloned().unwrap_or(Value::Null),
            "claimed": status.get("claimed").cloned().unwrap_or(Value::Null),
            "pending": status.get("pending").cloned().unwrap_or(Value::Null),
            "today": status.get("today").cloned().unwrap_or(Value::Null),
            "supported": status.get("supported").cloned().unwrap_or(json!(false)),
            "autoEnabled": status.get("autoEnabled").cloned().unwrap_or(Value::Null),
        },
        "syncAt": sync_at,
        "error": if error.is_empty() || row_status == "ok" {
            Value::Null
        } else {
            json!(error)
        },
        "cached": false,
        "stale": false,
    })
}

/// 从缓存行构造（`refresh=false` 且快照够新时走这条）。
pub fn cached_row(payload: &Value, captured_at: &str, stale: bool) -> Option<Value> {
    if !payload.is_object() {
        return None;
    }
    let mut row = payload.clone();
    row["cached"] = json!(true);
    row["stale"] = json!(stale);
    row["capturedAt"] = json!(captured_at);
    Some(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(state: &str) -> Value {
        json!({
            "state": state,
            "claimable": 1,
            "claimed": 0,
            "pending": 0,
            "supported": true,
            "today": {"campaignId": "c1", "credits": 100},
            "checkedAt": "2026-11-14T10:00:00Z",
            "error": {"kind": "x", "message": "原因"},
        })
    }

    #[test]
    fn only_the_two_qoder_editions_are_known() {
        assert!(PROVIDERS.contains(&"qoder_cn"));
        assert!(PROVIDERS.contains(&"qoder_global"));
        assert!(!PROVIDERS.contains(&"workbuddy"));
        assert!(!PROVIDERS.contains(&"qoder"));
    }

    #[test]
    fn unverified_row_never_reports_zero_quota() {
        let row = unverified_row("qa_test", "qoder_cn", "测试", "own_browser", "阻塞原因");
        assert_eq!(row["status"], "unverified");
        // 未知额度必须是 null，绝不能是 0：0 会被界面读成"用完了"。
        assert!(row["total"].is_null());
        assert!(row["remaining"].is_null());
        assert!(row["used"].is_null());
        assert!(row["credits"].is_null());
        assert_eq!(row["error"], "阻塞原因");
    }

    #[test]
    fn claimable_row_carries_the_reward_not_a_balance() {
        let row = row_from_status("qa_1", "qoder_cn", "号", "own_pat", &status("claimable"));
        assert_eq!(row["status"], "ok");
        assert_eq!(row["unit"], "credits");
        // 余额永不伪造。
        assert!(row["total"].is_null());
        assert!(row["remaining"].is_null());
        // 奖励是独立一块，界面不会把它当余额相加。
        assert_eq!(row["credits"]["claimable"], 1);
        assert_eq!(row["credits"]["today"]["credits"], 100);
        assert!(row["error"].is_null());
    }

    #[test]
    fn unsupported_and_missing_credential_are_unverified_not_errors() {
        for state in ["unsupported", "no_credential"] {
            let row = row_from_status("qa_1", "qoder_global", "号", "unknown", &status(state));
            assert_eq!(row["status"], "unverified", "{state}");
            assert_eq!(row["reason"], state);
            assert_eq!(row["error"], "原因");
        }
    }

    #[test]
    fn unauthorized_is_its_own_status() {
        let row = row_from_status(
            "qa_1",
            "qoder_cn",
            "号",
            "own_browser",
            &status("unauthorized"),
        );
        assert_eq!(row["status"], "unauthorized");
    }

    #[test]
    fn cached_row_keeps_the_snapshot_time() {
        let payload = row_from_status("qa_1", "qoder_cn", "号", "own_pat", &status("already"));
        let row = cached_row(&payload, "2026-11-14T09:00:00Z", true).unwrap();
        assert_eq!(row["cached"], true);
        assert_eq!(row["stale"], true);
        assert_eq!(row["capturedAt"], "2026-11-14T09:00:00Z");
        assert!(cached_row(&Value::Null, "x", false).is_none());
    }
}
