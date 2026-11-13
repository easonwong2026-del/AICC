//! 单账号签到流程：查状态 → 提交 → 回查 → 记账。
//!
//! 顺序是 P0 要求（PRD §4.2），**不许调换**：
//!
//! 1. 平台 / 开关（自动轮次）
//! 2. 账号级互斥（自动与手动撞车时只有一个能跑）
//! 3. 取有效授权（Broker：默认 Magpie、Own 兜底）
//! 4. 查服务端今日状态 —— 已签到就结束，一个写请求都不发
//! 5. 提交前**再问一次开关**（PRD §5.4 的 `WriteGate`）
//! 6. 提交
//! 7. 回查：确认已签到才算 `claimed`
//! 8. 记账：每日结论 + 每次尝试
//!
//! 第 4 步是"Magpie 可能已经替用户签过"的唯一防线。本地没有记录**不是**
//! 签到的理由 —— 本地记录只能证明"我们没签过"。

use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};

use serde::Serialize;

use crate::app::App;
use crate::auth::broker;
use crate::clock::{beijing_date, now_rfc3339};
use crate::storage::checkin::{NewAttempt, NewCheckinRecord};
use crate::storage::AccountRecord;

use super::api::{self, CheckinError, ErrorKind, StatusReply, SubmitReply, Transport};
use super::state::{after_status, after_verify, AfterStatus, State};

/// 谁触发的这次执行。落库后用来区分"AICC 自动签到"与"用户点了一下"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Auto,
    Manual,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }
}

/// 每次真要发 POST 之前问一句"还允许写吗"。
///
/// 自动轮次传"这个账号现在开没开"；手动轮次传恒真 —— 用户点了按钮就是
/// 显式授权，不受自动开关约束（但仍受服务端状态检查与幂等限制）。
pub type WriteGate<'a> = &'a (dyn Fn() -> bool + Send + Sync);

/// 一次执行的结果，直接喂给界面与 RPC。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountOutcome {
    pub account_id: String,
    pub nickname: String,
    /// `state::State::as_str`
    pub state: &'static str,
    /// `auto` / `manual`
    pub source: &'static str,
    /// 服务端怎么说的：`Some(true)` 已签、`Some(false)` 未签、`None` 说不清。
    pub server_checked_in: Option<bool>,
    /// **只有 `claimed` 才有值**：服务端确认到账的奖励。
    pub credits: Option<i64>,
    /// 提交之后回查确认过。
    pub verified: bool,
    /// 这一轮真的发过写请求。
    pub submitted: bool,
    /// 今天更早的时候 AICC 自己已经签过（本地有 `claimed` 记录）。
    pub claimed_by_aicc_earlier: bool,
    pub error_kind: Option<String>,
    pub error_detail: Option<String>,
    pub checked_at: String,
}

impl AccountOutcome {
    fn new(account: &AccountRecord, source: Source) -> Self {
        Self {
            account_id: account.id.clone(),
            nickname: account.nickname.clone(),
            state: "unchecked",
            source: source.as_str(),
            server_checked_in: None,
            credits: None,
            verified: false,
            submitted: false,
            claimed_by_aicc_earlier: false,
            error_kind: None,
            error_detail: None,
            checked_at: now_rfc3339(),
        }
    }

    fn state(mut self, state: State) -> Self {
        self.state = state.as_str();
        self.checked_at = now_rfc3339();
        self
    }

    fn error(mut self, kind: &str, detail: Option<String>) -> Self {
        self.error_kind = Some(kind.to_string());
        self.error_detail = detail.map(|text| crate::rpc::sanitize(&text));
        self
    }
}

/// 账号级互斥：同一账号同一时刻只允许一次执行。
///
/// 不跨进程（worker 单实例由 App 保证），但足够挡住"自动 tick 与手动点击
/// 同时进来"这条真实路径 —— 那是最容易把同一天签到两次的场景。
fn in_flight() -> &'static Mutex<HashSet<String>> {
    static SLOT: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(HashSet::new()))
}

struct InFlight {
    account_id: String,
}

impl InFlight {
    fn acquire(account_id: &str) -> Option<Self> {
        let mut set = in_flight()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if !set.insert(account_id.to_string()) {
            return None;
        }
        Some(Self {
            account_id: account_id.to_string(),
        })
    }
}

impl Drop for InFlight {
    fn drop(&mut self) {
        let mut set = in_flight()
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        set.remove(&self.account_id);
    }
}

/// 某个账号现在有没有在执行。界面据此禁用按钮。
pub fn is_running(account_id: &str) -> bool {
    in_flight()
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .contains(account_id)
}

fn kind_of(err: &CheckinError) -> State {
    match err.kind {
        ErrorKind::Unauthorized => State::Unauthorized,
        ErrorKind::UnknownStatus => State::Unknown,
        _ => State::Failed,
    }
}

