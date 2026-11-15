//! AICC 自有授权的本地 JSON 存储（PR-CRED-1）。
//!
//! 一份独立授权一个文件：`<data_dir>/secrets/v1/<uuid>.json`。同一个账号同时有
//! 浏览器授权和 PAT 时是两个文件，互不覆盖。
//!
//! # 为什么是明文 JSON
//!
//! 钥匙串的授权弹窗绑在**二进制签名**上，ad-hoc 签名的 Worker 每次重建都会让用户
//! 重新点一次"始终允许"，后台任务因此常年卡在授权框上。PRD 选择用本地明文文件
//! 换掉这条链路：`0700` / `0600` 能挡住同机其他普通用户，但挡不住同一 macOS
//! 用户权限下的恶意程序。这不是加密存储，首次使用必须向用户明示风险。
//!
//! # 写路径的三道闸
//!
//! 1. **跨进程文件锁**（`secrets/v1/.lock`，advisory `flock`）：Worker 与 UI
//!    可能同时想刷新同一份令牌，谁先拿到锁谁写。
//! 2. **`revision` 检查**：拿旧快照去覆盖新令牌的一律 [`CredentialErrorKind::Conflict`]，
//!    一个字节都不写。
//! 3. **同目录临时文件 + `rename`**：目标文件要么是旧内容、要么是新内容，
//!    不存在"写了一半"的中间态；崩溃只会留下 `.tmp-*` 垃圾，由 [`LocalJsonStore::sweep_orphans`] 清掉。
//!
//! # 读路径的硬规定
//!
//! 读失败**不许**降级成"没有凭据"：`Denied`（没权限）与 `NotFound`（真没有）
//! 必须分开，否则上层的兜底逻辑会以为该账号没授权，进而去碰钥匙串。

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::reference::{self, CredentialSlot};
use super::{CredentialError, CredentialErrorKind};

/// 文件结构版本。读到更高的版本要报 [`CredentialErrorKind::Unsupported`]，不能猜。
pub const SCHEMA_VERSION: u32 = 1;
/// `secrets` 目录名（挂在数据根下）。
pub const SECRETS_DIR: &str = "secrets";
/// 存储版本子目录：将来换布局时并存，不原地改写。
pub const SCHEMA_DIR: &str = "v1";
/// 凭据目录权限：只有属主可进入 / 列目录。
pub const SECRETS_MODE: u32 = 0o700;
/// 凭据文件权限：只有属主可读写。
pub const SECRET_MODE: u32 = 0o600;
/// 跨进程锁文件名。
pub const LOCK_FILE: &str = ".lock";
/// 临时文件前缀。写在同目录，`rename` 才是原子的。
const TEMP_PREFIX: &str = ".tmp-";
/// 等锁的默认上限：刷新是短操作，2 秒还没轮到就说明对方卡住了，报错比挂死好。
pub const DEFAULT_LOCK_TIMEOUT: Duration = Duration::from_millis(2_000);
const LOCK_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// 授权来源。当前只有 `own`：Magpie 的令牌**绝不**落盘（PRD §3.2）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretSource {
    /// AICC 浏览器 / 扫码 / PAT 授权。
    Own,
}

/// 凭据类型。PAT 是独立类型：**不允许**自动刷新。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CredentialKind {
    /// OAuth：access + 可选 refresh，两者作为一组原子更新。
    Oauth,
    /// 个人访问令牌：只有一个值，刷新等于重新登录。
    Pat,
}

/// 文件里的凭据段。
///
/// `Debug` 是手写的：派生的 `Debug` 会把令牌写进日志、异常和诊断包，
/// 而这几条路径都不受本模块控制。
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Credentials {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
}

impl Credentials {
    pub fn access_only(access: impl Into<String>) -> Self {
        Self {
            access_token: Some(access.into()),
            refresh_token: None,
        }
    }

    pub fn oauth(access: impl Into<String>, refresh: impl Into<String>) -> Self {
        Self {
            access_token: Some(access.into()),
            refresh_token: Some(refresh.into()),
        }
    }

    fn token(&self, slot: CredentialSlot) -> Option<&str> {
        let value = match slot {
            CredentialSlot::Access => self.access_token.as_deref(),
            CredentialSlot::Refresh => self.refresh_token.as_deref(),
        };
        value.filter(|text| !text.trim().is_empty())
    }

    fn validate(&self, credential_kind: CredentialKind) -> Result<(), CredentialError> {
        match self.token(CredentialSlot::Access) {
            Some(_) => {}
            None => {
                return Err(CredentialError::new(
                    CredentialErrorKind::Unsupported,
                    "凭据缺少访问令牌",
                ))
            }
        }
        if self.access_token.as_deref().is_some_and(has_control_chars) {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsupported,
                "访问令牌含控制字符",
            ));
        }
        if let Some(refresh) = self.refresh_token.as_deref() {
            if refresh.trim().is_empty() {
                return Err(CredentialError::new(
                    CredentialErrorKind::Unsupported,
                    "刷新令牌为空字符串",
                ));
            }
            if has_control_chars(refresh) {
                return Err(CredentialError::new(
                    CredentialErrorKind::Unsupported,
                    "刷新令牌含控制字符",
                ));
            }
        }
        if credential_kind == CredentialKind::Pat && self.refresh_token.is_some() {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsupported,
                "PAT 不允许携带刷新令牌",
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 只给"有没有、多长、指纹"，够排查，不够还原。
        f.debug_struct("Credentials")
            .field("access", &self.describe(CredentialSlot::Access))
            .field("refresh", &self.describe(CredentialSlot::Refresh))
            .finish()
    }
}

impl Credentials {
    fn describe(&self, slot: CredentialSlot) -> String {
        match self.token(slot) {
            Some(token) => format!(
                "len={} fp={}",
                token.chars().count(),
                crate::securestore::fingerprint(token.as_bytes())
            ),
            None => "none".to_string(),
        }
    }
}

