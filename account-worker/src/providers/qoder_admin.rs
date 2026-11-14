//! Qoder **服务账号**的删除（PR #77）。
//!
//! 与 `qoder_profiles` 里的"本机客户端 Profile"没有任何关系：这里删的是平台账号
//! 以及 AICC 自己持有的凭据，**不碰 Magpie 的配置，也不删 Qoder 客户端的本机数据**。
//!
//! ## 顺序不是随意的
//!
//! ```text
//! ① 关自动领取 + 推进 auth_epoch   → 让"以后"和"在途"都不再动这个账号
//! ② 删钥匙串三条键                 → 唯一需要重试的一步
//! ③ 删账号行                       → 授权行、领取记录级联删除，额度快照显式清理
//! ```
//!
//! **不能先删行**：行删掉之后就没有任何地方记录"这个账号曾经有哪三条键"，
//! 钥匙串里的令牌会变成谁也管不着的孤儿。反过来，钥匙串删不掉时**必须保留行**
//! 并报错——用户再点一次就能接着做（缺失的键按 `NotFound` 视为已删，可自愈）。
//!
//! ## 为什么先推进 `auth_epoch`
//!
//! 一次额度查询或领取要几秒。删号期间在途的请求拿着旧快照与旧令牌，
//! 回来后可能把结果写进快照表，甚至再发一次 POST。推进版本号之后：
//!
//! - 额度快照写入带版本核验，版本对不上就不写；
//! - 领取的 POST 前闸门读到版本变了，直接停下。
//!
//! 这个动作**不回滚**：它只会让旧请求作废，留着比回滚安全。

use serde_json::{json, Value};

use crate::app::App;
use crate::providers::qoder_auth;
use crate::providers::qoder_edition::QoderEdition;
use crate::providers::qoder_pat;
use crate::rpc::{RpcError, RpcResult};
use crate::securestore::SecretErrorKind;
use crate::storage::qoder_accounts::{SOURCE_OWN_BROWSER, SOURCE_OWN_PAT};

/// 该账号可能存在的三条凭据键。
///
/// 三条都试、缺的算成功：这样"删到一半失败再重来"能一路走完，
/// 不需要记录哪一步做过了。
pub fn secret_keys(account_id: &str) -> Vec<String> {
    vec![
        qoder_auth::access_key(account_id),
        qoder_auth::refresh_key(account_id),
        qoder_pat::pat_key(account_id),
    ]
}

pub struct Deleted {
    pub account_id: String,
    pub edition: &'static str,
    /// 真的从钥匙串里删掉的条数。
    pub secrets_deleted: usize,
    /// 本来就不存在的条数（重试时正常）。
    pub secrets_missing: usize,
}

impl Deleted {
    pub fn to_json(&self) -> Value {
        json!({
            "accountId": self.account_id,
            "edition": self.edition,
            "removed": true,
            "secretsDeleted": self.secrets_deleted,
            "secretsMissing": self.secrets_missing,
        })
    }
}

