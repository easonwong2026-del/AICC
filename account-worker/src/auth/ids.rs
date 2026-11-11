//! 授权的标识与钥匙串键名（G5）。
//!
//! 这一层只做纯字符串计算，不碰数据库也不碰钥匙串：迁移回填、Broker、RPC 校验
//! 都要用同一套规则，任何一处另写一遍都会让"同一个授权有两个名字"。

use sha2::{Digest, Sha256};

/// 自有授权（AICC 扫码 / 刷新）。
pub const KIND_OWN: &str = "own";
/// 借用授权（从 Magpie 导入的 access，永不持有 refresh）。
pub const KIND_MAGPIE: &str = "magpie";

pub const KINDS: &[&str] = &[KIND_OWN, KIND_MAGPIE];

/// SQLite 里的引用前缀：`keychain:workbuddy/<account_id>/own/access`。
pub const REF_PREFIX: &str = "keychain:";

pub fn is_known_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// 授权主键 = `sha256(account_id + NUL + kind)` 前 8 字节，16 位 hex。
///
/// 确定性有两个用处：回填时直接算出最终值（不需要 `seed-*` 之类的临时 ID），
/// 以及任何时候都能从（账号, kind）重算回来核对库里那一行是不是它。
pub fn auth_id(account_id: &str, kind: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(account_id.as_bytes());
    hasher.update([0u8]);
    hasher.update(kind.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(8)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 新键：`workbuddy/<account_id>/<kind>/<access|refresh>`。
///
/// `magpie` 只有 `access` —— 这个函数能拼出 `magpie/refresh`，但没有任何调用点会这么拼，
/// 表上的 `CHECK` 也是同一道闸。
pub fn secret_key(account_id: &str, kind: &str, which: &str) -> String {
    format!("workbuddy/{account_id}/{kind}/{which}")
}

pub fn access_key(account_id: &str, kind: &str) -> String {
    secret_key(account_id, kind, "access")
}

pub fn refresh_key(account_id: &str, kind: &str) -> String {
    secret_key(account_id, kind, "refresh")
}

pub fn keychain_ref(key: &str) -> String {
    format!("{REF_PREFIX}{key}")
}

/// 引用 → 钥匙串键。只接受我们自己写出来的形状，别的一律 None。
pub fn ref_to_key(reference: &str) -> Option<&str> {
    reference.strip_prefix(REF_PREFIX).filter(|k| !k.is_empty())
}

/// 旧键（v5 及以前）：`workbuddy/<uid>/access|refresh`。
///
/// 只留给迁移与回滚读：v8 才会删。RPC 不接受这个形状。
pub fn legacy_keys(uid: &str) -> (String, String) {
    (
        format!("workbuddy/{uid}/access"),
        format!("workbuddy/{uid}/refresh"),
    )
}

/// 旧键的 kind 归属：先看账号当前来源，空值按 own。
pub fn kind_from_source(source: &str) -> &'static str {
    if source == KIND_MAGPIE {
        KIND_MAGPIE
    } else {
        KIND_OWN
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_id_is_deterministic_and_kind_scoped() {
        let id = auth_id("6f1c2d9400001111", KIND_OWN);
        assert_eq!(id, auth_id("6f1c2d9400001111", KIND_OWN));
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(id, auth_id("6f1c2d9400001111", KIND_MAGPIE));
        assert_ne!(id, auth_id("6f1c2d9400001112", KIND_OWN));
    }

    #[test]
    fn auth_id_does_not_depend_on_separator_ambiguity() {
        // account_id 是 16 hex，不会含 NUL；这里再钉一次，防止将来改用可拼接口径。
        assert_ne!(auth_id("ab", "c"), auth_id("a", "bc"));
    }

    #[test]
    fn key_shape_matches_the_allowlist() {
        let key = access_key("0123456789abcdef", KIND_OWN);
        assert_eq!(key, "workbuddy/0123456789abcdef/own/access");
        assert_eq!(
            refresh_key("0123456789abcdef", KIND_OWN),
            "workbuddy/0123456789abcdef/own/refresh"
        );
        assert_eq!(ref_to_key(&keychain_ref(&key)), Some(key.as_str()));
        assert_eq!(ref_to_key("plain-key"), None);
    }

    #[test]
    fn legacy_keys_stay_on_the_old_shape() {
        let (access, refresh) = legacy_keys("6f1c2d94-1111");
        assert_eq!(access, "workbuddy/6f1c2d94-1111/access");
        assert_eq!(refresh, "workbuddy/6f1c2d94-1111/refresh");
    }
}
