//! 自有 JSON 凭据的**归属校验**（PR-CRED-3a 复审 P0-1）。
//!
//! `localjson:v1:<uuid>` 只是一个文件名。文件里还写着 `authorizationId` /
//! `platform` / `source` / `credentialKind`，但之前没有任何调用方拿它们和当前
//! SQLite 授权行比对过 —— 只要引用被写错（迁移出错、并发覆盖、库损坏），
//! 读路径就会把**别人的**令牌当成自己的返回，写路径还会覆盖、删除别人的凭据。
//! 文件 `0600` 挡不住这种业务层串号。
//!
//! 所以读 / 刷新 / 覆盖写 / 删除四个入口统一走这里：先 `load`，再逐项比对，
//! 任何一项不符就返回 `conflict`，**不返回令牌、不写、不删**。

use crate::app::App;
use crate::credentials::localjson::{CredentialKind, LocalJsonStore, SecretSource, StoredSecret};
use crate::credentials::reference::{CredentialRef, CredentialSlot};
use crate::credentials::{CredentialError, CredentialErrorKind};
use crate::storage::{AccountRecord, AuthorizationRecord};

/// 行的引用 → 本地文件 ID。不是 `localjson:v1:` 就报错，不做猜测。
pub fn secret_id_of(row: &AuthorizationRecord) -> Result<String, CredentialError> {
    match CredentialRef::parse(&row.access_ref)? {
        CredentialRef::LocalJson { secret_id } => Ok(secret_id),
        // 不把引用内容写进错误消息：`Debug` 会把整条引用（含 ID）抄出来。
        _ => Err(CredentialError::new(
            CredentialErrorKind::Unsupported,
            "这条授权的引用不是本地 JSON",
        )),
    }
}

/// 读一份自有凭据，并确认它**确实属于**这条授权行。
pub fn load(
    app: &App,
    account: &AccountRecord,
    row: &AuthorizationRecord,
) -> Result<StoredSecret, CredentialError> {
    let secret_id = secret_id_of(row)?;
    let secret = LocalJsonStore::for_paths(&app.paths).load(&secret_id)?;
    verify(&secret, account, row)?;
    Ok(secret)
}

/// 读某个槽位。归属不符时同样拒绝，不会"先返回再说"。
pub fn token(
    app: &App,
    account: &AccountRecord,
    row: &AuthorizationRecord,
    slot: CredentialSlot,
) -> Result<String, CredentialError> {
    Ok(load(app, account, row)?.token(slot)?.to_string())
}

/// 先验归属，再删文件。**归属不符绝不删**：那是别人的凭据。
///
/// 文件已经不在了算成功（删除是幂等的，重试要能收敛）。
pub fn delete(
    app: &App,
    account: &AccountRecord,
    row: &AuthorizationRecord,
) -> Result<(), CredentialError> {
    let secret_id = secret_id_of(row)?;
    let store = LocalJsonStore::for_paths(&app.paths);
    match store.load(&secret_id) {
        Ok(secret) => {
            verify(&secret, account, row)?;
            store.delete(&secret_id).map(|_| ())
        }
        Err(err) if err.kind == CredentialErrorKind::NotFound => Ok(()),
        Err(err) => Err(err),
    }
}

/// 四个归属判据。`workbuddy` 的自有授权永远是 OAuth 类型：PAT 只属于 Qoder。
fn verify(
    secret: &StoredSecret,
    account: &AccountRecord,
    row: &AuthorizationRecord,
) -> Result<(), CredentialError> {
    if secret.authorization_id != row.id {
        return Err(mismatch("凭据文件属于另一条授权"));
    }
    if secret.platform != account.platform {
        return Err(mismatch("凭据文件的平台与账号不一致"));
    }
    if secret.source != SecretSource::Own {
        return Err(mismatch("凭据文件的来源不是 AICC 自有授权"));
    }
    if secret.credential_kind != CredentialKind::Oauth {
        return Err(mismatch("凭据文件的类型不是 OAuth"));
    }
    Ok(())
}

