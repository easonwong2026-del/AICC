//! Qoder **服务账号**的删除（PR #77 / CRED-3b 重写）。
//!
//! 与 `qoder_profiles` 里的"本机客户端 Profile"没有任何关系：这里删的是平台账号
//! 以及 AICC 自己持有的凭据，**不碰 Magpie 的配置，也不删 Qoder 客户端的本机数据**。
//!
//! ## 顺序不是随意的
//!
//! ```text
//! ⓪ 读授权快照 → 制定清理计划     → 读不出来 / 引用不可信就一个字节都不动
//! ① 关自动领取 + 推进 auth_epoch  → 让"以后"和"在途"都不再动这个账号
//! ② 按计划删凭据                  → 唯一需要重试的一步
//! ③ 删账号行                      → 授权行、领取记录级联删除，额度快照显式清理
//! ```
//!
//! **不能先删行**：行删掉之后就没有任何地方记录"这个账号曾经有哪些凭据引用"，
//! 文件 / 钥匙串里的令牌会变成谁也管不着的孤儿。反过来，凭据删不掉时**必须保留行**
//! 并报错——用户再点一次就能接着做（缺失的算已删，可自愈）。
//!
//! ## 为什么按引用清理，而不是删固定的三条键
//!
//! CRED-3b 起凭据可能在本地 JSON 文件里，文件名是随机 UUID，**只有授权行知道**。
//! 固定删三条钥匙串键既清不掉文件（孤儿明文令牌留在盘上），又会去碰一个
//! 根本没用过钥匙串的账号（`JSON-only` 账号删除时不该触发任何钥匙串访问，
//! 也就不会弹授权框）。
//!
//! 计划里出现"读不出来的引用"或"指向别的键的引用"时**拒绝删除**：那种情况下
//! 我们无法区分"这个账号的令牌"和"别人的令牌"，删错就是永久丢凭据。
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
use crate::credentials::localjson::CredentialKind;
use crate::credentials::CredentialErrorKind;
use crate::providers::qoder_edition::QoderEdition;
use crate::providers::qoder_secret::{self, Storage};
use crate::rpc::{RpcError, RpcResult};
use crate::securestore::SecretErrorKind;
use crate::storage::qoder_accounts::{QoderAccountRecord, QoderAuthRecord};

/// 该账号可能存在的三条凭据键。
///
/// 只在"授权行一条都没有"的兜底路径里使用：那种情况下我们没有任何引用可依，
/// 只能按老的确定性键名清一遍（老账号的凭据确实在那里）。
pub fn secret_keys(account_id: &str) -> Vec<String> {
    vec![
        crate::providers::qoder_auth::access_key(account_id),
        crate::providers::qoder_auth::refresh_key(account_id),
        crate::providers::qoder_pat::pat_key(account_id),
    ]
}

pub struct Deleted {
    pub account_id: String,
    pub edition: &'static str,
    /// 真的删掉的凭据**份数**（按来源算：设备授权的 access + refresh 算一份）。
    pub secrets_deleted: usize,
    /// 本来就不存在的条数（重试时正常）。
    pub secrets_missing: usize,
    /// 其中来自本地 JSON 文件的条数。
    pub json_secrets: usize,
    /// 其中来自钥匙串的条数。
    pub keychain_secrets: usize,
    /// 只登记了关联、凭据归 Magpie 的来源条数（**不动**）。
    pub magpie_kept: usize,
}

impl Deleted {
    pub fn to_json(&self) -> Value {
        json!({
            "accountId": self.account_id,
            "edition": self.edition,
            "removed": true,
            "secretsDeleted": self.secrets_deleted,
            "secretsMissing": self.secrets_missing,
            "jsonSecretsDeleted": self.json_secrets,
            "keychainSecretsDeleted": self.keychain_secrets,
            "magpieKept": self.magpie_kept,
        })
    }
}

/// 计划里的一条自有凭据：删哪儿，以及**计划时它是什么版本**。
///
/// 版本必须在这里就记下来：删账号要跑"读计划 → 推进 epoch → 逐份删凭据"，
/// 中间另一进程可能对**同一个来源**重新授权（覆盖写同一份文件，`revision + 1`）。
/// 只有拿计划时的版本做比对，才不会把刚保存成功的新令牌删掉（复审第三轮 P0）。
struct PlannedSecret {
    row: QoderAuthRecord,
    kind: CredentialKind,
    /// 来自 JSON 文件（计数用）。
    json: bool,
    /// `None` = 计划时凭据就不在（幂等重试，直接算已删）。
    guard: Option<qoder_secret::DeleteGuard>,
}

/// 一条授权行都没有的旧安装：按确定性键名兜底，并记下计划时读到的值。
struct PlannedLegacy {
    key: String,
    bytes: Vec<u8>,
}

/// 清理计划：哪些凭据要删、哪些来源不动。
///
/// 计划在**动手之前**就要完整算好并验完：删除是不可逆的，一旦"先删掉第一份
/// 合法凭据、再发现第二份串号"，那次拒绝已经造成了永久损失。
struct CleanupPlan {
    secrets: Vec<PlannedSecret>,
    legacy: Vec<PlannedLegacy>,
    magpie_kept: usize,
}

