//! 账号级旅行编排：一次「检查 → 决定 → 执行 → 落快照」。
//!
//! 这个函数是全部 IO 的汇聚点，但不碰数据库：它进出都是 `TravelState` + 事件草稿，
//! 所以可以用 Mock 传输层跑完整分支（401 重同步、429 退避、重复领取、每日上限）。
//! 持久化在 `crate::storage::travel`，调度节奏在 `crate::travel::scheduler`。
//!
//! 三条硬约束写在这里，改代码前先读：
//!
//! 1. **只用 access token**。`Tokens::access` 从钥匙串取；401 时只允许
//!    `Tokens::resync` 重新从 Magpie 取一份 access，绝不调用 refresh 接口。
//! 2. **每次动作前先查服务端状态**。派出 / 领取都不是「凭本地记忆直接提交」，
//!    否则会和 Magpie、官方网页互相踩。
//! 3. **失败要退避**。429 / 超时 / 5xx 一律记连续失败次数并推后重试，
//!    令牌失效则给 6 小时冷却，不无限重试。

use chrono::{TimeZone, Utc};
use serde_json::Value;

use crate::storage::travel::TravelState;
use crate::storage::AccountRecord;

use super::api::{self, Transport};
use super::state::{
    self, is_auth_failure, Action, Label, Prior, ServerStatus, StatusError,
    TOKEN_INVALID_COOLDOWN_SECS,
};

/// 派出检查间隔：每 30 分钟看一次能不能派。
pub const DEPART_INTERVAL_SECS: i64 = 30 * 60;
/// 领取检查间隔：每 15 分钟看一次有没有到点可领的奖励。
pub const CLAIM_INTERVAL_SECS: i64 = 15 * 60;
/// 调度器心跳。比上面两个间隔细，才能在到点时尽快执行。
pub const TICK_INTERVAL_SECS: u64 = 60;

#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub depart_interval_secs: i64,
    pub claim_interval_secs: i64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            depart_interval_secs: DEPART_INTERVAL_SECS,
            claim_interval_secs: CLAIM_INTERVAL_SECS,
        }
    }
}

/// 凭据来源。线上实现读钥匙串 + 必要时从 Magpie 重新同步。
#[allow(async_fn_in_trait)]
pub trait Tokens {
    fn access(&self, account: &AccountRecord) -> Result<String, String>;
    /// 401 之后重新取一份 access。
    ///
    /// 是 async 的：自有授权要走 refresh，那是一次网络往返。实现方负责按
    /// "当前选中的来源"选路径，并且只做一次恢复（不循环）。
    async fn resync(&self, account: &AccountRecord) -> Result<String, String>;
}

#[derive(Debug, Clone)]
pub struct EventDraft {
    pub kind: &'static str,
    pub status: &'static str,
    pub message: Option<String>,
    pub credits: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub state: TravelState,
    pub events: Vec<EventDraft>,
}

fn iso(ts: i64) -> String {
    Utc.timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(crate::clock::now_rfc3339)
}

fn prior_of(state: &TravelState) -> Prior {
    Prior {
        server_state: state.server_state.clone(),
        record_id: state.record_id,
        claimed: state.claimed,
        claimed_record_id: state.claimed_record_id,
        depart_date: state.depart_date.clone(),
    }
}

/// 账号档位是否支持成长中心（国际版没有派 Buddy 旅行）。
///
/// **不要**在这里塞 `variant: "cn"`：那会让 `from_account` 直接短路成国内版，
/// 国际版账号（`www.workbuddy.ai`）也会被当成支持，然后对无此接口的域名发请求。
pub fn supports_travel(account: &AccountRecord) -> bool {
    let probe = serde_json::json!({
        "domain": account.auth_domain.clone().unwrap_or_default(),
    });
    wb_switch_core::modules::variant::WbVariant::from_account(&probe).supports_travel()
}

fn apply_status(state: &mut TravelState, status: &ServerStatus) {
    state.server_state = status.state.clone();
    state.daily_limit_reached = status.daily_limit_reached;
    state.record_id = status.record_id;
    if status.location_id.is_some() {
        state.location_id = status.location_id;
    }
    if !status.location_name.is_empty() {
        state.location_name = status.location_name.clone();
    }
    if status.depart_at > 0 {
        state.depart_at = status.depart_at;
    }
    if status.arrive_at > 0 {
        state.arrive_at = status.arrive_at;
    }
    if let Some(credit) = status.reward_credit {
        // 服务端在 traveling 阶段也会给预计奖励，记下来给界面显示。
        state.reward_credits = Some(credit);
    }
}

