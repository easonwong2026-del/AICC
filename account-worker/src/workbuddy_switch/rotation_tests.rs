//! 夜间轮换的确定性测试：Fake Clock + Fake Client + 注入式预检与活跃取证。
//!
//! **一条都不联网、一条都不碰真实 WorkBuddy**：
//!
//! | 注入点 | 测试里是什么 |
//! | --- | --- |
//! | `Clock` | 手动推进的毫秒数（整晚不需要真等） |
//! | `ClientControl` | 临时目录里的一个 JSON 文件 + 开关 |
//! | `IdleProbe` | 固定空闲秒数（默认"没在用"） |
//! | `OwnPrecheck` | 固定结论（accepted / rejected / 读不到） |
//! | `ActivityProbe` | 固定 JSON（也可以让它失败） |

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::Mutex;

use chrono::TimeZone;
use serde_json::{json, Value};

use crate::app::App;
use crate::paths::Paths;
use crate::rpc::RpcResult;
use crate::storage::authz::AuthorizationRecord;
use crate::storage::rotation::{
    RotationPlan, RotationRun, STATE_COMPLETED, STATE_DWELLING, STATE_SKIPPED,
};
use crate::storage::{AccountInput, AccountRecord};

use super::client::{self, ClientControl};
use super::plan;
use super::rotation::{
    self, active_window, next_window, validate_plan, ActivityProbe, BoxFut, Clock, IdleProbe,
    OwnPrecheck, RotationDeps,
};
use super::service::SwitchEnv;

// ---------------------------------------------------------------------------
// 假实现
// ---------------------------------------------------------------------------

struct TestClock(AtomicI64);

impl Clock for TestClock {
    fn now_ms(&self) -> i64 {
        self.0.load(Ordering::SeqCst)
    }
}

impl TestClock {
    fn set(&self, ms: i64) {
        self.0.store(ms, Ordering::SeqCst);
    }
    fn advance_secs(&self, secs: i64) {
        self.0.fetch_add(secs * 1000, Ordering::SeqCst);
    }
}

struct TestIdle(AtomicI64);

impl IdleProbe for TestIdle {
    fn idle_secs(&self) -> Option<i64> {
        Some(self.0.load(Ordering::SeqCst))
    }
}

struct TestControl {
    auth_path: PathBuf,
    installed: AtomicBool,
    running: AtomicBool,
    /// 启动时若文件里的账号不是它，就改回它（模拟"客户端不接受这份登录"）。
    ///
    /// 注意不能写成"每次启动都改"：拍原账号快照那一步也会启动客户端，
    /// 那一次文件里本来就是原账号，不该被算成拒绝。
    reject_unless: Mutex<Option<String>>,
    launches: AtomicU32,
    closes: AtomicU32,
}

impl TestControl {
    fn new(auth_path: PathBuf) -> Self {
        Self {
            auth_path,
            installed: AtomicBool::new(true),
            running: AtomicBool::new(true),
            reject_unless: Mutex::new(None),
            launches: AtomicU32::new(0),
            closes: AtomicU32::new(0),
        }
    }
    fn closes(&self) -> u32 {
        self.closes.load(Ordering::SeqCst)
    }
    fn launches(&self) -> u32 {
        self.launches.load(Ordering::SeqCst)
    }
}

