//! JSON-RPC 方法表。
//!
//! 命名约定：`<域>.<动作>`。PR1a 只覆盖本地能力，任何联网行为都不在这里。

use serde_json::{json, Value};

use std::sync::Arc;

use crate::app::App;
use crate::providers::{magpie, qoder, workbuddy, workbuddy_auth};
use crate::rpc::{RpcError, RpcResult};
use crate::securestore::SecretErrorKind;
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
    import_logins(app, logins, keys)
}

/// 真正的导入循环。把"读文件"和"搬账号"拆开，测试才能直接喂一份登录列表，
/// 不用去改 `MAGPIE_CONFIG_DIR`（那玩意是环境变量，并行测试里会互相打架）。
fn import_logins(app: &App, logins: Vec<magpie::MagpieLogin>, keys: &[String]) -> RpcResult<Value> {
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
        // 写之前先拍快照：后面任何一步失败都要把钥匙串恢复原样，
        // 否则会留下"令牌换了、账号状态没换"的不一致（或覆盖掉原有凭据）。
        let snapshot = CredentialSnapshot::capture(app, &login.uid);
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
        // 删不掉就回滚并跳过：留着旧的 refresh 等于让下次刷新拿一个死令牌去撞。
        match app.secure.delete(&refresh_key) {
            Ok(_) => {}
            Err(err) if err.kind == SecretErrorKind::NotFound => {}
            Err(err) => {
                snapshot.restore(app);
                skipped.push(json!({
                    "key": login.key,
                    "uid": login.uid,
                    "reason": "refresh_not_cleared",
                    "message": crate::rpc::sanitize(&err.message),
                }));
                continue;
            }
        }

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
            Err(err) => {
                // 库里没记住 = 这次导入没有发生。钥匙串也回到导入前，
                // 不能出现"有令牌、账号却还是旧状态"的组合。
                snapshot.restore(app);
                skipped.push(json!({
                    "key": login.key,
                    "uid": login.uid,
                    "reason": "storage",
                    "message": crate::rpc::sanitize(&err.message),
                }));
            }
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
/// 三条规矩，缺一条都可能删掉别人还在用的令牌：
/// 1. 先确认「这套 key 没有别的账号行在用」；
/// 2. 这个确认**查不出来时不能删**（数据库坏了就保守留着，宁可留垃圾也不误删）；
/// 3. 删除本身失败要留日志，不能 `let _ =` 一把梭。
fn purge_workbuddy_secrets(app: &App, account: &crate::storage::AccountRecord) {
    match app.with_db(|db| db.find_account("workbuddy", &account.uid)) {
        Ok(Some(other)) if other.id != account.id => {
            log_note(&format!(
                "跳过清理 {}：同一 uid 还有其它账号行在用它",
                account.uid
            ));
            return;
        }
        Ok(_) => {}
        Err(err) => {
            // 归属确认失败 = 不知道有没有别人在用 → 不删。
            log_note(&format!(
                "跳过清理 {}：无法确认凭据归属（{}）",
                account.uid, err.message
            ));
            return;
        }
    }
    let (access, refresh) = workbuddy_auth::secret_keys(&account.uid);
    for key in [access, refresh] {
        if let Err(err) = app.secure.delete(&key) {
            if err.kind != SecretErrorKind::NotFound {
                log_note(&format!("清理凭据失败 {key}：{}", err.message));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// 凭据写入：先快照，失败就回滚
// ---------------------------------------------------------------------------

/// 写凭据之前的快照。
///
/// 钥匙串没有事务：先写 access 再写 refresh，中间失败就会留下"新 access + 旧 refresh"，
/// 用户原来能用的那套也被覆盖掉了。这里在写之前把两个键原样读一遍，任何一步失败
/// 都拿它恢复 —— 宁可这次登录失败，也不能把还能用的凭据弄坏。
struct CredentialSnapshot {
    access_key: String,
    access: Option<Vec<u8>>,
    refresh_key: String,
    refresh: Option<Vec<u8>>,
}

impl CredentialSnapshot {
    fn capture(app: &App, uid: &str) -> Self {
        let (access_key, refresh_key) = workbuddy_auth::secret_keys(uid);
        Self {
            access: app.secure.get(&access_key).ok(),
            refresh: app.secure.get(&refresh_key).ok(),
            access_key,
            refresh_key,
        }
    }

    /// 恢复到快照状态。返回 false 表示没恢复干净（旧的也写不回去了），必须留痕。
    fn restore(&self, app: &App) -> bool {
        let access_ok = restore_one(app, &self.access_key, self.access.as_deref());
        let refresh_ok = restore_one(app, &self.refresh_key, self.refresh.as_deref());
        if !(access_ok && refresh_ok) {
            log_note("凭据回滚没有完全成功：钥匙串里可能留有半套凭据，请重新登录");
        }
        access_ok && refresh_ok
    }
}

fn restore_one(app: &App, key: &str, old: Option<&[u8]>) -> bool {
    match old {
        Some(bytes) => app.secure.set(key, bytes).is_ok(),
        None => match app.secure.delete(key) {
            Ok(_) => true,
            Err(err) => err.kind == SecretErrorKind::NotFound,
        },
    }
}

fn credential_error(err: crate::securestore::SecretError, stage: &str) -> RpcError {
    RpcError::new(
        crate::rpc::E_SECURESTORE,
        format!("写入凭据失败（{stage}）：{}", err.message),
    )
    .with_data(json!({ "kind": "credential_write_failed", "stage": stage }))
}

/// 写 AICC **自有**授权（扫码拿到的）。
///
/// 顺序仍是「先 access、再 refresh」：这条顺序正是历史上出问题的路径，现在由快照兜底，
/// 而不是靠换个顺序碰运气。返回的快照交给调用方，数据库写失败时还要用它回滚。
///
/// `refresh = None` 表示这次登录没发刷新令牌，此时必须把旧的删掉：留着会让
/// "新 access + 旧 refresh" 混搭，刷新必然失败。删不掉就整体失败并回滚 ——
/// 静默忽略等于把这个坑留到下一次刷新。
fn store_own_credentials(
    app: &App,
    uid: &str,
    access: &str,
    refresh: Option<&str>,
) -> RpcResult<CredentialSnapshot> {
    let snapshot = CredentialSnapshot::capture(app, uid);

    if let Err(err) = app.secure.set(&snapshot.access_key, access.as_bytes()) {
        snapshot.restore(app);
        return Err(credential_error(err, "access"));
    }

    match refresh {
        Some(token) => {
            if let Err(err) = app.secure.set(&snapshot.refresh_key, token.as_bytes()) {
                snapshot.restore(app);
                return Err(credential_error(err, "refresh"));
            }
        }
        None => match app.secure.delete(&snapshot.refresh_key) {
            Ok(_) => {}
            Err(err) if err.kind == SecretErrorKind::NotFound => {}
            Err(err) => {
                snapshot.restore(app);
                return Err(RpcError::new(
                    crate::rpc::E_SECURESTORE,
                    format!("旧的刷新令牌删不掉（{}），本次登录已回滚", err.message),
                )
                .with_data(json!({ "kind": "stale_refresh_not_cleared" })));
            }
        },
    }
    Ok(snapshot)
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
///
/// 回滚策略：入库失败时**恢复登录前的钥匙串**，而不是清空。老的实现直接
/// `purge_workbuddy_secrets`，会把用户原本有效的那套凭据一起删掉。
fn persist_login(app: &App, payload: &workbuddy_auth::TokenPayload) -> RpcResult<Value> {
    let snapshot = store_own_credentials(
        app,
        &payload.profile.uid,
        &payload.access_token,
        payload.refresh_token.as_deref(),
    )?;

    match write_login_row(app, payload) {
        Ok(value) => Ok(value),
        Err(err) => {
            snapshot.restore(app);
            Err(err)
        }
    }
}

/// 落库部分单独拆出来：方便用"一定失败"的写入器验证回滚，不必真把数据库弄坏。
fn write_login_row(app: &App, payload: &workbuddy_auth::TokenPayload) -> RpcResult<Value> {
    let uid = payload.profile.uid.clone();
    let (access_key, _) = workbuddy_auth::secret_keys(&uid);
    let expires_at = payload
        .expires_at_ms
        .and_then(workbuddy_auth::ms_to_rfc3339);

    app.with_db(|db| {
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
        Ok(json!({
            "state": "success",
            "reused": existed,
            "account": record,
        }))
    })
}

/// 落盘服务端刚轮换出来的令牌。
///
/// 关键点：**服务端轮换成功之后，旧的 refresh 就已经作废**。所以这里不能像登录那样
/// "失败就恢复快照"——那会把一个死令牌写回去，让账号看起来还能刷新。
/// 正确做法：尽量保住新的 access；refresh 落不下去就标失效、把死令牌删掉，
/// 明确告诉用户要重新登录。
fn persist_rotated_tokens(
    app: &App,
    id: &str,
    access_key: &str,
    refresh_key: &str,
    access: &str,
    refresh: Option<&str>,
    expires: Option<&str>,
) -> RpcResult<Value> {
    // 两个键**分别**尝试：access 写不进去时，新的 refresh 仍然是有效令牌，
    // 留住它，下一次刷新还能把账号救回来（写死"先 access 后 refresh 一起失败"会把这份也丢掉）。
    let access_saved = app.secure.set(access_key, access.as_bytes());
    let refresh_saved = match refresh {
        Some(next) => app.secure.set(refresh_key, next.as_bytes()),
        None => Ok(()), // 服务端没轮换 refresh，旧的那份继续有效
    };

    if access_saved.is_ok() && refresh_saved.is_ok() {
        app.with_db(|db| db.set_auth_state(id, "valid", expires))?;
        return Ok(json!({ "state": "valid", "tokenExpiresAt": expires }));
    }

    // 只有「服务端确实给了新 refresh、而这个新 refresh 没写进去」才说明旧 refresh 已经作废，
    // 这时必须把它清掉；否则旧 refresh 可能还能用，删了反而丢掉恢复能力。
    let rotated = refresh.is_some();
    let dead_refresh = rotated && refresh_saved.is_err();
    let cleared = if dead_refresh {
        clear_dead_refresh(app, refresh_key)
    } else {
        true
    };

    app.with_db(|db| db.set_auth_state(id, "invalid", None))?;
    let reason = if access_saved.is_err() {
        "新的访问令牌没能写进钥匙串"
    } else {
        "新的刷新令牌没能写进钥匙串"
    };
    Ok(json!({
        "state": "invalid",
        "needsRelogin": true,
        "error": {
            "kind": "refresh_not_persisted",
            "message": if dead_refresh && !cleared {
                format!("{reason}；旧刷新令牌已被服务端作废且未能清除，请重新登录")
            } else {
                format!("{reason}，请重新登录")
            },
        },
    }))
}

/// 删掉一个已经被服务端作废的 refresh。删不掉要留日志（返回值给调用方拼文案）。
fn clear_dead_refresh(app: &App, key: &str) -> bool {
    match app.secure.delete(key) {
        Ok(_) => true,
        Err(err) if err.kind == SecretErrorKind::NotFound => true,
        Err(err) => {
            log_note(&format!("旧刷新令牌清除失败（{key}）：{}", err.message));
            false
        }
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
            let expires = expires_at_ms.and_then(workbuddy_auth::ms_to_rfc3339);
            persist_rotated_tokens(
                app,
                id,
                &access_key,
                &refresh_key,
                &access_token,
                refresh_token.as_deref(),
                expires.as_deref(),
            )
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

    // -----------------------------------------------------------------------
    // 失败注入：写一半、库里失败、删除失败，都不能把原来能用的东西弄坏
    // -----------------------------------------------------------------------

    fn read_refresh(app: &App, uid: &str) -> Option<String> {
        let (_, refresh_key) = workbuddy_auth::secret_keys(uid);
        app.secure
            .get(&refresh_key)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
    }

    fn seed_pair(app: &App, uid: &str, access: &str, refresh: &str) {
        let (access_key, refresh_key) = workbuddy_auth::secret_keys(uid);
        app.secure.set(&access_key, access.as_bytes()).unwrap();
        app.secure.set(&refresh_key, refresh.as_bytes()).unwrap();
    }

    fn magpie_login(uid: &str, access: &str) -> magpie::MagpieLogin {
        magpie::MagpieLogin {
            key: format!("workbuddy#{uid}"),
            uid: uid.to_string(),
            nickname: Some("借来的".to_string()),
            domain: "www.workbuddy.cn".to_string(),
            access_token: access.to_string(),
            expires_at_ms: Some(1_795_525_945_623),
        }
    }

    /// access 写进去了、refresh 写失败：必须回到原来那套，不能被"半套新凭据"顶掉。
    #[test]
    fn refresh_write_failure_keeps_the_old_pair() {
        let (app, _dir) = test_app(SecureStore::faulty(&["/refresh"], &[]));
        let uid = "6f1c2d94-aaaa-0001-0002-000000000001";
        seed_pair(&app, uid, "old-access", "old-refresh");
        app.secure.arm();

        let err = persist_login(&app, &payload(uid, "new-access", Some("new-refresh")))
            .expect_err("refresh 写不进去就该失败");
        assert_eq!(err.data.as_ref().unwrap()["stage"], json!("refresh"));

        assert_eq!(read(&app, uid).as_deref(), Some("old-access"));
        assert_eq!(read_refresh(&app, uid).as_deref(), Some("old-refresh"));
        assert!(
            app.with_db(|db| db.find_account("workbuddy", uid))
                .unwrap()
                .is_none(),
            "登录没成功就不该建出账号行"
        );
    }

    /// 连 access 都写不进去时同样要回滚（这条路径原来是"写一半就停"）。
    #[test]
    fn access_write_failure_keeps_the_old_pair() {
        let (app, _dir) = test_app(SecureStore::faulty(&["/access"], &[]));
        let uid = "6f1c2d94-aaaa-0001-0002-000000000002";
        seed_pair(&app, uid, "old-access", "old-refresh");
        app.secure.arm();

        let err = persist_login(&app, &payload(uid, "new-access", Some("new-refresh")))
            .expect_err("access 写不进去就该失败");
        assert_eq!(err.data.as_ref().unwrap()["stage"], json!("access"));
        assert_eq!(read(&app, uid).as_deref(), Some("old-access"));
        assert_eq!(read_refresh(&app, uid).as_deref(), Some("old-refresh"));
    }

    /// 登录没发新 refresh、旧的又删不掉：整体失败并回滚，不能留下"新 access + 旧 refresh"。
    #[test]
    fn stale_refresh_cleanup_failure_aborts_the_login() {
        let (app, _dir) = test_app(SecureStore::faulty(&[], &["/refresh"]));
        let uid = "6f1c2d94-aaaa-0001-0002-000000000003";
        seed_pair(&app, uid, "old-access", "old-refresh");
        app.secure.arm();

        let err = persist_login(&app, &payload(uid, "new-access", None))
            .expect_err("旧的 refresh 删不掉就该失败");
        assert_eq!(
            err.data.as_ref().unwrap()["kind"],
            json!("stale_refresh_not_cleared")
        );
        assert_eq!(read(&app, uid).as_deref(), Some("old-access"));
        assert_eq!(read_refresh(&app, uid).as_deref(), Some("old-refresh"));
    }

    /// 入库失败：恢复登录前的凭据，**不能**把老账号的令牌删掉。
    #[test]
    fn db_failure_keeps_previous_credentials() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-aaaa-0001-0002-000000000004";
        let seeded = seed_magpie_account(&app, uid);
        let (access_key, _) = workbuddy_auth::secret_keys(uid);
        app.secure.set(&access_key, b"borrowed-access").unwrap();

        // 让所有 UPDATE 都失败：模拟"令牌写好了、库里却写不进去"。
        app.with_db(|db| {
            db.exec_raw(
                "CREATE TRIGGER no_update BEFORE UPDATE ON accounts
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            )
        })
        .expect("install trigger");

        let err = persist_login(&app, &payload(uid, "own-access", Some("own-refresh")))
            .expect_err("库里写不进去就该失败");
        assert_eq!(err.code, crate::rpc::E_STORAGE);

        assert_eq!(
            read(&app, uid).as_deref(),
            Some("borrowed-access"),
            "原来那套凭据必须原样保留"
        );
        assert!(read_refresh(&app, uid).is_none(), "新的 refresh 不能留下");
        let record = app
            .with_db(|db| db.get_account(&seeded.id))
            .unwrap()
            .unwrap();
        assert_eq!(record.auth_source, "magpie", "账号状态也不能变");
    }

    /// 服务端已经轮换、refresh 却写不进去：新 access 尽力保住，死 refresh 必须删掉，
    /// 而且**不能**把旧 refresh 恢复回去假装还能刷新。
    #[test]
    fn rotation_failure_marks_invalid_and_drops_the_dead_refresh() {
        let (app, _dir) = test_app(SecureStore::faulty(&["/refresh"], &[]));
        let uid = "6f1c2d94-aaaa-0001-0002-000000000005";
        app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: uid.to_string(),
                auth_status: Some("valid".to_string()),
                auth_source: Some(OWN_SOURCE.to_string()),
                ..Default::default()
            })
        })
        .expect("seed account");
        seed_pair(&app, uid, "old-access", "dead-refresh");
        app.secure.arm();

        let (access_key, refresh_key) = workbuddy_auth::secret_keys(uid);
        let id = crate::storage::account_id("workbuddy", uid);
        let out = persist_rotated_tokens(
            &app,
            &id,
            &access_key,
            &refresh_key,
            "new-access",
            Some("new-refresh"),
            None,
        )
        .expect("不应该抛错，而是返回 invalid");

        assert_eq!(out["state"], json!("invalid"));
        assert_eq!(out["needsRelogin"], json!(true));
        assert_eq!(out["error"]["kind"], json!("refresh_not_persisted"));
        assert_eq!(
            read(&app, uid).as_deref(),
            Some("new-access"),
            "新 access 能写就写，别把还能用的丢掉"
        );
        assert!(
            read_refresh(&app, uid).is_none(),
            "被服务端作废的旧 refresh 必须清掉，不能恢复成「看起来还能刷新」"
        );
        let record = app
            .with_db(|db| db.find_account("workbuddy", uid))
            .unwrap()
            .unwrap();
        assert_eq!(record.auth_status, "invalid");
    }

    /// 连新 access 都写不进去时同样标失效，并且把死 refresh 清掉。
    #[test]
    fn rotation_access_failure_also_invalidates() {
        let (app, _dir) = test_app(SecureStore::faulty(&["/access"], &[]));
        let uid = "6f1c2d94-aaaa-0001-0002-000000000006";
        app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: uid.to_string(),
                auth_status: Some("valid".to_string()),
                auth_source: Some(OWN_SOURCE.to_string()),
                ..Default::default()
            })
        })
        .expect("seed account");
        seed_pair(&app, uid, "old-access", "dead-refresh");
        app.secure.arm();

        let (access_key, refresh_key) = workbuddy_auth::secret_keys(uid);
        let id = crate::storage::account_id("workbuddy", uid);
        let out = persist_rotated_tokens(
            &app,
            &id,
            &access_key,
            &refresh_key,
            "new-access",
            Some("new-refresh"),
            None,
        )
        .expect("返回 invalid 而不是抛错");

        assert_eq!(out["state"], json!("invalid"));
        assert_eq!(out["needsRelogin"], json!(true));
        assert_eq!(
            read(&app, uid).as_deref(),
            Some("old-access"),
            "access 没写进去，旧值原样留着"
        );
        assert_eq!(
            read_refresh(&app, uid).as_deref(),
            Some("new-refresh"),
            "新的 refresh 是有效令牌，必须留住，下次刷新才有救"
        );
    }

    /// 导入时钥匙串写成功、入库失败：钥匙串必须回滚，不能留下"有令牌、没账号"。
    #[test]
    fn import_db_failure_rolls_back_the_keychain() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-bbbb-0001-0002-000000000001";
        app.with_db(|db| {
            db.exec_raw(
                "CREATE TRIGGER no_insert BEFORE INSERT ON accounts
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            )
        })
        .expect("install trigger");

        let out = import_logins(&app, vec![magpie_login(uid, "magpie-access")], &[]).unwrap();
        assert_eq!(out["imported"], json!(0), "{out}");
        assert_eq!(out["skipped"][0]["reason"], json!("storage"), "{out}");
        assert!(read(&app, uid).is_none(), "入库失败就不能留令牌");
    }

    /// 借用的账号必须删掉遗留 refresh；删不掉就回滚并跳过，不静默。
    #[test]
    fn import_refresh_delete_failure_skips_and_rolls_back() {
        let (app, _dir) = test_app(SecureStore::faulty(&[], &["/refresh"]));
        let uid = "6f1c2d94-bbbb-0001-0002-000000000002";
        app.secure.arm();

        let out = import_logins(&app, vec![magpie_login(uid, "magpie-access")], &[]).unwrap();
        assert_eq!(out["imported"], json!(0), "{out}");
        assert_eq!(
            out["skipped"][0]["reason"],
            json!("refresh_not_cleared"),
            "{out}"
        );
        assert!(read(&app, uid).is_none(), "回滚后不能留令牌");
    }

    /// 归属查询失败（数据库坏了）时必须保守：不删。
    #[test]
    fn purge_keeps_credentials_when_ownership_cannot_be_checked() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-cccc-0001-0002-000000000001";
        let seeded = seed_magpie_account(&app, uid);
        let (access_key, _) = workbuddy_auth::secret_keys(uid);
        app.secure.set(&access_key, b"still-in-use").unwrap();

        // 把表删掉：`find_account` 直接报错，模拟"查不出归属"。
        app.with_db(|db| db.exec_raw("DROP TABLE accounts"))
            .expect("drop table");

        purge_workbuddy_secrets(&app, &seeded);
        assert_eq!(
            read(&app, uid).as_deref(),
            Some("still-in-use"),
            "查不出归属就不能删"
        );
    }
}
