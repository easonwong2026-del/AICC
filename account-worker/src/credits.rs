//! Qoder 每日 Credits：查询 / 手动领取 / 自动领取（PR #75）。
//!
//! 协议解析在 `providers::qoder_credits`，这里只做三件事：挑凭据、决定做什么、
//! 把结果记进 `qoder_credit_runs`。**它不缓存活动状态**——能不能领永远以服务端
//! 当次返回为准。
//!
//! ## 四条纪律
//!
//! 1. **International 不发请求**。没有证据的接口不猜，界面不显示领取按钮。
//! 2. **401 / 403 不重试**。授权问题重试不会变好，只会多打几次接口。
//! 3. **POST 超时先回查，不重发**。领取是幂等的，但盲目重发会让"到底领了几次"
//!    无法回答；回查活动列表能给出确定结论。
//! 4. **自动领取默认关闭**，每账号独立开关；同一活动周期成功过一次就不再提交。

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::App;
use crate::providers::qoder_auth::access_key;
use crate::providers::qoder_credits::{self as api, CreditsError, Snapshot};
use crate::providers::qoder_edition::QoderEdition;
use crate::providers::qoder_pat;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::qoder_accounts::{QoderAccountRecord, SOURCE_OWN_BROWSER, SOURCE_OWN_PAT};
use crate::storage::qoder_credits::{RunInput, OUTCOMES};

/// 自动领取同一账号两次尝试之间的最小间隔。
///
/// 参考实现是一天跑一次；AICC 有心跳，所以必须自己限速：20 分钟一次，
/// 一天最多 70 次左右的 GET，且只要当天已经领到就立刻停手。
const AUTO_MIN_INTERVAL: Duration = Duration::from_secs(20 * 60);

static LAST_ATTEMPT: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn last_attempt() -> &'static Mutex<HashMap<String, Instant>> {
    LAST_ATTEMPT.get_or_init(|| Mutex::new(HashMap::new()))
}

fn throttled(account_id: &str, force: bool) -> bool {
    if force {
        return false;
    }
    let map = last_attempt().lock().expect("last_attempt");
    match map.get(account_id) {
        Some(at) => at.elapsed() < AUTO_MIN_INTERVAL,
        None => false,
    }
}

fn mark_attempt(account_id: &str) {
    if let Ok(mut map) = last_attempt().lock() {
        map.insert(account_id.to_string(), Instant::now());
    }
}

// ---------------------------------------------------------------------------
// 账号级领取互斥
// ---------------------------------------------------------------------------

/// 手动与自动**共用同一把锁**。否则用户点「立即领取」的同一秒心跳也跑起来，
/// 两个请求会同时打同一个活动：服务端幂等会让其中一个变成 `replayed`，
/// 但"到底提交了几次"就再也说不清了。
static CLAIMING: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

fn claiming() -> &'static Mutex<HashSet<String>> {
    CLAIMING.get_or_init(|| Mutex::new(HashSet::new()))
}

struct ClaimLock {
    account_id: String,
}

impl Drop for ClaimLock {
    fn drop(&mut self) {
        if let Ok(mut set) = claiming().lock() {
            set.remove(&self.account_id);
        }
    }
}

/// 拿不到就说明这个账号已经有一次领取在跑。`None` 是正常结果，不是错误。
fn try_claim_lock(account_id: &str) -> Option<ClaimLock> {
    let mut set = claiming().lock().ok()?;
    if !set.insert(account_id.to_string()) {
        return None;
    }
    Some(ClaimLock {
        account_id: account_id.to_string(),
    })
}

// ---------------------------------------------------------------------------
// 凭据
// ---------------------------------------------------------------------------

struct Resolved {
    source: &'static str,
    token: String,
}

impl Resolved {
    fn to_json(&self) -> Value {
        json!(self.source)
    }
}

/// 挑一份能用的凭据：**浏览器授权优先，PAT 兜底**。
///
/// 只有 Magpie 关联的账号一个请求都不发：那份令牌归 Magpie，AICC 手里没有凭据，
/// 说"领取失败"会让人以为是权限问题，其实是"没有东西可发"。
fn resolve_token(app: &App, account: &QoderAccountRecord) -> Result<Resolved, String> {
    let rows = app
        .with_db(|db| db.list_qoder_authorizations(&account.id))
        .map_err(|err| err.message)?;

    let usable = |source: &str| {
        rows.iter().any(|row| {
            row.source == source
                && row.secret_ref.is_some()
                && row.auth_status != "invalid"
                && row.auth_status != "denied"
        })
    };

    if usable(SOURCE_OWN_BROWSER) {
        let key = access_key(&account.id);
        if let Ok(bytes) = app.secure.get(&key) {
            if let Ok(token) = String::from_utf8(bytes) {
                if !token.trim().is_empty() {
                    return Ok(Resolved {
                        source: SOURCE_OWN_BROWSER,
                        token,
                    });
                }
            }
        }
        // 授权行说有凭据、钥匙串里读不到：明确说出来，不要悄悄换 PAT。
        return Err("浏览器授权的凭据读不到（钥匙串里没有），请重新授权".to_string());
    }

    if usable(SOURCE_OWN_PAT) {
        match qoder_pat::read_secret(app, &account.id) {
            Ok(token) if !token.trim().is_empty() => {
                return Ok(Resolved {
                    source: SOURCE_OWN_PAT,
                    token,
                })
            }
            Ok(_) => return Err("PAT 为空，请重新保存".to_string()),
            Err(err) => return Err(err.message),
        }
    }

    if rows.iter().any(|row| row.source == "magpie") {
        return Err(
            "这个账号只有 Magpie 关联，没有 AICC 自己的凭据；请先浏览器授权或保存 PAT".to_string(),
        );
    }
    Err("还没有可用的授权来源".to_string())
}

// ---------------------------------------------------------------------------
// 查询
// ---------------------------------------------------------------------------