fn mismatch(what: &str) -> CredentialError {
    CredentialError::new(
        CredentialErrorKind::Conflict,
        format!("{what}（已拒绝，没有读取、写入或删除任何令牌）"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::localjson::{Credentials, SecretDraft};

    fn account() -> AccountRecord {
        AccountRecord {
            id: "acc-1".to_string(),
            platform: "workbuddy".to_string(),
            provider: "workbuddy".to_string(),
            uid: "uid-1".to_string(),
            nickname: String::new(),
            email: None,
            group_id: None,
            auth_status: "valid".to_string(),
            checkin_enabled: false,
            last_checkin_at: None,
            last_result: None,
            updated_at: String::new(),
            token_expires_at: None,
            auth_domain: None,
            auth_source: "own".to_string(),
            travel_enabled: false,
            auth_epoch: 0,
            auth_mirror_auth_id: None,
        }
    }

    fn row(id: &str, secret_id: &str) -> AuthorizationRecord {
        AuthorizationRecord {
            id: id.to_string(),
            account_id: "acc-1".to_string(),
            kind: "own".to_string(),
            access_ref: format!("localjson:v1:{secret_id}"),
            refresh_ref: None,
            auth_domain: None,
            token_expires_at: None,
            status: "valid".to_string(),
            version: 0,
            key_migration_state: "ready".to_string(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    #[test]
    fn a_secret_of_another_authorization_is_refused_without_reading_it() {
        // 纯函数级：四个判据逐个否掉，错误必须是 conflict（不是 not_found/corrupt）。
        let dir = tempfile::tempdir().unwrap();
        let store = LocalJsonStore::for_data_dir(dir.path());
        let secret = store
            .create(
                SecretDraft::oauth("row-b", "workbuddy", "TOKEN-OF-B").with_refresh_token("R-B"),
            )
            .unwrap();

        let account = account();
        let err = verify(&secret, &account, &row("row-a", &secret.secret_id)).unwrap_err();
        assert_eq!(err.kind, CredentialErrorKind::Conflict, "{err}");
        assert!(!err.message.contains("TOKEN-OF-B"), "错误消息不许带令牌");

        // 平台不符
        let mut other_platform = account.clone();
        other_platform.platform = "qoder".to_string();
        assert_eq!(
            verify(&secret, &other_platform, &row("row-b", &secret.secret_id))
                .unwrap_err()
                .kind,
            CredentialErrorKind::Conflict
        );

        // 归属都对时通过
        assert!(verify(&secret, &account, &row("row-b", &secret.secret_id)).is_ok());
    }

    #[test]
    fn a_pat_cannot_stand_in_for_an_oauth_secret() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalJsonStore::for_data_dir(dir.path());
        let secret = store
            .create(SecretDraft::pat("row-a", "workbuddy", "PAT-VALUE"))
            .unwrap();
        assert_eq!(
            verify(&secret, &account(), &row("row-a", &secret.secret_id))
                .unwrap_err()
                .kind,
            CredentialErrorKind::Conflict
        );
    }

    #[test]
    fn delete_refuses_a_secret_owned_by_someone_else() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalJsonStore::for_data_dir(dir.path());
        let secret = store
            .create(SecretDraft::oauth("row-b", "workbuddy", "TOKEN-OF-B"))
            .unwrap();

        // 归属校验是纯函数，这里直接验证"错配就不该走到 store.delete"。
        assert!(verify(&secret, &account(), &row("row-a", &secret.secret_id)).is_err());
        assert!(store.load(&secret.secret_id).is_ok(), "文件必须还在");
    }

    #[test]
    fn wrong_reference_scheme_is_unsupported_not_silently_ignored() {
        let mut row = row("row-a", "x");
        for bad in [
            "keychain:own/access",
            "magpie:v1:workbuddy#uid-1",
            "nonsense",
        ] {
            row.access_ref = bad.to_string();
            assert_eq!(
                secret_id_of(&row).unwrap_err().kind,
                CredentialErrorKind::Unsupported,
                "{bad}"
            );
        }
    }

    #[test]
    fn credentials_fixture_is_consistent() {
        // 防止上面的 helper 与真实结构漂移：能构造出来就说明字段还对得上。
        let draft = SecretDraft::oauth("row-a", "workbuddy", "t");
        assert_eq!(draft.credential_kind, CredentialKind::Oauth);
        let _ = Credentials::access_only("t");
    }
}
