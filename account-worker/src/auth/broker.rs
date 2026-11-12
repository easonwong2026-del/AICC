//! 只读凭据 Broker（M1）。
//!
//! 规则（设计稿 §6）：
//! * 首选来源是配置项 `auth_preferred_source`，默认 **magpie**；
//! * 首选没有可用授权时**读时**兜底到另一份，不改库、不改状态；
//! * 取用一份授权**绝不写另一份** —— 这里只有 `SecureStore::get`，没有任何写；
//! * 能回答"这次用的是哪个来源"，供界面与日志使用；
//! * M1 阶段不接生产写入路径：签到 / 旅行仍走原有 `AppTokens`。
//!
//! 密钥只在本模块内以 `String` 形式短暂存在，`Resolved` 刻意不实现 `Serialize` / `Debug`，
//! 避免哪天真被顺手打进 RPC 或日志。

use serde_json::{json, Value};

use crate::app::App;
use crate::rpc::{RpcError, RpcResult};
use crate::securestore::SecretErrorKind;
use crate::storage::AccountRecord;

use super::ids;

/// 设置项：首选授权来源。
pub const PREFERRED_SETTING: &str = "auth_preferred_source";
/// 默认首选 Magpie：它自己会保活与刷新，AICC 只使用那份无状态 access，冲突面最小。
pub const PREFERRED_DEFAULT: &str = ids::KIND_MAGPIE;

/// 令牌离过期不到这么久就先算"不可用"，避免拿着一个马上失效的令牌去打接口。
const EXPIRY_SKEW_SECONDS: i64 = 300;

pub struct Resolved {
    pub authorization_id: String,
    pub kind: String,
    pub access: String,
    /// 首选不可用、这次用了兜底时说明原因；正常取用为 None。
    pub degraded: Option<String>,
}

pub enum Outcome {
    Resolved(Resolved),
    /// 没有可用授权。`reason` 是稳定分类，`detail` 是给人看的一句话（已脱敏）。
    Unavailable {
        reason: String,
        detail: String,
    },
}

/// 读首选来源。值非法或读库失败时回默认值 —— 设置坏了不该让整个授权链断掉。
pub fn preferred_source(app: &App) -> String {
    match app.with_db(|db| db.get_setting(PREFERRED_SETTING)) {
        Ok(Some(value)) if ids::is_known_kind(&value) => value,
        _ => PREFERRED_DEFAULT.to_string(),
    }
}

/// 校验并写首选来源。只接受 `magpie` / `own`。
pub fn set_preferred_source(app: &App, value: &str) -> RpcResult<()> {
    if !ids::is_known_kind(value) {
        return Err(RpcError::invalid_params(format!(
            "auth_preferred_source 只支持 magpie / own，收到 {value}"
        )));
    }
    app.with_db(|db| db.set_setting(PREFERRED_SETTING, value))
}

/// 读设置项当前生效值。目前只有首选来源一项。
pub fn read_setting(app: &App, key: &str) -> RpcResult<Option<String>> {
    match key {
        PREFERRED_SETTING => Ok(Some(preferred_source(app))),
        other => Err(RpcError::invalid_params(format!("未知设置项：{other}"))),
    }
}

/// 取一份可用授权。**只读**：不写库、不改状态、不写 last_used_at。
pub fn resolve(app: &App, account: &AccountRecord) -> RpcResult<Outcome> {
    let rows = app.with_db(|db| db.list_authorizations(&account.id))?;
    if rows.is_empty() {
        return Ok(Outcome::Unavailable {
            reason: "no_authorization".to_string(),
            detail: "该账号还没有任何授权记录".to_string(),
        });
    }

    let preferred = preferred_source(app);
    let ordered = order_for(&rows, &preferred);

    let mut blocked: Option<(String, String)> = None;
    for (index, row) in ordered.iter().enumerate() {
        if !is_usable(row) {
            continue;
        }
        match read_access(app, account, row) {
            Ok(access) => {
                return Ok(Outcome::Resolved(Resolved {
                    authorization_id: row.id.clone(),
                    kind: row.kind.clone(),
                    access,
                    degraded: (index > 0)
                        .then(|| format!("首选 {preferred} 不可用，已兜底到 {}", row.kind)),
                }));
            }
            Err(err) if err.kind == SecretErrorKind::NotFound => {
                blocked.get_or_insert((
                    "missing_credential".to_string(),
                    format!("{} 的凭据不在钥匙串里", row.kind),
                ));
            }
            Err(err) => {
                blocked.get_or_insert((
                    "keychain_denied".to_string(),
                    crate::rpc::sanitize(&err.message),
                ));
            }
        }
    }

    let (reason, detail) = blocked.unwrap_or_else(|| {
        (
            "no_usable_authorization".to_string(),
            "两份授权都不可用（过期或已失效），请重新扫码或去 Magpie 登录".to_string(),
        )
    });
    Ok(Outcome::Unavailable { reason, detail })
}

