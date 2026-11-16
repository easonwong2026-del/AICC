//! 奖励操作的本地账本策略：**下一步该做什么，只由这里决定**（PR #95）。
//!
//! ## 为什么需要账本
//!
//! 兑换 / 抽奖 / 补签都是"消耗一次机会"的写操作。它们的共同危险不是"失败"，
//! 而是**不知道成没成**：
//!
//! ```text
//! 发出 POST /lottery/draw → 本机超时
//!   服务端其实已经抽掉了，但我们没收到响应
//! 若此时生成新 client_token 再抽一次 → 用户被多扣一次次数
//! ```
//!
//! 所以每一次发送都必须**先落盘再发**，并把 `client_token` 存在磁盘上：进程崩了、
//! 换了个进程起来，也只能用**同一个** token 去回查，不能重新抽一次。
//!
//! ## 调用方要遵守的时序
//!
//! 1. `plan(None, ..)` → `Insert`：先 `open_reward_op` 落盘，再 `mark_sent`，再发请求；
//! 2. 拿到可判定的结果 → `resolve_reward_op`；
//! 3. 拿到 `Indeterminate` / 5xx / `Unknown` → 落 `unknown`，下一轮 `plan` 会给出
//!    `Recheck`，**先回查服务端再决定**，期间不许发写请求。
//!
//! ## 不做什么
//!
//! 这里不发请求、不碰凭据、不做重试循环。`Recheck` 只说明"必须先查服务端"，
//! 查完怎么收敛由 #96/#97 的编排层决定。

#![allow(dead_code)] // #96/#97 接线；策略先锁死，避免接线时临时加"就这一次"的例外。

use super::streak_rewards::client_token;

/// 已经落盘、还没结束。发请求之前就要有它。
pub const PHASE_PENDING: &str = "pending";
/// 服务端明确没执行（404 路由不匹配 / tier 参数不认识）：允许用**同一个 token**
/// 再发一次。它仍然占着目标，所以并发下不会变成两次操作。
pub const PHASE_READY: &str = "ready";
/// 请求已经发出（或即将发出，两者在崩溃语义上等价）。
pub const PHASE_SENT: &str = "sent";
/// 服务端明确给了结论：到账。
pub const PHASE_CONFIRMED: &str = "confirmed";
/// 服务端明确拒绝（常态拒绝或幂等回执）。
pub const PHASE_REJECTED: &str = "rejected";
/// 请求没能完成，**且回查确认它确实没生效**。只有这种"核实过没做"才允许关闭。
pub const PHASE_FAILED: &str = "failed";
/// 结果不明，等回查。
pub const PHASE_UNKNOWN: &str = "unknown";
/// 冻结：认不出来的 403 / 风控 / 尝试次数用尽，等人工核对。
pub const PHASE_FROZEN: &str = "frozen";

/// 一个目标最多发三次。超过就冻结 —— 继续重试只会让"到底扣了几次"更难说清。
///
/// **真正的闸门在存储层**（`storage::reward_ops::MAX_SEND_ATTEMPTS` 写进
/// `mark_reward_op_sent` 的原子 `UPDATE ... AND attempt < ?`）。这里这份只是让
/// `plan()` 提前给出 `Blocked`，不能替代存储层 —— 调用方可以不问 `plan()` 就发（PR #96）。
pub const MAX_SEND_ATTEMPTS: i64 = crate::storage::reward_ops::MAX_SEND_ATTEMPTS;

/// 未决阶段：同一账号同一目标，物理上只允许有一条。
pub const OPEN_PHASES: [&str; 5] = [
    PHASE_PENDING,
    PHASE_READY,
    PHASE_SENT,
    PHASE_UNKNOWN,
    PHASE_FROZEN,
];

/// 账本里的一行（只取决策需要的字段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Open {
    pub id: String,
    pub phase: String,
    pub attempt: i64,
    pub client_token: Option<String>,
}

impl Open {
    pub fn new(
        id: impl Into<String>,
        phase: impl Into<String>,
        attempt: i64,
        token: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            phase: phase.into(),
            attempt,
            client_token: token,
        }
    }
}

