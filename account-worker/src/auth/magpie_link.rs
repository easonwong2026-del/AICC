//! Magpie 动态关联：按 `source_key` **现读**原始 JSON，不复制令牌。
//!
//! 规则来自 PRD §3：
//!
//! * 只取 access；refresh 连读都不读。Magpie 自己负责刷新。
//! * 每次使用都读最新文件（含短时缓存的话必须校验平台、来源键、UID、有效期）。
//! * 平台 / UID 对不上就是**串号**，必须报错，绝不能把别人的令牌发出去。
//! * 文件读不了 ≠ 没有凭据：前者是环境问题，后者才该提示重新登录。
//!
//! 这个模块只做"给定账号 + 来源键，找到那条登录态"的判断，写路径在
//! `crate::auth::relink` 与 `crate::auth::service::save_magpie`。

use crate::credentials::{CredentialError, CredentialErrorKind};
use crate::providers::magpie;
use crate::storage::AccountRecord;
use crate::storage::AuthorizationRecord;

/// 为什么这条关联不可用。分类是稳定契约，文案才是给人看的。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkProblem {
    /// Magpie 没装、文件读不了 —— 环境问题，不代表账号没授权。
    FileUnavailable,
    /// 文件里没有这个来源键：Magpie 里退登了，或者换了账号键。
    SourceMissing,
    /// 来源键对应的不是这个平台（CN / International 串了）。
    PlatformMismatch,
    /// 来源键对应的 UID 不是这个账号。
    UidMismatch,
    /// 令牌已过期：等 Magpie 刷新，或改用自有授权。
    Expired,
}

impl LinkProblem {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FileUnavailable => "magpie_unavailable",
            Self::SourceMissing => "magpie_link_missing",
            Self::PlatformMismatch => "magpie_platform_mismatch",
            Self::UidMismatch => "magpie_uid_mismatch",
            Self::Expired => "magpie_token_expired",
        }
    }

    /// 归类给 Broker。三档必须分开：
    ///
    /// * 串号 → `Conflict`（关联指错账号，界面要提示"重新关联"而不是"重新登录"）；
    /// * 文件读不了 → `Unavailable`（环境问题，别把账号状态写坏）；
    /// * 键不存在 / 已过期 → `NotFound`（这一次没有可用令牌）。
    pub fn error_kind(self) -> CredentialErrorKind {
        match self {
            Self::PlatformMismatch | Self::UidMismatch => CredentialErrorKind::Conflict,
            Self::FileUnavailable => CredentialErrorKind::Unavailable,
            Self::SourceMissing | Self::Expired => CredentialErrorKind::NotFound,
        }
    }
}

/// 平台是否一致。
///
/// 只比 `platform` 不够：WorkBuddy 国内版与国际版都是 `workbuddy`，请求的是两个
/// 不同域名，拿错了会把 CN 令牌发给 International API。所以还要比 provider。
pub(crate) fn platform_matches(login: &magpie::MagpieLogin, account: &AccountRecord) -> bool {
    if login.provider.platform != account.platform {
        return false;
    }
    match (login.account_provider(), account.provider.as_str()) {
        (Some(login_provider), account_provider) if !account_provider.is_empty() => {
            login_provider == account_provider
        }
        // 老账号可能没有 provider 列（v13 之前）：此时退回只比 platform。
        _ => true,
    }
}

/// 在给定登录列表里找一条**平台 + UID + 来源键**都对得上的记录。
///
/// 抽出来是为了让写路径（重新关联）和读路径用同一套判据 —— 两处各写一遍，
/// 迟早出现"UI 说关联成功、读的时候又串号"。
///
/// `now_ms = None` 表示**不校验有效期**：重新关联只关心"这条引用指向谁"，
/// 令牌新旧由读路径判。
pub fn find_link<'a>(
    logins: &'a [magpie::MagpieLogin],
    account: &AccountRecord,
    source_key: &str,
    now_ms: Option<i64>,
) -> Result<&'a magpie::MagpieLogin, LinkProblem> {
    let login = logins
        .iter()
        .find(|login| login.key == source_key)
        .ok_or(LinkProblem::SourceMissing)?;
    if !platform_matches(login, account) {
        return Err(LinkProblem::PlatformMismatch);
    }
    if login.uid != account.uid {
        return Err(LinkProblem::UidMismatch);
    }
    if let Some(now_ms) = now_ms {
        if login.expires_at_ms.map(|at| at <= now_ms).unwrap_or(false) {
            return Err(LinkProblem::Expired);
        }
    }
    Ok(login)
}