/// 一个凭据文件的内容。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StoredSecret {
    pub schema_version: u32,
    /// 随机 UUID，同时也是文件名。**不**用 UID / 邮箱 / 昵称命名。
    pub secret_id: String,
    /// 对应的 `account_authorizations.id`（或 Qoder 授权的行 ID）。
    pub authorization_id: String,
    pub platform: String,
    pub source: SecretSource,
    pub credential_kind: CredentialKind,
    /// 从 0 开始，每次成功写入 +1。旧快照据此作废。
    pub revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    pub credentials: Credentials,
}

impl StoredSecret {
    /// 取指定槽位的令牌。缺这个槽位是 `NotFound`，不是空串。
    pub fn token(&self, slot: CredentialSlot) -> Result<&str, CredentialError> {
        self.credentials.token(slot).ok_or_else(|| {
            CredentialError::new(
                CredentialErrorKind::NotFound,
                format!("凭据 {} 没有 {} 令牌", self.secret_id, slot),
            )
        })
    }

    /// 令牌指纹（sha256 前 8 hex）：核对"是不是同一条"用，不可逆。
    pub fn fingerprint(&self, slot: CredentialSlot) -> Option<String> {
        self.credentials
            .token(slot)
            .map(|token| crate::securestore::fingerprint(token.as_bytes()))
    }

    /// 是否已过期（含调用方给的余量）。没有 `expiresAt` 一律按"未过期"处理：
    /// 缺信息不等于已失效，真失效由接口 401 判出来。
    pub fn is_expired_within(&self, skew: chrono::Duration) -> bool {
        let Some(raw) = self.expires_at.as_deref() else {
            return false;
        };
        match DateTime::parse_from_rfc3339(raw) {
            Ok(at) => at.timestamp() <= Utc::now().timestamp() + skew.num_seconds(),
            Err(_) => false,
        }
    }

    fn validate(&self) -> Result<(), CredentialError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsupported,
                format!("凭据文件结构版本不支持：{}", self.schema_version),
            ));
        }
        if !reference::valid_secret_id(&self.secret_id) {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsafe,
                "凭据 ID 非法",
            ));
        }
        if self.authorization_id.trim().is_empty()
            || self.authorization_id.chars().any(|c| c.is_control())
        {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsupported,
                "授权 ID 为空或含控制字符",
            ));
        }
        validate_platform(&self.platform)?;
        if let Some(raw) = self.expires_at.as_deref() {
            if DateTime::parse_from_rfc3339(raw).is_err() {
                return Err(CredentialError::new(
                    CredentialErrorKind::Unsupported,
                    "过期时间不是 RFC3339",
                ));
            }
        }
        self.credentials.validate(self.credential_kind)
    }
}

/// 新建凭据用的草稿：`secretId` 与 `revision` 由存储生成，调用方不能指定。
///
/// `Debug` 手写脱敏，理由同 [`Credentials`]。
#[derive(Clone)]
pub struct SecretDraft {
    pub authorization_id: String,
    pub platform: String,
    pub source: SecretSource,
    pub credential_kind: CredentialKind,
    pub expires_at: Option<DateTime<Utc>>,
    pub access_token: String,
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for SecretDraft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretDraft")
            .field("authorization_id", &self.authorization_id)
            .field("platform", &self.platform)
            .field("source", &self.source)
            .field("credential_kind", &self.credential_kind)
            .field("expires_at", &self.expires_at)
            .field("access", &"(redacted)")
            .field(
                "refresh",
                &if self.refresh_token.is_some() {
                    "(redacted)"
                } else {
                    "none"
                },
            )
            .finish()
    }
}

impl SecretDraft {
    /// OAuth 授权（WorkBuddy 浏览器 / 扫码、Qoder 设备授权）。
    pub fn oauth(
        authorization_id: impl Into<String>,
        platform: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Self {
        Self {
            authorization_id: authorization_id.into(),
            platform: platform.into(),
            source: SecretSource::Own,
            credential_kind: CredentialKind::Oauth,
            expires_at: None,
            access_token: access_token.into(),
            refresh_token: None,
        }
    }

    /// PAT 授权：只有一个值，不带 refresh。
    pub fn pat(
        authorization_id: impl Into<String>,
        platform: impl Into<String>,
        token: impl Into<String>,
    ) -> Self {
        Self {
            credential_kind: CredentialKind::Pat,
            ..Self::oauth(authorization_id, platform, token)
        }
    }

    pub fn with_refresh_token(mut self, refresh: impl Into<String>) -> Self {
        self.refresh_token = Some(refresh.into());
        self
    }

    pub fn with_expires_at(mut self, at: DateTime<Utc>) -> Self {
        self.expires_at = Some(at);
        self
    }

    fn into_record(self, secret_id: String) -> Result<StoredSecret, CredentialError> {
        let record = StoredSecret {
            schema_version: SCHEMA_VERSION,
            secret_id,
            authorization_id: self.authorization_id,
            platform: self.platform,
            source: self.source,
            credential_kind: self.credential_kind,
            revision: 0,
            expires_at: self
                .expires_at
                .map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true)),
            credentials: Credentials {
                access_token: Some(self.access_token),
                refresh_token: self.refresh_token,
            },
        };
        record.validate()?;
        Ok(record)
    }
}

/// 一次扫描的结果：能读的、读不了的。
#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    pub secrets: Vec<StoredSecret>,
    pub broken: Vec<BrokenEntry>,
}

impl ScanReport {
    pub fn is_empty(&self) -> bool {
        self.secrets.is_empty() && self.broken.is_empty()
    }
}

/// 读不出来的条目。**不删、不覆盖**：交给上层的修复流程处理。
#[derive(Debug, Clone)]
pub struct BrokenEntry {
    pub secret_id: String,
    pub kind: CredentialErrorKind,
    pub message: String,
}

/// 本地 JSON 凭据存储。
pub struct LocalJsonStore {
    root: PathBuf,
    lock_timeout: Duration,
    /// 仅测试：一次性故障注入点（0 = 未武装）。
    #[cfg(test)]
    faults: std::sync::atomic::AtomicU8,
}

/// 仅测试用的写入故障注入点。真实的"磁盘写不进去""rename 前进程被杀"
/// 在单测里造不出来，只能显式打开这两个开关。
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultPoint {
    /// 临时文件创建 / 写入阶段失败：目标文件必须保持旧内容。
    TempFile = 1,
    /// 临时文件写完了、`rename` 之前崩溃：旧内容仍然有效，临时文件残留。
    Rename = 2,
}