impl From<&crate::storage::reward_ops::RewardOp> for Open {
    fn from(op: &crate::storage::reward_ops::RewardOp) -> Self {
        Self {
            id: op.id.clone(),
            phase: op.phase.clone(),
            attempt: op.attempt,
            client_token: op.client_token.clone(),
        }
    }
}

/// 为什么必须先回查。界面上要能说清"现在为什么不动"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecheckReason {
    /// 上一次请求已经发出，但没拿到可判定的响应（超时 / 5xx / 认不出来）。
    PreviousResultUnknown,
    /// 上一轮已经把它落成 unknown，还没回查。
    AwaitingVerification,
    /// 记录停在 `pending` 但已经发过 —— 不该出现的中间态，按"可能已发出"处理。
    InFlight,
}

impl RecheckReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreviousResultUnknown => "previous_result_unknown",
            Self::AwaitingVerification => "awaiting_verification",
            Self::InFlight => "in_flight",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::PreviousResultUnknown | Self::InFlight => {
                "上一次请求的结果还没确认，先回查服务端"
            }
            Self::AwaitingVerification => "这一条还没核验，先回查服务端状态",
        }
    }
}

/// 下一步动作。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// 新建一条账本记录并发送。**唯一**允许生成新 `client_token` 的分支。
    Insert { client_token: Option<String> },
    /// 沿用已经落盘的 token 重发（崩溃发生在发送之前，服务端什么都没收到）。
    Resend {
        op_id: String,
        client_token: Option<String>,
    },
    /// 必须先回查服务端。**不许发写请求。**
    Recheck {
        op_id: String,
        reason: RecheckReason,
    },
    /// 停手，等人工核对。
    Blocked {
        op_id: Option<String>,
        reason: &'static str,
    },
}

/// 决策入口。`needs_token` 为假时（补签）不生成幂等键。
///
/// **顺序很重要**：先判"必须回查"，再判"能不能发"。
/// 次数上限只约束"再发一次"，绝不能约束只读回查 —— 否则第 3 次发送后
/// 就再也查不了结果，那条记录会永远悬在 `sent`，用户看到的是"卡住了"。
pub fn plan(existing: Option<&Open>, needs_token: bool) -> Next {
    let Some(op) = existing else {
        return Next::Insert {
            client_token: needs_token.then(client_token),
        };
    };

    match op.phase.as_str() {
        // 已经发出去、结果不明：**不论第几次**都要回查。这是唯一的收敛路径。
        PHASE_SENT => Next::Recheck {
            op_id: op.id.clone(),
            reason: RecheckReason::PreviousResultUnknown,
        },
        PHASE_UNKNOWN => Next::Recheck {
            op_id: op.id.clone(),
            reason: RecheckReason::AwaitingVerification,
        },
        // 服务端明确没执行、也没别的信息可查：只剩"再发一次"或"停手"。
        PHASE_READY if op.attempt >= MAX_SEND_ATTEMPTS => Next::Blocked {
            op_id: Some(op.id.clone()),
            reason: "尝试次数已用尽，需要人工核对服务端状态",
        },
        PHASE_READY => Next::Resend {
            op_id: op.id.clone(),
            client_token: op.client_token.clone(),
        },
        // 落盘了但还没发：token 是那时选的，必须沿用，否则等于换了一次新操作。
        PHASE_PENDING if op.attempt == 0 => Next::Resend {
            op_id: op.id.clone(),
            client_token: op.client_token.clone(),
        },
        PHASE_PENDING => Next::Recheck {
            op_id: op.id.clone(),
            reason: RecheckReason::InFlight,
        },
        PHASE_FROZEN => Next::Blocked {
            op_id: Some(op.id.clone()),
            reason: "这一条已冻结，等人工核对",
        },
        // 认不出来的阶段绝不能当成"可以发"。
        _ => Next::Blocked {
            op_id: Some(op.id.clone()),
            reason: "账本里的阶段不认识，停止发送",
        },
    }
}

// ---------------------------------------------------------------------------
// 回查：只读判定（PR #96）
// ---------------------------------------------------------------------------

