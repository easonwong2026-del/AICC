//! 接单与领奖：把服务端返回翻成"到底发生了什么"。
//!
//! 这一层只做判定，不发请求 —— 所以整段逻辑都能在 Mock 数据上跑测试。
//!
//! 三条硬规则，全部来自 2026-11-12 的实测（见 `docs/growth-center-2026-11-12.md`）：
//!
//! 1. **HTTP 200 不等于成功**。accept 对不存在的任务也返回 `200 / code 0`，
//!    真正的结果在 `data.results[]` 里逐条给 `status: "ok" | "error"`。
//! 2. **`already_claimed: true` 不是失败**。它是幂等回执，此时 `credit/energy` 都是 0。
//!    把它当失败会让每一轮都报警，当成功又会重复计积分 —— 只能单独记一类。
//! 3. **拿不到 `data` 就不许猜**。缺字段时判 `Unknown`，交给调用方回查，
//!    绝不退化成"应该成功了"。

use serde::Serialize;
use serde_json::Value;

use super::models::{GrowthTask, TaskStatus};

// ---------------------------------------------------------------------------
// 决策
// ---------------------------------------------------------------------------

/// 对一个任务该做什么。判定只看 `locked` 与 `accept_status`。
///
/// **不看** `catalog` 的分类：分类是给人看的（自动 / 真实操作 / 人工），
/// 决定"能不能自动动它"的只有服务端状态。一个标着"人工"的任务，只要状态是
/// `not_accepted` 且没锁定，AICC 就该替用户把名报上 —— 报名不是替用户完成任务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Accept,
    Claim,
    /// 等它自然发生：已接受、进行中。
    Wait,
    /// 明确不动，附原因（用于界面与执行记录）。
    Skip(&'static str),
}

pub fn decide(task: &GrowthTask) -> Action {
    if task.locked {
        return Action::Skip("locked");
    }
    match task.status {
        TaskStatus::NotAccepted => Action::Accept,
        TaskStatus::Accepted | TaskStatus::InProgress => Action::Wait,
        TaskStatus::Completed => Action::Claim,
        TaskStatus::Claimed => Action::Skip("claimed"),
        // 没见过的状态一律不动。这条最容易被"优化"掉，也最容易出事：
        // 未来服务端加一个 `expired`，猜成 NotAccepted 就会去接一个已过期的任务。
        TaskStatus::Unknown | TaskStatus::Locked => Action::Skip("unknown_status"),
    }
}

// ---------------------------------------------------------------------------
// 接单结果
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AcceptOutcome {
    pub accepted: Vec<String>,
    /// `(task_code, message)`。服务端逐条给原因，原样保留。
    pub rejected: Vec<(String, String)>,
    /// 响应里根本没有 `results` 数组：不知道成没成，交给调用方回查。
    pub unknown: bool,
}

impl AcceptOutcome {}

/// `results` 可能在 `data.results` 或顶层 `results`（两种都见过）。
fn locate_results(response: &Value) -> Option<&Vec<Value>> {
    if let Some(list) = response.get("results").and_then(Value::as_array) {
        return Some(list);
    }
    response
        .get("data")
        .and_then(|data| data.get("results"))
        .and_then(Value::as_array)
}

/// 解析接单响应。
///
/// `requested` 是这次发出去的任务码，用来在响应残缺时判断"哪些没被提到"——
/// 没被提到的**不算成功**，也不硬算失败，直接归入 rejected 并写明"响应里没有这条"。
pub fn parse_accept(response: &Value, requested: &[String]) -> AcceptOutcome {
    let Some(results) = locate_results(response) else {
        return AcceptOutcome {
            accepted: Vec::new(),
            rejected: Vec::new(),
            unknown: true,
        };
    };

    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for item in results {
        let Some(code) = item.get("task_code").and_then(Value::as_str) else {
            continue;
        };
        let status = item.get("status").and_then(Value::as_str).unwrap_or("");
        if status == "error" {
            let message = item
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("未说明原因");
            rejected.push((code.to_string(), message.to_string()));
        } else {
            // 服务端只会给 ok / error；别的值当成功是赌，当失败是误报。
            // 这里按"成功"处理但保留原始 status 的依据：`results` 里出现的条目
            // 本身就是服务端对这条任务的答复，非 error 即已受理。
            accepted.push(code.to_string());
        }
    }

    for code in requested {
        if !results
            .iter()
            .any(|item| item.get("task_code").and_then(Value::as_str) == Some(code.as_str()))
        {
            rejected.push((code.clone(), "响应中没有这条任务的结果".to_string()));
        }
    }

    AcceptOutcome {
        accepted,
        rejected,
        unknown: false,
    }
}

