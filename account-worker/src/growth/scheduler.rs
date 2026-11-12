//! 后台调度：多久查一次、什么时候真的动手。
//!
//! 三条原则：
//!
//! 1. **默认关闭**。开关没打开时这里一个写请求都不发，只读也不发 ——
//!    连"顺手看一眼"都不做，避免用户以为自己在被盯着。
//! 2. **tick 幂等**。`run_tick` 由 App 侧定时器每 60 秒叫一次，它自己判断
//!    "到点没有"。这样定时器抖动、界面重进都不会把服务端打成筛子。
//! 3. **不宣称常驻**。worker 是 App 的子进程，App 退出即停。
//!    界面上必须如实写"仅在 AICC 运行时执行"。

use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{json, Value};

use crate::app::App;
use crate::clock::now_rfc3339;
use crate::rpc::RpcResult;

use super::manager::{self, Options};

/// 检查间隔：3 分钟。任务状态变化不频繁，而每次 tick 只发 1 个 `/tasks`。
pub const DEFAULT_INTERVAL_SECS: i64 = 180;
/// 允许的最小间隔。再小就变成刷接口了。
pub const MIN_INTERVAL_SECS: i64 = 60;

/// 设置项：总开关。**默认关闭**，用户显式开启后才有写操作。
pub const SETTING_ENABLED: &str = "growth_auto_enabled";
/// 设置项：上次 tick 的完成时间（RFC3339）。
pub const SETTING_LAST_TICK: &str = "growth_auto_last_tick";
/// 设置项：上次 tick 的结论摘要（一句话，给人看）。
pub const SETTING_LAST_RESULT: &str = "growth_auto_last_result";
/// 设置项：间隔秒数。
pub const SETTING_INTERVAL: &str = "growth_auto_interval_secs";

/// 同一时刻只允许一个 tick。App 侧定时器和服务端手动触发都可能撞进来。
static TICK_RUNNING: AtomicBool = AtomicBool::new(false);

/// 开关决定这一轮能不能写。`force` 只跳过间隔判断，**不跳过开关**：
/// 用户点「立即检查」时如果总开关是关的，那就是一次只读检查。
pub fn options_for(enabled: bool) -> Options {
    if enabled {
        Options::FULL
    } else {
        Options::READ_ONLY
    }
}

pub fn enabled(app: &App) -> bool {
    matches!(
        app.with_db(|db| db.get_setting(SETTING_ENABLED)),
        Ok(Some(value)) if value == "1" || value == "true"
    )
}

pub fn set_enabled(app: &App, enabled: bool) -> RpcResult<()> {
    app.with_db(|db| db.set_setting(SETTING_ENABLED, if enabled { "1" } else { "0" }))
}

pub fn interval_secs(app: &App) -> i64 {
    app.with_db(|db| db.get_setting(SETTING_INTERVAL))
        .ok()
        .flatten()
        .and_then(|raw| raw.trim().parse::<i64>().ok())
        .map(|secs| secs.max(MIN_INTERVAL_SECS))
        .unwrap_or(DEFAULT_INTERVAL_SECS)
}

fn last_tick_at(app: &App) -> Option<chrono::DateTime<chrono::Utc>> {
    let raw = app
        .with_db(|db| db.get_setting(SETTING_LAST_TICK))
        .ok()
        .flatten()?;
    chrono::DateTime::parse_from_rfc3339(&raw)
        .ok()
        .map(|at| at.with_timezone(&chrono::Utc))
}

/// 界面要的运行时状态。不含任何凭据。
pub fn auto_status(app: &App) -> Value {
    let enabled = enabled(app);
    let interval = interval_secs(app);
    let last = last_tick_at(app);
    let next = last.map(|at| at + chrono::Duration::seconds(interval));
    let last_result = app
        .with_db(|db| db.get_setting(SETTING_LAST_RESULT))
        .ok()
        .flatten();
    json!({
        "enabled": enabled,
        "intervalSeconds": interval,
        "running": TICK_RUNNING.load(Ordering::Relaxed),
        "lastTickAt": last.map(|at| at.to_rfc3339()),
        "nextTickAt": next.map(|at| at.to_rfc3339()),
        "lastResult": last_result,
        // 如实写清楚：App 退出就没有人在跑。
        "owner": "AICC 的账号助手进程",
        "continuesAfterQuit": false,
    })
}

