//! 连登奖励兑换、活动抽奖、补签卡使用：**请求形态 + 结果判定，不发请求**。
//!
//! 协议事实与证据等级见 `docs/growth-rewards-protocol-2026-11-16.md`。三条边界：
//!
//! 1. **只产出 `suffix + body`，不拼 URL。** 唯一的拼接点是 `growth::api::endpoint`。
//!    上一版由本模块自己再拼 `/activity/growth/xxx`，调用方一旦传进已经是
//!    `/v2/activity/growth` 的前缀就变成 `.../activity/growth/activity/growth/redeem`。
//!
//! 2. **判定要能分开"业务常态"和"真故障"。** 连登天数不足是 403、抽奖次数为 0 是 400、
//!    补签日期没断是 400 —— 这三种都是常态。把它们和 401 混在一起，会让账号被标成
//!    授权失效去换源；把它们和未知错误码混在一起，会让界面报假故障。
//!
//! 3. **只有 401 能改账号凭据状态。** 见 [`Outcome::credential_effect`]。
//!
//! 这个模块里没有 `api::post` 的调用点。#96 接线前必须先拿到 A 级证据（真实写请求）。

#![allow(dead_code)] // 接线在 #96/#97；形态与判定先锁死，避免接线时临时改语义。

use chrono::NaiveDate;
use serde::Serialize;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// 动作
// ---------------------------------------------------------------------------

/// 三个会消耗机会的动作。字符串就是账本 `action` 列的取值。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Redeem,
    LotteryDraw,
    MakeupCard,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Redeem => "redeem",
            Self::LotteryDraw => "lottery_draw",
            Self::MakeupCard => "makeup_card",
        }
    }
}

/// 连登奖励的三个档位。服务端认 `7d` / `14d` / `28d` 这种**档位标识**。
///
/// 不是天数：实测传 `"starter"` / `"7"` / `7` 分别得到 unknown tier / unknown tier /
/// invalid request，只有 `"7d"` 会 200（88lin `signin.py:2073`，证据 B）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Tier {
    Days7,
    Days14,
    Days28,
}

impl Tier {
    pub const ALL: [Tier; 3] = [Tier::Days7, Tier::Days14, Tier::Days28];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Days7 => "7d",
            Self::Days14 => "14d",
            Self::Days28 => "28d",
        }
    }

    /// 只认档位标识。天数、档位名一律返回 `None` —— 那是"不该发出去的请求"。
    pub fn parse(raw: &str) -> Option<Tier> {
        match raw.trim() {
            "7d" => Some(Self::Days7),
            "14d" => Some(Self::Days14),
            "28d" => Some(Self::Days28),
            _ => None,
        }
    }

    /// 兜底写法：天数。**只允许**在 `InvalidRequest(UnknownTier)` 之后用一次 ——
    /// 那类 400 发生在参数校验阶段，服务端没兑换任何东西，重试不会重复领取。
    pub fn days(self) -> i64 {
        match self {
            Self::Days7 => 7,
            Self::Days14 => 14,
            Self::Days28 => 28,
        }
    }

    /// 档位的两个状态键。两种服务端形态都被实测见过，所以都要认：
    ///
    /// - `tier_7d_status` / `tier_14d_status` / `tier_28d_status`（AICC 自己读过的形态）
    /// - `starter_status` / `advanced_status` / `legendary_status`（88lin 读的 `/redeem/summary`）
    pub fn status_keys(self) -> [&'static str; 2] {
        match self {
            Self::Days7 => ["tier_7d_status", "starter_status"],
            Self::Days14 => ["tier_14d_status", "advanced_status"],
            Self::Days28 => ["tier_28d_status", "legendary_status"],
        }
    }

    /// 展示名，仅用于界面与日志。
    pub fn label(self) -> &'static str {
        match self {
            Self::Days7 => "入门（7 天）",
            Self::Days14 => "进阶（14 天）",
            Self::Days28 => "巅峰（28 天）",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Redeem(Tier),
    LotteryDraw,
    MakeupCard { target_date: String },
}

impl Command {
    pub fn action(&self) -> Action {
        match self {
            Self::Redeem(_) => Action::Redeem,
            Self::LotteryDraw => Action::LotteryDraw,
            Self::MakeupCard { .. } => Action::MakeupCard,
        }
    }

    /// 账本里用来区分"同一个动作的不同目标"。抽奖没有目标，所以是空串 ——
    /// 效果是"同一账号同时只允许一条未决的抽奖记录"，两个进程不会各抽一次。
    pub fn target(&self) -> String {
        match self {
            Self::Redeem(tier) => tier.as_str().to_string(),
            Self::LotteryDraw => String::new(),
            Self::MakeupCard { target_date } => target_date.clone(),
        }
    }
}

// ---------------------------------------------------------------------------
// 请求形态
// ---------------------------------------------------------------------------

pub const SUFFIX_REDEEM: &str = "/redeem";
pub const SUFFIX_LOTTERY_DRAW: &str = "/lottery/draw";
pub const SUFFIX_MAKEUP_USE: &str = "/makeup-cards/use";

