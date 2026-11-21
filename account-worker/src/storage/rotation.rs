//! 夜间账号轮换的计划、轮次与步骤（v27）。
//!
//! 三张表，三种生命周期：
//!
//! | 表 | 一条记录代表 | 谁改它 |
//! | --- | --- | --- |
//! | `workbuddy_rotation_plan` | 用户配置（单行，`id='default'`） | 界面保存 |
//! | `workbuddy_rotation_run` | **一轮**夜间计划（含最初账号的快照引用） | 调度器按状态机推进 |
//! | `workbuddy_rotation_step` | 这一轮里的**一个账号** | 调度器 + 活跃取证 |
//!
//! ## 为什么轮次要单独一张表
//!
//! `workbuddy_switch_journal` 记的是"一次切换"，语义是"回到上一站"。夜间计划要的是
//! "回到**这一轮开始前**那一站"——A→B→C→D 之后必须回 A，不是回 C。所以本轮的
//! 原始会话快照（`origin_*` 五列）与 journal 完全分开存，即使中途那条 journal 被
//! 结算、被后续切换取代，也不影响这一轮能不能回到 A。
//!
//! ## 一天最多一轮
//!
//! `UNIQUE(plan_id, plan_date)` 把"同一天不重复执行"做成数据库约束。重复心跳、
//! 两个 worker 同时 tick、AICC 重启都不会凭空多跑一轮：插入冲突就是"今天已经跑过"。

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::clock::now_rfc3339;
use crate::rpc::{RpcError, RpcResult};

/// 计划只有一行：配置是"这台机器上的夜间轮换"，不是多套方案。
pub const DEFAULT_PLAN_ID: &str = "default";
/// 不支持把计划存成多套，但**保留 id 列**：以后要加"工作日 / 周末两套"时不用改表。
pub const DEFAULT_TIMEZONE: &str = "Asia/Shanghai";
/// 22:30。
pub const DEFAULT_START_MINUTES: i64 = 22 * 60 + 30;
/// 07:00（次日）。
pub const DEFAULT_END_MINUTES: i64 = 7 * 60;
/// 每个账号默认停留 60 分钟。
pub const DEFAULT_DWELL_SECONDS: i64 = 3600;
pub const MIN_DWELL_SECONDS: i64 = 5 * 60;
pub const MAX_DWELL_SECONDS: i64 = 6 * 3600;

pub const STATE_SCHEDULED: &str = "scheduled";
pub const STATE_PRECHECKING: &str = "prechecking";
pub const STATE_SWITCHING: &str = "switching";
pub const STATE_DWELLING: &str = "dwelling";
pub const STATE_RESTORING: &str = "restoring";
pub const STATE_COMPLETED: &str = "completed";
pub const STATE_SKIPPED: &str = "skipped";
pub const STATE_FAILED: &str = "failed";
/// 该恢复却不敢自动覆盖（用户中途换过账号、快照损坏）：必须由人来决定。
pub const STATE_RECOVERY_REQUIRED: &str = "recovery_required";

/// 终态：不再推进，也不再需要 tick 看它。
pub const TERMINAL_STATES: &[&str] = &[
    STATE_COMPLETED,
    STATE_SKIPPED,
    STATE_FAILED,
    STATE_RECOVERY_REQUIRED,
];

/// 这一轮是谁发起的。
///
/// 区别很要紧：**关闭自动计划不该打断用户手动跑的那一轮**。没有这个字段时，
/// `runNow` 起来的一轮会在下一次心跳被当成"关闭开关后遗留的轮次"直接收尾。
pub const TRIGGER_SCHEDULED: &str = "scheduled";
pub const TRIGGER_MANUAL: &str = "manual";

