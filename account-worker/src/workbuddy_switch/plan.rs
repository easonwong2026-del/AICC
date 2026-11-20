//! 「这个账号能不能切、用哪份授权切」的唯一判定入口。
//!
//! `status` 用它给界面答案，`switch` 用它决定实际写哪份令牌 —— **两处必须同源**。
//! 界面显示 `magpie_access_temporary`、实际却用了自有授权，是这一层最不能犯的错。
//!
//! ## 两种模式
//!
//! | 模式 | 令牌 | 谁来续期 | 生命周期 |
//! | --- | --- | --- | --- |
//! | [`Mode::OwnRefresh`] | 自有 access + refresh | WorkBuddy 自己刷新 | 长期，直到用户退出 |
//! | [`Mode::MagpieTemporary`] | **只有** Magpie 的 access | 没人续期 | 到 access 过期为止 |
//!
//! Magpie 模式刻意不碰 refresh：那是 Magpie 的凭据，AICC 只借无状态 access。
//! 代价是它必然过期，所以这里要算清"还能用多久"，并给界面一个保守的恢复截止。
//!
//! ## 来源选择必须跟 Broker 一致
//!
//! 账号可以同时有 own / magpie 两份授权，`auth_preferred_source` 决定优先用哪份。
//! 这里的顺序完全照抄 [`crate::auth::broker`]：先首选、后另一份，首选不可用时用
//! 另一份并标 `degraded`，不静默换源。

use crate::app::App;
use crate::auth::magpie_link::{self, LinkProblem};
use crate::providers::magpie;
use crate::storage::{AccountRecord, AuthorizationRecord};

/// 允许切换所需的最小剩余有效期。
///
/// 30 分钟是个保守起点：Magpie 的 access 通常是小时级，低于半小时的窗口里
/// 用户还没做完"切过去 → 用一次 → 切回来"，切换就失去意义了。
/// 结论来自设计取舍而不是实测，所以做成常量，后续按真机数据调。
pub const MIN_SWITCH_REMAINING_SECS: i64 = 30 * 60;

/// 建议恢复截止要比 access 过期提前多久。
///
/// 恢复本身要关客户端、写文件、等客户端启动（最坏 ~45 秒）。留 15 分钟是为了
/// 让"看到提醒 → 点恢复"这条路不会刚好撞上过期。
pub const RESTORE_SAFETY_MARGIN_SECS: i64 = 15 * 60;

/// 授权模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// AICC 自有授权：access + refresh。
    OwnRefresh,
    /// Magpie 借来的 access，一次性、临时。
    MagpieTemporary,
}

impl Mode {
    /// 进 JSON / 进 journal 的稳定值。界面按这个分支写文案。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OwnRefresh => "own_refresh",
            Self::MagpieTemporary => "magpie_access_temporary",
        }
    }

    pub fn temporary(self) -> bool {
        matches!(self, Self::MagpieTemporary)
    }
}

/// 判定结论。`Ready` 才允许切换。
#[derive(Debug, Clone)]
pub enum PlanOutcome {
    Ready(Plan),
    Blocked {
        reason: &'static str,
        detail: String,
    },
}

impl PlanOutcome {
    pub fn blocked(reason: &'static str, detail: impl Into<String>) -> Self {
        Self::Blocked {
            reason,
            detail: detail.into(),
        }
    }
}

/// 一次切换要用的材料（**不含令牌**，令牌由 `switch` 现取）。
#[derive(Debug, Clone)]
pub struct Plan {
    pub mode: Mode,
    /// Magpie 模式必填：`plugin-auth.json` 里的键。
    pub source_key: Option<String>,
    /// access 到期时间（毫秒）。自有授权可能未知；Magpie 模式必然有值。
    pub access_expires_at_ms: Option<i64>,
    /// 保守的恢复截止（毫秒）：到期前留出 [`RESTORE_SAFETY_MARGIN_SECS`]。
    pub safe_restore_until_ms: Option<i64>,
    pub remaining_secs: Option<i64>,
    /// 首选来源不可用、这次用了另一份时的说明。
    pub degraded: Option<String>,
}

