//! WorkBuddy 成长中心（PR #67：只读接入）。
//!
//! 这里的代码只回答一个问题："这个账号现在的成长中心长什么样"。
//! 接受任务、领奖、补签、兑换、抽奖、开盲盒全部在 #68 / #69，
//! 所以本模块**没有 POST**，也不会改任何授权状态。
//!
//! 凭据只从 `auth::broker` 取：默认 Magpie、Own 兜底，两份都不碰对方的键。

pub mod activity;
pub mod api;
pub mod catalog;
pub mod evidence;
pub mod manager;
pub mod models;
pub mod probe;
pub mod reward_ledger;
pub mod rewards;
pub mod rewards_state;
pub mod rewards_write;
pub mod scanner;
pub mod scheduler;
pub mod streak_rewards;

use std::sync::Arc;

use crate::app::App;
use crate::auth::broker;
use crate::storage::AccountRecord;

pub use scanner::{AccountScan, Counts, FieldError};

/// 扫一个账号。取不到凭据时返回一个非错误态的占位结果 ——
/// "没有授权"是账号的合法状态，不是扫描失败。
pub async fn scan_account(app: &Arc<App>, account: &AccountRecord) -> AccountScan {
    // 成长中心只实现了国内版（`www.codebuddy.cn`）。国际版是另一个站点：
    // 拿它的授权去请求国内接口只会 401，还会把授权状态搞脏。
    if !account.supports_workbuddy_cn_apis() {
        return unsupported(account);
    }
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

/// 版本不支持：**没有发任何请求**，也没有碰授权状态。
///
/// 国际版没有成长中心这项功能，所以正常列表里根本不会出现这个账号；
/// 这个分支只挡住"显式指定了 AI 账号 id"的调用。
fn unsupported(account: &AccountRecord) -> AccountScan {
    let mut scan = unavailable(
        account,
        "unsupported_provider",
        "WorkBuddy AI（国际版）没有成长中心功能；AICC 没有向任何接口发送请求",
    );
    scan.state = "unsupported";
    scan
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
        // 国际版没有成长任务：不扫描，也不在结果里出现。
        if !account.supports_workbuddy_cn_apis() {
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
    let mut tasks_completed = 0i64;
    let mut tasks_total = 0i64;
    let mut needs_auth = 0usize;
    for scan in scans {
        claimable += scan.counts.claimable;
        acceptable += scan.counts.acceptable;
        manual += scan.counts.manual;
        unknown += scan.counts.unknown;
        energy += scan.profile.energy.unwrap_or(0);
        tasks_completed += scan.profile.completed_tasks.unwrap_or(0);
        tasks_total += scan.profile.total_tasks.unwrap_or(0);
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
        // 成长中心没有"积分"接口，这里给的是档案里的完成数，不是积分。
        "tasksCompleted": tasks_completed,
        "tasksTotal": tasks_total,
        "needsAuth": needs_auth,
    })
}

// ---------------------------------------------------------------------------
// 奖励状态（PR #96，只读）
// ---------------------------------------------------------------------------

/// 一个账号的奖励状态。**这里没有任何 POST**：三个写动作仍未接线。
///
/// 与 `scan_account` 共用同一套凭据解析与"国际版不支持"的判定，避免两处规则漂移。
pub async fn reward_state(app: &Arc<App>, account: &AccountRecord) -> rewards_state::RewardState {
    let mut state = rewards_state::RewardState::empty(&account.id, &account.nickname);

    if !account.supports_workbuddy_cn_apis() {
        state.state = "unsupported";
        state.errors.push(FieldError {
            field: "provider",
            kind: "unsupported_provider".to_string(),
            http: None,
            code: None,
            message: "WorkBuddy 国际版没有成长中心；没有向任何接口发送请求".to_string(),
        });
        return attach_ledger(app, account, state);
    }

    let resolved = match broker::resolve(app, account) {
        Ok(broker::Outcome::Resolved(resolved)) => resolved,
        Ok(broker::Outcome::Unavailable { reason, detail }) => {
            state.state = "missing_credential";
            state.errors.push(FieldError {
                field: "credential",
                kind: reason,
                http: None,
                code: None,
                message: detail,
            });
            return attach_ledger(app, account, state);
        }
        Err(err) => {
            state.state = "missing_credential";
            state.errors.push(FieldError {
                field: "credential",
                kind: "error".to_string(),
                http: None,
                code: None,
                message: err.message,
            });
            return attach_ledger(app, account, state);
        }
    };

    let domain = account.auth_domain.clone().unwrap_or_default();
    let (fetched, _unauthorized) =
        rewards_state::fetch(&resolved.access, &account.uid, &domain).await;
    state = fetched;
    state.account_id = account.id.clone();
    state.nickname = account.nickname.clone();
    attach_ledger(app, account, state)
}

/// 账本部分只读本地库：**它不代表服务端状态**，只回答"有没有一条没核验完的操作"。
/// 把本地账本信息贴到状态上。**读不到就说读不到**：
///
/// - `awaitingVerification` 留 `None`（JSON `null`），不是 0；
/// - 同时压一条 `field="ledger"` 的错误，`state` 因此降级成 `partial`。
///
/// 这条路径一定跑得通，所以抽成纯函数：`Err` 分支必须有测试，否则哪天数据库
/// 打不开，界面会安静地显示"没有未核验记录"——那是错误结论，不是保守结论。
fn apply_ledger(
    mut state: rewards_state::RewardState,
    ledger: crate::rpc::RpcResult<(
        Vec<crate::storage::reward_ops::RewardOp>,
        Vec<crate::storage::reward_ops::RewardOp>,
    )>,
) -> rewards_state::RewardState {
    match ledger {
        Ok((awaiting, recent)) => {
            state.awaiting_verification = Some(awaiting.len());
            state.recent_operations = recent.iter().map(rewards_state::operation_view).collect();
        }
        Err(err) => {
            state.awaiting_verification = None;
            state.errors.push(FieldError {
                field: "ledger",
                kind: "storage".to_string(),
                http: None,
                code: Some(err.code),
                message: format!("本地奖励账本读取失败：{}", err.message),
            });
            if state.state == "ok" {
                state.state = "partial";
            }
        }
    }
    state
}

fn attach_ledger(
    app: &Arc<App>,
    account: &AccountRecord,
    state: rewards_state::RewardState,
) -> rewards_state::RewardState {
    let ledger = app.with_db(|db| {
        Ok((
            db.reward_ops_awaiting_verification(&account.id)?,
            db.reward_ops(&account.id, 5)?,
        ))
    });
    apply_ledger(state, ledger)
}

/// 全部账号（或指定账号）的奖励状态，账号之间串行。
///
/// **账号列表读不到就返回错误**，不能返回空数组：空数组意味着"你没有账号"，
/// 而真实情况是"我们读不到账号"。两者在界面上完全不同。
pub async fn reward_states(
    app: &Arc<App>,
    ids: &[String],
) -> crate::rpc::RpcResult<Vec<rewards_state::RewardState>> {
    let accounts = app.with_db(|db| db.list_accounts(&Default::default()))?;
    let mut out = Vec::new();
    for account in accounts {
        // 国际版没有成长中心：不扫描、也不出现在结果里（`scan_fleet` 同样的规则）。
        if !account.supports_workbuddy_cn_apis() {
            continue;
        }
        if !ids.is_empty() && !ids.contains(&account.id) {
            continue;
        }
        out.push(reward_state(app, &account).await);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rpc::{RpcError, E_STORAGE};

    fn empty_state() -> rewards_state::RewardState {
        rewards_state::RewardState::empty("acc", "昵称")
    }

    #[test]
    fn a_working_ledger_reports_the_real_awaiting_count() {
        let state = apply_ledger(empty_state(), Ok((Vec::new(), Vec::new())));
        assert_eq!(state.awaiting_verification, Some(0));
        assert!(state.errors.is_empty());
        assert_eq!(
            state.state, "failed",
            "空状态原样保留，不会因为账本正常而变 ok"
        );
    }

    /// P1 回归：本地账本读不到时**不许显示成 0**。
    ///
    /// 0 的意思是"一切都结算完了"，而真实情况是"我们没读到账本"。界面上这两句话
    /// 完全不同，静默默认 0 会让用户以为没有待核验的操作。
    #[test]
    fn an_unreadable_ledger_is_unknown_not_zero() {
        let state = apply_ledger(
            empty_state(),
            Err(RpcError::new(E_STORAGE, "database is locked")),
        );
        assert_eq!(state.awaiting_verification, None, "读不到就是 null，不是 0");
        assert_eq!(state.recent_operations.len(), 0);
        let ledger_error = state
            .errors
            .iter()
            .find(|error| error.field == "ledger")
            .expect("必须留下一条 ledger 错误");
        assert_eq!(ledger_error.kind, "storage");
        assert!(ledger_error.message.contains("database is locked"));
    }

    #[test]
    fn an_unreadable_ledger_degrades_an_otherwise_ok_state() {
        let mut state = empty_state();
        state.state = "ok";
        let state = apply_ledger(state, Err(RpcError::new(E_STORAGE, "io")));
        assert_eq!(state.state, "partial", "有错误就不能再报 ok");
    }

    #[test]
    fn an_unreadable_ledger_does_not_mask_a_more_specific_state() {
        // 已经 unauthorized 的账号，不该被账本错误改写成 partial。
        let mut state = empty_state();
        state.state = "unauthorized";
        let state = apply_ledger(state, Err(RpcError::new(E_STORAGE, "io")));
        assert_eq!(state.state, "unauthorized");
        assert_eq!(state.awaiting_verification, None);
    }
}

/// 只读取证：四个 GET 抓下来，脱敏后返回（PR #100）。
///
/// 与 `reward_state` 共用凭据解析；**没有任何写接口**参与。
pub async fn capture_evidence(
    app: &Arc<App>,
    account_id: &str,
) -> crate::rpc::RpcResult<evidence::EvidenceReport> {
    let account = app
        .with_db(|db| db.get_account(account_id))?
        .ok_or_else(|| crate::rpc::RpcError::new(crate::rpc::E_NOT_FOUND, "账号不存在"))?;
    if !account.supports_workbuddy_cn_apis() {
        return Err(crate::rpc::RpcError::invalid_params(
            "国际版没有成长中心，不做取证",
        ));
    }
    let resolved = match broker::resolve(app, &account) {
        Ok(broker::Outcome::Resolved(resolved)) => resolved,
        Ok(broker::Outcome::Unavailable { reason, detail }) => {
            return Err(crate::rpc::RpcError::new(
                crate::rpc::E_AUTH,
                format!("没有可用凭据（{reason}）：{detail}"),
            ))
        }
        Err(err) => return Err(err),
    };

    let mut report = evidence::capture(
        &resolved.access,
        &account.uid,
        account.auth_domain.as_deref().unwrap_or_default(),
    )
    .await;
    report.account_id = account.id;
    Ok(report)
}
