//! JSON-RPC 方法表。
//!
//! 命名约定：`<域>.<动作>`。PR1a 只覆盖本地能力，任何联网行为都不在这里。

use serde_json::{json, Value};

use std::sync::Arc;

use crate::app::App;
use crate::providers::{magpie, qoder, workbuddy, workbuddy_auth};
use crate::rpc::{RpcError, RpcResult};
use crate::storage::{AccountFilter, AccountInput};
use crate::travel::{scheduler as travel_scheduler, service::EventDraft};
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
    "auth.workbuddy.start",
    "auth.workbuddy.poll",
    "auth.workbuddy.cancel",
    "auth.workbuddy.refresh",
    "providers.magpie.accounts",
    "auth.magpie.import",
    "auth.workbuddy.status",
    // ---- Buddy 旅行（PR1c）----
    "travel.summary",
    "travel.events",
    "travel.refresh",
    "travel.depart",
    "travel.claim",
    "travel.setEnabled",
    "travel.auto.status",
    "travel.auto.tick",
];

pub async fn dispatch(app: &Arc<App>, method: &str, params: &Value) -> RpcResult<Value> {
    // JSON-RPC 允许整个省略 `params`（键不存在时是 null）。统一当空对象，
    // 免得每个方法都各自判一次 null。
    let empty = json!({});
    let params = if params.is_null() { &empty } else { params };

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
            let existing = app.with_db(|db| db.get_account(&id))?;
            let deleted = app.with_db(|db| db.delete_account(&id))?;
            // 账号删掉后令牌没有主人了，必须一起清，避免钥匙串里留孤儿凭据。
            if deleted {
                if let Some(account) = existing {
                    purge_workbuddy_secrets(app, &account);
                }
            }
            Ok(json!({ "deleted": deleted }))
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

        // 只回指纹与长度，**永远不回明文**。
        //
        // 历史上有过 `reveal: true`，等于把钥匙串能力原样暴露给 RPC 调用方：
        // 任何能写 stdin 的进程都能把 access / refresh 读走。真实的内部调用
        // （查登录态、刷新、旅行）是 Rust 里直接 `app.secure.get()`，
        // 不经过这层协议，所以关掉它不影响任何功能。
        //
        // 仍然接受并忽略 `reveal` 参数：老调用方拿到的字段少了，但不会报错。
        "secure.get" => {
            let key = require_str(params, "key")?;
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

        // ---- WorkBuddy 扫码登录（PR1b-1）----
        "auth.workbuddy.start" => {
            let info = workbuddy_auth::start(WbVariant::Cn)
                .await
                .map_err(auth_rpc_error)?;
            Ok(json!({
                "platform": "workbuddy",
                "loginId": info.login_id,
                "verificationUri": info.verification_uri,
                "expiresIn": info.expires_in,
                // 前端自己渲染二维码，这里只给要编码的字符串。
                "qrPayload": info.verification_uri,
            }))
        }

        "auth.workbuddy.poll" => {
            let login_id = require_str(params, "loginId")?;
            match workbuddy_auth::poll(&login_id).await {
                workbuddy_auth::Polled::Pending => Ok(json!({ "state": "pending" })),
                workbuddy_auth::Polled::Failed(err) => Ok(json!({
                    "state": poll_state(&err.kind),
                    "error": { "kind": err.kind, "message": err.message },
                })),
                workbuddy_auth::Polled::Token(payload) => match persist_login(app, &payload) {
                    Ok(result) => {
                        workbuddy_auth::complete(&login_id, result.clone());
                        Ok(result)
                    }
                    Err(err) => {
                        workbuddy_auth::fail(
                            &login_id,
                            workbuddy_auth::KIND_NETWORK,
                            err.message.clone(),
                        );
                        Ok(json!({
                            "state": "failed",
                            "error": { "kind": "storage", "message": err.message },
                        }))
                    }
                },
            }
        }

        "auth.workbuddy.cancel" => {
            let login_id = require_str(params, "loginId")?;
            Ok(json!({ "cancelled": workbuddy_auth::cancel(&login_id) }))
        }

        "auth.workbuddy.refresh" => {
            let id = require_str(params, "id")?;
            refresh_account(app, &id).await
        }

        // ---- 从 Magpie 借用登录态（PR1b-1）----
        //
        // 只读 Magpie 的 access token，不碰 refresh：refresh 是有状态的单份凭据，
        // 两边各自刷新会互相踢下线。过期后用 `auth.magpie.import` 重新同步。
        "providers.magpie.accounts" => Ok(magpie_preview(app)),

        "auth.workbuddy.status" => {
            let id = require_str(params, "id")?;
            workbuddy_account_status(app, &id).await
        }

        // ---- Buddy 旅行（PR1c）----
        //
        // 三段式：`summary` 只看本地快照（界面轮询用它，不打接口）；
        // `refresh` 查一次真实状态；`depart` / `claim` 是用户显式点的动作。
        "travel.summary" => Ok(travel_summary(app)),

        "travel.events" => {
            let id = require_str(params, "id")?;
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(20)
                .min(200) as u32;
            app.with_db(|db| Ok(json!({ "events": db.travel_events(&id, limit)? })))
        }

        "travel.setEnabled" => {
            let id = require_str(params, "id")?;
            let enabled = require_bool(params, "enabled")?;
            app.with_db(|db| {
                if db.get_account(&id)?.is_none() {
                    return Err(RpcError::not_found(format!("账号不存在：{id}")));
                }
                db.set_travel_enabled(&id, enabled)?;
                Ok(json!({ "id": id, "travelEnabled": enabled }))
            })
        }

        // 刷新是**纯只读**：只查状态，看到「已到达」也不提交领取。
        "travel.refresh" => {
            let id = require_str(params, "id")?;
            travel_sync(app, &id, SyncIntent::Observe).await
        }

        // 每个按钮只做一件事：派出不顺手领奖，领奖不顺手派出。
        "travel.depart" => {
            let id = require_str(params, "id")?;
            travel_sync(app, &id, SyncIntent::Depart).await
        }

        "travel.claim" => {
            let id = require_str(params, "id")?;
            travel_sync(app, &id, SyncIntent::Claim).await
        }

        "travel.auto.status" => Ok(travel_scheduler::auto_status(app)),

        "travel.auto.tick" => Ok(travel_scheduler::run_tick(app).await),

        "auth.magpie.import" => {
            let keys: Vec<String> = params
                .get("keys")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            import_magpie_accounts(app, &keys)
        }

        other => Err(RpcError::method_not_found(other)),
    }
}

