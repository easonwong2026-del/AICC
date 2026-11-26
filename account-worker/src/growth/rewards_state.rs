//! 奖励状态（PR #96）：**只读**。把三档兑换、抽奖次数、补签卡与可补签日期
//! 查清楚，并把账本里"还没核验"的记录一起带出来。
//!
//! 这个模块**没有任何写接口**。抽奖 / 兑换 / 补签的真实 POST 要等 AICC 自己拿到
//! A 级接口证据才接线（`docs/growth-rewards-protocol-2026-11-16.md` §八）。
//!
//! ## 两个容易做错的地方
//!
//! 1. **档位状态有两个来源**：`/streak` 的 `redemption_status.tier_7d_status` 与
//!    `/redeem/summary` 的 `tier_7d_status` / `starter_status`。两边都见过，所以两边都认，
//!    但**空值不等于"可领"** —— 空 / 缺字段是"状态未知"，不能拿去换（88lin 就是踩了这个
//!    才出现"档位兑换永远不发生"和"对三档无脑 POST"两种反向故障）。
//! 2. **可补签日期不能来自 `makeup_dates`**：那是"已经补过"的历史。真正的断签来自
//!    `/heatmap` 的 `cells[]`（`score == 0`），并且要按服务端自己的 `today` 与
//!    `launch_date` 划边界，不能用本机时区。

use serde::Serialize;
use serde_json::Value;

use chrono::{Datelike, NaiveDate};

use super::api::{GrowthError, ReadOnlyEndpoints};
use super::models::as_i64;
use super::scanner::{dig_i64, FieldError};
#[cfg(test)]
use super::streak_rewards::LedgerPhase;
use super::streak_rewards::Tier;

/// 活动起始日：官方前端 `canMakeup` 写死的下限。
pub const ACTIVITY_START: (i32, u32, u32) = (2026, 6, 17);

/// 一张档位卡片。`state` 是界面唯一该看的东西，`status` 保留服务端原话便于排查。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TierState {
    pub tier: String,
    pub days: i64,
    /// 服务端原始状态字符串（可能为空）。
    pub status: Option<String>,
    /// `locked` / `claimable` / `claimed` / `ended` / `unknown`
    pub state: &'static str,
    /// 只有 `claimable` 才为真。#96 不提供"点一下就换"的入口，这里先给界面判断依据。
    pub claimable: bool,
}

impl TierState {
    pub fn unknown(tier: Tier) -> Self {
        Self {
            tier: tier.as_str().to_string(),
            days: tier.days(),
            status: None,
            state: "unknown",
            claimable: false,
        }
    }
}

/// 补签卡与可补签日期。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MakeupState {
    pub balance: Option<i64>,
    /// 官方上限（服务端给了就带上）。
    pub max: Option<i64>,
    /// 服务端自己的"今天"，来自 `/heatmap` 的 `today.date`。
    pub today: Option<String>,
    /// 可补签日期，最近的优先。**只有服务端确认过的断签日才会出现在这里。**
    pub candidates: Vec<String>,
    /// 为什么现在不能补签：`no_broken_day`（本来就没断）/ `no_cards`（确认 0 张）/
    /// `balance_unknown`（余额没查到，**不能说成没有卡**）/ `calendar_missing` / 各 `CalendarProblem`。
    pub note: Option<&'static str>,
}

/// 一条历史操作（来自本地账本，不是服务端）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OperationView {
    pub id: String,
    pub action: String,
    pub target: String,
    pub phase: String,
    pub attempt: i64,
    pub outcome: Option<String>,
    pub grant: Option<String>,
    pub detail: Option<String>,
    pub created_at: String,
    /// 这条记录是否还占着目标（`plan()` 会给回查或冻结）。
    pub open: bool,
}

/// 一个账号的奖励状态。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardState {
    pub account_id: String,
    pub nickname: String,
    /// `ok` / `partial` / `unauthorized` / `missing_credential` / `unsupported` / `failed`
    pub state: &'static str,
    pub streak_days: Option<i64>,
    pub tiers: Vec<TierState>,
    pub lottery_chances: Option<i64>,
    pub makeup: MakeupState,
    /// 账本里等回查的条数。**这是只读提示，不会自动收敛。**
    ///
    /// `None` 表示**本地账本读不到**（数据库打不开/查询失败）。这不是 0：
    /// 把"不知道"渲染成"没有未核验记录"，用户会以为一切都已经结算完。
    pub awaiting_verification: Option<usize>,
    pub recent_operations: Vec<OperationView>,
    pub errors: Vec<FieldError>,
    pub scanned_at: String,
    /// 恒为真：本 RPC 不发任何写请求。
    pub read_only: bool,
}

