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
//!   依然是 0。所以这里把"今日签到"和"今日活跃"分成两个字段，绝不互相推导，
//!   也绝不因为签到成功就显示"今天已完成"。
//! - **`next_tier_remaining` 不能自己减。** 实测出现过 `streak.days=0` 但
//!   `next_tier_remaining=6` 的账号；本地用 `tier - days` 会算出 7，和界面对不上。
//!   这个数字原样透传。
//! - **"今天"只能取服务端的。** `/heatmap` 的 `today.date` 是唯一口径；本机日期在
//!   00:00–08:00 之间会先翻页。签到记录也只有 `local_date` 等于服务端今天时才作数。
//!
//! 这里没有 POST，也不会伪造活跃事件：脚本刷活跃属于官方条款里的取消资格情形。

use serde::Serialize;
use serde_json::Value;

use chrono::NaiveDate;

use crate::app::App;
use crate::auth::broker;
use crate::clock::now_rfc3339;
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
    /// 顶栏：今日已活跃 / 待处理 / 未知 / 需要重新授权。
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
/// 后者是"没问到"。把后者当 false 会催用户去做一件可能已经做过的事。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityAccount {
    pub account_id: String,
    pub nickname: String,
    /// `ok` / `partial` / `unauthorized` / `missing_credential` / `unsupported`
    pub state: &'static str,
    pub server_today: Option<String>,
    pub today_active: Option<bool>,
    pub today_score: Option<i64>,
    /// 服务端给用户看的那句话（未活跃时是"开始使用以点亮今日热力墙"）。
    pub today_note: Option<String>,
    pub streak_days: Option<i64>,
    pub next_tier: Option<String>,
    pub next_tier_remaining: Option<i64>,
    /// 本周期累计活跃天数（`month_total_days`），与连登天数不是一回事。
    pub month_active_days: Option<i64>,
    pub tiers: Vec<TierState>,
    /// 最近一次有热度的日期（今天也算）。
    pub last_active_date: Option<String>,
    /// 服务端今天往前 7 天里，有热度的天数。
    pub active_days_7: Option<i64>,
    /// `/heatmap` 的 `updated_at`：这个账号最后一次产生行为的时间。
    /// 用来区分"今天确实没动"与"统计还没落库"。
    pub updated_at: Option<String>,
    /// 本地签到记录（若服务端今天有）。
    pub checkin: Option<ActivityCheckin>,
    pub diagnosis: Diagnosis,
    pub errors: Vec<FieldError>,
    pub fetched_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActivityCheckin {
    /// 服务端原话：`success` / `already` / `failed` …
    pub status: String,
    pub checked_at: String,
    pub local_date: String,
    /// `auto` / `manual`。
    pub source: Option<String>,
}

/// 诊断结论：给界面一句话，给排序一个"要不要处理"。
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Diagnosis {
    /// `active` / `checkin_only` / `checkin_missing` / `stale_stats` / `unknown` / `needs_auth` / `unsupported`
    pub code: &'static str,
    pub label: &'static str,
    /// 是否值得提醒用户去官方客户端做事。
    pub needs_action: bool,
}

impl Diagnosis {
    const ACTIVE: Self = Self {
        code: "active",
        label: "今日已活跃",
        needs_action: false,
    };
    const CHECKIN_ONLY: Self = Self {
        code: "checkin_only",
        label: "签到已完成，今日未活跃",
        needs_action: true,
    };
    const CHECKIN_MISSING: Self = Self {
        code: "checkin_missing",
        label: "今日签到未执行",
        needs_action: true,
    };
    const STALE_STATS: Self = Self {
        code: "stale_stats",
        label: "今天有行为，活跃尚未落库（可能延迟）",
        needs_action: false,
    };
    const UNKNOWN: Self = Self {
        code: "unknown",
        label: "活跃状态未知",
        needs_action: false,
    };
    const NEEDS_AUTH: Self = Self {
        code: "needs_auth",
        label: "授权不可用",
        needs_action: true,
    };
    const UNSUPPORTED: Self = Self {
        code: "unsupported",
        label: "国际版没有成长中心",
        needs_action: false,
    };
}

