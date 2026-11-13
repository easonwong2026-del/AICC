//! 每日签到：开关、每日结果、每次尝试。
//!
//! 三张表各管一件事，**不要混用**：
//!
//! | 表 | 答的问题 | 一天几行 |
//! | --- | --- | --- |
//! | `checkin_records` | 这个账号今天最后是什么结论 | 1（UNIQUE 约束） |
//! | `task_executions` | 每次尝试怎么失败的、下次什么时候再试 | N |
//! | `accounts.last_result` | 界面列表上那一行文字 | 1（列） |
//!
//! 两条写入铁律（P0，改之前先读 PRD §6.2）：
//!
//! 1. **只降级不升级的字段是 `status`**：服务端确认过 `claimed` 之后，
//!    后面任何 `already` / `failed` / `unknown` 都不许把它改回去，也不许把奖励抹成 NULL。
//!    一次网络抖动不能把当天已经到账的 100 积分变成"失败"。
//! 2. **签到开关是独立字段**。走 `accounts.upsert` 改开关会顺带推进 `auth_epoch`
//!    （见 MIGRATION_V5），让在途的授权操作全部作废 —— 点一下开关就把旅行/成长中心
//!    的旧快照废掉，属于"开关不该有的副作用"。所以这里只更新签到相关列。

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::clock::now_rfc3339;
use crate::rpc::RpcResult;

/// `checkin_records.status` 的取值，受表的 CHECK 约束限制。
pub const STATUS_CLAIMED: &str = "claimed";
pub const STATUS_ALREADY: &str = "already";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_SKIPPED: &str = "skipped";

/// `task_executions.task_kind`。用一张已有表记尝试，就不必为"诊断字段"再造一张。
pub const TASK_KIND: &str = "daily_checkin";

/// `task_executions.status` 的取值（表上有 CHECK 约束）。
pub const ATTEMPT_SUCCESS: &str = "success";
pub const ATTEMPT_FAILED: &str = "failed";
pub const ATTEMPT_SKIPPED: &str = "skipped";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinRecord {
    pub id: i64,
    pub account_id: String,
    pub platform: String,
    pub local_date: String,
    pub status: String,
    pub credits: Option<i64>,
    pub checked_at: String,
    pub error_kind: Option<String>,
    /// `auto` / `manual`：这一天的结论是被谁触发的。
    pub execution_source: Option<String>,
}

pub struct NewCheckinRecord<'a> {
    pub account_id: &'a str,
    pub platform: &'a str,
    pub local_date: &'a str,
    pub status: &'a str,
    pub credits: Option<i64>,
    pub error_kind: Option<&'a str>,
    pub execution_source: &'a str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckinAttempt {
    pub id: i64,
    pub account_id: Option<String>,
    pub status: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub credits: Option<i64>,
    pub error_kind: Option<String>,
    pub error_detail: Option<String>,
    pub next_retry_at: Option<String>,
}

pub struct NewAttempt<'a> {
    pub account_id: &'a str,
    pub status: &'a str,
    pub started_at: &'a str,
    pub finished_at: Option<&'a str>,
    pub credits: Option<i64>,
    pub error_kind: Option<&'a str>,
    pub error_detail: Option<&'a str>,
    pub next_retry_at: Option<&'a str>,
}

const RECORD_COLUMNS: &str = "id, account_id, platform, local_date, status, credits, checked_at,
    error_kind, execution_source";

fn map_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<CheckinRecord> {
    Ok(CheckinRecord {
        id: row.get(0)?,
        account_id: row.get(1)?,
        platform: row.get(2)?,
        local_date: row.get(3)?,
        status: row.get(4)?,
        credits: row.get(5)?,
        checked_at: row.get(6)?,
        error_kind: row.get(7)?,
        execution_source: row.get(8)?,
    })
}