impl ClientControl for TestControl {
    fn variant(&self) -> wb_switch_core::modules::variant::WbVariant {
        wb_switch_core::modules::variant::WbVariant::Cn
    }
    fn app_path(&self) -> PathBuf {
        PathBuf::from("/Applications/WorkBuddy.app")
    }
    fn installed(&self) -> bool {
        self.installed.load(Ordering::SeqCst)
    }
    fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
    fn close(&self, _timeout_secs: i64) -> Result<(), String> {
        self.closes.fetch_add(1, Ordering::SeqCst);
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }
    fn launch(&self) -> Result<(), String> {
        self.launches.fetch_add(1, Ordering::SeqCst);
        let reject_unless = self
            .reject_unless
            .lock()
            .map(|slot| slot.clone())
            .unwrap_or(None);
        if let Some(expected) = reject_unless {
            let live = client::read_live(&self.auth_path);
            if live.uid.as_deref() != Some(expected.as_str()) {
                std::fs::write(&self.auth_path, auth_json(&expected, "AT-OTHER")).unwrap();
            }
        }
        self.running.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn sleep_ms(&self, _ms: u64) {}
    fn verify_window(&self) -> (u64, u64) {
        (0, 80)
    }
}

struct TestPrecheck(Mutex<Result<String, String>>);

impl OwnPrecheck for TestPrecheck {
    fn check<'a>(
        &'a self,
        _app: &'a App,
        _account: &'a AccountRecord,
    ) -> BoxFut<'a, RpcResult<String>> {
        Box::pin(async move {
            match self
                .0
                .lock()
                .map(|slot| slot.clone())
                .unwrap_or_else(|_| Err("锁中毒".to_string()))
            {
                Ok(state) => Ok(state),
                Err(message) => Err(crate::rpc::RpcError::new(crate::rpc::E_AUTH, message)),
            }
        })
    }
}

struct TestActivity(Mutex<Value>);

impl ActivityProbe for TestActivity {
    fn snapshot<'a>(&'a self, _app: &'a App, _account_id: &'a str) -> BoxFut<'a, Value> {
        Box::pin(async move {
            self.0
                .lock()
                .map(|slot| slot.clone())
                .unwrap_or_else(|_| json!({ "state": "failed" }))
        })
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn auth_json(uid: &str, access: &str) -> String {
    serde_json::to_string_pretty(&json!({
        "auth": {
            "accessToken": access,
            "refreshToken": format!("RT-{uid}"),
            "tokenType": "Bearer",
            "domain": "www.codebuddy.cn",
        },
        "account": {"uid": uid, "nickname": uid},
        "allAccounts": [{"uid": uid, "nickname": uid}],
    }))
    .unwrap()
}

/// 北京时间。
fn beijing(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    chrono::FixedOffset::east_opt(8 * 3600)
        .expect("UTC+8")
        .with_ymd_and_hms(year, month, day, hour, minute, 0)
        .single()
        .expect("合法时间")
        .timestamp_millis()
}

struct Harness {
    _dir: tempfile::TempDir,
    app: App,
    env: SwitchEnv,
    control: TestControl,
    clock: TestClock,
    idle: TestIdle,
    precheck: TestPrecheck,
    activity: TestActivity,
    a: AccountRecord,
    b: AccountRecord,
    c: AccountRecord,
}

impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let paths = Paths {
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data.clone(),
            wb_home_owned: true,
        };
        let app = App::with_secure(paths, crate::securestore::SecureStore::memory());
        app.with_db(|db| db.migrate().map(|_| ())).unwrap();

        let a = account(&app, "uid-a", "甲");
        let b = account(&app, "uid-b", "乙");
        let c = account(&app, "uid-c", "丙");
        for row in [&a, &b, &c] {
            seed_own(&app, row);
        }

        let auth_path = dir.path().join("workbuddy-desktop.info");
        std::fs::write(&auth_path, auth_json("uid-a", "AT-A")).unwrap();

        Self {
            env: SwitchEnv {
                auth_path: auth_path.clone(),
                snapshot_dir: data.join("workbuddy-switch"),
            },
            control: TestControl::new(auth_path),
            clock: TestClock(AtomicI64::new(beijing(2026, 11, 20, 22, 30))),
            idle: TestIdle(AtomicI64::new(3600)),
            precheck: TestPrecheck(Mutex::new(Ok("accepted".to_string()))),
            activity: TestActivity(Mutex::new(json!({
                "state": "ok", "todayActive": true, "streakDays": 2
            }))),
            a,
            b,
            c,
            app,
            _dir: dir,
        }
    }

    fn deps(&self) -> RotationDeps<'_> {
        RotationDeps {
            control: &self.control,
            env: &self.env,
            clock: &self.clock,
            idle: &self.idle,
            precheck: &self.precheck,
            activity: &self.activity,
        }
    }

    /// 22:30 → 07:00，只选周五（2026-11-20 是周五）。
    fn plan(&self, accounts: &[&AccountRecord], dwell: i64) {
        let mut plan = RotationPlan::defaults();
        plan.enabled = true;
        plan.weekdays = vec![5];
        plan.dwell_seconds = dwell;
        plan.account_ids = accounts.iter().map(|row| row.id.clone()).collect();
        self.app.with_db(|db| db.save_rotation_plan(&plan)).unwrap();
        self.app
            .with_db(|db| db.set_rotation_enabled(true))
            .unwrap();
    }

    fn tick(&self, force: bool) -> Value {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let deps = self.deps();
        runtime.block_on(async move {
            rotation::run_tick_with(&self.app, &deps, force)
                .await
                .unwrap()
        })
    }

    fn live_uid(&self) -> Option<String> {
        client::read_live(&self.env.auth_path).uid
    }

    fn run(&self) -> RotationRun {
        self.app
            .with_db(|db| db.latest_rotation_run())
            .unwrap()
            .expect("应当有一轮记录")
    }
}

