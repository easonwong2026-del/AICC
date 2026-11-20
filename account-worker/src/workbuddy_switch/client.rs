//! WorkBuddy 客户端的两件事：**认证文件**与**进程启停**。
//!
//! 这一层刻意很薄，而且**全部可注入**：账号切换会关掉用户正在用的客户端，
//! 单元测试绝不能真去 `pkill WorkBuddy`。所以进程控制抽成 [`ClientControl`]，
//! 认证文件路径由调用方给（生产用 `WbVariant::auth_file_path()`，测试用临时目录）。
//!
//! ## 认证文件长什么样（2026-11 实测，国内版）
//!
//! ```json
//! {
//!   "auth":    { "accessToken": {"$wbEncrypted":1,...}, "refreshToken": {...},
//!                "tokenType":"Bearer", "domain":"www.workbuddy.cn",
//!                "expiresAt":1797552874249, ... },
//!   "account":  { "uid":"...", "nickname": {"$wbEncrypted":1,...}, ... },
//!   "accounts": [ {...} ],       // 客户端自己的多账号列表
//!   "allAccounts": [ {...} ]
//! }
//! ```
//!
//! 关键事实：**列表里只有身份字段，没有令牌；令牌只有当前账号那一份**
//! （`auth.accessToken` / `auth.refreshToken`）。所以切换 = 换掉 `auth` 里的令牌
//! ＋ 把 `account` 指向目标账号，不重启客户端它不会重读。
//!
//! 令牌字段可能是明文串，也可能是信封对象。AICC 自有 OAuth 拿到的是明文，
//! 所以这里写出去的是明文；客户端启动后会自己重新封装（写后校验与"客户端是否
//! 改写文件"都在这一层观察，见 [`LiveFile::sha256`]）。

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::Value;
use sha2::{Digest, Sha256};

use wb_switch_core::modules::variant::WbVariant;

/// 客户端"已退出"标记：文件名在完整认证名后追加，不替换 `.info` 扩展名。
pub const LOGOUT_SUFFIX: &str = ".logged-out";

/// 校验窗口默认值：客户端启动后至少观察这么久，防止"刚写进去就被客户端改回去"。
pub const VERIFY_SETTLE_MS: u64 = 2_500;
/// 校验总预算。冷启动的客户端可能很慢，超时只报"未确认"，不回滚。
pub const VERIFY_BUDGET_MS: u64 = 25_000;

/// 认证文件的一次快照（只读观察结果）。
#[derive(Debug, Clone)]
pub struct LiveFile {
    pub exists: bool,
    pub uid: Option<String>,
    /// 令牌字段的形态：`plaintext` / `envelope` / `mixed` / `None`（没读到）。
    pub token_form: Option<&'static str>,
    /// 令牌摘要（8 hex）。**不返回明文**，只用于"还是不是同一份令牌"。
    pub token_fingerprint: Option<String>,
    pub size: u64,
    pub modified_ms: Option<i64>,
    /// 客户端留下了退出标记时，即使文件里有凭据也不算已登录。
    pub logout_marker: bool,
    pub accounts_len: usize,
    pub sha256: Option<String>,
    pub parse_error: Option<String>,
    /// 解析出来的完整内容。调用方要复用它（例如沿用客户端自己那份 account 对象）。
    pub json: Option<Value>,
}

impl LiveFile {
    pub fn missing(path: &Path) -> Self {
        Self {
            exists: false,
            uid: None,
            token_form: None,
            token_fingerprint: None,
            size: 0,
            modified_ms: None,
            logout_marker: logout_marker_path(path).exists(),
            accounts_len: 0,
            sha256: None,
            parse_error: None,
            json: None,
        }
    }

    /// 文件里有凭据、没有退出标记、UID 也读得到 —— 才算"有一个可报告的登录态"。
    pub fn logged_in(&self) -> bool {
        self.uid.is_some() && self.token_form.is_some() && !self.logout_marker
    }
}

