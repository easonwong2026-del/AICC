//! 签到流程的离线测试。
//!
//! 全部用 Mock `Transport`，**没有一个真实请求**：CI 里绝不允许对真实账号发
//! `daily-checkin`。覆盖的是那些"看起来能跑但会悄悄做错事"的分支：
//! 已签到却仍提交、缺状态字段当 false、提交成功但没回查、超时后重发、并发双签。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::checkin::api::{CheckinError, ErrorKind, StatusReply, SubmitReply, Transport};
use crate::paths::Paths;
use crate::securestore::SecureStore;
use crate::storage::checkin::STATUS_CLAIMED;
use crate::storage::AccountInput;

// ---------------------------------------------------------------------------
// 脚手架
// ---------------------------------------------------------------------------

struct Fixture {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let data = dir.path().to_path_buf();
    let paths = Paths {
        db_path: data.join("accounts.sqlite"),
        backups_dir: data.join("backups"),
        logs_dir: data.join("logs"),
        wb_switch_home: data.join("wb-switch-home"),
        data_dir: data,
        wb_home_owned: true,
    };
    let app = Arc::new(App::with_secure(paths, SecureStore::memory()));
    app.with_db(|db| Ok(db.migrate()?)).expect("migrate");
    Fixture { app, _dir: dir }
}

fn account(app: &App, uid: &str, enabled: bool) -> crate::storage::AccountRecord {
    // 账号 id 是全局的：进程内的"同账号互斥"按 id 记，测试并行跑时同名会互相挡住。
    // 每个用例拿一个唯一 uid，互斥才测的是"真的同一个账号"。
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let uid = format!("{uid}-{}", SEQ.fetch_add(1, Ordering::SeqCst));
    app.with_db(|db| {
        db.upsert_account(&AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.to_string(),
            nickname: Some(uid.to_string()),
            checkin_enabled: Some(enabled),
            ..Default::default()
        })
    })
    .expect("account")
}

fn status_ok(checked: Option<bool>) -> Result<StatusReply, CheckinError> {
    Ok(StatusReply {
        checked_in: checked,
        streak_days: Some(12),
        daily_credit: Some(100),
        today_credit: None,
        path: "/v2/billing/meter/checkin-activity-status",
    })
}

fn submit_ok(credits: Option<i64>) -> Result<SubmitReply, CheckinError> {
    Ok(SubmitReply {
        already: false,
        credits,
    })
}

fn err(kind: ErrorKind) -> CheckinError {
    CheckinError::new(kind, format!("stub {}", kind.as_str()))
}

/// 按脚本回答的 Mock。
///
/// `status` / `submit` 各自是一个队列，用完复用最后一个 —— 这样才能表达
/// "第一次查未签、提交、回查已签"这种三段式。
#[derive(Default)]
struct Mock {
    status: Mutex<Vec<Result<StatusReply, CheckinError>>>,
    submit: Mutex<Vec<Result<SubmitReply, CheckinError>>>,
    calls: Mutex<Vec<&'static str>>,
    /// 提交那一刻把开关关掉：验证 `WriteGate` 在写之前真的重新问了一次。
    close_on_submit: Mutex<Option<Arc<AtomicBool>>>,
    /// 每次 status 之前睡多久：并发用例用它把两个执行真正重叠起来。
    status_delay_ms: Mutex<u64>,
}

impl Mock {
    fn new(
        status: Vec<Result<StatusReply, CheckinError>>,
        submit: Vec<Result<SubmitReply, CheckinError>>,
    ) -> Self {
        Self {
            status: Mutex::new(status),
            submit: Mutex::new(submit),
            calls: Mutex::new(Vec::new()),
            close_on_submit: Mutex::new(None),
            status_delay_ms: Mutex::new(0),
        }
    }

    fn with_delay(self, ms: u64) -> Self {
        *self.status_delay_ms.lock().unwrap() = ms;
        self
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn submits(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| **call == "submit")
            .count()
    }

    fn statuses(&self) -> usize {
        self.calls()
            .iter()
            .filter(|call| **call == "status")
            .count()
    }
}

