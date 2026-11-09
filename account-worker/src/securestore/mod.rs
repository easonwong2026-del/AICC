//! 凭据存储。
//!
//! 原则（PRD §9.2）：SQLite 只存引用，明文凭据进钥匙串；日志里永不出现明文。
//! 因此这里的每个返回值都带指纹（sha256 前 8 hex），调用方默认只能看到指纹。
//!
//! 后台助手是 ad-hoc 签名的辅助可执行文件，每次重新构建签名都会变，
//! macOS 可能因此重新询问钥匙串授权。这是预期行为，用 `denied` 区分于"没有凭据"。

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::rpc::RpcError;

pub const SERVICE: &str = "com.aieink.dashboard.menubar.account-worker";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Unavailable 只在非 macOS 后端使用，macOS 构建里不会构造
pub enum SecretErrorKind {
    NotFound,
    Denied,
    Unavailable,
    Other,
}

#[derive(Debug, Clone)]
pub struct SecretError {
    pub kind: SecretErrorKind,
    pub message: String,
    pub os_status: Option<i32>,
}

impl SecretError {
    fn new(kind: SecretErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            os_status: None,
        }
    }

    fn with_status(mut self, status: i32) -> Self {
        self.os_status = Some(status);
        self
    }
}

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl From<SecretError> for RpcError {
    fn from(err: SecretError) -> Self {
        use crate::rpc::E_SECURESTORE;
        let kind = match err.kind {
            SecretErrorKind::NotFound => "not_found",
            SecretErrorKind::Denied => "denied",
            SecretErrorKind::Unavailable => "unavailable",
            SecretErrorKind::Other => "error",
        };
        RpcError::new(E_SECURESTORE, err.message.clone()).with_data(json!({
            "kind": kind,
            "osStatus": err.os_status,
        }))
    }
}

enum Backend {
    Keychain { service: String },
    Memory(Mutex<HashMap<String, Vec<u8>>>),
}

pub struct SecureStore {
    backend: Backend,
}

impl SecureStore {
    pub fn keychain() -> Self {
        Self {
            backend: Backend::Keychain {
                service: SERVICE.to_string(),
            },
        }
    }

    /// 仅测试用：不碰真实钥匙串。
    pub fn memory() -> Self {
        Self {
            backend: Backend::Memory(Mutex::new(HashMap::new())),
        }
    }

    pub fn backend_name(&self) -> &'static str {
        match self.backend {
            Backend::Keychain { .. } => "keychain",
            Backend::Memory(_) => "memory",
        }
    }

    pub fn service_name(&self) -> Option<&str> {
        match &self.backend {
            Backend::Keychain { service } => Some(service.as_str()),
            Backend::Memory(_) => None,
        }
    }

    pub fn set(&self, account: &str, secret: &[u8]) -> Result<(), SecretError> {
        validate_account(account)?;
        match &self.backend {
            Backend::Keychain { service } => platform::set(service, account, secret),
            Backend::Memory(map) => {
                map.lock()
                    .expect("memory store")
                    .insert(account.to_string(), secret.to_vec());
                Ok(())
            }
        }
    }

    pub fn get(&self, account: &str) -> Result<Vec<u8>, SecretError> {
        validate_account(account)?;
        match &self.backend {
            Backend::Keychain { service } => platform::get(service, account),
            Backend::Memory(map) => map
                .lock()
                .expect("memory store")
                .get(account)
                .cloned()
                .ok_or_else(|| SecretError::new(SecretErrorKind::NotFound, "凭据不存在")),
        }
    }

    pub fn delete(&self, account: &str) -> Result<bool, SecretError> {
        validate_account(account)?;
        match &self.backend {
            Backend::Keychain { service } => platform::delete(service, account),
            Backend::Memory(map) => Ok(map.lock().expect("memory store").remove(account).is_some()),
        }
    }

    /// 读写删一整套探针。`health` 用它验证"凭据存储可用"，而不是等真的存账号才发现。
    pub fn probe(&self) -> Value {
        let account = format!("__probe__{}", std::process::id());
        let started = std::time::Instant::now();
        let value = format!(
            "probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );

        let result = self
            .set(&account, value.as_bytes())
            .and_then(|_| self.get(&account))
            .and_then(|read| {
                if read == value.as_bytes() {
                    Ok(())
                } else {
                    Err(SecretError::new(SecretErrorKind::Other, "回读内容不一致"))
                }
            });
        // 探针必须删掉，不能给用户留垃圾条目。
        let cleanup = self.delete(&account);

        let latency_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(()) => json!({
                "ok": true,
                "backend": self.backend_name(),
                "service": self.service_name(),
                "latencyMs": latency_ms,
                "cleanupOk": cleanup.is_ok(),
            }),
            Err(err) => json!({
                "ok": false,
                "backend": self.backend_name(),
                "service": self.service_name(),
                "latencyMs": latency_ms,
                "kind": match err.kind {
                    SecretErrorKind::NotFound => "not_found",
                    SecretErrorKind::Denied => "denied",
                    SecretErrorKind::Unavailable => "unavailable",
                    SecretErrorKind::Other => "error",
                },
                "osStatus": err.os_status,
                "message": crate::rpc::sanitize(&err.message),
            }),
        }
    }
}

fn validate_account(account: &str) -> Result<(), SecretError> {
    if account.trim().is_empty() {
        return Err(SecretError::new(SecretErrorKind::Other, "凭据键不能为空"));
    }
    if account.len() > 256 || account.chars().any(|c| c.is_control()) {
        return Err(SecretError::new(SecretErrorKind::Other, "凭据键非法"));
    }
    Ok(())
}

