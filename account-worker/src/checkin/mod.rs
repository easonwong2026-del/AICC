//! WorkBuddy 每日签到（PR #69）。
//!
//! 这个模块只回答三件事：
//!
//! 1. 每个账号**今天**签到了没有（以服务端为准，本地只做对照）。
//! 2. 没签到的账号要不要签、能不能签、签完确认了没有。
//! 3. 今天是谁（自动 / 手动）在什么时候触发的、结果如何。
//!
//! 明确不做：成长中心任务（#67/#68）、Buddy 旅行（#64）、补签卡、抽奖、兑换、
//! Qoder 签到。签到是独立模块，用户可以只开签到，不碰旅行与成长中心。
//!
//! 凭据只从 `auth::broker` 取：默认 Magpie、Own 兜底，两份都不碰对方的键，
//! 也**不读取、不刷新 Magpie 的 refresh token**。

pub mod api;
pub mod manager;
pub mod scheduler;
pub mod state;

use std::sync::Arc;

use serde_json::{json, Value};

use crate::app::App;
use crate::auth::broker;
use crate::clock::beijing_date;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::checkin::CheckinRecord;
use crate::storage::AccountRecord;

use api::{CheckinError, ErrorKind, Transport};
use manager::Source;
use state::State;

/// 单账号的今日状态（只读）。**不写库、不发签到 POST**。
pub struct AccountStatus {
    pub account: AccountRecord,
    pub state: State,
    pub server_checked_in: Option<bool>,
    pub streak_days: Option<i64>,
    pub daily_credit: Option<i64>,
    pub today_credit: Option<i64>,
    /// 实际生效的状态路径（两条候选之一）。线上排查时直接看这个。
    pub path: Option<&'static str>,
    pub record: Option<CheckinRecord>,
    pub error: Option<CheckinError>,
}

/// 查一个账号今天的服务端状态。只读。
pub async fn probe_account(app: &Arc<App>, account: &AccountRecord) -> AccountStatus {
    let today = beijing_date();
    let record = app
        .with_db(|db| db.checkin_record(&account.id, &today))
        .ok()
        .flatten();

    let mut status = AccountStatus {
        account: account.clone(),
        state: State::Unchecked,
        server_checked_in: None,
        streak_days: None,
        daily_credit: None,
        today_credit: None,
        path: None,
        record,
        error: None,
    };

    // **取凭据之前**就要拦：WorkBuddy AI 是另一个站点，`LiveTransport` 的域名
    // 写死在国内版上，走到下一行就等于把国际版令牌发到国内域名。
    if !account.supports_workbuddy_cn_apis() {
        status.state = State::Unsupported;
        status.error = Some(CheckinError::new(
            ErrorKind::Business,
            if account.platform == "workbuddy" {
                "WorkBuddy AI（国际版）的签到接口尚未接入；AICC 没有发送任何请求".to_string()
            } else {
                "签到只支持 WorkBuddy".to_string()
            },
        ));
        return status;
    }
    let resolved = match broker::resolve(app, account) {
        Ok(broker::Outcome::Resolved(resolved)) => resolved,
        Ok(broker::Outcome::Unavailable { reason, detail }) => {
            status.state = State::NoCredential;
            status.error = Some(CheckinError::new(
                ErrorKind::Business,
                format!("{reason}：{detail}"),
            ));
            return status;
        }
        Err(err) => {
            status.state = State::NoCredential;
            status.error = Some(CheckinError::new(ErrorKind::Business, err.message));
            return status;
        }
    };
    let domain = account.auth_domain.clone().unwrap_or_default();
    let transport = api::LiveTransport::new(&resolved.access, &account.uid, &domain);
    probe_with(status, &transport).await
}