impl LocalJsonStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            lock_timeout: DEFAULT_LOCK_TIMEOUT,
            #[cfg(test)]
            faults: std::sync::atomic::AtomicU8::new(0),
        }
    }

    /// 数据根下的标准位置：`<data_dir>/secrets/v1`。
    pub fn for_data_dir(data_dir: impl AsRef<Path>) -> Self {
        Self::new(data_dir.as_ref().join(SECRETS_DIR).join(SCHEMA_DIR))
    }

    /// 从进程路径配置构造。CRED-2 / CRED-3 会用它接进 Broker。
    pub fn for_paths(paths: &crate::paths::Paths) -> Self {
        Self::for_data_dir(&paths.data_dir)
    }

    pub fn with_lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = timeout;
        self
    }

    /// 仅测试：武装一次性故障注入。触发后自动复位。
    #[cfg(test)]
    pub fn inject_fault(&self, point: FaultPoint) {
        self.faults
            .store(point as u8, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn take_fault(&self) -> Option<FaultPoint> {
        match self.faults.swap(0, std::sync::atomic::Ordering::SeqCst) {
            1 => Some(FaultPoint::TempFile),
            2 => Some(FaultPoint::Rename),
            _ => None,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 建目录并收紧权限到 `0700`。祖先目录（数据根）不归这里管，不动它的权限。
    pub fn ensure_root(&self) -> Result<(), CredentialError> {
        if let Some(parent) = self.root.parent() {
            if parent
                .file_name()
                .map(|n| n == SECRETS_DIR)
                .unwrap_or(false)
            {
                fs::create_dir_all(parent)
                    .map_err(|e| CredentialError::io(&e, "创建 secrets 目录失败"))?;
                set_mode(parent, SECRETS_MODE)
                    .map_err(|e| CredentialError::io(&e, "设置 secrets 目录权限失败"))?;
            }
        }
        fs::create_dir_all(&self.root).map_err(|e| CredentialError::io(&e, "创建凭据目录失败"))?;

        let meta = fs::symlink_metadata(&self.root)
            .map_err(|e| CredentialError::io(&e, "读取凭据目录信息失败"))?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsafe,
                "凭据目录不是普通目录（可能是符号链接）",
            ));
        }
        // 已经存在但权限更宽的目录，直接收紧；这是我们自己的目录。
        set_mode(&self.root, SECRETS_MODE)
            .map_err(|e| CredentialError::io(&e, "设置凭据目录权限失败"))?;
        Ok(())
    }

    /// 某个凭据文件的路径。ID 非法一律 `Unsafe`，不给目录穿越留缝。
    pub fn secret_path(&self, secret_id: &str) -> Result<PathBuf, CredentialError> {
        if !reference::valid_secret_id(secret_id) {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsafe,
                "凭据 ID 非法",
            ));
        }
        Ok(self.root.join(format!("{secret_id}.json")))
    }

    /// 新建一份凭据。`secretId` 由这里生成，`revision` 从 0 开始。
    pub fn create(&self, draft: SecretDraft) -> Result<StoredSecret, CredentialError> {
        let _lock = self.lock()?;
        // 持锁期间不可能有在写的临时文件，残留的一律是崩溃垃圾。
        self.sweep_orphans_unlocked(Duration::ZERO)?;

        let secret_id = Uuid::new_v4().to_string();
        let record = draft.into_record(secret_id)?;
        let path = self.secret_path(&record.secret_id)?;
        // 撞名（理论上不会）也不能覆盖别人的文件。
        if fs::symlink_metadata(&path).is_ok() {
            return Err(CredentialError::new(
                CredentialErrorKind::Conflict,
                "凭据 ID 已存在",
            ));
        }
        self.write_atomic(&record)?;
        Ok(record)
    }

    pub fn load(&self, secret_id: &str) -> Result<StoredSecret, CredentialError> {
        self.load_at_revision(secret_id, None)
    }

    /// 读并校验 `revision`。对不上就是 [`CredentialErrorKind::Conflict`]。
    pub fn load_at_revision(
        &self,
        secret_id: &str,
        expected_revision: Option<u64>,
    ) -> Result<StoredSecret, CredentialError> {
        let path = self.secret_path(secret_id)?;
        let bytes = self.read_checked(&path, secret_id)?;
        let record = decode(secret_id, &bytes)?;
        if let Some(expected) = expected_revision {
            if record.revision != expected {
                return Err(conflict(secret_id, expected, record.revision));
            }
        }
        Ok(record)
    }

    /// 取某个槽位的令牌。
    pub fn token(&self, secret_id: &str, slot: CredentialSlot) -> Result<String, CredentialError> {
        Ok(self.load(secret_id)?.token(slot)?.to_string())
    }

    /// 整份覆盖写：`expected_revision` 必须等于盘上的当前值，否则一字节都不写。
    /// 成功返回 `revision + 1` 的新记录。
    pub fn write(
        &self,
        record: &StoredSecret,
        expected_revision: u64,
    ) -> Result<StoredSecret, CredentialError> {
        let _lock = self.lock()?;
        let current = self.load_unlocked(&record.secret_id)?;
        if current.revision != expected_revision {
            return Err(conflict(
                &record.secret_id,
                expected_revision,
                current.revision,
            ));
        }
        let mut next = record.clone();
        next.schema_version = SCHEMA_VERSION;
        next.secret_id = current.secret_id;
        next.revision = expected_revision + 1;
        next.validate()?;
        self.write_atomic(&next)?;
        Ok(next)
    }

    /// 只换凭据段（access + refresh 作为一组），元数据不动。
    /// `expires_at = None` 会清掉过期时间 —— 调用方要么给新值，要么用 [`Self::write`]。
    pub fn replace_credentials(
        &self,
        secret_id: &str,
        expected_revision: u64,
        credentials: Credentials,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<StoredSecret, CredentialError> {
        let _lock = self.lock()?;
        let mut next = self.load_unlocked(secret_id)?;
        if next.revision != expected_revision {
            return Err(conflict(secret_id, expected_revision, next.revision));
        }
        next.credentials = credentials;
        next.expires_at = expires_at.map(|at| at.to_rfc3339_opts(SecondsFormat::Secs, true));
        next.revision = expected_revision + 1;
        next.validate()?;
        self.write_atomic(&next)?;
        Ok(next)
    }

    /// 删除一份凭据。文件不在返回 `false`（幂等），符号链接拒绝删。
    pub fn delete(&self, secret_id: &str) -> Result<bool, CredentialError> {
        let _lock = self.lock()?;
        let path = self.secret_path(secret_id)?;
        match fs::symlink_metadata(&path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(err) => Err(CredentialError::io(&err, "读取凭据文件失败")),
            Ok(meta) => {
                if meta.file_type().is_symlink() {
                    return Err(CredentialError::new(
                        CredentialErrorKind::Unsafe,
                        "拒绝删除符号链接形式的凭据",
                    ));
                }
                fs::remove_file(&path).map_err(|e| CredentialError::io(&e, "删除凭据文件失败"))?;
                sync_dir(&self.root)?;
                Ok(true)
            }
        }
    }

    /// 列目录：能读的进 `secrets`，读不出来的进 `broken`。
    ///
    /// 刻意**不**因为一个文件坏掉就整体失败：一个坏文件不该让账号列表整页空白，
    /// 但它也绝不能被当成"这个账号没有凭据"。
    pub fn scan(&self) -> Result<ScanReport, CredentialError> {
        self.ensure_root()?;
        let mut report = ScanReport::default();
        let entries =
            fs::read_dir(&self.root).map_err(|e| CredentialError::io(&e, "列凭据目录失败"))?;
        for entry in entries {
            let entry = entry.map_err(|e| CredentialError::io(&e, "列凭据目录失败"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(secret_id) = name.strip_suffix(".json") else {
                continue;
            };
            if !reference::valid_secret_id(secret_id) {
                report.broken.push(BrokenEntry {
                    secret_id: secret_id.to_string(),
                    kind: CredentialErrorKind::Unsafe,
                    message: "凭据文件名不合法".to_string(),
                });
                continue;
            }
            match self.load(secret_id) {
                Ok(secret) => report.secrets.push(secret),
                Err(err) => report.broken.push(BrokenEntry {
                    secret_id: secret_id.to_string(),
                    kind: err.kind,
                    message: err.message,
                }),
            }
        }
        report.secrets.sort_by(|a, b| a.secret_id.cmp(&b.secret_id));
        report.broken.sort_by(|a, b| a.secret_id.cmp(&b.secret_id));
        Ok(report)
    }

    /// 清理崩溃残留的 `.tmp-*`。持锁调用，所以没有"正在写"的临时文件。
    pub fn sweep_orphans(&self, older_than: Duration) -> Result<usize, CredentialError> {
        let _lock = self.lock()?;
        self.sweep_orphans_unlocked(older_than)
    }

    fn sweep_orphans_unlocked(&self, older_than: Duration) -> Result<usize, CredentialError> {
        let entries =
            fs::read_dir(&self.root).map_err(|e| CredentialError::io(&e, "列凭据目录失败"))?;
        let mut removed = 0usize;
        for entry in entries {
            let entry = entry.map_err(|e| CredentialError::io(&e, "列凭据目录失败"))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with(TEMP_PREFIX) {
                continue;
            }
            let path = entry.path();
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(_) => continue,
            };
            if !meta.is_file() {
                continue; // 不碰目录，更不碰符号链接指向的东西
            }
            if !older_than.is_zero() {
                let age = meta
                    .modified()
                    .ok()
                    .and_then(|at| SystemTime::now().duration_since(at).ok());
                if age.map(|age| age < older_than).unwrap_or(false) {
                    continue;
                }
            }
            if fs::remove_file(&path).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    // -- 内部实现 ---------------------------------------------------------

    fn load_unlocked(&self, secret_id: &str) -> Result<StoredSecret, CredentialError> {
        let path = self.secret_path(secret_id)?;
        let bytes = self.read_checked(&path, secret_id)?;
        decode(secret_id, &bytes)
    }

    fn lock(&self) -> Result<FileLock, CredentialError> {
        self.ensure_root()?;
        let path = self.root.join(LOCK_FILE);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|e| CredentialError::io(&e, "打开凭据锁文件失败"))?;
        let _ = set_mode(&path, SECRET_MODE);

        let deadline = Instant::now() + self.lock_timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(FileLock { file }),
                Err(std::fs::TryLockError::WouldBlock) => {
                    if Instant::now() >= deadline {
                        // 超时必须报错：挂着不动会让整个 RPC 队列卡死（PRD §8）。
                        return Err(CredentialError::new(
                            CredentialErrorKind::LockTimeout,
                            "凭据目录被另一个 AICC 进程占用",
                        ));
                    }
                    std::thread::sleep(LOCK_POLL_INTERVAL);
                }
                Err(std::fs::TryLockError::Error(err)) => {
                    return Err(CredentialError::io(&err, "凭据目录加锁失败"))
                }
            }
        }
    }

    /// 读文件：先看路径上的元数据（挡符号链接），再按 `O_NOFOLLOW` 打开，
    /// 打开后再看一遍 fd 自己的元数据 —— 检查的和读的必须是同一个 inode。
    fn read_checked(&self, path: &Path, secret_id: &str) -> Result<Vec<u8>, CredentialError> {
        let meta = match fs::symlink_metadata(path) {
            Ok(meta) => meta,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(CredentialError::new(
                    CredentialErrorKind::NotFound,
                    format!("凭据不存在：{secret_id}"),
                ))
            }
            Err(err) => return Err(CredentialError::io(&err, "读取凭据文件失败")),
        };
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsafe,
                format!("凭据不是普通文件：{secret_id}"),
            ));
        }
        check_file_mode(&meta, secret_id)?;

        let mut file =
            open_read_no_follow(path).map_err(|e| CredentialError::io(&e, "打开凭据文件失败"))?;
        let fd_meta = file
            .metadata()
            .map_err(|e| CredentialError::io(&e, "读取凭据文件信息失败"))?;
        if !fd_meta.is_file() {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsafe,
                format!("凭据不是普通文件：{secret_id}"),
            ));
        }
        check_file_mode(&fd_meta, secret_id)?;

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| CredentialError::io(&e, "读取凭据文件失败"))?;
        Ok(bytes)
    }

    fn write_atomic(&self, record: &StoredSecret) -> Result<(), CredentialError> {
        let target = self.secret_path(&record.secret_id)?;
        let mut payload = serde_json::to_vec_pretty(record)
            .map_err(|e| CredentialError::new(CredentialErrorKind::Io, e.to_string()))?;
        payload.push(b'\n');

        let temp = self
            .root
            .join(format!("{TEMP_PREFIX}{}", Uuid::new_v4().simple()));

        #[cfg(test)]
        let fault = self.take_fault();

        #[cfg(test)]
        if fault == Some(FaultPoint::TempFile) {
            return Err(CredentialError::new(
                CredentialErrorKind::Io,
                "注入故障：临时凭据文件写入失败",
            ));
        }

        // 1) 同目录临时文件：只能是新内容，旧文件一个字节都没动。
        if let Err(err) = write_temp_file(&temp, &payload) {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }

        #[cfg(test)]
        if fault == Some(FaultPoint::Rename) {
            // 模拟"临时文件写完、rename 之前进程被杀"：临时文件故意留着，
            // 目标文件仍是旧内容；下次写入开头的 sweep 会清掉它。
            return Err(CredentialError::new(
                CredentialErrorKind::Io,
                "注入故障：替换凭据文件前中断",
            ));
        }

        // 2) rename 是原子的：读到的要么是旧内容、要么是新内容。
        if let Err(err) = commit_temp_file(&temp, &target) {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }
        Ok(())
    }
}

