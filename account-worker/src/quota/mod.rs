//! 四平台额度看板（M4）：统一模型 + 分平台适配器。
//!
//! 统一模型的字段是固定的，平台之间只允许三种差异，且都必须显式表达：
//!
//! | 差异 | 表达方式 |
//! | --- | --- |
//! | 计量单位不同（credits / tokens / 未知） | `unit` 原样带出，**不同单位不相加** |
//! | 额度未知 | `total/used/remaining` 是 `null`，**不是 0** |
//! | 能力没验证过 | `status = "unverified"` + `error` 说明原因，且**零请求** |
//!
//! 只读契约：本模块不改账号、不改授权状态、不写 `last_used_at`。唯一的写入是
//! 额度快照（`quota_snapshots`），那是缓存，不是业务状态。

pub mod qoder;
pub mod workbuddy;

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::app::App;
use crate::auth::broker::{self, Outcome};
use crate::rpc::{RpcError, RpcResult};
use crate::storage::{AccountFilter, AccountRecord};

/// 看板允许出现的 provider。顺序即界面顺序。
pub const PROVIDERS: &[&str] = &["workbuddy", "workbuddy_ai", "qoder_cn", "qoder_global"];

/// 历史缓存保留天数（与 `credit_usage` 的上游默认一致）。
const HISTORY_KEEP_DAYS: i64 = 90;

/// 缓存新鲜度：5 分钟内不重复打网络。超过就算过期，`refresh=false` 也会重取。
const CACHE_TTL_SECS: i64 = 300;

/// 一次刷新最多同时打几个账号。每个账号两次请求、各 10 秒上限。
const MAX_CONCURRENT: usize = 3;

