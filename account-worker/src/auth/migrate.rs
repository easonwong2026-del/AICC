//! 旧钥匙串键 → 新键的搬运状态机（G4）。
//!
//! 为什么不在 SQL 里做：钥匙串没有事务、会弹系统授权框、可能被用户拒绝。
//! 塞进数据库迁移事务的结果是"库回滚了、钥匙串写了一半"，比不迁移更糟。
//!
//! 所以这里是一个**可中断、可重入**的搬运器：
//!
//! ```text
//! pending ──► migrating ──► ready        读到旧凭据并写成功（或本来就没有）
//!                  │
//!                  └─────► failed        读取被拒 / 写入失败
//!                             │
//!                             └──► pending（下次启动重试）
//! ```
//!
//! 三条硬规则：
//! 1. **只复制已存在的授权**，不凭空造第二套凭据；旧键一个都不删（删除是 v8 的事）。
//! 2. **读取失败 ≠ 凭据不存在**：只有 `NotFound` 才算"没有"，`Denied` 要进 `failed`。
//! 3. 目标键已存在且内容一致 → 直接 `ready`，重复执行没有副作用。

use serde_json::{json, Value};

use crate::app::App;
use crate::rpc::RpcResult;
use crate::securestore::SecretErrorKind;
use crate::storage::authz::{KEY_STATE_FAILED, KEY_STATE_MIGRATING, KEY_STATE_READY};

use super::ids;

#[derive(Debug, Default, Clone)]
pub struct Report {
    /// 这次真正搬了内容的授权数。
    pub moved: usize,
    /// 本来就不需要搬（旧键不存在，或目标已经一致）。
    pub unchanged: usize,
    pub failed: usize,
    /// 迁移失败原因的分类，不含任何凭据内容。
    pub error_kind: Option<String>,
}

enum Copy {
    Copied,
    AlreadyThere,
    Nothing,
}

/// 跑一轮搬运。已经 `ready` 的行不会再被读到，所以重复调用是廉价的。
pub fn run(app: &App) -> RpcResult<Report> {
    let pending = app.with_db(|db| db.authorizations_pending_key_migration())?;
    let mut report = Report::default();

    for row in pending {
        let Some(account) = app.with_db(|db| db.get_account(&row.account_id))? else {
            // 账号已经删了，行会被外键级联清掉；这里什么都不做。
            continue;
        };
        // 上一轮可能崩在 migrating：当成 pending 继续处理（可重入）。
        app.with_db(|db| db.set_key_migration_state(&row.id, KEY_STATE_MIGRATING, None))?;

        match move_one(app, &account.uid, &row) {
            Ok(Copy::Copied) => {
                app.with_db(|db| db.set_key_migration_state(&row.id, KEY_STATE_READY, None))?;
                report.moved += 1;
            }
            Ok(Copy::AlreadyThere | Copy::Nothing) => {
                app.with_db(|db| db.set_key_migration_state(&row.id, KEY_STATE_READY, None))?;
                report.unchanged += 1;
            }
            Err((kind, message)) => {
                app.with_db(|db| {
                    db.set_key_migration_state(&row.id, KEY_STATE_FAILED, Some(&message))
                })?;
                report.failed += 1;
                report.error_kind.get_or_insert(kind);
            }
        }
    }
    Ok(report)
}

/// 搬一份授权：access 必搬，refresh 只搬 own 的。
fn move_one(
    app: &App,
    uid: &str,
    row: &crate::storage::AuthorizationRecord,
) -> Result<Copy, (String, String)> {
    let (legacy_access, legacy_refresh) = ids::legacy_keys(uid);
    let Some(target_access) = row.access_key() else {
        return Err((
            "bad_ref".to_string(),
            "授权记录的 access 引用不可用".to_string(),
        ));
    };

    let mut outcome = copy_key(app, &legacy_access, &target_access)?;

    // magpie 永远没有 refresh：这一段代码里根本没有它的分支，
    // 表上的 CHECK 也只能拦住"写了引用"，拦不住"读了 refresh"。
    if row.kind == ids::KIND_OWN {
        if let Some(target_refresh) = row.refresh_key() {
            match copy_key(app, &legacy_refresh, &target_refresh)? {
                Copy::Copied => outcome = Copy::Copied,
                Copy::AlreadyThere | Copy::Nothing => {}
            }
        }
    }
    Ok(outcome)
}

