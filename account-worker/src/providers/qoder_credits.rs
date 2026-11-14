//! Qoder 每日 Credits 活动的协议层：**只做请求与解析，不写库**。
//!
//! 接口形态来自社区参考实现 `88lin/workbuddy-auto-signin` 的 `qoder_signin.py`
//! （MIT），并在本机对域名做了可达性确认；**AICC 侧还没有过一份带真实凭据的
//! 成功响应**，所以：
//!
//! - 只有 **Qoder CN** 有适配器。International 的对应能力**未验证**，
//!   `capability()` 返回 `unsupported` 且**一个请求都不发**——把 CN 的路径
//!   换个域名就当成国际版能用，是最容易犯也最难发现的错。
//! - 任何"今天能领"的判定都以服务端返回的 `startAt` / `endAt` 为准，
//!   不在本地推算 10:00 刷新点。参考实现的文档说 UTC+8 每天 10:00 刷新，
//!   我们只把它当说明文字，不当事实来源。
//!
//! ## 端点（CN）
//!
//! ```text
//! GET  {openapi}/sash/api/v1/me/campaigns
//! POST {openapi}/sash/api/v1/me/campaigns/{campaignId}/claim   （服务端幂等）
//! ```
//!
//! 缺 `Cosy-ClientType: 10` 时服务端返回**空活动列表**（不是错误）——这是最阴的
//! 一种失败：看起来"今天没有活动"。所以这个头是硬编码的，不放进可选项。

use std::time::Duration;

use serde_json::{json, Value};

use crate::providers::qoder_edition::QoderEdition;

/// 单次请求上限。领取一轮最多三次请求（查 → 领 → 回查），
/// 单次 10 秒能保证整轮落在 Swift 侧 45 秒的 RPC 超时之内。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// 只对**可安全重放**的请求（GET campaigns）做退避重试。
/// 领取是 POST，超时后必须先回查，不能直接重发。
const RETRY_DELAYS_MS: &[u64] = &[1_500, 4_000];

/// 缺了它服务端会返回空列表，看起来像"今天没有活动"。
pub const CLIENT_TYPE: &str = "10";
pub const USER_AGENT: &str = "Qoder/claim";

pub const CAMPAIGNS_PATH: &str = "/sash/api/v1/me/campaigns";

/// 该版本是否实现了 Credits 领取。
///
/// `Some(base)` = 有适配器（基址写死官方域名，绝不用账号里的 domain 拼）；
/// `None` = 没有证据，界面不该显示领取按钮。International 目前是 `None`，
/// 原因见 `docs/QODER_CREDITS.md` §3。
pub fn capability(edition: QoderEdition) -> Option<&'static str> {
    match edition {
        QoderEdition::Cn => Some("https://openapi.qoder.com.cn"),
        QoderEdition::Global => None,
    }
}

pub fn unsupported_reason(edition: QoderEdition) -> &'static str {
    match edition {
        QoderEdition::Cn => "Qoder CN 的 Credits 接口尚未用真实凭据验证通过，结果只代表本地解析正确",
        QoderEdition::Global => {
            "Qoder International 没有已证实的 Credits 活动接口；不把国内版接口换域名套用，本版不发任何请求"
        }
    }
}

pub fn campaigns_url(edition: QoderEdition) -> Option<String> {
    capability(edition).map(|base| format!("{base}{CAMPAIGNS_PATH}"))
}

pub fn claim_url(edition: QoderEdition, campaign_id: &str) -> Option<String> {
    capability(edition)
        .map(|base| format!("{base}{CAMPAIGNS_PATH}/{}/claim", urlencode(campaign_id)))
}

/// campaign id 是服务端给的不透明串。放路径里必须编码，
/// 否则一个带 `/` 或 `?` 的 id 会改变请求目标（这类 id 一旦被构造就能改路由）。
fn urlencode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// 一个领取型活动。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Campaign {
    pub id: String,
    pub key: String,
    pub action_type: String,
    pub claim_status: String,
    pub start_at: Option<i64>,
    pub end_at: Option<i64>,
    /// `benefit.amount`。缺失就是 `None`**不是 0**。
    pub credits: Option<i64>,
    pub validity_days: Option<i64>,
}

