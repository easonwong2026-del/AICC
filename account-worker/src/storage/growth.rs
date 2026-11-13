//! 成长中心执行记录（v9）。
//!
//! 这张表回答的是"AICC 都对谁做过什么、结果如何"，**不是**"任务现在是什么状态"。
//! 任务状态永远来自 `/tasks`；本地记录只用来在界面上显示"最近一次执行结果"
//! 和排查"为什么这个任务没领到"。两者冲突时以服务端为准。

use rusqlite::params;
use serde::Serialize;

use crate::clock::now_rfc3339;
use crate::rpc::RpcResult;

/// 写入时的动作。只有这两种会改服务端状态。
pub const ACTION_ACCEPT: &str = "accept";
pub const ACTION_CLAIM: &str = "claim";

/// 结果分类。`already` 是幂等回执，既不算成功也不算失败。
pub const OUTCOME_OK: &str = "ok";
pub const OUTCOME_ALREADY: &str = "already";
pub const OUTCOME_REJECTED: &str = "rejected";
pub const OUTCOME_UNKNOWN: &str = "unknown";
pub const OUTCOME_FAILED: &str = "failed";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrowthRun {
    pub id: i64,
    pub account_id: String,
    pub task_code: String,
    pub action: String,
    pub outcome: String,
    pub credit: Option<i64>,
    pub energy: Option<i64>,
    pub error_kind: Option<String>,
    pub error_detail: Option<String>,
    pub executed_at: String,
}

pub struct NewGrowthRun<'a> {
    pub account_id: &'a str,
    pub task_code: &'a str,
    pub action: &'a str,
    pub outcome: &'a str,
    pub credit: Option<i64>,
    pub energy: Option<i64>,
    pub error_kind: Option<&'a str>,
    pub error_detail: Option<&'a str>,
}

const RUN_COLUMNS: &str = "id, account_id, task_code, action, outcome, credit, energy,
    error_kind, error_detail, executed_at";

fn map_run(row: &rusqlite::Row<'_>) -> rusqlite::Result<GrowthRun> {
    Ok(GrowthRun {
        id: row.get(0)?,
        account_id: row.get(1)?,
        task_code: row.get(2)?,
        action: row.get(3)?,
        outcome: row.get(4)?,
        credit: row.get(5)?,
        energy: row.get(6)?,
        error_kind: row.get(7)?,
        error_detail: row.get(8)?,
        executed_at: row.get(9)?,
    })
}