// ---------------------------------------------------------------------------
// 领奖结果
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ClaimOutcome {
    /// 真发了奖励。`credit` / `energy` 是服务端实发值，可能和列表里的配置不同。
    Applied { credit: i64, energy: i64 },
    /// 之前已经领过。幂等回执，不是错误。
    AlreadyClaimed,
    /// 服务端明确拒绝（例如 `task not completed`）。
    Rejected { message: String },
    /// 响应缺关键字段：不知道发没发。调用方必须回查任务状态，不许重发。
    Unknown { message: String },
}

fn int_of(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|n| n as i64))
        .or_else(|| value.as_str()?.trim().parse().ok())
}

/// 解析领奖响应。
///
/// 真实成功响应（2026-11-12 实测）：
/// ```json
/// {"code":0,"msg":"OK","data":{"already_claimed":false,"credit":100,"energy":5}}
/// ```
/// 重复领取：`{"code":0,"data":{"already_claimed":true,"credit":0,"energy":0}}`
pub fn parse_claim(response: &Value) -> ClaimOutcome {
    let code = response
        .get("code")
        .and_then(|v| int_of(Some(v)))
        .unwrap_or(0);
    if code != 0 && code != 200 {
        let message = response
            .get("msg")
            .or_else(|| response.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("领奖被拒绝")
            .to_string();
        return ClaimOutcome::Rejected { message };
    }

    let data = match response.get("data") {
        Some(data) if data.is_object() => data,
        // 有过响应把结果摊在顶层的版本，兜一层。
        _ => response,
    };

    let already = data
        .get("already_claimed")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let credit = int_of(data.get("credit"));
    let energy = int_of(data.get("energy"));

    if already {
        return ClaimOutcome::AlreadyClaimed;
    }
    match (credit, energy) {
        (Some(credit), energy) => ClaimOutcome::Applied {
            credit,
            energy: energy.unwrap_or(0),
        },
        (None, Some(energy)) => ClaimOutcome::Applied { credit: 0, energy },
        (None, None) => ClaimOutcome::Unknown {
            message: "响应里既没有 already_claimed 也没有奖励数值".to_string(),
        },
    }
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn task(status: TaskStatus, locked: bool) -> GrowthTask {
        GrowthTask {
            code: "create_canvas".to_string(),
            title: "创建画布".to_string(),
            description: None,
            condition: None,
            task_type: None,
            tag: None,
            jump_url: None,
            locked,
            current: 0,
            target: 1,
            reward_credit: 100,
            reward_energy: 0,
            status,
            raw_status: None,
        }
    }

    #[test]
    fn decision_follows_server_status_only() {
        assert_eq!(
            decide(&task(TaskStatus::NotAccepted, false)),
            Action::Accept
        );
        assert_eq!(decide(&task(TaskStatus::Accepted, false)), Action::Wait);
        assert_eq!(decide(&task(TaskStatus::InProgress, false)), Action::Wait);
        assert_eq!(decide(&task(TaskStatus::Completed, false)), Action::Claim);
        assert_eq!(
            decide(&task(TaskStatus::Claimed, false)),
            Action::Skip("claimed")
        );
    }

    #[test]
    fn unknown_and_locked_never_write() {
        for status in [TaskStatus::Unknown, TaskStatus::Locked] {
            assert_eq!(
                decide(&task(status, false)),
                Action::Skip("unknown_status"),
                "{status:?} 不能触发写操作"
            );
        }
        assert_eq!(
            decide(&task(TaskStatus::NotAccepted, true)),
            Action::Skip("locked"),
            "锁定优先于状态"
        );
        assert_eq!(
            decide(&task(TaskStatus::Completed, true)),
            Action::Skip("locked")
        );
    }

    #[test]
    fn manual_class_tasks_are_still_acceptable() {
        // 分类只管展示：人工任务照样可以被接单（报名不是替用户完成）。
        let mut manual = task(TaskStatus::NotAccepted, false);
        manual.code = "wb_wechat_oa_subscribe_task".to_string();
        assert_eq!(decide(&manual), Action::Accept);
    }

    #[test]
    fn accept_parses_per_task_results() {
        let response = json!({
            "code": 0,
            "data": {"results": [
                {"task_code": "a", "status": "ok"},
                {"task_code": "b", "status": "error", "message": "prerequisite not met"},
            ]}
        });
        let outcome = parse_accept(&response, &["a".to_string(), "b".to_string()]);
        assert_eq!(outcome.accepted, vec!["a"]);
        assert_eq!(
            outcome.rejected,
            vec![("b".to_string(), "prerequisite not met".to_string())]
        );
        assert!(!outcome.unknown);
    }

    #[test]
    fn accept_without_results_is_unknown_not_success() {
        let outcome = parse_accept(&json!({"code": 0, "msg": "OK"}), &["a".to_string()]);
        assert!(outcome.unknown, "缺 results 不许猜成成功");
        assert!(outcome.accepted.is_empty());
    }

    #[test]
    fn accept_flags_requested_codes_missing_from_the_response() {
        let response =
            json!({"code": 0, "data": {"results": [{"task_code": "a", "status": "ok"}]}});
        let outcome = parse_accept(&response, &["a".to_string(), "b".to_string()]);
        assert_eq!(outcome.accepted, vec!["a"]);
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(outcome.rejected[0].0, "b");
        assert!(outcome.rejected[0].1.contains("响应中没有"));
    }

    #[test]
    fn accept_reads_results_at_the_top_level_too() {
        let response = json!({"code": 0, "results": [{"task_code": "a", "status": "ok"}]});
        let outcome = parse_accept(&response, &["a".to_string()]);
        assert_eq!(outcome.accepted, vec!["a"]);
    }

    #[test]
    fn claim_success_uses_server_amounts() {
        let response = json!({
            "code": 0, "msg": "OK",
            "data": {"already_claimed": false, "credit": 100, "energy": 5}
        });
        assert_eq!(
            parse_claim(&response),
            ClaimOutcome::Applied {
                credit: 100,
                energy: 5
            }
        );
    }

    #[test]
    fn claim_already_claimed_is_not_a_failure() {
        let response = json!({
            "code": 0, "data": {"already_claimed": true, "credit": 0, "energy": 0}
        });
        assert_eq!(parse_claim(&response), ClaimOutcome::AlreadyClaimed);
    }

    #[test]
    fn claim_rejection_keeps_the_server_message() {
        let response = json!({"code": 400, "msg": "task not completed"});
        match parse_claim(&response) {
            ClaimOutcome::Rejected { message } => assert_eq!(message, "task not completed"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn claim_without_amounts_is_unknown_and_must_be_rechecked() {
        let response = json!({"code": 0, "data": {}});
        let outcome = parse_claim(&response);
        assert!(matches!(outcome, ClaimOutcome::Unknown { .. }));
        assert!(
            matches!(outcome, ClaimOutcome::Unknown { .. }),
            "缺字段必须回查，不能当成功"
        );
    }

    #[test]
    fn claim_energy_only_still_counts_as_applied() {
        let response = json!({"code": 0, "data": {"already_claimed": false, "energy": 5}});
        assert_eq!(
            parse_claim(&response),
            ClaimOutcome::Applied {
                credit: 0,
                energy: 5
            }
        );
    }
}
