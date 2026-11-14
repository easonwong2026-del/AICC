//! 账号级执行：接单 → 领奖 → 复验。
//!
//! 这是 #68 的核心。它**不执行任何真实任务**，只做三件事：
//! 把没报名的任务报上名、把已完成的任务奖励领掉、领完回头核对。
//! 用户正常用 WorkBuddy 让任务自然完成，AICC 负责其余部分。
//!
//! 四条必须守住的边界：
//!
//! 1. **服务端状态是唯一依据**。本地没有"我认为它完成了"这种状态。
//! 2. **一次一轮**。同一个账号在一个 tick 里只跑一次，不会并发接单与领奖。
//! 3. **写操作零重试**。超时后先回查服务端，再决定要不要（下一轮）再试。
//! 4. **`already` 不是失败**。重复领奖的回执单独记一类，不报警也不计分。

use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::clock::now_rfc3339;
use crate::storage::growth::{
    NewGrowthRun, ACTION_ACCEPT, ACTION_CLAIM, OUTCOME_ALREADY, OUTCOME_FAILED, OUTCOME_OK,
    OUTCOME_REJECTED, OUTCOME_UNKNOWN,
};

use super::api::{ErrorKind, GrowthError, ReadOnlyEndpoints, WriteEndpoints, ACCEPT_BATCH};
use super::models::{locate_tasks, parse_task, GrowthTask, TaskStatus};
use super::rewards::{
    decide, is_task_not_completed, parse_accept, parse_claim, Action, ClaimOutcome,
};

/// 一轮里允许领几个奖。领奖是逐个请求，一轮领几十个会把单次 tick 拖到分钟级；
/// 剩下的留到下一轮，反正任务不会跑。
pub const CLAIM_BUDGET: usize = 12;

/// 两次写操作之间歇一下，别把四个账号的接单+领奖在几百毫秒内全打完。
const WRITE_GAP: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub accept: bool,
    pub claim: bool,
}

impl Options {
    pub const READ_ONLY: Self = Self {
        accept: false,
        claim: false,
    };
    pub const FULL: Self = Self {
        accept: true,
        claim: true,
    };
}

/// 每次真要发 POST 之前问一句"还允许写吗"。
///
/// 为什么不是启动时读一次就够：用户在接单/领奖进行到一半时关掉开关，
/// 剩下那些请求就不该再发出去。已经发出去的按它自己的结果走完并复核，
/// 但**新的一个都不许发**。
pub type WriteGate<'a> = &'a (dyn Fn() -> bool + Send + Sync);

/// 传输层抽象。存在的唯一理由：让写流程能在 Mock 上跑测试。
///
/// CI 里绝不能对真实账号发 POST，而"领奖超时后回查"这种分支又只有靠注入
/// 才能覆盖 —— 所以这层抽象不是为好看。
#[allow(async_fn_in_trait)]
pub trait Transport {
    async fn tasks(&self) -> Result<Value, GrowthError>;
    async fn accept(&self, codes: &[String]) -> Result<Value, GrowthError>;
    async fn claim(&self, code: &str) -> Result<Value, GrowthError>;
}

/// 线上实现：直接打成长中心。
pub struct LiveTransport {
    read: ReadOnlyEndpoints,
    write: WriteEndpoints,
}

impl LiveTransport {
    pub fn new(access: &str, uid: &str, domain: &str) -> Self {
        Self {
            read: ReadOnlyEndpoints::new(access, uid, domain),
            write: WriteEndpoints::new(access, uid, domain),
        }
    }
}

impl Transport for LiveTransport {
    async fn tasks(&self) -> Result<Value, GrowthError> {
        self.read.tasks().await.map(|(body, _)| body)
    }
    async fn accept(&self, codes: &[String]) -> Result<Value, GrowthError> {
        self.write.accept(codes).await.map(|(body, _)| body)
    }
    async fn claim(&self, code: &str) -> Result<Value, GrowthError> {
        self.write.claim(code).await.map(|(body, _)| body)
    }
}