fn account(app: &App, uid: &str, nickname: &str) -> AccountRecord {
    let id = app
        .with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: uid.to_string(),
                nickname: Some(nickname.to_string()),
                ..Default::default()
            })
        })
        .unwrap()
        .id;
    app.with_db(|db| db.get_account(&id)).unwrap().unwrap()
}

/// 自有长期授权：令牌放内存凭据库，行里只留引用。
fn seed_own(app: &App, account: &AccountRecord) {
    // 键名必须与生产代码一致：`read_own_refresh` 按 account.id 生成键，
    // 不看 `refresh_ref` 里的字符串（那是"引用"，不是键）。
    let keys = crate::auth::service::own_keys(&account.id);
    let refresh_key = keys.refresh.clone().expect("own 一定有 refresh 键");
    app.secure
        .set(&keys.access, format!("AT-{}", account.uid).as_bytes())
        .unwrap();
    app.secure
        .set(&refresh_key, format!("RT-{}", account.uid).as_bytes())
        .unwrap();
    let now = crate::clock::now_rfc3339();
    app.with_db(|db| {
        db.upsert_authorization(&AuthorizationRecord {
            id: format!("{}-own", account.id),
            account_id: account.id.clone(),
            kind: "own".to_string(),
            access_ref: format!("keychain:{}", keys.access),
            refresh_ref: Some(format!("keychain:{refresh_key}")),
            auth_domain: Some("www.codebuddy.cn".to_string()),
            token_expires_at: None,
            status: "valid".to_string(),
            version: 0,
            key_migration_state: "ready".to_string(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: now.clone(),
            updated_at: now,
        })
    })
    .unwrap();
}

// ---------------------------------------------------------------------------
// 时窗
// ---------------------------------------------------------------------------

#[test]
fn a_window_crossing_midnight_contains_both_sides() {
    let plan = {
        let mut plan = RotationPlan::defaults();
        plan.weekdays = vec![5];
        plan
    };
    let window = active_window(&plan, beijing(2026, 11, 20, 22, 30)).expect("22:30 在窗口内");
    assert_eq!(window.date.format("%Y-%m-%d").to_string(), "2026-11-20");
    assert!(window.end_ms > window.start_ms);

    let window = active_window(&plan, beijing(2026, 11, 21, 2, 0)).expect("跨午夜后仍在窗口内");
    assert_eq!(window.date.format("%Y-%m-%d").to_string(), "2026-11-20");

    assert!(active_window(&plan, beijing(2026, 11, 21, 8, 0)).is_none());
}

#[test]
fn weekdays_gate_the_window_and_next_run_skips_unselected_days() {
    let mut plan = RotationPlan::defaults();
    plan.weekdays = vec![1]; // 只周一
    assert!(active_window(&plan, beijing(2026, 11, 20, 23, 0)).is_none());
    let next = next_window(&plan, beijing(2026, 11, 20, 23, 0)).expect("下一个周一");
    assert_eq!(next.date.format("%Y-%m-%d").to_string(), "2026-11-23");
}

#[test]
fn an_unsupported_timezone_is_rejected_instead_of_guessed() {
    let mut plan = RotationPlan::defaults();
    plan.timezone = "Europe/Berlin".to_string();
    plan.account_ids = vec!["acc".into()];
    let err = validate_plan(&plan).unwrap_err();
    assert!(err.contains("暂不支持时区"), "{err}");
}

#[test]
fn waking_up_late_skips_instead_of_catching_up() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    h.clock.set(beijing(2026, 11, 20, 23, 10));

    let report = h.tick(false);
    assert_eq!(report["status"], "skipped");
    assert_eq!(report["reason"], "late_start");
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"), "不能动客户端");
    assert_eq!(h.run().state, STATE_SKIPPED);
    assert_eq!(h.tick(false)["status"], "already_ran", "同一晚不补跑第二轮");
}

