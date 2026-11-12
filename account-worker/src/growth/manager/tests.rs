//! 接单 / 领奖流程的离线测试。
//!
//! 全部用 Mock `Transport`，**没有一个真实请求**：CI 里绝不允许对真实账号发 POST。
//! 覆盖的是那些"看起来能跑但会悄悄做错事"的分支：超时后回查、`already_claimed`、
//! 缺 `results` 数组、限流、授权失效、未知状态。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::*;
use crate::paths::Paths;
use crate::securestore::SecureStore;
use crate::storage::growth::{OUTCOME_OK, OUTCOME_REJECTED, OUTCOME_UNKNOWN};

// ---------------------------------------------------------------------------
// 测试脚手架
// ---------------------------------------------------------------------------

fn test_app() -> (std::sync::Arc<App>, tempfile::TempDir) {
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
    let app = std::sync::Arc::new(App::with_secure(paths, SecureStore::memory()));
    app.with_db(|db| Ok(db.migrate()?)).expect("migrate");
    (app, dir)
}

fn account(app: &App, uid: &str) -> String {
    app.with_db(|db| {
        db.upsert_account(&crate::storage::AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.to_string(),
            nickname: Some(uid.to_string()),
            ..Default::default()
        })
    })
    .expect("account")
    .id
}

fn task(code: &str, status: &str, locked: bool) -> Value {
    json!({
        "task_code": code,
        "title": format!("任务 {code}"),
        "accept_status": status,
        "locked": locked,
        "reward_credit": 100,
        "reward_energy": 5,
    })
}

fn tasks_body(items: Vec<Value>) -> Value {
    json!({ "code": 0, "data": { "tasks": items } })
}

fn err(kind: ErrorKind, message: &str) -> GrowthError {
    GrowthError::new(kind, message)
}

/// 按序返回的 Mock：`tasks` 队列用完最后一个会一直复用。
#[derive(Default)]
struct Mock {
    tasks: Mutex<Vec<Value>>,
    /// 每次 accept 依次取一条；用完复用最后一个。
    accepts: Mutex<Vec<Result<Value, GrowthError>>>,
    /// claim 按 task_code 取；没配的返回 `task not completed`。
    claims: Mutex<Vec<(String, Result<Value, GrowthError>)>>,
    calls: Mutex<Vec<String>>,
    /// 第 N 次 accept / claim 之后把开关关掉，模拟用户在运行途中点关闭。
    close_after_accept: Mutex<Option<(Arc<AtomicBool>, usize)>>,
    close_after_claim: Mutex<Option<(Arc<AtomicBool>, usize)>>,
    accept_count: AtomicUsize,
    claim_count: AtomicUsize,
}

impl Mock {
    fn with_tasks(items: Vec<Value>) -> Self {
        Self {
            tasks: Mutex::new(vec![tasks_body(items)]),
            ..Default::default()
        }
    }

    fn tasks_pages(mut self, pages: Vec<Value>) -> Self {
        self.tasks = Mutex::new(pages);
        self
    }

    fn accept_reply(self, reply: Result<Value, GrowthError>) -> Self {
        self.accepts.lock().unwrap().push(reply);
        self
    }

    fn claim_reply(self, code: &str, reply: Result<Value, GrowthError>) -> Self {
        self.claims.lock().unwrap().push((code.to_string(), reply));
        self
    }

    fn close_switch_after_accept(self, switch: Arc<AtomicBool>, after: usize) -> Self {
        *self.close_after_accept.lock().unwrap() = Some((switch, after));
        self
    }

    fn close_switch_after_claim(self, switch: Arc<AtomicBool>, after: usize) -> Self {
        *self.close_after_claim.lock().unwrap() = Some((switch, after));
        self
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn write_calls(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| call.starts_with("accept") || call.starts_with("claim"))
            .collect()
    }
}

impl Transport for Mock {
    async fn tasks(&self) -> Result<Value, GrowthError> {
        self.calls.lock().unwrap().push("tasks".to_string());
        let mut queue = self.tasks.lock().unwrap();
        if queue.len() > 1 {
            Ok(queue.remove(0))
        } else {
            Ok(queue
                .first()
                .cloned()
                .unwrap_or(json!({"code": 0, "data": {"tasks": []}})))
        }
    }

