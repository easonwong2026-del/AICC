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
//! ## 五条硬规则
//!
//! 1. **不自动迁移**：没有定时任务、没有启动时迁移，只有 RPC。
//! 2. **没有明文授意就不写**：`run` 自己检查 `credentials_json_consent`，
//!    不依赖界面那层确认（RPC 可以直调）。
//! 3. **迁移前备份**：`VACUUM INTO` 出一份库快照 + 一份清单（清单**不含令牌**），
//!    目录 0700、名称带随机后缀、快照做完整性检查。
//! 4. **核验成功前不删钥匙串**：整个 CRED-4 一个 Keychain 条目都不删；
//!    回滚的依据就是"原条目还在，而且还是那一份"。
//! 5. **回滚只命中迁移实例**：靠 `credential_migrations` 记录（v22）判定，
//!    绝不按"当前是 localjson"批量回滚 —— 那会把 CRED-3a/3b 正常登录得到的
//!    有效授权切回可能已经失效的旧钥匙串条目。
//!
//! ## 为什么要进凭据事务
//!
//! 迁移要"读钥匙串 → 写 JSON → 条件提交引用"。这三步之间如果被别人插进来
//! 重新授权，就会出现"行指向新文件、文件里是旧令牌"。所以**盘点、备份、
//! 迁移、回滚全部**和 `persist` / `remove` / `delete` 用同一把锁
//! （`qoder_secret::begin`）。
//!
//! ## 三条落库不变式（第六轮复审）
//!
//! 进程锁挡不住"进程被杀"，所以真正的一致性靠 SQLite 事务：
//!
//! | 不变式 | 做法 | 违反的后果 |
//! | --- | --- | --- |
//! | 换引用 ⇔ 写回滚依据 | 同一个事务（`commit_migration_*`） | 行指向新文件、却没有回滚依据 |
//! | 回滚改引用 ⇔ 标记副本待清理 | 同一个事务（`commit_rollback_*`） | 记录先删、文件没删，谁也不知道该清哪个 |
//! | 原引用必须是钥匙串键 | 写记录前 `validate_migration` 拒绝空/异构引用 | 显示"可回滚"但回滚必然失败 |
//!
//! 摘要一律用完整 SHA-256（[`crate::securestore::digest`]），而且**与写进副本的
//! 是同一份字节**：少一个 refresh 摘要，回滚就少一道"还是那一份吗"的校验。

use std::cell::Cell;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::app::App;
use crate::auth::ids;
use crate::credentials::localjson::{CredentialKind, LocalJsonStore, SecretDraft, SecretSource};
use crate::credentials::reference;
use crate::providers::qoder_edition::QoderEdition;
use crate::providers::qoder_secret;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::authz::{
    AuthorizationRecord, CredentialMigrationRecord, KEY_STATE_READY, STORAGE_KEYCHAIN,
    STORAGE_LOCALJSON, STORAGE_MAGPIE,
};
use crate::storage::qoder_accounts::{QoderAuthExpectation, QoderAuthRecord, QoderLifecycle};

pub const DOMAIN_WORKBUDDY: &str = "workbuddy";
pub const DOMAIN_QODER: &str = "qoder";

/// 不可迁移的原因码。界面按码给文案，不做字符串匹配。
const BLOCKED_ALREADY_JSON: &str = "already_json";
const BLOCKED_MAGPIE: &str = "magpie_managed";
const BLOCKED_REF_UNUSABLE: &str = "reference_unusable";
/// CRED-5：借用授权仍写着 `keychain:` 引用。
///
/// 这类行**不能**按旧凭据迁移（那会把 Magpie 的 access 复制成本机明文文件，
/// 违反 PRD §3「借用授权的 access 每次现读、不持久化」），也**不能**当成
/// `magpie_managed`（它不是"已经关联好了"，而是每次读取都会去敲钥匙串，
/// 正是弹窗与 RPC 阻塞的真正来源）。它们只需要一次显式的重新关联，
/// 把引用换成 `magpie:v1:<source_key>` —— 一个令牌都不搬。
const BLOCKED_NEEDS_RELINK: &str = "needs_relink";

/// 迁移范围。
///
/// **字段不存在才是"不限定"**：显式传了空串、数字、对象一律报错。之前用
/// `as_str()` 会让 `{"accountId": 123}` 退化成"全部账号"——一个参数写错的调用
/// 就能触发全量迁移或全量回滚。
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub domain: Option<String>,
    pub account_id: Option<String>,
}

impl Scope {
    pub fn parse(params: &Value) -> RpcResult<Self> {
        let domain = match params.get("domain") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => match value.as_str() {
                DOMAIN_WORKBUDDY | DOMAIN_QODER => Some(value.clone()),
                other => {
                    return Err(RpcError::invalid_params(format!(
                        "domain 只支持 workbuddy / qoder，收到 {other}"
                    )))
                }
            },
            Some(other) => {
                return Err(RpcError::invalid_params(format!(
                    "domain 必须是字符串，收到 {other}"
                )))
            }
        };
        let account_id = match params.get("accountId") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_string())
            }
            Some(Value::String(_)) => {
                return Err(RpcError::invalid_params(
                    "accountId 不能是空串：要处理全部账号就不要传这个字段",
                ))
            }
            Some(other) => {
                return Err(RpcError::invalid_params(format!(
                    "accountId 必须是字符串，收到 {other}"
                )))
            }
        };
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
        json!({ "domain": self.domain, "accountId": self.account_id })
    }
}

/// 盘点里的一条。**不含任何令牌内容**。
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
    /// 有迁移记录，且当前行仍是那次迁移的产物 —— 只有这种允许回滚。
    rollbackable: bool,
    row: Row,
}

enum Row {
    Workbuddy(Box<AuthorizationRecord>),
    Qoder {
        row: Box<QoderAuthRecord>,
        lifecycle: QoderLifecycle,
    },
}

impl Entry {
    fn migratable(&self) -> bool {
        self.blocked.is_none() && self.storage == STORAGE_KEYCHAIN
    }

    /// 唯一一种"可修复但不迁移"的状态：改引用即可，不动任何令牌。
    fn needs_relink(&self) -> bool {
        self.blocked == Some(BLOCKED_NEEDS_RELINK)
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
            "rollbackable": self.rollbackable,
            "blocked": self.blocked,
            // 界面据此决定给不给"重新关联"按钮，不做 blocked 字符串匹配。
            "action": if self.needs_relink() { Some("relink") } else { None },
        })
    }
}