/// 一次请求：**前缀相对**的 suffix + body。域名与前缀由 `api::endpoint` 负责。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub suffix: &'static str,
    pub body: Value,
}

/// 构造请求时的前置拒绝。**这一类错误一个请求都不发。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// 补签日期必须是自己算出来的 `YYYY-MM-DD`，不是从别处抄来的字符串。
    InvalidTargetDate(String),
}

/// `client_token` 是幂等键：同一次操作重发要带同一个值，否则服务端可能当成两次。
/// 官方前端用 `u-<uuid>` 拼，服务端只做去重、不校验格式（证据 B）。
pub fn client_token() -> String {
    format!("u-{}", uuid::Uuid::new_v4())
}

pub fn call(command: &Command, client_token: &str) -> Result<Call, Refusal> {
    match command {
        Command::Redeem(tier) => Ok(Call {
            suffix: SUFFIX_REDEEM,
            body: json!({ "tier": tier.as_str(), "client_token": client_token }),
        }),
        Command::LotteryDraw => Ok(Call {
            suffix: SUFFIX_LOTTERY_DRAW,
            body: json!({ "client_token": client_token }),
        }),
        Command::MakeupCard { target_date } => {
            let date = valid_date(target_date)
                .ok_or_else(|| Refusal::InvalidTargetDate(target_date.clone()))?;
            Ok(Call {
                suffix: SUFFIX_MAKEUP_USE,
                // 补签不带幂等键：幂等键是 `(账号, 日期)`，服务端自己会拒"没断的日期"。
                body: json!({ "target_date": date }),
            })
        }
    }
}

/// 严格 `YYYY-MM-DD`：位数、分隔符、可解析三者都要对。
/// 宽宽松松地放过去，等于把一个猜出来的日期发给服务端去消耗补签卡。
fn valid_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let bytes = raw.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return None;
    }
    NaiveDate::parse_from_str(raw, "%Y-%m-%d").ok()?;
    Some(raw.to_string())
}

// ---------------------------------------------------------------------------
// 结果
// ---------------------------------------------------------------------------

/// 服务端实际发了什么。四个字段全部来自响应里的数值，缺就是 `None` —— 不补 0。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Grant {
    pub credits: Option<i64>,
    pub energy: Option<i64>,
    pub cards: Option<i64>,
    pub chances: Option<i64>,
}

impl Grant {
    pub fn is_empty(&self) -> bool {
        self.credits.is_none()
            && self.energy.is_none()
            && self.cards.is_none()
            && self.chances.is_none()
    }
}

/// 抽奖的奖品。实物奖要提醒用户自己去填收件信息（AICC 不代填）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Prize {
    pub name: Option<String>,
    pub needs_address: bool,
}

/// 明确的常态拒绝。这些**不是故障**，不该进失败统计，也不该改授权状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rejection {
    /// 403 且文案是"连登天数不足"：档位还没解锁。
    TierLocked,
    /// 400 且文案是"没有抽奖次数"。
    NoChance,
    /// 400 且文案是"这一天没断签、不用补"。
    NothingToMakeup,
    /// 活动结束 / 已下线。
    ActivityEnded,
}

impl Rejection {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TierLocked => "tier_locked",
            Self::NoChance => "no_chance",
            Self::NothingToMakeup => "nothing_to_makeup",
            Self::ActivityEnded => "activity_ended",
        }
    }

    /// 界面上的一句话。**用"暂不可用"而不是"失败"** —— 用户没做错任何事。
    pub fn describe(self) -> &'static str {
        match self {
            Self::TierLocked => "连登天数还没到这一档",
            Self::NoChance => "没有可用的抽奖次数",
            Self::NothingToMakeup => "这一天没有断签，不需要补签",
            Self::ActivityEnded => "活动已结束",
        }
    }
}

/// 参数校验阶段的拒绝。服务端**什么都没做**，所以可以换一种写法重发一次。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Invalid {
    UnknownTier,
}

impl Invalid {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnknownTier => "unknown_tier",
        }
    }
}

/// 一次动作到底发生了什么。**没有 `Failed` 兜底**：认不出来就是 `Unknown`，
/// 交给调用方回查服务端，而不是编一个结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// 到账，带上服务端给的实发数值。
    Granted { grant: Grant, prize: Option<Prize> },
    /// 幂等回执：这次没领到新的，但状态是对的。
    AlreadyClaimed,
    /// 常态拒绝。
    Rejected(Rejection),
    /// 403 但文案认不出来：业务拒绝，授权没问题。
    Forbidden { message: String },
    /// 401：这份授权确实失效了。**唯一**允许改账号凭据状态的判定。
    Unauthorized,
    /// 429：等一会儿。
    RateLimited { retry_after_secs: Option<i64> },
    /// 5xx：可能已经生效，不许原地重发。
    ServerError { status: u16 },
    /// 没有响应（超时 / 连接断）：**可能已经在服务端生效**，必须先回查。
    Indeterminate,
    /// 404：这条路径什么都没发生，换前缀再问是安全的。
    RouteMissing,
    /// 参数校验期拒绝：换一种写法重发是安全的。
    InvalidRequest(Invalid),
    /// 认不出来。不猜。
    Unknown {
        status: u16,
        code: Option<i64>,
        message: String,
    },
}