/// 整轮刷新的硬预算。Swift 侧 RPC 超时是 45 秒，这里远小于它，
/// 超预算的账号返回"缓存 + 过期"而不是把整个调用拖死。
const BOARD_BUDGET: Duration = Duration::from_secs(20);

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardParams {
    /// 只看某个平台。
    #[serde(default)]
    pub provider: Option<String>,
    /// 只看某个账号。
    #[serde(default)]
    pub account_id: Option<String>,
    /// 用量统计窗口（含今天）。默认 7 天。
    #[serde(default)]
    pub days: Option<i64>,
    /// 强制走网络。`false` 时优先用缓存。
    #[serde(default)]
    pub refresh: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryParams {
    pub account_id: String,
    #[serde(default)]
    pub days: Option<i64>,
}

fn unknown_provider(provider: &str) -> RpcError {
    RpcError::invalid_params(format!(
        "未知平台：{provider}（可选：{}）",
        PROVIDERS.join(" / ")
    ))
}

/// 组装看板。`refresh=false` 时若缓存还在 5 分钟有效期内就直接用，省掉一次网络。
///
/// 批量刷新的三条约束：**有限并发**（`MAX_CONCURRENT`）、**整体预算**
/// （`BOARD_BUDGET`）、**部分结果**（超预算的账号不拖累已经拿到的账号）。
pub async fn board(app: &Arc<App>, params: BoardParams) -> RpcResult<Value> {
    let days = params.days.unwrap_or(7).clamp(1, 90);
    if let Some(provider) = params.provider.as_deref() {
        if !PROVIDERS.contains(&provider) {
            return Err(unknown_provider(provider));
        }
    }

    let workbuddy_accounts = app.with_db(|db| db.list_accounts(&AccountFilter::default()))?;
    let qoder_accounts = app.with_db(|db| db.list_qoder_accounts(None))?;

    // 顺手裁一次历史：快照是观测数据，没有理由无限增长。失败不影响取数。
    let before = crate::clock::now_rfc3339_at_days_ago(HISTORY_KEEP_DAYS);
    app.with_db(|db| db.prune_quota_snapshots(&before)).ok();

    let mut rows: Vec<Value> = Vec::with_capacity(workbuddy_accounts.len() + qoder_accounts.len());

    // WorkBuddy 账号并发取数：信号量控并发，JoinSet 收结果。
    let deadline = tokio::time::Instant::now() + BOARD_BUDGET;
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT));
    let mut set: JoinSet<Value> = JoinSet::new();
    for account in workbuddy_accounts {
        if !wanted(
            params.provider.as_deref(),
            &account.provider,
            params.account_id.as_deref(),
            &account.id,
        ) {
            continue;
        }
        let app = Arc::clone(app);
        let semaphore = Arc::clone(&semaphore);
        let refresh = params.refresh.unwrap_or(false);
        set.spawn(async move {
            // 信号量只在取数期间占用；`acquire_owned` 保证超时也不会漏放。
            let _permit = semaphore.acquire_owned().await;
            workbuddy_row(&app, account, days, refresh, deadline).await
        });
    }
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(row) => rows.push(row),
            Err(err) => rows.push(json!({
                "accountId": "",
                "provider": "workbuddy",
                "nickname": "",
                "source": "unknown",
                "status": "error",
                "reason": "task_failed",
                "unit": Value::Null,
                "total": Value::Null,
                "used": Value::Null,
                "remaining": Value::Null,
                "plan": Value::Null,
                "isPaid": Value::Null,
                "usage": Value::Null,
                "usageError": Value::Null,
                "syncAt": crate::clock::now_rfc3339(),
                "error": format!("取数任务异常退出：{err}"),
                "cached": false,
                "stale": false,
            })),
        }
    }

    for account in &qoder_accounts {
        // `edition` 本身就是 `qoder_cn` / `qoder_global`，即 provider。
        let provider = account.edition.as_str();
        if !wanted(
            params.provider.as_deref(),
            provider,
            params.account_id.as_deref(),
            &account.id,
        ) {
            continue;
        }
        let source = app
            .with_db(|db| db.list_qoder_authorizations(&account.id))
            .map(|rows| rows.first().map(|r| r.source.clone()))
            .ok()
            .flatten()
            .unwrap_or_else(|| "unknown".to_string());
        // Qoder 账号很少，串行就够；每个账号仍受整轮 deadline 约束。
        rows.push(
            qoder_row(
                app,
                account,
                &source,
                params.refresh.unwrap_or(false),
                deadline,
            )
            .await,
        );
    }

    rows.sort_by_key(|row| {
        let provider = row["provider"].as_str().unwrap_or_default();
        let rank = PROVIDERS
            .iter()
            .position(|p| *p == provider)
            .unwrap_or(PROVIDERS.len());
        (
            rank,
            row["nickname"].as_str().unwrap_or_default().to_string(),
        )
    });

    // 明确告诉界面：这些平台一个网络请求都没发，不是"暂时取不到"。
    // 现在按实际结果算：任何一行拿到 `ok`，就不再算"未验证"。
    let unverified: Vec<&str> = PROVIDERS
        .iter()
        .copied()
        .filter(|provider| {
            rows.iter()
                .filter(|row| row["provider"].as_str() == Some(provider))
                .all(|row| row["status"].as_str() == Some("unverified"))
        })
        .collect();

    Ok(json!({
        "rows": rows,
        "days": days,
        "today": crate::clock::beijing_date(),
        "cacheTtlSeconds": CACHE_TTL_SECS,
        "unverifiedProviders": unverified,
        "budgetSeconds": BOARD_BUDGET.as_secs(),
    }))
}

/// 一个 Qoder 账号的一行。
///
/// 三条规则：
/// 1. **缓存优先**：5 分钟内且没要求刷新就直接返回快照，避免每次开面板都打网络；
/// 2. **`refresh` 只影响这一次**：它不写回任何开关，也不改账号状态；
/// 3. **写入要过版本核验**：`auth_epoch` 对不上就不写（换过授权 / 删过号）。
async fn qoder_row(
    app: &Arc<App>,
    account: &crate::storage::qoder_accounts::QoderAccountRecord,
    source: &str,
    refresh: bool,
    deadline: tokio::time::Instant,
) -> Value {
    let cached = app
        .with_db(|db| db.latest_quota_snapshot(&account.id))
        .ok()
        .flatten();

    if !refresh {
        if let Some(snapshot) = cached
            .as_ref()
            .filter(|s| snapshot_age_secs(&s.captured_at) <= CACHE_TTL_SECS)
        {
            if let Some(row) = qoder::cached_row(
                &parse_payload(&snapshot.payload),
                &snapshot.captured_at,
                false,
            ) {
                return row;
            }
        }
    }

    if tokio::time::Instant::now() >= deadline {
        return qoder_cached_or_blocked(account, source, cached.as_ref(), "本轮刷新超出时间预算");
    }

    let status =
        match tokio::time::timeout_at(deadline, crate::credits::status(app, &account.id)).await {
            Ok(Ok(value)) => value,
            Ok(Err(err)) => {
                return qoder_cached_or_blocked(account, source, cached.as_ref(), &err.message)
            }
            Err(_) => {
                return qoder_cached_or_blocked(
                    account,
                    source,
                    cached.as_ref(),
                    "Qoder 查询超出本轮时间预算",
                )
            }
        };

    // 授权来源用**这次真正发请求的那一份**，不是"授权列表里的第一条想当然"。
    // 列表顺序不代表实际选择：同一账号可能同时有 Magpie 关联、浏览器授权与 PAT。
    let actual_source = status
        .get("authSource")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| source.to_string());
    let row = qoder::row_from_status(
        &account.id,
        &account.edition,
        &account.nickname,
        &actual_source,
        &status,
    );

    // 只有真的拿到过服务端结论（含"今天没有活动"）才落缓存：
    // 401 / 网络错误的行不该变成历史记录。
    if matches!(row["status"].as_str(), Some("ok")) {
        app.with_db(|db| {
            db.save_qoder_quota_snapshot(
                &account.id,
                &account.edition,
                "ok",
                &row,
                Some(account.auth_epoch),
            )
            .map(|_| ())
        })
        .ok();
    }

    row
}