/// 记一次失败。`delta` 是连续失败次数的增量（大多数是 1；「无 Buddy」不动它，
/// 因为它不是错误，只是这一轮没有可派的 Buddy）。
///
/// `cooldown` 拿到最终的失败次数再算退避，避免每个调用点自己拍一个秒数。
fn fail(
    state: &mut TravelState,
    label: Label,
    error: String,
    now: i64,
    delta: i64,
    cooldown: impl FnOnce(i64) -> i64,
) {
    let failures = (state.failures + delta).max(0);
    state.label = label.as_str().to_string();
    state.failures = failures;
    state.last_error = Some(api::clean_message(&error));
    state.retry_after = Some(iso(now + cooldown(failures)));
    state.updated_at = iso(now);
}

fn ok_touch(state: &mut TravelState, now: i64) {
    state.last_checked_at = Some(iso(now));
    state.updated_at = iso(now);
    state.failures = 0;
    state.retry_after = None;
    state.last_error = None;
}

/// 一次完整的账号同步。
///
/// 两个开关都必须显式给：
/// * `allow_depart = false`：只标 `dispatchable`，不派出（还没到 30 分钟窗口，或只想看状态）；
/// * `allow_claim = false`：**纯只读**。看到 `arrived` 也只标状态、不提交领取。
///   界面的「刷新状态」走这个模式，避免用户只想看一眼却触发了领奖。
#[allow(clippy::too_many_arguments)]
pub async fn sync_account<T: Transport, K: Tokens>(
    transport: &T,
    tokens: &K,
    account: &AccountRecord,
    mut prior: TravelState,
    now: i64,
    today: &str,
    allow_depart: bool,
    allow_claim: bool,
) -> Outcome {
    let mut events = Vec::new();
    prior.account_id = account.id.clone();

    if account.platform != "workbuddy" || !supports_travel(account) {
        prior.label = Label::Unsupported.as_str().to_string();
        prior.server_state.clear();
        prior.updated_at = iso(now);
        prior.last_checked_at = Some(iso(now));
        prior.retry_after = None;
        return Outcome {
            state: prior,
            events,
        };
    }

    let domain = account.auth_domain.clone().unwrap_or_default();
    let access = match tokens.access(account) {
        Ok(token) => token,
        Err(err) => {
            fail(&mut prior, Label::Failed, err, now, 1, |_| {
                TOKEN_INVALID_COOLDOWN_SECS
            });
            events.push(EventDraft {
                kind: "status",
                status: "failed",
                message: prior.last_error.clone(),
                credits: None,
            });
            return Outcome {
                state: prior,
                events,
            };
        }
    };

    // ---- 查状态（先查再做，绝不凭本地记忆直接提交）----
    let mut payload = api::build_payload(&account.uid, &domain, &access);
    let mut resp = transport.status(&payload).await;
    if is_auth_failure(&resp) {
        match tokens.resync(account).await {
            Ok(fresh) => {
                payload = api::build_payload(&account.uid, &domain, &fresh);
                resp = transport.status(&payload).await;
            }
            Err(err) => {
                fail(
                    &mut prior,
                    Label::Failed,
                    format!("访问令牌失效，且无法重新同步：{err}"),
                    now,
                    1,
                    |_| TOKEN_INVALID_COOLDOWN_SECS,
                );
                events.push(EventDraft {
                    kind: "status",
                    status: "failed",
                    message: prior.last_error.clone(),
                    credits: None,
                });
                return Outcome {
                    state: prior,
                    events,
                };
            }
        }
    }

    let status = match state::parse_status(&resp) {
        Ok(status) => status,
        Err(err) => {
            let (label, cooldown) = retry_policy(&err, prior.failures + 1);
            fail(&mut prior, label, err.message.clone(), now, 1, |_| cooldown);
            events.push(EventDraft {
                kind: "status",
                status: "failed",
                message: prior.last_error.clone(),
                credits: None,
            });
            return Outcome {
                state: prior,
                events,
            };
        }
    };

    prior.conflict = state::detect_conflict(&status, &prior_of(&prior));
    apply_status(&mut prior, &status);
    ok_touch(&mut prior, now);

    match state::next_action(&status, &prior_of(&prior), today) {
        Action::AlreadyClaimed => {
            prior.label = Label::DoneToday.as_str().to_string();
            events.push(EventDraft {
                kind: "claim",
                status: "skipped",
                message: Some("这一程已经领取过，未重复提交".to_string()),
                credits: None,
            });
        }
        Action::Wait => {
            prior.label = if status.state == "arrived" {
                Label::Arrived.as_str().to_string()
            } else {
                Label::Traveling.as_str().to_string()
            };
        }
        Action::DoneToday => {
            prior.label = Label::DoneToday.as_str().to_string();
            if prior.depart_date.is_none() {
                prior.depart_date = Some(today.to_string());
            }
        }
        Action::Depart if !allow_depart => {
            prior.label = Label::Dispatchable.as_str().to_string();
        }
        Action::Claim if !allow_claim => {
            prior.label = Label::Arrived.as_str().to_string();
            events.push(EventDraft {
                kind: "claim",
                status: "skipped",
                message: Some("只读检查：检测到可领取，未提交".to_string()),
                credits: None,
            });
        }
        Action::Depart => {
            let outcome = do_depart(
                transport, tokens, account, &domain, &payload, prior, now, today,
            )
            .await;
            prior = outcome.state;
            events.extend(outcome.events);
        }
        Action::Claim => {
            let outcome = do_claim(
                transport, tokens, account, &domain, &payload, prior, &status, now, today,
            )
            .await;
            prior = outcome.state;
            events.extend(outcome.events);
        }
    }

    Outcome {
        state: prior,
        events,
    }
}