/// 单个任务上发生的事，直接喂给界面与 `growth_runs`。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRun {
    pub task_code: String,
    pub title: String,
    /// `accept` / `claim`
    pub action: &'static str,
    /// `ok` / `already` / `rejected` / `unknown` / `failed`
    pub outcome: &'static str,
    pub credit: Option<i64>,
    pub energy: Option<i64>,
    pub error_kind: Option<String>,
    pub error_detail: Option<String>,
    /// 领奖之后回查到的服务端状态（`claimed` 才算真的到账）。
    pub verified_status: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountRun {
    pub account_id: String,
    pub nickname: String,
    /// `ok` / `partial` / `unauthorized` / `failed` / `skipped`
    pub state: &'static str,
    pub accepted: usize,
    pub claimed: usize,
    pub already_claimed: usize,
    pub rejected: usize,
    pub credit: i64,
    pub energy: i64,
    pub tasks: Vec<TaskRun>,
    pub errors: Vec<String>,
    /// 用户中途关掉自动管理，导致这一轮没跑完。
    pub stopped_by_user: bool,
    pub started_at: String,
    pub finished_at: Option<String>,
}

impl AccountRun {
    fn new(account_id: &str, nickname: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            nickname: nickname.to_string(),
            state: "ok",
            accepted: 0,
            claimed: 0,
            already_claimed: 0,
            rejected: 0,
            credit: 0,
            energy: 0,
            tasks: Vec::new(),
            errors: Vec::new(),
            stopped_by_user: false,
            started_at: now_rfc3339(),
            finished_at: None,
        }
    }
}

fn detail_of(err: &GrowthError) -> (String, String) {
    (err.kind.as_str().to_string(), err.message.clone())
}

/// 从一次 `/tasks` 响应里取某个任务的状态。
fn status_of(response: &Value, code: &str) -> Option<TaskStatus> {
    let list = locate_tasks(response)?;
    for item in list {
        if item.get("task_code").and_then(Value::as_str) == Some(code) {
            return Some(parse_task(item)?.status);
        }
    }
    None
}

fn tasks_from(response: &Value) -> Vec<GrowthTask> {
    locate_tasks(response)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .filter_map(parse_task)
        .collect()
}

fn record(app: &App, account_id: &str, run: &TaskRun) {
    // 记录失败不该打断这一轮：结果已经在返回值里，界面能看到。
    let _ = app.with_db(|db| {
        db.insert_growth_run(NewGrowthRun {
            account_id,
            task_code: &run.task_code,
            action: run.action,
            outcome: run.outcome,
            credit: run.credit,
            energy: run.energy,
            error_kind: run.error_kind.as_deref(),
            error_detail: run.error_detail.as_deref(),
        })
    });
}

/// 跑一个账号。调用方负责串行。
pub async fn run_account(
    app: &App,
    account_id: &str,
    nickname: &str,
    access: &str,
    uid: &str,
    domain: &str,
    options: Options,
    gate: WriteGate<'_>,
) -> AccountRun {
    // 第二道闸门：真正的写入口再确认一次版本。调度器已经筛过，但这里是
    // "拿凭据 → 发请求"之间唯一的位置，不能只依赖上游筛选。
    //
    // 读库失败或账号不存在时**必须拒绝执行**：以前用 `if let Ok(Some(..))`，
    // 检查失败就当成"没问题"继续跑，等于闸门可以被一次数据库抖动绕开。
    let account = match app.with_db(|db| db.get_account(account_id)) {
        Ok(Some(account)) => account,
        Ok(None) => {
            return refused(
                account_id,
                nickname,
                "account_missing",
                "账号不存在，拒绝执行成长任务",
            )
        }
        Err(err) => {
            return refused(
                account_id,
                nickname,
                "storage_error",
                &format!("读取账号失败，拒绝执行成长任务：{}", err.message),
            )
        }
    };
    if !account.supports_workbuddy_cn_apis() {
        return refused(
            account_id,
            nickname,
            "unsupported_provider",
            "WorkBuddy AI（国际版）的成长中心接口尚未接入，未发送任何请求",
        );
    }
    let transport = LiveTransport::new(access, uid, domain);
    run_account_with(app, account_id, nickname, &transport, options, gate).await
}

/// 拒绝执行：**一个请求都不发**，但要说清为什么。
fn refused(account_id: &str, nickname: &str, kind: &str, message: &str) -> AccountRun {
    let mut run = AccountRun::new(account_id, nickname);
    run.state = if kind == "unsupported_provider" {
        "unsupported"
    } else {
        "failed"
    };
    run.errors.push(format!("{kind}：{message}"));
    run.finished_at = Some(now_rfc3339());
    run
}

