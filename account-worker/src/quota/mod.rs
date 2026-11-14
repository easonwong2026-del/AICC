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
        rows.push(qoder::unverified_row(
            &account.id,
            provider,
            &account.nickname,
            &source,
            QODER_BLOCKER,
        ));
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

    Ok(json!({
        "rows": rows,
        "days": days,
        "today": crate::clock::beijing_date(),
        "cacheTtlSeconds": CACHE_TTL_SECS,
        // 明确告诉界面：这些平台一个网络请求都没发，不是"暂时取不到"。
        "unverifiedProviders": qoder::PROVIDERS,
        "budgetSeconds": BOARD_BUDGET.as_secs(),
    }))
}

/// Qoder 的阻塞原因。写死在代码里而不是从数据库读：它是能力事实，不是配置。
const QODER_BLOCKER: &str = "Qoder 官方 OIDC 未提供公开客户端注册（token 端点无 none 认证方式），\
且 Credits 业务接口无公开文档与成功样本；本版零请求，不显示为 0";

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
/// 三条降级规则：
/// 1. 缓存在 5 分钟内 → 直接用，不打网络；
/// 2. 过期或要求刷新 → 重新取，**额度与用量各自独立成败**；
/// 3. 取数失败但有旧快照 → 返回旧数据并标 `stale`，不把有用的数丢掉。
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
        if let Some(snapshot) = cached.as_ref() {
            if snapshot_age_secs(&snapshot.captured_at) <= CACHE_TTL_SECS {
                if let Some(row) = cached_row(snapshot, false) {
                    return row;
                }
            }
        }
    }

    // 预算已经用完：不再发起请求，用旧数据（如果有）标过期返回。
    if tokio::time::Instant::now() >= deadline {
        return degraded_or_error(
            &account,
            &account.auth_source,
            cached.as_ref(),
            "timeout",
            "本轮刷新超出时间预算，未查询该账号",
        );
    }

    let (source, access) = match broker::resolve(app, &account) {
        Ok(Outcome::Resolved(resolved)) => (resolved.kind.clone(), resolved.access),
        Ok(Outcome::Unavailable { reason, detail }) => {
            return degraded_or_error(
                &account,
                &account.auth_source,
                cached.as_ref(),
                "unauthorized",
                &format!("{reason}: {detail}"),
            );
        }
        Err(err) => {
            return degraded_or_error(
                &account,
                &account.auth_source,
                cached.as_ref(),
                "error",
                &err.message,
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
                    failure.status(),
                    &failure.detail(),
                );
            }
            Err(_) => {
                return degraded_or_error(
                    &account,
                    &source,
                    cached.as_ref(),
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
    app.with_db(|db| db.save_quota_snapshot(&account.id, &account.provider, "ok", &row))
        .ok();
    row
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
fn degraded_or_error(
    account: &AccountRecord,
    source: &str,
    cached: Option<&crate::storage::quota::QuotaSnapshot>,
    reason: &str,
    detail: &str,
) -> Value {
    if let Some(snapshot) = cached {
        if let Some(mut row) = cached_row(snapshot, true) {
            // 旧数据照常显示，但必须说清楚"这是旧的、这次为什么没取到"。
            row["status"] = json!("stale");
            row["error"] = json!(crate::rpc::sanitize(detail));
            row["reason"] = json!(reason);
            return row;
        }
    }
    error_row(account, source, status_for(reason), reason, detail)
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
        let row = degraded_or_error(&account(), "magpie", Some(&cached), "rate_limited", "429");
        assert_eq!(row["status"], "stale");
        assert_eq!(row["stale"], true);
        assert_eq!(row["cached"], true);
        // 旧数据还在，但原因写得清楚。
        assert_eq!(row["remaining"], 90.0);
        assert_eq!(row["error"], "429");
        assert_eq!(row["reason"], "rate_limited");
    }

    #[test]
    fn failure_without_a_snapshot_is_a_real_error() {
        let row = degraded_or_error(&account(), "magpie", None, "unauthorized", "凭据不在");
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
    fn wanted_filters_by_provider_and_account() {
        assert!(wanted(None, "workbuddy", None, "a1"));
        assert!(wanted(Some("workbuddy"), "workbuddy", None, "a1"));
        assert!(!wanted(Some("workbuddy_ai"), "workbuddy", None, "a1"));
        assert!(wanted(None, "workbuddy", Some("a1"), "a1"));
        assert!(!wanted(None, "workbuddy", Some("a2"), "a1"));
    }
}