impl Campaign {
    /// 是否落在服务端给的窗口内。缺字段按 active 处理（宽松，与参考实现一致）。
    pub fn is_active(&self, now_secs: i64) -> bool {
        if let Some(start) = self.start_at {
            if now_secs < start {
                return false;
            }
        }
        if let Some(end) = self.end_at {
            if now_secs >= end {
                return false;
            }
        }
        true
    }

    pub fn is_claimable(&self, now_secs: i64) -> bool {
        self.action_type == "CLAIM_BENEFIT"
            && self.is_active(now_secs)
            && self.claim_status == "CLAIMABLE"
    }

    pub fn is_claimed(&self, now_secs: i64) -> bool {
        self.action_type == "CLAIM_BENEFIT"
            && self.is_active(now_secs)
            && self.claim_status == "CLAIMED"
    }

    pub fn to_json(&self) -> Value {
        json!({
            "campaignId": self.id,
            "campaignKey": self.key,
            "actionType": self.action_type,
            "claimStatus": self.claim_status,
            "startAt": self.start_at,
            "endAt": self.end_at,
            "credits": self.credits,
            "validityDays": self.validity_days,
        })
    }
}

/// 从 `campaigns` 响应里取出活动数组。**任何结构不认识都返回空数组**，
/// 由调用方把 `unparsable` 的结论单独报出去，而不是当成"没有活动"。
pub fn parse_campaigns(body: &Value) -> Vec<Campaign> {
    let source = body
        .get("data")
        .filter(|data| data.get("campaigns").is_some())
        .unwrap_or(body);
    let Some(items) = source.get("campaigns").and_then(Value::as_array) else {
        return Vec::new();
    };
    items.iter().filter_map(parse_campaign).collect()
}

fn parse_campaign(value: &Value) -> Option<Campaign> {
    let id = text(value, &["campaignId", "campaign_id", "id"])?;
    let key = text(value, &["campaignKey", "campaign_key"]).unwrap_or_else(|| id.clone());
    let action_type = text(value, &["actionType", "action_type"]).unwrap_or_default();
    let claim_status = text(value, &["claimStatus", "claim_status"]).unwrap_or_default();
    let benefit = value.get("benefit");
    let credits = benefit
        .and_then(|b| number(b, &["amount"]))
        .or_else(|| number(value, &["credits", "amount"]));
    let validity_days = benefit
        .and_then(|b| b.get("validity"))
        .and_then(|v| number(v, &["days"]));
    Some(Campaign {
        id,
        key,
        action_type,
        claim_status,
        start_at: number(value, &["startAt", "start_at"]),
        end_at: number(value, &["endAt", "end_at"]),
        credits,
        validity_days,
    })
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

fn number(value: &Value, keys: &[&str]) -> Option<i64> {
    for key in keys {
        if let Some(number) = value.get(*key).and_then(Value::as_i64) {
            return Some(number);
        }
        // 有些网关把数字写成字符串。
        if let Some(text) = value.get(*key).and_then(Value::as_str) {
            if let Ok(parsed) = text.trim().parse::<i64>() {
                return Some(parsed);
            }
        }
    }
    None
}

/// 活动快照 + 分类结果。`now` 由调用方传入，便于测试固定时间。
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub campaigns: Vec<Campaign>,
    pub todo: Vec<Campaign>,
    pub claimed: Vec<Campaign>,
    /// 落在窗口内、但状态既不是 CLAIMABLE 也不是 CLAIMED 的活动。
    pub pending: Vec<Campaign>,
}

pub fn classify(campaigns: Vec<Campaign>, now_secs: i64) -> Snapshot {
    let mut todo = Vec::new();
    let mut claimed = Vec::new();
    let mut pending = Vec::new();
    for campaign in &campaigns {
        if campaign.action_type != "CLAIM_BENEFIT" {
            continue;
        }
        if campaign.is_claimable(now_secs) {
            todo.push(campaign.clone());
        } else if campaign.is_claimed(now_secs) {
            claimed.push(campaign.clone());
        } else if campaign.is_active(now_secs) {
            pending.push(campaign.clone());
        }
    }
    Snapshot {
        campaigns,
        todo,
        claimed,
        pending,
    }
}

/// 领取接口的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    /// 服务端说"这次是新领到的"还是"之前已经领过（幂等重放）"。
    pub replayed: bool,
    pub credits: Option<i64>,
    /// 服务端原样状态串，用于日志与排查（不含令牌）。
    pub status: String,
}

