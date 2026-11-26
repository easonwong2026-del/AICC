//! 夜间账号轮换：**按时切换一串账号，并在窗口结束时回到这一轮最初的那个**。
//!
//! ## 一句话
//!
//! 22:30 原本登录 A → 23:00 切 B → 00:00 切 C → 01:00 切 D → 07:00 **回 A**，
//! 不是回 C。所以"最初登录态"必须单独存一份，不能靠 `switch_journal` 的
//! "回到上一站"。
//!
//! ## 三层职责
//!
//! | 层 | 负责 | 不负责 |
//! | --- | --- | --- |
//! | 本模块 | 时窗、状态机、原账号快照、恢复、活跃取证 | 界面文案 |
//! | [`super::service`] | 一次切换的编排（关客户端 / 写认证 / 回读 / 回滚） | 计划与多账号 |
//! | [`super::plan`] | 单个账号能不能切、用哪份授权 | 什么时候切 |
//!
//! ## 状态机（每一步都可以被下一次 tick 接着推进）
//!
//! ```text
//! scheduled ──► prechecking ──► switching ──► dwelling ──┐
//!                    │              │              │      │
//!                    ├─► skipped    ├─► restoring ◄┘      │
//!                    └─► failed     │                     │
//!                                   └─► recovery_required ◄┘
//! restoring ──► completed / recovery_required / failed
//! ```
//!
//! 所有推进都用 `state` 的 CAS：同一时刻只可能有一个 worker 把 `dwelling` 改成
//! `switching`。另一个拿到 `false`，这一轮就什么都不做——重复心跳与多进程并发
//! 都不会造成重复切换。
//!
//! ## 什么时候**不**动手
//!
//! - 客户端没装 / 没登录（没有可恢复的"最初账号"）→ 本轮 `skipped`。
//! - 用户正在使用（HID 空闲时间 < 2 分钟）→ 推迟这一跳，不突然关客户端。
//! - 错过开始时间超过 10 分钟（可能刚从休眠唤醒）→ 本轮 `skipped`，**不补跑**。
//! - 用户在轮换期间自己换了账号 → 立即停止计划，且**不覆盖**他的选择。
//! - 凭据预检失败（401 / Magpie 剩余不够）→ 跳过那个账号，不硬切。
//!
//! ## 已知限制（界面与文档必须说同一句话）
//!
//! 调度器活在 `aicc-account-worker` 里，而它是 `AICC.app` 的子进程。
//! **AICC 退出后计划不会继续执行**；Mac 真正休眠时也不会"假装执行过"——
//! 醒来后如果已经过了开始时间 + 10 分钟，这一轮直接跳过。

use std::path::{Path, PathBuf};

use chrono::{Datelike, Duration, FixedOffset, NaiveDate, NaiveDateTime, TimeZone};
use serde_json::{json, Value};

use crate::app::App;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::rotation::{
    self, NewRotationRun, RotationOrigin, RotationPlan, RotationRun, RotationStep,
    RotationSwitchLink, STATE_COMPLETED, STATE_DWELLING, STATE_PRECHECKING,
    STATE_RECOVERY_REQUIRED, STATE_RESTORING, STATE_SCHEDULED, STATE_SKIPPED, STATE_SWITCHING,
    STEP_ALREADY_LIVE, STEP_DONE, STEP_DWELLING, STEP_FAILED, STEP_PLANNED, STEP_SKIPPED,
    STEP_SWITCHING,
};
use crate::storage::AccountRecord;

use super::client::{self, ClientControl};
use super::plan::{self, PlanOutcome};
use super::service::{self, SwitchEnv};

/// 错过开始时间多久就不再补跑。
///
/// 10 分钟足够覆盖一次短暂的时钟漂移；再久就多半是"Mac 刚睡醒"——
/// 那种情况下补跑会把整晚的时间表挤成一团。
pub const LATE_START_TOLERANCE_SECS: i64 = 10 * 60;

/// 窗口结束前留出的恢复时间。恢复要关客户端、写文件、等它起来。
pub const RESTORE_RESERVE_SECS: i64 = 5 * 60;

/// 用户"正在使用"的判定：HID 空闲时间低于这个值就认为他在键盘/鼠标前。
///
/// 只是**保守推迟**，不是硬性禁止：到了必须恢复的时候仍然执行。
pub const USER_ACTIVE_IDLE_SECS: i64 = 120;

/// 一轮里最多几个账号。再多就不是"轮换"，而是把客户端当拨盘。
pub const MAX_ACCOUNTS_PER_RUN: usize = 12;

/// 支持的 IANA 时区。
///
/// 仓库没有 `chrono-tz`，手写 DST 规则一定会出错。这里只放行**固定偏移**且
/// 无夏令时的两个；其余配置在保存时直接拒绝，而不是悄悄按 +8 算。
pub const SUPPORTED_TIMEZONES: &[&str] = &["Asia/Shanghai", "UTC"];

// ---------------------------------------------------------------------------
// 注入点
// ---------------------------------------------------------------------------

/// 时钟。整晚计划必须能在测试里"推进到第二天"，不能真的等。
pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        chrono::Utc::now().timestamp_millis()
    }
}

/// 用户是否正在使用这台机器。
///
/// `None` = 判不出来。**判不出来不等于正在使用**：那会让计划永远不执行；
/// 也不能等于"没在用"而贸然关掉客户端。调用方按"保守但会推进"处理。
pub trait IdleProbe: Send + Sync {
    fn idle_secs(&self) -> Option<i64>;
}

pub struct SystemIdle;

impl IdleProbe for SystemIdle {
    fn idle_secs(&self) -> Option<i64> {
        idle_secs_with(probe_ioreg)
    }
}

/// 两段式读取：先窄查询，再全量 dump。
///
/// 窄查询（`-c IOHIDSystem -d 1`）在旧系统上最快，但 **macOS 26/27 上它只回
/// `Root`**：`IOHIDSystem` 不再作为 IOService 暴露，`HIDIdleTime` 一条都没有。
/// 全量 `-l` 里仍然有这条属性（本机 0.16s 左右）。只认窄查询的写法会让
/// "读不到空闲"变成常态，进而每一轮都保守跳过 —— 计划永远不执行。
///
/// 两段都失败仍然返回 `None`：**读不到不等于用户不在用**，不能据此关客户端。
fn idle_secs_with(probe: impl Fn(&[&str]) -> Option<i64>) -> Option<i64> {
    probe(&["-c", "IOHIDSystem", "-d", "1"]).or_else(|| probe(&["-l"]))
}

/// 跑一次 `ioreg` 并解析。绝对路径：GUI 进程的 `PATH` 不一定带 `/usr/sbin`。
fn probe_ioreg(args: &[&str]) -> Option<i64> {
    let output = std::process::Command::new("/usr/sbin/ioreg")
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_idle(&String::from_utf8_lossy(&output.stdout))
}

/// `"HIDIdleTime" = <纳秒>` → 秒。
///
/// 属性缺失、值不是数字、溢出都返回 `None`；**不猜**，也不把失败当成 0 秒
/// （0 秒的意思是"刚刚还在动"，会把一轮轮换一直推迟）。
fn parse_idle(text: &str) -> Option<i64> {
    const MARKER: &str = "\"HIDIdleTime\" =";
    let start = text.find(MARKER)? + MARKER.len();
    let digits: String = text[start..]
        .trim_start()
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let nanos: i64 = digits.parse().ok()?;
    Some(nanos / 1_000_000_000)
}

// 进程内互斥：tick、立即执行、暂停、恢复都可能同时被触发。
//
// 数据库层面还有 `open_slot` 与状态 CAS 兜底，但那只能挡住"重复切换"，
// 挡不住"一边恢复一边切换"。这个开关让第二个动作直接返回忙。
//
// 用线程局部而不是全局静态：worker 是单线程 runtime（tick 与 RPC 在同一线程），
// 而 libtest 每个用例一个线程——全局静态会让并行的测试用例互相把对方挡在门外，
// 测出来的全是 busy。
thread_local! {
    static ROTATION_BUSY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

struct BusyGuard;

impl BusyGuard {
    fn acquire() -> Option<Self> {
        ROTATION_BUSY.with(|flag| (!flag.replace(true)).then_some(BusyGuard))
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        ROTATION_BUSY.with(|flag| flag.set(false));
    }
}

/// 一次 future 的类型别名：手写 trait 对象时用得着。
pub type BoxFut<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// 自有授权的只读预检。
///
/// 抽成 trait 有两个原因：**测试绝对不能联网**；以及"预检失败"与"预检说不能用"
/// 必须是两条不同的路径（前者跳过并记录，后者跳过并提示重新授权）。
pub trait OwnPrecheck: Send + Sync {
    fn check<'a>(
        &'a self,
        app: &'a App,
        account: &'a AccountRecord,
    ) -> BoxFut<'a, RpcResult<String>>;
}

pub struct SystemPrecheck;

impl OwnPrecheck for SystemPrecheck {
    fn check<'a>(
        &'a self,
        app: &'a App,
        account: &'a AccountRecord,
    ) -> BoxFut<'a, RpcResult<String>> {
        Box::pin(async move { service::precheck_own(app, account).await })
    }
}

/// 活跃取证（只读）。失败不影响恢复，也不影响任何判定。
pub trait ActivityProbe: Send + Sync {
    fn snapshot<'a>(&'a self, app: &'a App, account_id: &'a str) -> BoxFut<'a, Value>;
}

pub struct SystemActivity;

impl ActivityProbe for SystemActivity {
    fn snapshot<'a>(&'a self, app: &'a App, account_id: &'a str) -> BoxFut<'a, Value> {
        Box::pin(async move { service::activity_snapshot(app, account_id).await })
    }
}

/// **跨进程**独占：同一台机器上只允许一个 AICC 进程操作 WorkBuddy 客户端。
///
/// 为什么进程内互斥不够：两个 `aicc-account-worker`（例如旧 App 没退干净、
/// 或用户手动开了两个）各自持有一份自己的 `BusyGuard`，status 里的 CAS 也只能
/// 保证"状态迁移不重复"，挡不住两个进程同时进 `prechecking` 各关一次客户端。
///
/// 用 `flock` 而不是锁文件里的 pid：进程崩溃时内核自动释放，不需要"清理陈旧锁"，
/// 也不会因为 pid 复用判错。锁文件本身残留是无害的。
struct RotationLock {
    _file: std::fs::File,
}