/// 读全量状态：WorkBuddy + Qoder 两侧，逐条判定能不能迁 / 能不能回滚。
fn collect(app: &App) -> RpcResult<Vec<Entry>> {
    let (accounts, rows, qoder_accounts, qoder_rows, migrations) = app.with_db(|db| {
        Ok((
            db.list_accounts(&Default::default())?,
            db.list_all_authorizations()?,
            db.list_qoder_accounts(None)?,
            db.list_all_qoder_authorizations()?,
            db.list_credential_migrations()?,
        ))
    })?;

    let record_of =
        |domain: &str, account_id: &str, source: &str| -> Option<&CredentialMigrationRecord> {
            migrations.iter().find(|item| {
                item.domain == domain && item.account_id == account_id && item.source == source
            })
        };

    let mut out = Vec::new();
    for row in rows {
        let Some(account) = accounts.iter().find(|item| item.id == row.account_id) else {
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
                rollbackable: false,
                row: Row::Workbuddy(Box::new(row)),
            });
            continue;
        };
        let storage = StorageView::of(&row.access_ref);
        let blocked = if row.kind != ids::KIND_OWN {
            // CRED-5：借用的旧行要看**引用**，不能只看 kind。
            // 已经挂上 `magpie:v1:` 才是真的"由 Magpie 管理"；还写着 `keychain:`
            // 的每读一次就敲一次钥匙串，必须显式重新关联。
            if storage == STORAGE_KEYCHAIN {
                Some(BLOCKED_NEEDS_RELINK)
            } else {
                Some(BLOCKED_MAGPIE)
            }
        } else if storage == STORAGE_LOCALJSON {
            Some(BLOCKED_ALREADY_JSON)
        } else if storage == STORAGE_MAGPIE {
            Some(BLOCKED_MAGPIE)
        } else if row.access_key().is_none() {
            Some(BLOCKED_REF_UNUSABLE)
        } else {
            None
        };
        // 只有"记录还在 + 行仍指着我迁过去的那个文件 + 版本没动过 + 原引用是钥匙串键"
        // 才算可回滚。判定只有一个来源（`rollback_basis`），绝不在这里另写一套。
        let rollbackable = rollback_basis(
            record_of(DOMAIN_WORKBUDDY, &row.account_id, &row.kind),
            &row.access_ref,
            row.version,
        );
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
            rollbackable,
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
                rollbackable: false,
                row: Row::Qoder {
                    row: Box::new(row),
                    lifecycle: QoderLifecycle {
                        generation: String::new(),
                        auth_epoch: 0,
                    },
                },
            });
            continue;
        };
        let lifecycle = app
            .with_db(|db| db.qoder_lifecycle(&account.id))?
            .ok_or_else(|| RpcError::internal("Qoder 账号读不到生命周期"))?;
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
        let rollbackable = rollback_basis(
            record_of(DOMAIN_QODER, &row.account_id, &row.source),
            row.secret_ref.as_deref().unwrap_or_default(),
            row.auth_version,
        );
        out.push(Entry {
            domain: DOMAIN_QODER,
            platform: platform_name(&account.edition),
            account_id: account.id.clone(),
            nickname: account.nickname.clone(),
            source: row.source.clone(),
            storage,
            // 老行的 `secret_ref` 是 NULL，凭据其实在钥匙串、键名由账号派生。
            // 显示成"没有引用"会让人以为这条授权坏了（复审 P0-A）。
            reference: row
                .secret_ref
                .clone()
                .or_else(|| Some(qoder_secret::keychain_reference(&account.id, &row.source))),
            fingerprint: row.fingerprint.clone(),
            blocked,
            rollbackable,
            row: Row::Qoder {
                row: Box::new(row),
                lifecycle,
            },
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

/// 迁移记录是不是"仍然描述当前这一行"。
///
/// 四个条件缺一不可，其中前两条是复审 P0-A 的补丁：原引用为空 / 不是钥匙串键
/// 的记录**看着存在**，但回滚时根本无处可去。这种记录既不能显示成"可回滚"，
/// 也不能当成校验摘要的依据。
fn rollback_basis(
    record: Option<&CredentialMigrationRecord>,
    current_ref: &str,
    current_version: i64,
) -> bool {
    let Some(record) = record else {
        return false;
    };
    if !record.original_ref.starts_with(reference::KEYCHAIN_PREFIX) {
        return false;
    }
    // 没有原令牌摘要 = 回滚时无从核对"还是那一份"，所以不算回滚依据。
    // v22 曾经允许这条路径写下 None（二次读取失败），必须在判定这一层就挡住，
    // 否则界面会说"可回滚"，点下去才发现没有安全门（第七轮复审 P1-1）。
    if record
        .original_fingerprint
        .as_deref()
        .unwrap_or_default()
        .is_empty()
    {
        return false;
    }
    if record.migrated_ref.is_empty() || record.migrated_ref != current_ref {
        return false;
    }
    record.migrated_version == current_version
}

/// 这条授权上是否还压着一笔"回滚后副本没删掉"的债。
///
/// 债没还完就再迁移，会让新记录覆盖旧的（同一个 `id`），旧副本从此失联
/// （第七轮复审 P0-1）。所以迁移入口先问一次，并给用户一个原因码。
fn pending_cleanup_for(
    app: &App,
    domain: &str,
    account_id: &str,
    source: &str,
) -> Result<bool, MigrateError> {
    let pending = app
        .with_db(|db| db.pending_cleanup_migrations())
        .map_err(|err| MigrateError::Failed(err.message.clone()))?;
    Ok(pending.iter().any(|record| {
        record.domain == domain && record.account_id == account_id && record.source == source
    }))
}

/// 摘要比对：新记录是完整 SHA-256（64 位），早期构建写的是 8 位短指纹。
///
/// 两种都认，是为了让"已经用旧构建跑过一次迁移"的库仍然能回滚；**只写新的**，
/// 因为严格一致性才是这次要的（复审 P0-C）。
fn digest_matches(stored: &str, bytes: &[u8]) -> bool {
    if stored.len() == 64 {
        crate::securestore::digest(bytes) == stored
    } else {
        crate::securestore::fingerprint(bytes) == stored
    }
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
    let mut rollbackable = 0;
    let mut needs_relink = 0;
    for entry in entries {
        if entry.rollbackable {
            rollbackable += 1;
        }
        // `needs_relink` 单独一档：它既不是"可迁移"，也不是"引用异常"，
        // 更不是"已经由 Magpie 管理"。混进任何一个都会让界面给出错误的下一步。
        if entry.needs_relink() {
            needs_relink += 1;
        } else if entry.migratable() {
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
        "needsRelink": needs_relink,
        "blocked": blocked,
        "rollbackable": rollbackable,
        "total": entries.len(),
    })
}

pub fn inventory(app: &App) -> RpcResult<Value> {
    let entries = collect(app)?;
    // "已迁移"只认记录数：CRED-3 之后原生新建的 JSON 授权不是迁移来的。
    let migrated_records = app
        .with_db(|db| db.credential_migration_record_count())
        .unwrap_or(0);
    Ok(json!({
        "generatedAt": crate::clock::now_rfc3339(),
        "summary": summary_json(&entries),
        "migratedRecords": migrated_records,
        "items": entries.iter().map(Entry::to_json).collect::<Vec<_>>(),
        "note": "只盘点有授权行的凭据；没有任何授权行的旧钥匙串条目不在范围内（没有引用可依，无法安全迁移）",
    }))
}

/// CRED-5：把仍指向钥匙串的**借用授权**改成 Magpie 动态关联。
///
/// 与 `credentials.migration.run` 的分工必须说清楚：
///
/// * 迁移：把 AICC **自有**令牌复制成本机 JSON（有明文风险，要同意）；
/// * 重新关联：只把 `keychain:<key>` 换成 `magpie:v1:<source_key>`，
///   **一个令牌都不读、不写、不复制**，所以不需要明文同意，
///   也不会创建任何 `secrets/v1/*.json`。旧钥匙串条目照旧保留。
///
/// 这才是"借用授权不再弹窗"的正解：弹窗源头是读钥匙串，换成现读 Magpie
/// 文件之后这条路就断了；而把它迁移到 JSON 反而违反 PRD §3 的
/// 「借用授权的 access 每次现读、不持久化」。
pub fn relink(app: &App, params: &Value) -> RpcResult<Value> {
    let scope = Scope::parse(params)?;
    if scope
        .domain
        .as_deref()
        .is_some_and(|domain| domain != DOMAIN_WORKBUDDY)
    {
        return Err(RpcError::invalid_params(
            "重新关联只适用于 WorkBuddy 的借用授权（Qoder 有自己的授权流程）",
        ));
    }

    // 与迁移、登录共用同一道锁：改引用与写凭据不能交错。
    let _txn = qoder_secret::begin(app)?;
    let report = crate::auth::relink::relink_pending(app, scope.account_id.as_deref())?;

    // 改完之后立刻重新盘点，界面只发一次请求就能拿到最新状态。
    let entries = collect(app)?;
    Ok(json!({
        "relinked": report.get("relinked").cloned().unwrap_or(json!(0)),
        "pending": report.get("pending").cloned().unwrap_or(json!(0)),
        "skipped": report.get("skipped").cloned().unwrap_or(json!([])),
        // 旧钥匙串条目刻意保留：这是回滚与排查的唯一依据。
        "legacyKeysKept": true,
        "summary": summary_json(&entries),
    }))
}

// ---------------------------------------------------------------------------
// 迁移
// ---------------------------------------------------------------------------

enum Outcome {
    Migrated { reference: String },
    Skipped(&'static str),
    Failed(String),
}

pub fn run(app: &App, params: &Value) -> RpcResult<Value> {
    run_with(app, params, || {})
}

/// `before_migrate` 是测试用的确定性交错点：在"盘点 + 备份"之后、"动手"之前。
pub(crate) fn run_with(
    app: &App,
    params: &Value,
    before_migrate: impl FnOnce(),
) -> RpcResult<Value> {
    let scope = Scope::parse(params)?;

    // 写明文这件事必须在后端也有授意：RPC 可以绕过界面直接调。
    if !crate::auth::consent::json_storage_enabled(app) {
        return Err(RpcError::invalid_params(
            "还没有启用本地 JSON 存储：请先在「凭据存储」里确认明文风险，再执行迁移",
        )
        .with_data(json!({ "kind": "json_storage_not_enabled" })));
    }

    // **盘点与备份都在锁内**：计划与备份必须是同一个受保护时点。
    let _txn = qoder_secret::begin(app)?;
    // 上一次回滚留下的"副本待清理"先收拾掉：这里的每一步都是幂等的，
    // 清不掉也不影响本次迁移（P1-E 的重试入口）。
    drain_pending_cleanup(app);
    let entries = collect(app)?;
    let targets: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.migratable() && scope.covers(entry.domain, &entry.account_id))
        .collect();

    let backup = if targets.is_empty() {
        None
    } else {
        Some(backup_before_migration(app, &entries)?)
    };

    before_migrate();

    let mut items = Vec::new();
    let mut migrated = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    for entry in targets {
        match migrate_one(app, entry) {
            Outcome::Migrated { reference } => {
                migrated += 1;
                items.push(json!({
                    "domain": entry.domain,
                    "accountId": entry.account_id,
                    "source": entry.source,
                    "outcome": "migrated",
                    "reference": reference,
                    // 记录与引用同一个事务写：成功就一定能回滚。
                    "rollbackable": true,
                }));
            }
            Outcome::Skipped(reason) => {
                skipped += 1;
                items.push(json!({
                    "domain": entry.domain,
                    "accountId": entry.account_id,
                    "source": entry.source,
                    "outcome": "skipped",
                    "reason": reason,
                }));
            }
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
        "skipped": skipped,
        "failed": failed,
        "items": items,
    }))
}

fn migrate_one(app: &App, entry: &Entry) -> Outcome {
    let result = match &entry.row {
        Row::Workbuddy(row) => migrate_workbuddy(app, row),
        Row::Qoder { row, lifecycle } => migrate_qoder(app, row, lifecycle),
    };
    match result {
        Ok(outcome) => outcome,
        Err(MigrateError::Skipped(reason)) => Outcome::Skipped(reason),
        Err(MigrateError::Failed(reason)) => Outcome::Failed(reason),
    }
}

enum MigrateError {
    Skipped(&'static str),
    Failed(String),
}

// 测试注入：让"写迁移记录"这一步必定失败。
//
// 用途只有一个——证明引用切换与记录写入**同生共死**（复审 P0-B）：注入之后
// 仍然必须满足"行还指着钥匙串、盘上没有副本"。
//
// 生产构建里这个开关恒为关闭：`cfg(not(test))` 版本什么都不做。
#[cfg(test)]
thread_local! {
    /// 还剩几次"写记录必定失败"的注入。线程局部，测试并行跑不会互相干扰。
    static RECORD_WRITE_FAULTS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn maybe_fault_record(record: &mut CredentialMigrationRecord) {
    let hit = RECORD_WRITE_FAULTS.with(|cell| {
        let left = cell.get();
        cell.set(left.saturating_sub(1));
        left > 0
    });
    if hit {
        // 存储层会拒绝这条记录（原引用不是钥匙串键）→ 整个事务回滚。
        record.original_ref.clear();
    }
}

#[cfg(not(test))]
fn maybe_fault_record(_record: &mut CredentialMigrationRecord) {}

/// 让下一次（`times` 次）迁移的记录写入必定失败。**仅测试可用**。
#[cfg(test)]
fn arm_record_write_fault(times: u32) {
    RECORD_WRITE_FAULTS.with(|cell| cell.set(times));
}

/// 新建的副本：**没被 keep 就一定会被清理**。
///
/// 之前只在"提交落空"这一个分支清理，`replace_authorization_cas` 返回 `Err`、
/// 读账号失败、读 refresh 失败这些路径都会留下没人引用的**明文令牌文件**。
/// 现在把清理挂在 Drop 上，任何提前返回都覆盖得到。
struct PendingCopy<'a> {
    app: &'a App,
    secret_id: String,
    reference: String,
    keep: Cell<bool>,
}

impl<'a> PendingCopy<'a> {
    fn new(app: &'a App, secret_id: String, reference: String) -> Self {
        Self {
            app,
            secret_id,
            reference,
            keep: Cell::new(false),
        }
    }

    fn keep(&self) {
        self.keep.set(true);
    }
}

impl Drop for PendingCopy<'_> {
    fn drop(&mut self) {
        if self.keep.get() {
            return;
        }
        // 只有**两张表都没有引用**才删：删的是"我这份没人要的副本"。
        let wb = self
            .app
            .with_db(|db| db.count_authorizations_referencing(&self.reference))
            .unwrap_or(1);
        let qoder = self
            .app
            .with_db(|db| db.qoder_authorizations_referencing(&self.reference))
            .unwrap_or(1);
        if wb != 0 || qoder != 0 {
            crate::handlers::log_note("迁移失败后保留了一份已被引用的凭据（不是孤儿），请人工核对");
            return;
        }
        let store = LocalJsonStore::for_paths(&self.app.paths);
        if let Err(err) = store.delete(&self.secret_id) {
            crate::handlers::log_note(&format!(
                "迁移失败后清理未引用的凭据失败（{}）：{}",
                err.kind.as_str(),
                crate::rpc::sanitize(&err.message)
            ));
        }
    }
}

fn migrate_workbuddy(app: &App, plan: &AuthorizationRecord) -> Result<Outcome, MigrateError> {
    // 第七轮复审 P0-1：同一条授权上还压着"回滚后没删掉的副本"时**不许再迁**。
    // 否则新一次的 UPSERT 会把旧记录改写成本次的，旧副本从此再无人追踪。
    // 存储层还有第二道（`write_migration` 的 UPSERT WHERE），这里是给用户的原因码。
    if pending_cleanup_for(app, DOMAIN_WORKBUDDY, &plan.account_id, &plan.kind)? {
        return Err(MigrateError::Skipped("cleanup_pending"));
    }
    let account = app
        .with_db(|db| db.get_account(&plan.account_id))
        .map_err(|err| MigrateError::Failed(err.message.clone()))?
        .ok_or(MigrateError::Skipped("account_gone"))?;

    // 锁内复核：还是计划里的那一行吗？不是就跳过，一个字节都不动。
    let current = app
        .with_db(|db| db.find_authorization(&plan.account_id, &plan.kind))
        .map_err(|err| MigrateError::Failed(err.message.clone()))?
        .ok_or(MigrateError::Skipped("authorization_gone"))?;
    if current.id != plan.id
        || current.access_ref != plan.access_ref
        || current.version != plan.version
    {
        return Err(MigrateError::Skipped("authorization_changed"));
    }

    let access_key = current
        .access_key()
        .ok_or(MigrateError::Failed("引用不是钥匙串键".to_string()))?;
    if access_key != ids::access_key(&account.id, &current.kind) {
        return Err(MigrateError::Failed(
            "钥匙串引用指向别的键，拒绝迁移".to_string(),
        ));
    }
    // **refresh 引用也要严格验归属**：只剥前缀不比对，会把别的账号的 refresh 抄过来。
    let refresh_key = match current.refresh_ref.as_deref() {
        Some(_) => {
            let key = current.refresh_key().ok_or_else(|| {
                MigrateError::Failed("refresh 引用不是合法的钥匙串键".to_string())
            })?;
            if key != ids::refresh_key(&account.id, &current.kind) {
                return Err(MigrateError::Failed(
                    "refresh 引用指向别的键，拒绝迁移".to_string(),
                ));
            }
            Some(key)
        }
        None => None,
    };

    let access = read_key(app, &access_key, None)?;
    // 行里写了 refresh 就必须读得到：读不到宁可失败，也不静默丢掉刷新能力。
    let refresh = match refresh_key.as_deref() {
        Some(key) => Some(read_key(app, key, Some("refresh"))?),
        None => None,
    };

    // 摘要来自**刚读到的这份字节**，不是再读一次（复审 P0-C）。
    let access_digest = crate::securestore::digest(access.as_bytes());
    let refresh_digest = refresh
        .as_deref()
        .map(|token| crate::securestore::digest(token.as_bytes()));

    let expires = current
        .token_expires_at
        .as_deref()
        .and_then(crate::auth::service::parse_rfc3339);
    let mut draft = SecretDraft::oauth(current.id.clone(), &account.platform, access.clone());
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
    let pending = PendingCopy::new(app, secret.secret_id.clone(), new_ref.clone());

    let mut next = current.clone();
    next.access_ref = new_ref.clone();
    next.refresh_ref = refresh.as_ref().map(|_| new_ref.clone());
    // 迁移后这份凭据就是"已搬完"的状态；漏了这一条，兼容面板会一直认为
    // 旧键还要读（G4 的 key_migration_state 语义）。
    next.key_migration_state = KEY_STATE_READY.to_string();

    // 回滚依据：原引用就是当前那行**验过归属**的钥匙串引用，绝不写空。
    let mut migration = CredentialMigrationRecord {
        id: CredentialMigrationRecord::id_of(DOMAIN_WORKBUDDY, &account.id, &current.kind),
        domain: DOMAIN_WORKBUDDY.to_string(),
        account_id: account.id.clone(),
        source: current.kind.clone(),
        original_ref: current.access_ref.clone(),
        original_fingerprint: Some(access_digest),
        original_refresh_fingerprint: refresh_digest,
        migrated_ref: new_ref.clone(),
        // CAS 成功后版本必然 +1：对着这一行算，不靠"读回来看看"。
        migrated_version: current.version + 1,
        created_at: crate::clock::now_rfc3339(),
        cleanup_pending: false,
    };
    maybe_fault_record(&mut migration);

    // **换引用 + 写记录是一个事务**（复审 P0-B）：记录不合格或写入失败时，
    // 引用也不会被改成一个没有回滚依据的新位置。
    let committed = app
        .with_db(|db| {
            db.commit_migration_workbuddy(&next, current.version, &current.access_ref, &migration)
        })
        .map_err(|err| MigrateError::Failed(err.message.clone()))?;
    if !committed {
        return Err(MigrateError::Skipped("authorization_changed"));
    }
    pending.keep();

    // 镜像列（`accounts.auth_mirror_auth_id`）跟着引用走，否则面板显示旧来源。
    if let Err(err) = crate::auth::service::sync_mirror(app, &account.id) {
        crate::handlers::log_note(&format!("迁移后同步账号镜像失败：{}", err.message));
    }

    Ok(Outcome::Migrated { reference: new_ref })
}

fn migrate_qoder(
    app: &App,
    plan: &QoderAuthRecord,
    plan_lifecycle: &QoderLifecycle,
) -> Result<Outcome, MigrateError> {
    // 同 WorkBuddy：上一轮回滚的副本还没删掉，就不许覆盖那条记录（第七轮 P0-1）。
    if pending_cleanup_for(app, DOMAIN_QODER, &plan.account_id, &plan.source)? {
        return Err(MigrateError::Skipped("cleanup_pending"));
    }
    let account = app
        .with_db(|db| db.get_qoder_account(&plan.account_id))
        .map_err(|err| MigrateError::Failed(err.message.clone()))?
        .ok_or(MigrateError::Skipped("account_gone"))?;

    // 锁内复核：行、版本、来源都得还是计划里那一条。
    let current = qoder_secret::row_of(app, &plan.account_id, &plan.source)
        .ok_or(MigrateError::Skipped("authorization_gone"))?;
    if current.id != plan.id
        || current.secret_ref != plan.secret_ref
        || current.auth_version != plan.auth_version
    {
        return Err(MigrateError::Skipped("authorization_changed"));
    }
    if plan_lifecycle.generation.is_empty() {
        return Err(MigrateError::Skipped("account_gone"));
    }

    // 原引用**只能由账号 + 来源派生**：老行的 `secret_ref` 是 NULL（键名一直是由
    // account_id 推出来的），照抄 `secret_ref` 会写出空串 —— 记录看着存在，
    // 回滚必然失败（复审 P0-A）。行里写了引用就必须正好是这一条，否则拒绝。
    let original_ref = qoder_secret::keychain_reference(&account.id, &plan.source);
    if let Some(recorded) = plan.secret_ref.as_deref() {
        if recorded != original_ref {
            return Err(MigrateError::Failed(
                "钥匙串引用不是这条来源的主键，拒绝迁移".to_string(),
            ));
        }
    }

    let copy = qoder_secret::write_json_copy(app, &account, &current)
        .map_err(|err| MigrateError::Failed(err.message.clone()))?;
    let reference = copy.write.reference().to_string();
    let secret_id = reference
        .strip_prefix(reference::LOCAL_JSON_PREFIX)
        .unwrap_or_default()
        .to_string();
    let pending = PendingCopy::new(app, secret_id, reference.clone());

    // **用计划快照做 CAS**，不拿执行时重读到的新行当预期：否则"盘点之后别人
    // 重新登录"这条交错会被当成正常提交，把新授权盖回旧令牌。
    let expectation = QoderAuthExpectation::Present {
        secret_ref: plan.secret_ref.clone(),
        lifecycle: plan_lifecycle.clone(),
        auth_version: plan.auth_version,
    };
    let mut migration = CredentialMigrationRecord {
        id: CredentialMigrationRecord::id_of(DOMAIN_QODER, &account.id, &plan.source),
        domain: DOMAIN_QODER.to_string(),
        account_id: account.id.clone(),
        source: plan.source.clone(),
        original_ref,
        // 摘要在 `write_json_copy` 里与副本**同源**算出：不读第二遍。
        original_fingerprint: Some(copy.access_digest.clone()),
        original_refresh_fingerprint: copy.refresh_digest.clone(),
        migrated_ref: reference.clone(),
        // 这次提交必然把 auth_version 推进一格，照着计划快照算。
        migrated_version: plan.auth_version + 1,
        created_at: crate::clock::now_rfc3339(),
        cleanup_pending: false,
    };
    maybe_fault_record(&mut migration);

    // 换引用 + 写记录同一个事务（复审 P0-B）。
    let committed = qoder_secret::commit_with_record(
        app,
        &account.id,
        &plan.source,
        &copy.write,
        &plan.auth_status,
        plan.token_expires_at.as_deref(),
        plan.fingerprint.as_deref(),
        &expectation,
        &migration,
    )
    .map_err(|err| MigrateError::Failed(err.message.clone()))?;
    let Some(record) = committed else {
        return Err(MigrateError::Skipped("authorization_changed"));
    };
    pending.keep();
    if record.auth_version != migration.migrated_version {
        // 记录里的版本对不上行，回滚会把它判成"迁移后被重新授权过"而拒绝。
        crate::handlers::log_note("Qoder 迁移后版本号与记录不一致：该条回滚可能被判成已重新授权");
    }

    Ok(Outcome::Migrated { reference })
}

/// 读一个钥匙串键，并按错误分类决定是"跳过"还是"失败"。
///
/// `NotFound` 有两种含义：本来就没有（可接受）与"行说有却没有"（必须失败）。
/// 调用方用 `slot` 区分：`Some("refresh")` 表示行里明确写了这个槽位。
fn read_key(app: &App, key: &str, slot: Option<&str>) -> Result<String, MigrateError> {
    match app.secure.get(key) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|_| {
            MigrateError::Failed(match slot {
                Some(name) => format!("{name} 不是合法 UTF-8"),
                None => "钥匙串里的令牌不是合法 UTF-8".to_string(),
            })
        }),
        Err(err) => Err(keychain_error(err, slot)),
    }
}