/// 跑一轮。到时候没到、没开开关都直接返回 `skipped`，不发任何请求。
pub async fn run_tick(app: &std::sync::Arc<App>, force: bool) -> Value {
    if !enabled(app) && !force {
        return json!({ "skipped": "disabled" });
    }
    if !force {
        let interval = interval_secs(app);
        if let Some(last) = last_tick_at(app) {
            let due = last + chrono::Duration::seconds(interval);
            if chrono::Utc::now() < due {
                return json!({
                    "skipped": "not_due",
                    "nextTickAt": due.to_rfc3339(),
                });
            }
        }
    }
    if TICK_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return json!({ "skipped": "already_running" });
    }

    let started = now_rfc3339();
    let options = options_for(enabled(app));

    let accounts = match app.with_db(|db| db.list_accounts(&Default::default())) {
        Ok(list) => list,
        Err(err) => {
            TICK_RUNNING.store(false, Ordering::SeqCst);
            return json!({ "error": err.message });
        }
    };

    let mut runs = Vec::new();
    for account in accounts {
        if account.platform != "workbuddy" {
            continue;
        }
        // 账号级串行：一个账号跑完再下一个。某个账号失败不影响后面的。
        let resolved = match crate::auth::broker::resolve(app, &account) {
            Ok(crate::auth::broker::Outcome::Resolved(resolved)) => resolved,
            Ok(crate::auth::broker::Outcome::Unavailable { reason, detail }) => {
                runs.push(json!({
                    "accountId": account.id,
                    "nickname": account.nickname,
                    "state": "skipped",
                    "reason": reason,
                    "detail": detail,
                }));
                continue;
            }
            Err(err) => {
                runs.push(json!({
                    "accountId": account.id,
                    "nickname": account.nickname,
                    "state": "failed",
                    "detail": err.message,
                }));
                continue;
            }
        };
        let domain = account.auth_domain.clone().unwrap_or_default();
        let run = manager::run_account(
            app,
            &account.id,
            &account.nickname,
            &resolved.access,
            &account.uid,
            &domain,
            options,
        )
        .await;
        runs.push(json!(run));
    }

    let summary = summarize(&runs);
    let _ = app.with_db(|db| db.set_setting(SETTING_LAST_TICK, &now_rfc3339()));
    let _ = app.with_db(|db| db.set_setting(SETTING_LAST_RESULT, &summary));

    TICK_RUNNING.store(false, Ordering::SeqCst);
    json!({
        "skipped": Value::Null,
        "startedAt": started,
        "finishedAt": now_rfc3339(),
        "readOnly": options.accept == false && options.claim == false,
        "summary": summary,
        "accounts": runs,
    })
}