pub fn account(app: &App, account_id: &str) -> RpcResult<QoderAccountRecord> {
    app.with_db(|db| db.get_qoder_account(account_id))?
        .ok_or_else(|| RpcError::not_found(format!("Qoder 账号不存在：{account_id}")))
}

fn edition_of(account: &QoderAccountRecord) -> RpcResult<QoderEdition> {
    QoderEdition::parse(&account.edition)
        .ok_or_else(|| RpcError::invalid_params("账号的 edition 非法"))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 活动快照 → 对外 JSON。`state` 与数字必须能自洽：
/// 没有活动就 `nothing`，有活动就给出真实计数，绝不把未知写成 0。
fn snapshot_json(account: &QoderAccountRecord, snapshot: &Snapshot, now: i64) -> Value {
    let todo = &snapshot.todo;
    let claimed = &snapshot.claimed;
    let today = todo
        .first()
        .or_else(|| claimed.first())
        .or_else(|| snapshot.pending.first());

    json!({
        "accountId": account.id,
        "edition": account.edition,
        "state": if todo.is_empty() {
            if claimed.is_empty() { "nothing" } else { "already" }
        } else {
            "claimable"
        },
        "claimable": todo.len(),
        "claimed": claimed.len(),
        "pending": snapshot.pending.len(),
        "today": today.map(|campaign| json!({
            "campaignId": campaign.id,
            "campaignKey": campaign.key,
            "credits": campaign.credits,
            "claimStatus": campaign.claim_status,
            "windowStart": campaign.start_at,
            "windowEnd": campaign.end_at,
            "validityDays": campaign.validity_days,
            "active": campaign.is_active(now),
        })),
        "campaigns": snapshot.campaigns.iter().map(|c| c.to_json()).collect::<Vec<_>>(),
        "checkedAt": crate::clock::now_rfc3339(),
    })
}

/// 只读查询。**不发任何写请求**，也不改动账号或授权状态。
pub async fn status(app: &Arc<App>, account_id: &str) -> RpcResult<Value> {
    let account = account(app, account_id)?;
    let edition = edition_of(&account)?;

    if api::capability(edition).is_none() {
        // 没有适配器：零请求，并把原因说清楚（这是"未验证"，不是"没有活动"）。
        return Ok(json!({
            "accountId": account.id,
            "edition": account.edition,
            "supported": false,
            "state": "unsupported",
            "claimable": Value::Null,
            "claimed": Value::Null,
            "today": Value::Null,
            "error": { "kind": "unsupported", "message": api::unsupported_reason(edition) },
            "checkedAt": crate::clock::now_rfc3339(),
        }));
    }

    let resolved = match resolve_token(app, &account) {
        Ok(resolved) => resolved,
        Err(detail) => {
            return Ok(json!({
                "accountId": account.id,
                "edition": account.edition,
                "supported": true,
                "state": "no_credential",
                "authSource": Value::Null,
                "claimable": Value::Null,
                "claimed": Value::Null,
                "today": Value::Null,
                "error": { "kind": "no_credential", "message": crate::rpc::sanitize(&detail) },
                "checkedAt": crate::clock::now_rfc3339(),
            }))
        }
    };

    // 请求发起时的授权版本：401 回来时要确认"还是这份授权"。
    let epoch = account.auth_epoch;

    match api::fetch_campaigns(edition, &resolved.token).await {
        Ok(body) => {
            let now = now_secs();
            // 结构不认识一律 `error`，**不许**退化成"今天没有活动"。
            match api::parse_campaigns(&body) {
                Ok(campaigns) => {
                    let snapshot = api::classify(campaigns, now);
                    let mut value = snapshot_json(&account, &snapshot, now);
                    value["supported"] = json!(true);
                    value["authSource"] = resolved.to_json();
                    value["refreshHint"] = json!(
                        "以服务端活动窗口（windowEnd）为准；参考实现说北京时间每天 10:00 刷新，仅作说明"
                    );
                    Ok(value)
                }
                Err(err) => Ok(failure_status(&account, &resolved, &err)),
            }
        }
        Err(err) => {
            note_auth_failure(app, &account, &resolved, &err, epoch);
            Ok(failure_status(&account, &resolved, &err))
        }
    }
}

/// 查询失败时的统一形状。`state` 只有三种：`unauthorized` / `unsupported` / `error`。
fn failure_status(account: &QoderAccountRecord, resolved: &Resolved, err: &CreditsError) -> Value {
    json!({
        "accountId": account.id,
        "edition": account.edition,
        "supported": true,
        "state": if matches!(err, CreditsError::Unauthorized) { "unauthorized" } else { "error" },
        "authSource": resolved.to_json(),
        "claimable": Value::Null,
        "claimed": Value::Null,
        "today": Value::Null,
        "error": { "kind": err.kind(), "message": err.detail() },
        "checkedAt": crate::clock::now_rfc3339(),
    })
}

/// 只有 **401** 才把来源标成 `invalid`，而且必须核对授权版本。
///
/// 两层理由：
/// 1. **403 不是"凭据坏了"**，是权限范围问题（PAT 没勾对应权限、或触发了风控）。
///    标成 invalid 会让用户白去重新生成一张本来没问题的令牌。
/// 2. 一次请求要几秒，期间用户可能已经换了新 PAT。旧请求的结果不能把新授权标脏，
///    所以写入走 `set_qoder_auth_status_if_current(.., epoch)`，对不上就什么都不写。
///
/// 不删凭据：用户可能只是 PAT 被撤销，删掉就再也查不出原因了。
fn note_auth_failure(
    app: &App,
    account: &QoderAccountRecord,
    resolved: &Resolved,
    err: &CreditsError,
    epoch: i64,
) {
    if matches!(err, CreditsError::Unauthorized) {
        app.with_db(|db| {
            db.set_qoder_auth_status_if_current(&account.id, resolved.source, "invalid", epoch)
        })
        .ok();
    }
}

// ---------------------------------------------------------------------------
// 领取
// ---------------------------------------------------------------------------

fn record(app: &App, input: RunInput<'_>) {
    // 记录失败不该打断领取本身：结果已经拿到了，账可以补，但不能因此报错。
    // 但**必须留痕**：账写不进去，去重与"今天领过没"会一起失效，
    // 而界面上什么都看不出来。日志只带错误类与消息，不带令牌/URL。
    if let Err(err) = app.with_db(|db| db.record_qoder_credit_run(input).map(|_| ())) {
        crate::handlers::log_note(&format!(
            "Qoder 领取记录写入失败（领取结果已产生，去重可能失效）：{}",
            err.message
        ));
    }
}

/// 每次 POST 之前必须重新确认的三件事：**账号还在、授权没换、开关还开着**。
///
/// 前两者是同一个网络等待窗口里的两个事故：用户可能在等结果时删了账号，
/// 也可能换了一份新 PAT。任一发生，都不能再拿旧快照里的活动去提交——
/// 提交的是旧凭据名下的活动，新授权会莫名其妙多出一笔领取记录。
///
/// 返回 `Some(reason)` 表示必须停手，理由直接进结果给人看。
fn pre_post_stop(app: &App, account_id: &str, trigger: &str, epoch: i64) -> Option<&'static str> {
    let current = app
        .with_db(|db| db.qoder_auth_epoch(account_id))
        .ok()
        .flatten();
    let Some(current) = current else {
        return Some("account_removed");
    };
    if current != epoch {
        return Some("authorization_changed");
    }
    if trigger == "auto" {
        let still_on = app
            .with_db(|db| db.qoder_credits_auto(account_id))
            .unwrap_or(false);
        if !still_on {
            return Some("auto_disabled");
        }
    }
    None
}

