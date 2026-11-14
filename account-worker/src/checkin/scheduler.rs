//! 每日自动签到的调度。
//!
//! 四条原则：
//!
//! 1. **默认关闭**。每个账号的开关默认关，关着时自动轮次连状态查询都不发。
//! 2. **一天一次，北京时间 09:00 后**。日期判断只用 `clock::beijing_date()`，
//!    机器本地时区改到纽约也不影响"今天"是哪天（PRD §5.1）。
//! 3. **到点才跑，迟到补跑**。App 09:00 没开着，之后打开会补做当日检查；
//!    已经签过就跳过，**绝不补昨天**。
//! 4. **不宣称常驻**。worker 是 App 的子进程，App 退出即停。
//!
//! 复用 worker 已有的 60 秒心跳（`main.rs`），不新增进程、不开端口、不注册第二个定时器。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, FixedOffset, NaiveDate, TimeZone, Utc};
use serde_json::{json, Value};

use crate::app::App;
use crate::clock::{beijing_date, beijing_now, now_rfc3339};
use crate::rpc::RpcResult;
use crate::storage::AccountRecord;

use super::manager::{self, Source};

/// 计划执行时间：北京时间每天 09:00。
pub const PLAN_HOUR: u32 = 9;
/// 账号之间的错峰窗口（秒）。同一时刻最多一个请求，避免集中打接口。
pub const MAX_STAGGER_SECS: u32 = 300;
/// 一天最多尝试几次（含失败重试）。到了就等明天，不再骚扰服务端。
pub const MAX_ATTEMPTS_PER_DAY: i64 = 4;
/// 失败退避：第 1/2/3 次失败后分别等多久再查。
pub const RETRY_BACKOFF_SECS: [i64; 3] = [60, 300, 1800];
/// 429 的保守冷却：拿不到 `Retry-After` 时按 30 分钟算。
pub const RATE_LIMIT_COOLDOWN_SECS: i64 = 1800;
/// 多账号之间的间隔。串行执行本来就有网络耗时，这里再加一点，避免瞬时并发。
pub const INTER_ACCOUNT_DELAY: Duration = Duration::from_millis(800);

/// 上次 tick 的完成时间。
pub const SETTING_LAST_TICK: &str = "checkin_auto_last_tick";
/// 上次 tick 的一句话结论。
pub const SETTING_LAST_RESULT: &str = "checkin_auto_last_result";

/// 同一时刻只允许一个 tick。
static TICK_RUNNING: AtomicBool = AtomicBool::new(false);

/// 拿到就持有，panic 也会在 unwind 里放掉 —— 否则一次 panic 会让自动签到永远停摆。
struct TickGuard;

impl TickGuard {
    fn acquire() -> Option<Self> {
        TICK_RUNNING
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| Self)
    }
}

impl Drop for TickGuard {
    fn drop(&mut self) {
        TICK_RUNNING.store(false, Ordering::SeqCst);
    }
}

fn next_at_key(account_id: &str) -> String {
    format!("checkin_next_at:{account_id}")
}

/// 账号的计划时间 = 09:00 + 由账号 id 决定的固定偏移。
///
/// 确定性错峰：同一台机器每次算出来都一样（重启不会改变顺序），
/// 不同账号大概率落在不同分钟。
pub fn plan_time(date: NaiveDate, account_id: &str) -> DateTime<FixedOffset> {
    let offset = FixedOffset::east_opt(8 * 3600).expect("UTC+8");
    let base = date.and_hms_opt(PLAN_HOUR, 0, 0).expect("09:00 一定合法");
    offset
        .from_local_datetime(&base)
        .single()
        .expect("固定偏移不会歧义")
        + chrono::Duration::seconds(stagger_offset(account_id))
}

/// FNV-1a，取 0..=MAX_STAGGER_SECS。只要求确定性与分散，不需要密码学强度。
pub fn stagger_offset(account_id: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in account_id.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (hash % (MAX_STAGGER_SECS as u64 + 1)) as i64
}