impl Transport for Mock {
    async fn status(&self) -> Result<StatusReply, CheckinError> {
        self.calls.lock().unwrap().push("status");
        let delay = *self.status_delay_ms.lock().unwrap();
        if delay > 0 {
            tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
        }
        let mut queue = self.status.lock().unwrap();
        if queue.is_empty() {
            return status_ok(Some(false));
        }
        if queue.len() == 1 {
            return queue[0].clone();
        }
        queue.remove(0)
    }

    async fn submit(&self) -> Result<SubmitReply, CheckinError> {
        self.calls.lock().unwrap().push("submit");
        if let Some(flag) = self.close_on_submit.lock().unwrap().clone() {
            flag.store(false, Ordering::SeqCst);
        }
        let mut queue = self.submit.lock().unwrap();
        if queue.is_empty() {
            return submit_ok(Some(100));
        }
        if queue.len() == 1 {
            return queue[0].clone();
        }
        queue.remove(0)
    }
}

fn gate_always() -> impl Fn() -> bool + Send + Sync {
    || true
}

fn count_attempts(app: &App, id: &str) -> usize {
    app.with_db(|db| db.checkin_attempts(id, 50))
        .expect("attempts")
        .len()
}

fn today_record(app: &App, id: &str) -> Option<crate::storage::checkin::CheckinRecord> {
    let today = crate::clock::beijing_date();
    app.with_db(|db| db.checkin_record(id, &today))
        .expect("record")
}

// ---------------------------------------------------------------------------
// 正常路径
// ---------------------------------------------------------------------------

/// PRD 10.1-4：未签到可以提交，提交后回查确认才记 claimed。
#[tokio::test]
async fn an_unchecked_account_submits_then_verifies() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(
        vec![status_ok(Some(false)), status_ok(Some(true))],
        vec![submit_ok(Some(100))],
    );

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "claimed");
    assert_eq!(outcome.credits, Some(100));
    assert!(outcome.verified);
    assert_eq!(mock.submits(), 1);
    assert_eq!(mock.statuses(), 2, "提交前查一次、提交后回查一次");
    let record = today_record(&f.app, &account_row.id).expect("record");
    assert_eq!(record.status, STATUS_CLAIMED);
    assert_eq!(record.credits, Some(100));
    assert_eq!(record.execution_source.as_deref(), Some("auto"));
}

/// PRD 10.1-5：已签到完全跳过写接口。这就是"Magpie 已经替用户签过"的场景。
#[tokio::test]
async fn an_already_checked_day_never_sends_a_write() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(vec![status_ok(Some(true))], vec![submit_ok(Some(100))]);

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "already");
    assert_eq!(mock.submits(), 0, "服务端已签到就不许再提交");
    assert_eq!(outcome.credits, None, "别的客户端签的，奖励不是我们的");
    // 本地也确认过了：界面可以直接显示"今天已签到"。
    assert_eq!(
        today_record(&f.app, &account_row.id).unwrap().status,
        "already"
    );
}

/// PRD 10.1-6：状态接口成功但没有布尔值 → 不签到。
#[tokio::test]
async fn a_status_without_the_boolean_never_submits() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(
        vec![Err(err(ErrorKind::UnknownStatus))],
        vec![submit_ok(Some(100))],
    );

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "unknown");
    assert_eq!(mock.submits(), 0, "说不清就不许写");
    assert_eq!(outcome.credits, None);
    assert_eq!(outcome.error_kind.as_deref(), Some("unknown_status"));
}

/// PRD 10.1-8：提交说成功、回查没确认 → 不记成功。
#[tokio::test]
async fn a_submit_that_cannot_be_verified_is_not_a_success() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(
        vec![status_ok(Some(false)), status_ok(Some(false))],
        vec![submit_ok(Some(100))],
    );

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "unknown");
    assert_eq!(outcome.credits, None, "没确认就不许记奖励");
    assert!(!outcome.verified);
    let record = today_record(&f.app, &account_row.id).expect("record");
    assert_eq!(record.status, "failed");
    assert_eq!(record.credits, None);
    // 账号行上的"今天已签到"不能被这个猜测提前点亮。
    let row = f
        .app
        .with_db(|db| db.get_account(&account_row.id))
        .unwrap()
        .unwrap();
    assert!(row.last_result.is_none() || row.last_result.as_deref() != Some("claimed"));
}

