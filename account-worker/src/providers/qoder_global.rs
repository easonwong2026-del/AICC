//! Qoder International（Global）适配器。
//!
//! 与 CN 的差别**必须按各自证据实现**，不能因为"看起来一样"就直接复用：
//!
//! | 项 | CN | International |
//! | --- | --- | --- |
//! | Bundle ID | `com.qodercn.app` | `com.qoder.app`（本机实测） |
//! | 数据目录 | `com.qodercn.app*` | `com.qoder.app.stable`（本机实测） |
//! | 钥匙串 service | `Qoder CN App Safe Storage` | `Qoder App Safe Storage`（本机实测） |
//! | 钥匙串 account | `Qoder CN App Key` | `Qoder App Key`（本机实测） |
//! | 凭据文件 | `auth.v1.dat`（`v10` 信封） | `auth.v1.dat`（`v10` 信封，531B） |
//!
//! "信封一样"是**观察到的结果**，不是假设：`v10:` 头 + 16 字节对齐的密文在本机
//! 文件上验证过；但钥匙串口令是各版本自己的，所以这里的 `read_session` 用
//! Global 自己的 service/account 去取口令，取不到就如实报 `keychain_unavailable`。
//!
//! 本模块**不联网、不刷新令牌、不写回任何文件**。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::providers::qoder_crypto::{self, CryptoError};
use crate::providers::qoder_edition::QoderEdition;
use crate::providers::qoder_identity::{self, Identity};
use crate::securestore::{third_party_password, SecretErrorKind};

pub const AUTH_FILE: &str = "auth.v1.dat";
const EDITION: QoderEdition = QoderEdition::Global;

#[derive(Debug, Clone)]
pub struct GlobalSession {
    pub token: String,
    pub expires_at: Option<String>,
    pub fields: Vec<String>,
    pub identity: Identity,
}

#[derive(Debug)]
pub enum GlobalError {
    MissingAuthDir(String),
    AmbiguousAuthDir(Vec<String>),
    MissingAuthFile(String),
    UnsupportedFormat(String),
    KeychainUnavailable(String),
    KeychainDenied(String),
    DecryptFailed(String),
    TokenMissing,
}

impl GlobalError {
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
                "找到 {} 个 Qoder International 配置目录，必须指定具体 profile：{}",
                dirs.len(),
                dirs.join("、")
            ),
            Self::TokenMissing => "解密成功但没有 token 字段，可能是未登录或新版格式".into(),
        }
    }
}

fn map_crypto(err: CryptoError) -> GlobalError {
    match err {
        CryptoError::UnsupportedFormat(m) => GlobalError::UnsupportedFormat(m),
        CryptoError::DecryptFailed(m) => GlobalError::DecryptFailed(m),
    }
}

/// 解析要检查的目录：入参优先，其次**只在恰好一个候选**时自动选，
/// 多个候选报歧义 —— 多账号场景下"默认第一个"就是串号。
pub fn resolve_auth_dir(explicit: Option<&str>) -> Result<PathBuf, GlobalError> {
    if let Some(dir) = explicit.map(str::trim).filter(|d| !d.is_empty()) {
        return Ok(expand_home(dir));
    }
    let candidates = crate::providers::qoder_edition::find_data_dirs(EDITION);
    match candidates.len() {
        0 => Err(GlobalError::MissingAuthDir(
            "没有找到 Qoder International 配置目录，请先安装并登录桌面客户端".into(),
        )),
        1 => Ok(candidates[0].clone()),
        _ => Err(GlobalError::AmbiguousAuthDir(
            candidates.iter().map(|p| p.display().to_string()).collect(),
        )),
    }
}

fn expand_home(input: &str) -> PathBuf {
    if let Some(rest) = input.strip_prefix("~/") {
        return crate::paths::dirs_home().join(rest);
    }
    PathBuf::from(input)
}

/// 只查文件在不在，**不碰钥匙串**（`health` 每次启动都会问一次）。
pub fn presence(explicit_dir: Option<&str>) -> Value {
    match resolve_auth_dir(explicit_dir) {
        Ok(dir) => {
            let file = dir.join(AUTH_FILE);
            let exists = file.is_file();
            json!({
                "platform": EDITION.as_str(),
                "authDir": dir.to_string_lossy(),
                "authFile": file.to_string_lossy(),
                "authFileExists": exists,
                "state": if exists { "auth_file_present" } else { "missing_auth_file" },
                "checkedKeychain": false,
            })
        }
        Err(err) => json!({
            "platform": EDITION.as_str(),
            "state": err.state(),
            "authDir": Value::Null,
            "authFileExists": false,
            "reason": err.reason(),
            "checkedKeychain": false,
        }),
    }
}

/// 只读检查：能解出会话就给指纹与身份，解不出就给分类原因。
pub fn inspect(explicit_dir: Option<&str>) -> Value {
    let dir = match resolve_auth_dir(explicit_dir) {
        Ok(d) => d,
        Err(err) => {
            return json!({
                "platform": EDITION.as_str(),
                "state": err.state(),
                "authenticated": false,
                "reason": err.reason(),
                "authDir": Value::Null,
            })
        }
    };
    inspect_dir(&dir)
}