// ---------------------------------------------------------------------------
// 可注入的网络层
// ---------------------------------------------------------------------------

/// 领取流程用到的两个网络动作。
///
/// 抽出来只为一件事：**让"401 之后不再提交""POST 超时先回查""逐活动去重"
/// 这几条能被测试**。真实实现是 `Live`，测试用脚本化的假 transport，
/// 生产路径一个分支都不改。
pub trait Transport: Send + Sync {
    fn campaigns<'a>(
        &'a self,
        edition: QoderEdition,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Value, CreditsError>> + Send + 'a>>;

    fn claim<'a>(
        &'a self,
        edition: QoderEdition,
        token: &'a str,
        campaign_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<api::Claimed, CreditsError>> + Send + 'a>>;
}

/// 生产实现：直接打官方接口。
pub struct Live;

impl Transport for Live {
    fn campaigns<'a>(
        &'a self,
        edition: QoderEdition,
        token: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Value, CreditsError>> + Send + 'a>> {
        Box::pin(api::fetch_campaigns(edition, token))
    }

    fn claim<'a>(
        &'a self,
        edition: QoderEdition,
        token: &'a str,
        campaign_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<api::Claimed, CreditsError>> + Send + 'a>> {
        Box::pin(api::claim(edition, token, campaign_id))
    }
}

/// 手动 / 自动都走这里。
pub async fn claim(app: &Arc<App>, account_id: &str, trigger: &'static str) -> RpcResult<Value> {
    claim_with(app, account_id, trigger, &Live).await
}

