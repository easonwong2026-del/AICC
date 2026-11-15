//! 凭据引用与本地 JSON 存储（PR-CRED-1）。
//!
//! 这一层只铺地基，**不接入任何生产路径**：PR-CRED-1 不迁移真实账号，也不改变
//! 现有钥匙串默认行为。[`securestore`](crate::securestore) 仍是当前唯一被调用
//! 的凭据后端；本模块的消费方在 PR-CRED-2 / PR-CRED-3 落地。
//!
//! 两个子模块：
//! * [`reference`]：把 SQLite 里的引用串（`keychain:` / `localjson:v1:` / `magpie:v1:`）
//!   解析成**带类型**的枚举。引用方案必须显式，绝不能"解析不了就当钥匙串" ——
//!   那样迁移后的账号仍会突然弹钥匙串授权框。
//! * [`localjson`]：AICC 自有授权的一份一文件明文 JSON 存储（0700/0600、原子写入、
//!   跨进程锁、revision 检查）。
//!
//! # 明文存储的风险（必须对用户明示）
//!
//! 这是**明文 JSON**，不是加密存储。`0700` / `0600` 只能挡住同机其他普通用户，
//! 挡不住同一个 macOS 用户权限下的恶意程序。建议开启 FileVault，并且不要把
//! `secrets/` 放进云同步或未加密备份。

pub mod localjson;
pub mod reference;

/// 本进程已知的平台标识。
///
/// 只用来给上层做枚举 / 展示，**不**作为写入的硬白名单：新增平台不该因为这里没加
/// 就写不进去。真正的硬约束是 [`localjson`] 里的字符合法性校验。
pub const KNOWN_PLATFORMS: &[&str] = &["workbuddy", "workbuddy-ai", "qoder", "qoder-cn"];

/// 凭据层错误分类。分类是稳定契约（RPC `data.kind` 直接用这些字符串），
/// 文案不是：上层只该按 `kind` 分支。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialErrorKind {
    /// 凭据不存在（文件不在、或该槽位没有值）。
    NotFound,
    /// 没有权限（EACCES / EPERM / 只读卷），**不等于**"没有凭据"。
    Denied,
    /// 内容不可信：JSON 坏了、字段对不上、ID 与文件名不一致。
    Corrupt,
    /// 能读，但本版本不认识（比如将来更高的 `schemaVersion`）。
    Unsupported,
    /// 外部环境暂时不可用（Magpie 没装 / 文件读不了）。**不是**"没有凭据"，
    /// 不能借这个提示用户重新登录。
    Unavailable,
    /// 版本对不上：拿旧快照去覆盖新令牌，一个字节都不许写。
    Conflict,
    /// 等锁超时：另一个进程正在写。
    LockTimeout,
    /// 不安全：符号链接、目录穿越、非法 ID、文件权限过宽。
    Unsafe,
    /// 其他 IO 错误。
    Io,
}

impl CredentialErrorKind {
    /// 稳定的字符串标识，用于 RPC `data.kind` 与 `health` 统计。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotFound => "not_found",
            Self::Denied => "denied",
            Self::Corrupt => "corrupt",
            Self::Unsupported => "unsupported",
            Self::Unavailable => "unavailable",
            Self::Conflict => "conflict",
            Self::LockTimeout => "lock_timeout",
            Self::Unsafe => "unsafe",
            Self::Io => "io",
        }
    }
}

/// 凭据层错误。`message` 里**永远不含令牌**：本模块所有文案都只引用 ID、槽位与
/// 系统错误码，构造时就不给令牌留位置。
#[derive(Debug, Clone)]
pub struct CredentialError {
    pub kind: CredentialErrorKind,
    pub message: String,
    pub os_status: Option<i32>,
    /// 删除是否已经**部分生效**：一组凭据（例如 access + refresh）里有的删掉了、
    /// 有的失败。调用方据此把授权行标成不可用 —— 剩下半份凭据已经发不出请求，
    /// 继续显示 `valid` 会让用户看到"授权还在但领取一直失败"。
    pub partial: bool,
}

impl CredentialError {
    pub fn new(kind: CredentialErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            os_status: None,
            partial: false,
        }
    }

    /// 标记"这次删除只生效了一部分"。
    pub fn with_partial(mut self) -> Self {
        self.partial = true;
        self
    }

    pub fn with_status(mut self, status: i32) -> Self {
        self.os_status = Some(status);
        self
    }

    pub fn io(err: &std::io::Error, context: impl AsRef<str>) -> Self {
        let kind = match err.raw_os_error() {
            // EPERM / EACCES / EROFS：都是"现在读不了/写不了"，绝不能当成"没有凭据"。
            Some(1) | Some(13) | Some(30) => CredentialErrorKind::Denied,
            _ => CredentialErrorKind::Io,
        };
        let mut out = CredentialError::new(
            kind,
            format!(
                "{}：{}",
                context.as_ref(),
                crate::rpc::sanitize(&err.to_string())
            ),
        );
        out.os_status = err.raw_os_error();
        out
    }
}

impl std::fmt::Display for CredentialError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}（{}）", self.message, self.kind.as_str())
    }
}

impl From<CredentialError> for crate::rpc::RpcError {
    fn from(err: CredentialError) -> Self {
        use crate::rpc::E_SECURESTORE;
        crate::rpc::RpcError::new(E_SECURESTORE, err.message.clone()).with_data(serde_json::json!({
            "kind": err.kind.as_str(),
            "osStatus": err.os_status,
        }))
    }
}