/// 凭据状态的影响。整个成长中心只有一种情况该动凭据，所以这里没有"可能"。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialEffect {
    None,
    /// 401：可以让账号进入"待重新授权"。
    Reauthenticate,
}

/// 判定落到账本上的阶段。**这是"服务端说了什么"到"账本怎么记"的唯一翻译点**；
/// 存储层只认这几个字符串（`docs/growth-rewards-protocol-2026-11-16.md` §七）。
///
/// 关键区分是"能不能解除占用"：
///
/// | 阶段 | 解除占用 | 为什么 |
/// | --- | --- | --- |
/// | `confirmed` / `rejected` | 是 | 服务端明确给了终局答复 |
/// | `unknown` | 否 | 不知道做没做，只能回查 |
/// | `frozen` | 否 | 认不出来 / 可能风控，等人工 |
/// | `ready` | 否 | 服务端明确没执行，允许用**同一个 token** 再发一次 |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerPhase {
    Confirmed,
    Rejected,
    Unknown,
    Frozen,
    Ready,
}

impl LedgerPhase {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Rejected => "rejected",
            Self::Unknown => "unknown",
            Self::Frozen => "frozen",
            Self::Ready => "ready",
        }
    }

    /// 只有明确的终局答复才能解除"同一目标只允许一条未决记录"的占用。
    /// 未知 403、超时、5xx 都不在此列 —— 它们必须继续占着，否则下一轮就会生成
    /// 新的 `client_token`，把同一次用户意图变成两次消耗。
    pub fn closes(self) -> bool {
        matches!(self, Self::Confirmed | Self::Rejected)
    }
}

impl Outcome {
    /// 真的拿到了新奖励。
    pub fn granted(&self) -> bool {
        matches!(self, Self::Granted { .. })
    }

    /// 结论已定，账本可以关闭。`Unknown` / `RateLimited` / 5xx / 超时都不在此列。
    ///
    /// **`Forbidden` 也不在此列**：`403 risk control` 这类认不出来的拒绝，关掉之后
    /// 下一轮又会重新提交，等于拿同一份授权反复撞风控。它要走冻结。
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            Self::Granted { .. } | Self::AlreadyClaimed | Self::Rejected(_) | Self::Unauthorized
        )
    }

    /// 允许用**同一个 `client_token`** 再发一次：只限"服务端什么都没做"的两种。
    ///
    /// 5xx 与超时**不在**此列 —— 它们可能已经扣了次数，重发就是重复消耗。
    pub fn may_resend(&self) -> bool {
        matches!(self, Self::RouteMissing | Self::InvalidRequest(_))
    }

    /// 必须回查服务端才能定性。
    pub fn requires_recheck(&self) -> bool {
        matches!(
            self,
            Self::Indeterminate
                | Self::ServerError { .. }
                | Self::RateLimited { .. }
                | Self::Unknown { .. }
        )
    }

    /// 需要人工看一眼（不是"再查一次"就能解决的）。
    pub fn requires_attention(&self) -> bool {
        matches!(self, Self::Forbidden { .. })
    }

    /// 账本阶段。**一张表说完所有判定**，别在各个调用点再写一遍 match。
    pub fn ledger_phase(&self) -> LedgerPhase {
        match self {
            Self::Granted { .. } => LedgerPhase::Confirmed,
            // 幂等回执：这次没发新的，但状态已经是对的，目标可以释放。
            Self::AlreadyClaimed => LedgerPhase::Rejected,
            Self::Rejected(_) => LedgerPhase::Rejected,
            // 401 的请求根本没被受理，释放目标是安全的（重试会先要求重新授权）。
            Self::Unauthorized => LedgerPhase::Rejected,
            // 403 认不出来：业务拒绝 + 可能风控 → 冻结，不改凭据、不自动重发。
            Self::Forbidden { .. } => LedgerPhase::Frozen,
            Self::RateLimited { .. }
            | Self::ServerError { .. }
            | Self::Indeterminate
            | Self::Unknown { .. } => LedgerPhase::Unknown,
            // 服务端明确什么都没做，允许换前缀/换写法，用同一个 token 再发一次。
            Self::RouteMissing | Self::InvalidRequest(_) => LedgerPhase::Ready,
        }
    }

    pub fn credential_effect(&self) -> CredentialEffect {
        match self {
            Self::Unauthorized => CredentialEffect::Reauthenticate,
            _ => CredentialEffect::None,
        }
    }

    /// 账本 `outcome` 列的取值，也是日志里给排查用的名字。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Granted { .. } => "granted",
            Self::AlreadyClaimed => "already",
            Self::Rejected(rejection) => rejection.as_str(),
            Self::Forbidden { .. } => "forbidden",
            Self::Unauthorized => "unauthorized",
            Self::RateLimited { .. } => "rate_limited",
            Self::ServerError { .. } => "server",
            Self::Indeterminate => "indeterminate",
            Self::RouteMissing => "route_missing",
            Self::InvalidRequest(invalid) => invalid.as_str(),
            Self::Unknown { .. } => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// 判定
