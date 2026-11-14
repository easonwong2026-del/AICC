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
