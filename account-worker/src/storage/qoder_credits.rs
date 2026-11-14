//! Qoder 每日 Credits 领取记录（v17）。
//!
//! 只记"发过什么请求、服务端怎么回"，**不缓存活动状态**：能不能领永远以
//! `/sash/api/v1/me/campaigns` 的实时返回为准。这张表的三个用途：
//!
//! 1. 界面上的"上次领取结果"与失败原因；
//! 2. 自动领取的**同周期去重**：同一 `campaign_id` 已经 `ok`/`already` 过，
//!    就不在同一周期里再发一次 POST；
//! 3. 事后对账：某个 credits 是哪个来源、哪次执行拿到的。
//!
//! `credits` 为 NULL 表示"这次没拿到数量"（不是 0）。0 会被界面读成"没奖励"。

use rusqlite::params;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::clock::now_rfc3339;
use crate::rpc::RpcResult;

/// v17 迁移步骤：账号加自动领取开关，建领取账表。**两步都先检测再动手**。
///
/// 与 v16 同样的理由：`user_version` 可能被回拨，重复执行不能炸在
/// `duplicate column name` 上。
pub(super) fn migrate_v17(tx: &rusqlite::Transaction<'_>) -> crate::rpc::RpcResult<()> {
    let has_column: i64 = tx.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('qoder_accounts') WHERE name = 'credits_auto_enabled'",
        [],
        |row| row.get(0),
    )?;
    if has_column == 0 {
        tx.execute_batch(
            "ALTER TABLE qoder_accounts ADD COLUMN credits_auto_enabled INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    // 注意：多行字符串，不要改成行尾反斜杠续行（会把相邻两行粘成一个 token）。
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS qoder_credit_runs (
             id           INTEGER PRIMARY KEY AUTOINCREMENT,
             uid          TEXT NOT NULL UNIQUE,
             account_id   TEXT NOT NULL REFERENCES qoder_accounts(id) ON DELETE CASCADE,
             campaign_id  TEXT,
             campaign_key TEXT,
             outcome      TEXT NOT NULL CHECK (outcome IN (
                              'ok','already','nothing','rejected','forbidden','rate_limited',
                              'server_error','network','unsupported','failed')),
             credits      INTEGER,
             http_status  INTEGER,
             auth_source  TEXT,
             error_kind   TEXT,
             error_detail TEXT,
             trigger      TEXT NOT NULL CHECK (trigger IN ('manual', 'auto')),
             executed_at  TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS idx_qoder_credit_runs_account
             ON qoder_credit_runs (account_id, executed_at DESC);
         CREATE INDEX IF NOT EXISTS idx_qoder_credit_runs_campaign
             ON qoder_credit_runs (account_id, campaign_id, outcome);",
    )?;
    Ok(())
}

/// v18 迁移步骤：给领取账加上**活动周期**两列。
///
/// 为什么需要：`campaignId` 由服务端生成，同一个 id 在不同周期可能被复用
/// （参考实现把"日窗"当成活动周期）。只按 `campaignId` 去重会出现两种错：
///
/// - 跨周期复用同一个 id → 次日被当成"已经领过"，**领不到**；
/// - 没有周期信息时无法区分"这一期领过"和"上一期领过"。
///
/// 于是去重键变成 `(account_id, campaign_id, period_start)`。
/// 两列都可空：服务端不给窗口时就不参与去重（宁可多一次幂等 POST，
/// 也不能凭空挡住一次合法的领取）。
///
/// 与 v16 / v17 一样先检测再动手：`user_version` 被回拨后重复执行不能炸。
pub(super) fn migrate_v18(tx: &rusqlite::Transaction<'_>) -> crate::rpc::RpcResult<()> {
    for column in ["period_start", "period_end"] {
        let exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('qoder_credit_runs') WHERE name = ?1",
            [column],
            |row| row.get(0),
        )?;
        if exists == 0 {
            tx.execute_batch(&format!(
                "ALTER TABLE qoder_credit_runs ADD COLUMN {column} INTEGER;"
            ))?;
        }
    }
    Ok(())
}