pub const STEP_PLANNED: &str = "planned";
pub const STEP_SWITCHING: &str = "switching";
pub const STEP_DWELLING: &str = "dwelling";
pub const STEP_DONE: &str = "done";
pub const STEP_SKIPPED: &str = "skipped";
pub const STEP_FAILED: &str = "failed";
/// 客户端当前登录的就是它：没有写文件，也没有快照。
pub const STEP_ALREADY_LIVE: &str = "already_live";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationPlan {
    pub id: String,
    pub enabled: bool,
    pub timezone: String,
    /// ISO 星期：1=周一 … 7=周日。空表示每天都跑。
    pub weekdays: Vec<u8>,
    /// 本地时间的分钟数（0..1439）。`end < start` 表示跨午夜。
    pub start_minutes: i64,
    pub end_minutes: i64,
    pub account_ids: Vec<String>,
    pub dwell_seconds: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl RotationPlan {
    /// 默认配置：**关闭**。任何自动操作都必须由用户显式打开。
    pub fn defaults() -> Self {
        let now = now_rfc3339();
        Self {
            id: DEFAULT_PLAN_ID.to_string(),
            enabled: false,
            timezone: DEFAULT_TIMEZONE.to_string(),
            weekdays: vec![1, 2, 3, 4, 5, 6, 7],
            start_minutes: DEFAULT_START_MINUTES,
            end_minutes: DEFAULT_END_MINUTES,
            account_ids: Vec::new(),
            dwell_seconds: DEFAULT_DWELL_SECONDS,
            created_at: now.clone(),
            updated_at: now,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationRun {
    pub id: String,
    pub plan_id: String,
    /// 窗口**开始**那天的本地日期（`YYYY-MM-DD`）。跨午夜时结束是次日。
    pub plan_date: String,
    pub timezone: String,
    pub window_start: String,
    pub window_end: String,
    /// `scheduled`（自动计划）或 `manual`（用户点「立即执行一轮」）。
    pub trigger: String,
    pub origin_account_id: Option<String>,
    pub origin_uid: Option<String>,
    pub origin_snapshot_path: Option<String>,
    pub origin_snapshot_sha256: Option<String>,
    pub origin_marker: bool,
    pub origin_running: bool,
    pub state: String,
    /// 下一步要处理的账号下标（`account_ids` 的位置，不是 step 行数）。
    pub cursor: i64,
    pub current_journal_id: Option<String>,
    pub last_target_account_id: Option<String>,
    pub last_target_uid: Option<String>,
    pub restore_attempts: i64,
    pub restore_result: Option<String>,
    /// 用户显式按过「暂停」或关了开关：**必须持久化**。
    ///
    /// 只在内存/响应里表达是不够的：`stop()` 拿不到跨进程锁时请求就丢了，
    /// 下一次心跳会把手动轮次当"用户还想继续"接着切。
    pub stop_requested: bool,
    pub detail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl RotationRun {
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATES.contains(&self.state.as_str())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationStep {
    pub id: String,
    pub run_id: String,
    pub seq: i64,
    pub account_id: String,
    pub uid: String,
    pub mode: Option<String>,
    pub planned_at: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub status: String,
    pub journal_id: Option<String>,
    pub skip_reason: Option<String>,
    pub detail: Option<String>,
    /// 自有授权的只读预检结论：`accepted` / `rejected` / `unknown`。
    pub auth_precheck: Option<String>,
    pub activity_before: Option<Value>,
    pub activity_after: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

pub struct NewRotationRun<'a> {
    pub id: &'a str,
    pub plan_id: &'a str,
    pub plan_date: &'a str,
    pub timezone: &'a str,
    pub window_start: &'a str,
    pub window_end: &'a str,
    /// [`TRIGGER_SCHEDULED`] 或 [`TRIGGER_MANUAL`]。
    pub trigger: &'a str,
}

/// 切换 journal 与轮次之间的**唯一**关联点。
///
/// `first` 那一跳的快照就是"本轮最初的登录态"，所以只有它需要把 `origin_*` 写进轮次。
pub struct RotationSwitchLink<'a> {
    pub run_id: &'a str,
    pub account_id: &'a str,
    pub uid: &'a str,
    pub is_first: bool,
}

fn parse_weekdays(raw: &str) -> Vec<u8> {
    raw.split(',')
        .filter_map(|part| part.trim().parse::<u8>().ok())
        .filter(|day| (1..=7).contains(day))
        .collect()
}

fn encode_weekdays(days: &[u8]) -> String {
    let mut sorted: Vec<u8> = days
        .iter()
        .copied()
        .filter(|d| (1..=7).contains(d))
        .collect();
    sorted.sort_unstable();
    sorted.dedup();
    sorted
        .iter()
        .map(|day| day.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

fn parse_ids(raw: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(raw).unwrap_or_default()
}

fn encode_ids(ids: &[String]) -> String {
    serde_json::to_string(ids).unwrap_or_else(|_| "[]".to_string())
}

fn map_plan(row: &rusqlite::Row<'_>) -> rusqlite::Result<RotationPlan> {
    Ok(RotationPlan {
        id: row.get("id")?,
        enabled: row.get::<_, i64>("enabled")? != 0,
        timezone: row.get("timezone")?,
        weekdays: parse_weekdays(&row.get::<_, String>("weekdays")?),
        start_minutes: row.get("start_minutes")?,
        end_minutes: row.get("end_minutes")?,
        account_ids: parse_ids(&row.get::<_, String>("account_ids")?),
        dwell_seconds: row.get("dwell_seconds")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<RotationRun> {
    Ok(RotationRun {
        id: row.get("id")?,
        plan_id: row.get("plan_id")?,
        plan_date: row.get("plan_date")?,
        timezone: row.get("timezone")?,
        window_start: row.get("window_start")?,
        window_end: row.get("window_end")?,
        trigger: row
            .get::<_, Option<String>>("trigger")?
            .unwrap_or_else(|| TRIGGER_SCHEDULED.to_string()),
        origin_account_id: row.get("origin_account_id")?,
        origin_uid: row.get("origin_uid")?,
        origin_snapshot_path: row.get("origin_snapshot_path")?,
        origin_snapshot_sha256: row.get("origin_snapshot_sha256")?,
        origin_marker: row.get::<_, i64>("origin_marker")? != 0,
        origin_running: row.get::<_, i64>("origin_running")? != 0,
        state: row.get("state")?,
        cursor: row.get("cursor")?,
        current_journal_id: row.get("current_journal_id")?,
        last_target_account_id: row.get("last_target_account_id")?,
        last_target_uid: row.get("last_target_uid")?,
        restore_attempts: row.get("restore_attempts")?,
        restore_result: row.get("restore_result")?,
        stop_requested: row.get::<_, Option<i64>>("stop_requested")?.unwrap_or(0) != 0,
        detail: row.get("detail")?,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn map_step(row: &rusqlite::Row<'_>) -> rusqlite::Result<RotationStep> {
    let decode =
        |raw: Option<String>| raw.and_then(|text| serde_json::from_str::<Value>(&text).ok());
    Ok(RotationStep {
        id: row.get("id")?,
        run_id: row.get("run_id")?,
        seq: row.get("seq")?,
        account_id: row.get("account_id")?,
        uid: row.get("uid")?,
        mode: row.get("mode")?,
        planned_at: row.get("planned_at")?,
        started_at: row.get("started_at")?,
        ended_at: row.get("ended_at")?,
        status: row.get("status")?,
        journal_id: row.get("journal_id")?,
        skip_reason: row.get("skip_reason")?,
        detail: row.get("detail")?,
        auth_precheck: row.get("auth_precheck")?,
        activity_before: decode(row.get::<_, Option<String>>("activity_before")?),
        activity_after: decode(row.get::<_, Option<String>>("activity_after")?),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// v28：给轮次加"谁发起的"。
///
/// 逐列判断后再 `ADD COLUMN`：迁移事务已提交但版本号没写下去时，重跑必须不炸。
pub fn migrate_v28(tx: &rusqlite::Transaction<'_>) -> RpcResult<()> {
    let mut stmt = tx.prepare("PRAGMA table_info(workbuddy_rotation_run)")?;
    let existing: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    if !existing.iter().any(|column| column == "trigger") {
        tx.execute(
            "ALTER TABLE workbuddy_rotation_run
                ADD COLUMN trigger TEXT NOT NULL DEFAULT 'scheduled'",
            [],
        )?;
    }
    Ok(())
}

/// v29：持久化"用户要求停止这一轮"。
///
/// 与 `enabled` 是两件事：开关只管**还开不开新轮次**；手动发起的一轮要能自己走完。
/// 但用户一旦明确按了暂停/关闭，这个意图必须盖过手动豁免。
pub fn migrate_v29(tx: &rusqlite::Transaction<'_>) -> RpcResult<()> {
    let mut stmt = tx.prepare("PRAGMA table_info(workbuddy_rotation_run)")?;
    let existing: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);
    if !existing.iter().any(|column| column == "stop_requested") {
        tx.execute(
            "ALTER TABLE workbuddy_rotation_run
                ADD COLUMN stop_requested INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    Ok(())
}

impl super::Db {
    /// 记下"这一轮要停"。幂等，重复调用无副作用。
    pub fn request_rotation_stop(&self, run_id: &str) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE workbuddy_rotation_run
                SET stop_requested = 1, updated_at = ?2
              WHERE id = ?1 AND stop_requested = 0",
            params![run_id, now_rfc3339()],
        )?;
        Ok(())
    }

    pub fn rotation_plan(&self) -> RpcResult<Option<RotationPlan>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM workbuddy_rotation_plan WHERE id = ?1",
                params![DEFAULT_PLAN_ID],
                map_plan,
            )
            .optional()?)
    }

    /// 没有配置时返回默认值（**关闭**），不写库。
    pub fn rotation_plan_or_default(&self) -> RpcResult<RotationPlan> {
        Ok(self.rotation_plan()?.unwrap_or_else(RotationPlan::defaults))
    }

    /// 保存配置。`enabled` 单独由 [`Self::set_rotation_enabled`] 管理：
    /// 保存一次时间不该顺手把计划打开。
    pub fn save_rotation_plan(&self, plan: &RotationPlan) -> RpcResult<()> {
        let now = now_rfc3339();
        self.conn.execute(
            "INSERT INTO workbuddy_rotation_plan (
                 id, enabled, timezone, weekdays, start_minutes, end_minutes,
                 account_ids, dwell_seconds, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT(id) DO UPDATE SET
                 timezone = excluded.timezone,
                 weekdays = excluded.weekdays,
                 start_minutes = excluded.start_minutes,
                 end_minutes = excluded.end_minutes,
                 account_ids = excluded.account_ids,
                 dwell_seconds = excluded.dwell_seconds,
                 updated_at = excluded.updated_at",
            params![
                plan.id,
                i64::from(plan.enabled),
                plan.timezone,
                encode_weekdays(&plan.weekdays),
                plan.start_minutes,
                plan.end_minutes,
                encode_ids(&plan.account_ids),
                plan.dwell_seconds,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn set_rotation_enabled(&self, enabled: bool) -> RpcResult<()> {
        let now = now_rfc3339();
        // 第一次开启时可能还没有配置行：先落一行默认值（仍按传入的开关）。
        self.conn.execute(
            "INSERT INTO workbuddy_rotation_plan (
                 id, enabled, timezone, weekdays, start_minutes, end_minutes,
                 account_ids, dwell_seconds, created_at, updated_at
             ) VALUES (?1, ?2, ?3, '1,2,3,4,5,6,7', ?4, ?5, '[]', ?6, ?7, ?7)
             ON CONFLICT(id) DO UPDATE SET enabled = excluded.enabled, updated_at = excluded.updated_at",
            params![
                DEFAULT_PLAN_ID,
                i64::from(enabled),
                DEFAULT_TIMEZONE,
                DEFAULT_START_MINUTES,
                DEFAULT_END_MINUTES,
                DEFAULT_DWELL_SECONDS,
                now,
            ],
        )?;
        Ok(())
    }

    /// 开一轮。同一天已经有记录时返回 `already_ran_today`——**不覆盖**已有轮次。
    pub fn begin_rotation_run(&self, new: &NewRotationRun<'_>) -> RpcResult<bool> {
        let now = now_rfc3339();
        let result = self.conn.execute(
            "INSERT INTO workbuddy_rotation_run (
                 id, plan_id, plan_date, timezone, window_start, window_end, trigger,
                 state, cursor, restore_attempts, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, 0, ?9, ?9)",
            params![
                new.id,
                new.plan_id,
                new.plan_date,
                new.timezone,
                new.window_start,
                new.window_end,
                new.trigger,
                STATE_SCHEDULED,
                now,
            ],
        );
        match result {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Ok(false)
            }
            Err(err) => Err(RpcError::storage(err)),
        }
    }

    pub fn rotation_run(&self, id: &str) -> RpcResult<Option<RotationRun>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM workbuddy_rotation_run WHERE id = ?1",
                params![id],
                map_run,
            )
            .optional()?)
    }

    /// 还没走完的那一轮（至多一条：一天最多一轮，且旧轮次会被收尾）。
    pub fn active_rotation_run(&self) -> RpcResult<Option<RotationRun>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM workbuddy_rotation_run
                  WHERE state NOT IN ('completed','skipped','failed','recovery_required')
                  ORDER BY created_at DESC, rowid DESC LIMIT 1",
                [],
                map_run,
            )
            .optional()?)
    }

    /// 崩溃恢复用：最近一轮，不论状态。
    pub fn latest_rotation_run(&self) -> RpcResult<Option<RotationRun>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM workbuddy_rotation_run
                  ORDER BY created_at DESC, rowid DESC LIMIT 1",
                [],
                map_run,
            )
            .optional()?)
    }

    /// 按日期取那一轮（用于"今天是否已经跑过"）。`plan_date` 是本地日期。
    pub fn rotation_run_for_date(&self, plan_date: &str) -> RpcResult<Option<RotationRun>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM workbuddy_rotation_run WHERE plan_id = ?1 AND plan_date = ?2",
                params![DEFAULT_PLAN_ID, plan_date],
                map_run,
            )
            .optional()?)
    }

    /// 状态 CAS：只有当前状态是 `from` 时才推进。
    ///
    /// 两个 worker 同时 tick 时，只有一个能把 `dwelling` 改成 `switching`；
    /// 另一个拿到 `false`，这一轮就什么都不做。
    pub fn cas_rotation_state(&self, id: &str, from: &str, to: &str) -> RpcResult<bool> {
        let changed = self.conn.execute(
            "UPDATE workbuddy_rotation_run
                SET state = ?3, updated_at = ?4
              WHERE id = ?1 AND state = ?2",
            params![id, from, to, now_rfc3339()],
        )?;
        Ok(changed == 1)
    }

    pub fn set_rotation_state(&self, id: &str, state: &str, detail: Option<&str>) -> RpcResult<()> {
        let changed = self.conn.execute(
            "UPDATE workbuddy_rotation_run
                SET state = ?2, detail = COALESCE(?3, detail), updated_at = ?4
              WHERE id = ?1",
            params![id, state, detail, now_rfc3339()],
        )?;
        if changed == 0 {
            return Err(RpcError::not_found(format!("轮换记录不存在：{id}")));
        }
        Ok(())
    }

    pub fn set_rotation_cursor(&self, id: &str, cursor: i64) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE workbuddy_rotation_run SET cursor = ?2, updated_at = ?3 WHERE id = ?1",
            params![id, cursor, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 记"本轮最后一次成功切到的账号"——恢复时用它区分「还在我们的控制里」与
    /// 「用户自己换过」。
    pub fn set_rotation_last_target(&self, id: &str, account_id: &str, uid: &str) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE workbuddy_rotation_run
                SET last_target_account_id = ?2, last_target_uid = ?3, updated_at = ?4
              WHERE id = ?1",
            params![id, account_id, uid, now_rfc3339()],
        )?;
        Ok(())
    }

    pub fn set_rotation_origin(&self, id: &str, origin: &RotationOrigin<'_>) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE workbuddy_rotation_run
                SET origin_account_id = ?2, origin_uid = ?3, origin_snapshot_path = ?4,
                    origin_snapshot_sha256 = ?5, origin_marker = ?6, origin_running = ?7,
                    updated_at = ?8
              WHERE id = ?1",
            params![
                id,
                origin.account_id,
                origin.uid,
                origin.snapshot_path,
                origin.snapshot_sha256,
                i64::from(origin.marker),
                i64::from(origin.running),
                now_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn note_rotation_restore(&self, id: &str, attempts: i64, result: &str) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE workbuddy_rotation_run
                SET restore_attempts = ?2, restore_result = ?3, updated_at = ?4
              WHERE id = ?1",
            params![id, attempts, result, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 开一条步骤。`(run_id, seq)` 唯一：重复 tick 第二次会撞唯一键，
    /// 调用方按"已存在"处理（更新而不是插入）。
    pub fn begin_rotation_step(
        &self,
        id: &str,
        run_id: &str,
        seq: i64,
        account_id: &str,
        uid: &str,
        planned_at: Option<&str>,
    ) -> RpcResult<bool> {
        let now = now_rfc3339();
        let result = self.conn.execute(
            "INSERT INTO workbuddy_rotation_step (
                 id, run_id, seq, account_id, uid, planned_at, status, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                id,
                run_id,
                seq,
                account_id,
                uid,
                planned_at,
                STEP_PLANNED,
                now,
            ],
        );
        match result {
            Ok(_) => Ok(true),
            Err(rusqlite::Error::SqliteFailure(err, _))
                if err.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Ok(false)
            }
            Err(err) => Err(RpcError::storage(err)),
        }
    }

    pub fn rotation_step_by_seq(&self, run_id: &str, seq: i64) -> RpcResult<Option<RotationStep>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM workbuddy_rotation_step WHERE run_id = ?1 AND seq = ?2",
                params![run_id, seq],
                map_step,
            )
            .optional()?)
    }

    pub fn rotation_steps(&self, run_id: &str) -> RpcResult<Vec<RotationStep>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM workbuddy_rotation_step WHERE run_id = ?1 ORDER BY seq")?;
        let rows = stmt.query_map(params![run_id], map_step)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 步骤进入"正在切换"。带上 journal id 便于事后对齐。
    pub fn start_rotation_step(
        &self,
        id: &str,
        mode: &str,
        journal_id: Option<&str>,
        auth_precheck: Option<&str>,
    ) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE workbuddy_rotation_step
                SET status = ?2, mode = COALESCE(?3, mode), journal_id = COALESCE(?4, journal_id),
                    auth_precheck = COALESCE(?5, auth_precheck),
                    started_at = COALESCE(started_at, ?6), updated_at = ?6
              WHERE id = ?1",
            params![
                id,
                STEP_SWITCHING,
                mode,
                journal_id,
                auth_precheck,
                now_rfc3339()
            ],
        )?;
        Ok(())
    }

    /// 收尾一步。`at` 由调用方给：调度器用的是自己的时钟，而库里的
    /// `now_rfc3339()` 是系统时间——测试推进时钟时两者会差几个小时，
    /// 停留截止时间就会算错（表现为"刚切完立刻恢复"）。
    pub fn finish_rotation_step(
        &self,
        id: &str,
        status: &str,
        detail: Option<&str>,
        skip_reason: Option<&str>,
        at: Option<&str>,
    ) -> RpcResult<()> {
        let now = now_rfc3339();
        self.conn.execute(
            "UPDATE workbuddy_rotation_step
                SET status = ?2, detail = COALESCE(?3, detail),
                    skip_reason = COALESCE(?4, skip_reason),
                    ended_at = COALESCE(ended_at, ?6, ?5), updated_at = ?5
              WHERE id = ?1",
            params![id, status, detail, skip_reason, now, at],
        )?;
        Ok(())
    }

    pub fn set_rotation_step_activity(
        &self,
        id: &str,
        before: Option<&Value>,
        after: Option<&Value>,
    ) -> RpcResult<()> {
        let encode = |value: Option<&Value>| value.and_then(|v| serde_json::to_string(v).ok());
        self.conn.execute(
            "UPDATE workbuddy_rotation_step
                SET activity_before = COALESCE(?2, activity_before),
                    activity_after = COALESCE(?3, activity_after),
                    updated_at = ?4
              WHERE id = ?1",
            params![id, encode(before), encode(after), now_rfc3339()],
        )?;
        Ok(())
    }

    /// 最近 N 轮（界面「执行记录」用）。
    pub fn rotation_runs(&self, limit: u32) -> RpcResult<Vec<RotationRun>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM workbuddy_rotation_run ORDER BY created_at DESC, rowid DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit.max(1)], map_run)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