// ---------------------------------------------------------------------------

/// 把一次响应翻成结果。`http_status == 0` 表示**没有拿到响应**（超时/断开）。
///
/// 判定顺序是有讲究的：403 先看文案是不是"天数不足"，**再看**它是不是鉴权问题 ——
/// 反过来会让一个未解锁的档位直接中止整个成长中心（88lin `signin.py:2096` 踩过）。
pub fn classify(command: &Command, http_status: u16, body: &Value) -> Outcome {
    if http_status == 0 {
        return Outcome::Indeterminate;
    }

    let action = command.action();
    let code = code_of(body);
    let message = message_of(body);
    let lowered = message.to_lowercase();

    if http_status == 404 || code == Some(404) {
        return Outcome::RouteMissing;
    }
    if http_status == 401 || code == Some(401) {
        return Outcome::Unauthorized;
    }
    if http_status == 429 || code == Some(429) {
        return Outcome::RateLimited {
            retry_after_secs: retry_after(body),
        };
    }
    if http_status == 403 || code == Some(403) {
        return from_message(action, &lowered).unwrap_or(Outcome::Forbidden { message });
    }
    if http_status >= 500 || matches!(code, Some(500..=599)) {
        return Outcome::ServerError {
            status: http_status,
        };
    }

    let ok_status = (200..300).contains(&http_status);
    let ok_code = matches!(code, None | Some(0) | Some(200));
    if ok_status && ok_code {
        return success(action, http_status, body);
    }

    // 走到这里：要么状态码不是 2xx，要么 2xx 但带非 0 业务码。先认常态拒绝。
    if let Some(known) = from_message(action, &lowered) {
        return known;
    }
    if is_unknown_tier(http_status, code, &lowered) {
        return Outcome::InvalidRequest(Invalid::UnknownTier);
    }
    // 5xx 已经在上面的分支返回，所以这里剩下的 4xx / 3xx 都是"服务端明确拒绝，
    // 但我们不认识这个理由"。按 Unknown 处理：回查服务端，不原地重发，也不编结论。
    Outcome::Unknown {
        status: http_status,
        code,
        message: if message.is_empty() {
            format!("成长中心返回 HTTP {http_status}")
        } else {
            message
        },
    }
}

/// 成功分支。**2xx + `code: 0` 也不等于"一定到账"**：
/// `data.results[]` 是接单/领奖的逐条信封，这三个动作没有"逐条结果"的语义，
/// 出现它说明我们读错了接口 —— 判 `Unknown`，绝不当成功。
fn success(action: Action, http_status: u16, body: &Value) -> Outcome {
    let data = data_of(body);

    if has_results_array(body) {
        return Outcome::Unknown {
            status: http_status,
            code: code_of(body),
            message: "响应里出现逐条结果数组，这三个动作不该有这种形态".to_string(),
        };
    }

    if flag(data, &["already", "already_claimed", "alreadyClaimed"]) {
        return Outcome::AlreadyClaimed;
    }
    if flag(data, &["locked", "not_eligible", "notEligible"]) || flag(data, &["insufficient"]) {
        return Outcome::Rejected(default_rejection(action));
    }

    let grant = parse_grant(data);
    let prize = if action == Action::LotteryDraw {
        parse_prize(data)
    } else {
        None
    };
    if grant.is_empty() && prize.is_none() && !flag(data, &["ok", "success"]) {
        // 没有可以判定的字段。不猜"应该成功了吧"。
        return Outcome::Unknown {
            status: http_status,
            code: code_of(body),
            message: "响应里没有可判定的到账字段".to_string(),
        };
    }
    Outcome::Granted { grant, prize }
}

fn default_rejection(action: Action) -> Rejection {
    match action {
        Action::Redeem => Rejection::TierLocked,
        Action::LotteryDraw => Rejection::NoChance,
        Action::MakeupCard => Rejection::NothingToMakeup,
    }
}

/// 按服务端文案认"常态拒绝"。这里只认**有证据的文案**，其余一律 `None`。
fn from_message(action: Action, lowered: &str) -> Option<Outcome> {
    if lowered.is_empty() {
        return None;
    }
    if lowered.contains("already") || lowered.contains("已领取") || lowered.contains("已兑换")
    {
        return Some(Outcome::AlreadyClaimed);
    }
    if lowered.contains("天数不足")
        || lowered.contains("未解锁")
        || lowered.contains("not reached")
        || lowered.contains("tier not")
        || lowered.contains("locked")
    {
        return Some(Outcome::Rejected(Rejection::TierLocked));
    }
    if lowered.contains("no makeup needed") || lowered.contains("date is not broken") {
        return Some(Outcome::Rejected(Rejection::NothingToMakeup));
    }
    if (lowered.contains("insufficient") || lowered.contains("not enough"))
        && (lowered.contains("chance") || lowered.contains("balance"))
    {
        return Some(Outcome::Rejected(Rejection::NoChance));
    }
    if (lowered.contains("insufficient") || lowered.contains("not enough"))
        && (lowered.contains("card") || lowered.contains("补签") || lowered.contains("makeup"))
    {
        return Some(Outcome::Rejected(Rejection::NothingToMakeup));
    }
    if lowered.contains("活动已结束") || lowered.contains("ended") || lowered.contains("closed")
    {
        return Some(Outcome::Rejected(Rejection::ActivityEnded));
    }
    // 认不出来但明确是"不可用"：抽奖按没次数、补签按不用补，都不算故障。
    if action == Action::LotteryDraw && lowered.contains("no chance") {
        return Some(Outcome::Rejected(Rejection::NoChance));
    }
    None
}