    async fn accept(&self, codes: &[String]) -> Result<Value, GrowthError> {
        self.calls
            .lock()
            .unwrap()
            .push(format!("accept:{}", codes.join(",")));
        let count = self.accept_count.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((switch, after)) = self.close_after_accept.lock().unwrap().as_ref() {
            if count >= *after {
                switch.store(false, Ordering::SeqCst);
            }
        }
        let mut queue = self.accepts.lock().unwrap();
        if queue.len() > 1 {
            queue.remove(0)
        } else {
            queue
                .first()
                .cloned()
                .unwrap_or(Ok(json!({"code": 0, "data": {"results": []}})))
        }
    }

    async fn claim(&self, code: &str) -> Result<Value, GrowthError> {
        self.calls.lock().unwrap().push(format!("claim:{code}"));
        let count = self.claim_count.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some((switch, after)) = self.close_after_claim.lock().unwrap().as_ref() {
            if count >= *after {
                switch.store(false, Ordering::SeqCst);
            }
        }
        let mut queue = self.claims.lock().unwrap();
        // 同一个任务可以配多条回复，按顺序消费 —— "第一次被拒、第二次成功"
        // 这种跨轮次的场景只有这样才测得出来。
        if let Some(index) = queue.iter().position(|(target, _)| target == code) {
            return queue.remove(index).1;
        }
        Err(err(ErrorKind::Business, "task not completed"))
    }
}

fn accept_ok(codes: &[&str]) -> Value {
    json!({
        "code": 0,
        "data": {"results": codes.iter().map(|c| json!({"task_code": c, "status": "ok"})).collect::<Vec<_>>()}
    })
}

fn claim_ok(credit: i64, energy: i64) -> Value {
    json!({"code": 0, "msg": "OK", "data": {"already_claimed": false, "credit": credit, "energy": energy}})
}

fn claim_already() -> Value {
    json!({"code": 0, "msg": "OK", "data": {"already_claimed": true, "credit": 0, "energy": 0}})
}

async fn run(app: &App, id: &str, mock: &Mock, options: Options) -> AccountRun {
    let open = || true;
    run_account_with(app, id, "测试号", mock, options, &open).await
}

/// 带可开关 gate 的版本：用来验证"跑到一半关开关"。
async fn run_gated(
    app: &App,
    id: &str,
    mock: &Mock,
    options: Options,
    gate: WriteGate<'_>,
) -> AccountRun {
    run_account_with(app, id, "测试号", mock, options, gate).await
}

// ---------------------------------------------------------------------------
// §八 1 / 2 / 4：19 项任务的资格判断与接单
// ---------------------------------------------------------------------------

#[test]
fn plan_of_counts_the_real_nineteen_task_snapshot() {
    // 2026-11-12 卡里的真实分布：15 claimed / 1 in_progress / 3 not_accepted。
    let mut tasks = Vec::new();
    for i in 0..15 {
        tasks.push(parse_task(&task(&format!("claimed_{i}"), "claimed", false)).unwrap());
    }
    tasks.push(parse_task(&task("running", "in_progress", false)).unwrap());
    for i in 0..3 {
        tasks.push(parse_task(&task(&format!("todo_{i}"), "not_accepted", false)).unwrap());
    }
    assert_eq!(tasks.len(), 19);
    assert_eq!(plan_of(&tasks), (3, 0), "只有 3 个待接单，没有待领奖");

    // 把两个换成 completed：可领奖 2，可接单仍是 3。
    tasks[0] = parse_task(&task("done_a", "completed", false)).unwrap();
    tasks[1] = parse_task(&task("done_b", "completed", false)).unwrap();
    assert_eq!(plan_of(&tasks), (3, 2));
}