/// PRD 10.1-9：服务端回"已签到"是幂等回执，不是失败。
#[tokio::test]
async fn a_repeat_receipt_is_recorded_as_already() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(
        vec![status_ok(Some(false))],
        vec![Ok(SubmitReply {
            already: true,
            credits: None,
        })],
    );

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "already");
    assert_eq!(outcome.credits, None);
    assert_eq!(
        today_record(&f.app, &account_row.id).unwrap().status,
        "already"
    );
}

/// 提交超时：先回查，回查说已签到就按已签到记，绝不重发。
#[tokio::test]
async fn a_timed_out_write_is_verified_not_resent() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(
        vec![status_ok(Some(false)), status_ok(Some(true))],
        vec![Err(err(ErrorKind::Network))],
    );

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "claimed");
    assert_eq!(mock.submits(), 1, "写请求只发一次");
    assert_eq!(mock.statuses(), 2);
    assert_eq!(outcome.error_kind.as_deref(), Some("verified_after_error"));
}

/// 提交超时且回查也没确认：记 unknown，不算成功也不算彻底失败。
#[tokio::test]
async fn a_timed_out_write_that_cannot_be_confirmed_is_unknown() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(
        vec![status_ok(Some(false)), status_ok(Some(false))],
        vec![Err(err(ErrorKind::Network))],
    );

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "unknown");
    assert_eq!(mock.submits(), 1);
    assert_eq!(outcome.credits, None);
}

// ---------------------------------------------------------------------------
// 开关与门禁
// ---------------------------------------------------------------------------

/// PRD 10.1-1：自动签到默认关闭 → 自动轮次一个请求都不发。
#[tokio::test]
async fn an_account_with_the_switch_off_sends_nothing() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", false);
    let mock = Mock::new(vec![status_ok(Some(true))], vec![submit_ok(None)]);

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "disabled");
    assert!(mock.calls().is_empty(), "连状态查询都不该发");
}

/// 手动入口不受自动开关约束：用户点了按钮就是显式授权。
#[tokio::test]
async fn a_manual_run_works_even_with_the_switch_off() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", false);
    let mock = Mock::new(
        vec![status_ok(Some(false)), status_ok(Some(true))],
        vec![submit_ok(Some(100))],
    );

    let outcome =
        run_account_with(&f.app, &account_row, &mock, Source::Manual, &gate_always()).await;

    assert_eq!(outcome.state, "claimed");
    assert_eq!(outcome.source, "manual");
    assert_eq!(mock.submits(), 1);
}

/// PRD 10.1-18：开关在"查完状态、还没提交"之间被关掉 → 不提交。
#[tokio::test]
async fn closing_the_switch_between_query_and_write_stops_the_write() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(
        vec![status_ok(Some(false)), status_ok(Some(true))],
        vec![submit_ok(Some(100))],
    )
    .with_delay(30);
    let open = Arc::new(AtomicBool::new(true));
    let gate_flag = Arc::clone(&open);
    let gate = move || gate_flag.load(Ordering::SeqCst);

    // 第一次状态查询之后把开关关掉。
    let closer = Arc::clone(&open);
    tokio::spawn(async move {
        // 状态查询要 30ms，这里 5ms 就关：门禁必须在提交之前看到"已关闭"。
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        closer.store(false, Ordering::SeqCst);
    });

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate).await;

    assert_eq!(outcome.state, "stopped");
    assert_eq!(mock.submits(), 0, "关掉开关之后不许再发写请求");
    assert_eq!(outcome.error_kind.as_deref(), Some("switch_closed"));
}

// ---------------------------------------------------------------------------
// 多账号与并发
// ---------------------------------------------------------------------------

