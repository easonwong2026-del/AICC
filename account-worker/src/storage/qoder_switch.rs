//! 快照切换日志（v12）。
//!
//! 为什么需要一张表而不是内存变量：切换是"多个文件 + 多个步骤"的操作，
//! 进程在中途崩溃（或被用户强杀）后，重启时必须知道**上一次切换走到哪一步**，
//! 才能把 live 目录恢复成用户原来的账号。
//!
//! 状态机（`phase`）：
//!
//! ```text
//! prepared ──stage 全部就绪并校验──▶ staged ──rename 全部成功并校验──▶ committed
//!    │                                  │
//!    └────────── rollback ──────────────┴──▶ rolled_back
//! ```
//!
//! - `prepared` / `staged` 都算"未完成"，重启时一律按备份回滚；
//! - `committed` / `rolled_back` 是终态，恢复时只需清掉日志；
//! - 同一版本同时只允许一条未完成日志（唯一索引兜住并发）。

use rusqlite::{params, OptionalExtension};
use serde::Serialize;

use crate::clock::now_rfc3339;
use crate::rpc::{RpcError, RpcResult};

pub const PHASE_PREPARED: &str = "prepared";
pub const PHASE_STAGED: &str = "staged";
pub const PHASE_COMMITTED: &str = "committed";
pub const PHASE_ROLLED_BACK: &str = "rolled_back";
pub const PHASE_CLEARED: &str = "cleared";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchJournal {
    pub id: String,
    pub profile_id: String,
    pub edition: String,
    pub phase: String,
    pub live_dir: String,
    pub backup_dir: String,
    pub files: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl crate::storage::Db {
    /// 开一条切换日志。同一版本已有未完成日志时直接拒绝（并发保护）。
    pub fn begin_switch(&mut self, journal: &SwitchJournal) -> RpcResult<()> {
        let files_json = serde_json::to_string(&journal.files)
            .map_err(|e| RpcError::internal(format!("序列化文件清单失败：{e}")))?;
        let now = now_rfc3339();
        let result = self.conn.execute(
            "INSERT INTO qoder_switch_journal
                (id, profile_id, edition, phase, live_dir, backup_dir, files_json, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            params![
                journal.id,
                journal.profile_id,
                journal.edition,
                PHASE_PREPARED,
                journal.live_dir,
                journal.backup_dir,
                files_json,
                now,
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(err) if err.to_string().contains("UNIQUE") => Err(RpcError::invalid_params(
                "已有未完成的切换在进行，请先等它结束或重启 AICC 让它自动恢复",
            )),
            Err(err) => Err(RpcError::storage(err)),
        }
    }

    pub fn set_switch_phase(&mut self, id: &str, phase: &str) -> RpcResult<()> {
        if ![
            PHASE_PREPARED,
            PHASE_STAGED,
            PHASE_COMMITTED,
            PHASE_ROLLED_BACK,
            PHASE_CLEARED,
        ]
        .contains(&phase)
        {
            return Err(RpcError::invalid_params(format!("非法切换状态：{phase}")));
        }
        let changed = self.conn.execute(
            "UPDATE qoder_switch_journal SET phase = ?1, updated_at = ?2 WHERE id = ?3",
            params![phase, now_rfc3339(), id],
        )?;
        if changed == 0 {
            return Err(RpcError::not_found(format!("切换日志不存在：{id}")));
        }
        Ok(())
    }

    pub fn active_switches(&self, edition: Option<&str>) -> RpcResult<Vec<SwitchJournal>> {
        let mut sql = String::from(
            "SELECT id, profile_id, edition, phase, live_dir, backup_dir, files_json, created_at, updated_at
             FROM qoder_switch_journal WHERE phase IN ('prepared','staged')",
        );
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(edition) = edition.filter(|e| !e.is_empty()) {
            sql.push_str(" AND edition = ?");
            args.push(Box::new(edition.to_string()));
        }
        sql.push_str(" ORDER BY created_at ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), map_journal)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get_switch(&self, id: &str) -> RpcResult<Option<SwitchJournal>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, profile_id, edition, phase, live_dir, backup_dir, files_json, created_at, updated_at
             FROM qoder_switch_journal WHERE id = ?1",
        )?;
        Ok(stmt.query_row(params![id], map_journal).optional()?)
    }

    /// 终态日志直接删掉，别让表越积越大。
    pub fn finish_switch(&mut self, id: &str) -> RpcResult<()> {
        self.conn.execute(
            "DELETE FROM qoder_switch_journal WHERE id = ?1",
            params![id],
        )?;
        Ok(())
    }
}

fn map_journal(row: &rusqlite::Row<'_>) -> rusqlite::Result<SwitchJournal> {
    let files_json: String = row.get(6)?;
    Ok(SwitchJournal {
        id: row.get(0)?,
        profile_id: row.get(1)?,
        edition: row.get(2)?,
        phase: row.get(3)?,
        live_dir: row.get(4)?,
        backup_dir: row.get(5)?,
        files: serde_json::from_str(&files_json).unwrap_or_default(),
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    fn temp_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("accounts.sqlite")).unwrap();
        (db, dir)
    }

    fn journal(id: &str, edition: &str) -> SwitchJournal {
        SwitchJournal {
            id: id.into(),
            profile_id: "qp_1".into(),
            edition: edition.into(),
            phase: PHASE_PREPARED.into(),
            live_dir: "/live".into(),
            backup_dir: "/backup".into(),
            files: vec!["auth.v1.dat".into()],
            created_at: "t".into(),
            updated_at: "t".into(),
        }
    }

    #[test]
    fn second_active_switch_is_rejected() {
        let (mut db, _dir) = temp_db();
        db.begin_switch(&journal("s1", "qoder_global")).unwrap();
        let err = db.begin_switch(&journal("s2", "qoder_global")).unwrap_err();
        assert!(err.message.contains("未完成的切换"), "msg={}", err.message);
        // 另一个版本互不影响。
        db.begin_switch(&journal("s3", "qoder_cn")).unwrap();
    }

    #[test]
    fn finished_switch_frees_the_slot() {
        let (mut db, _dir) = temp_db();
        db.begin_switch(&journal("s1", "qoder_global")).unwrap();
        db.set_switch_phase("s1", PHASE_ROLLED_BACK).unwrap();
        db.finish_switch("s1").unwrap();
        db.begin_switch(&journal("s2", "qoder_global")).unwrap();
    }

    #[test]
    fn phase_transitions_and_active_query() {
        let (mut db, _dir) = temp_db();
        db.begin_switch(&journal("s1", "qoder_global")).unwrap();
        assert_eq!(db.active_switches(None).unwrap().len(), 1);
        db.set_switch_phase("s1", PHASE_STAGED).unwrap();
        assert_eq!(db.get_switch("s1").unwrap().unwrap().phase, PHASE_STAGED);
        db.set_switch_phase("s1", PHASE_COMMITTED).unwrap();
        assert!(db.active_switches(None).unwrap().is_empty());
    }

    #[test]
    fn rejects_unknown_phase() {
        let (mut db, _dir) = temp_db();
        db.begin_switch(&journal("s1", "qoder_global")).unwrap();
        assert!(db.set_switch_phase("s1", "weird").is_err());
    }
}