// ---------------------------------------------------------------------------
// 主流程
// ---------------------------------------------------------------------------

#[test]
fn one_account_switches_and_comes_back() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);

    let report = h.tick(false);
    {
        let run = h.run();
        for step in h.app.with_db(|db| db.rotation_steps(&run.id)).unwrap() {
            eprintln!(
                "[dbg] step {} {} {:?} {:?}",
                step.seq, step.status, step.skip_reason, step.detail
            );
        }
    }
    assert_eq!(report["status"], STATE_DWELLING, "{report}");
    assert_eq!(h.live_uid().as_deref(), Some("uid-b"), "应当切到乙");
    let run = h.run();
    assert_eq!(run.origin_uid.as_deref(), Some("uid-a"));
    assert!(run.origin_snapshot_sha256.is_some(), "必须有原账号快照");

    h.clock.advance_secs(300);
    let report = h.tick(false);
    assert_eq!(report["status"], STATE_COMPLETED, "{report}");
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"), "必须回到最初账号");
    assert_eq!(h.run().restore_result.as_deref(), Some("restored"));
}

/// A → B → C → **A**：不是"回上一站"，是回这一轮最初那个。
#[test]
fn three_accounts_rotate_and_return_to_the_original() {
    let h = Harness::new();
    h.plan(&[&h.b, &h.c], 300);

    assert_eq!(h.tick(false)["status"], STATE_DWELLING);
    assert_eq!(h.live_uid().as_deref(), Some("uid-b"));

    h.clock.advance_secs(300);
    assert_eq!(h.tick(false)["status"], STATE_DWELLING);
    assert_eq!(h.live_uid().as_deref(), Some("uid-c"), "第二跳应当是丙");

    h.clock.advance_secs(300);
    let report = h.tick(false);
    assert_eq!(report["status"], STATE_COMPLETED, "{report}");
    assert_eq!(
        h.live_uid().as_deref(),
        Some("uid-a"),
        "A→B→C 之后必须回 A，不能回 B"
    );

    let run = h.run();
    let steps = h.app.with_db(|db| db.rotation_steps(&run.id)).unwrap();
    assert_eq!(steps.iter().filter(|step| step.status == "done").count(), 2);
}

#[test]
fn a_second_tick_in_the_same_night_does_not_start_another_run() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    h.tick(false);
    h.clock.advance_secs(300);
    h.tick(false);

    let before = h.app.with_db(|db| db.rotation_runs(10)).unwrap().len();
    let report = h.tick(false);
    assert!(
        report["status"] == "already_ran" || report["status"] == "idle",
        "{report}"
    );
    let after = h.app.with_db(|db| db.rotation_runs(10)).unwrap().len();
    assert_eq!(before, after, "同一天不能多出一轮");
}

// ---------------------------------------------------------------------------
// 凭据
// ---------------------------------------------------------------------------

