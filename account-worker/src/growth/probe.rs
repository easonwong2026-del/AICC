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
    decide_with(db, account_id, command, true)
}

/// `one_shot`：取证探针只许一次；正式功能允许多次（每次仍要重新确认余额）。
pub fn decide_with(
    db: &mut Db,
    account_id: &str,
    command: &Command,
    one_shot: bool,
) -> RpcResult<ProbeDecision> {
    let action = command.action().as_str();
    let target = command.target();
    let needs_token = !matches!(command, Command::MakeupCard { .. });

    // 一次性探针：这个目标**已经有结论**（confirmed/rejected/failed）之后不再开新的。
    //
    // 注意只拦"已结束"的情形：还开着的记录要继续走同一个 attempt（回查或重发），
    // 那是同一次操作，不是第二次消耗。
    if one_shot
        && db
            .open_reward_op_for(account_id, action, &target)?
            .is_none()
        && db.has_any_reward_op(account_id, action, &target)?
    {
        return Ok(ProbeDecision::Blocked {
            op_id: None,
            reason: "这个目标已经探针过一次（含已完成），探针只允许一次性执行",
        });
    }

    // token 先生成，再**随记录一起**插入：不存在"行已落盘但 token 还没写"的窗口，
    // 另一个线程也就不可能读到 pending 且 token 为空的行。
    let token = needs_token.then(streak_rewards::client_token);

    let opened = db.open_reward_op(&crate::storage::reward_ops::NewRewardOp {
        account_id,
        action,
        target: &target,
        client_token: token.as_deref(),
    })?;

    if opened.was_created() {
        let op_id = opened.op().id.clone();
        let call = streak_rewards::call(command, token.as_deref().unwrap_or_default())
            .map_err(|_| RpcError::invalid_params("补签日期格式非法，未发送任何请求"))?;
        return Ok(ProbeDecision::Send {
            op_id,
            client_token: token,
            suffix: call.suffix,
            body: call.body,
        });
    }

    let view = reward_ledger::Open::from(opened.op());
    match reward_ledger::plan(Some(&view), needs_token) {
        Next::Resend {
            op_id,
            client_token,
        } => {
            let call = streak_rewards::call(command, client_token.as_deref().unwrap_or_default())
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
        Next::Insert { .. } => Ok(ProbeDecision::Blocked {
            op_id: Some(view.id.clone()),
            reason: "账本状态异常：已有记录却要求新建",
        }),
    }
}

/// 把一个判定翻成要落盘的结论。
///
/// 复用 `reward_ledger::verdict()` 的语义，输入来自"刚刚那次 HTTP 调用"：
/// 只有 `Granted` 算 `Applied`，拒绝类算 `Rejected`，其余（超时、5xx、未知）一律
/// `Inconclusive` —— 于是落 `unknown`，**绝不重发**。
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

/// 发送器抽象。存在的唯一理由：让"到底发了几次 POST"可以在测试里被数出来，
/// 而不必真联网。
pub trait ProbeSender {
    fn send(
        &self,
        suffix: &str,
        body: Value,
    ) -> impl std::future::Future<Output = Result<Value, super::api::GrowthError>>;
}

/// 真实发送器：**固定前缀、只发一次**（`post_exact`，不做 404 退让）。
pub struct HttpSender {
    pub payload: Value,
}

impl ProbeSender for HttpSender {
    async fn send(&self, suffix: &str, body: Value) -> Result<Value, super::api::GrowthError> {
        super::api::post_exact(&self.payload, suffix, body).await
    }
}

/// 发一次。封装在这里，让正式写路径与探针共用同一条"只发一次"的实现。
pub async fn send_once<S: ProbeSender>(
    sender: &S,
    suffix: &str,
    body: Value,
) -> Result<Value, super::api::GrowthError> {
    sender.send(suffix, body).await
}

/// 发送前的只读检查结果。**余额/可领性为 0 时不许发**。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    pub ready: bool,
    pub balance: Option<i64>,
    /// 服务端当前的状态字符串（档位用），用于回查对比。
    pub observed: Option<String>,
    pub reason: Option<&'static str>,
}

