//! 成长活跃状态（PR #103，**只读**）。
//!
//! 只回答两个问题：
//!
//! 1. 这个账号今天的成长热力墙亮了没有（`/heatmap` 的 `today.is_active`）；
//! 2. 离下一档 7 / 14 / 28 天还差几天（`/streak` 的 `next_tier_remaining`）。
//!
//! 三个来自实测（`docs/activity-rules-2026-11-17.md`）的硬约束：
//!
//! - **签到与活跃是两条独立计数。** 四个账号连续 5 天签到成功，其中三个热力墙
//!   依然是 0。所以这里把"今日签到"和"今日活跃"分成两个字段，绝不互相推导。
//! - **`next_tier_remaining` 不能自己减。** 实测出现过 `streak.days=0` 但
//!   `next_tier_remaining=6` 的账号；本地用 `tier - days` 会算出 7。
//! - **"今天"只能取服务端的。** `/heatmap` 的 `today.date` 是唯一口径；本机日期在
//!   00:00–08:00 之间会先翻页。
//!
//! 两条"不能推断"的纪律（2026-11-18 修订）：
//!
//! - `updated_at` 只是**服务端最后更新时间**，不能证明当天完成过真实任务，因此
//!   **不参与诊断**，只作为排查线索展示。
//! - 本地没有当天签到记录 **不等于没签到**：可能是在官方客户端或 Magpie 签的。
//!   这时才去读一次签到状态接口（只读、每账号最多一次）。
//!
//! 这里没有 POST，也不会伪造活跃事件：脚本刷活跃属于官方条款里的取消资格情形。

use serde::Serialize;
use serde_json::{json, Value};

use chrono::NaiveDate;

use crate::app::App;
use crate::auth::broker;
use crate::clock::now_rfc3339;
use crate::storage::checkin::CheckinRecord;
use crate::storage::AccountRecord;

use super::api::{ErrorKind, ReadOnlyEndpoints};
use super::models::as_i64;
use super::rewards_state::{tier_states, TierState};
use super::FieldError;

/// 一台机器上所有国内版账号的活跃状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityFleet {
    pub accounts: Vec<ActivityAccount>,
    pub summary: ActivitySummary,
    pub fetched_at: String,
    /// 写死在响应里，避免界面以为这条路会改状态。
    pub read_only: bool,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivitySummary {
    pub total: usize,
    pub active: usize,
    pub pending: usize,
    pub unknown: usize,
    pub needs_auth: usize,
}

/// 一个账号的活跃档案。
///
/// 每一项都是 `Option`：查不到就是查不到，界面显示"—"。
/// **`today_active = Some(false)` 与 `None` 必须区分**：前者是"确认今天还没活跃"，
/// 后者是"没问到"。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityAccount {
    pub account_id: String,
    pub nickname: String,
    /// `ok` / `partial` / `unauthorized` / `missing_credential` / `unsupported` / `failed`
    pub state: &'static str,
    pub server_today: Option<String>,
    pub today_active: Option<bool>,
    pub today_score: Option<i64>,
    /// 服务端给用户看的那句话。
    pub today_note: Option<String>,
    pub streak_days: Option<i64>,
    pub next_tier: Option<String>,
    pub next_tier_remaining: Option<i64>,
    /// 本周期累计活跃天数（`month_total_days`），与连登天数不是一回事。
    pub month_active_days: Option<i64>,
    pub tiers: Vec<TierState>,
    pub last_active_date: Option<String>,
    /// 服务端今天往前 7 天里，有热度的天数。
    pub active_days_7: Option<i64>,
    /// `/heatmap` 的 `updated_at`：**只是服务端最后更新时间**。
    /// 不参与诊断，只用于排查"这条数据是不是很久没动过"。
    pub server_updated_at: Option<String>,
    /// 本地签到记录与服务端签到状态合成后的结论。永远是对象，不会是 null。
    pub checkin: ActivityCheckin,
    pub diagnosis: Diagnosis,
    pub errors: Vec<FieldError>,
    pub fetched_at: String,
}

/// 今日签到结论。**几种来源/状态各说各话**，界面照这个显示，不合并成"已完成"。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityCheckin {
    /// `confirmed` / `absent` / `failed` / `unconfirmed` / `auth_unavailable`
    pub state: &'static str,
    /// `server`（服务端签到状态接口）或 `local`（AICC 本地记录）；未知时为 `None`。
    pub source: Option<&'static str>,
    /// 本地记录的原始状态：`success` / `already` / `failed` / `unknown` / `disabled` …
    pub status: Option<String>,
    pub checked_at: Option<String>,
    pub local_date: Option<String>,
    /// 服务端签到记录里最近的一天（`checkin_dates[0]`）。用于排查"服务端认可、
    /// AICC 没有本地记录"这种情况，不参与判定。
    pub server_last_date: Option<String>,
    /// 一句话说明：为什么是这个结论。**不含令牌**。
    pub note: Option<String>,
}

impl ActivityCheckin {
    fn of(state: &'static str, source: Option<&'static str>) -> Self {
        Self {
            state,
            source,
            status: None,
            checked_at: None,
            local_date: None,
            server_last_date: None,
            note: None,
        }
    }