impl RewardState {
    pub fn empty(account_id: &str, nickname: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            nickname: nickname.to_string(),
            state: "failed",
            streak_days: None,
            tiers: Tier::ALL
                .iter()
                .map(|tier| TierState::unknown(*tier))
                .collect(),
            lottery_chances: None,
            makeup: MakeupState::default(),
            awaiting_verification: None,
            recent_operations: Vec::new(),
            errors: Vec::new(),
            scanned_at: crate::clock::now_rfc3339(),
            read_only: true,
        }
    }
}

// ---------------------------------------------------------------------------
// 档位
// ---------------------------------------------------------------------------

/// 把服务端的状态字符串翻成界面状态。**不认识就说不知道**，不默认"可领"。
pub fn tier_state_from(status: Option<&str>) -> &'static str {
    let Some(raw) = status.map(str::trim).filter(|text| !text.is_empty()) else {
        return "unknown";
    };
    match raw.to_lowercase().as_str() {
        "claimed" | "received" | "taken" => "claimed",
        "locked" | "not_reached" | "unreached" => "locked",
        "ended" | "expired" | "closed" | "finished" => "ended",
        // 只认这几个"可以换"的写法；别的一律 unknown —— 服务端哪天加个新状态，
        // 我们宁可不显示"可领"，也不能把不可领的档位画成可领。
        "available" | "ready" | "claimable" | "unlocked" | "can_claim" => "claimable",
        _ => "unknown",
    }
}

/// 从 `/streak` 与 `/redeem/summary` 两份响应里合成三档状态。
///
/// 取值顺序：先 `/streak` 的 `redemption_status.tier_<n>d_status`，再 `/redeem/summary`
/// 的 `tier_<n>d_status`，最后 `/redeem/summary` 的 `starter/advanced/legendary_status`。
/// 三个位置都被实测见过，先到先得，但**空字符串不算答案**。
pub fn tier_states(streak: Option<&Value>, summary: Option<&Value>) -> Vec<TierState> {
    let streak_node = streak.map(|value| value.get("data").unwrap_or(value));
    let summary_node = summary.map(|value| value.get("data").unwrap_or(value));

    Tier::ALL
        .iter()
        .map(|tier| {
            let keys = tier.status_keys();
            let raw = [
                streak_node
                    .and_then(|node| node.get("redemption_status"))
                    .and_then(|node| node.get(keys[0]))
                    .and_then(Value::as_str),
                summary_node
                    .and_then(|node| node.get(keys[0]))
                    .and_then(Value::as_str),
                summary_node
                    .and_then(|node| node.get(keys[1]))
                    .and_then(Value::as_str),
            ]
            .into_iter()
            .flatten()
            .map(str::trim)
            .find(|text| !text.is_empty());

            let state = tier_state_from(raw);
            TierState {
                tier: tier.as_str().to_string(),
                days: tier.days(),
                status: raw.map(|text| text.to_lowercase()),
                state,
                claimable: state == "claimable",
            }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 补签
// ---------------------------------------------------------------------------

/// 为什么算不出候选日期。每一种都要能对用户说清楚，不能静默返回空。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarProblem {
    /// `/heatmap` 没给 `today.date` —— 服务端日期是我们唯一的"今天"。
    MissingToday,
    /// `today` 不是 `YYYY-MM-DD`。
    BadToday,
    /// `/streak` 没给 `launch_date`。官方前端缺它时禁止补签。
    MissingLaunch,
    /// `launch_date` 不是 `YYYY-MM-DD`。
    BadLaunch,
    /// `streak.makeup_dates` 不是数组（已补签历史没法排除）。
    BadMakeupDates,
    /// `/heatmap` 没有 `cells[]`。
    MissingCells,
    /// 某个格子日期/分数非法，或同一天两个分数。
    BadCell,
}

impl CalendarProblem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MissingToday => "missing_today",
            Self::BadToday => "bad_today",
            Self::MissingLaunch => "missing_launch",
            Self::BadLaunch => "bad_launch",
            Self::BadMakeupDates => "bad_makeup_dates",
            Self::MissingCells => "missing_cells",
            Self::BadCell => "bad_cell",
        }
    }

    /// 给界面的一句话。**结构异常时整段停止**，不靠猜消耗补签卡。
    pub fn describe(self) -> &'static str {
        match self {
            Self::MissingToday | Self::BadToday => "活跃日历缺少可用的服务端日期",
            Self::MissingLaunch | Self::BadLaunch => "连登状态缺少活动上线日期，无法判断补签范围",
            Self::BadMakeupDates => "已补签记录格式异常",
            Self::MissingCells | Self::BadCell => "活跃日历格式异常",
        }
    }
}

