use std::sync::Arc;

use serde_json::{json, Value};
use wb_switch_core::modules::{config as wb_config, variant::WbVariant};

use super::{log_note, require_str, METHODS};
use crate::app::App;
use crate::providers::{cred_migrate, qoder, workbuddy};
use crate::rpc::{RpcError, RpcResult};
use crate::version;

pub(super) async fn dispatch(app: &Arc<App>, method: &str, params: &Value) -> RpcResult<Value> {
    match method {
        "ping" => Ok(json!({
            "pong": true,
            "protocol": version::PROTOCOL_VERSION,
            "uptimeMs": app.uptime_ms(),
        })),

        "health" => Ok(health(app)),

        "env.info" => Ok(env_info(app)),

        "worker.shutdown" => Ok(json!({ "shuttingDown": true })),

        "storage.init" => {
            let (version, path) =
                app.with_db(|db| Ok((db.migrate()?, db.path().to_string_lossy().to_string())))?;
            // 迁移失败（钥匙串被拒等）不阻断：读凭据会自动退回旧键，这里只把状态报出来。
            let report = crate::auth::migrate::run(app).ok();
            Ok(json!({
                "schemaVersion": version,
                "path": path,
                "credentialMigration": report.map(|r| json!({
                    "moved": r.moved,
                    "unchanged": r.unchanged,
                    "failed": r.failed,
                    "errorKind": r.error_kind,
                })),
            }))
        }

        "storage.status" => app.with_db(|db| db.status()),

        // ---- 局域网 HTTPS 身份（PR-B）----
        //
        // Keys never leave the worker: these three answer with the pin, the
        // expiry and the SAN list, and the private key stays in a 0600 file
        // the Python bridge reads directly.
        "tls.identity" => {
            let store =
                crate::tls_certs::TlsStore::new(crate::tls_certs::TlsStore::default_directory());
            match store.read() {
                Ok(identity) => Ok(serde_json::to_value(identity).unwrap_or_else(|_| json!({}))),
                Err(_) => Ok(
                    json!({ "spki": null, "generation": 0, "sans": [], "caCreated": false,
                                     "leafReissued": false, "notAfter": null }),
                ),
            }
        }

        "tls.ensure" => {
            let sans: Vec<String> = params
                .get("sans")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let store =
                crate::tls_certs::TlsStore::new(crate::tls_certs::TlsStore::default_directory());
            let identity = store
                .ensure(&sans)
                .map_err(|error| RpcError::internal(error.to_string()))?;
            serde_json::to_value(identity).map_err(|error| RpcError::internal(error.to_string()))
        }

        "tls.reset" => {
            let store =
                crate::tls_certs::TlsStore::new(crate::tls_certs::TlsStore::default_directory());
            let identity = store
                .reset()
                .map_err(|error| RpcError::internal(error.to_string()))?;
            serde_json::to_value(identity).map_err(|error| RpcError::internal(error.to_string()))
        }

        "groups.list" => app.with_db(|db| Ok(json!({ "groups": db.list_groups()? }))),

        "groups.upsert" => {
            let id = require_str(params, "id")?;
            let name = require_str(params, "name")?;
            let sort_order = params.get("sortOrder").and_then(Value::as_i64).unwrap_or(0);
            app.with_db(|db| {
                db.upsert_group(&id, &name, sort_order)?;
                Ok(json!({ "ok": true }))
            })
        }

        "secure.status" => {
            let report = app.secure.probe();
            app.record_secure_probe(report.clone());
            Ok(report)
        }

        "secure.set" => {
            require_secure_rpc("secure.set")?;
            let key = require_str(params, "key")?;
            let value = require_str(params, "value")?;
            validate_secure_key(&key)?;
            app.secure.set(&key, value.as_bytes())?;
            Ok(json!({
                "stored": true,
                "fingerprint": crate::securestore::fingerprint(value.as_bytes()),
            }))
        }

        // 只回指纹与长度，**永远不回明文**。
        //
        // 历史上有过 `reveal: true`，等于把钥匙串能力原样暴露给 RPC 调用方：
        // 任何能写 stdin 的进程都能把 access / refresh 读走。真实的内部调用
        // （查登录态、刷新、旅行）是 Rust 里直接 `app.secure.get()`，
        // 不经过这层协议，所以关掉它不影响任何功能。
        //
        // 仍然接受并忽略 `reveal` 参数：老调用方拿到的字段少了，但不会报错。
        "secure.get" => {
            require_secure_rpc("secure.get")?;
            let key = require_str(params, "key")?;
            validate_secure_key(&key)?;
            if params.get("reveal").and_then(Value::as_bool) == Some(true) {
                log_denied_reveal(&key);
            }
            let secret = app.secure.get(&key)?;
            Ok(json!({
                "exists": true,
                "fingerprint": crate::securestore::fingerprint(&secret),
                "length": secret.len(),
                "revealed": false,
            }))
        }

        "secure.delete" => {
            require_secure_rpc("secure.delete")?;
            let key = require_str(params, "key")?;
            validate_secure_key(&key)?;
            Ok(json!({ "deleted": app.secure.delete(&key)? }))
        }

        // ---- 全局设置（M3）----
        //
        // 只接受已知的 key：不给"随便存点什么"的口子，否则设置表会变成第二个
        // 无校验的键值存储，谁都能往里面塞东西。
        "settings.get" => {
            let key = params.get("key").and_then(Value::as_str);
            match key {
                Some(key) => {
                    require_setting_key(key)?;
                    let value = crate::auth::broker::read_setting(app, key)?;
                    Ok(json!({ "key": key, "value": value }))
                }
                None => Ok(json!({
                    "settings": {
                        crate::auth::broker::PREFERRED_SETTING: crate::auth::broker::preferred_source(app),
                        crate::auth::consent::JSON_CONSENT_SETTING: crate::auth::consent::stored_value(app),
                    }
                })),
            }
        }

        "settings.set" => {
            let key = require_str(params, "key")?;
            let value = require_str(params, "value")?;
            require_setting_key(&key)?;
            let mirror_errors: Vec<Value>;
            match key.as_str() {
                crate::auth::broker::PREFERRED_SETTING => {
                    // 与授权写入串行：切首选来源的同时在跑 resync，镜像会来回翻。
                    let _gate = crate::credentials::gate::begin(app)?;
                    crate::auth::broker::set_preferred_source(app, &value)?;
                    // 首选变了，旧版兼容镜像必须跟着换。失败不回滚设置本身 ——
                    // 设置是用户意图，镜像是派生值，报出来比悄悄不一致好。
                    mirror_errors = crate::auth::service::resync_all_mirrors(app)?
                        .into_iter()
                        .map(|(account_id, message)| {
                            json!({ "accountId": account_id, "message": message })
                        })
                        .collect();
                }
                crate::auth::consent::JSON_CONSENT_SETTING => {
                    // 明文 JSON 的首次告知：只有明确肯定才打开，其余一律当作"不启用"。
                    let enabled = matches!(value.trim(), "1" | "true" | "yes" | "on");
                    crate::auth::consent::set_json_storage(app, enabled)?;
                    mirror_errors = Vec::new();
                }
                other => return Err(RpcError::invalid_params(format!("未知设置项：{other}"))),
            }
            Ok(json!({ "key": key, "value": value, "mirrorErrors": mirror_errors }))
        }

        // ---- 单份授权删除（M2）----
        //
        // 只删这一份的键与记录：不删账号、不碰另一份、不动签到与旅行历史。
        "credentials.migration.inventory" => cred_migrate::inventory(app),
        "credentials.migration.run" => cred_migrate::run(app, &params),
        "credentials.migration.rollback" => cred_migrate::rollback(app, &params),
        "credentials.migration.verify" => cred_migrate::verify(app),
        "credentials.migration.relink" => cred_migrate::relink(app, &params),

        // 旧键搬运（G4）的显式重试。这是 `failed` 回到自动流程的唯一入口：
        // 会读一次钥匙串，可能弹系统授权框，所以必须由用户点。
        "credentials.keychain.retry" => {
            let _gate = crate::credentials::gate::begin(app)?;
            let (reset, report) = crate::auth::migrate::retry_failed(app)?;
            let remaining = crate::auth::migrate::failed_count(app)?;
            Ok(json!({
                "reset": reset,
                "moved": report.moved,
                "unchanged": report.unchanged,
                "failed": report.failed,
                "failedRemaining": remaining,
                "errorKind": report.error_kind,
            }))
        }

        // ---- Qoder 每日 Credits（PR #75）----
        //
        // 查询是只读的、领取是有副作用的、自动领取是**默认关闭**的。
        // 三者分开成不同方法，界面上的按钮也就能分开。
        "tasks.overview" => crate::tasks::overview(app),

        other => Err(RpcError::method_not_found(other)),
    }
}