impl CleanupPlan {
    fn is_empty(&self) -> bool {
        self.secrets.is_empty() && self.legacy.is_empty()
    }
}

/// 读授权快照，按引用制定计划，并**逐份预验证归属与版本**。拒绝时一个字节都不动。
///
/// 预验证的四件事：
///
/// 1. 引用能解析（magpie / 未知前缀 → 拒绝）；
/// 2. 钥匙串引用正好指向该来源的键（指向别的账号 → 拒绝）；
/// 3. JSON 文件能读出来、且四个归属字段与这一行一致（串号 → 拒绝）；
/// 4. 记下这一份凭据**当时**的版本（JSON `revision` / 钥匙串字节），删除时用它比对。
///
/// 文件**本来就不在**不算错误：那是上一次删除留下的半途状态，删号要能收敛。
fn plan_cleanup(app: &App, account: &QoderAccountRecord) -> RpcResult<CleanupPlan> {
    let rows = app.with_db(|db| db.list_qoder_authorizations(&account.id))?;
    let mut plan = CleanupPlan {
        secrets: Vec::new(),
        legacy: Vec::new(),
        magpie_kept: 0,
    };

    for row in rows {
        if !qoder_secret::is_own_source(&row.source) {
            // Magpie 的令牌归 Magpie，删 AICC 账号不碰它的文件。
            plan.magpie_kept += 1;
            continue;
        }
        let kind = qoder_secret::kind_of(&row.source);
        let storage = qoder_secret::storage_of(&row).map_err(|err| {
            refuse(
                &row,
                &err,
                "凭据引用不可用，拒绝删除（无法确认这些令牌属于这个账号）",
            )
        })?;
        // `observe_for_delete` 同时做三件事：验归属、读内容、记版本。
        // 读不出来（拒绝访问 / 文件坏）一律整次拒绝 —— 没有版本就没法安全地删。
        let guard = qoder_secret::observe_for_delete(app, account, &row, kind).map_err(|err| {
            let what = match storage {
                Storage::Json => "凭据文件归属校验未通过，拒绝删除（已核对的凭据一份都没动）",
                Storage::Keychain => "钥匙串引用不可用，拒绝删除（可能删到另一个账号的令牌）",
            };
            refuse(&row, &err, what)
        })?;
        plan.secrets.push(PlannedSecret {
            row,
            kind,
            json: storage == Storage::Json,
            guard,
        });
    }

    // 只有**一条授权行都没有**的账号才回退到老的确定性键名：那是本功能之前
    // 装的旧版本，凭据确实在钥匙串里，而库里没有任何引用可依。
    //
    // Magpie-only 账号同样"没有自有授权"，但它有 Magpie 行 —— 凭据归 Magpie，
    // 删除它**不能**碰钥匙串（那会平白触发一次系统授权框）。所以判据是
    // "整张表一行都没有"，不是"自有行为空"。
    if plan.is_empty() && plan.magpie_kept == 0 {
        for key in secret_keys(&account.id) {
            // 读得到才进计划：这条键同样可能被"同一来源重新登录"覆盖，
            // 删除前要拿它当时的值比对。
            match app.secure.get(&key) {
                Ok(bytes) => plan.legacy.push(PlannedLegacy { key, bytes }),
                Err(err) if err.kind == SecretErrorKind::NotFound => {}
                Err(err) => {
                    return Err(RpcError::new(
                        crate::rpc::E_SECURESTORE,
                        format!(
                            "读取旧版钥匙串凭据失败（{}）：账号未做任何改动，可重试",
                            err.message
                        ),
                    )
                    .with_data(json!({
                        "kind": "keychain_read_failed",
                        "stage": "qoder_accounts.delete",
                    })))
                }
            }
        }
    }
    Ok(plan)
}

fn refuse(
    row: &QoderAuthRecord,
    err: &crate::credentials::CredentialError,
    what: &str,
) -> RpcError {
    RpcError::new(crate::rpc::E_SECURESTORE, format!("{what}")).with_data(json!({
        "kind": "credential_ref_unusable",
        "reason": err.kind.as_str(),
        "source": row.source,
        "stage": "qoder_accounts.delete",
    }))
}

pub fn delete(app: &App, account_id: &str) -> RpcResult<Deleted> {
    delete_with(app, account_id, || {}, || {})
}

