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
    /// 取令牌走 Broker：默认 Magpie 优先、失败时读时兜底到自有授权（G1 / G6）。
    ///
    /// 这里不再自己拼钥匙串键 —— 键名多了三个之后，任何一处自己拼都会漏掉一种来源。
    fn access(&self, account: &AccountRecord) -> Result<String, String> {
        match crate::auth::broker::resolve(self.app, account) {
            Ok(crate::auth::broker::Outcome::Resolved(resolved)) => Ok(resolved.access),
            Ok(crate::auth::broker::Outcome::Unavailable { detail, .. }) => Err(detail),
            Err(err) => Err(err.message),
        }
    }

    /// 401 之后恢复一次。
    ///
    /// 顺序：
    /// 1. **按当前选中的来源**走各自的恢复路径 —— own 用 refresh，magpie 重新从文件读 access；
    /// 2. 主路失败后，**只读**地试另一份仍在有效期内的 access（不再发第二次请求）。
    ///
    /// 这样 "一次 401 只触发一次恢复"，既不会循环刷新，也不会因为一次失败就把账号打进冷却。
    async fn resync(&self, account: &AccountRecord) -> Result<String, String> {
        let selected = selected_kind(self.app, account);
        let primary = if selected == crate::auth::ids::KIND_OWN {
            self.refresh_own(account).await
        } else {
            self.resync_magpie(account)
        };

        match primary {
            Ok(token) => Ok(token),
            Err(err) => match other_access(self.app, account, &selected) {
                Some(token) => Ok(token),
                None => Err(err),
            },
        }
    }
}

impl AppTokens<'_> {
    /// 自有授权的恢复：走 refresh（会落盘新令牌并推进版本）。
    async fn refresh_own(&self, account: &AccountRecord) -> Result<String, String> {
        let report = crate::handlers::refresh_account(self.app, &account.id)
            .await
            .map_err(|err| err.message)?;
        if report.get("state").and_then(Value::as_str) != Some("valid") {
            return Err(report
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("自有授权刷新失败")
                .to_string());
        }
        crate::auth::service::read_access(self.app, account, crate::auth::ids::KIND_OWN)
            .map_err(|err| err.message)?
            .ok_or_else(|| "刷新后读不到新的访问令牌".to_string())
    }

    /// 借用授权的恢复：重新从 Magpie 读一次 access（**不刷新**）。
    fn resync_magpie(&self, account: &AccountRecord) -> Result<String, String> {
        let before =
            crate::auth::service::read_access(self.app, account, crate::auth::ids::KIND_MAGPIE)
                .ok()
                .flatten();
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
                let after = <Self as Tokens>::access(self, account)?;
                if before.as_deref() == Some(after.as_str()) {
                    return Err(
                        "Magpie 借用令牌虽未到标注过期时间，但已被服务端拒绝（401）；请在 Magpie 重新登录，或为该账号添加自有授权"
                            .to_string(),
                    );
                }
                Ok(after)
            }
            // 手上这份账号快照已经过期（期间用户扫码换了授权，或者账号刚被写过）：
            // 不算失败，直接用手上现有的令牌继续，下一轮自然读到新账号。
            Err(err) if crate::handlers::is_stale_snapshot(&err) => {
                <Self as Tokens>::access(self, account)
            }
            Err(err) => Err(err.message),
        }
    }
}

/// 当前 Broker 会选用哪一份；选不出来时退回首选来源，保证恢复路径有明确方向。
fn selected_kind(app: &App, account: &AccountRecord) -> String {
    match crate::auth::broker::resolve(app, account) {
        Ok(crate::auth::broker::Outcome::Resolved(resolved)) => resolved.kind,
        _ => crate::auth::broker::preferred_source(app),
    }
}

