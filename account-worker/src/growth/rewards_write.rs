//! 正式奖励写路径（PR #101）：抽奖 / 连登兑换 / 补签。
//!
//! 与取证探针的区别只有一个：**不设一次性限制** —— 用户可以对同一个档位、同一天
//! 反复发起（换来的结果由服务端决定），但每一次都必须：
//!
//! 1. 重新读一次真实余额/资格（`readiness`），不足就不发；
//! 2. 用户显式确认（`confirm=true` 才发，否则只返回预览）；
//! 3. 先写账本 → 标记 sent → **只发一次 POST**（固定前缀，无 404 退让）；
//! 4. 结果四态归一：`confirmed` / `rejected` / `unknown` / `frozen`；
//!    未知与冻结都不释放目标，也绝不自动重发。
//!
//! 复用 `probe` 里已经验证过的零件（command / readiness / sender / verdict），
//! 但**不共享一次性守卫**，那条只属于取证。

use serde_json::{json, Value};

use super::probe::{self, command as build_command, ProbeAction, ProbeDecision};
use super::streak_rewards::{Outcome, Tier};
use crate::rpc::{RpcError, RpcResult};
use crate::storage::reward_ops::Resolution;

/// 正式入口：`confirm=false` 时只做预览。
pub async fn run(
    app: &std::sync::Arc<crate::app::App>,
    account_id: &str,
    action: ProbeAction,
    target: Option<&str>,
    confirm: bool,
) -> RpcResult<Value> {
    let command = build_command(action, target)?;
    let account = app
        .with_db(|db| db.get_account(account_id))?
        .ok_or_else(|| RpcError::new(crate::rpc::E_NOT_FOUND, "账号不存在"))?;
    if !account.supports_workbuddy_cn_apis() {
        return Err(RpcError::invalid_params(
            "国际版没有成长中心，不会发任何请求",
        ));
    }

    let resolved = match crate::auth::broker::resolve(app, &account) {
        Ok(crate::auth::broker::Outcome::Resolved(resolved)) => resolved,
        Ok(crate::auth::broker::Outcome::Unavailable { reason, detail }) => {
            return Err(RpcError::new(
                crate::rpc::E_AUTH,
                format!("没有可用凭据（{reason}）：{detail}"),
            ))
        }
        Err(err) => return Err(err),
    };

    // 每次发送前都重读真实状态：余额是用户最后一道防线，不能用缓存糊过去。
    let readiness = probe::readiness(
        &resolved.access,
        &account.uid,
        account.auth_domain.as_deref().unwrap_or_default(),
        &command,
    )
    .await;

    let preview = json!({
        "accountId": account.id,
        "nickname": account.nickname,
        "action": action.as_str(),
        "target": command.target(),
        "balance": readiness.balance,
        "balancedReady": readiness.ready,
        "reason": readiness.reason,
        "cost": cost_text(action),
        "readOnly": !confirm,
    });

    if !confirm {
        return Ok(json!({
            "preview": preview,
            "sent": false,
            "decision": "preview",
            "note": "这只是预览：确认后才会写账本并发一次 POST",
        }));
    }

    if !readiness.ready {
        return Ok(json!({
            "preview": preview,
            "sent": false,
            "decision": "not_ready",
            "reason": readiness.reason.unwrap_or("当前不满足发送条件"),
        }));
    }

    // 正式路径**不**走一次性守卫，但仍共用同一套账本决策。
    let decision = app.with_db(|db| probe::decide_with(db, account_id, &command, false))?;
    let (op_id, suffix, body) = match decision {
        ProbeDecision::Send {
            op_id,
            suffix,
            body,
            ..
        } => (op_id, suffix, body),
        ProbeDecision::Recheck { op_id, reason } => {
            return Ok(json!({
                "sent": false,
                "opId": op_id,
                "decision": "recheck",
                "phase": "unknown",
                "reason": reason,
                "note": "上一次请求结果未定，先回查服务端；不会重发",
            }));
        }
        ProbeDecision::Blocked { op_id, reason } => {
            return Ok(json!({
                "sent": false,
                "opId": op_id,
                "decision": "blocked",
                "reason": reason,
            }));
        }
    };

    app.with_db(|db| db.mark_reward_op_sent(&op_id))?;

    let sender = probe::HttpSender {
        payload: json!({
            "access_token": resolved.access,
            "uid": account.uid,
            "domain": account.auth_domain.clone().unwrap_or_default(),
            "variant": "cn",
        }),
    };

    let (outcome, raw, http_status) = match probe::send_once(&sender, suffix, body).await {
        Ok(response) => (
            super::streak_rewards::classify(&command, 200, &response),
            Some(response),
            None,
        ),
        Err(err) => {
            let status = probe::status_for_error(&err);
            let synthesized = json!({ "code": err.code, "message": err.message });
            (
                super::streak_rewards::classify(&command, status, &synthesized),
                None,
                Some(status),
            )
        }
    };

    let (verdict, label) = probe::resolution_for(&outcome);
    let grant_value = match &outcome {
        Outcome::Granted { grant, .. } => {
            serde_json::to_value(grant).unwrap_or(Value::Null)
        }
        _ => Value::Null,
    };
    let detail = match &outcome {
        Outcome::Granted { .. } => "服务端回应已到账".to_string(),
        Outcome::Forbidden { message } => format!("403 {message}"),
        other => other.label().to_string(),
    };

    app.with_db(|db| {
        db.resolve_reward_op(
            &op_id,
            &Resolution {
                phase: verdict.phase(),
                outcome: label,
                grant: (!grant_value.is_null()).then_some(&grant_value),
                server_state: None,
                detail: Some(&detail),
                verified: verdict.verified(),
            },
        )
    })?;

    Ok(json!({
        "sent": true,
        "opId": op_id,
        "httpStatus": http_status,
        "phase": verdict.phase(),
        "outcome": label,
        "state": state_of(verdict.phase()),
        "message": message_for(verdict.phase(), &outcome),
        "response": raw.as_ref().map(super::evidence::sanitize_json),
    }))
}

