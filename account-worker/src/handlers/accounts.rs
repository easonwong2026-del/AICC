use std::sync::Arc;
#[cfg(test)]
use std::sync::Mutex;

use serde_json::{json, Value};

use super::{log_note, poll_state, require_str, superseded};
use crate::app::App;
use crate::providers::qoder_edition::QoderEdition;
use crate::providers::{
    magpie, qoder, qoder_admin, qoder_auth, qoder_pat, workbuddy, workbuddy_auth,
};
use crate::qoder_profiles::manager as qoder_manager;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::{AccountFilter, AccountInput};

pub(super) async fn dispatch(app: &Arc<App>, method: &str, params: &Value) -> RpcResult<Value> {
    match method {
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
            account_value(app, record)
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
            let _gate = crate::credentials::gate::begin(app)?;
            let Some(account) = app.with_db(|db| db.get_account(&id))? else {
                return Ok(json!({
                    "deleted": false,
                    "failedKeys": [],
                    "failedSecrets": [],
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
            // 自有授权的 JSON 文件也要跟着账号一起删；动态 Magpie 行没有文件。
            // `account_secret_plan` 逐行验归属：引用指错时**不删**，而是拒绝整次删除 ——
            // 那可能是另一个账号的有效凭据。
            let secrets = crate::auth::service::account_secret_plan(app, &account, &rows);
            if !secrets.refused.is_empty() {
                log_note(&format!(
                    "账号 {} 有 {} 份凭据归属校验失败，已停止删除",
                    account.id,
                    secrets.refused.len()
                ));
                return Ok(json!({
                    "deleted": false,
                    "failedKeys": [],
                    "failedSecrets": [],
                    "refusedSecrets": secrets.refused,
                    "orphanCredentials": false,
                    "reason": "secret_ownership_mismatch",
                }));
            }
            let failed_keys = crate::auth::service::purge_keys(app, &plan);
            let failed_secrets = crate::auth::service::purge_secrets(app, &secrets.owned);
            if !failed_keys.is_empty() || !failed_secrets.is_empty() {
                // 删不掉的凭据要可见，账号保留以便重试。
                return Ok(json!({
                    "deleted": false,
                    "failedKeys": failed_keys,
                    "failedSecrets": failed_secrets,
                    "orphanCredentials": true,
                }));
            }
            match app.with_db(|db| db.delete_account(&id)) {
                Ok(deleted) => Ok(json!({
                    "deleted": deleted,
                    "failedKeys": [],
                    "failedSecrets": [],
                    "orphanCredentials": false,
                })),
                // 凭据已经清掉了，账号行因为数据库错误留下：不谎报成功，但可以重试
                // （清凭据这一步对已删的键 / 文件是幂等的）。
                Err(err) => {
                    log_note(&format!(
                        "账号行删除失败（{id}）：{}",
                        crate::rpc::sanitize(&err.message)
                    ));
                    Ok(json!({
                        "deleted": false,
                        "failedKeys": [],
                        "failedSecrets": [],
                        "orphanCredentials": false,
                        "recordError": "account_delete_failed",
                    }))
                }
            }
        }

        "auth.authorization.delete" => {
            let account_id = require_str(params, "accountId")?;
            let kind = require_str(params, "kind")?;
            let _gate = crate::credentials::gate::begin(app)?;
            if app.with_db(|db| db.get_account(&account_id))?.is_none() {
                return Err(RpcError::not_found(format!("账号不存在：{account_id}")));
            }
            let report = crate::auth::service::delete_authorization(app, &account_id, &kind)?;
            if !report.failed_keys.is_empty() || !report.failed_secrets.is_empty() {
                return Err(RpcError::new(
                    crate::rpc::E_SECURESTORE,
                    "授权记录已保留，但有凭据没能删除，可重试",
                )
                .with_data(json!({
                    "kind": "credential_delete_failed",
                    "failedKeys": report.failed_keys,
                    "failedSecrets": report.failed_secrets,
                })));
            }
            if let Some(reason) = &report.secret_error {
                // 归属不符：一个字节都没删。这不是"删除失败"，是"不敢删"。
                return Err(RpcError::new(
                    crate::rpc::E_SECURESTORE,
                    "凭据文件不属于这条授权，已拒绝删除（没有删除任何文件）",
                )
                .with_data(json!({
                    "kind": "secret_ownership_mismatch",
                    "reason": reason,
                })));
            }
            if let Some(reason) = &report.record_error {
                // 凭据已经删掉了、记录因为数据库错误留下：这句话必须说清楚，
                // 否则界面会以为整份授权还在。
                return Err(RpcError::internal(format!(
                    "凭据已删除，但授权记录删除失败（{reason}）；可以重试删除"
                ))
                .with_data(json!({
                    "kind": "record_delete_failed",
                    "removedSecrets": report.removed_secrets,
                })));
            }
            Ok(json!({
                "accountId": account_id,
                "kind": kind,
                "deleted": report.record_deleted,
                "removedKeys": report.removed_keys.len(),
                "removedSecrets": report.removed_secrets.len(),
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

        // 删号：读授权快照 → 关自动领取 → 推进授权版本 → 按引用删凭据 → 删行。
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
        // 1. **令牌只从请求进、只往凭据后端去**（钥匙串或本地 JSON，见 `qoder_secret`）：
        //    不回显、不进错误信息、不进数据库。
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
                // 期间这条授权被别的流程换过：什么都没删，让界面提示刷新重试。
                "authorizationChanged": removed.authorization_changed,
                // 行已摘掉但凭据没删干净（已以 missing 放回）：**不是**成功，要重试。
                "pendingCleanup": removed.pending_cleanup,
            }))
        }

        // ---- 旧 Keychain 凭据迁移（CRED-4）----
        //
        // 四个动作全部显式触发：没有定时任务、没有启动时迁移。读出钥匙串会弹
        // 系统授权框，后台跑等于把 RPC 队列卡住；明文落盘也必须用户自己点头。
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
        "qoder.credits.auto.tick" => {
            // `force` 只跳过 20 分钟的最小间隔，不绕过账号开关。
            let force = params
                .get("force")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            Ok(crate::credits::run_tick(app, force).await)
        }

        // ---- WorkBuddy 扫码登录（PR1b-1）----
        "providers.magpie.accounts" => Ok(magpie_preview(app)),

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

/// 账号列表带上每份授权（不含令牌）。查询在 `with_db` 外面做，避免嵌套锁。
fn with_authorizations(
    app: &App,
    accounts: Vec<crate::storage::AccountRecord>,
) -> RpcResult<Vec<Value>> {
    let magpie_cache = crate::auth::magpie_link::MagpieLoginsCache::new();
    let mut out = Vec::with_capacity(accounts.len());
    for account in accounts {
        out.push(account_value_with_cache(app, account, &magpie_cache)?);
    }
    Ok(out)
}

/// 账号的对外视图（列表与详情共用一条路径）。
///
/// 借用授权的真实到期时间在 Magpie 文件里，库里那列是导入时的快照。这里补一次
/// 只读修正，否则界面的 `isTokenExpired` 会把一份可用授权一直显示成「已过期」。
/// **只改返回值，不写库**；读不到 / 关联对不上时保留库里那份。
fn account_value(app: &App, record: crate::storage::AccountRecord) -> RpcResult<Value> {
    let magpie_cache = crate::auth::magpie_link::MagpieLoginsCache::new();
    account_value_with_cache(app, record, &magpie_cache)
}

fn account_value_with_cache(
    app: &App,
    mut record: crate::storage::AccountRecord,
    magpie_cache: &crate::auth::magpie_link::MagpieLoginsCache,
) -> RpcResult<Value> {
    if let Some(expiry) =
        crate::auth::service::display_expiry_with_cache(app, &record, magpie_cache)
    {
        record.token_expires_at = Some(expiry);
    }
    let mut value = json!(record);
    value["authorizations"] = json!(crate::auth::broker::describe_with_cache(
        app,
        &record.id,
        magpie_cache
    )?);
    Ok(value)
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
pub(super) fn import_logins(
    app: &App,
    logins: Vec<magpie::MagpieLogin>,
    keys: &[String],
) -> RpcResult<Value> {
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
        let _gate = crate::credentials::gate::begin(app)?;

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
    let _gate = crate::credentials::gate::begin(app)?;
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

/// 必须显式给出、且只能是两个合法版本之一。
///
/// **绝不回落**到任何一个默认版本：那会用错站点去验凭据，
/// 把一次国内版操作记成国际版（或反过来）。
fn require_edition(params: &Value) -> RpcResult<QoderEdition> {
    let raw = require_str(params, "edition")?;
    QoderEdition::parse(&raw)
        .ok_or_else(|| RpcError::invalid_params("edition 只支持 qoder_cn / qoder_global"))
}