#[tokio::test]
async fn accepts_not_accepted_and_skips_the_rest() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-accept");
    let mock = Mock::with_tasks(vec![
        task("todo_a", "not_accepted", false),
        task("todo_b", "not_accepted", false),
        task("done", "completed", false),
        task("running", "in_progress", false),
        task("got", "claimed", false),
    ])
    .accept_reply(Ok(accept_ok(&["todo_a", "todo_b"])))
    .claim_reply("done", Ok(claim_ok(100, 5)))
    // 三次读取：初始 → 接单后 → 领奖后复核
    .tasks_pages(vec![
        tasks_body(vec![
            task("todo_a", "not_accepted", false),
            task("todo_b", "not_accepted", false),
            task("done", "completed", false),
            task("running", "in_progress", false),
            task("got", "claimed", false),
        ]),
        tasks_body(vec![
            task("todo_a", "accepted", false),
            task("todo_b", "accepted", false),
            task("done", "completed", false),
            task("running", "in_progress", false),
            task("got", "claimed", false),
        ]),
        tasks_body(vec![
            task("todo_a", "accepted", false),
            task("todo_b", "accepted", false),
            task("done", "claimed", false),
            task("running", "in_progress", false),
            task("got", "claimed", false),
        ]),
    ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.accepted, 2, "{run:?}");
    assert_eq!(run.claimed, 1);
    assert_eq!(run.already_claimed, 0);

    // claimed 的任务不该再被接单；in_progress 也不该。
    let accept_call = mock
        .calls()
        .into_iter()
        .find(|c| c.starts_with("accept:"))
        .expect("accept called");
    assert_eq!(accept_call, "accept:todo_a,todo_b");
}

#[tokio::test]
async fn accept_is_capped_at_twenty_per_batch() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-batch");
    let items: Vec<Value> = (0..25)
        .map(|i| task(&format!("t{i}"), "not_accepted", false))
        .collect();
    let first_batch: Vec<String> = (0..20).map(|i| format!("t{i}")).collect();
    let first_batch: Vec<&str> = first_batch.iter().map(String::as_str).collect();
    // 接单后必须回查到"已接受"，否则新逻辑会全部记 not_confirmed。
    let after: Vec<Value> = (0..25)
        .map(|i| task(&format!("t{i}"), "accepted", false))
        .collect();
    let mock = Mock::with_tasks(items)
        .accept_reply(Ok(accept_ok(&first_batch)))
        .accept_reply(Ok(accept_ok(&["t20", "t21", "t22", "t23", "t24"])))
        .tasks_pages(vec![
            tasks_body(
                (0..25)
                    .map(|i| task(&format!("t{i}"), "not_accepted", false))
                    .collect(),
            ),
            tasks_body(after),
        ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.accepted, 25, "{run:?}");
    let accepts: Vec<String> = mock
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("accept:"))
        .collect();
    assert_eq!(accepts.len(), 2, "25 个要分两批");
    assert_eq!(accepts[0].split(',').count(), 20, "第一批 20 个");
    assert_eq!(accepts[1].split(',').count(), 5);
}

#[tokio::test]
async fn rejected_accept_keeps_the_server_message() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-reject");
    let mock = Mock::with_tasks(vec![task("needs_buddy", "not_accepted", false)]).accept_reply(Ok(
        json!({"code": 0, "data": {"results": [
            {"task_code": "needs_buddy", "status": "error",
             "message": "prerequisite not met: first_buddy"}
        ]}}),
    ));

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.rejected, 1);
    assert_eq!(run.accepted, 0);
    let entry = &run.tasks[0];
    assert_eq!(entry.outcome, OUTCOME_REJECTED);
    assert!(entry
        .error_detail
        .as_deref()
        .unwrap()
        .contains("first_buddy"));
}

#[tokio::test]
async fn accept_without_results_is_unknown_and_not_counted() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-noresults");
    let mock = Mock::with_tasks(vec![task("t1", "not_accepted", false)])
        .accept_reply(Ok(json!({"code": 0, "msg": "OK"})));

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.accepted, 0, "没有 results 不能算接单成功");
    assert_eq!(run.tasks[0].outcome, OUTCOME_UNKNOWN);
}

// ---------------------------------------------------------------------------
// §八 7 / 8 / 9 / 10 / 11：领奖
// ---------------------------------------------------------------------------