/// `tier` 这个值本身不认识：400 的**参数校验**拒绝。
///
/// 只有它允许"换写法再试一次"，因为服务端在参数阶段就返回了，什么都没兑换。
/// 这正是 88lin 2026-09-11 那条教训的反面用法 —— 不能因为收到 `invalid request`
/// 就反推参数格式，必须是文案里点名了 `tier`。
fn is_unknown_tier(http_status: u16, code: Option<i64>, lowered: &str) -> bool {
    let is_400 = http_status == 400 || code == Some(400);
    is_400
        && lowered.contains("tier")
        && (lowered.contains("unknown")
            || lowered.contains("unsupported")
            || lowered.contains("invalid"))
}

// ---------------------------------------------------------------------------
// 小工具
// ---------------------------------------------------------------------------

fn data_of(body: &Value) -> &Value {
    match body.get("data") {
        Some(Value::Object(_)) => &body["data"],
        _ => body,
    }
}

fn has_results_array(body: &Value) -> bool {
    if body.get("results").map(Value::is_array).unwrap_or(false) {
        return true;
    }
    body.get("data")
        .and_then(|data| data.get("results"))
        .map(Value::is_array)
        .unwrap_or(false)
}

fn code_of(body: &Value) -> Option<i64> {
    let raw = body
        .get("code")
        .or_else(|| body.get("errno"))
        .or_else(|| body.get("statusCode"))?;
    if let Some(n) = raw.as_i64() {
        return Some(n);
    }
    raw.as_str()?.trim().parse().ok()
}

fn message_of(body: &Value) -> String {
    let raw = body
        .get("message")
        .or_else(|| body.get("msg"))
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .map(|text| text.chars().take(200).collect::<String>())
        .unwrap_or_default();
    // 服务端文案会进界面和账本，统一过一遍脱敏（截断 + 抹掉疑似令牌）。
    crate::rpc::sanitize(&raw)
}

fn retry_after(body: &Value) -> Option<i64> {
    body.get("retry_after")
        .or_else(|| body.get("retryAfter"))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_str()?.trim().parse().ok())
        })
}

fn flag(value: &Value, keys: &[&str]) -> bool {
    keys.iter()
        .any(|key| value.get(*key).and_then(Value::as_bool).unwrap_or(false))
}

fn number(value: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        match value.get(*key) {
            Some(Value::Number(number)) => {
                if let Some(parsed) = number.as_i64() {
                    return Some(parsed);
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

fn text(value: &Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.to_string());
            }
        }
    }
    None
}

/// 实发数值。**`*_granted` 优先**：兑换的真实字段名带 `_granted` 后缀，
/// 直接读 `credit` 恒为空，会把兑换所得漏计（88lin `signin.py:1693`，证据 B）。
///
/// `makeup_cards` 刻意不在列表里：它在响应里是**余额**，不是"发了多少张"。
fn parse_grant(data: &Value) -> Grant {
    Grant {
        credits: number(
            data,
            &[
                "credit_granted",
                "credits_granted",
                "credit",
                "credits",
                "reward_credit",
            ],
        ),
        energy: number(
            data,
            &[
                "energy_granted",
                "energies_granted",
                "energy",
                "reward_energy",
            ],
        ),
        cards: number(data, &["cards_granted", "makeup_cards_granted"]),
        chances: number(data, &["chances_granted", "lottery_chances_granted"]),
    }
}

