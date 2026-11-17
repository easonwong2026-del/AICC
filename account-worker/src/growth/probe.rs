//! 受控写操作探针（PR #100）：**dev-only**，一次只发一个请求。
//!
//! ## 它为什么存在
//!
//! 抽奖 / 兑换 / 补签三个写接口目前只有 B 级证据（第三方实测）。要变成 A 级，必须由
//! AICC 自己发一次真实请求并保存响应 —— 但"发一次"本身会消耗一次机会或一张卡。
//! 所以这里把它做成一个受控动作：显式确认、严格枚举、先写账本、只发一次、不重试、
//! 结果不明就停在 `unknown`。
//!
//! ## 它不是什么
//!
//! - **不是第二条写入路径**：所有状态变更仍然走 `storage::reward_ops` 的账本，
//!   决策仍由 `reward_ledger::plan` 给出，凭据门禁仍由调用方持有；
//! - **不会出现在正式构建里**：`handlers` 只在 `cfg(debug_assertions)` 下注册 RPC；
//! - **不接受任意 URL**：`ProbeAction` 是封闭枚举，suffix 与 body 由
//!   `streak_rewards::call` 生成。
//!
//! ## 一次调用的完整时序
//!
//! ```text
//! parse → 决议(Send/Recheck/Blocked) → mark_sent → POST 一次
//!        → verdict() → resolve_reward_op → 返回脱敏后的响应
//! ```

use serde_json::{json, Value};

use super::reward_ledger::{self, Next, Observation, Verdict};
use super::streak_rewards::{self, Command, Outcome, Tier};
use crate::rpc::{RpcError, RpcResult};
use crate::storage::reward_ops::Resolution;
use crate::storage::Db;

/// 允许探针执行的动作。**封闭枚举**：不认识的字符串直接拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeAction {
    LotteryDraw,
    Redeem,
    MakeupCard,
}

impl ProbeAction {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "lottery_draw" => Some(Self::LotteryDraw),
            "redeem" => Some(Self::Redeem),
            "makeup_card" => Some(Self::MakeupCard),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::LotteryDraw => "lottery_draw",
            Self::Redeem => "redeem",
            Self::MakeupCard => "makeup_card",
        }
    }
}

/// 把 `(action, target)` 翻成一次具体操作。参数不全就拒绝 —— 不做默认值猜测。
pub fn command(action: ProbeAction, target: Option<&str>) -> Result<Command, RpcError> {
    match action {
        ProbeAction::LotteryDraw => Ok(Command::LotteryDraw),
        ProbeAction::Redeem => {
            let tier = target
                .and_then(Tier::parse)
                .ok_or_else(|| RpcError::invalid_params("redeem 必须带 target=7d|14d|28d"))?;
            Ok(Command::Redeem(tier))
        }
        ProbeAction::MakeupCard => {
            let date = target
                .filter(|text| !text.trim().is_empty())
                .ok_or_else(|| RpcError::invalid_params("makeup_card 必须带 target=YYYY-MM-DD"))?;
            // 日期格式由 `Command` 自己校验；这里只要求非空。
            Ok(Command::MakeupCard {
                target_date: date.trim().to_string(),
            })
        }
    }
}

/// 决议结果。`Send` 是唯一会真的发请求的分支。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeDecision {
    Send {
        op_id: String,
        client_token: Option<String>,
        suffix: &'static str,
        body: Value,
    },
    Recheck {
        op_id: String,
        reason: &'static str,
    },
    Blocked {
        op_id: Option<String>,
        reason: &'static str,
    },
}

