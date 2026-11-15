//! 旧 Magpie 授权 → 动态关联（v20 的显式重新关联）。
//!
//! v19 及以前的借用授权把 access **复制进了钥匙串**，`access_ref` 是 `keychain:...`。
//! v20 的迁移只加列、不改引用（不猜归属）；要不要换成 `magpie:v1:<source_key>`
//! 由这里显式决定，并留下可审计的返回值。
//!
//! 判据（PRD §3.3）：
//!
//! * 有 `source_key` → 按 key 找，且平台 / UID 必须对得上；
//! * 没有 `source_key` → 按「平台 + provider + UID」唯一匹配，**唯一**才改；
//! * 匹配不到、有歧义、或 Magpie 文件读不了 → 不动这一行，交回界面让用户重新关联；
//! * 旧钥匙串条目**不删**：迁移期要能回滚（PRD §7.2）。
//!
//! 这是**幂等**操作：已经关联过的行不在输入里，重复调用只会重复跳过。

use serde_json::{json, Value};

use crate::app::App;
use crate::credentials::reference;
use crate::providers::magpie;
use crate::rpc::RpcResult;
use crate::storage::authz::STORAGE_MAGPIE;
use crate::storage::AuthorizationRecord;

use super::magpie_link::{self, LinkProblem};

/// 一次重新关联的结果：改了几行、还剩几行、每行为什么没改。
pub fn relink_pending(app: &App, only_account: Option<&str>) -> RpcResult<Value> {
    let rows = app.with_db(|db| db.authorizations_pending_magpie_relink())?;

    // 文件读不了是环境问题，不是"这些账号没授权"：整体记一次原因，逐行回同一个值。
    let logins = magpie::read_logins();
    let now = magpie::now_ms();

    let mut relinked: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    let mut pending = 0usize;

    for row in rows {
        if only_account.is_some_and(|id| id != row.account_id) {
            continue;
        }
        pending += 1;

        let Some(account) = app.with_db(|db| db.get_account(&row.account_id))? else {
            pending -= 1;
            skipped.push(json!({
                "authorizationId": row.id,
                "accountId": row.account_id,
                "reason": "account_missing",
            }));
            continue;
        };

        let target = match &logins {
            Err(err) => Err(("magpie_unavailable".to_string(), Some(err.message()))),
            Ok(logins) => match link_target(logins, &account, &row, now) {
                Ok(key) => Ok(key),
                Err(problem) => Err((problem.as_str().to_string(), None)),
            },
        };

        match target {
            Ok(source_key) => {
                let reference = reference::magpie_ref(&source_key)
                    .map_err(|err| crate::rpc::RpcError::internal(err.message))?;
                let changed = app.with_db(|db| {
                    db.rebind_authorization_ref(
                        &row.id,
                        &reference,
                        STORAGE_MAGPIE,
                        Some(&source_key),
                    )
                })?;
                if changed {
                    pending -= 1;
                    relinked.push(json!({
                        "authorizationId": row.id,
                        "accountId": row.account_id,
                        "sourceKey": source_key,
                        "accessRef": reference,
                    }));
                } else {
                    skipped.push(json!({
                        "authorizationId": row.id,
                        "accountId": row.account_id,
                        "reason": "authorization_missing",
                    }));
                }
            }
            Err((reason, detail)) => {
                skipped.push(json!({
                    "authorizationId": row.id,
                    "accountId": row.account_id,
                    "reason": reason,
                    "message": detail,
                }));
            }
        }
    }

    Ok(json!({
        "relinked": relinked.len(),
        "pending": pending,
        "skipped": skipped,
        // 旧钥匙串条目刻意保留：迁移期要能回滚，清理交给后续的显式操作。
        "legacyKeysKept": true,
    }))
}

/// 决定这一行该关联到哪个来源键。
fn link_target(
    logins: &[magpie::MagpieLogin],
    account: &crate::storage::AccountRecord,
    row: &AuthorizationRecord,
    now_ms: i64,
) -> Result<String, LinkProblem> {
    match row
        .source_key
        .as_deref()
        .filter(|key| !key.trim().is_empty())
    {
        Some(key) => magpie_link::find_link(logins, account, key, None)
            .map(|login| login.key.clone())
            .map_err(|problem| problem),
        None => {
            // 没有来源键：只能靠"平台 + provider + UID"唯一匹配。多于一条就是有歧义，
            // 猜任何一条都可能把两个账号的令牌搞混。
            let candidates: Vec<&magpie::MagpieLogin> = logins
                .iter()
                .filter(|login| magpie_link::platform_matches(login, account))
                .filter(|login| login.uid == account.uid)
                .filter(|login| login.expires_at_ms.map(|at| at > now_ms).unwrap_or(true))
                .collect();
            match candidates.len() {
                1 => Ok(candidates[0].key.clone()),
                0 => Err(LinkProblem::SourceMissing),
                _ => Err(LinkProblem::SourceMissing),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(uid: &str) -> crate::storage::AccountRecord {
        crate::storage::AccountRecord {
            id: "acc".into(),
            platform: "workbuddy".into(),
            provider: "workbuddy".into(),
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

    fn row(source_key: Option<&str>) -> AuthorizationRecord {
        AuthorizationRecord {
            id: "auth".into(),
            account_id: "acc".into(),
            kind: "magpie".into(),
            access_ref: "keychain:workbuddy/acc/magpie/access".into(),
            refresh_ref: None,
            auth_domain: None,
            token_expires_at: None,
            status: "valid".into(),
            version: 0,
            key_migration_state: "ready".into(),
            source_key: source_key.map(str::to_string),
            last_used_at: None,
            last_error: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn login(key: &str, uid: &str) -> magpie::MagpieLogin {
        magpie::MagpieLogin {
            key: key.to_string(),
            provider: magpie::provider_of(key).expect("provider"),
            uid: uid.to_string(),
            nickname: None,
            domain: String::new(),
            access_token: "ACCESS".into(),
            expires_at_ms: None,
        }
    }

    #[test]
    fn known_source_key_is_reused_even_when_the_token_looks_stale() {
        // 重新关联只关心"这条引用指向谁"，不关心令牌是否过期：
        // 过期由读路径判，关系本身是对的。
        let logins = vec![login("workbuddy#abc", "uid-1")];
        let target = link_target(&logins, &account("uid-1"), &row(Some("workbuddy#abc")), 1);
        assert_eq!(target.expect("relink"), "workbuddy#abc");
    }

    #[test]
    fn missing_source_key_requires_a_unique_match() {
        let logins = vec![login("workbuddy#abc", "uid-1")];
        assert_eq!(
            link_target(&logins, &account("uid-1"), &row(None), 1).expect("unique"),
            "workbuddy#abc"
        );

        // 同一 UID 两条登录态：有歧义，不许猜。
        let logins = vec![
            login("workbuddy#abc", "uid-1"),
            login("workbuddy#def", "uid-1"),
        ];
        assert!(link_target(&logins, &account("uid-1"), &row(None), 1).is_err());
    }

    #[test]
    fn mismatched_uid_is_never_relinked() {
        let logins = vec![login("workbuddy#abc", "uid-2")];
        let problem = link_target(&logins, &account("uid-1"), &row(Some("workbuddy#abc")), 1)
            .expect_err("uid 不一致");
        assert_eq!(problem, LinkProblem::UidMismatch);
    }
}