#[test]
fn a_rejected_own_grant_is_skipped_and_the_client_is_restored() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    *h.precheck.0.lock().unwrap() = Ok("rejected".to_string());

    let report = h.tick(false);
    assert!(
        report["status"] == "skipped" || report["status"] == STATE_COMPLETED,
        "{report}"
    );
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"), "不能强行切过去");
    let run = h.run();
    let steps = h.app.with_db(|db| db.rotation_steps(&run.id)).unwrap();
    assert!(steps
        .iter()
        .any(|step| step.skip_reason.as_deref() == Some("own_grant_rejected")));
}

#[test]
fn an_unreadable_own_grant_is_skipped_too() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    *h.precheck.0.lock().unwrap() = Err("读取自有授权失败".to_string());

    let report = h.tick(false);
    assert!(
        report["status"] == "skipped" || report["status"] == STATE_COMPLETED,
        "{report}"
    );
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"));
}

/// Magpie 的 access 够 30 分钟门槛，但**不够覆盖停留 + 恢复余量**时跳过。
#[test]
fn magpie_access_that_cannot_cover_the_dwell_is_skipped() {
    use crate::providers::magpie;

    let h = Harness::new();
    h.plan(&[&h.b], 3600);

    // 乙改成只有 Magpie 授权。
    let _ = h
        .app
        .with_db(|db| db.delete_authorization(&format!("{}-own", h.b.id)));
    let now = crate::clock::now_rfc3339();
    h.app
        .with_db(|db| {
            db.upsert_authorization(&AuthorizationRecord {
                id: format!("{}-magpie", h.b.id),
                account_id: h.b.id.clone(),
                kind: "magpie".to_string(),
                access_ref: "magpie:v1:workbuddy".to_string(),
                refresh_ref: None,
                auth_domain: Some("www.codebuddy.cn".to_string()),
                token_expires_at: None,
                status: "valid".to_string(),
                version: 0,
                key_migration_state: "ready".to_string(),
                source_key: Some("workbuddy".to_string()),
                last_used_at: None,
                last_error: None,
                created_at: now.clone(),
                updated_at: now,
            })
        })
        .unwrap();
    magpie::test_set_logins(Some(vec![magpie::MagpieLogin {
        key: "workbuddy".to_string(),
        provider: magpie::provider_of("workbuddy").unwrap(),
        uid: "uid-b".to_string(),
        nickname: None,
        domain: "www.codebuddy.cn".to_string(),
        access_token: "AT-MAG".to_string(),
        // 40 分钟：够切换门槛（30），不够 1 小时停留 + 5 分钟恢复余量。
        expires_at_ms: Some(magpie::now_ms() + 40 * 60 * 1000),
    }]));

    let report = h.tick(false);
    magpie::test_reset();

    assert!(
        report["status"] == "skipped" || report["status"] == STATE_COMPLETED,
        "{report}"
    );
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"), "不能切过去");
    let run = h.run();
    let steps = h.app.with_db(|db| db.rotation_steps(&run.id)).unwrap();
    assert!(steps
        .iter()
        .any(|step| step.skip_reason.as_deref() == Some("magpie_too_short_for_dwell")));
}

// ---------------------------------------------------------------------------
// 用户打断 / 崩溃 / 损坏
// ---------------------------------------------------------------------------

#[test]
fn a_manual_switch_during_the_night_stops_the_plan_without_overwriting() {
    let h = Harness::new();
    h.plan(&[&h.b, &h.c], 300);
    h.tick(false);
    assert_eq!(h.live_uid().as_deref(), Some("uid-b"));

    // 用户自己换到了别的账号（既不是 origin，也不是本轮目标）。
    std::fs::write(h.env.auth_path.clone(), auth_json("uid-user", "AT-USER")).unwrap();

    let report = h.tick(false);
    assert_eq!(report["status"], STATE_COMPLETED, "{report}");
    assert_eq!(
        h.live_uid().as_deref(),
        Some("uid-user"),
        "不得覆盖用户的选择"
    );
    let run = h.run();
    assert!(run.detail.as_deref().unwrap_or_default().contains("手动"));
}