impl RotationLock {
    fn acquire(path: &Path) -> RpcResult<Option<Self>> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| RpcError::internal(format!("创建轮换锁目录失败：{err}")))?;
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|err| RpcError::internal(format!("打开轮换锁失败：{err}")))?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            // 另一个进程正在跑：不是错误，这一轮让给它。
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(err)) => {
                Err(RpcError::internal(format!("轮换锁不可用：{err}")))
            }
        }
    }
}

/// 一次操作客户端所需的全部独占权：进程内 + 跨进程。
///
/// **手动切换 / 恢复与夜间轮换共用这一把**。只护轮换是不够的：用户点
/// 「切换到此账号」时，两边会各关一次客户端、互相覆盖同一个认证文件。
pub struct ClientGate {
    _local: BusyGuard,
    _cross_process: RotationLock,
}

/// 拿客户端独占权。拿不到返回 `None`（另一个动作正在跑），不排队、不阻塞。
pub fn acquire_client_gate(env: &SwitchEnv) -> RpcResult<Option<ClientGate>> {
    let Some(local) = BusyGuard::acquire() else {
        return Ok(None);
    };
    let path = env.snapshot_dir.join("rotation.lock");
    let Some(cross_process) = RotationLock::acquire(&path)? else {
        return Ok(None);
    };
    Ok(Some(ClientGate {
        _local: local,
        _cross_process: cross_process,
    }))
}

fn acquire_gate(deps: &RotationDeps<'_>) -> RpcResult<Option<ClientGate>> {
    acquire_client_gate(deps.env)
}

/// 手动切换 / 恢复的准入：要么拿到独占权，要么明确拒绝。
///
/// 拿到之后立刻把正在跑的这一轮**收尾**（不是 `restoring`）：用户刚选的账号
/// 才是他要的，任何自动恢复都不该盖过去。此刻没有 tick 在跑，标记是安全的。
pub fn begin_manual_client_write(app: &App, env: &SwitchEnv) -> RpcResult<ClientGate> {
    let Some(gate) = acquire_client_gate(env)? else {
        return Err(RpcError::new(
            crate::rpc::E_SWITCH,
            "AICC 正在切换账号（自动轮换或另一个窗口），请稍后再试",
        )
        .with_data(json!({ "kind": "switch_busy" })));
    };
    if let Some(run) = app.with_db(|db| db.active_rotation_run())? {
        app.with_db(|db| {
            db.set_rotation_state(
                &run.id,
                STATE_COMPLETED,
                Some("你在轮换期间手动换号了，本轮已停止，不会覆盖你选的账号"),
            )
        })?;
    }
    Ok(gate)
}

pub struct RotationDeps<'a> {
    pub control: &'a dyn ClientControl,
    pub env: &'a SwitchEnv,
    pub clock: &'a dyn Clock,
    pub idle: &'a dyn IdleProbe,
    pub precheck: &'a dyn OwnPrecheck,
    pub activity: &'a dyn ActivityProbe,
}

// ---------------------------------------------------------------------------
// 时区与窗口
// ---------------------------------------------------------------------------

/// 固定偏移。只接受 [`SUPPORTED_TIMEZONES`]。
pub fn tz_offset(name: &str) -> Option<FixedOffset> {
    match name {
        "Asia/Shanghai" => FixedOffset::east_opt(8 * 3600),
        "UTC" => FixedOffset::east_opt(0),
        _ => None,
    }
}

fn local_to_ms(at: NaiveDateTime, tz: FixedOffset) -> i64 {
    tz.from_local_datetime(&at)
        .single()
        .map(|value| value.timestamp_millis())
        .unwrap_or(0)
}

fn ms_to_rfc3339(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

fn parse_rfc3339_ms(raw: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|at| at.timestamp_millis())
}

/// 一轮的时间窗口。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// 窗口**开始**那天的本地日期。跨午夜时结束在次日。
    pub date: NaiveDate,
    pub start_ms: i64,
    pub end_ms: i64,
}

impl Window {
    pub fn contains(&self, now_ms: i64) -> bool {
        now_ms >= self.start_ms && now_ms < self.end_ms
    }
}

/// 某个本地日期对应的窗口。`end <= start` 表示跨午夜。
pub fn window_for_date(plan: &RotationPlan, date: NaiveDate) -> Option<Window> {
    let tz = tz_offset(&plan.timezone)?;
    let start = date.and_hms_opt(
        (plan.start_minutes / 60).try_into().ok()?,
        (plan.start_minutes % 60).try_into().ok()?,
        0,
    )?;
    let end_date = if plan.end_minutes <= plan.start_minutes {
        date.succ_opt()?
    } else {
        date
    };
    let end = end_date.and_hms_opt(
        (plan.end_minutes / 60).try_into().ok()?,
        (plan.end_minutes % 60).try_into().ok()?,
        0,
    )?;
    Some(Window {
        date,
        start_ms: local_to_ms(start, tz),
        end_ms: local_to_ms(end, tz),
    })
}

fn local_date(ms: i64, tz: FixedOffset) -> NaiveDate {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|at| at.with_timezone(&tz).date_naive())
        .unwrap_or_else(|| chrono::Utc::now().with_timezone(&tz).date_naive())
}

/// 现在是否落在某个窗口里。会同时看"昨天开始、今天结束"的跨午夜窗口。
pub fn active_window(plan: &RotationPlan, now_ms: i64) -> Option<Window> {
    let tz = tz_offset(&plan.timezone)?;
    let today = local_date(now_ms, tz);
    for delta in [-1i64, 0] {
        let Some(date) = today.checked_add_signed(Duration::days(delta)) else {
            continue;
        };
        let Some(window) = window_for_date(plan, date) else {
            continue;
        };
        let weekday = date.weekday().number_from_monday() as u8;
        if !plan.weekdays.is_empty() && !plan.weekdays.contains(&weekday) {
            continue;
        }
        if window.contains(now_ms) {
            return Some(window);
        }
    }
    None
}