/// 解析领取响应。**只有 `status == "CLAIMED"` 才算成功**：
/// 200 + 别的状态（例如 `PENDING`）不能报告成已领取。
pub fn parse_claim(body: &Value) -> Result<Claimed, CreditsError> {
    let data = body
        .get("data")
        .filter(|data| data.is_object())
        .unwrap_or(body);
    let status = data
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if status != "CLAIMED" {
        return Err(CreditsError::Unparsable(if status.is_empty() {
            "领取响应里没有 status".to_string()
        } else {
            format!("领取响应 status={status}")
        }));
    }
    let credits = data
        .get("benefit")
        .and_then(|b| number(b, &["amount"]))
        .or_else(|| number(data, &["credits", "amount"]));
    Ok(Claimed {
        replayed: data
            .get("replayed")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        credits,
        status,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreditsError {
    /// 该版本没有适配器（International）。
    Unsupported(String),
    /// 401：凭据不再被接受。
    Unauthorized,
    /// 403：有凭据但没权限。
    Forbidden,
    RateLimited,
    Server(u16),
    Network(String),
    /// 200 但结构不认识。
    Unparsable(String),
}

impl CreditsError {
    /// 与 `qoder_credit_runs.outcome` 对齐。
    pub fn outcome(&self) -> &'static str {
        match self {
            CreditsError::Unsupported(_) => "unsupported",
            CreditsError::Unauthorized => "rejected",
            CreditsError::Forbidden => "forbidden",
            CreditsError::RateLimited => "rate_limited",
            CreditsError::Server(_) => "server_error",
            CreditsError::Network(_) => "network",
            CreditsError::Unparsable(_) => "failed",
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            CreditsError::Unsupported(_) => "unsupported",
            CreditsError::Unauthorized => "unauthorized",
            CreditsError::Forbidden => "forbidden",
            CreditsError::RateLimited => "rate_limited",
            CreditsError::Server(_) => "server",
            CreditsError::Network(_) => "network",
            CreditsError::Unparsable(_) => "unparsable",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            CreditsError::Unsupported(reason) => reason.clone(),
            CreditsError::Unauthorized => "凭据被服务端拒绝（401），需要重新授权".to_string(),
            CreditsError::Forbidden => "凭据没有领取权限（403）".to_string(),
            CreditsError::RateLimited => "被限流（429），本轮不再重试".to_string(),
            CreditsError::Server(code) => format!("服务端错误（HTTP {code}）"),
            CreditsError::Network(message) => crate::rpc::sanitize(message),
            CreditsError::Unparsable(message) => {
                format!("响应结构不认识：{}", crate::rpc::sanitize(message))
            }
        }
    }

    pub fn http_status(&self) -> Option<i64> {
        match self {
            CreditsError::Unauthorized => Some(401),
            CreditsError::Forbidden => Some(403),
            CreditsError::RateLimited => Some(429),
            CreditsError::Server(code) => Some(*code as i64),
            _ => None,
        }
    }

    /// 只在"重发一次也不会更糟"时重试。领取（POST）不走这条路。
    fn retryable(&self) -> bool {
        matches!(
            self,
            CreditsError::Network(_) | CreditsError::RateLimited | CreditsError::Server(_)
        )
    }
}

fn headers(token: &str) -> reqwest::header::HeaderMap {
    use reqwest::header::{HeaderMap, HeaderValue};
    let mut map = HeaderMap::new();
    map.insert("Accept", HeaderValue::from_static("application/json"));
    map.insert("User-Agent", HeaderValue::from_static(USER_AGENT));
    // 缺这个头服务端返回空活动列表——不是报错，是静默的空。
    map.insert("Cosy-ClientType", HeaderValue::from_static(CLIENT_TYPE));
    if let Ok(value) = HeaderValue::from_str(&format!("Bearer {token}")) {
        map.insert("Authorization", value);
    }
    map
}

fn client() -> Result<reqwest::Client, CreditsError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| CreditsError::Network(e.to_string()))
}

fn classify_response(status: u16, body: &Value) -> Result<Value, CreditsError> {
    match status {
        200..=299 => Ok(body.clone()),
        401 => Err(CreditsError::Unauthorized),
        403 => Err(CreditsError::Forbidden),
        429 => Err(CreditsError::RateLimited),
        0 => Err(CreditsError::Network("请求没有拿到响应".to_string())),
        other => {
            let message = body
                .get("message")
                .or_else(|| body.get("msg"))
                .and_then(Value::as_str)
                .unwrap_or("");
            if message.is_empty() {
                Err(CreditsError::Server(other))
            } else {
                Err(CreditsError::Unparsable(format!("HTTP {other}：{message}")))
            }
        }
    }
}

