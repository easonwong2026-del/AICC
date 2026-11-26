use std::sync::Arc;

use serde_json::{json, Value};
use wb_switch_core::modules::variant::WbVariant;

use super::{is_stale_snapshot, log_note, poll_state, require_str, resync_magpie_access};
use crate::app::App;
use crate::providers::workbuddy_auth;
use crate::rpc::{RpcError, RpcResult};
use crate::securestore::SecretErrorKind;
use crate::storage::AccountInput;

pub(super) async fn dispatch(app: &Arc<App>, method: &str, params: &Value) -> RpcResult<Value> {
    match method {
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
        "auth.workbuddy.status" => {
            let id = require_str(params, "id")?;
            workbuddy_account_status(app, &id).await
        }

        // ---- Buddy 旅行（PR1c）----
        //
        // 三段式：`summary` 只看本地快照（界面轮询用它，不打接口）；
        // `refresh` 查一次真实状态；`depart` / `claim` 是用户显式点的动作。
        other => Err(RpcError::method_not_found(other)),
    }
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
pub(super) async fn workbuddy_account_status(app: &App, id: &str) -> RpcResult<Value> {
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

fn auth_rpc_error(err: workbuddy_auth::AuthError) -> RpcError {
    RpcError::new(crate::rpc::E_AUTH, err.message).with_data(json!({ "kind": err.kind }))
}

/// 登录成功后落库：令牌进**自有授权的键**，账号进 SQLite，返回值里没有任何令牌。
///
/// 只写 `own`：从 Magpie 导入过的那份 `magpie/access` 一个字节都不动（G1）。
/// 回滚策略：入库失败时恢复登录前的自有凭据快照，而不是清空 —— 用户原来能用的那套
/// 不能因为一次失败登录被删掉。
pub(super) fn persist_login(
    app: &App,
    payload: &workbuddy_auth::TokenPayload,
    variant: WbVariant,
) -> RpcResult<Value> {
    // 整个落库过程都在锁里：钥匙串写入 + 数据库写入必须成对出现，
    // 否则并发的 resync / refresh 会看到中间态。
    let _gate = crate::credentials::gate::begin(app)?;
    let uid = payload.profile.uid.clone();
    let account_id = crate::storage::account_id("workbuddy", &uid);

    // 顺序：写凭据 → 建账号行 → 记授权行。账号行还不存在时授权行会被外键拒掉，
    // 所以凭据必须最先落，而它也是唯一需要显式回滚的一步（钥匙串没有事务）。
    // 新登录默认写 AICC 自己的 JSON（一次写、原子替换）；只有 CRED-4 之前
    // 还没迁移的老账号继续走钥匙串。
    let expires = payload
        .expires_at_ms
        .and_then(workbuddy_auth::ms_to_rfc3339);
    let write = crate::auth::service::write_own(
        app,
        &account_id,
        "workbuddy",
        &payload.access_token,
        payload.refresh_token.as_deref(),
        expires
            .as_deref()
            .and_then(crate::auth::service::parse_rfc3339),
    )?;

    let (mut value, existed) = match write_login_row(app, payload, variant) {
        Ok(pair) => pair,
        Err(err) => return Err(fail_login(app, &write, err)),
    };

    match crate::auth::service::record_grant(
        app,
        &account_id,
        &uid,
        crate::auth::ids::KIND_OWN,
        expires.as_deref(),
        Some((write.access_ref(), write.refresh_ref())),
        write.expectation(),
    ) {
        Ok(saved) => {
            if let Some(reason) = saved.degraded {
                // 凭据和授权行都好了，只有兼容别名没同步成功：如实报降级。
                value["degraded"] = json!({ "kind": "alias_sync_failed", "message": reason });
            }
            // 让界面知道这次令牌落在哪：没确认过明文风险时它仍是钥匙串。
            value["credentialStorage"] = json!(saved.authorization.storage_kind());
            if !crate::auth::consent::json_storage_enabled(app) {
                value["jsonConsentRequired"] = json!(true);
            }
            Ok(value)
        }
        // 竞争失败：另一个 Worker 先建好/改好了这条授权。只丢弃"这次新建、没人引用"
        // 的文件；复用已有文件的那次写入保持不动（那是刚拿到的有效令牌）。
        // 账号行也不撤 —— 它可能是胜者建的。
        Err(err) if is_stale_snapshot(&err) => {
            write.discard_unreferenced(app);
            Err(err)
        }
        Err(err) => {
            // 授权行没记上 = 这次登录没有完成。凭据回滚，新建的账号行也撤掉。
            let clean = write.restore(app);
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

/// 刷新成功后把新令牌写回 AICC 的 JSON 文件。
///
/// 与钥匙串版本的区别：`replace_credentials` 在锁内**一次换掉 access + refresh**，
/// 不存在"新 access + 旧 refresh"的中间态；写失败时旧凭据原样有效。
pub(super) fn persist_rotated_json(
    app: &App,
    account: &crate::storage::AccountRecord,
    row: &crate::storage::AuthorizationRecord,
    access: &str,
    refresh: Option<&str>,
    expires: Option<&str>,
) -> RpcResult<Value> {
    use crate::credentials::localjson::LocalJsonStore;

    let secret_id = match crate::auth::owned::secret_id_of(row) {
        Ok(secret_id) => secret_id,
        Err(_) => {
            return Ok(json!({
                "state": "invalid",
                "needsRelogin": true,
                "error": {
                    "kind": "credential_ref_unusable",
                    "message": "自有授权的凭据引用不可用，请重新登录",
                },
            }))
        }
    };
    // 轮换会**写**这个文件：和读路径一样先验归属。引用指错时写下去就是覆盖别人的令牌。
    let verified = match crate::auth::owned::load(app, account, row) {
        Ok(secret) => secret,
        Err(err) => return Ok(unverified_rotation(&err)),
    };
    let store = LocalJsonStore::for_paths(&app.paths);
    commit_rotated_json(
        app, row, &store, &secret_id, &verified, access, refresh, expires,
    )
}

/// 归属/可读性校验没过时的回应。
///
/// 串号（`conflict`）**不**标 `needsRelogin`：重新登录解决不了"库指错了文件"，
/// 还会把这条坏引用盖成一条新授权，问题就此消失。让它显式留在界面上。
fn unverified_rotation(err: &crate::credentials::CredentialError) -> Value {
    let kind = match err.kind {
        crate::credentials::CredentialErrorKind::Conflict => "credential_ownership_mismatch",
        crate::credentials::CredentialErrorKind::NotFound => "credential_unreadable",
        other => other.as_str(),
    };
    json!({
        "state": "invalid",
        "needsRelogin": err.kind != crate::credentials::CredentialErrorKind::Conflict,
        "error": {
            "kind": kind,
            "message": crate::rpc::sanitize(&err.message),
        },
    })
}

/// 真正落盘的那一步。`expected_revision` 显式传进来：调用方读到的版本是多少，
/// 这次写入就必须建立在那个版本上。并发刷新时后到的那一方会拿到 `conflict`，
/// 结果是 `superseded`（不覆盖、不标失效），而不是把人家的新令牌盖掉。
#[allow(clippy::too_many_arguments)]

pub(crate) fn commit_rotated_json(
    app: &App,
    row: &crate::storage::AuthorizationRecord,
    store: &crate::credentials::localjson::LocalJsonStore,
    secret_id: &str,
    // 已经验过归属的那份快照：`revision` 与旧 refresh 都从它取。
    verified: &crate::credentials::localjson::StoredSecret,
    access: &str,
    refresh: Option<&str>,
    expires: Option<&str>,
) -> RpcResult<Value> {
    use crate::credentials::localjson::Credentials;

    // 服务端没轮换 refresh 时，旧的那份继续有效：用验过的那份快照里的值一起写。
    let credentials = match refresh {
        Some(next) => Credentials::oauth(access, next),
        None => match verified.credentials.refresh_token.as_deref() {
            Some(previous) => Credentials::oauth(access, previous),
            None => Credentials::access_only(access),
        },
    };
    let expires_at = expires.and_then(crate::auth::service::parse_rfc3339);

    let updated =
        match store.replace_credentials(secret_id, verified.revision, credentials, expires_at) {
            Ok(updated) => updated,
            // 另一个进程已经写过这个文件：我这份结论过时了，不覆盖、也不标记失效
            // ——盘上那份才是新的，下一次读会拿到它。
            Err(err) if err.kind == crate::credentials::CredentialErrorKind::Conflict => {
                return Ok(json!({
                    "state": "superseded",
                    "reason": "credential_revision_changed",
                }))
            }
            // 写失败：文件保持旧内容（原子替换保证没有半份），但服务端已经把旧的 refresh
            // 作废了，这份授权确实不能再用 —— 标 invalid，等用户重新登录。
            Err(err) => {
                let _ = crate::auth::service::record_status(
                    app,
                    row,
                    "invalid",
                    None,
                    Some("refresh_not_persisted"),
                );
                return Ok(json!({
                    "state": "invalid",
                    "needsRelogin": true,
                    "error": {
                        "kind": "refresh_not_persisted",
                        "message": crate::rpc::sanitize(&err.message),
                    },
                }));
            }
        };

    // **提交边界**：文件写完之后必须确认 SQLite 那条授权还指着这份文件、版本也没变。
    // 否则就是"给一份已经不被引用的旧文件写了新令牌，还向界面报告刷新成功"。
    match crate::auth::service::record_status(app, row, "valid", expires, None) {
        Ok(true) => Ok(json!({
            "state": "valid",
            "tokenExpiresAt": expires,
            "secretRevision": updated.revision,
        })),
        // CAS 失败。**版本变了不等于引用变了**，这两件事的后果相反：
        Ok(false) => {
            match app.with_db(|db| db.find_authorization(&row.account_id, &row.kind)) {
                // ① 还指着同一份文件：别的流程只是更新了状态 / 有效期。
                // 我们刚写进去的令牌是最新的，回滚只会把账号退回到服务端已作废的旧 refresh。
                // 保留新令牌，如实报告版本冲突。
                Ok(Some(current)) if current.access_ref == row.access_ref => Ok(json!({
                    "state": "superseded",
                    "reason": "authorization_version_changed",
                    "fileReverted": false,
                    "fileUpdated": true,
                })),
                // ② 引用被迁到别的文件（或整行被删）：F 已经没人引用，撤销这次写入。
                Ok(_) => {
                    let reverted = revert_rotation(store, secret_id, &updated, verified);
                    Ok(json!({
                        "state": "superseded",
                        "reason": "authorization_moved",
                        "fileReverted": reverted,
                        // 回滚失败 = 一份没人引用的文件里留着刚拿到的有效令牌，必须让调用方看见。
                        "orphanSecret": !reverted,
                    }))
                }
                // ③ 读不出来：不知道 F 还有没有主人。回滚的风险更大（可能毁掉唯一有效的
                // refresh），所以保留新令牌，但**不**声称成功。
                Err(err) => Ok(json!({
                    "state": "invalid",
                    "needsRelogin": false,
                    "fileUpdated": true,
                    "error": {
                        "kind": "rotation_state_unknown",
                        "message": crate::rpc::sanitize(&err.message),
                    },
                })),
            }
        }
        // 库写不进去：文件里的新令牌是真的、可用的（旧 refresh 已被服务端作废），
        // 所以**不回滚**，但也不能声称成功。
        Err(err) => Ok(json!({
            "state": "invalid",
            "needsRelogin": false,
            "fileUpdated": true,
            "error": {
                "kind": "rotation_not_committed",
                "message": crate::rpc::sanitize(&err.message),
            },
        })),
    }
}

/// 把文件写回"轮换前"的内容。返回 false = 没写回去，调用方要留痕。
fn revert_rotation(
    store: &crate::credentials::localjson::LocalJsonStore,
    secret_id: &str,
    updated: &crate::credentials::localjson::StoredSecret,
    verified: &crate::credentials::localjson::StoredSecret,
) -> bool {
    let expires = verified
        .expires_at
        .as_deref()
        .and_then(crate::auth::service::parse_rfc3339);
    match store.replace_credentials(
        secret_id,
        updated.revision,
        verified.credentials.clone(),
        expires,
    ) {
        Ok(_) => true,
        Err(err) => {
            crate::handlers::log_note(&format!(
                "轮换回滚失败（{}）：{}",
                err.kind.as_str(),
                crate::rpc::sanitize(&err.message)
            ));
            false
        }
    }
}

/// 登录失败时统一收尾：回滚钥匙串，回滚不干净就如实说。
fn fail_login(app: &App, write: &crate::auth::service::OwnWrite, err: RpcError) -> RpcError {
    if write.restore(app) {
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
pub(super) fn persist_rotated_tokens(
    app: &App,
    account: &crate::storage::AccountRecord,
    expected_epoch: i64,
    access: &str,
    refresh: Option<&str>,
    expires: Option<&str>,
) -> RpcResult<Value> {
    let _gate = crate::credentials::gate::begin(app)?;
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
    // PR-CRED-3：JSON 行整组换令牌（access + refresh 一次替换），不碰钥匙串。
    if row.storage_kind() == crate::storage::authz::STORAGE_LOCALJSON {
        return persist_rotated_json(app, &current, &row, access, refresh, expires);
    }
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
        // 老账号（`keychain:` 行）同样不能"写完就报成功"：状态提交的 CAS 失败
        // 说明这一行已经被别的流程改过，界面不该收到"刷新成功"。
        return match crate::auth::service::record_status(app, &row, "valid", expires, None) {
            Ok(true) => Ok(json!({ "state": "valid", "tokenExpiresAt": expires })),
            Ok(false) => Ok(json!({
                "state": "superseded",
                "reason": "authorization_moved",
                // 钥匙串的键名是按账号确定的，令牌本身仍然可用；
                // 只是这一行的状态没被这次刷新更新。
                "fileReverted": true,
            })),
            Err(err) => Ok(json!({
                "state": "invalid",
                "needsRelogin": false,
                "fileUpdated": true,
                "error": {
                    "kind": "rotation_not_committed",
                    "message": crate::rpc::sanitize(&err.message),
                },
            })),
        };
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