fn keychain_error(err: crate::securestore::SecretError, slot: Option<&str>) -> MigrateError {
    let name = slot.unwrap_or("access");
    match err.kind {
        // 行里写了这个槽位却读不到：不许当成"没有 refresh"。
        crate::securestore::SecretErrorKind::NotFound => {
            MigrateError::Failed(format!("{name} 引用存在但钥匙串里读不到"))
        }
        crate::securestore::SecretErrorKind::Denied => {
            MigrateError::Failed(format!("读取 {name} 被系统拒绝（授权框未允许）"))
        }
        _ => MigrateError::Failed(format!("读取 {name} 失败：{}", err.message)),
    }
}

// ---------------------------------------------------------------------------
// 回滚
// ---------------------------------------------------------------------------

pub fn rollback(app: &App, params: &Value) -> RpcResult<Value> {
    let scope = Scope::parse(params)?;
    let _txn = qoder_secret::begin(app)?;
    // 上一轮没清干净的副本先重试一次（P1-E）：回滚本身就是"清理"动作，
    // 顺手把欠账收掉比让用户自己去猜更靠谱。
    drain_pending_cleanup(app);
    let entries = collect(app)?;

    // **只回滚"有迁移记录且仍是那次迁移产物"的行**：按"当前是 localjson"筛会把
    // CRED-3a/3b 正常登录得到的有效授权一起切回旧钥匙串。
    let targets: Vec<&Entry> = entries
        .iter()
        .filter(|entry| entry.rollbackable && scope.covers(entry.domain, &entry.account_id))
        .collect();

    let mut items = Vec::new();
    let mut rolled_back = 0usize;
    let mut cleanup_pending = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;
    for entry in targets {
        match rollback_one(app, entry) {
            Ok(pending) => {
                rolled_back += 1;
                if pending {
                    cleanup_pending += 1;
                }
                items.push(json!({
                    "domain": entry.domain,
                    "accountId": entry.account_id,
                    "source": entry.source,
                    "outcome": "rolled_back",
                    // true = 引用已切回钥匙串，但副本文件没删掉；记录留着等重试。
                    "cleanupPending": pending,
                }));
            }
            Err(RollbackError::Skipped(reason)) => {
                skipped += 1;
                items.push(json!({
                    "domain": entry.domain,
                    "accountId": entry.account_id,
                    "source": entry.source,
                    "outcome": "skipped",
                    "reason": reason,
                }));
            }
            Err(RollbackError::Failed(reason)) => {
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
        "cleanupPending": cleanup_pending,
        "skipped": skipped,
        "failed": failed,
        "items": items,
    }))
}