/// 执行前先 GET 一次真实状态：抽奖看次数、兑换看档位、补签看卡。
///
/// 这一步同时是"给用户看的预览"：账号、动作、余额、预计消耗。
/// 抽奖次数的判断（纯函数，可单测）。
pub fn judge_lottery(balance: Option<i64>) -> Readiness {
    match balance {
        Some(0) => Readiness {
            ready: false,
            balance: Some(0),
            observed: None,
            reason: Some("抽奖次数为 0，不发送"),
        },
        Some(value) if value > 0 => Readiness {
            ready: true,
            balance: Some(value),
            observed: None,
            reason: None,
        },
        // 负数一样是不可发送：服务端不可能给我们负次数，出现就是读错了。
        Some(value) => Readiness {
            ready: false,
            balance: Some(value),
            observed: None,
            reason: Some("余额为负，按异常处理，不发送"),
        },
        None => Readiness {
            ready: false,
            balance: None,
            observed: None,
            reason: Some("查不到抽奖次数，不发送"),
        },
    }
}

/// 档位状态的判断（纯函数）。
pub fn judge_tier(status: Option<&str>) -> Readiness {
    let lowered = status.map(|text| text.trim().to_lowercase());
    let observed = lowered.clone();
    match lowered.as_deref() {
        Some("available") | Some("ready") | Some("claimable") | Some("unlocked") => Readiness {
            ready: true,
            balance: None,
            observed: None,
            reason: None,
        },
        Some("claimed") => Readiness {
            ready: false,
            balance: None,
            observed: observed.clone(),
            reason: Some("该档位已经领过"),
        },
        Some("locked") => Readiness {
            ready: false,
            balance: None,
            observed: observed.clone(),
            reason: Some("该档位还没达成"),
        },
        _ => Readiness {
            ready: false,
            balance: None,
            observed: None,
            reason: Some("档位状态不明确，不发送"),
        },
    }
}

/// 补签卡的判断（纯函数）。
pub fn judge_makeup(balance: Option<i64>) -> Readiness {
    match balance {
        Some(0) => Readiness {
            ready: false,
            balance: Some(0),
            observed: None,
            reason: Some("补签卡为 0，不发送"),
        },
        Some(value) if value > 0 => Readiness {
            ready: true,
            balance: Some(value),
            observed: None,
            reason: None,
        },
        // 负数一样是不可发送：服务端不可能给我们负次数，出现就是读错了。
        Some(value) => Readiness {
            ready: false,
            balance: Some(value),
            observed: None,
            reason: Some("余额为负，按异常处理，不发送"),
        },
        None => Readiness {
            ready: false,
            balance: None,
            observed: None,
            reason: Some("查不到补签卡余额，不发送"),
        },
    }
}