/// 可注入的探测主体：生产用 `LiveTransport`，测试用 Mock 断言"一次请求都没发"。
pub(crate) async fn probe_with<T: api::Transport>(
    mut status: AccountStatus,
    transport: &T,
) -> AccountStatus {
    match transport.status().await {
        Ok(reply) => {
            status.server_checked_in = reply.checked_in;
            status.path = Some(reply.path);
            status.streak_days = reply.streak_days;
            status.daily_credit = reply.daily_credit;
            status.today_credit = reply.today_credit;
            status.state = match reply.checked_in {
                Some(true) => {
                    // 本地有 claimed 才是"AICC 签的"；否则只能说服务端已签过。
                    if status
                        .record
                        .as_ref()
                        .map(|row| row.status == crate::storage::checkin::STATUS_CLAIMED)
                        .unwrap_or(false)
                    {
                        State::Claimed
                    } else {
                        State::Already
                    }
                }
                Some(false) => State::Unchecked,
                None => State::Unknown,
            };
            if status.state == State::Unknown {
                status.error = Some(CheckinError::new(
                    ErrorKind::UnknownStatus,
                    "服务端没有返回今日签到状态",
                ));
            }
        }
        Err(err) => {
            status.state = match err.kind {
                ErrorKind::Unauthorized => State::Unauthorized,
                ErrorKind::UnknownStatus => State::Unknown,
                _ => State::Failed,
            };
            status.error = Some(err);
        }
    }
    status
}

fn status_row(status: &AccountStatus) -> Value {
    let record = status.record.as_ref();
    let claimed_by_aicc = record
        .map(|row| row.status == crate::storage::checkin::STATUS_CLAIMED)
        .unwrap_or(false);
    // 没有凭据时不要报成"未授权"——那会让人以为令牌过期了。
    let error_kind = match status.state {
        State::NoCredential => Some("no_credential".to_string()),
        // 版本/平台不支持要有明确的码，界面与日志才不用去猜括号里的中文。
        State::Unsupported => Some(
            if status.account.platform == "workbuddy" {
                "unsupported_provider"
            } else {
                "unsupported_platform"
            }
            .to_string(),
        ),
        _ => status
            .error
            .as_ref()
            .map(|err| err.kind.as_str().to_string()),
    };
    json!({
        "accountId": status.account.id,
        "nickname": status.account.nickname,
        "enabled": status.account.checkin_enabled,
        "state": status.state.as_str(),
        "serverCheckedIn": status.server_checked_in,
        // 只有 AICC 确认到账的奖励才显示数字；别的客户端签的没有数字可显示。
        "credits": record.and_then(|row| row.credits),
        "localStatus": record.map(|row| row.status.clone()),
        "localCheckedAt": record.map(|row| row.checked_at.clone()),
        "claimedByAiccToday": claimed_by_aicc,
        "streakDays": status.streak_days,
        "dailyCredit": status.daily_credit,
        "todayCredit": status.today_credit,
        "path": status.path,
        "errorKind": error_kind,
        "errorDetail": status.error.as_ref().map(|err| err.message.clone()),
        "httpStatus": status.error.as_ref().and_then(|err| err.http),
        "businessCode": status.error.as_ref().and_then(|err| err.code),
        "running": manager::is_running(&status.account.id),
    })
}

fn local_row(account: &AccountRecord, record: Option<CheckinRecord>, today: &str) -> Value {
    // 只读离线视图：不碰网络，状态完全来自本地记录。界面首屏用它先出图。
    let state = match &record {
        Some(row)
            if row.local_date == today && row.status == crate::storage::checkin::STATUS_CLAIMED =>
        {
            "claimed"
        }
        Some(row)
            if row.local_date == today && row.status == crate::storage::checkin::STATUS_ALREADY =>
        {
            "already"
        }
        Some(row)
            if row.local_date == today && row.status == crate::storage::checkin::STATUS_FAILED =>
        {
            "failed"
        }
        _ => "unknown",
    };
    json!({
        "accountId": account.id,
        "nickname": account.nickname,
        "enabled": account.checkin_enabled,
        "state": state,
        "serverCheckedIn": Value::Null,
        "credits": record.as_ref().and_then(|row| row.credits),
        "localStatus": record.as_ref().map(|row| row.status.clone()),
        "localCheckedAt": record.as_ref().map(|row| row.checked_at.clone()),
        "claimedByAiccToday": state == "claimed",
        "streakDays": Value::Null,
        "dailyCredit": Value::Null,
        "todayCredit": Value::Null,
        "path": Value::Null,
        "errorKind": record.as_ref().and_then(|row| row.error_kind.clone()),
        "errorDetail": Value::Null,
        "httpStatus": Value::Null,
        "businessCode": Value::Null,
        "running": manager::is_running(&account.id),
    })
}