impl std::fmt::Debug for LocalJsonStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalJsonStore")
            .field("root", &self.root)
            .field("lock_timeout", &self.lock_timeout)
            .finish()
    }
}

/// 持锁期间有效。`File` 一关（Drop）锁就释放，所以只需要把 fd 留住。
struct FileLock {
    file: File,
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn conflict(secret_id: &str, expected: u64, actual: u64) -> CredentialError {
    CredentialError::new(
        CredentialErrorKind::Conflict,
        format!("凭据 {secret_id} 版本不符：期望 {expected}，实际 {actual}"),
    )
}

fn decode(secret_id: &str, bytes: &[u8]) -> Result<StoredSecret, CredentialError> {
    // 不带 serde 的原始错误：它的 `invalid type: string "..."` 会把令牌抄进消息里。
    // 只保留行列号，足够定位，不带内容。
    let record: StoredSecret = serde_json::from_slice(bytes).map_err(|e| {
        CredentialError::new(
            CredentialErrorKind::Corrupt,
            format!(
                "凭据文件不是合法 JSON（第 {} 行第 {} 列）",
                e.line(),
                e.column()
            ),
        )
    })?;
    if record.schema_version != SCHEMA_VERSION {
        return Err(CredentialError::new(
            CredentialErrorKind::Unsupported,
            format!("凭据文件结构版本不支持：{}", record.schema_version),
        ));
    }
    if record.secret_id != secret_id {
        return Err(CredentialError::new(
            CredentialErrorKind::Corrupt,
            "凭据文件内的 ID 与文件名不一致",
        ));
    }
    record.validate().map_err(|err| match err.kind {
        CredentialErrorKind::Unsafe => err,
        // 内容层面的问题一律按"文件坏了"报，避免把校验细节当契约。
        _ => CredentialError::new(CredentialErrorKind::Corrupt, err.message),
    })?;
    Ok(record)
}

fn validate_platform(platform: &str) -> Result<(), CredentialError> {
    let ok = !platform.is_empty()
        && platform.len() <= 32
        && platform
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(CredentialError::new(
            CredentialErrorKind::Unsupported,
            "平台标识非法",
        ))
    }
}