/// 另一份授权**现成**的 access：只读钥匙串，不看网络。
///
/// 只在主恢复路径明确失败后才调用；状态是失效类或已过期的就不算可用，
/// 免得把一份死令牌交回去再撞一次 401。
fn other_access(app: &App, account: &AccountRecord, selected: &str) -> Option<String> {
    let other = if selected == crate::auth::ids::KIND_OWN {
        crate::auth::ids::KIND_MAGPIE
    } else {
        crate::auth::ids::KIND_OWN
    };
    let row = app
        .with_db(|db| db.find_authorization(&account.id, other))
        .ok()
        .flatten()?;
    if !crate::auth::broker::is_usable(&row) {
        return None;
    }
    crate::auth::service::read_access(app, account, other)
        .ok()
        .flatten()
        .filter(|token| !token.trim().is_empty())
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
    // 国际版的 Buddy 旅行接口没验证过：不发请求，也不写旅行状态。
    if !account.supports_workbuddy_cn_apis() {
        return Err(RpcError::invalid_params(
            "WorkBuddy AI（国际版）没有 Buddy 旅行功能，未发送任何请求",
        ));
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
        // 国际版没有 Buddy 旅行：直接跳过，不发请求、不写状态、不产生结果行。
        if !account.supports_workbuddy_cn_apis() {
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::providers::magpie;
    use crate::securestore::SecureStore;
    use crate::storage::{AccountInput, AuthorizationRecord};
    use tempfile::TempDir;

    fn test_app() -> (TempDir, App) {
        let dir = TempDir::new().expect("tempdir");
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).expect("create data dir");
        let paths = Paths {
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data,
            wb_home_owned: true,
        };
        let app = App::with_secure(paths, SecureStore::memory());
        app.with_db(|db| db.migrate().map(|_| ()))
            .expect("migrate db");
        (dir, app)
    }

    fn seed_magpie_account(app: &App, token: &str) -> AccountRecord {
        let id = app
            .with_db(|db| {
                db.upsert_account(&AccountInput {
                    platform: "workbuddy".to_string(),
                    uid: "uid-stale-magpie".to_string(),
                    nickname: Some("高亮".to_string()),
                    auth_domain: Some("www.workbuddy.cn".to_string()),
                    auth_source: Some("magpie".to_string()),
                    auth_status: Some("valid".to_string()),
                    token_expires_at: Some("2099-12-31T00:00:00Z".to_string()),
                    travel_enabled: Some(true),
                    ..Default::default()
                })
            })
            .expect("seed account")
            .id;
        let account = app
            .with_db(|db| db.get_account(&id))
            .expect("get account")
            .expect("account exists");
        let now = crate::clock::now_rfc3339();
        let row = AuthorizationRecord {
            id: "auth-magpie-stale".to_string(),
            account_id: account.id.clone(),
            kind: crate::auth::ids::KIND_MAGPIE.to_string(),
            access_ref: "magpie:v1:workbuddy#stale".to_string(),
            refresh_ref: None,
            auth_domain: Some("www.workbuddy.cn".to_string()),
            token_expires_at: Some("2099-12-31T00:00:00Z".to_string()),
            status: "valid".to_string(),
            version: 1,
            key_migration_state: "ready".to_string(),
            source_key: Some("workbuddy#stale".to_string()),
            last_used_at: None,
            last_error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        app.with_db(|db| db.upsert_authorization(&row))
            .expect("seed magpie auth");
        magpie::test_set_logins(Some(vec![magpie::MagpieLogin {
            key: "workbuddy#stale".to_string(),
            provider: magpie::provider_of("workbuddy").expect("provider"),
            uid: "uid-stale-magpie".to_string(),
            nickname: Some("高亮".to_string()),
            domain: "www.workbuddy.cn".to_string(),
            access_token: token.to_string(),
            expires_at_ms: Some(magpie::now_ms() + 86_400_000),
        }]));
        account
    }

    #[tokio::test]
    async fn resync_magpie_reports_clear_error_when_magpie_file_token_is_unchanged() {
        let (_dir, app) = test_app();
        let account = seed_magpie_account(&app, "revoked-access-token");

        let tokens = AppTokens { app: &app };
        let err = tokens
            .resync(&account)
            .await
            .expect_err("unchanged revoked token must not pretend resync succeeded");
        magpie::test_reset();

        assert!(
            err.contains("Magpie 借用令牌") && err.contains("401") && err.contains("自有授权"),
            "unexpected error message: {err}"
        );
    }

    #[tokio::test]
    async fn resync_magpie_falls_back_to_own_when_magpie_token_is_unchanged() {
        let (_dir, app) = test_app();
        let account = seed_magpie_account(&app, "revoked-access-token");

        // 同时补一份可用自有授权：Magpie 未更新时应无感兜底到自有授权。
        let now = crate::clock::now_rfc3339();
        let keys = crate::auth::service::own_keys(&account.id);
        app.secure
            .set(&keys.access, b"fresh-own-access-token")
            .expect("store own token");
        let own_row = AuthorizationRecord {
            id: "auth-own-valid".to_string(),
            account_id: account.id.clone(),
            kind: crate::auth::ids::KIND_OWN.to_string(),
            access_ref: format!("keychain:{}", keys.access),
            refresh_ref: keys.refresh.map(|k| format!("keychain:{k}")),
            auth_domain: Some("www.workbuddy.cn".to_string()),
            token_expires_at: Some("2099-12-31T00:00:00Z".to_string()),
            status: "valid".to_string(),
            version: 1,
            key_migration_state: "ready".to_string(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        app.with_db(|db| db.upsert_authorization(&own_row))
            .expect("seed own auth");

        let tokens = AppTokens { app: &app };
        let token = tokens
            .resync(&account)
            .await
            .expect("should fall back to own token");
        magpie::test_reset();

        assert_eq!(token, "fresh-own-access-token");
    }
}
