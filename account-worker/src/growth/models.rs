//! 成长中心的数据形状（PR #67 只读阶段）。
//!
//! 服务端是 snake_case、字段经常缺；界面要的是稳定结构。所以这里只做一件事：
//! **把一份可能缺字段的 JSON 归一化成一定能渲染的结构**，缺什么补什么，
//! 猜不出来的标 `Unknown` —— 绝不用默认值假装"已完成"。
//!
//! 关键约定：`accept_status` 与 `status` 是两个不同字段。前者是"我报没报名"，
//! 后者是"服务端认定做没做完"。旧实现只看 `status` 就把任务接单，结果把
//! 已领取的任务又领了一遍。

use serde::Serialize;
use serde_json::Value;

/// 任务的接受 / 完成状态。
///
/// 服务端目前给出五种取值；未知取值一律落到 `Unknown` 并原样保留，
/// 因为"没见过的状态"意味着我们不知道它能不能领奖，这时候必须不动。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    NotAccepted,
    Accepted,
    InProgress,
    Completed,
    Claimed,
    Locked,
    Unknown,
}

impl TaskStatus {
    pub fn parse(raw: &str) -> Self {
        match raw.trim() {
            "not_accepted" => Self::NotAccepted,
            "accepted" => Self::Accepted,
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "claimed" => Self::Claimed,
            "locked" => Self::Locked,
            _ => Self::Unknown,
        }
    }

    /// 待领奖 = 服务端明确说完成，且不是已领取。
    pub fn claimable(self) -> bool {
        self == Self::Completed
    }

    /// 可接受 = 还没报名，且没有锁定。
    pub fn acceptable(self, locked: bool) -> bool {
        self == Self::NotAccepted && !locked
    }
}

/// 一个成长中心任务。字段全部只读，来自 `/tasks`。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthTask {
    pub code: String,
    pub title: String,
    pub description: Option<String>,
    /// 完成条件（服务端的 `task_desc`），和 `description` 不是一回事：
    /// 前者写"做什么才算完成"，后者写"怎么去那里"。
    pub condition: Option<String>,
    pub task_type: Option<String>,
    pub tag: Option<String>,
    pub jump_url: Option<String>,
    pub locked: bool,
    pub current: i64,
    pub target: i64,
    pub reward_credit: i64,
    pub reward_energy: i64,
    pub status: TaskStatus,
    /// 服务端原始状态字符串，便于线上发现新取值。
    pub raw_status: Option<String>,
}

impl GrowthTask {
    pub fn claimable(&self) -> bool {
        !self.locked && self.status.claimable()
    }

    pub fn acceptable(&self) -> bool {
        self.status.acceptable(self.locked)
    }
}

/// 成长档案 / 积分 / 能量 / 连签等汇总信息。
///
/// 每一项都是 `Option`：查不到就是查不到，界面显示"—"，
/// 不用 0 冒充"没有积分"。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthProfile {
    /// 成长等级。`/profile` 给的是 `level`，不是积分 —— 成长中心页展示的
    /// 那串数字其实是等级，别把它当积分用。
    pub level: Option<i64>,
    /// 已完成 / 全部任务数，来自 `/profile`。
    pub completed_tasks: Option<i64>,
    pub total_tasks: Option<i64>,
    /// 积分。**`/profile` 不返回这个字段**，所以 #67 阶段永远为空，
    /// 界面上显示「—」。等 #68 领奖时按服务端回执累计，才谈得上"积分"。
    pub credit: Option<i64>,
    pub energy: Option<i64>,
    pub streak_days: Option<i64>,
    pub makeup_cards: Option<i64>,
    pub lottery_chances: Option<i64>,
    pub heatmap_today: Option<String>,
    /// 连登 7 / 14 / 28 天的兑换状态，按档位名索引。
    pub redeem: Vec<RedeemTier>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedeemTier {
    pub tier: String,
    /// `claimed` / `locked` / 其它（可兑换）。
    pub status: String,
    pub claimable: bool,
}

/// 取第一个能当整数的字段。字符串数字也认（服务端偶尔把数字发成字符串）。
pub fn as_i64(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(n) = value.as_i64() {
        return Some(n);
    }
    if let Some(n) = value.as_f64() {
        return Some(n as i64);
    }
    value.as_str()?.trim().parse::<i64>().ok()
}