impl super::Db {
    /// 记一条执行结果。写失败不该影响主流程 —— 但调用方要看得见，所以返回 Result。
    pub fn insert_growth_run(&self, run: NewGrowthRun<'_>) -> RpcResult<i64> {
        self.conn.execute(
            "INSERT INTO growth_runs
                (account_id, task_code, action, outcome, credit, energy,
                 error_kind, error_detail, executed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                run.account_id,
                run.task_code,
                run.action,
                run.outcome,
                run.credit,
                run.energy,
                run.error_kind,
                run.error_detail,
                now_rfc3339(),
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn growth_runs(&self, account_id: &str, limit: u32) -> RpcResult<Vec<GrowthRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM growth_runs
              WHERE account_id = ?1 ORDER BY executed_at DESC, id DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![account_id, limit], map_run)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 每个 task 各取最近一条，界面上的任务行用它显示"最近执行结果"。
    pub fn latest_growth_run_per_task(&self, account_id: &str) -> RpcResult<Vec<GrowthRun>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {RUN_COLUMNS} FROM growth_runs
              WHERE account_id = ?1
                AND id IN (
                    SELECT MAX(id) FROM growth_runs WHERE account_id = ?1 GROUP BY task_code
                )
              ORDER BY executed_at DESC"
        ))?;
        let rows = stmt.query_map(params![account_id], map_run)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 累计领到的积分与能量（只统计真正到账的那部分）。
    pub fn growth_totals(&self, account_id: &str) -> RpcResult<(i64, i64)> {
        let row: (Option<i64>, Option<i64>) = self.conn.query_row(
            "SELECT SUM(credit), SUM(energy) FROM growth_runs
              WHERE account_id = ?1 AND action = 'claim' AND outcome = 'ok'",
            params![account_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        Ok((row.0.unwrap_or(0), row.1.unwrap_or(0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (super::super::Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut db = super::super::Db::open(&dir.path().join("accounts.sqlite")).expect("open");
        db.migrate().expect("migrate");
        (db, dir)
    }

    fn account(db: &mut super::super::Db) -> String {
        let record = db
            .upsert_account(&super::super::AccountInput {
                platform: "workbuddy".to_string(),
                uid: "uid-growth".to_string(),
                ..Default::default()
            })
            .expect("account");
        record.id
    }

    fn run<'a>(
        account_id: &'a str,
        code: &'a str,
        action: &'a str,
        outcome: &'a str,
    ) -> NewGrowthRun<'a> {
        NewGrowthRun {
            account_id,
            task_code: code,
            action,
            outcome,
            credit: None,
            energy: None,
            error_kind: None,
            error_detail: None,
        }
    }

    #[test]
    fn v9_creates_the_runs_table() {
        let (db, _dir) = db();
        assert_eq!(db.schema_version().unwrap(), super::super::LATEST_VERSION);
        assert!(db.growth_runs("nobody", 10).unwrap().is_empty());
    }

    #[test]
    fn runs_are_scoped_per_account_and_newest_first() {
        let (mut db, _dir) = db();
        let a = account(&mut db);
        db.insert_growth_run(run(&a, "a1", ACTION_ACCEPT, OUTCOME_OK))
            .unwrap();
        db.insert_growth_run(run(&a, "a2", ACTION_CLAIM, OUTCOME_ALREADY))
            .unwrap();

        let rows = db.growth_runs(&a, 10).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].task_code, "a2", "最近的在前");

        assert!(db.growth_runs("someone-else", 10).unwrap().is_empty());
    }

    #[test]
    fn latest_per_task_keeps_one_row_each() {
        let (mut db, _dir) = db();
        let a = account(&mut db);
        db.insert_growth_run(run(&a, "same", ACTION_ACCEPT, OUTCOME_OK))
            .unwrap();
        db.insert_growth_run(run(&a, "same", ACTION_CLAIM, OUTCOME_OK))
            .unwrap();
        db.insert_growth_run(run(&a, "other", ACTION_ACCEPT, OUTCOME_REJECTED))
            .unwrap();

        let latest = db.latest_growth_run_per_task(&a).unwrap();
        assert_eq!(latest.len(), 2);
        let same = latest.iter().find(|r| r.task_code == "same").unwrap();
        assert_eq!(same.action, ACTION_CLAIM, "同一个任务只留最后一次");
    }

    #[test]
    fn totals_only_count_actually_granted_rewards() {
        let (mut db, _dir) = db();
        let a = account(&mut db);
        db.insert_growth_run(NewGrowthRun {
            account_id: &a,
            task_code: "t1",
            action: ACTION_CLAIM,
            outcome: OUTCOME_OK,
            credit: Some(100),
            energy: Some(5),
            error_kind: None,
            error_detail: None,
        })
        .unwrap();
        // 已领过：不能算进总数
        db.insert_growth_run(NewGrowthRun {
            account_id: &a,
            task_code: "t2",
            action: ACTION_CLAIM,
            outcome: OUTCOME_ALREADY,
            credit: Some(0),
            energy: Some(0),
            error_kind: None,
            error_detail: None,
        })
        .unwrap();
        // 接单不产生奖励
        db.insert_growth_run(run(&a, "t3", ACTION_ACCEPT, OUTCOME_OK))
            .unwrap();

        assert_eq!(db.growth_totals(&a).unwrap(), (100, 5));
    }

    #[test]
    fn deleting_an_account_cascades_its_runs() {
        let (mut db, _dir) = db();
        let a = account(&mut db);
        db.insert_growth_run(run(&a, "t1", ACTION_ACCEPT, OUTCOME_OK))
            .unwrap();
        db.delete_account(&a).unwrap();
        assert!(db.growth_runs(&a, 10).unwrap().is_empty());
    }
}