/// 两个确定性交错点（测试用）：
///
/// * `before_secret_delete`：计划已生成、epoch 已推进，凭据还没动 —— 用来把
///   "同一来源重新授权"固定进这个窗口；
/// * `before_account_delete`：凭据清完、账号行未删 —— 用来把"新增另一份来源"固定进来。
pub(crate) fn delete_with(
    app: &App,
    account_id: &str,
    before_secret_delete: impl FnOnce(),
    before_account_delete: impl FnOnce(),
) -> RpcResult<Deleted> {
    let account = app
        .with_db(|db| db.get_qoder_account(account_id))?
        .ok_or_else(|| RpcError::not_found(format!("Qoder 账号不存在：{account_id}")))?;
    let edition = QoderEdition::parse(&account.edition)
        .ok_or_else(|| RpcError::invalid_params("账号的 edition 非法"))?;

    // ⓪ 先制定计划：引用不可信时直接拒绝，账号、开关、版本号都不动。
    let plan = plan_cleanup(app, &account)?;

    // ① 先让以后与在途都不再动它。开关记下来，失败时还原。
    let previous_auto = app
        .with_db(|db| db.qoder_credits_auto(account_id))
        .unwrap_or(false);
    app.with_db(|db| db.set_qoder_credits_auto(account_id, false))?;
    // 推进后的生命周期就是"我这一代"：最后删账号行时拿它做条件。
    let Some(claimed) = app.with_db(|db| db.bump_qoder_auth_epoch(account_id))? else {
        // 账号在计划之后被别人删掉了：结果等价，不报错。
        return Ok(Deleted {
            account_id: account.id,
            edition: edition.as_str(),
            secrets_deleted: 0,
            secrets_missing: 0,
            json_secrets: 0,
            keychain_secrets: 0,
            magpie_kept: plan.magpie_kept,
        });
    };

    before_secret_delete();

    let mut secrets_deleted = 0usize;
    let mut secrets_missing = 0usize;
    let mut json_secrets = 0usize;
    let mut keychain_secrets = 0usize;
    // 只有**这次真的删掉**凭据、且行仍指着那份旧引用的来源才允许被标成 missing。
    let mut removed_sources: Vec<(String, Option<String>)> = Vec::new();

    // 中断收尾：还原自动开关、把"凭据已删"的行标 missing、返回可重试的错误。
    let abort = |kind: &'static str,
                 message: String,
                 reason: Option<&'static str>,
                 deleted: usize,
                 missing: usize,
                 removed: &[(String, Option<String>)]|
     -> RpcError {
        app.with_db(|db| db.set_qoder_credits_auto(account_id, previous_auto))
            .ok();
        mark_sources_missing(app, account_id, removed, &claimed.generation);
        RpcError::new(crate::rpc::E_SECURESTORE, message).with_data(json!({
            "kind": kind,
            "reason": reason,
            "stage": "qoder_accounts.delete",
            "deleted": deleted,
            "missing": missing,
        }))
    };

    // ② 逐份删凭据，**每一份都带计划时的版本**。
    //
    // 这是"同一来源重新授权"的防线：对方会把同一个文件覆盖成 `revision + 1`，
    // 拿旧版本去删会被拒绝，于是这里立刻停手 —— 新令牌一个字节都不动。
    for planned in &plan.secrets {
        let Some(guard) = planned.guard.as_ref() else {
            // 计划时就不在：幂等重试。
            secrets_missing += 1;
            continue;
        };
        match qoder_secret::delete_guarded(app, &account, &planned.row, planned.kind, guard) {
            Ok(true) => {
                secrets_deleted += 1;
                if planned.json {
                    json_secrets += 1;
                } else {
                    keychain_secrets += 1;
                }
                removed_sources.push((planned.row.source.clone(), planned.row.secret_ref.clone()));
            }
            Ok(false) => secrets_missing += 1,
            // 版本变了：这份凭据在计划之后被重新写过（同一来源重新授权）。
            // **停止删除**，不要动这份新令牌。
            Err(err) if err.kind == CredentialErrorKind::Conflict => {
                return Err(abort(
                    "credential_changed_during_delete",
                    format!(
                        "账号删除期间「{}」的凭据被重新授权，已停止删除以免删掉新令牌；请重试",
                        planned.row.source
                    ),
                    Some(err.kind.as_str()),
                    secrets_deleted,
                    secrets_missing,
                    &removed_sources,
                ));
            }
            Err(err) => {
                return Err(abort(
                    "secret_delete_failed",
                    format!(
                        "删除凭据失败（{}）：账号与授权记录已保留，可重试",
                        crate::rpc::sanitize(&err.message)
                    ),
                    Some(err.kind.as_str()),
                    secrets_deleted,
                    secrets_missing,
                    &removed_sources,
                ));
            }
        }
    }

    for legacy in &plan.legacy {
        match qoder_secret::delete_keychain_guarded(app, &legacy.key, &legacy.bytes) {
            Ok(true) => {
                secrets_deleted += 1;
                keychain_secrets += 1;
            }
            Ok(false) => secrets_missing += 1,
            Err(err) if err.kind == CredentialErrorKind::Conflict => {
                return Err(abort(
                    "credential_changed_during_delete",
                    "账号删除期间旧版钥匙串凭据被重新写入，已停止删除；请重试".to_string(),
                    Some(err.kind.as_str()),
                    secrets_deleted,
                    secrets_missing,
                    &removed_sources,
                ));
            }
            Err(err) => {
                return Err(abort(
                    "secret_delete_failed",
                    format!(
                        "删除钥匙串凭据失败（{}）：账号与授权记录已保留，可重试",
                        crate::rpc::sanitize(&err.message)
                    ),
                    Some(err.kind.as_str()),
                    secrets_deleted,
                    secrets_missing,
                    &removed_sources,
                ));
            }
        }
    }

    before_account_delete();

    // ③ 删行：**必须带生命周期条件**。期间若有人新保存了一份授权（它会读到
    // 推进后的 epoch 并合法提交），那份新凭据不在计划里 —— 直接删账号行就会让
    // 它变成无人引用的有效令牌（复审 P0-B）。
    let deleted = Deleted {
        account_id: account.id.clone(),
        edition: edition.as_str(),
        secrets_deleted,
        secrets_missing,
        json_secrets,
        keychain_secrets,
        magpie_kept: plan.magpie_kept,
    };
    let outcome = app.with_db(|db| db.delete_qoder_account_if_current(&account.id, &claimed));
    match outcome {
        Ok(true) => Ok(deleted),
        // 账号行已经不在（别人删掉了）：结果等价。
        Ok(false)
            if app
                .with_db(|db| db.get_qoder_account(&account.id))
                .map(|found| found.is_none())
                .unwrap_or(false) =>
        {
            Ok(deleted)
        }
        // 删号期间出现了新的授权：**拒绝删除**，保留可重试的记录。
        Ok(false) => {
            app.with_db(|db| db.set_qoder_credits_auto(account_id, previous_auto))
                .ok();
            mark_sources_missing(app, account_id, &removed_sources, &claimed.generation);
            Err(RpcError::new(
                crate::rpc::E_SECURESTORE,
                "删除期间这个账号又写入了一份新授权，已停止删除以保证那份凭据不被孤立；请重试",
            )
            .with_data(json!({
                "kind": "authorization_changed_during_delete",
                "stage": "qoder_accounts.delete",
                "deleted": secrets_deleted,
                "missing": secrets_missing,
            })))
        }
        Err(err) => {
            app.with_db(|db| db.set_qoder_credits_auto(account_id, previous_auto))
                .ok();
            mark_sources_missing(app, account_id, &removed_sources, &claimed.generation);
            Err(err)
        }
    }
}

