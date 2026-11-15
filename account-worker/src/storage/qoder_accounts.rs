//! Qoder 平台服务账号与授权来源（v13）。
//!
//! ## 和 `qoder_profiles` 的区别（别混）
//!
//! | 表 | 是什么 | 依赖客户端吗 |
//! | --- | --- | --- |
//! | `qoder_profiles` | **本机客户端**的认证快照，用于识别"本机登录的是谁" | 依赖 |
//! | `qoder_accounts` | **平台服务**上的账号身份（UID） | 不依赖 |
//! | `qoder_authorizations` | 某个账号的一份授权来源（magpie / 浏览器） | 不依赖 |
//!
//! 所以：**不装 Qoder 客户端也能把账号导进来**。网页授权、Magpie 导入都只写后两张表。
//!
//! ## 授权来源的所有权
//!
//! - `magpie`：只登记关联（`source_key`），**不复制令牌**——Magpie 自己的可轮换
//!   refresh token 由 Magpie 刷新，AICC 复制一份再刷新只会互相踢下线。
//!   `secret_ref` 在这类行里恒为空。
//! - `own_browser`：令牌在钥匙串，这里只有 `secret_ref`（钥匙串键）与指纹。
//! - `own_pat`：用户在 Qoder 账户设置里自建的 Personal Access Token。同样只在
//!   钥匙串里存一份（`qoder/<account_id>/pat/token`），且**不会刷新**——
//!   它是长期凭据，失效只能由用户重新生成。与 `own_browser` 是两行，
//!   互不覆盖：浏览器授权过期不该把 PAT 一起标脏，反之亦然。
//!
//! 同一账号可以有多个来源并存：两行分别存在，互不覆盖。

use rusqlite::{params, OptionalExtension};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::clock::now_rfc3339;
use crate::rpc::{RpcError, RpcResult};

pub const SOURCE_MAGPIE: &str = "magpie";
pub const SOURCE_OWN_BROWSER: &str = "own_browser";
pub const SOURCE_OWN_PAT: &str = "own_pat";
pub const SOURCES: &[&str] = &[SOURCE_MAGPIE, SOURCE_OWN_BROWSER, SOURCE_OWN_PAT];
pub const EDITIONS: &[&str] = &["qoder_cn", "qoder_global"];
pub const AUTH_STATUSES: &[&str] = &[
    "unknown", "valid", "expired", "invalid", "denied", "missing",
];

/// v16 迁移步骤：给账号加 `auth_epoch`，并把授权表的 `source` CHECK 扩到
/// `own_pat`。**两步都先检测再动手**，重复执行是空操作。
///
/// 为什么要检测而不是直接写 SQL：`user_version` 可能被人工回拨，或者上一次
/// 迁移事务已提交但版本号写入失败；那种库再跑一遍 `ALTER TABLE ... ADD COLUMN`
/// 会以 `duplicate column name` 直接崩掉，用户看到的是"数据库坏了"。
pub(super) fn rebuild_v16(tx: &rusqlite::Transaction<'_>) -> RpcResult<()> {
    let has_epoch: i64 = tx.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('qoder_accounts') WHERE name = 'auth_epoch'",
        [],
        |row| row.get(0),
    )?;
    if has_epoch == 0 {
        tx.execute_batch(
            "ALTER TABLE qoder_accounts ADD COLUMN auth_epoch INTEGER NOT NULL DEFAULT 0;",
        )?;
    }

    let table_sql: String = tx.query_row(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'qoder_authorizations'",
        [],
        |row| row.get(0),
    )?;
    if table_sql.contains("own_pat") {
        return Ok(());
    }

    // 先建新表、搬数据、再改名：`foreign_keys=ON` 下这张表是**子表**
    // （引用 `qoder_accounts`），搬数据时父行仍在，外键检查能过。
    //
    // 注意：这段是**多行字符串**，不要改成行尾反斜杠续行——那会把相邻两行
    // 粘成一个 token（`updated_at)SELECT`），SQL 直接语法错误（v15 踩过一次）。
    tx.execute_batch(
        "CREATE TABLE qoder_authorizations_v16 (
             id               TEXT PRIMARY KEY,
             account_id       TEXT NOT NULL REFERENCES qoder_accounts(id) ON DELETE CASCADE,
             source           TEXT NOT NULL CHECK (source IN ('magpie', 'own_browser', 'own_pat')),
             source_key       TEXT,
             secret_ref       TEXT,
             auth_status      TEXT NOT NULL DEFAULT 'unknown'
                              CHECK (auth_status IN ('unknown','valid','expired','invalid','denied','missing')),
             token_expires_at TEXT,
             fingerprint      TEXT,
             created_at       TEXT NOT NULL,
             updated_at       TEXT NOT NULL,
             UNIQUE (account_id, source)
         );
         INSERT INTO qoder_authorizations_v16 (
             id, account_id, source, source_key, secret_ref, auth_status,
             token_expires_at, fingerprint, created_at, updated_at)
         SELECT id, account_id, source, source_key, secret_ref, auth_status,
                token_expires_at, fingerprint, created_at, updated_at
           FROM qoder_authorizations;
         DROP TABLE qoder_authorizations;
         ALTER TABLE qoder_authorizations_v16 RENAME TO qoder_authorizations;
         CREATE INDEX IF NOT EXISTS idx_qoder_auth_account
             ON qoder_authorizations (account_id);",
    )?;
    Ok(())
}

/// v19 迁移步骤：给账号加 `generation`。
///
/// **为什么 `auth_epoch` 一个人不够**：账号 id 是 `(edition, uid)` 的确定性哈希，
/// 删掉再用同一个 UID 登录会复用同一个 id；新行的 `auth_epoch` 从 0 重新开始，
/// 于是旧生命周期里发起的请求（epoch 也是 0）会"恰好"通过版本核验，把结果写进
/// 新账号。`generation` 在**建行时**取一个随机值，删行即随之消失，重建必得新值，
/// 所以它是"这一次生命周期"的标识，而不是"这一次授权"的版本。
///
/// `auth_epoch` 继续保留：它描述的是同一生命周期内授权被换过几次，两者一起用。
///
/// 与 v16 / v17 / v18 一样先检测再动手；回填用 `randomblob` 逐行求值，
/// 不会把所有旧行写成同一个值。
pub(super) fn migrate_v19(tx: &rusqlite::Transaction<'_>) -> RpcResult<()> {
    let has_generation: i64 = tx.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('qoder_accounts') WHERE name = 'generation'",
        [],
        |row| row.get(0),
    )?;
    if has_generation == 0 {
        tx.execute_batch(
            "ALTER TABLE qoder_accounts ADD COLUMN generation TEXT NOT NULL DEFAULT '';",
        )?;
    }
    tx.execute_batch(
        "UPDATE qoder_accounts SET generation = lower(hex(randomblob(16))) WHERE generation = '';",
    )?;
    Ok(())
}

/// v21：给授权行加**授权版本号**。
///
/// 与 `auth_epoch` 的区别：epoch 是**账号级**的（任何一份来源写入都会前进），
/// 而 `auth_version` 是**这一行**的授权次数。判断"我删掉的还是不是这一份凭据"
/// 必须看后者 —— 指纹在重新保存同一串令牌时不变，`secret_ref` 在原地覆盖时也不变。
pub(super) fn migrate_v21(tx: &rusqlite::Transaction<'_>) -> RpcResult<()> {
    let has_version: i64 = tx.query_row(
        "SELECT COUNT(*) FROM pragma_table_info('qoder_authorizations') WHERE name = 'auth_version'",
        [],
        |row| row.get(0),
    )?;
    if has_version == 0 {
        tx.execute_batch(
            "ALTER TABLE qoder_authorizations ADD COLUMN auth_version INTEGER NOT NULL DEFAULT 0;",
        )?;
    }
    Ok(())
}