pub async fn readiness(access: &str, uid: &str, domain: &str, command: &Command) -> Readiness {
    let endpoints = super::api::ReadOnlyEndpoints::new(access, uid, domain);
    match command {
        Command::LotteryDraw => match endpoints.lottery_chances().await {
            Ok((body, _)) => {
                let node = body.get("data").unwrap_or(&body);
                let balance = node
                    .get("balance")
                    .or_else(|| node.get("chances"))
                    .and_then(Value::as_i64);
                judge_lottery(balance)
            }
            Err(_) => judge_lottery(None),
        },
        Command::Redeem(tier) => match endpoints.redeem_summary().await {
            Ok((body, _)) => {
                let node = body.get("data").unwrap_or(&body);
                let keys: [&str; 2] = match tier {
                    Tier::Days7 => ["tier_7d_status", "starter_status"],
                    Tier::Days14 => ["tier_14d_status", "advanced_status"],
                    Tier::Days28 => ["tier_28d_status", "legendary_status"],
                };
                let status = keys
                    .iter()
                    .find_map(|key| node.get(*key).and_then(Value::as_str))
                    .map(|text| text.trim().to_lowercase());
                judge_tier(status.as_deref())
            }
            Err(_) => judge_tier(None),
        },
        Command::MakeupCard { target_date } => {
            let (streak, heatmap) = tokio::join!(endpoints.streak(), endpoints.heatmap());
            let Ok((streak_body, _)) = streak else {
                return judge_makeup(None);
            };
            let node = streak_body.get("data").unwrap_or(&streak_body);
            let balance = super::scanner::read_makeup_cards(Some(node));
            let mut readiness = judge_makeup(balance);
            if !readiness.ready {
                return readiness;
            }
            // 日期必须真的在"可补签候选"里：RPC 层只校验格式，资格要问服务端。
            let Ok((heatmap_body, _)) = heatmap else {
                readiness.ready = false;
                readiness.reason = Some("查不到活跃日历，不发送");
                return readiness;
            };
            match super::rewards_state::makeup_candidates(&streak_body, &heatmap_body) {
                Ok(days) => {
                    if !days.iter().any(|day| day == target_date) {
                        readiness.ready = false;
                        readiness.reason = Some("这一天不在可补签候选里，不发送");
                    }
                }
                Err(_) => {
                    readiness.ready = false;
                    readiness.reason = Some("活跃日历异常，无法确认补签资格");
                }
            }
            readiness
        }
    }
}

/// 传输层错误 → 用于分类的 HTTP 状态。
///
/// 超时/连接断必须是 **0**：`classify` 把 0 认成"没有响应"，
/// 也就是 `Indeterminate → unknown`。给它编一个 500 会让人误以为服务端拒绝了。
pub fn status_for_error(err: &super::api::GrowthError) -> u16 {
    if let Some(http) = err.http {
        return http as u16;
    }
    match err.kind {
        super::api::ErrorKind::Unauthorized => 401,
        super::api::ErrorKind::Forbidden => 403,
        super::api::ErrorKind::RateLimited => 429,
        super::api::ErrorKind::RouteMissing => 404,
        _ => 0,
    }
}

/// 决议 + 发送 + 落盘。**发送次数由 sender 决定，生产里就是 1 次**。
///
/// `readiness` 由调用方先算好传进来：测试可以绕过真实 GET，
/// 但仍然走同一套"余额不足不发"的判断。
pub async fn run_with<S: ProbeSender>(
    app: &std::sync::Arc<crate::app::App>,
    account_id: &str,
    command: &Command,
    readiness: &Readiness,
    sender: S,
) -> RpcResult<Value> {
    let sender = &sender;
    if !readiness.ready {
        return Ok(json!({
            "sent": false,
            "decision": "not_ready",
            "balance": readiness.balance,
            "reason": readiness.reason.unwrap_or("未满足发送条件"),
            "note": "没有向服务端发出任何写请求",
        }));
    }

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

    // 记账在发送之前。缺少幂等键的抽奖/兑换会在这里被存储层拒绝。
    app.with_db(|db| db.mark_reward_op_sent(&op_id))?;

    let (outcome, raw, http_status) = match sender.send(suffix, body).await {
        Ok(response) => (
            streak_rewards::classify(command, 200, &response),
            Some(response),
            None,
        ),
        Err(err) => {
            let status = status_for_error(&err);
            let synthesized = json!({ "code": err.code, "message": err.message });
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
    let grant_value = grant
        .map(|grant| serde_json::to_value(grant).unwrap_or(Value::Null))
        .unwrap_or(Value::Null);

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
        "balanceBefore": readiness.balance,
        "phase": verdict.phase(),
        "outcome": label,
        "verdictNote": verdict.describe(),
        "response": sanitized,
    }))
}