#[tokio::test]
async fn completed_task_is_claimed_and_verified_against_the_server() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-claim");
    let mock = Mock::with_tasks(vec![task("done", "completed", false)])
        .claim_reply("done", Ok(claim_ok(100, 5)))
        // 领完之后回查：服务端说 claimed
        .tasks_pages(vec![
            tasks_body(vec![task("done", "completed", false)]),
            tasks_body(vec![task("done", "claimed", false)]),
        ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.claimed, 1, "{run:?}");
    assert_eq!(run.credit, 100);
    assert_eq!(run.energy, 5);
    assert_eq!(run.tasks[0].verified_status.as_deref(), Some("claimed"));
    assert_eq!(run.tasks[0].outcome, OUTCOME_OK);
}

#[tokio::test]
async fn already_claimed_is_neither_success_nor_failure() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-already");
    let mock = Mock::with_tasks(vec![task("done", "completed", false)])
        .claim_reply("done", Ok(claim_already()))
        .tasks_pages(vec![
            tasks_body(vec![task("done", "completed", false)]),
            tasks_body(vec![task("done", "claimed", false)]),
        ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.already_claimed, 1);
    assert_eq!(run.claimed, 0, "已领过不能算新领到");
    assert_eq!(run.credit, 0, "也不能把 0 积分算成奖励");
    assert_eq!(run.rejected, 0);
}

#[tokio::test]
async fn claim_timeout_recovers_by_rechecking_the_server() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-timeout");
    let mock = Mock::with_tasks(vec![task("done", "completed", false)])
        .claim_reply("done", Err(err(ErrorKind::Network, "请求超时（15s）")))
        .tasks_pages(vec![
            tasks_body(vec![task("done", "completed", false)]),
            // 超时其实已经生效：回查看到 claimed，就该认定到账
            tasks_body(vec![task("done", "claimed", false)]),
        ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.claimed, 1, "{run:?}");
    assert_eq!(run.tasks[0].outcome, OUTCOME_OK);
    assert_eq!(run.tasks[0].error_kind, None, "复核到账后不该留错误标记");
    // 只发了一次 claim：超时不重发
    let claims: Vec<String> = mock
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("claim:"))
        .collect();
    assert_eq!(claims.len(), 1, "写操作零重试");
}

#[tokio::test]
async fn claim_that_did_not_land_is_not_counted_as_success() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-notlanded");
    let mock = Mock::with_tasks(vec![task("done", "completed", false)])
        .claim_reply("done", Ok(claim_ok(100, 5)))
        // 服务端说发了，但状态没变 —— 不能记成功
        .tasks_pages(vec![
            tasks_body(vec![task("done", "completed", false)]),
            tasks_body(vec![task("done", "completed", false)]),
        ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.claimed, 0, "{run:?}");
    assert_eq!(run.credit, 0);
    assert_eq!(run.tasks[0].outcome, OUTCOME_UNKNOWN);
    assert_eq!(run.tasks[0].error_kind.as_deref(), Some("not_landed"));
}

