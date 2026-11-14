//! 从 Magpie 借用登录态（WorkBuddy CN / WorkBuddy AI / Qoder CN / Qoder Global）。
//!
//! Magpie 自己完成了各平台的 OAuth，并在 `~/.config/magpie/plugin-auth.json`
//! 里按**账号键**保存了 `access` / `refresh` / `uid` / `expires`（明文 JSON）：
//!
//! | Magpie 键 | AICC platform | AICC provider | 备注 |
//! | --- | --- | --- | --- |
//! | `workbuddy`、`workbuddy#xxxxxx` | `workbuddy` | `workbuddy` | 国内版（copilot.tencent.com） |
//! | `workbuddy-ai`、`workbuddy-ai#…` | `workbuddy` | `workbuddy_ai` | 国际版（www.workbuddy.ai） |
//! | `qoder`、`qoder#xxxxxx` | `qoder_global` | — | 写 `qoder_accounts`，不写 `accounts` |
//! | `qoder-cn`、`qoder-cn#…` | `qoder_cn` | — | 同上 |
//!
//! 一个 provider 可以有**多个账号**（带 `#` 后缀的键），所以这里按前缀逐个匹配，
//! 不是"每个 provider 只取第一条"。
//!
//! AICC 只**读**这个文件，并且对 WorkBuddy **只借 access token**：
//!
//! * access 是无状态 JWT，多端同时用是正常行为，不会影响 Magpie；
//! * refresh 是有状态的单份凭据，两边各自刷新会互相踢下线 —— 所以这里
//!   既不读取 refresh，也不写入 AICC 的钥匙串，过期后重新从 Magpie 同步。
//! * Qoder 的 Magpie 来源连令牌都不复制：只在 `qoder_authorizations` 里登记
//!   `source_key`，用的时候重新读当前文件（见 `storage/qoder_accounts.rs`）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::providers::workbuddy_auth::ms_to_rfc3339;

/// 账号来源标记，与 `storage::AccountRecord::auth_source` 对应。
pub const SOURCE_MAGPIE: &str = "magpie";

const AUTH_FILE: &str = "plugin-auth.json";
const PROVIDERS_FILE: &str = "plugin-providers.json";

/// 一个 Magpie provider 到 AICC 平台/服务的映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Provider {
    /// `plugin-auth.json` 里的键前缀，也是 `plugin-providers.json` 的 `id`。
    pub id: &'static str,
    /// AICC 的平台标识。
    pub platform: &'static str,
    /// WorkBuddy 用：`workbuddy` / `workbuddy_ai`；Qoder 为 None。
    pub provider: Option<&'static str>,
    /// Qoder 用：`qoder_cn` / `qoder_global`。
    pub edition: Option<&'static str>,
    /// 昵称兜底前缀与界面显示名。
    pub label: &'static str,
}

/// 支持的 provider，顺序即界面顺序。
pub const PROVIDERS: &[Provider] = &[
    Provider {
        id: "workbuddy",
        platform: "workbuddy",
        provider: Some("workbuddy"),
        edition: None,
        label: "WorkBuddy",
    },
    Provider {
        id: "workbuddy-ai",
        platform: "workbuddy",
        provider: Some("workbuddy_ai"),
        edition: None,
        label: "WorkBuddy AI",
    },
    Provider {
        id: "qoder",
        platform: "qoder_global",
        provider: None,
        edition: Some("qoder_global"),
        label: "Qoder",
    },
    Provider {
        id: "qoder-cn",
        platform: "qoder_cn",
        provider: None,
        edition: Some("qoder_cn"),
        label: "Qoder CN",
    },
];

/// 按键名找 provider：`qoder` 与 `qoder#2ntprc` 都算 `qoder`。
///
/// 必须**先匹配最长的 id**，否则 `qoder-cn` 会被 `qoder` 抢走。
pub fn provider_of(key: &str) -> Option<&'static Provider> {
    let head = key.split('#').next().unwrap_or(key);
    let mut best: Option<&'static Provider> = None;
    for provider in PROVIDERS {
        if provider.id == head {
            match best {
                Some(current) if current.id.len() >= provider.id.len() => {}
                _ => best = Some(provider),
            }
        }
    }
    best
}

#[derive(Debug, Clone)]
pub struct MagpieLogin {
    /// Magpie 内部的账号键：`workbuddy`、`workbuddy#2ntprc`、`qoder-cn`……
    pub key: String,
    /// 命中的 provider。
    pub provider: &'static Provider,
    pub uid: String,
    pub nickname: Option<String>,
    pub domain: String,
    pub access_token: String,
    pub expires_at_ms: Option<i64>,
}