/// AICC 自有授权：扫码拿到、AICC 自己持有 refresh token。
const OWN_SOURCE: &str = "own";

fn log_note(message: &str) {
    eprintln!("[aicc-account-worker] {}", crate::rpc::sanitize(message));
}

/// `secure.get` 收到 `reveal: true` 时留一行痕迹：能力已经关掉，
/// 但"有人还在这么问"本身就是值得进日志的信号。
fn log_denied_reveal(key: &str) {
    log_note(&format!(
        "拒绝 secure.get 的 reveal 请求（key={key}），只返回指纹"
    ));
}

fn health(app: &App) -> Value {
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

// ---------------------------------------------------------------------------
// WorkBuddy 登录辅助
// ---------------------------------------------------------------------------

/// 用库里存着的 access token 查一次真实登录态（只读）。
///
/// 这是"验证令牌确实能用"的唯一诚实方式：不查这一步，`auth_status` 只是
/// 我们自己写进去的乐观值。请求失败时顺手把账号标成 invalid/expired，
/// 界面就不会再显示一个骗人的"认证有效"。
async fn workbuddy_account_status(app: &App, id: &str) -> RpcResult<Value> {
    let account = app
        .with_db(|db| db.get_account(id))?
        .ok_or_else(|| RpcError::not_found(format!("账号不存在：{id}")))?;
    if account.platform != "workbuddy" {
        return Err(RpcError::invalid_params("目前只支持查询 WorkBuddy 账号"));
    }

    let (access_key, _) = workbuddy_auth::secret_keys(&account.uid);
    let access = match app.secure.get(&access_key) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        Err(err) => {
            return Ok(json!({
                "state": "missing_credential",
                "error": {
                    "kind": "missing_credential",
                    "message": crate::rpc::sanitize(&err.message),
                },
            }));
        }
    };

    let mut payload = json!({
        "access_token": access,
        "uid": account.uid,
        "domain": account.auth_domain.clone().unwrap_or_default(),
        "variant": "cn",
    });
    let mut status = wb_switch_core::modules::checkin::get_checkin_status(&payload).await;

    // Magpie 借来的令牌过期时，先重新同步一次再判定失败，避免误报"账号失效"。
    if status.get("ok").and_then(Value::as_bool) != Some(true)
        && account.auth_source == magpie::SOURCE_MAGPIE
    {
        if let Ok(synced) = resync_magpie_access(app, &account) {
            if synced.get("state").and_then(Value::as_str) == Some("valid") {
                if let Ok(bytes) = app.secure.get(&access_key) {
                    payload["access_token"] = json!(String::from_utf8_lossy(&bytes).to_string());
                    status = wb_switch_core::modules::checkin::get_checkin_status(&payload).await;
                }
            }
        }
    }

    let ok = status.get("ok").and_then(Value::as_bool).unwrap_or(false);
    if ok {
        app.with_db(|db| db.set_auth_state(id, "valid", None))?;
    } else if status.get("statusUnsupported").is_none() {
        app.with_db(|db| db.set_auth_state(id, "invalid", None))?;
    }

    Ok(json!({
        "state": if ok { "valid" } else { "invalid" },
        "todayCheckedIn": status.get("todayCheckedIn").cloned().unwrap_or(Value::Null),
        "streakDays": status.get("raw").and_then(|r| r.get("streak_days")).cloned().unwrap_or(Value::Null),
        "dailyCredit": status.get("raw").and_then(|r| r.get("daily_credit")).cloned().unwrap_or(Value::Null),
        "todayCredit": status.get("raw").and_then(|r| r.get("today_credit")).cloned().unwrap_or(Value::Null),
        "statusUnsupported": status.get("statusUnsupported").cloned().unwrap_or(Value::Null),
        "error": status.get("error").cloned().unwrap_or(Value::Null),
    }))
}