    fn with_note(mut self, note: impl Into<String>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// 服务端签到状态接口的观测结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckinProbe {
    /// 服务端明确说今天签过了。
    Checked,
    /// 服务端明确说今天没签。
    NotChecked,
    /// 接口成功但没给出可判定的字段，或调用失败。
    Indeterminate,
}

/// 探针的一次观测：结论 + 服务端最近一次签到日期。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckinProbeView {
    pub verdict: CheckinProbe,
    pub last_date: Option<String>,
}

impl CheckinProbeView {
    pub fn indeterminate() -> Self {
        Self {
            verdict: CheckinProbe::Indeterminate,
            last_date: None,
        }
    }

    fn of(verdict: CheckinProbe, last_date: Option<String>) -> Self {
        Self { verdict, last_date }
    }
}

/// 诊断结论：给界面一句话，给排序一个"要不要处理"。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Diagnosis {
    /// `active` / `checkin_only` / `checkin_unverified` / `inactive` / `stale_stats` /
    /// `unknown` / `needs_auth` / `unsupported`
    pub code: &'static str,
    pub label: &'static str,
    /// 该做什么。`active` 为空字符串。
    pub action: &'static str,
    /// 是否值得提醒用户动手。
    pub needs_action: bool,
}

impl Diagnosis {
    const ACTIVE: Self = Self {
        code: "active",
        label: "今日已活跃",
        action: "",
        needs_action: false,
    };
    const CHECKIN_ONLY: Self = Self {
        code: "checkin_only",
        label: "签到已完成，今日未活跃",
        action: "去 WorkBuddy 客户端做一个真实任务，回来刷新回查",
        needs_action: true,
    };
    const CHECKIN_UNVERIFIED: Self = Self {
        code: "checkin_unverified",
        label: "签到状态未确认",
        action: "去签到页确认，或等下一次自动签到",
        needs_action: true,
    };
    const INACTIVE: Self = Self {
        code: "inactive",
        label: "今天没签到也没活跃",
        action: "先补签到，再用客户端做一次真实任务",
        needs_action: true,
    };
    const STALE_STATS: Self = Self {
        code: "stale_stats",
        label: "服务端有分数但未标记活跃（可能延迟）",
        action: "稍后点刷新回查，不用重复做事",
        needs_action: false,
    };
    const UNKNOWN: Self = Self {
        code: "unknown",
        label: "活跃状态未知",
        action: "刷新重试",
        needs_action: false,
    };
    const NEEDS_AUTH: Self = Self {
        code: "needs_auth",
        label: "授权不可用",
        action: "重新授权或同步账号",
        needs_action: true,
    };
    const UNSUPPORTED: Self = Self {
        code: "unsupported",
        label: "国际版没有成长中心",
        action: "",
        needs_action: false,
    };
}