/// 一句话结论，落在设置里给界面显示。
fn summarize(runs: &[Value]) -> String {
    let mut accepted = 0;
    let mut claimed = 0;
    let mut already = 0;
    let mut rejected = 0;
    let mut credit = 0;
    let mut problems = 0;
    for run in runs {
        accepted += run.get("accepted").and_then(Value::as_i64).unwrap_or(0);
        claimed += run.get("claimed").and_then(Value::as_i64).unwrap_or(0);
        already += run
            .get("alreadyClaimed")
            .and_then(Value::as_i64)
            .unwrap_or(0);
        rejected += run.get("rejected").and_then(Value::as_i64).unwrap_or(0);
        credit += run.get("credit").and_then(Value::as_i64).unwrap_or(0);
        let state = run.get("state").and_then(Value::as_str).unwrap_or("");
        if matches!(state, "failed" | "unauthorized" | "partial") {
            problems += 1;
        }
    }
    let mut parts = Vec::new();
    if accepted > 0 {
        parts.push(format!("接单 {accepted}"));
    }
    if claimed > 0 {
        parts.push(format!("领奖 {claimed}（+{credit} 积分）"));
    }
    if already > 0 {
        parts.push(format!("已领过 {already}"));
    }
    if rejected > 0 {
        parts.push(format!("被拒 {rejected}"));
    }
    if problems > 0 {
        parts.push(format!("{problems} 个账号有问题"));
    }
    if parts.is_empty() {
        "没有需要处理的任务".to_string()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;

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

    #[test]
    fn it_is_disabled_by_default() {
        let (app, _dir) = test_app();
        assert!(!enabled(&app), "默认必须是关的：没开开关就不该有任何写操作");
        assert_eq!(interval_secs(&app), DEFAULT_INTERVAL_SECS);
        let status = auto_status(&app);
        assert_eq!(status["enabled"], serde_json::json!(false));
        assert_eq!(status["continuesAfterQuit"], serde_json::json!(false));
    }

    #[test]
    fn the_switch_persists_and_round_trips() {
        let (app, _dir) = test_app();
        set_enabled(&app, true).expect("on");
        assert!(enabled(&app));
        set_enabled(&app, false).expect("off");
        assert!(!enabled(&app));
    }

    #[test]
    fn force_does_not_override_the_switch() {
        // force 只跳过"到点没到点"，不跳过开关：关着就是只读。
        assert_eq!(options_for(false), Options::READ_ONLY);
        assert_eq!(options_for(true), Options::FULL);
        assert!(!options_for(false).accept);
        assert!(!options_for(false).claim);
    }

    #[test]
    fn interval_has_a_floor() {
        let (app, _dir) = test_app();
        // 库里被写成 5 秒也按 60 秒算：再小就是在刷接口了。
        app.with_db(|db| db.set_setting(SETTING_INTERVAL, "5"))
            .expect("stamp");
        assert_eq!(interval_secs(&app), MIN_INTERVAL_SECS);
    }

    /// 关着的时候 tick 什么都不做 —— 不发请求、不写库、不改时间戳。
    /// 这就是"关闭自动管理后不再执行自动写操作"的那条测试。
    #[tokio::test]
    async fn a_disabled_tick_does_nothing() {
        let (app, _dir) = test_app();
        let result = run_tick(&app, false).await;
        assert_eq!(result["skipped"], serde_json::json!("disabled"));
        assert!(
            app.with_db(|db| db.get_setting(SETTING_LAST_TICK))
                .unwrap()
                .is_none(),
            "没跑就不该留下上次运行时间"
        );
    }

    /// 到点之前再来一次会被挡掉：界面反复打开、定时器抖动都不会把接口打穿。
    #[tokio::test]
    async fn a_tick_that_is_not_due_is_skipped() {
        let (app, _dir) = test_app();
        set_enabled(&app, true).expect("on");
        app.with_db(|db| db.set_setting(SETTING_LAST_TICK, &now_rfc3339()))
            .expect("stamp");

        let result = run_tick(&app, false).await;
        assert_eq!(result["skipped"], serde_json::json!("not_due"));
        assert!(result["nextTickAt"].is_string());
    }

    /// 上次运行时间是很久以前（App 关了再开）：应该立刻认为到点。
    #[test]
    fn a_long_gap_means_it_is_due() {
        let (app, _dir) = test_app();
        set_enabled(&app, true).expect("on");
        let long_ago = (chrono::Utc::now() - chrono::Duration::hours(6)).to_rfc3339();
        app.with_db(|db| db.set_setting(SETTING_LAST_TICK, &long_ago))
            .expect("stamp");

        let last = last_tick_at(&app).expect("parsed");
        let due = last + chrono::Duration::seconds(interval_secs(&app));
        assert!(chrono::Utc::now() >= due, "六小时前跑过，现在就该跑");
    }
}