/// 取不到时：有旧快照就降级成 `stale`，没有就给出一句能行动的原因。
fn qoder_cached_or_blocked(
    account: &crate::storage::qoder_accounts::QoderAccountRecord,
    source: &str,
    cached: Option<&crate::storage::quota::QuotaSnapshot>,
    detail: &str,
) -> Value {
    if let Some(snapshot) = cached {
        if let Some(mut row) = qoder::cached_row(
            &parse_payload(&snapshot.payload),
            &snapshot.captured_at,
            true,
        ) {
            row["status"] = json!("stale");
            row["error"] = json!(crate::rpc::sanitize(detail));
            return row;
        }
    }
    qoder::unverified_row(
        &account.id,
        &account.edition,
        &account.nickname,
        source,
        &crate::rpc::sanitize(detail),
    )
}

fn wanted(
    provider: Option<&str>,
    row_provider: &str,
    account_id: Option<&str>,
    row_account: &str,
) -> bool {
    provider.map(|p| p == row_provider).unwrap_or(true)
        && account_id.map(|id| id == row_account).unwrap_or(true)
}

/// 一个 WorkBuddy 账号的一行。
///
/// 四条降级规则：
/// 1. 缓存在 5 分钟内**且统计窗口一致** → 直接用，不打网络；
/// 2. 缓存新但窗口不一致 → 余额复用，**用量按请求的窗口重取**（7 天的消耗不能冒充 30 天）；
/// 3. 过期或要求刷新 → 重新取，**额度与用量各自独立成败**；
/// 4. 取数失败但有旧快照 → 返回旧数据并标 `stale`，不把有用的数丢掉。
async fn workbuddy_row(
    app: &Arc<App>,
    account: AccountRecord,
    days: i64,
    refresh: bool,
    deadline: tokio::time::Instant,
) -> Value {
    let cached = app
        .with_db(|db| db.latest_quota_snapshot(&account.id))
        .ok()
        .flatten();

    if !refresh {
        if let Some(snapshot) = cached
            .as_ref()
            .filter(|s| snapshot_age_secs(&s.captured_at) <= CACHE_TTL_SECS)
        {
            if cached_usage_window(snapshot) == Some(days) {
                if let Some(row) = cached_row(snapshot, false) {
                    return row;
                }
            } else {
                // 余额还是 5 分钟内的，可以复用；用量必须按这一轮的窗口重取。
                return usage_only_row(app, &account, snapshot, days, deadline).await;
            }
        }
    }

    // 预算已经用完：不再发起请求，用旧数据（如果有）标过期返回。
    if tokio::time::Instant::now() >= deadline {
        return degraded_or_error(
            &account,
            &account.auth_source,
            cached.as_ref(),
            days,
            "timeout",
            "本轮刷新超出时间预算，未查询该账号",
        );
    }

    let (source, access) = match resolve_access(app, &account, deadline).await {
        Ok(resolved) => resolved,
        Err(detail) => {
            // 凭据读不到（含钥匙串等授权超时）一律按授权不可用降级，
            // 有旧快照就继续显示旧数据。
            let status = if detail.contains("时间预算") {
                "timeout"
            } else {
                "unauthorized"
            };
            return degraded_or_error(
                &account,
                &account.auth_source,
                cached.as_ref(),
                days,
                status,
                &detail,
            );
        }
    };

    // 额度：这一条挂了，整行只能是失败或旧数据。
    let quota =
        match tokio::time::timeout_at(deadline, workbuddy::fetch_quota(&account, &access)).await {
            Ok(Ok(quota)) => quota,
            Ok(Err(failure)) => {
                return degraded_or_error(
                    &account,
                    &source,
                    cached.as_ref(),
                    days,
                    failure.status(),
                    &failure.detail(),
                );
            }
            Err(_) => {
                return degraded_or_error(
                    &account,
                    &source,
                    cached.as_ref(),
                    days,
                    "timeout",
                    "额度查询超出本轮时间预算",
                );
            }
        };

    // 用量：**独立降级**。这里失败只写进 `usageError`，余额照常返回。
    let (usage, usage_error) =
        match tokio::time::timeout_at(deadline, workbuddy::fetch_usage(&account, &access, days))
            .await
        {
            Ok(Ok(usage)) => (Some(usage), None),
            Ok(Err(failure)) => (None, Some(failure.detail())),
            Err(_) => (None, Some("用量查询超出本轮时间预算".to_string())),
        };

    let row = ok_row(&account, &source, &quota, usage.as_ref(), usage_error);
    // 请求期间账号可能已被删除或重新授权：写入时**原子核验身份**，否则会留下
    // 孤儿快照；账号 id 是 uid 的哈希，重新导入会复用同一个 id。
    app.with_db(|db| {
        db.save_quota_snapshot(
            &account.id,
            &account.provider,
            "ok",
            &row,
            Some(account.auth_epoch),
        )
        .map(|_| ())
    })
    .ok();
    row
}

