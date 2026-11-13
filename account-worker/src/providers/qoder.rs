//! Qoder CN 适配器（PR1a：本地认证解密 + 登录态判定，不联网）。
//!
//! ## 凭据格式（macOS）
//!
//! `auth.v1.dat` 以 `v10` 开头，其后是 **AES-128-CBC** 密文：
//!
//! - IV 固定为 16 个空格（0x20）
//! - 填充 PKCS7
//! - 密钥 = PBKDF2-HMAC-SHA1(钥匙串口令, "saltysalt", 1003 轮, 16 字节)
//! - 钥匙串条目：service `Qoder CN App Safe Storage`，account `Qoder CN App Key`
//!
//! 参考实现：88lin/workbuddy-auto-signin `qoder_signin.py`（MIT）。
//! 与参考实现的差别：那边把 AES 交给 Qoder 自带的 Node 运行时，这里用 Rust
//! 直接算，所以**不依赖 Qoder 客户端进程**。
//!
//! 注意钥匙串口令是**机器级**的：它能解开本机任意一份 `auth.v1.dat`。
//! 因此"一份 auth.v1.dat = 一个账号"，多账号必须各自一份独立 profile 目录。
//!
//! 算法细节已抽到 `providers::qoder_crypto`，与 International 共用**算法**、
//! 但**不共用**密钥、路径和钥匙串条目（见 `providers::qoder_global`）。
//!
//! ## 本机复验状态
//!
//! PR #70 落地时这台机器上只装了 International 客户端，CN 没有安装，
//! 所以本文件的路径/钥匙串取值**未被本机复验**（`providers::qoder_edition`
//! 里对 CN 的 `verified_on_this_machine` 为 false）。解密算法本身有单测覆盖。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::providers::qoder_crypto::{self, CryptoError};
use crate::providers::qoder_identity::{self, Identity};
use crate::securestore::{fingerprint, third_party_password, SecretErrorKind};

pub const AUTH_FILE: &str = "auth.v1.dat";
pub const AUTH_DIR_PREFIX: &str = "com.qodercn.app";
pub const AUTH_DIR_ENV: &str = "QODER_AUTH_DIR";
pub const KEYCHAIN_SERVICE: &str = "Qoder CN App Safe Storage";
pub const KEYCHAIN_ACCOUNT: &str = "Qoder CN App Key";

/// 解密后的会话：只留本地判定需要的字段，令牌不出本模块。
#[derive(Debug, Clone)]
pub struct QoderSession {
    pub token: String,
    pub expires_at: Option<String>,
    pub fields: Vec<String>,
    pub identity: Identity,
}

#[derive(Debug)]
pub enum QoderError {
    MissingAuthDir(String),
    AmbiguousAuthDir(Vec<String>),
    MissingAuthFile(String),
    UnsupportedFormat(String),
    KeychainUnavailable(String),
    KeychainDenied(String),
    DecryptFailed(String),
    TokenMissing,
}

impl QoderError {
    pub fn state(&self) -> &'static str {
        match self {
            Self::MissingAuthDir(_) => "missing_auth_dir",
            Self::AmbiguousAuthDir(_) => "ambiguous_auth_dir",
            Self::MissingAuthFile(_) => "missing_auth_file",
            Self::UnsupportedFormat(_) => "unsupported_format",
            Self::KeychainUnavailable(_) => "keychain_unavailable",
            Self::KeychainDenied(_) => "keychain_denied",
            Self::DecryptFailed(_) => "decrypt_failed",
            Self::TokenMissing => "token_missing",
        }
    }

    pub fn reason(&self) -> String {
        match self {
            Self::MissingAuthDir(m)
            | Self::MissingAuthFile(m)
            | Self::UnsupportedFormat(m)
            | Self::KeychainUnavailable(m)
            | Self::KeychainDenied(m)
            | Self::DecryptFailed(m) => m.clone(),
            Self::AmbiguousAuthDir(dirs) => format!(
                "找到 {} 个 Qoder 配置目录，必须用 QODER_AUTH_DIR 明确指定其中一个，不能自动挑：{}",
                dirs.len(),
                dirs.join("、")
            ),
            Self::TokenMissing => "解密成功但没有 token 字段，可能是未登录或新版格式".to_string(),
        }
    }
}

fn map_crypto(err: CryptoError) -> QoderError {
    match err {
        CryptoError::UnsupportedFormat(m) => QoderError::UnsupportedFormat(m),
        CryptoError::DecryptFailed(m) => QoderError::DecryptFailed(m),
    }
}