#[tokio::test]
async fn rate_limited_claim_is_recorded_unknown_without_retry() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-429");
    let mock = Mock::with_tasks(vec![task("done", "completed", false)])
        .claim_reply(
            "done",
            Err(err(ErrorKind::RateLimited, "too many requests")),
        )
        .tasks_pages(vec![
            tasks_body(vec![task("done", "completed", false)]),
            tasks_body(vec![task("done", "completed", false)]),
        ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(
        run.tasks[0].outcome, OUTCOME_UNKNOWN,
        "限流不是失败也不是成功"
    );
    assert_eq!(run.tasks[0].error_kind.as_deref(), Some("rate_limited"));
    assert_eq!(
        mock.calls()
            .iter()
            .filter(|c| c.starts_with("claim:"))
            .count(),
        1,
        "429 之后不许重发"
    );
}

#[tokio::test]
async fn one_rejected_claim_does_not_block_the_next_task() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-mixed");
    let mock = Mock::with_tasks(vec![
        task("bad", "completed", false),
        task("good", "completed", false),
    ])
    .claim_reply("bad", Err(err(ErrorKind::Business, "task not completed")))
    .claim_reply("good", Ok(claim_ok(50, 0)))
    .tasks_pages(vec![
        tasks_body(vec![
            task("bad", "completed", false),
            task("good", "completed", false),
        ]),
        tasks_body(vec![
            task("bad", "completed", false),
            task("good", "claimed", false),
        ]),
    ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.rejected, 1);
    assert_eq!(run.claimed, 1, "一个任务被拒不该影响后面的");
    assert_eq!(run.credit, 50);
}

// ---------------------------------------------------------------------------
// §八 5 / 13 / 17：不该写的情况
// ---------------------------------------------------------------------------

#[tokio::test]
async fn in_progress_and_unknown_never_trigger_writes() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-nowrite");
    let mock = Mock::with_tasks(vec![
        task("running", "in_progress", false),
        task("weird", "brand_new_status", false),
        task("locked_done", "completed", true),
    ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert!(
        mock.write_calls().is_empty(),
        "不该有任何写请求：{:?}",
        mock.write_calls()
    );
    assert_eq!(run.accepted, 0);
    assert_eq!(run.claimed, 0);
}

#[tokio::test]
async fn unauthorized_on_tasks_stops_the_account_without_writing() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-401");
    // 用一个永远 401 的 mock：tasks 走 Err 分支
    struct Unauthorized;
    impl Transport for Unauthorized {
        async fn tasks(&self) -> Result<Value, GrowthError> {
            Err(err(ErrorKind::Unauthorized, "unauthorized"))
        }
        async fn accept(&self, _codes: &[String]) -> Result<Value, GrowthError> {
            panic!("授权失效时不能接单");
        }
        async fn claim(&self, _code: &str) -> Result<Value, GrowthError> {
            panic!("授权失效时不能领奖");
        }
    }

    let open = || true;
    let run = run_account_with(&app, &id, "测试号", &Unauthorized, Options::FULL, &open).await;
    assert_eq!(run.state, "unauthorized");
    assert_eq!(run.accepted, 0);
    assert_eq!(run.claimed, 0);
}

#[tokio::test]
async fn read_only_options_never_call_the_write_endpoints() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-readonly");
    let mock = Mock::with_tasks(vec![
        task("todo", "not_accepted", false),
        task("done", "completed", false),
    ]);

    let run = run(&app, &id, &mock, Options::READ_ONLY).await;
    assert!(mock.write_calls().is_empty(), "{:?}", mock.write_calls());
    assert_eq!(run.accepted, 0);
    assert_eq!(run.claimed, 0);
}

// ---------------------------------------------------------------------------
// §八 12：多账号隔离
// ---------------------------------------------------------------------------

#[tokio::test]
async fn accounts_stay_isolated_in_records_and_counts() {
    let (app, _dir) = test_app();
    let a = account(&app, "uid-a");
    let b = account(&app, "uid-b");

    let mock_a = Mock::with_tasks(vec![task("a_task", "completed", false)])
        .claim_reply("a_task", Ok(claim_ok(100, 5)))
        .tasks_pages(vec![
            tasks_body(vec![task("a_task", "completed", false)]),
            tasks_body(vec![task("a_task", "claimed", false)]),
        ]);
    let mock_b = Mock::with_tasks(vec![task("b_task", "not_accepted", false)])
        .accept_reply(Ok(accept_ok(&["b_task"])))
        .tasks_pages(vec![
            tasks_body(vec![task("b_task", "not_accepted", false)]),
            tasks_body(vec![task("b_task", "accepted", false)]),
        ]);

    let run_a = run(&app, &a, &mock_a, Options::FULL).await;
    let run_b = run(&app, &b, &mock_b, Options::FULL).await;

    assert_eq!(run_a.claimed, 1);
    assert_eq!(run_a.accepted, 0);
    assert_eq!(run_b.accepted, 1);
    assert_eq!(run_b.claimed, 0);

    let runs_a = app.with_db(|db| db.growth_runs(&a, 20)).unwrap();
    let runs_b = app.with_db(|db| db.growth_runs(&b, 20)).unwrap();
    assert!(runs_a.iter().all(|r| r.task_code == "a_task"), "{runs_a:?}");
    assert!(runs_b.iter().all(|r| r.task_code == "b_task"), "{runs_b:?}");

    // 累计积分也只算 A 的
    assert_eq!(app.with_db(|db| db.growth_totals(&a)).unwrap(), (100, 5));
    assert_eq!(app.with_db(|db| db.growth_totals(&b)).unwrap(), (0, 0));
}