pub async fn claim_with(
    app: &Arc<App>,
    account_id: &str,
    trigger: &'static str,
    transport: &dyn Transport,
) -> RpcResult<Value> {
    if !crate::storage::qoder_credits::SOURCES.contains(&trigger) {
        return Err(RpcError::invalid_params("trigger 只支持 manual / auto"));
    }
    let account = account(app, account_id)?;
    let edition = edition_of(&account)?;

    if api::capability(edition).is_none() {
        record(
            app,
            RunInput {
                account_id: &account.id,
                campaign_id: None,
                campaign_key: None,
                period_start: None,
                period_end: None,
                outcome: "unsupported",
                credits: None,
                http_status: None,
                auth_source: None,
                error_kind: Some("unsupported"),
                error_detail: Some(api::unsupported_reason(edition)),
                trigger,
            },
        );
        return Ok(json!({
            "accountId": account.id,
            "state": "unsupported",
            "claimed": false,
            "error": { "kind": "unsupported", "message": api::unsupported_reason(edition) },
        }));
    }

    let resolved = match resolve_token(app, &account) {
        Ok(resolved) => resolved,
        Err(detail) => {
            return Ok(json!({
                "accountId": account.id,
                "state": "no_credential",
                "claimed": false,
                "error": { "kind": "no_credential", "message": crate::rpc::sanitize(&detail) },
            }))
        }
    };

    // 账号级互斥：手动与自动共用同一把锁，避免同一秒各提交一次。
    let Some(_lock) = try_claim_lock(&account.id) else {
        return Ok(json!({
            "accountId": account.id,
            "state": "busy",
            "claimed": false,
            "reason": "another_claim_in_flight",
            "error": { "kind": "busy", "message": "这个账号正在领取，请稍后再试" },
        }));
    };

    // 自动执行在动手前**再确认一次开关**：拿到锁之前用户可能刚把它关掉。
    if trigger == "auto" {
        let still_on = app
            .with_db(|db| db.qoder_credits_auto(&account.id))
            .unwrap_or(false);
        if !still_on {
            return Ok(json!({
                "accountId": account.id,
                "state": "skipped",
                "reason": "auto_disabled",
                "claimed": false,
            }));
        }
    }

    let epoch = account.auth_epoch;
    let now = now_secs();
    let snapshot = match transport.campaigns(edition, &resolved.token).await {
        Ok(body) => match api::parse_campaigns(&body) {
            Ok(campaigns) => api::classify(campaigns, now),
            // 结构不认识：绝不能当成"今天没有活动"。
            Err(err) => {
                let detail = err.detail();
                record(
                    app,
                    RunInput {
                        account_id: &account.id,
                        campaign_id: None,
                        campaign_key: None,
                        period_start: None,
                        period_end: None,
                        outcome: err.outcome(),
                        credits: None,
                        http_status: err.http_status(),
                        auth_source: Some(resolved.source),
                        error_kind: Some(err.kind()),
                        error_detail: Some(detail.as_str()),
                        trigger,
                    },
                );
                return Ok(json!({
                    "accountId": account.id,
                    "state": "failed",
                    "claimed": false,
                    "authSource": resolved.to_json(),
                    "error": { "kind": err.kind(), "message": err.detail() },
                }));
            }
        },
        Err(err) => {
            note_auth_failure(app, &account, &resolved, &err, epoch);
            let detail = err.detail();
            record(
                app,
                RunInput {
                    account_id: &account.id,
                    campaign_id: None,
                    campaign_key: None,
                    period_start: None,
                    period_end: None,
                    outcome: err.outcome(),
                    credits: None,
                    http_status: err.http_status(),
                    auth_source: Some(resolved.source),
                    error_kind: Some(err.kind()),
                    error_detail: Some(detail.as_str()),
                    trigger,
                },
            );
            return Ok(json!({
                "accountId": account.id,
                "state": "failed",
                "claimed": false,
                "authSource": resolved.to_json(),
                "error": { "kind": err.kind(), "message": err.detail() },
            }));
        }
    };

    // 没有可领活动：分别回答"今天已经领过" / "窗口内但状态未开放" / "完全没有活动"。
    if snapshot.todo.is_empty() {
        let today = snapshot.claimed.first().cloned();
        if let Some(campaign) = today.clone() {
            record(
                app,
                RunInput {
                    account_id: &account.id,
                    campaign_id: Some(&campaign.id),
                    campaign_key: Some(&campaign.key),
                    period_start: campaign.start_at,
                    period_end: campaign.end_at,
                    outcome: "already",
                    credits: campaign.credits,
                    http_status: None,
                    auth_source: Some(resolved.source),
                    error_kind: None,
                    error_detail: None,
                    trigger,
                },
            );
        }
        let mut value = snapshot_json(&account, &snapshot, now);
        value["claimed"] = json!(false);
        value["authSource"] = resolved.to_json();
        return Ok(value);
    }

    let mut results: Vec<Value> = Vec::new();
    let mut posted = 0usize;
    for campaign in &snapshot.todo {
        // **逐个活动去重**：这一项这周期已经成功过就不发 POST，
        // 但其它还没领的照样要领。按整批判断"全都领过没"会让部分已领的场景重复提交。
        let recorded = app
            .with_db(|db| db.qoder_credit_claimed(&account.id, &campaign.id, campaign.start_at))
            .unwrap_or(false);
        if recorded {
            results.push(json!({
                "campaignId": campaign.id,
                "outcome": "already",
                "credits": campaign.credits,
                "skipped": "recorded_this_period",
            }));
            continue;
        }

        // 这是最后一个能反悔的位置：POST 一旦发出去就是既成事实。
        // 账号被删、授权被换、开关被关，任何一种都停下来，不再提交。
        if let Some(reason) = pre_post_stop(app, &account.id, trigger, epoch) {
            results.push(json!({
                "campaignId": campaign.id,
                "outcome": "skipped",
                "reason": reason,
            }));
            break;
        }

        posted += 1;
        match transport
            .claim(edition, &resolved.token, &campaign.id)
            .await
        {
            Ok(claimed) => {
                let outcome = if claimed.replayed { "already" } else { "ok" };
                record(
                    app,
                    RunInput {
                        account_id: &account.id,
                        campaign_id: Some(&campaign.id),
                        campaign_key: Some(&campaign.key),
                        period_start: campaign.start_at,
                        period_end: campaign.end_at,
                        outcome,
                        credits: claimed.credits.or(campaign.credits),
                        http_status: Some(200),
                        auth_source: Some(resolved.source),
                        error_kind: None,
                        error_detail: None,
                        trigger,
                    },
                );
                results.push(json!({
                    "campaignId": campaign.id,
                    "outcome": outcome,
                    "credits": claimed.credits.or(campaign.credits),
                    "replayed": claimed.replayed,
                }));
            }
            Err(err) => {
                // 超时 / 网络：**先回查**，服务端很可能已经领了。
                if matches!(err, CreditsError::Network(_)) {
                    if let Ok(body) = transport.campaigns(edition, &resolved.token).await {
                        // 回查也看不懂结构：那就没法确认，按原错误记录，不猜。
                        let after = match api::parse_campaigns(&body) {
                            Ok(campaigns) => api::classify(campaigns, now_secs()),
                            Err(err) => {
                                let detail = err.detail();
                                record(
                                    app,
                                    RunInput {
                                        account_id: &account.id,
                                        campaign_id: Some(&campaign.id),
                                        campaign_key: Some(&campaign.key),
                                        period_start: campaign.start_at,
                                        period_end: campaign.end_at,
                                        outcome: err.outcome(),
                                        credits: None,
                                        http_status: err.http_status(),
                                        auth_source: Some(resolved.source),
                                        error_kind: Some(err.kind()),
                                        error_detail: Some(detail.as_str()),
                                        trigger,
                                    },
                                );
                                results.push(json!({
                                    "campaignId": campaign.id,
                                    "outcome": err.outcome(),
                                    "error": { "kind": err.kind(), "message": err.detail() },
                                }));
                                continue;
                            }
                        };
                        let confirmed = after
                            .claimed
                            .iter()
                            .find(|item| item.id == campaign.id)
                            .cloned();
                        if let Some(item) = confirmed {
                            record(
                                app,
                                RunInput {
                                    account_id: &account.id,
                                    campaign_id: Some(&item.id),
                                    campaign_key: Some(&item.key),
                                    period_start: item.start_at,
                                    period_end: item.end_at,
                                    outcome: "already",
                                    credits: item.credits.or(campaign.credits),
                                    http_status: None,
                                    auth_source: Some(resolved.source),
                                    error_kind: None,
                                    error_detail: None,
                                    trigger,
                                },
                            );
                            results.push(json!({
                                "campaignId": campaign.id,
                                "outcome": "already",
                                "credits": item.credits.or(campaign.credits),
                                "confirmedByRecheck": true,
                            }));
                            continue;
                        }
                    }
                }

                note_auth_failure(app, &account, &resolved, &err, epoch);
                let detail = err.detail();
                record(
                    app,
                    RunInput {
                        account_id: &account.id,
                        campaign_id: Some(&campaign.id),
                        campaign_key: Some(&campaign.key),
                        period_start: campaign.start_at,
                        period_end: campaign.end_at,
                        outcome: err.outcome(),
                        credits: None,
                        http_status: err.http_status(),
                        auth_source: Some(resolved.source),
                        error_kind: Some(err.kind()),
                        error_detail: Some(detail.as_str()),
                        trigger,
                    },
                );
                results.push(json!({
                    "campaignId": campaign.id,
                    "outcome": err.outcome(),
                    "error": { "kind": err.kind(), "message": err.detail() },
                }));
                // 授权问题会打断整批：后面的活动同样会被拒。
                if matches!(err, CreditsError::Unauthorized | CreditsError::Forbidden) {
                    break;
                }
            }
        }
    }

    // 执行后立即回查：界面上的"已领取"必须是服务端说的，不是我们推断的。
    // 回查失败或结构不认识时 `after` 为 `None`：界面据此不显示"已领"，也不猜。
    let confirmed = match transport.campaigns(edition, &resolved.token).await {
        Ok(body) => api::parse_campaigns(&body)
            .ok()
            .map(|campaigns| api::classify(campaigns, now_secs()))
            .map(|after| snapshot_json(&account, &after, now_secs())),
        Err(_) => None,
    };

    let any_success = results.iter().any(|item| {
        item.get("outcome").and_then(Value::as_str) == Some("ok")
            || item.get("outcome").and_then(Value::as_str) == Some("already")
    });

    Ok(json!({
        "accountId": account.id,
        "state": if any_success { "done" } else { "failed" },
        "claimed": any_success,
        "authSource": resolved.to_json(),
        "posted": posted,
        "results": results,
        "after": confirmed,
        "checkedAt": crate::clock::now_rfc3339(),
    }))
}