// ---------------------------------------------------------------------------
// 纯解析：把两份响应变成可以单独测试的视图
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeatmapView {
    pub today: Option<String>,
    pub today_active: Option<bool>,
    pub today_score: Option<i64>,
    pub today_note: Option<String>,
    pub updated_at: Option<String>,
    pub last_active_date: Option<String>,
    /// 服务端今天（含）往前 7 天里有热度的天数。
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
        // 老形态：只回 `today` 字符串。
        .or_else(|| non_empty(today))
        // 兜底：`range.end_date` 就是服务端日历的最后一个格子。
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

/// 诊断。**顺序即优先级**，前几条相互排斥。
///
/// - `active`：服务端说今天亮了，其它都不用看；
/// - `checkin_missing`：签到记录缺失 —— 这是"签到没跑"，不是"活跃没做"；
/// - `stale_stats`：`updated_at` 落在服务端今天却没有 `is_active`，
///   多半是统计还没落库，提醒用户"稍后回查"而不是"快点去做"；
/// - `checkin_only`：签到在、活跃确实没有 —— 最常见的一种；
/// - `unknown`：没问到，**不当作未活跃**。
pub fn diagnose(
    today_active: Option<bool>,
    server_today: Option<&str>,
    updated_at: Option<&str>,
    checkin_status: Option<&str>,
) -> Diagnosis {
    match today_active {
        Some(true) => Diagnosis::ACTIVE,
        None => Diagnosis::UNKNOWN,
        Some(false) => {
            if checkin_status.is_none() {
                return Diagnosis::CHECKIN_MISSING;
            }
            let updated_today = match (updated_at, server_today) {
                (Some(updated), Some(today)) => updated.starts_with(today),
                _ => false,
            };
            if updated_today {
                Diagnosis::STALE_STATS
            } else {
                Diagnosis::CHECKIN_ONLY
            }
        }
    }
}

/// 从解析好的视图 + 本地签到记录拼出一个账号的结论。
///
/// 抽成纯函数是为了可以离网测试：跨日、未知、延迟四种情况都在这里断言。
pub fn assemble(
    account_id: &str,
    nickname: &str,
    heatmap: Option<&Value>,
    streak: Option<&Value>,
    checkin: Option<&crate::storage::checkin::CheckinRecord>,
) -> ActivityAccount {
    let heat = heatmap.map(parse_heatmap).unwrap_or_default();
    let strk = streak.map(parse_streak).unwrap_or_default();

    // 签到记录只有在"就是服务端今天"时才算数。拿昨天的 `already` 当今天签到，
    // 是最容易犯、也最难看出来的一个错。
    let checkin = checkin.map(|record| ActivityCheckin {
        status: record.status.clone(),
        checked_at: record.checked_at.clone(),
        local_date: record.local_date.clone(),
        source: record.execution_source.clone(),
    });
    let today_checkin = checkin
        .as_ref()
        .filter(|record| Some(record.local_date.as_str()) == heat.today.as_deref());

    let diagnosis = diagnose(
        heat.today_active,
        heat.today.as_deref(),
        heat.updated_at.as_deref(),
        today_checkin.map(|record| record.status.as_str()),
    );

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
        updated_at: heat.updated_at,
        checkin: today_checkin.cloned(),
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
        updated_at: None,
        checkin: None,
        diagnosis,
        errors: Vec::new(),
        fetched_at: now_rfc3339(),
    }
}

// ---------------------------------------------------------------------------
// 采集
// ---------------------------------------------------------------------------