/// 只重取用量：余额取自 5 分钟内的快照，窗口按本次请求。
///
/// 这条路径存在的唯一原因：`days` 是请求参数，缓存里存的是上次的窗口。
/// 把 7 天的消耗显示成 30 天，比不显示更糟。
async fn usage_only_row(
    app: &Arc<App>,
    account: &AccountRecord,
    snapshot: &crate::storage::quota::QuotaSnapshot,
    days: i64,
    deadline: tokio::time::Instant,
) -> Value {
    let mut row = match cached_row(snapshot, false) {
        Some(row) => row,
        // 快照内容坏掉（不是对象）就退回完整刷新；**必须立刻返回**，
        // 否则会继续走下面的用量请求，白打一次网络还可能把结果又存一遍。
        None => return Box::pin(workbuddy_row(app, account.clone(), days, true, deadline)).await,
    };

    let access = match resolve_access(app, account, deadline).await {
        Ok((source, access)) => {
            row["source"] = json!(source);
            access
        }
        Err(failure) => {
            // 拿不到凭据：余额照旧，用量明确置空。
            row["usage"] = Value::Null;
            row["usageError"] = json!(crate::rpc::sanitize(&failure));
            return row;
        }
    };

    match tokio::time::timeout_at(deadline, workbuddy::fetch_usage(account, &access, days)).await {
        Ok(Ok(usage)) => {
            row["usage"] = usage_json(&usage);
            row["usageError"] = Value::Null;
            // 用新窗口的用量覆盖快照，下一次同窗口查询才能真正命中缓存。
            let epoch = Some(account.auth_epoch);
            app.with_db(|db| {
                db.save_quota_snapshot(&account.id, &account.provider, "ok", &row, epoch)
                    .map(|_| ())
            })
            .ok();
            row
        }
        Ok(Err(failure)) => {
            row["usage"] = Value::Null;
            row["usageError"] = json!(failure.detail());
            row
        }
        Err(_) => {
            row["usage"] = Value::Null;
            row["usageError"] = json!("用量查询超出本轮时间预算");
            row
        }
    }
}

