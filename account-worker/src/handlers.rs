//! JSON-RPC 方法表。
//!
//! 命名约定：`<域>.<动作>`。PR1a 只覆盖本地能力，任何联网行为都不在这里。

use serde_json::{json, Value};

use std::sync::{Arc, Mutex, MutexGuard};

use crate::app::App;
use crate::providers::qoder_edition::QoderEdition;
use crate::providers::{
    magpie, qoder, qoder_admin, qoder_auth, qoder_pat, workbuddy, workbuddy_auth,
};
use crate::qoder_profiles::manager as qoder_manager;
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
    // 国际版的同名只读检查：与 CN 分开，绝不共用密钥/路径。
    "providers.qoder_global.presence",
    "providers.qoder_global.inspect",
    // ---- Qoder 双版本多账号（PR #70）----
    "qoder.editions.detect",
    "qoder.profiles.list",
    "qoder.profiles.scan",
    "qoder.profiles.create",
    "qoder.profiles.import",
    "qoder.profiles.adopt",
    "qoder.profiles.inspect",
    "qoder.profiles.capture",
    "qoder.profiles.rename",
    "qoder.profiles.open",
    "qoder.profiles.close",
    "qoder.profiles.activate",
    "qoder.profiles.remove",
    "qoder.profiles.status",
    "qoder.profiles.recover",
    // ---- Qoder 平台账号（PR #71，v13）----
    "qoder.accounts.list",
    // P0（PR #77）：删掉一个 Qoder 服务账号，连同它自己的凭据。
    "qoder.accounts.delete",
    // ---- Qoder 设备授权（PR #73）----
    "qoder.auth.start",
    "qoder.auth.poll",
    "qoder.auth.cancel",
    // ---- Qoder PAT 备用授权（PR #73 第二轮）----
    "qoder.auth.pat.save",
    "qoder.auth.pat.check",
    "qoder.auth.pat.remove",
    // ---- Qoder 每日 Credits（PR #75）----
    "qoder.credits.status",
    "qoder.credits.claim",
    "qoder.credits.runs",
    "qoder.credits.auto.status",
    "qoder.credits.auto.setEnabled",
    "qoder.credits.auto.tick",
    // ---- 统一任务汇总（PR #76）----
    "tasks.overview",
    "auth.workbuddy.start",
    "auth.workbuddy.poll",
    "auth.workbuddy.cancel",
    "auth.workbuddy.refresh",
    "providers.magpie.accounts",
    "auth.magpie.import",
    // PR-CRED-2：把旧的钥匙串引用改成 Magpie 动态关联（幂等、不猜归属）。
    "auth.magpie.relink",
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
    // ---- 成长中心自动管理（PR #68）----
    "growth.auto.status",
    "growth.auto.setEnabled",
    "growth.auto.tick",
    "growth.runs",
    // ---- WorkBuddy 每日签到（PR #69）----
    "checkin.status",
    "checkin.run",
    "checkin.runPending",
    "checkin.setEnabled",
    "checkin.auto.status",
    "checkin.auto.tick",
    "checkin.records",
    // ---- 四平台额度看板（M4）----
    "quota.board",
    "quota.history",
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
            let Some(account) = app.with_db(|db| db.get_account(&id))? else {
                return Ok(json!({
                    "deleted": false,
                    "failedKeys": [],
                    "orphanCredentials": false,
                }));
            };

            // 先读授权快照，再决定要不要碰钥匙串：
            // * 全是动态引用（`magpie:v1:` / `localjson:v1:`）→ 一次都不碰；
            // * 还有 `keychain:` 引用 → 只清它自己记录的那几个键 + 旧别名。
            // 顺序也很重要：先清凭据、后删库。反过来一旦清理失败，账号行已经没了，
            // 就再也没法按 id 重试，只能留成孤儿。
            let rows = app.with_db(|db| db.list_authorizations(&account.id))?;
            let plan = crate::auth::service::account_purge_plan(&rows, &account);
            let failed_keys = crate::auth::service::purge_keys(app, &plan);
            if !failed_keys.is_empty() {
                // 删不掉的键要可见，账号保留以便重试。
                return Ok(json!({
                    "deleted": false,
                    "failedKeys": failed_keys,
                    "orphanCredentials": true,
                }));
            }
            let deleted = app.with_db(|db| db.delete_account(&id))?;
            Ok(json!({
                "deleted": deleted,
                "failedKeys": [],
                "orphanCredentials": false,
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

        "providers.qoder_global.presence" => {
            let dir = params.get("authDir").and_then(Value::as_str);
            Ok(crate::providers::qoder_global::presence(dir))
        }

        "providers.qoder_global.inspect" => {
            let dir = params.get("authDir").and_then(Value::as_str);
            Ok(crate::providers::qoder_global::inspect(dir))
        }

        // ---- Qoder 双版本多账号（PR #70）----
        // 每个方法都带显式 edition 或 profileId，**没有**任何"用全局环境变量
        // 切换当前账号"的隐式状态：多账号场景下那就是串号源头。
        "qoder.editions.detect" => Ok(qoder_manager::editions_detect()),

        "qoder.profiles.list" => qoder_manager::list(app, params),

        "qoder.profiles.scan" => qoder_manager::scan(app, params),

        "qoder.profiles.create" => qoder_manager::create(app, params),

        "qoder.profiles.import" => qoder_manager::import(app, params),

        "qoder.profiles.adopt" => qoder_manager::adopt(app, params),

        "qoder.profiles.inspect" => qoder_manager::inspect(app, params),

        "qoder.profiles.capture" => qoder_manager::capture(app, params),

        "qoder.profiles.rename" => qoder_manager::rename(app, params),

        "qoder.profiles.open" => qoder_manager::open(app, params),

        "qoder.profiles.close" => qoder_manager::close(app, params),

        "qoder.profiles.activate" => qoder_manager::activate(app, params),

        "qoder.profiles.remove" => qoder_manager::remove(app, params),

        "qoder.profiles.status" => qoder_manager::status(app, params),

        "qoder.profiles.recover" => qoder_manager::recover(app, params),

        "qoder.accounts.list" => qoder_accounts_list(app, params),

        // 删号：关自动领取 → 推进授权版本 → 删钥匙串三条键 → 删行（授权/记录级联）。
        // 顺序写在 `providers/qoder_admin.rs` 顶部，别在这里改。
        "qoder.accounts.delete" => {
            let account_id = require_str(params, "accountId")?;
            Ok(qoder_admin::delete(app, &account_id)?.to_json())
        }

        // ---- Qoder 设备授权（PR #73）----
        //
        // 与 WorkBuddy 的扫码登录是**两套独立流程**：域名、client id、钥匙串键
        // 都按 edition 分开；这里也不碰 `accounts` 表（Qoder 账号在 v13 的表里）。
        "qoder.auth.start" => {
            let raw = require_str(params, "edition")?;
            let edition = QoderEdition::parse(&raw).ok_or_else(|| {
                RpcError::invalid_params("edition 只支持 qoder_cn / qoder_global")
            })?;
            // 发起阶段不联网：只生成 PKCE 并返回登录页。
            let info = qoder_auth::start(edition);
            Ok(json!({
                "edition": edition.as_str(),
                "displayName": edition.display_name(),
                "loginId": info.login_id,
                "verificationUri": info.verification_uri,
                "expiresIn": info.expires_in,
                "qrPayload": info.verification_uri,
            }))
        }

        "qoder.auth.poll" => {
            let login_id = require_str(params, "loginId")?;
            match qoder_auth::poll(&login_id).await {
                qoder_auth::Polled::Pending => Ok(json!({ "state": "pending" })),
                qoder_auth::Polled::Failed(err) => Ok(json!({
                    "state": poll_state(err.kind),
                    "error": { "kind": err.kind, "message": err.message },
                })),
                qoder_auth::Polled::Token(token) => {
                    // 版本只能从会话里原子取：取不到就说明会话已取消 / 过期 / 结束，
                    // 一律不落库（与 #72 同一套门禁，不允许任何默认值兜底）。
                    let edition = match qoder_auth::begin_commit(&login_id) {
                        Ok(edition) => edition,
                        Err(err) => {
                            return Ok(json!({
                                "state": poll_state(err.kind),
                                "error": { "kind": err.kind, "message": err.message },
                            }))
                        }
                    };
                    match qoder_auth::persist(app, edition, &token) {
                        Ok(saved) => {
                            qoder_auth::complete(&login_id);
                            let mut value = saved.to_json();
                            value["state"] = json!("success");
                            Ok(value)
                        }
                        Err(err) => {
                            qoder_auth::fail(
                                &login_id,
                                qoder_auth::KIND_NETWORK,
                                err.message.clone(),
                            );
                            Ok(json!({
                                "state": "failed",
                                "error": { "kind": "storage", "message": err.message },
                            }))
                        }
                    }
                }
            }
        }

        "qoder.auth.cancel" => {
            let login_id = require_str(params, "loginId")?;
            Ok(json!({ "cancelled": qoder_auth::cancel(&login_id) }))
        }

        // ---- Qoder PAT 备用授权 ----
        //
        // 三条规则决定了这里的形状：
        // 1. **令牌只从请求进、只往钥匙串去**：不回显、不进错误信息、不进数据库。
        // 2. **落库前必须真打一次 userinfo**：拿不到 uid 就不写。
        // 3. **"身份已验证" ≠ "能领 Credits"**：返回体里分开报，后者恒为 unknown。
        "qoder.auth.pat.save" => {
            let edition = require_edition(params)?;
            let raw = require_str(params, "token")?;
            let token =
                qoder_pat::normalize(&raw).map_err(|err| RpcError::invalid_params(err.message))?;
            match qoder_pat::verify(edition, &token).await {
                Ok(identity) => {
                    let saved = qoder_pat::persist(app, edition, &token, &identity)?;
                    let mut value = saved.to_json();
                    value["saved"] = json!(true);
                    value["state"] = json!("verified_identity");
                    Ok(value)
                }
                Err(err) => Ok(json!({
                    "saved": false,
                    "state": "failed",
                    "error": err.to_json(),
                })),
            }
        }

        "qoder.auth.pat.check" => {
            let account_id = require_str(params, "accountId")?;
            let account = app
                .with_db(|db| db.get_qoder_account(&account_id))?
                .ok_or_else(|| RpcError::not_found(format!("Qoder 账号不存在：{account_id}")))?;
            let edition = QoderEdition::parse(&account.edition)
                .ok_or_else(|| RpcError::invalid_params("账号的 edition 非法"))?;
            match qoder_pat::recheck(app, edition, &account_id).await {
                Ok(identity) => Ok(json!({
                    "state": "valid",
                    "identity": {
                        "uid": identity.uid,
                        "name": identity.name,
                        "email": identity.email,
                    },
                })),
                Err(err) => Ok(json!({
                    "state": "failed",
                    "error": err.to_json(),
                })),
            }
        }

        "qoder.auth.pat.remove" => {
            let account_id = require_str(params, "accountId")?;
            let removed = qoder_pat::remove(app, &account_id)?;
            Ok(json!({
                "removed": removed.removed,
                "secretDeleted": removed.secret_deleted,
            }))
        }

        // ---- Qoder 每日 Credits（PR #75）----
        //
        // 查询是只读的、领取是有副作用的、自动领取是**默认关闭**的。
        // 三者分开成不同方法，界面上的按钮也就能分开。
        "qoder.credits.status" => {
            let account_id = require_str(params, "accountId")?;
            crate::credits::status(app, &account_id).await
        }

        "qoder.credits.claim" => {
            let account_id = require_str(params, "accountId")?;
            crate::credits::claim(app, &account_id, "manual").await
        }

        "qoder.credits.runs" => {
            let account_id = params
                .get("accountId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty());
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20) as u32;
            crate::credits::runs(app, account_id, limit)
        }

        "qoder.credits.auto.status" => crate::credits::auto_status(app),

        "qoder.credits.auto.setEnabled" => {
            let account_id = require_str(params, "accountId")?;
            let enabled = params
                .get("enabled")
                .and_then(Value::as_bool)
                .ok_or_else(|| RpcError::invalid_params("缺少参数 enabled（布尔）"))?;
            crate::credits::set_auto(app, &account_id, enabled)
        }

        // ---- 统一任务汇总（PR #76）----
        //
        // 只读本地记录：不进网络、不改状态。界面顶部的数字全部来自这里。
        "tasks.overview" => crate::tasks::overview(app),

        "qoder.credits.auto.tick" => {
            // `force` 只跳过 20 分钟的最小间隔，不绕过账号开关。
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(crate::credits::run_tick(app, force).await)
        }

        // ---- WorkBuddy 扫码登录（PR1b-1）----
        "auth.workbuddy.start" => {
            // 档位必须显式给且严格解析：默认国内版只为兼容旧调用方，
            // 但写错值不能悄悄回落（那会用国内版登录国际账号）。
            let raw = params
                .get("variant")
                .and_then(Value::as_str)
                .unwrap_or("cn");
            let variant = workbuddy_auth::parse_variant(raw)
                .ok_or_else(|| RpcError::invalid_params("variant 只支持 cn / ai"))?;
            let info = workbuddy_auth::start(variant)
                .await
                .map_err(auth_rpc_error)?;
            Ok(json!({
                "platform": "workbuddy",
                "provider": workbuddy_auth::provider_of(variant),
                "variant": variant.as_str(),
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
                workbuddy_auth::Polled::Token(payload) => {
                    // 档位从**会话**里原子取，取不到就说明会话已取消 / 过期 / 结束，
                    // 直接拒绝落库。这里不允许任何"取不到就当国内版"的兜底：
                    // 那会把一次已取消的国际版登录记成国内版账号。
                    let variant = match workbuddy_auth::begin_commit(&login_id) {
                        Ok(variant) => variant,
                        Err(err) => {
                            return Ok(json!({
                                "state": poll_state(err.kind),
                                "error": { "kind": err.kind, "message": err.message },
                            }))
                        }
                    };
                    match persist_login(app, &payload, variant) {
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
                    }
                }
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

        // ---- 成长中心自动管理（PR #68）----
        //
        // 只有三个方法，而且开关默认关闭：没打开时 tick 连只读都不做。
        "growth.auto.status" => Ok(crate::growth::scheduler::auto_status(app)),

        "growth.auto.setEnabled" => {
            let enabled = require_bool(params, "enabled")?;
            crate::growth::scheduler::set_enabled(app, enabled)?;
            log_note(&format!(
                "成长中心自动管理{}",
                if enabled { "已开启" } else { "已关闭" }
            ));
            Ok(crate::growth::scheduler::auto_status(app))
        }

        // `force`：用户点「立即检查」时传 true，绕过间隔判断。
        // 但写操作仍受总开关约束：开关关着就是一次只读检查。
        "growth.auto.tick" => {
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let result = crate::growth::scheduler::run_tick(app, force).await;
            Ok(result)
        }

        // 执行记录：每个任务取最近一条 + 累计到账。
        "growth.runs" => {
            let id = require_str(params, "accountId")?;
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(50)
                .min(200) as u32;
            app.with_db(|db| {
                let latest = db.latest_growth_run_per_task(&id)?;
                let (credit, energy) = db.growth_totals(&id)?;
                Ok(json!({
                    "accountId": id,
                    "latestByTask": latest,
                    "recent": db.growth_runs(&id, limit)?,
                    "grantedCredit": credit,
                    "grantedEnergy": energy,
                }))
            })
        }

        // ---- WorkBuddy 每日签到（PR #69）----
        //
        // 三个只读、三个显式写、一个调度。规则：
        // - `checkin.status` 只查状态，**不提交**；`cached` 时连网络都不碰。
        // - `checkin.run` / `checkin.runPending` 是用户点的按钮，不受自动开关约束，
        //   但仍然先查服务端状态，已签到就返回 already，不会重复提交。
        // - 返回里没有任何令牌，也不接受外部 URL。
        "checkin.status" => {
            let ids = string_list(params, "accountIds");
            let cached = params
                .get("cached")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(crate::checkin::fleet_status(app, &ids, cached).await)
        }

        "checkin.run" => {
            let id = require_str(params, "accountId")?;
            let outcome = crate::checkin::run_one(app, &id).await?;
            log_note(&format!(
                "手动签到 {}：{}",
                id,
                outcome.get("state").and_then(Value::as_str).unwrap_or("?")
            ));
            Ok(outcome)
        }

        "checkin.runPending" => {
            let ids = string_list(params, "accountIds");
            let result = crate::checkin::run_pending(app, &ids).await;
            log_note(&format!(
                "批量签到：{}",
                result.get("summary").and_then(Value::as_str).unwrap_or("?")
            ));
            Ok(result)
        }

        "checkin.setEnabled" => {
            let id = require_str(params, "accountId")?;
            let enabled = require_bool(params, "enabled")?;
            // 专用写入：不走 accounts.upsert，所以不动 auth_epoch（PRD §2）。
            crate::checkin::scheduler::set_enabled(app, &id, enabled)?;
            log_note(&format!(
                "账号 {id} 的自动签到{}",
                if enabled { "已开启" } else { "已关闭" }
            ));
            Ok(json!({
                "accountId": id,
                "enabled": enabled,
                "auto": crate::checkin::scheduler::auto_status(app),
            }))
        }

        "checkin.auto.status" => Ok(crate::checkin::scheduler::auto_status(app)),

        // `force` 只跳过"到点没到点"，**不绕过账号开关**。
        "checkin.auto.tick" => {
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(crate::checkin::scheduler::run_tick(app, force).await)
        }

        "checkin.records" => {
            let id = params.get("accountId").and_then(Value::as_str);
            let limit = params
                .get("limit")
                .and_then(Value::as_u64)
                .unwrap_or(30)
                .min(200) as u32;
            crate::checkin::records(app, id, limit)
        }

        // ---- 四平台额度看板（M4）----
        //
        // 只读：不改账号、不改授权状态、不写 last_used_at。唯一写入是快照缓存。
        // Qoder 两版在这里是 `unverified` 且**零请求**，不是"暂时取不到"。
        "quota.board" => {
            let parsed: crate::quota::BoardParams = serde_json::from_value(params.clone())
                .map_err(|e| RpcError::invalid_params(format!("额度参数非法：{e}")))?;
            crate::quota::board(app, parsed).await
        }

        "quota.history" => {
            let parsed: crate::quota::HistoryParams = serde_json::from_value(params.clone())
                .map_err(|e| RpcError::invalid_params(format!("额度参数非法：{e}")))?;
            crate::quota::history(app, parsed)
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

        "auth.magpie.relink" => {
            let account_id = params
                .get("accountId")
                .and_then(Value::as_str)
                .map(str::to_string);
            crate::auth::relink::relink_pending(app, account_id.as_deref())
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
pub(crate) fn superseded(reason: &str) -> RpcError {
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
    // 国际版：探测端点、请求头、账号域全都不一样，用国内版探针去问只会把
    // 一份好授权误判成 invalid。**不取凭据、不发请求、不改任何状态。**
    if !account.supports_workbuddy_cn_apis() {
        return Ok(json!({
            "state": "unsupported",
            "provider": account.provider,
            "error": {
                "kind": "unsupported_provider",
                "message": "WorkBuddy AI（国际版）的授权检查接口尚未接入；AICC 没有向任何接口发送请求",
            },
            "authorizationSummary": broker::summary(app, &account),
        }));
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

/// `qoder.accounts.list`：Qoder CN / Global 平台账号 + 各自的授权来源。
///
/// 与 `qoder.profiles.list` 是两件事：这里是在线服务账号（不依赖客户端），
/// 那边是本机客户端的认证快照。界面上必须分开显示，不能互相顶替。
fn qoder_accounts_list(app: &App, params: &Value) -> RpcResult<Value> {
    let edition = params
        .get("edition")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if let Some(edition) = edition {
        if !crate::storage::qoder_accounts::EDITIONS.contains(&edition) {
            return Err(RpcError::invalid_params(
                "edition 只支持 qoder_cn / qoder_global",
            ));
        }
    }
    let accounts = app.with_db(|db| db.list_qoder_accounts(edition))?;
    let mut items = Vec::with_capacity(accounts.len());
    for account in accounts {
        let authorizations = app.with_db(|db| db.list_qoder_authorizations(&account.id))?;
        let mut value = account.to_json();
        value["authorizations"] = json!(authorizations
            .iter()
            .map(|row| row.to_json())
            .collect::<Vec<_>>());
        value["sources"] = json!(authorizations
            .iter()
            .map(|row| row.source.clone())
            .collect::<Vec<_>>());
        items.push(value);
    }
    Ok(json!({ "accounts": items }))
}

/// 列出 Magpie 里的四平台账号，并标出哪些已经在 AICC 里。
///
/// 判定"已导入"按平台分两套：
/// - WorkBuddy（国内版 / 国际版）：`accounts` 表的 `(provider, uid)`；
/// - Qoder（CN / Global）：`qoder_accounts` 表的 `(edition, uid)`。
///
/// **只读**：这里不写任何表，也不碰 Magpie 的文件。
fn magpie_preview(app: &App) -> Value {
    let mut report = magpie::preview();
    if report.get("available").and_then(Value::as_bool) != Some(true) {
        return report;
    }

    // WorkBuddy：provider + uid → (id, 实际使用的来源)
    let workbuddy: std::collections::HashMap<(String, String), (String, String)> = app
        .with_db(|db| {
            db.list_accounts(&AccountFilter {
                platform: Some("workbuddy".to_string()),
                ..Default::default()
            })
        })
        .unwrap_or_default()
        .into_iter()
        .map(|a| ((a.provider.clone(), a.uid.clone()), (a.id, a.auth_source)))
        .collect();

    // Qoder：edition + uid → 账号 id
    let qoder: std::collections::HashMap<(String, String), String> = app
        .with_db(|db| db.list_qoder_accounts(None))
        .unwrap_or_default()
        .into_iter()
        .map(|a| ((a.edition.clone(), a.uid.clone()), a.id))
        .collect();

    if let Some(accounts) = report.get_mut("accounts").and_then(Value::as_array_mut) {
        for item in accounts.iter_mut() {
            let uid = item.get("uid").and_then(Value::as_str).unwrap_or_default();
            let platform = item
                .get("platform")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let is_qoder = platform.starts_with("qoder");
            let hit = if is_qoder {
                let edition = item
                    .get("edition")
                    .and_then(Value::as_str)
                    .unwrap_or(platform);
                qoder
                    .get(&(edition.to_string(), uid.to_string()))
                    .map(|id| (id.clone(), "magpie".to_string()))
            } else {
                let provider = item
                    .get("provider")
                    .and_then(Value::as_str)
                    .unwrap_or("workbuddy");
                workbuddy
                    .get(&(provider.to_string(), uid.to_string()))
                    .cloned()
            };
            item["imported"] = json!(hit.is_some());
            item["importedSource"] = json!(hit.as_ref().map(|(_, source)| source.clone()));
            item["accountId"] = json!(hit.map(|(id, _)| id));
            item["importTarget"] = json!(if is_qoder {
                "qoder_accounts"
            } else {
                "accounts"
            });
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
        // Qoder（CN / Global）走自己的两张表：不进 accounts、不碰钥匙串。
        // 过期也照样登记，界面上标"需重新登录"比"账号凭空消失"更好用。
        if login.edition().is_some() {
            match import_qoder_login(app, &login) {
                Ok(value) => imported.push(value),
                Err(reason) => skipped.push(reason),
            }
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
        let provider = login.account_provider().unwrap_or("workbuddy");
        if let Some(record) = &existing {
            if record.provider != provider {
                skipped.push(json!({
                    "key": login.key,
                    "uid": login.uid,
                    "reason": "uid_conflict",
                    "message": format!(
                        "同一 UID 已属于 {}，不能再作为 {} 导入；两个服务是不同账号",
                        record.provider, provider
                    ),
                }));
                continue;
            }
        }
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
                    .unwrap_or_else(|| magpie::short_uid_for(login.provider.label, &login.uid)),
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
                provider: Some(provider.to_string()),
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
            &login.key,
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

/// 把一条 Magpie 的 Qoder 账号登记进 `qoder_accounts` / `qoder_authorizations`。
///
/// 三条硬规则：
/// 1. **不复制令牌**。`magpie` 来源的行里 `secret_ref` 恒为空，用的时候重新读
///    Magpie 的当前文件；Magpie 的 refresh token 只归 Magpie 刷新。
/// 2. 不覆盖另一个来源。同一个账号的 `own_browser` 授权行完全不受影响。
/// 3. 昵称只在为空时补：用户改过的名字不会被下一次导入冲掉（在存储层实现）。
fn import_qoder_login(app: &App, login: &magpie::MagpieLogin) -> Result<Value, Value> {
    let edition = login.edition().ok_or_else(|| {
        json!({ "key": login.key, "reason": "unsupported_platform", "message": "不是 Qoder 账号" })
    })?;
    let expires_at = login.expires_at_ms.and_then(workbuddy_auth::ms_to_rfc3339);
    let status = if login
        .expires_at_ms
        .map(|at| at <= magpie::now_ms())
        .unwrap_or(false)
    {
        "expired"
    } else {
        "valid"
    };
    let nickname = login
        .nickname
        .clone()
        .unwrap_or_else(|| magpie::short_uid_for(login.provider.label, &login.uid));

    let account = app
        .with_db(|db| db.upsert_qoder_account(edition, &login.uid, Some(&nickname), None))
        .map_err(|err| {
            json!({
                "key": login.key,
                "uid": login.uid,
                "reason": "storage",
                "message": crate::rpc::sanitize(&err.message),
            })
        })?;

    let authorization = app
        .with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                crate::storage::qoder_accounts::SOURCE_MAGPIE,
                Some(&login.key),
                None,
                status,
                expires_at.as_deref(),
                None,
            )
        })
        .map_err(|err| {
            json!({
                "key": login.key,
                "uid": login.uid,
                "reason": "storage",
                "message": crate::rpc::sanitize(&err.message),
            })
        })?;

    Ok(json!({
        "account": account.to_json(),
        "authorization": authorization.to_json(),
        "platform": login.platform(),
        "edition": edition,
        "provider": Value::Null,
    }))
}

/// Magpie 账号不用 refresh token，改为"重新从 Magpie 同步一次 access token"。
///
/// 同步函数：只做「读 Magpie 文件 → 校验关联 → 更新 magpie 那一行」，没有任何 await。
/// 旅行调度器也要在非 async 的凭据钩子里调它，所以不能挂在 async fn 上。
///
/// 三条判据必须同时成立，缺一不可：
///
/// 1. **来源键**：行里有 `source_key` 就只认那一条登录态；没有时按
///    「平台 + provider + UID」找，**唯一**匹配才回填（PRD §3.3 不许猜）。
/// 2. **平台 + UID**：同 UID 的 WorkBuddy CN / AI 是两条不同登录态，拿错就会串号。
/// 3. **文件可读性**：读不到 ≠ 过期。文件读不了时保持授权行原状、返回可重试的错误，
///    绝不把状态写成 `expired`（那会让 Broker 之后一直跳过它）。
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
    let Some(row) =
        app.with_db(|db| db.find_authorization(&current.id, crate::auth::ids::KIND_MAGPIE))?
    else {
        return Err(RpcError::new(
            crate::rpc::E_AUTH,
            "该账号没有 Magpie 授权，不能从 Magpie 覆盖凭据",
        )
        .with_data(json!({ "kind": "not_magpie_source" })));
    };

    // 文件读不了：环境问题。保持原状态，让上层可以重试。
    let logins = match magpie::read_logins() {
        Ok(logins) => logins,
        Err(err) => {
            return Ok(unavailable_report(&err));
        }
    };
    let now = magpie::now_ms();

    let login = match resolve_login_for(&logins, &current, &row, now) {
        Ok(login) => login,
        Err(ResolveProblem::Expired) => {
            // 这条是真的过期了：标 expired 是对的，Magpie 刷新后会自动恢复。
            let _ = crate::auth::service::record_status(
                app,
                &row,
                "expired",
                None,
                Some("magpie_token_expired"),
            );
            return Ok(json!({
                "state": "expired",
                "source": magpie::SOURCE_MAGPIE,
                "error": {
                    "kind": "magpie_token_expired",
                    "message": "Magpie 里的令牌也已过期。请在 Magpie 重新登录，或改用扫码授权。",
                },
            }));
        }
        Err(problem) => {
            // 串号 / 来源键不见了 / 多个候选：都不是"过期"，不许改状态。
            return Ok(json!({
                "state": "needs_relink",
                "source": magpie::SOURCE_MAGPIE,
                "error": {
                    "kind": problem.as_str(),
                    "message": problem.message(),
                },
            }));
        }
    };

    let expires = login.expires_at_ms.and_then(workbuddy_auth::ms_to_rfc3339);
    // 守卫用的是**读文件之前**那一行的版本：读完文件、写完库之间如果有人改过
    // （重新导入 / relink / 另一个 Worker），这次写入会被 CAS 拒掉，而不是把新关联覆盖回去。
    let saved = crate::auth::service::save_magpie_guarded(
        app,
        &current.id,
        &current.uid,
        &login.key,
        expires.as_deref(),
        Some(crate::auth::service::MagpieGuard::of(&row)),
    )?;
    Ok(json!({
        "state": "valid",
        "resynced": true,
        "tokenExpiresAt": saved.authorization.token_expires_at,
        "source": magpie::SOURCE_MAGPIE,
        "degraded": saved.degraded,
    }))
}

/// 文件读不了时的报告：不写库、不改状态。
fn unavailable_report(err: &magpie::MagpieError) -> Value {
    json!({
        "state": "unavailable",
        "source": magpie::SOURCE_MAGPIE,
        "error": {
            "kind": "magpie_unavailable",
            "message": format!("Magpie 登录态读不了（{}），请稍后重试或检查 Magpie", crate::rpc::sanitize(&err.message())),
        },
    })
}

/// 重同步为什么选不出登录态。分类型是因为**处理的后果不一样**：
/// 只有 `Expired` 可以写 `expired`，其余都必须保持原状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolveProblem {
    Expired,
    SourceMissing,
    PlatformMismatch,
    UidMismatch,
    Ambiguous,
}

impl ResolveProblem {
    fn as_str(self) -> &'static str {
        match self {
            Self::Expired => "magpie_token_expired",
            Self::SourceMissing => "magpie_link_missing",
            Self::PlatformMismatch => "magpie_platform_mismatch",
            Self::UidMismatch => "magpie_uid_mismatch",
            Self::Ambiguous => "magpie_relink_required",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::Expired => "Magpie 里的令牌已过期，请在 Magpie 重新登录",
            Self::SourceMissing => "Magpie 里找不到这条登录态，请重新关联",
            Self::PlatformMismatch => "这条关联指向另一个平台（国内版 / 国际版），请重新关联",
            Self::UidMismatch => "这条关联指向另一个账号，请重新关联",
            Self::Ambiguous => "Magpie 里有多个同 UID 的登录态，无法确定关联到哪一个",
        }
    }
}

/// 选登录态：有来源键就认它，没有就要求唯一匹配。
fn resolve_login_for<'a>(
    logins: &'a [magpie::MagpieLogin],
    account: &crate::storage::AccountRecord,
    row: &crate::storage::AuthorizationRecord,
    now_ms: i64,
) -> Result<&'a magpie::MagpieLogin, ResolveProblem> {
    if let Some(source_key) = row
        .source_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
    {
        return crate::auth::magpie_link::find_link(logins, account, source_key, Some(now_ms))
            .map_err(|problem| match problem {
                crate::auth::magpie_link::LinkProblem::Expired => ResolveProblem::Expired,
                crate::auth::magpie_link::LinkProblem::SourceMissing => {
                    ResolveProblem::SourceMissing
                }
                crate::auth::magpie_link::LinkProblem::PlatformMismatch => {
                    ResolveProblem::PlatformMismatch
                }
                crate::auth::magpie_link::LinkProblem::UidMismatch => ResolveProblem::UidMismatch,
                crate::auth::magpie_link::LinkProblem::FileUnavailable => {
                    ResolveProblem::SourceMissing
                }
            });
    }

    // 没有来源键：只在唯一匹配时回填，两个候选就报歧义。
    let candidates: Vec<&magpie::MagpieLogin> = logins
        .iter()
        .filter(|login| crate::auth::magpie_link::platform_matches(login, account))
        .filter(|login| login.uid == account.uid)
        .collect();
    let live: Vec<&magpie::MagpieLogin> = candidates
        .iter()
        .copied()
        .filter(|login| login.expires_at_ms.map(|at| at > now_ms).unwrap_or(true))
        .collect();
    match live.len() {
        1 => Ok(live[0]),
        0 if !candidates.is_empty() => Err(ResolveProblem::Expired),
        0 => Err(ResolveProblem::SourceMissing),
        _ => Err(ResolveProblem::Ambiguous),
    }
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
fn persist_login(
    app: &App,
    payload: &workbuddy_auth::TokenPayload,
    variant: WbVariant,
) -> RpcResult<Value> {
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

    let (mut value, existed) = match write_login_row(app, payload, variant) {
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
fn write_login_row(
    app: &App,
    payload: &workbuddy_auth::TokenPayload,
    variant: WbVariant,
) -> RpcResult<(Value, bool)> {
    let uid = payload.profile.uid.clone();
    let domain = Some(payload.domain.clone()).filter(|d| !d.is_empty());
    // 档位决定 provider：两个站点共用 platform，只有这一列能区分。
    let provider = workbuddy_auth::provider_of(variant);

    let (existed, record) = app.with_db(|db| {
        // 同一个 UID 已经属于另一个站点：明确冲突，不合并、不改写。
        let existing = db.find_account("workbuddy", &uid)?;
        if let Some(record) = &existing {
            if record.provider != provider {
                return Err(RpcError::invalid_params(format!(
                    "同一 UID 已属于 {}，不能再作为 {provider} 登录；两个服务是不同账号",
                    record.provider
                ))
                .with_data(json!({
                    "code": "uid_conflict",
                    "existingProvider": record.provider,
                    "requestedProvider": provider,
                    "uid": uid,
                })));
            }
        }
        let existed = existing.is_some();
        let record = db.upsert_account(&AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.clone(),
            nickname: payload.profile.nickname.clone(),
            email: payload.profile.email.clone(),
            auth_domain: domain.clone(),
            provider: Some(provider.to_string()),
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
    // 刷新也是国内版协议（`WbVariant::Cn` 的 token 端点 + 国内 domain）。
    // 国际版走这里只会浪费一次请求，失败时还会把一份好授权标成 invalid。
    // **不查授权、不读钥匙串、不改状态。**
    if !account.supports_workbuddy_cn_apis() {
        return Ok(json!({
            "state": "unsupported",
            "provider": account.provider,
            "error": {
                "kind": "unsupported_provider",
                "message": "WorkBuddy AI（国际版）的令牌刷新接口尚未接入；AICC 没有发送请求，也没有改动授权状态",
            },
        }));
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

/// 必须显式给出、且只能是两个合法版本之一。
///
/// **绝不回落**到任何一个默认版本：那会用错站点去验凭据，
/// 把一次国内版操作记成国际版（或反过来）。
fn require_edition(params: &Value) -> RpcResult<QoderEdition> {
    let raw = require_str(params, "edition")?;
    QoderEdition::parse(&raw)
        .ok_or_else(|| RpcError::invalid_params("edition 只支持 qoder_cn / qoder_global"))
}

/// 可选的字符串数组参数。缺省 / `null` / 类型不对都当空数组（= 全部）。
fn string_list(params: &Value, key: &str) -> Vec<String> {
    params
        .get(key)
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
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
            provider: magpie::provider_of("workbuddy").unwrap(),
            uid: uid.to_string(),
            nickname: Some("借来的".to_string()),
            domain: "www.workbuddy.cn".to_string(),
            access_token: access.to_string(),
            expires_at_ms: Some(1_795_525_945_623),
        }
    }

    /// `auth.workbuddy.refresh` 也是国内版协议：国际版必须**在查授权之前**被挡住，
    /// 而且不能因为一次"刷新"把一份好授权标成 invalid。
    /// 档位解析必须严格：写错值不能被"回落国内版"掩盖。
    #[test]
    fn variant_parsing_is_strict() {
        use wb_switch_core::modules::variant::WbVariant as V;
        assert_eq!(workbuddy_auth::parse_variant("cn"), Some(V::Cn));
        assert_eq!(workbuddy_auth::parse_variant(" CN "), Some(V::Cn));
        assert_eq!(workbuddy_auth::parse_variant("ai"), Some(V::Ai));
        assert_eq!(workbuddy_auth::parse_variant("intl"), Some(V::Ai));
        assert_eq!(workbuddy_auth::parse_variant("bogus"), None);
        assert_eq!(workbuddy_auth::parse_variant(""), None);
    }

    #[test]
    fn provider_mapping_matches_the_v13_model() {
        use wb_switch_core::modules::variant::WbVariant as V;
        assert_eq!(workbuddy_auth::provider_of(V::Cn), "workbuddy");
        assert_eq!(workbuddy_auth::provider_of(V::Ai), "workbuddy_ai");
    }

    /// 国际版也要能"Magpie 借用的授权 + 自有登录授权"并存：
    /// 登录只写 own，导入的 magpie/access 一个字节都不动。
    #[test]
    fn ai_account_keeps_magpie_and_own_authorizations_side_by_side() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "ai-dual-uid";
        import_logins(
            &app,
            vec![magpie_login_with("workbuddy-ai", uid, "borrowed-ai")],
            &[],
        )
        .expect("import");
        let record = account(&app, uid);
        assert_eq!(record.provider, "workbuddy_ai");

        persist_login(
            &app,
            &payload(uid, "own-access-ai", Some("own-refresh-ai")),
            WbVariant::Ai,
        )
        .expect("browser login");

        let want_ref = format!("magpie:v1:{}", "workbuddy-ai");
        assert_eq!(
            magpie_ref(&app, &record.id).as_deref(),
            Some(want_ref.as_str()),
            "自有登录不能覆盖借用授权"
        );
        assert!(
            magpie_access(&app, &record.id).is_none(),
            "借用授权不进钥匙串"
        );
        assert_eq!(
            own_access(&app, &record.id).as_deref(),
            Some("own-access-ai")
        );
        let kinds: Vec<String> = app
            .with_db(|db| db.list_authorizations(&record.id))
            .unwrap()
            .into_iter()
            .map(|row| row.kind)
            .collect();
        assert!(kinds.contains(&"own".to_string()), "{kinds:?}");
        assert!(kinds.contains(&"magpie".to_string()), "{kinds:?}");
        assert_eq!(account(&app, uid).provider, "workbuddy_ai");
    }

    /// `auth.workbuddy.start` 的 variant 参数在**发请求之前**就要校验：
    /// 这里用非法值调用真实派发入口，必须直接报参数错，不会有任何网络行为。
    #[tokio::test]
    async fn start_rejects_an_unknown_variant_without_a_request() {
        let (app, _dir) = test_app(SecureStore::memory());
        let err = dispatch(&app, "auth.workbuddy.start", &json!({ "variant": "lite" }))
            .await
            .unwrap_err();
        assert!(err.message.contains("variant"), "{}", err.message);
    }

    /// 国际版登录：账号记成 `workbuddy_ai`，凭据进的是同一套按 uid 隔离的键；
    /// 同一 UID 换档位登录必须冲突并**回滚**已写入的凭据。
    #[test]
    fn ai_login_marks_the_provider_and_refuses_to_cross_variants() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "ai-login-uid";
        persist_login(
            &app,
            &payload(uid, "ai-access", Some("ai-refresh")),
            WbVariant::Ai,
        )
        .expect("ai login");

        let record = account(&app, uid);
        assert_eq!(record.provider, "workbuddy_ai");
        assert_eq!(record.platform, "workbuddy");
        assert_eq!(own_access(&app, &record.id).as_deref(), Some("ai-access"));

        // 同一个 UID 用国内版再登录：明确冲突。
        let err = persist_login(
            &app,
            &payload(uid, "cn-access", Some("cn-refresh")),
            WbVariant::Cn,
        )
        .unwrap_err();
        assert_eq!(err.data.as_ref().unwrap()["code"], json!("uid_conflict"));

        // 冲突发生在凭据写入之后，必须回滚回去。
        assert_eq!(
            own_access(&app, &record.id).as_deref(),
            Some("ai-access"),
            "冲突时不能把已有凭据覆盖掉"
        );
        assert_eq!(own_refresh(&app, &record.id).as_deref(), Some("ai-refresh"));
        assert_eq!(account(&app, uid).provider, "workbuddy_ai");
    }

    #[tokio::test]
    async fn ai_account_refresh_is_unsupported_and_keeps_the_authorization() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "ai-refresh-uid";
        app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: uid.to_string(),
                provider: Some("workbuddy_ai".to_string()),
                ..Default::default()
            })
        })
        .expect("account");
        // 国际版登录走 `WbVariant::Ai`：provider 由档位决定，不是继承来的。
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Ai,
        )
        .expect("persist login");

        let record = account(&app, uid);
        assert_eq!(record.provider, "workbuddy_ai");
        let before = app
            .with_db(|db| db.find_authorization(&record.id, ids::KIND_OWN))
            .unwrap()
            .expect("own authorization");

        let out = refresh_account(&app, &record.id).await.expect("refresh");

        assert_eq!(out["state"], json!("unsupported"));
        assert_eq!(out["error"]["kind"], json!("unsupported_provider"));
        let after = app
            .with_db(|db| db.find_authorization(&record.id, ids::KIND_OWN))
            .unwrap()
            .expect("own authorization");
        assert_eq!(after.status, before.status, "没发请求就不许把授权标成失效");
        assert_eq!(after.version, before.version, "授权没有被写过");
        assert_eq!(
            own_access(&app, &record.id).as_deref(),
            Some("own-access"),
            "钥匙串里的令牌一个字节都没动"
        );
        assert_eq!(
            own_refresh(&app, &record.id).as_deref(),
            Some("own-refresh")
        );
    }

    #[tokio::test]
    async fn ai_account_status_is_unsupported_and_changes_nothing() {
        let (app, _dir) = test_app(SecureStore::memory());
        let record = app
            .with_db(|db| {
                db.upsert_account(&AccountInput {
                    platform: "workbuddy".to_string(),
                    uid: "ai-status-uid".to_string(),
                    provider: Some("workbuddy_ai".to_string()),
                    ..Default::default()
                })
            })
            .expect("account");
        let before = record.auth_status.clone();

        let out = workbuddy_account_status(&app, &record.id)
            .await
            .expect("status");

        assert_eq!(out["state"], json!("unsupported"));
        assert_eq!(out["error"]["kind"], json!("unsupported_provider"));
        assert_eq!(out["provider"], json!("workbuddy_ai"));
        let after = app
            .with_db(|db| db.get_account(&record.id))
            .unwrap()
            .unwrap();
        assert_eq!(after.auth_status, before, "没发请求就不许改授权状态");
    }

    fn magpie_login_with(key: &str, uid: &str, access: &str) -> magpie::MagpieLogin {
        magpie::MagpieLogin {
            key: key.to_string(),
            provider: magpie::provider_of(key).expect("provider"),
            uid: uid.to_string(),
            nickname: None,
            domain: String::new(),
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

    fn magpie_ref(app: &App, account_id: &str) -> Option<String> {
        app.with_db(|db| db.find_authorization(account_id, ids::KIND_MAGPIE))
            .ok()
            .flatten()
            .map(|row| row.access_ref)
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
        let out = persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("persist");
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

    /// PR-CRED-2 起导入**只登记引用**：钥匙串里既没有新键也没有旧别名。
    #[test]
    fn magpie_import_registers_a_reference_and_writes_no_keychain_key() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-2222-3333-4444-555566667777";
        let out = import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        assert_eq!(out["imported"], json!(1), "{out}");

        let acc = account(&app, uid);
        let row = app
            .with_db(|db| db.find_authorization(&acc.id, ids::KIND_MAGPIE))
            .expect("query")
            .expect("row");
        assert_eq!(row.storage_kind(), "magpie");
        assert_eq!(row.access_ref, format!("magpie:v1:workbuddy#{uid}"));
        assert_eq!(
            row.source_key.as_deref(),
            Some(format!("workbuddy#{uid}").as_str())
        );

        assert!(
            magpie_access(&app, &acc.id).is_none(),
            "不得把 Magpie 令牌复制进钥匙串"
        );
        assert!(
            legacy_access(&app, uid).is_none(),
            "借用授权不写旧别名：那又是一次钥匙串写入"
        );
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

    /// 借用授权的令牌**每次现读**：Magpie 刷新之后，AICC 下一次请求就是新值。
    #[test]
    fn borrowed_access_is_read_fresh_on_every_use() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-2222-0001-0002-00000000000b";
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed-1")]));
        import_logins(&app, vec![magpie_login(uid, "borrowed-1")], &[]).expect("import");
        let acc = account(&app, uid);

        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => assert_eq!(resolved.access, "borrowed-1"),
            broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
        }

        // 模拟 Magpie 自己刷新了令牌：AICC 不做任何写入就该读到新值。
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed-2")]));
        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => assert_eq!(
                resolved.access, "borrowed-2",
                "动态关联必须现读，不能缓存旧令牌"
            ),
            broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
        }
        assert!(
            magpie_access(&app, &acc.id).is_none(),
            "读到新令牌也不许把它复制进钥匙串"
        );
        magpie::test_set_logins(None);
    }

    /// P0-1 回归：库里的过期时间是**旧快照**，不能拿它挡住 Magpie 刷新后的新令牌。
    #[test]
    fn a_stale_db_expiry_does_not_block_a_refreshed_magpie_token() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-2222-0001-0002-00000000000f";

        let stale = magpie_login(uid, "borrowed-old");
        magpie::test_set_logins(Some(vec![stale.clone()]));
        import_logins(&app, vec![stale], &[]).expect("import");
        let acc = account(&app, uid);

        // 时间跨过导入时那个有效期：库里的状态与到期时间都写成"已过期"。
        app.with_db(|db| {
            db.exec_raw(
                "UPDATE account_authorizations
                    SET status = 'expired', token_expires_at = '2000-01-01T00:00:00Z'
                  WHERE kind = 'magpie'",
            )
        })
        .expect("mark stale");

        // Magpie 自己刷新了：文件里是新的、有效期到未来。AICC 不做任何导入。
        let fresh = magpie_login(uid, "borrowed-new");
        magpie::test_set_logins(Some(vec![fresh]));

        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => assert_eq!(
                resolved.access, "borrowed-new",
                "旧快照的过期时间不许挡住动态读取"
            ),
            broker::Outcome::Unavailable { reason, .. } => {
                panic!("不该不可用：{reason}（旧状态把新令牌锁死了）")
            }
        }

        // 界面要能看出"库里过期、实际可用"。
        let described = broker::describe(&app, &acc.id).expect("describe");
        assert_eq!(described[0]["status"], json!("expired"));
        assert_eq!(described[0]["effectiveStatus"], json!("valid"));
        assert_eq!(described[0]["recoverable"], json!(true));
        magpie::test_set_logins(None);
    }

    /// P1 回归：Magpie 文件读不了 ≠ 没有凭据，reason 必须单独分类。
    #[test]
    fn an_unreadable_magpie_file_reports_unavailable_not_missing_credential() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-2222-0001-0002-000000000010";
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let acc = account(&app, uid);

        // 只留借用授权，再让文件"读不了"。
        magpie::test_set_unreadable();
        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => panic!("不该有令牌：{}", resolved.access),
            broker::Outcome::Unavailable { reason, detail } => {
                assert_eq!(reason, "magpie_unavailable", "{detail}");
            }
        }
        // 状态不能被写成 expired（文件问题不是授权问题）。
        let row = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "valid", "读不到文件不许改授权状态");
    }

    /// P0-2 回归（a）：同 UID 两个 provider，必须按来源键 / provider 选，不能只看 UID。
    #[test]
    fn resync_picks_the_login_matching_provider_and_source_key() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "shared-uid-1";
        let ai = magpie_login_with("workbuddy-ai#shared-uid-1", uid, "ai-token");
        magpie::test_set_logins(Some(vec![ai.clone()]));
        import_logins(&app, vec![ai], &[]).expect("import");
        let acc = account(&app, uid);
        assert_eq!(acc.provider, "workbuddy_ai");

        // 文件里同时有国内版与同 UID 的国际版：国内版排前面。
        let cn = magpie_login_with("workbuddy#shared-uid-1", uid, "cn-token");
        magpie::test_set_logins(Some(vec![
            cn,
            magpie_login_with("workbuddy-ai#shared-uid-1", uid, "ai-token-2"),
        ]));

        let out = resync_magpie_access(&app, &acc).expect("resync");
        assert_eq!(out["state"], json!("valid"), "{out}");
        let row = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .unwrap();
        assert_eq!(
            row.access_ref, "magpie:v1:workbuddy-ai#shared-uid-1",
            "不能被同 UID 的国内版覆盖"
        );
        assert_eq!(row.source_key.as_deref(), Some("workbuddy-ai#shared-uid-1"));
        magpie::test_set_logins(None);
    }

    /// P0-2 回归（b）：文件暂时不可读 → 不改状态；恢复之后立刻可用。
    #[test]
    fn resync_keeps_the_row_intact_while_magpie_is_unreadable() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-2222-0001-0002-000000000011";
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let acc = account(&app, uid);

        magpie::test_set_unreadable();
        let out = resync_magpie_access(&app, &acc).expect("resync");
        assert_eq!(out["state"], json!("unavailable"), "{out}");
        assert_eq!(out["error"]["kind"], json!("magpie_unavailable"));
        let row = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .unwrap();
        assert_eq!(row.status, "valid", "文件读不了不许把状态写成 expired");
        assert_eq!(row.access_ref, format!("magpie:v1:workbuddy#{uid}"));

        // 文件回来：不需要任何重新导入就能恢复。
        magpie::test_reset();
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed-again")]));
        let out = resync_magpie_access(&app, &acc).expect("resync again");
        assert_eq!(out["state"], json!("valid"), "{out}");
        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => assert_eq!(resolved.access, "borrowed-again"),
            broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
        }
        magpie::test_set_logins(None);
    }

    /// Magpie 日志里换成别的 UID：这是串号，必须报出来，不能当"没凭据"糊过去。
    #[test]
    fn a_link_pointing_at_another_account_is_reported_as_mismatch() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-2222-0001-0002-00000000000c";
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let acc = account(&app, uid);

        // 同一个来源键，但文件里那条登录态属于别人。
        magpie::test_set_logins(Some(vec![magpie_login_with(
            &format!("workbuddy#{uid}"),
            "someone-else",
            "stolen",
        )]));
        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => {
                panic!("不能拿别人的令牌：{}", resolved.access)
            }
            broker::Outcome::Unavailable { reason, .. } => {
                assert_eq!(reason, "link_mismatch", "串号要有独立分类");
            }
        }
        magpie::test_set_logins(None);
    }

    /// Magpie 没装 / 文件丢了：按既有规则兜底到自有授权，并把原因带上。
    #[test]
    fn unavailable_magpie_falls_back_to_the_own_authorization() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-2222-0001-0002-00000000000d";
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(&app, &payload(uid, "own-access", None), WbVariant::Cn).expect("login");
        let acc = account(&app, uid);

        // 文件突然读不了了。
        magpie::test_set_logins(None);
        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => {
                assert_eq!(resolved.kind, "own", "首选不可用要兜底");
                assert!(resolved.degraded.is_some(), "兜底必须说明原因");
            }
            broker::Outcome::Unavailable { reason, .. } => panic!("不该整体不可用：{reason}"),
        }
        // 借用授权那一行不能被误删：文件暂时不可读不等于账号没授权。
        assert!(app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .is_some());
    }

    /// v20 重新关联：旧钥匙串引用 → `magpie:v1:`，旧键保留可回滚。
    #[test]
    fn relink_moves_a_legacy_keychain_ref_to_the_dynamic_link() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-2222-0001-0002-00000000000e";
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let acc = account(&app, uid);

        // 退回 v19 的形状：引用指向钥匙串，里面躺着当年复制进去的令牌。
        let key = ids::access_key(&acc.id, ids::KIND_MAGPIE);
        app.secure.set(&key, b"legacy-token").unwrap();
        app.with_db(|db| {
            db.exec_raw(&format!(
                "UPDATE account_authorizations
                    SET access_ref = 'keychain:{key}', ref_updated_at = NULL
                  WHERE account_id = '{}' AND kind = 'magpie'",
                acc.id
            ))
        })
        .expect("rewind to v19");
        let before = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .unwrap();
        assert_eq!(before.storage_kind(), "keychain");

        let out = crate::auth::relink::relink_pending(&app, None).expect("relink");
        assert_eq!(out["relinked"], json!(1), "{out}");
        assert_eq!(out["pending"], json!(0), "{out}");
        assert_eq!(out["legacyKeysKept"], json!(true), "不许自动删旧键");

        let after = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .unwrap();
        assert_eq!(after.storage_kind(), "magpie");
        assert_eq!(after.access_ref, format!("magpie:v1:workbuddy#{uid}"));
        assert!(after.version > before.version, "换引用要推进版本");
        assert_eq!(
            read_key(&app, &key).as_deref(),
            Some("legacy-token"),
            "旧钥匙串条目原样保留"
        );

        // 幂等：再跑一次没有可改的行。
        let again = crate::auth::relink::relink_pending(&app, None).expect("relink again");
        assert_eq!(again["relinked"], json!(0), "{again}");
        assert_eq!(again["pending"], json!(0), "{again}");
        magpie::test_set_logins(None);
    }

    #[test]
    fn qoder_accounts_go_to_their_own_tables_without_a_secret() {
        let (app, _dir) = test_app(SecureStore::memory());
        let out = import_logins(
            &app,
            vec![magpie_login_with("qoder-cn", "qn-uid-1", "qd-access")],
            &[],
        )
        .expect("import");
        assert_eq!(out["imported"], json!(1), "{out}");

        // accounts 表里不能出现 Qoder 平台服务账号。
        assert!(
            app.with_db(|db| db.list_accounts(&AccountFilter::default()))
                .unwrap()
                .is_empty(),
            "Qoder 账号不能写进 accounts"
        );

        let accounts = app.with_db(|db| db.list_qoder_accounts(None)).unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].edition, "qoder_cn");

        let auths = app
            .with_db(|db| db.list_qoder_authorizations(&accounts[0].id))
            .unwrap();
        assert_eq!(auths.len(), 1);
        assert_eq!(auths[0].source, "magpie");
        assert_eq!(auths[0].source_key.as_deref(), Some("qoder-cn"));
        assert!(auths[0].secret_ref.is_none(), "magpie 来源不许持有凭据");
        assert_eq!(
            out["accounts"][0]["authorization"]["hasSecret"],
            json!(false)
        );
    }

    #[test]
    fn qoder_global_and_cn_are_separate_accounts() {
        let (app, _dir) = test_app(SecureStore::memory());
        import_logins(
            &app,
            vec![
                magpie_login_with("qoder", "shared-uid", "a"),
                magpie_login_with("qoder-cn", "shared-uid", "b"),
            ],
            &[],
        )
        .unwrap();
        let accounts = app.with_db(|db| db.list_qoder_accounts(None)).unwrap();
        assert_eq!(accounts.len(), 2, "同一 UID 在两个版本是两个账号");
    }

    #[test]
    fn workbuddy_ai_import_marks_the_provider() {
        let (app, _dir) = test_app(SecureStore::memory());
        let out = import_logins(
            &app,
            vec![magpie_login_with("workbuddy-ai", "ai-uid-1", "ai-access")],
            &[],
        )
        .expect("import");
        assert_eq!(out["imported"], json!(1), "{out}");
        let record = account(&app, "ai-uid-1");
        assert_eq!(record.platform, "workbuddy");
        assert_eq!(record.provider, "workbuddy_ai");
    }

    #[test]
    fn same_uid_in_the_other_provider_is_a_conflict_not_a_merge() {
        let (app, _dir) = test_app(SecureStore::memory());
        import_logins(
            &app,
            vec![magpie_login_with("workbuddy", "same-uid", "cn")],
            &[],
        )
        .unwrap();
        let out = import_logins(
            &app,
            vec![magpie_login_with("workbuddy-ai", "same-uid", "ai")],
            &[],
        )
        .unwrap();
        assert_eq!(out["imported"], json!(0), "{out}");
        assert_eq!(out["skipped"][0]["reason"], json!("uid_conflict"));
        let accounts = app
            .with_db(|db| db.list_accounts(&AccountFilter::default()))
            .unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].provider, "workbuddy", "已有账号不被改写");
    }

    /// 核心场景：先导入、再扫码、再导入 —— 三份操作互不覆盖。
    #[test]
    fn two_authorizations_survive_login_and_reimport() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-3333-4444-5555-666677778888";

        import_logins(&app, vec![magpie_login(uid, "borrowed-1")], &[]).expect("import");
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
        import_logins(&app, vec![magpie_login(uid, "borrowed-2")], &[]).expect("re-import");

        let acc = account(&app, uid);
        assert_eq!(
            own_access(&app, &acc.id).as_deref(),
            Some("own-access"),
            "重新导入不能覆盖自有授权"
        );
        assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("own-refresh"));
        assert!(
            magpie_access(&app, &acc.id).is_none(),
            "重新导入同样不写钥匙串"
        );
        let magpie_row = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .unwrap();
        assert_eq!(
            magpie_row.source_key.as_deref(),
            Some(format!("workbuddy#{uid}").as_str())
        );
        assert_eq!(magpie_row.storage_kind(), "magpie");

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
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
        let acc = account(&app, uid);

        // 默认 magpie 优先：只登记引用，别名里什么都不写。
        assert_eq!(acc.auth_source, "magpie");
        assert!(legacy_access(&app, uid).is_none(), "借用授权不写旧别名");
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
        match broker::resolve(&app, &acc).expect("resolve") {
            broker::Outcome::Resolved(resolved) => {
                assert_eq!(resolved.kind, "magpie");
                assert_eq!(resolved.access, "borrowed", "动态关联要现读文件里的令牌");
            }
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
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
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
        persist_login(
            &app,
            &payload(uid, "first-access", Some("first-refresh")),
            WbVariant::Cn,
        )
        .expect("first");
        persist_login(&app, &payload(uid, "second-access", None), WbVariant::Cn).expect("second");

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
        let err = persist_login(
            &app,
            &payload(uid, "access", Some("refresh")),
            WbVariant::Cn,
        )
        .expect_err("must fail");
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

        let err = persist_login(
            &app,
            &payload(uid, "new-access", Some("new-refresh-boom")),
            WbVariant::Cn,
        )
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

        let err = persist_login(
            &app,
            &payload(uid, "new-access-boom", Some("new-refresh")),
            WbVariant::Cn,
        )
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

        let err = persist_login(&app, &payload(uid, "new-access", None), WbVariant::Cn)
            .expect_err("必须失败");
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

        let err = persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect_err("库里写不进去就该失败");
        assert_eq!(err.code, crate::rpc::E_STORAGE);

        let want_ref = format!("magpie:v1:workbuddy#{uid}");
        assert_eq!(
            magpie_ref(&app, &acc.id).as_deref(),
            Some(want_ref.as_str()),
            "借用授权必须原样保留"
        );
        assert!(
            magpie_access(&app, &acc.id).is_none(),
            "借用授权始终不进钥匙串"
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

        let err = persist_login(
            &app,
            &payload(uid, "new-access", Some("new-refresh")),
            WbVariant::Cn,
        )
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

        let err = persist_login(
            &app,
            &payload(uid, "new-access", Some("new-refresh")),
            WbVariant::Cn,
        )
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
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("persist");
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
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
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
        persist_login(&app, &payload(uid, "access-only", None), WbVariant::Cn).expect("login");
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
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
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
        persist_login(
            &app,
            &payload(uid, "old-access", Some("dead-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
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

    /// PR-CRED-2 的核心不变量：Magpie 导入**一个字节都不进钥匙串**。
    ///
    /// 旧实现先写钥匙串再写库，写库失败要回滚；现在只登记 `magpie:v1:` 引用，
    /// 连"回滚"这个动作都不存在了 —— 失败时天然没有半成品。
    #[test]
    fn magpie_import_writes_no_keychain_entry_even_when_storage_fails() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-7777-0001-0002-000000000001";
        import_logins(&app, vec![magpie_login(uid, "borrowed-1")], &[]).expect("import");
        let acc = account(&app, uid);

        assert!(
            magpie_access(&app, &acc.id).is_none(),
            "导入不得把 Magpie 令牌复制进钥匙串"
        );
        let row = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .expect("query")
            .expect("row");
        assert_eq!(row.storage_kind(), "magpie");
        assert_eq!(
            row.access_ref,
            format!("magpie:v1:workbuddy#{uid}"),
            "只登记来源引用"
        );
        assert!(row.refresh_ref.is_none(), "借用授权永远没有刷新令牌");

        app.with_db(|db| {
            db.exec_raw(
                "CREATE TRIGGER no_auth_update BEFORE UPDATE ON account_authorizations
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
            )
        })
        .expect("install trigger");

        let err = match service::save_magpie(&app, &acc.id, uid, &format!("workbuddy#{uid}"), None)
        {
            Ok(_) => panic!("库里写不进去就该失败"),
            Err(err) => err,
        };
        assert_eq!(err.code, crate::rpc::E_STORAGE);
        assert!(
            magpie_access(&app, &acc.id).is_none(),
            "失败路径同样不写钥匙串"
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
        // 借用授权的令牌现在从 Magpie 文件现读，单测里用线程局部的登录态代替。
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
        // 自有授权有 access、没有 refresh：refresh 一定失败，且不需要联网
        persist_login(&app, &payload(uid, "own-access", None), WbVariant::Cn).expect("login");
        broker::set_preferred_source(&app, "own").expect("prefer own");
        let acc = account(&app, uid);

        let tokens = AppTokens { app: &app };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        let result = runtime.block_on(tokens.resync(&acc));

        assert_eq!(result.as_deref(), Ok("borrowed"), "主路失败要兜底到另一份");
        let want_ref = format!("magpie:v1:workbuddy#{uid}");
        assert_eq!(
            magpie_ref(&app, &acc.id).as_deref(),
            Some(want_ref.as_str()),
            "兜底只读，不许改另一份的引用"
        );
        assert!(
            magpie_access(&app, &acc.id).is_none(),
            "兜底也不会把令牌复制进钥匙串"
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
        persist_login(&app, &payload(uid, "own-access", None), WbVariant::Cn).expect("login");
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

    /// 动态 Magpie 关联的删除**不碰钥匙串**：我们只持有一条引用，令牌在
    /// Magpie 自己的文件里。用"所有删除都失败"的后端来证明它没被调用。
    #[test]
    fn deleting_a_dynamic_magpie_authorization_never_touches_the_keychain() {
        let (app, _dir) = test_app(SecureStore::faulty(&[], &["workbuddy/", "legacy"]));
        let uid = "6f1c2d94-9999-0001-0002-000000000003";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let acc = account(&app, uid);

        // 旧安装遗留的别名：删除动态关联时**必须保留**（回滚要用）。
        let (legacy_access_key, legacy_refresh_key) = ids::legacy_keys(uid);
        app.secure
            .set(&legacy_access_key, b"legacy-access")
            .unwrap();
        app.secure
            .set(&legacy_refresh_key, b"legacy-refresh")
            .unwrap();
        // 武装之后任何 delete 都会返回 denied：只要删除路径碰了钥匙串就会被抓到。
        app.secure.arm();

        let report = service::delete_authorization(&app, &acc.id, "magpie").expect("delete");
        assert!(report.record_deleted, "不得因钥匙串被拒而失败");
        assert!(
            report.removed_keys.is_empty() && report.failed_keys.is_empty(),
            "动态关联的删除不许碰钥匙串：{:?} / {:?}",
            report.removed_keys,
            report.failed_keys
        );
        let after = account(&app, uid);
        assert!(after.auth_mirror_auth_id.is_none(), "镜像必须清空");
        assert_eq!(after.auth_status, "unknown");
        assert!(
            app.with_db(|db| db.find_authorization(&acc.id, "magpie"))
                .unwrap()
                .is_none(),
            "授权行必须删掉"
        );
        // 为回滚保留的旧别名一个都不能少。
        assert_eq!(
            read_key(&app, &legacy_access_key).as_deref(),
            Some("legacy-access")
        );
        assert_eq!(
            read_key(&app, &legacy_refresh_key).as_deref(),
            Some("legacy-refresh")
        );
    }

    /// 仍是 `keychain:` 引用的旧 Magpie 行：按旧安全规则处理，删除时连旧别名一起清。
    #[test]
    fn deleting_a_legacy_keychain_magpie_row_still_purges_the_alias() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000004";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let acc = account(&app, uid);

        // 退回 v19 的形状：引用指向钥匙串。
        let key = ids::access_key(&acc.id, ids::KIND_MAGPIE);
        app.secure.set(&key, b"legacy-token").unwrap();
        app.with_db(|db| {
            db.exec_raw(&format!(
                "UPDATE account_authorizations SET access_ref = 'keychain:{key}'
                  WHERE account_id = '{}' AND kind = 'magpie'",
                acc.id
            ))
        })
        .expect("rewind");
        let (legacy_access_key, legacy_refresh_key) = ids::legacy_keys(uid);
        app.secure
            .set(&legacy_access_key, b"legacy-access")
            .unwrap();
        app.secure
            .set(&legacy_refresh_key, b"legacy-refresh")
            .unwrap();

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

        let acc = account(&app, uid);
        assert!(acc.auth_mirror_auth_id.is_none());
        assert_eq!(acc.auth_status, "unknown");
    }

    /// P0-5：整账号删除也要按引用算。Magpie-only 动态账号必须 **0 次**钥匙串调用。
    #[tokio::test]
    async fn deleting_a_magpie_only_account_never_touches_the_keychain() {
        // 后端"所有删除都失败"：只要删账号碰到钥匙串，就会进 failedKeys / 删不掉账号。
        let (app, _dir) = test_app(SecureStore::faulty(&[], &["workbuddy/", "legacy"]));
        let uid = "6f1c2d94-9999-0001-0002-000000000021";
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let acc = account(&app, uid);

        // 旧安装遗留的别名：动态账号删除时原样保留（回滚要用）。
        let (legacy_access_key, legacy_refresh_key) = ids::legacy_keys(uid);
        app.secure
            .set(&legacy_access_key, b"legacy-access")
            .unwrap();
        app.secure
            .set(&legacy_refresh_key, b"legacy-refresh")
            .unwrap();
        app.secure.arm();

        let out = dispatch(&app, "accounts.delete", &json!({ "id": acc.id }))
            .await
            .expect("delete");
        assert_eq!(out["deleted"], json!(true), "{out}");
        assert_eq!(out["failedKeys"], json!([]), "不许碰钥匙串：{out}");
        assert_eq!(out["orphanCredentials"], json!(false), "{out}");

        assert!(
            app.with_db(|db| db.get_account(&acc.id)).unwrap().is_none(),
            "账号必须删掉"
        );
        assert_eq!(
            read_key(&app, &legacy_access_key).as_deref(),
            Some("legacy-access"),
            "回滚用的旧别名不许删"
        );
        assert_eq!(
            read_key(&app, &legacy_refresh_key).as_deref(),
            Some("legacy-refresh")
        );
        // Magpie 原文件（这里用注入的登录态代表）不能被动过。
        assert_eq!(magpie::read_logins().expect("logins").len(), 1);
        magpie::test_set_logins(None);
    }

    /// 还是 `keychain:` 引用的混合账号：钥匙串那一侧仍按旧安全语义清理。
    #[tokio::test]
    async fn deleting_a_mixed_account_purges_the_keychain_side() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000022";
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
        let acc = account(&app, uid);
        let (legacy_access_key, legacy_refresh_key) = ids::legacy_keys(uid);
        app.secure
            .set(&legacy_access_key, b"legacy-access")
            .unwrap();
        app.secure
            .set(&legacy_refresh_key, b"legacy-refresh")
            .unwrap();

        let out = dispatch(&app, "accounts.delete", &json!({ "id": acc.id }))
            .await
            .expect("delete");
        assert_eq!(out["deleted"], json!(true), "{out}");
        assert_eq!(out["failedKeys"], json!([]), "{out}");

        assert!(own_access(&app, &acc.id).is_none(), "自有 access 必须清掉");
        assert!(own_refresh(&app, &acc.id).is_none());
        assert!(legacy_access(&app, uid).is_none(), "旧别名必须清掉");
        assert!(read_key(&app, &ids::legacy_keys(uid).1).is_none());
        magpie::test_set_logins(None);
    }

    /// 清理失败时**不删库**：否则账号一消失，就再也没法按 id 重试，只能留孤儿。
    #[tokio::test]
    async fn a_failed_purge_keeps_the_account_for_retry() {
        let (app, _dir) = test_app(SecureStore::faulty(&[], &["workbuddy/"]));
        let uid = "6f1c2d94-9999-0001-0002-000000000023";
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
        let acc = account(&app, uid);
        app.secure.arm();

        let out = dispatch(&app, "accounts.delete", &json!({ "id": acc.id }))
            .await
            .expect("delete");
        assert_eq!(out["deleted"], json!(false), "{out}");
        assert!(
            !out["failedKeys"].as_array().unwrap().is_empty(),
            "删不掉的键必须可见：{out}"
        );
        assert!(
            app.with_db(|db| db.get_account(&acc.id)).unwrap().is_some(),
            "清理失败要保留账号行，否则无法重试"
        );
    }

    /// P1-6：迟到的 resync 不许覆盖别人刚改好的来源键。
    #[test]
    fn a_late_resync_does_not_overwrite_a_newer_magpie_link() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000024";
        magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        let acc = account(&app, uid);

        // resync 的"读快照"时刻。
        let snapshot = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .unwrap();

        // 另一个流程（重新导入 / 用户操作）先把来源键换成别的。
        let newer_key = format!("workbuddy#{uid}");
        service::save_magpie(&app, &acc.id, uid, &newer_key, None).expect("newer save");
        let newer = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .unwrap();
        assert!(newer.version > snapshot.version);

        // 迟到的写入拿着旧快照：必须被 CAS 拒掉。
        let err = match service::save_magpie_guarded(
            &app,
            &acc.id,
            uid,
            &format!("workbuddy#{uid}"),
            None,
            Some(service::MagpieGuard::of(&snapshot)),
        ) {
            Ok(_) => panic!("旧快照不许覆盖"),
            Err(err) => err,
        };
        assert!(
            is_stale_snapshot(&err),
            "要归类成 superseded：{:?}",
            err.data
        );

        let after = app
            .with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .unwrap();
        assert_eq!(after.access_ref, newer.access_ref);
        assert_eq!(after.version, newer.version);
        magpie::test_set_logins(None);
    }

    #[test]
    fn deleting_one_authorization_leaves_the_other_intact() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000001";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
        let acc = account(&app, uid);

        let report = service::delete_authorization(&app, &acc.id, "own").expect("delete");
        assert!(report.record_deleted);
        assert!(own_access(&app, &acc.id).is_none());
        assert!(own_refresh(&app, &acc.id).is_none());
        let want_ref = format!("magpie:v1:workbuddy#{uid}");
        assert_eq!(
            magpie_ref(&app, &acc.id).as_deref(),
            Some(want_ref.as_str()),
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
    fn deleting_an_account_purges_only_the_keychain_side() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000002";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
        let acc = account(&app, uid);

        let rows = app.with_db(|db| db.list_authorizations(&acc.id)).unwrap();
        let plan = service::account_purge_plan(&rows, &acc);
        assert!(
            plan.iter().all(|key| !key.contains("/magpie/")),
            "动态 magpie 引用不该产生任何钥匙串键：{plan:?}"
        );
        let failed = service::purge_keys(&app, &plan);
        assert!(failed.is_empty(), "{failed:?}");
        assert!(own_access(&app, &acc.id).is_none());
        assert!(own_refresh(&app, &acc.id).is_none());
        assert!(magpie_access(&app, &acc.id).is_none());
        assert!(legacy_access(&app, uid).is_none());
        assert!(read_key(&app, &ids::legacy_keys(uid).1).is_none());
    }
    /// 只剩一份时删它才算"最后一份"；删的是两份里的一份，别名要留给另一份。
    #[test]
    fn deleting_one_of_two_keeps_the_legacy_alias() {
        let (app, _dir) = test_app(SecureStore::memory());
        let uid = "6f1c2d94-9999-0001-0002-000000000005";
        import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
        let acc = account(&app, uid);

        service::delete_authorization(&app, &acc.id, "own").expect("delete");
        let acc = account(&app, uid);
        assert_eq!(acc.auth_source, "magpie", "镜像应切到还活着的那份");
        assert!(
            legacy_access(&app, uid).is_none(),
            "镜像切到借用授权后也不写别名（宁可旧版本读不到，也不再碰钥匙串）"
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
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
        let acc = account(&app, uid);
        assert_eq!(acc.auth_source, "magpie");
        assert!(
            legacy_access(&app, uid).is_none(),
            "借用授权不写旧别名：那是又一次隐性钥匙串写入"
        );

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
        persist_login(
            &app,
            &payload(uid, "own-access", Some("own-refresh")),
            WbVariant::Cn,
        )
        .expect("login");
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