/// 生产入口：解析凭据，然后用真实传输层跑一遍。
pub async fn run_account(
    app: &Arc<App>,
    account: &AccountRecord,
    source: Source,
    gate: WriteGate<'_>,
) -> AccountOutcome {
    if account.platform != "workbuddy" {
        return AccountOutcome::new(account, source)
            .state(State::Unsupported)
            .error("unsupported_platform", Some("签到只支持 WorkBuddy".into()));
    }
    // 自动轮次先看开关：关着就一个请求都不发（连状态查询也不发）。
    // 手动轮次不看：用户点了按钮就是显式授权。
    if source == Source::Auto && !account.checkin_enabled {
        return AccountOutcome::new(account, source)
            .state(State::Disabled)
            .error("switch_off", Some("该账号未开启自动签到".into()));
    }

    let resolved = match broker::resolve(app, account) {
        Ok(broker::Outcome::Resolved(resolved)) => resolved,
        Ok(broker::Outcome::Unavailable { reason, detail }) => {
            return AccountOutcome::new(account, source)
                .state(State::NoCredential)
                .error(&reason, Some(detail));
        }
        Err(err) => {
            return AccountOutcome::new(account, source)
                .state(State::NoCredential)
                .error("error", Some(err.message));
        }
    };

    let domain = account.auth_domain.clone().unwrap_or_default();
    let transport = api::LiveTransport::new(&resolved.access, &account.uid, &domain);
    run_account_with(app, account, &transport, source, gate).await
}

/// 可注入的执行主体：测试用 Mock `Transport`，生产用 `LiveTransport`。
#[cfg(test)]
mod tests;

pub async fn run_account_with<T: Transport>(
    app: &Arc<App>,
    account: &AccountRecord,
    transport: &T,
    source: Source,
    gate: WriteGate<'_>,
) -> AccountOutcome {
    let today = beijing_date();
    let mut outcome = AccountOutcome::new(account, source);

    if account.platform != "workbuddy" {
        // 别的平台不签也不记账：往 `checkin_records` 里塞一行 Qoder 的"跳过"
        // 只会让历史查询出现永远不会有意义的数据。
        return outcome
            .state(State::Unsupported)
            .error("unsupported_platform", Some("签到只支持 WorkBuddy".into()));
    }
    if source == Source::Auto && !account.checkin_enabled {
        outcome = outcome
            .state(State::Disabled)
            .error("switch_off", Some("该账号未开启自动签到".into()));
        record_attempt(app, &outcome, State::Disabled, None);
        persist(app, &outcome, State::Disabled, None);
        return outcome;
    }

    // 今天更早的时候 AICC 自己签过没有：界面要区分"AICC 这次签的"与
    // "服务端本来就已经签过（可能是 Magpie）"。
    if let Ok(Some(record)) = app.with_db(|db| db.checkin_record(&account.id, &today)) {
        outcome.claimed_by_aicc_earlier = record.status == crate::storage::checkin::STATUS_CLAIMED;
    }

    let Some(_guard) = InFlight::acquire(&account.id) else {
        // 并发拒绝：不写每日结论，也不写尝试记录（那是噪声）。
        return outcome
            .state(State::Busy)
            .error("busy", Some("该账号正在签到".into()));
    };

    // ---- 第 4 步：查状态 -------------------------------------------------
    let status: StatusReply = match transport.status().await {
        Ok(reply) => reply,
        Err(err) => {
            let state = kind_of(&err);
            outcome = outcome
                .state(state)
                .error(err.kind.as_str(), Some(err.message.clone()));
            record_attempt(app, &outcome, state, None);
            persist(app, &outcome, state, None);
            return outcome;
        }
    };
    outcome.server_checked_in = status.checked_in;

    match after_status(status.checked_in) {
        AfterStatus::Already => {
            outcome = outcome.state(State::Already);
            record_attempt(app, &outcome, State::Already, None);
            persist(app, &outcome, State::Already, None);
            return outcome;
        }
        AfterStatus::Unknown => {
            // 服务端成功但答不出布尔值：不提交、不记成功，等下一轮重查。
            outcome = outcome.state(State::Unknown).error(
                ErrorKind::UnknownStatus.as_str(),
                Some("服务端没有返回今日签到状态，已停止写入".into()),
            );
            record_attempt(app, &outcome, State::Unknown, None);
            persist(app, &outcome, State::Unknown, None);
            return outcome;
        }
        AfterStatus::Submit => {}
    }

    // ---- 第 5 步：写之前再问一次开关 ------------------------------------
    if !gate() {
        outcome = outcome
            .state(State::Stopped)
            .error("switch_closed", Some("自动签到已关闭，未提交".into()));
        record_attempt(app, &outcome, State::Stopped, None);
        persist(app, &outcome, State::Stopped, None);
        return outcome;
    }

    // ---- 第 6 步：提交 ---------------------------------------------------
    let submit: SubmitReply = match transport.submit().await {
        Ok(reply) => reply,
        Err(err) => {
            outcome = outcome.submitted_error();
            let state = kind_of(&err);
            // 超时 / 5xx：请求**可能已经在服务端生效**。这时唯一正确的动作是回查，
            // 而不是重发（PRD §4.3）。回查说已签到，就按 claimed 记（奖励未知）。
            if matches!(err.kind, ErrorKind::Network | ErrorKind::Server) {
                let verified = transport.status().await.ok().and_then(|r| r.checked_in);
                outcome.server_checked_in = verified;
                if verified == Some(true) {
                    // 记一个可辨认的原因：状态是 claimed，但触发它的是"写超时 + 回查确认"。
                    let claimed = outcome.state(State::Claimed).error(
                        "verified_after_error",
                        Some(format!("{}（回查确认已签到）", err.message)),
                    );
                    record_attempt(app, &claimed, State::Claimed, None);
                    persist(app, &claimed, State::Claimed, None);
                    return claimed;
                }
                let unknown = outcome.state(State::Unknown).error(
                    "not_confirmed",
                    Some(format!("{}（回查未确认今日已签到）", err.message)),
                );
                record_attempt(app, &unknown, State::Unknown, None);
                persist(app, &unknown, State::Unknown, None);
                return unknown;
            }
            outcome = outcome
                .state(state)
                .error(err.kind.as_str(), Some(err.message.clone()));
            record_attempt(app, &outcome, state, None);
            persist(app, &outcome, state, None);
            return outcome;
        }
    };
    outcome.submitted = true;

    if submit.already {
        // 幂等回执：今天确实签过了，但这次没拿到任何奖励。
        outcome = outcome.state(State::Already);
        record_attempt(app, &outcome, State::Already, None);
        persist(app, &outcome, State::Already, None);
        return outcome;
    }

    // ---- 第 7 步：回查确认 ----------------------------------------------
    let verify = transport.status().await;
    let verified_checked = match &verify {
        Ok(reply) => reply.checked_in,
        Err(_) => None,
    };
    outcome.server_checked_in = verified_checked;
    match after_verify(verified_checked) {
        super::state::AfterVerify::Claimed => {
            outcome = outcome
                .verified()
                .state(State::Claimed)
                .with_credits(submit.credits);
            record_attempt(app, &outcome, State::Claimed, submit.credits);
            persist(app, &outcome, State::Claimed, submit.credits);
        }
        super::state::AfterVerify::Unknown => {
            // 提交返回成功但回查没确认：不记成功、不记奖励（PRD §4.2 第 8 条）。
            let detail = match &verify {
                Ok(_) => "签到已提交，但回查未确认今日已签到".to_string(),
                Err(err) => format!("签到已提交，回查失败：{}", err.message),
            };
            outcome = outcome
                .state(State::Unknown)
                .error("not_confirmed", Some(detail));
            record_attempt(app, &outcome, State::Unknown, None);
            persist(app, &outcome, State::Unknown, None);
        }
    }
    outcome
}