enum RollbackError {
    Skipped(&'static str),
    Failed(String),
}

/// 回滚一条。`Ok(bool)` 里的 bool = 副本还没删掉（需要重试）。
fn rollback_one(app: &App, entry: &Entry) -> Result<bool, RollbackError> {
    let record = load_record(app, entry).ok_or(RollbackError::Skipped("not_migrated"))?;
    match &entry.row {
        Row::Workbuddy(row) => rollback_workbuddy(app, row, &record),
        Row::Qoder { row, lifecycle } => rollback_qoder(app, row, lifecycle, &record),
    }
}

fn load_record(app: &App, entry: &Entry) -> Option<CredentialMigrationRecord> {
    let id = CredentialMigrationRecord::id_of(entry.domain, &entry.account_id, &entry.source);
    app.with_db(|db| db.find_credential_migration(&id))
        .ok()
        .flatten()
}

fn rollback_workbuddy(
    app: &App,
    row: &AuthorizationRecord,
    record: &CredentialMigrationRecord,
) -> Result<bool, RollbackError> {
    // 1. 行必须仍是那次迁移的结果（引用没换、版本没动）。
    if row.access_ref != record.migrated_ref {
        return Err(RollbackError::Skipped("row_moved_since_migration"));
    }
    if row.version != record.migrated_version {
        return Err(RollbackError::Skipped("row_reauthorized_since_migration"));
    }
    // 2. 记录本身必须能指回钥匙串，而且要带着原令牌摘要（复审 P0-A / P1-1）。
    if !record.original_ref.starts_with(reference::KEYCHAIN_PREFIX) {
        return Err(RollbackError::Skipped("record_unusable"));
    }
    let Some(access_digest) = record
        .original_fingerprint
        .as_deref()
        .filter(|d| !d.is_empty())
    else {
        return Err(RollbackError::Skipped("record_without_digest"));
    };

    // 3. 原钥匙串整组必须还在，而且**还是迁移时那一份**（摘要相等）。
    let access_key = record
        .original_ref
        .strip_prefix(reference::KEYCHAIN_PREFIX)
        .ok_or_else(|| RollbackError::Failed("迁移记录里的原引用不是钥匙串键".to_string()))?;
    verify_original(app, access_key, access_digest, "access")?;
    let refresh_key = ids::refresh_key(&row.account_id, &row.kind);
    let has_refresh = row.refresh_ref.is_some() || record.original_refresh_fingerprint.is_some();
    if has_refresh {
        let Some(refresh_digest) = record
            .original_refresh_fingerprint
            .as_deref()
            .filter(|d| !d.is_empty())
        else {
            // 这行需要 refresh，但记录里没有它的摘要：无法证明原组仍是同一份。
            return Err(RollbackError::Skipped("record_without_refresh_digest"));
        };
        verify_original(app, &refresh_key, refresh_digest, "refresh")?;
    }

    // 4. CAS 改回原引用 + 标记副本待清理，**同一个事务**（复审 P0-B / P1-E）。
    let mut next = row.clone();
    next.access_ref = record.original_ref.clone();
    next.refresh_ref = has_refresh.then(|| ids::keychain_ref(&refresh_key));
    let committed = app
        .with_db(|db| db.commit_rollback_workbuddy(&next, row.version, &row.access_ref, &record.id))
        .map_err(|err| RollbackError::Failed(err.message.clone()))?;
    if !committed {
        return Err(RollbackError::Skipped("authorization_changed"));
    }
    if let Err(err) = crate::auth::service::sync_mirror(app, &row.account_id) {
        crate::handlers::log_note(&format!("回滚后同步账号镜像失败：{}", err.message));
    }

    Ok(finish_rollback(app, record))
}

fn rollback_qoder(
    app: &App,
    row: &QoderAuthRecord,
    lifecycle: &QoderLifecycle,
    record: &CredentialMigrationRecord,
) -> Result<bool, RollbackError> {
    if row.secret_ref.as_deref() != Some(record.migrated_ref.as_str()) {
        return Err(RollbackError::Skipped("row_moved_since_migration"));
    }
    if row.auth_version != record.migrated_version {
        return Err(RollbackError::Skipped("row_reauthorized_since_migration"));
    }
    if !record.original_ref.starts_with(reference::KEYCHAIN_PREFIX) {
        return Err(RollbackError::Skipped("record_unusable"));
    }
    let Some(access_digest) = record
        .original_fingerprint
        .as_deref()
        .filter(|d| !d.is_empty())
    else {
        return Err(RollbackError::Skipped("record_without_digest"));
    };

    let access_key = record
        .original_ref
        .strip_prefix(reference::KEYCHAIN_PREFIX)
        .ok_or_else(|| RollbackError::Failed("迁移记录里的原引用不是钥匙串键".to_string()))?;
    verify_original(app, access_key, access_digest, "access")?;
    if let Some(expected) = record
        .original_refresh_fingerprint
        .as_deref()
        .filter(|d| !d.is_empty())
    {
        let key = qoder_secret::keychain_keys(&row.account_id, &row.source)
            .get(1)
            .cloned()
            .ok_or_else(|| RollbackError::Failed("这条来源没有 refresh 键".to_string()))?;
        verify_original(app, &key, expected, "refresh")?;
    }

    let expectation = QoderAuthExpectation::Present {
        secret_ref: row.secret_ref.clone(),
        lifecycle: lifecycle.clone(),
        auth_version: row.auth_version,
    };
    // 改回引用 + 标记待清理，同一个事务（复审 P0-B / P1-E）。
    let committed = qoder_secret::commit_reference_rolled_back(
        app,
        &row.account_id,
        &row.source,
        &record.original_ref,
        &row.auth_status,
        row.token_expires_at.as_deref(),
        row.fingerprint.as_deref(),
        &expectation,
        &record.id,
    )
    .map_err(|err| RollbackError::Failed(err.message.clone()))?;
    if committed.is_none() {
        return Err(RollbackError::Skipped("authorization_changed"));
    }

    Ok(finish_rollback(app, record))
}

/// 回滚收尾：尽力删掉副本，**删成功才删记录**。
///
/// 返回 true = 副本还留着（记录已带 `cleanup_pending`，`verify` 会报出来，
/// 下一次 `run` / `rollback` 会重试）。
///
/// 之前的顺序是"先删记录、再尽力删文件"：文件删失败就只剩一行日志，记录没了，
/// 谁也不知道那个明文令牌文件该不该清（复审 P1-E）。
fn finish_rollback(app: &App, record: &CredentialMigrationRecord) -> bool {
    if let Err(reason) = cleanup_copy(app, record) {
        crate::handlers::log_note(&format!(
            "回滚后副本未清理（{}）：{}，已留待重试",
            record.id, reason
        ));
        return true;
    }
    if let Err(err) = app.with_db(|db| db.delete_credential_migration(&record.id)) {
        crate::handlers::log_note(&format!("回滚后删除迁移记录失败：{}", err.message));
        return true;
    }
    false
}

/// 删掉回滚后不再被引用的副本文件。`Ok(())` = 该删的都删了（或本来就不该删）。
///
/// 删除前要过四道**独立**的核对，任何一道不过就保留文件、让记录留在
/// `cleanup_pending`（第七轮复审 P1-3）：
///
/// 1. 两张表都没有行再引用它；
/// 2. 文件里的 `authorizationId` 就是这条记录该有的那个；
/// 3. `platform` / `source` / `credentialKind` 与这条授权的真实身份一致；
/// 4. 删除走 **revision CAS**（`delete_at_revision`）：读完之后文件被换过，
///    这次删除就不执行 —— 免得把刚写进去的新令牌当成旧副本删掉。
fn cleanup_copy(app: &App, record: &CredentialMigrationRecord) -> Result<(), String> {
    let Some(secret_id) = record
        .migrated_ref
        .strip_prefix(reference::LOCAL_JSON_PREFIX)
    else {
        return Err("副本引用形状异常".to_string());
    };
    // 1. 仍被任何行引用 → 那是别人的凭据，保留文件（记录可以删）。
    let wb = app
        .with_db(|db| db.count_authorizations_referencing(&record.migrated_ref))
        .unwrap_or(1);
    let qoder = app
        .with_db(|db| db.qoder_authorizations_referencing(&record.migrated_ref))
        .unwrap_or(1);
    if wb != 0 || qoder != 0 {
        return Ok(());
    }
    let (platform, kind) = expected_copy_identity(app, record)?;
    let store = LocalJsonStore::for_paths(&app.paths);
    let secret = match store.load(secret_id) {
        Ok(secret) => secret,
        // 文件已经不在了 = 目标达成。
        Err(err) if err.kind == crate::credentials::CredentialErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("读不到副本（{}）", err.kind.as_str())),
    };
    // 2/3. 归属四项逐项核对，不做"大概对"的判断。
    if secret.authorization_id != copy_owner(record) {
        return Err("副本的 authorizationId 与记录不符".to_string());
    }
    if secret.platform != platform {
        return Err("副本的 platform 与账号不符".to_string());
    }
    if secret.source != SecretSource::Own || secret.credential_kind != kind {
        return Err("副本的来源/类型与授权不符".to_string());
    }
    // 4. revision CAS：读完之后被换掉就不删。
    match store.delete_at_revision(secret_id, secret.revision) {
        Ok(_) => Ok(()),
        Err(err) if err.kind == crate::credentials::CredentialErrorKind::Conflict => {
            Err("副本在读与删之间被替换过，本次不删除".to_string())
        }
        Err(err) => Err(format!(
            "删除失败（{}）：{}",
            err.kind.as_str(),
            crate::rpc::sanitize(&err.message)
        )),
    }
}

/// 这份副本**应该**长什么样：平台与凭据类型都从账号现状推导。
///
/// 账号已经不在了就无法核对归属 —— 那就不删（保留 `cleanup_pending`）。
/// 宁可留一个无人引用的明文文件让 `verify` 报出来，也不猜着删。
fn expected_copy_identity(
    app: &App,
    record: &CredentialMigrationRecord,
) -> Result<(String, CredentialKind), String> {
    if record.domain == DOMAIN_QODER {
        let account = app
            .with_db(|db| db.get_qoder_account(&record.account_id))
            .map_err(|err| err.message)?
            .ok_or("账号已不存在，无法核对副本归属")?;
        let edition = QoderEdition::parse(&account.edition).ok_or("账号的 edition 非法")?;
        Ok((
            qoder_secret::platform_of(edition).to_string(),
            qoder_secret::kind_of(&record.source),
        ))
    } else {
        let account = app
            .with_db(|db| db.get_account(&record.account_id))
            .map_err(|err| err.message)?
            .ok_or("账号已不存在，无法核对副本归属")?;
        Ok((account.platform, CredentialKind::Oauth))
    }
}

/// 副本里的 `authorizationId` 应该是谁。由记录本身推导，不依赖调用方多传一个值。
fn copy_owner(record: &CredentialMigrationRecord) -> String {
    if record.domain == DOMAIN_QODER {
        qoder_secret::authorization_id(&record.account_id, &record.source)
    } else {
        ids::auth_id(&record.account_id, &record.source)
    }
}

/// 重试所有"回滚后副本待清理"的记录（P1-E 的重试入口）。
///
/// 幂等：清掉的删记录，清不掉的留着下次再试，绝不会因为一次失败就丢掉线索。
fn drain_pending_cleanup(app: &App) {
    let Ok(records) = app.with_db(|db| db.pending_cleanup_migrations()) else {
        return;
    };
    for record in records {
        match cleanup_copy(app, &record) {
            Ok(()) => {
                if let Err(err) = app.with_db(|db| db.delete_credential_migration(&record.id)) {
                    crate::handlers::log_note(&format!(
                        "清理待删副本后删除迁移记录失败：{}",
                        err.message
                    ));
                }
            }
            Err(reason) => crate::handlers::log_note(&format!(
                "回滚副本仍未清理（{}）：{}，下次再试",
                record.id, reason
            )),
        }
    }
}