/// 今天这个账号该不该跑。
///
/// - 今天已有确认结论（`claimed` / `already`）→ 不跑。
/// - 还没到 09:00+偏移 → 等。
/// - 上一次失败留下了 `next_at` 且没到 → 等退避。
pub fn is_due(
    now: DateTime<FixedOffset>,
    plan: DateTime<FixedOffset>,
    done_today: bool,
    next_at: Option<DateTime<Utc>>,
) -> bool {
    if done_today {
        return false;
    }
    if now < plan {
        return false;
    }
    if let Some(at) = next_at {
        if now.with_timezone(&Utc) < at {
            return false;
        }
    }
    true
}

/// 界面上"下次自动检查"的那个时间。
pub fn next_run_at(
    now: DateTime<FixedOffset>,
    account_id: &str,
    done_today: bool,
    next_at: Option<DateTime<Utc>>,
) -> DateTime<FixedOffset> {
    let today = now.date_naive();
    if done_today {
        return plan_time(today + chrono::Duration::days(1), account_id);
    }
    let plan = plan_time(today, account_id);
    if now < plan {
        return plan;
    }
    if let Some(at) = next_at {
        if at > now.with_timezone(&Utc) {
            return at.with_timezone(&FixedOffset::east_opt(8 * 3600).expect("UTC+8"));
        }
    }
    // 已经过了计划时间又没签：下一轮 tick 就会补做，所以"下次"就是现在。
    now
}

/// 失败的退避秒数。按"今天已经失败过几次"选档，封顶 30 分钟。
pub fn backoff_secs(failures_today: i64, rate_limited: bool) -> i64 {
    if rate_limited {
        return RATE_LIMIT_COOLDOWN_SECS;
    }
    let index = failures_today.clamp(1, RETRY_BACKOFF_SECS.len() as i64) as usize - 1;
    RETRY_BACKOFF_SECS[index]
}

