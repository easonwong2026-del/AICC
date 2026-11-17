//! 只读取证（PR #100）：把四个 GET 的真实响应抓下来，脱敏后归档。
//!
//! ## 为什么需要这一层
//!
//! `growth-rewards-protocol-2026-11-16.md` 里有一半结论是 B/C 级（第三方实测或源码推断）。
//! 真接线之前必须换成本机账号的 A 级证据，否则"抽奖响应里的积分字段在哪"这种问题
//! 只能靠猜 —— 而猜错一次就是一次真实消耗。
//!
//! ## 边界
//!
//! - **只有 GET**：四个接口都是现有只读查询，不改变任何服务端状态；
//! - **只出口脱敏后的结构**：疑似凭据的键直接丢弃，邮箱/手机号打码，长字符串截断；
//! - 本模块不做网络调度，`capture()` 由调用方（dev-only RPC）驱动。
//!
//! ## 输出的两份东西
//!
//! 1. `endpoints`：每个接口一段 `{ path, method, status, prefix, body }`，`body` 已脱敏；
//! 2. `findings`：我们**想知道答案的那几个字段**在响应里的真实位置与类型 ——
//!    这是给人看的结论，比原样贴一份 JSON 有用得多。

use serde::Serialize;
use serde_json::{json, Map, Value};

use super::api::ReadOnlyEndpoints;

/// 单条字符串最多保留多少字符。
const MAX_STRING_CHARS: usize = 160;
/// 数组最多保留多少项。
///
/// 240 不够：热力图 `cells[]` 是一年多的数据（实测 365+），
/// 截断会让"最新几天没断签"这种结论无从核对。放到 800 足够覆盖一年半。
const MAX_ARRAY_ITEMS: usize = 800;
/// 递归深度上限：再深就不是我们关心的结构了。
const MAX_DEPTH: usize = 8;

/// 一个接口的采集结果。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapturedEndpoint {
    pub name: &'static str,
    pub method: &'static str,
    pub path: &'static str,
    /// HTTP 状态码。
    ///
    /// `None` 表示**成功**：api 层已经把 2xx 校验过，但没有把具体状态码透传出来，
    /// 所以这里不编一个 200 写进证据。失败时是服务端真实状态码。
    pub http_status: Option<u16>,
    /// 这次实际生效的路径前缀。
    pub prefix: Option<String>,
    /// 业务码（响应体里的 `code`）。
    pub code: Option<i64>,
    /// 脱敏后的响应体。失败时为 `null`。
    pub body: Value,
    /// 失败原因（已脱敏）。
    pub error: Option<String>,
}