fn has_control_chars(text: &str) -> bool {
    text.chars().any(|c| c.is_control())
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn check_file_mode(meta: &fs::Metadata, secret_id: &str) -> Result<(), CredentialError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 != 0 {
        return Err(CredentialError::new(
            CredentialErrorKind::Unsafe,
            format!("凭据文件权限过宽（{mode:o}）：{secret_id}"),
        ));
    }
    if mode & 0o400 == 0 {
        return Err(CredentialError::new(
            CredentialErrorKind::Unsafe,
            format!("凭据文件不可读（{mode:o}）：{secret_id}"),
        ));
    }
    Ok(())
}

#[cfg(not(unix))]
fn check_file_mode(_meta: &fs::Metadata, _secret_id: &str) -> Result<(), CredentialError> {
    Ok(())
}

#[cfg(unix)]
const O_NOFOLLOW: i32 = {
    #[cfg(target_os = "macos")]
    {
        0x0100
    }
    #[cfg(not(target_os = "macos"))]
    {
        0o400000
    }
};

fn open_read_no_follow(path: &Path) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.custom_flags(O_NOFOLLOW);
    }
    opts.open(path)
}

fn create_private_file(path: &Path) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(SECRET_MODE);
    }
    opts.open(path)
}

/// 写同目录临时文件并落盘。
fn write_temp_file(temp: &Path, payload: &[u8]) -> Result<(), CredentialError> {
    let mut file =
        create_private_file(temp).map_err(|e| CredentialError::io(&e, "创建临时凭据文件失败"))?;
    file.write_all(payload)
        .map_err(|e| CredentialError::io(&e, "写入临时凭据文件失败"))?;
    // umask 会把 mode 参数抹掉一些位，这里显式再钉一次。
    set_mode(temp, SECRET_MODE).map_err(|e| CredentialError::io(&e, "设置临时凭据文件权限失败"))?;
    file.sync_all()
        .map_err(|e| CredentialError::io(&e, "刷新临时凭据文件失败"))
}

/// 原子替换 + 目录 fsync。目录不 fsync 的话，崩溃后 rename 本身可能没落地。
fn commit_temp_file(temp: &Path, target: &Path) -> Result<(), CredentialError> {
    fs::rename(temp, target).map_err(|e| CredentialError::io(&e, "替换凭据文件失败"))?;
    let dir = target.parent().unwrap_or(Path::new("."));
    sync_dir(dir)
}