/// 一次**账号生命周期**的标识：`generation` + `auth_epoch`。
///
/// 两者必须一起核验，缺一不可：
///
/// | 场景 | `generation` | `auth_epoch` |
/// | --- | --- | --- |
/// | 换了一份 PAT / 浏览器授权 | 不变 | +1 |
/// | 删号后用同一个 UID 重新登录 | **变** | **回到 0** |
///
/// 只看 `auth_epoch` 时第二种情况会误判成"没变过"——新行从 0 起，而旧请求
/// 记的也是 0。这个结构的存在就是为了让那种比较不可能被写错。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QoderLifecycle {
    pub generation: String,
    pub auth_epoch: i64,
}

impl QoderLifecycle {
    /// 从一条账号记录取出当时的生命周期。**取数前调一次，写入时带上它。**
    pub fn of(account: &QoderAccountRecord) -> Self {
        Self {
            generation: account.generation.clone(),
            auth_epoch: account.auth_epoch,
        }
    }
}

/// 条件提交的预期（CRED-3b）。
///
/// 判据是"写凭据之前观察到的那一行" + 账号生命周期 + 行版本：
///
/// | 场景 | 判定 |
/// | --- | --- |
/// | 首次授权，两进程同时写 | 只有一个 `INSERT` 成功，另一个拿到 `None` |
/// | 重复授权 / PAT 更新 | `UPDATE` 必须命中同一个 `secret_ref` 与同一个 `auth_version` |
/// | 引用被别人迁走 | `secret_ref` 对不上 → `None` |
/// | 删号后同 UID 重建 | `generation` 变了 → `None`（epoch 会归零，单看它挡不住） |
/// | 期间别的授权推进了 epoch | `auth_epoch` 变了 → `None` |
/// | 行被删掉又重建 | `auth_version` 取 `epoch + 1`，旧版本命中不到 |
///
/// **`Absent` 也必须带生命周期**：账号 id 是 `(edition, uid)` 的确定性哈希，
/// 删号后用同一个 UID 重建会得到**同一个 id**、`generation` 却是新的。只靠
/// "这一行还不存在"判断就够的话，一个在删号前发起的旧登录会在重建之后
/// 把旧会话的令牌插进新账号。
///
/// **版本号不能回拨**：`auth_version` 初值与补偿恢复都取 `auth_epoch + 1`，
/// 所以同一账号内它严格单调（详见 `migrate_v21` 与 `restore_qoder_authorization`）。
#[derive(Debug, Clone)]
pub enum QoderAuthExpectation {
    /// 写凭据时这一行还不存在：只允许 `INSERT`，且账号必须还是写凭据时那一个。
    Absent { lifecycle: QoderLifecycle },
    /// 写凭据时看到的是这一行：`UPDATE` 必须命中同一引用、同一版本、同一生命周期。
    ///
    /// `auth_version` 是额外的双保险：即使 `secret_ref` 与生命周期都没变，
    /// 另一个进程刚刚提交过这一行也必须落空。
    Present {
        secret_ref: Option<String>,
        lifecycle: QoderLifecycle,
        auth_version: i64,
    },
}

#[allow(dead_code)] // 供测试与后续删除路径读取，避免各调用点自己 match
impl QoderAuthExpectation {
    /// 写凭据时观察到的生命周期。删除 / 状态写入都要按它做核验。
    pub fn lifecycle(&self) -> &QoderLifecycle {
        match self {
            Self::Absent { lifecycle } | Self::Present { lifecycle, .. } => lifecycle,
        }
    }

    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent { .. })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderAccountRecord {
    pub id: String,
    pub edition: String,
    pub uid: String,
    pub nickname: String,
    pub email: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// 凭据版本号（v16）。**任何一次授权写入或删除都会 +1**。
    ///
    /// 用途与 `accounts.auth_epoch` 完全一致：额度取数有两三次网络往返，
    /// 期间用户可能重新授权甚至删号；账号 id 是 `(edition, uid)` 的确定性哈希，
    /// 删了再导入会**复用同一个 id**，晚到的快照会被当成"刚取过"。
    pub auth_epoch: i64,
    /// 账号生命周期标识（v19）：建行时随机生成，**删号重建必得新值**。
    ///
    /// 单独看 `auth_epoch` 挡不住"删了又用同一个 UID 登录"：新行从 0 开始，
    /// 与旧请求记下的 0 相等。所有"晚到的写入"必须同时核对它。
    pub generation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QoderAuthRecord {
    pub id: String,
    pub account_id: String,
    pub source: String,
    pub source_key: Option<String>,
    pub secret_ref: Option<String>,
    pub auth_status: String,
    pub token_expires_at: Option<String>,
    pub fingerprint: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    /// 这一行被提交过多少次（v21）。**唯一可靠的授权版本标识**：
    /// 重新保存同一串令牌（指纹不变）、原地覆盖（`secret_ref` 不变）都会让它前进。
    pub auth_version: i64,
}

impl QoderAuthRecord {
    /// 给 RPC / 界面用：**不含令牌**，只有"有没有凭据"。
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "accountId": self.account_id,
            "source": self.source,
            "sourceKey": self.source_key,
            "hasSecret": self.secret_ref.is_some(),
            "authStatus": self.auth_status,
            "tokenExpiresAt": self.token_expires_at,
            "fingerprint": self.fingerprint,
            "updatedAt": self.updated_at,
        })
    }
}

impl QoderAccountRecord {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "edition": self.edition,
            "uid": self.uid,
            "nickname": self.nickname,
            "email": self.email,
            "updatedAt": self.updated_at,
        })
    }
}