impl MagpieLogin {
    pub fn platform(&self) -> &'static str {
        self.provider.platform
    }

    /// `accounts.provider` 的值；Qoder 没有这个概念，返回 None。
    pub fn account_provider(&self) -> Option<&'static str> {
        self.provider.provider
    }

    pub fn edition(&self) -> Option<&'static str> {
        self.provider.edition
    }
}

#[derive(Debug)]
pub enum MagpieError {
    /// 机器上没装 Magpie（或它从没登录过）。
    NotInstalled(PathBuf),
    /// 文件在，但读不出来 / 不是预期结构。
    Unreadable(String),
}

impl MagpieError {
    pub fn message(&self) -> String {
        match self {
            Self::NotInstalled(dir) => {
                format!("没有找到 Magpie 的登录数据（{}）", dir.to_string_lossy())
            }
            Self::Unreadable(detail) => format!("Magpie 登录数据无法读取：{detail}"),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::NotInstalled(_) => "not_installed",
            Self::Unreadable(_) => "unreadable",
        }
    }
}

/// Magpie 把配置放在 `~/.config/magpie`（不是 macOS 的 Application Support）。
/// `MAGPIE_CONFIG_DIR` 只给测试与非常规安装用。
pub fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("MAGPIE_CONFIG_DIR") {
        if !dir.trim().is_empty() {
            return PathBuf::from(dir);
        }
    }
    crate::paths::dirs_home().join(".config/magpie")
}

pub fn read_logins() -> Result<Vec<MagpieLogin>, MagpieError> {
    read_logins_from(&config_dir())
}

/// 读取**所有支持 provider** 的账号。没有匹配到任何 provider 的键直接跳过。
pub fn read_logins_from(dir: &Path) -> Result<Vec<MagpieLogin>, MagpieError> {
    let auth_path = dir.join(AUTH_FILE);
    if !auth_path.is_file() {
        return Err(MagpieError::NotInstalled(dir.to_path_buf()));
    }

    let auth = read_json(&auth_path)?;
    let nicknames = read_nicknames(&dir.join(PROVIDERS_FILE));

    let map = auth.as_object().ok_or_else(|| {
        MagpieError::Unreadable(format!("{} 顶层不是对象", auth_path.to_string_lossy()))
    })?;

    let mut out = Vec::new();
    for (key, entry) in map {
        let Some(provider) = provider_of(key) else {
            continue;
        };
        let access = entry
            .get("access")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        let uid = entry
            .get("uid")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        // 没有 uid 或令牌的记录无法变成账号，直接略过（不算错误）。
        if access.is_empty() || uid.is_empty() {
            continue;
        }
        out.push(MagpieLogin {
            key: key.clone(),
            provider,
            uid,
            nickname: nicknames.get(key).cloned().flatten(),
            domain: entry
                .get("domain")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            access_token: access,
            expires_at_ms: entry.get("expires").and_then(Value::as_i64),
        });
    }

    // 顺序稳定：先按 provider 表顺序，再按键名。
    out.sort_by(|a, b| {
        let rank = |p: &'static Provider| PROVIDERS.iter().position(|x| x.id == p.id).unwrap_or(99);
        rank(a.provider)
            .cmp(&rank(b.provider))
            .then_with(|| a.key.cmp(&b.key))
    });
    if out.is_empty() {
        return Err(MagpieError::Unreadable(
            "Magpie 里没有已登录的 WorkBuddy / Qoder 账号".to_string(),
        ));
    }
    Ok(out)
}