/// 列出 Magpie 里的 WorkBuddy 账号，并标出哪些已经在 AICC 账号库里。
fn magpie_preview(app: &App) -> Value {
    let mut report = magpie::preview();
    if report.get("available").and_then(Value::as_bool) != Some(true) {
        return report;
    }
    let known: std::collections::HashMap<String, (String, String)> = app
        .with_db(|db| {
            db.list_accounts(&AccountFilter {
                platform: Some("workbuddy".to_string()),
                ..Default::default()
            })
        })
        .unwrap_or_default()
        .into_iter()
        .map(|a| (a.uid, (a.auth_source, a.id)))
        .collect();

    if let Some(accounts) = report.get_mut("accounts").and_then(Value::as_array_mut) {
        for item in accounts.iter_mut() {
            let uid = item.get("uid").and_then(Value::as_str).unwrap_or_default();
            let hit = known.get(uid);
            item["imported"] = json!(hit.is_some());
            item["importedSource"] = json!(hit.map(|(source, _)| source.clone()));
            item["accountId"] = json!(hit.map(|(_, id)| id.clone()));
        }
    }
    report
}

/// 把 Magpie 的账号搬进 AICC：access token 进钥匙串，账号进 SQLite。
///
/// 四条不变量（改动前先看）：
/// 1. 只搬 access，**绝不**读 / 写 refresh；顺手把该 uid 下遗留的 refresh 删掉，
///    因为 magpie 来源的账号任何情况下都不该拿 refresh 去刷新（会互相踢下线）。
/// 2. 已有的 `own`（扫码授权）账号不被"降级"成借用。
/// 3. 单条失败（钥匙串写不进 / 入库失败）只跳过该条，不整批失败。
/// 4. 不覆盖用户改过的昵称：只在首次导入或库里的昵称为空时才写 Magpie 的名字。
fn import_magpie_accounts(app: &App, keys: &[String]) -> RpcResult<Value> {
    let logins = magpie::read_logins().map_err(|err| {
        RpcError::new(crate::rpc::E_AUTH, err.message()).with_data(json!({ "kind": err.kind() }))
    })?;
    let now = magpie::now_ms();

    let mut imported: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();

    for login in logins {
        if !keys.is_empty() && !keys.iter().any(|k| k == &login.key) {
            continue;
        }
        if login.expires_at_ms.map(|at| at <= now).unwrap_or(false) {
            skipped.push(json!({ "key": login.key, "reason": "token_expired" }));
            continue;
        }

        // 已有的独立授权不能被"降级"成借用。
        let existing = app.with_db(|db| db.find_account("workbuddy", &login.uid))?;
        if let Some(record) = &existing {
            if record.auth_source == OWN_SOURCE {
                skipped.push(json!({
                    "key": login.key,
                    "uid": login.uid,
                    "reason": "already_authorized",
                }));
                continue;
            }
        }

        let (access_key, refresh_key) = workbuddy_auth::secret_keys(&login.uid);
        if let Err(err) = app.secure.set(&access_key, login.access_token.as_bytes()) {
            skipped.push(json!({
                "key": login.key,
                "uid": login.uid,
                "reason": "keychain",
                "message": crate::rpc::sanitize(&err.message),
            }));
            continue;
        }
        // 借用授权的账号不允许持有 refresh（历史版本可能在切来源时留下一条）。
        let _ = app.secure.delete(&refresh_key);

        let expires_at = login.expires_at_ms.and_then(workbuddy_auth::ms_to_rfc3339);
        let keep_nickname = existing
            .as_ref()
            .map(|record| !record.nickname.trim().is_empty())
            .unwrap_or(false);
        let nickname = if keep_nickname {
            None
        } else {
            Some(
                login
                    .nickname
                    .clone()
                    .unwrap_or_else(|| magpie::short_uid(&login.uid)),
            )
        };
        let uid = login.uid.clone();
        let domain = login.domain.clone();
        let written = app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: uid.clone(),
                nickname: nickname.clone(),
                auth_ref: Some(format!("keychain:{access_key}")),
                auth_status: Some("valid".to_string()),
                token_expires_at: expires_at.clone(),
                auth_domain: Some(domain.clone()).filter(|d| !d.is_empty()),
                auth_source: Some(magpie::SOURCE_MAGPIE.to_string()),
                ..Default::default()
            })
        });
        match written {
            Ok(record) => imported.push(json!(record)),
            Err(err) => skipped.push(json!({
                "key": login.key,
                "uid": login.uid,
                "reason": "storage",
                "message": crate::rpc::sanitize(&err.message),
            })),
        }
    }

    Ok(json!({
        "imported": imported.len(),
        "skipped": skipped,
        "accounts": imported,
        "source": magpie::SOURCE_MAGPIE,
    }))
}