// ---------------------------------------------------------------------------
// 记录与预算
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_write_leaves_a_run_record() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-record");
    let mock = Mock::with_tasks(vec![
        task("todo", "not_accepted", false),
        task("done", "completed", false),
    ])
    .accept_reply(Ok(accept_ok(&["todo"])))
    .claim_reply("done", Ok(claim_ok(100, 5)))
    .tasks_pages(vec![
        tasks_body(vec![
            task("todo", "not_accepted", false),
            task("done", "completed", false),
        ]),
        tasks_body(vec![
            task("todo", "accepted", false),
            task("done", "completed", false),
        ]),
        tasks_body(vec![
            task("todo", "accepted", false),
            task("done", "claimed", false),
        ]),
    ]);

    run(&app, &id, &mock, Options::FULL).await;
    let runs = app.with_db(|db| db.growth_runs(&id, 20)).unwrap();
    assert_eq!(runs.len(), 2, "{runs:?}");
    assert!(runs
        .iter()
        .any(|r| r.action == "accept" && r.outcome == OUTCOME_OK));
    assert!(runs
        .iter()
        .any(|r| r.action == "claim" && r.outcome == OUTCOME_OK));
}

#[tokio::test]
async fn claim_budget_leaves_the_rest_for_the_next_round() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-budget");
    let items: Vec<Value> = (0..(CLAIM_BUDGET + 2))
        .map(|i| task(&format!("d{i}"), "completed", false))
        .collect();
    let mut mock = Mock::with_tasks(items.clone());
    for i in 0..(CLAIM_BUDGET + 2) {
        mock = mock.claim_reply(&format!("d{i}"), Ok(claim_ok(10, 0)));
    }
    // 复验页：所有任务都变成 claimed，否则"没落地"分支会全部记成 unknown。
    mock = mock.tasks_pages(vec![
        tasks_body(items.clone()),
        tasks_body(
            (0..(CLAIM_BUDGET + 2))
                .map(|i| task(&format!("d{i}"), "claimed", false))
                .collect(),
        ),
    ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.claimed, CLAIM_BUDGET, "一轮最多领这么多");
    assert!(
        run.errors.iter().any(|e| e.contains("上限")),
        "剩下的要留到下一轮并说明：{:?}",
        run.errors
    );
}

// ---------------------------------------------------------------------------
// 验收修正：接单确认、401 立即停止、task not completed
// ---------------------------------------------------------------------------

/// 接口说 ok、但服务端状态没动：不许记成功。
#[tokio::test]
async fn accept_that_did_not_change_the_status_is_not_counted() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-accept-noland");
    let mock = Mock::with_tasks(vec![task("todo", "not_accepted", false)])
        .accept_reply(Ok(accept_ok(&["todo"])))
        // 回查还是 not_accepted
        .tasks_pages(vec![
            tasks_body(vec![task("todo", "not_accepted", false)]),
            tasks_body(vec![task("todo", "not_accepted", false)]),
        ]);

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.accepted, 0, "状态没变就不能说接单成功：{run:?}");
    assert_eq!(run.tasks[0].outcome, OUTCOME_UNKNOWN);
    assert_eq!(run.tasks[0].error_kind.as_deref(), Some("not_confirmed"));
    assert_eq!(
        run.tasks[0].verified_status.as_deref(),
        Some("not_accepted")
    );
}

/// 接单首批就 401：剩下的批次一个都不许发，领奖阶段也不许进。
#[tokio::test]
async fn unauthorized_during_accept_stops_every_remaining_write() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-accept-401");
    // 25 个待接单 = 两批。第一批就 401。
    let items: Vec<Value> = (0..25)
        .map(|i| task(&format!("t{i}"), "not_accepted", false))
        .collect();
    let mock =
        Mock::with_tasks(items).accept_reply(Err(err(ErrorKind::Unauthorized, "unauthorized")));

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.state, "unauthorized");
    let writes = mock.write_calls();
    assert_eq!(writes.len(), 1, "只该发出第一批那一次：{writes:?}");
    assert!(writes[0].starts_with("accept:"));
    assert!(
        !writes.iter().any(|c| c.starts_with("claim:")),
        "授权失效后不许再进领奖：{writes:?}"
    );
}

