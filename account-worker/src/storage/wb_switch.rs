//! WorkBuddy **客户端**账号切换日志（v25）。
//!
//! 和 `qoder_switch` 不是一回事：那张表记的是 Qoder Profile 的文件组切换，
//! 这张表记的是"把 `workbuddy-desktop.info` 换成另一个账号并重启客户端"。
//!
//! ## 为什么一定要落库
//!
//! 切换的中间态是危险的：认证文件已经写成 B，客户端可能还没起来，或者起来了
//! 又把它改回去。AICC 进程本身也可能在这中间被杀掉（切换会关闭客户端，用户
//! 完全可能顺手把整个 App 也退了）。没有 journal 的话，下次启动只能看到
//! "当前登录是 B"，无从知道"本来应该是 A"，也就没法恢复。
//!
//! ## open_slot 的作用
//!
//! 同一时刻只允许一条未完成切换。`open_slot = 1` 表示未完成，完成后置 `NULL`。
//! SQLite 的唯一索引把 NULL 视为互不相等，所以历史行可以无限多，而"未完成"
//! 至多一条 —— 这条约束在数据库里，不依赖调用方记性好。

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use serde_json::Value;

use crate::clock::now_rfc3339;
use crate::rpc::{RpcError, RpcResult};

/// 快照已落盘、还没动客户端文件。
pub const PHASE_PREPARED: &str = "prepared";
/// 目标账号已写入客户端认证文件。
pub const PHASE_WRITTEN: &str = "written";
/// 客户端已（重新）启动，正在校验。
pub const PHASE_LAUNCHED: &str = "launched";
/// 客户端被确认采用了目标账号。
pub const PHASE_VERIFIED: &str = "verified";
/// 文件已写成目标账号，但没确认到客户端进程（例如启动很慢）。
///
/// **不是失败**：不回滚，交给用户点「刷新状态」再确认。
pub const PHASE_UNCONFIRMED: &str = "unconfirmed";
/// 失败且**未能**自动回滚（需要用户手动恢复）。
pub const PHASE_FAILED: &str = "failed";
/// 已恢复到切换前的登录态（自动回滚或用户手动恢复都算）。
pub const PHASE_RESTORED: &str = "restored";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchJournal {
    pub id: String,
    pub target_account_id: String,
    pub target_uid: String,
    pub previous_uid: Option<String>,
    pub previous_running: bool,
    pub previous_marker: bool,
    pub phase: String,
    pub snapshot_path: String,
    pub snapshot_sha256: String,
    pub token_form: Option<String>,
    /// 这次切换用的是哪份授权：`own` / `magpie`。v26 之前的记录为 NULL。
    pub auth_source: Option<String>,
    /// 是不是**临时**登录（Magpie access-only）。临时登录成功后仍要保留恢复入口。
    pub temporary: bool,
    /// 写进客户端的 access 到期时间（RFC3339）。只有 Magpie 模式必然有值。
    pub access_expires_at: Option<String>,
    /// access 的摘要（8 hex）。**只存摘要，不存令牌。**
    pub access_fingerprint: Option<String>,
    /// Magpie 来源键，便于事后核对"当时借的是哪一条"。
    pub source_key: Option<String>,
    /// `pending` / `done` / `superseded`。与 `open_slot` 无关：
    /// `open_slot` 管"切换正在执行"，这个管"临时授权还没还原"。
    pub restore_state: String,
    /// 服务端是否接受这份 access：`accepted` / `rejected` / `unknown`。
    pub auth_accepted: Option<String>,
    pub detail: Option<String>,
    pub activity_before: Option<Value>,
    pub activity_after: Option<Value>,
    pub created_at: String,
    pub updated_at: String,
}

/// 临时授权还没还原：要给界面一个持续的「恢复原账号」入口。
pub const RESTORE_PENDING: &str = "pending";
/// 已经还原过（或本来就回到了原账号）。
pub const RESTORE_DONE: &str = "done";

/// 新建一条切换记录。`open_slot` 恒为 1，由唯一索引挡住第二条。
pub struct NewSwitchJournal<'a> {
    pub id: &'a str,
    pub target_account_id: &'a str,
    pub target_uid: &'a str,
    pub previous_uid: Option<&'a str>,
    pub previous_running: bool,
    pub previous_marker: bool,
    pub snapshot_path: &'a str,
    pub snapshot_sha256: &'a str,
    pub token_form: Option<&'a str>,
    pub auth_source: &'a str,
    pub temporary: bool,
    pub access_expires_at: Option<&'a str>,
    pub access_fingerprint: Option<&'a str>,
    pub source_key: Option<&'a str>,
}