/// 一条"这个字段到底在哪"的结论。
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FieldFinding {
    /// 我们想找的语义名，例如 `lottery_chances`。
    pub field: &'static str,
    /// 命中的 JSON 路径（点号分隔，数组用 `[]`）。没找到就是空。
    pub paths: Vec<String>,
    /// 第一个命中值的类型：`number` / `string` / `bool` / `object` / `array`。
    pub kind: Option<&'static str>,
    /// 第一个命中的原值（已脱敏、已截断），便于核对量级。
    pub sample: Option<Value>,
    /// 结论：`confirmed` / `missing`。
    pub verdict: &'static str,
    /// 证据等级。抓到了就是 A（本机真实账号），没抓到仍是 C。
    pub grade: &'static str,
    /// 中文说明。
    pub note: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvidenceReport {
    pub captured_at: String,
    pub account_id: String,
    /// 客户端版本（`cn`）。
    pub variant: &'static str,
    pub read_only: bool,
    pub endpoints: Vec<CapturedEndpoint>,
    pub findings: Vec<FieldFinding>,
}

// ---------------------------------------------------------------------------
// 脱敏
// ---------------------------------------------------------------------------

/// 键名里出现这些词就不进证据：可能直接是凭据或会话。
const SECRET_KEYS: [&str; 10] = [
    "token",
    "secret",
    "password",
    "cookie",
    "authorization",
    "bearer",
    "session",
    "refresh",
    "credential",
    "private",
];

/// 这些键的值要打码（隐私），但键本身要保留 —— 知道"有邮箱字段"是有用的。
const PRIVATE_KEYS: [&str; 4] = ["email", "phone", "mobile", "nickname"];

fn is_secret_key(key: &str) -> bool {
    let lowered = key.to_lowercase();
    SECRET_KEYS.iter().any(|bad| lowered.contains(bad))
}

fn is_private_key(key: &str) -> bool {
    let lowered = key.to_lowercase();
    PRIVATE_KEYS.iter().any(|private| lowered.contains(private))
}

fn mask(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= 2 {
        return "**".to_string();
    }
    format!("{}**{}", chars[0], chars[chars.len() - 1])
}

/// 递归脱敏。规则都很硬，宁可丢信息也不把凭据写进仓库：
///
/// - 键名像凭据 → 整个键丢掉（连同值）；
/// - 键名像隐私 → 值打码成 `a**m`；
/// - 字符串 → `rpc::sanitize`（抹掉 ≥32 位的令牌串）+ 截断；
/// - 数组最多 240 项、深度最多 8 层。
pub fn sanitize_json(value: &Value) -> Value {
    sanitize_at(value, 0)
}

fn sanitize_at(value: &Value, depth: usize) -> Value {
    if depth >= MAX_DEPTH {
        return json!("<depth-limit>");
    }
    match value {
        Value::Object(map) => {
            let mut out = Map::new();
            for (key, item) in map {
                if is_secret_key(key) {
                    out.insert(key.clone(), json!("<redacted-key>"));
                    continue;
                }
                if is_private_key(key) {
                    let masked = item
                        .as_str()
                        .map(mask)
                        .unwrap_or_else(|| "<redacted>".to_string());
                    out.insert(key.clone(), Value::String(masked));
                    continue;
                }
                out.insert(key.clone(), sanitize_at(item, depth + 1));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .take(MAX_ARRAY_ITEMS)
                .map(|item| sanitize_at(item, depth + 1))
                .collect(),
        ),
        Value::String(text) => {
            let cleaned = crate::rpc::sanitize(text);
            let cut: String = cleaned.trim().chars().take(MAX_STRING_CHARS).collect();
            Value::String(cut)
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// 字段定位
// ---------------------------------------------------------------------------

/// 在 JSON 里找某个键，返回所有路径（点号分隔，数组用 `[]`）。
pub fn find_paths(value: &Value, key: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    walk(value, "", key, 0, &mut out);
    out
}

fn walk(value: &Value, prefix: &str, key: &str, depth: usize, out: &mut Vec<(String, Value)>) {
    if depth >= MAX_DEPTH {
        return;
    }
    match value {
        Value::Object(map) => {
            for (name, item) in map {
                let path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}.{name}")
                };
                if name.eq_ignore_ascii_case(key) {
                    out.push((path.clone(), item.clone()));
                }
                walk(item, &path, key, depth + 1, out);
            }
        }
        Value::Array(items) => {
            for item in items.iter().take(MAX_ARRAY_ITEMS) {
                let path = format!("{prefix}[]");
                walk(item, &path, key, depth + 1, out);
            }
        }
        _ => {}
    }
}

fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Bool(_) => "bool",
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::Null => "null",
    }
}

fn finding(
    corpus: &[(String, Value)],
    field: &'static str,
    keys: &[&str],
    note: &'static str,
) -> FieldFinding {
    let mut paths = Vec::new();
    let mut sample = None;
    let mut kind = None;
    for key in keys {
        for (path, value) in find_paths_in(corpus, key) {
            if sample.is_none() {
                sample = Some(sanitize_json(&value));
                kind = Some(kind_of(&value));
            }
            paths.push(path);
        }
    }
    let verdict = if paths.is_empty() {
        "missing"
    } else {
        "confirmed"
    };
    FieldFinding {
        field,
        paths,
        kind,
        sample,
        verdict,
        grade: if verdict == "confirmed" { "A" } else { "C" },
        note,
    }
}

/// 在一组"接口 → 响应"里找键。返回带接口名的路径，便于定位。
fn find_paths_in(corpus: &[(String, Value)], key: &str) -> Vec<(String, Value)> {
    let mut out = Vec::new();
    for (name, body) in corpus {
        for (path, value) in find_paths(body, key) {
            out.push((format!("{name}:{path}"), value));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 采集
// ---------------------------------------------------------------------------

/// 抓四个只读接口。**全部是 GET**，不改变任何服务端状态。
pub async fn capture(access: &str, uid: &str, domain: &str) -> EvidenceReport {
    let endpoints = ReadOnlyEndpoints::new(access, uid, domain);
    let (streak, redeem, heatmap, chances) = tokio::join!(
        endpoints.streak(),
        endpoints.redeem_summary(),
        endpoints.heatmap(),
        endpoints.lottery_chances(),
    );

    let mut captured = Vec::new();
    let mut corpus: Vec<(String, Value)> = Vec::new();

    for (name, path, result) in [
        ("streak", "/streak", streak),
        ("redeemSummary", "/redeem/summary", redeem),
        ("heatmap", "/heatmap", heatmap),
        ("lotteryChances", "/lottery/chances", chances),
    ] {
        match result {
            Ok((body, prefix)) => {
                let sanitized = sanitize_json(&body);
                corpus.push((name.to_string(), sanitized.clone()));
                captured.push(CapturedEndpoint {
                    name,
                    method: "GET",
                    path,
                    http_status: None,
                    prefix: Some(prefix.to_string()),
                    code: super::api::code_of(&body),
                    body: sanitized,
                    error: None,
                });
            }
            Err(err) => captured.push(CapturedEndpoint {
                name,
                method: "GET",
                path,
                http_status: err.http.map(|code| code as u16),
                prefix: None,
                code: err.code,
                body: Value::Null,
                error: Some(err.message.clone()),
            }),
        }
    }

    let findings = vec![
        finding(
            &corpus,
            "lottery_chances",
            &["chances", "balance", "lottery_chances"],
            "抽奖次数的真实字段名与层级；决定界面读哪个键",
        ),
        finding(
            &corpus,
            "credits",
            &["credit", "credits", "reward_credit"],
            "积分字段是否存在（成长中心的档案接口没有积分）",
        ),
        finding(&corpus, "energy", &["energy", "energies"], "能量字段的位置"),
        finding(
            &corpus,
            "makeup_cards",
            &["makeup_cards"],
            "补签卡余额是对象还是裸数字",
        ),
        finding(
            &corpus,
            "makeup_balance",
            &["balance", "count"],
            "`balance` 到底属于哪个对象：抽奖次数还是补签卡",
        ),
        finding(
            &corpus,
            "tier_7d_status",
            &["tier_7d_status", "starter_status"],
            "7 天档位状态键名（两种写法都可能出现）",
        ),
        finding(
            &corpus,
            "tier_14d_status",
            &["tier_14d_status", "advanced_status"],
            "14 天档位状态键名",
        ),
        finding(
            &corpus,
            "tier_28d_status",
            &["tier_28d_status", "legendary_status"],
            "28 天档位状态键名",
        ),
        finding(
            &corpus,
            "today",
            &["today"],
            "服务端日期字段：`today` 是字符串还是 `{date}` 对象",
        ),
        finding(
            &corpus,
            "launch_date",
            &["launch_date"],
            "活动上线日期，补签窗口的下限",
        ),
        finding(
            &corpus,
            "cells",
            &["cells"],
            "热力图格子：字段名与 `score` 的位置",
        ),
    ];

    EvidenceReport {
        captured_at: crate::clock::now_rfc3339(),
        account_id: String::new(),
        variant: "cn",
        read_only: true,
        endpoints: captured,
        findings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_dropped_before_they_reach_the_repository() {
        let raw = json!({
            "code": 0,
            "access_token": "should-not-appear",
            "nested": { "refreshToken": "nope", "authorization": "Bearer x" },
            "data": { "chances": 1 }
        });
        let clean = sanitize_json(&raw);
        let text = serde_json::to_string(&clean).unwrap();
        assert!(!text.contains("should-not-appear"));
        assert!(!text.contains("Bearer x"));
        assert!(!text.contains("nope"));
        assert_eq!(clean["data"]["chances"], 1);
        assert_eq!(clean["access_token"], "<redacted-key>");
    }

    #[test]
    fn private_values_are_masked_but_the_keys_stay() {
        let clean = sanitize_json(&json!({ "email": "someone@example.com", "nickname": "王翊" }));
        assert_eq!(clean["email"], "s**m");
        // 两个字的名字不做"首尾各留一个"，直接全遮。
        assert_eq!(clean["nickname"], "**");
        assert_eq!(
            sanitize_json(&json!({ "nickname": "王翊名" }))["nickname"],
            "王**名"
        );
    }

    #[test]
    fn long_token_like_strings_are_redacted_and_truncated() {
        let long = "a".repeat(80);
        let clean = sanitize_json(&json!({ "note": long }));
        let text = clean["note"].as_str().unwrap();
        assert!(!text.contains(&"a".repeat(40)));
        assert!(text.chars().count() <= MAX_STRING_CHARS);
    }

    #[test]
    fn arrays_and_depth_are_capped() {
        let items: Vec<Value> = (0..(MAX_ARRAY_ITEMS + 100))
            .map(|i| json!({ "score": i }))
            .collect();
        let clean = sanitize_json(&json!({ "cells": items }));
        assert_eq!(clean["cells"].as_array().unwrap().len(), MAX_ARRAY_ITEMS);

        let mut deep = json!(1);
        for _ in 0..20 {
            deep = json!({ "a": deep });
        }
        let text = serde_json::to_string(&sanitize_json(&deep)).unwrap();
        assert!(text.contains("<depth-limit>"));
    }

    #[test]
    fn find_paths_reports_nested_and_array_positions() {
        let body = json!({
            "data": { "streak": { "makeup_cards": { "balance": 2 } }, "cells": [{ "score": 0 }] }
        });
        let hits = find_paths(&body, "makeup_cards");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].0, "data.streak.makeup_cards");

        let scores = find_paths(&body, "score");
        assert_eq!(scores[0].0, "data.cells[].score");
    }

    #[test]
    fn a_finding_without_hits_is_marked_missing_not_guessed() {
        let report_hit = finding(
            &[("streak".into(), json!({ "chances": 3 }))],
            "lottery_chances",
            &["chances"],
            "x",
        );
        assert_eq!(report_hit.verdict, "confirmed");
        assert_eq!(report_hit.grade, "A");
        assert_eq!(report_hit.kind, Some("number"));
        assert_eq!(report_hit.paths, vec!["streak:chances".to_string()]);

        let missing = finding(&[], "lottery_chances", &["chances"], "x");
        assert_eq!(missing.verdict, "missing");
        assert_eq!(missing.grade, "C", "没抓到就不能升等级");
        assert!(missing.paths.is_empty());
    }

    #[test]
    fn finding_samples_are_sanitised_too() {
        let finding = finding(
            &[(
                "streak".into(),
                json!({ "makeup_cards": { "balance": 2, "x": "secret" } }),
            )],
            "makeup_cards",
            &["makeup_cards"],
            "x",
        );
        let sample = finding.sample.unwrap();
        assert_eq!(sample["balance"], 2);
    }
}