impl AccountOutcome {
    fn submitted_error(mut self) -> Self {
        self.submitted = true;
        self
    }

    fn verified(mut self) -> Self {
        self.verified = true;
        self
    }

    fn with_credits(mut self, credits: Option<i64>) -> Self {
        self.credits = credits;
        self
    }
}

/// 落每日结论。`state.writes_daily_record()` 之外的状态不写。
fn persist(app: &App, outcome: &AccountOutcome, state: State, credits: Option<i64>) {
    if !state.writes_daily_record() {
        return;
    }
    let today = beijing_date();
    let status = state.daily_status();
    let error_kind = if status == crate::storage::checkin::STATUS_FAILED {
        outcome.error_kind.as_deref()
    } else {
        None
    };
    let _ = app.with_db(|db| {
        let account = db.get_account(&outcome.account_id)?;
        let platform = account
            .map(|row| row.platform)
            .unwrap_or_else(|| "workbuddy".to_string());
        db.upsert_checkin_record(NewCheckinRecord {
            account_id: &outcome.account_id,
            platform: &platform,
            local_date: &today,
            status,
            credits,
            error_kind,
            execution_source: outcome.source,
        })?;
        // 只有服务端确认过的结论才动账号行：界面上那句"今天已签到"
        // 不能因为我们"猜"了一次就提前出现。
        if state.checked_today() {
            db.record_checkin_result(&outcome.account_id, &outcome.checked_at, state.as_str())?;
        }
        Ok(())
    });
}

/// 记一次尝试。失败不影响主流程：结论已经在返回值里，界面能看到。
fn record_attempt(app: &App, outcome: &AccountOutcome, state: State, credits: Option<i64>) {
    let attempt_status = if state.checked_today() {
        crate::storage::checkin::ATTEMPT_SUCCESS
    } else if matches!(
        state,
        State::Disabled | State::Unsupported | State::Stopped | State::Unchecked
    ) {
        crate::storage::checkin::ATTEMPT_SKIPPED
    } else {
        crate::storage::checkin::ATTEMPT_FAILED
    };
    let _ = app.with_db(|db| {
        db.insert_checkin_attempt(NewAttempt {
            account_id: &outcome.account_id,
            status: attempt_status,
            started_at: &outcome.checked_at,
            finished_at: Some(&outcome.checked_at),
            credits,
            error_kind: outcome.error_kind.as_deref(),
            error_detail: outcome.error_detail.as_deref(),
            next_retry_at: None,
        })
    });
}