/// 首选来源排前面，其余按 kind 固定顺序，保证同一状态下的结果可复现。
fn order_for<'a>(
    rows: &'a [crate::storage::AuthorizationRecord],
    preferred: &str,
) -> Vec<&'a crate::storage::AuthorizationRecord> {
    let mut out: Vec<&crate::storage::AuthorizationRecord> = rows.iter().collect();
    out.sort_by_key(|row| u8::from(row.kind != preferred));
    out
}

/// 可用 = 状态不是失效类，且没有明确过期（含 5 分钟余量）。
///
/// `token_expires_at` 为空时按"可用"处理：没有到期信息不等于已过期，
/// 真的失效会由探测接口（M2）判出来。
pub(crate) fn is_usable(row: &crate::storage::AuthorizationRecord) -> bool {
    if ["invalid", "expired", "denied"].contains(&row.status.as_str()) {
        return false;
    }
    match row.token_expires_at.as_deref() {
        None => true,
        Some(raw) => match chrono::DateTime::parse_from_rfc3339(raw) {
            Ok(at) => {
                let now = chrono::Utc::now();
                at.timestamp() > now.timestamp() + EXPIRY_SKEW_SECONDS
            }
            Err(_) => true, // 时间格式不认识，不敢据此判定失效
        },
    }
}

/// 读 access。`key_migration_state = ready` 之前读旧键（G4）。
///
/// 迁移中断时会出现"库里写着新键、钥匙串里还没有"的组合，所以两侧都试：
/// 先按迁移状态选首选位置，读不到再退到另一个位置。读到 `Denied` 直接上抛 ——
/// 那不是"没有"，而是"现在读不了"，不能当成空。
fn read_access(
    app: &App,
    account: &AccountRecord,
    row: &crate::storage::AuthorizationRecord,
) -> Result<String, crate::securestore::SecretError> {
    let (legacy_access, _) = ids::legacy_keys(&account.uid);
    let new_key = row.access_key();

    let mut order: Vec<&str> = Vec::new();
    if row.credentials_migrated() {
        if let Some(key) = new_key.as_deref() {
            order.push(key);
        }
        order.push(&legacy_access);
    } else {
        order.push(&legacy_access);
        if let Some(key) = new_key.as_deref() {
            order.push(key);
        }
    }

    let mut last: Option<crate::securestore::SecretError> = None;
    for key in order {
        match app.secure.get(key) {
            Ok(bytes) => {
                let token =
                    String::from_utf8(bytes).map_err(|_| crate::securestore::SecretError {
                        kind: SecretErrorKind::Other,
                        message: "凭据不是 UTF-8".to_string(),
                        os_status: None,
                    })?;
                if token.trim().is_empty() {
                    last = Some(crate::securestore::SecretError {
                        kind: SecretErrorKind::NotFound,
                        message: "访问令牌为空".to_string(),
                        os_status: None,
                    });
                    continue;
                }
                return Ok(token);
            }
            Err(err) if err.kind == SecretErrorKind::NotFound => {
                last = Some(err);
            }
            Err(err) => return Err(err),
        }
    }
    Err(last.unwrap_or(crate::securestore::SecretError {
        kind: SecretErrorKind::NotFound,
        message: "凭据不存在".to_string(),
        os_status: None,
    }))
}

/// 界面 / `health` 用的授权快照。**不含令牌**，只给状态与指纹级信息。
pub fn describe(app: &App, account_id: &str) -> RpcResult<Vec<Value>> {
    let rows = app.with_db(|db| db.list_authorizations(account_id))?;
    Ok(rows
        .iter()
        .map(|row| {
            json!({
                "id": row.id,
                "kind": row.kind,
                "status": row.status,
                "tokenExpiresAt": row.token_expires_at,
                "sourceKey": row.source_key,
                "keyMigrationState": row.key_migration_state,
                "lastUsedAt": row.last_used_at,
                "lastError": row.last_error,
            })
        })
        .collect())
}

