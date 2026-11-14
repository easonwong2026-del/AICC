//! 额度快照缓存（v14）。
//!
//! 只做三件事：写一行、取最新一行、按时间范围取若干行。**不做聚合**：
//! 跨平台的单位不同，聚合必须由调用方按单位分组，存储层不替它决定。
//!
//! 保留期由调用方控制（`prune`），这里不设隐式 TTL——静默删数据比留垃圾更难查。

use serde_json::Value;

use crate::rpc::RpcResult;

#[derive(Debug, Clone)]
pub struct QuotaSnapshot {
    pub captured_at: String,
    pub provider: String,
    pub status: String,
    pub payload: String,
}

/// v15 修复步骤：v14 最初把 `account_id` 建成指向 `accounts(id)` 的外键，
/// 而 Qoder 服务账号在 `qoder_accounts` —— 一张"四平台通用"的缓存表不能这样建。
///
/// SQLite 改不了外键，只能重建表并搬数据。**新建库跑 v14 时已经无外键，
/// 这一步检测到没有外键就什么都不做**，所以对两类库都安全。
pub(super) fn rebuild_v15(tx: &rusqlite::Transaction<'_>) -> crate::rpc::RpcResult<()> {
    let foreign_keys: i64 = tx.query_row(
        "SELECT COUNT(*) FROM pragma_foreign_key_list('quota_snapshots')",
        [],
        |row| row.get(0),
    )?;
    if foreign_keys == 0 {
        return Ok(());
    }
    // 注意：这段是**多行字符串**，不要改成行尾反斜杠续行——那会把相邻两行
    // 粘成一个 token（`payloadSELECT`），SQL 直接语法错误。
    tx.execute_batch(
        "CREATE TABLE quota_snapshots_v15 (
             id          TEXT PRIMARY KEY,
             account_id  TEXT NOT NULL,
             provider    TEXT NOT NULL,
             captured_at TEXT NOT NULL,
             status      TEXT NOT NULL,
             payload     TEXT NOT NULL
         );
         INSERT INTO quota_snapshots_v15 (id, account_id, provider, captured_at, status, payload)
             SELECT id, account_id, provider, captured_at, status, payload FROM quota_snapshots;
         DROP TABLE quota_snapshots;
         ALTER TABLE quota_snapshots_v15 RENAME TO quota_snapshots;
         CREATE INDEX IF NOT EXISTS idx_quota_snapshots_account
             ON quota_snapshots (account_id, captured_at);",
    )?;
    Ok(())
}