fn parse_prize(data: &Value) -> Option<Prize> {
    let name = text(data, &["prize_name", "prizeName", "prize", "name"]);
    let needs_address = flag(data, &["need_address", "require_address", "needAddress"]);
    if name.is_none() && !needs_address {
        return None;
    }
    Some(Prize {
        name,
        needs_address,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redeem() -> Command {
        Command::Redeem(Tier::Days14)
    }
    fn draw() -> Command {
        Command::LotteryDraw
    }
    fn makeup() -> Command {
        Command::MakeupCard {
            target_date: "2026-11-10".into(),
        }
    }

    // --- 请求形态 -----------------------------------------------------------

    #[test]
    fn suffixes_are_prefix_relative_and_never_carry_the_growth_prefix() {
        for command in [redeem(), draw(), makeup()] {
            let call = call(&command, "tok").expect("call");
            assert!(
                call.suffix.starts_with('/'),
                "{} 必须以 / 开头",
                call.suffix
            );
            assert!(
                !call.suffix.contains("activity"),
                "{} 自己带了成长前缀，会与前缀拼成两段",
                call.suffix
            );
        }
        assert_eq!(call(&redeem(), "t").unwrap().suffix, "/redeem");
        assert_eq!(call(&draw(), "t").unwrap().suffix, "/lottery/draw");
        assert_eq!(call(&makeup(), "t").unwrap().suffix, "/makeup-cards/use");
    }

    #[test]
    fn redeem_and_draw_carry_the_idempotency_key_but_makeup_does_not() {
        let r = call(&redeem(), "tok-1").unwrap();
        assert_eq!(r.body["tier"], "14d");
        assert_eq!(r.body["client_token"], "tok-1");

        let d = call(&draw(), "tok-2").unwrap();
        assert_eq!(d.body["client_token"], "tok-2");

        let m = call(&makeup(), "ignored").unwrap();
        assert_eq!(m.body["target_date"], "2026-11-10");
        assert!(m.body.get("client_token").is_none(), "补签不带幂等键");
    }

    #[test]
    fn a_malformed_date_is_refused_before_any_request() {
        for bad in ["2026-1-1", "20261110", "2026/11/10", "", "2026-13-01"] {
            let command = Command::MakeupCard {
                target_date: bad.into(),
            };
            assert_eq!(
                call(&command, "tok"),
                Err(Refusal::InvalidTargetDate(bad.into())),
                "{bad} 不该被发出去"
            );
        }
    }

    #[test]
    fn client_tokens_are_unique_and_follow_the_official_shape() {
        let a = client_token();
        let b = client_token();
        assert!(a.starts_with("u-"), "官方前端是 u-<uuid>");
        assert_ne!(a, b);
        assert_eq!(a.len(), 38, "u- 加 36 位 uuid");
    }

    // --- 档位 ---------------------------------------------------------------

    #[test]
    fn tiers_round_trip_and_reject_days_and_names() {
        for tier in Tier::ALL {
            assert_eq!(Tier::parse(tier.as_str()), Some(tier));
        }
        for wrong in ["30d", "14", "starter", "advanced", "7"] {
            assert_eq!(Tier::parse(wrong), None, "{wrong} 不是档位标识");
        }
        assert_eq!(Tier::Days14.days(), 14, "天数只作兜底写法");
    }

    #[test]
    fn every_tier_knows_both_status_key_shapes() {
        assert_eq!(
            Tier::Days7.status_keys(),
            ["tier_7d_status", "starter_status"]
        );
        assert_eq!(
            Tier::Days28.status_keys(),
            ["tier_28d_status", "legendary_status"]
        );
    }

    // --- 到账 ---------------------------------------------------------------

    #[test]
    fn a_redeem_reports_the_granted_fields_not_the_plain_ones() {
        let body = json!({
            "code": 0,
            "data": {
                "credit_granted": 50,
                "energy_granted": 3,
                "cards_granted": 1,
                "chances_granted": 1
            }
        });
        let outcome = classify(&redeem(), 200, &body);
        assert_eq!(
            outcome,
            Outcome::Granted {
                grant: Grant {
                    credits: Some(50),
                    energy: Some(3),
                    cards: Some(1),
                    chances: Some(1),
                },
                prize: None,
            }
        );
        assert!(outcome.terminal());
        assert!(!outcome.may_resend(), "到账了就别再发");
    }

    #[test]
    fn a_draw_reports_the_prize_and_the_address_requirement() {
        let body = json!({
            "code": 0,
            "data": { "prize_name": "冰箱贴", "need_address": true }
        });
        match classify(&draw(), 200, &body) {
            Outcome::Granted { prize, .. } => {
                let prize = prize.expect("prize");
                assert_eq!(prize.name.as_deref(), Some("冰箱贴"));
                assert!(prize.needs_address, "实物奖要提醒填地址");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_success_receipt_without_any_grant_field_is_unknown_not_success() {
        // code 0 + 空 data：可能是成功，但没证据。回查服务端比猜一个"成功"便宜。
        let outcome = classify(&makeup(), 200, &json!({"code": 0, "data": {}}));
        assert!(matches!(outcome, Outcome::Unknown { .. }));
        assert!(!outcome.granted());
        assert!(outcome.requires_recheck());
    }

    #[test]
    fn a_per_task_results_envelope_is_unknown_for_these_actions() {
        // 这是 accept / claim 的信封。出现在这里说明读错了接口。
        let body = json!({"code": 0, "data": {"results": [{"status": "ok"}]}});
        assert!(matches!(
            classify(&redeem(), 200, &body),
            Outcome::Unknown { .. }
        ));
    }

    // --- 常态拒绝 -----------------------------------------------------------

    #[test]
    fn an_unlocked_tier_is_a_business_rejection_not_an_auth_failure() {
        // 88lin 实测：403 + 「连登天数不足」。它是常态，不能让账号变成"待重新授权"。
        let outcome = classify(
            &redeem(),
            403,
            &json!({"code": 403, "msg": "连续登录天数不足"}),
        );
        assert_eq!(outcome, Outcome::Rejected(Rejection::TierLocked));
        assert_eq!(outcome.credential_effect(), CredentialEffect::None);
        assert!(outcome.terminal());
    }

    #[test]
    fn a_403_with_an_unrecognised_message_is_forbidden_and_still_does_not_touch_credentials() {
        let outcome = classify(&redeem(), 403, &json!({"code": 403, "msg": "risk control"}));
        assert!(matches!(outcome, Outcome::Forbidden { .. }));
        assert_eq!(outcome.credential_effect(), CredentialEffect::None);
        assert_ne!(outcome.label(), "unauthorized");
        // 认不出来的 403 不能当"已完成"：关掉之后下一轮又会去撞风控。
        assert!(!outcome.terminal(), "未知 403 不许关闭账本");
        assert!(outcome.requires_attention());
        assert_eq!(outcome.ledger_phase(), LedgerPhase::Frozen);
        assert!(!outcome.ledger_phase().closes());
    }

    #[test]
    fn an_unknown_403_is_frozen_rather_than_retried() {
        for message in ["risk control", "restricted", "forbidden by policy"] {
            let outcome = classify(&redeem(), 403, &json!({"code": 403, "msg": message}));
            assert_eq!(outcome.ledger_phase().as_str(), "frozen", "{message}");
            assert!(!outcome.may_resend(), "{message} 不许自动重发");
            assert!(
                !outcome.requires_recheck(),
                "{message} 不是'再查一次'能解决的"
            );
        }
    }

    #[test]
    fn only_401_may_change_the_account_credential_state() {
        assert_eq!(
            classify(&redeem(), 401, &json!({"code": 401})).credential_effect(),
            CredentialEffect::Reauthenticate
        );
        for (status, body) in [
            (403, json!({"code": 403, "msg": "连续登录天数不足"})),
            (
                400,
                json!({"code": 400, "msg": "insufficient lottery chance balance"}),
            ),
            (429, json!({"code": 429})),
            (500, json!({"code": 500})),
            (200, json!({"code": 0, "data": {"credit": 1}})),
        ] {
            assert_eq!(
                classify(&redeem(), status, &body).credential_effect(),
                CredentialEffect::None,
                "{status} 不许动凭据"
            );
        }
    }

    #[test]
    fn no_lottery_chance_is_a_rejection_not_a_failure() {
        let body = json!({"code": 400, "msg": "insufficient lottery chance balance"});
        let outcome = classify(&draw(), 400, &body);
        assert_eq!(outcome, Outcome::Rejected(Rejection::NoChance));
        assert!(outcome.terminal());
        assert!(!outcome.requires_recheck());
    }

    #[test]
    fn a_date_that_was_never_broken_needs_no_makeup() {
        let body = json!({"code": 400, "msg": "date is not broken, no makeup needed"});
        let outcome = classify(&makeup(), 400, &body);
        assert_eq!(outcome, Outcome::Rejected(Rejection::NothingToMakeup));
        assert!(outcome.terminal());
    }

    #[test]
    fn an_activity_that_ended_is_reported_as_ended_not_as_eligibility() {
        let body = json!({"code": 10086, "msg": "活动已结束"});
        assert_eq!(
            classify(&redeem(), 200, &body),
            Outcome::Rejected(Rejection::ActivityEnded)
        );
    }

    #[test]
    fn an_unknown_tier_is_a_parameter_refusal_that_may_be_resent() {
        // 参数校验阶段就拒了：服务端没兑换任何东西，可以退回天数再试一次。
        let body = json!({"code": 400, "msg": "unknown tier: starter"});
        let outcome = classify(&redeem(), 400, &body);
        assert_eq!(outcome, Outcome::InvalidRequest(Invalid::UnknownTier));
        assert!(outcome.may_resend());
        assert!(!outcome.terminal());
    }

    #[test]
    fn a_bare_invalid_request_is_not_a_tier_problem() {
        // 「业务拒绝的文案不能反推参数格式正确」：不点名 tier 就不许重试。
        let body = json!({"code": 400, "msg": "invalid request"});
        let outcome = classify(&redeem(), 400, &body);
        assert!(!outcome.may_resend(), "{outcome:?}");
    }

    // --- 未定 / 可重发 ------------------------------------------------------

    #[test]
    fn a_timeout_is_indeterminate_and_must_be_rechecked() {
        let outcome = classify(&draw(), 0, &json!({}));
        assert_eq!(outcome, Outcome::Indeterminate);
        assert!(outcome.requires_recheck());
        assert!(!outcome.may_resend(), "超时可能已经抽掉了，不许重发");
        assert!(!outcome.terminal());
    }

    #[test]
    fn a_5xx_may_have_been_applied_so_it_is_not_resendable() {
        let outcome = classify(&draw(), 502, &json!({}));
        assert_eq!(outcome, Outcome::ServerError { status: 502 });
        assert!(outcome.requires_recheck());
        assert!(!outcome.may_resend());
    }

    #[test]
    fn a_route_miss_is_the_only_status_level_reason_to_try_another_prefix() {
        let outcome = classify(&redeem(), 404, &json!({"code": 404}));
        assert_eq!(outcome, Outcome::RouteMissing);
        assert!(outcome.may_resend(), "404 表示那条路径什么都没发生");
        assert!(!outcome.terminal());
    }

    #[test]
    fn rate_limiting_is_separated_from_authorization_and_carries_the_hint() {
        let outcome = classify(&draw(), 429, &json!({"retry_after": 30}));
        assert_eq!(
            outcome,
            Outcome::RateLimited {
                retry_after_secs: Some(30)
            }
        );
        assert!(outcome.requires_recheck());
        assert_eq!(outcome.credential_effect(), CredentialEffect::None);
    }

    #[test]
    fn an_unrecognised_success_body_is_unknown_and_never_success() {
        let outcome = classify(&redeem(), 200, &json!({"data": {"something": 1}}));
        assert!(matches!(outcome, Outcome::Unknown { .. }));
        assert!(!outcome.granted());
        assert!(!outcome.terminal());
        assert!(outcome.requires_recheck());
    }

    #[test]
    fn an_unknown_business_code_is_unknown_not_not_eligible() {
        // 上一版把"所有非 0 业务码"都说成 NotEligible。那会把活动结束、
        // 账号被限制、参数错统统说成"没资格"，界面据此禁用按钮还给人错误结论。
        let outcome = classify(&redeem(), 200, &json!({"code": 12345, "msg": ""}));
        assert!(matches!(
            outcome,
            Outcome::Unknown {
                code: Some(12345),
                ..
            }
        ));
        assert_eq!(outcome.label(), "unknown");
    }

    #[test]
    fn an_idempotent_receipt_is_neither_granted_nor_a_failure() {
        let outcome = classify(&redeem(), 200, &json!({"data": {"already_claimed": true}}));
        assert_eq!(outcome, Outcome::AlreadyClaimed);
        assert!(outcome.terminal());
        assert!(!outcome.granted());
    }

    #[test]
    fn insufficient_inside_a_success_body_maps_to_the_action_specific_rejection() {
        assert_eq!(
            classify(&draw(), 200, &json!({"data": {"insufficient": true}})),
            Outcome::Rejected(Rejection::NoChance)
        );
        assert_eq!(
            classify(&makeup(), 200, &json!({"data": {"insufficient": true}})),
            Outcome::Rejected(Rejection::NothingToMakeup)
        );
    }

    // --- 账本阶段 -----------------------------------------------------------

    /// 判定 → 账本阶段的完整映射。任何一类都不允许"落不进去"。
    #[test]
    fn every_outcome_maps_to_exactly_one_ledger_phase() {
        let cases = [
            (
                classify(&redeem(), 200, &json!({"data": {"credit": 1}})),
                "confirmed",
            ),
            (
                classify(&redeem(), 200, &json!({"data": {"already": true}})),
                "rejected",
            ),
            (
                classify(&redeem(), 403, &json!({"msg": "连续登录天数不足"})),
                "rejected",
            ),
            (classify(&redeem(), 401, &json!({})), "rejected"),
            (
                classify(&redeem(), 403, &json!({"msg": "risk control"})),
                "frozen",
            ),
            (classify(&draw(), 429, &json!({})), "unknown"),
            (classify(&draw(), 502, &json!({})), "unknown"),
            (classify(&draw(), 0, &json!({})), "unknown"),
            (
                classify(&draw(), 200, &json!({"data": {"?": 1}})),
                "unknown",
            ),
            (classify(&redeem(), 404, &json!({})), "ready"),
            (
                classify(&redeem(), 400, &json!({"msg": "unknown tier"})),
                "ready",
            ),
        ];
        for (outcome, expected) in cases {
            assert_eq!(outcome.ledger_phase().as_str(), expected, "{outcome:?}");
        }
    }

    /// 只有明确终局才解除占用。这条是 P0-1 / P0-2 的语义锚点：
    /// 超时、5xx、未知业务码、未知 403 都必须继续占着目标。
    #[test]
    fn only_confirmed_and_rejected_free_the_target() {
        let closes = [
            classify(&redeem(), 200, &json!({"data": {"credit": 1}})),
            classify(&redeem(), 200, &json!({"data": {"already": true}})),
            classify(
                &draw(),
                400,
                &json!({"msg": "insufficient lottery chance balance"}),
            ),
            classify(&redeem(), 401, &json!({})),
        ];
        for outcome in closes {
            assert!(outcome.ledger_phase().closes(), "{outcome:?} 应该能关闭");
            assert!(outcome.terminal(), "{outcome:?} 应该是终态");
        }

        let must_stay_open = [
            classify(&draw(), 0, &json!({})),
            classify(&draw(), 503, &json!({})),
            classify(&draw(), 429, &json!({})),
            classify(&redeem(), 200, &json!({"code": 9999})),
            classify(&redeem(), 403, &json!({"msg": "risk control"})),
            classify(&redeem(), 404, &json!({})),
            classify(&redeem(), 400, &json!({"msg": "unknown tier"})),
        ];
        for outcome in must_stay_open {
            assert!(!outcome.ledger_phase().closes(), "{outcome:?} 不许关闭");
            assert!(!outcome.terminal(), "{outcome:?} 不许当终态");
        }
    }
}