pub fn delete(app: &App, account_id: &str) -> RpcResult<Deleted> {
    let account = app
        .with_db(|db| db.get_qoder_account(account_id))?
        .ok_or_else(|| RpcError::not_found(format!("Qoder 账号不存在：{account_id}")))?;
    let edition = QoderEdition::parse(&account.edition)
        .ok_or_else(|| RpcError::invalid_params("账号的 edition 非法"))?;

    // ① 先让以后与在途都不再动它。开关记下来，失败时还原。
    let previous_auto = app
        .with_db(|db| db.qoder_credits_auto(account_id))
        .unwrap_or(false);
    app.with_db(|db| db.set_qoder_credits_auto(account_id, false))?;
    app.with_db(|db| db.bump_qoder_auth_epoch(account_id))?;

    // ② 钥匙串。任何一条删失败都要停手并保留行，否则凭据无主。
    let mut secrets_deleted = 0usize;
    let mut secrets_missing = 0usize;
    for key in secret_keys(account_id) {
        match app.secure.delete(&key) {
            Ok(_) => secrets_deleted += 1,
            Err(err) if err.kind == SecretErrorKind::NotFound => secrets_missing += 1,
            Err(err) => {
                // 只还原自动开关；`auth_epoch` 不回滚（它只让旧请求作废）。
                app.with_db(|db| db.set_qoder_credits_auto(account_id, previous_auto))
                    .ok();
                return Err(RpcError::new(
                    crate::rpc::E_SECURESTORE,
                    format!(
                        "删除钥匙串凭据失败（{}）：账号与授权记录已保留，可重试",
                        err.message
                    ),
                )
                .with_data(json!({
                    "kind": "secret_delete_failed",
                    "stage": "qoder_accounts.delete",
                })));
            }
        }
    }

    // ③ 删行。授权行与领取记录走外键级联，额度快照在 `delete_qoder_account` 里显式清。
    match app.with_db(|db| db.delete_qoder_account(account_id)) {
        Ok(true) => Ok(Deleted {
            account_id: account.id,
            edition: edition.as_str(),
            secrets_deleted,
            secrets_missing,
        }),
        // 行在第一步之后被别人删掉了：结果等价，不报错。
        Ok(false) => Ok(Deleted {
            account_id: account.id,
            edition: edition.as_str(),
            secrets_deleted,
            secrets_missing,
        }),
        Err(err) => {
            // 凭据已经没了但行还在：把来源标成 missing，界面上能看出
            // "凭据已清除、可重试删除"，而不是显示一条"有效"的死授权。
            app.with_db(|db| {
                for source in [SOURCE_OWN_BROWSER, SOURCE_OWN_PAT] {
                    db.set_qoder_auth_status(account_id, source, "missing")?;
                }
                Ok(())
            })
            .ok();
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::providers::qoder_auth::Identity;
    use crate::securestore::SecureStore;
    use crate::storage::qoder_credits::RunInput;

    fn paths(dir: &std::path::Path) -> Paths {
        Paths {
            db_path: dir.join("accounts.sqlite"),
            backups_dir: dir.join("backups"),
            logs_dir: dir.join("logs"),
            wb_switch_home: dir.join("wb-switch-home"),
            data_dir: dir.to_path_buf(),
            wb_home_owned: true,
        }
    }

    fn app_with(secure: SecureStore) -> (App, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let app = App::with_secure(paths(dir.path()), secure);
        (app, dir)
    }

    fn test_app() -> (App, tempfile::TempDir) {
        app_with(SecureStore::memory())
    }

    const PAT: &str = "qp_abcdefghijklmnopqrstuvwxyz012345";

    /// 造一个"三份授权都在"的账号：浏览器 access/refresh + PAT + 一条领取记录 + 一份快照。
    fn seed_full(app: &App) -> String {
        let account = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-del", Some("待删"), None))
            .unwrap();
        app.secure
            .set(&qoder_auth::access_key(&account.id), b"device-access")
            .unwrap();
        app.secure
            .set(&qoder_auth::refresh_key(&account.id), b"device-refresh")
            .unwrap();
        app.secure
            .set(&qoder_pat::pat_key(&account.id), PAT.as_bytes())
            .unwrap();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                None,
                Some("keychain:a"),
                "valid",
                None,
                None,
            )?;
            db.upsert_qoder_authorization(
                &account.id,
                SOURCE_OWN_PAT,
                None,
                Some("keychain:b"),
                "valid",
                None,
                None,
            )?;
            db.record_qoder_credit_run(RunInput {
                account_id: &account.id,
                generation: &account.generation,
                campaign_id: Some("c-1"),
                campaign_key: Some("daily"),
                period_start: Some(1_700_000_000),
                period_end: Some(1_700_086_400),
                outcome: "ok",
                credits: Some(100),
                http_status: Some(200),
                auth_source: Some(SOURCE_OWN_PAT),
                error_kind: None,
                error_detail: None,
                trigger: "manual",
            })?;
            db.save_qoder_quota_snapshot(
                &account.id,
                "qoder_cn",
                "ok",
                &json!({"unit": "credits"}),
                None,
            )
            .map(|_| ())
        })
        .unwrap();
        account.id
    }

    fn secrets_left(app: &App, account_id: &str) -> usize {
        secret_keys(account_id)
            .iter()
            .filter(|key| app.secure.get(key).is_ok())
            .count()
    }

    #[test]
    fn deleting_removes_secrets_rows_runs_and_snapshots() {
        let (app, _dir) = test_app();
        let id = seed_full(&app);
        assert_eq!(secrets_left(&app, &id), 3);

        let deleted = delete(&app, &id).unwrap();
        assert_eq!(deleted.secrets_deleted, 3);
        assert_eq!(deleted.secrets_missing, 0);
        assert_eq!(secrets_left(&app, &id), 0, "三条凭据都要清掉");

        assert!(app
            .with_db(|db| db.get_qoder_account(&id))
            .unwrap()
            .is_none());
        assert!(app
            .with_db(|db| db.list_qoder_authorizations(&id))
            .unwrap()
            .is_empty());
        assert!(app
            .with_db(|db| db.list_qoder_credit_runs(Some(&id), 10))
            .unwrap()
            .is_empty());
        assert!(app
            .with_db(|db| db.latest_quota_snapshot(&id))
            .unwrap()
            .is_none());
    }

    /// Keychain 删不掉：**行必须留下**，否则令牌就再也没人认领。
    #[test]
    fn a_failed_secret_delete_keeps_the_account_and_restores_the_auto_flag() {
        let (app, _dir) = app_with(SecureStore::faulty(&[], &["qoder/"]));
        let id = seed_full(&app);
        app.with_db(|db| db.set_qoder_credits_auto(&id, true))
            .unwrap();
        app.secure.arm();

        let err = delete(&app, &id).err().expect("删钥匙串失败必须报错");
        assert!(err.message.contains("可重试"), "{}", err.message);

        app.secure.disarm();
        assert!(
            app.with_db(|db| db.get_qoder_account(&id))
                .unwrap()
                .is_some(),
            "账号必须留着，否则凭据无主"
        );
        assert_eq!(
            app.with_db(|db| db.list_qoder_authorizations(&id))
                .unwrap()
                .len(),
            2
        );
        assert!(
            app.with_db(|db| db.qoder_credits_auto(&id)).unwrap(),
            "失败后要把自动开关还原成删除前的值"
        );
        assert_eq!(secrets_left(&app, &id), 3, "凭据一条都没少");
    }

    /// 重试是可自愈的：第一次已删掉一部分，第二次按 NotFound 跳过并走完。
    #[test]
    fn deleting_twice_finishes_the_job() {
        let (app, _dir) = test_app();
        let id = seed_full(&app);
        // 模拟"上次删了就崩"：只删掉一条键。
        app.secure
            .delete(&qoder_auth::refresh_key(&id))
            .expect("预置删除");

        let deleted = delete(&app, &id).unwrap();
        // 缺的那条按"已删"处理；内存后端与真钥匙串对"删不存在的键"返回值不同，
        // 所以只断言总数，不钉死两者的拆分。
        assert_eq!(deleted.secrets_deleted + deleted.secrets_missing, 3);
        assert!(app
            .with_db(|db| db.get_qoder_account(&id))
            .unwrap()
            .is_none());
    }

    /// 删不存在的账号：明确 404，不静默成功。
    #[test]
    fn deleting_an_unknown_account_is_reported() {
        let (app, _dir) = test_app();
        let err = delete(&app, "qa_missing").err().expect("必须报错");
        assert_eq!(err.code, crate::rpc::E_NOT_FOUND);
    }

    /// 删除后重新登录：账号行回来，且只有新授权这一份来源。
    #[test]
    fn signing_in_again_after_a_delete_starts_clean() {
        let (app, _dir) = test_app();
        let id = seed_full(&app);
        delete(&app, &id).unwrap();

        let saved = qoder_pat::persist(
            &app,
            QoderEdition::Cn,
            PAT,
            &Identity {
                uid: "u-del".into(),
                name: Some("重新登录".into()),
                email: None,
            },
        )
        .unwrap();
        assert_eq!(saved.account_id, id, "账号 id 由 uid 决定，复用同一个");
        let rows = app.with_db(|db| db.list_qoder_authorizations(&id)).unwrap();
        assert_eq!(rows.len(), 1, "旧的两份来源不能复活");
        assert_eq!(rows[0].source, SOURCE_OWN_PAT);
        assert_eq!(secrets_left(&app, &id), 1);
    }

    /// 删除必须先让在途请求作废：`auth_epoch` 前进，额度快照写入被拒。
    #[test]
    fn deleting_advances_the_epoch_so_late_writes_are_rejected() {
        let (app, _dir) = test_app();
        let id = seed_full(&app);
        let before = app.with_db(|db| db.qoder_lifecycle(&id)).unwrap().unwrap();

        // 先停掉自动领取，再单独走一遍"推进版本"的路径（与 delete 内部一致）。
        app.with_db(|db| db.set_qoder_credits_auto(&id, false))
            .unwrap();
        app.with_db(|db| db.bump_qoder_auth_epoch(&id)).unwrap();
        let after = app.with_db(|db| db.qoder_lifecycle(&id)).unwrap().unwrap();
        assert!(after.auth_epoch > before.auth_epoch);
        assert_eq!(
            after.generation, before.generation,
            "推进 epoch 不换生命周期"
        );
        assert!(!app
            .with_db(|db| db.save_qoder_quota_snapshot(
                &id,
                "qoder_cn",
                "ok",
                &json!({}),
                Some(&before)
            ))
            .unwrap());
    }

    /// 删号后用同一个 UID 重新登录：**旧生命周期的一切写入都必须落不下来**。
    ///
    /// 这是本 PR 修的那个 P0 的核心断言：新账号的 id 与旧的一样，`auth_epoch`
    /// 也是 0（与旧账号初始值相同），只有 `generation` 不同。快照与领取记录
    /// 都要靠它把旧数据挡在新账号之外。
    #[test]
    fn writes_from_the_old_lifecycle_are_rejected_after_a_relogin() {
        let (app, _dir) = test_app();
        let old_id = seed_full(&app);
        let old_life = app
            .with_db(|db| db.qoder_lifecycle(&old_id))
            .unwrap()
            .unwrap();

        // 删掉再重新登录（同一个 uid → 同一个 id）。
        delete(&app, &old_id).unwrap();
        let fresh = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-del", Some("重新登录"), None))
            .unwrap();
        let new_life = app
            .with_db(|db| db.qoder_lifecycle(&fresh.id))
            .unwrap()
            .unwrap();

        assert_eq!(fresh.id, old_id);
        // epoch 归零：这里旧账号有两份来源所以是 2；若旧账号只换过一次授权，
        // 新值就会与旧值都是 0——那正是只比 epoch 挡不住的场景（见 credits 的并发用例）。
        assert_eq!(new_life.auth_epoch, 0, "重建后 epoch 从 0 重新开始");
        assert_ne!(new_life.generation, old_life.generation);

        // 旧生命周期的额度快照写不进来。
        assert!(!app
            .with_db(|db| db.save_qoder_quota_snapshot(
                &old_id,
                "qoder_cn",
                "ok",
                &json!({"stale": true}),
                Some(&old_life)
            ))
            .unwrap());
        assert!(
            app.with_db(|db| db.latest_quota_snapshot(&old_id))
                .unwrap()
                .is_none(),
            "新账号不该看到旧生命周期的快照"
        );

        // 旧生命周期的领取记录也写不进来（否则新账号"今天已经领过"）。
        let err = app
            .with_db(|db| {
                db.record_qoder_credit_run(RunInput {
                    account_id: &old_id,
                    generation: &old_life.generation,
                    campaign_id: Some("c-9"),
                    campaign_key: Some("daily"),
                    period_start: Some(1_700_000_000),
                    period_end: Some(1_700_086_400),
                    outcome: "ok",
                    credits: Some(100),
                    http_status: Some(200),
                    auth_source: Some(SOURCE_OWN_PAT),
                    error_kind: None,
                    error_detail: None,
                    trigger: "manual",
                })
            })
            .err()
            .expect("旧生命周期不许记账");
        assert_eq!(err.code, crate::rpc::E_NOT_FOUND);
        assert!(app
            .with_db(|db| db.list_qoder_credit_runs(Some(&old_id), 10))
            .unwrap()
            .is_empty());

        // 新生命周期自己的写入照常。
        assert!(app
            .with_db(|db| db.record_qoder_credit_run(RunInput {
                account_id: &fresh.id,
                generation: &new_life.generation,
                campaign_id: Some("c-9"),
                campaign_key: Some("daily"),
                period_start: Some(1_700_000_000),
                period_end: Some(1_700_086_400),
                outcome: "ok",
                credits: Some(100),
                http_status: Some(200),
                auth_source: Some(SOURCE_OWN_PAT),
                error_kind: None,
                error_detail: None,
                trigger: "manual",
            }))
            .is_ok());
    }
}
