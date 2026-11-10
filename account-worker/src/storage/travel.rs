//! Buddy 旅行的持久化。
//!
//! 两张表，职责分开：
//!
//! * `travel_states` —— 每个账号**一行**的最新快照。界面直接读它，不需要联网；
//!   调度器也靠它判断「上次检查是什么时候」「今天是否已经派出过」。
//! * `travel_events` —— 追加日志。派出的地点、领取的积分、失败原因都留痕，
//!   用于回答「昨天为什么没派出」这类问题，也用于避免重复领取。
//!
//! 状态机本身在 `crate::travel::state`，这里只负责存取，不含判断。

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::clock::now_rfc3339;
use crate::rpc::RpcResult;

/// 单个账号的旅行快照。
///
/// 字段全部来自服务端 `data`（`server_state`、`arrive_at`、`record_id`……）
/// 加上本地推断出的 `label`：界面显示 `label`，排障看 `server_state`。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TravelState {
    pub account_id: String,
    /// 界面用的粗粒度状态，见 `crate::travel::state::Label`。
    pub label: String,
    /// 服务端原样返回的 `state`（`idle` / `traveling` / `arrived`），空表示还没查到。
    pub server_state: String,
    pub location_id: Option<i64>,
    pub location_name: String,
    /// 派出时间（秒）。服务端可能给毫秒，写库前统一成秒。
    pub depart_at: i64,
    pub arrive_at: i64,
    pub record_id: i64,
    pub daily_limit_reached: bool,
    pub claimed: bool,
    pub reward_credits: Option<i64>,
    pub claimed_at: Option<String>,
    /// 已经领取过的那一程的 `record_id`。重复领取靠它兜住。
    pub claimed_record_id: i64,
    /// 最后一次**成功**派出当天的北京日期（`YYYY-MM-DD`），本地每日上限用。
    pub depart_date: Option<String>,
    pub last_checked_at: Option<String>,
    pub last_depart_attempt_at: Option<String>,
    /// 连续失败次数，退避算法的输入。
    pub failures: i64,
    /// 退避截止时间（RFC3339）。到点之前不再打接口。
    pub retry_after: Option<String>,
    pub last_error: Option<String>,
    /// 检测到「别的客户端也在操作」时的说明。
    pub conflict: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TravelEvent {
    pub id: i64,
    pub account_id: String,
    pub kind: String,
    pub status: String,
    pub message: Option<String>,
    pub credits: Option<i64>,
    pub created_at: String,
}

fn map_state(row: &rusqlite::Row<'_>) -> rusqlite::Result<TravelState> {
    Ok(TravelState {
        account_id: row.get(0)?,
        label: row.get(1)?,
        server_state: row.get(2)?,
        location_id: row.get(3)?,
        location_name: row.get(4)?,
        depart_at: row.get(5)?,
        arrive_at: row.get(6)?,
        record_id: row.get(7)?,
        daily_limit_reached: row.get::<_, i64>(8)? != 0,
        claimed: row.get::<_, i64>(9)? != 0,
        reward_credits: row.get(10)?,
        claimed_at: row.get(11)?,
        claimed_record_id: row.get(12)?,
        depart_date: row.get(13)?,
        last_checked_at: row.get(14)?,
        last_depart_attempt_at: row.get(15)?,
        failures: row.get(16)?,
        retry_after: row.get(17)?,
        last_error: row.get(18)?,
        conflict: row.get(19)?,
        updated_at: row.get(20)?,
    })
}

const STATE_COLUMNS: &str = "account_id, label, server_state, location_id, location_name, \
     depart_at, arrive_at, record_id, daily_limit_reached, claimed, reward_credits, claimed_at, \
     claimed_record_id, depart_date, last_checked_at, last_depart_attempt_at, failures, \
     retry_after, last_error, conflict, updated_at";