impl Plan {
    pub fn temporary(&self) -> bool {
        self.mode.temporary()
    }
}

/// 稳定原因码。界面按这些值显示原因，不要解析中文。
pub mod reason {
    pub const OK: &str = "ok";
    /// 客户端当前登录的就是它。
    pub const LIVE: &str = "live";
    pub const UNSUPPORTED_PROVIDER: &str = "unsupported_provider";
    pub const CLIENT_NOT_INSTALLED: &str = "client_not_installed";
    pub const NO_CREDENTIAL: &str = "no_credential";
    /// 自有授权里没有 refresh。
    pub const MISSING_REFRESH: &str = "missing_refresh";
    /// Magpie 文件读不出来。
    pub const MAGPIE_UNAVAILABLE: &str = "magpie_unavailable";
    /// 授权行没记来源键，或文件里已经没有这个键。
    pub const MAGPIE_SOURCE_MISSING: &str = "magpie_source_missing";
    pub const MAGPIE_UID_MISMATCH: &str = "magpie_uid_mismatch";
    pub const MAGPIE_PLATFORM_MISMATCH: &str = "magpie_platform_mismatch";
    pub const MAGPIE_ACCESS_EMPTY: &str = "magpie_access_empty";
    pub const MAGPIE_ACCESS_EXPIRED: &str = "magpie_access_expired";
    /// Magpie 没写 `expires`：不猜有效期，直接拒绝。
    pub const MAGPIE_EXPIRY_UNKNOWN: &str = "magpie_expiry_unknown";
    /// 剩余有效期低于 [`MIN_SWITCH_REMAINING_SECS`]。
    pub const MAGPIE_TOO_SHORT: &str = "magpie_too_short";
    /// 上一次临时切换还没恢复，先还原再切。
    pub const TEMPORARY_PENDING: &str = "temporary_pending";
    /// 客户端当前登录的不是这一轮切换的目标（用户手动换过）。
    pub const LIVE_CHANGED: &str = "live_changed";
}