/// 今天 00:00（北京）对应的 UTC 时间戳，用来数"今天尝试了几次"。
///
/// 库里 `started_at` 全是 UTC RFC3339（`Z` 结尾），所以这个上限也必须是 UTC，
/// 否则字符串比较会错 8 小时。
pub fn beijing_day_start_utc(now: DateTime<FixedOffset>) -> String {
    let offset = FixedOffset::east_opt(8 * 3600).expect("UTC+8");
    let midnight = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .expect("00:00 一定合法");
    offset
        .from_local_datetime(&midnight)
        .single()
        .expect("固定偏移不会歧义")
        .with_timezone(&Utc)
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn read_next_at(app: &App, account_id: &str) -> Option<DateTime<Utc>> {
    let raw = app
        .with_db(|db| db.get_setting(&next_at_key(account_id)))
        .ok()
        .flatten()?;
    DateTime::parse_from_rfc3339(&raw)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn write_next_at(app: &App, account_id: &str, at: Option<DateTime<FixedOffset>>) {
    let key = next_at_key(account_id);
    let value = at.map(|at| {
        at.with_timezone(&Utc)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    });
    let _ = app.with_db(|db| match value {
        Some(text) => db.set_setting(&key, &text),
        // 清掉：用空串表示"没有额外等待"，与"从没写过"等价。
        None => db.set_setting(&key, ""),
    });
}

fn done_today(app: &App, account_id: &str, today: &str) -> bool {
    matches!(
        app.with_db(|db| db.checkin_record(account_id, today)),
        Ok(Some(record))
            if record.status == crate::storage::checkin::STATUS_CLAIMED
                || record.status == crate::storage::checkin::STATUS_ALREADY
    )
}

fn attempts_today(app: &App, account_id: &str, day_start: &str) -> i64 {
    app.with_db(|db| db.checkin_attempts_since(account_id, day_start))
        .unwrap_or(0)
}

/// 这个账号现在开没开。`WriteGate` 每次发写请求前都问一次。
fn still_enabled(app: &App, account_id: &str) -> bool {
    // 开关**和**版本都要重新确认：写请求发出之前，任何一条不满足都停。
    matches!(
        app.with_db(|db| db.get_account(account_id)),
        Ok(Some(account)) if account.checkin_enabled && account.supports_workbuddy_cn_apis()
    )
}

pub fn enabled_accounts(app: &App) -> Vec<AccountRecord> {
    app.with_db(|db| db.list_accounts(&Default::default()))
        .unwrap_or_default()
        .into_iter()
        // 自动轮次只跑国内版：国际版协议未接入，不能拿它的授权去请求国内接口。
        .filter(|account| account.supports_workbuddy_cn_apis() && account.checkin_enabled)
        .collect()
}

/// 界面要的调度状态。不含任何凭据。
pub fn auto_status(app: &App) -> Value {
    let now = beijing_now();
    let today = beijing_date();
    let day_start = beijing_day_start_utc(now);
    let accounts = app
        .with_db(|db| db.list_accounts(&Default::default()))
        .unwrap_or_default()
        .into_iter()
        .filter(|account| account.platform == "workbuddy")
        .collect::<Vec<_>>();

    let mut rows = Vec::new();
    let mut enabled = 0usize;
    let mut next_all: Option<DateTime<FixedOffset>> = None;
    for account in &accounts {
        // 国际版没有签到：不进调度，也不在列表里出现。
        if !account.supports_workbuddy_cn_apis() {
            continue;
        }
        let supported = true;
        let done = done_today(app, &account.id, &today);
        let next = read_next_at(app, &account.id);
        let at = next_run_at(now, &account.id, done, next);
        // 只有国内版会真正执行；国际版照常列出（用户得看见它），但不参与调度。
        if account.checkin_enabled && supported {
            enabled += 1;
            next_all = Some(match next_all {
                Some(current) if current <= at => current,
                _ => at,
            });
        }
        rows.push(json!({
            "accountId": account.id,
            "nickname": account.nickname,
            "enabled": account.checkin_enabled,
            "checkedToday": done,
            "lastResult": account.last_result,
            "lastCheckinAt": account.last_checkin_at,
            "nextRunAt": if supported { json!(at.to_rfc3339()) } else { Value::Null },
            "attemptsToday": attempts_today(app, &account.id, &day_start),
            "running": manager::is_running(&account.id),
            "provider": account.provider,
            "supported": supported,
        }));
    }

    let last_tick = app
        .with_db(|db| db.get_setting(SETTING_LAST_TICK))
        .ok()
        .flatten()
        .filter(|text| !text.is_empty());
    let last_result = app
        .with_db(|db| db.get_setting(SETTING_LAST_RESULT))
        .ok()
        .flatten()
        .filter(|text| !text.is_empty());

    json!({
        "enabled": enabled > 0,
        "enabledAccounts": enabled,
        "totalAccounts": rows.len(),
        "planTime": format!("{PLAN_HOUR:02}:00"),
        "timezone": "UTC+8（北京时间）",
        "staggerSeconds": MAX_STAGGER_SECS,
        "maxAttemptsPerDay": MAX_ATTEMPTS_PER_DAY,
        "running": TICK_RUNNING.load(Ordering::Relaxed),
        "lastTickAt": last_tick,
        "nextRunAt": next_all.map(|at| at.to_rfc3339()),
        "lastResult": last_result,
        "owner": "AICC 的账号助手进程",
        "continuesAfterQuit": false,
        "accounts": rows,
    })
}

/// 打开 / 关闭某个账号的自动签到。
///
/// 走专用写入：**不经过 `accounts.upsert`**，所以不会推进 `auth_epoch`
/// （PRD §2「已发现的一个重要问题」）。
pub fn set_enabled(app: &App, account_id: &str, enabled: bool) -> RpcResult<()> {
    let changed = app.with_db(|db| db.set_checkin_enabled(account_id, enabled))?;
    if !changed {
        return Err(crate::rpc::RpcError::not_found(format!(
            "账号不存在：{account_id}"
        )));
    }
    // 打开开关时清掉旧的退避：用户刚开启，不该等一个昨天的冷却。
    if enabled {
        write_next_at(app, account_id, None);
    }
    Ok(())
}

/// 跑一轮自动签到。
///
/// `force` 只跳过"到点没到点"，**不绕过账号开关**：没开的账号一个请求都不发。
pub async fn run_tick(app: &Arc<App>, force: bool) -> Value {
    let Some(_guard) = TickGuard::acquire() else {
        return json!({ "skipped": "already_running" });
    };
    let started = now_rfc3339();
    let mut value = run_tick_inner(app, force).await;
    value["startedAt"] = json!(started);
    value["finishedAt"] = json!(now_rfc3339());
    value
}

async fn run_tick_inner(app: &Arc<App>, force: bool) -> Value {
    let accounts = enabled_accounts(app);
    if accounts.is_empty() {
        return json!({ "skipped": "no_accounts_enabled", "accounts": [] });
    }

    let today = beijing_date();
    let mut runs: Vec<Value> = Vec::new();
    let mut ran = 0usize;

    for (index, account) in accounts.iter().enumerate() {
        if index > 0 {
            // 账号之间串行 + 少量间隔：不同账号不该在同一毫秒打同一个接口。
            tokio::time::sleep(INTER_ACCOUNT_DELAY).await;
        }
        let now = beijing_now();
        let day_start = beijing_day_start_utc(now);
        let plan = plan_time(now.date_naive(), &account.id);
        let done = done_today(app, &account.id, &today);
        let next = read_next_at(app, &account.id);

        if !force && !is_due(now, plan, done, next) {
            runs.push(json!({
                "accountId": account.id,
                "nickname": account.nickname,
                "state": if done { "already" } else { "waiting" },
                "reason": if done { "done_today" } else { "not_due" },
                "nextRunAt": next_run_at(now, &account.id, done, next).to_rfc3339(),
            }));
            continue;
        }

        let attempts = attempts_today(app, &account.id, &day_start);
        if done {
            runs.push(json!({
                "accountId": account.id,
                "nickname": account.nickname,
                "state": "already",
                "reason": "done_today",
                "nextRunAt": next_run_at(now, &account.id, true, next).to_rfc3339(),
            }));
            continue;
        }
        // 一天最多 N 次，**force 也不例外**。
        //
        // force 的语义只有一个："现在没到计划时间也跑一次"。它不能变成
        // "无限重试"的开关 —— 那样用户点几下「立即检查」就能把接口打成筛子，
        // 而这正是 #69 里唯一自愿放弃的速率保证。
        if attempts >= MAX_ATTEMPTS_PER_DAY {
            runs.push(json!({
                "accountId": account.id,
                "nickname": account.nickname,
                "state": "skipped",
                "reason": "attempts_exhausted",
                "attemptsToday": attempts,
                "nextRunAt": next_run_at(now, &account.id, false, read_next_at(app, &account.id))
                    .to_rfc3339(),
            }));
            continue;
        }

        ran += 1;
        let gate_app = Arc::clone(app);
        let gate_id = account.id.clone();
        let gate = move || still_enabled(&gate_app, &gate_id);
        let outcome = manager::run_account(app, account, Source::Auto, &gate).await;

        // 按结果安排下一次。
        let state = outcome.state;
        let rate_limited = outcome.error_kind.as_deref() == Some("rate_limited");
        let failures = attempts + 1;
        let now_after = beijing_now();
        match state {
            "claimed" | "already" => write_next_at(app, &account.id, None),
            "unauthorized" | "no_credential" => {
                // 今天别再试：授权问题重试也不会好。
                write_next_at(
                    app,
                    &account.id,
                    Some(plan_time(
                        now_after.date_naive() + chrono::Duration::days(1),
                        &account.id,
                    )),
                );
            }
            "failed" | "unknown" => {
                if failures >= MAX_ATTEMPTS_PER_DAY {
                    write_next_at(
                        app,
                        &account.id,
                        Some(plan_time(
                            now_after.date_naive() + chrono::Duration::days(1),
                            &account.id,
                        )),
                    );
                } else {
                    let delay = backoff_secs(failures, rate_limited);
                    write_next_at(
                        app,
                        &account.id,
                        Some(now_after + chrono::Duration::seconds(delay)),
                    );
                }
            }
            _ => {}
        }

        let mut run = json!(outcome);
        run["attemptsToday"] = json!(failures);
        if !matches!(state, "claimed" | "already") {
            run["nextRunAt"] = json!(next_run_at(
                beijing_now(),
                &account.id,
                false,
                read_next_at(app, &account.id)
            )
            .to_rfc3339());
        }
        runs.push(run);
    }

    let summary = summarize(&runs);
    if ran > 0 {
        // 只有真的跑过才推进：否则用户打开开关后要再等一天。
        let _ = app.with_db(|db| db.set_setting(SETTING_LAST_TICK, &now_rfc3339()));
        let _ = app.with_db(|db| db.set_setting(SETTING_LAST_RESULT, &summary));
    }

    json!({
        "skipped": Value::Null,
        "summary": summary,
        "accounts": runs,
        "ranAccounts": ran,
    })
}

/// 一句话结论，落在设置里给界面显示。
fn summarize(runs: &[Value]) -> String {
    let mut claimed = 0;
    let mut already = 0;
    let mut failed = 0;
    let mut waiting = 0;
    let mut credit = 0i64;
    for run in runs {
        match run.get("state").and_then(Value::as_str).unwrap_or("") {
            "claimed" => claimed += 1,
            "already" => already += 1,
            "failed" | "unknown" | "unauthorized" | "no_credential" => failed += 1,
            _ => waiting += 1,
        }
        credit += run.get("credits").and_then(Value::as_i64).unwrap_or(0);
    }
    let mut parts = Vec::new();
    if claimed > 0 {
        parts.push(format!("签到 {claimed} 个（+{credit}）"));
    }
    if already > 0 {
        parts.push(format!("已签过 {already} 个"));
    }
    if failed > 0 {
        parts.push(format!("{failed} 个失败"));
    }
    if waiting > 0 {
        parts.push(format!("{waiting} 个待处理"));
    }
    if parts.is_empty() {
        "没有需要处理的账号".to_string()
    } else {
        parts.join(" · ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checkin::state::State;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;
    use crate::storage::checkin::NewCheckinRecord;
    use crate::storage::AccountInput;
    use chrono::Timelike;

    fn test_app() -> (Arc<App>, tempfile::TempDir) {
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
        (app, dir)
    }

    fn account(app: &App, uid: &str, enabled: bool) -> String {
        app.with_db(|db| {
            let record = db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: uid.to_string(),
                nickname: Some(uid.to_string()),
                checkin_enabled: Some(enabled),
                ..Default::default()
            })?;
            Ok(record.id)
        })
        .expect("account")
    }

    fn at(text: &str) -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339(text).expect("rfc3339")
    }

    /// `TICK_RUNNING` 是进程级静态量，同进程里并行跑两个 tick 用例会互相挡住。
    /// 生产里只有一个心跳在叫它，这里只需要把测试串起来。
    fn tick_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
    }

    #[test]
    fn everything_is_off_by_default() {
        let (app, _dir) = test_app();
        account(&app, "uid-a", false);
        let status = auto_status(&app);
        assert_eq!(status["enabled"], json!(false));
        assert_eq!(status["enabledAccounts"], json!(0));
        assert_eq!(status["planTime"], json!("09:00"));
        assert_eq!(status["continuesAfterQuit"], json!(false));
    }

    #[test]
    fn nothing_runs_before_nine_beijing_time() {
        let plan = plan_time(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(), "acct");
        let early = at("2026-11-13T08:00:00+08:00");
        assert!(!is_due(early, plan, false, None), "09:00 之前不跑");
        let late = at("2026-11-13T09:30:00+08:00");
        assert!(is_due(late, plan, false, None));
    }

    #[test]
    fn a_done_day_never_runs_again() {
        let plan = plan_time(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(), "acct");
        let evening = at("2026-11-13T22:00:00+08:00");
        assert!(!is_due(evening, plan, true, None), "今天签过就不再跑");
    }

    #[test]
    fn the_date_comes_from_beijing_not_from_the_machine() {
        // 北京 11-14 00:30 = UTC 11-13 16:30。机器在 UTC 时区时，
        // 本地日期还是 13 号，但签到日期必须是 14 号。
        let beijing = at("2026-11-14T00:30:00+08:00");
        assert_eq!(
            beijing.date_naive(),
            NaiveDate::from_ymd_opt(2026, 11, 14).unwrap()
        );
        assert_eq!(beijing_day_start_utc(beijing), "2026-11-13T16:00:00Z");
    }

    #[test]
    fn staggering_is_deterministic_and_spread_out() {
        let a = stagger_offset("account-a");
        let b = stagger_offset("account-b");
        assert_eq!(a, stagger_offset("account-a"), "同一账号必须稳定");
        assert!(a <= MAX_STAGGER_SECS as i64);
        assert!(b <= MAX_STAGGER_SECS as i64);
        let plan_a = plan_time(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(), "account-a");
        assert_eq!(plan_a.hour(), PLAN_HOUR);
    }

    #[test]
    fn backoff_grows_and_caps() {
        assert_eq!(backoff_secs(1, false), 60);
        assert_eq!(backoff_secs(2, false), 300);
        assert_eq!(backoff_secs(3, false), 1800);
        assert_eq!(backoff_secs(9, false), 1800, "封顶 30 分钟");
        assert_eq!(
            backoff_secs(1, true),
            RATE_LIMIT_COOLDOWN_SECS,
            "429 保守冷却"
        );
    }

    #[test]
    fn a_backoff_blocks_the_next_tick_until_it_expires() {
        let plan = plan_time(NaiveDate::from_ymd_opt(2026, 11, 13).unwrap(), "acct");
        let now = at("2026-11-13T09:10:00+08:00");
        let later = now + chrono::Duration::seconds(300);
        assert!(!is_due(now, plan, false, Some(later.with_timezone(&Utc))));
        assert!(is_due(
            later + chrono::Duration::seconds(1),
            plan,
            false,
            Some(later.with_timezone(&Utc))
        ));
    }

    #[test]
    fn next_run_points_at_tomorrow_once_today_is_done() {
        let now = at("2026-11-13T10:00:00+08:00");
        let next = next_run_at(now, "acct", true, None);
        assert_eq!(
            next.date_naive(),
            NaiveDate::from_ymd_opt(2026, 11, 14).unwrap()
        );
    }

    #[test]
    fn the_switch_persists_and_never_touches_the_credential_epoch() {
        let (app, _dir) = test_app();
        let id = account(&app, "uid-a", false);
        let before = app
            .with_db(|db| db.get_account(&id))
            .unwrap()
            .unwrap()
            .auth_epoch;

        set_enabled(&app, &id, true).expect("on");
        let after = app.with_db(|db| db.get_account(&id)).unwrap().unwrap();
        assert!(after.checkin_enabled);
        assert_eq!(after.auth_epoch, before);

        assert!(set_enabled(&app, "nobody", true).is_err());
        assert_eq!(auto_status(&app)["enabledAccounts"], json!(1));
    }

    #[tokio::test]
    async fn a_tick_with_no_enabled_account_sends_nothing() {
        let _serial = tick_lock();
        let (app, _dir) = test_app();
        account(&app, "uid-a", false);
        let result = run_tick(&app, false).await;
        assert_eq!(result["skipped"], json!("no_accounts_enabled"));
        assert!(
            app.with_db(|db| db.get_setting(SETTING_LAST_TICK))
                .unwrap()
                .is_none(),
            "没跑就不该留下上次运行时间"
        );
    }

    /// 开关关着时 `force` 也只能什么都不做：force 不绕过账号开关。
    #[tokio::test]
    async fn force_does_not_bypass_the_per_account_switch() {
        let _serial = tick_lock();
        let (app, _dir) = test_app();
        account(&app, "uid-a", false);
        let result = run_tick(&app, true).await;
        assert_eq!(result["skipped"], json!("no_accounts_enabled"));
    }

    /// `force` 只跳过"到没到点"，**不许跳过一天 4 次的上限**。
    ///
    /// 不联网：上限判断发生在取凭据之前，所以这条用例在 CI 里是安全的。
    #[tokio::test]
    async fn force_cannot_bypass_the_daily_attempt_cap() {
        let _serial = tick_lock();
        let (app, _dir) = test_app();
        let id = account(&app, "uid-a", true);
        let now = now_rfc3339();
        for _ in 0..MAX_ATTEMPTS_PER_DAY {
            app.with_db(|db| {
                db.insert_checkin_attempt(crate::storage::checkin::NewAttempt {
                    account_id: &id,
                    status: crate::storage::checkin::ATTEMPT_FAILED,
                    started_at: &now,
                    finished_at: Some(&now),
                    credits: None,
                    error_kind: Some("network"),
                    error_detail: None,
                    next_retry_at: None,
                })
                .map(|_| ())
            })
            .unwrap();
        }

        let result = run_tick(&app, true).await;
        let runs = result["accounts"].as_array().expect("rows");
        assert_eq!(runs[0]["reason"], json!("attempts_exhausted"), "{result}");
        assert_eq!(result["ranAccounts"], json!(0), "满额之后 force 也不许再跑");
    }

    /// 还有余量时 `force` 要真的把账号送进执行路径（这里因为无凭据而立刻返回，
    /// 不会联网）。它证明上一条不是因为"force 根本不跑"才通过的。
    #[tokio::test]
    async fn force_still_runs_an_account_that_has_attempts_left() {
        let _serial = tick_lock();
        let (app, _dir) = test_app();
        account(&app, "uid-a", true);

        let result = run_tick(&app, true).await;
        assert_eq!(result["ranAccounts"], json!(1), "{result}");
        let runs = result["accounts"].as_array().expect("rows");
        assert_eq!(runs[0]["state"], json!("no_credential"), "{result}");
    }

    /// 今天已经有确认结论：即使到点也不发请求。
    #[tokio::test]
    async fn a_day_already_claimed_is_reported_but_not_rerun() {
        let _serial = tick_lock();
        let (app, _dir) = test_app();
        let id = account(&app, "uid-a", true);
        let today = beijing_date();
        app.with_db(|db| {
            db.upsert_checkin_record(NewCheckinRecord {
                account_id: &id,
                platform: "workbuddy",
                local_date: &today,
                status: crate::storage::checkin::STATUS_CLAIMED,
                credits: Some(100),
                error_kind: None,
                execution_source: "auto",
            })
            .map(|_| ())
        })
        .unwrap();

        let result = run_tick(&app, true).await;
        let runs = result["accounts"].as_array().expect("rows");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0]["reason"], json!("done_today"));
        assert_eq!(result["ranAccounts"], json!(0), "一个账号都不该真跑");
    }

    #[test]
    fn an_account_row_without_a_record_is_never_counted_as_done() {
        let (app, _dir) = test_app();
        let id = account(&app, "uid-a", true);
        assert!(!done_today(&app, &id, &beijing_date()));
    }

    /// PRD 10.1-19：跨日重置。昨天签过不影响今天"该跑"。
    #[test]
    fn yesterday_success_does_not_mark_today_as_done() {
        let (app, _dir) = test_app();
        let id = account(&app, "uid-a", true);
        app.with_db(|db| {
            db.upsert_checkin_record(NewCheckinRecord {
                account_id: &id,
                platform: "workbuddy",
                local_date: "2026-11-12",
                status: crate::storage::checkin::STATUS_CLAIMED,
                credits: Some(100),
                error_kind: None,
                execution_source: "auto",
            })
            .map(|_| ())
        })
        .unwrap();

        assert!(
            done_today(&app, &id, "2026-11-12"),
            "昨天那一天确实是签过的"
        );
        assert!(!done_today(&app, &id, "2026-11-13"), "换一天就必须重新检查");
    }

    /// PRD 10.1-21：App 重启后调度能恢复 —— 退避时间与尝试次数都在库里。
    #[test]
    fn the_retry_state_survives_a_restart() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let paths = Paths {
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data.clone(),
            wb_home_owned: true,
        };

        let future = beijing_now() + chrono::Duration::seconds(600);
        {
            let app = Arc::new(App::with_secure(paths.clone(), SecureStore::memory()));
            app.with_db(|db| Ok(db.migrate()?)).expect("migrate");
            let id = account(&app, "uid-a", true);
            write_next_at(&app, &id, Some(future));
            app.with_db(|db| {
                db.insert_checkin_attempt(crate::storage::checkin::NewAttempt {
                    account_id: &id,
                    status: crate::storage::checkin::ATTEMPT_FAILED,
                    started_at: &now_rfc3339(),
                    finished_at: Some(&now_rfc3339()),
                    credits: None,
                    error_kind: Some("network"),
                    error_detail: None,
                    next_retry_at: None,
                })
                .map(|_| ())
            })
            .unwrap();
        }

        // 新进程（新的 App 实例）打开同一个库：退避还在，所以现在不该跑。
        let app = Arc::new(App::with_secure(paths, SecureStore::memory()));
        app.with_db(|db| Ok(db.migrate()?)).expect("migrate");
        let id = app
            .with_db(|db| db.list_accounts(&Default::default()))
            .unwrap()
            .first()
            .map(|row| row.id.clone())
            .expect("账号还在");
        let now = beijing_now();
        let plan = plan_time(now.date_naive(), &id);
        assert!(!is_due(now, plan, false, read_next_at(&app, &id)));
        assert_eq!(
            attempts_today(&app, &id, &beijing_day_start_utc(now)),
            1,
            "尝试次数从库里读，不靠内存"
        );
    }

    #[test]
    fn state_strings_match_the_ui_contract() {
        // 界面按这些字符串分支，改名字要同时改 Swift。
        assert_eq!(State::Claimed.as_str(), "claimed");
        assert_eq!(State::Already.as_str(), "already");
        assert_eq!(State::Unknown.as_str(), "unknown");
        assert_eq!(State::Unauthorized.as_str(), "unauthorized");
        assert_eq!(State::Disabled.as_str(), "disabled");
        assert_eq!(State::Unsupported.as_str(), "unsupported");
        assert_eq!(State::Stopped.as_str(), "stopped");
    }

    /// 国际版**没有签到这项功能**：它不该出现在签到列表里，
    /// 也不该被自动轮次选中 —— 不是"暂不支持"，是根本没有。
    #[test]
    fn workbuddy_ai_is_absent_from_the_checkin_surface() {
        let (app, _dir) = test_app();
        let cn = account(&app, "cn-uid", true);
        app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: "ai-uid".to_string(),
                provider: Some("workbuddy_ai".to_string()),
                checkin_enabled: Some(true),
                ..Default::default()
            })?;
            Ok(())
        })
        .expect("ai account");

        let picked: Vec<String> = enabled_accounts(&app)
            .iter()
            .map(|a| a.id.clone())
            .collect();
        assert_eq!(picked, vec![cn.clone()], "国际版不能进自动轮次");

        let status = auto_status(&app);
        let rows = status["accounts"].as_array().unwrap();
        assert_eq!(rows.len(), 1, "签到列表里不该出现国际版：{rows:?}");
        assert_eq!(rows[0]["accountId"], serde_json::json!(cn));
        assert_eq!(status["enabledAccounts"], serde_json::json!(1));
        assert_eq!(status["totalAccounts"], serde_json::json!(1));
    }
}