/// 失败 → (标签, 冷却秒数)。429 与超时按失败次数退避；401 已经在上面单独处理。
fn retry_policy(err: &StatusError, failures: i64) -> (Label, i64) {
    match err.kind {
        "unauthorized" => (Label::Failed, TOKEN_INVALID_COOLDOWN_SECS),
        "rate_limited" => (Label::Failed, state::backoff_secs(failures).max(300)),
        _ => (Label::Failed, state::backoff_secs(failures)),
    }
}

#[allow(clippy::too_many_arguments)]
async fn do_claim<T: Transport, K: Tokens>(
    transport: &T,
    tokens: &K,
    account: &AccountRecord,
    domain: &str,
    payload: &Value,
    mut prior: TravelState,
    status: &ServerStatus,
    now: i64,
    today: &str,
) -> Outcome {
    let mut events = Vec::new();
    let mut payload = payload.clone();
    let mut resp = transport.claim(&payload, status.record_id).await;

    // 401：只重同步一次，然后重试一次。仍失败就交给退避，不循环。
    if is_auth_failure(&resp) {
        match tokens.resync(account).await {
            Ok(fresh) => {
                payload = api::build_payload(&account.uid, domain, &fresh);
                resp = transport.claim(&payload, status.record_id).await;
            }
            Err(err) => {
                events.push(EventDraft {
                    kind: "claim",
                    status: "failed",
                    message: Some(api::clean_message(&format!("重新取 access 失败：{err}"))),
                    credits: None,
                });
                fail(
                    &mut prior,
                    Label::Failed,
                    format!("重新取 access 失败：{err}"),
                    now,
                    1,
                    |_| TOKEN_INVALID_COOLDOWN_SECS,
                );
                return Outcome {
                    state: prior,
                    events,
                };
            }
        }
    }

    let code = resp.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code == 429 {
        events.push(EventDraft {
            kind: "claim",
            status: "failed",
            message: Some("领取被限流（429），稍后重试".to_string()),
            credits: None,
        });
        fail(
            &mut prior,
            Label::Arrived,
            "领取被限流（429）".to_string(),
            now,
            1,
            state::backoff_secs,
        );
        return Outcome {
            state: prior,
            events,
        };
    }

    let ok = resp.get("ok").and_then(Value::as_bool) == Some(true);
    if ok {
        let credits = resp
            .get("rewardCredit")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .or(status.reward_credit);
        prior.claimed = true;
        prior.claimed_at = Some(iso(now));
        prior.claimed_record_id = if status.record_id > 0 {
            status.record_id
        } else {
            prior.record_id
        };
        if let Some(credits) = credits {
            prior.reward_credits = Some(credits);
        }
        prior.server_state = "idle".to_string();
        prior.label = Label::DoneToday.as_str().to_string();
        if prior.depart_date.is_none() {
            prior.depart_date = Some(today.to_string());
        }
        ok_touch(&mut prior, now);
        events.push(EventDraft {
            kind: "claim",
            status: "success",
            message: Some(match credits {
                Some(c) => format!("领取成功，+{c}"),
                None => "领取成功".to_string(),
            }),
            credits,
        });
        return Outcome {
            state: prior,
            events,
        };
    }

    let message = state::error_text(&resp);
    let lowered = message.to_lowercase();
    // 「已经没有可领的了」= 其它客户端先领走，属于正常竞态，不该报错重试。
    if lowered.contains("no unclaimed") || lowered.contains("daily_limit") {
        prior.claimed = true;
        prior.claimed_at = Some(iso(now));
        prior.claimed_record_id = status.record_id;
        prior.label = Label::DoneToday.as_str().to_string();
        prior.conflict = Some("奖励已在其它客户端领取".to_string());
        ok_touch(&mut prior, now);
        events.push(EventDraft {
            kind: "claim",
            status: "skipped",
            message: Some("奖励已在其它客户端领取".to_string()),
            credits: None,
        });
        return Outcome {
            state: prior,
            events,
        };
    }
    if lowered.contains("not arrived") {
        prior.label = Label::Arrived.as_str().to_string();
        events.push(EventDraft {
            kind: "claim",
            status: "skipped",
            message: Some("服务端认为还没到达，下一轮再试".to_string()),
            credits: None,
        });
        fail(&mut prior, Label::Arrived, message, now, 0, |_| {
            state::backoff_secs(1)
        });
        return Outcome {
            state: prior,
            events,
        };
    }

    fail(
        &mut prior,
        Label::Failed,
        message.clone(),
        now,
        1,
        state::backoff_secs,
    );
    events.push(EventDraft {
        kind: "claim",
        status: "failed",
        message: Some(prior.last_error.clone().unwrap_or(message)),
        credits: None,
    });
    Outcome {
        state: prior,
        events,
    }
}