/// 四种状态给界面用：成功 / 被拒 / 未定 / 冻结。
///
/// `unknown` 与 `frozen` 都是"还没结论"，界面上不能写成失败 —— 前者等回查，
/// 后者等人。混成一个"失败"会让用户以为可以再点一次。
pub fn state_of(phase: &str) -> &'static str {
    match phase {
        "confirmed" => "succeeded",
        "rejected" => "rejected",
        "unknown" | "sent" | "pending" | "ready" => "pending",
        "frozen" => "frozen",
        _ => "failed",
    }
}

fn message_for(phase: &str, outcome: &Outcome) -> String {
    match phase {
        "confirmed" => "已到账".to_string(),
        "rejected" => match outcome {
            Outcome::AlreadyClaimed => "这次没有新到账：服务端说已经领过".to_string(),
            other => format!("服务端拒绝：{}", other.label()),
        },
        "unknown" => "结果未确定：请求可能已经发出，AICC 会先回查再决定，不会重发".to_string(),
        "frozen" => "已冻结：服务端返回了无法识别的拒绝，需要人工核对".to_string(),
        _ => "状态异常，需要人工核对".to_string(),
    }
}

fn cost_text(action: ProbeAction) -> &'static str {
    match action {
        ProbeAction::LotteryDraw => "消耗 1 次抽奖机会",
        ProbeAction::Redeem => "消耗 1 个连登档位",
        ProbeAction::MakeupCard => "消耗 1 张补签卡",
    }
}

/// 档位标识 → `Tier`，供 RPC 层做参数校验。
pub fn parse_tier(raw: &str) -> RpcResult<Tier> {
    Tier::parse(raw).ok_or_else(|| RpcError::invalid_params("target 只能是 7d / 14d / 28d"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_phase_maps_to_exactly_one_ui_state() {
        assert_eq!(state_of("confirmed"), "succeeded");
        assert_eq!(state_of("rejected"), "rejected");
        for pending in ["unknown", "sent", "pending", "ready"] {
            assert_eq!(state_of(pending), "pending", "{pending}");
        }
        assert_eq!(state_of("frozen"), "frozen");
        assert_eq!(state_of("nonsense"), "failed");
    }

    #[test]
    fn messages_never_call_an_unknown_result_a_failure() {
        let unknown = message_for("unknown", &Outcome::Indeterminate);
        assert!(unknown.contains("未确定"));
        assert!(!unknown.contains("失败"));
        assert!(unknown.contains("不会重发"));

        let frozen = message_for("frozen", &Outcome::Forbidden { message: "risk control".into() });
        assert!(frozen.contains("冻结"));
    }

    #[test]
    fn rejection_wording_distinguishes_already_claimed() {
        assert!(message_for("rejected", &Outcome::AlreadyClaimed).contains("已经领过"));
    }

    #[test]
    fn costs_are_explicit_for_every_action() {
        assert!(cost_text(ProbeAction::LotteryDraw).contains("抽奖机会"));
        assert!(cost_text(ProbeAction::Redeem).contains("档位"));
        assert!(cost_text(ProbeAction::MakeupCard).contains("补签卡"));
    }
}
