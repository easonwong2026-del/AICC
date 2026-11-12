//! WorkBuddy 成长中心（PR #67：只读接入）。
//!
//! 这里的代码只回答一个问题："这个账号现在的成长中心长什么样"。
//! 接受任务、领奖、补签、兑换、抽奖、开盲盒全部在 #68 / #69，
//! 所以本模块**没有 POST**，也不会改任何授权状态。
//!
//! 凭据只从 `auth::broker` 取：默认 Magpie、Own 兜底，两份都不碰对方的键。

pub mod api;
pub mod catalog;
pub mod models;
pub mod scanner;

use std::sync::Arc;

use crate::app::App;
use crate::auth::broker;
use crate::storage::AccountRecord;

pub use scanner::{AccountScan, Counts, FieldError};

/// 扫一个账号。取不到凭据时返回一个非错误态的占位结果 ——
/// "没有授权"是账号的合法状态，不是扫描失败。
pub async fn scan_account(app: &Arc<App>, account: &AccountRecord) -> AccountScan {
    let resolved = match broker::resolve(app, account) {
        Ok(broker::Outcome::Resolved(resolved)) => resolved,
        Ok(broker::Outcome::Unavailable { reason, detail }) => {
            return unavailable(account, &reason, &detail);
        }
        Err(err) => return unavailable(account, "error", &err.message),
    };

    let domain = account.auth_domain.clone().unwrap_or_default();
    let mut result = scanner::scan(
        &account.id,
        &account.nickname,
        &resolved.access,
        &account.uid,
        &domain,
    )
    .await;
    // 用哪个来源扫的，是排查"为什么这个账号能读到、那个读不到"的第一条线索。
    result.auth_source = Some(resolved.kind);
    result.degraded = resolved.degraded;
    result
}

fn unavailable(account: &AccountRecord, reason: &str, detail: &str) -> AccountScan {
    AccountScan {
        account_id: account.id.clone(),
        nickname: account.nickname.clone(),
        path_prefix: None,
        state: "missing_credential",
        tasks: Vec::new(),
        profile: models::GrowthProfile::default(),
        travel: None,
        errors: vec![FieldError {
            field: "credential",
            kind: reason.to_string(),
            http: None,
            code: None,
            message: detail.to_string(),
        }],
        scanned_at: crate::clock::now_rfc3339(),
        counts: Counts::default(),
        auth_source: None,
        degraded: None,
    }
}

/// 全部账号一次扫描，**账号之间串行**。
pub async fn scan_fleet(app: &Arc<App>, ids: &[String]) -> Vec<AccountScan> {
    let accounts = match app.with_db(|db| db.list_accounts(&Default::default())) {
        Ok(list) => list,
        Err(_) => Vec::new(),
    };
    let mut out = Vec::new();
    for account in accounts {
        if account.platform != "workbuddy" {
            continue;
        }
        if !ids.is_empty() && !ids.contains(&account.id) {
            continue;
        }
        out.push(scan_account(app, &account).await);
    }
    out
}

/// 顶栏那几个数字。
pub fn overview(scans: &[AccountScan]) -> serde_json::Value {
    let mut claimable = 0usize;
    let mut acceptable = 0usize;
    let mut manual = 0usize;
    let mut unknown = 0usize;
    let mut traveling = 0usize;
    let mut energy = 0i64;
    let mut credit = 0i64;
    let mut needs_auth = 0usize;
    for scan in scans {
        claimable += scan.counts.claimable;
        acceptable += scan.counts.acceptable;
        manual += scan.counts.manual;
        unknown += scan.counts.unknown;
        energy += scan.profile.energy.unwrap_or(0);
        credit += scan.profile.credit.unwrap_or(0);
        if scan.travel.as_ref().and_then(|t| t.state.as_deref()) == Some("traveling") {
            traveling += 1;
        }
        if matches!(scan.state, "unauthorized" | "missing_credential") {
            needs_auth += 1;
        }
    }
    serde_json::json!({
        "accounts": scans.len(),
        "claimable": claimable,
        "acceptable": acceptable,
        "manual": manual,
        "unknown": unknown,
        "traveling": traveling,
        "energy": energy,
        "credit": credit,
        "needsAuth": needs_auth,
    })
}