/// 这个账号能不能切。**只读库 + 只读 Magpie 文件**，不读任何令牌、不弹钥匙串。
pub fn resolve(
    app: &App,
    account: &AccountRecord,
    installed: bool,
    live_uid: Option<&str>,
) -> PlanOutcome {
    if account.platform != "workbuddy" {
        return PlanOutcome::blocked(reason::UNSUPPORTED_PROVIDER, "只支持 WorkBuddy 账号");
    }
    if account.provider != "workbuddy" {
        return PlanOutcome::blocked(
            reason::UNSUPPORTED_PROVIDER,
            "WorkBuddy AI（国际版）的客户端切换尚未接通",
        );
    }
    if live_uid == Some(account.uid.as_str()) {
        return PlanOutcome::blocked(reason::LIVE, "客户端当前登录的就是这个账号");
    }
    if !installed {
        return PlanOutcome::blocked(
            reason::CLIENT_NOT_INSTALLED,
            "这台机器上没找到 WorkBuddy 客户端",
        );
    }

    let rows = match app.with_db(|db| db.list_authorizations(&account.id)) {
        Ok(rows) => rows,
        Err(err) => {
            return PlanOutcome::blocked(
                reason::NO_CREDENTIAL,
                format!("读授权失败：{}", err.message),
            )
        }
    };
    let own = rows.iter().find(|row| row.kind == "own");
    let magpie_row = rows.iter().find(|row| row.kind == "magpie");
    if own.is_none() && magpie_row.is_none() {
        return PlanOutcome::blocked(reason::NO_CREDENTIAL, "这个账号还没有登记任何授权来源");
    }

    // 顺序照抄 broker：首选在前，另一份兜底。
    let preferred = crate::auth::broker::preferred_source(app);
    let magpie_first = preferred == crate::auth::ids::KIND_MAGPIE;
    let order: [&str; 2] = if magpie_first {
        [crate::auth::ids::KIND_MAGPIE, crate::auth::ids::KIND_OWN]
    } else {
        [crate::auth::ids::KIND_OWN, crate::auth::ids::KIND_MAGPIE]
    };

    let mut first_block: Option<(String, String)> = None;
    for (index, kind) in order.iter().enumerate() {
        let attempt = if *kind == crate::auth::ids::KIND_OWN {
            own.map(try_own)
                .unwrap_or_else(|| Err((reason::NO_CREDENTIAL, "这个账号没有自有授权".to_string())))
        } else {
            magpie_row
                .map(|row| try_magpie(account, row))
                .unwrap_or_else(|| {
                    Err((
                        reason::NO_CREDENTIAL,
                        "这个账号没有 Magpie 关联".to_string(),
                    ))
                })
        };
        match attempt {
            Ok(mut plan) => {
                if index > 0 {
                    // 首选不可用：把原因带出去，界面要能说明"为什么这次用的是另一份"。
                    let (why, _) = first_block.clone().unwrap_or_default();
                    plan.degraded =
                        Some(format!("首选 {preferred} 不可用（{why}），已改用 {}", kind));
                }
                return PlanOutcome::Ready(plan);
            }
            Err((why, detail)) => {
                if first_block.is_none() {
                    first_block = Some((why.to_string(), detail));
                }
            }
        }
    }

    let (why, detail) = first_block.unwrap_or((
        reason::NO_CREDENTIAL.to_string(),
        "没有可用的授权".to_string(),
    ));
    PlanOutcome::Blocked {
        reason: leak_static(&why),
        detail,
    }
}

fn try_own(row: &AuthorizationRecord) -> Result<Plan, (&'static str, String)> {
    if row.refresh_ref.is_none() {
        return Err((
            reason::MISSING_REFRESH,
            "这份自有授权缺刷新令牌，切过去之后无法续期；请重新用浏览器登录".to_string(),
        ));
    }
    Ok(Plan {
        mode: Mode::OwnRefresh,
        source_key: None,
        access_expires_at_ms: None,
        safe_restore_until_ms: None,
        remaining_secs: None,
        degraded: None,
    })
}