pub fn inspect_dir(dir: &Path) -> Value {
    match read_session(dir) {
        Ok(session) => json!({
            "platform": EDITION.as_str(),
            "state": "authenticated",
            "authenticated": true,
            "authDir": dir.to_string_lossy(),
            "authFile": dir.join(AUTH_FILE).to_string_lossy(),
            "tokenFingerprint": crate::securestore::fingerprint(session.token.as_bytes()),
            "tokenLength": session.token.len(),
            "expiresAt": session.expires_at,
            "fields": session.fields,
            "uid": session.identity.uid,
            "email": session.identity.email,
            "identitySource": session.identity.source,
            "reason": Value::Null,
        }),
        Err(err) => json!({
            "platform": EDITION.as_str(),
            "state": err.state(),
            "authenticated": false,
            "authDir": dir.to_string_lossy(),
            "authFile": dir.join(AUTH_FILE).to_string_lossy(),
            "reason": err.reason(),
        }),
    }
}

pub fn read_session(dir: &Path) -> Result<GlobalSession, GlobalError> {
    let path = dir.join(AUTH_FILE);
    let raw = std::fs::read(&path)
        .map_err(|_| GlobalError::MissingAuthFile(format!("{} 不存在或不可读", path.display())))?;
    if raw.len() <= qoder_crypto::FILE_MAGIC.len() {
        return Err(GlobalError::UnsupportedFormat(format!(
            "{AUTH_FILE} 只有 {} 字节，可能正被客户端改写",
            raw.len()
        )));
    }
    // 先看信封再问钥匙串：格式不认识就没必要弹授权框。
    if !qoder_crypto::has_envelope(&raw) {
        return Err(GlobalError::UnsupportedFormat(format!(
            "{AUTH_FILE} 前缀不是 v10（实际 {:?}），格式未支持",
            String::from_utf8_lossy(&raw[..3.min(raw.len())])
        )));
    }

    let password = third_party_password(EDITION.keychain_service(), EDITION.keychain_account())
        .map_err(|err| match err.kind {
            SecretErrorKind::Denied => GlobalError::KeychainDenied(
                "钥匙串授权被拒绝：请在系统弹窗中允许 AICC 读取 Qoder International 的存储口令"
                    .into(),
            ),
            SecretErrorKind::NotFound => GlobalError::KeychainUnavailable(format!(
                "钥匙串里没有「{}」，Qoder International 可能未登录或钥匙串已锁定",
                EDITION.keychain_service()
            )),
            _ => GlobalError::KeychainUnavailable(err.message),
        })?;

    let plain = qoder_crypto::decrypt_envelope(&raw, &password).map_err(map_crypto)?;
    let parsed: Value = serde_json::from_slice(&plain)
        .map_err(|_| GlobalError::DecryptFailed("解密成功但内容不是 JSON".into()))?;

    let fields = parsed
        .as_object()
        .map(|o| o.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();

    let token = parsed
        .get("token")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or(GlobalError::TokenMissing)?;
    if !qoder_crypto::token_shape_ok(&token) {
        return Err(GlobalError::DecryptFailed("token 形态异常".into()));
    }

    let expires_at = parsed
        .get("expiresAt")
        .or_else(|| parsed.get("expires_at"))
        .or_else(|| parsed.get("expiry"))
        .and_then(Value::as_str)
        .map(str::to_string);

    let identity = qoder_identity::extract(&parsed);

    Ok(GlobalSession {
        token,
        expires_at,
        fields,
        identity,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_auth(dir: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join(AUTH_FILE), bytes).unwrap();
    }

    #[test]
    fn uses_global_keychain_not_cn() {
        assert_eq!(EDITION.keychain_service(), "Qoder App Safe Storage");
        assert_ne!(
            EDITION.keychain_service(),
            QoderEdition::Cn.keychain_service()
        );
    }

    #[test]
    fn missing_file_reports_state() {
        let dir = tempfile::tempdir().unwrap();
        let err = read_session(dir.path()).unwrap_err();
        assert_eq!(err.state(), "missing_auth_file");
    }

    #[test]
    fn bad_magic_reports_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(dir.path(), b"v99garbagegarbage");
        let err = read_session(dir.path()).unwrap_err();
        assert_eq!(err.state(), "unsupported_format");
    }

    #[test]
    fn tiny_file_is_not_decrypted() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(dir.path(), b"v10");
        let err = read_session(dir.path()).unwrap_err();
        assert_eq!(err.state(), "unsupported_format");
    }

    #[test]
    fn presence_never_touches_keychain() {
        let dir = tempfile::tempdir().unwrap();
        write_auth(dir.path(), b"v10xxxxxxxxxxxxxxxx");
        let value = presence(Some(&dir.path().to_string_lossy()));
        assert_eq!(value["state"], "auth_file_present");
        assert_eq!(value["checkedKeychain"], false);
        assert_eq!(value["platform"], "qoder_global");
    }

    #[test]
    fn ambiguous_dirs_refuse_to_guess() {
        let err = GlobalError::AmbiguousAuthDir(vec!["/a".into(), "/b".into()]);
        assert_eq!(err.state(), "ambiguous_auth_dir");
    }
}