/// 昵称不在 `plugin-auth.json` 里，而在 `plugin-providers.json` 的
/// `accounts[].accountId`（就是界面上看到的"卡里""一锅端"）。按 `key` 对齐。
///
/// 四个 provider 都可能在文件里，一律按 key 收集：key 本身已经带 provider 前缀，
/// 不会串。
fn read_nicknames(path: &Path) -> HashMap<String, Option<String>> {
    let mut out = HashMap::new();
    let Ok(value) = read_json(path) else {
        return out;
    };
    let Some(providers) = value.as_array() else {
        return out;
    };
    for provider in providers {
        let Some(id) = provider.get("id").and_then(Value::as_str) else {
            continue;
        };
        if provider_of(id).is_none() {
            continue;
        }
        let Some(accounts) = provider.get("accounts").and_then(Value::as_array) else {
            continue;
        };
        for account in accounts {
            let Some(key) = account.get("key").and_then(Value::as_str) else {
                continue;
            };
            let name = account
                .get("accountId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            out.insert(key.to_string(), name);
        }
    }
    out
}

fn read_json(path: &Path) -> Result<Value, MagpieError> {
    let raw = std::fs::read(path)
        .map_err(|err| MagpieError::Unreadable(format!("{}：{err}", path.to_string_lossy())))?;
    serde_json::from_slice(&raw)
        .map_err(|err| MagpieError::Unreadable(format!("{}：{err}", path.to_string_lossy())))
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 单条账号的**脱敏**描述：给 RPC / 界面用，绝不含令牌。
pub fn describe(login: &MagpieLogin, now_ms: i64) -> Value {
    let expired = login.expires_at_ms.map(|at| at <= now_ms).unwrap_or(false);
    json!({
        "key": login.key,
        "platform": login.platform(),
        "provider": login.account_provider(),
        "edition": login.edition(),
        "providerLabel": login.provider.label,
        "uid": login.uid,
        "nickname": login
            .nickname
            .clone()
            .unwrap_or_else(|| short_uid_for(login.provider.label, &login.uid)),
        "domain": login.domain,
        "expiresAt": login.expires_at_ms.and_then(ms_to_rfc3339),
        "expiresInHours": login
            .expires_at_ms
            .map(|at| ((at - now_ms) as f64 / 3_600_000.0 * 10.0).round() / 10.0),
        "expired": expired,
        "source": SOURCE_MAGPIE,
    })
}

pub fn short_uid_for(label: &str, uid: &str) -> String {
    let head: String = uid.chars().take(6).collect();
    if head.len() < uid.len() {
        format!("{label} {head}")
    } else {
        format!("{label} {uid}")
    }
}

/// `providers.magpie.accounts` 的返回值。
pub fn preview() -> Value {
    preview_from(&config_dir())
}

/// 指定目录的版本，测试用（不依赖进程级环境变量）。
pub fn preview_from(dir: &Path) -> Value {
    match read_logins_from(dir) {
        Ok(logins) => {
            let now = now_ms();
            let accounts: Vec<Value> = logins.iter().map(|l| describe(l, now)).collect();
            let mut counts = serde_json::Map::new();
            for provider in PROVIDERS {
                let n = logins
                    .iter()
                    .filter(|l| l.provider.id == provider.id)
                    .count();
                counts.insert(provider.id.to_string(), json!(n));
            }
            json!({
                "available": true,
                "configDir": dir.to_string_lossy(),
                "accounts": accounts,
                "counts": counts,
                "providers": PROVIDERS
                    .iter()
                    .map(|p| json!({
                        "id": p.id,
                        "platform": p.platform,
                        "provider": p.provider,
                        "edition": p.edition,
                        "label": p.label,
                    }))
                    .collect::<Vec<_>>(),
            })
        }
        Err(err) => json!({
            "available": false,
            "configDir": dir.to_string_lossy(),
            "kind": err.kind(),
            "message": err.message(),
            "accounts": [],
            "counts": {},
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(dir: &Path, auth: Value, providers: Option<Value>) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(AUTH_FILE), auth.to_string()).unwrap();
        if let Some(providers) = providers {
            std::fs::write(dir.join(PROVIDERS_FILE), providers.to_string()).unwrap();
        }
    }

    fn sample_auth() -> Value {
        json!({
            "codex": {"access": "codex-access", "uid": "codex-uid"},
            "workbuddy": {
                "access": "a".repeat(1620),
                "refresh": "r".repeat(1622),
                "uid": "6f1c2d94-1111-2222-3333-444455556666",
                "domain": "www.workbuddy.cn",
                "expires": 1_795_354_934_547i64
            },
            "workbuddy#2ntprc": {
                "access": "b".repeat(1624),
                "refresh": "s".repeat(1626),
                "uid": "0a9b8c7d-aaaa-bbbb-cccc-ddddeeeeffff",
                "domain": "www.workbuddy.cn",
                "expires": 1_795_697_791_556i64
            }
        })
    }

    fn sample_providers() -> Value {
        json!([{
            "id": "workbuddy",
            "signedIn": true,
            "accounts": [
                {"key": "workbuddy", "accountId": "卡里"},
                {"key": "workbuddy#2ntprc", "accountId": "一锅端"}
            ]
        }])
    }

    #[test]
    fn reads_only_workbuddy_entries_and_maps_nicknames() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(), sample_auth(), Some(sample_providers()));

        let logins = read_logins_from(dir.path()).unwrap();
        assert_eq!(logins.len(), 2, "codex 那条不能被当成 WorkBuddy");
        assert_eq!(logins[0].key, "workbuddy");
        assert_eq!(logins[0].nickname.as_deref(), Some("卡里"));
        assert_eq!(logins[1].nickname.as_deref(), Some("一锅端"));
        assert_eq!(logins[1].uid, "0a9b8c7d-aaaa-bbbb-cccc-ddddeeeeffff");
    }

    #[test]
    fn missing_providers_file_still_yields_accounts() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(), sample_auth(), None);
        let logins = read_logins_from(dir.path()).unwrap();
        assert_eq!(logins.len(), 2);
        assert!(logins.iter().all(|l| l.nickname.is_none()));
    }

    #[test]
    fn missing_file_is_reported_as_not_installed() {
        let dir = tempfile::tempdir().unwrap();
        match read_logins_from(dir.path()) {
            Err(err) => assert_eq!(err.kind(), "not_installed"),
            Ok(_) => panic!("没有文件不应成功"),
        }
    }

    #[test]
    fn entries_without_uid_or_token_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        fixture(
            dir.path(),
            json!({
                "workbuddy": {"access": "", "uid": "u-1"},
                "workbuddy#x": {"access": "tok", "uid": ""},
            }),
            None,
        );
        match read_logins_from(dir.path()) {
            Err(err) => assert_eq!(err.kind(), "unreadable"),
            Ok(v) => panic!("应为空，实际 {} 条", v.len()),
        }
    }

    /// 描述里绝不能出现令牌 —— 它会直接进 RPC 响应和界面。
    #[test]
    fn describe_never_leaks_tokens() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(), sample_auth(), Some(sample_providers()));
        let login = read_logins_from(dir.path()).unwrap().remove(0);
        let text = describe(&login, now_ms()).to_string();
        assert!(!text.contains(&"a".repeat(32)), "access token 泄漏：{text}");
        assert!(!text.contains("refresh"), "不应提到 refresh 字段");
        assert!(text.contains("卡里"));
    }

    #[test]
    fn describe_flags_expired_tokens() {
        let login = MagpieLogin {
            key: "workbuddy".into(),
            provider: provider_of("workbuddy").unwrap(),
            uid: "u".into(),
            nickname: None,
            domain: "www.workbuddy.cn".into(),
            access_token: "tok".into(),
            expires_at_ms: Some(1_000),
        };
        let report = describe(&login, 2_000);
        assert_eq!(report["expired"], json!(true));
        assert_eq!(report["nickname"], json!("WorkBuddy u"));
        let hours = report["expiresInHours"].as_f64().unwrap();
        // 已过期：小时数 ≤ 0（-0.0 也是 0）。
        assert!(hours <= 0.0, "{report}");
    }

    #[test]
    fn preview_reports_unavailable_without_magpie() {
        let dir = tempfile::tempdir().unwrap();
        let report = preview_from(&dir.path().join("missing"));
        assert_eq!(report["available"], json!(false));
        assert_eq!(report["kind"], json!("not_installed"));
        assert_eq!(report["accounts"], json!([]));
    }

    #[test]
    fn preview_lists_accounts_without_tokens() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(), sample_auth(), Some(sample_providers()));
        let report = preview_from(dir.path());
        assert_eq!(report["available"], json!(true));
        assert_eq!(report["accounts"].as_array().unwrap().len(), 2);
        let text = report.to_string();
        assert!(
            !text.contains(&"a".repeat(32)),
            "preview 泄漏了 access token"
        );
        assert!(!text.contains(&"b".repeat(32)));
    }

    fn multi_site_auth() -> Value {
        json!({
            "codex": {"access": "codex-access", "uid": "codex-uid"},
            "workbuddy": {"access": "wb-1", "uid": "wb-uid-1", "domain": "www.workbuddy.cn"},
            "workbuddy#2ntprc": {"access": "wb-2", "uid": "wb-uid-2", "domain": "www.workbuddy.cn"},
            "workbuddy-ai": {"access": "ai-1", "uid": "ai-uid-1", "domain": "www.workbuddy.ai"},
            "workbuddy-ai#abcd12": {"access": "ai-2", "uid": "ai-uid-2", "domain": "www.workbuddy.ai"},
            "qoder": {"access": "qd-1", "uid": "qd-uid-1", "domain": "qoder.com"},
            "qoder#a1b2c3": {"access": "qd-2", "uid": "qd-uid-2", "domain": "qoder.com"},
            "qoder-cn": {"access": "qn-1", "uid": "qn-uid-1", "domain": "qoder.cn"},
            "qoder-cn#d4e5f6": {"access": "qn-2", "uid": "qn-uid-2", "domain": "qoder.cn"}
        })
    }

    #[test]
    fn maps_all_four_providers_and_keeps_every_account() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(), multi_site_auth(), None);
        let logins = read_logins_from(dir.path()).unwrap();
        // codex 不是我们的 provider。
        assert_eq!(logins.len(), 8);
        let count = |id: &str| logins.iter().filter(|l| l.provider.id == id).count();
        assert_eq!(count("workbuddy"), 2);
        assert_eq!(count("workbuddy-ai"), 2);
        assert_eq!(count("qoder"), 2);
        assert_eq!(count("qoder-cn"), 2);
    }

    #[test]
    fn qoder_cn_key_is_not_swallowed_by_qoder() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(), multi_site_auth(), None);
        let logins = read_logins_from(dir.path()).unwrap();
        let cn = logins
            .iter()
            .find(|l| l.key == "qoder-cn")
            .expect("必须认出 qoder-cn");
        assert_eq!(cn.platform(), "qoder_cn");
        assert_eq!(cn.edition(), Some("qoder_cn"));
        let global = logins.iter().find(|l| l.key == "qoder").unwrap();
        assert_eq!(global.platform(), "qoder_global");
        assert_eq!(global.edition(), Some("qoder_global"));
    }

    #[test]
    fn workbuddy_ai_is_a_separate_provider_on_the_same_platform() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(), multi_site_auth(), None);
        let logins = read_logins_from(dir.path()).unwrap();
        let ai = logins.iter().find(|l| l.key == "workbuddy-ai").unwrap();
        assert_eq!(ai.platform(), "workbuddy", "平台仍是 workbuddy");
        assert_eq!(
            ai.account_provider(),
            Some("workbuddy_ai"),
            "provider 区分服务"
        );
        assert_eq!(ai.edition(), None);
        let cn = logins.iter().find(|l| l.key == "workbuddy").unwrap();
        assert_eq!(cn.account_provider(), Some("workbuddy"));
    }

    #[test]
    fn preview_counts_every_provider() {
        let dir = tempfile::tempdir().unwrap();
        fixture(dir.path(), multi_site_auth(), None);
        let report = preview_from(dir.path());
        assert_eq!(report["available"], json!(true));
        assert_eq!(report["counts"]["workbuddy"], json!(2));
        assert_eq!(report["counts"]["workbuddy-ai"], json!(2));
        assert_eq!(report["counts"]["qoder"], json!(2));
        assert_eq!(report["counts"]["qoder-cn"], json!(2));
        assert_eq!(report["accounts"].as_array().unwrap().len(), 8);
        let text = report.to_string();
        assert!(!text.contains("wb-1"), "preview 泄漏令牌：{text}");
        assert!(!text.contains("qd-1"));
    }

    #[test]
    fn nicknames_are_matched_per_key_across_providers() {
        let dir = tempfile::tempdir().unwrap();
        fixture(
            dir.path(),
            multi_site_auth(),
            Some(json!([
                {"id": "workbuddy", "accounts": [{"key": "workbuddy", "accountId": "国内主号"}]},
                {"id": "workbuddy-ai", "accounts": [{"key": "workbuddy-ai", "accountId": "海外号"}]},
                {"id": "qoder", "accounts": [{"key": "qoder", "accountId": "Qoder 主力"}]},
                {"id": "qoder-cn", "accounts": [{"key": "qoder-cn", "accountId": "国内 Qoder"}]}
            ])),
        );
        let logins = read_logins_from(dir.path()).unwrap();
        let name = |key: &str| {
            logins
                .iter()
                .find(|l| l.key == key)
                .unwrap()
                .nickname
                .clone()
        };
        assert_eq!(name("workbuddy").as_deref(), Some("国内主号"));
        assert_eq!(name("workbuddy-ai").as_deref(), Some("海外号"));
        assert_eq!(name("qoder").as_deref(), Some("Qoder 主力"));
        assert_eq!(name("qoder-cn").as_deref(), Some("国内 Qoder"));
    }

    #[test]
    fn short_uid_uses_the_provider_label() {
        assert_eq!(short_uid_for("Qoder CN", "abcdef123456"), "Qoder CN abcdef");
        assert_eq!(short_uid_for("WorkBuddy", "abc"), "WorkBuddy abc");
    }
}
