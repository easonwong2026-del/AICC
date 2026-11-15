//! 凭据引用解析（PR-CRED-1 的"统一解析基础"）。
//!
//! SQLite 里只存引用串，不存令牌。历史上只有一种引用（`keychain:`）；本次新增
//! `localjson:v1:` 与 `magpie:v1:`，于是"引用是哪一种"必须由**显式前缀**决定：
//!
//! | 引用 | 含义 | 读失败时允许兜底到钥匙串吗 |
//! |---|---|---|
//! | `keychain:<key>` | 旧钥匙串凭据（未迁移） | 本身就是钥匙串 |
//! | `localjson:v1:<id>` | AICC 自有 JSON 凭据 | **绝不**（PRD §5.2） |
//! | `magpie:v1:<source-key>` | Magpie 动态关联 | **绝不** |
//!
//! [`CredentialRef::parse`] 对任何不认识的前缀直接报错，而不是退回钥匙串。
//! 这是"迁移完成后仍有弹窗"这类问题的根因防线：一旦解析可以"宽容地"落到
//! 钥匙串，任何一处笔误都会变成一次系统授权弹框。

use super::{CredentialError, CredentialErrorKind};

/// 旧钥匙串引用前缀。与 [`crate::auth::ids::REF_PREFIX`] 必须保持一致。
pub const KEYCHAIN_PREFIX: &str = "keychain:";
/// AICC 自有 JSON 引用前缀（带存储版本）。
pub const LOCAL_JSON_PREFIX: &str = "localjson:v1:";
/// Magpie 动态关联引用前缀（带引用版本）。
pub const MAGPIE_PREFIX: &str = "magpie:v1:";

/// 一份凭据里要取哪个槽位。
///
/// 存在的理由很具体：WorkBuddy 的 JSON 里 access 与 refresh 同文件共存，
/// 只能靠**调用方显式指定槽位**来区分，不能靠"读第一个非空字段"这种猜测。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSlot {
    Access,
    Refresh,
}

impl CredentialSlot {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Access => "access",
            Self::Refresh => "refresh",
        }
    }
}

impl std::fmt::Display for CredentialSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 解析后的引用。带类型的枚举意味着调用方**不可能**把 Magpie 引用误传给
/// `SecureStore::get` —— 那需要先写一个 `match` 分支。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialRef {
    /// 旧钥匙串。只有明确标记为未迁移的授权才允许走这条。
    Keychain { key: String },
    /// AICC 自有 JSON 文件，`secret_id` 是文件名（不含扩展名）。
    LocalJson { secret_id: String },
    /// Magpie 原始文件里的来源键；令牌每次现读，不落盘、不复制。
    Magpie { source_key: String },
}