impl super::Db {
    /// 改签到开关。**不碰 `auth_epoch`**，也不碰任何授权列。
    ///
    /// 返回是否真的更新了一行：账号不存在时是 `false`，调用方据此报 not_found，
    /// 而不是静默成功（界面上会显示"已开启"，库里什么都没有）。
    pub fn set_checkin_enabled(&mut self, account_id: &str, enabled: bool) -> RpcResult<bool> {
        let changed = self.conn.execute(
            "UPDATE accounts SET checkin_enabled = ?2, updated_at = ?3 WHERE id = ?1",
            params![account_id, i64::from(enabled), now_rfc3339()],
        )?;
        Ok(changed > 0)
    }

    /// 写"今天最后一次结论"和界面上那一行状态。
    ///
    /// 只有服务端确认过的结论才调用这里（`claimed` / `already`）；失败与未知
    /// 由 `insert_checkin_attempt` 记，不污染账号行。
    pub fn record_checkin_result(
        &mut self,
        account_id: &str,
        checked_at: &str,
        result: &str,
    ) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE accounts SET last_checkin_at = ?2, last_result = ?3, updated_at = ?4
              WHERE id = ?1",
            params![account_id, checked_at, result, now_rfc3339()],
        )?;
        Ok(())
    }

    /// 一天一条的最终结论。**幂等**：同一天重复写只会更新，不会插第二行。
    ///
    /// 降级保护：
    /// - 已 `claimed` 的一天，`already` / `failed` / `skipped` 一律不写（返回旧行）。
    /// - 已 `claimed` 的一天再 `claimed` 但这次没拿到奖励数额 → 保留旧奖励，不抹成 NULL。
    pub fn upsert_checkin_record(
        &mut self,
        input: NewCheckinRecord<'_>,
    ) -> RpcResult<CheckinRecord> {
        let existing = self.checkin_record(input.account_id, input.local_date)?;
        if let Some(old) = &existing {
            if old.status == STATUS_CLAIMED && input.status != STATUS_CLAIMED {
                // 今天已经确认到账，后面这些噪声不覆盖它。
                return Ok(old.clone());
            }
        }
        let credits = match (&existing, input.credits) {
            // 新的没数额、旧的确认过 → 保留旧数额。
            (Some(old), None) if old.status == STATUS_CLAIMED => old.credits,
            (_, value) => value,
        };
        let error_kind = if input.status == STATUS_CLAIMED || input.status == STATUS_ALREADY {
            None
        } else {
            input.error_kind
        };
        self.conn.execute(
            "INSERT INTO checkin_records
                (account_id, platform, local_date, status, credits, checked_at,
                 error_kind, execution_source)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (account_id, local_date) DO UPDATE SET
                 status = excluded.status,
                 credits = excluded.credits,
                 checked_at = excluded.checked_at,
                 error_kind = excluded.error_kind,
                 execution_source = excluded.execution_source",
            params![
                input.account_id,
                input.platform,
                input.local_date,
                input.status,
                credits,
                now_rfc3339(),
                error_kind,
                input.execution_source,
            ],
        )?;
        self.checkin_record(input.account_id, input.local_date)?
            .ok_or_else(|| crate::rpc::RpcError::new(crate::rpc::E_STORAGE, "签到记录写入后读不到"))
    }

    pub fn checkin_record(
        &self,
        account_id: &str,
        local_date: &str,
    ) -> RpcResult<Option<CheckinRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RECORD_COLUMNS} FROM checkin_records
              WHERE account_id = ?1 AND local_date = ?2"
        ))?;
        Ok(stmt
            .query_row(params![account_id, local_date], map_record)
            .optional()?)
    }

    /// 最近若干天。`account_id` 为 None 时返回全部账号的（界面"全部记录"用）。
    pub fn checkin_records(
        &self,
        account_id: Option<&str>,
        limit: u32,
    ) -> RpcResult<Vec<CheckinRecord>> {
        let (sql, args): (String, Vec<Box<dyn rusqlite::ToSql>>) = match account_id {
            Some(id) => (
                format!(
                    "SELECT {RECORD_COLUMNS} FROM checkin_records
                      WHERE account_id = ?1 ORDER BY local_date DESC, id DESC LIMIT ?2"
                ),
                vec![Box::new(id.to_string()), Box::new(limit)],
            ),
            None => (
                format!(
                    "SELECT {RECORD_COLUMNS} FROM checkin_records
                      ORDER BY local_date DESC, id DESC LIMIT ?1"
                ),
                vec![Box::new(limit)],
            ),
        };
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), map_record)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 记一次尝试。失败不影响主流程：结论已经在返回值里，界面能看到。
    pub fn insert_checkin_attempt(&self, attempt: NewAttempt<'_>) -> RpcResult<i64> {
        self.conn.execute(
            "INSERT INTO task_executions
                (account_id, platform, task_kind, status, started_at, finished_at,
                 credits, error_kind, error_detail, next_retry_at)
             VALUES (?1, 'workbuddy', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                attempt.account_id,
                TASK_KIND,
                attempt.status,
                attempt.started_at,
                attempt.finished_at,
                attempt.credits,
                attempt.error_kind,
                attempt.error_detail,
                attempt.next_retry_at,
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn checkin_attempts(&self, account_id: &str, limit: u32) -> RpcResult<Vec<CheckinAttempt>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, account_id, status, started_at, finished_at, credits,
                    error_kind, error_detail, next_retry_at
               FROM task_executions
              WHERE account_id = ?1 AND task_kind = ?2
              ORDER BY started_at DESC, id DESC LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![account_id, TASK_KIND, limit], |row| {
            Ok(CheckinAttempt {
                id: row.get(0)?,
                account_id: row.get(1)?,
                status: row.get(2)?,
                started_at: row.get(3)?,
                finished_at: row.get(4)?,
                credits: row.get(5)?,
                error_kind: row.get(6)?,
                error_detail: row.get(7)?,
                next_retry_at: row.get(8)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 今天这个账号已经尝试过几次。退避与"一天最多 N 次"都靠它，
    /// 存在库里而不是内存，App 重启后不会重新数一遍。
    pub fn checkin_attempts_since(&self, account_id: &str, since: &str) -> RpcResult<i64> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM task_executions
              WHERE account_id = ?1 AND task_kind = ?2 AND started_at >= ?3",
            params![account_id, TASK_KIND, since],
            |row| row.get(0),
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{AccountInput, Db};

    /// 造一个 v9 的库：逐条跑到 v9，钉住 user_version。
    /// `Db::open` 会一路迁到最新，所以旧库必须自己搭。
    fn v9_db(dir: &std::path::Path) -> std::path::PathBuf {
        let path = dir.join("accounts.sqlite");
        let conn = rusqlite::Connection::open(&path).expect("open legacy");
        for migration in super::super::MIGRATIONS {
            if migration.version > 9 {
                break;
            }
            for step in migration.steps {
                if let super::super::MigrationStep::Sql(sql) = step {
                    conn.execute_batch(sql).expect("legacy step");
                }
            }
        }
        conn.pragma_update(None, "user_version", 9)
            .expect("pin version");
        path
    }

    fn db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut db = Db::open(&dir.path().join("accounts.sqlite")).expect("open");
        db.migrate().expect("migrate");
        (db, dir)
    }

    fn account(db: &mut Db, uid: &str) -> String {
        db.upsert_account(&AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.to_string(),
            ..Default::default()
        })
        .expect("account")
        .id
    }

    fn record<'a>(
        id: &'a str,
        date: &'a str,
        status: &'a str,
        credits: Option<i64>,
    ) -> NewCheckinRecord<'a> {
        NewCheckinRecord {
            account_id: id,
            platform: "workbuddy",
            local_date: date,
            status,
            credits,
            error_kind: None,
            execution_source: "auto",
        }
    }

    #[test]
    fn the_switch_does_not_touch_the_credential_epoch() {
        let (mut db, _dir) = db();
        let id = account(&mut db, "uid-a");
        let before = db.get_account(&id).unwrap().unwrap().auth_epoch;

        assert!(db.set_checkin_enabled(&id, true).unwrap());
        let after = db.get_account(&id).unwrap().unwrap();
        assert!(after.checkin_enabled);
        assert_eq!(after.auth_epoch, before, "开关不是凭据变更");

        assert!(db.set_checkin_enabled(&id, false).unwrap());
        let off = db.get_account(&id).unwrap().unwrap();
        assert!(!off.checkin_enabled);
        assert_eq!(off.auth_epoch, before);
    }

    #[test]
    fn the_switch_reports_a_missing_account() {
        let (mut db, _dir) = db();
        assert!(!db.set_checkin_enabled("nobody", true).unwrap());
    }

    #[test]
    fn one_row_per_account_and_day() {
        let (mut db, _dir) = db();
        let id = account(&mut db, "uid-a");
        db.upsert_checkin_record(record(&id, "2026-11-13", STATUS_FAILED, None))
            .unwrap();
        db.upsert_checkin_record(record(&id, "2026-11-13", STATUS_CLAIMED, Some(100)))
            .unwrap();

        let rows = db.checkin_records(Some(&id), 10).unwrap();
        assert_eq!(rows.len(), 1, "同一天只有一条最终结论");
        assert_eq!(rows[0].status, STATUS_CLAIMED);
        assert_eq!(rows[0].credits, Some(100));
    }

    #[test]
    fn a_later_failure_cannot_overwrite_a_confirmed_claim() {
        let (mut db, _dir) = db();
        let id = account(&mut db, "uid-a");
        db.upsert_checkin_record(record(&id, "2026-11-13", STATUS_CLAIMED, Some(100)))
            .unwrap();
        let after = db
            .upsert_checkin_record(record(&id, "2026-11-13", STATUS_FAILED, None))
            .unwrap();

        assert_eq!(after.status, STATUS_CLAIMED);
        assert_eq!(after.credits, Some(100), "网络抖动不能把到账的积分抹掉");
    }

    #[test]
    fn an_already_does_not_erase_the_claimed_reward() {
        let (mut db, _dir) = db();
        let id = account(&mut db, "uid-a");
        db.upsert_checkin_record(record(&id, "2026-11-13", STATUS_CLAIMED, Some(100)))
            .unwrap();
        let after = db
            .upsert_checkin_record(record(&id, "2026-11-13", STATUS_ALREADY, None))
            .unwrap();
        assert_eq!(after.status, STATUS_CLAIMED);
        assert_eq!(after.credits, Some(100));
    }

    #[test]
    fn a_second_claim_without_a_reward_keeps_the_known_number() {
        let (mut db, _dir) = db();
        let id = account(&mut db, "uid-a");
        db.upsert_checkin_record(record(&id, "2026-11-13", STATUS_CLAIMED, Some(100)))
            .unwrap();
        let after = db
            .upsert_checkin_record(record(&id, "2026-11-13", STATUS_CLAIMED, None))
            .unwrap();
        assert_eq!(after.credits, Some(100), "没有新数字就保留旧数字");
    }

    #[test]
    fn records_are_scoped_per_account() {
        let (mut db, _dir) = db();
        let a = account(&mut db, "uid-a");
        let b = account(&mut db, "uid-b");
        db.upsert_checkin_record(record(&a, "2026-11-13", STATUS_CLAIMED, Some(100)))
            .unwrap();
        db.upsert_checkin_record(record(&b, "2026-11-13", STATUS_ALREADY, None))
            .unwrap();

        assert_eq!(db.checkin_records(Some(&a), 10).unwrap().len(), 1);
        assert_eq!(
            db.checkin_records(Some(&a), 10).unwrap()[0].status,
            STATUS_CLAIMED
        );
        assert_eq!(db.checkin_records(None, 10).unwrap().len(), 2);
        assert!(db.checkin_records(Some("nobody"), 10).unwrap().is_empty());
    }

    #[test]
    fn attempts_are_counted_per_account_and_window() {
        let (mut db, _dir) = db();
        let a = account(&mut db, "uid-a");
        let b = account(&mut db, "uid-b");
        for id in [&a, &a, &b] {
            db.insert_checkin_attempt(NewAttempt {
                account_id: id,
                status: ATTEMPT_FAILED,
                started_at: "2026-11-13T01:00:00Z",
                finished_at: None,
                credits: None,
                error_kind: Some("network"),
                error_detail: None,
                next_retry_at: None,
            })
            .unwrap();
        }
        assert_eq!(
            db.checkin_attempts_since(&a, "2026-11-13T00:00:00Z")
                .unwrap(),
            2
        );
        assert_eq!(
            db.checkin_attempts_since(&a, "2026-11-13T02:00:00Z")
                .unwrap(),
            0
        );
        assert_eq!(db.checkin_attempts(&b, 10).unwrap().len(), 1);
    }

    /// PRD 10.1-24：v9 → v10 升级不许丢任何旧数据。
    #[test]
    fn upgrading_from_v9_keeps_accounts_records_and_growth_history() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = v9_db(dir.path());

        // 在 v9 库里塞入"升级前的现场"：账号、签到记录、成长执行记录。
        {
            let conn = rusqlite::Connection::open(&path).expect("reopen v9");
            conn.execute_batch(
                "INSERT INTO accounts
                     (id, platform, uid, nickname, auth_status, checkin_enabled,
                      created_at, updated_at)
                 VALUES ('acc-1', 'workbuddy', 'uid-1', '卡里', 'valid', 1,
                         '2026-11-01T00:00:00Z', '2026-11-01T00:00:00Z');
                 INSERT INTO checkin_records
                     (account_id, platform, local_date, status, credits, checked_at)
                 VALUES ('acc-1', 'workbuddy', '2026-11-12', 'claimed', 100,
                         '2026-11-12T01:00:00Z');
                 INSERT INTO growth_runs
                     (account_id, task_code, action, outcome, credit, executed_at)
                 VALUES ('acc-1', 'Expert_lighthouse', 'claim', 'ok', 100,
                         '2026-11-12T02:00:00Z');",
            )
            .expect("seed v9");
        }

        let db = Db::open(&path).expect("upgrade");
        assert_eq!(db.schema_version().unwrap(), 10);
        assert!(db.integrity_ok());

        let account = db.get_account("acc-1").unwrap().expect("账号还在");
        assert!(account.checkin_enabled, "升级不许把开关重置");
        assert_eq!(account.auth_status, "valid");

        let old = db.checkin_record("acc-1", "2026-11-12").unwrap().unwrap();
        assert_eq!(old.status, STATUS_CLAIMED);
        assert_eq!(old.credits, Some(100));
        assert_eq!(old.error_kind, None, "旧记录的两列是 NULL，不是空串");

        assert_eq!(
            db.growth_runs("acc-1", 10).unwrap().len(),
            1,
            "成长记录还在"
        );
    }

    #[test]
    fn deleting_an_account_removes_its_records_and_attempts() {
        let (mut db, _dir) = db();
        let a = account(&mut db, "uid-a");
        db.upsert_checkin_record(record(&a, "2026-11-13", STATUS_CLAIMED, Some(1)))
            .unwrap();
        db.insert_checkin_attempt(NewAttempt {
            account_id: &a,
            status: ATTEMPT_SUCCESS,
            started_at: "2026-11-13T01:00:00Z",
            finished_at: None,
            credits: Some(1),
            error_kind: None,
            error_detail: None,
            next_retry_at: None,
        })
        .unwrap();

        db.delete_account(&a).unwrap();
        assert!(db.checkin_records(Some(&a), 10).unwrap().is_empty());
        assert!(db.checkin_attempts(&a, 10).unwrap().is_empty());
    }
}