pub struct RotationOrigin<'a> {
    pub account_id: Option<&'a str>,
    pub uid: Option<&'a str>,
    pub snapshot_path: Option<&'a str>,
    pub snapshot_sha256: Option<&'a str>,
    pub marker: bool,
    pub running: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    fn temp_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut db = Db::open(&dir.path().join("accounts.sqlite")).expect("open");
        db.migrate().expect("migrate");
        (dir, db)
    }

    fn plan() -> RotationPlan {
        let mut plan = RotationPlan::defaults();
        plan.weekdays = vec![7, 1, 1, 3];
        plan.account_ids = vec!["acc-a".into(), "acc-b".into()];
        plan
    }

    #[test]
    fn plan_round_trips_and_keeps_weekdays_sorted() {
        let (_dir, db) = temp_db();
        assert!(
            !db.rotation_plan_or_default().unwrap().enabled,
            "默认必须关闭"
        );
        db.save_rotation_plan(&plan()).unwrap();

        let saved = db.rotation_plan().unwrap().unwrap();
        assert_eq!(saved.weekdays, vec![1, 3, 7], "去重并排序");
        assert_eq!(saved.account_ids, vec!["acc-a", "acc-b"]);
        assert!(!saved.enabled, "保存配置不得顺手打开计划");
    }

    #[test]
    fn enabling_creates_the_row_when_absent() {
        let (_dir, db) = temp_db();
        db.set_rotation_enabled(true).unwrap();
        let saved = db.rotation_plan().unwrap().unwrap();
        assert!(saved.enabled);
        assert_eq!(saved.start_minutes, DEFAULT_START_MINUTES);
    }

    #[test]
    fn one_run_per_local_day_is_a_database_constraint() {
        let (_dir, db) = temp_db();
        let new = NewRotationRun {
            id: "run-1",
            plan_id: DEFAULT_PLAN_ID,
            plan_date: "2026-11-20",
            timezone: DEFAULT_TIMEZONE,
            window_start: "2026-11-20T14:30:00Z",
            window_end: "2026-11-21T23:00:00Z",
            trigger: TRIGGER_SCHEDULED,
        };
        assert!(db.begin_rotation_run(&new).unwrap());
        let again = db
            .begin_rotation_run(&NewRotationRun { id: "run-2", ..new })
            .unwrap();
        assert!(!again, "同一天第二次必须被拒绝");
        assert_eq!(
            db.active_rotation_run().unwrap().unwrap().id,
            "run-1",
            "不得覆盖已有轮次"
        );
    }

    #[test]
    fn state_cas_only_wins_once() {
        let (_dir, db) = temp_db();
        db.begin_rotation_run(&NewRotationRun {
            id: "run-1",
            plan_id: DEFAULT_PLAN_ID,
            plan_date: "2026-11-20",
            timezone: DEFAULT_TIMEZONE,
            window_start: "2026-11-20T14:30:00Z",
            window_end: "2026-11-21T23:00:00Z",
            trigger: TRIGGER_SCHEDULED,
        })
        .unwrap();

        assert!(db
            .cas_rotation_state("run-1", STATE_SCHEDULED, STATE_PRECHECKING)
            .unwrap());
        assert!(
            !db.cas_rotation_state("run-1", STATE_SCHEDULED, STATE_PRECHECKING)
                .unwrap(),
            "第二个 worker 必须拿不到这一轮"
        );
        assert_eq!(
            db.rotation_run("run-1").unwrap().unwrap().state,
            STATE_PRECHECKING
        );
    }

    #[test]
    fn steps_are_unique_per_sequence_and_keep_activity() {
        let (_dir, db) = temp_db();
        db.begin_rotation_run(&NewRotationRun {
            id: "run-1",
            plan_id: DEFAULT_PLAN_ID,
            plan_date: "2026-11-20",
            timezone: DEFAULT_TIMEZONE,
            window_start: "2026-11-20T14:30:00Z",
            window_end: "2026-11-21T23:00:00Z",
            trigger: TRIGGER_SCHEDULED,
        })
        .unwrap();

        assert!(db
            .begin_rotation_step("s-0", "run-1", 0, "acc-a", "uid-a", None)
            .unwrap());
        assert!(
            !db.begin_rotation_step("s-0b", "run-1", 0, "acc-a", "uid-a", None)
                .unwrap(),
            "(run_id, seq) 必须唯一"
        );
        db.set_rotation_step_activity(
            "s-0",
            Some(&serde_json::json!({"todayActive": false})),
            Some(&serde_json::json!({"todayActive": true})),
        )
        .unwrap();

        let steps = db.rotation_steps("run-1").unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(
            steps[0].activity_after.as_ref().unwrap()["todayActive"],
            true
        );
    }
}
