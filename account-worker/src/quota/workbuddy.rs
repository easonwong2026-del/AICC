//! WorkBuddy CN / AI 的只读额度适配器。
//!
//! 两个站点同一套协议，只是基址不同：
//!
//! | 档位 | 基址 | 证据 |
//! | --- | --- | --- |
//! | 国内版 | `https://www.codebuddy.cn` | 真实账号实测 2026-11-14 |
//! | 国际版 | `https://www.workbuddy.ai` | 真实账号实测 2026-11-14 |
//!
//! 两个端点（见 `docs/WORKBUDDY_QUOTA_API.md`）：
//!
//! - `POST /billing/meter/get-user-resource-summary`，body `{}`
//! - `POST /billing/meter/get-user-request-usage`，body `{startTime,endTime,pageNum,pageSize}`
//!
//! 三个不可妥协的判定：
//!
//! 1. **`/v2` 前缀不存在**：`/v2/billing/meter/...` 实测 404，只有无前缀的可达。
//! 2. **数值是字符串且可能是小数**（`"4597.75000075"`），`CapacityUnit` 会写
//!    `credit` 或 `credits` 两种。解析失败一律 `None`，**不当 0**——0 是"用完了"。
//! 3. **用量明细里有 `input`（用户 prompt 原文）**。本模块只取 `credit` 与
//!    `model`/`client`/`requestTime`/`requestId`，原文永不读出这个函数。

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::rpc::sanitize;

/// 单个请求上限。额度是只读查询，15 秒拿不到就该给用户一个明确结论。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// 明细一次取这么多行。超出的部分标记 `truncated`，不悄悄少算。
const USAGE_PAGE_SIZE: i64 = 3000;

/// 网关要求的客户端标识，与 `growth/api.rs` 保持同一个值。
const USER_AGENT: &str = "WorkBuddy";

pub const SUMMARY_PATH: &str = "/billing/meter/get-user-resource-summary";
pub const USAGE_PATH: &str = "/billing/meter/get-user-request-usage";

/// 档位 → 基址。**只认这两个官方站点**，账号上的 `auth_domain` 不参与决定，
/// 否则一个被改坏/被导入的 domain 就能把令牌发到第三方。
pub fn base_for(provider: &str) -> Option<&'static str> {
    match provider {
        "workbuddy" => Some("https://www.codebuddy.cn"),
        "workbuddy_ai" => Some("https://www.workbuddy.ai"),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quota {
    pub unit: String,
    pub total: Option<f64>,
    pub used: Option<f64>,
    pub remaining: Option<f64>,
    pub plan_name: Option<String>,
    pub is_paid: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Usage {
    pub window_days: i64,
    pub requests: i64,
    pub credits: f64,
    pub total_reported: i64,
    pub truncated: bool,
    pub models: Vec<(String, i64, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Failure {
    /// 401：这份授权不再被接受。
    Unauthorized,
    /// 403 / 429。
    Forbidden,
    RateLimited,
    Server(u16),
    /// 网络错误或超时。
    Network(String),
    /// 200 但结构不认识。
    Unparsable(String),
}

impl Failure {
    pub fn status(&self) -> &'static str {
        match self {
            Failure::Unauthorized => "unauthorized",
            Failure::Forbidden => "forbidden",
            Failure::RateLimited => "rate_limited",
            Failure::Server(_) => "error",
            Failure::Network(_) => "error",
            Failure::Unparsable(_) => "error",
        }
    }

    pub fn detail(&self) -> String {
        match self {
            Failure::Unauthorized => "授权已被服务端拒绝（401），需要重新登录".to_string(),
            Failure::Forbidden => "服务端拒绝访问（403）".to_string(),
            Failure::RateLimited => "被限流（429），稍后再试".to_string(),
            Failure::Server(_) => "服务端错误".to_string(),
            Failure::Network(msg) => sanitize(msg),
            Failure::Unparsable(msg) => format!("响应结构不认识：{}", sanitize(msg)),
        }
    }
}

fn client() -> Result<reqwest::Client, Failure> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|e| Failure::Network(e.to_string()))
}

async fn post_json(
    client: &reqwest::Client,
    url: &str,
    access: &str,
    account: &crate::storage::AccountRecord,
    body: Value,
) -> Result<Value, Failure> {
    let mut req = client
        .post(url)
        .header("Authorization", format!("Bearer {access}"))
        .header("Accept", "application/json")
        // 国内版网关**没有 User-Agent 就返回 403 `code=10085 请求不合法`**
        // （2026-11-14 实测：curl 带 UA 200，`-A ""` 403）。取值与 `growth/api.rs` 一致。
        .header("User-Agent", USER_AGENT)
        .header("X-User-Id", &account.uid);
    // `X-Domain` 只回填账号自己的域，缺失就不带：凭空猜一个域比不带更危险。
    if let Some(domain) = account
        .auth_domain
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        req = req.header("X-Domain", domain);
    }
    let resp = req.json(&body).send().await.map_err(|e| {
        // reqwest 的错误串里可能带完整 URL 和 query，先过 sanitize。
        Failure::Network(e.to_string())
    })?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(match status.as_u16() {
            401 => Failure::Unauthorized,
            403 => Failure::Forbidden,
            429 => Failure::RateLimited,
            code => Failure::Server(code),
        });
    }
    serde_json::from_str(&text).map_err(|e| Failure::Unparsable(format!("{e}")))
}