impl CredentialRef {
    /// 解析引用串。不认识的前缀返回 [`CredentialErrorKind::Unsupported`]。
    pub fn parse(raw: &str) -> Result<Self, CredentialError> {
        if raw.is_empty() {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsupported,
                "凭据引用为空",
            ));
        }
        if has_control_or_space(raw) {
            return Err(CredentialError::new(
                CredentialErrorKind::Unsupported,
                "凭据引用含空白或控制字符",
            ));
        }

        if let Some(key) = raw.strip_prefix(KEYCHAIN_PREFIX) {
            if !valid_keychain_key(key) {
                return Err(CredentialError::new(
                    CredentialErrorKind::Unsafe,
                    "钥匙串引用形状非法",
                ));
            }
            return Ok(Self::Keychain {
                key: key.to_string(),
            });
        }

        if let Some(secret_id) = raw.strip_prefix(LOCAL_JSON_PREFIX) {
            if !valid_secret_id(secret_id) {
                return Err(CredentialError::new(
                    CredentialErrorKind::Unsafe,
                    "本地 JSON 引用含非法 ID",
                ));
            }
            return Ok(Self::LocalJson {
                secret_id: secret_id.to_string(),
            });
        }

        if let Some(source_key) = raw.strip_prefix(MAGPIE_PREFIX) {
            if !valid_source_key(source_key) {
                return Err(CredentialError::new(
                    CredentialErrorKind::Unsafe,
                    "Magpie 引用含非法来源键",
                ));
            }
            return Ok(Self::Magpie {
                source_key: source_key.to_string(),
            });
        }

        // 关键：不猜、不兜底。`localjson:v2:`、`magpie:` 这种"看起来像"的写法
        // 也要被拒，否则版本升级时的半迁移数据会静默走错后端。
        Err(CredentialError::new(
            CredentialErrorKind::Unsupported,
            "未知凭据引用方案",
        ))
    }

    /// 稳定标识：`keychain` / `localjson` / `magpie`。
    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Keychain { .. } => "keychain",
            Self::LocalJson { .. } => "localjson",
            Self::Magpie { .. } => "magpie",
        }
    }

    /// 是否是需要系统授权（可能弹框）的旧钥匙串引用。
    pub fn needs_system_authorization(&self) -> bool {
        matches!(self, Self::Keychain { .. })
    }

    /// 是否是"每次使用现读"的动态引用（Magpie）。
    pub fn is_dynamic(&self) -> bool {
        matches!(self, Self::Magpie { .. })
    }

    /// 引用指向的键 / ID（不含前缀），供幂等比较与去重使用。
    pub fn target(&self) -> &str {
        match self {
            Self::Keychain { key } => key,
            Self::LocalJson { secret_id } => secret_id,
            Self::Magpie { source_key } => source_key,
        }
    }
}

impl std::fmt::Display for CredentialRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Keychain { key } => write!(f, "{KEYCHAIN_PREFIX}{key}"),
            Self::LocalJson { secret_id } => write!(f, "{LOCAL_JSON_PREFIX}{secret_id}"),
            Self::Magpie { source_key } => write!(f, "{MAGPIE_PREFIX}{source_key}"),
        }
    }
}

/// 构造 `localjson:v1:` 引用。ID 非法时返回错误而不是拼出一个坏串。
pub fn local_json_ref(secret_id: &str) -> Result<String, CredentialError> {
    if !valid_secret_id(secret_id) {
        return Err(CredentialError::new(
            CredentialErrorKind::Unsafe,
            "本地 JSON 引用含非法 ID",
        ));
    }
    Ok(format!("{LOCAL_JSON_PREFIX}{secret_id}"))
}

/// 构造 `magpie:v1:` 引用。
pub fn magpie_ref(source_key: &str) -> Result<String, CredentialError> {
    if !valid_source_key(source_key) {
        return Err(CredentialError::new(
            CredentialErrorKind::Unsafe,
            "Magpie 引用含非法来源键",
        ));
    }
    Ok(format!("{MAGPIE_PREFIX}{source_key}"))
}

fn has_control_or_space(raw: &str) -> bool {
    raw.chars().any(|c| c.is_control() || c.is_whitespace())
}

/// 旧键形状：`workbuddy/<id>/<kind>/access`。允许 `/`，但不许空段与 `..`。
fn valid_keychain_key(key: &str) -> bool {
    if key.is_empty() || key.len() > 256 {
        return false;
    }
    if !key
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '.' | '-'))
    {
        return false;
    }
    key.split('/')
        .all(|seg| !seg.is_empty() && seg != "." && seg != "..")
}