/// 可注入的执行主体。测试用 Mock 实现 `Transport`，生产用 `LiveTransport`。
pub async fn run_account_with<T: Transport>(
    app: &App,
    account_id: &str,
    nickname: &str,
    transport: &T,
    options: Options,
    gate: WriteGate<'_>,
) -> AccountRun {
    let mut run = AccountRun::new(account_id, nickname);

    // ---- 1. 拉一次任务列表 ----
    let first = match transport.tasks().await {
        Ok(body) => body,
        Err(err) => {
            run.state = if err.kind == ErrorKind::Unauthorized {
                "unauthorized"
            } else {
                "failed"
            };
            let (kind, message) = detail_of(&err);
            run.errors
                .push(format!("查询任务失败（{kind}）：{message}"));
            run.finished_at = Some(now_rfc3339());
            return run;
        }
    };

    let mut tasks = tasks_from(&first);

    // ---- 2. 接单 ----
    //
    // 三类结果分开处理：服务端明说 ok 的、明说 error 的、以及说不清的。
    // **只有第一类才算候选成功**，而且候选还要回查 `/tasks` 才算真的接下 ——
    // 200 + code 0 是"请求收到了"，不是"任务已接受"。
    if options.accept && run.state != "unauthorized" {
        let pending: Vec<(String, String)> = tasks
            .iter()
            .filter(|task| decide(task) == Action::Accept)
            .map(|task| (task.code.clone(), task.title.clone()))
            .collect();

        // 服务端说 ok、但还没被状态确认的那批。
        let mut candidates: Vec<(String, String)> = Vec::new();
        let mut touched = false;

        for batch in pending.chunks(ACCEPT_BATCH) {
            // 每批之前重新问一次：用户可能刚把开关关掉。
            if !gate() {
                run.stopped_by_user = true;
                run.errors.push("自动管理已关闭，中止剩余接单".to_string());
                break;
            }
            let codes: Vec<String> = batch.iter().map(|(code, _)| code.clone()).collect();
            match transport.accept(&codes).await {
                Ok(body) => {
                    touched = true;
                    let parsed = parse_accept(&body, &codes);
                    for (code, title) in batch {
                        if parsed.accepted.iter().any(|c| c == code) {
                            candidates.push((code.clone(), title.clone()));
                            continue;
                        }
                        let rejected_reason = parsed
                            .rejected
                            .iter()
                            .find(|(c, _)| c == code)
                            .map(|(_, message)| message.clone());
                        let (outcome, kind, detail) = match rejected_reason {
                            Some(message) => (OUTCOME_REJECTED, "rejected", message),
                            None => (
                                OUTCOME_UNKNOWN,
                                "unconfirmed",
                                if parsed.missing_results {
                                    "响应里没有逐条结果，未确认".to_string()
                                } else {
                                    "服务端未给出可识别的状态，未确认".to_string()
                                },
                            ),
                        };
                        if outcome == OUTCOME_REJECTED {
                            run.rejected += 1;
                        }
                        let task_run = TaskRun {
                            task_code: code.clone(),
                            title: title.clone(),
                            action: ACTION_ACCEPT,
                            outcome,
                            credit: None,
                            energy: None,
                            error_kind: Some(kind.to_string()),
                            error_detail: Some(detail),
                            verified_status: None,
                        };
                        record(app, account_id, &task_run);
                        run.tasks.push(task_run);
                    }
                }
                Err(err) => {
                    let (kind, message) = detail_of(&err);
                    // 整批失败：逐条记，**不重发**。
                    for (code, title) in batch {
                        let task_run = TaskRun {
                            task_code: code.clone(),
                            title: title.clone(),
                            action: ACTION_ACCEPT,
                            outcome: if err.kind.retryable() {
                                OUTCOME_UNKNOWN
                            } else {
                                OUTCOME_FAILED
                            },
                            credit: None,
                            energy: None,
                            error_kind: Some(kind.clone()),
                            error_detail: Some(message.clone()),
                            verified_status: None,
                        };
                        record(app, account_id, &task_run);
                        run.tasks.push(task_run);
                    }
                    run.errors.push(format!("接单失败（{kind}）：{message}"));
                    if err.kind == ErrorKind::Unauthorized {
                        // 明确 401：这份凭据已经不能用了。剩下的批次和后续领奖都不许再发。
                        run.state = "unauthorized";
                        break;
                    }
                }
            }
            tokio::time::sleep(WRITE_GAP).await;
        }

        // 回查确认：只有服务端状态真的动了，才把候选记成成功。
        if touched {
            match transport.tasks().await {
                Ok(body) => {
                    let after = tasks_from(&body);
                    for (code, title) in candidates {
                        let status = after
                            .iter()
                            .find(|task| task.code == code)
                            .map(|task| task.status);
                        let landed = matches!(
                            status,
                            Some(TaskStatus::Accepted)
                                | Some(TaskStatus::InProgress)
                                | Some(TaskStatus::Completed)
                                | Some(TaskStatus::Claimed)
                        );
                        let task_run = TaskRun {
                            task_code: code.clone(),
                            title,
                            action: ACTION_ACCEPT,
                            outcome: if landed { OUTCOME_OK } else { OUTCOME_UNKNOWN },
                            credit: None,
                            energy: None,
                            error_kind: (!landed).then(|| "not_confirmed".to_string()),
                            error_detail: (!landed)
                                .then(|| "接口返回成功，但重新查询任务状态没有变化".to_string()),
                            verified_status: status.map(|s| raw_status_name(s).to_string()),
                        };
                        if landed {
                            run.accepted += 1;
                        }
                        record(app, account_id, &task_run);
                        run.tasks.push(task_run);
                    }
                    // 领奖阶段必须基于变化后的状态。
                    if run.accepted > 0 {
                        tasks = after;
                    }
                }
                Err(err) => {
                    let (kind, message) = detail_of(&err);
                    // 回查不了：候选全部记 unknown，不猜。
                    for (code, title) in candidates {
                        let task_run = TaskRun {
                            task_code: code,
                            title,
                            action: ACTION_ACCEPT,
                            outcome: OUTCOME_UNKNOWN,
                            credit: None,
                            energy: None,
                            error_kind: Some(kind.clone()),
                            error_detail: Some(format!("接单后无法确认：{message}")),
                            verified_status: None,
                        };
                        record(app, account_id, &task_run);
                        run.tasks.push(task_run);
                    }
                    run.errors
                        .push(format!("接单后无法确认（{kind}）：{message}"));
                }
            }
        }
    }

    // ---- 3. 领奖 ----
    //
    // 接单阶段已经 401 就不再进这里：一份失效的令牌去领奖只会多几条 401。
    let mut stop_claims = run.state == "unauthorized";
    // 接单阶段被用户关掉开关：领奖整个阶段都不进。
    if options.claim && !stop_claims && !gate() {
        run.stopped_by_user = true;
        run.errors.push("自动管理已关闭，跳过领奖".to_string());
        stop_claims = true;
    }
    if options.claim && !stop_claims {
        let claimable: Vec<GrowthTask> = tasks
            .iter()
            .filter(|task| decide(task) == Action::Claim)
            .cloned()
            .collect();

        for task in claimable.iter().take(CLAIM_BUDGET) {
            // 每个任务领之前重新问一次：用户可能刚关掉开关。
            if !gate() {
                run.stopped_by_user = true;
                run.errors.push("自动管理已关闭，中止剩余领奖".to_string());
                break;
            }
            let outcome = transport.claim(&task.code).await;
            let mut needs_verify = false;

            let mut task_run = TaskRun {
                task_code: task.code.clone(),
                title: task.title.clone(),
                action: ACTION_CLAIM,
                outcome: OUTCOME_OK,
                credit: None,
                energy: None,
                error_kind: None,
                error_detail: None,
                verified_status: None,
            };

            match outcome {
                Ok(body) => match parse_claim(&body) {
                    ClaimOutcome::Applied { credit, energy } => {
                        task_run.credit = Some(credit);
                        task_run.energy = Some(energy);
                        task_run.outcome = OUTCOME_OK;
                        needs_verify = true;
                    }
                    ClaimOutcome::AlreadyClaimed => {
                        task_run.outcome = OUTCOME_ALREADY;
                        needs_verify = true;
                    }
                    ClaimOutcome::Rejected { message } => {
                        task_run.outcome = OUTCOME_REJECTED;
                        task_run.error_kind = Some("rejected".to_string());
                        task_run.error_detail = Some(message);
                    }
                    ClaimOutcome::Unknown { message } => {
                        // 响应缺关键字段：不知道发没发，必须回查。
                        task_run.outcome = OUTCOME_UNKNOWN;
                        task_run.error_kind = Some("no_payload".to_string());
                        task_run.error_detail = Some(message);
                        needs_verify = true;
                    }
                },
                Err(err) => {
                    let (kind, message) = detail_of(&err);
                    if is_task_not_completed(&err) {
                        // "还没做完"是常态，不是故障：记 rejected，下一轮重读状态再说。
                        task_run.outcome = OUTCOME_REJECTED;
                        task_run.error_kind = Some("not_completed".to_string());
                        task_run.error_detail = Some(message);
                    } else if err.kind == ErrorKind::Unauthorized {
                        // 明确 401：这份凭据不能用了，立即停止本账号剩余领奖。
                        task_run.outcome = OUTCOME_FAILED;
                        task_run.error_kind = Some(kind.clone());
                        task_run.error_detail = Some(message);
                        run.state = "unauthorized";
                        stop_claims = true;
                    } else if err.kind == ErrorKind::Network || err.kind.retryable() {
                        // 超时/限流/5xx 都有可能是"服务端已经处理完了才断"：
                        // 记 unknown 并回查，**绝不重发**。
                        task_run.outcome = OUTCOME_UNKNOWN;
                        task_run.error_kind = Some(kind.clone());
                        task_run.error_detail = Some(message);
                        needs_verify = true;
                    } else {
                        task_run.outcome = OUTCOME_FAILED;
                        task_run.error_kind = Some(kind.clone());
                        task_run.error_detail = Some(message);
                    }
                }
            }

            // ---- 4. 复验 ----
            if needs_verify {
                match transport.tasks().await {
                    Ok(body) => {
                        let status = status_of(&body, &task.code);
                        task_run.verified_status = status.map(|s| raw_status_name(s).to_string());
                        let landed = status == Some(TaskStatus::Claimed);
                        if task_run.outcome == OUTCOME_UNKNOWN && landed {
                            // 超时/缺字段但回查发现确实变 claimed：按已到账记。
                            task_run.outcome = OUTCOME_OK;
                            task_run.error_kind = None;
                            task_run.error_detail = None;
                        } else if task_run.outcome == OUTCOME_OK && !landed {
                            // 服务端说发了，状态却没变：不能记成功。
                            task_run.outcome = OUTCOME_UNKNOWN;
                            task_run.error_kind = Some("not_landed".to_string());
                            task_run.error_detail =
                                Some("领奖响应成功，但重新查询任务状态没有变化".to_string());
                        }
                    }
                    Err(err) => {
                        let (kind, message) = detail_of(&err);
                        task_run.outcome = OUTCOME_UNKNOWN;
                        task_run.error_kind = Some(kind.clone());
                        task_run.error_detail = Some(format!("领奖后无法复核：{message}"));
                    }
                }
            }

            // **复核之后**才结算：一次领奖只有走到这里才算数。
            match task_run.outcome {
                OUTCOME_OK => {
                    run.claimed += 1;
                    run.credit += task_run.credit.unwrap_or(0);
                    run.energy += task_run.energy.unwrap_or(0);
                }
                OUTCOME_ALREADY => run.already_claimed += 1,
                OUTCOME_REJECTED | OUTCOME_FAILED => run.rejected += 1,
                _ => {}
            }

            record(app, account_id, &task_run);
            run.tasks.push(task_run);
            if stop_claims {
                break;
            }
            tokio::time::sleep(WRITE_GAP).await;
        }

        if claimable.len() > CLAIM_BUDGET {
            run.errors.push(format!(
                "本轮领奖达到上限 {CLAIM_BUDGET}，剩余 {} 个留到下一轮",
                claimable.len() - CLAIM_BUDGET
            ));
        }
    }

    if run.state != "unauthorized" && !run.errors.is_empty() {
        run.state = if run.tasks.is_empty() {
            "failed"
        } else {
            "partial"
        };
    }
    run.finished_at = Some(now_rfc3339());
    run
}

fn raw_status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::NotAccepted => "not_accepted",
        TaskStatus::Accepted => "accepted",
        TaskStatus::InProgress => "in_progress",
        TaskStatus::Completed => "completed",
        TaskStatus::Claimed => "claimed",
        TaskStatus::Locked => "locked",
        TaskStatus::Unknown => "unknown",
    }
}

/// 一轮里"要不要真的写"：只有存在待接单或待领奖的任务时才写。
///
/// 这个判断让默认关闭状态下的监控也能派上用场：只读轮询照样能算出
/// "有 3 个任务可以接"，界面就能提示用户去开开关。
// 目前只有测试在用；#69 的批量计划会直接拿它算"这一轮值不值得发请求"。
#[allow(dead_code)]
pub fn plan_of(tasks: &[GrowthTask]) -> (usize, usize) {
    let mut acceptable = 0;
    let mut claimable = 0;
    for task in tasks {
        match decide(task) {
            Action::Accept => acceptable += 1,
            Action::Claim => claimable += 1,
            _ => {}
        }
    }
    (acceptable, claimable)
}

#[cfg(test)]
mod tests;
