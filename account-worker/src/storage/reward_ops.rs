//! 奖励操作账本（v24）：兑换 / 抽奖 / 补签的**每次尝试**都要先落盘。
//!
//! 策略在 `growth::reward_ledger`，这里只负责"记住"。表的意义是活过崩溃与并发：
//!
//! | 问题 | 这张表怎么回答 |
//! | --- | --- |
//! | 这次操作发过了吗？ | 有没有 `phase='sent'` 的记录 |
//! | 重发该带哪个 `client_token`？ | 记录里那一列 |
//! | 两个进程会不会各抽一次？ | 部分唯一索引：同一账号同一目标只允许一条未决记录 |
//! | 上次到底成没成？ | `phase='unknown'` + `server_state`（回查结果） |
//!
//! ## 跨进程保护的三层
//!
//! 1. **部分唯一索引** `ux_reward_ops_open`：同一 `(account, action, target)` 同时只能有
//!    一条 `pending/sent/unknown/frozen`。这层是物理约束，绕过代码也挡得住。
//! 2. **`BEGIN IMMEDIATE`**：`open_reward_op` 的"先查再插"在一次**写事务**里完成。
//!    连接已开 WAL 且 `busy_timeout=5s`（见 `Db::open`），另一个进程只会等锁，
//!    不会读到"还没有"然后一起插。
//! 3. **token 唯一索引** `ux_reward_ops_token`：同一个 token 不能落在两条记录上。
//!
//! 关了唯一索引也就关了幂等：所以测试里专门有一条"绕过代码直接插第二条"的用例。

#![allow(dead_code)] // #96/#97 接线；账本先落地，避免接线时临时加"就这一次"的例外。

use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::Serialize;

use crate::clock::now_rfc3339;
use crate::rpc::{RpcError, RpcResult};

use super::Db;

/// 一个目标同时只允许一条未决记录。SQL 里重复写了一遍，改动时同步两处。
pub const OPEN_PHASES_SQL: &str = "'pending','sent','unknown','frozen'";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RewardOp {
    pub id: String,
    pub account_id: String,
    pub action: String,
    pub target: String,
    pub client_token: Option<String>,
    pub phase: String,
    pub attempt: i64,
    /// agent 判定名（`granted` / `no_chance` / `unknown` …），排查时按它分组。
    pub outcome: Option<String>,
    /// 到账数值 JSON（credits / energy / cards / chances / prize）。
    pub grant_json: Option<String>,
    /// 回查到的服务端状态快照。
    pub server_state: Option<String>,
    pub detail: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub verified_at: Option<String>,
}

impl RewardOp {
    pub fn is_open(&self) -> bool {
        matches!(
            self.phase.as_str(),
            "pending" | "sent" | "unknown" | "frozen"
        )
    }
}

/// 新建一条记录时给进去的东西。`client_token` 补签时为 `None`。
pub struct NewRewardOp<'a> {
    pub account_id: &'a str,
    pub action: &'a str,
    pub target: &'a str,
    pub client_token: Option<&'a str>,
}

/// `open_reward_op` 的两种结果。拿到 `Existing` 的调用方**必须**改走回查，
/// 不能认为"我又拿到了一次机会"。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opened {
    Created(RewardOp),
    Existing(RewardOp),
}

impl Opened {
    pub fn op(&self) -> &RewardOp {
        match self {
            Self::Created(op) | Self::Existing(op) => op,
        }
    }

    pub fn was_created(&self) -> bool {
        matches!(self, Self::Created(_))
    }
}

/// 结束一条记录。`phase` 只接受终态或 `unknown` / `frozen`。
pub struct Resolution<'a> {
    pub phase: &'a str,
    pub outcome: &'a str,
    pub grant_json: Option<&'a str>,
    pub server_state: Option<&'a str>,
    pub detail: Option<&'a str>,
    /// 这一条结论是否经过服务端回查。
    pub verified: bool,
}

const COLUMNS: &str = "id, account_id, action, target, client_token, phase, attempt, outcome,
    grant_json, server_state, detail, created_at, updated_at, verified_at";

