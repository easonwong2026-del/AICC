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

use serde::Deserialize;
use serde_json::{json, Value};

use crate::app::App;
use crate::auth::broker::{self, Outcome};
use crate::rpc::{RpcError, RpcResult};
use crate::storage::{AccountFilter, AccountRecord};

/// 看板允许出现的 provider。顺序即界面顺序。
pub const PROVIDERS: &[&str] = &["workbuddy", "workbuddy_ai", "qoder_cn", "qoder_global"];

/// 历史缓存保留天数（与 `credit_usage` 的上游默认一致）。
const HISTORY_KEEP_DAYS: i64 = 90;

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

/// 组装看板。`refresh=false` 时如果已有缓存就直接用缓存，省掉一次网络。
pub async fn board(app: &App, params: BoardParams) -> RpcResult<Value> {
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

    for account in &workbuddy_accounts {
        if !wanted(
            params.provider.as_deref(),
            &account.provider,
            params.account_id.as_deref(),
            &account.id,
        ) {
            continue;
        }
        rows.push(workbuddy_row(app, account, days, params.refresh.unwrap_or(false)).await);
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
        PROVIDERS
            .iter()
            .position(|p| *p == provider)
            .unwrap_or(PROVIDERS.len())
    });

    Ok(json!({
        "rows": rows,
        "days": days,
        "today": crate::clock::beijing_date(),
        // 明确告诉界面：这些平台一个网络请求都没发，不是"暂时取不到"。
        "unverifiedProviders": qoder::PROVIDERS,
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

async fn workbuddy_row(app: &App, account: &AccountRecord, days: i64, refresh: bool) -> Value {
    // 没缓存时即使没要求刷新也要取一次，否则首次打开看板永远是空的。
    let cached = app
        .with_db(|db| db.latest_quota_snapshot(&account.id))
        .ok()
        .flatten();
    if !refresh {
        if let Some(snapshot) = cached {
            let mut row = parse_payload(&snapshot.payload);
            if row.is_object() {
                row["cached"] = json!(true);
                // 界面上"这份数据是什么时候的"必须来自快照本身，不能靠行的 syncAt
                // 反推——失败行也带 syncAt，那不是额度的时间。
                row["capturedAt"] = json!(snapshot.captured_at);
                return row;
            }
        }
    }

    let resolved = broker::resolve(app, account);
    let (source, access) = match resolved {
        Ok(Outcome::Resolved(resolved)) => (resolved.kind.clone(), resolved.access),
        Ok(Outcome::Unavailable { reason, detail }) => {
            return error_row(
                account,
                &account.auth_source,
                "unauthorized",
                &reason,
                &detail,
            );
        }
        Err(err) => {
            return error_row(
                account,
                &account.auth_source,
                "error",
                "broker",
                &err.message,
            );
        }
    };

    match workbuddy::fetch(account, &access, days).await {
        Ok((quota, usage)) => {
            let row = json!({
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
                "usage": usage.map(|u| json!({
                    "windowDays": u.window_days,
                    "requests": u.requests,
                    "credits": u.credits,
                    "totalReported": u.total_reported,
                    "truncated": u.truncated,
                    "models": u.models.iter().map(|(name, count, credit)| json!({
                        "model": name, "requests": count, "credits": credit,
                    })).collect::<Vec<_>>(),
                })),
                "syncAt": crate::clock::now_rfc3339(),
                "error": Value::Null,
                "cached": false,
            });
            app.with_db(|db| db.save_quota_snapshot(&account.id, &account.provider, "ok", &row))
                .ok();
            row
        }
        Err(failure) => error_row(
            account,
            &source,
            failure.status(),
            "fetch",
            &failure.detail(),
        ),
    }
}

/// 失败行也要落缓存：界面要能看到"上次为什么没取到"，而不是一片空白。
fn error_row(
    account: &AccountRecord,
    source: &str,
    status: &str,
    reason: &str,
    detail: &str,
) -> Value {
    // 失败行只返回不落库：一次临时 429 不该变成"历史记录"。

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
        "syncAt": crate::clock::now_rfc3339(),
        "error": crate::rpc::sanitize(detail),
        "cached": false,
    })
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