/// 下一次窗口。找不到（例如 weekdays 为空）返回 `None`。
pub fn next_window(plan: &RotationPlan, now_ms: i64) -> Option<Window> {
    let tz = tz_offset(&plan.timezone)?;
    let today = local_date(now_ms, tz);
    for delta in 0..8i64 {
        let date = today.checked_add_signed(Duration::days(delta))?;
        let weekday = date.weekday().number_from_monday() as u8;
        if !plan.weekdays.is_empty() && !plan.weekdays.contains(&weekday) {
            continue;
        }
        let window = window_for_date(plan, date)?;
        if window.start_ms > now_ms {
            return Some(window);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 配置校验
// ---------------------------------------------------------------------------

/// 保存前校验。任何一条不满足都直接报错——**不猜用户想要什么**。
pub fn validate_plan(plan: &RotationPlan) -> Result<(), String> {
    if tz_offset(&plan.timezone).is_none() {
        return Err(format!(
            "暂不支持时区「{}」，可选：{}",
            plan.timezone,
            SUPPORTED_TIMEZONES.join(" / ")
        ));
    }
    if plan.weekdays.is_empty() {
        return Err("至少要选一个星期".to_string());
    }
    if plan.start_minutes == plan.end_minutes {
        return Err("开始时间和结束时间不能相同".to_string());
    }
    if !(0..1440).contains(&plan.start_minutes) || !(0..1440).contains(&plan.end_minutes) {
        return Err("时间必须在 00:00 到 23:59 之间".to_string());
    }
    if plan.dwell_seconds < rotation::MIN_DWELL_SECONDS
        || plan.dwell_seconds > rotation::MAX_DWELL_SECONDS
    {
        return Err(format!(
            "每个账号停留时间要在 {} 分钟到 {} 小时之间",
            rotation::MIN_DWELL_SECONDS / 60,
            rotation::MAX_DWELL_SECONDS / 3600
        ));
    }
    if plan.account_ids.len() > MAX_ACCOUNTS_PER_RUN {
        return Err(format!("一轮最多 {MAX_ACCOUNTS_PER_RUN} 个账号"));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 生产入口
// ---------------------------------------------------------------------------

/// 生产 tick：自己构造系统实现。`force` = 用户点了「立即执行一轮」。
pub async fn run_tick(app: &App, force: bool) -> RpcResult<Value> {
    let control =
        super::SystemControl::for_variant(wb_switch_core::modules::variant::WbVariant::Cn);
    let env = SwitchEnv::system(app);
    let deps = RotationDeps {
        control: &control,
        env: &env,
        clock: &SystemClock,
        idle: &SystemIdle,
        precheck: &SystemPrecheck,
        activity: &SystemActivity,
    };
    run_tick_with(app, &deps, force).await
}

/// 一次心跳。设计成**幂等**：连续调两次，第二次什么都不会多做。
pub async fn run_tick_with(app: &App, deps: &RotationDeps<'_>, force: bool) -> RpcResult<Value> {
    let Some(_gate) = acquire_gate(deps)? else {
        return Ok(json!({ "status": "busy", "detail": "另一个 AICC 进程正在执行轮换" }));
    };
    let plan = app.with_db(|db| db.rotation_plan_or_default())?;
    let now = deps.clock.now_ms();

    // 开关关掉 = **不再开新轮次**，但还在跑的那一轮必须安全收尾。
    //
    // 这两件事必须分开：只把 enabled 置 0 而让轮次继续，用户会看到"关掉了还在切"；
    // 而直接丢弃轮次会让客户端停在一个轮换账号上过夜。所以这里的语义是
    // 「停止后续计划 + 立刻恢复原账号」，界面文案与它一致。
    if !plan.enabled && !force {
        if let Some(run) = app.with_db(|db| db.active_rotation_run())? {
            // 用户手动发起的一轮**不受自动开关影响**：它有自己算出来的窗口，
            // 会按停留时长走完再恢复。把它当成"关闭后的遗留"直接收尾，
            // 就会出现"点了立即执行，下一次心跳就把账号切回去"。
            if run.trigger == crate::storage::rotation::TRIGGER_MANUAL {
                return advance_with_depth(app, deps, &plan, run, now, 0).await;
            }
            if run.state != STATE_RESTORING {
                app.with_db(|db| {
                    db.set_rotation_state(
                        &run.id,
                        STATE_RESTORING,
                        Some("自动轮换已关闭，正在把账号恢复到这一轮最初登录的那个"),
                    )
                })?;
            }
            let run = reload(app, &run.id)?;
            return advance_with_depth(app, deps, &plan, run, now, 0).await;
        }
        return Ok(json!({ "status": "disabled" }));
    }

    if let Some(run) = app.with_db(|db| db.active_rotation_run())? {
        return advance_with_depth(app, deps, &plan, run, now, 0).await;
    }
    if plan.account_ids.is_empty() {
        return Ok(json!({ "status": "not_configured" }));
    }
    if let Err(why) = validate_plan(&plan) {
        return Ok(json!({ "status": "invalid_plan", "detail": why }));
    }

    let window = match (active_window(&plan, now), force) {
        (Some(window), _) => window,
        // 「立即执行一轮」可以不在窗口内：给用户做短时验证用，窗口按停留时长推算。
        (None, true) => {
            let count = plan.account_ids.len().max(1) as i64;
            let span_ms = (plan.dwell_seconds * count + RESTORE_RESERVE_SECS * 2) * 1000;
            let tz = tz_offset(&plan.timezone)
                .unwrap_or_else(|| FixedOffset::east_opt(8 * 3600).expect("UTC+8 固定偏移"));
            Window {
                date: local_date(now, tz),
                start_ms: now,
                end_ms: now + span_ms,
            }
        }
        (None, false) => {
            return Ok(json!({
                "status": "idle",
                "nextRunAt": next_window(&plan, now).map(|w| ms_to_rfc3339(w.start_ms)),
            }))
        }
    };

    let plan_date = window.date.format("%Y-%m-%d").to_string();
    if app
        .with_db(|db| db.rotation_run_for_date(&plan_date))?
        .is_some()
    {
        return Ok(json!({ "status": "already_ran", "planDate": plan_date }));
    }

    // 睡醒后才发现已经错过开始时间：不补跑，但要留一条记录，避免每分钟重试。
    if !force && now - window.start_ms > LATE_START_TOLERANCE_SECS * 1000 {
        let id = uuid::Uuid::new_v4().to_string();
        app.with_db(|db| {
            db.begin_rotation_run(&NewRotationRun {
                id: &id,
                plan_id: &plan.id,
                plan_date: &plan_date,
                timezone: &plan.timezone,
                window_start: &ms_to_rfc3339(window.start_ms),
                window_end: &ms_to_rfc3339(window.end_ms),
                trigger: crate::storage::rotation::TRIGGER_SCHEDULED,
            })?;
            db.set_rotation_state(
                &id,
                STATE_SKIPPED,
                Some("错过了开始时间（可能刚从休眠唤醒），本轮不补跑"),
            )
        })?;
        return Ok(json!({
            "status": "skipped",
            "runId": id,
            "reason": "late_start",
            "detail": "错过了开始时间，本轮不补跑；不会在第二天一次性补多轮",
        }));
    }

    let id = uuid::Uuid::new_v4().to_string();
    app.with_db(|db| {
        db.begin_rotation_run(&NewRotationRun {
            id: &id,
            plan_id: &plan.id,
            plan_date: &plan_date,
            timezone: &plan.timezone,
            window_start: &ms_to_rfc3339(window.start_ms),
            window_end: &ms_to_rfc3339(window.end_ms),
            trigger: if force {
                crate::storage::rotation::TRIGGER_MANUAL
            } else {
                crate::storage::rotation::TRIGGER_SCHEDULED
            },
        })
    })?;
    let run = app
        .with_db(|db| db.rotation_run(&id))?
        .ok_or_else(|| RpcError::internal("轮次创建后读不回来"))?;
    advance_with_depth(app, deps, &plan, run, now, 0).await
}

// ---------------------------------------------------------------------------
// 状态机
// ---------------------------------------------------------------------------

/// 一次 tick 内的推进层数上限。
///
/// 正常路径最多十几层（预检 → 切换 → 停留 → 恢复）。超过就是状态机在打转
/// （例如某个 CAS 一直失败却还在递归），宁可报错也不要让 worker 卡死。
const MAX_ADVANCE_DEPTH: u32 = 32;

async fn advance_with_depth(
    app: &App,
    deps: &RotationDeps<'_>,
    plan: &RotationPlan,
    run: RotationRun,
    now: i64,
    depth: u32,
) -> RpcResult<Value> {
    // 用户明确要求停止（暂停 / 关闭开关）：**优先于手动轮次的豁免**。
    // 请求在 `stop()` 里先落库，所以哪怕当时拿不到锁，下一次心跳也会照办。
    if run.stop_requested && !run.is_terminal() && run.state != STATE_RESTORING {
        app.with_db(|db| {
            db.set_rotation_state(
                &run.id,
                STATE_RESTORING,
                Some("已请求停止，正在恢复这一轮最初登录的账号"),
            )
        })?;
        let run = reload(app, &run.id)?;
        return Box::pin(advance_with_depth(app, deps, plan, run, now, depth + 1)).await;
    }

    if depth > MAX_ADVANCE_DEPTH {
        return Err(RpcError::internal(format!(
            "夜间轮换推进层数过深（state={}，cursor={}），已停止本次 tick",
            run.state, run.cursor
        )));
    }
    match run.state.as_str() {
        STATE_SCHEDULED => {
            if !app
                .with_db(|db| db.cas_rotation_state(&run.id, STATE_SCHEDULED, STATE_PRECHECKING))?
            {
                return Ok(busy(&run));
            }
            let run = reload(app, &run.id)?;
            Box::pin(advance_with_depth(app, deps, plan, run, now, depth + 1)).await
        }
        STATE_PRECHECKING => {
            let outcome = precheck(app, deps, plan, &run, now).await?;
            match outcome {
                Precheck::Ready => {
                    let run = reload(app, &run.id)?;
                    Box::pin(advance_with_depth(app, deps, plan, run, now, depth + 1)).await
                }
                Precheck::Wait { detail } => Ok(json!({
                    "status": STATE_PRECHECKING,
                    "runId": run.id,
                    "detail": detail,
                })),
                Precheck::Stop { state, detail } => {
                    app.with_db(|db| db.set_rotation_state(&run.id, state, Some(&detail)))?;
                    Ok(json!({
                        "status": state,
                        "runId": run.id,
                        "detail": detail,
                    }))
                }
            }
        }
        STATE_SWITCHING => {
            let run = reload(app, &run.id)?;
            let next = switch_current(app, deps, plan, &run, now).await?;
            let run = reload(app, &run.id)?;
            match next {
                StepGate::Continue => {
                    Box::pin(advance_with_depth(app, deps, plan, run, now, depth + 1)).await
                }
                StepGate::Wait { detail } => Ok(json!({
                    "status": STATE_SWITCHING,
                    "runId": run.id,
                    "detail": detail,
                })),
                StepGate::Restore { detail } => {
                    app.with_db(|db| {
                        db.cas_rotation_state(&run.id, STATE_SWITCHING, STATE_RESTORING)?;
                        db.set_rotation_state(&run.id, STATE_RESTORING, Some(&detail))
                    })?;
                    let run = reload(app, &run.id)?;
                    Box::pin(advance_with_depth(app, deps, plan, run, now, depth + 1)).await
                }
                StepGate::Halt { detail } => {
                    app.with_db(|db| {
                        db.set_rotation_state(&run.id, STATE_COMPLETED, Some(&detail))?;
                        let _ = db.settle_all_pending_temporary();
                        Ok::<(), RpcError>(())
                    })?;
                    Ok(json!({ "status": STATE_COMPLETED, "runId": run.id, "detail": detail }))
                }
            }
        }
        STATE_DWELLING => dwelling(app, deps, plan, &run, now, depth).await,
        STATE_RESTORING => {
            let outcome = restore_origin(app, deps, &run, false)?;
            let state = if outcome.ok {
                STATE_COMPLETED
            } else {
                STATE_RECOVERY_REQUIRED
            };
            // 保留"为什么开始恢复"（例如用户关了开关），再补上恢复结论。
            // 只写结论会把原因盖掉，第二天没人知道这一轮为什么提前结束。
            let detail = match run.detail.as_deref().filter(|text| !text.is_empty()) {
                Some(cause) => format!("{cause}；{}", outcome.detail),
                None => outcome.detail.clone(),
            };
            app.with_db(|db| {
                db.note_rotation_restore(&run.id, run.restore_attempts + 1, &outcome.code)?;
                db.set_rotation_state(&run.id, state, Some(&detail))
            })?;
            Ok(json!({
                "status": state,
                "runId": run.id,
                "restored": outcome.ok,
                "detail": outcome.detail,
            }))
        }
        _ => Ok(json!({ "status": run.state, "runId": run.id, "terminal": true })),
    }
}

fn busy(run: &RotationRun) -> Value {
    json!({ "status": "busy", "runId": run.id, "state": run.state })
}

fn reload(app: &App, id: &str) -> RpcResult<RotationRun> {
    app.with_db(|db| db.rotation_run(id))?
        .ok_or_else(|| RpcError::not_found(format!("轮换记录不存在：{id}")))
}

enum Precheck {
    Ready,
    /// 条件暂时不满足，但**不能**把这一轮结掉：等下一次心跳再看。
    /// 用在"用户正在用"这种会自己消失的情况上。
    Wait {
        detail: String,
    },
    Stop {
        state: &'static str,
        detail: String,
    },
}

/// 预检：决定这轮到底要不要动客户端，并把每一步先登记好。
///
/// **顺序很重要**：先算"有没有可切换的目标"，再决定要不要为拍原账号快照而
/// 关一次客户端。一个全都被跳过的轮次不应该重启客户端。
async fn precheck(
    app: &App,
    deps: &RotationDeps<'_>,
    plan: &RotationPlan,
    run: &RotationRun,
    now: i64,
) -> RpcResult<Precheck> {
    let installed = deps.control.installed();
    if !installed {
        return Ok(Precheck::Stop {
            state: STATE_SKIPPED,
            detail: "这台机器上没找到 WorkBuddy 客户端".to_string(),
        });
    }

    let accounts = app.with_db(|db| db.list_accounts(&Default::default()))?;
    let live = client::read_live(&deps.env.auth_path);
    let live_uid = live.uid.clone();

    let mut planned = 0usize;
    let mut blocked_reasons: Vec<String> = Vec::new();
    let mut plans: Vec<(AccountRecord, plan::Plan)> = Vec::new();
    let mut skipped: Vec<(AccountRecord, &'static str, String)> = Vec::new();
    let mut already_live: Vec<AccountRecord> = Vec::new();

    for (seq, account_id) in plan.account_ids.iter().enumerate() {
        let Some(account) = accounts.iter().find(|row| &row.id == account_id).cloned() else {
            blocked_reasons.push(format!("账号 {account_id} 已不在账号列表里"));
            continue;
        };
        if seq >= MAX_ACCOUNTS_PER_RUN {
            break;
        }
        match plan::resolve(app, &account, installed, live_uid.as_deref()) {
            PlanOutcome::Ready(plan) => {
                planned += 1;
                plans.push((account, plan));
            }
            PlanOutcome::Blocked { reason, detail } => {
                if reason == plan::reason::LIVE {
                    already_live.push(account);
                } else {
                    skipped.push((account, reason, detail));
                }
            }
        }
    }

    if planned == 0 {
        // 一个都切不了：不关客户端，也不拍快照。
        for (seq, account) in already_live.iter().enumerate() {
            let step_id = uuid::Uuid::new_v4().to_string();
            app.with_db(|db| {
                db.begin_rotation_step(
                    &step_id,
                    &run.id,
                    seq as i64,
                    &account.id,
                    &account.uid,
                    None,
                )?;
                db.finish_rotation_step(
                    &step_id,
                    STEP_ALREADY_LIVE,
                    Some("客户端当前登录的就是它"),
                    None,
                    Some(&ms_to_rfc3339(deps.clock.now_ms())),
                )
            })?;
        }
        for (seq, (account, reason, detail)) in skipped.iter().enumerate() {
            let step_id = uuid::Uuid::new_v4().to_string();
            let seq = (already_live.len() + seq) as i64;
            app.with_db(|db| {
                db.begin_rotation_step(&step_id, &run.id, seq, &account.id, &account.uid, None)?;
                db.finish_rotation_step(
                    &step_id,
                    STEP_SKIPPED,
                    Some(detail),
                    Some(reason),
                    Some(&ms_to_rfc3339(deps.clock.now_ms())),
                )
            })?;
        }
        let detail = if blocked_reasons.is_empty() {
            "没有可切换的账号：目标要么已经是当前登录，要么凭据不可用".to_string()
        } else {
            format!("没有可切换的账号：{}", blocked_reasons.join("；"))
        };
        return Ok(Precheck::Stop {
            state: STATE_SKIPPED,
            detail,
        });
    }

    // 到这里已经确定"有可切目标"。但**关客户端之前**必须先看两件事：
    // 时间够不够，以及用户是不是正在用。顺序错了就会在凌晨一点把用户正在写的
    // 文档连同客户端一起关掉。
    let window_end = parse_rfc3339_ms(&run.window_end).unwrap_or(now);
    let reserve_ms = RESTORE_RESERVE_SECS * 1000;
    let need_ms = plan.dwell_seconds * 1000;

    // 剩余时间连"切一次 + 停一会儿 + 留出恢复余量"都不够：根本不开始。
    if now + need_ms >= window_end - reserve_ms {
        return Ok(Precheck::Stop {
            state: STATE_SKIPPED,
            detail: "窗口剩余时间不足一个停留时长，本轮不执行".to_string(),
        });
    }

    // 用户在键盘前：不关客户端。还等得起就等下一跳，等不起就明确跳过。
    //
    // `None` = 读不到空闲状态（ioreg 失败 / 平台不支持）。**证明不了用户不在用，
    // 就按"在用"处理**：宁可整轮跳过，也不要在别人写文档时把客户端关掉。
    let idle = deps.idle.idle_secs();
    if idle.map_or(true, |secs| secs < USER_ACTIVE_IDLE_SECS) {
        let can_wait = now + need_ms + reserve_ms < window_end - reserve_ms;
        if can_wait {
            return Ok(Precheck::Wait {
                detail: format!(
                    "{}，等用户离开后再开始；现在没有关闭客户端",
                    idle_reason(idle)
                ),
            });
        }
        return Ok(Precheck::Stop {
            state: STATE_SKIPPED,
            detail: format!(
                "{}，且窗口剩余时间不足以再等一轮，本轮跳过（没有改动客户端）",
                idle_reason(idle)
            ),
        });
    }

    // 到这里才需要"最初登录态"：它要关一次客户端，不能白关。
    let Some(origin) = capture_origin(deps)? else {
        return Ok(Precheck::Stop {
            state: STATE_SKIPPED,
            detail: "客户端当前没有登录，没有可恢复的「最初账号」，本轮不执行".to_string(),
        });
    };
    // 关客户端期间用户可能手动换了号：再读一次，变了就放弃。
    let after = client::read_live(&deps.env.auth_path);
    if after.uid.as_deref() != Some(origin.uid.as_str()) {
        return Ok(Precheck::Stop {
            state: STATE_SKIPPED,
            detail: "在读取原账号状态时客户端登录变了，本轮放弃（不改动任何登录态）".to_string(),
        });
    }

    app.with_db(|db| {
        db.set_rotation_origin(
            &run.id,
            &RotationOrigin {
                account_id: accounts
                    .iter()
                    .find(|row| row.uid == origin.uid)
                    .map(|row| row.id.as_str()),
                uid: Some(&origin.uid),
                snapshot_path: Some(&origin.path.to_string_lossy()),
                snapshot_sha256: Some(&origin.sha),
                marker: origin.marker,
                running: origin.running,
            },
        )
    })?;

    // 逐个登记步骤（顺序 = 用户给的顺序），cursor 指向第一个可切换的。
    let mut seq = 0i64;
    let mut cursor: Option<i64> = None;
    for account in &already_live {
        let step_id = uuid::Uuid::new_v4().to_string();
        app.with_db(|db| {
            db.begin_rotation_step(&step_id, &run.id, seq, &account.id, &account.uid, None)?;
            db.finish_rotation_step(
                &step_id,
                STEP_ALREADY_LIVE,
                Some("客户端当前登录的就是它"),
                None,
                Some(&ms_to_rfc3339(deps.clock.now_ms())),
            )
        })?;
        seq += 1;
    }
    for (account, _) in &plans {
        let step_id = uuid::Uuid::new_v4().to_string();
        app.with_db(|db| {
            db.begin_rotation_step(&step_id, &run.id, seq, &account.id, &account.uid, None)
        })?;
        if cursor.is_none() {
            cursor = Some(seq);
        }
        seq += 1;
    }
    for (account, reason, detail) in &skipped {
        let step_id = uuid::Uuid::new_v4().to_string();
        app.with_db(|db| {
            db.begin_rotation_step(&step_id, &run.id, seq, &account.id, &account.uid, None)?;
            db.finish_rotation_step(
                &step_id,
                STEP_SKIPPED,
                Some(detail),
                Some(reason),
                Some(&ms_to_rfc3339(deps.clock.now_ms())),
            )
        })?;
        seq += 1;
    }

    let Some(cursor) = cursor else {
        return Ok(Precheck::Stop {
            state: STATE_SKIPPED,
            detail: "没有可切换的账号".to_string(),
        });
    };

    app.with_db(|db| {
        db.set_rotation_cursor(&run.id, cursor)?;
        db.cas_rotation_state(&run.id, STATE_PRECHECKING, STATE_SWITCHING)
    })?;
    Ok(Precheck::Ready)
}

/// 关客户端 → 读认证文件 → 存快照 → 启动客户端。
///
/// 这是**唯一**允许用来拍"最初登录态"的路径：客户端退出时会把内存里的最新
/// 令牌写回文件，不关它就可能存下一份即将过期的旧令牌。
pub struct OriginSnapshot {
    pub uid: String,
    pub path: PathBuf,
    pub sha: String,
    pub marker: bool,
    pub running: bool,
}

fn capture_origin(deps: &RotationDeps<'_>) -> RpcResult<Option<OriginSnapshot>> {
    let running = deps.control.is_running();
    if running {
        deps.control.close(20).map_err(|err| {
            RpcError::internal(format!("关闭 WorkBuddy 失败，本轮未执行：{err}"))
                .with_data(json!({ "kind": "close_failed" }))
        })?;
    }
    let bytes = std::fs::read(&deps.env.auth_path).unwrap_or_default();
    let marker = client::logout_marker_path(&deps.env.auth_path).exists();
    if bytes.is_empty() || marker {
        if running {
            let _ = deps.control.launch();
        }
        return Ok(None);
    }
    let Some(root) = serde_json::from_slice::<Value>(&bytes).ok() else {
        if running {
            let _ = deps.control.launch();
        }
        return Ok(None);
    };
    let Some(uid) = client::uid_of(&root) else {
        if running {
            let _ = deps.control.launch();
        }
        return Ok(None);
    };
    let sha = client::sha256_hex(&bytes);
    let dir = deps.env.snapshot_dir.join("rotation");
    let path = dir.join(format!("origin-{}.json", uuid::Uuid::new_v4().simple()));
    client::write_private(&path, &bytes).map_err(|err| {
        if running {
            let _ = deps.control.launch();
        }
        RpcError::internal(format!("保存原账号快照失败，本轮未执行：{err}"))
    })?;
    if running {
        let _ = deps.control.launch();
    }
    Ok(Some(OriginSnapshot {
        uid,
        path,
        sha,
        marker,
        running,
    }))
}

enum StepGate {
    Continue,
    Wait { detail: String },
    Restore { detail: String },
    Halt { detail: String },
}

/// 空闲状态的说明。`None` 不是"空闲 0 秒"，是"不知道"。
fn idle_reason(idle: Option<i64>) -> String {
    match idle {
        Some(secs) => format!("检测到正在使用（空闲 {secs} 秒）"),
        None => "读不到系统空闲状态，无法确认你不在使用客户端".to_string(),
    }
}

/// 这一步处理完了：把 cursor 推到下一个，没有下一个就进入恢复。
///
/// **所有"跳过/失败"路径都必须经过它**，否则 tick 会一直停在同一个账号上
/// （同一个凭据被拒绝时，每分钟重试一次，整晚什么都不做）。
fn advance_cursor(app: &App, run: &RotationRun) -> RpcResult<StepGate> {
    let next = run.cursor + 1;
    let total = max_seq(app, &run.id)?;
    app.with_db(|db| db.set_rotation_cursor(&run.id, next))?;
    if next >= total {
        return Ok(StepGate::Restore {
            detail: "没有更多可切换的账号".to_string(),
        });
    }
    Ok(StepGate::Continue)
}

/// 切换 `cursor` 指向的账号。
async fn switch_current(
    app: &App,
    deps: &RotationDeps<'_>,
    plan: &RotationPlan,
    run: &RotationRun,
    now: i64,
) -> RpcResult<StepGate> {
    let Some(step) = app.with_db(|db| db.rotation_step_by_seq(&run.id, run.cursor))? else {
        return Ok(StepGate::Restore {
            detail: "找不到下一步，直接收尾".to_string(),
        });
    };

    // 崩溃在切换中途：先看客户端现在到底是谁，别盲目再切一次。
    if step.status == STEP_SWITCHING {
        return reconcile(app, deps, run, &step).await;
    }
    // 这一步已经有结论（跳过了 / 完成了）：不要重切，直接推进。
    if !matches!(step.status.as_str(), STEP_PLANNED) {
        return advance_cursor(app, run);
    }

    // 用户中途改了登录态：停下来，且不覆盖。
    if let Some(reason) = user_diverged(app, deps, run) {
        return Ok(StepGate::Halt { detail: reason });
    }

    let live = client::read_live(&deps.env.auth_path);
    if live.uid.as_deref() == Some(step.uid.as_str()) {
        // 目标已经是当前登录（例如上一跳失败后客户端回到这里）。
        app.with_db(|db| {
            db.finish_rotation_step(
                &step.id,
                STEP_ALREADY_LIVE,
                Some("客户端已在这个账号"),
                None,
                Some(&ms_to_rfc3339(deps.clock.now_ms())),
            )
        })?;
        return advance_cursor(app, run);
    }

    // **窗口检查独立于空闲检查**。
    //
    // 等待用户、Mac 休眠、进程重启都会让 `now` 往前跳；如果只在"用户还忙"的
    // 分支里看窗口，那么"忙 → 等 → 用户走开但已经超窗"这条路会直接切过去。
    // 所以每次准备开始新的一跳，都用当前时间重新判断一次。
    if near_window_end(run, plan, now) {
        return Ok(StepGate::Restore {
            detail: "窗口剩余时间不足以再停留一次，不再开始新的切换，直接恢复原账号".to_string(),
        });
    }

    // 用户正在用（或读不到空闲状态）：推迟，不突然关客户端。
    let idle = deps.idle.idle_secs();
    if idle.map_or(true, |secs| secs < USER_ACTIVE_IDLE_SECS) {
        return Ok(StepGate::Wait {
            detail: format!("{}，这一跳推迟", idle_reason(idle)),
        });
    }

    let Some(account) = app.with_db(|db| db.get_account(&step.account_id))? else {
        app.with_db(|db| {
            db.finish_rotation_step(
                &step.id,
                STEP_SKIPPED,
                Some("账号已不存在"),
                Some("no_account"),
                Some(&ms_to_rfc3339(deps.clock.now_ms())),
            )
        })?;
        return advance_cursor(app, run);
    };

    // 真正切换前再判一次：预检与执行之间凭据可能已经失效。
    let outcome = plan::resolve(app, &account, deps.control.installed(), live.uid.as_deref());
    let switch_plan = match outcome {
        PlanOutcome::Ready(plan) => plan,
        PlanOutcome::Blocked { reason, detail } => {
            app.with_db(|db| {
                db.finish_rotation_step(
                    &step.id,
                    STEP_SKIPPED,
                    Some(&detail),
                    Some(reason),
                    Some(&ms_to_rfc3339(deps.clock.now_ms())),
                )
            })?;
            return advance_cursor(app, run);
        }
    };

    // 自有长期授权：真的发一次只读探针。**"库里有 refresh_ref" 不是有效性证据。**
    let mut precheck_outcome = None;
    if switch_plan.mode == plan::Mode::OwnRefresh {
        match deps.precheck.check(app, &account).await {
            Ok(state) => {
                precheck_outcome = Some(state.clone());
                if state == "rejected" {
                    let detail =
                        "自有授权已被服务端拒绝（401），跳过这个账号；请在 AICC 里重新授权";
                    app.with_db(|db| {
                        db.finish_rotation_step(
                            &step.id,
                            STEP_SKIPPED,
                            Some(detail),
                            Some("own_grant_rejected"),
                            Some(&ms_to_rfc3339(deps.clock.now_ms())),
                        )
                    })?;
                    return advance_cursor(app, run);
                }
            }
            Err(err) => {
                // 读不到令牌 = 这份授权不可用，跳过（不是"网络抖动"）。
                app.with_db(|db| {
                    db.finish_rotation_step(
                        &step.id,
                        STEP_SKIPPED,
                        Some(&err.message),
                        Some("own_grant_unreadable"),
                        Some(&ms_to_rfc3339(deps.clock.now_ms())),
                    )
                })?;
                return advance_cursor(app, run);
            }
        }
    }

    // Magpie 临时授权：剩余有效期必须覆盖"停留 + 恢复余量"。
    if switch_plan.mode == plan::Mode::MagpieTemporary {
        let need = plan.dwell_seconds + RESTORE_RESERVE_SECS;
        if switch_plan.remaining_secs.unwrap_or(0) < need {
            let detail = format!(
                "Magpie 访问令牌只剩 {} 分钟，不够覆盖 {} 分钟停留 + 恢复余量，跳过",
                switch_plan.remaining_secs.unwrap_or(0) / 60,
                plan.dwell_seconds / 60
            );
            app.with_db(|db| {
                db.finish_rotation_step(
                    &step.id,
                    STEP_SKIPPED,
                    Some(&detail),
                    Some("magpie_too_short_for_dwell"),
                    Some(&ms_to_rfc3339(deps.clock.now_ms())),
                )
            })?;
            return advance_cursor(app, run);
        }
    }

    let activity_before = deps.activity.snapshot(app, &account.id).await;

    // 预检是网络等待。期间用户可能已经按了暂停：意图已落库，就别再开这一跳。
    let stopped = app
        .with_db(|db| db.rotation_run(&run.id))?
        .map(|latest| latest.stop_requested)
        .unwrap_or(false);
    if stopped {
        app.with_db(|db| {
            db.finish_rotation_step(
                &step.id,
                STEP_SKIPPED,
                Some("已请求停止，这一跳没有执行"),
                Some("stop_requested"),
                Some(&ms_to_rfc3339(deps.clock.now_ms())),
            )
        })?;
        return Ok(StepGate::Restore {
            detail: "收到停止请求，不再开始新的切换，直接恢复原账号".to_string(),
        });
    }

    // 预检和活跃查询都可能等网络（或让 Mac 在等待中睡过去）。等完之后 `now`
    // 已经不是入口那个值了，所以真正动客户端之前用**当下**时间再确认一次窗口。
    let now = deps.clock.now_ms();
    if near_window_end(run, plan, now) {
        app.with_db(|db| {
            db.finish_rotation_step(
                &step.id,
                STEP_SKIPPED,
                Some("等待预检/活跃查询后窗口已不足，未切换"),
                Some("window_elapsed_during_wait"),
                Some(&ms_to_rfc3339(now)),
            )
        })?;
        return Ok(StepGate::Restore {
            detail: "等待期间窗口已经结束，不再切换，直接恢复原账号".to_string(),
        });
    }

    app.with_db(|db| {
        db.set_rotation_step_activity(&step.id, Some(&activity_before), None)?;
        db.start_rotation_step(
            &step.id,
            switch_plan.mode.as_str(),
            None,
            precheck_outcome.as_deref(),
        )
    })?;

    // 第一步的快照就是"本轮最初登录态"：由 service::apply 在关客户端后拍。
    let link = RotationSwitchLink {
        run_id: &run.id,
        account_id: &account.id,
        uid: &account.uid,
        is_first: run.origin_snapshot_path.is_none(),
    };
    let request = service::SwitchRequest {
        account_id: account.id.clone(),
        restart: true,
        observe_activity: false,
        // 轮换**不重复发服务端探针**：切换前已经做过一次只读预检（`OwnPrecheck`），
        // 停留结束时还有一次活跃回查。一晚切六个账号，每跳再探一次只是把请求翻倍。
        // 三个结论仍然分开记：step 状态（切换）、authPrecheck（授权）、activityAfter（活跃）。
        verify_server: false,
    };

    match service::switch_linked(app, &request, deps.control, deps.env, Some(&link)).await {
        Ok(result) => {
            let journal_id = result["journalId"].as_str().unwrap_or_default().to_string();
            let phase = result["phase"].as_str().unwrap_or("verified");
            let status = if phase == "unconfirmed" {
                STEP_DWELLING
            } else {
                STEP_DONE
            };
            app.with_db(|db| {
                db.finish_rotation_step(
                    &step.id,
                    status,
                    Some(&format!(
                        "mode={}，phase={}，authAccepted={}",
                        switch_plan.mode.as_str(),
                        phase,
                        result["authAccepted"].as_str().unwrap_or("unknown")
                    )),
                    None,
                    Some(&ms_to_rfc3339(deps.clock.now_ms())),
                )?;
                db.set_rotation_last_target(&run.id, &account.id, &account.uid)
            })?;
            let _ = journal_id;
            // 切换成功 → 进入停留，下一次 tick 计算何时该切下一个。
            app.with_db(|db| db.cas_rotation_state(&run.id, STATE_SWITCHING, STATE_DWELLING))?;
            Ok(StepGate::Continue)
        }
        Err(err) => {
            let restored = err
                .data
                .as_ref()
                .and_then(|data| data.get("restored"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            app.with_db(|db| {
                db.finish_rotation_step(
                    &step.id,
                    STEP_FAILED,
                    Some(&crate::rpc::sanitize(&err.message)),
                    Some("switch_failed"),
                    Some(&ms_to_rfc3339(deps.clock.now_ms())),
                )
            })?;
            if !restored {
                // 切换失败且没回滚成功：先尽力用本轮最初快照恢复。
                let detail = format!("切换失败且未能回滚：{}", err.message);
                return Ok(StepGate::Restore { detail });
            }
            advance_cursor(app, run)
        }
    }
}

fn max_seq(app: &App, run_id: &str) -> RpcResult<i64> {
    let steps = app.with_db(|db| db.rotation_steps(run_id))?;
    Ok(steps
        .iter()
        .map(|step| step.seq)
        .max()
        .map(|seq| seq + 1)
        .unwrap_or(0))
}

/// 崩溃后处于 `switching` 的步骤：按客户端**现在**是谁来决定怎么收。
async fn reconcile(
    app: &App,
    deps: &RotationDeps<'_>,
    run: &RotationRun,
    step: &RotationStep,
) -> RpcResult<StepGate> {
    let live = client::read_live(&deps.env.auth_path);
    if live.uid.as_deref() == Some(step.uid.as_str()) {
        app.with_db(|db| {
            db.finish_rotation_step(
                &step.id,
                STEP_DWELLING,
                Some("进程重启后确认客户端已停在目标账号"),
                None,
                Some(&ms_to_rfc3339(deps.clock.now_ms())),
            )?;
            db.set_rotation_last_target(&run.id, &step.account_id, &step.uid)
        })?;
        app.with_db(|db| db.cas_rotation_state(&run.id, STATE_SWITCHING, STATE_DWELLING))?;
        return Ok(StepGate::Continue);
    }
    if run.origin_uid.as_deref() == live.uid.as_deref() {
        // 没切成功，客户端还在原账号：把这一步记成失败，继续下一个。
        app.with_db(|db| {
            db.finish_rotation_step(
                &step.id,
                STEP_FAILED,
                Some("切换未完成，客户端仍是最初账号"),
                Some("interrupted"),
                Some(&ms_to_rfc3339(deps.clock.now_ms())),
            )
        })?;
        return advance_cursor(app, run);
    }
    Ok(StepGate::Restore {
        detail: "切换中途中断，且客户端登录的既不是原账号也不是目标账号".to_string(),
    })
}

async fn dwelling(
    app: &App,
    deps: &RotationDeps<'_>,
    plan: &RotationPlan,
    run: &RotationRun,
    now: i64,
    depth: u32,
) -> RpcResult<Value> {
    // 用户中途自己换了账号：立即停止，不覆盖。
    if let Some(reason) = user_diverged(app, deps, run) {
        app.with_db(|db| {
            db.set_rotation_state(&run.id, STATE_COMPLETED, Some(&reason))?;
            let _ = db.settle_all_pending_temporary();
            Ok::<(), RpcError>(())
        })?;
        return Ok(json!({ "status": STATE_COMPLETED, "runId": run.id, "detail": reason }));
    }

    let steps = app.with_db(|db| db.rotation_steps(&run.id))?;
    let current = steps.iter().find(|step| step.seq == run.cursor);
    let next_seq = steps
        .iter()
        .filter(|step| step.seq > run.cursor && step.status == STEP_PLANNED)
        .map(|step| step.seq)
        .min();

    let window_end = parse_rfc3339_ms(&run.window_end).unwrap_or(now);
    let reserve_ms = RESTORE_RESERVE_SECS * 1000;
    let dwell_until = current
        .and_then(|step| step.ended_at.as_deref())
        .and_then(parse_rfc3339_ms)
        .map(|ended| ended + plan.dwell_seconds * 1000)
        .unwrap_or(now + plan.dwell_seconds * 1000);

    // 到没到点、要不要因为窗口而提前离开。
    let dwell_over = now >= dwell_until;
    let must_leave = now + reserve_ms >= window_end;

    if !dwell_over && !must_leave {
        return Ok(json!({
            "status": STATE_DWELLING,
            "runId": run.id,
            "nextSwitchAt": ms_to_rfc3339(dwell_until),
        }));
    }

    // **停留真正结束**（或被窗口逼着走）时才查这一步的活跃。
    //
    // 刚切完就查等于自问自答：服务端那时还没记上这次登录，拿到的一定是旧值，
    // 用户第二天看到的"今日活跃"就永远是假的。
    if let Some(step) = current {
        if step.activity_after.is_none() {
            let after = deps.activity.snapshot(app, &step.account_id).await;
            let _ = app.with_db(|db| db.set_rotation_step_activity(&step.id, None, Some(&after)));
        }
    }

    let can_switch_another = next_seq.is_some()
        && !must_leave
        && now + plan.dwell_seconds * 1000 + reserve_ms < window_end;

    if let Some(next) = next_seq {
        if can_switch_another {
            app.with_db(|db| {
                db.set_rotation_cursor(&run.id, next)?;
                db.cas_rotation_state(&run.id, STATE_DWELLING, STATE_SWITCHING)
            })?;
            let run = reload(app, &run.id)?;
            return Box::pin(advance_with_depth(app, deps, plan, run, now, depth + 1)).await;
        }
    }

    // 窗口到头 / 没有下一个：进入恢复。
    app.with_db(|db| db.cas_rotation_state(&run.id, STATE_DWELLING, STATE_RESTORING))?;
    let run = reload(app, &run.id)?;
    Box::pin(advance_with_depth(app, deps, plan, run, now, depth + 1)).await
}

fn near_window_end(run: &RotationRun, plan: &RotationPlan, now: i64) -> bool {
    let window_end = parse_rfc3339_ms(&run.window_end).unwrap_or(now);
    now + plan.dwell_seconds * 1000 >= window_end - RESTORE_RESERVE_SECS * 1000
}

/// 用户是否已经把这个登录态改成了"不是我们造成的"。
///
/// - 回到最初账号 → 用户自己收尾了，停止计划（不需要再恢复）。
/// - 换到第三个账号 → 停止计划，且**不覆盖**。
/// - 客户端登出（`uid = None`）不算用户干预：Magpie access 过期正是要救的场景。
fn user_diverged(app: &App, deps: &RotationDeps<'_>, run: &RotationRun) -> Option<String> {
    let live = client::read_live(&deps.env.auth_path);
    let Some(current) = live.uid.as_deref() else {
        return None;
    };
    if live.logout_marker {
        return None;
    }
    // 这一轮还没成功切过任何账号：当前登录本来就该是最初账号，
    // 不能把它判成"用户改回来了"——否则第一次 tick 就会自我取消。
    let Some(last) = run.last_target_uid.as_deref() else {
        return None;
    };
    if Some(current) == Some(last) {
        return None;
    }
    if Some(current) == run.origin_uid.as_deref() {
        return Some("用户已经自己切回最初的账号，计划停止".to_string());
    }
    let _ = app;
    Some(format!(
        "检测到客户端登录被手动改成了另一个账号（{}），计划已停止且不会覆盖它",
        current.chars().take(8).collect::<String>()
    ))
}

pub struct RestoreOutcome {
    pub ok: bool,
    pub code: String,
    pub detail: String,
}

/// 回到这一轮最初的那个账号。
///
/// **不依赖** `switch_journal`：只认 `run.origin_*`。所以中途切了几次、
/// 那些 journal 怎么结算，都不影响这里。
pub fn restore_origin(
    app: &App,
    deps: &RotationDeps<'_>,
    run: &RotationRun,
    force: bool,
) -> RpcResult<RestoreOutcome> {
    let Some(snapshot_path) = run.origin_snapshot_path.as_deref() else {
        return Ok(RestoreOutcome {
            ok: true,
            code: "no_origin".to_string(),
            detail: "这一轮没有改动过登录态，不需要恢复".to_string(),
        });
    };
    let live = client::read_live(&deps.env.auth_path);
    if matches!(
        run.restore_result.as_deref(),
        Some("restored" | "already_origin")
    ) && live.uid.as_deref() == run.origin_uid.as_deref()
        && !live.logout_marker
    {
        let _ = app.with_db(|db| db.settle_all_pending_temporary());
        service::prune_settled_snapshots(app, deps.env, "");
        return Ok(RestoreOutcome {
            ok: true,
            code: "already_origin".to_string(),
            detail: "客户端已经是最初的账号".to_string(),
        });
    }
    let Some(expected_sha) = run.origin_snapshot_sha256.as_deref() else {
        return Ok(RestoreOutcome {
            ok: false,
            code: "snapshot_untracked".to_string(),
            detail: "原账号快照没有完整性记录，拒绝用它覆盖客户端".to_string(),
        });
    };
    let bytes = match std::fs::read(Path::new(snapshot_path)) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Ok(RestoreOutcome {
                ok: false,
                code: "snapshot_missing".to_string(),
                detail: format!("原账号快照读不到（{err}），请手动在客户端登录"),
            })
        }
    };
    if client::sha256_hex(&bytes) != expected_sha {
        return Ok(RestoreOutcome {
            ok: false,
            code: "snapshot_corrupt".to_string(),
            detail: "原账号快照与记录不一致，拒绝覆盖客户端登录态".to_string(),
        });
    }

    let live = client::read_live(&deps.env.auth_path);
    if live.uid.as_deref() == run.origin_uid.as_deref() && !live.logout_marker {
        let _ = app.with_db(|db| db.settle_all_pending_temporary());
        let _ = std::fs::remove_file(Path::new(snapshot_path));
        service::prune_settled_snapshots(app, deps.env, "");
        return Ok(RestoreOutcome {
            ok: true,
            code: "already_origin".to_string(),
            detail: "客户端已经是最初的账号".to_string(),
        });
    }
    // 用户手动换成别的账号：默认不动。`force` 只在这一条上生效。
    if !force && live.logged_in() && live.uid.as_deref() != run.last_target_uid.as_deref() {
        return Ok(RestoreOutcome {
            ok: false,
            code: "live_changed".to_string(),
            detail: "客户端当前登录的不是本轮的目标账号，可能是你手动换的；未覆盖".to_string(),
        });
    }

    let running = deps.control.is_running();
    if running {
        deps.control.close(20).map_err(|err| {
            RpcError::internal(format!("关闭 WorkBuddy 失败，未恢复任何文件：{err}"))
        })?;
    }
    client::write_private(&deps.env.auth_path, &bytes)
        .map_err(|err| RpcError::internal(format!("写回原账号快照失败：{err}")))?;
    apply_marker(&deps.env.auth_path, run.origin_marker);
    if run.origin_running {
        let _ = deps.control.launch();
    }

    let after = client::read_live(&deps.env.auth_path);
    let ok = after.uid.as_deref() == run.origin_uid.as_deref() && !after.logout_marker;
    if ok {
        let _ = app.with_db(|db| db.settle_all_pending_temporary());
        let _ = std::fs::remove_file(Path::new(snapshot_path));
        service::prune_settled_snapshots(app, deps.env, "");
    }
    Ok(RestoreOutcome {
        ok,
        code: if ok {
            "restored".to_string()
        } else {
            "restore_failed".to_string()
        },
        detail: if ok {
            "已恢复到这一轮最初登录的账号".to_string()
        } else {
            format!(
                "已写回快照，但客户端当前登录是 {}；请手动确认",
                after.uid.as_deref().unwrap_or("读不到")
            )
        },
    })
}

fn apply_marker(auth_path: &Path, present: bool) {
    let marker = client::logout_marker_path(auth_path);
    if present {
        let _ = std::fs::write(&marker, "logged-out");
    } else {
        match std::fs::remove_file(&marker) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => eprintln!("[rotation] 清理退出标记失败：{err}"),
        }
    }
}

// ---------------------------------------------------------------------------
// 查询与操作
// ---------------------------------------------------------------------------

/// 暂停/结束当前这一轮：**先尽力把原账号恢复回来**，再落终态。
pub fn stop(app: &App, control: &dyn ClientControl, env: &SwitchEnv) -> RpcResult<Value> {
    let deps = RotationDeps {
        control,
        env,
        clock: &SystemClock,
        idle: &SystemIdle,
        precheck: &SystemPrecheck,
        activity: &SystemActivity,
    };
    // 与 tick 抢**同一把跨进程锁**：不能一边恢复一边被另一个进程切走。
    let Some(run) = app.with_db(|db| db.active_rotation_run())? else {
        return Ok(json!({ "ok": true, "status": "nothing_to_stop" }));
    };
    // 先落库，再抢锁。反过来的话，锁被别人占着时用户的停止意图就丢了，
    // 下一次心跳会认为这一轮没人要停，继续切下一个账号。
    app.with_db(|db| db.request_rotation_stop(&run.id))?;
    let Some(_gate) = acquire_gate(&deps)? else {
        return Ok(json!({
            "ok": false,
            "status": "stop_requested",
            "runId": run.id,
            "detail": "已请求暂停；当前操作结束后会自动恢复原账号",
        }));
    };
    if run.origin_snapshot_path.is_none() {
        app.with_db(|db| {
            db.set_rotation_state(&run.id, STATE_COMPLETED, Some("已取消（尚未改动登录态）"))
        })?;
        return Ok(
            json!({ "ok": true, "status": STATE_COMPLETED, "runId": run.id, "restored": true }),
        );
    }
    let outcome = restore_origin(app, &deps, &run, false)?;
    app.with_db(|db| {
        db.note_rotation_restore(&run.id, run.restore_attempts + 1, &outcome.code)?;
        db.set_rotation_state(
            &run.id,
            if outcome.ok {
                STATE_COMPLETED
            } else {
                STATE_RECOVERY_REQUIRED
            },
            Some(&outcome.detail),
        )
    })?;
    Ok(json!({
        "ok": outcome.ok,
        "status": if outcome.ok { STATE_COMPLETED } else { STATE_RECOVERY_REQUIRED },
        "runId": run.id,
        "restored": outcome.ok,
        "detail": outcome.detail,
    }))
}

/// 手动「恢复原账号」。`run_id` 为空时取最近一轮。
pub fn restore_now(
    app: &App,
    control: &dyn ClientControl,
    env: &SwitchEnv,
    run_id: Option<&str>,
    force: bool,
) -> RpcResult<Value> {
    let deps = RotationDeps {
        control,
        env,
        clock: &SystemClock,
        idle: &SystemIdle,
        precheck: &SystemPrecheck,
        activity: &SystemActivity,
    };
    let Some(_gate) = acquire_gate(&deps)? else {
        return Ok(json!({ "ok": false, "status": "busy", "detail": "另一项轮换操作正在执行" }));
    };
    let run = match run_id {
        Some(id) => app.with_db(|db| db.rotation_run(id))?,
        None => app.with_db(|db| db.latest_rotation_run())?,
    };
    let Some(run) = run else {
        return Ok(json!({
            "ok": true,
            "restored": false,
            "reason": "nothing_to_restore",
            "detail": "还没有可恢复的轮换记录",
        }));
    };
    let outcome = restore_origin(app, &deps, &run, force)?;
    app.with_db(|db| {
        db.note_rotation_restore(&run.id, run.restore_attempts + 1, &outcome.code)?;
        if outcome.ok {
            // `recovery_required` 的含义是"等人处理"，不是"永远结束"。
            // 人工把故障排除后恢复成功，就必须收口成 completed，否则状态页
            // 会一直挂着"需要你确认"，历史记录里也永远看不到这次恢复。
            let reopenable = run.state == STATE_RECOVERY_REQUIRED || !run.is_terminal();
            if reopenable {
                db.set_rotation_state(&run.id, STATE_COMPLETED, Some(&outcome.detail))?;
            }
        } else {
            db.set_rotation_state(&run.id, STATE_RECOVERY_REQUIRED, Some(&outcome.detail))?;
        }
        Ok::<(), RpcError>(())
    })?;
    Ok(json!({
        "ok": outcome.ok,
        "restored": outcome.ok,
        "runId": run.id,
        "code": outcome.code,
        "detail": outcome.detail,
        "originUid": run.origin_uid,
    }))
}

/// 计划 + 当前轮次 + 最近记录。界面只读这一份。
pub fn status(
    app: &App,
    control: &dyn ClientControl,
    env: &SwitchEnv,
    clock: &dyn Clock,
) -> RpcResult<Value> {
    let plan = app.with_db(|db| db.rotation_plan_or_default())?;
    let now = clock.now_ms();
    let active = app.with_db(|db| db.active_rotation_run())?;
    let latest = app.with_db(|db| db.latest_rotation_run())?;
    let history = app.with_db(|db| db.rotation_runs(5))?;
    let live = client::read_live(&env.auth_path);

    let schedule = match (active_window(&plan, now), next_window(&plan, now)) {
        (Some(window), _) => json!({
            "inWindow": true,
            "windowStart": ms_to_rfc3339(window.start_ms),
            "windowEnd": ms_to_rfc3339(window.end_ms),
            "nextRunAt": Value::Null,
        }),
        (None, Some(window)) => json!({
            "inWindow": false,
            "windowStart": Value::Null,
            "windowEnd": Value::Null,
            "nextRunAt": ms_to_rfc3339(window.start_ms),
            "nextRunEndAt": ms_to_rfc3339(window.end_ms),
        }),
        (None, None) => json!({ "inWindow": false, "nextRunAt": Value::Null }),
    };

    let mut history_view = Vec::new();
    for run in &history {
        let steps = app.with_db(|db| db.rotation_steps(&run.id))?;
        history_view.push(run_view(run, &steps, &plan));
    }

    Ok(json!({
        "ok": true,
        "plan": {
            "id": plan.id,
            "enabled": plan.enabled,
            "timezone": plan.timezone,
            "weekdays": plan.weekdays,
            "startMinutes": plan.start_minutes,
            "endMinutes": plan.end_minutes,
            "accountIds": plan.account_ids,
            "dwellSeconds": plan.dwell_seconds,
            "startText": clock_text(plan.start_minutes),
            "endText": clock_text(plan.end_minutes),
        },
        "schedule": schedule,
        "live": {
            "uid": live.uid,
            "loggedIn": live.logged_in(),
            "temporary": Value::Null,
        },
        "client": {
            "installed": control.installed(),
            "running": control.installed().then(|| control.is_running()),
        },
        "current": active
            .as_ref()
            .map(|run| run_view(
                run,
                &app.with_db(|db| db.rotation_steps(&run.id)).unwrap_or_default(),
                &plan,
            )),
        "lastRun": latest.as_ref().map(|run| run_view(
            run,
            &app.with_db(|db| db.rotation_steps(&run.id)).unwrap_or_default(),
            &plan,
        )),
        "history": history_view,
        "scheduler": {
            // 这句话必须和实际行为一致：没有 launchd，AICC 退出就停。
            "owner": "AICC.app 的账号助手进程",
            "runsWhileAppClosed": false,
            "tickSeconds": 60,
        },
        "limits": {
            "lateStartToleranceSeconds": LATE_START_TOLERANCE_SECS,
            "restoreReserveSeconds": RESTORE_RESERVE_SECS,
            "userActiveIdleSeconds": USER_ACTIVE_IDLE_SECS,
            "supportedTimezones": SUPPORTED_TIMEZONES,
        },
    }))
}

fn run_view(run: &RotationRun, steps: &[RotationStep], plan: &RotationPlan) -> Value {
    let current = steps.iter().find(|step| step.seq == run.cursor);
    // 停留中时"下一次切换"是用户最想知道的：第一个账号切过去之后，
    // 界面不能只写"停留中"，得说清下一次几点动。
    let next_switch_at = if run.state == STATE_DWELLING {
        let has_next = steps
            .iter()
            .any(|step| step.seq > run.cursor && step.status == STEP_PLANNED);
        if has_next {
            current
                .and_then(|step| step.ended_at.as_deref())
                .and_then(parse_rfc3339_ms)
                .map(|ended| ms_to_rfc3339(ended + plan.dwell_seconds * 1000))
        } else {
            None
        }
    } else {
        None
    };
    json!({
        "id": run.id,
        "planDate": run.plan_date,
        "timezone": run.timezone,
        "trigger": run.trigger,
        "windowStart": run.window_start,
        "windowEnd": run.window_end,
        "state": run.state,
        "cursor": run.cursor,
        "originUid": run.origin_uid,
        "originAccountId": run.origin_account_id,
        "originSnapshotTracked": run.origin_snapshot_sha256.is_some(),
        "lastTargetUid": run.last_target_uid,
        "lastTargetAccountId": run.last_target_account_id,
        "restoreAttempts": run.restore_attempts,
        "restoreResult": run.restore_result,
        "detail": run.detail,
        "currentAccountId": current.map(|step| step.account_id.clone()),
        "currentUid": current.map(|step| step.uid.clone()),
        "nextSwitchAt": next_switch_at,
        "steps": steps.iter().map(step_view).collect::<Vec<_>>(),
        "createdAt": run.created_at,
        "updatedAt": run.updated_at,
    })
}

fn step_view(step: &RotationStep) -> Value {
    json!({
        "id": step.id,
        "seq": step.seq,
        "accountId": step.account_id,
        "uid": step.uid,
        "mode": step.mode,
        "status": step.status,
        "startedAt": step.started_at,
        "endedAt": step.ended_at,
        "skipReason": step.skip_reason,
        "detail": step.detail,
        "authPrecheck": step.auth_precheck,
        "activityBefore": step.activity_before,
        "activityAfter": step.activity_after,
    })
}

fn clock_text(minutes: i64) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// dry-run：只演算，不写库、不动客户端、不读任何令牌。
pub fn dry_run(
    app: &App,
    control: &dyn ClientControl,
    env: &SwitchEnv,
    clock: &dyn Clock,
) -> RpcResult<Value> {
    let plan = app.with_db(|db| db.rotation_plan_or_default())?;
    let now = clock.now_ms();
    if let Err(why) = validate_plan(&plan) {
        return Ok(json!({ "ok": false, "error": why }));
    }
    let accounts = app.with_db(|db| db.list_accounts(&Default::default()))?;
    let live = client::read_live(&env.auth_path);
    let installed = control.installed();

    let mut steps = Vec::new();
    let mut cursor_ms = active_window(&plan, now)
        .map(|window| now.max(window.start_ms))
        .or_else(|| next_window(&plan, now).map(|window| window.start_ms))
        .unwrap_or(now);

    for (index, account_id) in plan.account_ids.iter().enumerate() {
        let account = accounts.iter().find(|row| &row.id == account_id);
        let entry = match account {
            None => json!({
                "accountId": account_id,
                "nickname": Value::Null,
                "outcome": "skip",
                "reason": "account_missing",
                "detail": "账号已不在列表里",
                "plannedAt": ms_to_rfc3339(cursor_ms),
            }),
            Some(account) => match plan::resolve(app, account, installed, live.uid.as_deref()) {
                PlanOutcome::Ready(ready) => {
                    // 临时的 Magpie 授权必须能覆盖"停留 + 恢复余量"，否则演算结果就是跳过。
                    let too_short = ready.temporary()
                        && ready
                            .remaining_secs
                            .map(|left| left < dwell_need(&plan))
                            .unwrap_or(true);
                    json!({
                        "accountId": account.id,
                        "nickname": account.nickname,
                        "uid": account.uid,
                        "outcome": if too_short { "skip" } else { "switch" },
                        "reason": if too_short { Value::String("magpie_too_short_for_dwell".into()) } else { Value::Null },
                        "mode": ready.mode.as_str(),
                        "temporary": ready.temporary(),
                        "remainingSeconds": ready.remaining_secs,
                        "accessExpiresAt": ready.access_expires_at_ms.map(ms_to_rfc3339),
                        "plannedAt": ms_to_rfc3339(cursor_ms),
                    })
                }
                PlanOutcome::Blocked { reason, detail } => {
                    let outcome = if reason == plan::reason::LIVE {
                        "already_live"
                    } else {
                        "skip"
                    };
                    json!({
                        "accountId": account.id,
                        "nickname": account.nickname,
                        "uid": account.uid,
                        "outcome": outcome,
                        "reason": reason,
                        "detail": detail,
                        "plannedAt": ms_to_rfc3339(cursor_ms),
                    })
                }
            },
        };
        steps.push(entry);
        if index + 1 < plan.account_ids.len() {
            cursor_ms += plan.dwell_seconds * 1000;
        }
    }

    Ok(json!({
        "ok": true,
        "dryRun": true,
        "planDate": active_window(&plan, now)
            .map(|window| window.date.format("%Y-%m-%d").to_string())
            .or_else(|| next_window(&plan, now).map(|window| window.date.format("%Y-%m-%d").to_string())),
        "startAt": active_window(&plan, now)
            .map(|window| ms_to_rfc3339(now.max(window.start_ms)))
            .or_else(|| next_window(&plan, now).map(|window| ms_to_rfc3339(window.start_ms))),
        "endAt": active_window(&plan, now)
            .map(|window| ms_to_rfc3339(window.end_ms))
            .or_else(|| next_window(&plan, now).map(|window| ms_to_rfc3339(window.end_ms))),
        "originUid": live.uid,
        "originLoggedIn": live.logged_in(),
        "steps": steps,
        "warnings": dry_run_warnings(&plan, &live),
    }))
}

/// 临时的 Magpie 授权必须覆盖"停留 + 恢复余量"，否则不该切。
fn dwell_need(plan: &RotationPlan) -> i64 {
    plan.dwell_seconds + RESTORE_RESERVE_SECS
}

fn dry_run_warnings(plan: &RotationPlan, live: &client::LiveFile) -> Vec<String> {
    let mut warnings = Vec::new();
    if !live.logged_in() {
        warnings.push("客户端当前没有登录，本轮不会执行（没有可恢复的最初账号）".to_string());
    }
    if plan.account_ids.is_empty() {
        warnings.push("还没有选择参与轮换的账号".to_string());
    }
    if tz_offset(&plan.timezone).is_none() {
        warnings.push(format!("时区不受支持：{}", plan.timezone));
    }
    warnings.push(format!(
        "每切一个账号都会关闭并重启 WorkBuddy；本轮最多重启 {} 次（含恢复）",
        plan.account_ids.len() + 1
    ));
    warnings
}

#[cfg(test)]
mod idle_tests {
    use super::*;
    use std::cell::Cell;

    /// 正常值：纳秒 → 秒。
    #[test]
    fn parses_idle_nanoseconds_into_seconds() {
        let text = "    | | |   \"HIDIdleTime\" = 153997320083\n";
        assert_eq!(parse_idle(text), Some(153));
    }

    /// 0 是**真实值**（刚刚动过），不是"读不到"。
    #[test]
    fn zero_idle_time_is_a_value() {
        assert_eq!(parse_idle("\"HIDIdleTime\" = 0"), Some(0));
    }

    /// 超过 120 秒阈值的那一侧照原样返回，阈值本身不在这里判。
    #[test]
    fn long_idle_time_is_returned_unchanged() {
        assert_eq!(parse_idle("\"HIDIdleTime\" = 3600000000000"), Some(3600));
        assert_eq!(parse_idle("\"HIDIdleTime\" = 126000000000"), Some(126));
    }

    /// 属性缺失：返回 None，绝不猜成"空闲 0 秒"。
    #[test]
    fn missing_property_is_unknown() {
        assert_eq!(parse_idle("+-o Root  <class IORegistryEntry>\n"), None);
        assert_eq!(parse_idle(""), None);
    }

    /// 格式不对（非数字 / 被截断）：同样是 None。
    #[test]
    fn malformed_value_is_unknown() {
        assert_eq!(parse_idle("\"HIDIdleTime\" = abc"), None);
        assert_eq!(parse_idle("\"HIDIdleTime\" ="), None);
        assert_eq!(parse_idle("\"HIDIdleTime\""), None);
    }

    /// 窄查询拿不到时回落到全量 dump（本机 macOS 27 的真实情况）。
    #[test]
    fn falls_back_to_the_full_dump() {
        let calls = Cell::new(0);
        let idle = idle_secs_with(|args| {
            calls.set(calls.get() + 1);
            if args == ["-c", "IOHIDSystem", "-d", "1"] {
                None // macOS 26/27：只回 Root
            } else {
                Some(0)
            }
        });
        assert_eq!(idle, Some(0));
        assert_eq!(calls.get(), 2, "第二段必须真的被调用");
    }

    /// 窄查询成功就不再多跑一次全量 dump。
    #[test]
    fn narrow_query_short_circuits() {
        let calls = Cell::new(0);
        let idle = idle_secs_with(|_| {
            calls.set(calls.get() + 1);
            Some(150)
        });
        assert_eq!(idle, Some(150));
        assert_eq!(calls.get(), 1);
    }

    /// 两段都失败：None。调用方据此保守推迟，而不是当成"用户不在用"。
    #[test]
    fn both_probes_failing_stays_unknown() {
        assert_eq!(idle_secs_with(|_| None), None);
    }

    /// 真机只读检查（CI 不跑，Linux 上没有 ioreg）：
    ///
    /// ```text
    /// cargo test --offline real_ioreg -- --ignored --nocapture
    /// ```
    ///
    /// 只断言"能读到一个数"，不判阈值 —— 空闲秒数随机器状态变化，写死会变成
    /// 又一个定时炸弹。
    #[test]
    #[ignore = "读真实 ioreg：只在本机手动执行"]
    fn real_ioreg_reports_idle_seconds() {
        let idle = SystemIdle
            .idle_secs()
            .expect("macOS 上两段式探测必须读到 HIDIdleTime");
        assert!(idle >= 0, "空闲秒数不应为负：{idle}");
        println!("HIDIdleTime -> {idle}s");
    }
}