impl super::Db {
    pub fn travel_state(&self, account_id: &str) -> RpcResult<Option<TravelState>> {
        let sql = format!("SELECT {STATE_COLUMNS} FROM travel_states WHERE account_id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt.query_row(params![account_id], map_state).optional()?)
    }

    pub fn travel_states(&self) -> RpcResult<Vec<TravelState>> {
        let sql = format!("SELECT {STATE_COLUMNS} FROM travel_states");
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], map_state)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 整行覆盖（快照语义）。`updated_at` 由调用方给，便于把一次操作里的多个改动
    /// 写成同一个时间点。
    pub fn save_travel_state(&self, state: &TravelState) -> RpcResult<()> {
        self.conn.execute(
            "INSERT INTO travel_states (
                 account_id, label, server_state, location_id, location_name,
                 depart_at, arrive_at, record_id, daily_limit_reached, claimed,
                 reward_credits, claimed_at, claimed_record_id, depart_date,
                 last_checked_at, last_depart_attempt_at, failures, retry_after,
                 last_error, conflict, updated_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21)
             ON CONFLICT (account_id) DO UPDATE SET
                 label = excluded.label,
                 server_state = excluded.server_state,
                 location_id = excluded.location_id,
                 location_name = excluded.location_name,
                 depart_at = excluded.depart_at,
                 arrive_at = excluded.arrive_at,
                 record_id = excluded.record_id,
                 daily_limit_reached = excluded.daily_limit_reached,
                 claimed = excluded.claimed,
                 reward_credits = excluded.reward_credits,
                 claimed_at = excluded.claimed_at,
                 claimed_record_id = excluded.claimed_record_id,
                 depart_date = excluded.depart_date,
                 last_checked_at = excluded.last_checked_at,
                 last_depart_attempt_at = excluded.last_depart_attempt_at,
                 failures = excluded.failures,
                 retry_after = excluded.retry_after,
                 last_error = excluded.last_error,
                 conflict = excluded.conflict,
                 updated_at = excluded.updated_at",
            params![
                state.account_id,
                state.label,
                state.server_state,
                state.location_id,
                state.location_name,
                state.depart_at,
                state.arrive_at,
                state.record_id,
                i64::from(state.daily_limit_reached),
                i64::from(state.claimed),
                state.reward_credits,
                state.claimed_at,
                state.claimed_record_id,
                state.depart_date,
                state.last_checked_at,
                state.last_depart_attempt_at,
                state.failures,
                state.retry_after,
                state.last_error,
                state.conflict,
                state.updated_at,
            ],
        )?;
        Ok(())
    }

    /// 只记录事件，不改快照。`message` 已经在调用方过了一遍脱敏。
    pub fn append_travel_event(
        &self,
        account_id: &str,
        kind: &str,
        status: &str,
        message: Option<&str>,
        credits: Option<i64>,
    ) -> RpcResult<()> {
        self.conn.execute(
            "INSERT INTO travel_events (account_id, kind, status, message, credits, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![account_id, kind, status, message, credits, now_rfc3339()],
        )?;
        Ok(())
    }

    pub fn travel_events(&self, account_id: &str, limit: u32) -> RpcResult<Vec<TravelEvent>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, account_id, kind, status, message, credits, created_at
             FROM travel_events WHERE account_id = ?1
             ORDER BY id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![account_id, limit.min(200) as i64], |row| {
            Ok(TravelEvent {
                id: row.get(0)?,
                account_id: row.get(1)?,
                kind: row.get(2)?,
                status: row.get(3)?,
                message: row.get(4)?,
                credits: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    pub fn set_travel_enabled(&self, account_id: &str, enabled: bool) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE accounts SET travel_enabled = ?1, updated_at = ?2 WHERE id = ?3",
            params![i64::from(enabled), now_rfc3339(), account_id],
        )?;
        Ok(())
    }

    /// 当前开着自动旅行的账号 id。
    pub fn travel_enabled_account_ids(&self) -> RpcResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM accounts WHERE platform = 'workbuddy' AND travel_enabled = 1",
        )?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 跨日：把昨天的「今日已完成」降级成「未知」，等下一轮重新问服务端。
    ///
    /// **不清理** `reward_credits` / `claimed_at`：那是历史事实，界面要显示
    /// 「上次领取 +5」；每日上限靠 `depart_date` 与服务端的 `daily_limit_reached`
    /// 判断，不依赖这几个字段。仍在 traveling / arrived 的行程保持原样。
    pub fn roll_travel_states_to_today(&self, today: &str) -> RpcResult<usize> {
        let n = self.conn.execute(
            "UPDATE travel_states
                SET label = 'unknown',
                    daily_limit_reached = 0,
                    updated_at = ?1
              WHERE depart_date IS NOT NULL
                AND depart_date <> ?2
                AND label IN ('done_today', 'dispatchable')",
            params![now_rfc3339(), today],
        )?;
        Ok(n)
    }
}