/// 读一次凭据。**钥匙串调用是同步阻塞的**（首次可能弹系统授权框），所以放在
/// `spawn_blocking` 里并同样受 deadline 约束：超时就返回错误，让这一行降级，
/// 而不是把整轮看板卡在这里。
///
/// 注意：超时只是让**调用方**不再等待；被阻塞的那个线程会一直挂着直到用户
/// 回答系统弹窗。这是 macOS 钥匙串的行为，任何实现都绕不开。
async fn resolve_access(
    app: &Arc<App>,
    account: &AccountRecord,
    deadline: tokio::time::Instant,
) -> Result<(String, String), String> {
    let app = Arc::clone(app);
    let owned = account.clone();
    let handle = tokio::task::spawn_blocking(move || broker::resolve(&app, &owned));
    match tokio::time::timeout_at(deadline, handle).await {
        Ok(Ok(Ok(Outcome::Resolved(resolved)))) => Ok((resolved.kind, resolved.access)),
        Ok(Ok(Ok(Outcome::Unavailable { reason, detail }))) => Err(format!("{reason}: {detail}")),
        Ok(Ok(Err(err))) => Err(err.message),
        // 阻塞任务 panic 或超时。
        Ok(Err(err)) => Err(format!("凭据读取异常：{err}")),
        Err(_) => Err("凭据读取超出本轮时间预算（可能在等待钥匙串授权）".to_string()),
    }
}

/// 成功行。额度一定有，用量可能没有（`usageError` 单独说明）。
fn ok_row(
    account: &AccountRecord,
    source: &str,
    quota: &workbuddy::Quota,
    usage: Option<&workbuddy::Usage>,
    usage_error: Option<String>,
) -> Value {
    json!({
        "accountId": account.id,
        "provider": account.provider,
        "nickname": account.nickname,
        "source": source,
        "status": "ok",
        "unit": quota.unit,
        "total": quota.total,
        "used": quota.used,
        "remaining": quota.remaining,
        "plan": quota.plan_name,
        "isPaid": quota.is_paid,
        "usage": usage.map(usage_json),
        "usageError": usage_error,
        "syncAt": crate::clock::now_rfc3339(),
        "error": Value::Null,
        "cached": false,
        "stale": false,
    })
}

fn usage_json(usage: &workbuddy::Usage) -> Value {
    json!({
        "windowDays": usage.window_days,
        "requests": usage.requests,
        "credits": usage.credits,
        "totalReported": usage.total_reported,
        "truncated": usage.truncated,
        "models": usage
            .models
            .iter()
            .map(|(name, count, credit)| json!({
                "model": name, "requests": count, "credits": credit,
            }))
            .collect::<Vec<_>>(),
    })
}

/// 快照里那行原样返回，只改写"这份数据现在是什么状态"。
fn cached_row(snapshot: &crate::storage::quota::QuotaSnapshot, stale: bool) -> Option<Value> {
    let mut row = parse_payload(&snapshot.payload);
    if !row.is_object() {
        return None;
    }
    row["cached"] = json!(true);
    row["stale"] = json!(stale);
    // 界面上"这份数据是什么时候的"必须来自快照本身，不能靠行的 syncAt 反推。
    row["capturedAt"] = json!(snapshot.captured_at);
    Some(row)
}

/// 取数失败：有旧快照就降级成 `stale`，没有才真正报错。
///
/// 降级同样要守**窗口规则**：旧快照里的用量属于它自己那次统计窗口，
/// 与本次请求的 `days` 不一致时必须置空，否则界面会把 7 天的消耗挂在 30 天的查询下。
fn degraded_or_error(
    account: &AccountRecord,
    source: &str,
    cached: Option<&crate::storage::quota::QuotaSnapshot>,
    days: i64,
    reason: &str,
    detail: &str,
) -> Value {
    if let Some(snapshot) = cached {
        if let Some(mut row) = cached_row(snapshot, true) {
            // 旧数据照常显示，但必须说清楚"这是旧的、这次为什么没取到"。
            row["status"] = json!("stale");
            row["error"] = json!(crate::rpc::sanitize(detail));
            row["reason"] = json!(reason);
            drop_mismatched_usage(&mut row, days);
            return row;
        }
    }
    error_row(account, source, status_for(reason), reason, detail)
}

/// 旧行的用量窗口与本次请求不符时清空用量并说明。余额不受影响。
fn drop_mismatched_usage(row: &mut Value, days: i64) {
    let cached_window = row["usage"]["windowDays"].as_i64();
    if row["usage"].is_null() || cached_window == Some(days) {
        return;
    }
    let explain = match cached_window {
        Some(window) => {
            format!("旧快照的用量是 {window} 天窗口，与本次请求的 {days} 天不一致，已隐藏")
        }
        None => format!("旧快照没有可用的 {days} 天用量，已隐藏"),
    };
    row["usage"] = Value::Null;
    row["usageError"] = json!(explain);
}

/// `reason` 来自 `Failure::status()` 或本地分类，这里只做一次归一。
fn status_for(reason: &str) -> &str {
    match reason {
        "unauthorized" | "forbidden" | "rate_limited" | "timeout" => reason,
        _ => "error",
    }
}

