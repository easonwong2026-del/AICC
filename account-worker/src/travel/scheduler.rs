//! 常驻调度：谁在跑、多久跑一次、这一轮做了什么。
//!
//! **依赖关系必须说清楚**：调度器活在 `aicc-account-worker` 进程里，而这个进程是
//! `AICC.app` 的子进程。所以自动旅行的真实前提是「AICC.app 正在运行」——
//! 菜单栏 App 关掉，日程就停了。任何界面文案都不许暗示"退出后仍在后台跑"。
//!
//! 一轮 tick 的顺序：跨日重置 → 取开关账号 → 逐个「到期才查」→ 落快照 + 事件。
//! 同一时刻只允许一个旅行操作（不管是自动还是手动），避免自己和自己打架。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::app::App;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::travel::TravelState;
use crate::storage::AccountRecord;

use super::api::LiveTransport;
use super::service::{self, Config, EventDraft, Outcome, Tokens};
use super::state::Label;

/// 全局互斥：一轮 tick 或一次手动操作在跑时，其它旅行动作直接返回忙。
static TRAVEL_BUSY: AtomicBool = AtomicBool::new(false);
static LAST_TICK: Mutex<Option<Value>> = Mutex::new(None);

/// 谁在承载调度。给界面与文档用同一句话，避免两处说法不一致。
pub const SCHEDULER_OWNER: &str = "AICC.app 的账号助手进程（aicc-account-worker）";

struct BusyGuard;

impl BusyGuard {
    fn acquire() -> Option<Self> {
        TRAVEL_BUSY
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .ok()
            .map(|_| BusyGuard)
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        TRAVEL_BUSY.store(false, Ordering::SeqCst);
    }
}

/// 凭据来源：钥匙串 + 必要时从 Magpie 重新同步 access（**不碰 refresh**）。
pub struct AppTokens<'a> {
    pub app: &'a App,
}

impl Tokens for AppTokens<'_> {
    fn access(&self, account: &AccountRecord) -> Result<String, String> {
        let (key, _) = crate::providers::workbuddy_auth::secret_keys(&account.uid);
        let bytes = self.app.secure.get(&key).map_err(|err| err.message)?;
        let token = String::from_utf8(bytes).map_err(|_| "凭据不是 UTF-8".to_string())?;
        if token.trim().is_empty() {
            return Err("钥匙串里的访问令牌为空，请重新登录".to_string());
        }
        Ok(token)
    }

    fn resync(&self, account: &AccountRecord) -> Result<String, String> {
        if account.auth_source != crate::providers::magpie::SOURCE_MAGPIE {
            return Err("该账号是独立扫码授权，请重新扫码登录".to_string());
        }
        match crate::handlers::resync_magpie_access(self.app, account) {
            Ok(report) => {
                if report.get("state").and_then(Value::as_str) != Some("valid") {
                    return Err(report
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(Value::as_str)
                        .unwrap_or("Magpie 里的令牌也已过期")
                        .to_string());
                }
                <Self as Tokens>::access(self, account)
            }
            // 手上这份账号快照已经过期（用户刚好扫码换成了自有授权）：worker 拒绝写入，
            // 我们也不该报错——直接用手上现在的令牌继续，下一轮会读到新账号。
            Err(err) if crate::handlers::is_superseded(&err) => {
                <Self as Tokens>::access(self, account)
            }
            Err(err) => Err(err.message),
        }
    }
}

fn load_state(app: &App, account_id: &str) -> RpcResult<TravelState> {
    Ok(app
        .with_db(|db| db.travel_state(account_id))?
        .unwrap_or_else(|| TravelState {
            account_id: account_id.to_string(),
            label: Label::Unknown.as_str().to_string(),
            ..Default::default()
        }))
}

fn persist(app: &App, outcome: &Outcome) -> RpcResult<()> {
    app.with_db(|db| {
        db.save_travel_state(&outcome.state)?;
        for event in &outcome.events {
            db.append_travel_event(
                &outcome.state.account_id,
                event.kind,
                event.status,
                event.message.as_deref(),
                event.credits,
            )?;
        }
        Ok(())
    })
}

fn event_json(events: &[EventDraft]) -> Value {
    json!(events
        .iter()
        .map(|e| json!({
            "kind": e.kind,
            "status": e.status,
            "message": e.message,
            "credits": e.credits,
        }))
        .collect::<Vec<_>>())
}