/// 回查时**观察到的事实**。它不是"请求成功了吗"，而是"服务端现在长什么样"。
///
/// #96 只负责把观察结果翻成结论；真正去 GET 的编排在 #97。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation {
    /// 目标变成"已完成"：抽奖次数少了 / 档位变 claimed / 补签日期进了 makeup_dates。
    Applied,
    /// 目标状态与发请求之前完全一致 —— 说明那次请求确实没生效。
    NotApplied,
    /// 服务端明确拒绝（活动结束、不再满足条件）。
    Rejected,
    /// 看不出来：查询失败、字段缺失、状态自相矛盾。
    Inconclusive,
}

/// 回查结论。`phase` 直接就是账本要落的阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 真的生效了。
    Confirmed,
    /// 回查确认没生效 —— 这是唯一允许落 `failed` 的路径（并且必须带 verified）。
    NotApplied,
    /// 服务端明确拒绝。
    Rejected,
    /// 查不清：继续 `unknown`，继续占着目标。
    StillUnknown,
}

impl Verdict {
    pub fn phase(self) -> &'static str {
        match self {
            Self::Confirmed => PHASE_CONFIRMED,
            Self::NotApplied => PHASE_FAILED,
            Self::Rejected => PHASE_REJECTED,
            Self::StillUnknown => PHASE_UNKNOWN,
        }
    }

    /// 落盘时要不要带 `verified`。`failed` 必须带，否则存储层直接拒绝。
    pub fn verified(self) -> bool {
        !matches!(self, Self::StillUnknown)
    }

    pub fn describe(self) -> &'static str {
        match self {
            Self::Confirmed => "服务端状态显示这次操作已生效",
            Self::NotApplied => "服务端状态与操作前一致，这次请求没有生效",
            Self::Rejected => "服务端明确拒绝",
            Self::StillUnknown => "回查仍无法确认，保持占用并等待下一次回查",
        }
    }
}

/// 把观察结果翻成结论。**看不出来就不许下结论**：`Inconclusive` 只能落到
/// `StillUnknown`，绝不会变成"没生效"而释放目标（那是重复消耗的入口）。
pub fn verdict(observation: Observation) -> Verdict {
    match observation {
        Observation::Applied => Verdict::Confirmed,
        Observation::NotApplied => Verdict::NotApplied,
        Observation::Rejected => Verdict::Rejected,
        Observation::Inconclusive => Verdict::StillUnknown,
    }
}