// ---------------------------------------------------------------------------
// 纯解析：把响应变成可以单独测试的视图
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeatmapView {
    pub today: Option<String>,
    pub today_active: Option<bool>,
    pub today_score: Option<i64>,
    pub today_note: Option<String>,
    pub updated_at: Option<String>,
    pub last_active_date: Option<String>,
    pub active_days_7: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StreakView {
    pub days: Option<i64>,
    pub next_tier: Option<String>,
    pub next_tier_remaining: Option<i64>,
    pub month_active_days: Option<i64>,
}

fn node(body: &Value) -> &Value {
    body.get("data").unwrap_or(body)
}

fn non_empty(value: Option<&Value>) -> Option<String> {
    let text = value?.as_str()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

pub fn parse_heatmap(body: &Value) -> HeatmapView {
    let data = node(body);
    let today = data.get("today");
    let today_date = non_empty(today.and_then(|t| t.get("date")))
        .or_else(|| non_empty(today))
        .or_else(|| non_empty(data.get("range").and_then(|r| r.get("end_date"))));

    let cells = data.get("cells").and_then(Value::as_array);
    let mut last_active: Option<String> = None;
    let mut recent = 0i64;
    let window_start = today_date
        .as_deref()
        .and_then(|text| NaiveDate::parse_from_str(text, "%Y-%m-%d").ok())
        .map(|date| date - chrono::Duration::days(6));

    for cell in cells.into_iter().flatten() {
        let score = as_i64(cell.get("score")).unwrap_or(0);
        let Some(date) = non_empty(cell.get("date")) else {
            continue;
        };
        if score <= 0 {
            continue;
        }
        if last_active
            .as_deref()
            .map_or(true, |current| date.as_str() > current)
        {
            last_active = Some(date.clone());
        }
        if let (Some(start), Some(today)) = (window_start, today_date.as_deref()) {
            if let Ok(parsed) = NaiveDate::parse_from_str(&date, "%Y-%m-%d") {
                if parsed >= start && date.as_str() <= today {
                    recent += 1;
                }
            }
        }
    }

    HeatmapView {
        today_active: today
            .and_then(|t| t.get("is_active"))
            .and_then(Value::as_bool),
        today_score: today
            .and_then(|t| t.get("score"))
            .and_then(|v| as_i64(Some(v))),
        today_note: non_empty(today.and_then(|t| t.get("status_text"))),
        updated_at: non_empty(data.get("updated_at")),
        active_days_7: if cells.is_some() { Some(recent) } else { None },
        today: today_date,
        last_active_date: last_active,
    }
}

pub fn parse_streak(body: &Value) -> StreakView {
    let data = node(body);
    let streak = data.get("streak").unwrap_or(data);
    StreakView {
        days: as_i64(streak.get("days")),
        next_tier: non_empty(streak.get("next_tier")),
        next_tier_remaining: as_i64(streak.get("next_tier_remaining")),
        month_active_days: as_i64(streak.get("month_total_days")),
    }
}

/// 解析签到状态接口。
///
/// 上游把 `todayCheckedIn` 的缺省值写成了 `false` —— **不能照抄**：
/// "字段缺失"与"明确没签"在界面上是两句话。这里只认 `raw` 里显式给出的
/// `today_checked_in` / `todayCheckedIn`。
pub fn parse_checkin_probe(status: &Value, server_today: Option<&str>) -> CheckinProbeView {
    if status.get("ok").and_then(Value::as_bool) != Some(true) {
        return CheckinProbeView::indeterminate();
    }
    let raw = status.get("raw");
    // `checkin_dates` 是服务端自己列的"已签到日期"，用它比布尔值更硬：
    // 它能把"服务端今天"和"我们的今天"对齐。实测 AICC 没有本地记录、服务端却有
    // 2026-11-18 这一天 —— 只看布尔值就会误判成"签到没跑"。
    let dates: Vec<String> = raw
        .and_then(|raw| raw.get("checkin_dates"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str())
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let last_date = dates.first().cloned();

    if let (Some(today), false) = (server_today, dates.is_empty()) {
        let verdict = if dates.iter().any(|date| date == today) {
            CheckinProbe::Checked
        } else {
            CheckinProbe::NotChecked
        };
        return CheckinProbeView::of(verdict, last_date);
    }

    let explicit = raw
        .and_then(|raw| {
            raw.get("today_checked_in")
                .or_else(|| raw.get("todayCheckedIn"))
        })
        .and_then(Value::as_bool);
    let verdict = match explicit {
        Some(true) => CheckinProbe::Checked,
        Some(false) => CheckinProbe::NotChecked,
        None => CheckinProbe::Indeterminate,
    };
    CheckinProbeView::of(verdict, last_date)
}

/// 把"本地记录 + 服务端状态"合成今日签到结论。
///
/// 判断顺序：
/// 1. 没有服务端日期 → `unconfirmed`（不知道"今天"是哪天，任何记录都不能算）；
/// 2. 本地当天记录是成功类 → `confirmed`（来源 local）；
/// 3. 服务端明确签过 → `confirmed`（来源 server）——**能覆盖本地的 failed**，
///    因为服务端才是权威；
/// 4. 本地记录是失败类 → `failed`（来源 local）；
/// 5. 服务端明确没签 → `absent`（来源 server）；
/// 6. 其余 → `unconfirmed`。
pub fn resolve_checkin(
    server_today: Option<&str>,
    local: Option<&CheckinRecord>,
    probe: CheckinProbeView,
) -> ActivityCheckin {
    let Some(today) = server_today else {
        return ActivityCheckin::of("unconfirmed", None)
            .with_note("没有拿到服务端日期，无法判断今天的签到");
    };
    let verdict = probe.verdict;

    let local_today = local.filter(|record| record.local_date == today);
    let local_status = local_today.map(|record| record.status.trim().to_lowercase());

    let build = |state: &'static str, source: &'static str| {
        let mut view = ActivityCheckin::of(state, Some(source));
        if let Some(record) = local_today {
            view.status = Some(record.status.clone());
            view.checked_at = Some(record.checked_at.clone());
            view.local_date = Some(record.local_date.clone());
        }
        view.server_last_date = probe.last_date.clone();
        view
    };

    match local_status.as_deref() {
        Some("success") | Some("already") | Some("ok") | Some("done") => {
            build("confirmed", "local")
        }
        _ => match verdict {
            CheckinProbe::Checked => build("confirmed", "server"),
            CheckinProbe::NotChecked => match local_status.as_deref() {
                // 本地试过但失败，服务端也说没签：两条证据一致，说"失败"而不是"没签"。
                Some("failed") | Some("error") => build("failed", "local"),
                // 本地没有记录、服务端说没签：这是唯一能说"未签到"的情况。
                _ => build("absent", "server"),
            },
            CheckinProbe::Indeterminate => match local_status.as_deref() {
                // failed / unknown / disabled 都不许当成签到成功。
                Some("failed") | Some("error") => {
                    build("failed", "local").with_note("最近一次签到尝试失败；服务端状态未取到")
                }
                Some(other) => build("unconfirmed", "local")
                    .with_note(format!("本地记录状态为 {other}，不能当作已签到")),
                None => build("unconfirmed", "server")
                    .with_note("本地没有当天记录，服务端签到状态未取到"),
            },
        },
    }
}

/// 诊断。**顺序即优先级**，前几条相互排斥。
///
/// `updated_at` 不在这里出现：它只能说明服务端什么时候更新过，不能证明真实使用。
/// 唯一能说"可能延迟"的证据是服务端自相矛盾——有分数却没标记活跃。
pub fn diagnose(
    today_active: Option<bool>,
    today_score: Option<i64>,
    checkin: &ActivityCheckin,
) -> Diagnosis {
    match today_active {
        Some(true) => Diagnosis::ACTIVE,
        None => Diagnosis::UNKNOWN,
        Some(false) => {
            if today_score.unwrap_or(0) > 0 {
                return Diagnosis::STALE_STATS;
            }
            match checkin.state {
                "confirmed" => Diagnosis::CHECKIN_ONLY,
                "absent" => Diagnosis::INACTIVE,
                _ => Diagnosis::CHECKIN_UNVERIFIED,
            }
        }
    }
}

/// 从解析好的视图 + 签到结论拼出一个账号的结论。
pub fn assemble(
    account_id: &str,
    nickname: &str,
    heatmap: Option<&Value>,
    streak: Option<&Value>,
    checkin: ActivityCheckin,
) -> ActivityAccount {
    let heat = heatmap.map(parse_heatmap).unwrap_or_default();
    let strk = streak.map(parse_streak).unwrap_or_default();
    let diagnosis = diagnose(heat.today_active, heat.today_score, &checkin);

    let state = match (heatmap.is_some(), streak.is_some()) {
        (true, true) => "ok",
        (false, false) => "failed",
        _ => "partial",
    };

    ActivityAccount {
        account_id: account_id.to_string(),
        nickname: nickname.to_string(),
        state,
        server_today: heat.today,
        today_active: heat.today_active,
        today_score: heat.today_score,
        today_note: heat.today_note,
        streak_days: strk.days,
        next_tier: strk.next_tier,
        next_tier_remaining: strk.next_tier_remaining,
        month_active_days: strk.month_active_days,
        tiers: streak
            .map(|body| tier_states(Some(body), None))
            .unwrap_or_default(),
        last_active_date: heat.last_active_date,
        active_days_7: heat.active_days_7,
        server_updated_at: heat.updated_at,
        checkin,
        diagnosis,
        errors: Vec::new(),
        fetched_at: now_rfc3339(),
    }
}

fn empty_account(
    account: &AccountRecord,
    state: &'static str,
    diagnosis: Diagnosis,
) -> ActivityAccount {
    let checkin = if diagnosis.code == "needs_auth" {
        ActivityCheckin::of("auth_unavailable", None).with_note("授权不可用时无法判断签到")
    } else {
        ActivityCheckin::of("unconfirmed", None).with_note("国际版没有签到与成长中心")
    };
    ActivityAccount {
        account_id: account.id.clone(),
        nickname: account.nickname.clone(),
        state,
        server_today: None,
        today_active: None,
        today_score: None,
        today_note: None,
        streak_days: None,
        next_tier: None,
        next_tier_remaining: None,
        month_active_days: None,
        tiers: Vec::new(),
        last_active_date: None,
        active_days_7: None,
        server_updated_at: None,
        checkin,
        diagnosis,
        errors: Vec::new(),
        fetched_at: now_rfc3339(),
    }
}

// ---------------------------------------------------------------------------
// 采集
// ---------------------------------------------------------------------------

fn checkin_payload(access: &str, uid: &str, domain: &str) -> Value {
    json!({
        "access_token": access,
        "uid": uid,
        "domain": domain,
        "variant": "cn",
    })
}

/// 读一个账号的活跃状态。**最多三个 GET**：`/heatmap`、`/streak`，
/// 以及"本地拿不出成功签到记录时"才发的一次签到状态查询。
pub async fn account_activity(app: &App, account: &AccountRecord) -> ActivityAccount {
    if !account.supports_workbuddy_cn_apis() {
        return empty_account(account, "unsupported", Diagnosis::UNSUPPORTED);
    }

    let resolved = match broker::resolve(app, account) {
        Ok(broker::Outcome::Resolved(resolved)) => resolved,
        Ok(broker::Outcome::Unavailable { reason, detail }) => {
            let mut out = empty_account(account, "missing_credential", Diagnosis::NEEDS_AUTH);
            out.errors.push(FieldError {
                field: "credential",
                kind: reason,
                http: None,
                code: None,
                message: detail,
            });
            return out;
        }
        Err(err) => {
            let mut out = empty_account(account, "missing_credential", Diagnosis::NEEDS_AUTH);
            out.errors.push(FieldError {
                field: "credential",
                kind: "error".to_string(),
                http: None,
                code: None,
                message: err.message,
            });
            return out;
        }
    };

    let domain = account.auth_domain.clone().unwrap_or_default();
    let endpoints = ReadOnlyEndpoints::new(&resolved.access, &account.uid, &domain);
    let (heatmap, streak) = tokio::join!(endpoints.heatmap(), endpoints.streak());

    let mut errors = Vec::new();
    let mut unauthorized = false;
    let mut heatmap_body = None;
    let mut streak_body = None;

    match heatmap {
        Ok((body, _prefix)) => heatmap_body = Some(body),
        Err(err) => {
            unauthorized |= err.kind == ErrorKind::Unauthorized;
            errors.push(field_error("heatmap", &err));
        }
    }
    match streak {
        Ok((body, _prefix)) => streak_body = Some(body),
        Err(err) => {
            unauthorized |= err.kind == ErrorKind::Unauthorized;
            errors.push(field_error("streak", &err));
        }
    }

    // 签到记录按**服务端今天**取；服务端日期没拿到时不取，避免把昨天的记录
    // 当成今天的结论。
    let server_today = heatmap_body
        .as_ref()
        .and_then(|body| parse_heatmap(body).today);
    let local = server_today.as_deref().and_then(|today| {
        app.with_db(|db| db.checkin_record(&account.id, today))
            .ok()
            .flatten()
    });

    // 只有在"本地拿不出成功记录"时才多打一次签到状态接口：
    // 这是唯一能区分"没签到"与"在别处签了、AICC 不知道"的办法。
    let probe = if local
        .as_ref()
        .map(|record| is_success_status(&record.status))
        .unwrap_or(false)
    {
        CheckinProbeView::indeterminate()
    } else {
        let status = wb_switch_core::modules::checkin::get_checkin_status(&checkin_payload(
            &resolved.access,
            &account.uid,
            &domain,
        ))
        .await;
        parse_checkin_probe(&status, server_today.as_deref())
    };

    let checkin = resolve_checkin(server_today.as_deref(), local.as_ref(), probe);
    let mut out = assemble(
        &account.id,
        &account.nickname,
        heatmap_body.as_ref(),
        streak_body.as_ref(),
        checkin,
    );
    if unauthorized {
        out.state = "unauthorized";
        out.diagnosis = Diagnosis::NEEDS_AUTH;
        out.checkin =
            ActivityCheckin::of("auth_unavailable", None).with_note("授权已失效，签到状态无法确认");
    }
    out.errors = errors;
    out
}

fn is_success_status(status: &str) -> bool {
    matches!(
        status.trim().to_lowercase().as_str(),
        "success" | "already" | "ok" | "done"
    )
}

fn field_error(field: &'static str, err: &super::api::GrowthError) -> FieldError {
    FieldError {
        field,
        kind: err.kind.as_str().to_string(),
        http: err.http,
        code: err.code,
        message: err.message.clone(),
    }
}

/// 全部国内版账号（或指定账号）的活跃状态。
pub async fn fleet(app: &App, ids: &[String]) -> crate::rpc::RpcResult<ActivityFleet> {
    let accounts = app.with_db(|db| db.list_accounts(&Default::default()))?;
    let mut out = Vec::new();
    for account in accounts {
        if !account.supports_workbuddy_cn_apis() {
            continue;
        }
        if !ids.is_empty() && !ids.contains(&account.id) {
            continue;
        }
        out.push(account_activity(app, &account).await);
    }
    let summary = summarize(&out);
    Ok(ActivityFleet {
        accounts: out,
        summary,
        fetched_at: now_rfc3339(),
        read_only: true,
    })
}

pub fn summarize(accounts: &[ActivityAccount]) -> ActivitySummary {
    let mut summary = ActivitySummary {
        total: accounts.len(),
        ..Default::default()
    };
    for account in accounts {
        match account.diagnosis.code {
            "active" => summary.active += 1,
            "unknown" => summary.unknown += 1,
            "needs_auth" | "unsupported" => summary.needs_auth += 1,
            _ => summary.pending += 1,
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heatmap(today: &str, active: bool, score: i64, updated: &str, cells: Value) -> Value {
        json!({
            "code": 0,
            "data": {
                "today": {
                    "date": today,
                    "is_active": active,
                    "score": score,
                    "status_text": if active { "今日已活跃，继续点亮成长热力墙" } else { "开始使用以点亮今日热力墙" }
                },
                "updated_at": updated,
                "cells": cells,
                "range": { "days": 365, "start_date": "2025-11-18", "end_date": today }
            }
        })
    }

    fn streak(days: i64, next: &str, remaining: i64, month: i64) -> Value {
        json!({
            "code": 0,
            "data": {
                "streak": { "days": days, "next_tier": next, "next_tier_remaining": remaining, "month_total_days": month },
                "redemption_status": {
                    "tier_7d_status": if days >= 7 { "claimed" } else { "locked" },
                    "tier_14d_status": if days >= 14 { "claimed" } else { "locked" },
                    "tier_28d_status": "locked"
                },
                "makeup_cards": { "balance": 0, "max": 4 },
                "timezone": "Asia/Shanghai"
            }
        })
    }

    fn record(local_date: &str, status: &str) -> CheckinRecord {
        CheckinRecord {
            id: 1,
            account_id: "acc".into(),
            platform: "workbuddy".into(),
            local_date: local_date.into(),
            status: status.into(),
            credits: None,
            checked_at: format!("{local_date}T01:00:00Z"),
            error_kind: None,
            execution_source: Some("auto".into()),
        }
    }

    fn probe(verdict: CheckinProbe) -> CheckinProbeView {
        CheckinProbeView::of(verdict, None)
    }

    fn checkin_of(local: Option<&CheckinRecord>, verdict: CheckinProbe) -> ActivityCheckin {
        resolve_checkin(Some("2026-11-17"), local, probe(verdict))
    }

    fn build(active: bool, score: i64, updated: &str, checkin: ActivityCheckin) -> ActivityAccount {
        let heat = heatmap("2026-11-17", active, score, updated, json!([]));
        assemble(
            "acc",
            "高亮",
            Some(&heat),
            Some(&streak(0, "7d", 7, 0)),
            checkin,
        )
    }

    #[test]
    fn an_active_account_needs_nothing() {
        let heat = heatmap(
            "2026-11-17",
            true,
            105,
            "2026-11-17T22:38:04+08:00",
            json!([]),
        );
        let out = assemble(
            "acc",
            "卡里",
            Some(&heat),
            Some(&streak(17, "28d", 11, 17)),
            checkin_of(
                Some(&record("2026-11-17", "already")),
                CheckinProbe::Indeterminate,
            ),
        );
        assert_eq!(out.diagnosis.code, "active");
        assert!(!out.diagnosis.needs_action);
        assert_eq!(out.streak_days, Some(17));
        assert_eq!(out.next_tier_remaining, Some(11));
        assert_eq!(out.checkin.state, "confirmed");
        assert_eq!(out.checkin.source, Some("local"));
    }

    #[test]
    fn checkin_confirmed_but_not_active_is_the_common_case() {
        let out = build(
            false,
            0,
            "2026-10-25T21:46:00+08:00",
            checkin_of(
                Some(&record("2026-11-17", "already")),
                CheckinProbe::Indeterminate,
            ),
        );
        assert_eq!(out.diagnosis.code, "checkin_only");
        assert!(out.diagnosis.needs_action);
        assert_eq!(out.today_active, Some(false));
    }

    /// 本地没有记录，但服务端说今天签过了 —— 不能判成"签到没跑"。
    #[test]
    fn a_server_confirmed_checkin_without_a_local_record_is_not_a_missing_checkin() {
        let out = build(
            false,
            0,
            "2026-10-01T00:00:00+08:00",
            checkin_of(None, CheckinProbe::Checked),
        );
        assert_eq!(out.checkin.state, "confirmed");
        assert_eq!(out.checkin.source, Some("server"));
        assert_eq!(out.diagnosis.code, "checkin_only", "签到已确认，只是没活跃");
    }

    /// 本地与服务端都没签：这才是"未签到"。
    #[test]
    fn only_two_negative_signals_produce_absent() {
        let out = build(
            false,
            0,
            "2026-10-01T00:00:00+08:00",
            checkin_of(None, CheckinProbe::NotChecked),
        );
        assert_eq!(out.checkin.state, "absent");
        assert_eq!(out.diagnosis.code, "inactive");
    }

    /// 本地失败过，服务端也说没签：说"失败"，不是"没签"。
    #[test]
    fn a_failed_local_attempt_is_reported_as_failed() {
        let out = build(
            false,
            0,
            "2026-10-01T00:00:00+08:00",
            checkin_of(
                Some(&record("2026-11-17", "failed")),
                CheckinProbe::NotChecked,
            ),
        );
        assert_eq!(out.checkin.state, "failed");
        assert_eq!(out.diagnosis.code, "checkin_unverified");
    }

    /// failed / unknown / disabled 一律不许当成成功。
    #[test]
    fn no_non_success_status_is_ever_treated_as_checked_in() {
        for status in ["failed", "unknown", "disabled", "error", "", "skipped"] {
            let checkin = checkin_of(
                Some(&record("2026-11-17", status)),
                CheckinProbe::Indeterminate,
            );
            assert_ne!(checkin.state, "confirmed", "状态 {status} 被当成了已签到");
            let out = build(false, 0, "2026-10-01T00:00:00+08:00", checkin);
            assert_ne!(out.diagnosis.code, "checkin_only");
        }
    }

    /// 本地没有记录且服务端状态取不到：说"未确认"，不催人、也不说没签。
    #[test]
    fn an_unknown_checkin_is_unverified_not_missing() {
        let out = build(
            false,
            0,
            "2026-10-01T00:00:00+08:00",
            checkin_of(None, CheckinProbe::Indeterminate),
        );
        assert_eq!(out.checkin.state, "unconfirmed");
        assert!(out.checkin.note.is_some());
        assert_eq!(out.diagnosis.code, "checkin_unverified");
    }

    /// 服务端已签但本地记录是失败时，服务端优先。
    #[test]
    fn the_server_verdict_overrides_a_failed_local_attempt() {
        let out = build(
            false,
            0,
            "2026-10-01T00:00:00+08:00",
            checkin_of(Some(&record("2026-11-17", "failed")), CheckinProbe::Checked),
        );
        assert_eq!(out.checkin.state, "confirmed");
        assert_eq!(out.checkin.source, Some("server"));
    }

    /// 跨日：同一条本地记录，服务端日期翻到第二天之后就不能再算今天。
    #[test]
    fn a_rollover_invalidates_the_previous_days_checkin() {
        let checkin = resolve_checkin(
            Some("2026-11-18"),
            Some(&record("2026-11-17", "already")),
            probe(CheckinProbe::NotChecked),
        );
        assert_eq!(checkin.state, "absent", "昨天的 already 不能挂到今天");
        assert!(checkin.local_date.is_none());
    }

    /// 没有服务端日期时什么都不猜。
    #[test]
    fn without_a_server_date_the_checkin_stays_unconfirmed() {
        let checkin = resolve_checkin(
            None,
            Some(&record("2026-11-17", "already")),
            probe(CheckinProbe::Checked),
        );
        assert_eq!(checkin.state, "unconfirmed");
        assert!(checkin.note.unwrap_or_default().contains("服务端日期"));
    }

    /// `updated_at` 是今天不再等于"统计延迟"：没有分数就没有证据。
    #[test]
    fn a_fresh_updated_at_alone_proves_nothing() {
        let out = build(
            false,
            0,
            "2026-11-17T21:00:00+08:00",
            checkin_of(
                Some(&record("2026-11-17", "already")),
                CheckinProbe::Indeterminate,
            ),
        );
        assert_eq!(out.diagnosis.code, "checkin_only", "只有更新时间不能算延迟");
        assert!(out.diagnosis.needs_action);
    }

    /// 只有服务端自相矛盾（有分数、没标记活跃）才叫统计延迟。
    #[test]
    fn stale_stats_requires_a_positive_score_without_the_active_flag() {
        let out = build(
            false,
            42,
            "2026-11-17T21:00:00+08:00",
            checkin_of(
                Some(&record("2026-11-17", "already")),
                CheckinProbe::Indeterminate,
            ),
        );
        assert_eq!(out.diagnosis.code, "stale_stats");
        assert!(!out.diagnosis.needs_action, "延迟不该催用户马上做事");
        assert!(!out.diagnosis.action.is_empty());
    }

    #[test]
    fn a_missing_heatmap_is_unknown_not_inactive() {
        let out = assemble(
            "acc",
            "卡里",
            None,
            Some(&streak(17, "28d", 11, 17)),
            checkin_of(None, CheckinProbe::Indeterminate),
        );
        assert_eq!(out.diagnosis.code, "unknown");
        assert_eq!(out.today_active, None);
        assert!(!out.diagnosis.needs_action);
        assert_eq!(out.state, "partial");
    }

    #[test]
    fn both_endpoints_missing_is_failed() {
        let out = assemble(
            "acc",
            "卡里",
            None,
            None,
            checkin_of(None, CheckinProbe::Indeterminate),
        );
        assert_eq!(out.state, "failed");
    }

    /// 服务端的 `next_tier_remaining` 原样透传：实测有 `0 天 / 还差 6 天` 的账号。
    #[test]
    fn next_tier_remaining_is_server_truth_not_arithmetic() {
        let heat = heatmap(
            "2026-11-17",
            false,
            0,
            "2026-11-13T17:11:53+08:00",
            json!([]),
        );
        let out = assemble(
            "acc",
            "SAMSUNG",
            Some(&heat),
            Some(&streak(0, "7d", 6, 1)),
            checkin_of(
                Some(&record("2026-11-17", "already")),
                CheckinProbe::Indeterminate,
            ),
        );
        assert_eq!(out.next_tier.as_deref(), Some("7d"));
        assert_eq!(out.next_tier_remaining, Some(6));
        assert_ne!(out.streak_days.unwrap_or(0), 6);
    }

    #[test]
    fn active_days_in_the_last_week_follow_the_server_calendar() {
        let cells = json!([
            { "date": "2026-11-10", "score": 50 },
            { "date": "2026-11-11", "score": 10 },
            { "date": "2026-11-12", "score": 0 },
            { "date": "2026-11-13", "score": 57 },
            { "date": "2026-11-14", "score": 0 },
            { "date": "2026-11-17", "score": 105 }
        ]);
        let heat = heatmap("2026-11-17", true, 105, "2026-11-17T22:00:00+08:00", cells);
        let out = assemble(
            "acc",
            "卡里",
            Some(&heat),
            None,
            checkin_of(None, CheckinProbe::Indeterminate),
        );
        assert_eq!(out.active_days_7, Some(3));
        assert_eq!(out.last_active_date.as_deref(), Some("2026-11-17"));
        assert_eq!(out.streak_days, None, "连登没读到就是没读到，不推算");
    }

    #[test]
    fn tiers_come_from_the_shared_classifier() {
        let heat = heatmap(
            "2026-11-17",
            true,
            105,
            "2026-11-17T22:00:00+08:00",
            json!([]),
        );
        let out = assemble(
            "acc",
            "卡里",
            Some(&heat),
            Some(&streak(17, "28d", 11, 17)),
            checkin_of(None, CheckinProbe::Indeterminate),
        );
        let states: Vec<&str> = out.tiers.iter().map(|tier| tier.state).collect();
        assert_eq!(states, vec!["claimed", "claimed", "locked"]);
    }

    #[test]
    fn summary_counts_every_bucket() {
        let active = assemble(
            "a",
            "甲",
            Some(&heatmap(
                "2026-11-17",
                true,
                10,
                "2026-11-17T22:00:00+08:00",
                json!([]),
            )),
            Some(&streak(1, "7d", 6, 1)),
            checkin_of(None, CheckinProbe::Indeterminate),
        );
        let pending = build(
            false,
            0,
            "2026-10-01T00:00:00+08:00",
            checkin_of(None, CheckinProbe::Indeterminate),
        );
        let unknown = assemble(
            "c",
            "丙",
            None,
            None,
            checkin_of(None, CheckinProbe::Indeterminate),
        );
        let summary = summarize(&[active, pending, unknown]);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.active, 1);
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.unknown, 1);
    }

    #[test]
    fn a_status_text_is_kept_verbatim() {
        let out = build(
            false,
            0,
            "2026-10-01T00:00:00+08:00",
            checkin_of(None, CheckinProbe::Indeterminate),
        );
        assert_eq!(out.today_note.as_deref(), Some("开始使用以点亮今日热力墙"));
        assert_eq!(out.today_score, Some(0));
    }

    // -----------------------------------------------------------------------
    // 签到状态接口的解析
    // -----------------------------------------------------------------------

    #[test]
    fn the_probe_requires_an_explicit_field() {
        let today = Some("2026-11-18");

        // 上游把缺省值写成 false —— 不能照抄。
        let ok_without_field = json!({ "ok": true, "raw": { "other": 1 } });
        assert_eq!(
            parse_checkin_probe(&ok_without_field, today).verdict,
            CheckinProbe::Indeterminate
        );

        let failed_call = json!({ "ok": false, "error": "code=500" });
        assert_eq!(
            parse_checkin_probe(&failed_call, today).verdict,
            CheckinProbe::Indeterminate
        );

        let checked = json!({ "ok": true, "raw": { "today_checked_in": true } });
        assert_eq!(
            parse_checkin_probe(&checked, today).verdict,
            CheckinProbe::Checked
        );

        let not_checked = json!({ "ok": true, "raw": { "today_checked_in": false } });
        assert_eq!(
            parse_checkin_probe(&not_checked, today).verdict,
            CheckinProbe::NotChecked
        );

        let camel = json!({ "ok": true, "raw": { "todayCheckedIn": true } });
        assert_eq!(
            parse_checkin_probe(&camel, today).verdict,
            CheckinProbe::Checked
        );
    }

    /// `checkin_dates` 比布尔值更硬：服务端列了哪几天，就按哪几天算。
    #[test]
    fn checkin_dates_are_aligned_with_the_server_day() {
        let status = json!({
            "ok": true,
            "raw": {
                "today_checked_in": false,
                "checkin_dates": ["2026-11-18", "2026-11-17"]
            }
        });
        let view = parse_checkin_probe(&status, Some("2026-11-18"));
        assert_eq!(view.verdict, CheckinProbe::Checked, "日期列表优先于布尔值");
        assert_eq!(view.last_date.as_deref(), Some("2026-11-18"));

        // 服务端最新一天是昨天：今天还没签。
        let view = parse_checkin_probe(&status, Some("2026-11-19"));
        assert_eq!(view.verdict, CheckinProbe::NotChecked);
        assert_eq!(view.last_date.as_deref(), Some("2026-11-18"));

        // 没有服务端日期时退回布尔值。
        let stale = json!({ "ok": true, "raw": { "today_checked_in": true } });
        assert_eq!(
            parse_checkin_probe(&stale, None).verdict,
            CheckinProbe::Checked
        );
    }

    #[test]
    fn an_authorization_failure_is_not_a_missing_checkin() {
        let out = assemble(
            "acc",
            "卡里",
            None,
            None,
            ActivityCheckin::of("auth_unavailable", None),
        );
        assert_eq!(
            out.diagnosis.code, "unknown",
            "纯拼装不覆盖授权判断，由采集层改写"
        );
        assert_eq!(out.checkin.state, "auth_unavailable");
    }

    /// 诊断矩阵：任何组合都不许"没有 is_active=true 却报 active"。
    #[test]
    fn the_diagnosis_matrix_never_claims_activity_without_evidence() {
        let scores = [None, Some(0), Some(30)];
        let states = [
            "confirmed",
            "absent",
            "failed",
            "unconfirmed",
            "auth_unavailable",
        ];
        for active in [None, Some(true), Some(false)] {
            for score in scores {
                for state in states {
                    let checkin = ActivityCheckin::of(state, None);
                    let diagnosis = diagnose(active, score, &checkin);
                    if active != Some(true) {
                        assert_ne!(diagnosis.code, "active");
                    }
                    if active.is_none() {
                        assert_eq!(diagnosis.code, "unknown");
                        assert!(!diagnosis.needs_action);
                    }
                    if active == Some(false) && score.unwrap_or(0) > 0 {
                        assert_eq!(diagnosis.code, "stale_stats");
                    }
                    if diagnosis.needs_action {
                        assert!(!diagnosis.action.is_empty(), "要动手就必须说清做什么");
                    }
                }
            }
        }
    }

    #[test]
    fn reading_twice_is_idempotent() {
        let heat = heatmap(
            "2026-11-17",
            false,
            0,
            "2026-11-13T17:11:53+08:00",
            json!([{ "date": "2026-11-13", "score": 57 }]),
        );
        let streak_body = streak(0, "7d", 6, 1);
        let checkin = checkin_of(
            Some(&record("2026-11-17", "already")),
            CheckinProbe::Indeterminate,
        );
        let first = assemble(
            "acc",
            "SAMSUNG",
            Some(&heat),
            Some(&streak_body),
            checkin.clone(),
        );
        let second = assemble("acc", "SAMSUNG", Some(&heat), Some(&streak_body), checkin);
        assert_eq!(first.diagnosis, second.diagnosis);
        assert_eq!(first.today_active, second.today_active);
        assert_eq!(first.next_tier_remaining, second.next_tier_remaining);
    }
}