/// 只碰本地账本的决议层：**不发网络**，所以可以放心单测。
///
/// 顺序与正式接线一致：先 `open_reward_op`（落盘 / 复用），再 `plan()`。
/// `plan` 说 `Recheck` 或 `Blocked` 时，这里绝不返回 `Send`。
pub fn decide(db: &mut Db, account_id: &str, command: &Command) -> RpcResult<ProbeDecision> {
    let action = command.action().as_str();
    let target = command.target();
    let needs_token = !matches!(command, Command::MakeupCard { .. });

    let opened = db.open_reward_op(&crate::storage::reward_ops::NewRewardOp {
        account_id,
        action,
        target: &target,
        client_token: None, // 下面按需生成并钉入，避免"没发就留下 token"
    })?;

    // 新建的行：这次就是第一次发送。token 生成后立刻钉进账本，
    // 崩在发送之前也能靠它重发（同一次操作，不是新的一次）。
    if opened.was_created() {
        let op_id = opened.op().id.clone();
        let token = if needs_token {
            let token = streak_rewards::client_token();
            db.set_reward_op_token(&op_id, &token)?;
            token
        } else {
            String::new()
        };
        let call = streak_rewards::call(command, &token)
            .map_err(|_| RpcError::invalid_params("补签日期格式非法，未发送任何请求"))?;
        return Ok(ProbeDecision::Send {
            op_id,
            client_token: needs_token.then_some(token),
            suffix: call.suffix,
            body: call.body,
        });
    }

    // 已有未决记录：交给 plan() 决定，只有它说能发才发。
    let view = reward_ledger::Open::from(opened.op());
    match reward_ledger::plan(Some(&view), needs_token) {
        Next::Resend {
            op_id,
            client_token,
        } => {
            let token = client_token.clone().unwrap_or_default();
            let call = streak_rewards::call(command, &token)
                .map_err(|_| RpcError::invalid_params("补签日期格式非法，未发送任何请求"))?;
            Ok(ProbeDecision::Send {
                op_id,
                client_token,
                suffix: call.suffix,
                body: call.body,
            })
        }
        Next::Recheck { op_id, reason } => Ok(ProbeDecision::Recheck {
            op_id,
            reason: reason.as_str(),
        }),
        Next::Blocked { op_id, reason } => Ok(ProbeDecision::Blocked { op_id, reason }),
        // plan 对已有记录不会再给 Insert；真出现了说明账本坏了，停手。
        Next::Insert { .. } => Ok(ProbeDecision::Blocked {
            op_id: Some(view.id.clone()),
            reason: "账本状态异常：已有记录却要求新建",
        }),
    }
}

/// 把一个判定翻成要落盘的结论。
///
/// 这里复用 `reward_ledger::verdict()` 的语义，只是输入来自"刚刚那次 HTTP 调用"：
///
/// - 拿到响应 → `Applied`/`Inconclusive` 由 `classify` 决定（`Granted` 才算 Applied）；
/// - 没有响应（超时）→ `Inconclusive`，于是落 `unknown`，**绝不重发**。
pub fn observation(outcome: &Outcome) -> Observation {
    match outcome {
        Outcome::Granted { .. } => Observation::Applied,
        Outcome::Rejected(_) | Outcome::AlreadyClaimed => Observation::Rejected,
        _ => Observation::Inconclusive,
    }
}

pub fn resolution_for(outcome: &Outcome) -> (Verdict, &'static str) {
    (
        reward_ledger::verdict(observation(outcome)),
        outcome.label(),
    )
}

// ---------------------------------------------------------------------------
// 执行（dev-only RPC 调用它）
// ---------------------------------------------------------------------------

/// 传输层错误 → 用于分类的 HTTP 状态。
///
/// 超时/连接断必须是 **0**：`classify` 把 0 认成"没有响应"，也就是
/// `Indeterminate → unknown`。给它编一个 500 会让人误以为服务端拒绝了。
fn status_for_error(err: &super::api::GrowthError) -> u16 {
    if let Some(http) = err.http {
        return http as u16;
    }
    match err.kind {
        super::api::ErrorKind::Unauthorized => 401,
        super::api::ErrorKind::Forbidden => 403,
        super::api::ErrorKind::RateLimited => 429,
        super::api::ErrorKind::RouteMissing => 404,
        // Network / Server / 其它：当作没有可用状态。
        _ => 0,
    }
}