/// 解析要检查的认证目录。
///
/// 优先级：入参 > `QODER_AUTH_DIR` > 自动扫描。
/// 自动扫描**只在恰好一个候选**时才用；多个候选直接报歧义，
/// 避免"永远选第一个"导致多账号串号（PRD §5.2）。
pub fn resolve_auth_dir(explicit: Option<&str>) -> Result<PathBuf, QoderError> {
    if let Some(dir) = explicit.map(str::trim).filter(|d| !d.is_empty()) {
        return Ok(expand_home(dir));
    }
    if let Some(dir) = std::env::var(AUTH_DIR_ENV)
        .ok()
        .map(|d| d.trim().to_string())
    {
        if !dir.is_empty() {
            return Ok(expand_home(&dir));
        }
    }
    let candidates = scan_auth_dirs();
    match candidates.len() {
        0 => Err(QoderError::MissingAuthDir(format!(
            "没有找到 Qoder CN 配置目录（{}），请先安装并登录 Qoder CN",
            scan_root().display()
        ))),
        1 => Ok(candidates[0].clone()),
        _ => Err(QoderError::AmbiguousAuthDir(
            candidates.iter().map(|p| p.display().to_string()).collect(),
        )),
    }
}

pub fn scan_root() -> PathBuf {
    crate::paths::dirs_home()
        .join("Library")
        .join("Application Support")
}