/// 账号层面的授权摘要：两份是否存在、当前会用哪一份。
pub fn summary(app: &App, account: &AccountRecord) -> Value {
    let rows = app
        .with_db(|db| db.list_authorizations(&account.id))
        .unwrap_or_default();
    let has = |kind: &str| rows.iter().any(|row| row.kind == kind);
    let preferred = preferred_source(app);
    let selected = match resolve(app, account) {
        Ok(Outcome::Resolved(resolved)) => json!({
            "kind": resolved.kind,
            "authorizationId": resolved.authorization_id,
            "degraded": resolved.degraded,
        }),
        Ok(Outcome::Unavailable { reason, detail }) => json!({
            "kind": Value::Null,
            "reason": reason,
            "detail": detail,
        }),
        Err(err) => json!({ "kind": Value::Null, "reason": "error", "detail": err.message }),
    };
    json!({
        "preferred": preferred,
        "hasOwn": has(ids::KIND_OWN),
        "hasMagpie": has(ids::KIND_MAGPIE),
        "selected": selected,
        "authorizations": describe(app, &account.id).unwrap_or_default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_app() -> (std::sync::Arc<App>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let paths = crate::paths::Paths {
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data,
            wb_home_owned: true,
        };
        (
            std::sync::Arc::new(App::with_secure(
                paths,
                crate::securestore::SecureStore::memory(),
            )),
            dir,
        )
    }

    fn seed_account(app: &App) -> AccountRecord {
        app.with_db(|db| {
            db.upsert_account(&crate::storage::AccountInput {
                platform: "workbuddy".to_string(),
                uid: "uid-broker".to_string(),
                nickname: Some("测试".to_string()),
                ..Default::default()
            })
        })
        .expect("seed")
    }

    #[test]
    fn preferred_source_defaults_to_magpie_and_validates() {
        let (app, _dir) = test_app();
        assert_eq!(preferred_source(&app), "magpie");
        assert!(set_preferred_source(&app, "own").is_ok());
        assert_eq!(preferred_source(&app), "own");
        assert!(
            set_preferred_source(&app, "qoder").is_err(),
            "非法值必须被拒"
        );
        assert_eq!(preferred_source(&app), "own", "被拒的值不能写进库");
    }

    #[test]
    fn usable_ignores_expired_and_invalid_rows() {
        let mut row = crate::storage::AuthorizationRecord {
            id: "x".into(),
            account_id: "a".into(),
            kind: "own".into(),
            access_ref: "keychain:k".into(),
            refresh_ref: None,
            auth_domain: None,
            token_expires_at: None,
            status: "valid".into(),
            version: 0,
            key_migration_state: "ready".into(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(is_usable(&row));
        row.status = "invalid".into();
        assert!(!is_usable(&row));
        row.status = "valid".into();
        row.token_expires_at = Some("2000-01-01T00:00:00Z".into());
        assert!(!is_usable(&row), "过期时间已过必须算不可用");
        row.token_expires_at = Some("不是时间".into());
        assert!(is_usable(&row), "时间格式不认识时不该据此判定失效");
    }

    #[test]
    fn resolve_reads_the_legacy_key_until_migration_is_ready() {
        let (app, _dir) = test_app();
        let account = seed_account(&app);
        // v6 回填出来的行（pending），凭据还在旧键
        let row = app
            .with_db(|db| {
                db.upsert_authorization(&crate::storage::AuthorizationRecord {
                    id: ids::auth_id(&account.id, "own"),
                    account_id: account.id.clone(),
                    kind: "own".to_string(),
                    access_ref: ids::keychain_ref(&ids::access_key(&account.id, "own")),
                    refresh_ref: None,
                    auth_domain: None,
                    token_expires_at: None,
                    status: "valid".to_string(),
                    version: 0,
                    key_migration_state: "pending".to_string(),
                    source_key: None,
                    last_used_at: None,
                    last_error: None,
                    created_at: crate::clock::now_rfc3339(),
                    updated_at: crate::clock::now_rfc3339(),
                })?;
                db.get_authorization(&ids::auth_id(&account.id, "own"))
            })
            .expect("insert")
            .expect("row");
        assert_eq!(row.id, ids::auth_id(&account.id, "own"));

        let (legacy_access, _) = ids::legacy_keys(&account.uid);
        app.secure
            .set(&legacy_access, b"legacy-token")
            .expect("seed");

        match resolve(&app, &account).expect("resolve") {
            Outcome::Resolved(resolved) => {
                assert_eq!(resolved.kind, "own");
                assert_eq!(resolved.access, "legacy-token");
            }
            Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
        }
    }
}