#[test]
fn a_corrupt_origin_snapshot_is_refused_and_kept_for_a_human() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    h.tick(false);

    let run = h.run();
    let path = PathBuf::from(run.origin_snapshot_path.clone().unwrap());
    let mut bytes = std::fs::read(&path).unwrap();
    bytes.push(b' ');
    std::fs::write(&path, &bytes).unwrap();

    h.clock.advance_secs(300);
    let report = h.tick(false);
    assert_eq!(report["status"], "recovery_required", "{report}");
    assert!(!report["restored"].as_bool().unwrap_or(true));
    assert!(path.exists(), "损坏的快照不能删——要留给人工判断");
    assert_eq!(h.live_uid().as_deref(), Some("uid-b"), "不敢覆盖");
}

#[test]
fn a_missing_origin_snapshot_is_a_human_decision_not_a_guess() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    h.tick(false);

    let run = h.run();
    std::fs::remove_file(PathBuf::from(run.origin_snapshot_path.clone().unwrap())).unwrap();

    h.clock.advance_secs(300);
    let report = h.tick(false);
    assert_eq!(report["status"], "recovery_required", "{report}");
    assert_eq!(h.live_uid().as_deref(), Some("uid-b"));
}

/// 崩溃在"已写入目标、还没收尾"：重启后按客户端**现在是谁**决定，不重复切。
#[test]
fn a_crash_after_writing_the_target_is_reconciled_on_restart() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    h.tick(false);

    let run = h.run();
    h.app
        .with_db(|db| db.cas_rotation_state(&run.id, STATE_DWELLING, "switching"))
        .unwrap();
    let steps = h.app.with_db(|db| db.rotation_steps(&run.id)).unwrap();
    h.app
        .with_db(|db| db.start_rotation_step(&steps[0].id, "own_refresh", None, None))
        .unwrap();

    let report = h.tick(false);
    assert_eq!(report["status"], STATE_DWELLING, "{report}");
    assert_eq!(h.live_uid().as_deref(), Some("uid-b"), "不能重复切");

    h.clock.advance_secs(300);
    assert_eq!(h.tick(false)["status"], STATE_COMPLETED);
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"));
}

/// 临时授权过期导致客户端登出：仍然要能恢复到最初账号。
#[test]
fn an_expired_temporary_login_can_still_be_restored() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    h.tick(false);

    std::fs::write(&h.env.auth_path, "{}").unwrap();
    std::fs::write(client::logout_marker_path(&h.env.auth_path), "logged-out").unwrap();

    h.clock.advance_secs(300);
    let report = h.tick(false);
    assert_eq!(report["status"], STATE_COMPLETED, "{report}");
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"), "登出也要能救回来");
    assert!(!client::logout_marker_path(&h.env.auth_path).exists());
}

// ---------------------------------------------------------------------------
// 不泄漏 / 不阻塞 / 不误动
// ---------------------------------------------------------------------------

#[test]
fn tokens_never_appear_in_status_or_history() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    h.tick(false);
    h.clock.advance_secs(300);
    h.tick(false);

    let status = rotation::status(&h.app, &h.control, &h.env, &h.clock).unwrap();
    let text = serde_json::to_string(&status).unwrap();
    for forbidden in ["AT-uid-b", "RT-uid-b", "AT-A", "RT-uid-a"] {
        assert!(!text.contains(forbidden), "响应里出现了令牌：{forbidden}");
    }
    assert!(text.contains("originSnapshotTracked"));
}

#[test]
fn an_activity_probe_failure_does_not_block_the_restore() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    *h.activity.0.lock().unwrap() = json!({ "state": "failed", "error": "timeout" });

    assert_eq!(h.tick(false)["status"], STATE_DWELLING);
    h.clock.advance_secs(300);
    let report = h.tick(false);
    assert_eq!(report["status"], STATE_COMPLETED, "{report}");
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"));

    let run = h.run();
    let steps = h.app.with_db(|db| db.rotation_steps(&run.id)).unwrap();
    assert_eq!(
        steps[0].activity_before.as_ref().unwrap()["state"],
        "failed"
    );
}