/// 账号主键 = `qa_` + sha256(edition + NUL + uid) 前 12 hex。
///
/// 用哈希而不是自增：同一份 Magpie 数据重复导入必须落到同一行，不能靠"先查再写"。
pub fn qoder_account_id(edition: &str, uid: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(edition.as_bytes());
    hasher.update([0u8]);
    hasher.update(uid.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("qa_{hex}")
}

/// 授权主键 = `qz_` + sha256(account_id + NUL + source) 前 12 hex。
pub fn qoder_auth_id(account_id: &str, source: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(account_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(source.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("qz_{hex}")
}

pub(crate) const ACCOUNT_COLUMNS: &str =
    "id, edition, uid, nickname, email, created_at, updated_at, auth_epoch, generation";

const AUTH_COLUMNS: &str = "id, account_id, source, source_key, secret_ref, auth_status, \
     token_expires_at, fingerprint, created_at, updated_at, auth_version";

pub(crate) fn map_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<QoderAccountRecord> {
    Ok(QoderAccountRecord {
        id: row.get(0)?,
        edition: row.get(1)?,
        uid: row.get(2)?,
        nickname: row.get(3)?,
        email: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        auth_epoch: row.get(7)?,
        generation: row.get(8)?,
    })
}

fn map_auth(row: &rusqlite::Row<'_>) -> rusqlite::Result<QoderAuthRecord> {
    Ok(QoderAuthRecord {
        id: row.get(0)?,
        account_id: row.get(1)?,
        source: row.get(2)?,
        source_key: row.get(3)?,
        secret_ref: row.get(4)?,
        auth_status: row.get(5)?,
        token_expires_at: row.get(6)?,
        fingerprint: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
        auth_version: row.get(10)?,
    })
}

impl crate::storage::Db {
    /// 建/接管一个 Qoder 平台账号。
    ///
    /// 语义与 `upsert_account` 一致：**只补空**。昵称只在为空时写入，
    /// 邮箱只在为空时写入——用户改过的名字不会被下一次导入冲掉。
    pub fn upsert_qoder_account(
        &mut self,
        edition: &str,
        uid: &str,
        nickname: Option<&str>,
        email: Option<&str>,
    ) -> RpcResult<QoderAccountRecord> {
        if !EDITIONS.contains(&edition) {
            return Err(RpcError::invalid_params(format!(
                "edition 只支持 qoder_cn / qoder_global，收到 {edition}"
            )));
        }
        if uid.trim().is_empty() {
            return Err(RpcError::invalid_params("uid 不能为空"));
        }
        let id = qoder_account_id(edition, uid);
        let now = now_rfc3339();
        // 生命周期标识只在**建行**时写一次：`DO NOTHING` 与后面的 `UPDATE`
        // 都不碰它，所以重复导入、加一份新授权都不会让人以为账号"换了一个"。
        let generation = uuid::Uuid::new_v4().simple().to_string();
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO qoder_accounts (id, edition, uid, nickname, email, created_at, updated_at, generation)
             VALUES (?1, ?2, ?3, '', NULL, ?4, ?4, ?5)
             ON CONFLICT (edition, uid) DO NOTHING",
            params![id, edition, uid, now, generation],
        )?;
        let mut sets: Vec<&str> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(name) = nickname.map(str::trim).filter(|s| !s.is_empty()) {
            sets.push("nickname = CASE WHEN nickname = '' THEN ? ELSE nickname END");
            args.push(Box::new(name.to_string()));
        }
        if let Some(mail) = email.map(str::trim).filter(|s| !s.is_empty()) {
            sets.push("email = COALESCE(email, ?)");
            args.push(Box::new(mail.to_string()));
        }
        sets.push("updated_at = ?");
        args.push(Box::new(now));
        args.push(Box::new(edition.to_string()));
        args.push(Box::new(uid.to_string()));
        let sql = format!(
            "UPDATE qoder_accounts SET {} WHERE edition = ? AND uid = ?",
            sets.join(", ")
        );
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        tx.execute(&sql, refs.as_slice())?;
        tx.commit()?;

        self.find_qoder_account(edition, uid)?
            .ok_or_else(|| RpcError::storage("写入后读不到 Qoder 账号"))
    }

    pub fn get_qoder_account(&self, id: &str) -> RpcResult<Option<QoderAccountRecord>> {
        let sql = format!("SELECT {ACCOUNT_COLUMNS} FROM qoder_accounts WHERE id = ?1");
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt.query_row(params![id], map_account).optional()?)
    }

    pub fn find_qoder_account(
        &self,
        edition: &str,
        uid: &str,
    ) -> RpcResult<Option<QoderAccountRecord>> {
        let sql =
            format!("SELECT {ACCOUNT_COLUMNS} FROM qoder_accounts WHERE edition = ?1 AND uid = ?2");
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![edition, uid], map_account)
            .optional()?)
    }

    pub fn list_qoder_accounts(&self, edition: Option<&str>) -> RpcResult<Vec<QoderAccountRecord>> {
        let mut sql = format!("SELECT {ACCOUNT_COLUMNS} FROM qoder_accounts WHERE 1 = 1");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if let Some(edition) = edition.filter(|e| !e.is_empty()) {
            sql.push_str(" AND edition = ?");
            args.push(Box::new(edition.to_string()));
        }
        sql.push_str(" ORDER BY edition ASC, created_at ASC");
        let mut stmt = self.conn.prepare(&sql)?;
        let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
        let rows = stmt.query_map(refs.as_slice(), map_account)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 写一份授权来源。`(account_id, source)` 唯一：同一来源重复导入是**更新**
    /// 元数据，不会多出一行；另一个来源的行完全不受影响。
    #[allow(clippy::too_many_arguments)]
    pub fn upsert_qoder_authorization(
        &mut self,
        account_id: &str,
        source: &str,
        source_key: Option<&str>,
        secret_ref: Option<&str>,
        auth_status: &str,
        token_expires_at: Option<&str>,
        fingerprint: Option<&str>,
    ) -> RpcResult<QoderAuthRecord> {
        if !SOURCES.contains(&source) {
            return Err(RpcError::invalid_params(format!(
                "source 只支持 magpie / own_browser / own_pat，收到 {source}"
            )));
        }
        if !AUTH_STATUSES.contains(&auth_status) {
            return Err(RpcError::invalid_params(format!(
                "authStatus 取值非法：{auth_status}"
            )));
        }
        // Magpie 来源不许带凭据引用：Magpie 的令牌由 Magpie 刷新，AICC 只借用。
        if source == SOURCE_MAGPIE && secret_ref.is_some() {
            return Err(RpcError::invalid_params(
                "magpie 来源不能持有 AICC 自己的凭据引用",
            ));
        }
        if self.get_qoder_account(account_id)?.is_none() {
            return Err(RpcError::not_found(format!(
                "Qoder 账号不存在：{account_id}"
            )));
        }
        let id = qoder_auth_id(account_id, source);
        let now = now_rfc3339();
        // 授权写入与 `auth_epoch` 推进必须同一个事务：否则会出现"授权换了、
        // 版本号没动"的窗口，在途的旧额度快照就能合法写回。
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO qoder_authorizations (
                 id, account_id, source, source_key, secret_ref, auth_status,
                 token_expires_at, fingerprint, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)
             ON CONFLICT (account_id, source) DO UPDATE SET
                 source_key = COALESCE(excluded.source_key, qoder_authorizations.source_key),
                 secret_ref = COALESCE(excluded.secret_ref, qoder_authorizations.secret_ref),
                 auth_status = excluded.auth_status,
                 token_expires_at = COALESCE(excluded.token_expires_at, qoder_authorizations.token_expires_at),
                 fingerprint = COALESCE(excluded.fingerprint, qoder_authorizations.fingerprint),
                 -- 每次写入都算一次新的授权（v21）：版本号必须单调前进，
                 -- 否则「同一串令牌重新保存」会被后面的核验当成没变过。
                 auth_version = qoder_authorizations.auth_version + 1,
                 updated_at = excluded.updated_at",
            params![
                id,
                account_id,
                source,
                source_key,
                secret_ref,
                auth_status,
                token_expires_at,
                fingerprint,
                now
            ],
        )?;
        tx.execute(
            "UPDATE qoder_accounts SET auth_epoch = auth_epoch + 1, updated_at = ?1 WHERE id = ?2",
            params![now, account_id],
        )?;
        tx.commit()?;
        self.get_qoder_authorization(account_id, source)?
            .ok_or_else(|| RpcError::storage("写入后读不到 Qoder 授权"))
    }

    /// 条件提交一份授权：**引用、状态、有效期、指纹一次落库**，并在同一事务里
    /// 推进 `auth_epoch`。
    ///
    /// 返回 `Ok(None)` = 这一行在我写凭据期间被别人改过，什么都没写。
    /// 调用方必须按竞争失败处理（新建的凭据文件要清掉，覆盖写的那份保持不动）。
    #[allow(clippy::too_many_arguments)]
    pub fn commit_qoder_authorization(
        &mut self,
        account_id: &str,
        source: &str,
        secret_ref: Option<&str>,
        auth_status: &str,
        token_expires_at: Option<&str>,
        fingerprint: Option<&str>,
        expectation: &QoderAuthExpectation,
    ) -> RpcResult<Option<QoderAuthRecord>> {
        if !SOURCES.contains(&source) {
            return Err(RpcError::invalid_params(format!(
                "source 只支持 magpie / own_browser / own_pat，收到 {source}"
            )));
        }
        if !AUTH_STATUSES.contains(&auth_status) {
            return Err(RpcError::invalid_params(format!(
                "authStatus 取值非法：{auth_status}"
            )));
        }
        if source == SOURCE_MAGPIE && secret_ref.is_some() {
            return Err(RpcError::invalid_params(
                "magpie 来源不能持有 AICC 自己的凭据引用",
            ));
        }
        if self.get_qoder_account(account_id)?.is_none() {
            return Err(RpcError::not_found(format!(
                "Qoder 账号不存在：{account_id}"
            )));
        }

        let id = qoder_auth_id(account_id, source);
        let now = now_rfc3339();
        let tx = self.conn.transaction()?;
        let changed = match expectation {
            // `INSERT ... SELECT ... WHERE EXISTS`：生命周期对不上就插 0 行，
            // 不算错误（那是"账号已经被删掉重建了"，不是数据库坏了）。
            QoderAuthExpectation::Absent { lifecycle } => match tx.execute(
                "INSERT INTO qoder_authorizations (
                     id, account_id, source, source_key, secret_ref, auth_status,
                     token_expires_at, fingerprint, created_at, updated_at, auth_version
                 )
                 SELECT ?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?8,
                        -- 版本取「账号当前 epoch + 1」而不是常数 1：同一来源的行被删掉
                        -- 再插入时必须比上一次大（审计 P0-4 的 ABA）。epoch 只增不减，
                        -- 所以这个值在账号内严格单调，旧操作带旧版本永远命中不到新行。
                        (SELECT auth_epoch + 1 FROM qoder_accounts WHERE id = ?2)
                  WHERE EXISTS (SELECT 1 FROM qoder_accounts
                                WHERE id = ?2 AND generation = ?9 AND auth_epoch = ?10)",
                params![
                    id,
                    account_id,
                    source,
                    secret_ref,
                    auth_status,
                    token_expires_at,
                    fingerprint,
                    now,
                    lifecycle.generation,
                    lifecycle.auth_epoch
                ],
            ) {
                // **必须用真实行数**：`WHERE EXISTS` 不成立时 SQLite 不报错，只插 0 行。
                // 把 `Ok(_)` 当成 1 会把"账号已经被删掉重建"误判成插入成功。
                Ok(inserted) => inserted,
                // `(account_id, source)` 唯一：别人先建了。这是**正常**的竞争结果。
                Err(err) if is_constraint_violation(&err) => 0,
                Err(err) => return Err(err.into()),
            },
            QoderAuthExpectation::Present {
                secret_ref: expected,
                lifecycle,
                auth_version,
            } => tx.execute(
                "UPDATE qoder_authorizations
                    SET secret_ref = ?1, auth_status = ?2, token_expires_at = ?3,
                        fingerprint = ?4, updated_at = ?5,
                        auth_version = auth_version + 1
                  WHERE account_id = ?6 AND source = ?7
                    AND secret_ref IS ?8 AND auth_version = ?9
                    AND EXISTS (SELECT 1 FROM qoder_accounts
                                WHERE id = ?6 AND generation = ?10 AND auth_epoch = ?11)",
                params![
                    secret_ref,
                    auth_status,
                    token_expires_at,
                    fingerprint,
                    now,
                    account_id,
                    source,
                    expected,
                    auth_version,
                    lifecycle.generation,
                    lifecycle.auth_epoch
                ],
            )?,
        };

        if changed == 0 {
            // `tx` 在这里 Drop → 自动回滚，一个字节都没写。
            return Ok(None);
        }
        // 授权换了一份 = 在途的旧额度快照 / 领取请求立刻作废。
        tx.execute(
            "UPDATE qoder_accounts SET auth_epoch = auth_epoch + 1, updated_at = ?1 WHERE id = ?2",
            params![now, account_id],
        )?;
        tx.commit()?;
        self.get_qoder_authorization(account_id, source)
    }

    /// 有多少行授权引用着这个 `secret_ref`。
    ///
    /// 竞争失败清理用：只有在**没有任何行引用**时才允许删掉刚建的文件，
    /// 否则删的可能正是胜者的那一条。
    pub fn qoder_authorizations_referencing(&self, reference: &str) -> RpcResult<usize> {
        let mut stmt = self
            .conn
            .prepare("SELECT COUNT(*) FROM qoder_authorizations WHERE secret_ref = ?1")?;
        let count: i64 = stmt.query_row(params![reference], |row| row.get(0))?;
        Ok(count.max(0) as usize)
    }

    /// 只改授权状态（例如 401 后标 `invalid`），**不推进版本号**。
    ///
    /// 生产路径改用两个带核验的变体（`_if_current` / `_if_generation`）：无核验的
    /// 状态写入会在"账号删了又建"时标脏新账号。这里保留给测试与调试。
    #[allow(dead_code)]
    ///
    /// 状态是观测结论，不是新的凭据事实：把"我刚被拒绝"也当成授权变更，
    /// 会让同一轮里已经拿到的正常快照白白作废。
    pub fn set_qoder_auth_status(
        &mut self,
        account_id: &str,
        source: &str,
        auth_status: &str,
    ) -> RpcResult<bool> {
        if !AUTH_STATUSES.contains(&auth_status) {
            return Err(RpcError::invalid_params(format!(
                "authStatus 取值非法：{auth_status}"
            )));
        }
        let changed = self.conn.execute(
            "UPDATE qoder_authorizations SET auth_status = ?1, updated_at = ?2
             WHERE account_id = ?3 AND source = ?4",
            params![auth_status, now_rfc3339(), account_id, source],
        )?;
        Ok(changed > 0)
    }

    /// 只按"账号这一代 + 这一份凭据的授权版本"核验的状态更新。
    ///
    /// 用在"删号中途被打断、需要把已经清掉凭据的行标成 `missing`"这种收尾上。
    /// 两个条件缺一不可：
    ///
    /// - `generation`：同一个 UID 删了又建时，不能动新账号；
    /// - `auth_version`：**同一来源的重新授权往往复用同一个引用**（钥匙串固定键名、
    ///   JSON 原地覆盖都不换 `secret_ref`），重新保存**同一串**令牌连指纹都不变；
    ///   只有每次提交都前进的版本号能区分"这是不是我还删掉的那一份"（复审 P1-C）。
    ///
    /// **刻意不用 `auth_epoch`**：它是账号级的，别的来源新增一次授权也会推进它，
    /// 那样会把"确实被我删掉、且没人重登"的行也一并放过不标。
    ///
    /// 返回 `false` 表示"已经不是我删掉的那一份了"，什么都没写。
    #[allow(clippy::too_many_arguments)]
    pub fn set_qoder_auth_status_if_version(
        &mut self,
        account_id: &str,
        source: &str,
        auth_status: &str,
        generation: &str,
        auth_version: i64,
    ) -> RpcResult<bool> {
        if !AUTH_STATUSES.contains(&auth_status) {
            return Err(RpcError::invalid_params(format!(
                "authStatus 取值非法：{auth_status}"
            )));
        }
        let changed = self.conn.execute(
            "UPDATE qoder_authorizations SET auth_status = ?1, updated_at = ?2
             WHERE account_id = ?3 AND source = ?4 AND auth_version = ?5
               AND EXISTS (SELECT 1 FROM qoder_accounts
                           WHERE id = ?3 AND generation = ?6)",
            params![
                auth_status,
                now_rfc3339(),
                account_id,
                source,
                auth_version,
                generation
            ],
        )?;
        Ok(changed > 0)
    }

    /// 带版本核验的状态更新：**只有账号仍处于 `expected` 那次生命周期、且授权
    /// 没换过时才写**。
    ///
    /// 为什么必须有它：一次网络请求要几秒，期间用户可能已经换了一份新 PAT，
    /// 也可能把账号删了又用同一个 UID 重新登录。前一种情况下旧请求回来把新授权
    /// 标成 `invalid`，用户看到的是"刚存就失效"；后一种更糟——账号 id 相同、
    /// `auth_epoch` 又都从 0 起，只比 epoch 会写进**另一个账号**。
    /// 核验与写入是同一条语句，没有插队窗口。
    ///
    /// 返回 `false` 表示"账号已经不是那一个了"或授权变了，此时**什么都不写**。
    pub fn set_qoder_auth_status_if_current(
        &mut self,
        account_id: &str,
        source: &str,
        auth_status: &str,
        expected: &QoderLifecycle,
    ) -> RpcResult<bool> {
        if !AUTH_STATUSES.contains(&auth_status) {
            return Err(RpcError::invalid_params(format!(
                "authStatus 取值非法：{auth_status}"
            )));
        }
        let changed = self.conn.execute(
            "UPDATE qoder_authorizations SET auth_status = ?1, updated_at = ?2
             WHERE account_id = ?3 AND source = ?4
               AND EXISTS (SELECT 1 FROM qoder_accounts
                           WHERE id = ?3 AND generation = ?5 AND auth_epoch = ?6)",
            params![
                auth_status,
                now_rfc3339(),
                account_id,
                source,
                expected.generation,
                expected.auth_epoch
            ],
        )?;
        Ok(changed > 0)
    }

    pub fn get_qoder_authorization(
        &self,
        account_id: &str,
        source: &str,
    ) -> RpcResult<Option<QoderAuthRecord>> {
        let sql = format!(
            "SELECT {AUTH_COLUMNS} FROM qoder_authorizations WHERE account_id = ?1 AND source = ?2"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        Ok(stmt
            .query_row(params![account_id, source], map_auth)
            .optional()?)
    }

    pub fn list_qoder_authorizations(&self, account_id: &str) -> RpcResult<Vec<QoderAuthRecord>> {
        let sql = format!(
            "SELECT {AUTH_COLUMNS} FROM qoder_authorizations WHERE account_id = ?1 ORDER BY source ASC"
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![account_id], map_auth)?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    ///
    /// **生产路径一律走 [`Self::delete_qoder_authorization_if_current`]**：无条件删一份
    /// 授权会把并发写入的新授权一起带走（复审 P0-4）。这里保留给"刚建好就要撤"的
    /// 场景与测试。
    #[allow(dead_code)]
    pub fn delete_qoder_authorization(
        &mut self,
        account_id: &str,
        source: &str,
    ) -> RpcResult<bool> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "DELETE FROM qoder_authorizations WHERE account_id = ?1 AND source = ?2",
            params![account_id, source],
        )?;
        if changed > 0 {
            // 删掉一份来源也是授权事实变化：在途快照作废，账号若已无任何来源，
            // 更不该留下"旧令牌取回的数"。
            tx.execute(
                "UPDATE qoder_accounts SET auth_epoch = auth_epoch + 1, updated_at = ?1
                 WHERE id = ?2",
                params![now_rfc3339(), account_id],
            )?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    /// 带核验的删除：**只有"还是我观察到的那一行"才删**。
    ///
    /// 单独删一份授权（例如移除 PAT）与"重新保存同一来源"可以重叠。无核验的
    /// `DELETE WHERE account_id + source` 会把别人刚写好的**新**授权一起删掉，
    /// 留下一份没人引用的文件、而界面以为保存成功了。
    ///
    /// 核验项：同一 `secret_ref` + 同一 `auth_version`（行版本）+ 同一 `generation`。
    ///
    /// **不用 `auth_epoch`**：它是账号级的，别的来源（例如设备授权）写入也会推进
    /// 它，那样"移除 PAT"会被一次无关的写入挡下来。行版本才是"这一份凭据"的版本。
    /// 返回 `false` = 已经被改过，**什么都没删**。
    pub fn delete_qoder_authorization_if_current(
        &mut self,
        account_id: &str,
        source: &str,
        secret_ref: Option<&str>,
        auth_version: i64,
        generation: &str,
    ) -> RpcResult<bool> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "DELETE FROM qoder_authorizations
              WHERE account_id = ?1 AND source = ?2
                AND secret_ref IS ?3 AND auth_version = ?4
                AND EXISTS (SELECT 1 FROM qoder_accounts
                            WHERE id = ?1 AND generation = ?5)",
            params![account_id, source, secret_ref, auth_version, generation],
        )?;
        if changed > 0 {
            tx.execute(
                "UPDATE qoder_accounts SET auth_epoch = auth_epoch + 1, updated_at = ?1
                 WHERE id = ?2",
                params![now_rfc3339(), account_id],
            )?;
        }
        tx.commit()?;
        Ok(changed > 0)
    }

    /// 把一行授权按原样放回去（删凭据失败时的补偿），状态改成 `missing`。
    ///
    /// 为什么需要补偿：单独移除一份授权时**先摘引用、再销毁凭据**，这样并发的
    /// "重新保存"不可能被误删。代价是销毁那一步失败时行已经没了，必须放回去，
    /// 否则凭据变成没人认领的孤儿。放回去的行标成 `missing`：界面能看出
    /// "凭据没删干净、可重试"，`credits` 也不会再拿它去发请求（见 `resolve_token`）。
    ///
    /// ## 必须带生命周期
    ///
    /// 账号 id 是 `(edition, uid)` 的确定性哈希：删号重建会得到**同一个 id**。
    /// 没有守卫时，一次迟到的补偿会把旧凭据的引用写进**新**生命周期账号
    /// （复审 P1-D）。所以这里要求 `observed` 的 `generation` 仍然相等。
    ///
    /// **只比 `generation`，不比 `auth_epoch`**：删掉这一行本身就会推进 epoch，
    /// 补偿时看到的必然是新值，拿旧 epoch 去比永远不成立。`generation` 才是
    /// "还是那一个账号"的标识；epoch 变化只说明别人又写过授权，那种情况下
    /// 唯一键会让这次 INSERT 落空，不会覆盖更新的授权。
    pub fn restore_qoder_authorization(
        &mut self,
        row: &QoderAuthRecord,
        status: &str,
        observed: &QoderLifecycle,
    ) -> RpcResult<bool> {
        if !AUTH_STATUSES.contains(&status) {
            return Err(RpcError::invalid_params(format!(
                "authStatus 取值非法：{status}"
            )));
        }
        let changed = self.conn.execute(
            "INSERT INTO qoder_authorizations (
                 id, account_id, source, source_key, secret_ref, auth_status,
                 token_expires_at, fingerprint, created_at, updated_at, auth_version
             )
             SELECT ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                    -- 恢复出来的行也要拿一个**比旧版本大**的版本（审计 P0-4）：
                    -- 用旧版本号复活，等于让迟到的旧操作重新命中这一行。
                    (SELECT auth_epoch + 1 FROM qoder_accounts WHERE id = ?2)
              WHERE EXISTS (SELECT 1 FROM qoder_accounts
                            WHERE id = ?2 AND generation = ?11)
               AND NOT EXISTS (SELECT 1 FROM qoder_authorizations
                               WHERE account_id = ?2 AND source = ?3)",
            params![
                row.id,
                row.account_id,
                row.source,
                row.source_key,
                row.secret_ref,
                status,
                row.token_expires_at,
                row.fingerprint,
                row.created_at,
                now_rfc3339(),
                observed.generation
            ],
        )?;
        Ok(changed > 0)
    }

    /// 当前生命周期。取数（或发 POST）前读一次，写回时带上做原子核验。
    pub fn qoder_lifecycle(&self, account_id: &str) -> RpcResult<Option<QoderLifecycle>> {
        let mut stmt = self
            .conn
            .prepare("SELECT generation, auth_epoch FROM qoder_accounts WHERE id = ?1")?;
        Ok(stmt
            .query_row(params![account_id], |row| {
                Ok(QoderLifecycle {
                    generation: row.get(0)?,
                    auth_epoch: row.get(1)?,
                })
            })
            .optional()?)
    }

    /// 推进同一生命周期内的授权版本号，返回推进后的生命周期。
    ///
    /// 删号前调用：让**在途**的额度快照与领取请求立刻作废。`generation` 不动——
    /// 它代表"账号还是这一个"，删号本身会让整行连同它一起消失。
    /// 不回滚：它只让旧请求失效，留着比退回去安全。
    pub fn bump_qoder_auth_epoch(&mut self, account_id: &str) -> RpcResult<Option<QoderLifecycle>> {
        let changed = self.conn.execute(
            "UPDATE qoder_accounts SET auth_epoch = auth_epoch + 1, updated_at = ?1 WHERE id = ?2",
            params![now_rfc3339(), account_id],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        self.qoder_lifecycle(account_id)
    }

    /// 无条件删账号（外键级联删授权行）。**生产路径不走它**：删号要按引用清理
    /// 凭据、还要带生命周期条件，见 `providers::qoder_admin`。这里保留给测试与
    /// 一次性数据修复。
    #[allow(dead_code)]
    pub fn delete_qoder_account(&mut self, id: &str) -> RpcResult<bool> {
        // 与 accounts 一样：快照表无外键，删账号时显式清快照。
        self.purge_account_quota_snapshots(id)?;
        let changed = self
            .conn
            .execute("DELETE FROM qoder_accounts WHERE id = ?1", params![id])?;
        Ok(changed > 0)
    }

    /// 失败清理：**只删"还是我这一代、且确实一条授权都没有"的账号行**。
    ///
    /// 单条语句，所以"没有授权行"的判断与删除是原子的。`generation` 条件挡住
    /// 同一个 UID 删了又建的账号：旧调用拿到的 generation 已经不匹配，删不掉
    /// 新账号（审计 P1-1）。
    ///
    /// 返回 `true` 表示真的删掉了；`false` 表示"那已经不是我建的账号了"或
    /// "它已经有授权行"，两种都**不该由这次失败清理处理**。
    pub fn delete_qoder_account_if_empty(&mut self, id: &str, generation: &str) -> RpcResult<bool> {
        let changed = self.conn.execute(
            "DELETE FROM qoder_accounts
              WHERE id = ?1 AND generation = ?2
                AND NOT EXISTS (SELECT 1 FROM qoder_authorizations WHERE account_id = ?1)",
            params![id, generation],
        )?;
        if changed == 0 {
            return Ok(false);
        }
        // 与 `delete_qoder_account` 一致：快照表无外键，删账号后显式清。
        self.purge_account_quota_snapshots(id)?;
        Ok(true)
    }

    /// 带生命周期核验的删账号：**只有账号还停在我推进过的那一代才删**。
    ///
    /// 删号分两步：先按授权计划清凭据，再删账号行。两步之间可能有新的授权写入
    /// （`persist` 会读到推进后的生命周期，因此可以合法提交），那份新凭据不在
    /// 计划里，删完账号行就成了无人引用的有效令牌（复审 P0-B）。
    ///
    /// 所以最后这一步必须带条件：`generation` 必须是同一个账号，`auth_epoch`
    /// 必须还是**我推进之后**的值。任何一次新的授权写入都会推进 epoch，于是这里
    /// 返回 `false`，调用方据此拒绝删除并保留可重试的记录 —— 新凭据要么还在计划外
    /// 的那一行上被引用着，要么下一次删除把它算进计划。
    ///
    /// 反过来，如果这里返回 `true`，账号行连同授权行一起消失，此后任何在途的
    /// `persist` 提交都会因为 `EXISTS(generation, epoch)` 不成立而落空，并自行
    /// 清掉它刚建的文件 —— 不会留下孤儿。
    pub fn delete_qoder_account_if_current(
        &mut self,
        id: &str,
        expected: &QoderLifecycle,
    ) -> RpcResult<bool> {
        let tx = self.conn.transaction()?;
        let changed = tx.execute(
            "DELETE FROM qoder_accounts
              WHERE id = ?1 AND generation = ?2 AND auth_epoch = ?3",
            params![id, expected.generation, expected.auth_epoch],
        )?;
        if changed == 0 {
            // 什么都没删：不碰快照，`tx` 回滚。
            return Ok(false);
        }
        tx.commit()?;
        // 与 `delete_qoder_account` 一致：快照表无外键，删账号后显式清。
        self.purge_account_quota_snapshots(id)?;
        Ok(true)
    }
}