#[allow(clippy::too_many_arguments)]
async fn do_depart<T: Transport, K: Tokens>(
    transport: &T,
    tokens: &K,
    account: &AccountRecord,
    domain: &str,
    payload: &Value,
    mut prior: TravelState,
    now: i64,
    today: &str,
) -> Outcome {
    let mut events = Vec::new();
    let mut payload = payload.clone();

    let config = transport.config(&payload).await;
    if is_auth_failure(&config) {
        if let Ok(fresh) = tokens.resync(account).await {
            payload = api::build_payload(&account.uid, domain, &fresh);
        }
    }
    let locations = match api::parse_locations(&config) {
        Ok(list) => list,
        Err(err) => {
            events.push(EventDraft {
                kind: "depart",
                status: "failed",
                message: Some(err.clone()),
                credits: None,
            });
            fail(&mut prior, Label::Failed, err, now, 1, state::backoff_secs);
            return Outcome {
                state: prior,
                events,
            };
        }
    };

    prior.last_depart_attempt_at = Some(iso(now));
    let mut last_error = String::from("没有可用的旅行地点");
    for (index, (location_id, name)) in locations.iter().enumerate() {
        let resp = transport.depart(&payload, location_id).await;
        if resp.get("ok").and_then(Value::as_bool) == Some(true) {
            prior.server_state = "traveling".to_string();
            prior.label = Label::Traveling.as_str().to_string();
            prior.location_id = location_id.as_i64();
            prior.location_name = name.clone();
            prior.depart_date = Some(today.to_string());
            ok_touch(&mut prior, now);
            // 补一次状态，拿到真实的 arrive_at / record_id（失败不影响派出结果）。
            if let Ok(status) = state::parse_status(&transport.status(&payload).await) {
                apply_status(&mut prior, &status);
                prior.label = Label::Traveling.as_str().to_string();
            }
            events.push(EventDraft {
                kind: "depart",
                status: "success",
                message: Some(format!("已派往{}", name)),
                credits: None,
            });
            return Outcome {
                state: prior,
                events,
            };
        }

        let message = state::error_text(&resp);
        let code = resp.get("code").and_then(Value::as_i64).unwrap_or(0);
        if code == 429 {
            events.push(EventDraft {
                kind: "depart",
                status: "failed",
                message: Some("派出被限流（429），稍后重试".to_string()),
                credits: None,
            });
            fail(
                &mut prior,
                Label::Dispatchable,
                "派出被限流（429）".to_string(),
                now,
                1,
                |failures| state::backoff_secs(failures).max(300),
            );
            return Outcome {
                state: prior,
                events,
            };
        }

        match wb_switch_core::modules::travel::classify_depart_error(code, &message) {
            wb_switch_core::modules::travel::DepartClass::AlreadyTraveling => {
                // 竞态：我们刚查完 idle，别人已经派出去了。按「旅行中」处理。
                prior.label = Label::Traveling.as_str().to_string();
                prior.server_state = "traveling".to_string();
                prior.conflict = Some("派出时发现已在旅行中（其它客户端）".to_string());
                ok_touch(&mut prior, now);
                events.push(EventDraft {
                    kind: "depart",
                    status: "skipped",
                    message: Some("已在旅行中（其它客户端）".to_string()),
                    credits: None,
                });
                return Outcome {
                    state: prior,
                    events,
                };
            }
            wb_switch_core::modules::travel::DepartClass::DailyLimit => {
                prior.label = Label::DoneToday.as_str().to_string();
                prior.daily_limit_reached = true;
                prior.depart_date = Some(today.to_string());
                ok_touch(&mut prior, now);
                events.push(EventDraft {
                    kind: "depart",
                    status: "skipped",
                    message: Some("服务端今日上限已到".to_string()),
                    credits: None,
                });
                return Outcome {
                    state: prior,
                    events,
                };
            }
            wb_switch_core::modules::travel::DepartClass::NoBuddy => {
                events.push(EventDraft {
                    kind: "depart",
                    status: "skipped",
                    message: Some("没有可派出的 Buddy".to_string()),
                    credits: None,
                });
                // 可重试：Buddy 可能稍后才有，按派出间隔再来一次。
                fail(
                    &mut prior,
                    Label::NoBuddy,
                    "没有可派出的 Buddy".to_string(),
                    now,
                    0,
                    |_| DEPART_INTERVAL_SECS,
                );
                return Outcome {
                    state: prior,
                    events,
                };
            }
            wb_switch_core::modules::travel::DepartClass::LocationUnavailable => {
                last_error = format!("地点「{name}」不可用");
                if index + 1 < locations.len() {
                    continue;
                }
            }
            wb_switch_core::modules::travel::DepartClass::Other => {
                last_error = message.clone();
                break;
            }
        }
    }

    events.push(EventDraft {
        kind: "depart",
        status: "failed",
        message: Some(api::clean_message(&last_error)),
        credits: None,
    });
    fail(
        &mut prior,
        Label::Failed,
        last_error,
        now,
        1,
        state::backoff_secs,
    );
    Outcome {
        state: prior,
        events,
    }
}

/// 调度器要不要在这一轮处理这个账号。
///
/// 判定顺序：冷却中 → 跳过；今天已完成 → 只按领取间隔复查（跨夜到货）；
/// 其余按 15 分钟一轮。
pub fn is_due(state: &TravelState, now: i64, config: &Config) -> bool {
    if let Some(retry_after) = state.retry_after.as_deref() {
        if let Ok(at) = chrono::DateTime::parse_from_rfc3339(retry_after) {
            if at.timestamp() > now {
                return false;
            }
        }
    }
    let Some(last) = state.last_checked_at.as_deref() else {
        return true;
    };
    let Ok(last) = chrono::DateTime::parse_from_rfc3339(last) else {
        return true;
    };
    now - last.timestamp() >= config.claim_interval_secs
}

/// 到没到下一次「派出尝试」的窗口（30 分钟）。
pub fn depart_window_open(state: &TravelState, now: i64, config: &Config) -> bool {
    let Some(last) = state.last_depart_attempt_at.as_deref() else {
        return true;
    };
    let Ok(last) = chrono::DateTime::parse_from_rfc3339(last) else {
        return true;
    };
    now - last.timestamp() >= config.depart_interval_secs
}