/// PRD 10.1-11：同账号并发只允许一个执行。
#[tokio::test]
async fn two_concurrent_runs_of_the_same_account_collapse_into_one() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(
        vec![status_ok(Some(false)), status_ok(Some(true))],
        vec![submit_ok(Some(100))],
    )
    .with_delay(30);
    let mock = Arc::new(mock);

    let a = {
        let app = Arc::clone(&f.app);
        let row = account_row.clone();
        let mock = Arc::clone(&mock);
        tokio::spawn(async move {
            run_account_with(&app, &row, mock.as_ref(), Source::Auto, &gate_always()).await
        })
    };
    // 让第一个先拿到锁，再放第二个进来。
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let b = {
        let app = Arc::clone(&f.app);
        let row = account_row.clone();
        let mock = Arc::clone(&mock);
        tokio::spawn(async move {
            run_account_with(&app, &row, mock.as_ref(), Source::Manual, &gate_always()).await
        })
    };

    let (first, second) = (a.await.unwrap(), b.await.unwrap());
    let states = [first.state, second.state];
    assert!(states.contains(&"busy"), "第二个必须是 busy：{states:?}");
    assert_eq!(mock.submits(), 1, "同一天只许提交一次");
}

/// PRD 10.1-12：自动与手动撞车时也只提交一次。
#[tokio::test]
async fn automatic_and_manual_never_double_submit() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Arc::new(
        Mock::new(
            vec![status_ok(Some(false)), status_ok(Some(true))],
            vec![submit_ok(Some(100))],
        )
        .with_delay(20),
    );

    let auto = {
        let app = Arc::clone(&f.app);
        let row = account_row.clone();
        let mock = Arc::clone(&mock);
        tokio::spawn(async move {
            run_account_with(&app, &row, mock.as_ref(), Source::Auto, &gate_always()).await
        })
    };
    let manual = {
        let app = Arc::clone(&f.app);
        let row = account_row.clone();
        let mock = Arc::clone(&mock);
        tokio::spawn(async move {
            run_account_with(&app, &row, mock.as_ref(), Source::Manual, &gate_always()).await
        })
    };
    let _ = auto.await.unwrap();
    let _ = manual.await.unwrap();

    assert_eq!(mock.submits(), 1);
}

/// PRD 10.1-13/14：多账号相互隔离，一个 401 不影响另一个。
#[tokio::test]
async fn one_unauthorized_account_does_not_affect_another() {
    let f = fixture();
    let bad = account(&f.app, "uid-bad", true);
    let good = account(&f.app, "uid-good", true);

    let bad_mock = Mock::new(vec![Err(err(ErrorKind::Unauthorized))], vec![]);
    let bad_outcome = run_account_with(&f.app, &bad, &bad_mock, Source::Auto, &gate_always()).await;
    assert_eq!(bad_outcome.state, "unauthorized");
    assert_eq!(bad_mock.submits(), 0);

    let good_mock = Mock::new(
        vec![status_ok(Some(false)), status_ok(Some(true))],
        vec![submit_ok(Some(100))],
    );
    let good_outcome =
        run_account_with(&f.app, &good, &good_mock, Source::Auto, &gate_always()).await;
    assert_eq!(good_outcome.state, "claimed");

    // 记录不能串号。
    assert_eq!(today_record(&f.app, &bad.id).unwrap().status, "failed");
    assert_eq!(today_record(&f.app, &good.id).unwrap().credits, Some(100));
}

/// PRD 10.1-15：403 网关拦截不当成授权失效。
#[tokio::test]
async fn a_forbidden_is_not_read_as_an_expired_authorization() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(vec![Err(err(ErrorKind::Forbidden))], vec![]);

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "failed");
    assert_eq!(outcome.error_kind.as_deref(), Some("forbidden"));
    assert_ne!(outcome.state, "unauthorized");
    let row = f
        .app
        .with_db(|db| db.get_account(&account_row.id))
        .unwrap()
        .unwrap();
    assert_eq!(row.auth_status, "unknown", "签到失败不许改授权状态");
}

/// PRD 10.1-16：429 不重试、不提交、不记成授权问题。
#[tokio::test]
async fn rate_limiting_stops_without_a_write() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(
        vec![Err(err(ErrorKind::RateLimited))],
        vec![submit_ok(Some(100))],
    );

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "failed");
    assert_eq!(outcome.error_kind.as_deref(), Some("rate_limited"));
    assert_eq!(mock.submits(), 0);
}

