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
/// 请求已经发出（或即将发出，两者在崩溃语义上等价）。
pub const PHASE_SENT: &str = "sent";
/// 服务端明确给了结论：到账。
pub const PHASE_CONFIRMED: &str = "confirmed";
/// 服务端明确拒绝（常态拒绝或幂等回执）。
pub const PHASE_REJECTED: &str = "rejected";
/// 请求没能完成（网络、5xx 且回查也说不清）。
pub const PHASE_FAILED: &str = "failed";
/// 结果不明，等回查。
pub const PHASE_UNKNOWN: &str = "unknown";
/// 冻结：尝试次数用尽或阶段认不出来，等人工核对。
pub const PHASE_FROZEN: &str = "frozen";

/// 一个目标最多发三次。超过就冻结 —— 继续重试只会让"到底扣了几次"更难说清。
pub const MAX_SEND_ATTEMPTS: i64 = 3;

/// 未决阶段：同一账号同一目标，物理上只允许有一条。
pub const OPEN_PHASES: [&str; 4] = [PHASE_PENDING, PHASE_SENT, PHASE_UNKNOWN, PHASE_FROZEN];

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
/// 判定顺序：先把"不能发"的都说清楚，再考虑发。
pub fn plan(existing: Option<&Open>, needs_token: bool) -> Next {
    let Some(op) = existing else {
        return Next::Insert {
            client_token: needs_token.then(client_token),
        };
    };

    if op.attempt >= MAX_SEND_ATTEMPTS {
        return Next::Blocked {
            op_id: Some(op.id.clone()),
            reason: "尝试次数已用尽，需要人工核对服务端状态",
        };
    }

    match op.phase.as_str() {
        // 落盘了但还没发：token 是那时选的，必须沿用，否则等于换了一次新操作。
        PHASE_PENDING if op.attempt == 0 => Next::Resend {
            op_id: op.id.clone(),
            client_token: op.client_token.clone(),
        },
        PHASE_PENDING => Next::Recheck {
            op_id: op.id.clone(),
            reason: RecheckReason::InFlight,
        },
        PHASE_SENT => Next::Recheck {
            op_id: op.id.clone(),
            reason: RecheckReason::PreviousResultUnknown,
        },
        PHASE_UNKNOWN => Next::Recheck {
            op_id: op.id.clone(),
            reason: RecheckReason::AwaitingVerification,
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

    #[test]
    fn attempts_are_capped() {
        let exhausted = Open::new(
            "op-1",
            PHASE_UNKNOWN,
            MAX_SEND_ATTEMPTS,
            Some("u-token".into()),
        );
        assert!(matches!(plan(Some(&exhausted), true), Next::Blocked { .. }));
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
}