#[test]
fn dry_run_touches_nothing() {
    let h = Harness::new();
    h.plan(&[&h.b, &h.c], 300);
    let before = std::fs::read(&h.env.auth_path).unwrap();

    let report = rotation::dry_run(&h.app, &h.control, &h.env, &h.clock).unwrap();
    assert_eq!(report["dryRun"], true);
    assert_eq!(report["steps"].as_array().unwrap().len(), 2);
    assert_eq!(report["steps"][0]["outcome"], "switch");

    assert_eq!(std::fs::read(&h.env.auth_path).unwrap(), before);
    assert!(h.app.with_db(|db| db.rotation_runs(5)).unwrap().is_empty());
    assert_eq!(h.control.closes(), 0, "dry-run 不能关客户端");
}

#[test]
fn stopping_a_run_puts_the_original_account_back() {
    let h = Harness::new();
    h.plan(&[&h.b, &h.c], 300);
    h.tick(false);
    assert_eq!(h.live_uid().as_deref(), Some("uid-b"));

    let report = rotation::stop(&h.app, &h.control, &h.env).unwrap();
    assert_eq!(report["restored"], true, "{report}");
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"));

    h.clock.advance_secs(60);
    let again = h.tick(false);
    assert!(
        again["status"] == "already_ran" || again["status"] == "idle",
        "{again}"
    );
}

#[test]
fn a_disabled_plan_never_touches_the_client() {
    let h = Harness::new();
    let mut plan = RotationPlan::defaults();
    plan.weekdays = vec![5];
    plan.account_ids = vec![h.b.id.clone()];
    h.app.with_db(|db| db.save_rotation_plan(&plan)).unwrap();

    assert_eq!(h.tick(false)["status"], "disabled");
    assert_eq!(h.control.closes(), 0);
    assert_eq!(h.control.launches(), 0);
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"));
}

#[test]
fn a_target_that_is_already_live_is_not_switched_again() {
    let h = Harness::new();
    h.plan(&[&h.a], 300);

    let report = h.tick(false);
    assert_eq!(report["status"], "skipped", "{report}");
    assert_eq!(h.control.closes(), 0, "不该关客户端");
    let run = h.run();
    let steps = h.app.with_db(|db| db.rotation_steps(&run.id)).unwrap();
    assert_eq!(steps[0].status, "already_live");
}

/// 客户端不接受这次登录：回滚后这一跳记失败，计划继续收尾而不是卡住。
#[test]
fn a_client_that_rejects_the_login_does_not_wedge_the_run() {
    let h = Harness::new();
    h.plan(&[&h.b], 300);
    *h.control.reject_unless.lock().unwrap() = Some("uid-a".to_string());

    let report = h.tick(false);
    assert!(
        matches!(
            report["status"].as_str(),
            Some(STATE_COMPLETED) | Some("skipped") | Some(STATE_DWELLING)
        ),
        "{report}"
    );
    assert_eq!(h.live_uid().as_deref(), Some("uid-a"));

    h.clock.advance_secs(300);
    let again = h.tick(false);
    assert!(
        again["status"] == STATE_COMPLETED || again["status"] == "already_ran",
        "{again}"
    );
    assert!(matches!(
        h.run().state.as_str(),
        STATE_COMPLETED | "skipped"
    ));
}

#[test]
fn plan_resolution_is_shared_with_the_manual_switch() {
    let h = Harness::new();
    match plan::resolve(&h.app, &h.b, true, Some("uid-a")) {
        plan::PlanOutcome::Ready(ready) => assert_eq!(ready.mode, plan::Mode::OwnRefresh),
        plan::PlanOutcome::Blocked { reason, detail } => panic!("应当可切：{reason} {detail}"),
    }
}