/// Magpie 账号不用 refresh token，改为"重新从 Magpie 同步一次 access token"。
///
/// 同步函数：只做「读 Magpie 文件 → 写钥匙串 → 更新账号状态」，没有任何 await。
/// 旅行调度器也要在非 async 的凭据钩子里调它，所以不能挂在 async fn 上。
///
/// **只对 `auth_source = magpie` 的账号有效**。自有授权（扫码）账号走 refresh，
/// 这里多一道闸是因为写错来源的代价很实在：Magpie 里那份 access 可能比 AICC
/// 刚拿到的新令牌旧，覆盖之后账号会莫名其妙地"刷新后立刻失效"。
pub(crate) fn resync_magpie_access(
    app: &App,
    account: &crate::storage::AccountRecord,
) -> RpcResult<Value> {
    if account.auth_source != magpie::SOURCE_MAGPIE {
        return Err(RpcError::new(
            crate::rpc::E_AUTH,
            "该账号使用 AICC 自有授权，不能从 Magpie 覆盖凭据",
        )
        .with_data(json!({ "kind": "not_magpie_source" })));
    }
    let id = account.id.as_str();
    let logins = magpie::read_logins().unwrap_or_default();
    let now = magpie::now_ms();
    let hit = logins
        .into_iter()
        .find(|l| l.uid == account.uid && l.expires_at_ms.map(|at| at > now).unwrap_or(true));

    let Some(login) = hit else {
        app.with_db(|db| db.set_auth_state(id, "expired", None))?;
        return Ok(json!({
            "state": "expired",
            "error": {
                "kind": "magpie_token_expired",
                "message": "Magpie 里的令牌也已过期。请在 Magpie 重新登录，或改用扫码授权。",
            },
        }));
    };

    let (access_key, _) = workbuddy_auth::secret_keys(&account.uid);
    app.secure.set(&access_key, login.access_token.as_bytes())?;
    let expires = login.expires_at_ms.and_then(workbuddy_auth::ms_to_rfc3339);
    app.with_db(|db| db.set_auth_state(id, "valid", expires.as_deref()))?;
    Ok(json!({
        "state": "valid",
        "resynced": true,
        "tokenExpiresAt": expires,
        "source": magpie::SOURCE_MAGPIE,
    }))
}