/// 领奖过程中 401：当前账号剩下的任务不再领。
#[tokio::test]
async fn unauthorized_during_claim_stops_the_remaining_tasks() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-claim-401");
    let mock = Mock::with_tasks(vec![
        task("a", "completed", false),
        task("b", "completed", false),
        task("c", "completed", false),
    ])
    .claim_reply("a", Err(err(ErrorKind::Unauthorized, "unauthorized")))
    .claim_reply("b", Ok(claim_ok(100, 5)))
    .claim_reply("c", Ok(claim_ok(100, 5)));

    let run = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(run.state, "unauthorized");
    let claims: Vec<String> = mock
        .calls()
        .into_iter()
        .filter(|c| c.starts_with("claim:"))
        .collect();
    assert_eq!(claims, vec!["claim:a"], "401 之后不该再领：{claims:?}");
    assert_eq!(run.claimed, 0);
}

/// 一个账号 401 不能拖累另一个账号。
#[tokio::test]
async fn one_unauthorized_account_does_not_stop_another() {
    let (app, _dir) = test_app();
    let bad = account(&app, "uid-bad");
    let good = account(&app, "uid-good");

    let bad_mock = Mock::with_tasks(vec![task("x", "not_accepted", false)])
        .accept_reply(Err(err(ErrorKind::Unauthorized, "unauthorized")));
    let good_mock = Mock::with_tasks(vec![task("y", "not_accepted", false)])
        .accept_reply(Ok(accept_ok(&["y"])))
        .tasks_pages(vec![
            tasks_body(vec![task("y", "not_accepted", false)]),
            tasks_body(vec![task("y", "accepted", false)]),
        ]);

    let run_bad = run(&app, &bad, &bad_mock, Options::FULL).await;
    let run_good = run(&app, &good, &good_mock, Options::FULL).await;

    assert_eq!(run_bad.state, "unauthorized");
    assert_eq!(run_good.state, "ok");
    assert_eq!(run_good.accepted, 1, "另一个账号照常接单");
}

/// `task not completed` 是常态：记 rejected，不算系统错误、不算积分，
/// 下一轮重读状态还能正常领。
#[tokio::test]
async fn task_not_completed_is_rejected_and_retried_next_round() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-notcompleted");
    let mock = Mock::with_tasks(vec![task("done", "completed", false)])
        .claim_reply("done", Err(err(ErrorKind::Business, "task not completed")))
        .claim_reply("done", Ok(claim_ok(100, 5)))
        .tasks_pages(vec![
            // 第一轮：状态还是 completed（说明刚才那下确实不该领）
            tasks_body(vec![task("done", "completed", false)]),
            // 第二轮重读：仍是 completed，这次接口给成功
            tasks_body(vec![task("done", "completed", false)]),
            // 领完复核
            tasks_body(vec![task("done", "claimed", false)]),
        ]);

    let first = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(first.rejected, 1, "记 rejected：{first:?}");
    assert_eq!(first.claimed, 0, "没领到就不许记成功");
    assert_eq!(first.credit, 0, "更不能记积分");
    assert_eq!(first.tasks[0].error_kind.as_deref(), Some("not_completed"));
    assert_ne!(first.state, "failed", "这不是系统故障");

    // 第二轮：服务端状态允许了就照常领
    let second = run(&app, &id, &mock, Options::FULL).await;
    assert_eq!(second.claimed, 1, "{second:?}");
    assert_eq!(second.credit, 100);
}

// ---------------------------------------------------------------------------
// 开关在运行途中被关掉：后续 POST 一个都不许发
// ---------------------------------------------------------------------------

