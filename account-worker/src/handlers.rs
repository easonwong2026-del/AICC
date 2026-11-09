//! JSON-RPC 方法表。
//!
//! 命名约定：`<域>.<动作>`。PR1a 只覆盖本地能力，任何联网行为都不在这里。

use serde_json::{json, Value};

use crate::app::App;
use crate::providers::{qoder, workbuddy};
use crate::rpc::{RpcError, RpcResult};
use crate::storage::{AccountFilter, AccountInput};
use crate::version;
use wb_switch_core::modules::{config as wb_config, variant::WbVariant};

pub const METHODS: &[&str] = &[
    "ping",
    "health",
    "env.info",
    "worker.shutdown",
    "storage.init",
    "storage.status",
    "accounts.list",
    "accounts.get",
    "accounts.upsert",
    "accounts.delete",
    "groups.list",
    "groups.upsert",
    "secure.status",
    "secure.set",
    "secure.get",
    "secure.delete",
    "providers.workbuddy.inspect",
    "providers.workbuddy.oauth",
    "providers.qoder.presence",
    "providers.qoder.inspect",
];

pub fn dispatch(app: &App, method: &str, params: &Value) -> RpcResult<Value> {
    match method {
        "ping" => Ok(json!({
            "pong": true,
            "protocol": version::PROTOCOL_VERSION,
            "uptimeMs": app.uptime_ms(),
        })),

        "health" => Ok(health(app)),

        "env.info" => Ok(env_info(app)),

        "worker.shutdown" => Ok(json!({ "shuttingDown": true })),

        "storage.init" => app.with_db(|db| {
            let version = db.migrate()?;
            Ok(json!({
                "schemaVersion": version,
                "path": db.path().to_string_lossy(),
            }))
        }),

        "storage.status" => app.with_db(|db| db.status()),

        "accounts.list" => {
            let filter: AccountFilter = serde_json::from_value(params.clone())
                .map_err(|e| RpcError::invalid_params(format!("筛选参数非法：{e}")))?;
            app.with_db(|db| Ok(json!({ "accounts": db.list_accounts(&filter)? })))
        }

        "accounts.get" => {
            let id = require_str(params, "id")?;
            app.with_db(|db| match db.get_account(&id)? {
                Some(record) => Ok(json!(record)),
                None => Err(RpcError::not_found(format!("账号不存在：{id}"))),
            })
        }

        "accounts.upsert" => {
            let input: AccountInput = serde_json::from_value(params.clone())
                .map_err(|e| RpcError::invalid_params(format!("账号参数非法：{e}")))?;
            app.with_db(|db| Ok(json!(db.upsert_account(&input)?)))
        }

        "accounts.delete" => {
            let id = require_str(params, "id")?;
            app.with_db(|db| Ok(json!({ "deleted": db.delete_account(&id)? })))
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
            let key = require_str(params, "key")?;
            let value = require_str(params, "value")?;
            app.secure.set(&key, value.as_bytes())?;
            Ok(json!({
                "stored": true,
                "fingerprint": crate::securestore::fingerprint(value.as_bytes()),
            }))
        }

        "secure.get" => {
            let key = require_str(params, "key")?;
            let reveal = params
                .get("reveal")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let secret = app.secure.get(&key)?;
            let mut out = json!({
                "exists": true,
                "fingerprint": crate::securestore::fingerprint(&secret),
                "length": secret.len(),
            });
            // 默认不回明文：调用方要看明文必须显式要求。
            if reveal {
                out["value"] = json!(String::from_utf8_lossy(&secret));
            }
            Ok(out)
        }

        "secure.delete" => {
            let key = require_str(params, "key")?;
            Ok(json!({ "deleted": app.secure.delete(&key)? }))
        }

        "providers.workbuddy.inspect" => {
            let path = params.get("path").and_then(Value::as_str);
            Ok(workbuddy::inspect(path))
        }

        "providers.workbuddy.oauth" => Ok(workbuddy::oauth_capability()),

        "providers.qoder.presence" => {
            let dir = params.get("authDir").and_then(Value::as_str);
            Ok(qoder::presence(dir))
        }

        "providers.qoder.inspect" => {
            let dir = params.get("authDir").and_then(Value::as_str);
            Ok(qoder::inspect(dir))
        }

        other => Err(RpcError::method_not_found(other)),
    }
}

fn health(app: &App) -> Value {
    let db = app.db_snapshot();
    let secure = app.secure_snapshot();

    let workbuddy = workbuddy::inspect_path(&workbuddy::default_auth_path());
    let qoder = qoder::presence(None);

    let db_ok = db.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let mut health = app.base_health();
    health["status"] = json!(if db_ok { "ok" } else { "degraded" });
    health["database"] = db;
    health["secureStore"] = secure;
    health["providers"] = json!({ "workbuddy": workbuddy, "qoder": qoder });
    health["methods"] = json!(METHODS);
    health
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

fn require_str(params: &Value, key: &str) -> RpcResult<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("缺少参数 {key}")))
}