// ---------------------------------------------------------------------------
// 开关与记录
// ---------------------------------------------------------------------------

pub fn set_auto(app: &App, account_id: &str, enabled: bool) -> RpcResult<Value> {
    let account = account(app, account_id)?;
    let edition = edition_of(&account)?;
    if enabled && api::capability(edition).is_none() {
        // 打开一个永远不能执行的任务，只会制造"为什么没领到"的假问题。
        return Err(RpcError::invalid_params(format!(
            "{} 的 Credits 领取尚未验证，不能开启自动领取",
            edition.display_name()
        )));
    }
    let changed = app.with_db(|db| db.set_qoder_credits_auto(account_id, enabled))?;
    if !changed {
        return Err(RpcError::not_found(format!(
            "Qoder 账号不存在：{account_id}"
        )));
    }
    Ok(json!({ "accountId": account_id, "autoEnabled": enabled }))
}

pub fn runs(app: &App, account_id: Option<&str>, limit: u32) -> RpcResult<Value> {
    if let Some(account_id) = account_id.filter(|id| !id.is_empty()) {
        account(app, account_id)?;
    }
    let rows = app.with_db(|db| db.list_qoder_credit_runs(account_id, limit))?;
    Ok(json!({
        "runs": rows.iter().map(|row| row.to_json()).collect::<Vec<_>>(),
        "outcomes": OUTCOMES,
    }))
}

/// 自动领取的当前状态：给界面回答"开着吗、上次什么时候、下次大概什么时候"。
pub fn auto_status(app: &App) -> RpcResult<Value> {
    let accounts = app.with_db(|db| db.list_qoder_accounts(None))?;
    let mut items = Vec::new();
    for account in accounts {
        let enabled = app.with_db(|db| db.qoder_credits_auto(&account.id))?;
        let recent = app.with_db(|db| db.list_qoder_credit_runs(Some(&account.id), 1))?;
        let last = recent.first().map(|row| row.to_json());
        items.push(json!({
            "accountId": account.id,
            "edition": account.edition,
            "nickname": account.nickname,
            "autoEnabled": enabled,
            "supported": api::capability(
                QoderEdition::parse(&account.edition).unwrap_or(QoderEdition::Global)
            ).is_some(),
            "lastRun": last,
        }));
    }
    Ok(json!({ "accounts": items }))
}

// ---------------------------------------------------------------------------
// 自动领取（心跳）
// ---------------------------------------------------------------------------

/// 今天（北京时间）是否已经成功领到过。
///
/// 早先是"取最近 5 条再看日期"，当天先成功后又有 5 次失败时，成功那条会被挤出
/// 窗口，自动化于是每 20 分钟继续探一次。改成按当天零点起的范围直接问库：
/// 结果与记录条数无关。查询失败按"没领过"处理（宁可多探一次，也不漏领）。
fn already_done_today(app: &App, account_id: &str) -> bool {
    let since = crate::checkin::scheduler::beijing_day_start_utc(crate::clock::beijing_now());
    app.with_db(|db| db.qoder_credit_succeeded_since(account_id, &since))
        .unwrap_or(false)
}