/// `secure.get` 收到 `reveal: true` 时留一行痕迹：能力已经关掉，
/// 但"有人还在这么问"本身就是值得进日志的信号。
fn log_denied_reveal(key: &str) {
    log_note(&format!(
        "拒绝 secure.get 的 reveal 请求（key={key}），只返回指纹"
    ));
}

pub(super) fn health(app: &App) -> Value {
    let db = app.db_snapshot();
    let secure = app.secure_snapshot();

    let workbuddy = workbuddy::inspect_path(&workbuddy::default_auth_path());
    let qoder = qoder::presence(None);

    // 数据库能打开不等于数据可用：完整性自检失败必须算 degraded，
    // 否则界面会显示"正常"，而库其实已经坏了。
    let db_open = db.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let db_integrity = db
        .get("integrityOk")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let db_ok = db_open && db_integrity;

    let mut health = app.base_health();
    health["status"] = json!(if db_ok { "ok" } else { "degraded" });
    if db_open && !db_integrity {
        health["degradedReason"] = json!("database_integrity_check_failed");
    } else if !db_open {
        health["degradedReason"] = json!("database_unavailable");
    }
    health["database"] = db;
    health["secureStore"] = secure;
    health["credentialMigration"] = crate::auth::migrate::health(app);
    // 明文 JSON 是否已获用户确认：界面据此决定是否弹首次风险说明。
    health["credentials"] = crate::auth::consent::status(app);
    health["authorizations"] = authorization_fleet(app);
    health["providers"] = json!({ "workbuddy": workbuddy, "qoder": qoder });
    #[cfg(debug_assertions)]
    {
        // dev-only 的两条 RPC：正式构建不注册，也不出现在这里。
        let mut methods: Vec<&str> = METHODS.to_vec();
        methods.push("growth.rewards.evidence");
        methods.push("growth.rewards.probe");
        health["methods"] = json!(methods);
    }
    #[cfg(not(debug_assertions))]
    {
        health["methods"] = json!(METHODS);
    }
    health
}