/// 失败行只返回不落库：一次临时 429 不该变成"历史记录"。
fn error_row(
    account: &AccountRecord,
    source: &str,
    status: &str,
    reason: &str,
    detail: &str,
) -> Value {
    json!({
        "accountId": account.id,
        "provider": account.provider,
        "nickname": account.nickname,
        "source": source,
        "status": status,
        "reason": reason,
        "unit": Value::Null,
        "total": Value::Null,
        "used": Value::Null,
        "remaining": Value::Null,
        "plan": Value::Null,
        "isPaid": Value::Null,
        "usage": Value::Null,
        "usageError": Value::Null,
        "syncAt": crate::clock::now_rfc3339(),
        "error": crate::rpc::sanitize(detail),
        "cached": false,
        "stale": false,
    })
}

/// 快照里那次用量统计覆盖了多少天。没有用量（上次用量取数失败）时返回 `None`，
/// 这样下一轮会重取用量而不是把"空"当成命中。
fn cached_usage_window(snapshot: &crate::storage::quota::QuotaSnapshot) -> Option<i64> {
    parse_payload(&snapshot.payload)["usage"]["windowDays"].as_i64()
}

/// 快照距今多少秒。时间戳解析不了就当作"很旧"，宁可多打一次网络。
fn snapshot_age_secs(captured_at: &str) -> i64 {
    match chrono::DateTime::parse_from_rfc3339(captured_at) {
        Ok(at) => (chrono::Utc::now() - at.with_timezone(&chrono::Utc))
            .num_seconds()
            .max(0),
        Err(_) => i64::MAX,
    }
}

fn parse_payload(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap_or(Value::Null)
}