/// 跑一轮自动领取。与签到 / 成长共用同一个心跳，但**自己判断该不该动**。
///
/// `force` 只跳过 20 分钟的最小间隔，**不绕过账号开关**：
/// 没开开关的账号一个请求都不发。
pub async fn run_tick(app: &Arc<App>, force: bool) -> Value {
    let accounts = match app.with_db(|db| db.qoder_credits_auto_accounts()) {
        Ok(accounts) => accounts,
        Err(err) => return json!({ "status": "error", "error": err.message }),
    };
    if accounts.is_empty() {
        return json!({ "skipped": "no_accounts_enabled", "accounts": [] });
    }

    let mut runs: Vec<Value> = Vec::new();
    let mut ran = 0usize;
    for account in accounts {
        let edition = match QoderEdition::parse(&account.edition) {
            Some(edition) => edition,
            None => continue,
        };
        if api::capability(edition).is_none() {
            runs.push(json!({
                "accountId": account.id,
                "state": "skipped",
                "reason": "unsupported_edition",
            }));
            continue;
        }
        if already_done_today(app, &account.id) {
            runs.push(json!({
                "accountId": account.id,
                "state": "skipped",
                "reason": "done_today",
            }));
            continue;
        }
        if throttled(&account.id, force) {
            runs.push(json!({
                "accountId": account.id,
                "state": "skipped",
                "reason": "throttled",
            }));
            continue;
        }

        mark_attempt(&account.id);
        ran += 1;
        let result = claim(app, &account.id, "auto").await;
        let value = match result {
            Ok(value) => value,
            Err(err) => json!({ "accountId": account.id, "state": "failed", "error": err.message }),
        };
        runs.push(value);
    }

    json!({
        "status": "ok",
        "accounts": runs.len(),
        "ran": ran,
        "runs": runs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;

    fn test_app() -> (Arc<App>, tempfile::TempDir) {
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
        (
            Arc::new(App::with_secure(paths, SecureStore::memory())),
            dir,
        )
    }

    fn seed_account(app: &Arc<App>, edition: &str) -> QoderAccountRecord {
        app.with_db(|db| db.upsert_qoder_account(edition, "u-1", Some("积分号"), None))
            .unwrap()
    }

    #[test]
    fn magpie_only_accounts_have_no_credential() {
        let (app, _dir) = test_app();
        let account = seed_account(&app, "qoder_cn");
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                "magpie",
                Some("qoder#1"),
                None,
                "valid",
                None,
                None,
            )
        })
        .unwrap();
        // `Resolved` 刻意不实现 Debug（里面是令牌），所以取 `err()` 而不是 `unwrap_err()`。
        let err = resolve_token(&app, &account)
            .err()
            .expect("magpie-only 必须有错误");
        assert!(err.contains("Magpie"), "{err}");
    }

    #[test]
    fn pat_is_the_fallback_when_browser_auth_is_missing() {
        let (app, _dir) = test_app();
        let account = seed_account(&app, "qoder_cn");
        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        crate::providers::qoder_pat::persist(
            &app,
            QoderEdition::Cn,
            pat,
            &crate::providers::qoder_auth::Identity {
                uid: "u-1".into(),
                name: None,
                email: None,
            },
        )
        .unwrap();
        let resolved = resolve_token(&app, &account).unwrap();
        assert_eq!(resolved.source, SOURCE_OWN_PAT);
        assert_eq!(resolved.token, pat);
    }

    #[test]
    fn browser_auth_wins_over_pat() {
        let (app, _dir) = test_app();
        let account = seed_account(&app, "qoder_cn");
        crate::providers::qoder_pat::persist(
            &app,
            QoderEdition::Cn,
            "qp_abcdefghijklmnopqrstuvwxyz012345",
            &crate::providers::qoder_auth::Identity {
                uid: "u-1".into(),
                name: None,
                email: None,
            },
        )
        .unwrap();
        app.secure
            .set(&access_key(&account.id), b"device-token")
            .unwrap();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                None,
                Some("keychain:x"),
                "valid",
                None,
                None,
            )
        })
        .unwrap();
        let resolved = resolve_token(&app, &account).unwrap();
        assert_eq!(resolved.source, SOURCE_OWN_BROWSER);
        assert_eq!(resolved.token, "device-token");
    }

    #[test]
    fn invalid_source_is_never_selected() {
        let (app, _dir) = test_app();
        let account = seed_account(&app, "qoder_cn");
        app.secure
            .set(&access_key(&account.id), b"device-token")
            .unwrap();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                None,
                Some("keychain:x"),
                "invalid",
                None,
                None,
            )
        })
        .unwrap();
        assert!(resolve_token(&app, &account).is_err());
    }

    /// 手动与自动共用一把锁：同一账号同一时刻只能有一次领取在跑。
    #[test]
    fn the_claim_lock_is_per_account_and_released_on_drop() {
        let first = try_claim_lock("qa_lock");
        assert!(first.is_some(), "第一次应该拿到锁");
        assert!(
            try_claim_lock("qa_lock").is_none(),
            "同一账号第二次必须被挡"
        );
        // 别的账号不受影响。
        let other = try_claim_lock("qa_other");
        assert!(other.is_some());
        drop(other);
        drop(first);
        // 放掉之后可以重新拿。
        assert!(try_claim_lock("qa_lock").is_some());
    }

    /// POST 前的最后一道闸：账号还在、授权没换、开关还开着。
    #[test]
    fn the_pre_post_gate_stops_on_every_drift() {
        let (app, _dir) = test_app();
        let account = seed_account(&app, "qoder_cn");
        let epoch = app
            .with_db(|db| db.qoder_auth_epoch(&account.id))
            .unwrap()
            .unwrap();

        // 手动不受开关影响：开关关着也能手动领。
        assert!(pre_post_stop(&app, &account.id, "manual", epoch).is_none());

        // 自动：开关关着必须停。
        assert_eq!(
            pre_post_stop(&app, &account.id, "auto", epoch),
            Some("auto_disabled")
        );
        app.with_db(|db| db.set_qoder_credits_auto(&account.id, true))
            .unwrap();
        assert!(pre_post_stop(&app, &account.id, "auto", epoch).is_none());

        // 授权被换：旧 epoch 的结果不许继续提交。
        let token = "qp_abcdefghijklmnopqrstuvwxyz012345";
        crate::providers::qoder_pat::persist(
            &app,
            QoderEdition::Cn,
            token,
            &crate::providers::qoder_auth::Identity {
                uid: "u-1".into(),
                name: None,
                email: None,
            },
        )
        .unwrap();
        assert_eq!(
            pre_post_stop(&app, &account.id, "manual", epoch),
            Some("authorization_changed")
        );

        // 账号被删。
        let fresh = app
            .with_db(|db| db.qoder_auth_epoch(&account.id))
            .unwrap()
            .unwrap();
        app.with_db(|db| db.delete_qoder_account(&account.id))
            .unwrap();
        assert_eq!(
            pre_post_stop(&app, &account.id, "manual", fresh),
            Some("account_removed")
        );
    }

    // ---- 用假 transport 覆盖网络分支（PR #77）----

    use std::collections::VecDeque;

    /// 脚本化的假网络：按顺序吐预设结果，并记下真的提交了哪些活动。
    struct FakeTransport {
        campaigns: std::sync::Mutex<VecDeque<Result<Value, CreditsError>>>,
        claims: std::sync::Mutex<VecDeque<Result<api::Claimed, CreditsError>>>,
        claimed_ids: std::sync::Mutex<Vec<String>>,
    }

    impl FakeTransport {
        fn new(
            campaigns: Vec<Result<Value, CreditsError>>,
            claims: Vec<Result<api::Claimed, CreditsError>>,
        ) -> Self {
            Self {
                campaigns: std::sync::Mutex::new(campaigns.into()),
                claims: std::sync::Mutex::new(claims.into()),
                claimed_ids: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn claimed_ids(&self) -> Vec<String> {
            self.claimed_ids.lock().unwrap().clone()
        }
    }

    impl Transport for FakeTransport {
        fn campaigns<'a>(
            &'a self,
            _edition: QoderEdition,
            _token: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<Value, CreditsError>> + Send + 'a>> {
            Box::pin(async move {
                self.campaigns
                    .lock()
                    .unwrap()
                    .pop_front()
                    // 脚本用完了就当作"今天没有活动"，避免测试因为多一次回查而 panic。
                    .unwrap_or(Ok(json!({"campaigns": []})))
            })
        }

        fn claim<'a>(
            &'a self,
            _edition: QoderEdition,
            _token: &'a str,
            campaign_id: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<api::Claimed, CreditsError>> + Send + 'a>> {
            self.claimed_ids
                .lock()
                .unwrap()
                .push(campaign_id.to_string());
            Box::pin(async move {
                self.claims
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or(Err(CreditsError::Network("脚本用完了".into())))
            })
        }
    }

    fn campaigns_body(items: &[(&str, &str)]) -> Value {
        json!({"campaigns": items.iter().map(|(id, status)| json!({
            "campaignId": id,
            "campaignKey": format!("k-{id}"),
            "actionType": "CLAIM_BENEFIT",
            "claimStatus": status,
            "startAt": 1_700_000_000,
            "endAt": 1_900_000_000,
            "benefit": {"amount": 100}
        })).collect::<Vec<_>>()})
    }

    fn claimed_ok() -> api::Claimed {
        api::Claimed {
            replayed: false,
            credits: Some(100),
            status: "CLAIMED".to_string(),
        }
    }

    /// 有一个 PAT 可用的账号（`claim_with` 需要一个能解析出的凭据）。
    fn claimable_account(app: &Arc<App>) -> String {
        let account = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-net", Some("网络号"), None))
            .unwrap();
        crate::providers::qoder_pat::persist(
            app,
            QoderEdition::Cn,
            "qp_abcdefghijklmnopqrstuvwxyz012345",
            &crate::providers::qoder_auth::Identity {
                uid: "u-net".into(),
                name: None,
                email: None,
            },
        )
        .unwrap();
        account.id
    }

    fn run_claim(app: &Arc<App>, id: &str, fake: &FakeTransport) -> Value {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(claim_with(app, id, "manual", fake))
            .unwrap()
    }

    /// 401：第一个活动被拒后**立刻停手**，不碰后面的活动，并把来源标成 invalid。
    #[test]
    fn unauthorized_stops_the_batch_after_the_first_post() {
        let (app, _dir) = test_app();
        let id = claimable_account(&app);
        let fake = FakeTransport::new(
            vec![
                Ok(campaigns_body(&[("c1", "CLAIMABLE"), ("c2", "CLAIMABLE")])),
                Ok(campaigns_body(&[("c1", "CLAIMABLE"), ("c2", "CLAIMABLE")])),
            ],
            vec![Err(CreditsError::Unauthorized)],
        );
        let value = run_claim(&app, &id, &fake);

        assert_eq!(fake.claimed_ids(), vec!["c1"], "401 之后不许再提交");
        assert_eq!(value["state"], "failed");
        assert_eq!(value["results"][0]["outcome"], "rejected");

        let rows = app.with_db(|db| db.list_qoder_authorizations(&id)).unwrap();
        assert_eq!(rows[0].auth_status, "invalid");
    }

    /// 403：同样停手，但**不改授权状态**——权限不足不是凭据失效。
    #[test]
    fn forbidden_stops_the_batch_without_marking_the_credential_invalid() {
        let (app, _dir) = test_app();
        let id = claimable_account(&app);
        let fake = FakeTransport::new(
            vec![
                Ok(campaigns_body(&[("c1", "CLAIMABLE"), ("c2", "CLAIMABLE")])),
                Ok(campaigns_body(&[("c1", "CLAIMABLE"), ("c2", "CLAIMABLE")])),
            ],
            vec![Err(CreditsError::Forbidden)],
        );
        let value = run_claim(&app, &id, &fake);

        assert_eq!(fake.claimed_ids(), vec!["c1"]);
        assert_eq!(value["results"][0]["outcome"], "forbidden");
        let rows = app.with_db(|db| db.list_qoder_authorizations(&id)).unwrap();
        assert_eq!(rows[0].auth_status, "valid", "403 不能把凭据标坏");
    }

    /// POST 超时：**不重发**，先回查；回查说已领就记 `already`。
    #[test]
    fn a_timed_out_post_is_confirmed_by_a_recheck_not_resent() {
        let (app, _dir) = test_app();
        let id = claimable_account(&app);
        let fake = FakeTransport::new(
            vec![
                Ok(campaigns_body(&[("c1", "CLAIMABLE")])),
                // 回查：服务端已经记成已领。
                Ok(campaigns_body(&[("c1", "CLAIMED")])),
                // 执行后回查。
                Ok(campaigns_body(&[("c1", "CLAIMED")])),
            ],
            vec![Err(CreditsError::Network("timed out".into()))],
        );
        let value = run_claim(&app, &id, &fake);

        assert_eq!(fake.claimed_ids(), vec!["c1"], "超时后只能提交一次");
        assert_eq!(value["results"][0]["outcome"], "already");
        assert_eq!(value["results"][0]["confirmedByRecheck"], true);
        assert_eq!(value["state"], "done");
    }

    /// 逐活动去重：这一期已经领过的活动不发 POST，没领的照常发。
    #[test]
    fn only_unclaimed_campaigns_are_submitted() {
        let (app, _dir) = test_app();
        let id = claimable_account(&app);
        // c1 在本期已经成功过。
        app.with_db(|db| {
            db.record_qoder_credit_run(crate::storage::qoder_credits::RunInput {
                account_id: &id,
                campaign_id: Some("c1"),
                campaign_key: Some("k-c1"),
                period_start: Some(1_700_000_000),
                period_end: Some(1_900_000_000),
                outcome: "ok",
                credits: Some(100),
                http_status: Some(200),
                auth_source: Some("own_pat"),
                error_kind: None,
                error_detail: None,
                trigger: "manual",
            })
        })
        .unwrap();

        let fake = FakeTransport::new(
            vec![
                Ok(campaigns_body(&[("c1", "CLAIMABLE"), ("c2", "CLAIMABLE")])),
                Ok(campaigns_body(&[("c1", "CLAIMED"), ("c2", "CLAIMED")])),
            ],
            vec![Ok(claimed_ok())],
        );
        let value = run_claim(&app, &id, &fake);

        assert_eq!(fake.claimed_ids(), vec!["c2"], "已领过的 c1 不许再提交");
        assert_eq!(value["posted"], 1);
        assert_eq!(value["results"][0]["skipped"], "recorded_this_period");
    }

    #[test]
    fn auto_cannot_be_enabled_for_an_unverified_edition() {
        let (app, _dir) = test_app();
        let account = seed_account(&app, "qoder_global");
        let err = set_auto(&app, &account.id, true).unwrap_err();
        assert!(err.message.contains("尚未验证"), "{}", err.message);
        // 关闭是允许的：关一个本来没开的东西不该报错。
        assert!(set_auto(&app, &account.id, false).is_ok());
    }

    #[test]
    fn auto_flag_round_trips() {
        let (app, _dir) = test_app();
        let account = seed_account(&app, "qoder_cn");
        assert!(!app
            .with_db(|db| db.qoder_credits_auto(&account.id))
            .unwrap());
        set_auto(&app, &account.id, true).unwrap();
        assert!(app
            .with_db(|db| db.qoder_credits_auto(&account.id))
            .unwrap());
        assert_eq!(
            app.with_db(|db| db.qoder_credits_auto_accounts())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn tick_with_nothing_enabled_makes_no_requests() {
        let (app, _dir) = test_app();
        seed_account(&app, "qoder_cn");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let report = runtime.block_on(run_tick(&app, false));
        assert_eq!(report["skipped"], "no_accounts_enabled");
    }

    #[test]
    fn snapshots_report_absence_instead_of_zero() {
        let account = QoderAccountRecord {
            id: "qa_x".into(),
            edition: "qoder_cn".into(),
            uid: "u".into(),
            nickname: String::new(),
            email: None,
            created_at: String::new(),
            updated_at: String::new(),
            auth_epoch: 0,
        };
        let snapshot = api::classify(Vec::new(), super::now_secs());
        let value = snapshot_json(&account, &snapshot, super::now_secs());
        assert_eq!(value["state"], "nothing");
        assert_eq!(value["claimable"], 0);
        assert!(value["today"].is_null(), "没有活动时不能编一个今天");
    }

    #[test]
    fn done_today_reads_beijing_dates_not_utc() {
        let (app, _dir) = test_app();
        let account = seed_account(&app, "qoder_cn");
        // 北京时间今天 00:30 = 昨天 UTC，必须算"今天"。
        let beijing_now = crate::clock::beijing_now();
        let midnight = beijing_now.date_naive().and_hms_opt(0, 30, 0).unwrap();
        // 北京时间 00:30 = UTC 前一天 16:30。
        let at = chrono::DateTime::<chrono::Utc>::from_naive_utc_and_offset(
            midnight - chrono::Duration::hours(8),
            chrono::Utc,
        );
        let executed_at = at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        app.with_db(|db| {
            db.exec_raw(&format!(
                "INSERT INTO qoder_credit_runs
                    (uid, account_id, campaign_id, campaign_key, outcome, credits,
                     http_status, auth_source, error_kind, error_detail, trigger, executed_at)
                 VALUES ('qc_test', '{}', 'c-1', 'daily', 'ok', 100, 200,
                         'own_browser', NULL, NULL, 'auto', '{}')",
                account.id, executed_at
            ))
        })
        .unwrap();
        assert!(already_done_today(&app, &account.id));
    }
}
