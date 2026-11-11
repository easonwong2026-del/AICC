//! 从 Magpie 借用 WorkBuddy 登录态。
//!
//! Magpie 自己完成了 WorkBuddy 的 OAuth，并在 `~/.config/magpie/plugin-auth.json`
//! 里按账号保存了 `access` / `refresh` / `uid` / `expires`（明文 JSON）。
//!
//! AICC 只**读**这个文件，并且**只借 access token**：
//!
//! * access 是无状态 JWT，多端同时用是正常行为，不会影响 Magpie；
//! * refresh 是有状态的单份凭据，两边各自刷新会互相踢下线 —— 所以这里
//!   既不读取 refresh，也不写入 AICC 的钥匙串，过期后重新从 Magpie 同步。
//!
//! 这样即便 Magpie 被 AICC 的导入完全忽略，Magpie 也照旧工作；反过来
//! 用户哪天从 Magpie 里删掉账号，AICC 里那条只会在令牌过期后变成"已过期"。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::providers::workbuddy_auth::ms_to_rfc3339;

/// 账号来源标记，与 `storage::AccountRecord::auth_source` 对应。
pub const SOURCE_MAGPIE: &str = "magpie";

const AUTH_FILE: &str = "plugin-auth.json";
const PROVIDERS_FILE: &str = "plugin-providers.json";
const KEY_PREFIX: &str = "workbuddy";

#[derive(Debug, Clone)]
pub struct MagpieLogin {
    /// Magpie 内部的账号键：`workbuddy`、`workbuddy#2ntprc`……
    pub key: String,
    pub uid: String,
    pub nickname: Option<String>,
    pub domain: String,
    pub access_token: String,
    pub expires_at_ms: Option<i64>,
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
        // 只关心 workbuddy 及其分账号（`workbuddy#xxxxxx`）。
        let is_workbuddy = key == KEY_PREFIX || key.starts_with(&format!("{KEY_PREFIX}#"));
        if !is_workbuddy {
            continue;
        }
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

    out.sort_by(|a, b| a.key.cmp(&b.key));
    if out.is_empty() {
        return Err(MagpieError::Unreadable(
            "Magpie 里没有已登录的 WorkBuddy 账号".to_string(),
        ));
    }
    Ok(out)
}

/// 昵称不在 `plugin-auth.json` 里，而在 `plugin-providers.json` 的
/// `accounts[].accountId`（就是界面上看到的"卡里""一锅端"）。按 `key` 对齐。
fn read_nicknames(path: &Path) -> HashMap<String, Option<String>> {
    let mut out = HashMap::new();
    let Ok(value) = read_json(path) else {
        return out;
    };
    let Some(providers) = value.as_array() else {
        return out;
    };
    for provider in providers {
        if provider.get("id").and_then(Value::as_str) != Some(KEY_PREFIX) {
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
        "platform": "workbuddy",
        "uid": login.uid,
        "nickname": login.nickname.clone().unwrap_or_else(|| short_uid(&login.uid)),
        "domain": login.domain,
        "expiresAt": login.expires_at_ms.and_then(ms_to_rfc3339),
        "expiresInHours": login
            .expires_at_ms
            .map(|at| ((at - now_ms) as f64 / 3_600_000.0 * 10.0).round() / 10.0),
        "expired": expired,
        "source": SOURCE_MAGPIE,
    })
}

/// 昵称兜底：`WorkBuddy ab12cd`，比整串 UUID 好认。
pub fn short_uid(uid: &str) -> String {
    let head: String = uid.chars().take(6).collect();
    if head.len() < uid.len() {
        format!("WorkBuddy {head}")
    } else {
        format!("WorkBuddy {uid}")
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
            json!({
                "available": true,
                "configDir": dir.to_string_lossy(),
                "accounts": accounts,
            })
        }
        Err(err) => json!({
            "available": false,
            "configDir": dir.to_string_lossy(),
            "kind": err.kind(),
            "message": err.message(),
            "accounts": [],
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
}