/// 唯一键冲突。竞争的**正常**结果，不是"数据库坏了"。
fn is_constraint_violation(err: &rusqlite::Error) -> bool {
    matches!(
        err,
        rusqlite::Error::SqliteFailure(inner, _)
            if inner.code == rusqlite::ErrorCode::ConstraintViolation
    )
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

    #[test]
    fn same_uid_on_two_editions_is_two_accounts() {
        let (mut db, _dir) = db();
        let cn = db
            .upsert_qoder_account("qoder_cn", "u-1", Some("CN 账号"), None)
            .unwrap();
        let global = db
            .upsert_qoder_account("qoder_global", "u-1", Some("Global 账号"), None)
            .unwrap();
        assert_ne!(cn.id, global.id);
        assert_eq!(db.list_qoder_accounts(None).unwrap().len(), 2);
        assert_eq!(db.list_qoder_accounts(Some("qoder_cn")).unwrap().len(), 1);
    }

    #[test]
    fn reimporting_keeps_user_edited_nickname() {
        let (mut db, _dir) = db();
        db.upsert_qoder_account("qoder_global", "u-1", Some("我改的名字"), None)
            .unwrap();
        let again = db
            .upsert_qoder_account("qoder_global", "u-1", Some("Magpie 的名字"), None)
            .unwrap();
        assert_eq!(again.nickname, "我改的名字");
        assert_eq!(db.list_qoder_accounts(None).unwrap().len(), 1);
    }

    #[test]
    fn two_sources_coexist_and_do_not_overwrite_each_other() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_global", "u-1", None, None)
            .unwrap();
        db.upsert_qoder_authorization(
            &account.id,
            SOURCE_MAGPIE,
            Some("qoder#2ntprc"),
            None,
            "valid",
            Some("2026-12-12T03:39:39Z"),
            None,
        )
        .unwrap();
        db.upsert_qoder_authorization(
            &account.id,
            SOURCE_OWN_BROWSER,
            None,
            Some("qoder/global/u-1"),
            "valid",
            None,
            Some("688d92f2"),
        )
        .unwrap();

        let rows = db.list_qoder_authorizations(&account.id).unwrap();
        assert_eq!(rows.len(), 2);
        let magpie = rows.iter().find(|r| r.source == SOURCE_MAGPIE).unwrap();
        assert_eq!(magpie.source_key.as_deref(), Some("qoder#2ntprc"));
        assert!(magpie.secret_ref.is_none(), "magpie 来源不许有凭据引用");
        // 再导一次 magpie 不会动 own_browser 那行。
        db.upsert_qoder_authorization(
            &account.id,
            SOURCE_MAGPIE,
            Some("qoder#2ntprc"),
            None,
            "expired",
            None,
            None,
        )
        .unwrap();
        let rows = db.list_qoder_authorizations(&account.id).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            rows.iter()
                .find(|r| r.source == SOURCE_MAGPIE)
                .unwrap()
                .auth_status,
            "expired"
        );
        assert_eq!(
            rows.iter()
                .find(|r| r.source == SOURCE_OWN_BROWSER)
                .unwrap()
                .auth_status,
            "valid"
        );
    }

    #[test]
    fn magpie_authorization_cannot_carry_a_secret() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        let err = db
            .upsert_qoder_authorization(
                &account.id,
                SOURCE_MAGPIE,
                Some("qoder-cn"),
                Some("qoder/cn/u-1"),
                "valid",
                None,
                None,
            )
            .unwrap_err();
        assert!(err.message.contains("magpie"));
    }

    #[test]
    fn authorization_needs_an_existing_account() {
        let (mut db, _dir) = db();
        assert!(db
            .upsert_qoder_authorization(
                "qa_missing",
                SOURCE_MAGPIE,
                None,
                None,
                "valid",
                None,
                None
            )
            .is_err());
    }

    #[test]
    fn deleting_an_account_cascades_its_authorizations() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_global", "u-1", None, None)
            .unwrap();
        db.upsert_qoder_authorization(
            &account.id,
            SOURCE_OWN_BROWSER,
            None,
            Some("ref"),
            "valid",
            None,
            None,
        )
        .unwrap();
        assert!(db.delete_qoder_account(&account.id).unwrap());
        assert!(db
            .list_qoder_authorizations(&account.id)
            .unwrap()
            .is_empty());
    }

    // -- 条件提交（CRED-3b）---------------------------------------------

    fn lifecycle_of(db: &Db, account_id: &str) -> QoderLifecycle {
        db.qoder_lifecycle(account_id).unwrap().expect("生命周期")
    }

    #[test]
    fn an_absent_row_can_only_be_inserted_once() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        let first = db
            .commit_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                Some("localjson:v1:aaaa-1111"),
                "valid",
                None,
                None,
                &QoderAuthExpectation::Absent {
                    lifecycle: lifecycle_of(&db, &account.id),
                },
            )
            .unwrap();
        assert!(first.is_some());

        // 另一个进程也以为是"首次"：必须放弃，而不是覆盖。
        let second = db
            .commit_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                Some("localjson:v1:bbbb-2222"),
                "valid",
                None,
                None,
                &QoderAuthExpectation::Absent {
                    // 注意这里用**最新**生命周期：唯一键才是这一条的分水岭。
                    lifecycle: lifecycle_of(&db, &account.id),
                },
            )
            .unwrap();
        assert!(second.is_none(), "竞争失败必须是 None，不是错误也不是覆盖");
        assert_eq!(
            db.get_qoder_authorization(&account.id, SOURCE_OWN_BROWSER)
                .unwrap()
                .unwrap()
                .secret_ref
                .as_deref(),
            Some("localjson:v1:aaaa-1111")
        );
    }

    #[test]
    fn a_present_row_is_only_updated_when_the_reference_still_matches() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        db.commit_qoder_authorization(
            &account.id,
            SOURCE_OWN_BROWSER,
            Some("localjson:v1:old"),
            "valid",
            None,
            None,
            &QoderAuthExpectation::Absent {
                lifecycle: lifecycle_of(&db, &account.id),
            },
        )
        .unwrap();
        let expected = QoderAuthExpectation::Present {
            secret_ref: Some("localjson:v1:old".to_string()),
            lifecycle: lifecycle_of(&db, &account.id),
            auth_version: 1,
        };

        // 引用被迁到别的文件之后，迟到的写入必须落空。
        db.commit_qoder_authorization(
            &account.id,
            SOURCE_OWN_BROWSER,
            Some("localjson:v1:moved"),
            "valid",
            None,
            None,
            &QoderAuthExpectation::Present {
                secret_ref: Some("localjson:v1:old".to_string()),
                lifecycle: expected_lifecycle(&expected),
                auth_version: expected_version(&expected),
            },
        )
        .unwrap();

        let stale = db
            .commit_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                Some("localjson:v1:late"),
                "valid",
                None,
                None,
                &expected,
            )
            .unwrap();
        assert!(stale.is_none(), "引用已变，迟到写入不许覆盖");
        assert_eq!(
            db.get_qoder_authorization(&account.id, SOURCE_OWN_BROWSER)
                .unwrap()
                .unwrap()
                .secret_ref
                .as_deref(),
            Some("localjson:v1:moved")
        );
    }

    fn expected_lifecycle(expectation: &QoderAuthExpectation) -> QoderLifecycle {
        match expectation {
            QoderAuthExpectation::Present { lifecycle, .. } => lifecycle.clone(),
            QoderAuthExpectation::Absent { .. } => panic!("必须是 Present"),
        }
    }

    fn expected_version(expectation: &QoderAuthExpectation) -> i64 {
        match expectation {
            QoderAuthExpectation::Present { auth_version, .. } => *auth_version,
            QoderAuthExpectation::Absent { .. } => panic!("必须是 Present"),
        }
    }

    /// 删号后用同一个 UID 重建：`auth_epoch` 回到 0，只有 `generation` 能挡住旧写入。
    #[test]
    fn a_recreated_account_rejects_writes_from_the_old_lifecycle() {
        let (mut db, _dir) = db();
        let old = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        db.commit_qoder_authorization(
            &old.id,
            SOURCE_OWN_BROWSER,
            Some("localjson:v1:old"),
            "valid",
            None,
            None,
            &QoderAuthExpectation::Absent {
                lifecycle: lifecycle_of(&db, &old.id),
            },
        )
        .unwrap();
        let old_lifecycle = lifecycle_of(&db, &old.id);

        db.delete_qoder_account(&old.id).unwrap();
        let fresh = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        assert_eq!(fresh.id, old.id, "账号 id 由 (edition, uid) 决定");
        assert_eq!(lifecycle_of(&db, &fresh.id).auth_epoch, 0);

        let stale = db
            .commit_qoder_authorization(
                &old.id,
                SOURCE_OWN_BROWSER,
                Some("localjson:v1:old"),
                "valid",
                None,
                None,
                &QoderAuthExpectation::Present {
                    secret_ref: Some("localjson:v1:old".to_string()),
                    lifecycle: old_lifecycle,
                    auth_version: 1,
                },
            )
            .unwrap();
        assert!(stale.is_none(), "旧生命周期的写入必须被拒");
        assert!(db
            .get_qoder_authorization(&old.id, SOURCE_OWN_BROWSER)
            .unwrap()
            .is_none());
    }

    /// 复审 P0-2：**首次授权**的预期也带生命周期，旧登录不能写进删号重建后的新账号。
    #[test]
    fn an_absent_commit_from_a_deleted_account_is_rejected() {
        let (mut db, _dir) = db();
        let old = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        let old_life = lifecycle_of(&db, &old.id);
        let stale = QoderAuthExpectation::Absent {
            lifecycle: old_life.clone(),
        };

        // 删号 + 同 UID 重建：account_id 相同，generation 变了，epoch 归零。
        db.delete_qoder_account(&old.id).unwrap();
        let fresh = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        assert_eq!(fresh.id, old.id);
        // 旧的 generation 必须**在删除前**记下：两个变量都查同一个 id，删完再比就一样了。
        assert_ne!(lifecycle_of(&db, &fresh.id).generation, old_life.generation);

        let stale_insert = db
            .commit_qoder_authorization(
                &old.id,
                SOURCE_OWN_BROWSER,
                Some("localjson:v1:stale"),
                "valid",
                None,
                None,
                &stale,
            )
            .unwrap();
        assert!(stale_insert.is_none(), "旧生命周期的首次授权必须被拒");
        assert!(
            db.get_qoder_authorization(&old.id, SOURCE_OWN_BROWSER)
                .unwrap()
                .is_none(),
            "新账号里不能出现旧会话的授权"
        );
        assert_eq!(
            lifecycle_of(&db, &fresh.id).auth_epoch,
            0,
            "被拒绝的写入不许推进版本号"
        );
    }

    /// 复审 P0-2 的另一半：别的来源推进了 epoch，旧的 `Absent` 同样必须落空。
    #[test]
    fn an_absent_commit_is_rejected_after_another_source_moved_the_epoch() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        let before = QoderAuthExpectation::Absent {
            lifecycle: lifecycle_of(&db, &account.id),
        };
        // 另一个来源（PAT）先写入，epoch 前进。
        db.commit_qoder_authorization(
            &account.id,
            SOURCE_OWN_PAT,
            Some("localjson:v1:pat"),
            "valid",
            None,
            None,
            &QoderAuthExpectation::Absent {
                lifecycle: lifecycle_of(&db, &account.id),
            },
        )
        .unwrap();

        let stale = db
            .commit_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                Some("localjson:v1:browser"),
                "valid",
                None,
                None,
                &before,
            )
            .unwrap();
        assert!(stale.is_none(), "epoch 已经变了，旧观察不许落地");
        assert!(db
            .get_qoder_authorization(&account.id, SOURCE_OWN_BROWSER)
            .unwrap()
            .is_none());
    }

    /// 复审 P0-4：带核验的删除不会带走比它更新的授权。
    #[test]
    fn a_guarded_delete_refuses_a_newer_authorization() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        db.commit_qoder_authorization(
            &account.id,
            SOURCE_OWN_PAT,
            Some("localjson:v1:old"),
            "valid",
            None,
            None,
            &QoderAuthExpectation::Absent {
                lifecycle: lifecycle_of(&db, &account.id),
            },
        )
        .unwrap();
        let stale = lifecycle_of(&db, &account.id);

        // 新的 PAT 保存：引用换了、epoch 也前进了。
        db.commit_qoder_authorization(
            &account.id,
            SOURCE_OWN_PAT,
            Some("localjson:v1:new"),
            "valid",
            None,
            None,
            &QoderAuthExpectation::Present {
                secret_ref: Some("localjson:v1:old".to_string()),
                lifecycle: stale.clone(),
                auth_version: 1,
            },
        )
        .unwrap();

        assert!(
            !db.delete_qoder_authorization_if_current(
                &account.id,
                SOURCE_OWN_PAT,
                Some("localjson:v1:old"),
                1,
                &stale.generation,
            )
            .unwrap(),
            "旧快照的删除必须落空"
        );
        let row = db
            .get_qoder_authorization(&account.id, SOURCE_OWN_PAT)
            .unwrap()
            .expect("新授权必须还在");
        assert_eq!(row.secret_ref.as_deref(), Some("localjson:v1:new"));
        assert_eq!(row.auth_status, "valid");

        // 拿最新快照就能删。
        let current = lifecycle_of(&db, &account.id);
        assert!(db
            .delete_qoder_authorization_if_current(
                &account.id,
                SOURCE_OWN_PAT,
                Some("localjson:v1:new"),
                2,
                &current.generation,
            )
            .unwrap());
        assert!(db
            .get_qoder_authorization(&account.id, SOURCE_OWN_PAT)
            .unwrap()
            .is_none());
    }

    /// 审计 P0-4：删行重建后**不能复用旧的 `auth_version`**。
    ///
    /// 版本号曾经是常数 1，删行再插入又变回 1 —— 一个在途的旧操作（带 1）就会
    /// 命中新实例。现在初值取 `账号 epoch + 1`，epoch 只增不减，所以旧版本永远
    /// 命中不到新行。
    #[test]
    fn a_reinserted_row_never_reuses_an_old_auth_version() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_cn", "u-aba", None, None)
            .unwrap();
        let first = db
            .commit_qoder_authorization(
                &account.id,
                SOURCE_OWN_PAT,
                Some("localjson:v1:one"),
                "valid",
                None,
                None,
                &QoderAuthExpectation::Absent {
                    lifecycle: lifecycle_of(&db, &account.id),
                },
            )
            .unwrap()
            .expect("首次授权");
        let old_version = first.auth_version;

        // 删掉这一行，再用同一来源重新授权（等价于"用户移除 PAT 后重新保存"）。
        assert!(db
            .delete_qoder_authorization_if_current(
                &account.id,
                SOURCE_OWN_PAT,
                Some("localjson:v1:one"),
                old_version,
                &lifecycle_of(&db, &account.id).generation,
            )
            .unwrap());
        let rebuilt = db
            .commit_qoder_authorization(
                &account.id,
                SOURCE_OWN_PAT,
                Some("localjson:v1:two"),
                "valid",
                None,
                None,
                &QoderAuthExpectation::Absent {
                    lifecycle: lifecycle_of(&db, &account.id),
                },
            )
            .unwrap()
            .expect("重建授权");
        assert!(
            rebuilt.auth_version > old_version,
            "重建后的版本必须更大：{} vs {}",
            rebuilt.auth_version,
            old_version
        );

        // 一个拿着旧版本的迟到操作不能碰新行。
        assert!(
            !db.set_qoder_auth_status_if_version(
                &account.id,
                SOURCE_OWN_PAT,
                "missing",
                &lifecycle_of(&db, &account.id).generation,
                old_version,
            )
            .unwrap(),
            "旧版本必须命中不到新实例"
        );
        assert_eq!(
            db.get_qoder_authorization(&account.id, SOURCE_OWN_PAT)
                .unwrap()
                .unwrap()
                .auth_status,
            "valid"
        );
    }

    /// 审计 P1-1：失败清理只能删"我建的那一代、且一条授权都没有"的账号。
    #[test]
    fn cleanup_refuses_a_rebuilt_account_or_one_with_authorizations() {
        let (mut db, _dir) = db();
        let old = db
            .upsert_qoder_account("qoder_cn", "u-clean", None, None)
            .unwrap();
        let stale_generation = old.generation.clone();
        db.delete_qoder_account(&old.id).unwrap();

        // 同一个 UID 重建：id 相同、generation 不同。
        let fresh = db
            .upsert_qoder_account("qoder_cn", "u-clean", None, None)
            .unwrap();
        assert_eq!(fresh.id, old.id);
        assert!(
            !db.delete_qoder_account_if_empty(&fresh.id, &stale_generation)
                .unwrap(),
            "旧 generation 的清理不许删新账号"
        );
        assert!(db.get_qoder_account(&fresh.id).unwrap().is_some());

        // 同一代但已经有授权行：也不许删（那是别人的账号，不是"我建的空账号"）。
        db.commit_qoder_authorization(
            &fresh.id,
            SOURCE_OWN_PAT,
            Some("localjson:v1:p"),
            "valid",
            None,
            None,
            &QoderAuthExpectation::Absent {
                lifecycle: lifecycle_of(&db, &fresh.id),
            },
        )
        .unwrap();
        assert!(
            !db.delete_qoder_account_if_empty(&fresh.id, &fresh.generation)
                .unwrap(),
            "有授权行时不许删"
        );
        assert!(db.get_qoder_account(&fresh.id).unwrap().is_some());
    }

    /// 补偿写入：删凭据失败时把行以 `missing` 放回去，可重试。
    #[test]
    fn a_restored_authorization_is_marked_missing_and_retryable() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        let row = db
            .commit_qoder_authorization(
                &account.id,
                SOURCE_OWN_PAT,
                Some("localjson:v1:p"),
                "valid",
                None,
                None,
                &QoderAuthExpectation::Absent {
                    lifecycle: lifecycle_of(&db, &account.id),
                },
            )
            .unwrap()
            .unwrap();
        let observed = lifecycle_of(&db, &account.id);
        db.delete_qoder_account(&account.id).unwrap();
        assert!(
            !db.restore_qoder_authorization(&row, "missing", &observed)
                .unwrap(),
            "账号不在时放不回去（EXISTS 不成立），调用方要留痕"
        );

        let account = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        // 复审 P1-D：账号已经换了 generation，旧行**不许**复活到新生命周期。
        assert!(
            !db.restore_qoder_authorization(&row, "missing", &observed)
                .unwrap(),
            "旧生命周期的补偿必须落空"
        );
        assert!(
            db.get_qoder_authorization(&account.id, SOURCE_OWN_PAT)
                .unwrap()
                .is_none(),
            "新账号里不能出现旧凭据的引用"
        );

        // 同一代账号（模拟"删行但没删账号"）才能放回去。
        let current = lifecycle_of(&db, &account.id);
        assert!(db
            .restore_qoder_authorization(&row, "missing", &current)
            .unwrap());
        let restored = db
            .get_qoder_authorization(&account.id, SOURCE_OWN_PAT)
            .unwrap()
            .expect("行必须回来");
        assert_eq!(restored.auth_status, "missing");
        assert_eq!(restored.secret_ref.as_deref(), Some("localjson:v1:p"));
        // 幂等：再放一次不会覆盖已有行。
        assert!(!db
            .restore_qoder_authorization(&row, "missing", &current)
            .unwrap());
    }

    #[test]
    fn committing_advances_the_epoch_and_reference_counts_are_exact() {
        let (mut db, _dir) = db();
        let account = db
            .upsert_qoder_account("qoder_cn", "u-1", None, None)
            .unwrap();
        let before = lifecycle_of(&db, &account.id);
        db.commit_qoder_authorization(
            &account.id,
            SOURCE_OWN_PAT,
            Some("localjson:v1:p"),
            "valid",
            None,
            None,
            &QoderAuthExpectation::Absent {
                lifecycle: before.clone(),
            },
        )
        .unwrap();
        let after = lifecycle_of(&db, &account.id);
        assert_eq!(after.auth_epoch, before.auth_epoch + 1);
        assert_eq!(after.generation, before.generation, "换授权不换生命周期");
        assert_eq!(
            db.qoder_authorizations_referencing("localjson:v1:p")
                .unwrap(),
            1
        );
        assert_eq!(
            db.qoder_authorizations_referencing("localjson:v1:nope")
                .unwrap(),
            0
        );
    }

    #[test]
    fn token_columns_do_not_exist() {
        let (db, _dir) = db();
        let mut stmt = db
            .conn
            .prepare("PRAGMA table_info(qoder_authorizations)")
            .unwrap();
        let cols: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .filter_map(Result::ok)
            .collect();
        assert!(
            !cols
                .iter()
                .any(|c| c.contains("token") && c != "token_expires_at"),
            "cols={cols:?}"
        );
    }
}
