//! Buddy 旅行状态机（纯逻辑，不碰网络、不碰数据库）。
//!
//! 这一段是全部判断的唯一出口，所有边界都在这里用单元测试钉住：
//!
//! * 服务端状态到界面标签的映射
//! * 「该派出 / 该领取 / 今日已完」的判定
//! * 别的客户端同时在操作时的冲突识别
//! * 失败退避曲线
//!
//! 好处很实际：Mock 测试可以直接喂一段 `serde_json::json!`，不需要起网络，
//! 也不需要造数据库。IO 在 `api.rs` / `service.rs`，这里只有函数。

use serde_json::Value;

/// 界面标签。字符串会落进 `travel_states.label`，改名要连带迁移。
pub const LABEL_DISPATCHABLE: &str = "dispatchable";
pub const LABEL_TRAVELING: &str = "traveling";
pub const LABEL_ARRIVED: &str = "arrived";
pub const LABEL_DONE_TODAY: &str = "done_today";
pub const LABEL_NO_BUDDY: &str = "no_buddy";
pub const LABEL_UNSUPPORTED: &str = "unsupported";
pub const LABEL_FAILED: &str = "failed";
pub const LABEL_UNKNOWN: &str = "unknown";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    Dispatchable,
    Traveling,
    Arrived,
    DoneToday,
    NoBuddy,
    Unsupported,
    Failed,
    Unknown,
}

impl Label {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dispatchable => LABEL_DISPATCHABLE,
            Self::Traveling => LABEL_TRAVELING,
            Self::Arrived => LABEL_ARRIVED,
            Self::DoneToday => LABEL_DONE_TODAY,
            Self::NoBuddy => LABEL_NO_BUDDY,
            Self::Unsupported => LABEL_UNSUPPORTED,
            Self::Failed => LABEL_FAILED,
            Self::Unknown => LABEL_UNKNOWN,
        }
    }
}

/// 服务端 `/status` 的视图。字段口径与官方一致，不做乐观补全：
/// 缺 `state` 或给了不认识的 `state` 都算失败，绝不当成 `idle`。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerStatus {
    pub state: String,
    pub daily_limit_reached: bool,
    pub location_id: Option<i64>,
    pub location_name: String,
    pub depart_at: i64,
    pub arrive_at: i64,
    pub record_id: i64,
    pub reward_credit: Option<i64>,
    pub server_now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusError {
    /// `unauthorized` / `rate_limited` / `timeout` / `network` / `shape`
    pub kind: &'static str,
    pub message: String,
}

impl StatusError {
    fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }
}

/// 把「秒 / 毫秒」统一成秒。服务端两处字段混用过，宁可多一层。
pub fn to_secs(ts: i64) -> i64 {
    if ts > 1_000_000_000_000 {
        ts / 1000
    } else {
        ts
    }
}

fn as_i64(value: Option<&Value>) -> i64 {
    value
        .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
        .unwrap_or(0)
}

/// 解析 `/status` 响应。
///
/// 401/403 单独归类成 `unauthorized`，让上层决定「重新从 Magpie 取 access」而不是退避重试。
/// 解析 `/status`。
///
/// 输入是**上游封装归一化之后**的形状（`fetch_travel_status` 的返回值）：
///
/// ```text
/// 成功：{ ok:true, state, dailyLimitReached, locationId, locationName,
///         departAt, arriveAt, recordId, rewardCredit, serverNow }
/// 失败：{ ok:false, code, error }
/// ```
///
/// 401/403 单独归类成 `unauthorized`，让上层决定「重新从 Magpie 取 access」
/// 而不是盲目退避重试。缺 `state` 或状态未知一律当失败，绝不当成 `idle`——
/// 那会导致「明明在旅行却被标成今日已完成」。
pub fn parse_status(resp: &Value) -> Result<ServerStatus, StatusError> {
    if wb_switch_core::modules::travel::is_unauthorized(resp) {
        return Err(StatusError::new("unauthorized", error_text(resp)));
    }
    let code = as_i64(resp.get("code"));
    if code == 429 {
        return Err(StatusError::new("rate_limited", error_text(resp)));
    }
    if resp.get("ok").and_then(Value::as_bool) != Some(true) {
        let text = error_text(resp);
        let lowered = text.to_lowercase();
        let kind = if code == -3 || lowered.contains("travel state") {
            "shape"
        } else if lowered.contains("timeout") || text.contains("超时") {
            "timeout"
        } else if code == -1 || code == 0 {
            "network"
        } else {
            "http"
        };
        return Err(StatusError::new(kind, text));
    }

    let raw_state = resp.get("state").and_then(Value::as_str).unwrap_or("");
    let Some(state) = wb_switch_core::modules::travel::parse_travel_state(Some(raw_state)) else {
        return Err(StatusError::new(
            "shape",
            if raw_state.trim().is_empty() {
                "服务端没有返回旅行状态".to_string()
            } else {
                format!("未知的旅行状态：{raw_state}")
            },
        ));
    };
    Ok(ServerStatus {
        state: state.to_string(),
        daily_limit_reached: resp
            .get("dailyLimitReached")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        location_id: resp.get("locationId").and_then(|v| v.as_i64()),
        location_name: resp
            .get("locationName")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        depart_at: to_secs(as_i64(resp.get("departAt"))),
        arrive_at: to_secs(as_i64(resp.get("arriveAt"))),
        record_id: as_i64(resp.get("recordId")),
        reward_credit: resp
            .get("rewardCredit")
            .and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)))
            .filter(|v| *v != 0),
        server_now: to_secs(as_i64(resp.get("serverNow"))),
    })
}