fn map_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SwitchJournal> {
    let activity_before: Option<String> = row.get("activity_before")?;
    let activity_after: Option<String> = row.get("activity_after")?;
    Ok(SwitchJournal {
        id: row.get("id")?,
        target_account_id: row.get("target_account_id")?,
        target_uid: row.get("target_uid")?,
        previous_uid: row.get("previous_uid")?,
        previous_running: row.get::<_, i64>("previous_running")? != 0,
        previous_marker: row.get::<_, i64>("previous_marker")? != 0,
        phase: row.get("phase")?,
        snapshot_path: row.get("snapshot_path")?,
        snapshot_sha256: row.get("snapshot_sha256")?,
        token_form: row.get("token_form")?,
        auth_source: row.get("auth_source")?,
        temporary: row.get::<_, i64>("temporary")? != 0,
        access_expires_at: row.get("access_expires_at")?,
        access_fingerprint: row.get("access_fingerprint")?,
        source_key: row.get("source_key")?,
        restore_state: row
            .get::<_, Option<String>>("restore_state")?
            .unwrap_or_else(|| RESTORE_PENDING.to_string()),
        auth_accepted: row.get("auth_accepted")?,
        detail: row.get("detail")?,
        // 证据字段解析不出来不该让整行读不出来：坏 JSON 当没记过。
        activity_before: activity_before.and_then(|text| serde_json::from_str(&text).ok()),
        activity_after: activity_after.and_then(|text| serde_json::from_str(&text).ok()),
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

fn is_unique_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _)
            if e.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

/// v26：临时授权（Magpie access-only）要记的东西。
///
/// 逐列 `ADD COLUMN`，每一列都先查 `table_info`：迁移事务已提交但版本号没写下去时，
/// 重跑必须不炸在 `duplicate column name` 上。
pub fn migrate_v26(tx: &rusqlite::Transaction<'_>) -> RpcResult<()> {
    const COLUMNS: &[(&str, &str)] = &[
        ("auth_source", "TEXT"),
        ("temporary", "INTEGER NOT NULL DEFAULT 0"),
        ("access_expires_at", "TEXT"),
        ("access_fingerprint", "TEXT"),
        ("source_key", "TEXT"),
        ("restore_state", "TEXT NOT NULL DEFAULT 'pending'"),
        ("auth_accepted", "TEXT"),
    ];

    let mut stmt = tx.prepare("PRAGMA table_info(workbuddy_switch_journal)")?;
    let existing: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(stmt);

    for (name, decl) in COLUMNS {
        if existing.iter().any(|column| column == name) {
            continue;
        }
        tx.execute(
            &format!("ALTER TABLE workbuddy_switch_journal ADD COLUMN {name} {decl}"),
            [],
        )?;
    }

    tx.execute(
        "CREATE INDEX IF NOT EXISTS idx_wb_switch_temporary
            ON workbuddy_switch_journal (temporary, restore_state)",
        [],
    )?;
    Ok(())
}

impl super::Db {
    /// 开一条切换记录。已有未完成切换时返回 `switch_busy`，**不覆盖**那一条。
    pub fn begin_wb_switch(&self, new: &NewSwitchJournal<'_>) -> RpcResult<()> {
        let now = now_rfc3339();
        let result = self.conn.execute(
            "INSERT INTO workbuddy_switch_journal (
                 id, target_account_id, target_uid, previous_uid, previous_running,
                 previous_marker, phase, open_slot, snapshot_path, snapshot_sha256,
                 token_form, auth_source, temporary, access_expires_at,
                 access_fingerprint, source_key, restore_state, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15,
                       'pending', ?16, ?16)",
            params![
                new.id,
                new.target_account_id,
                new.target_uid,
                new.previous_uid,
                i64::from(new.previous_running),
                i64::from(new.previous_marker),
                PHASE_PREPARED,
                new.snapshot_path,
                new.snapshot_sha256,
                new.token_form,
                new.auth_source,
                i64::from(new.temporary),
                new.access_expires_at,
                new.access_fingerprint,
                new.source_key,
                now,
            ],
        );
        match result {
            Ok(_) => Ok(()),
            Err(err) if is_unique_violation(&err) => Err(RpcError::new(
                crate::rpc::E_SWITCH,
                "已有一条未完成的账号切换，先恢复或等它结束",
            )
            .with_data(serde_json::json!({ "kind": "switch_busy" }))),
            Err(err) => Err(RpcError::storage(err)),
        }
    }

    /// 推进阶段。**不动 `open_slot`**：只有 `close_switch_journal` 才释放。
    pub fn set_wb_switch_phase(
        &self,
        id: &str,
        phase: &str,
        detail: Option<&str>,
    ) -> RpcResult<()> {
        let changed = self.conn.execute(
            "UPDATE workbuddy_switch_journal
                SET phase = ?2, detail = COALESCE(?3, detail), updated_at = ?4
              WHERE id = ?1",
            params![id, phase, detail, now_rfc3339()],
        )?;
        if changed == 0 {
            return Err(RpcError::not_found(format!("切换记录不存在：{id}")));
        }
        Ok(())
    }

    /// 记一次活跃证据。字段名固定，方便 `switch` 与事后回查对齐。
    pub fn set_wb_switch_activity(
        &self,
        id: &str,
        before: Option<&Value>,
        after: Option<&Value>,
    ) -> RpcResult<()> {
        let encode = |value: Option<&Value>| -> Option<String> {
            value.and_then(|v| serde_json::to_string(v).ok())
        };
        let changed = self.conn.execute(
            "UPDATE workbuddy_switch_journal
                SET activity_before = COALESCE(?2, activity_before),
                    activity_after  = COALESCE(?3, activity_after),
                    updated_at = ?4
              WHERE id = ?1",
            params![id, encode(before), encode(after), now_rfc3339()],
        )?;
        if changed == 0 {
            return Err(RpcError::not_found(format!("切换记录不存在：{id}")));
        }
        Ok(())
    }

    /// 收尾：写终态并释放 `open_slot`。重复调用是安全的（已释放时 `open_slot` 仍是 NULL）。
    pub fn close_wb_switch(&self, id: &str, phase: &str, detail: Option<&str>) -> RpcResult<()> {
        let changed = self.conn.execute(
            "UPDATE workbuddy_switch_journal
                SET phase = ?2, detail = COALESCE(?3, detail), open_slot = NULL, updated_at = ?4
              WHERE id = ?1",
            params![id, phase, detail, now_rfc3339()],
        )?;
        if changed == 0 {
            return Err(RpcError::not_found(format!("切换记录不存在：{id}")));
        }
        Ok(())
    }

    pub fn wb_switch_journal(&self, id: &str) -> RpcResult<Option<SwitchJournal>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM workbuddy_switch_journal WHERE id = ?1",
                params![id],
                map_row,
            )
            .optional()?)
    }

    /// 当前未完成的那一条（至多一条，由唯一索引保证）。
    pub fn open_wb_switch(&self) -> RpcResult<Option<SwitchJournal>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM workbuddy_switch_journal WHERE open_slot IS NOT NULL",
                [],
                map_row,
            )
            .optional()?)
    }

    pub fn latest_wb_switch(&self) -> RpcResult<Option<SwitchJournal>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM workbuddy_switch_journal ORDER BY created_at DESC, rowid DESC LIMIT 1",
                [],
                map_row,
            )
            .optional()?)
    }

    /// 还没还原的**临时**登录（Magpie access-only）。
    ///
    /// 与 `open_slot` 刻意分开：切换早就结束了，但这份临时登录还在生效，
    /// 必须一直保留"能回到原账号"的记录。
    pub fn pending_temporary_restore(&self) -> RpcResult<Option<SwitchJournal>> {
        Ok(self
            .conn
            .query_row(
                "SELECT * FROM workbuddy_switch_journal
                  WHERE temporary = 1 AND restore_state = ?1
                  ORDER BY created_at DESC, rowid DESC LIMIT 1",
                params![RESTORE_PENDING],
                map_row,
            )
            .optional()?)
    }

    /// 改恢复状态。`done` 表示"这条不再提供恢复"。
    pub fn set_restore_state(&self, id: &str, state: &str) -> RpcResult<()> {
        let changed = self.conn.execute(
            "UPDATE workbuddy_switch_journal
                SET restore_state = ?2, updated_at = ?3
              WHERE id = ?1",
            params![id, state, now_rfc3339()],
        )?;
        if changed == 0 {
            return Err(RpcError::not_found(format!("切换记录不存在：{id}")));
        }
        Ok(())
    }

    /// 记服务端验证结论：`accepted` / `rejected` / `unknown`。
    pub fn set_switch_auth_accepted(&self, id: &str, outcome: &str) -> RpcResult<()> {
        self.conn.execute(
            "UPDATE workbuddy_switch_journal
                SET auth_accepted = ?2, updated_at = ?3
              WHERE id = ?1",
            params![id, outcome, now_rfc3339()],
        )?;
        Ok(())
    }

    pub fn wb_switch_journals(&self, limit: u32) -> RpcResult<Vec<SwitchJournal>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM workbuddy_switch_journal ORDER BY created_at DESC, rowid DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit.max(1)], map_row)?;
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

    fn temp_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut db = Db::open(&dir.path().join("accounts.sqlite")).expect("open");
        db.migrate().expect("migrate");
        (dir, db)
    }

    fn new_journal<'a>(id: &'a str, target: &'a str) -> NewSwitchJournal<'a> {
        NewSwitchJournal {
            id,
            target_account_id: "acc-b",
            target_uid: target,
            previous_uid: Some("uid-a"),
            previous_running: true,
            previous_marker: false,
            snapshot_path: "/tmp/snap.json",
            snapshot_sha256: "deadbeef",
            token_form: Some("plaintext"),
            auth_source: "own",
            temporary: false,
            access_expires_at: None,
            access_fingerprint: None,
            source_key: None,
        }
    }

    /// v26 必须能重跑：迁移事务已提交、版本号没写下去时不能炸在 `duplicate column`。
    #[test]
    fn v26_migration_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = rusqlite::Connection::open(dir.path().join("accounts.sqlite")).unwrap();
        conn.execute_batch(super::super::MIGRATION_V25_SQL).unwrap();
        for _ in 0..2 {
            let tx = conn.transaction().unwrap();
            migrate_v26(&tx).unwrap();
            tx.commit().unwrap();
        }

        let mut stmt = conn
            .prepare("PRAGMA table_info(workbuddy_switch_journal)")
            .unwrap();
        let columns: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        for expected in [
            "auth_source",
            "temporary",
            "access_expires_at",
            "access_fingerprint",
            "source_key",
            "restore_state",
            "auth_accepted",
        ] {
            assert!(columns.iter().any(|c| c == expected), "缺列 {expected}");
        }
    }

    /// 同一时刻只允许一条未完成切换：第二条必须被拒绝，且**不覆盖**第一条。
    #[test]
    fn only_one_open_switch_is_allowed() {
        let (_db, db) = temp_db();
        db.begin_wb_switch(&new_journal("j-1", "uid-b")).unwrap();

        let err = db
            .begin_wb_switch(&new_journal("j-2", "uid-c"))
            .unwrap_err();
        assert_eq!(err.code, crate::rpc::E_SWITCH);

        let open = db.open_wb_switch().unwrap().expect("第一条仍应未完成");
        assert_eq!(open.id, "j-1");
        assert_eq!(open.phase, PHASE_PREPARED);
    }

    /// 收尾后可以再开一条；历史保留。
    #[test]
    fn closing_releases_the_slot_and_keeps_history() {
        let (_db, db) = temp_db();
        db.begin_wb_switch(&new_journal("j-1", "uid-b")).unwrap();
        db.set_wb_switch_phase("j-1", PHASE_WRITTEN, None).unwrap();
        db.close_wb_switch("j-1", PHASE_RESTORED, Some("已回滚"))
            .unwrap();

        assert!(db.open_wb_switch().unwrap().is_none());
        db.begin_wb_switch(&new_journal("j-2", "uid-c")).unwrap();

        let history = db.wb_switch_journals(10).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].id, "j-2", "最近的排在最前");
        assert_eq!(history[1].phase, PHASE_RESTORED);
        assert_eq!(history[1].detail.as_deref(), Some("已回滚"));
    }

    /// 活跃证据：写进去能原样读出来；坏 JSON 不影响整行可读。
    #[test]
    fn activity_evidence_round_trips() {
        let (_db, db) = temp_db();
        db.begin_wb_switch(&new_journal("j-1", "uid-b")).unwrap();
        db.set_wb_switch_activity(
            "j-1",
            Some(&serde_json::json!({"todayActive": false, "streakDays": 0})),
            Some(&serde_json::json!({"todayActive": true, "streakDays": 1})),
        )
        .unwrap();

        let row = db.wb_switch_journal("j-1").unwrap().unwrap();
        assert_eq!(row.activity_before.as_ref().unwrap()["todayActive"], false);
        assert_eq!(row.activity_after.as_ref().unwrap()["todayActive"], true);
    }
}