/// 某个账号的历史快照，按时间升序。
pub fn history(app: &App, params: HistoryParams) -> RpcResult<Value> {
    let days = params.days.unwrap_or(30).clamp(1, HISTORY_KEEP_DAYS);
    let since = crate::clock::now_rfc3339_at_days_ago(days);
    let rows = app.with_db(|db| db.quota_history(&params.account_id, &since))?;
    let points: Vec<Value> = rows
        .iter()
        .filter_map(|row| {
            let mut point = serde_json::from_str::<Value>(&row.payload).ok()?;
            // 时间与状态以**表里的列**为准：payload 是抓取当时的快照，
            // 而这两列是索引，历史查询就是靠它们排序与过滤的。
            point["capturedAt"] = json!(row.captured_at);
            point["provider"] = json!(row.provider);
            point["status"] = json!(row.status);
            Some(point)
        })
        .collect();
    Ok(json!({
        "accountId": params.account_id,
        "days": days,
        "points": points,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::quota::QuotaSnapshot;

    fn account() -> AccountRecord {
        AccountRecord {
            id: "a1".into(),
            platform: "workbuddy".into(),
            uid: "u1".into(),
            nickname: "测试".into(),
            email: None,
            group_id: None,
            auth_status: "valid".into(),
            checkin_enabled: false,
            last_checkin_at: None,
            last_result: None,
            updated_at: "2026-11-14T00:00:00Z".into(),
            token_expires_at: None,
            auth_domain: Some("www.codebuddy.cn".into()),
            auth_source: "magpie".into(),
            travel_enabled: false,
            auth_epoch: 0,
            auth_mirror_auth_id: None,
            provider: "workbuddy".into(),
        }
    }

    fn quota() -> workbuddy::Quota {
        workbuddy::Quota {
            unit: "credits".into(),
            total: Some(100.0),
            used: Some(10.0),
            remaining: Some(90.0),
            plan_name: Some("体验版".into()),
            is_paid: Some(false),
        }
    }

    fn snapshot(payload: Value) -> QuotaSnapshot {
        QuotaSnapshot {
            captured_at: crate::clock::now_rfc3339(),
            provider: "workbuddy".into(),
            status: "ok".into(),
            payload: payload.to_string(),
        }
    }

    #[test]
    fn usage_failure_keeps_the_balance() {
        // 额度成功、用量 429：余额必须在，错误单独报告。
        let row = ok_row(
            &account(),
            "magpie",
            &quota(),
            None,
            Some("被限流（429），稍后再试".into()),
        );
        assert_eq!(row["status"], "ok");
        assert_eq!(row["remaining"], 90.0);
        assert!(row["usage"].is_null());
        assert_eq!(row["usageError"], "被限流（429），稍后再试");
        // 用量失败不该把整行标成失败。
        assert!(row["error"].is_null());
    }

    #[test]
    fn refresh_failure_returns_the_old_row_marked_stale() {
        let cached = snapshot(json!({
            "accountId": "a1", "provider": "workbuddy", "status": "ok",
            "unit": "credits", "total": 100.0, "remaining": 90.0,
        }));
        let row = degraded_or_error(
            &account(),
            "magpie",
            Some(&cached),
            7,
            "rate_limited",
            "429",
        );
        assert_eq!(row["status"], "stale");
        assert_eq!(row["stale"], true);
        assert_eq!(row["cached"], true);
        // 旧数据还在，但原因写得清楚。
        assert_eq!(row["remaining"], 90.0);
        assert_eq!(row["error"], "429");
        assert_eq!(row["reason"], "rate_limited");
    }

    /// 降级到旧快照时也要守窗口：30 天的请求不能把 7 天的消耗一起端上来。
    #[test]
    fn a_stale_row_hides_usage_from_another_window() {
        let cached = snapshot(json!({
            "accountId": "a1", "provider": "workbuddy", "status": "ok",
            "unit": "credits", "total": 100.0, "remaining": 90.0,
            "usage": {"windowDays": 7, "requests": 42, "credits": 1.5},
        }));

        let row = degraded_or_error(
            &account(),
            "magpie",
            Some(&cached),
            30,
            "rate_limited",
            "429",
        );
        assert_eq!(row["status"], "stale");
        assert_eq!(row["remaining"], 90.0, "余额照旧保留");
        assert!(row["usage"].is_null(), "七天用量不能挂在三十天窗口下");
        let why = row["usageError"].as_str().unwrap_or_default();
        assert!(why.contains("7") && why.contains("30"), "{why}");

        // 窗口一致时旧用量可以照常显示。
        let same = degraded_or_error(
            &account(),
            "magpie",
            Some(&cached),
            7,
            "rate_limited",
            "429",
        );
        assert_eq!(same["usage"]["windowDays"], 7);
        assert_eq!(same["usage"]["requests"], 42);
    }

    /// 快照内容坏掉时退回完整刷新，并**直接返回**那条结果。
    #[tokio::test]
    async fn a_broken_snapshot_falls_back_to_a_full_refresh() {
        let (app, _dir) = test_app();
        let record = app
            .with_db(|db| {
                db.upsert_account(&crate::storage::AccountInput {
                    platform: "workbuddy".into(),
                    uid: "u-broken".into(),
                    nickname: None,
                    email: None,
                    group_id: None,
                    auth_ref: None,
                    profile_ref: None,
                    auth_status: None,
                    checkin_enabled: None,
                    token_expires_at: None,
                    auth_domain: None,
                    auth_source: None,
                    travel_enabled: None,
                    provider: Some("workbuddy".into()),
                })
            })
            .unwrap();
        // 存一条"不是对象"的 payload：解析成功但不是可用的行。
        app.with_db(|db| {
            db.save_quota_snapshot(&record.id, "workbuddy", "ok", &json!([1, 2]), None)
                .map(|_| ())
        })
        .unwrap();

        // 预算已耗尽 → 完整刷新路径立刻返回 timeout 行（不会去发用量请求）。
        let deadline = tokio::time::Instant::now();
        let row = workbuddy_row(&app, record, 7, false, deadline).await;
        assert_eq!(row["status"], "timeout");
        assert_eq!(row["cached"], false, "坏缓存不能被当命中");
    }

    #[test]
    fn failure_without_a_snapshot_is_a_real_error() {
        let row = degraded_or_error(&account(), "magpie", None, 7, "unauthorized", "凭据不在");
        assert_eq!(row["status"], "unauthorized");
        assert!(row["remaining"].is_null());
        assert_eq!(row["stale"], false);
    }

    #[test]
    fn status_never_invents_new_values() {
        assert_eq!(status_for("unauthorized"), "unauthorized");
        assert_eq!(status_for("rate_limited"), "rate_limited");
        assert_eq!(status_for("timeout"), "timeout");
        assert_eq!(status_for("whatever"), "error");
    }

    #[test]
    fn cache_ttl_accepts_fresh_and_rejects_old() {
        let now = crate::clock::now_rfc3339();
        assert!(snapshot_age_secs(&now) <= CACHE_TTL_SECS);
        let old = crate::clock::now_rfc3339_at_days_ago(1);
        assert!(snapshot_age_secs(&old) > CACHE_TTL_SECS);
        // 时间戳坏掉时当作"很旧"，宁可多打一次网络。
        assert_eq!(snapshot_age_secs("not-a-time"), i64::MAX);
    }

    fn test_app() -> (Arc<App>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let paths = crate::paths::Paths {
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data.clone(),
            db_path: data.join("accounts.sqlite"),
            wb_home_owned: true,
        };
        let app = Arc::new(App::with_secure(
            paths,
            crate::securestore::SecureStore::memory(),
        ));
        app.with_db(|db| Ok(db.migrate()?)).expect("migrate");
        (app, dir)
    }

    /// 预算用完时**一个请求都不发**，直接返回 timeout（没有快照就是失败行）。
    #[tokio::test]
    async fn expired_budget_returns_timeout_without_any_request() {
        let (app, _dir) = test_app();
        let deadline = tokio::time::Instant::now();
        let row = workbuddy_row(&app, account(), 7, true, deadline).await;
        assert_eq!(row["status"], "timeout");
        assert!(row["remaining"].is_null());
        assert_eq!(row["stale"], false);
    }

    #[test]
    fn cached_usage_window_comes_from_the_payload() {
        let with_usage = snapshot(json!({"usage": {"windowDays": 7, "requests": 3}}));
        assert_eq!(cached_usage_window(&with_usage), Some(7));
        // 上次用量没取到 → 没有窗口 → 这一轮必须重取，不能当成命中。
        let no_usage = snapshot(json!({"usage": null}));
        assert_eq!(cached_usage_window(&no_usage), None);
        assert_eq!(cached_usage_window(&snapshot(json!({}))), None);
    }

    /// 5 分钟内但窗口不同：**余额可复用，用量必须置空重取**，
    /// 绝不能把 7 天的消耗显示成 30 天的。
    #[tokio::test]
    async fn a_different_window_never_reuses_the_cached_usage() {
        let (app, _dir) = test_app();
        let record = app
            .with_db(|db| {
                db.upsert_account(&crate::storage::AccountInput {
                    platform: "workbuddy".into(),
                    uid: "u-window".into(),
                    nickname: None,
                    email: None,
                    group_id: None,
                    auth_ref: None,
                    profile_ref: None,
                    auth_status: None,
                    checkin_enabled: None,
                    token_expires_at: None,
                    auth_domain: None,
                    auth_source: None,
                    travel_enabled: None,
                    provider: Some("workbuddy".into()),
                })
            })
            .unwrap();

        // 先放一条「7 天窗口」的新鲜快照。
        let payload = json!({
            "accountId": record.id, "provider": "workbuddy", "status": "ok",
            "unit": "credits", "remaining": 90.0,
            "usage": {"windowDays": 7, "requests": 42, "credits": 1.5},
            "cached": false, "stale": false,
        });
        app.with_db(|db| {
            db.save_quota_snapshot(&record.id, "workbuddy", "ok", &payload, None)
                .map(|_| ())
        })
        .unwrap();

        // 同窗口 → 命中缓存，用量原样返回。
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        let hit = workbuddy_row(&app, record.clone(), 7, false, deadline).await;
        assert_eq!(hit["cached"], true);
        assert_eq!(hit["usage"]["windowDays"], 7);
        assert_eq!(hit["usage"]["requests"], 42);

        // 换窗口 → 不许复用 7 天的用量。这里没有凭据，所以用量必须是空 + 有原因，
        // 余额仍然从缓存里来。
        let miss = workbuddy_row(&app, record.clone(), 30, false, deadline).await;
        assert_eq!(miss["remaining"], 90.0, "余额可以复用");
        assert!(miss["usage"].is_null(), "七天用量不能冒充三十天");
        assert!(
            !miss["usageError"].as_str().unwrap_or_default().is_empty(),
            "用量被弃用时要说明原因"
        );
    }

    #[test]
    fn wanted_filters_by_provider_and_account() {
        assert!(wanted(None, "workbuddy", None, "a1"));
        assert!(wanted(Some("workbuddy"), "workbuddy", None, "a1"));
        assert!(!wanted(Some("workbuddy_ai"), "workbuddy", None, "a1"));
        assert!(wanted(None, "workbuddy", Some("a1"), "a1"));
        assert!(!wanted(None, "workbuddy", Some("a2"), "a1"));
    }
}