/// 第一批接单发出去之后用户关掉开关：第二批不许再发。
#[tokio::test]
async fn closing_the_switch_after_the_first_accept_batch_stops_the_second() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-close-accept");
    let switch = Arc::new(AtomicBool::new(true));

    let items: Vec<Value> = (0..25)
        .map(|i| task(&format!("t{i}"), "not_accepted", false))
        .collect();
    let first_batch: Vec<String> = (0..20).map(|i| format!("t{i}")).collect();
    let first_ref: Vec<&str> = first_batch.iter().map(String::as_str).collect();
    let mock = Mock::with_tasks(items)
        .accept_reply(Ok(accept_ok(&first_ref)))
        .close_switch_after_accept(switch.clone(), 1);

    let gate = {
        let switch = switch.clone();
        move || switch.load(Ordering::SeqCst)
    };
    let run = run_gated(&app, &id, &mock, Options::FULL, &gate).await;

    let accepts = mock
        .calls()
        .iter()
        .filter(|call| call.starts_with("accept:"))
        .count();
    assert_eq!(accepts, 1, "开关关了就不该再发第二批");
    assert!(run.stopped_by_user, "{run:?}");
    assert!(
        run.errors.iter().any(|e| e.contains("中止剩余接单")),
        "{:?}",
        run.errors
    );
}

/// 领完第一个之后关掉开关：剩下的任务一个都不领，
/// 但**已经发出去的那一个照常复核**。
#[tokio::test]
async fn closing_the_switch_after_one_claim_stops_the_rest_but_still_verifies() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-close-claim");
    let switch = Arc::new(AtomicBool::new(true));

    let mock = Mock::with_tasks(vec![
        task("a", "completed", false),
        task("b", "completed", false),
        task("c", "completed", false),
    ])
    .claim_reply("a", Ok(claim_ok(100, 5)))
    .claim_reply("b", Ok(claim_ok(100, 5)))
    .claim_reply("c", Ok(claim_ok(100, 5)))
    .close_switch_after_claim(switch.clone(), 1)
    .tasks_pages(vec![
        tasks_body(vec![
            task("a", "completed", false),
            task("b", "completed", false),
            task("c", "completed", false),
        ]),
        // a 领完的复核页
        tasks_body(vec![
            task("a", "claimed", false),
            task("b", "completed", false),
            task("c", "completed", false),
        ]),
    ]);

    let gate = {
        let switch = switch.clone();
        move || switch.load(Ordering::SeqCst)
    };
    let run = run_gated(&app, &id, &mock, Options::FULL, &gate).await;

    let claims: Vec<String> = mock
        .calls()
        .into_iter()
        .filter(|call| call.starts_with("claim:"))
        .collect();
    assert_eq!(claims, vec!["claim:a"], "只该领第一个：{claims:?}");
    assert_eq!(run.claimed, 1, "已发出的那个要照常算");
    assert_eq!(run.credit, 100);
    assert_eq!(
        run.tasks[0].verified_status.as_deref(),
        Some("claimed"),
        "已发出的请求仍要复核"
    );
    assert!(run.stopped_by_user);
    assert!(
        run.errors.iter().any(|e| e.contains("中止剩余领奖")),
        "{:?}",
        run.errors
    );
}

/// 接单阶段关掉开关：领奖整个阶段都不进。
#[tokio::test]
async fn closing_the_switch_after_accept_skips_the_claim_phase() {
    let (app, _dir) = test_app();
    let id = account(&app, "uid-close-before-claim");
    let switch = Arc::new(AtomicBool::new(true));

    let mock = Mock::with_tasks(vec![
        task("todo", "not_accepted", false),
        task("done", "completed", false),
    ])
    .accept_reply(Ok(accept_ok(&["todo"])))
    .claim_reply("done", Ok(claim_ok(100, 5)))
    .close_switch_after_accept(switch.clone(), 1)
    .tasks_pages(vec![
        tasks_body(vec![
            task("todo", "not_accepted", false),
            task("done", "completed", false),
        ]),
        tasks_body(vec![
            task("todo", "accepted", false),
            task("done", "completed", false),
        ]),
    ]);

    let gate = {
        let switch = switch.clone();
        move || switch.load(Ordering::SeqCst)
    };
    let run = run_gated(&app, &id, &mock, Options::FULL, &gate).await;

    assert!(
        !mock.calls().iter().any(|call| call.starts_with("claim:")),
        "开关关了就不该进领奖：{:?}",
        mock.calls()
    );
    assert_eq!(run.claimed, 0);
    assert!(run.stopped_by_user);
}