/// 读一个账号的活跃状态。两个 GET，账号之间串行。
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

    // 签到记录按**服务端今天**取；服务端日期没拿到时干脆不取，避免把昨天的记录
    // 当成今天的结论。
    let server_today = heatmap_body
        .as_ref()
        .and_then(|body| parse_heatmap(body).today);
    let checkin = server_today.as_deref().and_then(|today| {
        app.with_db(|db| db.checkin_record(&account.id, today))
            .ok()
            .flatten()
    });

    let mut out = assemble(
        &account.id,
        &account.nickname,
        heatmap_body.as_ref(),
        streak_body.as_ref(),
        checkin.as_ref(),
    );
    if unauthorized {
        out.state = "unauthorized";
        out.diagnosis = Diagnosis::NEEDS_AUTH;
    }
    out.errors = errors;
    out
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
///
/// **账号列表读不到就返回错误**，不返回空数组：空数组在界面上意味着"你没有账号"。
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
    use serde_json::json;

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

    fn record(local_date: &str, status: &str) -> crate::storage::checkin::CheckinRecord {
        crate::storage::checkin::CheckinRecord {
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
            Some(&record("2026-11-17", "already")),
        );
        assert_eq!(out.diagnosis.code, "active");
        assert!(!out.diagnosis.needs_action);
        assert_eq!(out.streak_days, Some(17));
        assert_eq!(out.next_tier_remaining, Some(11));
    }

    #[test]
    fn checkin_without_activity_is_the_common_case() {
        let heat = heatmap(
            "2026-11-17",
            false,
            0,
            "2026-10-25T21:46:00+08:00",
            json!([]),
        );
        let out = assemble(
            "acc",
            "高亮",
            Some(&heat),
            Some(&streak(0, "7d", 7, 0)),
            Some(&record("2026-11-17", "already")),
        );
        assert_eq!(out.diagnosis.code, "checkin_only");
        assert!(out.diagnosis.needs_action);
        assert_eq!(out.today_active, Some(false));
    }

    /// 签到没跑和活跃没做是两件事，界面上的动作完全不同。
    #[test]
    fn a_missing_checkin_is_not_reported_as_inactivity() {
        let heat = heatmap(
            "2026-11-17",
            false,
            0,
            "2026-10-25T21:46:00+08:00",
            json!([]),
        );
        let out = assemble(
            "acc",
            "高亮",
            Some(&heat),
            Some(&streak(0, "7d", 7, 0)),
            None,
        );
        assert_eq!(out.diagnosis.code, "checkin_missing");
        assert!(out.checkin.is_none());
    }

    /// 昨天的 `already` 不能当今天的签到。
    #[test]
    fn yesterdays_checkin_does_not_count_for_today() {
        let heat = heatmap(
            "2026-11-17",
            false,
            0,
            "2026-10-25T21:46:00+08:00",
            json!([]),
        );
        let out = assemble(
            "acc",
            "高亮",
            Some(&heat),
            Some(&streak(0, "7d", 7, 0)),
            Some(&record("2026-11-16", "already")),
        );
        assert_eq!(out.diagnosis.code, "checkin_missing");
        assert!(out.checkin.is_none(), "跨日之后旧记录不再挂到账号上");
    }

    #[test]
    fn an_update_today_without_is_active_reads_as_delayed() {
        let heat = heatmap(
            "2026-11-17",
            false,
            0,
            "2026-11-17T21:00:00+08:00",
            json!([]),
        );
        let out = assemble(
            "acc",
            "卡里",
            Some(&heat),
            Some(&streak(3, "7d", 4, 3)),
            Some(&record("2026-11-17", "already")),
        );
        assert_eq!(out.diagnosis.code, "stale_stats");
        assert!(!out.diagnosis.needs_action, "延迟不该催用户马上做事");
    }

    /// 没问到 ≠ 没活跃。这条错了会催用户去重复做一件已经做完的事。
    #[test]
    fn a_missing_heatmap_is_unknown_not_inactive() {
        let out = assemble("acc", "卡里", None, Some(&streak(17, "28d", 11, 17)), None);
        assert_eq!(out.diagnosis.code, "unknown");
        assert_eq!(out.today_active, None);
        assert!(!out.diagnosis.needs_action);
        assert_eq!(out.state, "partial");
    }

    #[test]
    fn both_endpoints_missing_is_failed() {
        let out = assemble("acc", "卡里", None, None, None);
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
            Some(&record("2026-11-17", "already")),
        );
        assert_eq!(out.next_tier.as_deref(), Some("7d"));
        assert_eq!(out.next_tier_remaining, Some(6));
        assert_ne!(out.streak_days.unwrap_or(0), 6, "不能把剩余天数当连登天数");
    }

    #[test]
    fn active_days_in_the_last_week_follow_the_server_calendar() {
        let cells = json!([
            { "date": "2026-11-10", "score": 50 },   // 窗口外（今天 11-17，窗口 11-11..11-17）
            { "date": "2026-11-11", "score": 10 },
            { "date": "2026-11-12", "score": 0 },
            { "date": "2026-11-13", "score": 57 },
            { "date": "2026-11-14", "score": 0 },
            { "date": "2026-11-17", "score": 105 }
        ]);
        let heat = heatmap("2026-11-17", true, 105, "2026-11-17T22:00:00+08:00", cells);
        let out = assemble("acc", "卡里", Some(&heat), None, None);
        assert_eq!(out.active_days_7, Some(3));
        assert_eq!(out.last_active_date.as_deref(), Some("2026-11-17"));
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
            None,
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
            None,
        );
        let pending = assemble(
            "b",
            "乙",
            Some(&heatmap(
                "2026-11-17",
                false,
                0,
                "2026-10-01T00:00:00+08:00",
                json!([]),
            )),
            Some(&streak(0, "7d", 7, 0)),
            None,
        );
        let unknown = assemble("c", "丙", None, None, None);
        let summary = summarize(&[active, pending, unknown]);
        assert_eq!(summary.total, 3);
        assert_eq!(summary.active, 1);
        assert_eq!(summary.pending, 1);
        assert_eq!(summary.unknown, 1);
    }

    #[test]
    fn a_status_text_is_kept_verbatim() {
        let heat = heatmap(
            "2026-11-17",
            false,
            0,
            "2026-10-01T00:00:00+08:00",
            json!([]),
        );
        let out = assemble("acc", "高亮", Some(&heat), None, None);
        assert_eq!(out.today_note.as_deref(), Some("开始使用以点亮今日热力墙"));
        assert_eq!(out.today_score, Some(0));
    }
}