fn map_op(row: &rusqlite::Row<'_>) -> rusqlite::Result<RewardOp> {
    Ok(RewardOp {
        id: row.get(0)?,
        account_id: row.get(1)?,
        action: row.get(2)?,
        target: row.get(3)?,
        client_token: row.get(4)?,
        phase: row.get(5)?,
        attempt: row.get(6)?,
        outcome: row.get(7)?,
        grant_json: row.get(8)?,
        server_state: row.get(9)?,
        detail: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
        verified_at: row.get(13)?,
    })
}

fn closed_phase(phase: &str) -> bool {
    matches!(
        phase,
        "confirmed" | "rejected" | "failed" | "unknown" | "frozen"
    )
}

impl Db {
    /// 取（或在同一个写事务里新建）一条未决记录。
    ///
    /// `BEGIN IMMEDIATE` 是这里的核心：先读后写在事务里，两个进程不会都看到"没有"。
    /// 如果已经有一条未决记录，返回 `Opened::Existing` —— 调用方必须回查服务端。
    pub fn open_reward_op(&mut self, new: &NewRewardOp<'_>) -> RpcResult<Opened> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;

        let existing = tx
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM reward_operations
                      WHERE account_id = ?1 AND action = ?2 AND target = ?3
                        AND phase IN ({OPEN_PHASES_SQL})
                      ORDER BY created_at DESC LIMIT 1"
                ),
                params![new.account_id, new.action, new.target],
                map_op,
            )
            .optional()?;
        if let Some(op) = existing {
            tx.commit()?;
            return Ok(Opened::Existing(op));
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = now_rfc3339();
        tx.execute(
            "INSERT INTO reward_operations
                (id, account_id, action, target, client_token, phase, attempt,
                 outcome, grant_json, server_state, detail, created_at, updated_at, verified_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'pending', 0, NULL, NULL, NULL, NULL, ?6, ?6, NULL)",
            params![
                id,
                new.account_id,
                new.action,
                new.target,
                new.client_token,
                now
            ],
        )?;
        let op = tx.query_row(
            &format!("SELECT {COLUMNS} FROM reward_operations WHERE id = ?1"),
            params![id],
            map_op,
        )?;
        tx.commit()?;
        Ok(Opened::Created(op))
    }

    /// 记账"请求即将发出"。**必须在真正发请求之前调用**：
    /// 崩在中间最多是"多回查一次"，反过来就是"可能重复消耗"。
    ///
    /// 返回这一次的尝试序号（从 1 开始）。
    pub fn mark_reward_op_sent(&mut self, id: &str) -> RpcResult<i64> {
        let now = now_rfc3339();
        let changed = self.conn.execute(
            "UPDATE reward_operations
                SET phase = 'sent', attempt = attempt + 1, updated_at = ?2
              WHERE id = ?1 AND phase = 'pending'",
            params![id, now],
        )?;
        if changed != 1 {
            // 状态已经不是 pending：说明另一个进程/另一轮已经动过它。不发。
            return Err(RpcError::internal("这条操作不在待发送状态，已停止发送"));
        }
        let attempt: i64 = self.conn.query_row(
            "SELECT attempt FROM reward_operations WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(attempt)
    }

    /// 落结论。只允许改未决记录 —— 已经结束的记录不能被后来的回查覆盖。
    pub fn resolve_reward_op(&mut self, id: &str, res: &Resolution<'_>) -> RpcResult<()> {
        if !closed_phase(res.phase) {
            return Err(RpcError::new(
                crate::rpc::INVALID_PARAMS,
                format!("不是可以落盘的结束状态：{}", res.phase),
            ));
        }
        let now = now_rfc3339();
        let verified_at = res.verified.then(|| now.clone());
        let changed = self.conn.execute(
            "UPDATE reward_operations
                SET phase = ?2, outcome = ?3, grant_json = ?4, server_state = ?5,
                    detail = ?6, updated_at = ?7, verified_at = ?8
              WHERE id = ?1 AND phase IN ('pending','sent','unknown','frozen')",
            params![
                id,
                res.phase,
                res.outcome,
                res.grant_json,
                res.server_state,
                res.detail,
                now,
                verified_at
            ],
        )?;
        if changed != 1 {
            return Err(RpcError::internal("这条操作已经结束，结论不再覆盖"));
        }
        Ok(())
    }

    pub fn reward_op(&self, id: &str) -> RpcResult<Option<RewardOp>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {COLUMNS} FROM reward_operations WHERE id = ?1"),
                params![id],
                map_op,
            )
            .optional()?)
    }

    /// 某个目标当前的未决记录（`plan` 的输入）。
    pub fn open_reward_op_for(
        &self,
        account_id: &str,
        action: &str,
        target: &str,
    ) -> RpcResult<Option<RewardOp>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM reward_operations
                      WHERE account_id = ?1 AND action = ?2 AND target = ?3
                        AND phase IN ({OPEN_PHASES_SQL})
                      ORDER BY created_at DESC LIMIT 1"
                ),
                params![account_id, action, target],
                map_op,
            )
            .optional()?)
    }

    /// 需要回查的记录（`sent` / `unknown`）。#97 的恢复流程用它决定"今天先查谁"。
    pub fn reward_ops_awaiting_verification(&self, account_id: &str) -> RpcResult<Vec<RewardOp>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM reward_operations
              WHERE account_id = ?1 AND phase IN ('sent','unknown')
              ORDER BY created_at ASC"
        ))?;
        let rows = stmt.query_map(params![account_id], map_op)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 最近的操作记录（界面上的"执行历史"）。
    pub fn reward_ops(&self, account_id: &str, limit: u32) -> RpcResult<Vec<RewardOp>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {COLUMNS} FROM reward_operations
              WHERE account_id = ?1 ORDER BY created_at DESC, rowid DESC LIMIT ?2"
        ))?;
        let rows = stmt.query_map(params![account_id, limit], map_op)?;
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
    use crate::growth::reward_ledger::{self, Next};
    use std::path::PathBuf;

    fn temp_db() -> (Db, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db = Db::open(&dir.path().join("accounts.sqlite")).expect("open");
        (db, dir)
    }

    fn new_op<'a>(account: &'a str, token: Option<&'a str>) -> NewRewardOp<'a> {
        NewRewardOp {
            account_id: account,
            action: "lottery_draw",
            target: "",
            client_token: token,
        }
    }

    fn conclude(phase: &'static str) -> Resolution<'static> {
        Resolution {
            phase,
            outcome: "granted",
            grant_json: Some("{\"credits\":1}"),
            server_state: Some("chances=0"),
            detail: None,
            verified: true,
        }
    }

    #[test]
    fn a_new_operation_is_recorded_before_anything_is_sent() {
        let (mut db, _dir) = temp_db();
        let opened = db.open_reward_op(&new_op("acc", Some("u-1"))).unwrap();
        assert!(opened.was_created());
        let op = opened.op();
        assert_eq!(op.phase, "pending");
        assert_eq!(op.attempt, 0);
        assert_eq!(op.client_token.as_deref(), Some("u-1"));
        assert!(op.is_open());
        assert!(op.verified_at.is_none());
    }

    #[test]
    fn opening_twice_returns_the_existing_record_and_keeps_the_same_token() {
        let (mut db, _dir) = temp_db();
        let first = db.open_reward_op(&new_op("acc", Some("u-1"))).unwrap();
        let second = db.open_reward_op(&new_op("acc", Some("u-999"))).unwrap();
        assert!(!second.was_created(), "第二次不能再建一条");
        assert_eq!(second.op().id, first.op().id);
        assert_eq!(
            second.op().client_token.as_deref(),
            Some("u-1"),
            "幂等键必须是第一次那个，不能被新生成的覆盖"
        );
    }

    #[test]
    fn only_one_open_operation_per_account_and_target() {
        let (mut db, _dir) = temp_db();
        db.open_reward_op(&new_op("acc", Some("u-1"))).unwrap();
        // 另一个账号不受影响。
        db.open_reward_op(&new_op("other", Some("u-2"))).unwrap();
        // 补签按日期区分，不同日期是不同目标。
        for date in ["2026-11-09", "2026-11-10"] {
            db.open_reward_op(&NewRewardOp {
                account_id: "acc",
                action: "makeup_card",
                target: date,
                client_token: None,
            })
            .unwrap();
        }
        assert!(db
            .open_reward_op_for("acc", "lottery_draw", "")
            .unwrap()
            .is_some());
        assert!(db
            .open_reward_op_for("other", "lottery_draw", "")
            .unwrap()
            .is_some());
        for date in ["2026-11-09", "2026-11-10"] {
            assert!(db
                .open_reward_op_for("acc", "makeup_card", date)
                .unwrap()
                .is_some());
        }
    }

    /// 这一条盯的是"绕过代码也挡得住"：直接 INSERT 第二条未决记录必须失败。
    #[test]
    fn the_database_itself_refuses_a_second_open_row() {
        let (mut db, _dir) = temp_db();
        db.open_reward_op(&new_op("acc", Some("u-1"))).unwrap();
        let direct = db.exec_raw(
            "INSERT INTO reward_operations
                (id, account_id, action, target, client_token, phase, attempt, created_at, updated_at)
             VALUES ('raw', 'acc', 'lottery_draw', '', 'u-2', 'pending', 0,
                     '2026-11-16T00:00:00Z', '2026-11-16T00:00:00Z')",
        );
        assert!(direct.is_err(), "唯一索引没生效，并发就靠不住了");
    }

    #[test]
    fn a_closed_operation_frees_the_target_and_drops_out_of_the_open_query() {
        let (mut db, _dir) = temp_db();
        let opened = db.open_reward_op(&new_op("acc", Some("u-1"))).unwrap();
        let id = opened.op().id.clone();
        db.mark_reward_op_sent(&id).unwrap();
        db.resolve_reward_op(&id, &conclude("confirmed")).unwrap();

        assert!(db
            .open_reward_op_for("acc", "lottery_draw", "")
            .unwrap()
            .is_none());
        let fresh = db.open_reward_op(&new_op("acc", Some("u-2"))).unwrap();
        assert!(fresh.was_created(), "上一次结束了，下一次是新操作");
        assert_ne!(fresh.op().id, id);
    }

    #[test]
    fn marking_sent_twice_is_refused_so_a_request_is_never_double_counted() {
        let (mut db, _dir) = temp_db();
        let opened = db.open_reward_op(&new_op("acc", Some("u-1"))).unwrap();
        let id = opened.op().id.clone();
        assert_eq!(db.mark_reward_op_sent(&id).unwrap(), 1);
        assert!(db.mark_reward_op_sent(&id).is_err(), "同一轮不能记两次发送");
    }

    #[test]
    fn a_closed_operation_is_never_overwritten_by_a_late_verification() {
        let (mut db, _dir) = temp_db();
        let opened = db.open_reward_op(&new_op("acc", Some("u-1"))).unwrap();
        let id = opened.op().id.clone();
        db.mark_reward_op_sent(&id).unwrap();
        db.resolve_reward_op(&id, &conclude("confirmed")).unwrap();

        let retry = db.resolve_reward_op(&id, &conclude("rejected"));
        assert!(retry.is_err(), "结论不能被后来的回查改写");
        assert_eq!(db.reward_op(&id).unwrap().unwrap().phase, "confirmed");
    }

    #[test]
    fn only_closing_phases_can_be_resolved_into() {
        let (mut db, _dir) = temp_db();
        let opened = db.open_reward_op(&new_op("acc", Some("u-1"))).unwrap();
        let id = opened.op().id.clone();
        let mut res = conclude("sent");
        assert!(
            db.resolve_reward_op(&id, &res).is_err(),
            "不能把结论落成 sent（它不是一个结束状态）"
        );
        res.phase = "unknown";
        assert!(
            db.resolve_reward_op(&id, &res).is_ok(),
            "unknown 是一个合法结论"
        );
    }

    #[test]
    fn a_timeout_leaves_the_operation_awaiting_verification() {
        let (mut db, _dir) = temp_db();
        let opened = db.open_reward_op(&new_op("acc", Some("u-1"))).unwrap();
        let id = opened.op().id.clone();
        db.mark_reward_op_sent(&id).unwrap();
        db.resolve_reward_op(
            &id,
            &Resolution {
                phase: "unknown",
                outcome: "indeterminate",
                grant_json: None,
                server_state: None,
                detail: Some("请求超时（15s）"),
                verified: false,
            },
        )
        .unwrap();

        let pending = db.reward_ops_awaiting_verification("acc").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].outcome.as_deref(), Some("indeterminate"));
        assert!(pending[0].verified_at.is_none(), "还没回查过");
    }

    /// 崩溃恢复：进程 A 发了请求就没了；进程 B 起来必须**回查**，而且要用同一个 token。
    #[test]
    fn a_restart_reuses_the_persisted_token_and_asks_for_a_recheck() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path: PathBuf = dir.path().join("accounts.sqlite");

        let token = {
            let mut first = Db::open(&path).expect("open");
            let opened = first
                .open_reward_op(&new_op("acc", Some("u-persisted")))
                .unwrap();
            first.mark_reward_op_sent(&opened.op().id).unwrap();
            opened.op().client_token.clone().expect("token")
            // first 在这里被 drop，等价于进程退出
        };

        let second = Db::open(&path).expect("reopen");
        let open = second
            .open_reward_op_for("acc", "lottery_draw", "")
            .expect("query")
            .expect("未决记录必须还在");
        assert_eq!(open.phase, "sent");
        let view = reward_ledger::Open::from(&open);
        match reward_ledger::plan(Some(&view), true) {
            Next::Recheck { .. } => {}
            other => panic!("重启后必须先回查，实际是 {other:?}"),
        }
        assert_eq!(
            view.client_token.as_deref(),
            Some(token.as_str()),
            "回查/重发都必须用落盘的那个 token"
        );
        assert!(!matches!(
            reward_ledger::plan(Some(&view), true),
            Next::Insert { .. }
        ));
    }

    /// 两个连接（等价两个进程）同时开同一条：只能有一个 Created。
    #[test]
    fn two_connections_cannot_both_create_the_same_operation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("accounts.sqlite");
        // 先把库建好，避免两个线程同时跑迁移。
        drop(Db::open(&path).expect("open"));

        let mut handles = Vec::new();
        for index in 0..2 {
            let path = path.clone();
            handles.push(std::thread::spawn(move || {
                let mut db = Db::open(&path).expect("open");
                let token = format!("u-{index}");
                let opened = db.open_reward_op(&new_op("acc", Some(&token))).unwrap();
                opened.was_created()
            }));
        }
        let results: Vec<bool> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            results.iter().filter(|created| **created).count(),
            1,
            "两个进程都建了一条：{results:?}"
        );

        let db = Db::open(&path).expect("open");
        assert_eq!(db.reward_ops("acc", 10).unwrap().len(), 1);
    }

    #[test]
    fn history_is_scoped_to_one_account_and_newest_first() {
        let (mut db, _dir) = temp_db();
        for (account, token) in [("a", "u-a1"), ("b", "u-b1")] {
            let opened = db.open_reward_op(&new_op(account, Some(token))).unwrap();
            let id = opened.op().id.clone();
            db.mark_reward_op_sent(&id).unwrap();
            db.resolve_reward_op(&id, &conclude("confirmed")).unwrap();
        }
        let rows = db.reward_ops("a", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].account_id, "a");
        assert_eq!(rows[0].phase, "confirmed");
        assert_eq!(rows[0].outcome.as_deref(), Some("granted"));
        assert!(db.reward_ops("nobody", 10).unwrap().is_empty());
    }
}
