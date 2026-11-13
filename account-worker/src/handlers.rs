//! JSON-RPC 方法表。
//!
//! 命名约定：`<域>.<动作>`。PR1a 只覆盖本地能力，任何联网行为都不在这里。

use serde_json::{json, Value};

use std::sync::{Arc, Mutex, MutexGuard};

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
    "settings.get",
    "settings.set",
    "auth.authorization.delete",
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
    // ---- 成长中心（PR #67：只读）----
    "growth.scan",
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

        "accounts.list" => {
            let filter: AccountFilter = serde_json::from_value(params.clone())
                .map_err(|e| RpcError::invalid_params(format!("筛选参数非法：{e}")))?;
            let accounts = app.with_db(|db| db.list_accounts(&filter))?;
            Ok(json!({ "accounts": with_authorizations(app, accounts)? }))
        }

        "accounts.get" => {
            let id = require_str(params, "id")?;
            let record = app
                .with_db(|db| db.get_account(&id))?
                .ok_or_else(|| RpcError::not_found(format!("账号不存在：{id}")))?;
            let mut value = json!(record);
            value["authorizations"] = json!(crate::auth::broker::describe(app, &id)?);
            Ok(value)
        }

        "accounts.upsert" => {
            let input: AccountInput = serde_json::from_value(params.clone())
                .map_err(|e| RpcError::invalid_params(format!("账号参数非法：{e}")))?;
            // 授权相关字段不接受调用方直写：`auth_status` / `auth_source` 是派生镜像（G6），
            // 谁都能改的话，"来源与凭据不一致"就又回来了。改了就被静默忽略。
            let input = crate::auth::service::account_input_without_auth(&input);
            let record = app.with_db(|db| db.upsert_account(&input))?;
            // 账号上没有授权行时，这个调用是空操作；有的话顺带刷新一次镜像。
            let _ = crate::auth::service::sync_mirror(app, &record.id);
            crate::auth::service::account_view(app, &record)
        }

        "accounts.delete" => {
            let id = require_str(params, "id")?;
            // 与导入 / resync / refresh 串行：否则删除刚完成，在途的 resync 又会把令牌写回去。
            let _guard = auth_write_guard();
            let existing = app.with_db(|db| db.get_account(&id))?;
            let deleted = app.with_db(|db| db.delete_account(&id))?;
            // 账号删掉后令牌没有主人了，必须一起清，避免钥匙串里留孤儿凭据。
            // 五条键：own/access、own/refresh、magpie/access 与两条旧别名。
            let mut failed_keys: Vec<String> = Vec::new();
            if deleted {
                if let Some(account) = existing {
                    failed_keys = crate::auth::service::purge_account_keys(app, &account);
                }
            }
            Ok(json!({
                "deleted": deleted,
                // 删不掉的键要可见：宁可界面报一条，也不能让凭据在钥匙串里变孤儿。
                "failedKeys": failed_keys,
                "orphanCredentials": !failed_keys.is_empty(),
            }))
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
                    let _guard = auth_write_guard();
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
                other => return Err(RpcError::invalid_params(format!("未知设置项：{other}"))),
            }
            Ok(json!({ "key": key, "value": value, "mirrorErrors": mirror_errors }))
        }

        // ---- 单份授权删除（M2）----
        //
        // 只删这一份的键与记录：不删账号、不碰另一份、不动签到与旅行历史。
        "auth.authorization.delete" => {
            let account_id = require_str(params, "accountId")?;
            let kind = require_str(params, "kind")?;
            let _guard = auth_write_guard();
            if app.with_db(|db| db.get_account(&account_id))?.is_none() {
                return Err(RpcError::not_found(format!("账号不存在：{account_id}")));
            }
            let report = crate::auth::service::delete_authorization(app, &account_id, &kind)?;
            if !report.failed_keys.is_empty() {
                return Err(RpcError::new(
                    crate::rpc::E_SECURESTORE,
                    "授权记录已保留，但有凭据未能从钥匙串删除，可重试",
                )
                .with_data(json!({
                    "kind": "credential_delete_failed",
                    "failedKeys": report.failed_keys,
                })));
            }
            Ok(json!({
                "accountId": account_id,
                "kind": kind,
                "deleted": report.record_deleted,
                "removedKeys": report.removed_keys.len(),
                // 镜像没同步上不影响删除本身，但必须让调用方看见。
                "mirrorError": report.mirror_error,
            }))
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

        // ---- 成长中心（PR #67：只读）----
        //
        // 一次调用扫完（或扫指定）账号，返回每个账号的任务、积分、能量、连签
        // 与旅行状态。**这里没有任何写接口**：接受任务、领奖在 #68，
        // 每个都会有自己的开关与验收。
        //
        // 与签到 / 旅行共用同一把凭据锁：扫描虽然只读，但读的是同一份 access，
        // 不能与「正在轮换令牌」的写入撞在一起。
        "growth.scan" => {
            let ids: Vec<String> = params
                .get("accountIds")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let scans = crate::growth::scan_fleet(app, &ids).await;
            let overview = crate::growth::overview(&scans);
            // 扫描日志：只记结论，不记令牌与任务正文。
            let summary: Vec<String> = scans
                .iter()
                .map(|scan| {
                    format!(
                        "{}={}(tasks {}, claimable {}, errors {})",
                        scan.account_id,
                        scan.state,
                        scan.counts.total,
                        scan.counts.claimable,
                        scan.errors.len()
                    )
                })
                .collect();
            log_note(&format!("成长中心扫描：{}", summary.join("; ")));
            Ok(json!({
                "scans": scans,
                "overview": overview,
                "scannedAt": crate::clock::now_rfc3339(),
                "readOnly": true,
            }))
        }

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

/// 凭据写入的进程内互斥。
///
/// 会改凭据的路径不止一条：扫码落库、Magpie 导入、401 重同步、刷新轮换、删除账号。
/// worker 是单进程，一把锁就够保证「同一时刻只有一个写入者」，配合 `auth_epoch`
/// 的版本比对，即便写入者拿的是几秒前的账号快照，也不会覆盖掉别人刚写好的授权。
static AUTH_WRITE_LOCK: Mutex<()> = Mutex::new(());

fn auth_write_guard() -> MutexGuard<'static, ()> {
    // 上一个持锁者 panic 过也继续：这把锁保护的是"别同时写"，不是被保护的数据本身。
    AUTH_WRITE_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

/// 快照已经过期（账号换了授权 / 其间被别的路径写过）。**不是错误，是拒绝写入**。
fn superseded(reason: &str) -> RpcError {
    RpcError::new(
        crate::rpc::E_AUTH,
        "账号授权已变更，已放弃这次基于旧快照的写入",
    )
    .with_data(json!({ "kind": "superseded", "reason": reason }))
}

/// 旧快照冲突的两种表现，调用方按同一种情况处理：
/// - `superseded`：来源没变、但账号被别人写过（版本号对不上）；
/// - `not_magpie_source`：来源已经变成自有授权，就更不该再用 Magpie 的令牌。
///
/// 旅行侧尤其要认这两种：它们不代表失败，只代表"我手上这份账号已经过时"，
/// 不能因此把账号打进 6 小时冷却。
pub(crate) fn is_stale_snapshot(err: &RpcError) -> bool {
    matches!(
        err_kind(err),
        Some("superseded") | Some("not_magpie_source")
    )
}

fn err_kind(err: &RpcError) -> Option<&str> {
    err.data
        .as_ref()
        .and_then(|data| data.get("kind"))
        .and_then(Value::as_str)
}

pub(crate) fn log_note(message: &str) {
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
    health["credentialMigration"] = crate::auth::migrate::health(app);
    health["authorizations"] = authorization_fleet(app);
    health["providers"] = json!({ "workbuddy": workbuddy, "qoder": qoder });
    health["methods"] = json!(METHODS);
    health
}

/// 账号列表带上每份授权（不含令牌）。查询在 `with_db` 外面做，避免嵌套锁。
fn with_authorizations(
    app: &App,
    accounts: Vec<crate::storage::AccountRecord>,
) -> RpcResult<Vec<Value>> {
    let mut out = Vec::with_capacity(accounts.len());
    for account in accounts {
        let mut value = json!(account);
        value["authorizations"] = json!(crate::auth::broker::describe(app, &account.id)?);
        out.push(value);
    }
    Ok(out)
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
    json!({ "usable": usable, "rows": rows, "preferred": crate::auth::broker::preferred_source(app) })
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

/// 用当前选用的授权查一次真实登录态（只读）。
///
/// 两步走，顺序不能反：
/// 1. **先探测**（`auth::probe`）：保留 HTTP 语义，只有明确的 401 才算"这份授权失效"，
///    才能换源。429 / 5xx / 超时 / 判不出来的 403 一律 `Transient`，不改状态。
/// 2. 探测通过后再查签到详情，给界面填 `todayCheckedIn` 之类的字段。
///
/// 旧实现直接看 `get_checkin_status().ok`：它把 401、429、超时全压成 `ok:false`，
/// 于是"网络抖一下"就会被写成 invalid，把一份还能用的授权标死。
async fn workbuddy_account_status(app: &App, id: &str) -> RpcResult<Value> {
    use crate::auth::{broker, probe, service};

    let account = app
        .with_db(|db| db.get_account(id))?
        .ok_or_else(|| RpcError::not_found(format!("账号不存在：{id}")))?;
    if account.platform != "workbuddy" {
        return Err(RpcError::invalid_params("目前只支持查询 WorkBuddy 账号"));
    }

    let mut resolved = match broker::resolve(app, &account)? {
        broker::Outcome::Resolved(resolved) => resolved,
        broker::Outcome::Unavailable { reason, detail } => {
            return Ok(json!({
                "state": "missing_credential",
                "error": { "kind": reason, "message": detail },
                "authorizationSummary": broker::summary(app, &account),
            }));
        }
    };

    let domain = account.auth_domain.clone().unwrap_or_default();
    let mut report = probe::probe(WbVariant::Cn, &resolved.access, &account.uid, &domain).await;

    // 只有明确的认证失败才动状态、才换源。
    if report.outcome == probe::Probe::Unauthorized {
        if let Some(row) = app.with_db(|db| db.get_authorization(&resolved.authorization_id))? {
            let _ = service::record_status(app, &row, "invalid", None, Some("unauthorized"));
        }
        if let Ok(broker::Outcome::Resolved(next)) = broker::resolve(app, &account) {
            if next.authorization_id != resolved.authorization_id {
                report = probe::probe(WbVariant::Cn, &next.access, &account.uid, &domain).await;
                resolved = next;
            }
        }
    }

    let probe_view = json!({
        "outcome": report.outcome.as_str(),
        "httpStatus": report.http_status,
        "businessCode": report.business_code,
    });

    if report.outcome == probe::Probe::Transient {
        return Ok(json!({
            "state": "transient",
            "authSource": resolved.kind,
            "authorizationId": resolved.authorization_id,
            "degraded": resolved.degraded,
            "probe": probe_view,
            "error": {
                "kind": "transient",
                "message": report.detail.unwrap_or_else(|| "暂时无法确认授权状态，请稍后再试".to_string()),
            },
        }));
    }

    if report.outcome == probe::Probe::Unauthorized {
        // 两份都不行：账号层面就是失效，但**不动** own / magpie 的状态以外的任何东西。
        return Ok(json!({
            "state": "invalid",
            "authSource": resolved.kind,
            "authorizationId": resolved.authorization_id,
            "probe": probe_view,
            "error": {
                "kind": "unauthorized",
                "message": "授权已失效，请重新扫码，或去 Magpie 登录",
            },
            "authorizationSummary": broker::summary(app, &account),
        }));
    }

    // 探测通过：把这份授权标成 valid（带版本校验，旧快照不覆盖新授权）。
    if let Some(row) = app.with_db(|db| db.get_authorization(&resolved.authorization_id))? {
        let _ = service::record_status(app, &row, "valid", None, None);
    }

    let payload = json!({
        "access_token": resolved.access,
        "uid": account.uid,
        "domain": domain,
        "variant": "cn",
    });
    let status = wb_switch_core::modules::checkin::get_checkin_status(&payload).await;
    let ok = status.get("ok").and_then(Value::as_bool).unwrap_or(false);

    Ok(json!({
        // 探测通过就是 valid：详情查不到只影响面板上的数字，不影响授权状态。
        "state": "valid",
        "authSource": resolved.kind,
        "authorizationId": resolved.authorization_id,
        "degraded": resolved.degraded,
        "probe": probe_view,
        "todayCheckedIn": status.get("todayCheckedIn").cloned().unwrap_or(Value::Null),
        "streakDays": status.get("raw").and_then(|r| r.get("streak_days")).cloned().unwrap_or(Value::Null),
        "dailyCredit": status.get("raw").and_then(|r| r.get("daily_credit")).cloned().unwrap_or(Value::Null),
        "todayCredit": status.get("raw").and_then(|r| r.get("today_credit")).cloned().unwrap_or(Value::Null),
        "statusUnsupported": status.get("statusUnsupported").cloned().unwrap_or(Value::Null),
        // 探测通过但详情查询失败：授权本身是好的，别把它写成 invalid。
        "detailError": if ok { Value::Null } else { status.get("error").cloned().unwrap_or(Value::Null) },
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

/// 测试钩子：在"来源检查之前"插入一次外部动作（比如扫码登录落库）。
///
/// 生产构建里是空函数，编译器直接内联掉；只有 `cfg(test)` 才存在那个静态槽位。
#[cfg(test)]
type ImportRaceHook = (String, Box<dyn FnOnce() + Send>);

#[cfg(test)]
static IMPORT_RACE_HOOK: Mutex<Option<ImportRaceHook>> = Mutex::new(None);

/// 按 uid 匹配：测试是并行跑的，钩子只能影响设它的那个用例。
#[cfg(test)]
fn run_import_race_hook(uid: &str) {
    let mut slot = IMPORT_RACE_HOOK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let matches = slot
        .as_ref()
        .map(|(target, _)| target == uid)
        .unwrap_or(false);
    if matches {
        if let Some((_, hook)) = slot.take() {
            hook();
        }
    }
}

#[cfg(not(test))]
fn run_import_race_hook(_uid: &str) {}

/// 真正的导入循环。把"读文件"和"搬账号"拆开，测试才能直接喂一份登录列表，
/// 不用去改 `MAGPIE_CONFIG_DIR`（那玩意是环境变量，并行测试里会互相打架）。
///
/// **导入只写 magpie 那一行**：已经扫码登录过的账号照样能导入，own 的凭据一个字节不动（G1）。
/// 旧实现会在 own 存在时整条跳过，那是"一个账号只有一份凭据"时代的权宜之计。
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

        // 这一行是给测试用的：模拟"检查之前 / 检查之后"发生了一次扫码登录。
        // 生产构建里是空函数，编译期就没了。
        run_import_race_hook(&login.uid);

        // 每个账号的"写钥匙串 → 写库"必须在锁里完成，来源判断也在锁里。
        let _guard = auth_write_guard();

        let existing = app.with_db(|db| db.find_account("workbuddy", &login.uid))?;
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
        let expires_at = login.expires_at_ms.and_then(workbuddy_auth::ms_to_rfc3339);
        let domain = Some(login.domain.clone()).filter(|d| !d.is_empty());

        let account = match app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: login.uid.clone(),
                nickname: nickname.clone(),
                auth_domain: domain.clone(),
                ..Default::default()
            })
        }) {
            Ok(record) => record,
            Err(err) => {
                skipped.push(json!({
                    "key": login.key,
                    "uid": login.uid,
                    "reason": "storage",
                    "message": crate::rpc::sanitize(&err.message),
                }));
                continue;
            }
        };

        match crate::auth::service::save_magpie(
            app,
            &account.id,
            &login.uid,
            &login.access_token,
            Some(&login.key),
            expires_at.as_deref(),
        ) {
            Ok(saved) => {
                let fresh = app
                    .with_db(|db| db.get_account(&account.id))?
                    .unwrap_or(account);
                imported.push(json!({
                    "account": fresh,
                    "authorization": saved.authorization,
                    "degraded": saved.degraded,
                }));
            }
            Err(err) => {
                // 凭据没写进去 = 这次导入没有发生。新建的账号行也要撤掉，
                // 不能留下"有账号、没凭据"的半成品。
                if existing.is_none() {
                    let _ = app.with_db(|db| db.delete_account(&account.id));
                }
                skipped.push(json!({
                    "key": login.key,
                    "uid": login.uid,
                    "reason": if err.code == crate::rpc::E_SECURESTORE { "keychain" } else { "storage" },
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
/// 同步函数：只做「读 Magpie 文件 → 写 magpie/access → 更新状态」，没有任何 await。
/// 旅行调度器也要在非 async 的凭据钩子里调它，所以不能挂在 async fn 上。
///
/// **只写 magpie 那一行**。自有授权的键连读都不读，更不会写。
pub(crate) fn resync_magpie_access(
    app: &App,
    account: &crate::storage::AccountRecord,
) -> RpcResult<Value> {
    let _guard = auth_write_guard();
    // 调用方可能拿着几秒前的快照（自动旅行就是这样）。先按 id 重新读一次库：
    // 版本变了说明"我手上这份已经不是现在这个账号"，直接放弃。
    let current = app
        .with_db(|db| db.get_account(&account.id))?
        .ok_or_else(|| RpcError::not_found("账号已不存在，放弃重同步"))?;
    if current.auth_epoch != account.auth_epoch {
        return Err(superseded("epoch_changed"));
    }
    // 前提是"这个账号确实有一份借用授权"，而不是"镜像当前指向 magpie"。
    if app
        .with_db(|db| db.find_authorization(&current.id, crate::auth::ids::KIND_MAGPIE))?
        .is_none()
    {
        return Err(RpcError::new(
            crate::rpc::E_AUTH,
            "该账号没有 Magpie 授权，不能从 Magpie 覆盖凭据",
        )
        .with_data(json!({ "kind": "not_magpie_source" })));
    }

    let logins = magpie::read_logins().unwrap_or_default();
    let now = magpie::now_ms();
    let hit = logins
        .into_iter()
        .find(|l| l.uid == current.uid && l.expires_at_ms.map(|at| at > now).unwrap_or(true));

    let Some(login) = hit else {
        // Magpie 那份也没了：只把这个账号的 magpie 授权标过期，own 不受影响。
        if let Some(row) =
            app.with_db(|db| db.find_authorization(&current.id, crate::auth::ids::KIND_MAGPIE))?
        {
            let _ = crate::auth::service::record_status(
                app,
                &row,
                "expired",
                None,
                Some("magpie_token_expired"),
            );
        }
        return Ok(json!({
            "state": "expired",
            "source": magpie::SOURCE_MAGPIE,
            "error": {
                "kind": "magpie_token_expired",
                "message": "Magpie 里的令牌也已过期。请在 Magpie 重新登录，或改用扫码授权。",
            },
        }));
    };

    let expires = login.expires_at_ms.and_then(workbuddy_auth::ms_to_rfc3339);
    let saved = crate::auth::service::save_magpie(
        app,
        &current.id,
        &current.uid,
        &login.access_token,
        Some(&login.key),
        expires.as_deref(),
    )?;
    Ok(json!({
        "state": "valid",
        "resynced": true,
        "tokenExpiresAt": saved.authorization.token_expires_at,
        "source": magpie::SOURCE_MAGPIE,
        "degraded": saved.degraded,
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

/// 登录成功后落库：令牌进**自有授权的键**，账号进 SQLite，返回值里没有任何令牌。
///
/// 只写 `own`：从 Magpie 导入过的那份 `magpie/access` 一个字节都不动（G1）。
/// 回滚策略：入库失败时恢复登录前的自有凭据快照，而不是清空 —— 用户原来能用的那套
/// 不能因为一次失败登录被删掉。
fn persist_login(app: &App, payload: &workbuddy_auth::TokenPayload) -> RpcResult<Value> {
    // 整个落库过程都在锁里：钥匙串写入 + 数据库写入必须成对出现，
    // 否则并发的 resync / refresh 会看到中间态。
    let _guard = auth_write_guard();
    let uid = payload.profile.uid.clone();
    let account_id = crate::storage::account_id("workbuddy", &uid);

    // 顺序：写凭据 → 建账号行 → 记授权行。账号行还不存在时授权行会被外键拒掉，
    // 所以凭据必须最先落，而它也是唯一需要显式回滚的一步（钥匙串没有事务）。
    let snapshot = crate::auth::service::write_own(
        app,
        &account_id,
        &payload.access_token,
        payload.refresh_token.as_deref(),
    )?;

    let (mut value, existed) = match write_login_row(app, payload) {
        Ok(pair) => pair,
        Err(err) => return Err(fail_login(app, &snapshot, err)),
    };

    let expires = payload
        .expires_at_ms
        .and_then(workbuddy_auth::ms_to_rfc3339);
    match crate::auth::service::record_grant(
        app,
        &account_id,
        &uid,
        crate::auth::ids::KIND_OWN,
        expires.as_deref(),
    ) {
        Ok(saved) => {
            if let Some(reason) = saved.degraded {
                // 凭据和授权行都好了，只有兼容别名没同步成功：如实报降级。
                value["degraded"] = json!({ "kind": "alias_sync_failed", "message": reason });
            }
            Ok(value)
        }
        Err(err) => {
            // 授权行没记上 = 这次登录没有完成。凭据回滚，新建的账号行也撤掉。
            let clean = snapshot.restore(app);
            if !existed {
                let _ = app.with_db(|db| db.delete_account(&account_id));
            }
            Err(if clean {
                err
            } else {
                RpcError::new(
                    err.code,
                    format!("{}（并且旧凭据未能完整恢复，请重新登录）", err.message),
                )
                .with_data(json!({ "kind": "rollback_incomplete", "cause": err.data }))
            })
        }
    }
}

/// 登录失败时统一收尾：回滚钥匙串，回滚不干净就如实说。
fn fail_login(app: &App, snapshot: &crate::auth::service::Snapshot, err: RpcError) -> RpcError {
    if snapshot.restore(app) {
        return err;
    }
    RpcError::new(
        err.code,
        format!("{}（并且旧凭据未能完整恢复，请重新登录）", err.message),
    )
    .with_data(json!({ "kind": "rollback_incomplete", "cause": err.data }))
}

/// 落库部分单独拆出来：方便用"一定失败"的写入器验证回滚，不必真把数据库弄坏。
///
/// 只写用户可编辑的字段：授权相关的四列由 `service::sync_mirror` 从授权行派生（G6）。
/// 返回 (响应体, 是否新账号)。
fn write_login_row(app: &App, payload: &workbuddy_auth::TokenPayload) -> RpcResult<(Value, bool)> {
    let uid = payload.profile.uid.clone();
    let domain = Some(payload.domain.clone()).filter(|d| !d.is_empty());

    let (existed, record) = app.with_db(|db| {
        let existed = db.find_account("workbuddy", &uid)?.is_some();
        let record = db.upsert_account(&AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.clone(),
            nickname: payload.profile.nickname.clone(),
            email: payload.profile.email.clone(),
            auth_domain: domain.clone(),
            ..Default::default()
        })?;
        Ok((existed, record))
    })?;

    Ok((
        json!({
            "state": "success",
            "reused": existed,
            "accountId": record.id,
        }),
        existed,
    ))
}

/// 落盘服务端刚轮换出来的令牌。
///
/// 关键点：**服务端轮换成功之后，旧的 refresh 就已经作废**。所以这里不能像登录那样
/// "失败就恢复快照"——那会把一个死令牌写回去，让账号看起来还能刷新。
/// 正确做法：尽量保住新的 access；refresh 落不下去就标失效、把死令牌删掉，
/// 明确告诉用户要重新登录。
///
/// 只写 own 的键：刷新是自有授权的独占能力，magpie 走的是"重新同步 access"。
fn persist_rotated_tokens(
    app: &App,
    account: &crate::storage::AccountRecord,
    expected_epoch: i64,
    access: &str,
    refresh: Option<&str>,
    expires: Option<&str>,
) -> RpcResult<Value> {
    let _guard = auth_write_guard();
    // 刷新是一次网络往返：出发时读的账号，回来时可能已经被扫码换成了新的授权。
    // 版本对不上就什么都不写 —— 那份新令牌属于"上一个授权"，写下去就是覆盖。
    let current = match app.with_db(|db| db.get_account(&account.id))? {
        Some(record) => record,
        None => {
            return Ok(json!({
                "state": "superseded",
                "reason": "account_deleted",
            }))
        }
    };
    if current.auth_epoch != expected_epoch {
        return Ok(json!({ "state": "superseded", "reason": "epoch_changed" }));
    }
    let Some(row) =
        app.with_db(|db| db.find_authorization(&current.id, crate::auth::ids::KIND_OWN))?
    else {
        return Ok(json!({ "state": "superseded", "reason": "no_own_authorization" }));
    };
    let keys = crate::auth::service::own_keys(&current.id);
    let refresh_key = keys.refresh.clone().expect("own 一定有 refresh 键");

    // 两个键**分别**尝试：access 写不进去时，新的 refresh 仍然是有效令牌，
    // 留住它，下一次刷新还能把账号救回来。
    let access_saved = app.secure.set(&keys.access, access.as_bytes());
    let refresh_saved = match refresh {
        Some(next) => app.secure.set(&refresh_key, next.as_bytes()),
        None => Ok(()), // 服务端没轮换 refresh，旧的那份继续有效
    };

    if access_saved.is_ok() && refresh_saved.is_ok() {
        let _ = crate::auth::service::record_status(app, &row, "valid", expires, None);
        return Ok(json!({ "state": "valid", "tokenExpiresAt": expires }));
    }

    // 只有「服务端确实给了新 refresh、而这个新 refresh 没写进去」才说明旧 refresh 已经作废，
    // 这时必须把它清掉；否则旧 refresh 可能还能用，删了反而丢掉恢复能力。
    let rotated = refresh.is_some();
    let dead_refresh = rotated && refresh_saved.is_err();
    let cleared = if dead_refresh {
        clear_dead_refresh(app, &refresh_key)
    } else {
        true
    };

    let reason = if access_saved.is_err() {
        "新的访问令牌没能写进钥匙串"
    } else {
        "新的刷新令牌没能写进钥匙串"
    };
    let detail = if dead_refresh && !cleared {
        format!("{reason}；旧刷新令牌已被服务端作废且未能清除，请重新登录")
    } else {
        format!("{reason}，请重新登录")
    };
    let _ = crate::auth::service::record_status(
        app,
        &row,
        "invalid",
        None,
        Some("refresh_not_persisted"),
    );

    Ok(json!({
        "state": "invalid",
        "needsRelogin": true,
        "error": { "kind": "refresh_not_persisted", "message": detail },
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
///
/// 没有 own 授权（只有借用）时走 Magpie 重同步：借用授权永远不刷新。
pub(crate) async fn refresh_account(app: &App, id: &str) -> RpcResult<Value> {
    let account = app
        .with_db(|db| db.get_account(id))?
        .ok_or_else(|| RpcError::not_found(format!("账号不存在：{id}")))?;
    if account.platform != "workbuddy" {
        return Err(RpcError::invalid_params("目前只支持刷新 WorkBuddy 账号"));
    }
    let own = app.with_db(|db| db.find_authorization(&account.id, crate::auth::ids::KIND_OWN))?;
    let Some(own) = own else {
        return resync_magpie_access(app, &account);
    };

    let Some(access) =
        crate::auth::service::read_access(app, &account, crate::auth::ids::KIND_OWN)?
    else {
        let _ = crate::auth::service::record_status(
            app,
            &own,
            "invalid",
            None,
            Some("missing_credential"),
        );
        return Ok(json!({
            "state": "invalid",
            "error": { "kind": "missing_credential", "message": "钥匙串里没有访问令牌，请重新登录" },
        }));
    };
    let Some(refresh) = crate::auth::service::read_own_refresh(app, &account)? else {
        let _ = crate::auth::service::record_status(
            app,
            &own,
            "invalid",
            None,
            Some("missing_credential"),
        );
        return Ok(json!({
            "state": "invalid",
            "error": { "kind": "missing_credential", "message": "钥匙串里没有刷新令牌，请重新登录" },
        }));
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
                &account,
                account.auth_epoch,
                &access_token,
                refresh_token.as_deref(),
                expires.as_deref(),
            )
        }
        workbuddy_auth::Refreshed::Invalid(err) => {
            let _ =
                crate::auth::service::record_status(app, &own, "invalid", None, Some(&err.kind));
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

/// 通用 `secure.*` 默认关闭：内部逻辑走 `SecureStore`，不经过 RPC。
///
/// 能写 stdin 的进程本来就能覆盖 AICC 的凭据，这条通道不该长期存在。
/// 兼容开关只给受控的开发 / 迁移场景用，而且仍然要过键名白名单。
fn require_secure_rpc(method: &str) -> RpcResult<()> {
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
fn validate_secure_key(key: &str) -> RpcResult<()> {
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

fn require_setting_key(key: &str) -> RpcResult<()> {
    if key == crate::auth::broker::PREFERRED_SETTING {
        Ok(())
    } else {
        Err(RpcError::invalid_params(format!("未知设置项：{key}")))
    }
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
    //! 多来源授权的回归测试（G1 / G3 / G5 / G6）。
    //!
    //! 全部离线：内存钥匙串 + 临时库 + 直接调内部函数。网络路径（探测、签到详情）在
    //! `auth::probe` 的纯函数测试与 status 的分支里覆盖，这里只钉凭据归属与版本。

    use super::*;
    use crate::auth::{broker, ids, service};
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

    fn account(app: &App, uid: &str) -> crate::storage::AccountRecord {
        app.with_db(|db| db.find_account("workbuddy", uid))
            .expect("query")
            .expect("account")
    }

    fn read_key(app: &App, key: &str) -> Option<String> {
        app.secure
            .get(key)
            .ok()
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
    }

    fn own_access(app: &App, account_id: &str) -> Option<String> {
        read_key(app, &ids::access_key(account_id, ids::KIND_OWN))
    }

    fn own_refresh(app: &App, account_id: &str) -> Option<String> {
        read_key(app, &ids::refresh_key(account_id, ids::KIND_OWN))
    }

    fn magpie_access(app: &App, account_id: &str) -> Option<String> {
        read_key(app, &ids::access_key(account_id, ids::KIND_MAGPIE))
    }

    fn legacy_access(app: &App, uid: &str) -> Option<String> {
        read_key(app, &ids::legacy_keys(uid).0)
    }

    // -----------------------------------------------------------------------
    // G1：两套授权互不覆盖
    // -----------------------------------------------------------------------

    #[test]
    fn own_login_writes_only_the_own_keys() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-1111-2222-3333-444455556666";
        let out =
            persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("persist");
        assert_eq!(out["state"], json!("success"));

        let acc = account(&app, uid);
        assert_eq!(own_access(&app, &acc.id).as_deref(), Some("own-access"));
        assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("own-refresh"));
        assert!(
            magpie_access(&app, &acc.id).is_none(),
            "登录不能顺手造一份借用授权"
        );
        // 只有一份授权时，它就是镜像来源：旧别名必须同步。
        assert_eq!(legacy_access(&app, uid).as_deref(), Some("own-access"));
        assert_eq!(acc.auth_source, "own");
    }

    #[test]
    fn magpie_import_writes_only_the_magpie_key() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-2222-3333-4444-555566667777";
        let out = import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        assert_eq!(out["imported"], json!(1), "{out}");

        let acc = account(&app, uid);
        assert_eq!(magpie_access(&app, &acc.id).as_deref(), Some("borrowed"));
        assert!(
            own_access(&app, &acc.id).is_none(),
            "导入不能凭空造一份自有授权"
        );
        assert!(
            app.secure
                .get(&ids::refresh_key(&acc.id, ids::KIND_MAGPIE))
                .is_err(),
            "magpie/refresh 这个键不允许存在"
        );
    }

    /// 核心场景：先导入、再扫码、再导入 —— 三份操作互不覆盖。
    #[test]
    fn two_authorizations_survive_login_and_reimport() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-3333-4444-5555-666677778888";

        import_logins(&app, vec![magpie_login(uid, "borrowed-1")], &[]).expect("import");
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("login");
        import_logins(&app, vec![magpie_login(uid, "borrowed-2")], &[]).expect("re-import");

        let acc = account(&app, uid);
        assert_eq!(
            own_access(&app, &acc.id).as_deref(),
            Some("own-access"),
            "重新导入不能覆盖自有授权"
        );
        assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("own-refresh"));
        assert_eq!(
            magpie_access(&app, &acc.id).as_deref(),
            Some("borrowed-2"),
            "重新导入只更新借用授权"
        );

        let rows = app.with_db(|db| db.list_authorizations(&acc.id)).unwrap();
        assert_eq!(rows.len(), 2, "两份授权并存，各一行");
        let kinds: Vec<&str> = rows.iter().map(|row| row.kind.as_str()).collect();
        assert_eq!(kinds, vec!["magpie", "own"], "按 kind 排序，各一行");
    }

    /// 扫码不能把 magpie 那份挤掉，反之亦然；镜像跟着首选来源走（G6）。
    #[test]
    fn mirror_follows_the_preferred_source() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-4444-5555-6666-777788889999";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("login");
        let acc = account(&app, uid);

        // 默认 magpie 优先
        assert_eq!(acc.auth_source, "magpie");
        assert_eq!(legacy_access(&app, uid).as_deref(), Some("borrowed"));
        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => assert_eq!(resolved.kind, "magpie"),
            broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
        }

        broker::set_preferred_source(&app, "own").expect("set");
        service::sync_mirror(&app, &acc.id).expect("mirror");
        let acc = account(&app, uid);
        assert_eq!(acc.auth_source, "own");
        assert_eq!(legacy_access(&app, uid).as_deref(), Some("own-access"));
        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => {
                assert_eq!(resolved.kind, "own");
                assert_eq!(resolved.access, "own-access");
            }
            broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
        }
    }

    /// 首选那份失效时兜底到另一份，且不改另一份的状态。
    #[test]
    fn invalid_preferred_falls_back_to_the_other_authorization() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-5555-6666-7777-888899990000";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("login");
        let acc = account(&app, uid);

        let magpie_row = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .unwrap();
        assert!(
            service::record_status(&app, &magpie_row, "invalid", None, Some("unauthorized"))
                .unwrap()
        );

        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => {
                assert_eq!(resolved.kind, "own", "首选坏了就该兜底");
                assert!(resolved.degraded.is_some(), "兜底必须说明原因");
                assert_eq!(resolved.access, "own-access");
            }
            broker::Outcome::Unavailable { reason, .. } => panic!("应当兜底：{reason}"),
        }
        let own_row = app
            .with_db(|db| db.find_authorization(&acc.id, "own"))
            .unwrap()
            .unwrap();
        assert_eq!(own_row.status, "valid", "兜底不能顺手改另一份的状态");
    }

    /// 表上的 CHECK 必须挡住"magpie 带 refresh"这种行。
    #[test]
    fn magpie_row_cannot_carry_a_refresh_ref() {
        let (app, _dir) = test_app(SecureStore::memory());
        let acc = app
            .with_db(|db| {
                db.upsert_account(&AccountInput {
                    platform: "workbuddy".into(),
                    uid: "uid-check".into(),
                    ..Default::default()
                })
            })
            .expect("seed");
        let row = crate::storage::AuthorizationRecord {
            id: ids::auth_id(&acc.id, "magpie"),
            account_id: acc.id.clone(),
            kind: "magpie".to_string(),
            access_ref: ids::keychain_ref(&ids::access_key(&acc.id, "magpie")),
            refresh_ref: Some("keychain:workbuddy/x/magpie/refresh".into()),
            auth_domain: None,
            token_expires_at: None,
            status: "valid".into(),
            version: 0,
            key_migration_state: "ready".into(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
        };
        assert!(
            app.with_db(|db| db.upsert_authorization(&row)).is_err(),
            "CHECK (kind <> 'magpie' OR refresh_ref IS NULL) 必须生效"
        );
    }

    // -----------------------------------------------------------------------
    // 写入失败 / 回滚
    // -----------------------------------------------------------------------

    #[test]
    fn login_without_refresh_drops_the_stale_one() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-aaaa-bbbb-cccc-ddddeeeeffff";
        persist_login(&app, &payload(uid, "first-access", Some("first-refresh"))).expect("first");
        persist_login(&app, &payload(uid, "second-access", None)).expect("second");

        let acc = account(&app, uid);
        assert!(
            own_refresh(&app, &acc.id).is_none(),
            "没有新 refresh 必须清掉旧的"
        );
        assert_eq!(own_access(&app, &acc.id).as_deref(), Some("second-access"));
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

    /// access 写进去了、refresh 写失败：必须回到原来那套。
    #[test]
    fn refresh_write_failure_keeps_the_old_pair() {
        let (app, _dir) = test_app(SecureStore::faulty_on_value("boom"));
        let uid = "6f1c2d94-aaaa-0001-0002-000000000001";
        let account_id = crate::storage::account_id("workbuddy", uid);
        app.secure
            .set(&ids::access_key(&account_id, "own"), b"old-access")
            .unwrap();
        app.secure
            .set(&ids::refresh_key(&account_id, "own"), b"old-refresh")
            .unwrap();
        app.secure.arm();

        let err = persist_login(&app, &payload(uid, "new-access", Some("new-refresh-boom")))
            .expect_err("refresh 写不进去就该失败");
        assert_eq!(err.data.as_ref().unwrap()["stage"], json!("own/refresh"));
        assert_eq!(own_access(&app, &account_id).as_deref(), Some("old-access"));
        assert_eq!(
            own_refresh(&app, &account_id).as_deref(),
            Some("old-refresh")
        );
    }

    #[test]
    fn access_write_failure_keeps_the_old_pair() {
        let (app, _dir) = test_app(SecureStore::faulty_on_value("boom"));
        let uid = "6f1c2d94-aaaa-0001-0002-000000000002";
        let account_id = crate::storage::account_id("workbuddy", uid);
        app.secure
            .set(&ids::access_key(&account_id, "own"), b"old-access")
            .unwrap();
        app.secure
            .set(&ids::refresh_key(&account_id, "own"), b"old-refresh")
            .unwrap();
        app.secure.arm();

        let err = persist_login(&app, &payload(uid, "new-access-boom", Some("new-refresh")))
            .expect_err("access 写不进去就该失败");
        assert_eq!(err.data.as_ref().unwrap()["stage"], json!("own/access"));
        assert_eq!(own_access(&app, &account_id).as_deref(), Some("old-access"));
        assert_eq!(
            own_refresh(&app, &account_id).as_deref(),
            Some("old-refresh")
        );
    }

    /// 登录没发新 refresh、旧的又删不掉：整体失败并回滚。
    #[test]
    fn stale_refresh_cleanup_failure_aborts_the_login() {
        let (app, _dir) = test_app(SecureStore::faulty(&[], &["/own/refresh"]));
        let uid = "6f1c2d94-aaaa-0001-0002-000000000003";
        let account_id = crate::storage::account_id("workbuddy", uid);
        app.secure
            .set(&ids::access_key(&account_id, "own"), b"old-access")
            .unwrap();
        app.secure
            .set(&ids::refresh_key(&account_id, "own"), b"old-refresh")
            .unwrap();
        app.secure.arm();

        let err = persist_login(&app, &payload(uid, "new-access", None)).expect_err("必须失败");
        assert_eq!(
            err.data.as_ref().unwrap()["kind"],
            json!("stale_refresh_not_cleared")
        );
        assert_eq!(own_access(&app, &account_id).as_deref(), Some("old-access"));
        assert_eq!(
            own_refresh(&app, &account_id).as_deref(),
            Some("old-refresh")
        );
    }

    /// 入库失败：恢复登录前的凭据，借用那份一个字节不动。
    #[test]
    fn db_failure_keeps_previous_credentials() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-aaaa-0001-0002-000000000004";
        import_logins(&app, vec![magpie_login(uid, "borrowed-access")], &[]).expect("import");
        let acc = account(&app, uid);

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
            magpie_access(&app, &acc.id).as_deref(),
            Some("borrowed-access"),
            "借用授权必须原样保留"
        );
        assert!(
            own_access(&app, &acc.id).is_none(),
            "登录失败不能留下自有凭据"
        );
    }

    #[test]
    fn snapshot_read_failure_aborts_the_login() {
        let (app, _dir) = test_app(SecureStore::faulty_on_read());
        let uid = "6f1c2d94-dddd-0001-0002-000000000001";
        let account_id = crate::storage::account_id("workbuddy", uid);
        app.secure
            .set(&ids::access_key(&account_id, "own"), b"old-access")
            .unwrap();
        app.secure
            .set(&ids::refresh_key(&account_id, "own"), b"old-refresh")
            .unwrap();
        app.secure.arm();

        let err = persist_login(&app, &payload(uid, "new-access", Some("new-refresh")))
            .expect_err("读不到旧凭据就不能写");
        assert_eq!(
            err.data.as_ref().unwrap()["kind"],
            json!("snapshot_unreadable")
        );

        app.secure.disarm();
        assert_eq!(own_access(&app, &account_id).as_deref(), Some("old-access"));
        assert_eq!(
            own_refresh(&app, &account_id).as_deref(),
            Some("old-refresh")
        );
    }

    /// 回滚本身失败时必须报 `rollback_incomplete`。
    #[test]
    fn rollback_failure_is_reported_as_incomplete() {
        let (app, _dir) = test_app(SecureStore::faulty(&["/own/access"], &[]));
        let uid = "6f1c2d94-dddd-0001-0002-000000000002";
        let account_id = crate::storage::account_id("workbuddy", uid);
        app.secure
            .set(&ids::access_key(&account_id, "own"), b"old-access")
            .unwrap();
        app.secure.arm();

        let err = persist_login(&app, &payload(uid, "new-access", Some("new-refresh")))
            .expect_err("写不进去就该失败");
        assert_eq!(
            err.data.as_ref().unwrap()["kind"],
            json!("rollback_incomplete"),
            "{err:?}"
        );
        assert!(err.message.contains("未能完整恢复"), "{}", err.message);
    }

    // -----------------------------------------------------------------------
    // 重同步 / 刷新
    // -----------------------------------------------------------------------

    #[test]
    fn resync_refuses_an_account_without_magpie_authorization() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-1234-5678-90ab-cdef12345678";
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("persist");
        let acc = account(&app, uid);

        let err = resync_magpie_access(&app, &acc).expect_err("没有 magpie 授权必须被拒绝");
        assert_eq!(
            err.data.as_ref().unwrap()["kind"],
            json!("not_magpie_source")
        );
        assert_eq!(own_access(&app, &acc.id).as_deref(), Some("own-access"));
    }

    #[test]
    fn resync_rejects_a_snapshot_whose_epoch_changed() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-eeee-0001-0002-000000000002";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let stale = account(&app, uid);

        app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: uid.to_string(),
                nickname: Some("刚改过".to_string()),
                ..Default::default()
            })
        })
        .expect("touch");

        let err = resync_magpie_access(&app, &stale).expect_err("版本对不上就该拒绝");
        assert_eq!(err.data.as_ref().unwrap()["kind"], json!("superseded"));
    }

    /// 扫码之后，拿着旧快照去 resync magpie：版本变了，必须拒绝，own 一个字节不动。
    #[test]
    fn stale_snapshot_cannot_overwrite_a_new_own_login() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-eeee-0001-0002-000000000001";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let stale = account(&app, uid);
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("login");
        let acc = account(&app, uid);

        let err = resync_magpie_access(&app, &stale).expect_err("旧快照必须被拒绝");
        assert_eq!(err.data.as_ref().unwrap()["kind"], json!("superseded"));
        assert_eq!(own_access(&app, &acc.id).as_deref(), Some("own-access"));
        assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("own-refresh"));
    }

    /// 自有授权缺 refresh：刷新必须明确报"要重新登录"，且不能顺手删掉还能用的 access。
    #[test]
    fn refresh_without_own_refresh_reports_missing_credential() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-eeee-0001-0002-000000000004";
        // 登录时服务端没发 refresh → 只留 access
        persist_login(&app, &payload(uid, "access-only", None)).expect("login");
        let acc = account(&app, uid);
        let account_id = acc.id.clone();

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let out = runtime
            .block_on(refresh_account(&app, &acc.id))
            .expect("refresh 不该抛错");

        assert_eq!(out["state"], json!("invalid"));
        assert_eq!(out["error"]["kind"], json!("missing_credential"));
        assert_eq!(
            own_access(&app, &account_id).as_deref(),
            Some("access-only"),
            "access 不能被顺手删掉：用户可能还想用它只读查询"
        );
        let row = app
            .with_db(|db| db.find_authorization(&account_id, "own"))
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "invalid");
    }

    #[test]
    fn rotation_is_dropped_when_the_epoch_moved_on() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-eeee-0001-0002-000000000003";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let stale = account(&app, uid);
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("login");
        let acc = account(&app, uid);

        let out = persist_rotated_tokens(
            &app,
            &stale,
            stale.auth_epoch,
            "rotated-access",
            Some("rotated-refresh"),
            None,
        )
        .expect("应当返回 superseded 而不是报错");
        assert_eq!(out["state"], json!("superseded"));
        assert_eq!(own_access(&app, &acc.id).as_deref(), Some("own-access"));
        assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("own-refresh"));
    }

    /// 服务端轮换了、refresh 写不进去：新 access 尽力保住，死 refresh 必须删掉。
    #[test]
    fn rotation_failure_marks_invalid_and_drops_the_dead_refresh() {
        let (app, _dir) = test_app(SecureStore::faulty(&["/own/refresh"], &[]));
        let uid = "6f1c2d94-aaaa-0001-0002-000000000005";
        persist_login(&app, &payload(uid, "old-access", Some("dead-refresh"))).expect("login");
        let acc = account(&app, uid);
        let account_id = acc.id.clone();
        app.secure.arm();

        let out = persist_rotated_tokens(
            &app,
            &acc,
            acc.auth_epoch,
            "new-access",
            Some("new-refresh"),
            None,
        )
        .expect("返回 invalid 而不是抛错");

        assert_eq!(out["state"], json!("invalid"));
        assert_eq!(out["needsRelogin"], json!(true));
        assert_eq!(out["error"]["kind"], json!("refresh_not_persisted"));
        assert_eq!(
            own_access(&app, &account_id).as_deref(),
            Some("new-access"),
            "新 access 能写就写"
        );
        assert!(
            own_refresh(&app, &account_id).is_none(),
            "被服务端作废的 refresh 必须清掉"
        );
        let row = app
            .with_db(|db| db.find_authorization(&account_id, "own"))
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "invalid");
    }

    /// P0：Magpie 的 access 写进钥匙串了、授权行却写不进去 —— 必须把钥匙串恢复原样。
    /// 否则会留下"新令牌在钥匙串、库里还是旧状态"的组合。
    #[test]
    fn magpie_db_failure_restores_the_keychain() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-7777-0001-0002-000000000001";
        import_logins(&app, vec![magpie_login(uid, "borrowed-1")], &[]).expect("import");
        let acc = account(&app, uid);

        app.with_db(|db| {
            db.exec_raw(
                "CREATE TRIGGER no_auth_update BEFORE UPDATE ON account_authorizations
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            )
        })
        .expect("install trigger");

        let err = match service::save_magpie(&app, &acc.id, uid, "borrowed-2", None, None) {
            Ok(_) => panic!("库里写不进去就该失败"),
            Err(err) => err,
        };
        assert_eq!(err.code, crate::rpc::E_STORAGE);
        assert_eq!(
            magpie_access(&app, &acc.id).as_deref(),
            Some("borrowed-1"),
            "钥匙串必须回到写入前的内容"
        );
    }

    /// 旅行侧：选中的是自有授权但刷新不成（没有 refresh），要兜底到另一份现成的 access。
    /// 这条路径不发网络请求，所以能在单测里跑。
    #[test]
    fn travel_resync_falls_back_to_the_other_authorization() {
        use crate::travel::scheduler::AppTokens;
        use crate::travel::service::Tokens;

        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-7777-0001-0002-000000000002";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        // 自有授权有 access、没有 refresh：refresh 一定失败，且不需要联网
        persist_login(&app, &payload(uid, "own-access", None)).expect("login");
        broker::set_preferred_source(&app, "own").expect("prefer own");
        let acc = account(&app, uid);

        let tokens = AppTokens { app: &app };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = runtime.block_on(tokens.resync(&acc));

        assert_eq!(result.as_deref(), Ok("borrowed"), "主路失败要兜底到另一份");
        assert_eq!(
            magpie_access(&app, &acc.id).as_deref(),
            Some("borrowed"),
            "兜底只读，不许改另一份"
        );
        let own = app
            .with_db(|db| db.find_authorization(&acc.id, "own"))
            .unwrap()
            .unwrap();
        assert_eq!(own.status, "invalid", "自有授权缺 refresh 应标失效");
    }

    /// 两份都拿不出来时才报错，不会无限重试。
    #[test]
    fn travel_resync_reports_when_both_sources_are_unusable() {
        use crate::travel::scheduler::AppTokens;
        use crate::travel::service::Tokens;

        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-7777-0001-0002-000000000003";
        persist_login(&app, &payload(uid, "own-access", None)).expect("login");
        broker::set_preferred_source(&app, "own").expect("prefer own");
        let acc = account(&app, uid);

        let tokens = AppTokens { app: &app };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = runtime.block_on(tokens.resync(&acc));
        assert!(result.is_err(), "没有可用来源就该报错，而不是返回空令牌");
    }

    // -----------------------------------------------------------------------
    // 删除
    // -----------------------------------------------------------------------

    #[test]
    fn deleting_one_authorization_leaves_the_other_intact() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000001";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("login");
        let acc = account(&app, uid);

        let report = service::delete_authorization(&app, &acc.id, "own").expect("delete");
        assert!(report.record_deleted);
        assert!(own_access(&app, &acc.id).is_none());
        assert!(own_refresh(&app, &acc.id).is_none());
        assert_eq!(
            magpie_access(&app, &acc.id).as_deref(),
            Some("borrowed"),
            "另一份授权必须原样保留"
        );
        assert!(app
            .with_db(|db| db.find_authorization(&acc.id, "own"))
            .unwrap()
            .is_none());
        assert!(app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .is_some());
    }

    #[test]
    fn deleting_an_account_purges_five_keys() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000002";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("login");
        let acc = account(&app, uid);

        let failed = service::purge_account_keys(&app, &acc);
        assert!(failed.is_empty(), "{failed:?}");
        assert!(own_access(&app, &acc.id).is_none());
        assert!(own_refresh(&app, &acc.id).is_none());
        assert!(magpie_access(&app, &acc.id).is_none());
        assert!(legacy_access(&app, uid).is_none());
        assert!(read_key(&app, &ids::legacy_keys(uid).1).is_none());
    }

    /// 删掉最后一份授权：旧别名键必须一起清，镜像列必须一起空。
    /// 否则这个 uid 下次重新导入时会读到上一轮的令牌，界面上也还显示着"可用"。
    #[test]
    fn deleting_the_last_authorization_purges_the_legacy_alias() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000004";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let acc = account(&app, uid);
        assert_eq!(legacy_access(&app, uid).as_deref(), Some("borrowed"));

        let report = service::delete_authorization(&app, &acc.id, "magpie").expect("delete");
        assert!(report.record_deleted, "{:?}", report.failed_keys);
        assert!(
            report
                .removed_keys
                .iter()
                .any(|key| key.ends_with("/access") && key.contains(uid)),
            "旧别名 access 应在删除范围内：{:?}",
            report.removed_keys
        );
        assert!(legacy_access(&app, uid).is_none(), "旧别名 access 必须清掉");
        assert!(
            read_key(&app, &ids::legacy_keys(uid).1).is_none(),
            "旧别名 refresh 必须清掉"
        );
        assert!(magpie_access(&app, &acc.id).is_none());

        let acc = account(&app, uid);
        assert!(
            acc.auth_mirror_auth_id.is_none(),
            "一份授权都不剩时镜像必须清空"
        );
        assert_eq!(acc.auth_status, "unknown", "不能让界面以为还有可用授权");
    }

    /// 只剩一份时删它才算"最后一份"；删的是两份里的一份，别名要留给另一份。
    #[test]
    fn deleting_one_of_two_keeps_the_legacy_alias() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000005";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("login");
        let acc = account(&app, uid);

        service::delete_authorization(&app, &acc.id, "own").expect("delete");
        assert_eq!(
            legacy_access(&app, uid).as_deref(),
            Some("borrowed"),
            "镜像应切到还活着的那份，而不是把别名删掉"
        );
        assert!(
            read_key(&app, &ids::legacy_keys(uid).1).is_none(),
            "镜像是借用授权，别名下不能留 refresh"
        );
    }

    /// 改全局首选来源必须当场同步所有账号的兼容镜像，
    /// 不能等到下一次登录 / 导入才生效 —— 否则界面说 own、旧版二进制还在用 magpie。
    #[tokio::test]
    async fn changing_the_preferred_source_resyncs_every_mirror() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000006";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("login");
        let acc = account(&app, uid);
        assert_eq!(acc.auth_source, "magpie");
        assert_eq!(legacy_access(&app, uid).as_deref(), Some("borrowed"));

        // 走真正的 RPC 路径：settings.set 而不是直接改 broker
        let out = dispatch(
            &app,
            "settings.set",
            &json!({ "key": "auth_preferred_source", "value": "own" }),
        )
        .await
        .expect("settings.set");
        assert_eq!(out["mirrorErrors"], json!([]), "{out}");

        let acc = account(&app, uid);
        assert_eq!(acc.auth_source, "own", "镜像列要立刻跟着换");
        assert_eq!(
            legacy_access(&app, uid).as_deref(),
            Some("own-access"),
            "旧别名键也要立刻跟着换"
        );
        assert_eq!(
            read_key(&app, &ids::legacy_keys(uid).1).as_deref(),
            Some("own-refresh")
        );
    }

    /// 删不掉的键必须如实报出来（不吞错），并且记录留着以便重试。
    #[test]
    fn failed_key_deletion_is_reported_and_record_survives() {
        let (app, _dir) = test_app(SecureStore::faulty(&[], &["/own/access"]));
        let uid = "6f1c2d94-9999-0001-0002-000000000003";
        persist_login(&app, &payload(uid, "own-access", Some("own-refresh"))).expect("login");
        let acc = account(&app, uid);
        app.secure.arm();

        let report = service::delete_authorization(&app, &acc.id, "own").expect("report");
        assert_eq!(report.failed_keys.len(), 1);
        assert!(!report.record_deleted, "删不干净就不能声称删除成功");
        assert!(
            app.with_db(|db| db.find_authorization(&acc.id, "own"))
                .unwrap()
                .is_some(),
            "记录必须留着，否则再也找不到这次失败"
        );
    }

    // -----------------------------------------------------------------------
    // 旧快照与状态守卫
    // -----------------------------------------------------------------------

    #[test]
    fn stale_snapshot_covers_both_rejection_kinds() {
        assert!(super::is_stale_snapshot(&superseded("epoch_changed")));
        assert!(super::is_stale_snapshot(
            &RpcError::new(crate::rpc::E_AUTH, "own")
                .with_data(json!({ "kind": "not_magpie_source" }))
        ));
        assert!(!super::is_stale_snapshot(
            &RpcError::new(crate::rpc::E_AUTH, "magpie 令牌过期")
                .with_data(json!({ "kind": "magpie_token_expired" }))
        ));
        assert!(!super::is_stale_snapshot(&RpcError::internal("boom")));
    }

    // -----------------------------------------------------------------------
    // RPC 收口
    // -----------------------------------------------------------------------

    #[test]
    fn secure_key_allowlist_only_accepts_the_three_shapes() {
        for key in [
            "workbuddy/0123456789abcdef/own/access",
            "workbuddy/0123456789abcdef/own/refresh",
            "workbuddy/0123456789abcdef/magpie/access",
        ] {
            assert!(super::validate_secure_key(key).is_ok(), "{key}");
        }
        for key in [
            // 不存在这个键
            "workbuddy/0123456789abcdef/magpie/refresh",
            // 旧 UID 格式：只给 Rust 内部迁移用
            "workbuddy/6f1c2d94-1111-2222/access",
            // 位数不对
            "workbuddy/abc/own/access",
            // 探针键
            "__probe__1234",
            "anything/else",
        ] {
            assert!(super::validate_secure_key(key).is_err(), "{key} 不该被接受");
        }
    }

    #[test]
    fn secure_rpc_is_closed_unless_the_switch_is_on() {
        // CI 与本地都不该设这个变量；真设了就把断言让开，避免误报。
        if std::env::var("AICC_ALLOW_SECURE_RPC").as_deref() == Ok("1") {
            return;
        }
        let err = super::require_secure_rpc("secure.set").expect_err("默认必须关闭");
        assert_eq!(
            err.code,
            crate::rpc::METHOD_NOT_FOUND,
            "默认应当 method_not_found"
        );
    }

    #[test]
    fn only_the_known_setting_key_is_accepted() {
        assert!(super::require_setting_key(broker::PREFERRED_SETTING).is_ok());
        assert!(super::require_setting_key("auth_something_else").is_err());
    }
}