fn parse_day(text: &str) -> Option<NaiveDate> {
    let text = text.trim();
    if text.len() != 10 {
        return None;
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

fn month_start(day: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(day.year(), day.month(), 1).unwrap_or(day)
}

fn activity_start() -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(ACTIVITY_START.0, ACTIVITY_START.1, ACTIVITY_START.2)
}

/// 算出可以补签的日期，最近的排前面。
///
/// 规则全部来自官方前端的 `canMakeup`（88lin 的 `_makeup_candidates` 是同一份）：
///
/// - `start = max(当月 1 号, launch_date, 2026-06-17)`；
/// - 只看 `start <= day < today`（服务端的今天，不含今天）；
/// - `score == 0` 才是断签；
/// - `makeup_dates` 里的日期是**已经补过**的，要排除；
/// - 日历里没出现的日期**不推测为断签**（缺项 ≠ 零分）。
pub fn makeup_candidates(streak: &Value, heatmap: &Value) -> Result<Vec<String>, CalendarProblem> {
    let streak = streak.get("data").unwrap_or(streak);
    let heatmap = heatmap.get("data").unwrap_or(heatmap);

    let today = heatmap
        .get("today")
        .and_then(|node| node.get("date").or(Some(node)))
        .and_then(Value::as_str)
        .ok_or(CalendarProblem::MissingToday)?;
    let today = parse_day(today).ok_or(CalendarProblem::BadToday)?;

    let launch = streak
        .get("launch_date")
        .and_then(Value::as_str)
        .ok_or(CalendarProblem::MissingLaunch)?;
    let launch = parse_day(launch).ok_or(CalendarProblem::BadLaunch)?;

    let mut start = month_start(today).max(launch);
    if let Some(activity) = activity_start() {
        start = start.max(activity);
    }

    // 已补签历史。缺字段按空数组处理（老账号可能还没补过），但类型错了要停 ——
    // 类型错意味着我们读错了结构，继续下去就是把已补的日期再提一次。
    let made_up: Vec<NaiveDate> = match streak
        .get("streak")
        .and_then(|node| node.get("makeup_dates"))
    {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .and_then(parse_day)
                    .ok_or(CalendarProblem::BadMakeupDates)
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err(CalendarProblem::BadMakeupDates),
    };

    let cells = heatmap
        .get("cells")
        .and_then(Value::as_array)
        .ok_or(CalendarProblem::MissingCells)?;

    let mut scores: Vec<(NaiveDate, i64)> = Vec::with_capacity(cells.len());
    for cell in cells {
        let day = cell
            .get("date")
            .or_else(|| cell.get("day"))
            .and_then(Value::as_str)
            .and_then(parse_day)
            .ok_or(CalendarProblem::BadCell)?;
        let score = as_i64(cell.get("score")).ok_or(CalendarProblem::BadCell)?;
        if score < 0 {
            return Err(CalendarProblem::BadCell);
        }
        if let Some((_, previous)) = scores.iter().find(|(existing, _)| *existing == day) {
            if *previous != score {
                // 同一天两个分数：说明我们读错了结构，停。
                return Err(CalendarProblem::BadCell);
            }
            continue;
        }
        scores.push((day, score));
    }

    let mut candidates: Vec<String> = scores
        .into_iter()
        .filter(|(day, score)| {
            *score == 0 && *day >= start && *day < today && !made_up.contains(day)
        })
        .map(|(day, _)| day.format("%Y-%m-%d").to_string())
        .collect();
    candidates.sort_by(|a, b| b.cmp(a));
    Ok(candidates)
}

/// 补签卡余额。`{"balance": n}` 与裸数字两种形状都认。
pub fn makeup_balance(streak: &Value) -> (Option<i64>, Option<i64>) {
    let node = streak.get("data").unwrap_or(streak);
    let Some(cards) = node.get("makeup_cards") else {
        return (None, None);
    };
    if let Some(balance) = dig_i64(Some(cards), &[&["balance"], &["count"]]) {
        let max = dig_i64(Some(cards), &[&["max"], &["max_balance"]]);
        return (Some(balance), max);
    }
    (as_i64(Some(cards)), None)
}

/// 把三份响应合成补签视图。
pub fn makeup_state(
    streak: Option<&Value>,
    heatmap: Option<&Value>,
) -> (MakeupState, Option<CalendarProblem>) {
    let mut state = MakeupState::default();
    if let Some(streak) = streak {
        let (balance, max) = makeup_balance(streak);
        state.balance = balance;
        state.max = max;
    }
    if let Some(heatmap) = heatmap {
        let node = heatmap.get("data").unwrap_or(heatmap);
        state.today = node
            .get("today")
            .and_then(|today| today.get("date").or(Some(today)))
            .and_then(Value::as_str)
            .map(str::to_string);
    }

    match (streak, heatmap) {
        (Some(streak), Some(heatmap)) => match makeup_candidates(streak, heatmap) {
            Ok(days) => {
                if days.is_empty() {
                    state.note = Some("no_broken_day");
                } else {
                    // 余额未知 ≠ 没有卡。把 None 当 0 会让界面写着"没有补签卡"，
                    // 而用户其实还有卡 —— 那是错误结论，不是保守结论。
                    state.note = match state.balance {
                        Some(balance) if balance <= 0 => Some("no_cards"),
                        Some(_) => None,
                        None => Some("balance_unknown"),
                    };
                }
                state.candidates = days;
            }
            Err(problem) => {
                state.note = Some(problem.as_str());
                return (state, Some(problem));
            }
        },
        _ => {
            state.note = Some("calendar_missing");
        }
    }
    (state, None)
}

// ---------------------------------------------------------------------------
// 从线上把状态取回来
// ---------------------------------------------------------------------------

fn push_error(errors: &mut Vec<FieldError>, field: &'static str, err: &GrowthError) {
    errors.push(FieldError {
        field,
        kind: err.kind.as_str().to_string(),
        http: err.http,
        code: err.code,
        message: err.message.clone(),
    });
}

/// 查一个账号。`access` / `uid` / `domain` 由调用方从 `auth::broker` 取。
pub async fn fetch(access: &str, uid: &str, domain: &str) -> (RewardState, bool) {
    let endpoints = ReadOnlyEndpoints::new(access, uid, domain);
    let (streak, summary, lottery, heatmap) = tokio::join!(
        endpoints.streak(),
        endpoints.redeem_summary(),
        endpoints.lottery_chances(),
        endpoints.heatmap(),
    );

    let mut state = RewardState::empty("", "");
    let mut errors = Vec::new();
    let mut unauthorized = false;

    fn note(
        errors: &mut Vec<FieldError>,
        unauthorized: &mut bool,
        field: &'static str,
        err: &GrowthError,
    ) {
        if err.kind == super::api::ErrorKind::Unauthorized {
            *unauthorized = true;
        }
        push_error(errors, field, err);
    }

    match &streak {
        Ok((value, _)) => {
            let node = value.get("data").unwrap_or(value);
            state.streak_days = dig_i64(Some(node), &[&["streak", "days"]])
                .or_else(|| dig_i64(Some(node), &[&["streak_days"], &["days"]]));
        }
        Err(err) => note(&mut errors, &mut unauthorized, "streak", err),
    }

    state.tiers = tier_states(
        streak.as_ref().ok().map(|(value, _)| value),
        summary.as_ref().ok().map(|(value, _)| value),
    );

    match &lottery {
        Ok((value, _)) => {
            let node = value.get("data").unwrap_or(value);
            state.lottery_chances = dig_i64(Some(node), &[&["balance"], &["chances"], &["count"]]);
        }
        Err(err) => note(&mut errors, &mut unauthorized, "lottery", err),
    }

    let (makeup, problem) = makeup_state(
        streak.as_ref().ok().map(|(value, _)| value),
        heatmap.as_ref().ok().map(|(value, _)| value),
    );
    state.makeup = makeup;
    if let Some(problem) = problem {
        errors.push(FieldError {
            field: "heatmap",
            kind: "calendar_invalid".to_string(),
            http: None,
            code: None,
            message: problem.describe().to_string(),
        });
    }
    if let Err(err) = &heatmap {
        note(&mut errors, &mut unauthorized, "heatmap", err);
    }
    if let Err(err) = &summary {
        // summary 只用来补档位状态，缺了不算致命：streak 那边通常也有。
        push_error(&mut errors, "redeem", err);
    }

    state.state = if unauthorized {
        "unauthorized"
    } else if errors.is_empty() {
        "ok"
    } else if state.tiers.iter().any(|tier| tier.state != "unknown") {
        "partial"
    } else {
        "failed"
    };
    state.errors = errors;
    state.scanned_at = crate::clock::now_rfc3339();
    (state, unauthorized)
}

/// 账本里"还占着目标"的记录必须是可解释的：界面靠它告诉用户"有一条没核验完"。
pub fn operation_view(op: &crate::storage::reward_ops::RewardOp) -> OperationView {
    OperationView {
        id: op.id.clone(),
        action: op.action.clone(),
        target: op.target.clone(),
        phase: op.phase.clone(),
        attempt: op.attempt,
        outcome: op.outcome.clone(),
        grant: op.grant_json.clone(),
        detail: op.detail.clone(),
        created_at: op.created_at.clone(),
        open: matches!(
            op.phase.as_str(),
            "pending" | "ready" | "sent" | "unknown" | "frozen"
        ),
    }
}

/// 这条记录下一步会变成什么（只读提示，不执行）。
#[cfg(test)]
pub fn phase_hint(phase: &str) -> &'static str {
    if phase == LedgerPhase::Unknown.as_str() {
        "等待回查服务端状态"
    } else if phase == LedgerPhase::Frozen.as_str() {
        "已冻结，需要人工核对"
    } else if phase == LedgerPhase::Ready.as_str() {
        "服务端上次没执行，可重发（需人工确认）"
    } else if phase == LedgerPhase::Confirmed.as_str() {
        "已到账"
    } else if phase == LedgerPhase::Rejected.as_str() {
        "服务端已拒绝或本就已领"
    } else {
        "待发送"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn heatmap(today: &str, cells: Value) -> Value {
        json!({ "today": { "date": today }, "cells": cells })
    }

    fn streak(launch: &str, made_up: Value) -> Value {
        json!({ "launch_date": launch, "streak": { "days": 3, "makeup_dates": made_up },
                "makeup_cards": { "balance": 2, "max": 4 } })
    }

    // --- 档位 -----------------------------------------------------------

    #[test]
    fn tier_state_never_defaults_to_claimable() {
        assert_eq!(tier_state_from(None), "unknown");
        assert_eq!(tier_state_from(Some("")), "unknown");
        assert_eq!(tier_state_from(Some("something_new")), "unknown");
        assert_eq!(tier_state_from(Some("claimed")), "claimed");
        assert_eq!(tier_state_from(Some("LOCKED")), "locked");
        assert_eq!(tier_state_from(Some("Available")), "claimable");
        assert_eq!(tier_state_from(Some("ended")), "ended");
    }

    #[test]
    fn tier_states_prefer_the_streak_redemption_status() {
        let streak = json!({ "redemption_status": { "tier_7d_status": "available" } });
        let summary = json!({ "tier_7d_status": "locked", "starter_status": "locked" });
        let tiers = tier_states(Some(&streak), Some(&summary));
        assert_eq!(tiers[0].state, "claimable", "streak 那份先到先得");
        assert_eq!(tiers[1].state, "unknown", "没给就是未知，不许猜可领");
        assert!(!tiers[1].claimable);
        assert_eq!(tiers[2].state, "unknown");
    }

    #[test]
    fn tier_states_fall_back_to_the_summary_spellings() {
        let summary = json!({
            "tier_14d_status": "claimed",
            "legendary_status": "available"
        });
        let tiers = tier_states(None, Some(&summary));
        assert_eq!(tiers[1].state, "claimed");
        assert_eq!(tiers[2].state, "claimable");
        assert_eq!(tiers[0].state, "unknown");
    }

    #[test]
    fn an_empty_streak_status_does_not_block_the_summary_value() {
        // 空字符串不算答案：要继续往后找。
        let streak = json!({ "redemption_status": { "tier_7d_status": "  " } });
        let summary = json!({ "tier_7d_status": "available" });
        assert_eq!(
            tier_states(Some(&streak), Some(&summary))[0].state,
            "claimable"
        );
    }

    #[test]
    fn every_tier_is_present_even_when_both_responses_are_missing() {
        let tiers = tier_states(None, None);
        assert_eq!(tiers.len(), 3);
        assert_eq!(
            tiers.iter().map(|t| t.days).collect::<Vec<_>>(),
            vec![7, 14, 28]
        );
        assert!(tiers.iter().all(|tier| !tier.claimable));
    }

    // --- 补签候选 -------------------------------------------------------

    #[test]
    fn a_zero_score_day_inside_the_window_is_a_candidate() {
        let streak = streak("2026-06-17", json!([]));
        let heatmap = heatmap(
            "2026-11-16",
            json!([
                { "date": "2026-11-15", "score": 0 },
                { "date": "2026-11-14", "score": 3 },
                { "date": "2026-11-10", "score": 0 }
            ]),
        );
        assert_eq!(
            makeup_candidates(&streak, &heatmap).unwrap(),
            vec!["2026-11-15", "2026-11-10"],
            "最近的排前面"
        );
    }

    #[test]
    fn already_made_up_dates_are_excluded_because_makeup_dates_is_history() {
        let streak = streak("2026-06-17", json!(["2026-11-15"]));
        let heatmap = heatmap(
            "2026-11-16",
            json!([
                { "date": "2026-11-15", "score": 0 },
                { "date": "2026-11-10", "score": 0 }
            ]),
        );
        assert_eq!(
            makeup_candidates(&streak, &heatmap).unwrap(),
            vec!["2026-11-10"]
        );
    }

    #[test]
    fn today_and_other_months_are_out_of_range() {
        let streak = streak("2026-06-17", json!([]));
        let heatmap = heatmap(
            "2026-11-16",
            json!([
                { "date": "2026-11-16", "score": 0 },
                { "date": "2026-10-31", "score": 0 },
                { "date": "2026-11-01", "score": 0 }
            ]),
        );
        assert_eq!(
            makeup_candidates(&streak, &heatmap).unwrap(),
            vec!["2026-11-01"],
            "不含今天，也不补上个月"
        );
    }

    #[test]
    fn the_window_never_starts_before_the_activity_launch() {
        // 活动 6-17 才开始：6 月初的零分不是断签。
        let streak = streak("2026-06-17", json!([]));
        let heatmap = heatmap(
            "2026-06-20",
            json!([
                { "date": "2026-06-05", "score": 0 },
                { "date": "2026-06-18", "score": 0 }
            ]),
        );
        assert_eq!(
            makeup_candidates(&streak, &heatmap).unwrap(),
            vec!["2026-06-18"]
        );
    }

    #[test]
    fn a_date_missing_from_the_calendar_is_not_assumed_broken() {
        let streak = streak("2026-06-17", json!([]));
        let heatmap = heatmap("2026-11-16", json!([{ "date": "2026-11-15", "score": 0 }]));
        let days = makeup_candidates(&streak, &heatmap).unwrap();
        assert_eq!(days, vec!["2026-11-15"], "没出现的日期不能当成断签");
    }

    #[test]
    fn a_broken_calendar_stops_instead_of_guessing() {
        let ok_streak = streak("2026-06-17", json!([]));

        // 没有服务端今天
        assert_eq!(
            makeup_candidates(&ok_streak, &json!({ "cells": [] })),
            Err(CalendarProblem::MissingToday)
        );
        // 没有上线日期
        assert_eq!(
            makeup_candidates(&json!({ "streak": {} }), &heatmap("2026-11-16", json!([]))),
            Err(CalendarProblem::MissingLaunch)
        );
        // makeup_dates 不是数组
        assert_eq!(
            makeup_candidates(
                &json!({ "launch_date": "2026-06-17", "streak": { "makeup_dates": 3 } }),
                &heatmap("2026-11-16", json!([]))
            ),
            Err(CalendarProblem::BadMakeupDates)
        );
        // 没有 cells
        assert_eq!(
            makeup_candidates(&ok_streak, &json!({ "today": { "date": "2026-11-16" } })),
            Err(CalendarProblem::MissingCells)
        );
        // 分数非法
        assert_eq!(
            makeup_candidates(
                &ok_streak,
                &heatmap("2026-11-16", json!([{ "date": "2026-11-15", "score": -1 }]))
            ),
            Err(CalendarProblem::BadCell)
        );
        // 同一天两个分数
        assert_eq!(
            makeup_candidates(
                &ok_streak,
                &heatmap(
                    "2026-11-16",
                    json!([
                        { "date": "2026-11-15", "score": 0 },
                        { "date": "2026-11-15", "score": 2 }
                    ])
                )
            ),
            Err(CalendarProblem::BadCell)
        );
    }

    #[test]
    fn makeup_balance_accepts_both_shapes() {
        assert_eq!(
            makeup_balance(&json!({ "makeup_cards": { "balance": 2, "max": 4 } })),
            (Some(2), Some(4))
        );
        assert_eq!(
            makeup_balance(&json!({ "makeup_cards": 3 })),
            (Some(3), None)
        );
        assert_eq!(makeup_balance(&json!({})), (None, None));
    }

    #[test]
    fn makeup_state_explains_why_there_is_nothing_to_do() {
        let streak = streak("2026-06-17", json!([]));
        let flat = heatmap("2026-11-16", json!([{ "date": "2026-11-14", "score": 5 }]));
        let (state, problem) = makeup_state(Some(&streak), Some(&flat));
        assert_eq!(problem, None);
        assert!(state.candidates.is_empty());
        assert_eq!(state.note, Some("no_broken_day"));
        assert_eq!(state.balance, Some(2));

        // 有断签但没卡
        let broken = heatmap("2026-11-16", json!([{ "date": "2026-11-15", "score": 0 }]));

        let no_cards = json!({ "launch_date": "2026-06-17", "streak": { "makeup_dates": [] },
                               "makeup_cards": { "balance": 0 } });
        let (state, _) = makeup_state(Some(&no_cards), Some(&broken));
        assert_eq!(state.candidates.len(), 1);
        assert_eq!(state.note, Some("no_cards"));

        // 有断签但**余额未知**：不许说成"没有卡"（P1 回归）
        let unknown_cards =
            json!({ "launch_date": "2026-06-17", "streak": { "makeup_dates": [] } });
        let (state, _) = makeup_state(Some(&unknown_cards), Some(&broken));
        assert_eq!(state.balance, None);
        assert_eq!(state.candidates.len(), 1);
        assert_eq!(
            state.note,
            Some("balance_unknown"),
            "余额查不到时必须单独一类，不能报成 no_cards"
        );

        // 还有卡：不报任何"不能补"的理由
        let with_cards = json!({ "launch_date": "2026-06-17", "streak": { "makeup_dates": [] },
                                 "makeup_cards": { "balance": 2 } });
        let (state, _) = makeup_state(Some(&with_cards), Some(&broken));
        assert_eq!(state.note, None);

        // 日历整段缺失
        let (state, _) = makeup_state(Some(&streak), None);
        assert_eq!(state.note, Some("calendar_missing"));
    }

    #[test]
    fn a_broken_calendar_is_reported_as_an_error_and_keeps_the_balance() {
        let streak = streak("2026-06-17", json!([]));
        let (state, problem) = makeup_state(Some(&streak), Some(&json!({ "cells": [] })));
        assert_eq!(problem, Some(CalendarProblem::MissingToday));
        assert_eq!(state.note, Some("missing_today"));
        assert_eq!(state.balance, Some(2), "余额还能显示，只是不能算候选");
        assert!(state.candidates.is_empty());
    }

    #[test]
    fn phase_hints_cover_every_ledger_phase() {
        for phase in [
            "pending",
            "ready",
            "sent",
            "confirmed",
            "rejected",
            "unknown",
            "frozen",
        ] {
            assert!(!phase_hint(phase).is_empty(), "{phase}");
        }
        assert_eq!(phase_hint("frozen"), "已冻结，需要人工核对");
    }
}