/// 客户端进程控制。生产实现见 [`SystemControl`]，测试用假实现。
pub trait ClientControl {
    fn variant(&self) -> WbVariant;
    fn app_path(&self) -> PathBuf;
    /// 客户端是否装在这台机器上（不可用时切换入口应当直接禁用）。
    fn installed(&self) -> bool;
    fn is_running(&self) -> bool;
    /// 关闭客户端并等到进程真的消失。失败要返回可读原因。
    fn close(&self, timeout_secs: i64) -> Result<(), String>;
    fn launch(&self) -> Result<(), String>;
    /// 轮询间隔。测试里覆盖成 no-op，避免真的等 25 秒。
    fn sleep_ms(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    /// `(稳定观察窗口, 总预算)` 毫秒。测试覆盖成几十毫秒，保持用例是秒级。
    fn verify_window(&self) -> (u64, u64) {
        (VERIFY_SETTLE_MS, VERIFY_BUDGET_MS)
    }
}

/// 真实客户端：启动/关闭走 vendored 上游实现（它已经处理了 bundle id、大小写、
/// 包内守护子进程这些坑，见 `wb-switch-core::modules::process`）。
pub struct SystemControl {
    variant: WbVariant,
}

impl SystemControl {
    pub fn for_variant(variant: WbVariant) -> Self {
        Self { variant }
    }
}

impl ClientControl for SystemControl {
    fn variant(&self) -> WbVariant {
        self.variant
    }

    fn app_path(&self) -> PathBuf {
        wb_switch_core::modules::auth_file::workbuddy_app_path(self.variant)
    }

    fn installed(&self) -> bool {
        self.app_path().exists()
    }

    fn is_running(&self) -> bool {
        wb_switch_core::modules::process::is_workbuddy_running(self.variant)
    }

    fn close(&self, timeout_secs: i64) -> Result<(), String> {
        wb_switch_core::modules::process::close_workbuddy(self.variant, timeout_secs)
    }

    fn launch(&self) -> Result<(), String> {
        wb_switch_core::modules::process::launch_workbuddy(self.variant, None)
    }
}

/// 退出标记路径：在完整文件名后追加后缀。
pub fn logout_marker_path(auth_path: &Path) -> PathBuf {
    let mut marker = auth_path.as_os_str().to_os_string();
    marker.push(LOGOUT_SUFFIX);
    PathBuf::from(marker)
}

/// 读一次客户端认证文件。**任何失败都变成字段**，不抛错：读不到本身就是要报告的事实。
pub fn read_live(path: &Path) -> LiveFile {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) => {
            let mut missing = LiveFile::missing(path);
            if err.kind() != std::io::ErrorKind::NotFound {
                missing.parse_error = Some(format!("读取失败：{err}"));
            }
            return missing;
        }
    };

    let mut live = LiveFile::missing(path);
    live.exists = true;
    live.size = bytes.len() as u64;
    live.sha256 = Some(sha256_hex(&bytes));
    live.modified_ms = std::fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64);

    let root: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(err) => {
            live.parse_error = Some(format!("不是合法 JSON：{err}"));
            return live;
        }
    };

    live.uid = uid_of(&root);
    live.token_form = token_form(&root);
    live.token_fingerprint = token_fingerprint(&root);
    live.accounts_len = account_list(&root).len();
    live.json = Some(root);
    live
}

/// 从认证文件内容里取当前登录的 uid。`account.uid` 优先，兼容根级旧写法。
pub fn uid_of(root: &Value) -> Option<String> {
    root.get("account")
        .and_then(|account| account.get("uid"))
        .and_then(Value::as_str)
        .or_else(|| root.get("uid").and_then(Value::as_str))
        .map(str::to_string)
}