fn copy_key(app: &App, from: &str, to: &str) -> Result<Copy, (String, String)> {
    let source = match app.secure.get(from) {
        Ok(bytes) => bytes,
        Err(err) if err.kind == SecretErrorKind::NotFound => {
            // 旧键没有内容：目标已经有就直接算完成，没有也算完成（本来就没什么可搬）。
            return match app.secure.get(to) {
                Ok(_) => Ok(Copy::AlreadyThere),
                Err(err) if err.kind == SecretErrorKind::NotFound => Ok(Copy::Nothing),
                Err(err) => Err((error_kind(&err.kind).to_string(), sanitized(&err.message))),
            };
        }
        // 读被拒不是"没有"：进 failed 等下次重试，绝不能标 ready。
        Err(err) => {
            return Err((error_kind(&err.kind).to_string(), sanitized(&err.message)));
        }
    };

    if let Ok(existing) = app.secure.get(to) {
        if existing == source {
            return Ok(Copy::AlreadyThere);
        }
    }
    app.secure
        .set(to, &source)
        .map(|_| Copy::Copied)
        .map_err(|err| (error_kind(&err.kind).to_string(), sanitized(&err.message)))
}

fn error_kind(kind: &SecretErrorKind) -> &'static str {
    match kind {
        SecretErrorKind::NotFound => "not_found",
        SecretErrorKind::Denied => "denied",
        SecretErrorKind::Unavailable => "unavailable",
        SecretErrorKind::Other => "error",
    }
}

fn sanitized(message: &str) -> String {
    crate::rpc::sanitize(message)
}