/// 单账号同步（自动 tick 与手动按钮共用）。
///
/// `manual = true` 时跳过退避冷却与 30 分钟派出窗口 —— 用户明确点了按钮，
/// 就不该被"还没到点"挡住；但服务端每日上限仍然由状态机拦。
pub async fn sync_one(
    app: &Arc<App>,
    account_id: &str,
    allow_depart: bool,
    allow_claim: bool,
    manual: bool,
) -> RpcResult<Outcome> {
    let Some(_guard) = BusyGuard::acquire() else {
        return Err(RpcError::new(
            crate::rpc::E_TRAVEL_BUSY,
            "另一轮旅行检查正在进行，请稍后重试",
        ));
    };
    let account = app
        .with_db(|db| db.get_account(account_id))?
        .ok_or_else(|| RpcError::not_found(format!("账号不存在：{account_id}")))?;
    if account.platform != "workbuddy" {
        return Err(RpcError::invalid_params("只有 WorkBuddy 账号有 Buddy 旅行"));
    }
    let prior = load_state(app, account_id)?;
    let now = chrono::Utc::now().timestamp();
    let today = crate::clock::beijing_date();
    let config = Config::default();
    let allow = allow_depart && (manual || service::depart_window_open(&prior, now, &config));

    let transport = LiveTransport;
    let tokens = AppTokens { app };
    let outcome = service::sync_account(
        &transport,
        &tokens,
        &account,
        prior,
        now,
        &today,
        allow,
        allow_claim,
    )
    .await;
    persist(app, &outcome)?;
    Ok(outcome)
}

/// 一轮自动检查。没开任何账号时立刻返回，不做无谓请求。
pub async fn run_tick(app: &Arc<App>) -> Value {
    let enabled = app
        .with_db(|db| db.travel_enabled_account_ids())
        .unwrap_or_default();
    if enabled.is_empty() {
        let snapshot = json!({
            "status": "idle",
            "reason": "no_enabled_accounts",
            "accounts": 0,
            "at": crate::clock::now_rfc3339(),
            "owner": SCHEDULER_OWNER,
        });
        set_last_tick(snapshot.clone());
        return snapshot;
    }

    let Some(_guard) = BusyGuard::acquire() else {
        return json!({"status": "skipped", "reason": "busy", "accounts": enabled.len()});
    };

    let today = crate::clock::beijing_date();
    let _ = app.with_db(|db| db.roll_travel_states_to_today(&today));
    let now = chrono::Utc::now().timestamp();
    let config = Config::default();
    let mut results = Vec::new();
    let mut checked = 0;

    for account_id in &enabled {
        let account = match app.with_db(|db| db.get_account(account_id)) {
            Ok(Some(account)) => account,
            Ok(None) => continue,
            Err(err) => {
                results.push(
                    json!({"accountId": account_id, "status": "error", "error": err.message}),
                );
                continue;
            }
        };
        let prior = match load_state(app, account_id) {
            Ok(state) => state,
            Err(err) => {
                results.push(
                    json!({"accountId": account_id, "status": "error", "error": err.message}),
                );
                continue;
            }
        };
        if !service::is_due(&prior, now, &config) {
            results.push(json!({
                "accountId": account_id,
                "status": "not_due",
                "label": prior.label,
                "retryAfter": prior.retry_after,
            }));
            continue;
        }
        let allow_depart = service::depart_window_open(&prior, now, &config);
        let transport = LiveTransport;
        let tokens = AppTokens { app };
        // 自动模式：能派就派、到点就领。
        let outcome = service::sync_account(
            &transport,
            &tokens,
            &account,
            prior,
            now,
            &today,
            allow_depart,
            true,
        )
        .await;
        checked += 1;
        if let Err(err) = persist(app, &outcome) {
            results.push(json!({
                "accountId": account_id,
                "status": "error",
                "error": err.message,
            }));
            continue;
        }
        results.push(json!({
            "accountId": account_id,
            "status": "checked",
            "label": outcome.state.label,
            "events": event_json(&outcome.events),
        }));
    }

    let snapshot = json!({
        "status": "ok",
        "accounts": enabled.len(),
        "checked": checked,
        "at": crate::clock::now_rfc3339(),
        "owner": SCHEDULER_OWNER,
        "results": results,
    });
    set_last_tick(snapshot.clone());
    snapshot
}

fn set_last_tick(value: Value) {
    if let Ok(mut slot) = LAST_TICK.lock() {
        *slot = Some(value);
    }
}

pub fn last_tick() -> Option<Value> {
    LAST_TICK.lock().ok().and_then(|slot| slot.clone())
}

/// `travel.auto.status`：界面用它说明「谁在跑、下一次什么时候」。
pub fn auto_status(app: &App) -> Value {
    let enabled = app
        .with_db(|db| db.travel_enabled_account_ids())
        .unwrap_or_default();
    json!({
        "owner": SCHEDULER_OWNER,
        "hostProcess": "AICC.app",
        "runsOnlyWhileAppRunning": true,
        "tickIntervalSecs": service::TICK_INTERVAL_SECS,
        "departIntervalSecs": service::DEPART_INTERVAL_SECS,
        "claimIntervalSecs": service::CLAIM_INTERVAL_SECS,
        "enabledAccounts": enabled.len(),
        "busy": TRAVEL_BUSY.load(Ordering::SeqCst),
        "lastTick": last_tick(),
        "today": crate::clock::beijing_date(),
    })
}
