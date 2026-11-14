//! 旅行编排的 Mock 测试。
//!
//! 没有网络、没有数据库、没有钥匙串：`MockTransport` 按队列吐响应，
//! `MockTokens` 给（或拒绝给）access。这样每条分支都是确定的，
//! 也能断言「到底发了几次请求」——比如重复领取必须是一次都不发。
//!
//! 真实接口的只读验证在 PR 的验收说明里（阶段 A），不在这里跑：
//! 单元测试不应该依赖公网和别人的账号。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use chrono::TimeZone;
use serde_json::{json, Value};

use crate::storage::travel::TravelState;
use crate::storage::AccountRecord;

use super::api::Transport;
use super::service::{self, Tokens};
use super::state::{Label, TOKEN_INVALID_COOLDOWN_SECS};

struct MockTransport {
    status: Mutex<Vec<Value>>,
    config: Mutex<Vec<Value>>,
    depart: Mutex<Vec<Value>>,
    claim: Mutex<Vec<Value>>,
    calls: Mutex<Vec<String>>,
}

impl MockTransport {
    fn new() -> Self {
        Self {
            status: Mutex::new(Vec::new()),
            config: Mutex::new(Vec::new()),
            depart: Mutex::new(Vec::new()),
            claim: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// 追加状态响应（**追加**而不是替换：一轮同步里状态会被查多次）。
    fn with_status(self, values: Vec<Value>) -> Self {
        self.status.lock().unwrap().extend(values);
        self
    }

    fn with_config(self, values: Vec<Value>) -> Self {
        self.config.lock().unwrap().extend(values);
        self
    }

    fn with_depart(self, values: Vec<Value>) -> Self {
        self.depart.lock().unwrap().extend(values);
        self
    }

    fn with_claim(self, values: Vec<Value>) -> Self {
        self.claim.lock().unwrap().extend(values);
        self
    }

    fn record(&self, call: &str) {
        self.calls.lock().unwrap().push(call.to_string());
    }

    fn call_count(&self, prefix: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with(prefix))
            .count()
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn next(queue: &Mutex<Vec<Value>>) -> Value {
        let mut guard = queue.lock().unwrap();
        if guard.is_empty() {
            // 多打一次接口就炸：测试想覆盖的正是"该发几次"。
            return json!({"ok": false, "error": "unexpected call beyond script"});
        }
        guard.remove(0)
    }
}

impl Transport for MockTransport {
    async fn config(&self, _payload: &Value) -> Value {
        self.record("config");
        Self::next(&self.config)
    }

    async fn status(&self, _payload: &Value) -> Value {
        self.record("status");
        Self::next(&self.status)
    }

    async fn depart(&self, _payload: &Value, _location_id: &Value) -> Value {
        self.record("depart");
        Self::next(&self.depart)
    }

    async fn claim(&self, _payload: &Value, _record_id: i64) -> Value {
        self.record("claim");
        Self::next(&self.claim)
    }
}

struct MockTokens {
    access: Mutex<Result<String, String>>,
    resync: Mutex<Result<String, String>>,
    resync_calls: AtomicUsize,
}

impl MockTokens {
    fn valid(access: &str) -> Self {
        Self {
            access: Mutex::new(Ok(access.to_string())),
            resync: Mutex::new(Ok("fresh-access".to_string())),
            resync_calls: AtomicUsize::new(0),
        }
    }

    fn without_access() -> Self {
        Self {
            access: Mutex::new(Err("钥匙串里没有访问令牌".to_string())),
            resync: Mutex::new(Err("Magpie 里也没有".to_string())),
            resync_calls: AtomicUsize::new(0),
        }
    }

    fn with_failing_resync(access: &str) -> Self {
        Self {
            access: Mutex::new(Ok(access.to_string())),
            resync: Mutex::new(Err("Magpie 里的令牌也已过期".to_string())),
            resync_calls: AtomicUsize::new(0),
        }
    }
}

impl Tokens for MockTokens {
    fn access(&self, _account: &AccountRecord) -> Result<String, String> {
        self.access.lock().unwrap().clone()
    }

    async fn resync(&self, _account: &AccountRecord) -> Result<String, String> {
        self.resync_calls.fetch_add(1, Ordering::SeqCst);
        self.resync.lock().unwrap().clone()
    }
}

fn account() -> AccountRecord {
    AccountRecord {
        id: "acc-1".to_string(),
        platform: "workbuddy".to_string(),
        uid: "uid-1".to_string(),
        nickname: "测试账号".to_string(),
        email: None,
        group_id: None,
        auth_status: "valid".to_string(),
        checkin_enabled: false,
        last_checkin_at: None,
        last_result: None,
        updated_at: "2026-11-10T00:00:00Z".to_string(),
        token_expires_at: None,
        auth_domain: Some("www.workbuddy.cn".to_string()),
        auth_source: "magpie".to_string(),
        travel_enabled: true,
        auth_epoch: 0,
        auth_mirror_auth_id: None,
        provider: "workbuddy".to_string(),
    }
}

fn prior() -> TravelState {
    TravelState {
        account_id: "acc-1".to_string(),
        label: Label::Unknown.as_str().to_string(),
        ..Default::default()
    }
}

fn status_idle() -> Value {
    json!({"ok":true,"state":"idle","dailyLimitReached":false,"recordId":0,
           "departAt":0,"arriveAt":0,"locationName":"","rewardCredit":null,"serverNow":0})
}

const NOW: i64 = 1_794_000_000;

/// 常规一轮：允许领取，是否允许派出由调用方给。
async fn run(
    transport: &MockTransport,
    tokens: &MockTokens,
    prior: TravelState,
    allow_depart: bool,
) -> service::Outcome {
    service::sync_account(
        transport,
        tokens,
        &account(),
        prior,
        NOW,
        "2026-11-10",
        allow_depart,
        true,
    )
    .await
}

/// 纯只读一轮（界面上的「刷新状态」）。
async fn observe(
    transport: &MockTransport,
    tokens: &MockTokens,
    prior: TravelState,
) -> service::Outcome {
    service::sync_account(
        transport,
        tokens,
        &account(),
        prior,
        NOW,
        "2026-11-10",
        false,
        false,
    )
    .await
}

/// 只有明确的认证失败才算 401。429 / 5xx / 传输错误即使文案里带 "token" 也不算。
#[test]
fn only_explicit_auth_failures_count_as_unauthorized() {
    use super::state::is_auth_failure;

    assert!(is_auth_failure(
        &json!({"code":401,"message":"unauthorized"})
    ));
    assert!(is_auth_failure(&json!({"code":403,"message":"登录已失效"})));
    assert!(
        !is_auth_failure(&json!({"code":403,"message":"activity not open"})),
        "活动未开放不是认证失败"
    );
    assert!(
        !is_auth_failure(&json!({"code":429,"message":"token 频率超限"})),
        "限流不能被当成令牌失效"
    );
    assert!(
        !is_auth_failure(&json!({"code":503,"message":"token 服务不可用"})),
        "5xx 不能被当成令牌失效"
    );
    assert!(
        !is_auth_failure(&json!({"code":-1,"message":"timeout token"})),
        "传输错误不能被当成令牌失效"
    );
}

/// 429 不触发换源，也不该用 6 小时的"令牌失效"冷却。
#[tokio::test]
async fn rate_limited_status_does_not_resync_or_use_token_cooldown() {
    let transport =
        MockTransport::new().with_status(vec![json!({"code":429,"message":"token 频率超限"})]);
    let tokens = MockTokens::valid("access-1");

    let outcome = observe(&transport, &tokens, prior()).await;

    assert_eq!(
        tokens.resync_calls.load(Ordering::SeqCst),
        0,
        "429 不换源、不重新同步"
    );
    assert_eq!(transport.call_count("status"), 1);
    let retry_after = outcome.state.retry_after.clone().expect("要退避");
    let at = chrono::DateTime::parse_from_rfc3339(&retry_after)
        .unwrap()
        .timestamp();
    assert!(
        at - NOW < TOKEN_INVALID_COOLDOWN_SECS,
        "429 走普通退避，不该打 6 小时令牌冷却"
    );
}

/// 5xx 同理：不换源，也不重复派出。
#[tokio::test]
async fn server_error_does_not_switch_source_or_depart() {
    let transport = MockTransport::new().with_status(vec![
        json!({"code":502,"message":"bad gateway: token 服务不可用"}),
    ]);
    let tokens = MockTokens::valid("access-1");

    let outcome = run(&transport, &tokens, prior(), true).await;

    assert_eq!(tokens.resync_calls.load(Ordering::SeqCst), 0);
    assert_eq!(transport.call_count("depart"), 0, "状态都没查清就不许派出");
    assert_eq!(outcome.state.label, Label::Failed.as_str());
}

#[tokio::test]
async fn idle_account_is_departed_when_window_open() {
    let transport = MockTransport::new()
        .with_status(vec![status_idle()])
        .with_config(vec![json!({"ok":true,"enabled":true,"locations":[
            {"id":1,"name":"咖啡馆"},{"id":2,"name":"健身房"}]})])
        .with_depart(vec![json!({"ok":true,"state":"traveling"})])
        .with_status(vec![json!({"ok":true,"state":"traveling","recordId":9,
            "arriveAt":1_794_010_800_i64,"locationName":"咖啡馆","dailyLimitReached":false})]);
    let tokens = MockTokens::valid("access-1");

    let outcome = run(&transport, &tokens, prior(), true).await;

    assert_eq!(outcome.state.label, Label::Traveling.as_str());
    assert_eq!(outcome.state.location_name, "咖啡馆");
    assert_eq!(outcome.state.arrive_at, 1_794_010_800);
    assert_eq!(outcome.state.depart_date.as_deref(), Some("2026-11-10"));
    assert_eq!(transport.call_count("depart"), 1);
    assert_eq!(tokens.resync_calls.load(Ordering::SeqCst), 0);
    assert!(outcome
        .events
        .iter()
        .any(|e| e.kind == "depart" && e.status == "success"));
}

#[tokio::test]
async fn idle_account_without_depart_window_is_only_marked_dispatchable() {
    let transport = MockTransport::new().with_status(vec![status_idle()]);
    let tokens = MockTokens::valid("access-1");

    let outcome = run(&transport, &tokens, prior(), false).await;

    assert_eq!(outcome.state.label, Label::Dispatchable.as_str());
    assert_eq!(transport.call_count("depart"), 0, "没到派出窗口就不该派");
    assert_eq!(transport.call_count("config"), 0);
}

#[tokio::test]
async fn unauthorized_access_is_resynced_once_and_retried() {
    let transport = MockTransport::new().with_status(vec![
        json!({"code":401,"message":"unauthorized"}),
        status_idle(),
    ]);
    let tokens = MockTokens::valid("expired-access");

    let outcome = run(&transport, &tokens, prior(), false).await;

    assert_eq!(
        tokens.resync_calls.load(Ordering::SeqCst),
        1,
        "只重同步一次"
    );
    assert_eq!(transport.call_count("status"), 2);
    assert_eq!(outcome.state.label, Label::Dispatchable.as_str());
    assert!(outcome.state.last_error.is_none());
}

#[tokio::test]
async fn token_dead_ends_in_long_cooldown_without_retry_storm() {
    let transport =
        MockTransport::new().with_status(vec![json!({"code":401,"message":"unauthorized"})]);
    let tokens = MockTokens::with_failing_resync("expired-access");

    let outcome = run(&transport, &tokens, prior(), true).await;

    assert_eq!(outcome.state.label, Label::Failed.as_str());
    assert_eq!(transport.call_count("status"), 1, "401 之后不再连环重试");
    assert_eq!(transport.call_count("depart"), 0);
    let retry_after = outcome.state.retry_after.clone().expect("要有冷却");
    let at = chrono::DateTime::parse_from_rfc3339(&retry_after).unwrap();
    assert_eq!(
        at.timestamp() - NOW,
        TOKEN_INVALID_COOLDOWN_SECS,
        "令牌失效直接冷却 6 小时"
    );
}

#[tokio::test]
async fn missing_credential_never_touches_the_network() {
    let transport = MockTransport::new();
    let tokens = MockTokens::without_access();

    let outcome = run(&transport, &tokens, prior(), true).await;

    assert_eq!(outcome.state.label, Label::Failed.as_str());
    assert!(transport.calls().is_empty(), "没有令牌就不该发任何请求");
    assert!(outcome
        .state
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("钥匙串"));
}

#[tokio::test]
async fn arrived_trip_is_claimed_and_credits_recorded() {
    let transport = MockTransport::new()
        .with_status(vec![json!({"ok":true,"state":"arrived","recordId":42,
            "dailyLimitReached":false,"arriveAt":1_793_999_000_i64,
            "locationName":"健身房","rewardCredit":5})])
        .with_claim(vec![json!({"ok":true,"rewardCredit":5})]);
    let tokens = MockTokens::valid("access-1");

    let outcome = run(&transport, &tokens, prior(), false).await;

    assert_eq!(outcome.state.label, Label::DoneToday.as_str());
    assert!(outcome.state.claimed);
    assert_eq!(outcome.state.reward_credits, Some(5));
    assert_eq!(outcome.state.claimed_record_id, 42);
    assert!(outcome.state.claimed_at.is_some());
    assert_eq!(transport.call_count("claim"), 1);
    assert!(outcome
        .events
        .iter()
        .any(|e| e.kind == "claim" && e.status == "success" && e.credits == Some(5)));
}

#[tokio::test]
async fn the_same_trip_is_never_claimed_twice() {
    let transport = MockTransport::new().with_status(vec![json!({"ok":true,
        "state":"arrived","recordId":42,"dailyLimitReached":false,
        "locationName":"健身房","rewardCredit":5})]);
    let tokens = MockTokens::valid("access-1");
    let mut state = prior();
    state.claimed = true;
    state.claimed_record_id = 42;
    state.server_state = "arrived".to_string();

    let outcome = run(&transport, &tokens, state, false).await;

    assert_eq!(
        transport.call_count("claim"),
        0,
        "同一程已经领过，不能再提交一次"
    );
    assert_eq!(outcome.state.label, Label::DoneToday.as_str());
    assert!(outcome
        .events
        .iter()
        .any(|e| e.status == "skipped" && e.message.as_deref().unwrap_or("").contains("重复")));
}

#[tokio::test]
async fn claim_is_skipped_when_another_client_already_took_it() {
    let transport = MockTransport::new()
        .with_status(vec![json!({"ok":true,"state":"arrived","recordId":43,
            "dailyLimitReached":false,"rewardCredit":5})])
        .with_claim(vec![
            json!({"ok":false,"code":400,"message":"no unclaimed travel"}),
        ]);
    let tokens = MockTokens::valid("access-1");

    let outcome = run(&transport, &tokens, prior(), false).await;

    assert!(outcome.state.claimed, "别处领走了也算已领，不要反复重试");
    assert_eq!(outcome.state.label, Label::DoneToday.as_str());
    assert!(outcome.state.conflict.is_some(), "要提示存在并发操作");
}

#[tokio::test]
async fn daily_limit_stops_departures_for_today() {
    let transport = MockTransport::new().with_status(vec![json!({"ok":true,
        "state":"idle","dailyLimitReached":true,"recordId":0})]);
    let tokens = MockTokens::valid("access-1");

    let outcome = run(&transport, &tokens, prior(), true).await;

    assert_eq!(outcome.state.label, Label::DoneToday.as_str());
    assert_eq!(transport.call_count("depart"), 0);
    assert_eq!(outcome.state.depart_date.as_deref(), Some("2026-11-10"));
}

#[tokio::test]
async fn local_daily_cap_blocks_a_second_departure_same_day() {
    let transport = MockTransport::new().with_status(vec![status_idle()]);
    let tokens = MockTokens::valid("access-1");
    let mut state = prior();
    state.depart_date = Some("2026-11-10".to_string());

    let outcome = run(&transport, &tokens, state, true).await;

    assert_eq!(outcome.state.label, Label::DoneToday.as_str());
    assert_eq!(transport.call_count("depart"), 0, "本地护栏：一天只派一次");
}

#[tokio::test]
async fn rate_limited_departure_backs_off() {
    let transport = MockTransport::new()
        .with_status(vec![status_idle()])
        .with_config(vec![
            json!({"ok":true,"enabled":true,"locations":[{"id":1,"name":"咖啡馆"}]}),
        ])
        .with_depart(vec![
            json!({"ok":false,"code":429,"message":"too many requests"}),
        ]);
    let tokens = MockTokens::valid("access-1");

    let outcome = run(&transport, &tokens, prior(), true).await;

    assert_eq!(outcome.state.label, Label::Dispatchable.as_str());
    assert_eq!(outcome.state.failures, 1);
    let retry_after = outcome.state.retry_after.clone().expect("429 必须退避");
    let at = chrono::DateTime::parse_from_rfc3339(&retry_after).unwrap();
    assert!(at.timestamp() > NOW, "退避时间要在未来");
    assert_eq!(transport.call_count("depart"), 1, "限流后不再连续重试");
}

#[tokio::test]
async fn no_buddy_is_retryable_and_not_counted_as_error() {
    let transport = MockTransport::new()
        .with_status(vec![status_idle()])
        .with_config(vec![
            json!({"ok":true,"enabled":true,"locations":[{"id":1,"name":"咖啡馆"}]}),
        ])
        .with_depart(vec![
            json!({"ok":false,"code":400,"message":"no active buddy"}),
        ]);
    let tokens = MockTokens::valid("access-1");
    let mut state = prior();
    state.failures = 2;

    let outcome = run(&transport, &tokens, state, true).await;

    assert_eq!(outcome.state.label, Label::NoBuddy.as_str());
    assert_eq!(
        outcome.state.failures, 0,
        "状态查询成功已经清零；无 Buddy 不额外抬高失败计数"
    );
    assert!(
        outcome.state.retry_after.is_some(),
        "下一轮派出窗口照旧等着"
    );
}

#[tokio::test]
async fn unavailable_location_falls_through_to_the_next_one() {
    let transport = MockTransport::new()
        .with_status(vec![status_idle()])
        .with_config(vec![json!({"ok":true,"enabled":true,"locations":[
            {"id":1,"name":"咖啡馆"},{"id":2,"name":"健身房"}]})])
        .with_depart(vec![
            json!({"ok":false,"code":400,"message":"location not available"}),
            json!({"ok":true,"state":"traveling"}),
        ])
        .with_status(vec![json!({"ok":true,"state":"traveling","recordId":11,
            "arriveAt":1_794_010_800_i64,"locationName":"健身房"})]);
    let tokens = MockTokens::valid("access-1");

    let outcome = run(&transport, &tokens, prior(), true).await;

    assert_eq!(outcome.state.label, Label::Traveling.as_str());
    assert_eq!(outcome.state.location_name, "健身房");
    assert_eq!(
        transport.call_count("depart"),
        2,
        "第一个地点不可用就换下一个"
    );
}

#[tokio::test]
async fn timeout_is_reported_and_deferred() {
    let transport = MockTransport::new().with_status(vec![
        json!({"ok":false,"code":-1,"error":"请求超时（20s）"}),
    ]);
    let tokens = MockTokens::valid("access-1");

    let outcome = run(&transport, &tokens, prior(), true).await;

    assert_eq!(outcome.state.label, Label::Failed.as_str());
    assert!(outcome
        .state
        .last_error
        .as_deref()
        .unwrap_or_default()
        .contains("超时"));
    assert!(outcome.state.retry_after.is_some());
}

#[tokio::test]
async fn unsupported_variant_short_circuits_without_requests() {
    let transport = MockTransport::new();
    let tokens = MockTokens::valid("access-1");
    let mut account = account();
    account.auth_domain = Some("www.workbuddy.ai".to_string());

    let outcome = service::sync_account(
        &transport,
        &tokens,
        &account,
        prior(),
        NOW,
        "2026-11-10",
        true,
        true,
    )
    .await;

    assert_eq!(outcome.state.label, Label::Unsupported.as_str());
    assert!(transport.calls().is_empty(), "国际版没有成长中心，不发请求");
}

#[tokio::test]
async fn conflict_is_reported_when_another_client_finished_the_trip() {
    let transport = MockTransport::new().with_status(vec![json!({"ok":true,
        "state":"idle","dailyLimitReached":true,"recordId":0})]);
    let tokens = MockTokens::valid("access-1");
    let mut state = prior();
    state.server_state = "traveling".to_string();
    state.depart_date = Some("2026-11-10".to_string());

    let outcome = run(&transport, &tokens, state, true).await;

    assert_eq!(outcome.state.label, Label::DoneToday.as_str());
    assert!(outcome
        .state
        .conflict
        .as_deref()
        .unwrap_or_default()
        .contains("其它客户端"));
}

#[test]
fn schedule_gating_respects_intervals_and_backoff() {
    let config = service::Config::default();
    let mut state = prior();
    assert!(service::is_due(&state, NOW, &config), "没查过就该查");

    state.last_checked_at = Some("2026-11-10T00:00:00Z".to_string());
    let checked = chrono::DateTime::parse_from_rfc3339("2026-11-10T00:00:00Z")
        .unwrap()
        .timestamp();
    assert!(
        !service::is_due(&state, checked + 60, &config),
        "刚查过不重复查"
    );
    assert!(service::is_due(
        &state,
        checked + service::CLAIM_INTERVAL_SECS,
        &config
    ));

    state.retry_after = Some(
        chrono::Utc
            .timestamp_opt(checked + 3600, 0)
            .single()
            .unwrap()
            .to_rfc3339(),
    );
    assert!(
        !service::is_due(&state, checked + 60, &config),
        "退避期内不查"
    );
    assert!(service::is_due(&state, checked + 3601, &config));

    let mut fresh = prior();
    assert!(service::depart_window_open(&fresh, NOW, &config));
    fresh.last_depart_attempt_at = Some(
        chrono::Utc
            .timestamp_opt(NOW - 60, 0)
            .single()
            .unwrap()
            .to_rfc3339(),
    );
    assert!(
        !service::depart_window_open(&fresh, NOW, &config),
        "30 分钟内不再派"
    );
}

/// 只读模式：看到「已到达」也只能标状态，绝不能提交领取。
#[tokio::test]
async fn observe_mode_never_submits_claims() {
    let transport = MockTransport::new().with_status(vec![json!({"ok":true,
        "state":"arrived","recordId":42,"dailyLimitReached":false,
        "locationName":"健身房","rewardCredit":5})]);
    let tokens = MockTokens::valid("access-1");

    let outcome = observe(&transport, &tokens, prior()).await;

    assert_eq!(outcome.state.label, Label::Arrived.as_str());
    assert_eq!(transport.call_count("claim"), 0, "只读刷新不能领奖");
    assert!(!outcome.state.claimed);
    assert!(
        outcome
            .events
            .iter()
            .any(|e| e.status == "skipped" && e.message.as_deref().unwrap_or("").contains("只读")),
        "要如实说明「检测到了但没提交」"
    );
}
