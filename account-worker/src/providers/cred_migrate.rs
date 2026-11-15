//! CRED-4：旧 Keychain 凭据的**显式迁移**。
//!
//! ## 这一阶段解决什么
//!
//! CRED-3a / 3b 之后新授权都落本地 JSON，但之前登录的账号仍然是
//! `keychain:` 引用。它们每次读取都可能触发一次系统授权框（ad-hoc 签名换
//! 构建就重置 ACL），这正是整个 V3 要解决的事。可是**不能自动迁移**：
//!
//! * 读钥匙串会弹框，后台任务里弹框等于把 RPC 队列卡住；
//! * 明文落盘是不可逆的风险转移，必须用户自己点头。
//!
//! 所以这里只提供四个动作，全部由用户显式触发：盘点 / 迁移 / 核验 / 回滚。
//!
//! ## 四条硬规则
//!
//! 1. **不自动迁移**：没有定时任务、没有启动时迁移，只有 RPC。
//! 2. **迁移前备份**：`VACUUM INTO` 出一份库快照 + 一份清单（清单**不含令牌**）。
//! 3. **核验成功前不删钥匙串**：整个 CRED-4 一个 Keychain 条目都不删；
//!    回滚的依据就是"原条目还在"。
//! 4. **迁移是可回滚的**：回滚 = 把引用改回 `keychain:` + 删掉本次新建的
//!    JSON 文件（先确认没有任何行引用它）。
//!
//! ## 为什么会话要进凭据事务
//!
//! 迁移要"读钥匙串 → 写 JSON → 条件提交引用"。这三步之间如果被别人插进来
//! 重新授权，就会出现"行指向新文件、文件里是旧令牌"。所以整条迁移和
//! `persist` / `remove` / `delete` 用同一把锁（`qoder_secret::begin`）。

use std::path::PathBuf;

use serde_json::{json, Value};

use crate::app::App;
use crate::auth::ids;
use crate::credentials::localjson::{CredentialKind, LocalJsonStore, SecretDraft, SecretSource};
use crate::credentials::reference;
use crate::providers::qoder_edition::QoderEdition;
use crate::providers::qoder_secret;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::authz::{
    AuthorizationRecord, STORAGE_KEYCHAIN, STORAGE_LOCALJSON, STORAGE_MAGPIE,
};
use crate::storage::qoder_accounts::QoderAuthRecord;

pub const DOMAIN_WORKBUDDY: &str = "workbuddy";
pub const DOMAIN_QODER: &str = "qoder";

/// 不可迁移的原因码。界面直接按码给文案，不猜。
const BLOCKED_ALREADY_JSON: &str = "already_json";
const BLOCKED_MAGPIE: &str = "magpie_managed";
const BLOCKED_REF_UNUSABLE: &str = "reference_unusable";

/// 迁移范围。
#[derive(Debug, Clone, Default)]
pub struct Scope {
    /// `workbuddy` / `qoder`；`None` = 两边都做。
    pub domain: Option<String>,
    /// 只处理这一个账号（`accounts.id` 或 `qoder_accounts.id`）。
    pub account_id: Option<String>,
}

impl Scope {
    pub fn parse(params: &Value) -> RpcResult<Self> {
        let domain = match params.get("domain").and_then(Value::as_str) {
            None => None,
            Some(value) if value == DOMAIN_WORKBUDDY || value == DOMAIN_QODER => {
                Some(value.to_string())
            }
            Some(other) => {
                return Err(RpcError::invalid_params(format!(
                    "domain 只支持 workbuddy / qoder，收到 {other}"
                )))
            }
        };
        let account_id = params
            .get("accountId")
            .and_then(Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string);
        Ok(Self { domain, account_id })
    }

    fn covers(&self, domain: &str, account_id: &str) -> bool {
        if let Some(wanted) = self.domain.as_deref() {
            if wanted != domain {
                return false;
            }
        }
        match self.account_id.as_deref() {
            Some(wanted) => wanted == account_id,
            None => true,
        }
    }

    fn to_json(&self) -> Value {
        json!({
            "domain": self.domain,
            "accountId": self.account_id,
        })
    }
}

/// 盘点里的一条。**不含任何令牌内容**：引用是文件 ID / 键名，指纹是摘要。
struct Entry {
    domain: &'static str,
    platform: String,
    account_id: String,
    nickname: String,
    source: String,
    storage: &'static str,
    reference: Option<String>,
    fingerprint: Option<String>,
    blocked: Option<&'static str>,
    row: Row,
}

enum Row {
    Workbuddy(Box<AuthorizationRecord>),
    Qoder(Box<QoderAuthRecord>),
}

impl Entry {
    fn migratable(&self) -> bool {
        self.blocked.is_none() && self.storage == STORAGE_KEYCHAIN
    }

    fn to_json(&self) -> Value {
        json!({
            "domain": self.domain,
            "platform": self.platform,
            "accountId": self.account_id,
            "nickname": self.nickname,
            "source": self.source,
            "storage": self.storage,
            "reference": self.reference,
            "fingerprint": self.fingerprint,
            "migratable": self.migratable(),
            "blocked": self.blocked,
        })
    }
}