/// 读活动列表。GET 是可重放的，所以 429/5xx/网络错误按 `RETRY_DELAYS_MS` 退避。
pub async fn fetch_campaigns(edition: QoderEdition, token: &str) -> Result<Value, CreditsError> {
    let url = campaigns_url(edition)
        .ok_or_else(|| CreditsError::Unsupported(unsupported_reason(edition).to_string()))?;
    let client = client()?;
    let mut attempt = 0usize;
    loop {
        let outcome = client.get(&url).headers(headers(token)).send().await;
        let error = match outcome {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body: Value = resp.json().await.unwrap_or(Value::Null);
                match classify_response(status, &body) {
                    Ok(value) => return Ok(value),
                    Err(err) => err,
                }
            }
            Err(err) => CreditsError::Network(err.to_string()),
        };
        if !error.retryable() || attempt >= RETRY_DELAYS_MS.len() {
            return Err(error);
        }
        tokio::time::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt])).await;
        attempt += 1;
    }
}

/// 领取指定活动。
///
/// **不重试**：POST 超时后直接返回 `Network`，由调用方回查活动列表确认结果。
/// 重发是安全的（服务端幂等），但会让"到底领了几次"无法回答，也会放大限流。
pub async fn claim(
    edition: QoderEdition,
    token: &str,
    campaign_id: &str,
) -> Result<Claimed, CreditsError> {
    let url = claim_url(edition, campaign_id)
        .ok_or_else(|| CreditsError::Unsupported(unsupported_reason(edition).to_string()))?;
    let client = client()?;
    // 空体 POST：参考实现发的就是空字节串，不是 `{}`。
    let resp = client
        .post(&url)
        .headers(headers(token))
        .body("")
        .send()
        .await
        .map_err(|e| CreditsError::Network(e.to_string()))?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let value = classify_response(status, &body)?;
    parse_claim(&value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn campaign(id: &str, status: &str) -> Campaign {
        Campaign {
            id: id.to_string(),
            key: format!("key-{id}"),
            action_type: "CLAIM_BENEFIT".to_string(),
            claim_status: status.to_string(),
            start_at: Some(1_700_000_000),
            end_at: Some(1_900_000_000),
            credits: Some(100),
            validity_days: Some(7),
        }
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn only_cn_has_an_adapter() {
        assert!(capability(QoderEdition::Cn).is_some());
        // 国际版没有证据：不换域名套用，也不发请求。
        assert!(capability(QoderEdition::Global).is_none());
        assert!(campaigns_url(QoderEdition::Global).is_none());
        assert!(unsupported_reason(QoderEdition::Global).contains("不发任何请求"));
    }

    #[test]
    fn urls_are_pinned_to_the_official_host() {
        assert_eq!(
            campaigns_url(QoderEdition::Cn).unwrap(),
            "https://openapi.qoder.com.cn/sash/api/v1/me/campaigns"
        );
        assert_eq!(
            claim_url(QoderEdition::Cn, "c-1").unwrap(),
            "https://openapi.qoder.com.cn/sash/api/v1/me/campaigns/c-1/claim"
        );
    }

    #[test]
    fn campaign_id_cannot_reshape_the_url() {
        // 一个带斜杠/问号的 id 不能变成另一条路由。
        let url = claim_url(QoderEdition::Cn, "a/../b?x=1").unwrap();
        assert!(url.ends_with("/a%2F..%2Fb%3Fx%3D1/claim"), "{url}");
        assert!(!url.contains("?x=1"));
    }

    #[test]
    fn parses_campaigns_and_keeps_missing_credits_null() {
        let body = json!({"campaigns": [
            {"campaignId": "c1", "campaignKey": "daily", "actionType": "CLAIM_BENEFIT",
             "claimStatus": "CLAIMABLE", "startAt": 1_700_000_000, "endAt": 1_900_000_000,
             "benefit": {"amount": 100, "validity": {"mode": "RELATIVE_DAYS", "days": 7}}},
            {"campaignId": "c2", "actionType": "CLAIM_BENEFIT", "claimStatus": "CLAIMABLE"}
        ]});
        let parsed = parse_campaigns(&body);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].credits, Some(100));
        assert_eq!(parsed[0].validity_days, Some(7));
        assert_eq!(parsed[0].key, "daily");
        // 没有 benefit → None，不是 0。
        assert!(parsed[1].credits.is_none());
        // 没有 endAt → 不拦。
        assert!(parsed[1].is_active(NOW));
    }

    #[test]
    fn classify_splits_todo_claimed_and_pending() {
        let snapshot = classify(
            vec![
                campaign("c1", "CLAIMABLE"),
                campaign("c2", "CLAIMED"),
                campaign("c3", "PENDING"),
                Campaign {
                    id: "c4".into(),
                    key: "watch".into(),
                    action_type: "WATCH_VIDEO".into(),
                    claim_status: "CLAIMABLE".into(),
                    start_at: None,
                    end_at: None,
                    credits: Some(5),
                    validity_days: None,
                },
            ],
            NOW,
        );
        assert_eq!(snapshot.todo.len(), 1);
        assert_eq!(snapshot.claimed.len(), 1);
        assert_eq!(snapshot.pending.len(), 1);
        // 非 CLAIM_BENEFIT 的活动不参与领取，但仍留在全量里供排查。
        assert_eq!(snapshot.campaigns.len(), 4);
    }

    #[test]
    fn window_boundaries_are_respected() {
        let mut c = campaign("c1", "CLAIMABLE");
        c.start_at = Some(NOW + 1);
        assert!(!c.is_claimable(NOW), "startAt 之前不该可领");
        c.start_at = Some(NOW);
        assert!(c.is_claimable(NOW), "startAt 当秒即开始");

        let mut c = campaign("c2", "CLAIMABLE");
        c.end_at = Some(NOW);
        assert!(!c.is_claimable(NOW), "endAt 到点即结束");

        // 缺时间戳 → 宽松放行。
        let mut c = campaign("c3", "CLAIMABLE");
        c.start_at = None;
        c.end_at = None;
        assert!(c.is_claimable(NOW));
    }

    #[test]
    fn claim_requires_the_server_to_say_claimed() {
        let ok = json!({"data": {"status": "CLAIMED", "replayed": false,
                                  "benefit": {"amount": 100}}});
        let claimed = parse_claim(&ok).unwrap();
        assert_eq!(claimed.credits, Some(100));
        assert!(!claimed.replayed);

        let replay = json!({"data": {"status": "CLAIMED", "replayed": true}});
        assert!(parse_claim(&replay).unwrap().replayed);
        // 奖励缺失时不能补 0。
        assert!(parse_claim(&replay).unwrap().credits.is_none());

        // 200 + 别的状态 => 不是成功。
        assert!(parse_claim(&json!({"data": {"status": "PENDING"}})).is_err());
        assert!(parse_claim(&json!({"data": {}})).is_err());
        assert!(parse_claim(&Value::Null).is_err());
    }

    #[test]
    fn http_statuses_map_to_the_right_outcome() {
        assert_eq!(
            classify_response(401, &Value::Null).unwrap_err().outcome(),
            "rejected"
        );
        assert_eq!(
            classify_response(403, &Value::Null).unwrap_err().outcome(),
            "forbidden"
        );
        assert_eq!(
            classify_response(429, &Value::Null).unwrap_err().outcome(),
            "rate_limited"
        );
        assert_eq!(
            classify_response(502, &Value::Null).unwrap_err().outcome(),
            "server_error"
        );
        assert_eq!(
            classify_response(0, &Value::Null).unwrap_err().outcome(),
            "network"
        );
        assert!(classify_response(200, &json!({})).is_ok());
    }

    #[test]
    fn only_transient_failures_are_retried() {
        assert!(CreditsError::RateLimited.retryable());
        assert!(CreditsError::Server(503).retryable());
        assert!(CreditsError::Network("timeout".into()).retryable());
        // 授权问题重试也不会变好。
        assert!(!CreditsError::Unauthorized.retryable());
        assert!(!CreditsError::Forbidden.retryable());
        assert!(!CreditsError::Unsupported("x".into()).retryable());
        assert!(!CreditsError::Unparsable("x".into()).retryable());
    }

    #[test]
    fn error_details_never_contain_the_token() {
        // 所有 detail 都由固定文案或服务端 message 组成，令牌不参与。
        let detail = CreditsError::Unauthorized.detail();
        assert!(!detail.to_lowercase().contains("bearer"));
        assert!(!detail.contains("qp_"));
    }
}
