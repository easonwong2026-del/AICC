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
use serde_json::Value;

use crate::clock::now_rfc3339;
use crate::rpc::{RpcError, RpcResult};

use super::Db;

/// 一个目标同时只允许一条未决记录。SQL 里重复写了一遍，改动时同步两处。
pub const OPEN_PHASES_SQL: &str = "'pending','ready','sent','unknown','frozen'";

/// 同一个目标最多发三次。
///
/// **这个数字在这里，不在调用方**：`plan()` 只是提前劝一句，真正拦住"第四次发送"的
/// 是 [`Db::mark_reward_op_sent`] 里那条 `AND attempt < ?3` 的原子更新 —— 调用方
/// 绕过 `plan()` 也发不出去（PR #96）。
pub const MAX_SEND_ATTEMPTS: i64 = 3;

/// `detail` 的字符上限（`rpc::sanitize` 之后再截）。
pub const MAX_DETAIL_CHARS: usize = 200;
/// 结构化回执的字节上限。超过就丢字段，绝不留一整份服务端响应。
pub const MAX_PAYLOAD_BYTES: usize = 512;
/// 结构化回执最多保留几个字段。
pub const MAX_PAYLOAD_KEYS: usize = 16;
/// 单个字符串值最多保留几个字符。
pub const MAX_PAYLOAD_VALUE_CHARS: usize = 64;

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
            "pending" | "ready" | "sent" | "unknown" | "frozen"
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

/// 结束一条记录。
///
/// `phase` 只接受终态（`confirmed`/`rejected`）和"继续占着"的 `unknown`/`frozen`，
/// 外加**回查确认过**的 `failed`。
///
/// `grant` / `server_state` 收的是**结构**而不是字符串：存储层只保留扁平标量字段，
/// 嵌套对象、数组、疑似凭据的键一律丢掉。调用方没有机会把一整份服务端响应塞进来。
pub struct Resolution<'a> {
    pub phase: &'a str,
    pub outcome: &'a str,
    pub grant: Option<&'a Value>,
    pub server_state: Option<&'a Value>,
    pub detail: Option<&'a str>,
    /// 这一条结论是否经过服务端回查。`failed` 必须有它。
    pub verified: bool,
}

/// 能落盘的阶段，以及 `failed` 的额外条件。
///
/// 这条规则是 P0-1 的落点：**结果不明（超时 / 5xx / 未知）只能落 `unknown`**，
/// 一旦允许它落成 `failed`，部分唯一索引就放开，下一轮会生成新的 `client_token`，
/// 同一次用户意图变成两次消耗。`failed` 只留给"回查之后确认确实没生效"。
fn closing_rule(phase: &str, verified: bool) -> Result<(), &'static str> {
    match phase {
        // 终局：服务端明确说了结果，目标可以释放。
        "confirmed" | "rejected" => Ok(()),
        // 继续占着目标：等回查（unknown）、等人工（frozen）、等用同一 token 重发（ready）。
        "unknown" | "frozen" | "ready" => Ok(()),
        "failed" if verified => Ok(()),
        "failed" => Err("failed 只能在回查确认请求确实没生效之后落盘；结果不明请落 unknown"),
        _ => Err("不是可以落盘的阶段"),
    }
}

/// `detail` 在存储边界强制脱敏 + 截断。调用方忘了也没关系。
fn sanitize_detail(detail: Option<&str>) -> Option<String> {
    let cleaned = crate::rpc::sanitize(detail?);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(MAX_DETAIL_CHARS).collect())
}

/// `grant`（到账回执）允许入库的字段：**只有已定义的业务数值与奖品信息**。
///
/// 正向白名单，不是黑名单：没在表里的字段一律丢，包括还没拿到 A 级证据的那些。
/// 新增字段要先有真实响应证据，再往这里加一行 —— 这条约束是 P1-3 的落点。
const GRANT_KEYS: &[&str] = &[
    // 到账数值。`*_granted` 是兑换/抽奖的实发字段，裸名是领奖的字段。
    "credit",
    "credits",
    "credit_granted",
    "credits_granted",
    "energy",
    "energy_granted",
    "cards",
    "cards_granted",
    "chances",
    "chances_granted",
    // 奖品与实物奖提示。
    "prize",
    "prize_name",
    "prizename",
    "need_address",
    "needaddress",
    "require_address",
    "requireaddress",
    // 幂等回执的标记，界面靠它区分"新领到"与"早就领过"。
    "already_claimed",
    "already",
];