/// 读全量状态：WorkBuddy 的 `account_authorizations` + Qoder 的
/// `qoder_authorizations`，逐条判定能不能迁。
fn collect(app: &App) -> RpcResult<Vec<Entry>> {
    let (accounts, rows, qoder_accounts, qoder_rows) = app.with_db(|db| {
        Ok((
            db.list_accounts(&Default::default())?,
            db.list_all_authorizations()?,
            db.list_qoder_accounts(None)?,
            db.list_all_qoder_authorizations()?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let Some(account) = accounts.iter().find(|item| item.id == row.account_id) else {
            // 账号已删、行还在（外键应该级联，但库被外力改过时要能显示出来）。
            out.push(Entry {
                domain: DOMAIN_WORKBUDDY,
                platform: String::new(),
                account_id: row.account_id.clone(),
                nickname: String::new(),
                source: row.kind.clone(),
                storage: StorageView::of(&row.access_ref),
                reference: Some(row.access_ref.clone()),
                fingerprint: None,
                blocked: Some(BLOCKED_REF_UNUSABLE),
                row: Row::Workbuddy(Box::new(row)),
            });
            continue;
        };
        let storage = StorageView::of(&row.access_ref);
        let blocked = if row.kind != ids::KIND_OWN {
            Some(BLOCKED_MAGPIE)
        } else if storage == STORAGE_LOCALJSON {
            Some(BLOCKED_ALREADY_JSON)
        } else if storage == STORAGE_MAGPIE {
            Some(BLOCKED_MAGPIE)
        } else if row.access_key().is_none() {
            Some(BLOCKED_REF_UNUSABLE)
        } else {
            None
        };
        out.push(Entry {
            domain: DOMAIN_WORKBUDDY,
            platform: account.platform.clone(),
            account_id: account.id.clone(),
            nickname: account.nickname.clone(),
            source: row.kind.clone(),
            storage,
            reference: Some(row.access_ref.clone()),
            fingerprint: None,
            blocked,
            row: Row::Workbuddy(Box::new(row)),
        });
    }

    for row in qoder_rows {
        let Some(account) = qoder_accounts.iter().find(|item| item.id == row.account_id) else {
            out.push(Entry {
                domain: DOMAIN_QODER,
                platform: String::new(),
                account_id: row.account_id.clone(),
                nickname: String::new(),
                source: row.source.clone(),
                storage: StorageView::of(row.secret_ref.as_deref().unwrap_or("")),
                reference: row.secret_ref.clone(),
                fingerprint: row.fingerprint.clone(),
                blocked: Some(BLOCKED_REF_UNUSABLE),
                row: Row::Qoder(Box::new(row)),
            });
            continue;
        };
        let storage = match qoder_secret::storage_of(&row) {
            Ok(qoder_secret::Storage::Json) => STORAGE_LOCALJSON,
            Ok(qoder_secret::Storage::Keychain) => STORAGE_KEYCHAIN,
            Err(_) => "unusable",
        };
        let blocked = if !qoder_secret::is_own_source(&row.source) {
            Some(BLOCKED_MAGPIE)
        } else if storage == STORAGE_LOCALJSON {
            Some(BLOCKED_ALREADY_JSON)
        } else if storage == "unusable" {
            Some(BLOCKED_REF_UNUSABLE)
        } else if qoder_secret::verify_keychain_ref(&row, &account.id).is_err() {
            Some(BLOCKED_REF_UNUSABLE)
        } else {
            None
        };
        out.push(Entry {
            domain: DOMAIN_QODER,
            platform: platform_name(&account.edition),
            account_id: account.id.clone(),
            nickname: account.nickname.clone(),
            source: row.source.clone(),
            storage,
            reference: row.secret_ref.clone(),
            fingerprint: row.fingerprint.clone(),
            blocked,
            row: Row::Qoder(Box::new(row)),
        });
    }

    out.sort_by(|a, b| {
        (a.domain, &a.platform, &a.account_id, &a.source).cmp(&(
            b.domain,
            &b.platform,
            &b.account_id,
            &b.source,
        ))
    });
    Ok(out)
}

fn platform_name(edition: &str) -> String {
    QoderEdition::parse(edition)
        .map(|edition| qoder_secret::platform_of(edition).to_string())
        .unwrap_or_else(|| edition.to_string())
}

/// 引用前缀 → 存储种类。**不做猜测**：未知前缀单独一类。
struct StorageView;

impl StorageView {
    fn of(reference: &str) -> &'static str {
        if reference.is_empty() {
            "unusable"
        } else if reference.starts_with(reference::LOCAL_JSON_PREFIX) {
            STORAGE_LOCALJSON
        } else if reference.starts_with(reference::KEYCHAIN_PREFIX) {
            STORAGE_KEYCHAIN
        } else if reference.starts_with("magpie:") {
            STORAGE_MAGPIE
        } else {
            "unusable"
        }
    }
}

fn summary_json(entries: &[Entry]) -> Value {
    let mut migratable = 0;
    let mut already_json = 0;
    let mut magpie = 0;
    let mut blocked = 0;
    for entry in entries {
        if entry.migratable() {
            migratable += 1;
        } else if entry.storage == STORAGE_LOCALJSON {
            already_json += 1;
        } else if entry.storage == STORAGE_MAGPIE || entry.blocked == Some(BLOCKED_MAGPIE) {
            magpie += 1;
        } else {
            blocked += 1;
        }
    }
    json!({
        "migratable": migratable,
        "alreadyJson": already_json,
        "magpie": magpie,
        "blocked": blocked,
        "total": entries.len(),
    })
}

pub fn inventory(app: &App) -> RpcResult<Value> {
    let entries = collect(app)?;
    Ok(json!({
        "generatedAt": crate::clock::now_rfc3339(),
        "summary": summary_json(&entries),
        "items": entries.iter().map(Entry::to_json).collect::<Vec<_>>(),
        "note": "只盘点有授权行的凭据；没有任何授权行的旧钥匙串条目不在范围内（没有引用可依，无法安全迁移）",
    }))
}

// ---------------------------------------------------------------------------
// 迁移
// ---------------------------------------------------------------------------

/// 迁移结果。
enum Outcome {
    Migrated(String),
    Skipped(&'static str),
    Failed(String),
}

pub fn run(app: &App, params: &Value) -> RpcResult<Value> {
    run_with(app, params, || {})
}

/// `before_migrate` 是测试用的确定性交错点：它在"盘点完成、备份完成"之后、
/// 真正动手之前执行，用来把"另一个进程刚好又保存了一次授权"固定进这个窗口。
pub(crate) fn run_with(
    app: &App,
    params: &Value,
    before_migrate: impl FnOnce(),
) -> RpcResult<Value> {
    let scope = Scope::parse(params)?;
    let entries = collect(app)?;
    let targets: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.migratable() && scope.covers(entry.domain, &entry.account_id))
        .collect();

    // 备份必须在动手之前：这是"回滚到迁移前"的唯一依据。
    let backup = if targets.is_empty() {
        None
    } else {
        Some(backup_before_migration(app, &entries)?)
    };

    // 整段在凭据事务里：写 JSON 与改引用之间不能被别的写入插进来。
    let _txn = qoder_secret::begin(app)?;

    before_migrate();

    let mut items = Vec::new();
    let mut migrated = 0usize;
    let mut failed = 0usize;
    for entry in targets {
        let outcome = migrate_one(app, entry);
        match &outcome {
            Outcome::Migrated(reference) => {
                migrated += 1;
                items.push(json!({
                    "domain": entry.domain,
                    "accountId": entry.account_id,
                    "source": entry.source,
                    "outcome": "migrated",
                    "reference": reference,
                }));
            }
            Outcome::Skipped(reason) => items.push(json!({
                "domain": entry.domain,
                "accountId": entry.account_id,
                "source": entry.source,
                "outcome": "skipped",
                "reason": reason,
            })),
            Outcome::Failed(reason) => {
                failed += 1;
                items.push(json!({
                    "domain": entry.domain,
                    "accountId": entry.account_id,
                    "source": entry.source,
                    "outcome": "failed",
                    "reason": reason,
                }));
            }
        }
    }

    Ok(json!({
        "completedAt": crate::clock::now_rfc3339(),
        "scope": scope.to_json(),
        "backup": backup.map(|path| path.to_string_lossy().to_string()),
        "migrated": migrated,
        "failed": failed,
        "items": items,
    }))
}

fn migrate_one(app: &App, entry: &Entry) -> Outcome {
    match &entry.row {
        Row::Workbuddy(row) => match migrate_workbuddy(app, row) {
            Ok(reference) => Outcome::Migrated(reference),
            Err(err) => classify(err),
        },
        Row::Qoder(row) => match migrate_qoder(app, row) {
            Ok(reference) => Outcome::Migrated(reference),
            Err(err) => classify(err),
        },
    }
}

/// 迁移失败的两类：`skipped` 是"本来就不该做"（已经被别人改过），
/// `failed` 是"做了但没成"，两者都不能算成功。
enum MigrateError {
    Skipped(&'static str),
    Failed(String),
}

fn classify(err: MigrateError) -> Outcome {
    match err {
        MigrateError::Skipped(reason) => Outcome::Skipped(reason),
        MigrateError::Failed(reason) => Outcome::Failed(reason),
    }
}

fn migrate_workbuddy(app: &App, row: &AuthorizationRecord) -> Result<String, MigrateError> {
    let account = app
        .with_db(|db| db.get_account(&row.account_id))
        .map_err(|err| MigrateError::Failed(err.message.clone()))?
        .ok_or(MigrateError::Skipped("account_gone"))?;

    let key = row
        .access_key()
        .ok_or(MigrateError::Failed("引用不是钥匙串键".to_string()))?;
    // 键名必须正好是这个账号这个来源的：指向别处说明库被改脏，迁移只会把串号固化。
    if key != ids::access_key(&account.id, &row.kind) {
        return Err(MigrateError::Failed(
            "钥匙串引用指向别的键，拒绝迁移".to_string(),
        ));
    }

    let bytes = app.secure.get(&key).map_err(|err| {
        // 拒绝访问要显式失败：这种时候"迁一个空凭据过去"比不迁更糟。
        MigrateError::Failed(format!("读钥匙串失败：{}", err.message))
    })?;
    let access = String::from_utf8(bytes)
        .map_err(|_| MigrateError::Failed("钥匙串里的令牌不是合法 UTF-8".to_string()))?;
    if access.trim().is_empty() {
        return Err(MigrateError::Skipped("keychain_empty"));
    }

    // refresh：行里有 refresh_ref 就必须读得到，读不到要失败而不是静默丢能力。
    let refresh = match row.refresh_key() {
        Some(refresh_key) => match app.secure.get(&refresh_key) {
            Ok(bytes) => Some(
                String::from_utf8(bytes)
                    .map_err(|_| MigrateError::Failed("refresh 不是合法 UTF-8".to_string()))?,
            ),
            Err(err) => {
                return Err(MigrateError::Failed(format!(
                    "行里有 refresh 引用但读不到：{}",
                    err.message
                )))
            }
        },
        None => None,
    };

    let expires = row
        .token_expires_at
        .as_deref()
        .and_then(crate::auth::service::parse_rfc3339);
    let mut draft = SecretDraft::oauth(row.id.clone(), &account.platform, access);
    if let Some(token) = refresh.as_deref() {
        draft = draft.with_refresh_token(token);
    }
    if let Some(at) = expires {
        draft = draft.with_expires_at(at);
    }
    let store = LocalJsonStore::for_paths(&app.paths);
    let secret = store
        .create(draft)
        .map_err(|err| MigrateError::Failed(format!("写本地凭据失败：{}", err.message)))?;
    let new_ref = reference::local_json_ref(&secret.secret_id)
        .map_err(|err| MigrateError::Failed(err.message))?;

    let mut next = row.clone();
    next.access_ref = new_ref.clone();
    // 两个槽位指向同一个文件（与登录路径一致）。
    next.refresh_ref = refresh.as_ref().map(|_| new_ref.clone());
    let committed = app
        .with_db(|db| db.replace_authorization_cas(&next, row.version, &row.access_ref))
        .map_err(|err| MigrateError::Failed(err.message.clone()))?;
    if !committed {
        // 行在迁移期间被改过：删掉刚建的文件（还没人引用），本次算跳过。
        discard_file(&store, &secret.secret_id);
        return Err(MigrateError::Skipped("authorization_changed"));
    }
    Ok(new_ref)
}

fn migrate_qoder(app: &App, row: &QoderAuthRecord) -> Result<String, MigrateError> {
    let account = app
        .with_db(|db| db.get_qoder_account(&row.account_id))
        .map_err(|err| MigrateError::Failed(err.message.clone()))?
        .ok_or(MigrateError::Skipped("account_gone"))?;
    let write = qoder_secret::write_json_copy(app, &account, row)
        .map_err(|err| MigrateError::Failed(err.message.clone()))?;
    let observed = qoder_secret::observe(app, &account.id, &row.source)
        .map_err(|err| MigrateError::Failed(err.message.clone()))?;
    let expectation = qoder_secret::expectation_of(&observed);
    let committed = qoder_secret::commit(
        app,
        &account.id,
        &row.source,
        &write,
        &row.auth_status,
        row.token_expires_at.as_deref(),
        row.fingerprint.as_deref(),
        &expectation,
    )
    .map_err(|err| MigrateError::Failed(err.message.clone()))?;
    match committed {
        Some(record) => Ok(record.secret_ref.unwrap_or_default()),
        None => {
            write.discard_unreferenced(app);
            Err(MigrateError::Skipped("authorization_changed"))
        }
    }
}

fn discard_file(store: &LocalJsonStore, secret_id: &str) {
    if let Err(err) = store.delete(secret_id) {
        crate::handlers::log_note(&format!(
            "迁移失败后清理未引用的凭据失败（{}）：{}",
            err.kind.as_str(),
            crate::rpc::sanitize(&err.message)
        ));
    }
}

// ---------------------------------------------------------------------------
// 回滚
// ---------------------------------------------------------------------------

pub fn rollback(app: &App, params: &Value) -> RpcResult<Value> {
    let scope = Scope::parse(params)?;
    let entries = collect(app)?;
    let targets: Vec<&Entry> = entries
        .iter()
        .filter(|entry| {
            entry.storage == STORAGE_LOCALJSON
                && entry.blocked == Some(BLOCKED_ALREADY_JSON)
                && scope.covers(entry.domain, &entry.account_id)
        })
        .collect();

    let _txn = qoder_secret::begin(app)?;

    let mut items = Vec::new();
    let mut rolled_back = 0usize;
    let mut failed = 0usize;
    for entry in targets {
        match rollback_one(app, entry) {
            Ok(()) => {
                rolled_back += 1;
                items.push(json!({
                    "domain": entry.domain,
                    "accountId": entry.account_id,
                    "source": entry.source,
                    "outcome": "rolled_back",
                }));
            }
            Err(reason) => {
                failed += 1;
                items.push(json!({
                    "domain": entry.domain,
                    "accountId": entry.account_id,
                    "source": entry.source,
                    "outcome": "failed",
                    "reason": reason,
                }));
            }
        }
    }

    Ok(json!({
        "completedAt": crate::clock::now_rfc3339(),
        "scope": scope.to_json(),
        "rolledBack": rolled_back,
        "failed": failed,
        "items": items,
    }))
}

fn rollback_one(app: &App, entry: &Entry) -> Result<(), String> {
    match &entry.row {
        Row::Workbuddy(row) => rollback_workbuddy(app, row),
        Row::Qoder(row) => rollback_qoder(app, row),
    }
}

fn rollback_workbuddy(app: &App, row: &AuthorizationRecord) -> Result<(), String> {
    let old_ref = row.access_ref.clone();
    let key = ids::access_key(&row.account_id, &row.kind);
    // 原条目必须还在：CRED-4 不删钥匙串，所以"不在"只能是外力删过。
    app.secure
        .get(&key)
        .map_err(|err| format!("钥匙串里的原凭据已不可用（{}），不能回滚", err.message))?;

    let mut next = row.clone();
    next.access_ref = ids::keychain_ref(&key);
    next.refresh_ref = row
        .refresh_ref
        .as_ref()
        .map(|_| ids::keychain_ref(&ids::refresh_key(&row.account_id, &row.kind)));
    let committed = app
        .with_db(|db| db.replace_authorization_cas(&next, row.version, &row.access_ref))
        .map_err(|err| err.message.clone())?;
    if !committed {
        return Err("授权行在回滚期间被改过，请重试".to_string());
    }
    // 引用已经改回去，再确认没人引用这份文件才删。
    let referenced = app
        .with_db(|db| db.count_authorizations_referencing(&old_ref))
        .map_err(|err| err.message.clone())?;
    if referenced == 0 {
        if let Some(secret_id) = old_ref.strip_prefix(reference::LOCAL_JSON_PREFIX) {
            let store = LocalJsonStore::for_paths(&app.paths);
            if let Err(err) = store.delete(secret_id) {
                // 文件删不掉不影响正确性：引用已经回到钥匙串，留痕即可。
                crate::handlers::log_note(&format!(
                    "回滚后旧凭据文件未删除（{}）：可由一致性检查清理",
                    err.kind.as_str()
                ));
            }
        }
    }
    Ok(())
}

fn rollback_qoder(app: &App, row: &QoderAuthRecord) -> Result<(), String> {
    let account = app
        .with_db(|db| db.get_qoder_account(&row.account_id))
        .map_err(|err| err.message.clone())?
        .ok_or_else(|| "账号不存在".to_string())?;
    let keys = qoder_secret::keychain_keys(&account.id, &row.source);
    let primary = keys
        .first()
        .ok_or_else(|| "这条来源没有钥匙串键".to_string())?;
    app.secure
        .get(primary)
        .map_err(|err| format!("钥匙串里的原凭据已不可用（{}），不能回滚", err.message))?;

    let target = qoder_secret::keychain_reference(&account.id, &row.source);
    let old_ref = row.secret_ref.clone().unwrap_or_default();
    let observed =
        qoder_secret::observe(app, &account.id, &row.source).map_err(|err| err.message.clone())?;
    let expectation = qoder_secret::expectation_of(&observed);
    let committed = qoder_secret::commit_reference(
        app,
        &account.id,
        &row.source,
        &target,
        &row.auth_status,
        row.token_expires_at.as_deref(),
        row.fingerprint.as_deref(),
        &expectation,
    )
    .map_err(|err| err.message.clone())?;
    if committed.is_none() {
        return Err("授权行在回滚期间被改过，请重试".to_string());
    }

    if let Some(secret_id) = old_ref.strip_prefix(reference::LOCAL_JSON_PREFIX) {
        let referenced = app
            .with_db(|db| db.qoder_authorizations_referencing(&old_ref))
            .map_err(|err| err.message.clone())?;
        if referenced == 0 {
            let store = LocalJsonStore::for_paths(&app.paths);
            if let Err(err) = store.delete(secret_id) {
                crate::handlers::log_note(&format!(
                    "回滚后旧凭据文件未删除（{}）：可由一致性检查清理",
                    err.kind.as_str()
                ));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 核验
// ---------------------------------------------------------------------------

/// 逐条核验 + 孤儿文件扫描。
///
/// * `localjson` 行：读回文件、验归属、令牌非空、指纹与行一致；
/// * `keychain` 行：报 `pending`（还没迁，不算错）；
/// * 目录里没有任何行引用的文件 → `orphans`（**只报不删**，清理是显式动作）。
pub fn verify(app: &App) -> RpcResult<Value> {
    let entries = collect(app)?;
    let store = LocalJsonStore::for_paths(&app.paths);

    let mut items = Vec::new();
    let mut ok = 0usize;
    let mut broken = 0usize;
    let mut pending = 0usize;

    for entry in &entries {
        match entry.storage {
            STORAGE_LOCALJSON => match verify_one(app, entry, &store) {
                Ok(()) => {
                    ok += 1;
                    items.push(json!({
                        "domain": entry.domain,
                        "accountId": entry.account_id,
                        "source": entry.source,
                        "state": "ok",
                    }));
                }
                Err(reason) => {
                    broken += 1;
                    items.push(json!({
                        "domain": entry.domain,
                        "accountId": entry.account_id,
                        "source": entry.source,
                        "state": "broken",
                        "reason": reason,
                    }));
                }
            },
            STORAGE_KEYCHAIN => {
                pending += 1;
                items.push(json!({
                    "domain": entry.domain,
                    "accountId": entry.account_id,
                    "source": entry.source,
                    "state": "pending_keychain",
                }));
            }
            // magpie 归 Magpie，迁移不涉及，也不算"待处理"。
            _ => {}
        }
    }

    // 孤儿：文件存在，但没有任何授权行引用它（迁移或删除中途被打断的产物）。
    let referenced = referenced_refs(app)?;
    let mut orphans = Vec::new();
    let report = store
        .scan()
        .map_err(|err| RpcError::new(crate::rpc::E_SECURESTORE, err.message))?;
    for secret in &report.secrets {
        let reference = reference::local_json_ref(&secret.secret_id)
            .map_err(|err| RpcError::internal(err.message))?;
        if !referenced.iter().any(|item| item == &reference) {
            orphans.push(json!({
                "secretId": secret.secret_id,
                "authorizationId": secret.authorization_id,
                "platform": secret.platform,
                "kind": match secret.credential_kind {
                    CredentialKind::Oauth => "oauth",
                    CredentialKind::Pat => "pat",
                },
                "source": match secret.source {
                    SecretSource::Own => "own",
                },
                "revision": secret.revision,
            }));
        }
    }

    Ok(json!({
        "checkedAt": crate::clock::now_rfc3339(),
        "ok": ok,
        "broken": broken,
        "pendingKeychain": pending,
        "orphans": orphans,
        "brokenFiles": report.broken.len(),
        "items": items,
    }))
}

fn verify_one(app: &App, entry: &Entry, store: &LocalJsonStore) -> Result<(), String> {
    let reference = entry
        .reference
        .as_deref()
        .ok_or_else(|| "行里没有引用".to_string())?;
    let secret_id = reference
        .strip_prefix(reference::LOCAL_JSON_PREFIX)
        .ok_or_else(|| "引用不是本地 JSON".to_string())?;
    let secret = store.load(secret_id).map_err(|err| err.message.clone())?;

    // 归属：文件里写的必须与这一行完全对应。
    let (expected_auth_id, expected_platform, expected_kind) = match &entry.row {
        Row::Workbuddy(row) => (
            row.id.clone(),
            entry.platform.clone(),
            CredentialKind::Oauth,
        ),
        Row::Qoder(row) => (
            qoder_secret::authorization_id(&row.account_id, &row.source),
            entry.platform.clone(),
            qoder_secret::kind_of(&row.source),
        ),
    };
    if secret.authorization_id != expected_auth_id {
        return Err("凭据文件属于另一条授权".to_string());
    }
    if secret.platform != expected_platform {
        return Err("凭据文件的平台与账号不一致".to_string());
    }
    if secret.source != SecretSource::Own || secret.credential_kind != expected_kind {
        return Err("凭据文件的来源/类型与授权不符".to_string());
    }
    let access = secret
        .token(reference::CredentialSlot::Access)
        .map_err(|err| err.message.clone())?;
    if access.trim().is_empty() {
        return Err("凭据文件里的令牌为空".to_string());
    }

    // 指纹：行里记的指纹必须就是这份文件里令牌的指纹。
    let fingerprint = entry.fingerprint.clone();
    if let Some(expected) = fingerprint {
        if expected != crate::securestore::fingerprint(access.as_bytes()) {
            return Err("行里的指纹与文件里的令牌不一致".to_string());
        }
    }

    // 读路径也要能拿到同一条令牌（防止"文件对、引用指错"这类问题）。
    let resolved = match &entry.row {
        Row::Workbuddy(row) => {
            let account = app
                .with_db(|db| db.get_account(&row.account_id))
                .map_err(|err| err.message.clone())?
                .ok_or_else(|| "账号不存在".to_string())?;
            crate::auth::owned::token(app, &account, row, reference::CredentialSlot::Access)
                .map_err(|err| err.message.clone())?
        }
        Row::Qoder(row) => {
            let account = app
                .with_db(|db| db.get_qoder_account(&row.account_id))
                .map_err(|err| err.message.clone())?
                .ok_or_else(|| "账号不存在".to_string())?;
            qoder_secret::read(app, &account, row, expected_kind)
                .map_err(|err| err.message.clone())?
        }
    };
    if resolved != access {
        return Err("读路径拿到的令牌与文件内容不一致".to_string());
    }
    Ok(())
}

/// 全库被引用的 `localjson:` 引用集合。
fn referenced_refs(app: &App) -> RpcResult<Vec<String>> {
    app.with_db(|db| {
        let mut out = Vec::new();
        for row in db.list_all_authorizations()? {
            out.push(row.access_ref.clone());
            if let Some(refresh) = row.refresh_ref.clone() {
                out.push(refresh);
            }
        }
        for row in db.list_all_qoder_authorizations()? {
            if let Some(reference) = row.secret_ref.clone() {
                out.push(reference);
            }
        }
        Ok(out)
    })
}

// ---------------------------------------------------------------------------
// 备份
// ---------------------------------------------------------------------------

/// 迁移前的备份：一份库快照 + 一份清单。**清单里没有令牌**。
fn backup_before_migration(app: &App, entries: &[Entry]) -> RpcResult<PathBuf> {
    let stamp = crate::clock::now_rfc3339().replace([':', '+'], "-");
    let dir = app
        .paths
        .backups_dir
        .join(format!("cred-migration-{stamp}"));
    std::fs::create_dir_all(&dir)?;

    let db_path = dir.join("accounts.sqlite");
    app.with_db(|db| db.backup_to(&db_path))?;

    // 清单：账号、来源、引用、指纹。**不含任何令牌内容**，所以可以放心说明位置。
    let manifest = json!({
        "createdAt": crate::clock::now_rfc3339(),
        "purpose": "CRED-4 迁移前备份。只含引用与摘要，不含令牌；原始凭据仍在系统钥匙串里。",
        "schemaVersion": crate::storage::LATEST_VERSION,
        "items": entries.iter().map(Entry::to_json).collect::<Vec<_>>(),
    });
    let manifest_path = dir.join("inventory.json");
    std::fs::write(
        &manifest_path,
        serde_json::to_vec_pretty(&manifest).map_err(|err| RpcError::internal(err.to_string()))?,
    )?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::providers::qoder_auth::{DeviceToken, Identity};
    use crate::securestore::SecureStore;

    fn paths(dir: &std::path::Path) -> Paths {
        Paths {
            db_path: dir.join("accounts.sqlite"),
            backups_dir: dir.join("backups"),
            logs_dir: dir.join("logs"),
            wb_switch_home: dir.join("wb-switch-home"),
            data_dir: dir.to_path_buf(),
            wb_home_owned: false,
        }
    }

    fn app_with(secure: SecureStore) -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = App::with_secure(paths(dir.path()), secure);
        (app, dir)
    }

    fn json_app() -> (App, tempfile::TempDir) {
        let (app, dir) = app_with(SecureStore::memory());
        crate::auth::consent::set_json_storage(&app, true).unwrap();
        (app, dir)
    }

    fn account(app: &App, uid: &str) -> crate::storage::AccountRecord {
        app.with_db(|db| {
            db.upsert_account(&crate::storage::AccountInput {
                platform: "workbuddy".into(),
                uid: uid.into(),
                nickname: Some("迁移账号".into()),
                ..Default::default()
            })
        })
        .unwrap()
    }

    fn token(uid: &str, access: &str) -> DeviceToken {
        DeviceToken {
            access_token: access.to_string(),
            refresh_token: Some("rt-1".to_string()),
            expires_at_ms: None,
            identity: Identity {
                uid: uid.to_string(),
                name: Some("迁移账号".to_string()),
                email: None,
            },
        }
    }

    /// 给 WorkBuddy 账号种一条**旧钥匙串**授权行（CRED-3a 之前的样子）。
    fn seed_keychain_row(app: &App, acc: &crate::storage::AccountRecord) -> AuthorizationRecord {
        let key = ids::access_key(&acc.id, ids::KIND_OWN);
        let refresh_key = ids::refresh_key(&acc.id, ids::KIND_OWN);
        app.secure.set(&key, b"WB-TOKEN").unwrap();
        app.secure.set(&refresh_key, b"WB-REFRESH").unwrap();
        let row = AuthorizationRecord {
            id: ids::auth_id(&acc.id, ids::KIND_OWN),
            account_id: acc.id.clone(),
            kind: ids::KIND_OWN.to_string(),
            access_ref: ids::keychain_ref(&key),
            refresh_ref: Some(ids::keychain_ref(&refresh_key)),
            auth_domain: None,
            token_expires_at: None,
            status: "valid".to_string(),
            version: 1,
            key_migration_state: crate::storage::authz::KEY_STATE_READY.to_string(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: "2026-11-01T00:00:00Z".to_string(),
            updated_at: "2026-11-01T00:00:00Z".to_string(),
        };
        app.with_db(|db| db.insert_authorization_if_absent(&row))
            .unwrap();
        row
    }

    #[test]
    fn inventory_lists_keychain_rows_as_migratable_and_never_reveals_tokens() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-inv");
        seed_keychain_row(&app, &acc);

        let value = inventory(&app).unwrap();
        assert_eq!(value["summary"]["migratable"], 1);
        let items = value["items"].as_array().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["storage"], "keychain");
        assert_eq!(items[0]["migratable"], true);
        // 盘点上不能出现令牌原文。
        let text = value.to_string();
        assert!(!text.contains("WB-TOKEN"), "盘点不许回显令牌");
        assert!(!text.contains("WB-REFRESH"));
    }

    #[test]
    fn migration_moves_the_reference_and_keeps_the_keychain_entry() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-mig");
        seed_keychain_row(&app, &acc);

        let value = run(&app, &json!({})).unwrap();
        assert_eq!(value["migrated"], 1, "{value}");
        assert!(value["backup"].as_str().is_some(), "迁移前必须有备份");

        let row = app
            .with_db(|db| db.find_authorization(&acc.id, ids::KIND_OWN))
            .unwrap()
            .unwrap();
        assert!(row.access_ref.starts_with("localjson:v1:"), "{row:?}");
        assert_eq!(row.storage_kind(), STORAGE_LOCALJSON);
        // 原钥匙串条目**一个都不许删**：它是回滚依据。
        assert_eq!(
            app.secure
                .get(&ids::access_key(&acc.id, ids::KIND_OWN))
                .unwrap(),
            b"WB-TOKEN"
        );
        // 读路径拿到的就是搬过去的令牌。
        assert_eq!(
            crate::auth::owned::token(&app, &acc, &row, reference::CredentialSlot::Access).unwrap(),
            "WB-TOKEN"
        );

        // 核验：一条 ok、零孤儿。
        let report = verify(&app).unwrap();
        assert_eq!(report["ok"], 1, "{report}");
        assert_eq!(report["broken"], 0);
        assert!(report["orphans"].as_array().unwrap().is_empty());
    }

    #[test]
    fn rollback_restores_the_keychain_reference_and_removes_the_copy() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-rb");
        seed_keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let migrated = app
            .with_db(|db| db.find_authorization(&acc.id, ids::KIND_OWN))
            .unwrap()
            .unwrap();
        let secret_id = migrated
            .access_ref
            .strip_prefix("localjson:v1:")
            .unwrap()
            .to_string();

        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["rolledBack"], 1, "{value}");

        let row = app
            .with_db(|db| db.find_authorization(&acc.id, ids::KIND_OWN))
            .unwrap()
            .unwrap();
        assert!(row.access_ref.starts_with("keychain:"), "{row:?}");
        assert!(row.refresh_ref.as_deref().unwrap().starts_with("keychain:"));
        // 回滚后那份复制品应当被删掉（没人引用）。
        assert!(LocalJsonStore::for_paths(&app.paths)
            .load(&secret_id)
            .is_err());
        assert_eq!(
            app.secure
                .get(&ids::access_key(&acc.id, ids::KIND_OWN))
                .unwrap(),
            b"WB-TOKEN"
        );
    }

    #[test]
    fn a_denied_keychain_read_is_a_failure_not_an_empty_migration() {
        // 读全拒：迁移必须失败，且不许留下任何引用或文件。
        let (app, _dir) = app_with(SecureStore::faulty_on_read());
        let acc = app
            .with_db(|db| {
                db.upsert_account(&crate::storage::AccountInput {
                    platform: "workbuddy".into(),
                    uid: "uid-denied".into(),
                    nickname: Some("拒绝".into()),
                    ..Default::default()
                })
            })
            .unwrap();
        // 先写行（写入不受 fail_reads 影响）。
        let key = ids::access_key(&acc.id, ids::KIND_OWN);
        let row = AuthorizationRecord {
            id: ids::auth_id(&acc.id, ids::KIND_OWN),
            account_id: acc.id.clone(),
            kind: ids::KIND_OWN.to_string(),
            access_ref: ids::keychain_ref(&key),
            refresh_ref: None,
            auth_domain: None,
            token_expires_at: None,
            status: "valid".to_string(),
            version: 1,
            key_migration_state: crate::storage::authz::KEY_STATE_READY.to_string(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: "2026-11-01T00:00:00Z".to_string(),
            updated_at: "2026-11-01T00:00:00Z".to_string(),
        };
        app.with_db(|db| db.insert_authorization_if_absent(&row))
            .unwrap();
        app.secure.arm();

        let value = run(&app, &json!({})).unwrap();
        assert_eq!(value["migrated"], 0);
        assert_eq!(value["failed"], 1, "{value}");
        let row = app
            .with_db(|db| db.find_authorization(&acc.id, ids::KIND_OWN))
            .unwrap()
            .unwrap();
        assert_eq!(row.access_ref, ids::keychain_ref(&key), "引用不许动");
        // 没留下任何文件。
        assert!(LocalJsonStore::for_paths(&app.paths)
            .scan()
            .unwrap()
            .secrets
            .is_empty());
    }

    /// 迁移期间授权行被改过：本次必须跳过，而且**不许留下任何文件**。
    #[test]
    fn a_row_changed_during_migration_is_skipped_and_leaves_no_file() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-cas");
        let row = seed_keychain_row(&app, &acc);

        let value = run_with(&app, &json!({}), || {
            // 另一个窗口重新保存了一次：版本前进、引用不变。
            let mut bumped = row.clone();
            bumped.status = "valid".to_string();
            app.with_db(|db| db.replace_authorization_cas(&bumped, row.version, &row.access_ref))
                .unwrap();
        })
        .unwrap();

        assert_eq!(value["migrated"], 0, "{value}");
        assert_eq!(
            value["items"][0]["outcome"], "skipped",
            "竞争失败要显式说跳过：{value}"
        );
        assert_eq!(value["items"][0]["reason"], "authorization_changed");

        let current = app
            .with_db(|db| db.find_authorization(&acc.id, ids::KIND_OWN))
            .unwrap()
            .unwrap();
        assert!(current.access_ref.starts_with("keychain:"), "引用不许动");
        assert!(
            LocalJsonStore::for_paths(&app.paths)
                .scan()
                .unwrap()
                .secrets
                .is_empty(),
            "竞争失败不许留下没人引用的文件"
        );
    }

    /// 备份里只有引用与摘要：**不含任何令牌**。
    #[test]
    fn the_backup_manifest_never_contains_tokens() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-backup");
        seed_keychain_row(&app, &acc);

        let value = run(&app, &json!({})).unwrap();
        let dir = PathBuf::from(value["backup"].as_str().expect("备份路径"));
        assert!(dir.join("accounts.sqlite").exists(), "必须有库快照");
        let manifest = std::fs::read_to_string(dir.join("inventory.json")).unwrap();
        assert!(!manifest.contains("WB-TOKEN"), "清单不许含令牌");
        assert!(!manifest.contains("WB-REFRESH"));
        assert!(manifest.contains("keychain:"), "清单要能看出迁移前是什么");

        // 库快照能被 SQLite 打开（VACUUM INTO 产出的是完整库）。
        let copy = crate::storage::Db::open(&dir.join("accounts.sqlite")).unwrap();
        assert!(copy
            .find_authorization(&acc.id, ids::KIND_OWN)
            .unwrap()
            .is_some());
    }

    #[test]
    fn verify_reports_an_unreferenced_file_as_an_orphan() {
        let (app, _dir) = json_app();
        let store = LocalJsonStore::for_paths(&app.paths);
        let orphan = store
            .create(SecretDraft::oauth("auth-orphan", "workbuddy", "ORPHAN"))
            .unwrap();

        let report = verify(&app).unwrap();
        let orphans = report["orphans"].as_array().unwrap();
        assert_eq!(orphans.len(), 1, "{report}");
        assert_eq!(orphans[0]["secretId"], orphan.secret_id);
        // 只报不删：核验是只读动作。
        assert!(store.load(&orphan.secret_id).is_ok());
        let text = report.to_string();
        assert!(!text.contains("ORPHAN"), "核验报告不许回显令牌");
    }

    #[test]
    fn migration_is_scoped_to_one_account_when_asked() {
        let (app, _dir) = json_app();
        let a = account(&app, "uid-a");
        let b = account(&app, "uid-b");
        seed_keychain_row(&app, &a);
        seed_keychain_row(&app, &b);

        let value = run(&app, &json!({ "accountId": a.id })).unwrap();
        assert_eq!(value["migrated"], 1);
        let kept = app
            .with_db(|db| db.find_authorization(&b.id, ids::KIND_OWN))
            .unwrap()
            .unwrap();
        assert!(kept.access_ref.starts_with("keychain:"), "别的账号不许动");
    }

    /// Qoder 侧：迁移后 Credits 仍能读到同一条令牌，且钥匙串条目还在。
    #[test]
    fn qoder_keychain_authorization_migrates_and_stays_readable() {
        let (app, _dir) = app_with(SecureStore::memory());
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-q", "Q-TOKEN"))
                .unwrap();
        let row = app
            .with_db(|db| db.get_qoder_authorization(&saved.account_id, "own_browser"))
            .unwrap()
            .unwrap();
        assert!(row.secret_ref.as_deref().unwrap().starts_with("keychain:"));

        let value = run(&app, &json!({ "domain": "qoder" })).unwrap();
        assert_eq!(value["migrated"], 1, "{value}");

        let row = app
            .with_db(|db| db.get_qoder_authorization(&saved.account_id, "own_browser"))
            .unwrap()
            .unwrap();
        assert!(
            row.secret_ref
                .as_deref()
                .unwrap()
                .starts_with("localjson:v1:"),
            "{row:?}"
        );
        let account = app
            .with_db(|db| db.get_qoder_account(&saved.account_id))
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::credits::resolve_token(&app, &account).unwrap().token,
            "Q-TOKEN"
        );
        // 原钥匙串条目还在。
        assert_eq!(
            app.secure
                .get(&crate::providers::qoder_auth::access_key(&saved.account_id))
                .unwrap(),
            b"Q-TOKEN"
        );
    }
}