pub fn error_text(resp: &Value) -> String {
    resp.get("message")
        .or_else(|| resp.get("msg"))
        .or_else(|| resp.get("error"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("code={}", as_i64(resp.get("code"))))
}

/// 快照里与判定相关的部分。用 `TravelState` 的字段喂进来，避免 state 模块依赖存储层。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Prior {
    pub server_state: String,
    pub record_id: i64,
    pub claimed: bool,
    pub claimed_record_id: i64,
    pub depart_date: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// 已到达且没领过：去领。
    Claim,
    /// 旅行中：等。
    Wait,
    /// 已到达，但这一程我们记录里已经领过：不重复提交。
    AlreadyClaimed,
    /// 可以派出。
    Depart,
    /// 今日已完（服务端每日上限，或本地当天已派出过）。
    DoneToday,
}

/// 官方状态机 + 两条本地护栏。
///
/// 与官网一致：`arrived → 领`、`traveling → 等`、`idle + daily_limit → 今天结束`、
/// `idle → 派`。本地护栏是 `claimed_record_id`（同一程不重复领）与
/// `depart_date == today`（同一天不重复派，防止服务端上限字段异常时刷接口）。
pub fn next_action(status: &ServerStatus, prior: &Prior, today: &str) -> Action {
    match status.state.as_str() {
        "arrived" => {
            if status.record_id > 0 && prior.claimed && prior.claimed_record_id == status.record_id
            {
                Action::AlreadyClaimed
            } else {
                Action::Claim
            }
        }
        "traveling" => Action::Wait,
        "idle" if status.daily_limit_reached => Action::DoneToday,
        "idle" if prior.depart_date.as_deref() == Some(today) => Action::DoneToday,
        "idle" => Action::Depart,
        _ => Action::Wait,
    }
}

/// 认不认为「别的客户端也在动这个账号」。
///
/// 不阻断操作，只写进快照给用户看：Magpie、wb-switch 或官方网页都可能同时在跑，
/// 静默互相覆盖比提示一句要糟得多。
pub fn detect_conflict(status: &ServerStatus, prior: &Prior) -> Option<String> {
    if prior.server_state == "traveling" && status.state == "idle" && status.daily_limit_reached {
        return Some("行程在其它客户端结束（服务端已计入今日上限）".to_string());
    }
    if prior.server_state == "arrived"
        && !prior.claimed
        && status.state == "idle"
        && !status.daily_limit_reached
    {
        return Some("这一趟的奖励可能在其它客户端被领走了".to_string());
    }
    if prior.server_state == "traveling"
        && status.state == "traveling"
        && prior.record_id > 0
        && status.record_id > 0
        && prior.record_id != status.record_id
    {
        return Some("另一个客户端重新派出了新的行程".to_string());
    }
    None
}

/// 失败退避：5 分钟起翻倍，1 小时封顶。
///
/// `failures` 是**连续**失败次数，成功一次就清零。上限存在的意义是让 429 /
/// 断网期间不再打接口，也不要一个小时才恢复。
pub fn backoff_secs(failures: i64) -> i64 {
    let n = failures.clamp(1, 6) as u32;
    (300i64 << (n - 1)).min(3600)
}

