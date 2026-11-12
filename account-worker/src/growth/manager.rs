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
use super::rewards::{decide, parse_accept, parse_claim, Action, ClaimOutcome};

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
) -> AccountRun {
    let transport = LiveTransport::new(access, uid, domain);
    run_account_with(app, account_id, nickname, &transport, options).await
}

/// 可注入的执行主体。测试用 Mock 实现 `Transport`，生产用 `LiveTransport`。
pub async fn run_account_with<T: Transport>(
    app: &App,
    account_id: &str,
    nickname: &str,
    transport: &T,
    options: Options,
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
    if options.accept {
        let pending: Vec<String> = tasks
            .iter()
            .filter(|task| decide(task) == Action::Accept)
            .map(|task| task.code.clone())
            .collect();
        let titles: Vec<(String, String)> = tasks
            .iter()
            .filter(|task| decide(task) == Action::Accept)
            .map(|task| (task.code.clone(), task.title.clone()))
            .collect();

        let mut accepted_any = false;
        for batch in pending.chunks(ACCEPT_BATCH) {
            let codes = batch.to_vec();
            let outcome = transport.accept(&codes).await;
            match outcome {
                Ok(body) => {
                    let parsed = parse_accept(&body, &codes);
                    for code in codes.iter() {
                        let title = titles
                            .iter()
                            .find(|(c, _)| c == code)
                            .map(|(_, t)| t.clone())
                            .unwrap_or_else(|| code.clone());
                        if parsed.accepted.iter().any(|c| c == code) {
                            accepted_any = true;
                            run.accepted += 1;
                            let task_run = TaskRun {
                                task_code: code.clone(),
                                title,
                                action: ACTION_ACCEPT,
                                outcome: OUTCOME_OK,
                                credit: None,
                                energy: None,
                                error_kind: None,
                                error_detail: None,
                                verified_status: None,
                            };
                            record(app, account_id, &task_run);
                            run.tasks.push(task_run);
                        } else {
                            run.rejected += 1;
                            let message = parsed
                                .rejected
                                .iter()
                                .find(|(c, _)| c == code)
                                .map(|(_, m)| m.clone())
                                .unwrap_or_else(|| {
                                    if parsed.unknown {
                                        "响应里没有逐条结果".to_string()
                                    } else {
                                        "服务端未确认".to_string()
                                    }
                                });
                            let task_run = TaskRun {
                                task_code: code.clone(),
                                title,
                                action: ACTION_ACCEPT,
                                outcome: if parsed.unknown {
                                    OUTCOME_UNKNOWN
                                } else {
                                    OUTCOME_REJECTED
                                },
                                credit: None,
                                energy: None,
                                error_kind: Some(if parsed.unknown {
                                    "no_results".to_string()
                                } else {
                                    "rejected".to_string()
                                }),
                                error_detail: Some(message),
                                verified_status: None,
                            };
                            record(app, account_id, &task_run);
                            run.tasks.push(task_run);
                        }
                    }
                }
                Err(err) => {
                    let (kind, message) = detail_of(&err);
                    // 整批失败：逐条记 unknown，不重发。
                    for code in codes.iter() {
                        let title = titles
                            .iter()
                            .find(|(c, _)| c == code)
                            .map(|(_, t)| t.clone())
                            .unwrap_or_else(|| code.clone());
                        let task_run = TaskRun {
                            task_code: code.clone(),
                            title,
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
                }
            }
            if !pending.is_empty() {
                tokio::time::sleep(WRITE_GAP).await;
            }
        }

        // 接完单必须重新拉一次：接下来判断"能不能领"要基于变化后的状态。
        if accepted_any {
            match transport.tasks().await {
                Ok(body) => tasks = tasks_from(&body),
                Err(err) => {
                    let (kind, message) = detail_of(&err);
                    run.errors
                        .push(format!("接单后重新查询失败（{kind}）：{message}"));
                }
            }
        }
    }

    // ---- 3. 领奖 ----
    if options.claim {
        let claimable: Vec<GrowthTask> = tasks
            .iter()
            .filter(|task| decide(task) == Action::Claim)
            .cloned()
            .collect();

        for task in claimable.iter().take(CLAIM_BUDGET) {
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
                    task_run.outcome = if err.kind == ErrorKind::Network {
                        // 超时有可能是"服务端已经处理完了才断"：只能回查。
                        OUTCOME_UNKNOWN
                    } else if err.kind.retryable() {
                        OUTCOME_UNKNOWN
                    } else {
                        OUTCOME_FAILED
                    };
                    task_run.error_kind = Some(kind.clone());
                    task_run.error_detail = Some(message);
                    needs_verify = err.kind == ErrorKind::Network || err.kind.retryable();
                    if err.kind == ErrorKind::Unauthorized {
                        run.state = "unauthorized";
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