fn auth_rpc_error(err: workbuddy_auth::AuthError) -> RpcError {
    RpcError::new(crate::rpc::E_AUTH, err.message).with_data(json!({ "kind": err.kind }))
}

/// 把内部错误分类翻译成界面状态。会话已结束的都归到 `finished`，
/// 前端据此回到"刷新账号列表"，而不是继续轮询。
fn poll_state(kind: &str) -> &'static str {
    match kind {
        workbuddy_auth::KIND_CANCELLED => "cancelled",
        workbuddy_auth::KIND_EXPIRED => "expired",
        workbuddy_auth::KIND_FINISHED | workbuddy_auth::KIND_UNKNOWN_LOGIN => "finished",
        _ => "failed",
    }
}

/// 删除一个账号留在钥匙串里的凭据。
///
/// 只剩「这套 key 没有别的账号在用」时才删：`workbuddy/<uid>/access` 是按 uid
/// 命名的，PR #66 之前一个 uid 只会有一行账号，但删除路径写死"删了就清"，
/// 一旦将来出现共用（或手工插入的重号），就会把别人正在用的令牌删掉。
fn purge_workbuddy_secrets(app: &App, account: &crate::storage::AccountRecord) {
    let still_used = app
        .with_db(|db| db.find_account("workbuddy", &account.uid))
        .ok()
        .flatten()
        .map(|other| other.id != account.id)
        .unwrap_or(false);
    if still_used {
        log_note(&format!(
            "跳过清理 {}：同一 uid 还有其它账号行在用它",
            account.uid
        ));
        return;
    }
    let (access, refresh) = workbuddy_auth::secret_keys(&account.uid);
    let _ = app.secure.delete(&access);
    let _ = app.secure.delete(&refresh);
}

/// 写 AICC **自有**授权（扫码拿到的）。
///
/// `refresh = None` 表示这次登录没发刷新令牌，此时必须把旧的删掉：留着会让
/// "新 access + 旧 refresh" 混搭，刷新必然失败，而且失败后还会被误判成账号失效。
fn store_own_credentials(
    app: &App,
    uid: &str,
    access: &str,
    refresh: Option<&str>,
) -> RpcResult<()> {
    let (access_key, refresh_key) = workbuddy_auth::secret_keys(uid);
    app.secure.set(&access_key, access.as_bytes())?;
    match refresh {
        Some(token) => app.secure.set(&refresh_key, token.as_bytes())?,
        None => {
            let _ = app.secure.delete(&refresh_key);
        }
    }
    Ok(())
}

fn read_secret(app: &App, key: &str) -> RpcResult<String> {
    let bytes = app.secure.get(key).map_err(|err| {
        RpcError::new(
            crate::rpc::E_SECURESTORE,
            format!("读取凭据失败（{}），请重新登录", err.message),
        )
        .with_data(json!({ "kind": "missing_credential", "key": key }))
    })?;
    String::from_utf8(bytes)
        .map_err(|_| RpcError::new(crate::rpc::E_SECURESTORE, "凭据不是 UTF-8，请重新登录"))
}