/// `health` 用：迁移进度。**不含任何凭据内容**。
pub fn health(app: &App) -> Value {
    let counts = app
        .with_db(|db| db.authorization_migration_counts())
        .unwrap_or_default();
    let count_of = |state: &str| {
        counts
            .iter()
            .find(|(key, _)| key == state)
            .map(|(_, n)| *n)
            .unwrap_or(0)
    };
    let pending = count_of("pending");
    let migrating = count_of("migrating");
    let ready = count_of("ready");
    let failed = count_of("failed");
    let last_error = app
        .with_db(|db| db.last_migration_error())
        .ok()
        .flatten()
        .map(|text| json!({ "message": text }));

    json!({
        "pending": pending,
        "migrating": migrating,
        "ready": ready,
        "failed": failed,
        "lastError": last_error,
        // 有 fail / pending 就说明还可以重试：下次启动或调用 storage.init 会再跑一轮。
        "retryable": failed > 0 || pending > 0 || migrating > 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;
    use std::sync::Arc;

    fn test_app(store: SecureStore) -> (Arc<App>, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let paths = Paths {
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data,
            wb_home_owned: true,
        };
        (Arc::new(App::with_secure(paths, store)), dir)
    }

    /// 建一个 v5 时代形态的账号：只有旧键里有凭据，授权行是回填出来的 pending。
    fn seed_legacy_account(app: &App, uid: &str, source: &str) -> crate::storage::AccountRecord {
        let account = app
            .with_db(|db| {
                db.upsert_account(&crate::storage::AccountInput {
                    platform: "workbuddy".to_string(),
                    uid: uid.to_string(),
                    auth_status: Some("valid".to_string()),
                    auth_source: Some(source.to_string()),
                    ..Default::default()
                })
            })
            .expect("seed account");
        // v6 回填（真实路径由迁移做，这里直接跑同一个函数体不现实，手工建等价行）
        app.with_db(|db| {
            let kind = ids::kind_from_source(source);
            db.upsert_authorization(&crate::storage::AuthorizationRecord {
                id: ids::auth_id(&account.id, kind),
                account_id: account.id.clone(),
                kind: kind.to_string(),
                access_ref: ids::keychain_ref(&ids::access_key(&account.id, kind)),
                refresh_ref: (kind == ids::KIND_OWN)
                    .then(|| ids::keychain_ref(&ids::refresh_key(&account.id, kind))),
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
            })
        })
        .expect("seed authorization");
        account
    }

    fn state_of(app: &App, id: &str) -> String {
        app.with_db(|db| db.get_authorization(id))
            .unwrap()
            .expect("row")
            .key_migration_state
    }

    #[test]
    fn own_credentials_are_copied_and_legacy_keys_survive() {
        let store = SecureStore::memory();
        let (app, _dir) = test_app(store);
        let account = seed_legacy_account(&app, "uid-own", "own");
        let (legacy_access, legacy_refresh) = ids::legacy_keys(&account.uid);
        app.secure.set(&legacy_access, b"access-1").unwrap();
        app.secure.set(&legacy_refresh, b"refresh-1").unwrap();

        let report = run(&app).expect("run");
        assert_eq!(report.failed, 0, "{report:?}");
        assert_eq!(report.moved, 1);

        let id = ids::auth_id(&account.id, "own");
        assert_eq!(state_of(&app, &id), "ready");
        assert_eq!(
            app.secure
                .get(&ids::access_key(&account.id, "own"))
                .unwrap(),
            b"access-1"
        );
        assert_eq!(
            app.secure
                .get(&ids::refresh_key(&account.id, "own"))
                .unwrap(),
            b"refresh-1"
        );
        // 旧键必须留着：回退旧二进制还要用它
        assert_eq!(app.secure.get(&legacy_access).unwrap(), b"access-1");
        assert_eq!(app.secure.get(&legacy_refresh).unwrap(), b"refresh-1");
    }

    #[test]
    fn magpie_never_gets_a_refresh_key() {
        let (app, _dir) = test_app(SecureStore::memory());
        let account = seed_legacy_account(&app, "uid-magpie", "magpie");
        let (legacy_access, legacy_refresh) = ids::legacy_keys(&account.uid);
        app.secure.set(&legacy_access, b"borrowed").unwrap();
        // 历史脏数据：magpie 账号下残留了一条 refresh
        app.secure.set(&legacy_refresh, b"stale").unwrap();

        run(&app).expect("run");
        assert_eq!(
            app.secure
                .get(&ids::access_key(&account.id, "magpie"))
                .unwrap(),
            b"borrowed"
        );
        assert!(
            app.secure
                .get(&ids::refresh_key(&account.id, "magpie"))
                .is_err(),
            "magpie/refresh 这个键不允许被创建"
        );
    }

    #[test]
    fn repeated_runs_are_idempotent() {
        let (app, _dir) = test_app(SecureStore::memory());
        let account = seed_legacy_account(&app, "uid-idem", "own");
        let (legacy_access, _) = ids::legacy_keys(&account.uid);
        app.secure.set(&legacy_access, b"a").unwrap();

        let first = run(&app).expect("first");
        assert_eq!(first.moved, 1);
        let second = run(&app).expect("second");
        assert_eq!(second.moved, 0, "第二轮不该再搬");
        assert_eq!(second.failed, 0);
        assert_eq!(
            app.with_db(|db| db.list_authorizations(&account.id))
                .unwrap()
                .len(),
            1,
            "重复启动不能长出第二行"
        );
    }

    /// 读被拒（用户点了不允许）必须失败并保持可重试，不能标 ready。
    #[test]
    fn denied_read_marks_failed_and_stays_retryable() {
        let (app, _dir) = test_app(SecureStore::faulty_on_read());
        let account = seed_legacy_account(&app, "uid-denied", "own");
        let (legacy_access, _) = ids::legacy_keys(&account.uid);
        app.secure.set(&legacy_access, b"a").unwrap();
        app.secure.arm();

        let report = run(&app).expect("run");
        assert_eq!(report.failed, 1, "{report:?}");
        assert_eq!(report.error_kind.as_deref(), Some("denied"));
        let id = ids::auth_id(&account.id, "own");
        assert_eq!(state_of(&app, &id), "failed");

        // 解除注入后再跑一轮：应当自己恢复
        app.secure.disarm();
        let report = run(&app).expect("retry");
        assert_eq!(report.failed, 0, "{report:?}");
        assert_eq!(state_of(&app, &id), "ready");
    }

    /// 写新键失败：旧键原样保留，该行 failed。
    #[test]
    fn write_failure_keeps_legacy_credentials() {
        let (app, _dir) = test_app(SecureStore::faulty(&["/own/access"], &[]));
        let account = seed_legacy_account(&app, "uid-wfail", "own");
        let (legacy_access, _) = ids::legacy_keys(&account.uid);
        app.secure.set(&legacy_access, b"a").unwrap();
        app.secure.arm();

        let report = run(&app).expect("run");
        assert_eq!(report.failed, 1, "{report:?}");
        assert_eq!(report.error_kind.as_deref(), Some("denied"));
        assert_eq!(app.secure.get(&legacy_access).unwrap(), b"a");
    }

    #[test]
    fn health_reports_counts_without_secrets() {
        let (app, _dir) = test_app(SecureStore::memory());
        seed_legacy_account(&app, "uid-h", "own");
        let before = health(&app);
        assert_eq!(before["pending"], json!(1));
        assert_eq!(before["retryable"], json!(true));

        run(&app).expect("run");
        let after = health(&app);
        assert_eq!(after["ready"], json!(1));
        assert_eq!(after["retryable"], json!(false));
        assert!(after["lastError"].is_null());
        // 整个报告里不该出现凭据键或令牌
        let text = after.to_string();
        assert!(!text.contains("workbuddy/"), "{text}");
    }
}
