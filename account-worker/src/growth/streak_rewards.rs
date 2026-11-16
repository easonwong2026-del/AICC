//! 连登奖励兑换、活动抽奖、补签卡使用：**只做请求形态与判定，不发请求**。
//!
//! 这三个动作与 `rewards.rs` 里的"接单/领奖"不同：它们会消耗账号的一次机会，
//! 一旦参数错了就是真金白银的损失。所以本模块只包含两件可测试的事：
//!
//! 1. **请求形态**（路径 + body）。参考第三方实现 `magpie-workbuddy-tasks`
//!    的 `engine/streak.mjs`：
//!
//!    | 动作 | 路径 | body |
//!    | --- | --- | --- |
//!    | 连登兑换 | `/activity/growth/redeem` | `{"tier":"14d","client_token":"<uuid>"}` |
//!    | 抽奖 | `/activity/growth/lottery/draw` | `{"client_token":"<uuid>"}` |
//!    | 补签 | `/activity/growth/makeup-cards/use` | `{"target_date":"YYYY-MM-DD"}` |
//!
//!    **这不是已验证的生产接口。** AICC 自己的实测前缀是 `/v2/activity/growth`
//!    （见 `api.rs` 的两个候选前缀），域名/前缀/当前协议必须用真实请求确认后
//!    才允许接线。因此路径由调用方传入的 `base` 拼出来，模块里不写死域名。
//!
//! 2. **判定**（`classify`）：把响应翻成"到底发生了什么"。判定必须能区分
//!    "领到了"、"本来就已经领过"、"次数不够"、"没资格"、"被限流"、"没登录"。
//!    把这些混成一个 `failed` 会让界面报假故障，也无法做退避重试。
//!
//! 接线（真正发请求）留到 C3/C4，且必须先拿到一次真实请求的证据。

#![allow(dead_code)] // 接线在 C3/C4；判定逻辑先锁定，避免接线时临时改判定。

use serde_json::{json, Value};

/// 连登奖励的三个档位。服务端用 `7d` / `14d` / `28d` 这种字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    Days7,
    Days14,
    Days28,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Days7 => "7d",
            Tier::Days14 => "14d",
            Tier::Days28 => "28d",
        }
    }

    pub fn parse(raw: &str) -> Option<Tier> {
        match raw {
            "7d" => Some(Tier::Days7),
            "14d" => Some(Tier::Days14),
            "28d" => Some(Tier::Days28),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Redeem(Tier),
    LotteryDraw,
    UseMakeupCard { target_date: String },
}

/// 一次动作的请求形态。`base` 由调用方给出（前缀未验证，不写死）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub url: String,
    pub body: Value,
}

/// `client_token` 是幂等键：同一次操作重试要带同一个值，否则服务端可能当成两次。
pub fn request(base: &str, command: &Command, client_token: &str) -> Request {
    let base = base.trim_end_matches('/');
    match command {
        Command::Redeem(tier) => Request {
            url: format!("{base}/activity/growth/redeem"),
            body: json!({ "tier": tier.as_str(), "client_token": client_token }),
        },
        Command::LotteryDraw => Request {
            url: format!("{base}/activity/growth/lottery/draw"),
            body: json!({ "client_token": client_token }),
        },
        Command::UseMakeupCard { target_date } => Request {
            url: format!("{base}/activity/growth/makeup-cards/use"),
            body: json!({ "target_date": target_date }),
        },
    }
}

/// 一次动作的结果。**没有** "failed" 这个兜底：分类不清就是 `Unknown`，
/// 交给调用方回查状态，而不是编一个结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Claimed {
        credits: Option<i64>,
        energy: Option<i64>,
    },
    /// 幂等回执：这次没领到新的，但状态是对的。
    AlreadyClaimed,
    /// 次数不够 / 档位未达成。
    NotEligible,
    /// 需要重新登录（401/403）。
    Unauthorized,
    /// 被限流，可以退避后重试。
    RateLimited {
        retry_after_secs: Option<i64>,
    },
    ServerError {
        status: u16,
    },
    /// 响应里没有能判定的字段 —— 不猜。
    Unknown,
}