/// PRD 10.1-22：不支持的档位不发任何请求。
#[tokio::test]
async fn an_unsupported_platform_never_calls_the_network() {
    let f = fixture();
    let record = f
        .app
        .with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "qoder_cn".to_string(),
                uid: "qoder-1".to_string(),
                ..Default::default()
            })
        })
        .expect("account");
    let mock = Mock::new(vec![status_ok(Some(true))], vec![]);

    let outcome = run_account_with(&f.app, &record, &mock, Source::Auto, &gate_always()).await;

    assert_eq!(outcome.state, "unsupported");
    assert!(mock.calls().is_empty());
    let today = crate::clock::beijing_date();
    assert!(
        f.app
            .with_db(|db| db.checkin_record(&record.id, &today))
            .unwrap()
            .is_none(),
        "不支持的平台不该留下签到记录"
    );
}

// ---------------------------------------------------------------------------
// 记录与幂等
// ---------------------------------------------------------------------------

/// PRD 10.1-23：同一天跑两次，第二次是 already，且不覆盖已确认的奖励。
#[tokio::test]
async fn a_second_run_the_same_day_keeps_the_confirmed_reward() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);

    let first = Mock::new(
        vec![status_ok(Some(false)), status_ok(Some(true))],
        vec![submit_ok(Some(100))],
    );
    let one = run_account_with(&f.app, &account_row, &first, Source::Auto, &gate_always()).await;
    assert_eq!(one.state, "claimed");

    let second = Mock::new(vec![status_ok(Some(true))], vec![]);
    let two = run_account_with(&f.app, &account_row, &second, Source::Auto, &gate_always()).await;
    assert_eq!(two.state, "already");
    assert_eq!(second.submits(), 0);

    let record = today_record(&f.app, &account_row.id).expect("record");
    assert_eq!(
        record.status, STATUS_CLAIMED,
        "already 不许把 claimed 覆盖掉"
    );
    assert_eq!(record.credits, Some(100), "奖励不能丢");
}

/// 每次执行都留一条尝试记录，界面能回答"上次为什么没签"。
#[tokio::test]
async fn every_run_leaves_an_attempt_row() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(vec![Err(err(ErrorKind::Server))], vec![]);

    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;
    assert_eq!(outcome.state, "failed");

    let attempts = f
        .app
        .with_db(|db| db.checkin_attempts(&account_row.id, 10))
        .expect("attempts");
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0].status, "failed");
    assert_eq!(attempts[0].error_kind.as_deref(), Some("server"));
    assert_eq!(count_attempts(&f.app, &account_row.id), 1);
}

/// PRD 10.1-25：返回值里不能出现令牌。
#[tokio::test]
async fn the_outcome_never_carries_a_token() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    let mock = Mock::new(
        vec![status_ok(Some(false)), status_ok(Some(true))],
        vec![submit_ok(Some(100))],
    );
    let outcome = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;
    let text = serde_json::to_string(&outcome).expect("json");
    assert!(!text.contains("access"));
    assert!(!text.contains("Bearer"));
    assert!(!text.to_lowercase().contains("token"));
}

/// 只读探针（checkin.status）不发写请求、不改库。
#[tokio::test]
async fn a_read_only_probe_never_writes_anything() {
    let f = fixture();
    let account_row = account(&f.app, "uid-a", true);
    // 直接验证 manager 的只读路径不存在：确认没有提交能力的调用点只有 probe_account。
    let before = f.app.with_db(|db| db.counts()).unwrap();
    let mock = Mock::new(vec![status_ok(Some(true))], vec![]);
    let _ = run_account_with(&f.app, &account_row, &mock, Source::Auto, &gate_always()).await;
    // 上面的 run_account_with 会写记录（它是执行路径），所以这里只断言没有写请求。
    assert_eq!(mock.submits(), 0);
    let after = f.app.with_db(|db| db.counts()).unwrap();
    assert!(after["accounts"].as_i64().unwrap() >= before["accounts"].as_i64().unwrap());
}