/// 令牌彻底不可用时给一个更长的冷却：重试再多次也不会变好，
/// 用户得先去 Magpie 重新登录或重新扫码。
pub const TOKEN_INVALID_COOLDOWN_SECS: i64 = 6 * 60 * 60;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn prior(state: &str) -> Prior {
        Prior {
            server_state: state.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn parses_idle_status() {
        let resp = json!({"ok":true,"state":"idle","dailyLimitReached":false,"recordId":0});
        let status = parse_status(&resp).expect("ok");
        assert_eq!(status.state, "idle");
        assert!(!status.daily_limit_reached);
        assert_eq!(status.record_id, 0);
    }

    #[test]
    fn parses_arrived_status_with_millis() {
        let resp = json!({"ok":true,"state":"arrived","recordId":42,
            "departAt":1788964568,"arriveAt":1788964568_000_i64,
            "locationId":3,"locationName":"健身房","rewardCredit":5});
        let status = parse_status(&resp).expect("ok");
        assert_eq!(status.arrive_at, 1788964568);
        assert_eq!(status.location_id, Some(3));
        assert_eq!(status.location_name, "健身房");
        assert_eq!(status.reward_credit, Some(5));
    }

    #[test]
    fn missing_state_is_shape_error_not_idle() {
        let resp = json!({"ok":true,"dailyLimitReached":false});
        let err = parse_status(&resp).expect_err("缺 state 必须报错");
        assert_eq!(err.kind, "shape");
        let resp = json!({"ok":true,"state":"teleporting"});
        assert_eq!(parse_status(&resp).unwrap_err().kind, "shape");
    }

    #[test]
    fn unauthorized_and_rate_limit_are_classified() {
        assert_eq!(
            parse_status(&json!({"ok":false,"code":401,"error":"unauthorized"}))
                .unwrap_err()
                .kind,
            "unauthorized"
        );
        assert_eq!(
            parse_status(&json!({"ok":false,"code":403}))
                .unwrap_err()
                .kind,
            "unauthorized"
        );
        assert_eq!(
            parse_status(&json!({"ok":false,"code":429,"error":"too many requests"}))
                .unwrap_err()
                .kind,
            "rate_limited"
        );
    }

    #[test]
    fn network_error_is_classified() {
        let err =
            parse_status(&json!({"ok":false,"code":-1,"error":"请求超时（20s）"})).unwrap_err();
        assert_eq!(err.kind, "timeout");
        let err =
            parse_status(&json!({"ok":false,"code":-1,"error":"connection reset"})).unwrap_err();
        assert_eq!(err.kind, "network");
    }

    #[test]
    fn action_follows_official_state_machine() {
        let idle = ServerStatus {
            state: "idle".into(),
            ..Default::default()
        };
        assert_eq!(
            next_action(&idle, &prior("idle"), "2026-11-10"),
            Action::Depart
        );

        let capped = ServerStatus {
            state: "idle".into(),
            daily_limit_reached: true,
            ..Default::default()
        };
        assert_eq!(
            next_action(&capped, &prior("idle"), "2026-11-10"),
            Action::DoneToday
        );

        let traveling = ServerStatus {
            state: "traveling".into(),
            ..Default::default()
        };
        assert_eq!(
            next_action(&traveling, &prior("traveling"), "2026-11-10"),
            Action::Wait
        );

        let arrived = ServerStatus {
            state: "arrived".into(),
            record_id: 7,
            ..Default::default()
        };
        assert_eq!(
            next_action(&arrived, &prior("traveling"), "2026-11-10"),
            Action::Claim
        );
    }

    #[test]
    fn same_record_is_never_claimed_twice() {
        let arrived = ServerStatus {
            state: "arrived".into(),
            record_id: 7,
            ..Default::default()
        };
        let mut p = prior("arrived");
        p.claimed = true;
        p.claimed_record_id = 7;
        assert_eq!(
            next_action(&arrived, &p, "2026-11-10"),
            Action::AlreadyClaimed
        );

        // 换了一程（record_id 变了）就该正常领取。
        let next = ServerStatus {
            state: "arrived".into(),
            record_id: 8,
            ..Default::default()
        };
        assert_eq!(next_action(&next, &p, "2026-11-10"), Action::Claim);
    }

    #[test]
    fn same_day_never_departs_twice() {
        let idle = ServerStatus {
            state: "idle".into(),
            ..Default::default()
        };
        let mut p = prior("idle");
        p.depart_date = Some("2026-11-10".into());
        assert_eq!(
            next_action(&idle, &p, "2026-11-10"),
            Action::DoneToday,
            "本地护栏：同一天不重复派"
        );
        assert_eq!(
            next_action(&idle, &p, "2026-11-11"),
            Action::Depart,
            "换天就该重新派"
        );
    }

    #[test]
    fn conflicts_are_detected() {
        let capped = ServerStatus {
            state: "idle".into(),
            daily_limit_reached: true,
            ..Default::default()
        };
        assert!(detect_conflict(&capped, &prior("traveling")).is_some());

        let idle = ServerStatus {
            state: "idle".into(),
            ..Default::default()
        };
        let mut arrived = prior("arrived");
        arrived.record_id = 3;
        assert!(detect_conflict(&idle, &arrived).is_some());

        let other_trip = ServerStatus {
            state: "traveling".into(),
            record_id: 99,
            ..Default::default()
        };
        let mut traveling = prior("traveling");
        traveling.record_id = 3;
        assert!(detect_conflict(&other_trip, &traveling).is_some());

        // 正常情况不报冲突。
        assert!(detect_conflict(&idle, &prior("idle")).is_none());
    }

    #[test]
    fn backoff_grows_then_caps() {
        assert_eq!(backoff_secs(0), 300, "0 次失败按最低档算");
        assert_eq!(backoff_secs(1), 300);
        assert_eq!(backoff_secs(2), 600);
        assert_eq!(backoff_secs(3), 1200);
        assert_eq!(backoff_secs(4), 2400);
        assert_eq!(backoff_secs(5), 3600);
        assert_eq!(backoff_secs(99), 3600);
    }
}