impl crate::storage::Db {
    /// 落一行快照。`payload` 是整行 JSON，主键由（账号, 秒级时间）拼成，
    /// 同一账号同一秒重复抓取会覆盖而不是堆积。
    pub fn save_quota_snapshot(
        &mut self,
        account_id: &str,
        provider: &str,
        status: &str,
        payload: &Value,
    ) -> RpcResult<()> {
        let captured_at = crate::clock::now_rfc3339();
        let id = format!("{account_id}:{captured_at}");
        self.conn.execute(
            "INSERT OR REPLACE INTO quota_snapshots \
             (id, account_id, provider, captured_at, status, payload) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                id,
                account_id,
                provider,
                captured_at,
                status,
                payload.to_string()
            ],
        )?;
        Ok(())
    }

    /// 最新一行。界面默认路径：有缓存先用缓存，避免每次打开都打网络。
    pub fn latest_quota_snapshot(&self, account_id: &str) -> RpcResult<Option<QuotaSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT captured_at, provider, status, payload FROM quota_snapshots \
             WHERE account_id = ?1 ORDER BY captured_at DESC LIMIT 1",
        )?;
        let mut rows = stmt.query([account_id])?;
        match rows.next()? {
            Some(row) => Ok(Some(QuotaSnapshot {
                captured_at: row.get(0)?,
                provider: row.get(1)?,
                status: row.get(2)?,
                payload: row.get(3)?,
            })),
            None => Ok(None),
        }
    }

    /// `since` 之后的所有快照，按时间升序（画趋势图直接从旧到新）。
    pub fn quota_history(&self, account_id: &str, since: &str) -> RpcResult<Vec<QuotaSnapshot>> {
        let mut stmt = self.conn.prepare(
            "SELECT captured_at, provider, status, payload FROM quota_snapshots \
             WHERE account_id = ?1 AND captured_at >= ?2 ORDER BY captured_at ASC",
        )?;
        let mut rows = stmt.query(rusqlite::params![account_id, since])?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            out.push(QuotaSnapshot {
                captured_at: row.get(0)?,
                provider: row.get(1)?,
                status: row.get(2)?,
                payload: row.get(3)?,
            });
        }
        Ok(out)
    }

    /// 账号删除时清掉它的快照。没有外键级联，这一步必须显式做：
    /// 账号 id 是 uid 的确定性哈希，重新添加同一个账号会**复用同一个 id**，
    /// 残留快照会被当成"这个账号刚取过"。
    pub fn purge_account_quota_snapshots(&mut self, account_id: &str) -> RpcResult<usize> {
        let n = self.conn.execute(
            "DELETE FROM quota_snapshots WHERE account_id = ?1",
            [account_id],
        )?;
        Ok(n)
    }

    /// 删掉 `before` 之前的快照。调用方给时间点，存储层不猜保留期。
    pub fn prune_quota_snapshots(&mut self, before: &str) -> RpcResult<usize> {
        let n = self.conn.execute(
            "DELETE FROM quota_snapshots WHERE captured_at < ?1",
            [before],
        )?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use tempfile::tempdir;

    use crate::storage::{AccountInput, Db};

    fn account(uid: &str, provider: &str) -> AccountInput {
        AccountInput {
            platform: "workbuddy".into(),
            uid: uid.into(),
            nickname: None,
            email: None,
            group_id: None,
            auth_ref: None,
            profile_ref: None,
            auth_status: None,
            checkin_enabled: None,
            token_expires_at: None,
            auth_domain: None,
            auth_source: None,
            travel_enabled: None,
            provider: Some(provider.into()),
        }
    }

    /// 建库 + 一个账号，返回（临时目录须保活, 库, 账号 id）。
    fn fresh() -> (tempfile::TempDir, Db, String) {
        let dir = tempdir().unwrap();
        let mut db = Db::open(&dir.path().join("t.sqlite")).unwrap();
        db.migrate().unwrap();
        let record = db.upsert_account(&account("u1", "workbuddy")).unwrap();
        (dir, db, record.id)
    }

    #[test]
    fn latest_returns_the_newest_row() {
        let (_dir, mut db, id) = fresh();
        db.save_quota_snapshot(&id, "workbuddy", "ok", &json!({"total": 1}))
            .unwrap();
        let latest = db.latest_quota_snapshot(&id).unwrap().expect("有快照");
        assert_eq!(latest.status, "ok");
        assert!(latest.payload.contains("\"total\":1"));
    }

    #[test]
    fn history_is_scoped_to_the_account() {
        let (_dir, mut db, id) = fresh();
        let other = db.upsert_account(&account("u2", "workbuddy_ai")).unwrap();
        db.save_quota_snapshot(&id, "workbuddy", "ok", &json!({"total": 1}))
            .unwrap();
        db.save_quota_snapshot(&other.id, "workbuddy_ai", "ok", &json!({"total": 2}))
            .unwrap();
        let rows = db.quota_history(&id, "1970-01-01T00:00:00Z").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].provider, "workbuddy");
    }

    #[test]
    fn pruning_keeps_rows_after_the_cutoff() {
        let (_dir, mut db, id) = fresh();
        db.save_quota_snapshot(&id, "workbuddy", "ok", &json!({}))
            .unwrap();
        let removed = db.prune_quota_snapshots("2000-01-01T00:00:00Z").unwrap();
        assert_eq!(removed, 0);
        let removed = db.prune_quota_snapshots("2999-01-01T00:00:00Z").unwrap();
        assert_eq!(removed, 1);
    }

    /// 已经跑过**旧 v14**（带外键）的库升级到 v15 后：表结构无外键、旧快照一行不丢。
    #[test]
    fn v15_repairs_a_database_that_already_ran_the_old_v14() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("t.sqlite");
        let id = {
            let mut db = Db::open(&path).unwrap();
            db.migrate().unwrap();
            let record = db.upsert_account(&account("u1", "workbuddy")).unwrap();
            // 退回旧 v14 的形状：带 accounts 外键的表 + 一行数据 + user_version=14。
            db.conn
                .execute_batch(
                    "DROP TABLE quota_snapshots;
                     CREATE TABLE quota_snapshots (
                         id          TEXT PRIMARY KEY,
                         account_id  TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
                         provider    TEXT NOT NULL,
                         captured_at TEXT NOT NULL,
                         status      TEXT NOT NULL,
                         payload     TEXT NOT NULL
                     );
                     CREATE INDEX idx_quota_snapshots_account
                         ON quota_snapshots (account_id, captured_at);
                     PRAGMA user_version = 14;",
                )
                .unwrap();
            record.id
        };

        // 旧结构：外键确实存在。
        let legacy_fk: i64 = {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_list('quota_snapshots')",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(legacy_fk, 1, "前置条件：旧 v14 表带一条外键");

        // 插入一行旧数据后再升级。
        {
            let mut db = Db::open(&path).unwrap();
            // 先把 user_version 拉回 14 才会真的跑 v15（open 已自动迁移到 15，这里模拟"旧库"）。
            db.conn
                .execute(
                    "INSERT INTO quota_snapshots (id, account_id, provider, captured_at, status, payload)
                     VALUES ('legacy', ?1, 'workbuddy', '2026-11-14T00:00:00Z', 'ok', '{}')",
                    [&id],
                )
                .unwrap();
            db.conn.pragma_update(None, "user_version", 14).unwrap();
        }

        let mut db = Db::open(&path).unwrap();
        let version = db.migrate().unwrap();
        assert_eq!(version, crate::storage::LATEST_VERSION);

        let fk_after: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_list('quota_snapshots')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fk_after, 0, "v15 之后不应再有外键");

        let kept = db
            .latest_quota_snapshot(&id)
            .unwrap()
            .expect("旧快照要留着");
        assert_eq!(kept.status, "ok");
        // 索引也重建了，历史查询照常可用。
        assert_eq!(
            db.quota_history(&id, "1970-01-01T00:00:00Z").unwrap().len(),
            1
        );
    }

    /// 新库跑完 v14 就已经无外键，v15 必须是空操作而不是重建出错。
    #[test]
    fn v15_is_a_noop_on_a_fresh_database() {
        let (_dir, db, id) = fresh();
        assert_eq!(db.schema_version().unwrap(), crate::storage::LATEST_VERSION);
        let fk: i64 = db
            .conn
            .query_row(
                "SELECT COUNT(*) FROM pragma_foreign_key_list('quota_snapshots')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fk, 0);
        assert!(db.latest_quota_snapshot(&id).unwrap().is_none());
    }

    #[test]
    fn deleting_the_account_cascades_to_snapshots() {
        let (_dir, mut db, id) = fresh();
        db.save_quota_snapshot(&id, "workbuddy", "ok", &json!({}))
            .unwrap();
        assert!(db.delete_account(&id).unwrap());
        let n: i64 = db
            .conn
            .query_row("SELECT COUNT(*) FROM quota_snapshots", [], |row| row.get(0))
            .unwrap();
        assert_eq!(n, 0, "账号删掉后不应留下孤儿快照");
    }
}
