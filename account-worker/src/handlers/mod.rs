//! JSON-RPC 方法表。
//!
//! 命名约定：`<域>.<动作>`。PR1a 只覆盖本地能力，任何联网行为都不在这里。

mod accounts;
mod automation;
mod system;
#[cfg(test)]
mod tests;
mod workbuddy_auth;

use std::sync::Arc;

use serde_json::{json, Value};

use crate::app::App;
use crate::providers::workbuddy_auth as wb_auth;
use crate::rpc::{RpcError, RpcResult};

pub(crate) use accounts::resync_magpie_access;
#[cfg(test)]
pub(crate) use workbuddy_auth::commit_rotated_json;
pub(crate) use workbuddy_auth::refresh_account;

#[cfg(test)]
use accounts::import_logins;
#[cfg(test)]
use system::{health, require_secure_rpc, require_setting_key, validate_secure_key};
#[cfg(test)]
use workbuddy_auth::{persist_login, persist_rotated_tokens, workbuddy_account_status};

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
    // ---- 旧 Keychain 凭据迁移（CRED-4）----
    "credentials.migration.inventory",
    "credentials.migration.run",
    "credentials.migration.rollback",
    "credentials.migration.verify",
    // CRED-5：借用授权只改引用、不搬令牌，因此不要求明文同意。
    "credentials.migration.relink",
    // CRED-5 复核 P0-1：旧键搬运失败后**只有**这个显式动作会重试。
    "credentials.keychain.retry",
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
    // ---- 成长活跃（PR #103：只读；签到与活跃分开算）----
    "growth.activity.status",
    // ---- 奖励状态（PR #96：只读；三个写动作仍未接线）----
    "growth.rewards.status",
    // ---- 正式奖励写操作（PR #101）----
    "growth.rewards.draw",
    "growth.rewards.redeem",
    "growth.rewards.makeup",
    "growth.rewards.recheck",
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
    // ---- WorkBuddy 客户端账号切换（WB-SWITCH-1）----
    //
    // `status` 只读；`switch` / `restore` 会关闭并重启 WorkBuddy 客户端。
    "workbuddy.client.status",
    "workbuddy.client.switch",
    "workbuddy.client.restore",
    // ---- 夜间账号轮换（WB-ROTATE-1）----
    //
    // `status` / `dryRun` 只读；`save` / `setEnabled` 只写配置；
    // `runNow` / `stop` / `restoreOrigin` 会关闭并重启客户端。
    "workbuddy.rotation.status",
    "workbuddy.rotation.save",
    "workbuddy.rotation.setEnabled",
    "workbuddy.rotation.dryRun",
    "workbuddy.rotation.runNow",
    "workbuddy.rotation.stop",
    "workbuddy.rotation.restoreOrigin",
    // ---- 局域网 HTTPS 身份（PR-B）----
    "tls.identity",
    "tls.ensure",
    "tls.reset",
];

pub async fn dispatch(app: &Arc<App>, method: &str, params: &Value) -> RpcResult<Value> {
    if matches!(
        method,
        "ping" | "health" | "env.info" | "worker.shutdown" | "tasks.overview"
    ) || method.starts_with("storage.")
        || method.starts_with("tls.")
        || method.starts_with("groups.")
        || method.starts_with("secure.")
        || method.starts_with("settings.")
        || method.starts_with("credentials.")
    {
        return system::dispatch(app, method, params).await;
    }

    if method.starts_with("accounts.")
        || method.starts_with("providers.")
        || method.starts_with("qoder.")
        || method == "auth.authorization.delete"
        || method.starts_with("auth.magpie.")
    {
        return accounts::dispatch(app, method, params).await;
    }

    if method.starts_with("auth.workbuddy.") {
        return workbuddy_auth::dispatch(app, method, params).await;
    }

    if method.starts_with("travel.")
        || method.starts_with("growth.")
        || method.starts_with("checkin.")
        || method.starts_with("quota.")
        || method.starts_with("workbuddy.")
    {
        return automation::dispatch(app, method, params).await;
    }

    Err(RpcError::method_not_found(method))
}

/// 凭据写入的**唯一门禁**在 `crate::credentials::gate`。
///
/// 这里刻意不再提供一把"只有进程内 Mutex"的锁：那正是第七轮复审 P0-2 的根因
/// （迁移持跨进程锁、WorkBuddy 写入只持进程内锁，于是两份令牌能混搭进同一份副本）。
/// 会改凭据的路径统一 `let _gate = crate::credentials::gate::begin(app)?;` ——
/// 先进程内互斥、后跨进程 flock，全代码只有这一个顺序。

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

/// 把内部错误分类翻译成界面状态。会话已结束的都归到 `finished`，
/// 前端据此回到"刷新账号列表"，而不是继续轮询。
pub(super) fn poll_state(kind: &str) -> &'static str {
    match kind {
        wb_auth::KIND_CANCELLED => "cancelled",
        wb_auth::KIND_EXPIRED => "expired",
        wb_auth::KIND_FINISHED | wb_auth::KIND_UNKNOWN_LOGIN => "finished",
        _ => "failed",
    }
}

pub(super) fn require_bool(params: &Value, key: &str) -> RpcResult<bool> {
    params
        .get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| RpcError::invalid_params(format!("缺少布尔参数 {key}")))
}

/// 可选字符串数组参数。**类型不对就报错，不当作空数组**。
///
/// `accountIds` 的语义是"只查这些账号"，空数组是"全部账号"。把
/// `"abc"` / `[1,2]` / `{a:1}` 静默当成空数组，等于用户想查一个账号、
/// 结果扫了全库 —— 既是意外访问，也让日志看起来像"没人指定"。
pub(super) fn optional_string_list(params: &Value, key: &str) -> RpcResult<Vec<String>> {
    let Some(raw) = params.get(key) else {
        return Ok(Vec::new());
    };
    if raw.is_null() {
        return Ok(Vec::new());
    }
    let Some(items) = raw.as_array() else {
        return Err(RpcError::invalid_params(format!("{key} 必须是字符串数组")));
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match item.as_str() {
            Some(text) => out.push(text.to_string()),
            None => return Err(RpcError::invalid_params(format!("{key} 里只能放字符串"))),
        }
    }
    Ok(out)
}

pub(super) fn require_str(params: &Value, key: &str) -> RpcResult<String> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| RpcError::invalid_params(format!("缺少参数 {key}")))
}

/// 可选的字符串数组参数。缺省 / `null` / 类型不对都当空数组（= 全部）。
pub(super) fn string_list(params: &Value, key: &str) -> Vec<String> {
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