/// 预览：只读，返回账号、动作、当前余额与预计消耗，**不发送任何写请求**。
pub async fn preview(
    app: &std::sync::Arc<crate::app::App>,
    account_id: &str,
    command: &Command,
) -> RpcResult<Value> {
    let (account, readiness) = prepare(app, account_id, command).await?;
    Ok(json!({
        "sent": false,
        "decision": "preview",
        "accountId": account.id,
        "nickname": account.nickname,
        "action": command.action().as_str(),
        "target": command.target(),
        "balance": readiness.balance,
        "ready": readiness.ready,
        "reason": readiness.reason,
        "cost": cost_text(command),
        "note": "这只是预览：确认后会先写账本，再发一次 POST",
    }))
}

fn cost_text(command: &Command) -> &'static str {
    match command {
        Command::LotteryDraw => "消耗 1 次抽奖机会",
        Command::Redeem(_) => "消耗 1 个连登档位",
        Command::MakeupCard { .. } => "消耗 1 张补签卡",
    }
}

/// 解析账号 + 跑只读 readiness。执行与预览共用。
async fn prepare(
    app: &std::sync::Arc<crate::app::App>,
    account_id: &str,
    command: &Command,
) -> RpcResult<(crate::storage::AccountRecord, Readiness)> {
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
    let readiness = readiness(
        &resolved.access,
        &account.uid,
        account.auth_domain.as_deref().unwrap_or_default(),
        command,
    )
    .await;
    Ok((account, readiness))
}