/// `server_state`（回查快照）允许入库的字段：只放核验与展示必需的那几个。
///
/// 服务端返回嵌套（`makeup_cards: {balance}`）时，调用方要先拍平成
/// `makeup_cards_balance` 再传进来 —— 嵌套结构在存储层被整段丢弃。
const SERVER_STATE_KEYS: &[&str] = &[
    "chances",
    "balance",
    "observed",
    "days",
    "streak_days",
    "makeup_cards",
    "makeup_cards_balance",
    "status",
    "tier",
    "today",
    "date",
    "checked_in",
    "daily_checkin",
    "tier_7d_status",
    "tier_14d_status",
    "tier_28d_status",
    "starter_status",
    "advanced_status",
    "legendary_status",
];

/// 结构化回执的清洗：**按白名单留字段**，只保留扁平标量，整体有字节上限。
///
/// 原始服务端响应不属于账本：它可能带令牌、带整棵活动配置、几百个字段。
/// 账本只需要"这次到账多少、服务端当时怎么看"，所以这里只认业务字段表。
fn sanitize_payload(value: Option<&Value>, allowed: &[&str]) -> Option<String> {
    let object = value?.as_object()?;
    let mut kept = serde_json::Map::new();
    for (key, raw) in object.iter() {
        if kept.len() >= MAX_PAYLOAD_KEYS {
            break;
        }
        if !payable_key(key, allowed) {
            continue;
        }
        let scalar = match raw {
            Value::Number(number) => Value::Number(number.clone()),
            Value::Bool(flag) => Value::Bool(*flag),
            Value::String(text) => {
                let cleaned = crate::rpc::sanitize(text);
                let cut: String = cleaned
                    .trim()
                    .chars()
                    .take(MAX_PAYLOAD_VALUE_CHARS)
                    .collect();
                Value::String(cut)
            }
            // 嵌套对象 / 数组 / null：账本记不住也不需要，直接丢。
            _ => continue,
        };
        kept.insert(key.clone(), scalar);
        if serde_json::to_string(&kept)
            .map(|text| text.len())
            .unwrap_or(usize::MAX)
            > MAX_PAYLOAD_BYTES
        {
            kept.remove(key);
            break;
        }
    }
    if kept.is_empty() {
        return None;
    }
    serde_json::to_string(&kept).ok()
}