/// 全部（或指定）账号的今日状态。`cached = true` 时不联网。
pub async fn fleet_status(app: &Arc<App>, ids: &[String], cached: bool) -> Value {
    let accounts = app
        .with_db(|db| db.list_accounts(&Default::default()))
        .unwrap_or_default()
        .into_iter()
        .filter(|account| account.platform == "workbuddy")
        .filter(|account| ids.is_empty() || ids.contains(&account.id))
        .collect::<Vec<_>>();

    let today = beijing_date();
    let mut rows = Vec::new();
    for account in &accounts {
        if cached {
            let record = app
                .with_db(|db| db.checkin_record(&account.id, &today))
                .ok()
                .flatten();
            rows.push(local_row(account, record, &today));
        } else {
            // 账号之间串行：一次刷新 4 个账号就是 4 个请求，不该并发打出去。
            let status = probe_account(app, account).await;
            rows.push(status_row(&status));
        }
    }
    let checked = rows
        .iter()
        .filter(|row| {
            matches!(
                row.get("state").and_then(Value::as_str),
                Some("claimed") | Some("already")
            )
        })
        .count();
    let enabled = rows
        .iter()
        .filter(|row| row.get("enabled").and_then(Value::as_bool) == Some(true))
        .count();
    // 只统计 AICC 自己确认到账的奖励，别把"别的客户端签的"算成我们的战果。
    let credit: i64 = rows
        .iter()
        .filter(|row| row.get("claimedByAiccToday").and_then(Value::as_bool) == Some(true))
        .filter_map(|row| row.get("credits").and_then(Value::as_i64))
        .sum();

    json!({
        "today": today,
        "cached": cached,
        "accounts": rows,
        "counts": {
            "accounts": accounts.len(),
            "checked": checked,
            "unchecked": accounts.len().saturating_sub(checked),
            "enabled": enabled,
            "credit": credit,
        },
        "checkedAt": crate::clock::now_rfc3339(),
    })
}

fn find_account(app: &App, account_id: &str) -> RpcResult<AccountRecord> {
    app.with_db(|db| db.get_account(account_id))?
        .ok_or_else(|| RpcError::not_found(format!("账号不存在：{account_id}")))
}

/// 用户显式点「立即签到」。
///
/// 手动入口**不受自动开关约束**，但仍然先查服务端、仍然幂等 ——
/// 用户连点两下不会签两次（第二次查状态就返回 `already`）。
pub async fn run_one(app: &Arc<App>, account_id: &str) -> RpcResult<Value> {
    let account = find_account(app, account_id)?;
    let always = || true;
    let outcome = manager::run_account(app, &account, Source::Manual, &always).await;
    Ok(json!(outcome))
}