/// 把响应翻成结果。只看 `http_status` 与 body 里明确出现的字段。
pub fn classify(http_status: u16, body: &Value) -> Outcome {
    if http_status == 401 || http_status == 403 {
        return Outcome::Unauthorized;
    }
    if http_status == 429 {
        return Outcome::RateLimited {
            retry_after_secs: body
                .get("retry_after")
                .or_else(|| body.get("retryAfter"))
                .and_then(Value::as_i64),
        };
    }
    if !(200..300).contains(&http_status) {
        return Outcome::ServerError {
            status: http_status,
        };
    }

    // 服务端有 `code` 字段时先看它：200 也可能是业务失败。
    if let Some(code) = body.get("code").and_then(Value::as_i64) {
        if code != 0 {
            let message = body
                .get("message")
                .or_else(|| body.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_lowercase();
            if message.contains("limit") || message.contains("频繁") || message.contains("too many")
            {
                return Outcome::RateLimited {
                    retry_after_secs: None,
                };
            }
            return Outcome::NotEligible;
        }
    }

    let data = body.get("data").unwrap_or(body);
    if flag(data, &["already", "already_claimed", "alreadyClaimed"]) {
        return Outcome::AlreadyClaimed;
    }
    // 明确写"不可领"的情况：档位没到 / 补签卡不足。
    if flag(
        data,
        &["locked", "not_eligible", "notEligible", "insufficient"],
    ) {
        return Outcome::NotEligible;
    }
    let credits = number(data, &["credits", "credit", "reward_credit"]);
    let energy = number(data, &["energy", "reward_energy"]);
    if credits.is_some() || energy.is_some() || flag(data, &["ok", "success"]) {
        return Outcome::Claimed { credits, energy };
    }
    Outcome::Unknown
}

fn flag(value: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .any(|key| value.get(*key).and_then(Value::as_bool).unwrap_or(false))
}

fn number(value: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        match value.get(*key) {
            Some(Value::Number(number)) => {
                if let Some(value) = number.as_i64() {
                    return Some(value);
                }
            }
            Some(Value::String(text)) => {
                if let Ok(parsed) = text.trim().parse::<i64>() {
                    return Some(parsed);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_match_the_documented_shapes() {
        let base = "https://example.test/v2/activity/growth-v2/";
        let redeem = request(base, &Command::Redeem(Tier::Days14), "token-1");
        assert_eq!(
            redeem.url,
            "https://example.test/v2/activity/growth-v2/activity/growth/redeem"
        );
        assert_eq!(redeem.body["tier"], "14d");
        assert_eq!(redeem.body["client_token"], "token-1");

        let draw = request("https://example.test", &Command::LotteryDraw, "token-2");
        assert_eq!(
            draw.url,
            "https://example.test/activity/growth/lottery/draw"
        );
        assert_eq!(draw.body["client_token"], "token-2");

        let makeup = request(
            "https://example.test",
            &Command::UseMakeupCard {
                target_date: "2026-11-10".into(),
            },
            "ignored",
        );
        assert_eq!(
            makeup.url,
            "https://example.test/activity/growth/makeup-cards/use"
        );
        assert_eq!(makeup.body["target_date"], "2026-11-10");
        assert!(makeup.body.get("client_token").is_none(), "补签不带幂等键");
    }

    #[test]
    fn a_successful_redeem_reports_what_was_granted() {
        let body = json!({"code": 0, "data": {"credits": 100}});
        assert_eq!(
            classify(200, &body),
            Outcome::Claimed {
                credits: Some(100),
                energy: None
            }
        );
    }

    #[test]
    fn an_idempotent_receipt_is_not_a_failure() {
        let body = json!({"code": 0, "data": {"already": true}});
        assert_eq!(classify(200, &body), Outcome::AlreadyClaimed);
        let camel = json!({"data": {"alreadyClaimed": true}});
        assert_eq!(classify(200, &camel), Outcome::AlreadyClaimed);
    }

    #[test]
    fn an_unreached_tier_is_not_reported_as_a_failure() {
        let body = json!({"code": 0, "data": {"locked": true}});
        assert_eq!(classify(200, &body), Outcome::NotEligible);
        let insufficient = json!({"data": {"insufficient": true}});
        assert_eq!(classify(200, &insufficient), Outcome::NotEligible);
    }

    #[test]
    fn a_200_with_a_business_error_code_is_not_success() {
        let body = json!({"code": 1001, "message": "tier not reached"});
        assert_eq!(classify(200, &body), Outcome::NotEligible);
        let limited = json!({"code": 1002, "message": "Too many requests"});
        assert_eq!(
            classify(200, &limited),
            Outcome::RateLimited {
                retry_after_secs: None
            }
        );
    }

    #[test]
    fn auth_and_limits_are_kept_apart() {
        assert_eq!(classify(401, &json!({})), Outcome::Unauthorized);
        assert_eq!(classify(403, &json!({})), Outcome::Unauthorized);
        assert_eq!(
            classify(429, &json!({"retry_after": 30})),
            Outcome::RateLimited {
                retry_after_secs: Some(30)
            }
        );
        assert_eq!(
            classify(500, &json!({})),
            Outcome::ServerError { status: 500 }
        );
    }

    #[test]
    fn an_unrecognisable_body_is_unknown_rather_than_success() {
        assert_eq!(
            classify(200, &json!({"data": {"something": 1}})),
            Outcome::Unknown
        );
        assert_eq!(classify(200, &json!({})), Outcome::Unknown);
    }

    #[test]
    fn tiers_round_trip_and_reject_unknown_values() {
        for tier in [Tier::Days7, Tier::Days14, Tier::Days28] {
            assert_eq!(Tier::parse(tier.as_str()), Some(tier));
        }
        assert_eq!(Tier::parse("30d"), None);
        assert_eq!(Tier::parse("14"), None);
    }
}