/// 可以进账本的键：**必须在白名单里**，且是个正常的短键名。
///
/// 白名单比对用小写，所以 `prizeName` 与 `prizename` 都命中 `prize_name` 那一类；
/// `api_key`、`session`、`credential`、`private_key` 这类字段没有对应业务含义，
/// 无论值多短都进不来（黑名单永远漏，白名单才收得住）。
fn payable_key(key: &str, allowed: &[&str]) -> bool {
    if key.is_empty() || key.len() > 32 {
        return false;
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return false;
    }
    let lowered = key.to_lowercase();
    allowed.contains(&lowered.as_str())
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
    /// 允许从 `pending`（第一次）或 `ready`（服务端上次明确没执行）进入，
    /// 但**始终沿用记录里那把 token** —— 换 token 等于换了一次新操作。
    ///
    /// 返回这一次的尝试序号（从 1 开始）。
    pub fn mark_reward_op_sent(&mut self, id: &str) -> RpcResult<i64> {
        let now = now_rfc3339();
        let changed = self.conn.execute(
            "UPDATE reward_operations
                SET phase = 'sent', attempt = attempt + 1, updated_at = ?2
              WHERE id = ?1 AND phase IN ('pending','ready') AND attempt < ?3
                AND (action = 'makeup_card'
                     OR (client_token IS NOT NULL AND client_token <> ''))",
            params![id, now, MAX_SEND_ATTEMPTS],
        )?;
        if changed != 1 {
            // 两种可能，都不发：状态已经被动过，或者次数用尽。
            // 次数检查放在**同一条 UPDATE** 里，不先读后写：两次并发发送不可能都成功。
            let exhausted: Option<i64> = self
                .conn
                .query_row(
                    "SELECT attempt FROM reward_operations WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .optional()?;
            return match exhausted {
                Some(attempt) if attempt >= MAX_SEND_ATTEMPTS => Err(RpcError::internal(
                    "这条操作已经发满次数，停止发送；需要人工核对服务端状态",
                )),
                _ => Err(RpcError::internal("这条操作不在待发送状态，已停止发送")),
            };
        }
        let attempt: i64 = self.conn.query_row(
            "SELECT attempt FROM reward_operations WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;
        Ok(attempt)
    }

    /// 回查专用：**只有 sent/unknown 能落结论**。
    ///
    /// 与 `resolve_reward_op` 的区别在 UPDATE 的 WHERE：读的时候是 unknown，
    /// 等网络回来时可能已经被别处写成 `frozen`（或已 confirmed）—— 这条旧回查
    /// 不得覆盖它们。判断和写入在同一条 SQL 里，没有 TOCTOU 窗口。
    pub fn resolve_reward_op_recheck(&mut self, id: &str, res: &Resolution<'_>) -> RpcResult<()> {
        if !matches!(res.phase, "confirmed" | "unknown" | "rejected") {
            return Err(RpcError::invalid_params(
                "回查只能落 confirmed/unknown/rejected",
            ));
        }
        let now = now_rfc3339();
        let verified_at = res.verified.then(|| now.clone());
        let server_state = sanitize_payload(res.server_state, SERVER_STATE_KEYS);
        let detail = sanitize_detail(res.detail);
        let changed = self.conn.execute(
            "UPDATE reward_operations
                SET phase = ?2, outcome = ?3, server_state = COALESCE(?4, server_state),
                    detail = ?5, updated_at = ?6, verified_at = ?7
              WHERE id = ?1 AND phase IN ('sent','unknown')",
            params![
                id,
                res.phase,
                res.outcome,
                server_state,
                detail,
                now,
                verified_at
            ],
        )?;
        if changed != 1 {
            return Err(RpcError::internal(
                "这条操作已被别的流程改写（可能已冻结或已确认），本次回查不覆盖它",
            ));
        }
        Ok(())
    }

    /// 落结论。只允许改未决记录 —— 已经结束的记录不能被后来的回查覆盖。
    ///
    /// 三条边界都在这里强制（不依赖调用方自觉）：
    /// 1. 结果不明只能落 `unknown` / `frozen`，`failed` 必须带 `verified`；
    /// 2. `detail` 一定脱敏并截断；
    /// 3. 回执只留扁平标量，原始响应进不来。
    pub fn resolve_reward_op(&mut self, id: &str, res: &Resolution<'_>) -> RpcResult<()> {
        if let Err(reason) = closing_rule(res.phase, res.verified) {
            return Err(RpcError::invalid_params(reason));
        }
        let now = now_rfc3339();
        let verified_at = res.verified.then(|| now.clone());
        let grant = sanitize_payload(res.grant, GRANT_KEYS);
        let server_state = sanitize_payload(res.server_state, SERVER_STATE_KEYS);
        let detail = sanitize_detail(res.detail);
        let changed = self.conn.execute(
            "UPDATE reward_operations
                SET phase = ?2, outcome = ?3, grant_json = ?4, server_state = COALESCE(?5, server_state),
                    detail = ?6, updated_at = ?7, verified_at = ?8
              WHERE id = ?1 AND phase IN ('pending','ready','sent','unknown','frozen')
                AND NOT (phase = 'frozen' AND ?2 = 'unknown')",
            params![
                id,
                res.phase,
                res.outcome,
                grant,
                server_state,
                detail,
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

    /// 记下发送前的服务端基线（余额/档位状态），回查时用来判断"变没变"。
    ///
    /// 只允许写一次：第一次发送前的快照才是比较基准，重发不能覆盖它。
    pub fn set_reward_op_baseline(&mut self, id: &str, baseline: &Value) -> RpcResult<()> {
        let text = serde_json::to_string(baseline).unwrap_or_else(|_| "{}".to_string());
        let now = now_rfc3339();
        self.conn.execute(
            "UPDATE reward_operations SET server_state = ?2, updated_at = ?3
              WHERE id = ?1 AND server_state IS NULL",
            params![id, text, now],
        )?;
        Ok(())
    }

    /// 这个 `(账号, 动作, 目标)` 有没有**任何**记录（含已结束的）。
    ///
    /// 一次性探针用它判断"这次机会是不是已经用掉了"—— 只看未决记录不够，
    /// `confirmed` 是终态，不占唯一索引，会被当成"从没做过"。
    pub fn has_any_reward_op(
        &self,
        account_id: &str,
        action: &str,
        target: &str,
    ) -> RpcResult<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM reward_operations
              WHERE account_id = ?1 AND action = ?2 AND target = ?3",
            params![account_id, action, target],
            |row| row.get(0),
        )?;
        Ok(count > 0)
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
    use crate::storage::AccountInput;
    use serde_json::json;
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

    fn conclude(phase: &'static str, verified: bool) -> Resolution<'static> {
        Resolution {
            phase,
            outcome: "granted",
            grant: None,
            server_state: None,
            detail: None,
            verified,
        }
    }

    fn open_then_send(db: &mut Db, account: &str, token: &str) -> String {
        let opened = db.open_reward_op(&new_op(account, Some(token))).unwrap();
        let id = opened.op().id.clone();
        db.mark_reward_op_sent(&id).unwrap();
        id
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
        db.open_reward_op(&new_op("other", Some("u-2"))).unwrap();
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
        let id = open_then_send(&mut db, "acc", "u-1");
        db.resolve_reward_op(&id, &conclude("confirmed", true))
            .unwrap();

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
        let id = open_then_send(&mut db, "acc", "u-1");
        assert!(db.mark_reward_op_sent(&id).is_err(), "同一轮不能记两次发送");
    }

    #[test]
    fn a_closed_operation_is_never_overwritten_by_a_late_verification() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        db.resolve_reward_op(&id, &conclude("confirmed", true))
            .unwrap();

        let retry = db.resolve_reward_op(&id, &conclude("rejected", true));
        assert!(retry.is_err(), "结论不能被后来的回查改写");
        assert_eq!(db.reward_op(&id).unwrap().unwrap().phase, "confirmed");
    }

    #[test]
    fn only_known_phases_can_be_resolved_into() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");

        assert!(
            db.resolve_reward_op(&id, &conclude("sent", true)).is_err(),
            "不能把结论落成 sent（不是结论）"
        );
        assert!(
            db.resolve_reward_op(&id, &conclude("pending", true))
                .is_err(),
            "不能把结论落成 pending（不是结论）"
        );
        assert!(
            db.resolve_reward_op(&id, &conclude("unknown", false))
                .is_ok(),
            "unknown 是一个合法结论"
        );
    }

    #[test]
    fn a_timeout_leaves_the_operation_awaiting_verification() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        db.resolve_reward_op(
            &id,
            &Resolution {
                phase: "unknown",
                outcome: "indeterminate",
                grant: None,
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

    /// **P0-1 回归**：超时 → unknown → 回查仍不确定 → 再次 open。
    /// 必须拿回原来那条（同一个 token），账本里不能出现第二条记录。
    #[test]
    fn an_indeterminate_result_never_frees_the_target() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-original");

        db.resolve_reward_op(&id, &conclude("unknown", false))
            .unwrap();

        let again = db.open_reward_op(&new_op("acc", Some("u-new"))).unwrap();
        assert!(!again.was_created(), "unknown 期间不许开新操作");
        assert_eq!(again.op().id, id);
        assert_eq!(
            again.op().client_token.as_deref(),
            Some("u-original"),
            "回查/重发必须用原来那把 token"
        );
        assert_eq!(db.reward_ops("acc", 10).unwrap().len(), 1, "不能有第二条");

        let view = reward_ledger::Open::from(again.op());
        assert!(matches!(
            reward_ledger::plan(Some(&view), true),
            Next::Recheck { .. }
        ));

        // 回查仍然不确定：还是 unknown，还占着。
        db.resolve_reward_op(&id, &conclude("unknown", false))
            .unwrap();
        let third = db.open_reward_op(&new_op("acc", Some("u-third"))).unwrap();
        assert!(!third.was_created());
        assert_eq!(db.reward_ops("acc", 10).unwrap().len(), 1);
    }

    /// P0-1：`failed` 是"核实过确实没发生"，没有核实就不许落。
    #[test]
    fn failed_requires_a_verified_recheck() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");

        let refused = db.resolve_reward_op(&id, &conclude("failed", false));
        assert!(refused.is_err(), "未核验的 failed 不许关闭账本");
        assert_eq!(
            db.reward_op(&id).unwrap().unwrap().phase,
            "sent",
            "拒绝之后状态必须原样保留"
        );

        db.resolve_reward_op(&id, &conclude("failed", true))
            .unwrap();
        assert!(db
            .open_reward_op_for("acc", "lottery_draw", "")
            .unwrap()
            .is_none());
    }

    /// P0-2 的存储侧：`frozen` 同样占着目标，不会被新的 open 顶掉。
    #[test]
    fn a_frozen_operation_keeps_the_target_and_blocks_new_tokens() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        db.resolve_reward_op(
            &id,
            &Resolution {
                phase: "frozen",
                outcome: "forbidden",
                grant: None,
                server_state: None,
                detail: Some("403 risk control"),
                verified: false,
            },
        )
        .unwrap();

        let again = db.open_reward_op(&new_op("acc", Some("u-2"))).unwrap();
        assert!(!again.was_created(), "冻结期间不许开新操作");
        assert_eq!(again.op().id, id);

        let view = reward_ledger::Open::from(again.op());
        assert!(matches!(
            reward_ledger::plan(Some(&view), true),
            Next::Blocked { .. }
        ));
    }

    /// PR #96：发送次数上限是**数据库约束**，不是 `plan()` 的建议。
    /// 调用方不问 `plan()` 直接反复 `mark_sent`，第四次也会被这条 UPDATE 拦住。
    #[test]
    fn the_send_cap_is_enforced_by_the_update_itself() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        assert_eq!(db.reward_op(&id).unwrap().unwrap().attempt, 1);

        // 想再发，必须先落一个"服务端明确没执行"的结论。
        for expected in 2..=MAX_SEND_ATTEMPTS {
            db.resolve_reward_op(&id, &conclude("ready", false))
                .unwrap();
            assert_eq!(db.mark_reward_op_sent(&id).unwrap(), expected);
        }

        db.resolve_reward_op(&id, &conclude("ready", false))
            .unwrap();
        let refused = db.mark_reward_op_sent(&id);
        assert!(refused.is_err(), "第 4 次发送必须被拒");
        assert!(
            refused.unwrap_err().message.contains("发满次数"),
            "错误要说清是次数用尽"
        );
        let op = db.reward_op(&id).unwrap().unwrap();
        assert_eq!(op.phase, "ready", "被拒之后状态不能被改成 sent");
        assert_eq!(op.attempt, MAX_SEND_ATTEMPTS);
    }

    /// TOCTOU：读时 unknown、等网络期间被改成 frozen，旧回查不得覆盖。
    #[test]
    fn a_recheck_that_lost_the_race_cannot_overwrite_a_frozen_row() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        db.resolve_reward_op(&id, &conclude("unknown", false))
            .unwrap();
        // 另一个流程在回查等待期间把它定性为冻结。
        db.resolve_reward_op(&id, &conclude("frozen", true))
            .unwrap();
        let stale = Resolution {
            phase: "unknown",
            outcome: "indeterminate",
            grant: None,
            server_state: None,
            detail: Some("stale recheck"),
            verified: false,
        };
        assert!(db.resolve_reward_op_recheck(&id, &stale).is_err());
        assert_eq!(db.reward_op(&id).unwrap().unwrap().phase, "frozen");
    }

    /// 冻结态不能被回查降级为 unknown：并发下先被定性冻结的记录，
    /// 后来那次回查只应"看得到"，不能改写它。
    #[test]
    fn a_frozen_operation_cannot_be_downgraded_by_a_late_recheck() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        db.resolve_reward_op(&id, &conclude("frozen", true))
            .unwrap();
        let late = Resolution {
            phase: "unknown",
            outcome: "indeterminate",
            grant: None,
            server_state: None,
            detail: Some("late recheck"),
            verified: false,
        };
        let changed = db.resolve_reward_op(&id, &late);
        assert!(changed.is_err() || db.reward_op(&id).unwrap().unwrap().phase == "frozen");
        assert_eq!(db.reward_op(&id).unwrap().unwrap().phase, "frozen");
    }

    /// 没有幂等键的抽奖/兑换记录**不许标记发送**：那是"用空 token 发请求"的入口。
    #[test]
    fn a_tokenless_draw_cannot_be_marked_as_sent() {
        let (mut db, _dir) = temp_db();
        let opened = db.open_reward_op(&new_op("acc", None)).unwrap();
        let id = opened.op().id.clone();
        let refused = db.mark_reward_op_sent(&id);
        assert!(refused.is_err(), "没有 token 的抽奖记录不许发");
        assert_eq!(db.reward_op(&id).unwrap().unwrap().phase, "pending");

        // 建行时就带 token 的那条可以发。
        let ok = db.open_reward_op(&new_op("acc2", Some("u-1"))).unwrap();
        assert!(db.mark_reward_op_sent(ok.op().id.as_str()).is_ok());
    }

    /// 补签没有幂等键（键是日期本身），不受上面那条约束。
    #[test]
    fn a_makeup_operation_may_send_without_a_token() {
        let (mut db, _dir) = temp_db();
        let opened = db
            .open_reward_op(&NewRewardOp {
                account_id: "acc",
                action: "makeup_card",
                target: "2026-11-10",
                client_token: None,
            })
            .unwrap();
        assert!(db.mark_reward_op_sent(opened.op().id.as_str()).is_ok());
    }

    /// 一次性探针的判据：**任何一条已结束的记录**都算用掉了这次机会。
    #[test]
    fn a_closed_operation_is_visible_to_the_one_shot_guard() {
        let (mut db, _dir) = temp_db();
        assert!(!db.has_any_reward_op("acc", "lottery_draw", "").unwrap());
        let id = open_then_send(&mut db, "acc", "u-1");
        assert!(db.has_any_reward_op("acc", "lottery_draw", "").unwrap());
        db.resolve_reward_op(&id, &conclude("confirmed", true))
            .unwrap();
        assert!(
            db.has_any_reward_op("acc", "lottery_draw", "").unwrap(),
            "confirmed 之后仍然算已用过"
        );
        // 别的动作不受影响。
        assert!(!db.has_any_reward_op("acc", "redeem", "").unwrap());
    }

    /// 服务端明确没执行（404 / tier 不认识）→ `ready`：允许用同一 token 再发一次。
    #[test]
    fn a_ready_operation_may_send_again_with_the_same_token() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        db.resolve_reward_op(&id, &conclude("ready", false))
            .unwrap();

        let stored = db.reward_op(&id).unwrap().unwrap();
        let view = reward_ledger::Open::from(&stored);
        match reward_ledger::plan(Some(&view), true) {
            Next::Resend { client_token, .. } => assert_eq!(client_token.as_deref(), Some("u-1")),
            other => panic!("ready 应该允许同 token 重发，实际 {other:?}"),
        }

        assert_eq!(db.mark_reward_op_sent(&id).unwrap(), 2, "第二次发送");
        assert_eq!(db.reward_op(&id).unwrap().unwrap().phase, "sent");
    }

    /// 崩溃恢复：进程 A 发了请求就没了；进程 B 起来必须**回查**，且用同一个 token。
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
        assert!(matches!(
            reward_ledger::plan(Some(&view), true),
            Next::Recheck { .. }
        ));
        assert_eq!(view.client_token.as_deref(), Some(token.as_str()));
    }

    /// 两个连接（等价两个进程）同时开同一条：只能有一个 Created。
    #[test]
    fn two_connections_cannot_both_create_the_same_operation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("accounts.sqlite");
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
            let id = open_then_send(&mut db, account, token);
            db.resolve_reward_op(&id, &conclude("confirmed", true))
                .unwrap();
        }
        let rows = db.reward_ops("a", 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].account_id, "a");
        assert_eq!(rows[0].phase, "confirmed");
        assert_eq!(rows[0].outcome.as_deref(), Some("granted"));
        assert!(db.reward_ops("nobody", 10).unwrap().is_empty());
    }

    // --- P1-2：账号删除与重新导入 -----------------------------------------

    fn add_account(db: &mut Db, uid: &str) -> String {
        db.upsert_account(&AccountInput {
            platform: "workbuddy".into(),
            uid: uid.into(),
            nickname: Some("测试".into()),
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
            provider: None,
        })
        .unwrap()
        .id
    }

    #[test]
    fn deleting_an_account_purges_its_reward_ledger() {
        let (mut db, _dir) = temp_db();
        let id = add_account(&mut db, "uid-1");
        db.open_reward_op(&new_op(&id, Some("u-1"))).unwrap();
        assert_eq!(db.reward_ops(&id, 10).unwrap().len(), 1);

        assert!(db.delete_account(&id).unwrap());
        assert!(
            db.reward_ops(&id, 10).unwrap().is_empty(),
            "删号必须把奖励账本一起清掉"
        );
    }

    /// 删掉再导入同一个 UID：id 是确定性哈希，所以还是同一个 id ——
    /// 旧账本必须已经没了，否则新账号一进来就被"未决记录"卡住。
    #[test]
    fn a_reimported_account_starts_with_a_clean_ledger() {
        let (mut db, _dir) = temp_db();
        let first = add_account(&mut db, "uid-1");
        let stale = open_then_send(&mut db, &first, "u-old");

        db.delete_account(&first).unwrap();

        let second = add_account(&mut db, "uid-1");
        assert_eq!(
            second, first,
            "同一 UID 复用同一个账号 id（这正是危险来源）"
        );

        let opened = db.open_reward_op(&new_op(&second, Some("u-new"))).unwrap();
        assert!(opened.was_created(), "重新导入后必须能开新操作");
        assert_ne!(opened.op().id, stale);
        assert_eq!(opened.op().client_token.as_deref(), Some("u-new"));
    }

    // --- P1-3：存储边界脱敏 -----------------------------------------------

    #[test]
    fn detail_is_sanitised_and_truncated_at_the_storage_boundary() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        let long = format!(
            "{} Bearer abcdefghijklmnopqrstuvwxyz0123456789",
            "很长的说明".repeat(80)
        );
        db.resolve_reward_op(
            &id,
            &Resolution {
                phase: "unknown",
                outcome: "unknown",
                grant: None,
                server_state: None,
                detail: Some(&long),
                verified: false,
            },
        )
        .unwrap();

        let detail = db.reward_op(&id).unwrap().unwrap().detail.unwrap();
        assert!(
            detail.chars().count() <= MAX_DETAIL_CHARS,
            "detail 必须截断：{} 字",
            detail.chars().count()
        );
        assert!(
            !detail.contains("abcdefghijklmnopqrstuvwxyz0123456789"),
            "必须脱敏"
        );
    }

    #[test]
    fn raw_server_payloads_cannot_be_stored() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        let grant = json!({
            "credits": 50,
            "energy": 3,
            "prize": "冰箱贴",
            "nested": { "raw": "整个响应" },
            "list": [1, 2, 3],
            "access_token": "should-not-be-here",
            // 白名单内的字段才允许出现，长值仍然要截断。
            "prize_name": "x".repeat(500)
        });
        let state = json!({ "chances": 0, "days": 14 });
        db.resolve_reward_op(
            &id,
            &Resolution {
                phase: "confirmed",
                outcome: "granted",
                grant: Some(&grant),
                server_state: Some(&state),
                detail: None,
                verified: true,
            },
        )
        .unwrap();

        let op = db.reward_op(&id).unwrap().unwrap();
        let stored: Value = serde_json::from_str(op.grant_json.as_deref().unwrap()).unwrap();
        assert_eq!(stored["credits"], 50);
        assert_eq!(stored["prize"], "冰箱贴");
        assert!(stored.get("nested").is_none(), "嵌套对象不许进账本");
        assert!(stored.get("list").is_none(), "数组不许进账本");
        assert!(stored.get("access_token").is_none(), "凭据字段不许进账本");
        assert!(
            stored["prize_name"].as_str().unwrap().chars().count() <= MAX_PAYLOAD_VALUE_CHARS,
            "长字符串必须截断"
        );
        assert!(stored.get("note").is_none(), "白名单外的字段一律不进");
        assert!(
            op.grant_json.as_deref().unwrap().len() <= MAX_PAYLOAD_BYTES,
            "整体必须有字节上限"
        );

        let state: Value = serde_json::from_str(op.server_state.as_deref().unwrap()).unwrap();
        assert_eq!(state["chances"], 0);
        assert_eq!(state["days"], 14);
    }

    /// P1-3 终审：黑名单会漏，**白名单才收得住**。
    ///
    /// 这些字段名都不含 token/secret 之类的关键词，值也很短（绕不过
    /// `rpc::sanitize` 的长令牌检测），但它们都不是业务字段 —— 一个都不许落盘。
    #[test]
    fn credential_shaped_fields_never_reach_the_ledger() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        let grant = json!({
            "credits": 50,
            "api_key": "short-secret",
            "session": "session123",
            "credential": "abc123",
            "private_key": "pk-1",
            "user": "someone",
            "device_id": "d-1"
        });
        let state = json!({
            "chances": 1,
            "api_key": "short-secret",
            "session": "session123",
            "credential": "abc123",
            "private_key": "pk-1",
            "cookie_jar": "c=1",
            "auth_headers": "Bearer x"
        });
        db.resolve_reward_op(
            &id,
            &Resolution {
                phase: "confirmed",
                outcome: "granted",
                grant: Some(&grant),
                server_state: Some(&state),
                detail: None,
                verified: true,
            },
        )
        .unwrap();

        let op = db.reward_op(&id).unwrap().unwrap();
        let grant: Value = serde_json::from_str(op.grant_json.as_deref().unwrap()).unwrap();
        assert_eq!(grant["credits"], 50, "业务字段要留下来");
        for banned in [
            "api_key",
            "session",
            "credential",
            "private_key",
            "user",
            "device_id",
        ] {
            assert!(
                grant.get(banned).is_none(),
                "{banned} 不是业务字段，不许进账本：{grant}"
            );
        }

        let state: Value = serde_json::from_str(op.server_state.as_deref().unwrap()).unwrap();
        assert_eq!(state["chances"], 1, "核验要用的状态字段要留下来");
        for banned in [
            "api_key",
            "session",
            "credential",
            "private_key",
            "cookie_jar",
            "auth_headers",
        ] {
            assert!(
                state.get(banned).is_none(),
                "{banned} 不是状态字段，不许进账本：{state}"
            );
        }
    }

    /// 白名单是**逐张表**的：`grant` 的字段不能混进 `server_state`，反之亦然。
    #[test]
    fn the_two_payload_kinds_do_not_share_a_whitelist() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        let grant = json!({ "credits": 5, "chances": 1, "days": 7 });
        let state = json!({ "chances": 1, "credits": 5, "days": 7 });
        db.resolve_reward_op(
            &id,
            &Resolution {
                phase: "confirmed",
                outcome: "granted",
                grant: Some(&grant),
                server_state: Some(&state),
                detail: None,
                verified: true,
            },
        )
        .unwrap();

        let op = db.reward_op(&id).unwrap().unwrap();
        let grant: Value = serde_json::from_str(op.grant_json.as_deref().unwrap()).unwrap();
        assert_eq!(grant["chances"], 1, "抽奖次数是到账字段");
        assert!(grant.get("days").is_none(), "连登天数是状态，不是到账");

        let state: Value = serde_json::from_str(op.server_state.as_deref().unwrap()).unwrap();
        assert_eq!(state["days"], 7);
        assert!(state.get("credits").is_none(), "积分是到账，不是回查状态");
    }

    #[test]
    fn a_payload_that_is_not_an_object_is_dropped_instead_of_stored_raw() {
        let (mut db, _dir) = temp_db();
        let id = open_then_send(&mut db, "acc", "u-1");
        let raw = json!("{\"code\":0,\"data\":{...整份响应...}}");
        let nested_only = json!({ "data": { "credit": 1 } });
        db.resolve_reward_op(
            &id,
            &Resolution {
                phase: "confirmed",
                outcome: "granted",
                grant: Some(&raw),
                server_state: Some(&nested_only),
                detail: None,
                verified: true,
            },
        )
        .unwrap();

        let op = db.reward_op(&id).unwrap().unwrap();
        assert!(op.grant_json.is_none(), "字符串不是结构化回执");
        assert!(op.server_state.is_none(), "只有嵌套对象等于没有可用字段");
    }
}