fn sync_dir(dir: &Path) -> Result<(), CredentialError> {
    let handle = File::open(dir).map_err(|e| CredentialError::io(&e, "打开凭据目录失败"))?;
    handle
        .sync_all()
        .map_err(|e| CredentialError::io(&e, "刷新凭据目录失败"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn store(dir: &tempfile::TempDir) -> LocalJsonStore {
        LocalJsonStore::for_data_dir(dir.path())
    }

    fn draft() -> SecretDraft {
        SecretDraft::oauth("auth-1", "workbuddy", "ACCESS-TOKEN-1")
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn chmod(path: &Path, mode: u32) {
        set_mode(path, mode).expect("chmod");
    }

    fn secret_file(store: &LocalJsonStore, secret_id: &str) -> PathBuf {
        store.root().join(format!("{secret_id}.json"))
    }

    fn rewrite(store: &LocalJsonStore, secret_id: &str, mutate: impl Fn(&mut serde_json::Value)) {
        let path = secret_file(store, secret_id);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
        mutate(&mut value);
        fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).expect("write");
        #[cfg(unix)]
        chmod(&path, 0o600);
    }

    // -- 基本读写与落盘形状 ---------------------------------------------

    #[test]
    fn create_writes_one_private_file_shaped_like_the_contract() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let record = store
            .create(draft().with_refresh_token("REFRESH-TOKEN-1"))
            .expect("create");

        assert_eq!(record.schema_version, SCHEMA_VERSION);
        assert_eq!(record.revision, 0);
        assert!(
            Uuid::parse_str(&record.secret_id).is_ok(),
            "文件名必须是随机 UUID，实际 {}",
            record.secret_id
        );
        assert!(
            !record.secret_id.contains("auth-1"),
            "ID 不能从账号 / 授权信息派生"
        );

        let root = dir.path().join("secrets").join("v1");
        assert_eq!(store.root(), root);
        #[cfg(unix)]
        {
            assert_eq!(mode_of(&root), 0o700, "凭据目录必须 0700");
            assert_eq!(
                mode_of(root.parent().unwrap()),
                0o700,
                "secrets 目录必须 0700"
            );
            let file = secret_file(&store, &record.secret_id);
            assert_eq!(mode_of(&file), 0o600, "凭据文件必须 0600");
        }

        let raw: serde_json::Value =
            serde_json::from_slice(&fs::read(secret_file(&store, &record.secret_id)).unwrap())
                .expect("json");
        assert_eq!(raw["schemaVersion"], 1);
        assert_eq!(raw["secretId"], record.secret_id.as_str());
        assert_eq!(raw["authorizationId"], "auth-1");
        assert_eq!(raw["platform"], "workbuddy");
        assert_eq!(raw["source"], "own");
        assert_eq!(raw["credentialKind"], "oauth");
        assert_eq!(raw["revision"], 0);
        assert_eq!(raw["credentials"]["accessToken"], "ACCESS-TOKEN-1");
        assert_eq!(raw["credentials"]["refreshToken"], "REFRESH-TOKEN-1");

        assert_eq!(store.load(&record.secret_id).expect("load"), record);
        assert_eq!(store.scan().unwrap().secrets.len(), 1);
    }

    #[test]
    fn slots_are_selected_explicitly_never_inferred() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let record = store
            .create(draft().with_refresh_token("REFRESH-TOKEN-1"))
            .expect("create");

        assert_eq!(
            store
                .token(&record.secret_id, CredentialSlot::Access)
                .unwrap(),
            "ACCESS-TOKEN-1"
        );
        assert_eq!(
            store
                .token(&record.secret_id, CredentialSlot::Refresh)
                .unwrap(),
            "REFRESH-TOKEN-1"
        );
        // 指纹不同 = 确实取了两个不同的字段，而不是"取第一个非空值"。
        assert_ne!(
            record.fingerprint(CredentialSlot::Access),
            record.fingerprint(CredentialSlot::Refresh)
        );
    }

    #[test]
    fn pat_has_no_refresh_slot_and_cannot_carry_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let record = store
            .create(SecretDraft::pat("auth-pat", "qoder", "PAT-VALUE"))
            .expect("create");
        assert_eq!(record.credential_kind, CredentialKind::Pat);
        assert_eq!(
            store
                .token(&record.secret_id, CredentialSlot::Refresh)
                .unwrap_err()
                .kind,
            CredentialErrorKind::NotFound,
            "PAT 的 refresh 槽位是空的，不是空串"
        );
        assert!(record.credentials.refresh_token.is_none());

        let err = store
            .create(SecretDraft::pat("auth-pat", "qoder", "PAT").with_refresh_token("R"))
            .unwrap_err();
        assert_eq!(err.kind, CredentialErrorKind::Unsupported);
    }

    #[test]
    fn empty_or_control_char_tokens_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        for bad in ["", "   ", "tok\nen"] {
            let err = store
                .create(SecretDraft::oauth("auth-1", "workbuddy", bad))
                .unwrap_err();
            assert_eq!(err.kind, CredentialErrorKind::Unsupported, "{}", err);
        }
        assert_eq!(store.scan().unwrap().secrets.len(), 0, "失败不能留下文件");
        assert!(store
            .create(SecretDraft::oauth("auth-1", "bad platform", "t"))
            .is_err());
    }

    // -- 版本与并发 -----------------------------------------------------

    #[test]
    fn replace_credentials_updates_the_pair_and_bumps_revision() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = store.create(draft()).expect("create");

        let next = store
            .replace_credentials(
                &created.secret_id,
                0,
                Credentials::oauth("ACCESS-2", "REFRESH-2"),
                None,
            )
            .expect("replace");
        assert_eq!(next.revision, 1);
        assert_eq!(next.token(CredentialSlot::Access).unwrap(), "ACCESS-2");
        assert_eq!(next.token(CredentialSlot::Refresh).unwrap(), "REFRESH-2");

        // 两组令牌一起换：盘上不能出现"新 access + 旧 refresh"。
        let on_disk = store.load(&created.secret_id).unwrap();
        assert_eq!(
            on_disk.credentials.access_token.as_deref(),
            Some("ACCESS-2")
        );
        assert_eq!(
            on_disk.credentials.refresh_token.as_deref(),
            Some("REFRESH-2")
        );
    }

    #[test]
    fn stale_revision_is_rejected_without_touching_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = store.create(draft()).expect("create");
        let before = fs::read(secret_file(&store, &created.secret_id)).unwrap();

        let err = store
            .replace_credentials(
                &created.secret_id,
                7,
                Credentials::access_only("ACCESS-STALE"),
                None,
            )
            .unwrap_err();
        assert_eq!(err.kind, CredentialErrorKind::Conflict);
        assert_eq!(
            fs::read(secret_file(&store, &created.secret_id)).unwrap(),
            before,
            "版本对不上时一个字节都不许写"
        );

        let err = store
            .write(&created, 3)
            .expect_err("整份覆盖写同样要校验版本");
        assert_eq!(err.kind, CredentialErrorKind::Conflict);
    }

    #[test]
    fn concurrent_writers_let_exactly_one_win() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = store.create(draft()).expect("create");

        let barrier = Arc::new(Barrier::new(4));
        let mut handles = Vec::new();
        for index in 0..4 {
            let root = store.root().to_path_buf();
            let secret_id = created.secret_id.clone();
            let barrier = Arc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                // 每个线程用一个独立实例，等价于"另一个进程"。
                let store = LocalJsonStore::new(root);
                barrier.wait();
                store
                    .replace_credentials(
                        &secret_id,
                        0,
                        Credentials::access_only(format!("ACCESS-{index}")),
                        None,
                    )
                    .map(|_| ())
            }));
        }

        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        let wins = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(wins, 1, "并发刷新必须只有一个成功：{results:?}");
        for result in results.iter().filter(|r| r.is_err()) {
            assert_eq!(
                result.as_ref().unwrap_err().kind,
                CredentialErrorKind::Conflict
            );
        }
        assert_eq!(store.load(&created.secret_id).unwrap().revision, 1);
    }

    #[test]
    fn lock_contention_surfaces_as_a_bounded_error() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir).with_lock_timeout(Duration::from_millis(60));
        let held = store.lock().expect("hold lock");

        let started = Instant::now();
        let err = store.create(draft()).unwrap_err();
        assert_eq!(err.kind, CredentialErrorKind::LockTimeout);
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "等锁必须有上限，不能挂死 RPC 队列"
        );

        drop(held);
        assert!(store.create(draft()).is_ok(), "锁释放后应能正常写入");
    }

    // -- 崩溃恢复与故障注入 ---------------------------------------------

    #[test]
    fn injected_write_failure_keeps_the_previous_credential() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = store
            .create(draft().with_refresh_token("R1"))
            .expect("create");

        store.inject_fault(FaultPoint::TempFile);
        let err = store
            .replace_credentials(
                &created.secret_id,
                0,
                Credentials::access_only("ACCESS-BROKEN"),
                None,
            )
            .unwrap_err();
        assert_eq!(err.kind, CredentialErrorKind::Io);

        let after = store.load(&created.secret_id).expect("旧凭据必须还在");
        assert_eq!(after, created);
        assert!(
            store.sweep_orphans(Duration::ZERO).unwrap() == 0,
            "失败路径要自己清掉临时文件"
        );
    }

    #[test]
    fn interrupted_write_leaves_old_file_plus_a_sweepable_temp() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = store.create(draft()).expect("create");

        store.inject_fault(FaultPoint::Rename);
        let err = store
            .replace_credentials(
                &created.secret_id,
                0,
                Credentials::access_only("ACCESS-LOST"),
                None,
            )
            .unwrap_err();
        assert_eq!(err.kind, CredentialErrorKind::Io);

        // 旧内容完好；临时文件残留但不参与读取。
        assert_eq!(store.load(&created.secret_id).unwrap(), created);
        let scan = store.scan().unwrap();
        assert_eq!(scan.secrets.len(), 1);
        assert!(scan.broken.is_empty(), "临时文件不该被当成坏凭据");

        assert_eq!(store.sweep_orphans(Duration::ZERO).unwrap(), 1);
        // 恢复后可以重试同一笔写入。
        let retried = store
            .replace_credentials(
                &created.secret_id,
                0,
                Credentials::access_only("ACCESS-RETRIED"),
                None,
            )
            .expect("重试成功");
        assert_eq!(retried.revision, 1);
    }

    #[test]
    fn create_sweeps_stale_temp_files_first() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        store.ensure_root().unwrap();
        let orphan = store.root().join(format!("{TEMP_PREFIX}deadbeef"));
        fs::write(&orphan, b"garbage").unwrap();

        store.create(draft()).expect("create");
        assert!(!orphan.exists(), "新建凭据前应清掉崩溃残留");
    }

    // -- 文件内容的不可信输入 -------------------------------------------

    #[test]
    fn corrupt_json_is_reported_not_silently_missing() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = store.create(draft()).expect("create");

        fs::write(secret_file(&store, &created.secret_id), b"{ not json").unwrap();
        #[cfg(unix)]
        chmod(&secret_file(&store, &created.secret_id), 0o600);

        let err = store.load(&created.secret_id).unwrap_err();
        assert_eq!(err.kind, CredentialErrorKind::Corrupt);
        assert!(!err.message.contains("ACCESS-TOKEN-1"));

        let scan = store.scan().unwrap();
        assert!(scan.secrets.is_empty());
        assert_eq!(scan.broken.len(), 1);
        assert_eq!(scan.broken[0].kind, CredentialErrorKind::Corrupt);
    }

    #[test]
    fn file_id_must_match_the_name_and_schema_must_be_known() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = store.create(draft()).expect("create");

        let alias = Uuid::new_v4().to_string();
        fs::copy(
            secret_file(&store, &created.secret_id),
            secret_file(&store, &alias),
        )
        .unwrap();
        #[cfg(unix)]
        chmod(&secret_file(&store, &alias), 0o600);
        assert_eq!(
            store.load(&alias).unwrap_err().kind,
            CredentialErrorKind::Corrupt,
            "文件内 ID 与文件名不一致必须拒读"
        );

        rewrite(&store, &created.secret_id, |value| {
            value["schemaVersion"] = serde_json::json!(2);
        });
        assert_eq!(
            store.load(&created.secret_id).unwrap_err().kind,
            CredentialErrorKind::Unsupported
        );

        rewrite(&store, &created.secret_id, |value| {
            value["credentials"]["refreshTokenAtRoot"] = serde_json::json!("oops");
        });
        assert_eq!(
            store.load(&created.secret_id).unwrap_err().kind,
            CredentialErrorKind::Corrupt,
            "多出来的字段必须被拒，否则字段错位会被静默忽略"
        );
    }

    #[test]
    fn unknown_keys_do_not_get_misread_as_access_tokens() {
        // 显式钉住"字段选择机制"：只有 accessToken 能出现在 access 槽位。
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = store.create(draft()).expect("create");
        rewrite(&store, &created.secret_id, |value| {
            value["credentials"] = serde_json::json!({ "refreshToken": "ONLY-REFRESH" });
        });
        let err = store.load(&created.secret_id).unwrap_err();
        assert_eq!(err.kind, CredentialErrorKind::Corrupt);
    }

    // -- 路径与权限的安全边界 -------------------------------------------

    #[test]
    fn symlinks_are_refused_on_read_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = store.create(draft()).expect("create");

        let outside = dir.path().join("outside.json");
        fs::copy(secret_file(&store, &created.secret_id), &outside).unwrap();
        fs::remove_file(secret_file(&store, &created.secret_id)).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, secret_file(&store, &created.secret_id)).unwrap();

        assert_eq!(
            store.load(&created.secret_id).unwrap_err().kind,
            CredentialErrorKind::Unsafe
        );
        assert_eq!(
            store.delete(&created.secret_id).unwrap_err().kind,
            CredentialErrorKind::Unsafe
        );
        assert!(outside.exists(), "拒绝删除软链，也不能删到目标文件");
    }

    #[test]
    fn traversal_style_ids_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        for bad in ["../escape", "a/b", "..", "", "x.json"] {
            assert_eq!(
                store.load(bad).unwrap_err().kind,
                CredentialErrorKind::Unsafe,
                "必须拒绝：{bad}"
            );
            assert_eq!(
                store.delete(bad).unwrap_err().kind,
                CredentialErrorKind::Unsafe,
                "必须拒绝：{bad}"
            );
        }
    }

    #[test]
    fn widened_file_mode_is_refused_and_can_be_tightened_back() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let created = store.create(draft()).expect("create");
        let path = secret_file(&store, &created.secret_id);

        #[cfg(unix)]
        {
            chmod(&path, 0o644);
            let err = store.load(&created.secret_id).unwrap_err();
            assert_eq!(err.kind, CredentialErrorKind::Unsafe, "{err}");
            assert!(err.message.contains("权限过宽"));

            chmod(&path, 0o600);
            assert!(store.load(&created.secret_id).is_ok());
        }
    }

    #[test]
    fn ensure_root_tightens_an_existing_directory() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let root = store.root().to_path_buf();
        fs::create_dir_all(&root).unwrap();
        #[cfg(unix)]
        chmod(&root, 0o755);

        store.ensure_root().expect("ensure");
        #[cfg(unix)]
        assert_eq!(mode_of(&root), 0o700);
    }

    // -- 删除与扫描 -----------------------------------------------------

    #[test]
    fn delete_only_removes_the_named_secret() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let first = store.create(draft()).expect("create 1");
        let second = store
            .create(SecretDraft::oauth("auth-2", "qoder", "ACCESS-2"))
            .expect("create 2");

        assert!(store.delete(&first.secret_id).unwrap());
        assert!(!store.delete(&first.secret_id).unwrap(), "删除是幂等的");
        assert_eq!(
            store.load(&first.secret_id).unwrap_err().kind,
            CredentialErrorKind::NotFound
        );
        assert_eq!(store.load(&second.secret_id).unwrap(), second);
    }

    #[test]
    fn scan_reports_broken_entries_without_hiding_healthy_ones() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        store.create(draft()).expect("create");
        store.ensure_root().unwrap();

        let broken_id = Uuid::new_v4().to_string();
        let path = secret_file(&store, &broken_id);
        fs::write(&path, b"{}").unwrap();
        #[cfg(unix)]
        chmod(&path, 0o600);

        let report = store.scan().unwrap();
        assert_eq!(report.secrets.len(), 1);
        assert_eq!(report.broken.len(), 1);
        assert_eq!(report.broken[0].secret_id, broken_id);
        assert_eq!(report.broken[0].kind, CredentialErrorKind::Corrupt);
        // 扫描不会顺手删掉坏文件：修复要由用户显式决定。
        assert!(path.exists());
    }

    // -- 泄漏面 ---------------------------------------------------------

    #[test]
    fn tokens_never_appear_in_debug_output_or_messages() {
        let access = "ACCESS-TOKEN-THAT-MUST-NOT-LEAK";
        let refresh = "REFRESH-TOKEN-THAT-MUST-NOT-LEAK";
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let draft = SecretDraft::oauth("auth-1", "workbuddy", access).with_refresh_token(refresh);
        let debug_draft = format!("{draft:?}");
        assert!(!debug_draft.contains(access));
        assert!(!debug_draft.contains(refresh));

        let record = store.create(draft).expect("create");
        for text in [
            format!("{record:?}"),
            format!("{:?}", record.credentials),
            format!("{store:?}"),
        ] {
            assert!(!text.contains(access), "{text}");
            assert!(!text.contains(refresh), "{text}");
        }

        // 坏文件也不能把令牌抄进错误消息。
        fs::write(
            secret_file(&store, &record.secret_id),
            format!("{{\"credentials\": \"{access}\"}}"),
        )
        .unwrap();
        #[cfg(unix)]
        chmod(&secret_file(&store, &record.secret_id), 0o600);
        let err = store.load(&record.secret_id).unwrap_err();
        assert!(!err.message.contains(access), "{err}");
    }
}