/// 用户显式点「立即签到未完成账号」。
///
/// 只处理服务端确认"未签到"的账号：先查状态，未签到才提交。
/// 已签到 / 说不清 / 授权失效的账号一律跳过，不会被批量按钮顺手重签。
pub async fn run_pending(app: &Arc<App>, ids: &[String]) -> Value {
    let accounts = app
        .with_db(|db| db.list_accounts(&Default::default()))
        .unwrap_or_default()
        .into_iter()
        .filter(|account| account.platform == "workbuddy")
        .filter(|account| ids.is_empty() || ids.contains(&account.id))
        .collect::<Vec<_>>();

    let mut rows = Vec::new();
    let mut submitted = 0usize;
    let mut already = 0usize;
    let mut skipped = 0usize;
    for (index, account) in accounts.iter().enumerate() {
        if index > 0 {
            tokio::time::sleep(scheduler::INTER_ACCOUNT_DELAY).await;
        }
        let always = || true;
        let outcome = manager::run_account(app, account, Source::Manual, &always).await;
        match outcome.state {
            "claimed" => submitted += 1,
            "already" => already += 1,
            "unchecked" | "busy" => skipped += 1,
            _ => {}
        }
        rows.push(json!(outcome));
    }

    let mut parts = Vec::new();
    if submitted > 0 {
        parts.push(format!("签到 {submitted} 个"));
    }
    if already > 0 {
        parts.push(format!("已签过 {already} 个"));
    }
    if skipped > 0 {
        parts.push(format!("跳过 {skipped} 个"));
    }
    json!({
        "summary": if parts.is_empty() { "没有需要签到的账号".to_string() } else { parts.join(" · ") },
        "accounts": rows,
        "claimed": submitted,
        "already": already,
        "skipped": skipped,
    })
}

/// 历史签到记录。
pub fn records(app: &App, account_id: Option<&str>, limit: u32) -> RpcResult<Value> {
    app.with_db(|db| {
        let rows = db.checkin_records(account_id, limit)?;
        let attempts = match account_id {
            Some(id) => db.checkin_attempts(id, limit)?,
            None => Vec::new(),
        };
        Ok(json!({ "records": rows, "attempts": attempts }))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;
    use crate::storage::AccountInput;

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
        let app = Arc::new(App::with_secure(paths, SecureStore::memory()));
        app.with_db(|db| Ok(db.migrate()?)).expect("migrate");
        (app, dir)
    }

    fn account(app: &App, uid: &str, provider: Option<&str>) -> AccountRecord {
        app.with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: uid.to_string(),
                nickname: Some(uid.to_string()),
                provider: provider.map(str::to_string),
                ..Default::default()
            })
        })
        .expect("account")
    }

    /// `checkin.status`（`cached=false`）会走在线探测，它必须自己挡版本：
    /// 国际版在**读凭据之前**就返回 unsupported，因此根本不可能发请求。
    ///
    /// 这里用一个连凭据都没有的账号：如果闸门被绕过，broker 会返回
    /// `no_credential`；实际拿到 `unsupported` 才证明顺序是对的。
    #[tokio::test]
    async fn status_probe_refuses_workbuddy_ai_before_touching_credentials() {
        let (app, _dir) = test_app();
        let ai = account(&app, "ai-probe-uid", Some("workbuddy_ai"));

        let status = probe_account(&app, &ai).await;
        assert_eq!(status.state, State::Unsupported);
        assert!(status.server_checked_in.is_none(), "没有查过服务端");
        assert!(status.path.is_none());

        let row = status_row(&status);
        assert_eq!(row["state"], json!("unsupported"));
        assert_eq!(row["errorKind"], json!("unsupported_provider"));
        assert_eq!(row["httpStatus"], Value::Null, "零网络请求");
    }

    /// 对照组：国内版账号没有凭据时仍然是 `no_credential`，
    /// 说明上面的 `unsupported` 不是"无差别拒绝"。
    #[tokio::test]
    async fn status_probe_still_reports_missing_credential_for_cn_accounts() {
        let (app, _dir) = test_app();
        let cn = account(&app, "cn-probe-uid", None);

        let status = probe_account(&app, &cn).await;
        assert_eq!(status.state, State::NoCredential);
        assert_eq!(status_row(&status)["errorKind"], json!("no_credential"));
    }
}