/// `health` 里的授权汇总：按 kind × status 计数，不含账号与令牌。
fn authorization_fleet(app: &App) -> Value {
    let counts = app
        .with_db(|db| db.authorization_status_counts())
        .unwrap_or_default();
    let mut rows = Vec::new();
    let mut usable = 0;
    for (kind, status, n) in counts {
        if status == "valid" {
            usable += n;
        }
        rows.push(json!({ "kind": kind, "status": status, "count": n }));
    }
    // v20：凭据现在可能落在三处。`keychain` 仍有份数 = 还没走动态关联的旧授权，
    // 界面据此判断"迁移是否做完"，所以这里按存储种类给一份计数。
    let storage = app
        .with_db(|db| db.authorization_storage_counts())
        .unwrap_or_default()
        .into_iter()
        .map(|(kind, count)| json!({ "kind": kind, "count": count }))
        .collect::<Vec<_>>();
    json!({
        "usable": usable,
        "rows": rows,
        "storage": storage,
        "preferred": crate::auth::broker::preferred_source(app),
    })
}

fn env_info(app: &App) -> Value {
    let cn = WbVariant::Cn.auth_file_path();
    let ai = WbVariant::Ai.auth_file_path();
    json!({
        "dataDir": app.paths.data_dir.to_string_lossy(),
        "wbSwitchHome": {
            "value": std::env::var(config_home_env()).ok(),
            "ownedByWorker": app.paths.wb_home_owned,
            "storeDir": wb_config::store_dir().to_string_lossy(),
        },
        "workbuddy": {
            "cn": { "path": cn.to_string_lossy(), "exists": cn.exists() },
            "ai": { "path": ai.to_string_lossy(), "exists": ai.exists() },
        },
        "qoder": {
            "envAuthDir": std::env::var(qoder::AUTH_DIR_ENV).ok(),
            "authDirs": qoder::scan_auth_dirs()
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
        },
        "platform": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
    })
}

fn config_home_env() -> &'static str {
    crate::paths::WB_HOME_ENV
}

/// 通用 `secure.*` 默认关闭：内部逻辑走 `SecureStore`，不经过 RPC。
///
/// 能写 stdin 的进程本来就能覆盖 AICC 的凭据，这条通道不该长期存在。
/// 兼容开关只给受控的开发 / 迁移场景用，而且仍然要过键名白名单。
pub(super) fn require_secure_rpc(method: &str) -> RpcResult<()> {
    if std::env::var("AICC_ALLOW_SECURE_RPC").as_deref() == Ok("1") {
        return Ok(());
    }
    Err(RpcError::method_not_found(method))
}

/// 白名单：只允许三形态的新键。旧 UID 格式与 `magpie/refresh` 一律拒绝。
///
/// `workbuddy/<16 hex>/own/access`
/// `workbuddy/<16 hex>/own/refresh`
/// `workbuddy/<16 hex>/magpie/access`
pub(super) fn validate_secure_key(key: &str) -> RpcResult<()> {
    let parts: Vec<&str> = key.split('/').collect();
    let shape_ok = parts.len() == 4
        && parts[0] == "workbuddy"
        && parts[1].len() == 16
        && parts[1].chars().all(|c| c.is_ascii_hexdigit())
        && matches!(
            (parts[2], parts[3]),
            ("own", "access") | ("own", "refresh") | ("magpie", "access")
        );
    if shape_ok {
        Ok(())
    } else {
        Err(RpcError::invalid_params(
            "只接受 workbuddy/<16 hex>/{own/access,own/refresh,magpie/access}",
        ))
    }
}

pub(super) fn require_setting_key(key: &str) -> RpcResult<()> {
    if key == crate::auth::broker::PREFERRED_SETTING
        || key == crate::auth::consent::JSON_CONSENT_SETTING
    {
        Ok(())
    } else {
        Err(RpcError::invalid_params(format!("未知设置项：{key}")))
    }
}