/// 执行一次探针：先只读核对余额，再决定要不要发。
pub async fn execute(
    app: &std::sync::Arc<crate::app::App>,
    account_id: &str,
    command: &Command,
) -> RpcResult<Value> {
    let (account, readiness) = prepare(app, account_id, command).await?;
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
    let sender = HttpSender {
        payload: json!({
            "access_token": resolved.access,
            "uid": account.uid,
            "domain": account.auth_domain.clone().unwrap_or_default(),
            "variant": "cn",
        }),
    };
    run_with(app, account_id, command, &readiness, sender).await
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

    /// P0-4 回归：**第一次已经 confirmed 之后，再调必须 blocked**。
    ///
    /// 现有去重只靠未决记录，而 `confirmed` 是终态、不占唯一索引 ——
    /// 没有这道守门，一次成功后还能再抽一次。
    #[test]
    fn a_completed_probe_blocks_a_second_one() {
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
                phase: "confirmed",
                outcome: "granted",
                grant: None,
                server_state: None,
                detail: None,
                verified: true,
            },
        )
        .unwrap();

        let again = decide(&mut db, &account, &Command::LotteryDraw).unwrap();
        assert!(
            matches!(again, ProbeDecision::Blocked { .. }),
            "已完成的探针不许再来一次：{again:?}"
        );
        assert_eq!(
            db.reward_ops(&account, 10).unwrap().len(),
            1,
            "也不能偷偷建新记录"
        );
    }

    /// P0 回归：**正式路径不受一次性守卫限制** —— 抽过一次之后还能再抽。
    ///
    /// 这是 `decide_with(one_shot=false)` 存在的全部意义；参数没接上时，
    /// 第二次抽奖会被永久 Blocked。
    #[test]
    fn the_product_path_may_repeat_a_finished_draw() {
        let (mut db, _dir) = temp_db();
        let account = add_account(&mut db, "uid-1");
        let ProbeDecision::Send { op_id, .. } =
            decide_with(&mut db, &account, &Command::LotteryDraw, false).unwrap()
        else {
            panic!("第一次应为 Send");
        };
        db.mark_reward_op_sent(&op_id).unwrap();
        db.resolve_reward_op(
            &op_id,
            &Resolution {
                phase: "confirmed",
                outcome: "granted",
                grant: None,
                server_state: None,
                detail: None,
                verified: true,
            },
        )
        .unwrap();

        let again = decide_with(&mut db, &account, &Command::LotteryDraw, false).unwrap();
        assert!(
            matches!(again, ProbeDecision::Send { .. }),
            "正式路径必须允许再次抽奖，实际 {again:?}"
        );
        // 此刻账本里多了一条新的未决记录：探针会接着处理这一条（不是直接 Blocked），
        // 但一次性守卫在它结束后依旧生效 —— 见 `a_completed_probe_blocks_a_second_one`。
    }

    /// 别的动作不受一次性限制影响。
    #[test]
    fn the_one_shot_guard_is_scoped_to_its_own_target() {
        let (mut db, _dir) = temp_db();
        let account = add_account(&mut db, "uid-1");
        let ProbeDecision::Send { op_id, .. } =
            decide(&mut db, &account, &Command::LotteryDraw).unwrap()
        else {
            panic!("应为 Send");
        };
        db.mark_reward_op_sent(&op_id).unwrap();
        db.resolve_reward_op(
            &op_id,
            &Resolution {
                phase: "rejected",
                outcome: "no_chance",
                grant: None,
                server_state: None,
                detail: None,
                verified: true,
            },
        )
        .unwrap();

        // 抽奖已完成 → 再次抽奖 blocked；但兑换 7d 是另一个目标，仍可探。
        assert!(matches!(
            decide(&mut db, &account, &Command::LotteryDraw).unwrap(),
            ProbeDecision::Blocked { .. }
        ));
        assert!(matches!(
            decide(&mut db, &account, &Command::Redeem(Tier::Days7)).unwrap(),
            ProbeDecision::Send { .. }
        ));
    }

    /// 余额为 0 / 查不到时一律不发（P0 附加保护）。
    #[test]
    fn a_zero_or_unknown_balance_never_becomes_a_send() {
        assert!(!judge_lottery(Some(0)).ready);
        assert_eq!(judge_lottery(Some(0)).balance, Some(0));
        assert!(!judge_lottery(None).ready);
        assert!(judge_lottery(Some(1)).ready);

        assert!(!judge_makeup(Some(0)).ready);
        assert!(!judge_makeup(None).ready);
        assert!(judge_makeup(Some(2)).ready);

        for status in ["claimed", "locked", "something_new", ""] {
            assert!(!judge_tier(Some(status)).ready, "{status} 不该允许发送");
        }
        assert!(judge_tier(Some("available")).ready);
        assert!(judge_tier(Some("READY")).ready, "大小写不敏感");
    }

    /// 探针的 POST 必须走固定前缀、**不做 404 退让**。
    ///
    /// 行为层面没法离线模拟 404，所以在源码层盯住：一旦有人把它改回
    /// `post()`（带 `for prefix in PREFIXES` 的退让），这条会红。
    #[test]
    fn the_probe_post_never_falls_back_to_the_other_prefix() {
        let api = include_str!("api.rs");
        let body = api
            .split("pub async fn post_exact")
            .nth(1)
            .expect("post_exact 不见了");
        let body = body.split("pub async fn post(").next().unwrap();
        assert!(body.contains("VERIFIED_PREFIX"), "必须固定前缀");
        assert!(
            !body.contains("for prefix in PREFIXES"),
            "不许再带 404 前缀退让"
        );

        let probe = include_str!("probe.rs");
        assert!(
            probe.contains("super::api::post_exact"),
            "探针必须用 post_exact"
        );
    }

    /// 取证与探针两条 RPC 只能出现在 `#[cfg(debug_assertions)]` 下。
    #[test]
    fn the_dev_only_rpcs_stay_behind_debug_assertions() {
        let handlers = include_str!("../handlers.rs");
        for method in ["growth.rewards.evidence", "growth.rewards.probe"] {
            let index = handlers
                .find(&format!("\"{method}\" =>"))
                .unwrap_or_else(|| panic!("{method} 分支不见了"));
            // 按字符回溯，避免切在多字节字符中间。
            let start = handlers[..index]
                .char_indices()
                .rev()
                .nth(200)
                .map(|(offset, _)| offset)
                .unwrap_or(0);
            let context = &handlers[start..index];
            assert!(
                context.contains("#[cfg(debug_assertions)]"),
                "{method} 必须留在 debug 构建里"
            );
        }
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
