//! 账号级只读扫描（PR #67）。
//!
//! 一次扫描 = 9 个 GET + 归一化 + 分类，**不含任何写操作**。
//!
//! 设计上的三个决定：
//!
//! 1. **单个接口挂了不影响其它接口**。成长中心的接口彼此独立，某个 403（例如
//!    "连登天数不足"）不该让任务列表也变成空的，否则界面会误导人。
//! 2. **401 早退**。一份失效的令牌去问 9 个接口，只会拿到 9 条 401，还把
//!    钥匙串读 9 次。第一个 401 就把整轮标成 `unauthorized` 返回。
//! 3. **账号之间串行**。同一个进程同时打 4 个账号的快照是安全的，但 PRD 要求
//!    账号级串行，而读也顺带占用了令牌 —— 保持一致的顺序比省几秒重要。

use serde::Serialize;
use serde_json::Value;

use crate::clock::now_rfc3339;

use super::api::{ErrorKind, GrowthError, ReadOnlyEndpoints};
use super::catalog::{self, TaskClass};
use super::models::{as_i64, locate_tasks, GrowthProfile, GrowthTask, RedeemTier};

/// 界面上的一个任务：归一化后的数据 + 分类 + 分类理由。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskView {
    #[serde(flatten)]
    pub task: GrowthTask,
    pub class: TaskClass,
    pub class_reason: &'static str,
}