/// 把 `progress` 对象与平铺的 `current` / `target` 两种结构归一化。
///
/// 顺序很重要：先看 `progress`（新结构），再退平铺（老结构）。
/// 两者都没有时给 0/0 —— 界面据此判断"没有进度信息"，而不是"进度为零"。
pub fn read_progress(item: &Value) -> (i64, i64) {
    let nested = item.get("progress");
    let nested_current = as_i64(nested.and_then(|p| p.get("current")));
    let nested_target = as_i64(nested.and_then(|p| p.get("target")));
    let flat_current = as_i64(item.get("current"));
    let flat_target = as_i64(item.get("target"));
    (
        nested_current.or(flat_current).unwrap_or(0),
        nested_target.or(flat_target).unwrap_or(0),
    )
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// 解析一个任务项。`title` 缺失时回落 `task_code`，`task_code` 缺失则丢弃这一项
/// （没有 code 就无法接单或领奖，留着只会让人以为能操作）。
pub fn parse_task(item: &Value) -> Option<GrowthTask> {
    let code = text(item.get("task_code")).or_else(|| text(item.get("taskCode")))?;
    let (current, target) = read_progress(item);
    let raw_status = text(item.get("accept_status"))
        .or_else(|| text(item.get("status")))
        .or_else(|| text(item.get("acceptStatus")));
    let locked = item.get("locked").and_then(Value::as_bool).unwrap_or(false);
    let status = match raw_status.as_deref() {
        Some(raw) => TaskStatus::parse(raw),
        None => TaskStatus::Unknown,
    };
    Some(GrowthTask {
        title: text(item.get("title")).unwrap_or_else(|| code.clone()),
        description: text(item.get("description")),
        condition: text(item.get("task_desc")).or_else(|| text(item.get("taskDesc"))),
        task_type: text(item.get("task_type")).or_else(|| text(item.get("taskType"))),
        tag: text(item.get("tag")),
        jump_url: text(item.get("jump_url")).or_else(|| text(item.get("jumpUrl"))),
        locked,
        current,
        target,
        reward_credit: as_i64(item.get("reward_credit")).unwrap_or(0),
        reward_energy: as_i64(item.get("reward_energy")).unwrap_or(0),
        status,
        raw_status,
        code,
    })
}

/// 从响应里找出任务数组。
///
/// 见过的三种形状：`{tasks:[...]}`、`{data:{tasks:[...]}}`、`{data:[...]}`。
/// 全部试一遍比赌一种安全 —— 赌错了界面就是空的，而且没有任何报错。
pub fn locate_tasks(response: &Value) -> Option<&Vec<Value>> {
    if let Some(list) = response.get("tasks").and_then(Value::as_array) {
        return Some(list);
    }
    let data = response.get("data")?;
    if let Some(list) = data.get("tasks").and_then(Value::as_array) {
        return Some(list);
    }
    if let Some(list) = data.as_array() {
        return Some(list);
    }
    if let Some(list) = data.get("list").and_then(Value::as_array) {
        return Some(list);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reads_nested_and_flat_progress() {
        let nested = json!({"progress": {"current": 2, "target": 5}});
        assert_eq!(read_progress(&nested), (2, 5));

        let flat = json!({"current": 1, "target": 3});
        assert_eq!(read_progress(&flat), (1, 3));

        // 两者都在时以 progress 为准：它是新结构，平铺值可能是旧缓存。
        let both = json!({"progress": {"current": 4, "target": 4}, "current": 0, "target": 1});
        assert_eq!(read_progress(&both), (4, 4));

        // 都没有时不假装有进度
        assert_eq!(read_progress(&json!({})), (0, 0));
    }

    #[test]
    fn progress_accepts_numeric_strings() {
        let item = json!({"progress": {"current": "3", "target": "7"}});
        assert_eq!(read_progress(&item), (3, 7));
        let junk = json!({"current": "很多", "target": null});
        assert_eq!(read_progress(&junk), (0, 0));
    }

    #[test]
    fn missing_task_code_is_dropped_and_missing_title_falls_back() {
        assert!(parse_task(&json!({"title": "没有 code"})).is_none());

        let task = parse_task(&json!({"task_code": "create_canvas"})).expect("task");
        assert_eq!(task.title, "create_canvas", "没有标题就用 code，不能空着");
        assert_eq!(task.status, TaskStatus::Unknown, "没有状态就是未知");
        assert!(!task.claimable(), "未知状态绝不能进领奖队列");
        assert!(!task.acceptable(), "未知状态也不能自动接受");
    }

    #[test]
    fn unknown_status_stays_unknown_and_keeps_the_raw_value() {
        let task = parse_task(&json!({
            "task_code": "x",
            "accept_status": "something_new",
        }))
        .expect("task");
        assert_eq!(task.status, TaskStatus::Unknown);
        assert_eq!(task.raw_status.as_deref(), Some("something_new"));
    }

    #[test]
    fn locked_tasks_are_neither_acceptable_nor_claimable() {
        let task = parse_task(&json!({
            "task_code": "x",
            "accept_status": "completed",
            "locked": true,
            "reward_credit": 10,
        }))
        .expect("task");
        assert_eq!(task.status, TaskStatus::Completed);
        assert!(!task.claimable(), "锁定的任务不能领奖");
        assert!(!task.acceptable());
    }

    #[test]
    fn completed_is_the_only_claimable_status() {
        for raw in ["not_accepted", "accepted", "in_progress", "claimed"] {
            let status = TaskStatus::parse(raw);
            assert!(!status.claimable(), "{raw} 不该进领奖队列");
        }
        assert!(TaskStatus::parse("completed").claimable());
    }

    #[test]
    fn locate_tasks_understands_the_three_shapes() {
        let direct = json!({"tasks": [{"task_code": "a"}]});
        assert_eq!(locate_tasks(&direct).map(Vec::len), Some(1));

        let nested =
            json!({"code": 0, "data": {"tasks": [{"task_code": "a"}, {"task_code": "b"}]}});
        assert_eq!(locate_tasks(&nested).map(Vec::len), Some(2));

        let array = json!({"data": [{"task_code": "a"}]});
        assert_eq!(locate_tasks(&array).map(Vec::len), Some(1));

        let list = json!({"data": {"list": [{"task_code": "a"}]}});
        assert_eq!(locate_tasks(&list).map(Vec::len), Some(1));

        assert!(locate_tasks(&json!({"code": 0})).is_none());
    }
}
