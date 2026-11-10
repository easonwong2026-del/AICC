//! WorkBuddy 适配器（PR1a：只读本地认证文件并如实报告能力）。
//!
//! ## 为什么不能"直接解密"
//!
//! 客户端把登录态写成 AES-GCM 信封，实测结构（2026-11，国内版桌面端）：
//!
//! ```json
//! {
//!   "auth":    { "accessToken": {"$wbEncrypted":1,"envelope":"<base64>"}, "refreshToken": {...},
//!                "expiresAt": 1762..., "domain": "..." },
//!   "account": { "uid": "...", "nickname": {"$wbEncrypted":1,...}, "uin": "..." },
//!   "accounts": [ { "uid": "...", "nickname": {...} } ]
//! }
//! ```
//!
//! 解信封的密钥由客户端自己的 Electron 原生模块提供
//! （`process._linkedBinding('electron_browser_workbuddy_storage').loggerGet()`），
//! **不在钥匙串里，也不在文件里**。所以：
//!
//! - 拿到明文 token 只有两条路：OAuth 流程直接给的明文，或者起一个 WorkBuddy
//!   运行时当凭据助手（88lin/workbuddy-signin 的做法）。
//! - 在没做这件事之前，本模块必须报 `decryptable: false`，不能假装解开了。
//!
//! PRD §9.2 里"WorkBuddy Token 存储在 Keychain"这句与实际不符，已在
//! `docs/ACCOUNT_WORKER.md` 记录。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::securestore::fingerprint;
use wb_switch_core::modules::variant::WbVariant;

/// 认证文件里的加密信封标记。
const ENVELOPE_MARKER: &str = "$wbEncrypted";
/// 解开信封需要的能力标识。
pub const REQUIRES_RUNTIME_BINDING: &str = "workbuddy-electron-native-storage";
/// 递归找信封的深度上限，防止异常文件把栈打穿。
const MAX_DEPTH: usize = 8;

/// 默认认证文件路径（国内版，真实用户主目录）。
pub fn default_auth_path() -> PathBuf {
    WbVariant::Cn.auth_file_path()
}

/// 只读检查认证文件，返回可直接进 JSON-RPC 的报告。
pub fn inspect(path: Option<&str>) -> Value {
    let path = match path {
        Some(p) if !p.trim().is_empty() => PathBuf::from(expand_home(p)),
        _ => default_auth_path(),
    };
    inspect_path(&path)
}

pub fn inspect_path(path: &Path) -> Value {
    let path_text = path.to_string_lossy().to_string();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return json!({
                "path": path_text,
                "exists": false,
                "state": "missing",
                "decryptable": false,
                "requires": REQUIRES_RUNTIME_BINDING,
                "reason": "WorkBuddy 客户端没有写出认证文件，先在本机登录一次桌面端",
            });
        }
        Err(err) => {
            return json!({
                "path": path_text,
                "exists": false,
                "state": "unreadable",
                "decryptable": false,
                "requires": REQUIRES_RUNTIME_BINDING,
                "reason": format!("读取失败：{err}"),
            });
        }
    };

    let parsed: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(_) => {
            return json!({
                "path": path_text,
                "exists": true,
                "state": "unrecognized",
                "decryptable": false,
                "requires": REQUIRES_RUNTIME_BINDING,
                "reason": "认证文件不是 JSON，可能是不支持的客户端版本",
                "sizeBytes": bytes.len(),
            });
        }
    };

    let fields: Vec<String> = parsed
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default();

    let mut envelopes: Vec<(String, Option<Value>)> = Vec::new();
    collect_envelopes(&parsed, "", &mut envelopes, 0);
    let encrypted_paths: Vec<String> = envelopes.iter().map(|(p, _)| p.clone()).collect();
    // 报"登录令牌那份"信封的元信息；昵称/手机号的信封长度没有诊断价值。
    let envelope_meta = envelopes
        .iter()
        .find(|(p, _)| p.ends_with("accessToken"))
        .or_else(|| envelopes.first())
        .and_then(|(_, m)| m.clone());

    if !encrypted_paths.is_empty() {
        let mut report = json!({
            "path": path_text,
            "exists": true,
            "sizeBytes": bytes.len(),
            "fields": fields,
            "state": "encrypted_envelope",
            "decryptable": false,
            "requires": REQUIRES_RUNTIME_BINDING,
            "reason": "登录态是 AES-GCM 信封，密钥只能由 WorkBuddy 客户端运行时提供；PR1a 不提供解密",
            "encryptedFieldPaths": encrypted_paths,
        });
        if let Some(meta) = envelope_meta {
            report["envelope"] = meta;
        }
        // 客户端自己在文件里维护了一个 accounts 数组（多账号的雏形）。
        // 只报数量，不报内容，避免把别人的账号信息带进日志。
        if let Some(list) = parsed.get("accounts").and_then(Value::as_array) {
            report["accountsArrayCount"] = json!(list.len());
        }
        return report;
    }

    if let Some((field, token)) = find_plaintext_token(&parsed) {
        return json!({
            "path": path_text,
            "exists": true,
            "sizeBytes": bytes.len(),
            "fields": fields,
            "state": "plaintext_token",
            "decryptable": true,
            "requires": Value::Null,
            "reason": "文件里有明文令牌（例如 OAuth 直接签发的形态）",
            "tokenFieldPath": field,
            // 只给指纹，明文不出这个函数。
            "tokenFingerprint": fingerprint(token.as_bytes()),
            "tokenLength": token.len(),
        });
    }

    json!({
        "path": path_text,
        "exists": true,
        "sizeBytes": bytes.len(),
        "fields": fields,
        "state": "unrecognized",
        "decryptable": false,
        "requires": REQUIRES_RUNTIME_BINDING,
        "reason": "既不是加密信封也没有明文令牌，字段结构未识别",
    })
}