/// `"4597.75000075"` / `4597.75` / `"0"` → `Some(f64)`；其它（含空串、`"-"`）→ `None`。
fn number(value: Option<&Value>) -> Option<f64> {
    match value? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

fn text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

/// 单位规范化：去空白、转小写、去掉末尾的 `s`。
///
/// `CapacityUnit` 在同一个账号的两个包里会分别写 `credit` 和 `credits`
/// （实测 2026-11-14），它们是同一笔账；而 `tokens` 与 `credits` 是两笔账，
/// 任何情况下都不能相加。
fn canonical_unit(unit: &str) -> String {
    let lower = unit.trim().to_ascii_lowercase();
    lower.strip_suffix('s').unwrap_or(&lower).to_string()
}

/// 解析额度摘要。`code != 0` 视为不认识的响应（服务端用业务码表达失败）。
pub fn parse_summary(body: &Value) -> Result<Quota, Failure> {
    let data = body
        .get("data")
        .filter(|d| d.is_object())
        .ok_or_else(|| Failure::Unparsable(retcode(body)))?;
    if let Some(code) = body.get("code").and_then(Value::as_i64) {
        if code != 0 {
            return Err(Failure::Unparsable(format!(
                "code={code} msg={}",
                text(body.get("msg")).unwrap_or_default()
            )));
        }
    }

    let packages = data
        .get("Packages")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();

    // 每份额度包各自有 total/remain/used。显示用的单位按第一次出现的样子原样带出，
    // 聚合按**规范化**单位分组：credit / credits 是同一笔账，credits / tokens 不是。
    let unit = packages
        .iter()
        .find_map(|p| text(p.get("CapacityUnit")))
        .unwrap_or_else(|| "credits".to_string());
    let group = canonical_unit(&unit);

    let mut sum_total = 0.0;
    let mut sum_used = 0.0;
    let mut sum_remaining = 0.0;
    let mut counted = 0usize;
    for pkg in &packages {
        let pkg_unit = text(pkg.get("CapacityUnit")).unwrap_or_else(|| unit.clone());
        // 真不同的单位（credits vs tokens）不相加；只是单复数不同的（credit / credits）
        // 归一后是同一个单位，必须相加——它们是同一笔账。
        if canonical_unit(&pkg_unit) != group {
            continue;
        }
        let total = number(pkg.get("CycleTotalCapacity"));
        let used = number(pkg.get("CycleUsedCapacity"));
        let remaining = number(pkg.get("CycleRemainCapacity"));
        if total.is_none() && remaining.is_none() {
            continue;
        }
        sum_total += total.unwrap_or(0.0);
        sum_used += used.unwrap_or(0.0);
        sum_remaining += remaining.unwrap_or(0.0);
        counted += 1;
    }

    // 实测结论（2026-11-14，国内版）：`TotalCredits` **是"可用额度合计"，不是总容量**。
    // 该账号 3 个包 4913/500/100（共 5513），剩余 4597.75/500/100，而 `TotalCredits`
    // 正好等于 4597.75+500+100 = 5197.75。所以它只能当 remaining 的兜底，
    // 拿来当 total 会把"还能用多少"写成"一共有多少"。
    let server_available = number(data.get("TotalCredits"));
    let total = (counted > 0).then_some(sum_total);
    let used = (counted > 0).then_some(sum_used);
    let remaining = if counted > 0 {
        Some(sum_remaining)
    } else {
        server_available
    };

    Ok(Quota {
        unit,
        total,
        used,
        remaining,
        plan_name: text(data.get("SubscriptionPackageName")),
        is_paid: data.get("IsPaidUser").and_then(Value::as_bool),
    })
}

/// 解析用量明细。只取计量字段，`input`（prompt 原文）不进入返回值。
pub fn parse_usage(body: &Value, window_days: i64) -> Result<Usage, Failure> {
    if let Some(code) = body.get("code").and_then(Value::as_i64) {
        if code != 0 {
            return Err(Failure::Unparsable(format!("code={code}")));
        }
    }
    let data = body
        .get("data")
        .filter(|d| d.is_object())
        .ok_or_else(|| Failure::Unparsable(retcode(body)))?;
    let rows = data
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let reported = data.get("total").and_then(Value::as_i64).unwrap_or(0);

    let mut credits = 0.0;
    let mut models: HashMap<String, (i64, f64)> = HashMap::new();
    let mut counted_rows = 0i64;
    for row in &rows {
        // `credit` 缺失或 <0 的行不计：上游同款判据，负数不是消耗。
        let Some(credit) = number(row.get("credit")).filter(|c| *c >= 0.0) else {
            continue;
        };
        credits += credit;
        counted_rows += 1;
        let model = text(row.get("model")).unwrap_or_else(|| "unknown".to_string());
        let slot = models.entry(model).or_insert((0, 0.0));
        slot.0 += 1;
        slot.1 += credit;
    }
    let mut models: Vec<(String, i64, f64)> = models
        .into_iter()
        .map(|(name, (count, credit))| (name, count, credit))
        .collect();
    models.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

    Ok(Usage {
        window_days,
        requests: counted_rows,
        credits,
        total_reported: reported,
        truncated: reported > rows.len() as i64,
        models,
    })
}

/// 响应里既没有 `data` 也没有可读错误时，给一条能定位的短句。
fn retcode(body: &Value) -> String {
    let code = body.get("code").map(|c| c.to_string()).unwrap_or_default();
    let msg = text(body.get("msg"))
        .or_else(|| text(body.get("message")))
        .or_else(|| text(body.get("error_msg")))
        .unwrap_or_default();
    format!("缺少 data（code={code} msg={msg}）")
}

/// 一次取齐额度 + 用量。`days <= 0` 时跳过用量（只要余额）。
pub async fn fetch(
    account: &crate::storage::AccountRecord,
    access: &str,
    days: i64,
) -> Result<(Quota, Option<Usage>), Failure> {
    let base = base_for(&account.provider)
        .ok_or_else(|| Failure::Unparsable(format!("未知档位：{}", account.provider)))?;
    let client = client()?;

    let summary_body = post_json(
        &client,
        &format!("{base}{SUMMARY_PATH}"),
        access,
        account,
        json!({}),
    )
    .await?;
    let quota = parse_summary(&summary_body)?;

    if days <= 0 {
        return Ok((quota, None));
    }
    let today = crate::clock::beijing_date();
    let from = crate::clock::beijing_date_offset_days(-(days - 1));
    let usage_body = post_json(
        &client,
        &format!("{base}{USAGE_PATH}"),
        access,
        account,
        json!({
            "startTime": format!("{from} 00:00:00"),
            "endTime": format!("{today} 23:59:59"),
            "pageNum": 1,
            "pageSize": USAGE_PAGE_SIZE,
        }),
    )
    .await?;
    let usage = parse_usage(&usage_body, days)?;
    Ok((quota, Some(usage)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实响应（2026-11-14，国际版，值保留原样、无凭据字段）。
    fn summary_fixture() -> Value {
        json!({
            "code": 0, "msg": "OK", "requestId": "fixture",
            "data": {
                "Packages": [
                    {"PackageCode":"A","CycleTotalCapacity":"250","CycleRemainCapacity":"250","CycleUsedCapacity":"0","CycleFrozenCapacity":"0","CapacityUnit":"credit","TotalCount":1},
                    {"PackageCode":"B","CycleTotalCapacity":"100","CycleRemainCapacity":"100","CycleUsedCapacity":"0","CycleFrozenCapacity":"0","CapacityUnit":"credits","TotalCount":1}
                ],
                "TotalCredits": "350",
                "SubscriptionPackageCode": "",
                "SubscriptionPackageName": "Free",
                "IsPaidUser": false,
                "IsProtectedPriceUser": false,
                "ProTrialStatus": 0,
                "CreditGroups": []
            }
        })
    }

    fn usage_fixture() -> Value {
        json!({
            "code": 0, "msg": "OK", "requestId": "fixture",
            "data": { "total": 3, "data": [
                {"requestId":"r1","credit":2,"model":"hy4-free","client":"WorkBuddy","requestTime":"2026-11-14 12:23:49","input":"不该被读取","agentPurpose":"conversation"},
                {"requestId":"r2","credit":1.5,"model":"hy4-free","client":"WorkBuddy","requestTime":"2026-11-14 12:30:00","input":"不该被读取","agentPurpose":"conversation"},
                {"requestId":"r3","credit":-1,"model":"broken","client":"WorkBuddy","requestTime":"2026-11-14 12:31:00"}
            ]}
        })
    }

    #[test]
    fn credit_and_credits_are_the_same_account() {
        let q = parse_summary(&summary_fixture()).expect("解析成功");
        // 真实响应里两个包分别写 credit / credits —— 同一笔账，必须相加。
        assert_eq!(q.unit, "credit");
        assert_eq!(q.total, Some(350.0));
        assert_eq!(q.remaining, Some(350.0));
        assert_eq!(q.used, Some(0.0));
        assert_eq!(q.plan_name.as_deref(), Some("Free"));
        assert_eq!(q.is_paid, Some(false));
    }

    #[test]
    fn genuinely_different_units_are_never_added() {
        let body = json!({"code":0,"data":{
            "Packages":[
                {"CycleTotalCapacity":"100","CycleRemainCapacity":"90","CycleUsedCapacity":"10","CapacityUnit":"credits"},
                {"CycleTotalCapacity":"50000","CycleRemainCapacity":"40000","CycleUsedCapacity":"10000","CapacityUnit":"tokens"}
            ],
            "TotalCredits":"100"
        }});
        let q = parse_summary(&body).unwrap();
        // 只算 credits 那一笔，tokens 不参与。
        assert_eq!(q.unit, "credits");
        assert_eq!(q.total, Some(100.0));
        assert_eq!(q.remaining, Some(90.0));
    }

    #[test]
    fn canonical_unit_folds_plurals_only() {
        assert_eq!(canonical_unit(" Credits "), "credit");
        assert_eq!(canonical_unit("credit"), "credit");
        assert_ne!(canonical_unit("tokens"), canonical_unit("credits"));
    }

    #[test]
    fn cn_decimal_remaining_is_not_rounded() {
        // 国内版真实形态：一个包 4913 总额、4597.75000075 剩余。
        let body = json!({"code":0,"data":{
            "Packages":[{"CycleTotalCapacity":"4913","CycleRemainCapacity":"4597.75000075","CycleUsedCapacity":"315.24999925","CapacityUnit":"credits"}],
            "TotalCredits":"4913","SubscriptionPackageName":"体验版","IsPaidUser":false
        }});
        let q = parse_summary(&body).unwrap();
        assert_eq!(q.total, Some(4913.0));
        assert_eq!(q.remaining, Some(4597.75000075));
        assert_eq!(q.used, Some(315.24999925));
    }

    #[test]
    fn total_credits_is_available_not_capacity() {
        // 国内版实测：3 个包 4913/500/100，剩余 4597.75/500/100，`TotalCredits`=5197.75
        // 正好等于剩余之和 → 它是"可用额度"，不能当总容量。
        let body = json!({"code":0,"data":{
            "Packages":[
                {"CycleTotalCapacity":"4913","CycleRemainCapacity":"4597.75000075","CycleUsedCapacity":"315.24999925","CapacityUnit":"credits"},
                {"CycleTotalCapacity":"500","CycleRemainCapacity":"500","CycleUsedCapacity":"0","CapacityUnit":"credits"},
                {"CycleTotalCapacity":"100","CycleRemainCapacity":"100","CycleUsedCapacity":"0","CapacityUnit":"credits"}
            ],
            "TotalCredits":"5197.75000075","SubscriptionPackageName":"体验版"
        }});
        let q = parse_summary(&body).unwrap();
        assert_eq!(q.total, Some(5513.0));
        assert_eq!(q.remaining, Some(5197.75000075));
        assert_eq!(q.used, Some(315.24999925));
    }

    #[test]
    fn empty_packages_fall_back_to_the_server_available_field() {
        let body = json!({"code":0,"data":{"Packages":[],"TotalCredits":"0"}});
        let q = parse_summary(&body).unwrap();
        // 没有包就没有 total/used 的来源；remaining 用服务端自己的可用额度。
        assert_eq!(q.total, None);
        assert_eq!(q.used, None);
        assert_eq!(q.remaining, Some(0.0));

        let missing = parse_summary(&json!({"code":0,"data":{"Packages":[]}})).unwrap();
        assert_eq!(missing.remaining, None, "服务端不给就不猜");
    }

    #[test]
    fn non_zero_code_is_unparsable() {
        let err = parse_summary(&json!({"code":10086,"msg":"token 失效","data":null})).unwrap_err();
        assert_eq!(err.status(), "error");
        assert!(err.detail().contains("10086"), "{}", err.detail());
    }

    #[test]
    fn usage_skips_negative_credit_and_ignores_prompt_text() {
        let u = parse_usage(&usage_fixture(), 7).unwrap();
        assert_eq!(u.requests, 2);
        assert_eq!(u.credits, 3.5);
        assert_eq!(u.total_reported, 3);
        assert!(!u.truncated);
        assert_eq!(u.models.len(), 1);
        assert_eq!(u.models[0].0, "hy4-free");
        assert_eq!(u.models[0].1, 2);
        // prompt 原文一个字都不能出现在结果里。
        let rendered = format!("{:?}", u);
        assert!(!rendered.contains("不该被读取"));
    }

    #[test]
    fn usage_marks_truncation_from_server_total() {
        let mut body = usage_fixture();
        body["data"]["total"] = json!(9000);
        let u = parse_usage(&body, 30).unwrap();
        assert!(u.truncated);
    }

    #[test]
    fn base_only_accepts_the_two_official_sites() {
        assert_eq!(base_for("workbuddy"), Some("https://www.codebuddy.cn"));
        assert_eq!(base_for("workbuddy_ai"), Some("https://www.workbuddy.ai"));
        assert_eq!(base_for("qoder_cn"), None);
        assert_eq!(base_for("evil.example"), None);
    }

    #[test]
    fn failure_statuses_are_distinct() {
        assert_eq!(Failure::Unauthorized.status(), "unauthorized");
        assert_eq!(Failure::RateLimited.status(), "rate_limited");
        assert_eq!(Failure::Server(502).status(), "error");
        assert_eq!(Failure::Network("timeout".into()).status(), "error");
    }
}