/// 原钥匙串条目必须还在，且与迁移时记录的摘要**一致**。
///
/// 摘要缺失一律拒绝（第七轮复审 P1-1）：v22 曾经在二次读取失败时写下 `None`，
/// 那种记录若被允许回滚，"还是不是那一份令牌"这道门就只剩"键存在"。现在
/// `rollback_basis` 也不把这些记录算作可回滚，两层各自独立成立。
fn verify_original(
    app: &App,
    key: &str,
    expected_digest: &str,
    slot: &str,
) -> Result<(), RollbackError> {
    let bytes = app.secure.get(key).map_err(|err| {
        RollbackError::Failed(format!("原 {slot} 已不可用（{}），不能回滚", err.message))
    })?;
    if !digest_matches(expected_digest, &bytes) {
        return Err(RollbackError::Failed(format!(
            "原 {slot} 已经不是迁移时那一份，不能回滚"
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 核验
// ---------------------------------------------------------------------------

/// 逐条核验 + 孤儿文件扫描。
pub fn verify(app: &App) -> RpcResult<Value> {
    let entries = collect(app)?;
    let store = LocalJsonStore::for_paths(&app.paths);
    let migrations = app.with_db(|db| db.list_credential_migrations())?;

    let mut items = Vec::new();
    let mut ok = 0usize;
    let mut broken = 0usize;
    let mut pending = 0usize;
    // 借用授权还指着钥匙串：不是"待迁移"，而是"待重新关联"。
    let mut needs_relink = 0usize;

    for entry in &entries {
        let record = migrations.iter().find(|record| {
            record.domain == entry.domain
                && record.account_id == entry.account_id
                && record.source == entry.source
        });
        match entry.storage {
            STORAGE_LOCALJSON => match verify_one(app, entry, &store, record) {
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
                // 同一条 `keychain:` 引用，三种完全不同的处境。混成一个
                // "pending_keychain" 会让用户以为"还有 7 条要迁移"，其实只有一部分需要动手，
                // 另一部分根本不该迁移（借用的 access 不持久化）。
                match entry.blocked {
                    Some(BLOCKED_NEEDS_RELINK) => {
                        needs_relink += 1;
                        items.push(json!({
                            "domain": entry.domain,
                            "accountId": entry.account_id,
                            "source": entry.source,
                            "state": "needs_relink",
                        }));
                    }
                    Some(BLOCKED_MAGPIE) => {
                        items.push(json!({
                            "domain": entry.domain,
                            "accountId": entry.account_id,
                            "source": entry.source,
                            "state": "magpie_managed",
                        }));
                    }
                    _ => {
                        pending += 1;
                        items.push(json!({
                            "domain": entry.domain,
                            "accountId": entry.account_id,
                            "source": entry.source,
                            "state": "pending_keychain",
                        }));
                    }
                }
            }
            _ => {}
        }
    }

    // 迁移记录只在"仍然描述当前那一行"时才算依据。之后重新授权过，它就是历史：
    // 拿它比对只会把正常的新授权报成损坏（复审 P1-D）。这里单独计数，不当作故障。
    let stale_records = migrations
        .iter()
        .filter(|record| {
            entries
                .iter()
                .find(|entry| {
                    entry.domain == record.domain
                        && entry.account_id == record.account_id
                        && entry.source == record.source
                })
                .is_none_or(|entry| {
                    !rollback_basis(
                        Some(record),
                        entry.reference.as_deref().unwrap_or_default(),
                        entry_version(entry),
                    )
                })
        })
        .count();

    let referenced = referenced_refs(app)?;
    // 待清理的副本已经单独上报了，别再算成孤儿：那是"有主的欠账"，不是异常文件。
    let awaiting_cleanup: Vec<&str> = migrations
        .iter()
        .filter(|record| record.cleanup_pending)
        .map(|record| record.migrated_ref.as_str())
        .collect();
    let mut orphans = Vec::new();
    let report = store
        .scan()
        .map_err(|err| RpcError::new(crate::rpc::E_SECURESTORE, err.message))?;
    for secret in &report.secrets {
        let reference = reference::local_json_ref(&secret.secret_id)
            .map_err(|err| RpcError::internal(err.message))?;
        if awaiting_cleanup.iter().any(|item| item == &reference) {
            continue;
        }
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

    // 回滚过、但副本文件没删掉的记录：留在这里等下一次 run / rollback 重试（P1-E）。
    let cleanup_pending: Vec<Value> = migrations
        .iter()
        .filter(|record| record.cleanup_pending)
        .map(|record| {
            json!({
                "domain": record.domain,
                "accountId": record.account_id,
                "source": record.source,
                "reference": record.migrated_ref,
            })
        })
        .collect();

    Ok(json!({
        "checkedAt": crate::clock::now_rfc3339(),
        "ok": ok,
        "broken": broken,
        "pendingKeychain": pending,
        "needsRelink": needs_relink,
        "rollbackable": entries.iter().filter(|entry| entry.rollbackable).count(),
        "staleRecords": stale_records,
        "pendingCleanup": cleanup_pending,
        "orphans": orphans,
        "brokenFiles": report.broken.len(),
        "items": items,
    }))
}

/// 行版本。WorkBuddy 用 `version`，Qoder 用 `auth_version`。
fn entry_version(entry: &Entry) -> i64 {
    match &entry.row {
        Row::Workbuddy(row) => row.version,
        Row::Qoder { row, .. } => row.auth_version,
    }
}

fn verify_one(
    app: &App,
    entry: &Entry,
    store: &LocalJsonStore,
    record: Option<&CredentialMigrationRecord>,
) -> Result<(), String> {
    let reference_value = entry
        .reference
        .as_deref()
        .ok_or_else(|| "行里没有引用".to_string())?;
    let secret_id = reference_value
        .strip_prefix(reference::LOCAL_JSON_PREFIX)
        .ok_or_else(|| "引用不是本地 JSON".to_string())?;
    let secret = store.load(secret_id).map_err(|err| err.message.clone())?;

    let (expected_auth_id, expected_platform, expected_kind) = match &entry.row {
        Row::Workbuddy(row) => (
            row.id.clone(),
            entry.platform.clone(),
            CredentialKind::Oauth,
        ),
        Row::Qoder { row, .. } => (
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

    // 记录只在"行还是那次迁移的产物"时作数；否则它是历史，不能拿来判定。
    let live =
        record.filter(|record| rollback_basis(Some(record), reference_value, entry_version(entry)));

    // **refresh 槽也要验**：只核 access 会出现"核验全 OK 但刷新失效"。
    //
    // Qoder 的 refresh 只有服务端下发时才有，而且只有**仍然有效**的迁移记录能证明
    // "当时有"。这里必须用 `live` 而不是"任何记录"：迁移后又重新授权成一份没有
    // refresh 的新文件时，旧记录说"有"、新文件说"没有"——那是正常的新授权，
    // 拿旧记录判它会把好凭据报成损坏（第七轮复审 P1-2）。
    let wants_refresh = match &entry.row {
        Row::Workbuddy(row) => row.refresh_ref.is_some(),
        Row::Qoder { .. } => live.is_some_and(|item| item.original_refresh_fingerprint.is_some()),
    };
    if wants_refresh {
        match secret.token(reference::CredentialSlot::Refresh) {
            Ok(token) if !token.trim().is_empty() => {}
            Ok(_) => return Err("凭据文件里的 refresh 是空的".to_string()),
            Err(_) => return Err("凭据文件里缺 refresh 槽".to_string()),
        }
    }

    // 迁移过的行：用记录里的摘要比对（WorkBuddy 行本身没有 fingerprint 列）。
    if let Some(record) = live {
        if let Some(expected) = record.original_fingerprint.as_deref() {
            if !digest_matches(expected, access.as_bytes()) {
                return Err("迁移记录里的摘要与文件里的令牌不一致".to_string());
            }
        }
        // refresh 也要比对内容，不能只看槽位在不在：被换成另一串 token、
        // 或者迁移时漏写了，都只有摘要能发现（复审 P1-D）。
        if let Some(expected) = record.original_refresh_fingerprint.as_deref() {
            let token = secret
                .token(reference::CredentialSlot::Refresh)
                .map_err(|err| err.message.clone())?;
            if !digest_matches(expected, token.as_bytes()) {
                return Err("迁移记录里的 refresh 摘要与文件里的令牌不一致".to_string());
            }
        }
    } else if let Some(expected) = entry.fingerprint.clone() {
        if expected != crate::securestore::fingerprint(access.as_bytes()) {
            return Err("行里的指纹与文件里的令牌不一致".to_string());
        }
    }

    let resolved = match &entry.row {
        Row::Workbuddy(row) => {
            let account = app
                .with_db(|db| db.get_account(&row.account_id))
                .map_err(|err| err.message.clone())?
                .ok_or_else(|| "账号不存在".to_string())?;
            crate::auth::owned::token(app, &account, row, reference::CredentialSlot::Access)
                .map_err(|err| err.message.clone())?
        }
        Row::Qoder { row, .. } => {
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

/// 迁移前的备份：库快照 + 清单。**清单里没有令牌**。
fn backup_before_migration(app: &App, entries: &[Entry]) -> RpcResult<PathBuf> {
    let stamp = crate::clock::now_rfc3339().replace([':', '+'], "-");
    // 秒级时间戳连跑两次会撞名，加随机后缀；目录 0700，且拒绝已存在的路径。
    let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let dir = app
        .paths
        .backups_dir
        .join(format!("cred-migration-{stamp}-{suffix}"));
    // 父目录可能还不存在（测试与全新安装）；`create_dir` 本身拒绝已存在的目标，
    // 这正是我们要的：绝不往一个已有目录里写第二份"备份"。
    std::fs::create_dir_all(&app.paths.backups_dir)?;
    std::fs::create_dir(&dir)?;
    restrict_dir(&dir)?;

    let db_path = dir.join("accounts.sqlite");
    app.with_db(|db| db.backup_to(&db_path))?;
    // 快照必须是一个能打开、且通过完整性检查的库 —— 否则"备份"是假的。
    if !crate::storage::Db::snapshot_is_sound(&db_path)? {
        return Err(RpcError::new(
            crate::rpc::E_SECURESTORE,
            format!("备份快照不完整：{}", dir.display()),
        )
        .with_data(json!({ "kind": "backup_incomplete" })));
    }

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

/// 备份目录必须只有自己能进：`0700` 且不是符号链接。
fn restrict_dir(dir: &Path) -> RpcResult<()> {
    let meta = std::fs::symlink_metadata(dir)?;
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(RpcError::new(
            crate::rpc::E_SECURESTORE,
            format!("备份目录不安全：{}", dir.display()),
        )
        .with_data(json!({ "kind": "backup_dir_unsafe" })));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
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

    fn keychain_row(app: &App, acc: &crate::storage::AccountRecord) -> AuthorizationRecord {
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
            key_migration_state: KEY_STATE_READY.to_string(),
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

    fn row_of(app: &App, acc: &crate::storage::AccountRecord) -> AuthorizationRecord {
        app.with_db(|db| db.find_authorization(&acc.id, ids::KIND_OWN))
            .unwrap()
            .unwrap()
    }

    fn no_secret_files(app: &App) -> bool {
        secret_file_count(app) == 0
    }

    fn secret_file_count(app: &App) -> usize {
        LocalJsonStore::for_paths(&app.paths)
            .scan()
            .map(|report| report.secrets.len())
            .unwrap_or(0)
    }

    #[test]
    fn inventory_lists_keychain_rows_as_migratable_and_never_reveals_tokens() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-inv");
        keychain_row(&app, &acc);

        let value = inventory(&app).unwrap();
        assert_eq!(value["summary"]["migratable"], 1);
        assert_eq!(value["summary"]["rollbackable"], 0);
        let items = value["items"].as_array().unwrap();
        assert_eq!(items[0]["storage"], "keychain");
        assert_eq!(items[0]["migratable"], true);
        let text = value.to_string();
        assert!(!text.contains("WB-TOKEN"), "盘点不许回显令牌");
        assert!(!text.contains("WB-REFRESH"));
    }

    /// CRED-5：借用授权还写着 `keychain:` 引用时，**不是**"由 Magpie 管理"，
    /// 也不是可迁移项 —— 它需要一次显式重新关联。
    ///
    /// 误判的代价很具体：标成 `magpie_managed` 会让用户以为"已经好了"，
    /// 而每次读取其实仍在敲钥匙串；标成 `migratable` 则会把 Magpie 的 access
    /// 复制成本机明文文件，违反 PRD §3。
    #[test]
    fn a_legacy_magpie_row_is_needs_relink_not_migratable_nor_managed() {
        let (app, _dir) = app_with(SecureStore::memory());
        let acc = account(&app, "uid-relink");
        let key = ids::access_key(&acc.id, ids::KIND_MAGPIE);
        app.secure.set(&key, b"BORROWED-TOKEN").unwrap();
        let row = AuthorizationRecord {
            id: ids::auth_id(&acc.id, ids::KIND_MAGPIE),
            account_id: acc.id.clone(),
            kind: ids::KIND_MAGPIE.to_string(),
            access_ref: ids::keychain_ref(&key),
            refresh_ref: None,
            auth_domain: None,
            token_expires_at: None,
            status: "valid".to_string(),
            version: 1,
            key_migration_state: KEY_STATE_READY.to_string(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: "2026-11-01T00:00:00Z".to_string(),
            updated_at: "2026-11-01T00:00:00Z".to_string(),
        };
        app.with_db(|db| db.insert_authorization_if_absent(&row))
            .unwrap();

        let value = inventory(&app).unwrap();
        assert_eq!(value["summary"]["needsRelink"], 1, "{value}");
        assert_eq!(value["summary"]["migratable"], 0, "借用授权不许迁移");
        assert_eq!(value["summary"]["magpie"], 0, "还没关联，不能算已管理");
        assert_eq!(value["summary"]["blocked"], 0, "可修复的项不算引用异常");
        let item = &value["items"][0];
        assert_eq!(item["blocked"], "needs_relink");
        assert_eq!(item["action"], "relink");
        assert_eq!(item["migratable"], false);
        assert!(
            !value.to_string().contains("BORROWED-TOKEN"),
            "盘点不许回显令牌"
        );

        // 迁移即使被调用也不会碰它。
        let migrated = run(&app, &json!({})).unwrap_err();
        assert!(
            migrated.message.contains("还没有启用本地 JSON"),
            "没有明文同意时迁移整体拒绝：{}",
            migrated.message
        );
        let after = app
            .with_db(|db| db.find_authorization(&acc.id, ids::KIND_MAGPIE))
            .unwrap()
            .unwrap();
        assert_eq!(
            after.access_ref,
            ids::keychain_ref(&key),
            "引用一个字节都没动"
        );
    }

    #[test]
    fn migration_moves_the_reference_records_it_and_keeps_the_keychain_entry() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-mig");
        keychain_row(&app, &acc);

        let value = run(&app, &json!({})).unwrap();
        assert_eq!(value["migrated"], 1, "{value}");
        assert!(value["backup"].as_str().is_some());

        let row = row_of(&app, &acc);
        assert!(row.access_ref.starts_with("localjson:v1:"), "{row:?}");
        assert_eq!(row.key_migration_state, KEY_STATE_READY);
        assert_eq!(
            app.secure
                .get(&ids::access_key(&acc.id, ids::KIND_OWN))
                .unwrap(),
            b"WB-TOKEN",
            "原钥匙串条目一个都不许删"
        );
        assert_eq!(
            crate::auth::owned::token(&app, &acc, &row, reference::CredentialSlot::Access).unwrap(),
            "WB-TOKEN"
        );
        let record = app
            .with_db(|db| {
                db.find_credential_migration(&CredentialMigrationRecord::id_of(
                    DOMAIN_WORKBUDDY,
                    &acc.id,
                    ids::KIND_OWN,
                ))
            })
            .unwrap()
            .expect("必须有迁移记录");
        assert_eq!(record.migrated_ref, row.access_ref);
        assert_eq!(record.migrated_version, row.version);

        let report = verify(&app).unwrap();
        assert_eq!(report["ok"], 1, "{report}");
        assert_eq!(report["broken"], 0);
        assert_eq!(report["rollbackable"], 1);
    }

    /// 审计 P0-1：**原本就是 JSON 的授权绝不能被回滚**，即使旧钥匙串条目还在。
    #[test]
    fn rollback_never_touches_a_natively_created_json_authorization() {
        let (app, _dir) = json_app();
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-native", "N"))
                .unwrap();
        let account = app
            .with_db(|db| db.get_qoder_account(&saved.account_id))
            .unwrap()
            .unwrap();
        let before = app
            .with_db(|db| db.get_qoder_authorization(&saved.account_id, "own_browser"))
            .unwrap()
            .unwrap();
        assert!(before
            .secret_ref
            .as_deref()
            .unwrap()
            .starts_with("localjson:"));

        // 旧钥匙串里还留着一份（模拟 CRED-3b 之前登录过）。
        app.secure
            .set(
                &crate::providers::qoder_auth::access_key(&saved.account_id),
                b"OLD",
            )
            .unwrap();

        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["rolledBack"], 0, "原生 JSON 不许回滚：{value}");
        let after = app
            .with_db(|db| db.get_qoder_authorization(&saved.account_id, "own_browser"))
            .unwrap()
            .unwrap();
        assert_eq!(after.secret_ref, before.secret_ref);
        assert_eq!(after.auth_version, before.auth_version);
        assert_eq!(after.auth_status, "valid");
        assert_eq!(
            crate::credits::resolve_token(&app, &account).unwrap().token,
            "N"
        );
    }

    #[test]
    fn rollback_restores_the_keychain_reference_and_removes_the_copy() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-rb");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let migrated = row_of(&app, &acc);
        let secret_id = migrated
            .access_ref
            .strip_prefix("localjson:v1:")
            .unwrap()
            .to_string();

        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["rolledBack"], 1, "{value}");

        let row = row_of(&app, &acc);
        assert!(row.access_ref.starts_with("keychain:"), "{row:?}");
        assert!(row.refresh_ref.as_deref().unwrap().starts_with("keychain:"));
        assert!(LocalJsonStore::for_paths(&app.paths)
            .load(&secret_id)
            .is_err());
        assert!(app
            .with_db(|db| db.list_credential_migrations())
            .unwrap()
            .is_empty());
    }

    /// 审计 P0-1：迁移后又重新登录（版本变了）→ 不许回滚，新令牌保持可用。
    #[test]
    fn rollback_skips_a_row_that_was_reauthorized_after_migration() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-reauth");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let migrated = row_of(&app, &acc);

        let mut next = migrated.clone();
        next.status = "valid".to_string();
        assert!(app
            .with_db(|db| {
                db.replace_authorization_cas(&next, migrated.version, &migrated.access_ref)
            })
            .unwrap());

        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["rolledBack"], 0, "{value}");
        // 版本变了 → 连"可回滚"都不成立，所以不会成为目标。
        assert!(value["items"].as_array().unwrap().is_empty(), "{value}");

        let row = row_of(&app, &acc);
        assert!(row.access_ref.starts_with("localjson:"), "有效授权不许动");
        assert_eq!(
            crate::auth::owned::token(&app, &acc, &row, reference::CredentialSlot::Access).unwrap(),
            "WB-TOKEN"
        );
    }

    /// 审计 P0-3：原 refresh 已经不是迁移时那一份 → 拒绝回滚。
    #[test]
    fn rollback_refuses_when_the_original_refresh_changed() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-rf");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();

        app.secure
            .set(&ids::refresh_key(&acc.id, ids::KIND_OWN), b"NEWER-REFRESH")
            .unwrap();

        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["rolledBack"], 0, "{value}");
        assert_eq!(value["items"][0]["outcome"], "failed");
        let row = row_of(&app, &acc);
        assert!(row.access_ref.starts_with("localjson:"), "不许切回旧令牌");
    }

    #[test]
    fn a_denied_keychain_read_is_a_failure_not_an_empty_migration() {
        let (app, _dir) = app_with(SecureStore::faulty_on_read());
        crate::auth::consent::set_json_storage(&app, true).unwrap();
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
            key_migration_state: KEY_STATE_READY.to_string(),
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
        assert_eq!(row_of(&app, &acc).access_ref, ids::keychain_ref(&key));
        assert!(no_secret_files(&app));
    }

    #[test]
    fn a_row_changed_during_migration_is_skipped_and_leaves_no_file() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-cas");
        let row = keychain_row(&app, &acc);

        let value = run_with(&app, &json!({}), || {
            let mut bumped = row.clone();
            bumped.status = "valid".to_string();
            app.with_db(|db| db.replace_authorization_cas(&bumped, row.version, &row.access_ref))
                .unwrap();
        })
        .unwrap();

        assert_eq!(value["migrated"], 0, "{value}");
        assert_eq!(value["items"][0]["outcome"], "skipped");
        assert_eq!(value["items"][0]["reason"], "authorization_changed");
        assert!(row_of(&app, &acc).access_ref.starts_with("keychain:"));
        assert!(no_secret_files(&app));
    }

    /// 审计 P0-4：类型错的参数不能被当成"不限定范围"。
    #[test]
    fn a_malformed_scope_is_rejected_and_changes_nothing() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-scope");
        keychain_row(&app, &acc);

        for params in [
            json!({ "accountId": "" }),
            json!({ "accountId": 123 }),
            json!({ "domain": 7 }),
            json!({ "domain": "workbuddy", "accountId": { "a": 1 } }),
        ] {
            let err = run(&app, &params).err().expect("错误参数必须报错");
            assert_eq!(err.code, crate::rpc::INVALID_PARAMS, "{params}");
            let err = rollback(&app, &params).err().expect("错误参数必须报错");
            assert_eq!(err.code, crate::rpc::INVALID_PARAMS, "{params}");
        }
        assert!(row_of(&app, &acc).access_ref.starts_with("keychain:"));
        assert!(no_secret_files(&app));
    }

    /// 没有明文授意时后端也拒绝（不能只靠界面确认）。
    #[test]
    fn migration_requires_the_json_storage_consent() {
        let (app, _dir) = app_with(SecureStore::memory());
        let acc = account(&app, "uid-consent");
        keychain_row(&app, &acc);

        let err = run(&app, &json!({})).err().expect("没有授意必须拒绝");
        assert_eq!(err.code, crate::rpc::INVALID_PARAMS);
        assert_eq!(
            err.data
                .as_ref()
                .and_then(|data| data.get("kind"))
                .and_then(Value::as_str),
            Some("json_storage_not_enabled")
        );
        assert!(row_of(&app, &acc).access_ref.starts_with("keychain:"));
    }

    #[test]
    fn the_backup_manifest_never_contains_tokens_and_the_snapshot_is_sound() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-backup");
        keychain_row(&app, &acc);

        let value = run(&app, &json!({})).unwrap();
        let dir = PathBuf::from(value["backup"].as_str().expect("备份路径"));
        assert!(dir.join("accounts.sqlite").exists());
        let manifest = std::fs::read_to_string(dir.join("inventory.json")).unwrap();
        assert!(!manifest.contains("WB-TOKEN"));
        assert!(!manifest.contains("WB-REFRESH"));
        assert!(manifest.contains("keychain:"));
        assert!(crate::storage::Db::snapshot_is_sound(&dir.join("accounts.sqlite")).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "备份目录必须 0700");
        }
    }

    /// 同一秒跑两次：目录名不能撞。
    #[test]
    fn two_runs_in_the_same_second_get_distinct_backups() {
        let (app, _dir) = json_app();
        let a = account(&app, "uid-1");
        let b = account(&app, "uid-2");
        keychain_row(&app, &a);
        keychain_row(&app, &b);

        let first = run(&app, &json!({ "accountId": a.id })).unwrap();
        let second = run(&app, &json!({ "accountId": b.id })).unwrap();
        assert_ne!(first["backup"], second["backup"]);
    }

    /// 审计 P0-3：refresh 引用指向别的账号的键 → 拒绝迁移（不能把别人的 refresh 抄过来）。
    #[test]
    fn a_refresh_reference_to_another_key_blocks_the_migration() {
        let (app, _dir) = json_app();
        let a = account(&app, "uid-own");
        let row = keychain_row(&app, &a);

        let mut bad = row.clone();
        bad.refresh_ref = Some(ids::keychain_ref("workbuddy/someone-else/own/refresh"));
        assert!(app
            .with_db(|db| db.replace_authorization_cas(&bad, row.version, &row.access_ref))
            .unwrap());

        let value = run(&app, &json!({})).unwrap();
        assert_eq!(value["migrated"], 0, "{value}");
        assert_eq!(value["failed"], 1, "{value}");
        assert!(
            value["items"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("refresh 引用指向别的键"),
            "{value}"
        );
        assert!(no_secret_files(&app), "拒绝时不许留下副本");
    }

    /// 审计 P0-3：Qoder 的 refresh 被系统拒绝读取时，不能当成"没有 refresh"。
    #[test]
    fn a_denied_qoder_refresh_read_fails_the_migration() {
        let (app, _dir) = app_with(SecureStore::faulty_on_read_keys(&["own/refresh"]));
        crate::auth::consent::set_json_storage(&app, true).unwrap();
        let account = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-qr", Some("q"), None))
            .unwrap();
        let access_key = crate::providers::qoder_auth::access_key(&account.id);
        let refresh_key = crate::providers::qoder_auth::refresh_key(&account.id);
        app.secure.set(&access_key, b"Q-TOKEN").unwrap();
        app.secure.set(&refresh_key, b"Q-REFRESH").unwrap();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                "own_browser",
                None,
                Some(&ids::keychain_ref(&access_key)),
                "valid",
                None,
                None,
            )
        })
        .unwrap();
        app.secure.arm();

        let value = run(&app, &json!({ "domain": "qoder" })).unwrap();
        assert_eq!(value["migrated"], 0, "{value}");
        assert_eq!(value["failed"], 1, "{value}");
        let row = app
            .with_db(|db| db.get_qoder_authorization(&account.id, "own_browser"))
            .unwrap()
            .unwrap();
        assert!(
            row.secret_ref.as_deref().unwrap().starts_with("keychain:"),
            "失败时引用不许动：{row:?}"
        );
        assert!(no_secret_files(&app));
    }

    /// 审计 P1-1：任何提前返回都必须清掉刚建的副本。
    #[test]
    fn a_pending_copy_is_removed_unless_it_is_kept() {
        let (app, _dir) = json_app();
        let store = LocalJsonStore::for_paths(&app.paths);
        let secret = store
            .create(SecretDraft::oauth("auth-pending", "workbuddy", "T"))
            .unwrap();
        let reference = reference::local_json_ref(&secret.secret_id).unwrap();

        {
            let _pending = PendingCopy::new(&app, secret.secret_id.clone(), reference.clone());
            // 没有 keep → 作用域结束就删。
        }
        assert!(
            store.load(&secret.secret_id).is_err(),
            "未 keep 的副本必须清掉"
        );

        let kept = store
            .create(SecretDraft::oauth("auth-kept", "workbuddy", "T"))
            .unwrap();
        let kept_ref = reference::local_json_ref(&kept.secret_id).unwrap();
        {
            let pending = PendingCopy::new(&app, kept.secret_id.clone(), kept_ref);
            pending.keep();
        }
        assert!(store.load(&kept.secret_id).is_ok(), "keep 过的副本必须保留");
    }

    /// 审计 P1-4：核验必须发现"access 在、refresh 缺"的文件。
    #[test]
    fn verify_flags_a_file_that_lost_its_refresh_slot() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-norf");
        let store = LocalJsonStore::for_paths(&app.paths);
        // 行说有两个槽位，文件里只有 access。
        let secret = store
            .create(SecretDraft::oauth(
                ids::auth_id(&acc.id, ids::KIND_OWN),
                "workbuddy",
                "ONLY-ACCESS",
            ))
            .unwrap();
        let reference = reference::local_json_ref(&secret.secret_id).unwrap();
        let row = AuthorizationRecord {
            id: ids::auth_id(&acc.id, ids::KIND_OWN),
            account_id: acc.id.clone(),
            kind: ids::KIND_OWN.to_string(),
            access_ref: reference.clone(),
            refresh_ref: Some(reference),
            auth_domain: None,
            token_expires_at: None,
            status: "valid".to_string(),
            version: 1,
            key_migration_state: KEY_STATE_READY.to_string(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: "2026-11-01T00:00:00Z".to_string(),
            updated_at: "2026-11-01T00:00:00Z".to_string(),
        };
        app.with_db(|db| db.insert_authorization_if_absent(&row))
            .unwrap();

        let report = verify(&app).unwrap();
        assert_eq!(report["broken"], 1, "{report}");
        assert!(
            report["items"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("refresh"),
            "{report}"
        );
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
        assert!(store.load(&orphan.secret_id).is_ok(), "只报不删");
        let text = report.to_string();
        assert!(!text.contains("ORPHAN"));
    }

    #[test]
    fn migration_is_scoped_to_one_account_when_asked() {
        let (app, _dir) = json_app();
        let a = account(&app, "uid-a");
        let b = account(&app, "uid-b");
        keychain_row(&app, &a);
        keychain_row(&app, &b);

        let value = run(&app, &json!({ "accountId": a.id })).unwrap();
        assert_eq!(value["migrated"], 1);
        assert!(row_of(&app, &b).access_ref.starts_with("keychain:"));
    }

    #[test]
    fn qoder_keychain_authorization_migrates_and_stays_readable() {
        // 先不开启明文同意 → persist 写钥匙串（CRED-3b 的老路径）。
        let (app, _dir) = app_with(SecureStore::memory());
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-q", "Q-TOKEN"))
                .unwrap();
        // 现在用户同意明文，再迁移。
        crate::auth::consent::set_json_storage(&app, true).unwrap();
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
        assert!(row
            .secret_ref
            .as_deref()
            .unwrap()
            .starts_with("localjson:v1:"));
        let account = app
            .with_db(|db| db.get_qoder_account(&saved.account_id))
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::credits::resolve_token(&app, &account).unwrap().token,
            "Q-TOKEN"
        );
        assert_eq!(
            app.secure
                .get(&crate::providers::qoder_auth::access_key(&saved.account_id))
                .unwrap(),
            b"Q-TOKEN"
        );

        let value = rollback(&app, &json!({ "domain": "qoder" })).unwrap();
        assert_eq!(value["rolledBack"], 1, "{value}");
        let row = app
            .with_db(|db| db.get_qoder_authorization(&saved.account_id, "own_browser"))
            .unwrap()
            .unwrap();
        assert!(row.secret_ref.as_deref().unwrap().starts_with("keychain:"));
    }

    /// 审计 P0-2：迁移用**计划快照**做 CAS。盘点之后别人重新登录过，
    /// 本次迁移必须落空，且不能把新授权盖回旧令牌。
    #[test]
    fn a_qoder_relogin_during_migration_wins_and_the_plan_is_dropped() {
        let (app, _dir) = app_with(SecureStore::memory());
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-race", "OLD"))
                .unwrap();
        crate::auth::consent::set_json_storage(&app, true).unwrap();

        let value = run_with(&app, &json!({ "domain": "qoder" }), || {
            // 盘点之后、迁移之前：同来源重新登录，引用与版本都变了。
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-race", "NEW"))
                .expect("重新登录必须成功");
        })
        .unwrap();

        assert_eq!(value["migrated"], 0, "{value}");
        assert_eq!(value["items"][0]["outcome"], "skipped");

        let row = app
            .with_db(|db| db.get_qoder_authorization(&saved.account_id, "own_browser"))
            .unwrap()
            .unwrap();
        let account = app
            .with_db(|db| db.get_qoder_account(&saved.account_id))
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::credits::resolve_token(&app, &account).unwrap().token,
            "NEW",
            "赢家的令牌不许被盖回旧值"
        );
        assert_eq!(row.auth_status, "valid");
    }

    /// 复审 P0-B：记录写不进去，引用就必须原封不动。
    ///
    /// 注入的失败发生在"换引用之后、提交之前"：只要两条语句不在同一个事务里，
    /// 这里就会看到"行指向新文件、却没有回滚依据"。
    #[test]
    fn a_failed_migration_record_write_rolls_back_the_reference_switch() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-atomic");
        keychain_row(&app, &acc);

        arm_record_write_fault(1);
        let value = run(&app, &json!({})).unwrap();
        assert_eq!(value["migrated"], 0, "{value}");
        assert_eq!(value["failed"], 1, "{value}");
        assert_eq!(
            row_of(&app, &acc).access_ref,
            ids::keychain_ref(&ids::access_key(&acc.id, ids::KIND_OWN)),
            "记录没写成，引用不许动"
        );
        assert!(no_secret_files(&app), "事务回滚后副本必须清掉");
        assert!(app
            .with_db(|db| db.list_credential_migrations())
            .unwrap()
            .is_empty());
    }

    /// 复审 P0-B（Qoder 侧）：同一个不变式。
    #[test]
    fn a_failed_qoder_record_write_keeps_the_keychain_reference() {
        let (app, _dir) = app_with(SecureStore::memory());
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-atomic", "Q"))
                .unwrap();
        crate::auth::consent::set_json_storage(&app, true).unwrap();

        arm_record_write_fault(1);
        let value = run(&app, &json!({ "domain": "qoder" })).unwrap();
        assert_eq!(value["migrated"], 0, "{value}");
        assert_eq!(value["failed"], 1, "{value}");

        let row = app
            .with_db(|db| db.get_qoder_authorization(&saved.account_id, "own_browser"))
            .unwrap()
            .unwrap();
        assert!(
            row.secret_ref.as_deref().unwrap().starts_with("keychain:"),
            "记录没写成，引用不许动：{row:?}"
        );
        assert!(no_secret_files(&app));
        assert!(app
            .with_db(|db| db.list_credential_migrations())
            .unwrap()
            .is_empty());
    }

    /// 复审 P0-A：老 Qoder 行的 `secret_ref` 是 NULL，记录里的原引用必须补成
    /// 真实的钥匙串键 —— 空串会让"可回滚"变成一句谎话。
    #[test]
    fn a_qoder_row_without_a_secret_ref_migrates_with_a_real_original_reference() {
        let (app, _dir) = app_with(SecureStore::memory());
        crate::auth::consent::set_json_storage(&app, true).unwrap();
        let acc = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-null", Some("老行"), None))
            .unwrap();
        let access_key = crate::providers::qoder_auth::access_key(&acc.id);
        app.secure.set(&access_key, b"Q-OLD").unwrap();
        // v16 之前的老行：有授权、没有 secret_ref（键名由账号派生）。
        app.with_db(|db| {
            db.upsert_qoder_authorization(&acc.id, "own_browser", None, None, "valid", None, None)
        })
        .unwrap();

        let value = run(&app, &json!({ "domain": "qoder" })).unwrap();
        assert_eq!(value["migrated"], 1, "{value}");

        let record = app
            .with_db(|db| {
                db.find_credential_migration(&CredentialMigrationRecord::id_of(
                    DOMAIN_QODER,
                    &acc.id,
                    "own_browser",
                ))
            })
            .unwrap()
            .expect("必须有迁移记录");
        assert_eq!(
            record.original_ref,
            qoder_secret::keychain_reference(&acc.id, "own_browser"),
            "原引用必须补成真实钥匙串键，不能是空串"
        );
        assert!(record.original_fingerprint.as_deref().unwrap().len() == 64);

        // "显示可回滚"与"回滚真的能跑"必须一致。
        assert_eq!(inventory(&app).unwrap()["summary"]["rollbackable"], 1);
        let value = rollback(&app, &json!({ "domain": "qoder" })).unwrap();
        assert_eq!(value["rolledBack"], 1, "{value}");
        let row = app
            .with_db(|db| db.get_qoder_authorization(&acc.id, "own_browser"))
            .unwrap()
            .unwrap();
        assert_eq!(
            row.secret_ref.as_deref(),
            Some(qoder_secret::keychain_reference(&acc.id, "own_browser").as_str())
        );
    }

    /// 复审 P0-A：早期构建可能已经写下了空原引用的记录 —— 那种记录既不能显示
    /// 成"可回滚"，也不能被回滚流程当成目标。
    #[test]
    fn a_stored_record_without_a_keychain_original_is_never_advertised_as_rollbackable() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-badrecord");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let row = row_of(&app, &acc);

        // 直接改库，模拟旧版本写下的坏记录（现在的写入路径会拒绝它）。
        app.with_db(|db| {
            db.exec_raw(&format!(
                "UPDATE credential_migrations SET original_ref = '' WHERE id = '{}';",
                CredentialMigrationRecord::id_of(DOMAIN_WORKBUDDY, &acc.id, ids::KIND_OWN)
            ))
        })
        .unwrap();

        assert_eq!(
            inventory(&app).unwrap()["summary"]["rollbackable"],
            0,
            "原引用为空的记录不算回滚依据"
        );
        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["rolledBack"], 0, "{value}");
        assert_eq!(
            row_of(&app, &acc).access_ref,
            row.access_ref,
            "一个字节都不许动"
        );
    }

    /// 复审 P1-D：记录只在"仍然描述当前行"时才算依据。之后重新授权过，
    /// 拿旧记录比对只会把正常的新授权报成损坏。
    #[test]
    fn verify_does_not_report_a_reauthorized_row_as_broken() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-stale");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let migrated = row_of(&app, &acc);

        // 模拟迁移后重新登录：新文件、新令牌、版本 +1。
        let store = LocalJsonStore::for_paths(&app.paths);
        let fresh = store
            .create(SecretDraft::oauth(
                ids::auth_id(&acc.id, ids::KIND_OWN),
                "workbuddy",
                "BRAND-NEW",
            ))
            .unwrap();
        let fresh_ref = reference::local_json_ref(&fresh.secret_id).unwrap();
        let mut next = migrated.clone();
        next.access_ref = fresh_ref.clone();
        next.refresh_ref = None;
        assert!(app
            .with_db(|db| {
                db.replace_authorization_cas(&next, migrated.version, &migrated.access_ref)
            })
            .unwrap());

        let report = verify(&app).unwrap();
        assert_eq!(report["broken"], 0, "新授权不许被旧记录报成损坏：{report}");
        assert_eq!(report["ok"], 1, "{report}");
        assert_eq!(report["staleRecords"], 1, "{report}");
        assert_eq!(report["rollbackable"], 0);
        // 老副本现在没人引用：报成孤儿（只报不删），不报损坏。
        assert_eq!(report["orphans"].as_array().unwrap().len(), 1, "{report}");
    }

    /// 复审 P1-D：refresh 只验"槽位在不在"会漏掉被换过的内容。
    #[test]
    fn verify_flags_a_migrated_file_whose_refresh_was_swapped() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-swapped");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let row = row_of(&app, &acc);
        let secret_id = row
            .access_ref
            .strip_prefix("localjson:v1:")
            .unwrap()
            .to_string();
        let store = LocalJsonStore::for_paths(&app.paths);
        let path = store.root().join(format!("{secret_id}.json"));

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("WB-REFRESH"), "{text}");
        std::fs::write(&path, text.replace("WB-REFRESH", "SWAPPED-REFRESH")).unwrap();

        let report = verify(&app).unwrap();
        assert_eq!(report["broken"], 1, "{report}");
        assert!(
            report["items"][0]["reason"]
                .as_str()
                .unwrap()
                .contains("refresh"),
            "{report}"
        );
    }

    /// 复审 P1-E：副本删不掉时，回滚本身仍然成功，但线索必须留下来（可定位、
    /// 可重试），而不是只剩一行日志。
    ///
    /// 用 `chflags uchg` 让 unlink 必定 EPERM：目录权限会被存储层自己"治好"
    /// （`ensure_root` 每次都把 `0700` 写回去），只有文件级的不可变标记挡得住。
    #[cfg(target_os = "macos")]
    #[test]
    fn a_copy_that_cannot_be_deleted_stays_visible_and_is_retried() {
        use std::process::Command;

        let chflags = |flag: &str, path: &std::path::Path| {
            let status = Command::new("chflags")
                .arg(flag)
                .arg(path)
                .status()
                .expect("chflags");
            assert!(status.success(), "chflags {flag} {}", path.display());
        };

        let (app, _dir) = json_app();
        let acc = account(&app, "uid-cleanup");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let migrated = row_of(&app, &acc);
        let secret_id = migrated
            .access_ref
            .strip_prefix("localjson:v1:")
            .unwrap()
            .to_string();
        let store = LocalJsonStore::for_paths(&app.paths);
        let path = store.root().join(format!("{secret_id}.json"));

        chflags("uchg", &path);
        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["rolledBack"], 1, "{value}");
        assert_eq!(value["cleanupPending"], 1, "{value}");
        assert!(
            row_of(&app, &acc).access_ref.starts_with("keychain:"),
            "回滚本身要成功"
        );
        assert!(path.exists(), "删不掉就留着，不能假装删了");
        let report = verify(&app).unwrap();
        assert_eq!(
            report["pendingCleanup"].as_array().unwrap().len(),
            1,
            "{report}"
        );
        assert_eq!(
            report["orphans"].as_array().unwrap().len(),
            0,
            "待清理的副本不该重复算成孤儿：{report}"
        );
        assert_eq!(
            app.with_db(|db| db.pending_cleanup_migrations())
                .unwrap()
                .len(),
            1,
            "记录要留着，才有重试的依据"
        );

        // 恢复可删之后，下一次回滚调用会先把欠账收掉。
        chflags("nouchg", &path);
        rollback(&app, &json!({})).unwrap();
        assert!(!path.exists(), "重试必须把副本删掉");
        assert!(app
            .with_db(|db| db.list_credential_migrations())
            .unwrap()
            .is_empty());
        assert_eq!(
            verify(&app).unwrap()["pendingCleanup"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    /// 复审 P0-C：摘要来自**写进副本的那一份字节**，而且是完整 SHA-256。
    #[test]
    fn migration_digests_are_taken_from_the_copied_bytes_in_full_length() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-digest");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();

        let record = app
            .with_db(|db| {
                db.find_credential_migration(&CredentialMigrationRecord::id_of(
                    DOMAIN_WORKBUDDY,
                    &acc.id,
                    ids::KIND_OWN,
                ))
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            record.original_fingerprint.as_deref(),
            Some(crate::securestore::digest(b"WB-TOKEN").as_str())
        );
        assert_eq!(
            record.original_refresh_fingerprint.as_deref(),
            Some(crate::securestore::digest(b"WB-REFRESH").as_str())
        );
    }

    /// 复审 P0-1（第七轮）：还没还掉的清理债务不许被下一次迁移覆盖。
    ///
    /// 交错是真的：回滚把引用切回钥匙串，但副本 F1 因为 `uchg` 删不掉 →
    /// 用户再点一次「迁移全部」→ 如果记录被 UPSERT 成本次的 F2，
    /// F1 就永远留在盘上、再没有任何地方记得它。
    #[cfg(target_os = "macos")]
    #[test]
    fn a_pending_cleanup_is_never_overwritten_by_the_next_migration() {
        use std::process::Command;

        let chflags = |flag: &str, path: &std::path::Path| {
            assert!(
                Command::new("chflags")
                    .arg(flag)
                    .arg(path)
                    .status()
                    .expect("chflags")
                    .success(),
                "chflags {flag} {}",
                path.display()
            );
        };

        let (app, _dir) = json_app();
        let acc = account(&app, "uid-debt");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let first = row_of(&app, &acc);
        let first_id = first
            .access_ref
            .strip_prefix("localjson:v1:")
            .unwrap()
            .to_string();
        let store = LocalJsonStore::for_paths(&app.paths);
        let first_path = store.root().join(format!("{first_id}.json"));
        let record_id = CredentialMigrationRecord::id_of(DOMAIN_WORKBUDDY, &acc.id, ids::KIND_OWN);

        // 回滚，但让副本删不掉 → 留下一笔 cleanup_pending。
        chflags("uchg", &first_path);
        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["cleanupPending"], 1, "{value}");
        let debt = app
            .with_db(|db| db.find_credential_migration(&record_id))
            .unwrap()
            .expect("债务记录必须还在");
        assert!(debt.cleanup_pending);
        assert_eq!(debt.migrated_ref, first.access_ref);

        // 再点一次迁移：必须挡住，且**不动**那笔债务。
        let value = run(&app, &json!({})).unwrap();
        assert_eq!(value["migrated"], 0, "{value}");
        assert_eq!(value["skipped"], 1, "{value}");
        assert_eq!(value["items"][0]["reason"], "cleanup_pending", "{value}");
        let after = app
            .with_db(|db| db.find_credential_migration(&record_id))
            .unwrap()
            .expect("债务记录不许被抹掉");
        assert!(after.cleanup_pending, "标志被重置了：{after:?}");
        assert_eq!(after.migrated_ref, first.access_ref, "被改写成了新副本");
        // 没有新副本产生（盘上只有那一份删不掉的 F1）。
        assert_eq!(secret_file_count(&app), 1, "不许偷偷再写一份明文");

        // 解除限制后，下一次回滚把债务清掉，随后迁移恢复正常。
        chflags("nouchg", &first_path);
        rollback(&app, &json!({})).unwrap();
        assert!(app
            .with_db(|db| db.find_credential_migration(&record_id))
            .unwrap()
            .is_none());
        assert!(!first_path.exists());
        let value = run(&app, &json!({})).unwrap();
        assert_eq!(value["migrated"], 1, "{value}");
    }

    /// 复审 P1-1：原令牌摘要为空的旧记录（v22 写入过 None）不得绕过安全门。
    ///
    /// 两道门各自独立：`rollback_basis` 不再把它算作可回滚，
    /// `verify_original` 也不接受缺失的摘要。
    #[test]
    fn a_record_without_an_original_digest_cannot_be_rolled_back() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-nodigest");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let migrated = row_of(&app, &acc);
        let record_id = CredentialMigrationRecord::id_of(DOMAIN_WORKBUDDY, &acc.id, ids::KIND_OWN);

        // 直接改库，模拟旧版本写下的"没有摘要"的记录。
        app.with_db(|db| {
            db.exec_raw(&format!(
                "UPDATE credential_migrations SET original_fingerprint = NULL WHERE id = '{record_id}';"
            ))
        })
        .unwrap();

        assert_eq!(
            inventory(&app).unwrap()["summary"]["rollbackable"],
            0,
            "没有摘要的记录不能显示成可回滚"
        );
        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["rolledBack"], 0, "{value}");
        assert_eq!(
            value["items"].as_array().unwrap().len(),
            0,
            "不是回滚目标，连尝试都不该有：{value}"
        );
        assert_eq!(
            row_of(&app, &acc).access_ref,
            migrated.access_ref,
            "一个字节都不许动"
        );
    }

    /// 复审 P1-1（兼容策略）：早期构建写下的 8 位短摘要仍允许回滚。
    ///
    /// 短摘要只有 32 bit，不适合当严格依据，但删掉兼容会让已经迁移过的库
    /// 永远回不去。策略是"读旧写新"：旧记录照旧能用，新记录一律完整 SHA-256。
    #[test]
    fn a_legacy_short_digest_still_rolls_back() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-shortdigest");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let record_id = CredentialMigrationRecord::id_of(DOMAIN_WORKBUDDY, &acc.id, ids::KIND_OWN);

        app.with_db(|db| {
            db.exec_raw(&format!(
                "UPDATE credential_migrations SET original_fingerprint = '{}' WHERE id = '{record_id}';",
                crate::securestore::fingerprint(b"WB-TOKEN")
            ))
        })
        .unwrap();

        assert_eq!(inventory(&app).unwrap()["summary"]["rollbackable"], 1);
        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["rolledBack"], 1, "{value}");
        assert!(row_of(&app, &acc).access_ref.starts_with("keychain:"));
    }

    /// 复审 P1-2：迁移后重新授权成"没有 refresh"的新授权，不能被旧记录误报损坏。
    #[test]
    fn verify_accepts_a_reauthorized_qoder_row_that_dropped_its_refresh() {
        let (app, _dir) = app_with(SecureStore::memory());
        let saved =
            crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-drop", "Q1"))
                .unwrap();
        crate::auth::consent::set_json_storage(&app, true).unwrap();
        let value = run(&app, &json!({ "domain": "qoder" })).unwrap();
        assert_eq!(value["migrated"], 1, "{value}");
        // 迁移记录里写着"原组有 refresh"（persist 用的是带 refresh 的 token）。
        let record_id =
            CredentialMigrationRecord::id_of(DOMAIN_QODER, &saved.account_id, "own_browser");
        assert!(app
            .with_db(|db| db.find_credential_migration(&record_id))
            .unwrap()
            .unwrap()
            .original_refresh_fingerprint
            .is_some());

        // 重新授权，这次服务端没给 refresh。
        let mut fresh = token("u-drop", "Q2");
        fresh.refresh_token = None;
        crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &fresh).unwrap();

        let report = verify(&app).unwrap();
        assert_eq!(report["broken"], 0, "新授权不许被旧记录报成损坏：{report}");
        assert_eq!(report["ok"], 1, "{report}");
        assert_eq!(report["staleRecords"], 1, "{report}");
    }

    /// 复审 P1-3：归属对不上就保留文件 + 保留债务，不静默删。
    #[cfg(unix)]
    #[test]
    fn cleanup_keeps_a_copy_whose_identity_does_not_match() {
        let (app, _dir) = json_app();
        let acc = account(&app, "uid-identity");
        keychain_row(&app, &acc);
        run(&app, &json!({})).unwrap();
        let migrated = row_of(&app, &acc);
        let secret_id = migrated
            .access_ref
            .strip_prefix("localjson:v1:")
            .unwrap()
            .to_string();
        let path = LocalJsonStore::for_paths(&app.paths)
            .root()
            .join(format!("{secret_id}.json"));
        let original = std::fs::read_to_string(&path).unwrap();

        // 把 platform 改掉：文件本身还能读，但身份与账号不符。
        std::fs::write(&path, original.replace("\"workbuddy\"", "\"qoder\"")).unwrap();
        let value = rollback(&app, &json!({})).unwrap();
        assert_eq!(value["rolledBack"], 1, "{value}");
        assert_eq!(value["cleanupPending"], 1, "{value}");
        assert!(path.exists(), "归属不符的文件不许删");
        let report = verify(&app).unwrap();
        assert_eq!(
            report["pendingCleanup"].as_array().unwrap().len(),
            1,
            "{report}"
        );

        // 恢复成正确身份（revision 不变）后，下一次回滚能把债清掉。
        std::fs::write(&path, &original).unwrap();
        rollback(&app, &json!({})).unwrap();
        assert!(!path.exists(), "身份对上之后应当删掉");
    }
}