fn try_magpie(
    account: &AccountRecord,
    row: &AuthorizationRecord,
) -> Result<Plan, (&'static str, String)> {
    let Some(source_key) = row.source_key.clone().filter(|key| !key.trim().is_empty()) else {
        return Err((
            reason::MAGPIE_SOURCE_MISSING,
            "这条 Magpie 授权没有记录来源键，请重新导入这个账号".to_string(),
        ));
    };

    let logins = magpie::read_logins().map_err(|err| {
        (
            reason::MAGPIE_UNAVAILABLE,
            format!(
                "Magpie 登录态不可读（{}）",
                crate::rpc::sanitize(&err.message())
            ),
        )
    })?;
    let now = magpie::now_ms();
    let login =
        magpie_link::find_link(&logins, account, &source_key, Some(now)).map_err(|problem| {
            let (why, detail) = match problem {
                LinkProblem::SourceMissing => (
                    reason::MAGPIE_SOURCE_MISSING,
                    "Magpie 里已经没有这个来源键，请重新导入或在 Magpie 里重新登录".to_string(),
                ),
                LinkProblem::UidMismatch => (
                    reason::MAGPIE_UID_MISMATCH,
                    "Magpie 里这条登录态属于另一个 UID，已拒绝（可能被其他账号覆盖）".to_string(),
                ),
                LinkProblem::PlatformMismatch => (
                    reason::MAGPIE_PLATFORM_MISMATCH,
                    "Magpie 里这条登录态不属于国内版 WorkBuddy".to_string(),
                ),
                LinkProblem::Expired => (
                    reason::MAGPIE_ACCESS_EXPIRED,
                    "Magpie 里的访问令牌已经过期，请先在 Magpie 里重新登录".to_string(),
                ),
                LinkProblem::FileUnavailable => (
                    reason::MAGPIE_UNAVAILABLE,
                    "Magpie 登录态不可读".to_string(),
                ),
            };
            (why, detail)
        })?;

    if login.access_token.trim().is_empty() {
        return Err((
            reason::MAGPIE_ACCESS_EMPTY,
            "Magpie 里的访问令牌是空的".to_string(),
        ));
    }
    let Some(expires_at) = login.expires_at_ms else {
        // 不猜有效期：不知道能用到什么时候，就不该把它写进客户端。
        return Err((
            reason::MAGPIE_EXPIRY_UNKNOWN,
            "Magpie 没有提供这个令牌的有效期，无法判断临时登录能用多久".to_string(),
        ));
    };
    let remaining_secs = (expires_at - now) / 1000;
    if remaining_secs <= 0 {
        return Err((
            reason::MAGPIE_ACCESS_EXPIRED,
            "Magpie 里的访问令牌已经过期".to_string(),
        ));
    }
    if remaining_secs < MIN_SWITCH_REMAINING_SECS {
        return Err((
            reason::MAGPIE_TOO_SHORT,
            format!(
                "Magpie 访问令牌只剩 {} 分钟，不足以完成一次临时切换（至少需要 {} 分钟）",
                remaining_secs / 60,
                MIN_SWITCH_REMAINING_SECS / 60
            ),
        ));
    }

    Ok(Plan {
        mode: Mode::MagpieTemporary,
        source_key: Some(source_key),
        access_expires_at_ms: Some(expires_at),
        safe_restore_until_ms: Some(expires_at - RESTORE_SAFETY_MARGIN_SECS * 1000),
        remaining_secs: Some(remaining_secs),
        degraded: None,
    })
}

/// `first_block` 里存的是 `String`（为了能放进同一个变量），这里换回 `&'static str`。
///
/// 只有 `reason::*` 常量会进来，找不到就退成 `no_credential` —— 宁可少一个精确原因，
/// 也不要在响应里回一个前端不认识的字符串。
fn leak_static(value: &str) -> &'static str {
    const ALL: &[&str] = &[
        reason::OK,
        reason::LIVE,
        reason::UNSUPPORTED_PROVIDER,
        reason::CLIENT_NOT_INSTALLED,
        reason::NO_CREDENTIAL,
        reason::MISSING_REFRESH,
        reason::MAGPIE_UNAVAILABLE,
        reason::MAGPIE_SOURCE_MISSING,
        reason::MAGPIE_UID_MISMATCH,
        reason::MAGPIE_PLATFORM_MISMATCH,
        reason::MAGPIE_ACCESS_EMPTY,
        reason::MAGPIE_ACCESS_EXPIRED,
        reason::MAGPIE_EXPIRY_UNKNOWN,
        reason::MAGPIE_TOO_SHORT,
        reason::TEMPORARY_PENDING,
        reason::LIVE_CHANGED,
    ];
    ALL.iter()
        .copied()
        .find(|known| *known == value)
        .unwrap_or(reason::NO_CREDENTIAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_round_trip_through_the_static_table() {
        assert_eq!(
            leak_static(reason::MAGPIE_TOO_SHORT),
            reason::MAGPIE_TOO_SHORT
        );
        assert_eq!(leak_static("not_a_reason"), reason::NO_CREDENTIAL);
    }

    #[test]
    fn mode_strings_are_stable() {
        assert_eq!(Mode::OwnRefresh.as_str(), "own_refresh");
        assert_eq!(Mode::MagpieTemporary.as_str(), "magpie_access_temporary");
        assert!(Mode::MagpieTemporary.temporary());
        assert!(!Mode::OwnRefresh.temporary());
    }
}