/// 单次 RPC 内的只读 Magpie 快照缓存。
///
/// `accounts.list` 会依次做镜像到期修正与每条授权行的动态状态/到期探测；
/// 不缓存的话有 N 个借用账号就会把 `plugin-auth.json` 重复读盘并解析 3N 次。
/// 按需惰性读取：没有 Magpie 关联时一次都不碰磁盘，有多个账号时整次调用只读 1 次。
pub(crate) struct MagpieLoginsCache {
    loaded: std::cell::OnceCell<Option<Vec<magpie::MagpieLogin>>>,
}

impl MagpieLoginsCache {
    pub(crate) fn new() -> Self {
        Self {
            loaded: std::cell::OnceCell::new(),
        }
    }

    pub(crate) fn get(&self) -> Option<&[magpie::MagpieLogin]> {
        self.loaded
            .get_or_init(|| magpie::read_logins().ok())
            .as_deref()
    }
}

/// 在已读出的 Magpie 登录列表中查找这条引用的到期时间（RFC3339）。
pub(crate) fn live_expiry_in(
    logins: &[magpie::MagpieLogin],
    account: &AccountRecord,
    row: &AuthorizationRecord,
) -> Option<String> {
    let source_key = row.source_key.as_deref()?;
    let login = find_link(logins, account, source_key, None).ok()?;
    login
        .expires_at_ms
        .and_then(crate::providers::workbuddy_auth::ms_to_rfc3339)
}

/// 现读 Magpie 文件里这条引用的到期时间（RFC3339）。
///
/// 只用于**展示与镜像**：库里的 `token_expires_at` 是导入那一刻的快照，
/// Magpie 自己换过令牌之后它就过期了，镜像再照抄它，界面就会把一份可用授权
/// 显示成「已过期」。这里只读时间，不碰令牌，也不改授权优先级。
///
/// 文件读不到、来源键不在、平台或 UID 对不上，一律返回 `None`，调用方保留原值。
/// `now_ms = None`：**已经过期的令牌也要拿到它的到期时间**，界面照实显示过期，
/// 而不是因为"读不到"退回一份更旧的快照。
pub(crate) fn live_expiry(account: &AccountRecord, row: &AuthorizationRecord) -> Option<String> {
    let logins = magpie::read_logins().ok()?;
    live_expiry_in(&logins, account, row)
}

/// 在已读出的 Magpie 登录列表中校验并返回可用访问令牌（**只返回 access**）。
pub(crate) fn access_in(
    logins: &[magpie::MagpieLogin],
    account: &AccountRecord,
    source_key: &str,
) -> Result<String, CredentialError> {
    let login =
        find_link(logins, account, source_key, Some(magpie::now_ms())).map_err(|problem| {
            CredentialError::new(
                problem.error_kind(),
                format!("Magpie 关联不可用：{}", problem.as_str()),
            )
        })?;
    if login.access_token.trim().is_empty() {
        return Err(CredentialError::new(
            CredentialErrorKind::NotFound,
            "Magpie 登录态没有访问令牌",
        ));
    }
    Ok(login.access_token.clone())
}