/// 把**本次确实删掉凭据**的来源标成 `missing`，按 `generation` 核验。
///
/// 三个约束，缺一不可：
///
/// 1. 只标这次删掉的来源 —— 期间新写入的授权不在列表里，绝不能标脏；
/// 2. 只标"还是这个账号"的行 —— 同一个 UID 删了又建时不能动新账号；
/// 3. `missing` 同时让 `credits::resolve_token` 不再使用它。
fn mark_sources_missing(
    app: &App,
    account_id: &str,
    sources: &[(String, Option<String>)],
    generation: &str,
) {
    if sources.is_empty() {
        return;
    }
    app.with_db(|db| {
        for (source, secret_ref) in sources {
            db.set_qoder_auth_status_if_generation(
                account_id,
                source,
                "missing",
                generation,
                secret_ref.as_deref(),
            )?;
        }
        Ok(())
    })
    .ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::localjson::LocalJsonStore;
    use crate::paths::Paths;
    use crate::providers::qoder_auth::{DeviceToken, Identity};
    use crate::securestore::SecureStore;
    use crate::storage::qoder_accounts::{SOURCE_OWN_BROWSER, SOURCE_OWN_PAT};
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

    /// 明文同意 + 全拒绝的钥匙串后端：用来证明 JSON-only 账号的删除**完全不碰钥匙串**。
    fn json_only_app() -> (App, tempfile::TempDir) {
        let (app, dir) = app_with(SecureStore::failing());
        crate::auth::consent::set_json_storage(&app, true).unwrap();
        (app, dir)
    }

    const PAT: &str = "qp_abcdefghijklmnopqrstuvwxyz012345";

    fn token(uid: &str, access: &str) -> DeviceToken {
        DeviceToken {
            access_token: access.to_string(),
            refresh_token: Some("device-refresh".to_string()),
            expires_at_ms: None,
            identity: Identity {
                uid: uid.to_string(),
                name: Some("待删".to_string()),
                email: None,
            },
        }
    }

    fn identity(uid: &str) -> Identity {
        Identity {
            uid: uid.to_string(),
            name: Some("PAT 账号".to_string()),
            email: None,
        }
    }

    /// 老账号：浏览器 access/refresh + PAT 都在钥匙串，外加一条领取记录与一份快照。
    fn seed_keychain(app: &App) -> String {
        let account = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-del", Some("待删"), None))
            .unwrap();
        app.secure
            .set(
                &crate::providers::qoder_auth::access_key(&account.id),
                b"device-access",
            )
            .unwrap();
        app.secure
            .set(
                &crate::providers::qoder_auth::refresh_key(&account.id),
                b"device-refresh",
            )
            .unwrap();
        app.secure
            .set(
                &crate::providers::qoder_pat::pat_key(&account.id),
                PAT.as_bytes(),
            )
            .unwrap();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                None,
                Some(&qoder_secret::keychain_reference(
                    &account.id,
                    SOURCE_OWN_BROWSER,
                )),
                "valid",
                None,
                None,
            )?;
            db.upsert_qoder_authorization(
                &account.id,
                SOURCE_OWN_PAT,
                None,
                Some(&qoder_secret::keychain_reference(
                    &account.id,
                    SOURCE_OWN_PAT,
                )),
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

    /// 新账号：设备授权 + PAT 都在本地 JSON 文件里。
    fn seed_json(app: &App) -> String {
        crate::providers::qoder_auth::persist(app, QoderEdition::Cn, &token("u-del", "dt-1"))
            .unwrap();
        let saved =
            crate::providers::qoder_pat::persist(app, QoderEdition::Cn, PAT, &identity("u-del"))
                .unwrap();
        saved.account_id
    }

    fn secrets_left(app: &App, account_id: &str) -> usize {
        secret_keys(account_id)
            .iter()
            .filter(|key| app.secure.get(key).is_ok())
            .count()
    }

    fn json_files(app: &App) -> usize {
        LocalJsonStore::for_paths(&app.paths)
            .scan()
            .map(|report| report.secrets.len())
            .unwrap_or(0)
    }

    #[test]
    fn deleting_a_keychain_account_removes_secrets_rows_runs_and_snapshots() {
        let (app, _dir) = test_app();
        let id = seed_keychain(&app);
        assert_eq!(secrets_left(&app, &id), 3);

        let deleted = delete(&app, &id).unwrap();
        // 按**来源**计数：设备授权（access + refresh 两条键）算一份，PAT 算一份。
        assert_eq!(deleted.secrets_deleted, 2);
        assert_eq!(deleted.secrets_missing, 0);
        assert_eq!(deleted.keychain_secrets, 2);
        assert_eq!(deleted.json_secrets, 0);
        assert_eq!(secrets_left(&app, &id), 0, "三条钥匙串键都要清掉");

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

    /// JSON-only 账号：**全程不访问钥匙串**（后端对任何操作都报错）。
    #[test]
    fn deleting_a_json_only_account_never_touches_the_keychain() {
        let (app, _dir) = json_only_app();
        let id = seed_json(&app);
        assert_eq!(json_files(&app), 2, "设备授权 + PAT 各一个文件");

        let deleted = delete(&app, &id).expect("JSON-only 账号必须能在钥匙串全拒的情况下删掉");
        assert_eq!(deleted.json_secrets, 2);
        assert_eq!(deleted.keychain_secrets, 0);
        assert_eq!(deleted.secrets_deleted, 2);
        assert_eq!(json_files(&app), 0, "两份文件都要清掉");
        assert!(app
            .with_db(|db| db.get_qoder_account(&id))
            .unwrap()
            .is_none());
    }

    /// 混合账号：一份 JSON、一份钥匙串。两种都要删干净。
    #[test]
    fn a_mixed_account_cleans_both_backends() {
        let (app, _dir) = test_app();
        // 先写钥匙串版 PAT，再同意明文并做设备授权 → 同一账号两种存储并存。
        let saved =
            crate::providers::qoder_pat::persist(&app, QoderEdition::Cn, PAT, &identity("u-mix"))
                .unwrap();
        crate::auth::consent::set_json_storage(&app, true).unwrap();
        crate::providers::qoder_auth::persist(&app, QoderEdition::Cn, &token("u-mix", "dt-1"))
            .unwrap();

        let deleted = delete(&app, &saved.account_id).unwrap();
        assert_eq!(deleted.json_secrets, 1);
        assert_eq!(deleted.keychain_secrets, 1);
        assert_eq!(json_files(&app), 0);
        assert_eq!(secrets_left(&app, &saved.account_id), 0);
    }

    /// Keychain 删不掉：**行必须留下**，否则令牌就再也没人认领。
    #[test]
    fn a_failed_secret_delete_keeps_the_account_and_restores_the_auto_flag() {
        let (app, _dir) = app_with(SecureStore::faulty(&[], &["qoder/"]));
        let id = seed_keychain(&app);
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

    /// 重试是可自愈的：第一次已删掉一部分，第二次按缺失跳过并走完。
    #[test]
    fn deleting_twice_finishes_the_job() {
        let (app, _dir) = test_app();
        let id = seed_keychain(&app);
        // 模拟"上次删了就崩"：只删掉一条键。
        app.secure
            .delete(&crate::providers::qoder_auth::refresh_key(&id))
            .expect("预置删除");

        let deleted = delete(&app, &id).unwrap();
        // 少了 refresh 键不影响这一份授权被删掉：仍按来源算一份。
        assert_eq!(deleted.secrets_deleted, 2);
        assert_eq!(secrets_left(&app, &id), 0);
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

    /// Magpie 关联：只登记关联，凭据归 Magpie —— 删除时不碰它的文件，也不报 `hasSecret`。
    ///
    /// 复审 P0-3：Magpie-only 账号**不是**"没有任何授权行"的旧安装，所以一个钥匙串
    /// 键都不该碰。这里用"钥匙串删除全部失败"的后端做真证明：真碰了就会报错。
    #[test]
    fn a_magpie_link_is_counted_but_never_deleted() {
        let (app, _dir) = app_with(SecureStore::faulty(&[], &["qoder/"]));
        app.secure.arm();
        let account = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-mag", None, None))
            .unwrap();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                "magpie",
                Some("qoder-cn#1"),
                None,
                "valid",
                None,
                None,
            )
        })
        .unwrap();

        let deleted = delete(&app, &account.id).expect("Magpie-only 账号必须能删掉");
        assert_eq!(deleted.magpie_kept, 1);
        assert_eq!(deleted.secrets_deleted, 0);
        assert_eq!(deleted.secrets_missing, 0, "Magpie-only 不许访问钥匙串");
        assert_eq!(deleted.keychain_secrets, 0);
        assert!(app
            .with_db(|db| db.get_qoder_account(&account.id))
            .unwrap()
            .is_none());
    }

    /// 复审 P0-3：**先完整验归属，再动手**。
    ///
    /// 两份 JSON 里第二份串到了别的授权：整次删除必须拒绝，而且第一份（合法的）
    /// 凭据内容、revision、自动领取开关、生命周期都得原样保留。
    #[test]
    fn a_foreign_secret_aborts_the_whole_delete_before_anything_is_removed() {
        let (app, _dir) = json_only_app();
        let id = seed_json(&app);
        assert_eq!(json_files(&app), 2);

        // 把 PAT 那一行指到一份"属于别人"的文件上（authorizationId 不匹配）。
        let store = LocalJsonStore::for_paths(&app.paths);
        let foreign = store
            .create(crate::credentials::localjson::SecretDraft::oauth(
                "qz_someone_else",
                "qoder_cn",
                "FOREIGN",
            ))
            .unwrap();
        let foreign_ref = format!("localjson:v1:{}", foreign.secret_id);
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &id,
                SOURCE_OWN_PAT,
                None,
                Some(&foreign_ref),
                "valid",
                None,
                None,
            )
        })
        .unwrap();

        // 记录删除前的状态：合法凭据的字节与 revision、自动开关、生命周期。
        let browser = app
            .with_db(|db| db.get_qoder_authorization(&id, SOURCE_OWN_BROWSER))
            .unwrap()
            .expect("设备授权行");
        let browser_secret_id = browser
            .secret_ref
            .as_deref()
            .unwrap()
            .trim_start_matches("localjson:v1:")
            .to_string();
        let before = store.load(&browser_secret_id).unwrap();
        app.with_db(|db| db.set_qoder_credits_auto(&id, true))
            .unwrap();
        let lifecycle_before = app.with_db(|db| db.qoder_lifecycle(&id)).unwrap().unwrap();

        let err = delete(&app, &id).err().expect("串号必须让整次删除失败");
        assert_eq!(
            err.data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(Value::as_str),
            Some("credential_ref_unusable")
        );

        // 合法凭据一个字节都没动。
        let after = store.load(&browser_secret_id).expect("第一份凭据必须还在");
        assert_eq!(after.revision, before.revision);
        assert_eq!(
            after
                .token(crate::credentials::reference::CredentialSlot::Access)
                .unwrap(),
            before
                .token(crate::credentials::reference::CredentialSlot::Access)
                .unwrap()
        );
        assert!(
            store.load(&foreign.secret_id).is_ok(),
            "发现的串号文件也不许删"
        );
        assert_eq!(json_files(&app), 3, "一份都不许少");

        // 账号、行、开关、生命周期全都保持原样（没有推进 epoch）。
        assert!(app
            .with_db(|db| db.get_qoder_account(&id))
            .unwrap()
            .is_some());
        assert_eq!(
            app.with_db(|db| db.list_qoder_authorizations(&id))
                .unwrap()
                .len(),
            2
        );
        assert!(app.with_db(|db| db.qoder_credits_auto(&id)).unwrap());
        assert_eq!(
            app.with_db(|db| db.qoder_lifecycle(&id)).unwrap().unwrap(),
            lifecycle_before
        );
    }

    /// **复审 P0-B 的确定性交错**：清完凭据、还没删账号行时，另一个窗口保存了一份
    /// 新授权（PAT）。它的文件不在计划里，若直接删账号行就会变成无人引用的有效令牌。
    ///
    /// 正确行为：条件删行发现 `auth_epoch` 变了 → **拒绝删除**，账号与那份新授权都
    /// 保留（下次删除会把它算进计划），并且不报成功。
    #[test]
    fn a_new_authorization_during_delete_aborts_without_orphaning_it() {
        let (app, _dir) = json_only_app();
        // 只种设备授权：PAT 是"删除期间才出现"的那一份。
        crate::providers::qoder_auth::persist(
            &app,
            QoderEdition::Cn,
            &token("u-late", "dt-browser"),
        )
        .unwrap();
        let id = app
            .with_db(|db| db.find_qoder_account("qoder_cn", "u-late"))
            .unwrap()
            .expect("账号")
            .id;
        assert_eq!(json_files(&app), 1);

        let pat = "qp_abcdefghijklmnopqrstuvwxyz012345";
        let err = delete_with(
            &app,
            &id,
            || {},
            || {
                // 另一个窗口在这个窗口期保存了 PAT：读到推进后的生命周期，合法提交。
                crate::providers::qoder_pat::persist(
                    &app,
                    QoderEdition::Cn,
                    pat,
                    &Identity {
                        uid: "u-late".into(),
                        name: None,
                        email: None,
                    },
                )
                .expect("新 PAT 必须保存成功");
            },
        )
        .err()
        .expect("删号期间出现新授权必须拒绝删除");

        assert_eq!(
            err.data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(Value::as_str),
            Some("authorization_changed_during_delete"),
            "{}",
            err.message
        );

        // 账号还在，那份新授权也还在并**被行引用着**（没有孤儿）。
        assert!(app
            .with_db(|db| db.get_qoder_account(&id))
            .unwrap()
            .is_some());
        let rows = app.with_db(|db| db.list_qoder_authorizations(&id)).unwrap();
        let pat_row = rows
            .iter()
            .find(|row| row.source == SOURCE_OWN_PAT)
            .expect("新 PAT 行必须还在");
        let referenced = pat_row.secret_ref.as_deref().expect("引用");
        let secret_id = referenced.trim_start_matches("localjson:v1:");
        let secret = LocalJsonStore::for_paths(&app.paths)
            .load(secret_id)
            .expect("新 PAT 的文件必须还在");
        assert_eq!(
            secret
                .token(crate::credentials::reference::CredentialSlot::Access)
                .unwrap(),
            pat
        );

        // 每个留在盘上的文件都有行引用：没有无主令牌。
        let store = LocalJsonStore::for_paths(&app.paths);
        for entry in store.scan().unwrap().secrets {
            let reference = format!("localjson:v1:{}", entry.secret_id);
            assert!(
                rows.iter()
                    .any(|row| row.secret_ref.as_deref() == Some(reference.as_str())),
                "文件 {} 没有任何行引用",
                entry.secret_id
            );
        }

        // 设备授权那份凭据已经按计划删掉了，行被标成 missing（不会再被 Credits 用）。
        let browser = rows
            .iter()
            .find(|row| row.source == SOURCE_OWN_BROWSER)
            .expect("设备授权行");
        assert_eq!(browser.auth_status, "missing");

        // 重试一次就能删干净。
        let deleted = delete(&app, &id).unwrap();
        assert_eq!(deleted.json_secrets, 1);
        assert!(app
            .with_db(|db| db.get_qoder_account(&id))
            .unwrap()
            .is_none());
        assert_eq!(json_files(&app), 0, "重试后不留文件");
    }

    /// **复审第三轮 P0 的确定性交错**：删除计划读到 `rev 0` 之后、真正删文件之前，
    /// 另一个进程对**同一个来源**重新授权（覆盖写同一份文件 → `rev 1`）。
    ///
    /// 旧实现用无条件删除，会把刚保存成功的新令牌永久删掉，然后才在删账号行时
    /// 发现 epoch 变了。现在按计划时的 `revision` 删除，版本一变就停手。
    #[test]
    fn a_same_source_relogin_during_delete_keeps_the_new_token() {
        let (app, _dir) = json_only_app();
        let saved = crate::providers::qoder_auth::persist(
            &app,
            QoderEdition::Cn,
            &token("u-relogin", "dt-old"),
        )
        .unwrap();
        let id = saved.account_id.clone();
        let row = app
            .with_db(|db| db.get_qoder_authorization(&id, SOURCE_OWN_BROWSER))
            .unwrap()
            .expect("设备授权行");
        let secret_id = row
            .secret_ref
            .as_deref()
            .unwrap()
            .trim_start_matches("localjson:v1:")
            .to_string();
        let store = LocalJsonStore::for_paths(&app.paths);
        assert_eq!(store.load(&secret_id).unwrap().revision, 0);

        let err = delete_with(
            &app,
            &id,
            || {
                // 同一来源重新授权：覆盖写同一个文件（rev 0 → 1），并推进 epoch。
                crate::providers::qoder_auth::persist(
                    &app,
                    QoderEdition::Cn,
                    &token("u-relogin", "dt-new"),
                )
                .expect("重新授权必须成功");
            },
            || {},
        )
        .err()
        .expect("凭据在删除期间被重写，必须停手");

        assert_eq!(
            err.data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(Value::as_str),
            Some("credential_changed_during_delete"),
            "{}",
            err.message
        );
        assert_eq!(
            err.data
                .as_ref()
                .and_then(|d| d.get("deleted"))
                .and_then(Value::as_u64),
            Some(0),
            "停手时一份凭据都没删"
        );

        // 新令牌原样保留，revision 就是它写进去的那一版。
        let after = store.load(&secret_id).expect("文件必须还在");
        assert_eq!(after.revision, 1);
        assert_eq!(
            after
                .token(crate::credentials::reference::CredentialSlot::Access)
                .unwrap(),
            "dt-new"
        );

        // 行仍然 valid、仍指向这份文件；读路径拿到的就是新令牌。
        let row = app
            .with_db(|db| db.get_qoder_authorization(&id, SOURCE_OWN_BROWSER))
            .unwrap()
            .expect("行必须还在");
        assert_eq!(row.auth_status, "valid");
        let account = app
            .with_db(|db| db.get_qoder_account(&id))
            .unwrap()
            .expect("账号必须还在");
        assert_eq!(
            qoder_secret::read(&app, &account, &row, CredentialKind::Oauth).unwrap(),
            "dt-new"
        );

        // 重试一次就能删干净。
        let deleted = delete(&app, &id).unwrap();
        assert_eq!(deleted.json_secrets, 1);
        assert_eq!(json_files(&app), 0);
        assert!(app
            .with_db(|db| db.get_qoder_account(&id))
            .unwrap()
            .is_none());
    }

    /// 同一交错的 PAT 版本：设备授权先被删掉（它没被改），PAT 被重新保存后停手。
    ///
    /// 断言三件事：新 PAT 原样可读、**没有被标成 missing**、已删掉的那份才是 missing。
    #[test]
    fn a_same_source_pat_relogin_during_delete_keeps_the_new_pat() {
        let (app, _dir) = json_only_app();
        let saved = crate::providers::qoder_auth::persist(
            &app,
            QoderEdition::Cn,
            &token("u-pat-race", "dt-1"),
        )
        .unwrap();
        let id = saved.account_id.clone();
        let new_pat = "qp_newnewnewnewnewnewnewnewnewnew02";
        crate::providers::qoder_pat::persist(
            &app,
            QoderEdition::Cn,
            "qp_oldoldoldoldoldoldoldoldoldold01",
            &Identity {
                uid: "u-pat-race".into(),
                name: None,
                email: None,
            },
        )
        .unwrap();

        let pat_row = app
            .with_db(|db| db.get_qoder_authorization(&id, SOURCE_OWN_PAT))
            .unwrap()
            .expect("PAT 行");
        let pat_secret = pat_row
            .secret_ref
            .as_deref()
            .unwrap()
            .trim_start_matches("localjson:v1:")
            .to_string();

        let err = delete_with(
            &app,
            &id,
            || {
                crate::providers::qoder_pat::persist(
                    &app,
                    QoderEdition::Cn,
                    new_pat,
                    &Identity {
                        uid: "u-pat-race".into(),
                        name: None,
                        email: None,
                    },
                )
                .expect("重新保存 PAT 必须成功");
            },
            || {},
        )
        .err()
        .expect("PAT 在删除期间被重写，必须停手");

        assert_eq!(
            err.data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(Value::as_str),
            Some("credential_changed_during_delete"),
            "{}",
            err.message
        );

        let store = LocalJsonStore::for_paths(&app.paths);
        let secret = store.load(&pat_secret).expect("新 PAT 的文件必须还在");
        assert_eq!(
            secret
                .token(crate::credentials::reference::CredentialSlot::Access)
                .unwrap(),
            new_pat
        );

        let rows = app.with_db(|db| db.list_qoder_authorizations(&id)).unwrap();
        let pat = rows
            .iter()
            .find(|row| row.source == SOURCE_OWN_PAT)
            .expect("PAT 行");
        assert_eq!(pat.auth_status, "valid", "赢家的行不许被标脏");
        let account = app
            .with_db(|db| db.get_qoder_account(&id))
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::providers::qoder_pat::read_secret(&app, &account, pat).unwrap(),
            new_pat
        );

        // 设备授权那份确实被删掉了，所以它才是 missing（不会被 Credits 使用）。
        let browser = rows
            .iter()
            .find(|row| row.source == SOURCE_OWN_BROWSER)
            .expect("设备授权行");
        assert_eq!(browser.auth_status, "missing");
        assert_eq!(json_files(&app), 1, "只剩新 PAT 那一份");
    }

    /// 引用指向别的账号的键：**拒绝删除**，账号与凭据都留着。
    #[test]
    fn a_reference_pointing_elsewhere_refuses_the_whole_delete() {
        let (app, _dir) = test_app();
        let account = app
            .with_db(|db| db.upsert_qoder_account("qoder_cn", "u-bad", None, None))
            .unwrap();
        app.secure
            .set(
                &crate::providers::qoder_auth::access_key(&account.id),
                b"keep-me",
            )
            .unwrap();
        app.with_db(|db| {
            db.upsert_qoder_authorization(
                &account.id,
                SOURCE_OWN_BROWSER,
                None,
                Some("keychain:qoder/another-account/own/access"),
                "valid",
                None,
                None,
            )
        })
        .unwrap();

        let err = delete(&app, &account.id)
            .err()
            .expect("引用不可信必须拒绝删除");
        assert_eq!(err.code, crate::rpc::E_SECURESTORE);
        assert_eq!(
            err.data
                .as_ref()
                .and_then(|d| d.get("kind"))
                .and_then(Value::as_str),
            Some("credential_ref_unusable")
        );
        assert!(
            app.with_db(|db| db.get_qoder_account(&account.id))
                .unwrap()
                .is_some(),
            "账号必须留着"
        );
        assert_eq!(secrets_left(&app, &account.id), 1, "凭据一个都不许动");
    }

    /// 删除后重新登录：账号行回来，且只有新授权这一份来源。
    #[test]
    fn signing_in_again_after_a_delete_starts_clean() {
        let (app, _dir) = test_app();
        let id = seed_keychain(&app);
        delete(&app, &id).unwrap();

        let saved =
            crate::providers::qoder_pat::persist(&app, QoderEdition::Cn, PAT, &identity("u-del"))
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
        let id = seed_keychain(&app);
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
    /// 这是 #77 修的那个 P0 的核心断言：新账号的 id 与旧的一样，`auth_epoch`
    /// 也是 0（与旧账号初始值相同），只有 `generation` 不同。快照与领取记录
    /// 都要靠它把旧数据挡在新账号之外。
    #[test]
    fn writes_from_the_old_lifecycle_are_rejected_after_a_relogin() {
        let (app, _dir) = test_app();
        let old_id = seed_keychain(&app);
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