/// 登录成功后落库：令牌进钥匙串，账号进 SQLite，返回值里没有任何令牌。
///
/// **必须把 `auth_source` 写成 `own`**：同一个 uid 可能先从 Magpie 导入过
/// （`auth_source = magpie`，只有借来的 access、没有 refresh）。扫码成功之后，
/// 钥匙串里的凭据已经换成 AICC 自己的了，如果来源还留着 `magpie`，
/// 后续 401 会去重新读 Magpie 的文件、把刚拿到的新令牌覆盖成 Magpie 里那份旧的。
fn persist_login(app: &App, payload: &workbuddy_auth::TokenPayload) -> RpcResult<Value> {
    let uid = payload.profile.uid.clone();
    let (access_key, _) = workbuddy_auth::secret_keys(&uid);

    // 先写钥匙串：写不进去就不入库，避免出现"账号在、令牌丢了"的半残状态。
    store_own_credentials(
        app,
        &uid,
        &payload.access_token,
        payload.refresh_token.as_deref(),
    )?;

    let expires_at = payload
        .expires_at_ms
        .and_then(workbuddy_auth::ms_to_rfc3339);
    let build = |db: &mut crate::storage::Db| {
        let existed = db.find_account("workbuddy", &uid)?.is_some();
        let record = db.upsert_account(&AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.clone(),
            nickname: payload.profile.nickname.clone(),
            email: payload.profile.email.clone(),
            auth_ref: Some(format!("keychain:{access_key}")),
            auth_status: Some("valid".to_string()),
            token_expires_at: expires_at.clone(),
            auth_domain: Some(payload.domain.clone()).filter(|d| !d.is_empty()),
            auth_source: Some(OWN_SOURCE.to_string()),
            ..Default::default()
        })?;
        Ok((existed, record))
    };

    match app.with_db(build) {
        Ok((existed, record)) => Ok(json!({
            "state": "success",
            "reused": existed,
            "account": record,
        })),
        Err(err) => {
            // 入库失败就把刚写进去的令牌撤掉，别留孤儿凭据。
            purge_workbuddy_secrets(app, &record_for_uid(&uid));
            Err(err)
        }
    }
}

/// 只有 uid、没有库行的临时账号视图，仅用于失败回滚时清凭据。
fn record_for_uid(uid: &str) -> crate::storage::AccountRecord {
    crate::storage::AccountRecord {
        id: crate::storage::account_id("workbuddy", uid),
        platform: "workbuddy".to_string(),
        uid: uid.to_string(),
        nickname: String::new(),
        email: None,
        group_id: None,
        auth_status: "unknown".to_string(),
        checkin_enabled: false,
        last_checkin_at: None,
        last_result: None,
        updated_at: crate::clock::now_rfc3339(),
        token_expires_at: None,
        auth_domain: None,
        auth_source: OWN_SOURCE.to_string(),
        travel_enabled: false,
    }
}

/// 用 refresh token 换新令牌；失败就标成 invalid，让界面提示重新登录。
async fn refresh_account(app: &App, id: &str) -> RpcResult<Value> {
    let account = app
        .with_db(|db| db.get_account(id))?
        .ok_or_else(|| RpcError::not_found(format!("账号不存在：{id}")))?;
    if account.platform != "workbuddy" {
        return Err(RpcError::invalid_params("目前只支持刷新 WorkBuddy 账号"));
    }
    // 借用 Magpie 的账号没有 refresh token，重新同步一次 access 即可。
    if account.auth_source == magpie::SOURCE_MAGPIE {
        return resync_magpie_access(app, &account);
    }

    let (access_key, refresh_key) = workbuddy_auth::secret_keys(&account.uid);
    let access = read_secret(app, &access_key)?;
    let refresh = match app.secure.get(&refresh_key) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        Err(_) => {
            app.with_db(|db| db.set_auth_state(id, "invalid", None))?;
            return Ok(json!({
                "state": "invalid",
                "error": { "kind": "missing_credential", "message": "钥匙串里没有刷新令牌，请重新登录" },
            }));
        }
    };

    let domain = account.auth_domain.clone().unwrap_or_default();
    match workbuddy_auth::refresh(WbVariant::Cn, &access, &refresh, &account.uid, &domain).await {
        workbuddy_auth::Refreshed::Valid {
            access_token,
            refresh_token,
            expires_at_ms,
        } => {
            app.secure.set(&access_key, access_token.as_bytes())?;
            if let Some(next) = refresh_token {
                app.secure.set(&refresh_key, next.as_bytes())?;
            }
            let expires = expires_at_ms.and_then(workbuddy_auth::ms_to_rfc3339);
            app.with_db(|db| db.set_auth_state(id, "valid", expires.as_deref()))?;
            Ok(json!({ "state": "valid", "tokenExpiresAt": expires }))
        }
        workbuddy_auth::Refreshed::Invalid(err) => {
            app.with_db(|db| db.set_auth_state(id, "invalid", None))?;
            Ok(json!({
                "state": "invalid",
                "error": { "kind": err.kind, "message": err.message },
            }))
        }
    }
}

// ---------------------------------------------------------------------------
// Buddy 旅行
// ---------------------------------------------------------------------------