/// 一个账号的扫描结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountScan {
    pub account_id: String,
    pub nickname: String,
    /// 这次实际生效的路径前缀（`/v2/activity/growth` 或 `/activity/growth`）。
    pub path_prefix: Option<String>,
    /// `ok` / `unauthorized` / `partial` / `failed`
    pub state: &'static str,
    pub tasks: Vec<TaskView>,
    pub profile: GrowthProfile,
    pub travel: Option<TravelView>,
    /// 逐接口的错误。**不含令牌**。
    pub errors: Vec<FieldError>,
    pub scanned_at: String,
    /// 分类统计，界面顶部直接用。
    pub counts: Counts,
    /// 这次用的是哪套授权（`magpie` / `own`），由调用方填。
    pub auth_source: Option<String>,
    /// 首选来源不可用、这次用了兜底时的原因。
    pub degraded: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FieldError {
    pub field: &'static str,
    pub kind: String,
    pub http: Option<i64>,
    pub code: Option<i64>,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TravelView {
    pub state: Option<String>,
    pub location: Option<String>,
    pub record_id: Option<i64>,
    pub arrive_at: Option<String>,
    pub daily_limit_reached: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    pub total: usize,
    pub claimable: usize,
    pub acceptable: usize,
    pub in_progress: usize,
    pub locked: usize,
    pub automatic: usize,
    pub real_action: usize,
    pub manual: usize,
    pub unknown: usize,
}

/// 从任意深度的对象里按候选路径取整数。
///
/// 服务端字段名不稳定（`credit` / `total_credit` / `credits` 都见过），
/// 与其逐接口写分支，不如把候选列清楚。
fn dig_i64(data: Option<&Value>, paths: &[&[&str]]) -> Option<i64> {
    let data = data?;
    for path in paths {
        let mut cursor = data;
        let mut ok = true;
        for segment in *path {
            match cursor.get(*segment) {
                Some(next) => cursor = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        if let Some(n) = as_i64(Some(cursor)) {
            return Some(n);
        }
    }
    None
}

fn dig_str(data: Option<&Value>, paths: &[&[&str]]) -> Option<String> {
    let data = data?;
    for path in paths {
        let mut cursor = data;
        let mut ok = true;
        for segment in *path {
            match cursor.get(*segment) {
                Some(next) => cursor = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        if let Some(text) = cursor.as_str() {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

/// `makeup_cards` 可能是 `{"balance": n}`，也可能直接是数字。两种都认。
pub fn read_makeup_cards(streak: Option<&Value>) -> Option<i64> {
    let cards = streak?.get("makeup_cards")?;
    as_i64(Some(cards)).or_else(|| dig_i64(Some(cards), &[&["balance"], &["count"]]))
}

/// 连登兑换档位。服务端用的是 starter / advanced / legendary 三个名字，
/// 对应 7 / 14 / 28 天；这里翻译成天数，界面才不用背这三个词。
pub fn read_redeem_tiers(summary: Option<&Value>) -> Vec<RedeemTier> {
    const TIERS: [(&str, &str); 3] = [
        ("7d", "starter_status"),
        ("14d", "advanced_status"),
        ("28d", "legendary_status"),
    ];
    let Some(summary) = summary else {
        return Vec::new();
    };
    TIERS
        .iter()
        .filter_map(|(tier, key)| {
            let status = summary.get(*key)?.as_str()?.trim().to_lowercase();
            if status.is_empty() {
                return None;
            }
            // 只有这两个值表示"不能换"；其余（含 ready / available）都算可换，
            // 但**能不能真的换**由 #68 在写之前再确认一次。
            let claimable = !matches!(status.as_str(), "claimed" | "locked");
            Some(RedeemTier {
                tier: (*tier).to_string(),
                status,
                claimable,
            })
        })
        .collect()
}

/// 旅行状态归一化。`state` 只用来看"能不能开盲盒"，不改任何东西。
pub fn read_travel(response: Option<&Value>) -> Option<TravelView> {
    let data = response?;
    // 有的响应把内容放在 data 下
    let node = data.get("data").unwrap_or(data);
    let state = dig_str(Some(node), &[&["state"], &["travel_state"], &["status"]]);
    if state.is_none() && node.get("record_id").is_none() {
        return None;
    }
    Some(TravelView {
        state,
        location: dig_str(Some(node), &[&["location", "name"], &["location_name"]]),
        record_id: dig_i64(
            Some(node),
            &[&["record_id"], &["recordId"], &["travel", "record_id"]],
        ),
        arrive_at: dig_str(Some(node), &[&["arrive_at"], &["arriveAt"]]),
        daily_limit_reached: node
            .get("daily_limit_reached")
            .and_then(Value::as_bool)
            .or_else(|| {
                node.get("daily_limit")
                    .and_then(|v| v.get("reached"))
                    .and_then(Value::as_bool)
            }),
    })
}

fn push_error(errors: &mut Vec<FieldError>, field: &'static str, err: &GrowthError) {
    errors.push(FieldError {
        field,
        kind: err.kind.as_str().to_string(),
        http: err.http,
        code: err.code,
        message: err.message.clone(),
    });
}

/// 扫一个账号。
///
/// 入参已经解析好令牌，函数本身不碰钥匙串、不碰数据库 —— 这样它能被单测直接驱动。
pub async fn scan(
    account_id: &str,
    nickname: &str,
    access: &str,
    uid: &str,
    domain: &str,
) -> AccountScan {
    let api = ReadOnlyEndpoints::new(access, uid, domain);

    let (tasks, profile, energy, streak, heatmap, redeem, lottery, quota, travel) = tokio::join!(
        api.tasks(),
        api.profile(),
        api.energy(),
        api.streak(),
        api.heatmap(),
        api.redeem_summary(),
        api.lottery_chances(),
        api.buddy_quota(),
        api.buddy_travel_status(),
    );

    let mut errors = Vec::new();
    let mut path_prefix: Option<String> = None;
    let mut unauthorized = false;

    // 先看任务：它是这个模块存在的理由，也最可能第一个暴露授权问题。
    let mut views = Vec::new();
    match &tasks {
        Ok((response, prefix)) => {
            path_prefix = Some((*prefix).to_string());
            let list = locate_tasks(response);
            for item in list.map(Vec::as_slice).unwrap_or_default() {
                if let Some(task) = super::models::parse_task(item) {
                    let class = catalog::classify(&task);
                    views.push(TaskView {
                        class,
                        class_reason: catalog::explain(class),
                        task,
                    });
                }
            }
        }
        Err(err) => {
            push_error(&mut errors, "tasks", err);
            unauthorized |= err.kind == ErrorKind::Unauthorized;
        }
    }

    // 401 之后不要再问剩下的接口：答案是确定的，只会多 8 条同样的错误。
    if unauthorized {
        return AccountScan {
            account_id: account_id.to_string(),
            nickname: nickname.to_string(),
            path_prefix,
            state: "unauthorized",
            tasks: Vec::new(),
            profile: GrowthProfile::default(),
            travel: None,
            errors,
            scanned_at: now_rfc3339(),
            counts: Counts::default(),
            auth_source: None,
            degraded: None,
        };
    }

    let mut profile_data = GrowthProfile::default();

    match &profile {
        Ok((response, prefix)) => {
            path_prefix.get_or_insert_with(|| (*prefix).to_string());
            let node = response.get("data").unwrap_or(response);
            profile_data.credit = dig_i64(
                Some(node),
                &[
                    &["credit"],
                    &["credits"],
                    &["total_credit"],
                    &["total_credits"],
                ],
            );
            profile_data.energy = dig_i64(Some(node), &[&["energy"], &["energy_balance"]]);
            profile_data.streak_days =
                dig_i64(Some(node), &[&["streak_days"], &["streak", "days"]]);
        }
        Err(err) => push_error(&mut errors, "profile", err),
    }

    match &energy {
        Ok((response, prefix)) => {
            path_prefix.get_or_insert_with(|| (*prefix).to_string());
            let node = response.get("data").unwrap_or(response);
            profile_data.energy = profile_data
                .energy
                .or_else(|| dig_i64(Some(node), &[&["balance"], &["energy"]]));
            // Buddy 能量配额单独一个接口，用来算能开几个盲盒。
            if let Ok((quota_response, _)) = &quota {
                let qnode = quota_response.get("data").unwrap_or(quota_response);
                if profile_data.energy.is_none() {
                    profile_data.energy =
                        dig_i64(Some(qnode), &[&["energy"], &["balance"], &["available"]]);
                }
            }
        }
        Err(err) => push_error(&mut errors, "energy", err),
    }

    match &streak {
        Ok((response, prefix)) => {
            path_prefix.get_or_insert_with(|| (*prefix).to_string());
            let node = response.get("data").unwrap_or(response);
            profile_data.streak_days = profile_data.streak_days.or_else(|| {
                dig_i64(
                    Some(node),
                    &[&["streak", "days"], &["streak_days"], &["days"]],
                )
            });
            profile_data.makeup_cards = read_makeup_cards(Some(node));
        }
        Err(err) => push_error(&mut errors, "streak", err),
    }

    match &heatmap {
        Ok((response, prefix)) => {
            path_prefix.get_or_insert_with(|| (*prefix).to_string());
            let node = response.get("data").unwrap_or(response);
            profile_data.heatmap_today = dig_str(Some(node), &[&["today", "date"], &["today"]]);
        }
        Err(err) => push_error(&mut errors, "heatmap", err),
    }

    match &redeem {
        Ok((response, prefix)) => {
            path_prefix.get_or_insert_with(|| (*prefix).to_string());
            let node = response.get("data").unwrap_or(response);
            profile_data.redeem = read_redeem_tiers(Some(node));
        }
        Err(err) => push_error(&mut errors, "redeem", err),
    }

    match &lottery {
        Ok((response, prefix)) => {
            path_prefix.get_or_insert_with(|| (*prefix).to_string());
            let node = response.get("data").unwrap_or(response);
            profile_data.lottery_chances =
                dig_i64(Some(node), &[&["balance"], &["chances"], &["count"]]);
        }
        Err(err) => push_error(&mut errors, "lottery", err),
    }

    if let Err(err) = &quota {
        // 配额查不到不影响任何展示字段，只在 energy 也没拿到时才有意义；
        // 仍然记下来，方便排查"界面为什么显示 —"。
        push_error(&mut errors, "buddyQuota", err);
    }

    let travel_view = match &travel {
        Ok((response, prefix)) => {
            path_prefix.get_or_insert_with(|| (*prefix).to_string());
            read_travel(Some(response))
        }
        Err(err) => {
            push_error(&mut errors, "buddyTravel", err);
            None
        }
    };

    let counts = count_views(&views);
    let state = if errors.is_empty() {
        "ok"
    } else if views.is_empty() && errors.len() > 1 {
        "failed"
    } else {
        "partial"
    };

    AccountScan {
        account_id: account_id.to_string(),
        nickname: nickname.to_string(),
        path_prefix,
        state,
        tasks: views,
        profile: profile_data,
        travel: travel_view,
        errors,
        scanned_at: now_rfc3339(),
        counts,
        auth_source: None,
        degraded: None,
    }
}

pub fn count_views(views: &[TaskView]) -> Counts {
    let mut counts = Counts {
        total: views.len(),
        ..Default::default()
    };
    for view in views {
        if view.task.claimable() {
            counts.claimable += 1;
        }
        if view.task.acceptable() {
            counts.acceptable += 1;
        }
        if view.task.locked {
            counts.locked += 1;
        }
        if view.task.status == super::models::TaskStatus::InProgress {
            counts.in_progress += 1;
        }
        match view.class {
            TaskClass::Automatic => counts.automatic += 1,
            TaskClass::RealAction => counts.real_action += 1,
            TaskClass::Manual => counts.manual += 1,
            TaskClass::Unknown => counts.unknown += 1,
        }
    }
    counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn makeup_cards_accepts_object_and_bare_number() {
        assert_eq!(
            read_makeup_cards(Some(&json!({"makeup_cards": {"balance": 2}}))),
            Some(2)
        );
        assert_eq!(
            read_makeup_cards(Some(&json!({"makeup_cards": 3}))),
            Some(3)
        );
        assert_eq!(read_makeup_cards(Some(&json!({}))), None);
        assert_eq!(read_makeup_cards(None), None);
    }

    #[test]
    fn redeem_tiers_map_to_days_and_mark_claimable() {
        let summary = json!({
            "starter_status": "available",
            "advanced_status": "claimed",
            "legendary_status": "locked",
        });
        let tiers = read_redeem_tiers(Some(&summary));
        assert_eq!(tiers.len(), 3);
        assert_eq!(tiers[0].tier, "7d");
        assert!(tiers[0].claimable);
        assert!(!tiers[1].claimable, "已领取的档位不能再兑");
        assert!(!tiers[2].claimable, "未解锁的档位不能兑");
    }

    #[test]
    fn missing_redeem_summary_yields_no_tiers() {
        assert!(read_redeem_tiers(None).is_empty());
        assert!(read_redeem_tiers(Some(&json!({}))).is_empty());
    }

    #[test]
    fn travel_view_handles_idle_and_missing() {
        let idle = json!({"state": "idle", "daily_limit_reached": false});
        let view = read_travel(Some(&idle)).expect("view");
        assert_eq!(view.state.as_deref(), Some("idle"));
        assert_eq!(view.daily_limit_reached, Some(false));

        assert!(
            read_travel(Some(&json!({}))).is_none(),
            "没有状态就不编一个出来"
        );
        assert!(read_travel(None).is_none());
    }

    #[test]
    fn travel_view_reads_a_wrapped_payload() {
        let wrapped =
            json!({"data": {"state": "arrived", "record_id": 42, "location": {"name": "咖啡馆"}}});
        let view = read_travel(Some(&wrapped)).expect("view");
        assert_eq!(view.state.as_deref(), Some("arrived"));
        assert_eq!(view.record_id, Some(42));
        assert_eq!(view.location.as_deref(), Some("咖啡馆"));
    }

    fn view(code: &str, status: super::super::models::TaskStatus, locked: bool) -> TaskView {
        let task = GrowthTask {
            code: code.to_string(),
            title: code.to_string(),
            description: None,
            condition: None,
            task_type: None,
            tag: None,
            jump_url: None,
            locked,
            current: 0,
            target: 1,
            reward_credit: 10,
            reward_energy: 0,
            status,
            raw_status: None,
        };
        let class = catalog::classify(&task);
        TaskView {
            task,
            class,
            class_reason: catalog::explain(class),
        }
    }

    #[test]
    fn counts_separate_claimable_from_acceptable() {
        use super::super::models::TaskStatus;
        let views = vec![
            view("a", TaskStatus::Completed, false),
            view("b", TaskStatus::NotAccepted, false),
            view("c", TaskStatus::NotAccepted, true),
            view("d", TaskStatus::Claimed, false),
            view("e", TaskStatus::InProgress, false),
        ];
        let counts = count_views(&views);
        assert_eq!(counts.total, 5);
        assert_eq!(counts.claimable, 1, "只有 completed 能领");
        assert_eq!(counts.acceptable, 1, "锁定的未接受任务不能接");
        assert_eq!(counts.locked, 1);
        assert_eq!(counts.in_progress, 1);
    }

    #[test]
    fn dig_walks_the_candidate_paths() {
        let data = json!({"a": {"b": 7}, "c": "9", "d": {"e": "text"}});
        assert_eq!(dig_i64(Some(&data), &[&["missing"], &["a", "b"]]), Some(7));
        assert_eq!(dig_i64(Some(&data), &[&["c"]]), Some(9));
        assert_eq!(
            dig_str(Some(&data), &[&["d", "e"]]).as_deref(),
            Some("text")
        );
        assert_eq!(dig_i64(Some(&data), &[&["zzz"]]), None);
    }
}