/// `auth.accessToken` 的形态。本函数只看形态，不解密。
pub fn token_form(root: &Value) -> Option<&'static str> {
    let access = root.get("auth").and_then(|auth| auth.get("accessToken"));
    let refresh = root.get("auth").and_then(|auth| auth.get("refreshToken"));
    let kind = |value: Option<&Value>| match value {
        Some(Value::String(text)) if !text.trim().is_empty() => Some("plaintext"),
        Some(Value::Object(map)) if map.get("$wbEncrypted").and_then(Value::as_u64) == Some(1) => {
            Some("envelope")
        }
        _ => None,
    };
    match (kind(access), kind(refresh)) {
        (Some(a), Some(b)) if a == b => Some(a),
        (Some(_), Some(_)) => Some("mixed"),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

/// 令牌摘要。信封形态按整段 JSON 取摘要（它本身已经是密文）。
fn token_fingerprint(root: &Value) -> Option<String> {
    let access = root.get("auth")?.get("accessToken")?;
    let bytes = match access {
        Value::String(text) if !text.trim().is_empty() => text.as_bytes().to_vec(),
        Value::Object(_) => serde_json::to_vec(access).ok()?,
        _ => return None,
    };
    Some(sha256_hex(&bytes).chars().take(8).collect())
}

/// 客户端自己的账号列表。两个键名在不同版本里都用过，读的顺序固定。
pub fn account_list(root: &Value) -> Vec<Value> {
    root.get("allAccounts")
        .or_else(|| root.get("accounts"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// 在客户端列表里按 uid 找那一条账号对象（丢掉昵称信封就等于丢信息，不能重建）。
pub fn find_account_entry<'a>(root: &'a Value, uid: &str) -> Option<&'a Value> {
    let list = root.get("allAccounts").or_else(|| root.get("accounts"))?;
    list.as_array()?
        .iter()
        .find(|item| item.get("uid").and_then(Value::as_str) == Some(uid))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 原子写：临时文件 `0600` → fsync → rename → fsync 目录。
///
/// 认证文件里是新写入的明文令牌，权限必须是 `0600`。`std::fs::write` 建出来的
/// 是 `0644`（受 umask 影响），rename 之后权限就跟着过去了 —— 这里不能省。
pub fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)?;
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "auth.info".to_string());
    let tmp = parent.join(format!(
        ".{file_name}.aicc-tmp-{}",
        uuid::Uuid::new_v4().simple()
    ));

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        if let Err(err) = file.write_all(bytes).and_then(|_| file.sync_all()) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
    }
    #[cfg(not(unix))]
    {
        if let Err(err) = std::fs::write(&tmp, bytes) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
    }

    if let Err(err) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    // 目录项也要落盘，否则断电后可能只剩一个空目录项。
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let dir = std::env::temp_dir()
                .join(format!("aicc-wb-client-{}", uuid::Uuid::new_v4().simple()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn auth(&self) -> PathBuf {
            self.0.join("workbuddy-desktop.info")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn missing_file_is_reported_not_raised() {
        let dir = TempDir::new();
        let live = read_live(&dir.auth());
        assert!(!live.exists);
        assert!(live.uid.is_none());
        assert!(live.parse_error.is_none());
        assert!(!live.logged_in());
    }

    #[test]
    fn envelope_and_plaintext_forms_are_distinguished() {
        let dir = TempDir::new();
        let envelope = json!({"$wbEncrypted": 1, "envelope": "abc"});

        std::fs::write(
            dir.auth(),
            serde_json::to_vec(&json!({
                "auth": {"accessToken": envelope, "refreshToken": envelope},
                "account": {"uid": "u-1"},
            }))
            .unwrap(),
        )
        .unwrap();
        let live = read_live(&dir.auth());
        assert_eq!(live.token_form, Some("envelope"));
        assert_eq!(live.uid.as_deref(), Some("u-1"));
        assert!(live.logged_in());

        std::fs::write(
            dir.auth(),
            serde_json::to_vec(&json!({
                "auth": {"accessToken": "AT", "refreshToken": "RT"},
                "account": {"uid": "u-2"},
            }))
            .unwrap(),
        )
        .unwrap();
        let live = read_live(&dir.auth());
        assert_eq!(live.token_form, Some("plaintext"));
        assert_eq!(live.token_fingerprint.as_ref().map(String::len), Some(8));
    }

    /// 退出标记存在时，文件里有凭据也不能算已登录 —— 客户端自己就是这么判的。
    #[test]
    fn logout_marker_wins_over_credentials() {
        let dir = TempDir::new();
        std::fs::write(
            dir.auth(),
            serde_json::to_vec(&json!({
                "auth": {"accessToken": "AT", "refreshToken": "RT"},
                "account": {"uid": "u-1"},
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(logout_marker_path(&dir.auth()), "logged-out").unwrap();

        let live = read_live(&dir.auth());
        assert!(live.logout_marker);
        assert!(!live.logged_in());
    }

    #[test]
    fn bad_json_keeps_size_and_sha() {
        let dir = TempDir::new();
        std::fs::write(dir.auth(), b"not json").unwrap();
        let live = read_live(&dir.auth());
        assert!(live.exists);
        assert!(live.parse_error.is_some());
        assert_eq!(live.size, 8);
        assert!(live.sha256.is_some());
        assert!(!live.logged_in());
    }

    /// 写入必须是 0600：里面是新写的明文令牌。
    #[cfg(unix)]
    #[test]
    fn private_write_uses_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new();
        let path = dir.auth();
        write_private(&path, b"{\"a\":1}").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "认证文件权限应为 0600，实际 {mode:o}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{\"a\":1}");
    }

    #[test]
    fn account_entry_lookup_prefers_all_accounts() {
        let root = json!({
            "accounts": [{"uid": "old", "nickname": "旧"}],
            "allAccounts": [{"uid": "new", "nickname": {"$wbEncrypted": 1}}],
        });
        let entry = find_account_entry(&root, "new").expect("应当命中最新的列表");
        assert!(entry["nickname"].is_object());
        assert!(find_account_entry(&root, "missing").is_none());
    }
}