/// 取当前可用的一条 Magpie 授权。**只返回 access**。
pub fn access_for(account: &AccountRecord, source_key: &str) -> Result<String, CredentialError> {
    let logins = magpie::read_logins().map_err(|err| {
        CredentialError::new(
            LinkProblem::FileUnavailable.error_kind(),
            format!(
                "Magpie 登录态不可读（{}）",
                crate::rpc::sanitize(&err.message())
            ),
        )
    })?;
    access_in(&logins, account, source_key)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(platform: &str, provider: &str, uid: &str) -> AccountRecord {
        AccountRecord {
            id: "acc".into(),
            platform: platform.into(),
            provider: provider.into(),
            uid: uid.into(),
            nickname: String::new(),
            email: None,
            group_id: None,
            auth_status: "valid".into(),
            checkin_enabled: false,
            last_checkin_at: None,
            last_result: None,
            updated_at: String::new(),
            token_expires_at: None,
            auth_domain: None,
            auth_source: "magpie".into(),
            travel_enabled: false,
            auth_epoch: 0,
            auth_mirror_auth_id: None,
        }
    }

    fn login(key: &str, uid: &str, expires_at_ms: Option<i64>) -> magpie::MagpieLogin {
        let provider = magpie::provider_of(key).expect("provider");
        magpie::MagpieLogin {
            key: key.to_string(),
            provider,
            uid: uid.to_string(),
            nickname: None,
            domain: "https://example".into(),
            access_token: "MAGPIE-ACCESS".into(),
            expires_at_ms,
        }
    }

    #[test]
    fn matching_requires_key_platform_provider_and_uid() {
        let cn = account("workbuddy", "workbuddy", "uid-1");
        let logins = vec![login("workbuddy#abc", "uid-1", Some(10_000))];
        let hit = find_link(&logins, &cn, "workbuddy#abc", Some(1_000)).expect("hit");
        assert_eq!(hit.uid, "uid-1");

        assert_eq!(
            find_link(&logins, &cn, "workbuddy#nope", Some(1_000)).unwrap_err(),
            LinkProblem::SourceMissing
        );
        let other_uid = account("workbuddy", "workbuddy", "uid-2");
        assert_eq!(
            find_link(&logins, &other_uid, "workbuddy#abc", Some(1_000)).unwrap_err(),
            LinkProblem::UidMismatch
        );
    }

    #[test]
    fn cn_and_international_never_cross_over() {
        let international = account("workbuddy", "workbuddy_ai", "uid-1");
        let logins = vec![login("workbuddy#abc", "uid-1", Some(10_000))];
        assert_eq!(
            find_link(&logins, &international, "workbuddy#abc", Some(1_000)).unwrap_err(),
            LinkProblem::PlatformMismatch
        );

        // 反过来：国际版的令牌也不能给国内版用。
        let cn = account("workbuddy", "workbuddy", "uid-1");
        let logins = vec![login("workbuddy-ai#abc", "uid-1", Some(10_000))];
        assert_eq!(
            find_link(&logins, &cn, "workbuddy-ai#abc", Some(1_000)).unwrap_err(),
            LinkProblem::PlatformMismatch
        );
    }

    #[test]
    fn expired_tokens_are_not_usable() {
        let cn = account("workbuddy", "workbuddy", "uid-1");
        let logins = vec![login("workbuddy#abc", "uid-1", Some(500))];
        assert_eq!(
            find_link(&logins, &cn, "workbuddy#abc", Some(1_000)).unwrap_err(),
            LinkProblem::Expired
        );
        // 没写过期时间时按"可能有效"处理：真失效由 401 判出来。
        let logins = vec![login("workbuddy#abc", "uid-1", None)];
        assert!(find_link(&logins, &cn, "workbuddy#abc", Some(1_000)).is_ok());
    }

    #[test]
    fn problem_kinds_are_stable_and_split_misbinding_from_unavailable() {
        assert_eq!(LinkProblem::UidMismatch.as_str(), "magpie_uid_mismatch");
        assert_eq!(
            LinkProblem::UidMismatch.error_kind(),
            CredentialErrorKind::Conflict
        );
        assert_eq!(
            LinkProblem::Expired.error_kind(),
            CredentialErrorKind::NotFound
        );
    }

    #[test]
    fn logins_cache_reads_once_per_rpc_scope() {
        magpie::test_set_logins(Some(vec![login("workbuddy#first", "uid-1", Some(10_000))]));
        let cache = MagpieLoginsCache::new();
        let first = cache.get().expect("first read");
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].key, "workbuddy#first");

        // 即使底层数据变化，同一缓存实例也绝不二次读盘。
        magpie::test_set_logins(Some(vec![login("workbuddy#second", "uid-2", Some(20_000))]));
        let cached = cache.get().expect("cached read");
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].key, "workbuddy#first");

        magpie::test_reset();
    }
}