/// 扫描 `~/Library/Application Support/com.qodercn.app*`，按修改时间倒序。
pub fn scan_auth_dirs() -> Vec<PathBuf> {
    let root = scan_root();
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(AUTH_DIR_PREFIX) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        found.push((mtime, path));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// 只查"有没有装/登没登录"，**不碰钥匙串**。
///
/// `health` 每次启动都会调用它，所以绝不能触发钥匙串授权弹窗。
pub fn presence(explicit_dir: Option<&str>) -> Value {
    match resolve_auth_dir(explicit_dir) {
        Ok(dir) => {
            let file = dir.join(AUTH_FILE);
            json!({
                "platform": "qoder_cn",
                "authDir": dir.to_string_lossy(),
                "authFile": file.to_string_lossy(),
                "authFileExists": file.is_file(),
                "state": if file.is_file() { "auth_file_present" } else { "missing_auth_file" },
                "checkedKeychain": false,
            })
        }
        Err(err) => json!({
            "platform": "qoder_cn",
            "state": err.state(),
            "authDir": Value::Null,
            "authFileExists": false,
            "reason": err.reason(),
            "checkedKeychain": false,
        }),
    }
}

/// 只读检查：拿得到会话就给指纹，拿不到就给分类原因。
pub fn inspect(explicit_dir: Option<&str>) -> Value {
    let dir = match resolve_auth_dir(explicit_dir) {
        Ok(d) => d,
        Err(err) => {
            return json!({
                "platform": "qoder_cn",
                "state": err.state(),
                "authenticated": false,
                "reason": err.reason(),
                "authDir": Value::Null,
            })
        }
    };
    inspect_dir(&dir)
}

/// 对指定目录做只读检查。profile 管理走这里，避免再解析一次目录。
pub fn inspect_dir(dir: &Path) -> Value {
    match read_session(dir) {
        Ok(session) => json!({
            "platform": "qoder_cn",
            "state": "authenticated",
            "authenticated": true,
            "authDir": dir.to_string_lossy(),
            "authFile": dir.join(AUTH_FILE).to_string_lossy(),
            "tokenFingerprint": fingerprint(session.token.as_bytes()),
            "tokenLength": session.token.len(),
            "expiresAt": session.expires_at,
            "fields": session.fields,
            "uid": session.identity.uid,
            "email": session.identity.email,
            "identitySource": session.identity.source,
            "reason": Value::Null,
        }),
        Err(err) => json!({
            "platform": "qoder_cn",
            "state": err.state(),
            "authenticated": false,
            "authDir": dir.to_string_lossy(),
            "authFile": dir.join(AUTH_FILE).to_string_lossy(),
            "reason": err.reason(),
        }),
    }
}

/// 读并解密 `auth.v1.dat`。
pub fn read_session(dir: &Path) -> Result<QoderSession, QoderError> {
    let path = dir.join(AUTH_FILE);
    let raw = std::fs::read(&path)
        .map_err(|_| QoderError::MissingAuthFile(format!("{} 不存在或不可读", path.display())))?;
    if raw.len() <= qoder_crypto::FILE_MAGIC.len() {
        return Err(QoderError::UnsupportedFormat(format!(
            "{} 只有 {} 字节，可能正被客户端改写",
            AUTH_FILE,
            raw.len()
        )));
    }
    if !qoder_crypto::has_envelope(&raw) {
        return Err(QoderError::UnsupportedFormat(format!(
            "{} 前缀不是 v10（实际 {:?}），格式未支持",
            AUTH_FILE,
            String::from_utf8_lossy(&raw[..3.min(raw.len())])
        )));
    }

    let password =
        third_party_password(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(|err| match err.kind {
            SecretErrorKind::Denied => QoderError::KeychainDenied(
                "钥匙串授权被拒绝：请在系统弹窗中允许 AICC 读取 Qoder CN 的存储口令".into(),
            ),
            SecretErrorKind::NotFound => QoderError::KeychainUnavailable(format!(
                "钥匙串里没有「{KEYCHAIN_SERVICE}」，Qoder CN 可能未登录或钥匙串已锁定"
            )),
            _ => QoderError::KeychainUnavailable(err.message),
        })?;

    let plain = qoder_crypto::decrypt_envelope(&raw, &password).map_err(map_crypto)?;
    let parsed: Value = serde_json::from_slice(&plain)
        .map_err(|_| QoderError::DecryptFailed("解密成功但内容不是 JSON".into()))?;

    let fields = parsed
        .as_object()
        .map(|o| o.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    let token = parsed
        .get("token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(QoderError::TokenMissing)?;

    // 令牌字符集/长度与参考实现一致，异常形态直接判为不可用，避免拿脏数据去打接口。
    if !qoder_crypto::token_shape_ok(&token) {
        return Err(QoderError::DecryptFailed("token 形态异常".into()));
    }

    Ok(QoderSession {
        token,
        expires_at: parsed
            .get("expiresAt")
            .and_then(Value::as_str)
            .map(str::to_string),
        fields,
        identity: qoder_identity::extract(&parsed),
    })
}

/// AES-128-CBC(IV=16 空格, PKCS7) + PBKDF2-HMAC-SHA1（保留公开签名，实现已共用）。
///
/// 生产路径走 `read_session`；这个函数保留给单测和"只想验算法"的调用方。
#[allow(dead_code)]
pub fn decrypt_mac(cipher_text: &[u8], password: &str) -> Result<Vec<u8>, QoderError> {
    qoder_crypto::decrypt_cbc(cipher_text, password).map_err(map_crypto)
}

fn expand_home(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/") {
        return crate::paths::dirs_home().join(rest);
    }
    PathBuf::from(input)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decrypts_reference_style_payload() {
        let session = r#"{"token":"abc.def-123_XYZ","expiresAt":"2026-12-01T00:00:00Z"}"#;
        let file = qoder_crypto::encrypt_for_test(session.as_bytes(), "pw-from-keychain");
        assert_eq!(&file[..3], b"v10");
        let plain = decrypt_mac(&file[3..], "pw-from-keychain").unwrap();
        assert_eq!(String::from_utf8(plain).unwrap(), session);
    }

    #[test]
    fn wrong_password_fails_cleanly() {
        let file = qoder_crypto::encrypt_for_test(br#"{"token":"x"}"#, "right");
        assert!(decrypt_mac(&file[3..], "wrong").is_err());
    }

    #[test]
    fn rejects_non_block_multiple() {
        assert!(decrypt_mac(&[1u8; 17], "p").is_err());
    }

    #[test]
    fn rejects_bad_padding() {
        assert!(qoder_crypto::unpad_pkcs7(vec![0u8; 16]).is_err());
    }

    #[test]
    fn read_session_reports_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_session(dir.path()).unwrap_err();
        assert_eq!(err.state(), "missing_auth_file");
    }

    #[test]
    fn read_session_rejects_bad_magic() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(AUTH_FILE), b"v99garbage").unwrap();
        let err = read_session(dir.path()).unwrap_err();
        assert_eq!(err.state(), "unsupported_format");
    }

    #[test]
    fn explicit_dir_wins_over_env() {
        let dir = tempfile::tempdir().unwrap();
        let explicit = dir.path().join("explicit");
        std::env::set_var(AUTH_DIR_ENV, "/tmp/should-not-win");
        let resolved = resolve_auth_dir(Some(explicit.to_str().unwrap())).unwrap();
        std::env::remove_var(AUTH_DIR_ENV);
        assert_eq!(resolved, explicit);
    }

    #[test]
    fn ambiguous_scan_refuses_to_guess() {
        let err = QoderError::AmbiguousAuthDir(vec!["/a".into(), "/b".into()]);
        assert_eq!(err.state(), "ambiguous_auth_dir");
        assert!(err.reason().contains("QODER_AUTH_DIR"));
    }

    #[test]
    fn cn_and_global_never_share_keychain() {
        assert_eq!(KEYCHAIN_SERVICE, "Qoder CN App Safe Storage");
        assert_ne!(
            KEYCHAIN_SERVICE,
            crate::providers::qoder_edition::QoderEdition::Global.keychain_service()
        );
    }
}