/// 这个账号现在能不能对某个目标发写请求（只读判断，不发送）。
pub fn may_send_now(existing: Option<&Open>, needs_token: bool) -> Next {
    plan(existing, needs_token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(phase: &str, attempt: i64) -> Open {
        Open::new("op-1", phase, attempt, Some("u-token".into()))
    }

    #[test]
    fn no_record_means_a_fresh_operation_with_a_fresh_token() {
        match plan(None, true) {
            Next::Insert { client_token } => {
                let token = client_token.expect("抽奖/兑换必须有幂等键");
                assert!(token.starts_with("u-"));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn makeup_gets_no_token_because_its_key_is_the_date() {
        assert_eq!(plan(None, false), Next::Insert { client_token: None });
    }

    #[test]
    fn a_crash_before_sending_reuses_the_persisted_token() {
        // 记录了 token、请求还没发出去。重发必须用同一个 token。
        assert_eq!(
            plan(Some(&open(PHASE_PENDING, 0)), true),
            Next::Resend {
                op_id: "op-1".into(),
                client_token: Some("u-token".into())
            }
        );
    }

    #[test]
    fn a_sent_operation_must_be_rechecked_and_never_resent() {
        assert_eq!(
            plan(Some(&open(PHASE_SENT, 1)), true),
            Next::Recheck {
                op_id: "op-1".into(),
                reason: RecheckReason::PreviousResultUnknown
            }
        );
    }

    #[test]
    fn an_unknown_operation_waits_for_verification() {
        assert_eq!(
            plan(Some(&open(PHASE_UNKNOWN, 1)), true),
            Next::Recheck {
                op_id: "op-1".into(),
                reason: RecheckReason::AwaitingVerification
            }
        );
    }

    #[test]
    fn a_pending_record_that_already_sent_something_is_rechecked() {
        // 中间态：phase 还是 pending 但 attempt > 0。可能已经发出，按 InFlight 处理。
        assert_eq!(
            plan(Some(&open(PHASE_PENDING, 1)), true),
            Next::Recheck {
                op_id: "op-1".into(),
                reason: RecheckReason::InFlight
            }
        );
    }

    /// P1-1：次数上限只约束"再发一次"，不能拦住只读回查。
    /// 否则第 3 次发送之后结果就永远查不到，记录停在 `sent` 上不会再动。
    #[test]
    fn the_attempt_cap_blocks_sending_but_never_blocks_a_recheck() {
        for phase in [PHASE_SENT, PHASE_UNKNOWN] {
            let exhausted = Open::new("op-1", phase, MAX_SEND_ATTEMPTS, Some("u-token".into()));
            match plan(Some(&exhausted), true) {
                Next::Recheck { op_id, .. } => assert_eq!(op_id, "op-1"),
                other => panic!("{phase} + attempt 用尽仍必须回查，实际 {other:?}"),
            }
        }

        // ready 是"服务端明确没执行"，没有可回查的东西，次数用尽就只能停手。
        let exhausted = Open::new(
            "op-1",
            PHASE_READY,
            MAX_SEND_ATTEMPTS,
            Some("u-token".into()),
        );
        assert!(matches!(plan(Some(&exhausted), true), Next::Blocked { .. }));
    }

    #[test]
    fn a_ready_operation_resends_with_the_same_token_until_the_cap() {
        for attempt in 0..MAX_SEND_ATTEMPTS {
            let op = Open::new("op-1", PHASE_READY, attempt, Some("u-token".into()));
            assert_eq!(
                plan(Some(&op), true),
                Next::Resend {
                    op_id: "op-1".into(),
                    client_token: Some("u-token".into())
                },
                "attempt={attempt} 应允许用同一 token 再发"
            );
        }
    }

    #[test]
    fn a_frozen_or_unrecognised_phase_never_leads_to_a_send() {
        assert!(matches!(
            plan(Some(&open(PHASE_FROZEN, 1)), true),
            Next::Blocked { .. }
        ));
        assert!(matches!(
            plan(Some(&open("something_new", 0)), true),
            Next::Blocked { .. }
        ));
    }

    #[test]
    fn no_branch_other_than_insert_invents_a_token() {
        // 遍历所有未决阶段：除了 Insert，别的分支都不该带出新 token。
        for phase in OPEN_PHASES {
            let op = open(phase, 1);
            match plan(Some(&op), true) {
                Next::Insert { .. } => panic!("有未决记录时不许新建：{phase}"),
                Next::Resend { client_token, .. } => {
                    assert_eq!(client_token.as_deref(), Some("u-token"), "{phase}")
                }
                Next::Recheck { .. } | Next::Blocked { .. } => {}
            }
        }
    }

    // --- 回查判定 -----------------------------------------------------------

    #[test]
    fn only_applied_observations_close_as_confirmed() {
        assert_eq!(verdict(Observation::Applied), Verdict::Confirmed);
        assert_eq!(verdict(Observation::Applied).phase(), "confirmed");
        assert!(verdict(Observation::Applied).verified());
    }

    #[test]
    fn an_inconclusive_recheck_never_claims_the_request_did_not_happen() {
        // 这是整条回查链上最危险的一次"猜"：猜成没生效就释放目标，下一轮会重发。
        assert_eq!(verdict(Observation::Inconclusive), Verdict::StillUnknown);
        assert_eq!(verdict(Observation::Inconclusive).phase(), "unknown");
        assert!(!verdict(Observation::Inconclusive).verified(), "没核实过");
    }

    #[test]
    fn not_applied_needs_verification_because_it_lands_on_failed() {
        let not_applied = verdict(Observation::NotApplied);
        assert_eq!(not_applied.phase(), "failed");
        assert!(
            not_applied.verified(),
            "failed 必须带 verified，否则存储层拒绝"
        );
    }

    #[test]
    fn a_rejected_recheck_closes_the_target() {
        let rejected = verdict(Observation::Rejected);
        assert_eq!(rejected.phase(), "rejected");
        assert!(rejected.verified());
    }
}