/// 本地快照汇总，**不发任何请求**。
///
/// 界面轮询这个（几秒一次也不会打接口），要更新状态时显式调 `travel.refresh`。
fn travel_summary(app: &App) -> Value {
    let accounts = app
        .with_db(|db| {
            db.list_accounts(&AccountFilter {
                platform: Some("workbuddy".to_string()),
                ..Default::default()
            })
        })
        .unwrap_or_default();
    let states = app.with_db(|db| db.travel_states()).unwrap_or_default();
    let by_id: std::collections::HashMap<&str, &crate::storage::travel::TravelState> =
        states.iter().map(|s| (s.account_id.as_str(), s)).collect();

    let rows: Vec<Value> = accounts
        .iter()
        .map(|account| {
            let state = by_id.get(account.id.as_str()).map(|s| (*s).clone());
            json!({
                "accountId": account.id,
                "uid": account.uid,
                "nickname": account.nickname,
                "authStatus": account.auth_status,
                "authSource": account.auth_source,
                "travelEnabled": account.travel_enabled,
                "tokenExpiresAt": account.token_expires_at,
                "travel": state,
            })
        })
        .collect();

    json!({
        "today": crate::clock::beijing_date(),
        "auto": travel_scheduler::auto_status(app),
        "accounts": rows,
    })
}

/// 一次手动同步的意图。三个动作互不越界，避免「点刷新却把奖励领了」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SyncIntent {
    /// 只查状态，不派出、不领取。
    Observe,
    /// 派出（是否真的能派由服务端每日上限决定）。
    Depart,
    /// 领取（只有 arrived 会真的提交）。
    Claim,
}

impl SyncIntent {
    fn flags(self) -> (bool, bool) {
        match self {
            Self::Observe => (false, false),
            Self::Depart => (true, false),
            Self::Claim => (false, true),
        }
    }
}

/// 单账号执行一次（查询 / 派出 / 领取）。
///
/// 返回值只含快照与事件，**不含令牌**；事件文案在写库前已经过 `sanitize`。
async fn travel_sync(app: &Arc<App>, account_id: &str, intent: SyncIntent) -> RpcResult<Value> {
    let (allow_depart, allow_claim) = intent.flags();
    let outcome =
        travel_scheduler::sync_one(app, account_id, allow_depart, allow_claim, true).await?;
    let events: Vec<Value> = outcome
        .events
        .iter()
        .map(|event: &EventDraft| {
            json!({
                "kind": event.kind,
                "status": event.status,
                "message": event.message,
                "credits": event.credits,
            })
        })
        .collect();
    Ok(json!({
        "accountId": account_id,
        "travel": outcome.state,
        "events": events,
        "auto": travel_scheduler::auto_status(app),
    }))
}

fn require_bool(params: &Value, key: &str) -> RpcResult<bool> {
    params
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| RpcError::invalid_params(format!("缺少布尔参数 {key}")))
}

fn require_str(params: &Value, key: &str) -> RpcResult<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("缺少参数 {key}")))
}

#[cfg(test)]
mod tests {
    //! 凭据归属的回归测试。
    //!
    //! 这些路径以前只靠"记得别写错"，所以出了问题（扫码后来源还是 magpie、
    //! 删除时把别人的令牌清掉、钥匙串失败留下半残账号）。现在用内存后端把
    //! 每一条钉住，不需要网络也不需要真钥匙串。

    use super::*;
    use crate::paths::Paths;
    use crate::providers::workbuddy_auth::{Profile, TokenPayload};
    use crate::securestore::SecureStore;

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

    fn payload(uid: &str, access: &str, refresh: Option<&str>) -> TokenPayload {
        TokenPayload {
            access_token: access.to_string(),
            refresh_token: refresh.map(str::to_string),
            domain: "www.workbuddy.cn".to_string(),
            expires_at_ms: Some(1_795_354_934_547),
            profile: Profile {
                uid: uid.to_string(),
                nickname: Some("测试账号".to_string()),
                email: None,
            },
        }
    }