/// 指纹：只用来核对"是不是同一条凭据"，不可逆。
pub fn fingerprint(secret: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret);
    hasher
        .finalize()
        .iter()
        .take(4)
        .map(|b| format!("{b:02x}"))
        .collect()
}

#[cfg(target_os = "macos")]
mod platform {
    use super::{SecretError, SecretErrorKind};

    const ERR_SEC_ITEM_NOT_FOUND: i32 = -25300;
    const ERR_SEC_AUTH_FAILED: i32 = -25293;
    const ERR_SEC_USER_CANCELED: i32 = -128;

    fn map_error(err: security_framework::base::Error) -> SecretError {
        let code = err.code();
        let kind = match code {
            ERR_SEC_ITEM_NOT_FOUND => SecretErrorKind::NotFound,
            ERR_SEC_AUTH_FAILED | ERR_SEC_USER_CANCELED => SecretErrorKind::Denied,
            _ => SecretErrorKind::Other,
        };
        let message = match kind {
            SecretErrorKind::NotFound => "钥匙串中没有该凭据".to_string(),
            SecretErrorKind::Denied => "钥匙串授权被拒绝，请在系统提示中允许 AICC 访问".to_string(),
            _ => format!("钥匙串操作失败（OSStatus {code}）"),
        };
        SecretError::new(kind, message).with_status(code)
    }

    pub fn set(service: &str, account: &str, secret: &[u8]) -> Result<(), SecretError> {
        // 先删再加：set_generic_password 遇到同名条目会报错，而我们要的是"覆盖"。
        let _ = security_framework::passwords::delete_generic_password(service, account);
        security_framework::passwords::set_generic_password(service, account, secret)
            .map_err(map_error)
    }

    pub fn get(service: &str, account: &str) -> Result<Vec<u8>, SecretError> {
        security_framework::passwords::get_generic_password(service, account).map_err(map_error)
    }

    pub fn delete(service: &str, account: &str) -> Result<bool, SecretError> {
        match security_framework::passwords::delete_generic_password(service, account) {
            Ok(()) => Ok(true),
            Err(err) if err.code() == ERR_SEC_ITEM_NOT_FOUND => Ok(false),
            Err(err) => Err(map_error(err)),
        }
    }

    /// 读取第三方应用（Qoder CN）的 Safe Storage 口令。
    ///
    /// 只读，不改动对方的钥匙串条目。首次访问系统可能弹授权框，
    /// 与 88lin/qoder_signin.py 的行为一致。
    pub fn third_party_get(service: &str, account: &str) -> Result<String, SecretError> {
        let bytes = get(service, account)?;
        String::from_utf8(bytes)
            .map_err(|_| SecretError::new(SecretErrorKind::Other, "钥匙串条目不是 UTF-8 字符串"))
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::{SecretError, SecretErrorKind};

    fn unsupported() -> SecretError {
        SecretError::new(
            SecretErrorKind::Unavailable,
            "当前平台没有实现钥匙串后端（PR1a 只做 macOS）",
        )
    }

    pub fn set(_service: &str, _account: &str, _secret: &[u8]) -> Result<(), SecretError> {
        Err(unsupported())
    }

    pub fn get(_service: &str, _account: &str) -> Result<Vec<u8>, SecretError> {
        Err(unsupported())
    }

    pub fn delete(_service: &str, _account: &str) -> Result<bool, SecretError> {
        Err(unsupported())
    }

    pub fn third_party_get(_service: &str, _account: &str) -> Result<String, SecretError> {
        Err(unsupported())
    }
}

/// 读第三方钥匙串口令（Qoder CN Safe Storage）。
pub fn third_party_password(service: &str, account: &str) -> Result<String, SecretError> {
    platform::third_party_get(service, account)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_backend_roundtrip() {
        let store = SecureStore::memory();
        store.set("account-a", b"secret-value").unwrap();
        assert_eq!(store.get("account-a").unwrap(), b"secret-value");
        assert!(store.delete("account-a").unwrap());
        assert!(!store.delete("account-a").unwrap());
        assert_eq!(
            store.get("account-a").unwrap_err().kind,
            SecretErrorKind::NotFound
        );
    }

    #[test]
    fn memory_backend_overwrites() {
        let store = SecureStore::memory();
        store.set("k", b"v1").unwrap();
        store.set("k", b"v2").unwrap();
        assert_eq!(store.get("k").unwrap(), b"v2");
    }

    #[test]
    fn probe_passes_on_memory_backend_and_leaves_no_residue() {
        let store = SecureStore::memory();
        let report = store.probe();
        assert_eq!(report["ok"], true, "探针应通过：{report}");
        assert_eq!(report["cleanupOk"], true);
        let account = format!("__probe__{}", std::process::id());
        assert!(store.get(&account).is_err(), "探针条目必须被删除");
    }

    #[test]
    fn fingerprint_is_stable_and_short() {
        assert_eq!(fingerprint(b"abc"), fingerprint(b"abc"));
        assert_ne!(fingerprint(b"abc"), fingerprint(b"abd"));
        assert_eq!(fingerprint(b"abc").len(), 8);
    }

    #[test]
    fn rejects_empty_or_control_chars() {
        let store = SecureStore::memory();
        assert!(store.set("", b"x").is_err());
        assert!(store.set("bad\nkey", b"x").is_err());
    }
}