/// 执行一次探针。**调用方必须已经持有凭据门禁**（`credentials::gate::begin`），
/// 并且只在 dev 构建里注册这个入口。
pub async fn execute(
    app: &std::sync::Arc<crate::app::App>,
    account_id: &str,
    command: &Command,
) -> RpcResult<Value> {
    let account = app
        .with_db(|db| db.get_account(account_id))?
        .ok_or_else(|| RpcError::new(crate::rpc::E_NOT_FOUND, "账号不存在"))?;
    if !account.supports_workbuddy_cn_apis() {
        return Err(RpcError::invalid_params(
            "国际版没有成长中心，探针不会发任何请求",
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

    let decision = app.with_db(|db| decide(db, account_id, command))?;

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
                "reason": reason,
                "note": "上一次请求的结果还没确认，先回查服务端；本探针不会重发",
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

    // 记账在发送之前：崩在中间最多多回查一次，反过来就是可能重复消耗。
    app.with_db(|db| db.mark_reward_op_sent(&op_id))?;

    let payload = json!({
        "access_token": resolved.access,
        "uid": account.uid,
        "domain": account.auth_domain.clone().unwrap_or_default(),
        "variant": "cn",
    });

    let (outcome, raw, http_status) = match super::api::post(&payload, suffix, body).await {
        Ok((response, _prefix)) => {
            let outcome = streak_rewards::classify(command, 200, &response);
            (outcome, Some(response), None)
        }
        Err(err) => {
            let status = status_for_error(&err);
            let synthesized = json!({
                "code": err.code,
                "message": err.message,
            });
            let outcome = streak_rewards::classify(command, status, &synthesized);
            (outcome, None, Some(status))
        }
    };

    let (verdict, label) = resolution_for(&outcome);
    let grant = match &outcome {
        Outcome::Granted { grant, .. } => Some(grant),
        _ => None,
    };
    let sanitized = raw.as_ref().map(super::evidence::sanitize_json);
    let detail = match &outcome {
        Outcome::Granted { .. } => "探针：服务端回应已到账".to_string(),
        Outcome::Forbidden { message } => format!("探针：403 {message}"),
        other => format!("探针：{}", other.label()),
    };

    app.with_db(|db| {
        db.resolve_reward_op(
            &op_id,
            &Resolution {
                phase: verdict.phase(),
                outcome: label,
                grant: grant
                    .map(|grant| serde_json::to_value(grant).unwrap_or(Value::Null))
                    .as_ref(),
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
        "verdictNote": verdict.describe(),
        // 响应体已脱敏：探针的产物要进仓库。
        "response": sanitized,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::reward_ops::Resolution;
    use crate::storage::{AccountInput, Db};

    fn temp_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("accounts.sqlite")).expect("open");
        (db, dir)
    }

    fn add_account(db: &mut Db, uid: &str) -> String {
        db.upsert_account(&AccountInput {
            platform: "workbuddy".into(),
            uid: uid.into(),
            nickname: Some("探针测试".into()),
            email: None,
            group_id: None,
            auth_ref: None,
            profile_ref: None,
            auth_status: None,
            checkin_enabled: None,
            token_expires_at: None,
            auth_domain: None,
            auth_source: None,
            travel_enabled: None,
            provider: None,
        })
        .unwrap()
        .id
    }

    #[test]
    fn only_three_actions_are_accepted() {
        assert_eq!(
            ProbeAction::parse("lottery_draw"),
            Some(ProbeAction::LotteryDraw)
        );
        assert_eq!(ProbeAction::parse("redeem"), Some(ProbeAction::Redeem));
        assert_eq!(
            ProbeAction::parse("makeup_card"),
            Some(ProbeAction::MakeupCard)
        );
        for bad in ["draw", "lottery", "url", "", "../../etc/passwd"] {
            assert_eq!(ProbeAction::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn parameters_are_required_not_defaulted() {
        assert!(command(ProbeAction::LotteryDraw, None).is_ok());
        assert!(command(ProbeAction::Redeem, None).is_err());
        assert!(
            command(ProbeAction::Redeem, Some("7")).is_err(),
            "天数不是档位标识"
        );
        assert!(command(ProbeAction::Redeem, Some("7d")).is_ok());
        assert!(command(ProbeAction::MakeupCard, None).is_err());
        assert!(command(ProbeAction::MakeupCard, Some("2026-11-10")).is_ok());
    }

    #[test]
    fn a_first_probe_sends_with_a_pinned_token() {
        let (mut db, _dir) = temp_db();
        let account = add_account(&mut db, "uid-1");
        let decision = decide(&mut db, &account, &Command::LotteryDraw).unwrap();
        let ProbeDecision::Send {
            op_id,
            client_token,
            suffix,
            body,
        } = decision
        else {
            panic!("第一次探针应该是 Send，实际 {decision:?}");
        };
        assert_eq!(suffix, "/lottery/draw");
        assert!(client_token.as_deref().unwrap_or("").starts_with("u-"));
        assert_eq!(body["client_token"], client_token.clone().unwrap());
        // token 已经钉进账本行：崩溃重启后必须复用它。
        let op = db.reward_op(&op_id).unwrap().unwrap();
        assert_eq!(op.client_token, client_token);
        assert_eq!(op.phase, "pending");
    }

    /// 重复调用：第二次必须变成 `Recheck`，不能再发一次。
    #[test]
    fn calling_probe_twice_does_not_send_twice() {
        let (mut db, _dir) = temp_db();
        let account = add_account(&mut db, "uid-1");
        let first = decide(&mut db, &account, &Command::LotteryDraw).unwrap();
        let ProbeDecision::Send { op_id, .. } = first else {
            panic!("第一次应为 Send");
        };
        db.mark_reward_op_sent(&op_id).unwrap();

        let second = decide(&mut db, &account, &Command::LotteryDraw).unwrap();
        match second {
            ProbeDecision::Recheck { op_id: again, .. } => assert_eq!(again, op_id),
            other => panic!("发送后必须只回查，实际 {other:?}"),
        }
    }

    /// 并发：两个线程同时决定，**只有一个能把这次发送记进账本**。
    ///
    /// 决议本身可以都是 Send（第二个线程看到的是"已落盘但还没发"的同一行，
    /// 该走的正是复用同一 token 重发）；真正拦住第二次消耗的是
    /// `mark_reward_op_sent` 的原子更新 —— 它只允许 pending → sent 一次。
    #[test]
    fn concurrent_probes_cannot_both_mark_the_send() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("accounts.sqlite");
        let account = {
            let mut db = Db::open(&path).expect("open");
            add_account(&mut db, "uid-1")
        };

        let mut handles = Vec::new();
        for _ in 0..2 {
            let path = path.clone();
            let account = account.clone();
            handles.push(std::thread::spawn(move || {
                let mut db = Db::open(&path).expect("open");
                match decide(&mut db, &account, &Command::LotteryDraw).unwrap() {
                    ProbeDecision::Send { op_id, .. } => db.mark_reward_op_sent(&op_id).is_ok(),
                    _ => false,
                }
            }));
        }
        let sends = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .filter(|sent| *sent)
            .count();
        assert_eq!(sends, 1, "两个线程都把同一次发送记进了账本");
    }

    /// 超时 → 落 unknown → 下一次探针只回查，不再发。
    #[test]
    fn a_timeout_blocks_the_next_probe_from_sending() {
        let (mut db, _dir) = temp_db();
        let account = add_account(&mut db, "uid-1");
        let ProbeDecision::Send { op_id, .. } =
            decide(&mut db, &account, &Command::LotteryDraw).unwrap()
        else {
            panic!("第一次应为 Send");
        };
        db.mark_reward_op_sent(&op_id).unwrap();
        db.resolve_reward_op(
            &op_id,
            &Resolution {
                phase: "unknown",
                outcome: "indeterminate",
                grant: None,
                server_state: None,
                detail: Some("请求超时（15s）"),
                verified: false,
            },
        )
        .unwrap();

        let next = decide(&mut db, &account, &Command::LotteryDraw).unwrap();
        assert!(matches!(next, ProbeDecision::Recheck { .. }), "{next:?}");
    }

    #[test]
    fn timeout_is_recorded_as_inconclusive_never_as_not_applied() {
        let (verdict, label) = resolution_for(&Outcome::Indeterminate);
        assert_eq!(verdict, Verdict::StillUnknown);
        assert_eq!(verdict.phase(), "unknown");
        assert_eq!(label, "indeterminate");
    }

    #[test]
    fn only_a_granted_response_counts_as_applied() {
        let (verdict, _) = resolution_for(&Outcome::Granted {
            grant: Default::default(),
            prize: None,
        });
        assert_eq!(verdict, Verdict::Confirmed);

        let (rejected, _) = resolution_for(&Outcome::Rejected(
            super::super::streak_rewards::Rejection::NoChance,
        ));
        assert_eq!(rejected, Verdict::Rejected);
    }
}