    fn seed_magpie_account(app: &App, uid: &str) -> crate::storage::AccountRecord {
        app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: uid.to_string(),
                nickname: Some("借来的".to_string()),
                auth_status: Some("valid".to_string()),
                auth_source: Some(magpie::SOURCE_MAGPIE.to_string()),
                ..Default::default()
            })
        })
        .expect("seed")
    }

    fn read(app: &App, uid: &str) -> Option<String> {
        let (access_key, _) = workbuddy_auth::secret_keys(uid);
        app.secure
            .get(&access_key)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
    }

    #[test]
    fn oauth_login_flips_magpie_account_to_own() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-1111-2222-3333-444455556666";
        let seeded = seed_magpie_account(&app, uid);
        assert_eq!(seeded.auth_source, magpie::SOURCE_MAGPIE);

        let result = persist_login(&app, &payload(uid, "fresh-access", Some("fresh-refresh")))
            .expect("persist");
        assert_eq!(result["state"], json!("success"));
        assert_eq!(
            result["reused"],
            json!(true),
            "同一个 uid 应该是复用而不是新建"
        );

        let record = app
            .with_db(|db| db.get_account(&seeded.id))
            .unwrap()
            .expect("account");
        assert_eq!(
            record.auth_source, OWN_SOURCE,
            "扫码之后来源必须变成 own，否则 401 会去 Magpie 拿旧令牌覆盖它"
        );
        assert_eq!(read(&app, uid).as_deref(), Some("fresh-access"));

        let (_, refresh_key) = workbuddy_auth::secret_keys(uid);
        let refresh = app.secure.get(&refresh_key).expect("refresh 应该写进去");
        assert_eq!(String::from_utf8_lossy(&refresh), "fresh-refresh");
    }

    #[test]
    fn oauth_login_without_refresh_drops_stale_refresh() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-aaaa-bbbb-cccc-ddddeeeeffff";
        persist_login(&app, &payload(uid, "first-access", Some("first-refresh"))).expect("first");

        // 第二次登录没发刷新令牌：旧的必须删掉，不能留下"新 access + 旧 refresh"。
        persist_login(&app, &payload(uid, "second-access", None)).expect("second");

        let (_, refresh_key) = workbuddy_auth::secret_keys(uid);
        assert!(
            app.secure.get(&refresh_key).is_err(),
            "没有新 refresh 时必须清掉旧的"
        );
        assert_eq!(read(&app, uid).as_deref(), Some("second-access"));
    }

    #[test]
    fn keychain_failure_leaves_no_half_account() {
        let (app, _dir) = test_app(SecureStore::failing());
        let uid = "6f1c2d94-9999-8888-7777-666655554444";

        let err =
            persist_login(&app, &payload(uid, "access", Some("refresh"))).expect_err("must fail");
        assert_eq!(err.code, crate::rpc::E_SECURESTORE);

        assert!(
            app.with_db(|db| db.find_account("workbuddy", uid))
                .unwrap()
                .is_none(),
            "钥匙串写不进去就不能留下账号行"
        );
    }

    #[test]
    fn resync_refuses_an_own_account() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-1234-5678-90ab-cdef12345678";
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("persist");
        let record = app
            .with_db(|db| db.find_account("workbuddy", uid))
            .unwrap()
            .expect("account");

        let err = resync_magpie_access(&app, &record).expect_err("own 账号必须被拒绝");
        assert_eq!(
            err.data.as_ref().unwrap()["kind"],
            json!("not_magpie_source")
        );
        assert_eq!(
            read(&app, uid).as_deref(),
            Some("own-access"),
            "拒绝之后凭据不能被改"
        );
    }

    #[test]
    fn purge_skips_when_another_row_shares_the_uid() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-0000-1111-2222-333344445555";
        let seeded = seed_magpie_account(&app, uid);
        app.secure
            .set(&workbuddy_auth::secret_keys(uid).0, b"still-in-use")
            .expect("seed secret");

        // 伪造一条"同 uid、不同 id"的账号行：这是将来一账号多授权时才可能出现的形态。
        let mut other = seeded.clone();
        other.id = "another-row".to_string();
        other.nickname = "另一行".to_string();
        purge_workbuddy_secrets(&app, &other);

        assert_eq!(
            read(&app, uid).as_deref(),
            Some("still-in-use"),
            "还有别的账号行用着这套 key 时不能清"
        );

        // 删掉真正的账号行之后，清理才会执行。
        app.with_db(|db| db.delete_account(&seeded.id)).unwrap();
        purge_workbuddy_secrets(&app, &seeded);
        assert!(read(&app, uid).is_none(), "没有账号行了就该清干净");
    }
}