/// 一次领取尝试的结论。取值与 `qoder_credits::Outcome` 一一对应。
pub const OUTCOMES: &[&str] = &[
    "ok",
    "already",
    "nothing",
    "rejected",
    "forbidden",
    "rate_limited",
    "server_error",
    "network",
    "unsupported",
    "failed",
];

/// 触发方式。自动与手动必须能分开统计，否则"自动化到底跑没跑"无从回答。
pub const SOURCES: &[&str] = &["manual", "auto"];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreditRun {
    pub id: i64,
    pub account_id: String,
    pub campaign_id: Option<String>,
    pub campaign_key: Option<String>,
    /// 服务端给的活动窗口（Unix 秒）。`None` = 服务端没给，不参与去重。
    pub period_start: Option<i64>,
    pub period_end: Option<i64>,
    pub outcome: String,
    pub credits: Option<i64>,
    pub http_status: Option<i64>,
    pub auth_source: Option<String>,
    pub error_kind: Option<String>,
    pub error_detail: Option<String>,
    pub trigger: String,
    pub executed_at: String,
}

impl CreditRun {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "accountId": self.account_id,
            "campaignId": self.campaign_id,
            "campaignKey": self.campaign_key,
            "periodStart": self.period_start,
            "periodEnd": self.period_end,
            "outcome": self.outcome,
            "credits": self.credits,
            "httpStatus": self.http_status,
            "authSource": self.auth_source,
            "errorKind": self.error_kind,
            "errorDetail": self.error_detail,
            "trigger": self.trigger,
            "executedAt": self.executed_at,
        })
    }
}