/// 本地 JSON 的 ID：UUID 形状，只用 `[A-Za-z0-9-]`。`.` 与 `/` 一律拒绝，
/// 从字符集上就堵死目录穿越，不依赖后来的 canonicalize。
pub fn valid_secret_id(id: &str) -> bool {
    (8..=64).contains(&id.len())
        && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

/// Magpie 来源键：`workbuddy`、`workbuddy#<uid>`、`qoder-cn#<uid>` 这类。
fn valid_source_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= 128
        && !key.contains("..")
        && key
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '#' | '_' | '-' | '.' | ':' | '@'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_ref_round_trips_and_matches_ids_helper() {
        let key = crate::auth::ids::access_key("0123456789abcdef", crate::auth::ids::KIND_OWN);
        let raw = crate::auth::ids::keychain_ref(&key);
        let parsed = CredentialRef::parse(&raw).expect("parse");
        assert_eq!(parsed, CredentialRef::Keychain { key: key.clone() });
        assert_eq!(parsed.to_string(), raw);
        assert_eq!(parsed.scheme(), "keychain");
        assert!(parsed.needs_system_authorization());
        assert!(!parsed.is_dynamic());
    }

    #[test]
    fn local_json_ref_round_trips() {
        let id = uuid::Uuid::new_v4().to_string();
        let raw = local_json_ref(&id).expect("build");
        assert_eq!(raw, format!("localjson:v1:{id}"));
        let parsed = CredentialRef::parse(&raw).expect("parse");
        assert_eq!(parsed, CredentialRef::LocalJson { secret_id: id });
        assert_eq!(parsed.to_string(), raw);
        assert_eq!(parsed.scheme(), "localjson");
        assert!(!parsed.needs_system_authorization());
    }

    #[test]
    fn magpie_ref_round_trips_with_uid_suffix() {
        let raw = magpie_ref("workbuddy#6f1c2d94").expect("build");
        assert_eq!(raw, "magpie:v1:workbuddy#6f1c2d94");
        let parsed = CredentialRef::parse(&raw).expect("parse");
        assert_eq!(parsed.target(), "workbuddy#6f1c2d94");
        assert!(parsed.is_dynamic());
        assert!(!parsed.needs_system_authorization());
    }

    #[test]
    fn unknown_scheme_is_rejected_instead_of_falling_back_to_keychain() {
        for raw in [
            "secret-store:foo",
            "localjson:deadbeef-cafe-4000-8000-000000000000", // 缺版本段
            "localjson:v2:deadbeef-cafe-4000-8000-000000000000",
            "magpie:workbuddy",
            "workbuddy/0123456789abcdef/own/access", // 忘写前缀的裸键
            "",
            "   ",
        ] {
            let err = CredentialRef::parse(raw).expect_err(raw);
            assert!(
                matches!(
                    err.kind,
                    CredentialErrorKind::Unsupported | CredentialErrorKind::Unsafe
                ),
                "{raw} => {err}"
            );
        }
    }

    #[test]
    fn traversal_and_separators_are_rejected() {
        for bad in [
            "localjson:v1:../../etc/passwd",
            "localjson:v1:a/b",
            "magpie:v1:..",
        ] {
            assert!(CredentialRef::parse(bad).is_err(), "必须拒绝：{bad}");
        }
        assert!(credential_error_of("localjson:v1:../x").kind == CredentialErrorKind::Unsafe);
    }

    fn credential_error_of(raw: &str) -> CredentialError {
        CredentialRef::parse(raw).expect_err(raw)
    }

    #[test]
    fn whitespace_and_control_chars_are_rejected() {
        assert!(CredentialRef::parse("keychain:workbuddy/ x/access").is_err());
        assert!(CredentialRef::parse("keychain:workbuddy/x/access\n").is_err());
        assert!(local_json_ref("id with space").is_err());
        assert!(magpie_ref("work buddy").is_err());
    }

    #[test]
    fn keychain_key_rejects_empty_segments_and_dotdot() {
        assert!(CredentialRef::parse("keychain:a//b").is_err());
        assert!(CredentialRef::parse("keychain:a/../b").is_err());
        assert!(CredentialRef::parse("keychain:a/./b").is_err());
        assert!(CredentialRef::parse("keychain:a/b").is_ok());
    }

    #[test]
    fn slot_names_are_stable() {
        assert_eq!(CredentialSlot::Access.as_str(), "access");
        assert_eq!(CredentialSlot::Refresh.to_string(), "refresh");
    }
}
