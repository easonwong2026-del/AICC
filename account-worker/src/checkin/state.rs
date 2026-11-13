//! 签到状态机（PRD §4.1）。
//!
//! 服务端是**唯一事实来源**。本地记录只回答问题"我们最后一次知道的是什么"，
//! 永远不能代替"现在到底是什么" —— 所以每一轮都先查状态，不看本地缓存下结论。
//!
//! ```text
//! disabled ──┐
//!            ├─ 不发任何请求
//! unsupported┘
//!
//! [查状态] ─┬─ Some(true)  → already   （跳过写请求）
//!            ├─ Some(false) → 提交
//!            └─ None        → unknown   （绝不提交）
//!
//! [提交] ─┬─ 服务端回执成功 → 回查
//!          ├─ 已签到回执     → already
//!          └─ 错误           → failed / unauthorized
//!
//! [回查] ─┬─ Some(true)  → claimed   （唯一可以写奖励的状态）
//!          └─ 其它        → unknown   （不写奖励，不记成功）
//! ```

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// 账号没开自动签到，自动轮次直接跳过。
    Disabled,
    /// 服务端确认今天尚未签到。
    Unchecked,
    /// 服务端确认今天已经签到（可能是别的客户端做的）。
    Already,
    /// 服务端确认这次签到成功。
    Claimed,
    /// 服务端状态无法判断。
    Unknown,
    /// 授权失效，停止这个账号。
    Unauthorized,
    /// 没有可用凭据（要重新登录或从 Magpie 同步）。
    NoCredential,
    /// 明确失败。
    Failed,
    /// 该档位不支持签到。
    Unsupported,
    /// 同一账号已有一次执行在跑。
    Busy,
    /// 用户中途关掉自动签到，这次没提交。
    Stopped,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Unchecked => "unchecked",
            Self::Already => "already",
            Self::Claimed => "claimed",
            Self::Unknown => "unknown",
            Self::Unauthorized => "unauthorized",
            Self::NoCredential => "no_credential",
            Self::Failed => "failed",
            Self::Unsupported => "unsupported",
            Self::Busy => "busy",
            Self::Stopped => "stopped",
        }
    }

    /// 落 `checkin_records.status` 时的取值。
    ///
    /// 表的 CHECK 只允许四个值（`claimed` / `already` / `failed` / `skipped`），
    /// 细分类走 `error_kind` 列 —— 不为了多几个枚举值去重建一张已经发布的表。
    pub fn daily_status(self) -> &'static str {
        match self {
            Self::Claimed => crate::storage::checkin::STATUS_CLAIMED,
            Self::Already => crate::storage::checkin::STATUS_ALREADY,
            Self::Disabled | Self::Unsupported | Self::Busy | Self::Stopped | Self::Unchecked => {
                crate::storage::checkin::STATUS_SKIPPED
            }
            Self::Unknown | Self::Unauthorized | Self::NoCredential | Self::Failed => {
                crate::storage::checkin::STATUS_FAILED
            }
        }
    }

    /// 服务端意义上的"今天已签"。`already` 与 `claimed` 都算，
    /// 但只有 `claimed` 能算"AICC 这次拿到的积分"。
    pub fn checked_today(self) -> bool {
        matches!(self, Self::Claimed | Self::Already)
    }

    /// 这一轮要不要为它写"最终结论"。
    ///
    /// `Unchecked` 是中间态（马上要提交），`Busy` 是并发拒绝，
    /// 两者都不该把当天的结论改掉。
    pub fn writes_daily_record(self) -> bool {
        !matches!(self, Self::Unchecked | Self::Busy)
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 查完状态之后该做什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterStatus {
    /// 已签到：不发写请求。
    Already,
    /// 明确未签到：可以提交。
    Submit,
    /// 说不清：不提交、不落成功。
    Unknown,
}

/// 这一步是整个模块的安全阀。
///
/// `None` 必须是 `Unknown`：`wb-switch-core` 在这里用 `unwrap_or(false)`，
/// 一旦接口改结构就会变成每天重复签到一次。PRD §3「接口处理原则」第 4 条。
pub fn after_status(checked: Option<bool>) -> AfterStatus {
    match checked {
        Some(true) => AfterStatus::Already,
        Some(false) => AfterStatus::Submit,
        None => AfterStatus::Unknown,
    }
}

/// 提交之后回查的结果怎么算。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AfterVerify {
    /// 回查确认已签到：唯一允许记 `claimed` 与写奖励的情况。
    Claimed,
    /// 回查说不清 / 说没签：记 `unknown`，不给奖励。
    Unknown,
}

pub fn after_verify(checked: Option<bool>) -> AfterVerify {
    match checked {
        Some(true) => AfterVerify::Claimed,
        _ => AfterVerify::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_boolean_never_submits() {
        assert_eq!(after_status(None), AfterStatus::Unknown);
        assert_eq!(after_status(Some(false)), AfterStatus::Submit);
        assert_eq!(after_status(Some(true)), AfterStatus::Already);
    }

    #[test]
    fn only_a_confirmed_lookback_counts_as_claimed() {
        assert_eq!(after_verify(Some(true)), AfterVerify::Claimed);
        assert_eq!(after_verify(Some(false)), AfterVerify::Unknown);
        assert_eq!(after_verify(None), AfterVerify::Unknown);
    }

    #[test]
    fn daily_status_stays_inside_the_tables_check_constraint() {
        for state in [
            State::Disabled,
            State::Unchecked,
            State::Already,
            State::Claimed,
            State::Unknown,
            State::Unauthorized,
            State::NoCredential,
            State::Failed,
            State::Unsupported,
            State::Busy,
            State::Stopped,
        ] {
            assert!(
                matches!(
                    state.daily_status(),
                    "claimed" | "already" | "failed" | "skipped"
                ),
                "{} 落库时会违反 CHECK 约束",
                state
            );
        }
    }

    #[test]
    fn only_claimed_and_already_count_as_checked_today() {
        assert!(State::Claimed.checked_today());
        assert!(State::Already.checked_today());
        assert!(!State::Unknown.checked_today());
        assert!(!State::Failed.checked_today());
        assert!(!State::Disabled.checked_today());
    }

    #[test]
    fn a_busy_account_does_not_rewrite_the_days_conclusion() {
        assert!(!State::Busy.writes_daily_record());
        assert!(!State::Unchecked.writes_daily_record());
        assert!(State::Unknown.writes_daily_record());
    }
}