/// 领取记录去重键：`qc_` + sha256(account|trigger|campaign|outcome|纳秒) 前 12 hex。
///
/// 它只为"同一毫秒内的重复写不要炸在唯一约束上"服务，**不是**业务幂等键：
/// 同一活动一小时内领两次是两条**应当都存在**的记录（一次失败一次成功）。
/// 因此时间戳精确到纳秒，并且把 outcome 一起算进去。
fn run_uid(
    account_id: &str,
    trigger: &str,
    campaign_id: &str,
    outcome: &str,
    executed_at: &str,
) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(account_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(trigger.as_bytes());
    hasher.update([0u8]);
    hasher.update(campaign_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(outcome.as_bytes());
    hasher.update([0u8]);
    hasher.update(executed_at.as_bytes());
    hasher.update([0u8]);
    hasher.update(nanos.to_string().as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    format!("qc_{hex}")
}

const COLUMNS: &str = "id, account_id, campaign_id, campaign_key, outcome, credits, \
     http_status, auth_source, error_kind, error_detail, trigger, executed_at, \
     period_start, period_end";

fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<CreditRun> {
    Ok(CreditRun {
        id: row.get(0)?,
        account_id: row.get(1)?,
        campaign_id: row.get(2)?,
        campaign_key: row.get(3)?,
        period_start: row.get(12)?,
        period_end: row.get(13)?,
        outcome: row.get(4)?,
        credits: row.get(5)?,
        http_status: row.get(6)?,
        auth_source: row.get(7)?,
        error_kind: row.get(8)?,
        error_detail: row.get(9)?,
        trigger: row.get(10)?,
        executed_at: row.get(11)?,
    })
}

/// 写一条记录要的全部内容。参数多到必须用具名结构，避免又一次"第 7 个参数是啥"。
pub struct RunInput<'a> {
    pub account_id: &'a str,
    pub campaign_id: Option<&'a str>,
    pub campaign_key: Option<&'a str>,
    pub period_start: Option<i64>,
    pub period_end: Option<i64>,
    pub outcome: &'a str,
    pub credits: Option<i64>,
    pub http_status: Option<i64>,
    pub auth_source: Option<&'a str>,
    pub error_kind: Option<&'a str>,
    pub error_detail: Option<&'a str>,
    pub trigger: &'a str,
}

impl crate::storage::Db {
    pub fn record_qoder_credit_run(&mut self, input: RunInput<'_>) -> RpcResult<CreditRun> {
        if !OUTCOMES.contains(&input.outcome) {
            return Err(crate::rpc::RpcError::invalid_params(format!(
                "outcome 取值非法：{}",
                input.outcome
            )));
        }
        if !SOURCES.contains(&input.trigger) {
            return Err(crate::rpc::RpcError::invalid_params(format!(
                "trigger 只支持 manual / auto，收到 {}",
                input.trigger
            )));
        }
        let executed_at = now_rfc3339();
        let uid = run_uid(
            input.account_id,
            input.trigger,
            input.campaign_id.unwrap_or(""),
            input.outcome,
            &executed_at,
        );
        // 记录不是业务状态：账号已经被删掉时**不写孤儿行**，但也不报错打断领取。
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO qoder_credit_runs (
                 uid, account_id, campaign_id, campaign_key, period_start, period_end,
                 outcome, credits, http_status, auth_source, error_kind, error_detail,
                 trigger, executed_at)
             SELECT ?1, ?2, ?3, ?4, ?13, ?14, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12
             WHERE EXISTS (SELECT 1 FROM qoder_accounts WHERE id = ?2)",
            params![
                uid,
                input.account_id,
                input.campaign_id,
                input.campaign_key,
                input.outcome,
                input.credits,
                input.http_status,
                input.auth_source,
                input.error_kind,
                input.error_detail,
                input.trigger,
                executed_at,
                input.period_start,
                input.period_end
            ],
        )?;
        if inserted == 0 {
            return Err(crate::rpc::RpcError::not_found(format!(
                "Qoder 账号不存在，未记录领取结果：{}",
                input.account_id
            )));
        }
        let id: i64 = self.conn.query_row(
            "SELECT id FROM qoder_credit_runs WHERE uid = ?1",
            params![uid],
            |row| row.get(0),
        )?;
        self.get_qoder_credit_run(id)?
            .ok_or_else(|| crate::rpc::RpcError::storage("写入后读不到领取记录"))
    }

    pub fn get_qoder_credit_run(&self, id: i64) -> RpcResult<Option<CreditRun>> {
        let sql = format!("SELECT {COLUMNS} FROM qoder_credit_runs WHERE id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params![id])?;
        match rows.next()? {
            Some(row) => Ok(Some(map_run(row)?)),
            None => Ok(None),
        }
    }

    pub fn list_qoder_credit_runs(
        &self,
        account_id: Option<&str>,
        limit: u32,
    ) -> RpcResult<Vec<CreditRun>> {
        let mut sql = format!("SELECT {COLUMNS} FROM qoder_credit_runs WHERE 1 = 1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(account_id) = account_id.filter(|id| !id.is_empty()) {
            sql.push_str(" AND account_id = ?");
            args.push(Box::new(account_id.to_string()));
        }
        sql.push_str(" ORDER BY executed_at DESC, id DESC LIMIT ?");
        args.push(Box::new(limit.clamp(1, 200) as i64));
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), map_run)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// `since` 之后（北京时间当天零点起）有没有成功记录。
    ///
    /// 与 `qoder_credit_claimed` 的分工：这个回答"今天领到过没有"，
    /// 不看活动 id、不看条数；那个回答"这一期这个活动领过没有"。
    pub fn qoder_credit_succeeded_since(&self, account_id: &str, since: &str) -> RpcResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM qoder_credit_runs
             WHERE account_id = ?1 AND executed_at >= ?2 AND outcome IN ('ok', 'already')",
            params![account_id, since],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// 这个账号在**这一期**活动上是否已经成功领过（`ok` / `already`）。
    ///
    /// 去重键是 `(account_id, campaign_id, period_start)`：
    ///
    /// - 服务端复用 `campaignId` 时，新周期 `period_start` 不同 → **不**算领过，
    ///   次日仍能领；
    /// - 同一周期重复调用 → 算领过，不重复提交。
    ///
    /// `period_start` 为 `None`（服务端没给窗口）时**永不匹配**：
    /// 宁可多一次幂等 POST，也不能因为认不出周期而挡掉一次合法领取。
    pub fn qoder_credit_claimed(
        &self,
        account_id: &str,
        campaign_id: &str,
        period_start: Option<i64>,
    ) -> RpcResult<bool> {
        let Some(period) = period_start else {
            return Ok(false);
        };
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM qoder_credit_runs
             WHERE account_id = ?1 AND campaign_id = ?2 AND period_start IS ?3
               AND outcome IN ('ok', 'already')",
            params![account_id, campaign_id, period],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// 打开 / 关闭某账号的自动领取。**不碰授权、不碰 auth_epoch**。
    pub fn set_qoder_credits_auto(&mut self, account_id: &str, enabled: bool) -> RpcResult<bool> {
        let changed = self.conn.execute(
            "UPDATE qoder_accounts SET credits_auto_enabled = ?1 WHERE id = ?2",
            params![if enabled { 1 } else { 0 }, account_id],
        )?;
        Ok(changed > 0)
    }

    pub fn qoder_credits_auto(&self, account_id: &str) -> RpcResult<bool> {
        let value: Option<i64> = self
            .conn
            .query_row(
                "SELECT credits_auto_enabled FROM qoder_accounts WHERE id = ?1",
                params![account_id],
                |row| row.get(0),
            )
            .ok();
        Ok(value.unwrap_or(0) != 0)
    }

    /// 所有打开自动领取的账号。自动任务只遍历这个集合。
    pub fn qoder_credits_auto_accounts(
        &self,
    ) -> RpcResult<Vec<super::qoder_accounts::QoderAccountRecord>> {
        let sql = format!(
            "SELECT {} FROM qoder_accounts WHERE credits_auto_enabled = 1 \
             ORDER BY edition ASC, created_at ASC",
            super::qoder_accounts::ACCOUNT_COLUMNS
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map([], super::qoder_accounts::map_account)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    fn db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("accounts.sqlite")).unwrap();
        (db, dir)
    }

    fn account(db: &mut Db) -> String {
        db.upsert_qoder_account("qoder_cn", "u-credits", Some("积分号"), None)
            .unwrap()
            .id
    }

    fn run<'a>(account_id: &'a str, outcome: &'a str, credits: Option<i64>) -> RunInput<'a> {
        RunInput {
            account_id,
            campaign_id: Some("c-1"),
            campaign_key: Some("daily"),
            period_start: Some(1_700_000_000),
            period_end: Some(1_700_086_400),
            outcome,
            credits,
            http_status: Some(200),
            auth_source: Some("own_browser"),
            error_kind: None,
            error_detail: None,
            trigger: "manual",
        }
    }

    #[test]
    fn records_survive_and_are_scoped_per_account() {
        let (mut db, _dir) = db();
        let id = account(&mut db);
        db.record_qoder_credit_run(run(&id, "ok", Some(100)))
            .unwrap();
        let rows = db.list_qoder_credit_runs(Some(&id), 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].outcome, "ok");
        assert_eq!(rows[0].credits, Some(100));
        assert_eq!(rows[0].trigger, "manual");
        assert_eq!(rows[0].auth_source.as_deref(), Some("own_browser"));
    }

    #[test]
    fn credits_stay_null_when_unknown_never_zero() {
        let (mut db, _dir) = db();
        let id = account(&mut db);
        let record = db
            .record_qoder_credit_run(RunInput {
                credits: None,
                outcome: "already",
                ..run(&id, "already", None)
            })
            .unwrap();
        assert!(record.credits.is_none(), "未知奖励不能记成 0");
    }

    #[test]
    fn invalid_outcome_is_rejected() {
        let (mut db, _dir) = db();
        let id = account(&mut db);
        assert!(db
            .record_qoder_credit_run(RunInput {
                outcome: "yolo",
                ..run(&id, "yolo", None)
            })
            .is_err());
    }

    #[test]
    fn run_for_a_deleted_account_is_not_written() {
        let (mut db, _dir) = db();
        let id = account(&mut db);
        db.delete_qoder_account(&id).unwrap();
        assert!(db.record_qoder_credit_run(run(&id, "ok", Some(1))).is_err());
        assert!(db.list_qoder_credit_runs(None, 10).unwrap().is_empty());
    }

    #[test]
    fn claim_dedupe_only_counts_successful_runs() {
        let (mut db, _dir) = db();
        let id = account(&mut db);
        let period = Some(1_700_000_000);
        // 失败不算"领过"：否则一次 429 会让当天再也领不到。
        db.record_qoder_credit_run(run(&id, "rate_limited", None))
            .unwrap();
        assert!(!db.qoder_credit_claimed(&id, "c-1", period).unwrap());
        db.record_qoder_credit_run(run(&id, "ok", Some(100)))
            .unwrap();
        assert!(db.qoder_credit_claimed(&id, "c-1", period).unwrap());
        // 另一个活动不受影响。
        assert!(!db.qoder_credit_claimed(&id, "c-2", period).unwrap());
    }

    #[test]
    fn succeeded_since_answers_by_range_not_by_recent_count() {
        let (mut db, _dir) = db();
        let id = account(&mut db);
        let today = crate::clock::beijing_now().format("%Y-%m-%d").to_string();
        let since = format!("{today}T00:00:00Z"); // 简化：只验证"范围"语义
        assert!(!db.qoder_credit_succeeded_since(&id, &since).unwrap());

        db.record_qoder_credit_run(run(&id, "ok", Some(100)))
            .unwrap();
        // 再灌 6 条失败记录：旧实现只取最近 5 条，成功那条会被挤出去。
        for _ in 0..6 {
            db.record_qoder_credit_run(RunInput {
                outcome: "network",
                ..run(&id, "network", None)
            })
            .unwrap();
        }
        assert!(
            db.qoder_credit_succeeded_since(&id, &since).unwrap(),
            "成功记录不该被后续失败挤出窗口"
        );
    }

    /// 服务端复用同一个 `campaignId` 时，**下一期必须还能领**。
    #[test]
    fn the_same_campaign_id_in_a_new_period_is_not_considered_claimed() {
        let (mut db, _dir) = db();
        let id = account(&mut db);
        db.record_qoder_credit_run(run(&id, "ok", Some(100)))
            .unwrap();

        // 同一期：领过了。
        assert!(db
            .qoder_credit_claimed(&id, "c-1", Some(1_700_000_000))
            .unwrap());
        // 次日同一个 id、窗口不同：不该被挡住。
        assert!(!db
            .qoder_credit_claimed(&id, "c-1", Some(1_700_086_400))
            .unwrap());
        // 服务端没给窗口：不参与去重（宁可多一次幂等 POST）。
        assert!(!db.qoder_credit_claimed(&id, "c-1", None).unwrap());
    }

    #[test]
    fn auto_flag_is_per_account_and_off_by_default() {
        let (mut db, _dir) = db();
        let id = account(&mut db);
        assert!(!db.qoder_credits_auto(&id).unwrap());
        assert!(db.qoder_credits_auto_accounts().unwrap().is_empty());
        assert!(db.set_qoder_credits_auto(&id, true).unwrap());
        assert!(db.qoder_credits_auto(&id).unwrap());
        assert_eq!(db.qoder_credits_auto_accounts().unwrap().len(), 1);
        assert!(db.set_qoder_credits_auto(&id, false).unwrap());
        assert!(db.qoder_credits_auto_accounts().unwrap().is_empty());
        // 不存在的账号返回 false，而不是静默成功。
        assert!(!db.set_qoder_credits_auto("qa_missing", true).unwrap());
    }

    #[test]
    fn deleting_the_account_takes_its_runs_away() {
        let (mut db, _dir) = db();
        let id = account(&mut db);
        db.record_qoder_credit_run(run(&id, "ok", Some(100)))
            .unwrap();
        db.delete_qoder_account(&id).unwrap();
        assert!(db.list_qoder_credit_runs(None, 10).unwrap().is_empty());
    }
}