/// OAuth 能力说明（PR1a 只声明，不实现）。
pub fn oauth_capability() -> Value {
    json!({
        "platform": "workbuddy",
        "variant": "cn",
        "apiEndpoint": WbVariant::Cn.api_endpoint(),
        "apiPrefix": WbVariant::Cn.api_prefix(),
        "oauthPlatform": WbVariant::Cn.oauth_platform(),
        "flow": [
            "POST {endpoint}{prefix}/auth/state?platform={oauthPlatform} -> state + authUrl",
            "用户在浏览器打开 authUrl 完成扫码/登录",
            "GET {endpoint}{prefix}/auth/token?state={state}",
            "GET {endpoint}{prefix}/login/account?state={state}",
        ],
        "implemented": false,
        "implementedIn": "PR1b",
        "requiresRealAccount": true,
        "note": "上游 wb-switch-core 已实现该流程；AICC 侧接线需要真实账号联调，PR1a 不接",
    })
}

/// 递归收集加密信封位置（`auth.accessToken` 这种字段路径），并取第一份元信息。
fn collect_envelopes(
    value: &Value,
    prefix: &str,
    out: &mut Vec<(String, Option<Value>)>,
    depth: usize,
) {
    if depth > MAX_DEPTH {
        return;
    }
    match value {
        Value::Object(map) => {
            if map.get(ENVELOPE_MARKER).and_then(Value::as_u64) == Some(1) {
                let path = if prefix.is_empty() {
                    "<root>".to_string()
                } else {
                    prefix.to_string()
                };
                let meta = map
                    .get("envelope")
                    .and_then(Value::as_str)
                    .and_then(decode_envelope_meta);
                out.push((path, meta));
                return;
            }
            for (key, child) in map {
                let next = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                collect_envelopes(child, &next, out, depth + 1);
            }
        }
        Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                // 数组下标用 [] 表示，避免和真实字段名混淆。
                let next = format!("{prefix}[{index}]");
                collect_envelopes(child, &next, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// 从信封里取出**非敏感**的元信息（suite / keyId / 长度）。
fn decode_envelope_meta(b64: &str) -> Option<Value> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let env: Value = serde_json::from_slice(&raw).ok()?;
    Some(json!({
        "suite": env.get("suite").cloned().unwrap_or(Value::Null),
        "keyId": env.get("keyId").cloned().unwrap_or(Value::Null),
        "ciphertextBytes": env
            .get("ciphertext")
            .and_then(Value::as_str)
            .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
            .map(|v| v.len()),
    }))
}

/// 在 JSON 里找明文令牌字段。只认明确的字段名，避免把普通字符串当令牌。
fn find_plaintext_token(value: &Value) -> Option<(String, String)> {
    const KEYS: &[&str] = &[
        "token",
        "accessToken",
        "access_token",
        "authToken",
        "idToken",
    ];

    fn walk(value: &Value, prefix: &str, depth: usize) -> Option<(String, String)> {
        if depth > MAX_DEPTH {
            return None;
        }
        match value {
            Value::Object(map) => {
                for (key, child) in map {
                    let next = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    if KEYS.contains(&key.as_str()) {
                        if let Some(token) = child.as_str() {
                            if !token.is_empty() {
                                return Some((next, token.to_string()));
                            }
                        }
                    }
                    if let Some(found) = walk(child, &next, depth + 1) {
                        return Some(found);
                    }
                }
                None
            }
            Value::Array(items) => items
                .iter()
                .enumerate()
                .find_map(|(i, child)| walk(child, &format!("{prefix}[{i}]"), depth + 1)),
            _ => None,
        }
    }

    walk(value, "", 0)
}

/// 只处理 `~/` 前缀，不做完整 shell 展开。
fn expand_home(input: &str) -> String {
    if let Some(rest) = input.strip_prefix("~/") {
        return crate::paths::dirs_home()
            .join(rest)
            .to_string_lossy()
            .into_owned();
    }
    input.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn write_temp(content: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("workbuddy-desktop.info");
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    fn envelope_json() -> Value {
        let envelope = json!({
            "suite": 1,
            "keyId": "0123456789abcdef",
            "nonce": base64::engine::general_purpose::STANDARD.encode([0u8; 12]),
            "authTag": base64::engine::general_purpose::STANDARD.encode([0u8; 16]),
            "ciphertext": base64::engine::general_purpose::STANDARD.encode([7u8; 32]),
        });
        json!({
            "$wbEncrypted": 1,
            "envelope": base64::engine::general_purpose::STANDARD
                .encode(serde_json::to_vec(&envelope).unwrap()),
        })
    }

    #[test]
    fn missing_file_is_reported_as_missing() {
        let dir = tempfile::tempdir().unwrap();
        let report = inspect_path(&dir.path().join("nope.info"));
        assert_eq!(report["state"], "missing");
        assert_eq!(report["decryptable"], false);
    }

    #[test]
    fn top_level_envelope_is_detected() {
        let (_dir, path) = write_temp(&serde_json::to_string(&envelope_json()).unwrap());
        let report = inspect_path(&path);
        assert_eq!(report["state"], "encrypted_envelope");
        assert_eq!(report["decryptable"], false, "不得假装已解密");
        assert_eq!(report["requires"], REQUIRES_RUNTIME_BINDING);
        assert_eq!(report["envelope"]["suite"], 1);
        assert_eq!(report["envelope"]["keyId"], "0123456789abcdef");
    }

    /// 真实客户端的结构是嵌套的：`auth.accessToken`、`account.nickname` 都是信封。
    /// 只认顶层会把明显有登录态的文件误报成 unrecognized，所以这里钉死递归行为。
    #[test]
    fn nested_client_shape_is_detected() {
        let file = json!({
            "auth": {
                "accessToken": envelope_json(),
                "refreshToken": envelope_json(),
                "tokenType": "Bearer",
                "domain": "www.codebuddy.cn",
                "expiresAt": 1762300000,
            },
            "account": {
                "uid": "1001",
                "nickname": envelope_json(),
                "uin": "1001",
            },
            "accounts": [
                {"uid": "1001", "nickname": envelope_json()},
                {"uid": "1002", "nickname": envelope_json()},
            ],
        });
        let (_dir, path) = write_temp(&serde_json::to_string(&file).unwrap());
        let report = inspect_path(&path);

        assert_eq!(report["state"], "encrypted_envelope");
        assert_eq!(report["decryptable"], false, "不得假装已解密");
        assert_eq!(report["accountsArrayCount"], 2);

        let paths: Vec<&str> = report["encryptedFieldPaths"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(paths.contains(&"auth.accessToken"), "{paths:?}");
        assert!(paths.contains(&"auth.refreshToken"), "{paths:?}");
        assert!(paths.contains(&"account.nickname"), "{paths:?}");
        assert!(paths.contains(&"accounts[0].nickname"), "{paths:?}");
        // 元信息要取登录令牌那份，而不是昵称的信封。
        assert_eq!(report["envelope"]["keyId"], "0123456789abcdef");

        // 报告里不能出现任何真实值。
        let text = report.to_string();
        assert!(!text.contains("1001"));
    }

    #[test]
    fn plaintext_token_reports_fingerprint_only() {
        let (_dir, path) = write_temp(r#"{"auth":{"token":"abcdef1234567890"},"email":"a@b.c"}"#);
        let report = inspect_path(&path);
        assert_eq!(report["state"], "plaintext_token");
        assert_eq!(report["decryptable"], true);
        assert_eq!(report["tokenLength"], 16);
        assert_eq!(report["tokenFieldPath"], "auth.token");
        assert!(
            !report.to_string().contains("abcdef1234567890"),
            "令牌明文不能出现在报告里"
        );
    }

    #[test]
    fn garbage_file_is_unrecognized() {
        let (_dir, path) = write_temp("not json");
        assert_eq!(inspect_path(&path)["state"], "unrecognized");
    }

    #[test]
    fn json_without_credentials_is_unrecognized() {
        let (_dir, path) = write_temp(r#"{"hello":"world"}"#);
        let report = inspect_path(&path);
        assert_eq!(report["state"], "unrecognized");
        assert_eq!(report["decryptable"], false);
    }

    #[test]
    fn default_path_points_at_real_client_not_switch_home() {
        // 关键回归：WB_SWITCH_HOME 只隔离上游存储，不能改变客户端认证文件位置。
        std::env::set_var("WB_SWITCH_HOME", "/tmp/aicc-vendor-path-test");
        let path = default_auth_path();
        std::env::remove_var("WB_SWITCH_HOME");
        let text = path.to_string_lossy().to_string();
        assert!(
            text.contains("Library/Application Support/CodeBuddyExtension"),
            "认证文件应位于真实用户目录，实际：{text}"
        );
        assert!(
            !text.contains("aicc-vendor-path-test"),
            "认证文件被 WB_SWITCH_HOME 带偏了：{text}"
        );
    }
}
