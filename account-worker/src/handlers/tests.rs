//! 多来源授权的回归测试（G1 / G3 / G5 / G6）。
//!
//! 全部离线：内存钥匙串 + 临时库 + 直接调内部函数。网络路径（探测、签到详情）在
//! `auth::probe` 的纯函数测试与 status 的分支里覆盖，这里只钉凭据归属与版本。

use serde_json::{json, Value};
use std::sync::Arc;
use wb_switch_core::modules::variant::WbVariant;

use super::*;
use crate::auth::{broker, ids, service};
use crate::paths::Paths;
use crate::providers::workbuddy_auth::{Profile, TokenPayload};
use crate::providers::{magpie, workbuddy_auth};
use crate::securestore::SecureStore;
use crate::storage::{AccountFilter, AccountInput};

/// 默认测试环境：**已确认**明文风险（否则新登录会走钥匙串，见 `consent`）。
fn test_app(store: SecureStore) -> (Arc<App>, tempfile::TempDir) {
    let (app, dir) = test_app_without_consent(store);
    crate::auth::consent::set_json_storage(&app, true).expect("enable json storage");
    (app, dir)
}

fn test_app_without_consent(store: SecureStore) -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let data = dir.path().to_path_buf();
    let paths = Paths {
        db_path: data.join("accounts.sqlite"),
        backups_dir: data.join("backups"),
        logs_dir: data.join("logs"),
        wb_switch_home: data.join("wb-switch-home"),
        data_dir: data,
        wb_home_owned: true,
    };
    (Arc::new(App::with_secure(paths, store)), dir)
}

/// 「有效令牌」夹具的有效期：从现在起 30 天。
///
/// 不写死绝对日期。写死的夹具到点就变成"过期令牌"：导入被判 `token_expired`，
/// 镜像兜底换选另一份授权，整组用例在某个日期之后莫名变红（这种炸弹在
/// 2.14.1 里已经排掉两个了）。断言也不要自己写日期 —— 用 `fixture_expiry()`
/// 从**同一份夹具**推导。
const FIXTURE_VALIDITY_DAYS: i64 = 30;

fn fixture_expiry_ms() -> i64 {
    ((magpie::now_ms() / 1000) + FIXTURE_VALIDITY_DAYS * 86_400) * 1000
}

/// 「Magpie 后来换发的新令牌」：比夹具再晚 7 天。同样不写日期。
fn refreshed_expiry_ms() -> i64 {
    ((magpie::now_ms() / 1000) + (FIXTURE_VALIDITY_DAYS + 7) * 86_400) * 1000
}

/// 夹具里那个到期时间对应的 RFC3339 —— 断言的期望值由它推导。
fn fixture_expiry(login: &magpie::MagpieLogin) -> String {
    crate::providers::workbuddy_auth::ms_to_rfc3339(
        login.expires_at_ms.expect("fixture 必须带到期时间"),
    )
    .expect("rfc3339")
}

fn payload(uid: &str, access: &str, refresh: Option<&str>) -> TokenPayload {
    TokenPayload {
        access_token: access.to_string(),
        refresh_token: refresh.map(str::to_string),
        domain: "www.workbuddy.cn".to_string(),
        // 相对时间：这份夹具代表"刚登录拿到的新令牌"。写死日期会变成定时
        // 炸弹——到点之后 `is_usable()` 判过期，镜像兜底换选另一份授权，
        // 断言就会莫名其妙地红。
        expires_at_ms: Some(fixture_expiry_ms()),
        profile: Profile {
            uid: uid.to_string(),
            nickname: Some("测试账号".to_string()),
            email: None,
        },
    }
}

fn magpie_login(uid: &str, access: &str) -> magpie::MagpieLogin {
    magpie::MagpieLogin {
        key: format!("workbuddy#{uid}"),
        provider: magpie::provider_of("workbuddy").unwrap(),
        uid: uid.to_string(),
        nickname: Some("借来的".to_string()),
        domain: "www.workbuddy.cn".to_string(),
        access_token: access.to_string(),
        expires_at_ms: Some(fixture_expiry_ms()),
    }
}

/// `auth.workbuddy.refresh` 也是国内版协议：国际版必须**在查授权之前**被挡住，
/// 而且不能因为一次"刷新"把一份好授权标成 invalid。
/// 档位解析必须严格：写错值不能被"回落国内版"掩盖。
#[test]
fn variant_parsing_is_strict() {
    use wb_switch_core::modules::variant::WbVariant as V;
    assert_eq!(workbuddy_auth::parse_variant("cn"), Some(V::Cn));
    assert_eq!(workbuddy_auth::parse_variant(" CN "), Some(V::Cn));
    assert_eq!(workbuddy_auth::parse_variant("ai"), Some(V::Ai));
    assert_eq!(workbuddy_auth::parse_variant("intl"), Some(V::Ai));
    assert_eq!(workbuddy_auth::parse_variant("bogus"), None);
    assert_eq!(workbuddy_auth::parse_variant(""), None);
}

#[test]
fn provider_mapping_matches_the_v13_model() {
    use wb_switch_core::modules::variant::WbVariant as V;
    assert_eq!(workbuddy_auth::provider_of(V::Cn), "workbuddy");
    assert_eq!(workbuddy_auth::provider_of(V::Ai), "workbuddy_ai");
}

/// 国际版也要能"Magpie 借用的授权 + 自有登录授权"并存：
/// 登录只写 own，导入的 magpie/access 一个字节都不动。
#[test]
fn ai_account_keeps_magpie_and_own_authorizations_side_by_side() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "ai-dual-uid";
    import_logins(
        &app,
        vec![magpie_login_with("workbuddy-ai", uid, "borrowed-ai")],
        &[],
    )
    .expect("import");
    let record = account(&app, uid);
    assert_eq!(record.provider, "workbuddy_ai");

    persist_login(
        &app,
        &payload(uid, "own-access-ai", Some("own-refresh-ai")),
        WbVariant::Ai,
    )
    .expect("browser login");

    let want_ref = format!("magpie:v1:{}", "workbuddy-ai");
    assert_eq!(
        magpie_ref(&app, &record.id).as_deref(),
        Some(want_ref.as_str()),
        "自有登录不能覆盖借用授权"
    );
    assert!(
        magpie_access(&app, &record.id).is_none(),
        "借用授权不进钥匙串"
    );
    assert_eq!(
        own_access(&app, &record.id).as_deref(),
        Some("own-access-ai")
    );
    // 国际版的自有授权同样落 JSON：provider 只影响账号归属，不影响凭据存储。
    assert_eq!(own_row(&app, &record.id).storage_kind(), "localjson");
    assert!(
        read_key(&app, &ids::access_key(&record.id, ids::KIND_OWN)).is_none(),
        "国际版登录也不许写钥匙串"
    );
    let kinds: Vec<String> = app
        .with_db(|db| db.list_authorizations(&record.id))
        .unwrap()
        .into_iter()
        .map(|row| row.kind)
        .collect();
    assert!(kinds.contains(&"own".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"magpie".to_string()), "{kinds:?}");
    assert_eq!(account(&app, uid).provider, "workbuddy_ai");
}

/// `auth.workbuddy.start` 的 variant 参数在**发请求之前**就要校验：
/// 这里用非法值调用真实派发入口，必须直接报参数错，不会有任何网络行为。
#[tokio::test]
async fn start_rejects_an_unknown_variant_without_a_request() {
    let (app, _dir) = test_app(SecureStore::memory());
    let err = dispatch(&app, "auth.workbuddy.start", &json!({ "variant": "lite" }))
        .await
        .unwrap_err();
    assert!(err.message.contains("variant"), "{}", err.message);
}

/// 国际版登录：账号记成 `workbuddy_ai`，凭据进的是同一套按 uid 隔离的键；
/// 同一 UID 换档位登录必须冲突并**回滚**已写入的凭据。
#[test]
fn ai_login_marks_the_provider_and_refuses_to_cross_variants() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "ai-login-uid";
    persist_login(
        &app,
        &payload(uid, "ai-access", Some("ai-refresh")),
        WbVariant::Ai,
    )
    .expect("ai login");

    let record = account(&app, uid);
    assert_eq!(record.provider, "workbuddy_ai");
    assert_eq!(record.platform, "workbuddy");
    assert_eq!(own_access(&app, &record.id).as_deref(), Some("ai-access"));

    // 同一个 UID 用国内版再登录：明确冲突。
    let err = persist_login(
        &app,
        &payload(uid, "cn-access", Some("cn-refresh")),
        WbVariant::Cn,
    )
    .unwrap_err();
    assert_eq!(err.data.as_ref().unwrap()["code"], json!("uid_conflict"));

    // 冲突发生在凭据写入之后，必须回滚回去。
    assert_eq!(
        own_access(&app, &record.id).as_deref(),
        Some("ai-access"),
        "冲突时不能把已有凭据覆盖掉"
    );
    assert_eq!(own_refresh(&app, &record.id).as_deref(), Some("ai-refresh"));
    assert_eq!(account(&app, uid).provider, "workbuddy_ai");
}

#[tokio::test]
async fn ai_account_refresh_is_unsupported_and_keeps_the_authorization() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "ai-refresh-uid";
    app.with_db(|db| {
        db.upsert_account(&AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.to_string(),
            provider: Some("workbuddy_ai".to_string()),
            ..Default::default()
        })
    })
    .expect("account");
    // 国际版登录走 `WbVariant::Ai`：provider 由档位决定，不是继承来的。
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Ai,
    )
    .expect("persist login");

    let record = account(&app, uid);
    assert_eq!(record.provider, "workbuddy_ai");
    let before = app
        .with_db(|db| db.find_authorization(&record.id, ids::KIND_OWN))
        .unwrap()
        .expect("own authorization");

    let out = refresh_account(&app, &record.id).await.expect("refresh");

    assert_eq!(out["state"], json!("unsupported"));
    assert_eq!(out["error"]["kind"], json!("unsupported_provider"));
    let after = app
        .with_db(|db| db.find_authorization(&record.id, ids::KIND_OWN))
        .unwrap()
        .expect("own authorization");
    assert_eq!(after.status, before.status, "没发请求就不许把授权标成失效");
    assert_eq!(after.version, before.version, "授权没有被写过");
    assert_eq!(
        own_access(&app, &record.id).as_deref(),
        Some("own-access"),
        "钥匙串里的令牌一个字节都没动"
    );
    assert_eq!(
        own_refresh(&app, &record.id).as_deref(),
        Some("own-refresh")
    );
}

#[tokio::test]
async fn ai_account_status_is_unsupported_and_changes_nothing() {
    let (app, _dir) = test_app(SecureStore::memory());
    let record = app
        .with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".to_string(),
                uid: "ai-status-uid".to_string(),
                provider: Some("workbuddy_ai".to_string()),
                ..Default::default()
            })
        })
        .expect("account");
    let before = record.auth_status.clone();

    let out = workbuddy_account_status(&app, &record.id)
        .await
        .expect("status");

    assert_eq!(out["state"], json!("unsupported"));
    assert_eq!(out["error"]["kind"], json!("unsupported_provider"));
    assert_eq!(out["provider"], json!("workbuddy_ai"));
    let after = app
        .with_db(|db| db.get_account(&record.id))
        .unwrap()
        .unwrap();
    assert_eq!(after.auth_status, before, "没发请求就不许改授权状态");
}

fn magpie_login_with(key: &str, uid: &str, access: &str) -> magpie::MagpieLogin {
    magpie::MagpieLogin {
        key: key.to_string(),
        provider: magpie::provider_of(key).expect("provider"),
        uid: uid.to_string(),
        nickname: None,
        domain: String::new(),
        access_token: access.to_string(),
        expires_at_ms: Some(fixture_expiry_ms()),
    }
}

fn account(app: &App, uid: &str) -> crate::storage::AccountRecord {
    app.with_db(|db| db.find_account("workbuddy", uid))
        .expect("query")
        .expect("account")
}

/// 造一份"CRED-4 之前的旧账号"：账号行 + `keychain:` 引用 + 钥匙串里的令牌。
///
/// CRED-3 只改**新**授权，这类行必须继续按老路径读写，直到用户显式迁移。
/// 没有这个 helper 就没法测"老路径仍然可靠"——直接调 persist_login 只会走 JSON。
fn seed_legacy_own(app: &App, uid: &str, access: &str, refresh: Option<&str>) -> String {
    let account_id = crate::storage::account_id("workbuddy", uid);
    let keys = service::own_keys(&account_id);
    app.secure
        .set(&keys.access, access.as_bytes())
        .expect("seed access");
    if let Some(token) = refresh {
        app.secure
            .set(
                keys.refresh.as_deref().expect("own 一定有 refresh 键"),
                token.as_bytes(),
            )
            .expect("seed refresh");
    }
    app.with_db(|db| {
        db.upsert_account(&crate::storage::AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.to_string(),
            provider: Some("workbuddy".to_string()),
            ..Default::default()
        })?;
        let now = crate::clock::now_rfc3339();
        db.upsert_authorization(&crate::storage::AuthorizationRecord {
            id: ids::auth_id(&account_id, ids::KIND_OWN),
            account_id: account_id.clone(),
            kind: ids::KIND_OWN.to_string(),
            access_ref: ids::keychain_ref(&keys.access),
            refresh_ref: keys.refresh.as_deref().map(ids::keychain_ref),
            auth_domain: None,
            token_expires_at: None,
            status: "valid".to_string(),
            version: 0,
            key_migration_state: "ready".to_string(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: now.clone(),
            updated_at: now,
        })
    })
    .expect("seed legacy row");
    account_id
}

fn own_row(app: &App, account_id: &str) -> crate::storage::AuthorizationRecord {
    app.with_db(|db| db.find_authorization(account_id, ids::KIND_OWN))
        .expect("query")
        .expect("own 授权应当存在")
}

/// 本地凭据目录里的所有文件（含坏文件），用来断言"有没有留下半套"。
fn secret_ids(app: &App) -> Vec<String> {
    crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
        .scan()
        .expect("scan")
        .secrets
        .into_iter()
        .map(|secret| secret.secret_id)
        .collect()
}

fn secret_id_of(app: &App, account_id: &str) -> String {
    match crate::credentials::reference::CredentialRef::parse(&own_row(app, account_id).access_ref)
    {
        Ok(crate::credentials::reference::CredentialRef::LocalJson { secret_id }) => secret_id,
        Ok(other) => panic!("应当是本 JSON 引用，实际 {other:?}"),
        Err(err) => panic!("引用不可解析：{err}"),
    }
}

fn read_key(app: &App, key: &str) -> Option<String> {
    app.secure
        .get(key)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
}

/// 自有授权的 access：**按引用读**（JSON 或钥匙串），与生产读路径一致。
fn own_access(app: &App, account_id: &str) -> Option<String> {
    let account = app
        .with_db(|db| db.get_account(account_id))
        .ok()
        .flatten()?;
    crate::auth::service::read_access(app, &account, ids::KIND_OWN)
        .ok()
        .flatten()
}

/// 自有授权的 refresh：同样按引用读。
fn own_refresh(app: &App, account_id: &str) -> Option<String> {
    let account = app
        .with_db(|db| db.get_account(account_id))
        .ok()
        .flatten()?;
    crate::auth::service::read_own_refresh(app, &account)
        .ok()
        .flatten()
}

fn magpie_ref(app: &App, account_id: &str) -> Option<String> {
    app.with_db(|db| db.find_authorization(account_id, ids::KIND_MAGPIE))
        .ok()
        .flatten()
        .map(|row| row.access_ref)
}

fn magpie_access(app: &App, account_id: &str) -> Option<String> {
    read_key(app, &ids::access_key(account_id, ids::KIND_MAGPIE))
}

fn legacy_access(app: &App, uid: &str) -> Option<String> {
    read_key(app, &ids::legacy_keys(uid).0)
}

// -----------------------------------------------------------------------
// G1：两套授权互不覆盖
// -----------------------------------------------------------------------

/// 新登录写 AICC 自己的 JSON：引用 `localjson:v1:`、文件 0600、钥匙串里没有新凭据。
///
/// 替代原来的 `own_login_writes_only_the_own_keys`：业务约束没变（登录只影响自有授权、
/// 不碰借用授权），变的是落点。
#[test]
fn own_login_writes_a_local_json_secret_and_no_keychain_entry() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-1111-2222-3333-444455556666";
    let out = persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("persist");
    assert_eq!(out["state"], json!("success"));

    let acc = account(&app, uid);
    let row = own_row(&app, &acc.id);
    assert_eq!(row.storage_kind(), "localjson");
    assert!(
        row.access_ref.starts_with("localjson:v1:"),
        "新授权必须落在本地 JSON：{}",
        row.access_ref
    );
    assert_eq!(
        row.refresh_ref.as_deref(),
        Some(row.access_ref.as_str()),
        "access 与 refresh 是同一个文件的两个槽位"
    );

    // 读得到，而且读的是文件。
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("own-access"));
    assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("own-refresh"));

    // 钥匙串里一个字节都不该有：新键、旧别名、借用键全都不存在。
    assert!(
        read_key(&app, &ids::access_key(&acc.id, ids::KIND_OWN)).is_none(),
        "新授权不许写钥匙串"
    );
    assert!(
        read_key(&app, &ids::refresh_key(&acc.id, ids::KIND_OWN)).is_none(),
        "新授权不许写钥匙串"
    );
    assert!(legacy_access(&app, uid).is_none(), "JSON 行不该同步旧别名");
    assert!(
        magpie_access(&app, &acc.id).is_none(),
        "登录不能顺手造一份借用授权"
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
        let path = store
            .root()
            .join(format!("{}.json", secret_id_of(&app, &acc.id)));
        let mode = std::fs::metadata(&path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "凭据文件必须 0600");
    }

    assert_eq!(acc.auth_source, "own");
}

/// PR-CRED-2 起导入**只登记引用**：钥匙串里既没有新键也没有旧别名。
#[test]
fn magpie_import_registers_a_reference_and_writes_no_keychain_key() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-3333-4444-555566667777";
    let out = import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    assert_eq!(out["imported"], json!(1), "{out}");

    let acc = account(&app, uid);
    let row = app
        .with_db(|db| db.find_authorization(&acc.id, ids::KIND_MAGPIE))
        .expect("query")
        .expect("row");
    assert_eq!(row.storage_kind(), "magpie");
    assert_eq!(row.access_ref, format!("magpie:v1:workbuddy#{uid}"));
    assert_eq!(
        row.source_key.as_deref(),
        Some(format!("workbuddy#{uid}").as_str())
    );

    assert!(
        magpie_access(&app, &acc.id).is_none(),
        "不得把 Magpie 令牌复制进钥匙串"
    );
    assert!(
        legacy_access(&app, uid).is_none(),
        "借用授权不写旧别名：那又是一次钥匙串写入"
    );
    assert!(
        own_access(&app, &acc.id).is_none(),
        "导入不能凭空造一份自有授权"
    );
    assert!(
        app.secure
            .get(&ids::refresh_key(&acc.id, ids::KIND_MAGPIE))
            .is_err(),
        "magpie/refresh 这个键不允许存在"
    );
}

/// 借用授权的令牌**每次现读**：Magpie 刷新之后，AICC 下一次请求就是新值。
#[test]
fn borrowed_access_is_read_fresh_on_every_use() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-00000000000b";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed-1")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed-1")], &[]).expect("import");
    let acc = account(&app, uid);

    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => assert_eq!(resolved.access, "borrowed-1"),
        broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
    }

    // 模拟 Magpie 自己刷新了令牌：AICC 不做任何写入就该读到新值。
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed-2")]));
    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => assert_eq!(
            resolved.access, "borrowed-2",
            "动态关联必须现读，不能缓存旧令牌"
        ),
        broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
    }
    assert!(
        magpie_access(&app, &acc.id).is_none(),
        "读到新令牌也不许把它复制进钥匙串"
    );
    magpie::test_set_logins(None);
}

/// P0-1 回归：库里的过期时间是**旧快照**，不能拿它挡住 Magpie 刷新后的新令牌。
#[test]
fn a_stale_db_expiry_does_not_block_a_refreshed_magpie_token() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-00000000000f";

    let stale = magpie_login(uid, "borrowed-old");
    magpie::test_set_logins(Some(vec![stale.clone()]));
    import_logins(&app, vec![stale], &[]).expect("import");
    let acc = account(&app, uid);

    // 时间跨过导入时那个有效期：库里的状态与到期时间都写成"已过期"。
    app.with_db(|db| {
        db.exec_raw(
            "UPDATE account_authorizations
                    SET status = 'expired', token_expires_at = '2000-01-01T00:00:00Z'
                  WHERE kind = 'magpie'",
        )
    })
    .expect("mark stale");

    // Magpie 自己刷新了：文件里是新的、有效期到未来。AICC 不做任何导入。
    let fresh = magpie_login(uid, "borrowed-new");
    magpie::test_set_logins(Some(vec![fresh]));

    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => assert_eq!(
            resolved.access, "borrowed-new",
            "旧快照的过期时间不许挡住动态读取"
        ),
        broker::Outcome::Unavailable { reason, .. } => {
            panic!("不该不可用：{reason}（旧状态把新令牌锁死了）")
        }
    }

    // 界面要能看出"库里过期、实际可用"。
    let described = broker::describe(&app, &acc.id).expect("describe");
    assert_eq!(described[0]["status"], json!("expired"));
    assert_eq!(described[0]["effectiveStatus"], json!("valid"));
    assert_eq!(described[0]["recoverable"], json!(true));
    magpie::test_set_logins(None);
}

/// P1 回归：Magpie 文件读不了 ≠ 没有凭据，reason 必须单独分类。
#[test]
fn an_unreadable_magpie_file_reports_unavailable_not_missing_credential() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-000000000010";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    // 只留借用授权，再让文件"读不了"。
    magpie::test_set_unreadable();
    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => panic!("不该有令牌：{}", resolved.access),
        broker::Outcome::Unavailable { reason, detail } => {
            assert_eq!(reason, "magpie_unavailable", "{detail}");
        }
    }
    // 状态不能被写成 expired（文件问题不是授权问题）。
    let row = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "valid", "读不到文件不许改授权状态");
}

/// P0-2 回归（a）：同 UID 两个 provider，必须按来源键 / provider 选，不能只看 UID。
#[test]
fn resync_picks_the_login_matching_provider_and_source_key() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "shared-uid-1";
    let ai = magpie_login_with("workbuddy-ai#shared-uid-1", uid, "ai-token");
    magpie::test_set_logins(Some(vec![ai.clone()]));
    import_logins(&app, vec![ai], &[]).expect("import");
    let acc = account(&app, uid);
    assert_eq!(acc.provider, "workbuddy_ai");

    // 文件里同时有国内版与同 UID 的国际版：国内版排前面。
    let cn = magpie_login_with("workbuddy#shared-uid-1", uid, "cn-token");
    magpie::test_set_logins(Some(vec![
        cn,
        magpie_login_with("workbuddy-ai#shared-uid-1", uid, "ai-token-2"),
    ]));

    let out = resync_magpie_access(&app, &acc).expect("resync");
    assert_eq!(out["state"], json!("valid"), "{out}");
    let row = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .unwrap();
    assert_eq!(
        row.access_ref, "magpie:v1:workbuddy-ai#shared-uid-1",
        "不能被同 UID 的国内版覆盖"
    );
    assert_eq!(row.source_key.as_deref(), Some("workbuddy-ai#shared-uid-1"));
    magpie::test_set_logins(None);
}

/// P0-2 回归（b）：文件暂时不可读 → 不改状态；恢复之后立刻可用。
#[test]
fn resync_keeps_the_row_intact_while_magpie_is_unreadable() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-000000000011";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    magpie::test_set_unreadable();
    let out = resync_magpie_access(&app, &acc).expect("resync");
    assert_eq!(out["state"], json!("unavailable"), "{out}");
    assert_eq!(out["error"]["kind"], json!("magpie_unavailable"));
    let row = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "valid", "文件读不了不许把状态写成 expired");
    assert_eq!(row.access_ref, format!("magpie:v1:workbuddy#{uid}"));

    // 文件回来：不需要任何重新导入就能恢复。
    magpie::test_reset();
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed-again")]));
    let out = resync_magpie_access(&app, &acc).expect("resync again");
    assert_eq!(out["state"], json!("valid"), "{out}");
    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => assert_eq!(resolved.access, "borrowed-again"),
        broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
    }
    magpie::test_set_logins(None);
}

/// Magpie 日志里换成别的 UID：这是串号，必须报出来，不能当"没凭据"糊过去。
#[test]
fn a_link_pointing_at_another_account_is_reported_as_mismatch() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-00000000000c";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    // 同一个来源键，但文件里那条登录态属于别人。
    magpie::test_set_logins(Some(vec![magpie_login_with(
        &format!("workbuddy#{uid}"),
        "someone-else",
        "stolen",
    )]));
    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => {
            panic!("不能拿别人的令牌：{}", resolved.access)
        }
        broker::Outcome::Unavailable { reason, .. } => {
            assert_eq!(reason, "link_mismatch", "串号要有独立分类");
        }
    }
    magpie::test_set_logins(None);
}

/// Magpie 没装 / 文件丢了：按既有规则兜底到自有授权，并把原因带上。
#[test]
fn unavailable_magpie_falls_back_to_the_own_authorization() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-00000000000d";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    persist_login(&app, &payload(uid, "own-access", None), WbVariant::Cn).expect("login");
    let acc = account(&app, uid);

    // 文件突然读不了了。
    magpie::test_set_logins(None);
    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => {
            assert_eq!(resolved.kind, "own", "首选不可用要兜底");
            assert!(resolved.degraded.is_some(), "兜底必须说明原因");
        }
        broker::Outcome::Unavailable { reason, .. } => panic!("不该整体不可用：{reason}"),
    }
    // 借用授权那一行不能被误删：文件暂时不可读不等于账号没授权。
    assert!(app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .is_some());
}

/// v20 重新关联：旧钥匙串引用 → `magpie:v1:`，旧键保留可回滚。
#[test]
fn relink_moves_a_legacy_keychain_ref_to_the_dynamic_link() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-00000000000e";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    // 退回 v19 的形状：引用指向钥匙串，里面躺着当年复制进去的令牌。
    let key = ids::access_key(&acc.id, ids::KIND_MAGPIE);
    app.secure.set(&key, b"legacy-token").unwrap();
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE account_authorizations
                    SET access_ref = 'keychain:{key}', ref_updated_at = NULL
                  WHERE account_id = '{}' AND kind = 'magpie'",
            acc.id
        ))
    })
    .expect("rewind to v19");
    let before = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .unwrap();
    assert_eq!(before.storage_kind(), "keychain");

    let out = crate::auth::relink::relink_pending(&app, None).expect("relink");
    assert_eq!(out["relinked"], json!(1), "{out}");
    assert_eq!(out["pending"], json!(0), "{out}");
    assert_eq!(out["legacyKeysKept"], json!(true), "不许自动删旧键");

    let after = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .unwrap();
    assert_eq!(after.storage_kind(), "magpie");
    assert_eq!(after.access_ref, format!("magpie:v1:workbuddy#{uid}"));
    assert!(after.version > before.version, "换引用要推进版本");
    assert_eq!(
        read_key(&app, &key).as_deref(),
        Some("legacy-token"),
        "旧钥匙串条目原样保留"
    );

    // 幂等：再跑一次没有可改的行。
    let again = crate::auth::relink::relink_pending(&app, None).expect("relink again");
    assert_eq!(again["relinked"], json!(0), "{again}");
    assert_eq!(again["pending"], json!(0), "{again}");
    magpie::test_set_logins(None);
}

/// CRED-5：迁移面板上的"重新关联"就是 `credentials.migration.relink`。
///
/// 它必须做到三件事：把引用换成动态关联、**不产生任何明文文件**、
/// 旧钥匙串条目原样保留（回滚依据）。
#[test]
fn migration_relink_repoints_a_borrowed_row_without_copying_any_token() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-00000000001f";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    // 退回 v19 的形状：引用指着钥匙串，里面躺着当年复制进去的令牌。
    let key = ids::access_key(&acc.id, ids::KIND_MAGPIE);
    app.secure.set(&key, b"legacy-token").unwrap();
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE account_authorizations
                    SET access_ref = 'keychain:{key}', source_key = NULL, ref_updated_at = NULL
                  WHERE account_id = '{}' AND kind = 'magpie'",
            acc.id
        ))
    })
    .expect("rewind to v19");

    // 盘点：单独一档，且给界面明确的下一步。
    let before = crate::providers::cred_migrate::inventory(&app).expect("inventory");
    assert_eq!(before["summary"]["needsRelink"], json!(1), "{before}");
    assert_eq!(before["summary"]["magpie"], json!(0), "{before}");
    assert_eq!(before["items"][0]["action"], json!("relink"), "{before}");

    let out = crate::providers::cred_migrate::relink(&app, &json!({})).expect("relink");
    assert_eq!(out["relinked"], json!(1), "{out}");
    assert_eq!(out["legacyKeysKept"], json!(true), "{out}");
    assert_eq!(out["summary"]["needsRelink"], json!(0), "{out}");
    assert_eq!(out["summary"]["magpie"], json!(1), "{out}");

    let after = app
        .with_db(|db| db.find_authorization(&acc.id, ids::KIND_MAGPIE))
        .unwrap()
        .unwrap();
    assert_eq!(after.access_ref, format!("magpie:v1:workbuddy#{uid}"));
    assert_eq!(
        read_key(&app, &key).as_deref(),
        Some("legacy-token"),
        "旧钥匙串条目必须保留"
    );

    // 一个明文文件都不许产生：重新关联只改引用。
    let secrets = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
        .scan()
        .map(|report| report.secrets.len())
        .unwrap_or(0);
    assert_eq!(secrets, 0, "重新关联不得落任何 secrets 文件");
    magpie::test_set_logins(None);
}

/// CRED-5 复核 S4：批量重新关联逐条提交。
///
/// 第 2 条写库失败时，前一条的成功必须如实报出来，后面那条照常处理 ——
/// 以前的写法用 `?` 直接中止，界面只拿到一个错误，已经改成功的行没人知道。
#[test]
fn a_batch_relink_reports_each_row_when_one_write_fails() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uids = [
        "6f1c2d94-2222-0001-0002-000000000031",
        "6f1c2d94-2222-0001-0002-000000000032",
        "6f1c2d94-2222-0001-0002-000000000033",
    ];
    let logins: Vec<_> = uids
        .iter()
        .map(|uid| magpie_login(uid, "borrowed"))
        .collect();
    magpie::test_set_logins(Some(logins.clone()));
    import_logins(&app, logins, &[]).expect("import");

    // 全部退回 v19 形状：引用指着钥匙串，没有来源键。
    app.with_db(|db| {
        db.exec_raw(
            "UPDATE account_authorizations
                    SET access_ref = 'keychain:' || account_id || '/magpie/access',
                        source_key = NULL,
                        ref_updated_at = NULL
                  WHERE kind = 'magpie'",
        )
    })
    .expect("rewind");

    // 第 2 条写入时模拟数据库异常。
    let mut seen = 0usize;
    let out = crate::auth::relink::relink_pending_with(&app, None, |_row| {
        seen += 1;
        if seen == 2 {
            return Err(crate::rpc::RpcError::internal("模拟数据库写失败"));
        }
        Ok(())
    })
    .expect("批量不该整体失败");

    assert_eq!(out["relinked"], json!(2), "{out}");
    assert_eq!(out["failed"], json!(1), "{out}");
    let items = out["items"].as_array().expect("items");
    assert_eq!(items.len(), 3, "三条都要有逐条结果：{out}");

    let failed_item = items
        .iter()
        .find(|item| item["ok"] == json!(false))
        .expect("必须有一条失败");
    assert_eq!(failed_item["reason"], json!("write_failed"));
    assert!(failed_item["message"].as_str().is_some(), "要带原因");
    assert!(
        !out.to_string().contains("borrowed"),
        "结果里不许出现令牌：{out}"
    );

    // 失败那一行引用没动，成功那两行已经换成动态关联。
    let failed_id = failed_item["authorizationId"].as_str().expect("id");
    let row = app
        .with_db(|db| db.get_authorization(failed_id))
        .unwrap()
        .expect("row");
    assert!(row.access_ref.starts_with("keychain:"), "{row:?}");
    let changed = app
        .with_db(|db| db.list_all_authorizations())
        .unwrap()
        .into_iter()
        .filter(|row| row.kind == ids::KIND_MAGPIE && row.access_ref.starts_with("magpie:v1:"))
        .count();
    assert_eq!(changed, 2, "成功的两行必须已经落库");
    magpie::test_set_logins(None);
}

/// 复审第二轮 #1：relink 成功后，旧键搬运的 `failed` 必须一并清掉。
///
/// 否则面板一直显示「旧键搬运失败」，用户点重试又会拿不到钥匙串目标键
/// （`move_one` 报 `bad_ref`），陷入"报了修不了"的状态。
#[test]
fn relinking_a_failed_row_clears_the_obsolete_key_migration_failure() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-000000000041";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    let key = ids::access_key(&acc.id, ids::KIND_MAGPIE);
    app.secure.set(&key, b"legacy-token").unwrap();
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE account_authorizations
                    SET access_ref = 'keychain:{key}', source_key = NULL, ref_updated_at = NULL
                  WHERE account_id = '{}' AND kind = 'magpie'",
            acc.id
        ))
    })
    .expect("rewind");
    let auth_id = ids::auth_id(&acc.id, ids::KIND_MAGPIE);
    app.with_db(|db| {
        db.set_key_migration_state(
            &auth_id,
            crate::storage::authz::KEY_STATE_FAILED,
            Some("denied"),
        )
    })
    .unwrap();
    assert_eq!(
        crate::auth::migrate::failed_count(&app).unwrap(),
        1,
        "先确认它能被报出来"
    );

    let out = crate::providers::cred_migrate::relink(&app, &json!({})).expect("relink");
    assert_eq!(out["relinked"], json!(1), "{out}");

    let state = app
        .with_db(|db| db.get_authorization(&auth_id))
        .unwrap()
        .expect("row")
        .key_migration_state;
    assert_eq!(state, "ready", "引用换了，搬运状态必须跟着收敛");
    assert_eq!(
        crate::auth::migrate::failed_count(&app).unwrap(),
        0,
        "面板不该再报这条失败"
    );

    let value = crate::providers::cred_migrate::inventory(&app).expect("inventory");
    assert_eq!(value["keyMigration"]["failed"], json!(0), "{value}");

    // 再点一次重试也是安全的：没有待搬运的行，也不会去读钥匙串。
    let (reset, report) = crate::auth::migrate::retry_failed(&app).expect("retry");
    assert_eq!(reset, 0, "没有 failed 行可重置");
    assert_eq!(report.moved + report.failed, 0, "{report:?}");
    magpie::test_set_logins(None);
}

/// 复审第二轮 #2：批量 relink 不许碰坏引用。
///
/// 候选以前是「不是 `magpie:v1:` 就算」，`unknown:v9:` 也会被选进来；
/// 一旦 Magpie 里有匹配的 UID，这条坏引用就会被顺手改写，而盘点明明报的是
/// `reference_unusable`。
#[test]
fn relink_skips_an_unparsable_reference_and_leaves_it_alone() {
    let (app, _dir) = test_app(SecureStore::memory());
    let good_uid = "6f1c2d94-2222-0001-0002-000000000042";
    let bad_uid = "6f1c2d94-2222-0001-0002-000000000043";
    let logins = vec![
        magpie_login(good_uid, "borrowed"),
        magpie_login(bad_uid, "borrowed"),
    ];
    magpie::test_set_logins(Some(logins.clone()));
    import_logins(&app, logins, &[]).expect("import");

    let good = account(&app, good_uid);
    let bad = account(&app, bad_uid);
    let good_key = ids::access_key(&good.id, ids::KIND_MAGPIE);
    app.secure.set(&good_key, b"legacy-token").unwrap();
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE account_authorizations
                    SET access_ref = 'keychain:{good_key}', source_key = NULL, ref_updated_at = NULL
                  WHERE account_id = '{}' AND kind = 'magpie'",
            good.id
        ))
    })
    .expect("rewind good");
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE account_authorizations
                    SET access_ref = 'unknown:v9:broken', source_key = NULL, ref_updated_at = NULL
                  WHERE account_id = '{}' AND kind = 'magpie'",
            bad.id
        ))
    })
    .expect("break bad");

    let out = crate::providers::cred_migrate::relink(&app, &json!({})).expect("relink");
    assert_eq!(out["relinked"], json!(1), "只该改那一条正常的：{out}");
    let bad_row = app
        .with_db(|db| db.find_authorization(&bad.id, ids::KIND_MAGPIE))
        .unwrap()
        .expect("row");
    assert_eq!(
        bad_row.access_ref, "unknown:v9:broken",
        "坏引用一个字节都不许动"
    );

    let value = crate::providers::cred_migrate::inventory(&app).expect("inventory");
    assert_eq!(value["summary"]["blocked"], json!(1), "{value}");
    assert_eq!(value["summary"]["needsRelink"], json!(0), "{value}");
    magpie::test_set_logins(None);
}

/// CRED-5：启用本地 JSON 之后，没有授权行的账号**不再**去读旧别名钥匙串。
///
/// 回退会在后台任务里弹一次授权框并阻塞 RPC 队列 —— 那正是本轮要消灭的东西。
#[test]
fn read_access_stops_falling_back_to_the_legacy_alias_once_json_storage_is_on() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-000000000020";
    let account_id = crate::storage::account_id("workbuddy", uid);
    let (legacy_access, _) = ids::legacy_keys(&uid);
    app.secure.set(&legacy_access, b"ALIAS-TOKEN").unwrap();

    let acc = crate::storage::AccountRecord {
        id: account_id,
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
        auth_source: "own".into(),
        travel_enabled: false,
        auth_epoch: 0,
        auth_mirror_auth_id: None,
    };

    let read = service::read_access(&app, &acc, ids::KIND_OWN).expect("read");
    assert_eq!(read, None, "不许回退到旧别名键");
    assert_eq!(
        read_key(&app, &legacy_access).as_deref(),
        Some("ALIAS-TOKEN"),
        "只是不读，不许删"
    );
}

/// 兼容口径：**还没**进入 v3 的库（没同意明文、也没有任何新引用）仍走老回退，
/// 否则极老安装会突然读不到令牌。
#[test]
fn read_access_keeps_the_legacy_alias_fallback_for_a_pure_v2_database() {
    let (app, _dir) = test_app_without_consent(SecureStore::memory());
    let uid = "6f1c2d94-2222-0001-0002-000000000021";
    let account_id = crate::storage::account_id("workbuddy", uid);
    let (legacy_access, _) = ids::legacy_keys(&uid);
    app.secure.set(&legacy_access, b"ALIAS-TOKEN").unwrap();

    let acc = crate::storage::AccountRecord {
        id: account_id,
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
        auth_source: "own".into(),
        travel_enabled: false,
        auth_epoch: 0,
        auth_mirror_auth_id: None,
    };

    let read = service::read_access(&app, &acc, ids::KIND_OWN).expect("read");
    assert_eq!(read.as_deref(), Some("ALIAS-TOKEN"));
}

#[test]
fn qoder_accounts_go_to_their_own_tables_without_a_secret() {
    let (app, _dir) = test_app(SecureStore::memory());
    let out = import_logins(
        &app,
        vec![magpie_login_with("qoder-cn", "qn-uid-1", "qd-access")],
        &[],
    )
    .expect("import");
    assert_eq!(out["imported"], json!(1), "{out}");

    // accounts 表里不能出现 Qoder 平台服务账号。
    assert!(
        app.with_db(|db| db.list_accounts(&AccountFilter::default()))
            .unwrap()
            .is_empty(),
        "Qoder 账号不能写进 accounts"
    );

    let accounts = app.with_db(|db| db.list_qoder_accounts(None)).unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].edition, "qoder_cn");

    let auths = app
        .with_db(|db| db.list_qoder_authorizations(&accounts[0].id))
        .unwrap();
    assert_eq!(auths.len(), 1);
    assert_eq!(auths[0].source, "magpie");
    assert_eq!(auths[0].source_key.as_deref(), Some("qoder-cn"));
    assert!(auths[0].secret_ref.is_none(), "magpie 来源不许持有凭据");
    assert_eq!(
        out["accounts"][0]["authorization"]["hasSecret"],
        json!(false)
    );
}

#[test]
fn qoder_global_and_cn_are_separate_accounts() {
    let (app, _dir) = test_app(SecureStore::memory());
    import_logins(
        &app,
        vec![
            magpie_login_with("qoder", "shared-uid", "a"),
            magpie_login_with("qoder-cn", "shared-uid", "b"),
        ],
        &[],
    )
    .unwrap();
    let accounts = app.with_db(|db| db.list_qoder_accounts(None)).unwrap();
    assert_eq!(accounts.len(), 2, "同一 UID 在两个版本是两个账号");
}

#[test]
fn workbuddy_ai_import_marks_the_provider() {
    let (app, _dir) = test_app(SecureStore::memory());
    let out = import_logins(
        &app,
        vec![magpie_login_with("workbuddy-ai", "ai-uid-1", "ai-access")],
        &[],
    )
    .expect("import");
    assert_eq!(out["imported"], json!(1), "{out}");
    let record = account(&app, "ai-uid-1");
    assert_eq!(record.platform, "workbuddy");
    assert_eq!(record.provider, "workbuddy_ai");
}

#[test]
fn same_uid_in_the_other_provider_is_a_conflict_not_a_merge() {
    let (app, _dir) = test_app(SecureStore::memory());
    import_logins(
        &app,
        vec![magpie_login_with("workbuddy", "same-uid", "cn")],
        &[],
    )
    .unwrap();
    let out = import_logins(
        &app,
        vec![magpie_login_with("workbuddy-ai", "same-uid", "ai")],
        &[],
    )
    .unwrap();
    assert_eq!(out["imported"], json!(0), "{out}");
    assert_eq!(out["skipped"][0]["reason"], json!("uid_conflict"));
    let accounts = app
        .with_db(|db| db.list_accounts(&AccountFilter::default()))
        .unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].provider, "workbuddy", "已有账号不被改写");
}

/// 核心场景：先导入、再扫码、再导入 —— 三份操作互不覆盖。
#[test]
fn two_authorizations_survive_login_and_reimport() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-3333-4444-5555-666677778888";

    import_logins(&app, vec![magpie_login(uid, "borrowed-1")], &[]).expect("import");
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    import_logins(&app, vec![magpie_login(uid, "borrowed-2")], &[]).expect("re-import");

    let acc = account(&app, uid);
    assert_eq!(
        own_access(&app, &acc.id).as_deref(),
        Some("own-access"),
        "重新导入不能覆盖自有授权"
    );
    assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("own-refresh"));
    assert!(
        magpie_access(&app, &acc.id).is_none(),
        "重新导入同样不写钥匙串"
    );
    let magpie_row = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .unwrap();
    assert_eq!(
        magpie_row.source_key.as_deref(),
        Some(format!("workbuddy#{uid}").as_str())
    );
    assert_eq!(magpie_row.storage_kind(), "magpie");

    let rows = app.with_db(|db| db.list_authorizations(&acc.id)).unwrap();
    assert_eq!(rows.len(), 2, "两份授权并存，各一行");
    let kinds: Vec<&str> = rows.iter().map(|row| row.kind.as_str()).collect();
    assert_eq!(kinds, vec!["magpie", "own"], "按 kind 排序，各一行");
}

#[test]
fn mirror_follows_the_preferred_source() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-4444-5555-6666-777788889999";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);

    // 默认 magpie 优先：只登记引用，别名里什么都不写。
    assert_eq!(acc.auth_source, "magpie");
    assert!(legacy_access(&app, uid).is_none(), "借用授权不写旧别名");
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => {
            assert_eq!(resolved.kind, "magpie");
            assert_eq!(resolved.access, "borrowed", "动态关联要现读文件里的令牌");
        }
        broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
    }

    broker::set_preferred_source(&app, "own").expect("set");
    service::sync_mirror(&app, &acc.id).expect("mirror");
    let acc = account(&app, uid);
    assert_eq!(acc.auth_source, "own");
    // 镜像是自有授权，但它现在住在 JSON 里 —— 旧别名仍然一个字节都不写。
    assert!(
        legacy_access(&app, uid).is_none(),
        "JSON 授权不走旧别名：那是又一次隐性钥匙串写入"
    );
    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => {
            assert_eq!(resolved.kind, "own");
            assert_eq!(resolved.access, "own-access");
        }
        broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
    }
}

// -----------------------------------------------------------------------
// 借用授权的到期时间以 Magpie 文件为准（2.14.1）
// -----------------------------------------------------------------------

/// 导入时落库的是**快照**；Magpie 换过令牌之后，镜像必须现读文件里的到期时间，
/// 否则界面会把一份可用授权一直显示成「已过期」。
#[test]
fn mirror_takes_magpie_expiry_from_the_file() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-aaaa-5555-6666-777788889999";
    let login = magpie_login(uid, "borrowed");
    let snapshot = fixture_expiry(&login);
    import_logins(&app, vec![login], &[]).expect("import");
    let acc = account(&app, uid);
    assert_eq!(
        acc.token_expires_at.as_deref(),
        Some(snapshot.as_str()),
        "导入时先落 Magpie 快照"
    );

    // Magpie 换发新令牌：文件里的到期时间往后推。
    let mut fresh = magpie_login(uid, "borrowed");
    fresh.expires_at_ms = Some(refreshed_expiry_ms());
    let want = fixture_expiry(&fresh);
    magpie::test_set_logins(Some(vec![fresh]));
    service::sync_mirror(&app, &acc.id).expect("mirror");

    let acc = account(&app, uid);
    assert_eq!(acc.token_expires_at.as_deref(), Some(want.as_str()));
    assert_eq!(acc.auth_status, "valid", "换个到期时间不是状态变化");
    assert_eq!(acc.auth_source, "magpie", "首选来源不变");
    magpie::test_reset();
}

/// 文件读不到 / 没装 Magpie：保留原有镜像值，不把「不知道」写成「已过期」。
#[test]
fn mirror_keeps_the_snapshot_when_magpie_cannot_be_read() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-bbbb-5555-6666-777788889999";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);
    let before = acc.token_expires_at.clone();
    assert!(before.is_some());

    magpie::test_set_unreadable();
    service::sync_mirror(&app, &acc.id).expect("mirror");

    let acc = account(&app, uid);
    assert_eq!(acc.token_expires_at, before, "读不到就保留旧值");
    assert_eq!(acc.auth_status, "valid");
    magpie::test_reset();
}

/// 同时有 own 与 magpie：刷新镜像只动镜像的到期时间，own 那一行一个字节都不变。
#[test]
fn mirror_refresh_leaves_the_own_authorization_alone() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-cccc-5555-6666-777788889999";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    let own_before = app
        .with_db(|db| db.find_authorization(&acc.id, "own"))
        .expect("query")
        .expect("own row");

    let mut fresh = magpie_login(uid, "borrowed");
    fresh.expires_at_ms = Some(refreshed_expiry_ms());
    let want = fixture_expiry(&fresh);
    magpie::test_set_logins(Some(vec![fresh]));
    service::sync_mirror(&app, &acc.id).expect("mirror");

    let own_after = app
        .with_db(|db| db.find_authorization(&acc.id, "own"))
        .expect("query")
        .expect("own row");
    assert_eq!(own_before.token_expires_at, own_after.token_expires_at);
    assert_eq!(own_before.version, own_after.version, "own 行不该被写");
    assert_eq!(own_before.access_ref, own_after.access_ref);
    assert_eq!(
        account(&app, uid).token_expires_at.as_deref(),
        Some(want.as_str())
    );
    magpie::test_reset();
}

/// 镜像里的到期时间已经过去、文件里换成新令牌：刷新只更新到期时间，
/// 不标 expired、不写 last_error、不要求重新登录。
#[test]
fn refreshing_an_expired_mirror_does_not_ask_for_a_relogin() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-dddd-5555-6666-777788889999";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    // Magpie 文件里换成一个**已经过期**的令牌：镜像照实显示过期。
    let mut expired = magpie_login(uid, "borrowed");
    expired.expires_at_ms = Some(1_600_000_000_000); // 2020-09-13
    magpie::test_set_logins(Some(vec![expired]));
    service::sync_mirror(&app, &acc.id).expect("mirror");
    assert_eq!(
        account(&app, uid).token_expires_at.as_deref(),
        Some("2020-09-13T12:26:40Z"),
        "文件里过期了，镜像就该说过期"
    );

    // 用户重新授权，Magpie 换发新令牌：刷新镜像即可恢复，不需要再登录一次。
    let mut fresh = magpie_login(uid, "borrowed");
    fresh.expires_at_ms = Some(refreshed_expiry_ms());
    let want = fixture_expiry(&fresh);
    magpie::test_set_logins(Some(vec![fresh]));
    service::sync_mirror(&app, &acc.id).expect("mirror");

    let acc = account(&app, uid);
    assert_eq!(acc.token_expires_at.as_deref(), Some(want.as_str()));
    assert_eq!(acc.auth_status, "valid");
    let rows = app
        .with_db(|db| db.list_authorizations(&acc.id))
        .expect("rows");
    for row in rows {
        assert_eq!(
            row.status, "valid",
            "刷新到期时间不是状态迁移：{}",
            row.kind
        );
        assert!(row.last_error.is_none(), "{}", row.kind);
    }
    magpie::test_reset();
}

// -----------------------------------------------------------------------
// 账号页的**实际读取路径**：`accounts.list` / `accounts.get`
//
// 界面上的「已过期」判的是 `tokenExpiresAt` 这一列（ConsoleAccount.isTokenExpired）。
// 只修 `sync_mirror` 不够 —— 那要等下一次导入或刷新才生效，用户重开页面看到的
// 还是旧快照。读取路径必须自己就能给出新值，而且**不许写库**。
// -----------------------------------------------------------------------

/// 库里留着旧快照、Magpie 文件已换新令牌：不导入、不重新登录、不手动刷新镜像，
/// 账号页的两条 RPC 都要返回新到期时间。
#[tokio::test]
async fn accounts_rpcs_report_the_live_magpie_expiry_without_import_or_relogin() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-eeee-5555-6666-777788889999";

    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    assert_eq!(acc.auth_source, "magpie", "默认首选借用授权");

    // 真实库里的样子：镜像和授权行都还是**导入时的旧快照**。
    const STALE: &str = "2026-11-22T13:42:14Z";
    let sql = format!(
        "UPDATE accounts SET token_expires_at = '{STALE}' WHERE id = '{id}';
             UPDATE account_authorizations SET token_expires_at = '{STALE}'
              WHERE account_id = '{id}' AND kind = 'magpie';",
        id = acc.id
    );
    app.with_db(|db| db.exec_raw(&sql)).expect("mark stale");

    // Magpie 自己换发新令牌：**只改文件**。
    let mut fresh = magpie_login(uid, "borrowed");
    fresh.expires_at_ms = Some(refreshed_expiry_ms());
    let want = fixture_expiry(&fresh);
    magpie::test_set_logins(Some(vec![fresh]));

    let list = dispatch(&app, "accounts.list", &json!({}))
        .await
        .expect("list");
    let row = list["accounts"]
        .as_array()
        .expect("accounts")
        .iter()
        .find(|row| row["id"] == json!(acc.id))
        .expect("account in list");
    assert_eq!(
        row["tokenExpiresAt"].as_str(),
        Some(want.as_str()),
        "列表卡片判过期看的就是这一列"
    );

    let one = dispatch(&app, "accounts.get", &json!({ "id": acc.id }))
        .await
        .expect("get");
    assert_eq!(one["tokenExpiresAt"].as_str(), Some(want.as_str()));

    let magpie_row = one["authorizations"]
        .as_array()
        .expect("authorizations")
        .iter()
        .find(|row| row["kind"] == json!("magpie"))
        .expect("magpie row");
    assert_eq!(
        magpie_row["tokenExpiresAt"].as_str(),
        Some(want.as_str()),
        "同一份授权不能同时显示两个日期"
    );

    // 只读：展示路径没有把新值写回库里那份快照。
    assert_eq!(
        account(&app, uid).token_expires_at.as_deref(),
        Some(STALE),
        "为显示日期而写库是另一回事，这里明确不做"
    );
    magpie::test_reset();
}

/// 边界 1：Magpie 文件读不到 → 退回库里那份，不把「不知道」写成过期或空。
#[tokio::test]
async fn accounts_rpcs_fall_back_when_the_magpie_file_is_unreadable() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-ffff-5555-6666-777788889999";
    let login = magpie_login(uid, "borrowed");
    let snapshot = fixture_expiry(&login);
    import_logins(&app, vec![login], &[]).expect("import");
    let acc = account(&app, uid);

    magpie::test_set_unreadable();
    let one = dispatch(&app, "accounts.get", &json!({ "id": acc.id }))
        .await
        .expect("get");
    assert_eq!(
        one["tokenExpiresAt"].as_str(),
        Some(snapshot.as_str()),
        "读不到文件就保留旧值"
    );
    magpie::test_reset();
}

/// 边界 2：来源键 / 平台 / UID 任何一项对不上，都**不采用**这条到期时间。
#[tokio::test]
async fn accounts_rpcs_ignore_a_link_that_belongs_to_another_account() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-1111-5555-6666-777788889999";
    let login = magpie_login(uid, "borrowed");
    let snapshot = fixture_expiry(&login);
    import_logins(&app, vec![login], &[]).expect("import");
    let acc = account(&app, uid);

    // 同一个来源键，UID 却是别人的：这是另一份凭据，不能拿来改这个账号的显示。
    let mut foreign = magpie_login_with("workbuddy", "someone-else", "borrowed");
    foreign.expires_at_ms = Some(refreshed_expiry_ms());
    magpie::test_set_logins(Some(vec![foreign]));

    let one = dispatch(&app, "accounts.get", &json!({ "id": acc.id }))
        .await
        .expect("get");
    assert_eq!(
        one["tokenExpiresAt"].as_str(),
        Some(snapshot.as_str()),
        "对不上的关联必须忽略，保留库里那份"
    );
    magpie::test_reset();
}

/// 边界 3：当前来源是自有授权时，Magpie 文件里换了什么都与这个账号的显示无关。
#[tokio::test]
async fn accounts_rpcs_never_let_magpie_override_an_own_source() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-2222-7777-6666-777788889999";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    broker::set_preferred_source(&app, "own").expect("prefer own");
    let acc = account(&app, uid);
    service::sync_mirror(&app, &acc.id).expect("mirror");
    let acc = account(&app, uid);
    assert_eq!(acc.auth_source, "own");

    let own_expiry = app
        .with_db(|db| db.find_authorization(&acc.id, "own"))
        .expect("query")
        .expect("own row")
        .token_expires_at
        .expect("own expiry");

    let mut fresh = magpie_login(uid, "borrowed");
    fresh.expires_at_ms = Some(refreshed_expiry_ms());
    magpie::test_set_logins(Some(vec![fresh]));

    let one = dispatch(&app, "accounts.get", &json!({ "id": acc.id }))
        .await
        .expect("get");
    assert_eq!(
        one["tokenExpiresAt"].as_str(),
        Some(own_expiry.as_str()),
        "来源是自有授权，不能被 Magpie 文件覆盖"
    );
    magpie::test_reset();
}

/// 单次 `accounts.list` RPC 在多账号场景下只读一次 Magpie 文件，并准确区分：
/// 恢复有效（DB expired + 文件 valid）、文件已过期、UID 串号、纯自有账号。
#[tokio::test]
async fn accounts_list_reads_magpie_once_across_multiple_accounts_and_covers_dynamic_states() {
    let (app, _dir) = test_app(SecureStore::memory());

    let uid_recovered = "6f1c2d94-aa01-5555-6666-777788889999";
    let uid_expired = "6f1c2d94-aa02-5555-6666-777788889999";
    let uid_mismatch = "6f1c2d94-aa03-5555-6666-777788889999";
    let uid_own = "6f1c2d94-aa04-5555-6666-777788889999";

    let l1 = magpie_login_with("workbuddy#r1", uid_recovered, "tok-r1");
    let l2 = magpie_login_with("workbuddy#r2", uid_expired, "tok-r2");
    let l3 = magpie_login_with("workbuddy#r3", uid_mismatch, "tok-r3");
    let l3_snapshot = fixture_expiry(&l3);
    import_logins(&app, vec![l1, l2, l3], &[]).expect("import three");
    persist_login(
        &app,
        &payload(uid_own, "own-only-access", Some("own-only-refresh")),
        WbVariant::Cn,
    )
    .expect("own login");

    let acc_recovered = account(&app, uid_recovered);
    let acc_expired = account(&app, uid_expired);
    let acc_mismatch = account(&app, uid_mismatch);
    let acc_own = account(&app, uid_own);

    // 把 acc_recovered 的库快照标成过期，模拟旧导入快照。
    const STALE: &str = "2020-09-13T12:26:40Z";
    let sql = format!(
            "UPDATE accounts SET auth_status = 'expired', token_expires_at = '{STALE}' WHERE id = '{id}';
             UPDATE account_authorizations SET status = 'expired', token_expires_at = '{STALE}'
              WHERE account_id = '{id}' AND kind = 'magpie';",
            id = acc_recovered.id
        );
    app.with_db(|db| db.exec_raw(&sql)).expect("mark stale");

    // 构造最新 Magpie 文件：r1 换新有效，r2 已过期，r3 串成别人的 UID。
    let mut live_r1 = magpie_login_with("workbuddy#r1", uid_recovered, "tok-r1-fresh");
    live_r1.expires_at_ms = Some(refreshed_expiry_ms());
    let want_r1 = fixture_expiry(&live_r1);

    let mut live_r2 = magpie_login_with("workbuddy#r2", uid_expired, "tok-r2-old");
    live_r2.expires_at_ms = Some(1_600_000_000_000); // 2020-09-13T12:26:40Z
    let want_r2 = fixture_expiry(&live_r2);

    let mut live_r3 = magpie_login_with("workbuddy#r3", "foreign-uid-999", "tok-r3-foreign");
    live_r3.expires_at_ms = Some(refreshed_expiry_ms());

    magpie::test_set_logins(Some(vec![live_r1, live_r2, live_r3]));
    magpie::test_reset_read_count();

    let list = dispatch(&app, "accounts.list", &json!({}))
        .await
        .expect("list");
    assert_eq!(
        magpie::test_read_count(),
        1,
        "整次 accounts.list RPC 无论有多少借用账号，只允许读取一次 Magpie 文件"
    );

    let rows = list["accounts"].as_array().expect("accounts array");
    let find_acc = |id: &str| {
        rows.iter()
            .find(|row| row["id"] == json!(id))
            .expect("account present")
    };
    fn find_auth<'a>(acc_val: &'a Value, kind: &str) -> &'a Value {
        acc_val["authorizations"]
            .as_array()
            .expect("authorizations")
            .iter()
            .find(|a| a["kind"] == json!(kind))
            .expect("auth row")
    }

    // 1) 恢复有效：DB expired，文件 valid + 未来到期时间
    let r1 = find_acc(&acc_recovered.id);
    let r1_auth = find_auth(r1, "magpie");
    assert_eq!(r1["tokenExpiresAt"].as_str(), Some(want_r1.as_str()));
    assert_eq!(r1_auth["tokenExpiresAt"].as_str(), Some(want_r1.as_str()));
    assert_eq!(r1_auth["effectiveStatus"].as_str(), Some("valid"));
    assert_eq!(r1_auth["recoverable"], json!(true));

    // 2) 文件已过期：上报真实过期时间 + effectiveStatus = expired
    let r2 = find_acc(&acc_expired.id);
    let r2_auth = find_auth(r2, "magpie");
    assert_eq!(r2["tokenExpiresAt"].as_str(), Some(want_r2.as_str()));
    assert_eq!(r2_auth["tokenExpiresAt"].as_str(), Some(want_r2.as_str()));
    assert_eq!(r2_auth["effectiveStatus"].as_str(), Some("expired"));
    assert_eq!(r2_auth["recoverable"], json!(false));

    // 3) UID 串号：effectiveStatus = mismatch，绝不采信别人的到期时间
    let r3 = find_acc(&acc_mismatch.id);
    let r3_auth = find_auth(r3, "magpie");
    assert_eq!(r3["tokenExpiresAt"].as_str(), Some(l3_snapshot.as_str()));
    assert_eq!(
        r3_auth["tokenExpiresAt"].as_str(),
        Some(l3_snapshot.as_str())
    );
    assert_eq!(r3_auth["effectiveStatus"].as_str(), Some("mismatch"));

    // 4) 纯自有账号：无动态状态且不受 Magpie 影响
    let r4 = find_acc(&acc_own.id);
    let r4_auth = find_auth(r4, "own");
    assert!(r4_auth["effectiveStatus"].is_null());

    // 保持纯只读：DB 里 acc_recovered 的旧快照一字未改
    assert_eq!(
        account(&app, uid_recovered).token_expires_at.as_deref(),
        Some(STALE)
    );
    magpie::test_reset();
}

/// 文件不可读时，多账号 `accounts.list` 同样只尝试读一次（OnceCell 缓存 None），
/// 保留库快照并标注 `effectiveStatus = "unavailable"`。
#[tokio::test]
async fn accounts_list_unreadable_magpie_reads_once_and_preserves_snapshot() {
    let (app, _dir) = test_app(SecureStore::memory());
    let l1 = magpie_login_with("workbuddy#u1", "uid-u1", "tok-u1");
    let l2 = magpie_login_with("workbuddy#u2", "uid-u2", "tok-u2");
    let s1 = fixture_expiry(&l1);
    let s2 = fixture_expiry(&l2);
    import_logins(&app, vec![l1, l2], &[]).expect("import two");
    let a1 = account(&app, "uid-u1");
    let a2 = account(&app, "uid-u2");

    magpie::test_set_unreadable();
    magpie::test_reset_read_count();

    let list = dispatch(&app, "accounts.list", &json!({}))
        .await
        .expect("list must succeed when magpie file is unreadable");
    assert_eq!(
        magpie::test_read_count(),
        1,
        "文件不可读时也只读一次，避免每账号重复失败 I/O"
    );

    let rows = list["accounts"].as_array().expect("accounts");
    for (id, expected_expiry) in [(&a1.id, s1.as_str()), (&a2.id, s2.as_str())] {
        let row = rows.iter().find(|r| r["id"] == json!(id)).expect("acc");
        assert_eq!(row["tokenExpiresAt"].as_str(), Some(expected_expiry));
        let auth = row["authorizations"]
            .as_array()
            .expect("auths")
            .iter()
            .find(|a| a["kind"] == json!("magpie"))
            .expect("magpie auth");
        assert_eq!(auth["effectiveStatus"].as_str(), Some("unavailable"));
        assert_eq!(auth["tokenExpiresAt"].as_str(), Some(expected_expiry));
    }
    magpie::test_reset();
}

/// 只有自有授权的账号列表绝不触发 Magpie 读盘；同时拥有 own（当前生效）+ magpie 的账号，
/// 无论 Magpie 文件变成新令牌、串号还是不可读，自有授权展示与顶层到期时间都完全隔离。
#[tokio::test]
async fn own_auth_is_isolated_from_magpie_reads_and_mutations() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid_only_own = "6f1c2d94-bb01-5555-6666-777788889999";
    persist_login(
        &app,
        &payload(uid_only_own, "own-a", Some("own-r")),
        WbVariant::Cn,
    )
    .expect("own login");

    // 只有 own 账号时，哪怕 Magpie 不可读也一次都不碰盘。
    magpie::test_set_unreadable();
    magpie::test_reset_read_count();
    let list = dispatch(&app, "accounts.list", &json!({}))
        .await
        .expect("list own only");
    assert_eq!(magpie::test_read_count(), 0, "无 Magpie 授权时不得读盘");
    assert_eq!(list["accounts"].as_array().unwrap().len(), 1);
    magpie::test_reset();

    // 同时拥有 own（首选）与 magpie（次选）：
    let uid_dual = "6f1c2d94-bb02-5555-6666-777788889999";
    let m = magpie_login_with("workbuddy#dual", uid_dual, "tok-dual");
    let m_snapshot = fixture_expiry(&m);
    import_logins(&app, vec![m], &[]).expect("import dual");
    persist_login(
        &app,
        &payload(uid_dual, "dual-own-access", Some("dual-own-refresh")),
        WbVariant::Cn,
    )
    .expect("dual own login");
    broker::set_preferred_source(&app, "own").expect("prefer own");
    let acc = account(&app, uid_dual);
    service::sync_mirror(&app, &acc.id).expect("sync mirror");
    let acc = account(&app, uid_dual);
    assert_eq!(acc.auth_source, "own");
    let own_expiry = app
        .with_db(|db| db.find_authorization(&acc.id, "own"))
        .expect("query")
        .expect("own row")
        .token_expires_at
        .expect("own expiry");

    // 场景 A：Magpie 串号
    let mut foreign = magpie_login_with("workbuddy#dual", "other-uid", "tok-other");
    foreign.expires_at_ms = Some(refreshed_expiry_ms());
    magpie::test_set_logins(Some(vec![foreign]));
    let view_mismatch = dispatch(&app, "accounts.get", &json!({ "id": acc.id }))
        .await
        .expect("get mismatch");
    assert_eq!(
        view_mismatch["tokenExpiresAt"].as_str(),
        Some(own_expiry.as_str())
    );
    let auths = view_mismatch["authorizations"].as_array().unwrap();
    let own_row = auths.iter().find(|a| a["kind"] == json!("own")).unwrap();
    let mag_row = auths.iter().find(|a| a["kind"] == json!("magpie")).unwrap();
    assert!(own_row["effectiveStatus"].is_null());
    assert_eq!(
        own_row["tokenExpiresAt"].as_str(),
        Some(own_expiry.as_str())
    );
    assert_eq!(mag_row["effectiveStatus"].as_str(), Some("mismatch"));
    assert_eq!(
        mag_row["tokenExpiresAt"].as_str(),
        Some(m_snapshot.as_str())
    );

    // 场景 B：Magpie 文件不可读
    magpie::test_set_unreadable();
    let view_unreadable = dispatch(&app, "accounts.get", &json!({ "id": acc.id }))
        .await
        .expect("get unreadable");
    assert_eq!(
        view_unreadable["tokenExpiresAt"].as_str(),
        Some(own_expiry.as_str())
    );
    let mag_row = view_unreadable["authorizations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == json!("magpie"))
        .unwrap();
    assert_eq!(mag_row["effectiveStatus"].as_str(), Some("unavailable"));
    magpie::test_reset();
}

/// 跨站点平台不匹配（CN vs International）与来源键缺失：
/// 绝不采信错误站点的到期时间，并分别报告 `mismatch` / `expired`。
#[tokio::test]
async fn accounts_get_distinguishes_platform_mismatch_and_missing_source_key() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-cc01-5555-6666-777788889999";
    let ai_login = magpie_login_with("workbuddy-ai#intl", uid, "tok-intl");
    let snapshot = fixture_expiry(&ai_login);
    import_logins(&app, vec![ai_login], &[]).expect("import ai");
    let acc = account(&app, uid);
    assert_eq!(acc.provider, "workbuddy_ai");

    // 场景 A：文件里把同一条记录换成了国内版键，来源键缺失 -> effectiveStatus = expired，保留快照
    let mut cn_login = magpie_login_with("workbuddy#cn", uid, "tok-cn");
    cn_login.expires_at_ms = Some(refreshed_expiry_ms());
    magpie::test_set_logins(Some(vec![cn_login]));
    let missing = dispatch(&app, "accounts.get", &json!({ "id": acc.id }))
        .await
        .expect("get missing");
    assert_eq!(missing["tokenExpiresAt"].as_str(), Some(snapshot.as_str()));
    let mag = missing["authorizations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == json!("magpie"))
        .unwrap();
    assert_eq!(mag["effectiveStatus"].as_str(), Some("expired"));

    // 场景 B：来源键存在，但数据库 provider 与 Magpie provider 跨站不匹配 -> effectiveStatus = mismatch
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE accounts SET provider = 'workbuddy' WHERE id = '{}';",
            acc.id
        ))
    })
    .expect("flip provider to cn");
    let mut intl_fresh = magpie_login_with("workbuddy-ai#intl", uid, "tok-intl-fresh");
    intl_fresh.expires_at_ms = Some(refreshed_expiry_ms());
    magpie::test_set_logins(Some(vec![intl_fresh]));
    let mismatch = dispatch(&app, "accounts.get", &json!({ "id": acc.id }))
        .await
        .expect("get platform mismatch");
    assert_eq!(mismatch["tokenExpiresAt"].as_str(), Some(snapshot.as_str()));
    let mag = mismatch["authorizations"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["kind"] == json!("magpie"))
        .unwrap();
    assert_eq!(mag["effectiveStatus"].as_str(), Some("mismatch"));
    magpie::test_reset();
}

/// 旧式无 `source_key` 的 magpie 行不应触发 Magpie 读盘，且 `service::account_view`
/// 内部的 `describe` 应复用同一份 `MagpieLoginsCache`（不额外多读一次）。
#[tokio::test]
async fn legacy_magpie_without_source_key_skips_read_and_account_view_reuses_cache() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid_legacy = "6f1c2d94-dd01-5555-6666-777788889999";
    let login = magpie_login_with("workbuddy#legacy", uid_legacy, "tok-legacy");
    import_logins(&app, vec![login], &[]).expect("import");
    let acc = account(&app, uid_legacy);
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE account_authorizations
                    SET access_ref = 'keychain:legacy-key', source_key = NULL
                  WHERE account_id = '{}' AND kind = 'magpie';",
            acc.id
        ))
    })
    .expect("convert to legacy keychain magpie");

    magpie::test_set_unreadable();
    magpie::test_reset_read_count();
    let _ = dispatch(&app, "accounts.get", &json!({ "id": acc.id }))
        .await
        .expect("get legacy");
    assert_eq!(
        magpie::test_read_count(),
        0,
        "没有 source_key 的旧式 magpie 行不应触发 Magpie 读盘"
    );
    magpie::test_reset();

    // 正常动态 magpie 账号走 service::account_view：
    // display_expiry + describe + summary.describe 共享一份 cache（+ resolve 最多 1 次 = 总计 <= 2）。
    let uid_view = "6f1c2d94-dd02-5555-6666-777788889999";
    let live = magpie_login_with("workbuddy#view", uid_view, "tok-view");
    import_logins(&app, vec![live.clone()], &[]).expect("import view");
    let acc_view = account(&app, uid_view);
    magpie::test_set_logins(Some(vec![live]));
    magpie::test_reset_read_count();
    let _ = service::account_view(&app, &acc_view).expect("account_view");
    assert!(
        magpie::test_read_count() <= 2,
        "account_view 内的 summary.describe 应复用缓存，实测读盘 {} 次",
        magpie::test_read_count()
    );
    magpie::test_reset();
}

/// 首选那份失效时兜底到另一份，且不改另一份的状态。
#[test]
fn invalid_preferred_falls_back_to_the_other_authorization() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-5555-6666-7777-888899990000";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);

    let magpie_row = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .unwrap();
    assert!(
        service::record_status(&app, &magpie_row, "invalid", None, Some("unauthorized")).unwrap()
    );

    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => {
            assert_eq!(resolved.kind, "own", "首选坏了就该兜底");
            assert!(resolved.degraded.is_some(), "兜底必须说明原因");
            assert_eq!(resolved.access, "own-access");
        }
        broker::Outcome::Unavailable { reason, .. } => panic!("应当兜底：{reason}"),
    }
    let own_row = app
        .with_db(|db| db.find_authorization(&acc.id, "own"))
        .unwrap()
        .unwrap();
    assert_eq!(own_row.status, "valid", "兜底不能顺手改另一份的状态");
}

/// 表上的 CHECK 必须挡住"magpie 带 refresh"这种行。
#[test]
fn magpie_row_cannot_carry_a_refresh_ref() {
    let (app, _dir) = test_app(SecureStore::memory());
    let acc = app
        .with_db(|db| {
            db.upsert_account(&AccountInput {
                platform: "workbuddy".into(),
                uid: "uid-check".into(),
                ..Default::default()
            })
        })
        .expect("seed");
    let row = crate::storage::AuthorizationRecord {
        id: ids::auth_id(&acc.id, "magpie"),
        account_id: acc.id.clone(),
        kind: "magpie".to_string(),
        access_ref: ids::keychain_ref(&ids::access_key(&acc.id, "magpie")),
        refresh_ref: Some("keychain:workbuddy/x/magpie/refresh".into()),
        auth_domain: None,
        token_expires_at: None,
        status: "valid".into(),
        version: 0,
        key_migration_state: "ready".into(),
        source_key: None,
        last_used_at: None,
        last_error: None,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    assert!(
        app.with_db(|db| db.upsert_authorization(&row)).is_err(),
        "CHECK (kind <> 'magpie' OR refresh_ref IS NULL) 必须生效"
    );
}

// -----------------------------------------------------------------------
// 写入失败 / 回滚
// -----------------------------------------------------------------------

#[test]
fn login_without_refresh_drops_the_stale_one() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-aaaa-bbbb-cccc-ddddeeeeffff";
    persist_login(
        &app,
        &payload(uid, "first-access", Some("first-refresh")),
        WbVariant::Cn,
    )
    .expect("first");
    persist_login(&app, &payload(uid, "second-access", None), WbVariant::Cn).expect("second");

    let acc = account(&app, uid);
    assert!(
        own_refresh(&app, &acc.id).is_none(),
        "没有新 refresh 必须清掉旧的"
    );
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("second-access"));
}

/// 新登录根本不碰钥匙串：后端把一切都拒了，登录照样成功。
#[test]
fn a_new_login_never_touches_a_failing_keychain() {
    let (app, _dir) = test_app(SecureStore::failing());
    let uid = "6f1c2d94-9999-8888-7777-666655554444";
    persist_login(
        &app,
        &payload(uid, "access", Some("refresh")),
        WbVariant::Cn,
    )
    .expect("JSON 路径不该被钥匙串挡住");

    let acc = account(&app, uid);
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("access"));
    assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("refresh"));
    assert_eq!(own_row(&app, &acc.id).storage_kind(), "localjson");
}

/// JSON 写入失败：账号行、授权行、凭据文件一个都不许留下。
#[test]
fn a_failed_json_write_leaves_no_secret_and_no_account() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-8888-7777-666655554445";
    crate::credentials::localjson::test_inject_fault(
        crate::credentials::localjson::FaultPoint::TempFile,
    );

    let err = persist_login(
        &app,
        &payload(uid, "access", Some("refresh")),
        WbVariant::Cn,
    )
    .expect_err("凭据写不进去就该失败");
    crate::credentials::localjson::test_clear_faults();

    assert_eq!(err.code, crate::rpc::E_SECURESTORE);
    assert!(
        app.with_db(|db| db.find_account("workbuddy", uid))
            .unwrap()
            .is_none(),
        "凭据没落盘就不能留下账号行"
    );
    assert!(secret_ids(&app).is_empty(), "不许留下半份凭据文件");
}

/// 老账号：refresh 写失败时旧的一对保持不变（含旧 refresh，它可能还能用）。
#[test]
fn legacy_keychain_refresh_write_failure_keeps_the_old_pair() {
    let (app, _dir) = test_app(SecureStore::faulty_on_value("boom"));
    let uid = "6f1c2d94-aaaa-0001-0002-000000000001";
    let account_id = seed_legacy_own(&app, uid, "old-access", Some("old-refresh"));
    app.secure.arm();

    let err = persist_login(
        &app,
        &payload(uid, "new-access", Some("new-refresh-boom")),
        WbVariant::Cn,
    )
    .expect_err("refresh 写不进去就该失败");
    assert_eq!(err.data.as_ref().unwrap()["stage"], json!("own/refresh"));
    assert_eq!(own_access(&app, &account_id).as_deref(), Some("old-access"));
    assert_eq!(
        own_refresh(&app, &account_id).as_deref(),
        Some("old-refresh")
    );
}

/// 老账号（`keychain:` 引用）的写入失败：新值写不进，旧值必须原样还在。
///
/// 这条必须显式造一份旧行 —— 直接调 `persist_login` 现在只会走 JSON，
/// 那样测的是另一条路径，等于把 CRED-4 之前的老账号完全丢出覆盖。
#[test]
fn legacy_keychain_login_keeps_the_old_pair_when_the_write_fails() {
    let (app, _dir) = test_app(SecureStore::faulty_on_value("boom"));
    let uid = "6f1c2d94-aaaa-0001-0002-000000000002";
    let account_id = seed_legacy_own(&app, uid, "old-access", Some("old-refresh"));
    app.secure.arm();

    let err = persist_login(
        &app,
        &payload(uid, "new-access-boom", Some("new-refresh")),
        WbVariant::Cn,
    )
    .expect_err("access 写不进去就该失败");
    assert_eq!(err.data.as_ref().unwrap()["stage"], json!("own/access"));
    assert_eq!(own_access(&app, &account_id).as_deref(), Some("old-access"));
    assert_eq!(
        own_refresh(&app, &account_id).as_deref(),
        Some("old-refresh")
    );
    // 老行不能被"升级"成 JSON：CRED-3 不做隐式迁移。
    assert_eq!(own_row(&app, &account_id).storage_kind(), "keychain");
}

/// 老账号：这次登录没给 refresh，旧的必须清掉；清不掉就整体失败。
#[test]
fn legacy_keychain_stale_refresh_cleanup_failure_aborts_the_login() {
    let (app, _dir) = test_app(SecureStore::faulty(&[], &["/own/refresh"]));
    let uid = "6f1c2d94-aaaa-0001-0002-000000000003";
    let account_id = seed_legacy_own(&app, uid, "old-access", Some("old-refresh"));
    app.secure.arm();

    let err = persist_login(&app, &payload(uid, "new-access", None), WbVariant::Cn)
        .expect_err("必须失败");
    assert_eq!(
        err.data.as_ref().unwrap()["kind"],
        json!("stale_refresh_not_cleared")
    );
    assert_eq!(own_access(&app, &account_id).as_deref(), Some("old-access"));
    assert_eq!(
        own_refresh(&app, &account_id).as_deref(),
        Some("old-refresh")
    );
}

/// 入库失败：恢复登录前的凭据，借用那份一个字节不动。
#[test]
fn db_failure_keeps_previous_credentials() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-aaaa-0001-0002-000000000004";
    import_logins(&app, vec![magpie_login(uid, "borrowed-access")], &[]).expect("import");
    let acc = account(&app, uid);

    app.with_db(|db| {
        db.exec_raw(
            "CREATE TRIGGER no_update BEFORE UPDATE ON accounts
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
    })
    .expect("install trigger");

    let err = persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect_err("库里写不进去就该失败");
    assert_eq!(err.code, crate::rpc::E_STORAGE);

    let want_ref = format!("magpie:v1:workbuddy#{uid}");
    assert_eq!(
        magpie_ref(&app, &acc.id).as_deref(),
        Some(want_ref.as_str()),
        "借用授权必须原样保留"
    );
    assert!(
        magpie_access(&app, &acc.id).is_none(),
        "借用授权始终不进钥匙串"
    );
    assert!(
        own_access(&app, &acc.id).is_none(),
        "登录失败不能留下自有凭据"
    );
}

/// 老账号：读不到旧凭据就不能盲写（否则快照回滚会写进一份猜的值）。
///
/// 读取的故障注入只在 `arm()` 之后生效，所以先种数据、再武装。
#[test]
fn legacy_keychain_snapshot_read_failure_aborts_the_login() {
    let (app, _dir) = test_app(SecureStore::faulty_on_read());
    let uid = "6f1c2d94-dddd-0001-0002-000000000001";
    let account_id = seed_legacy_own(&app, uid, "old-access", Some("old-refresh"));
    app.secure.arm();

    let err = persist_login(
        &app,
        &payload(uid, "new-access", Some("new-refresh")),
        WbVariant::Cn,
    )
    .expect_err("读不到旧凭据就不能写");
    assert_eq!(
        err.data.as_ref().unwrap()["kind"],
        json!("snapshot_unreadable")
    );

    app.secure.disarm();
    assert_eq!(own_access(&app, &account_id).as_deref(), Some("old-access"));
    assert_eq!(
        own_refresh(&app, &account_id).as_deref(),
        Some("old-refresh")
    );
}

/// 老账号：回滚本身也失败时必须如实报告，不能假装干净。
#[test]
fn legacy_keychain_rollback_failure_is_reported_as_incomplete() {
    let (app, _dir) = test_app(SecureStore::faulty(&["/own/access"], &[]));
    let uid = "6f1c2d94-dddd-0001-0002-000000000002";
    let account_id = seed_legacy_own(&app, uid, "old-access", None);
    app.secure.arm();

    let err = persist_login(
        &app,
        &payload(uid, "new-access", Some("new-refresh")),
        WbVariant::Cn,
    )
    .expect_err("写不进去就该失败");
    assert_eq!(
        err.data.as_ref().unwrap()["kind"],
        json!("rollback_incomplete"),
        "{err:?}"
    );
    assert!(err.message.contains("未能完整恢复"), "{}", err.message);
    assert_eq!(own_row(&app, &account_id).storage_kind(), "keychain");
}

// -----------------------------------------------------------------------
// 重同步 / 刷新
// -----------------------------------------------------------------------

#[test]
fn resync_refuses_an_account_without_magpie_authorization() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-1234-5678-90ab-cdef12345678";
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("persist");
    let acc = account(&app, uid);

    let err = resync_magpie_access(&app, &acc).expect_err("没有 magpie 授权必须被拒绝");
    assert_eq!(
        err.data.as_ref().unwrap()["kind"],
        json!("not_magpie_source")
    );
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("own-access"));
}

#[test]
fn resync_rejects_a_snapshot_whose_epoch_changed() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-eeee-0001-0002-000000000002";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let stale = account(&app, uid);

    app.with_db(|db| {
        db.upsert_account(&AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.to_string(),
            nickname: Some("刚改过".to_string()),
            ..Default::default()
        })
    })
    .expect("touch");

    let err = resync_magpie_access(&app, &stale).expect_err("版本对不上就该拒绝");
    assert_eq!(err.data.as_ref().unwrap()["kind"], json!("superseded"));
}

/// 扫码之后，拿着旧快照去 resync magpie：版本变了，必须拒绝，own 一个字节不动。
#[test]
fn stale_snapshot_cannot_overwrite_a_new_own_login() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-eeee-0001-0002-000000000001";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let stale = account(&app, uid);
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);

    let err = resync_magpie_access(&app, &stale).expect_err("旧快照必须被拒绝");
    assert_eq!(err.data.as_ref().unwrap()["kind"], json!("superseded"));
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("own-access"));
    assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("own-refresh"));
}

/// 自有授权缺 refresh：刷新必须明确报"要重新登录"，且不能顺手删掉还能用的 access。
#[test]
fn refresh_without_own_refresh_reports_missing_credential() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-eeee-0001-0002-000000000004";
    // 登录时服务端没发 refresh → 只留 access
    persist_login(&app, &payload(uid, "access-only", None), WbVariant::Cn).expect("login");
    let acc = account(&app, uid);
    let account_id = acc.id.clone();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let out = runtime
        .block_on(refresh_account(&app, &acc.id))
        .expect("refresh 不该抛错");

    assert_eq!(out["state"], json!("invalid"));
    assert_eq!(out["error"]["kind"], json!("missing_credential"));
    assert_eq!(
        own_access(&app, &account_id).as_deref(),
        Some("access-only"),
        "access 不能被顺手删掉：用户可能还想用它只读查询"
    );
    let row = app
        .with_db(|db| db.find_authorization(&account_id, "own"))
        .unwrap()
        .unwrap();
    assert_eq!(row.status, "invalid");
}

#[test]
fn rotation_is_dropped_when_the_epoch_moved_on() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-eeee-0001-0002-000000000003";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let stale = account(&app, uid);
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);

    let out = persist_rotated_tokens(
        &app,
        &stale,
        stale.auth_epoch,
        "rotated-access",
        Some("rotated-refresh"),
        None,
    )
    .expect("应当返回 superseded 而不是报错");
    assert_eq!(out["state"], json!("superseded"));
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("own-access"));
    assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("own-refresh"));
}

/// 轮换写失败：文件保持旧内容（原子替换，没有半份），但旧 refresh 已被服务端
/// 作废，所以这份授权确实不能再用 —— 标 invalid，要求重新登录。
#[test]
fn rotation_write_failure_keeps_the_file_intact_and_marks_invalid() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-aaaa-0001-0002-000000000005";
    persist_login(
        &app,
        &payload(uid, "old-access", Some("dead-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    let account_id = acc.id.clone();
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let secret_id = secret_id_of(&app, &account_id);
    let before = store.load(&secret_id).expect("load");

    crate::credentials::localjson::test_inject_fault(
        crate::credentials::localjson::FaultPoint::TempFile,
    );
    let out = persist_rotated_tokens(
        &app,
        &acc,
        acc.auth_epoch,
        "new-access",
        Some("new-refresh"),
        None,
    )
    .expect("返回 invalid 而不是抛错");
    crate::credentials::localjson::test_clear_faults();

    assert_eq!(out["state"], json!("invalid"));
    assert_eq!(out["needsRelogin"], json!(true));
    assert_eq!(out["error"]["kind"], json!("refresh_not_persisted"));

    // 盘上一个字节都没变：access + refresh 是一起失败的。
    let after = store.load(&secret_id).expect("load");
    assert_eq!(after.revision, before.revision);
    assert_eq!(own_access(&app, &account_id).as_deref(), Some("old-access"));
    assert_eq!(
        own_refresh(&app, &account_id).as_deref(),
        Some("dead-refresh")
    );
    assert_eq!(own_row(&app, &account_id).status, "invalid");
}

/// P0 回归：轮换**写**路径也必须验归属 —— 引用指错时不能改写别人的令牌。
#[test]
fn rotation_refuses_to_write_another_accounts_secret() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid_a = "6f1c2d94-0000-0001-0002-0000000000f1";
    let uid_b = "6f1c2d94-0000-0001-0002-0000000000f2";
    persist_login(
        &app,
        &payload(uid_a, "access-a", Some("refresh-a")),
        WbVariant::Cn,
    )
    .expect("login a");
    persist_login(
        &app,
        &payload(uid_b, "access-b", Some("refresh-b")),
        WbVariant::Cn,
    )
    .expect("login b");
    let a = account(&app, uid_a);
    let b = account(&app, uid_b);
    let b_secret = secret_id_of(&app, &b.id);
    let b_path = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
        .root()
        .join(format!("{b_secret}.json"));
    let b_bytes = std::fs::read(&b_path).expect("read b");

    // A 的引用被改成 B 的合法文件。
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE account_authorizations
                    SET access_ref = 'localjson:v1:{b_secret}',
                        refresh_ref = 'localjson:v1:{b_secret}'
                  WHERE account_id = '{}' AND kind = 'own'",
            a.id
        ))
    })
    .expect("tamper");

    let out = persist_rotated_tokens(
        &app,
        &a,
        a.auth_epoch,
        "attacker-access",
        Some("attacker-refresh"),
        None,
    )
    .expect("返回 invalid 而不是抛错");

    assert_eq!(out["state"], json!("invalid"), "{out}");
    assert_eq!(
        out["error"]["kind"],
        json!("credential_ownership_mismatch"),
        "{out}"
    );
    assert_eq!(
        out["needsRelogin"],
        json!(false),
        "串号不是「重新登录」能解决的，别把坏引用盖掉：{out}"
    );

    // B 的凭据一个字节都没变，而且仍然可读。
    assert_eq!(std::fs::read(&b_path).expect("read b"), b_bytes);
    assert_eq!(own_access(&app, &b.id).as_deref(), Some("access-b"));
    assert_eq!(own_refresh(&app, &b.id).as_deref(), Some("refresh-b"));
    // A 那一行也没被标成 invalid（不掩盖数据损坏）。
    assert_eq!(own_row(&app, &a.id).status, "valid");
}

/// P1 回归：`settings.set` 认的键，单键 `settings.get` 也必须能读回来。
#[tokio::test]
async fn settings_round_trip_covers_the_plaintext_consent_key() {
    let (app, _dir) = test_app_without_consent(SecureStore::memory());
    let key = crate::auth::consent::JSON_CONSENT_SETTING;

    let before = dispatch(&app, "settings.get", &json!({ "key": key }))
        .await
        .expect("get before");
    assert_eq!(before["value"], json!("0"), "{before}");

    dispatch(&app, "settings.set", &json!({ "key": key, "value": "1" }))
        .await
        .expect("set");

    let after = dispatch(&app, "settings.get", &json!({ "key": key }))
        .await
        .expect("get after");
    assert_eq!(after["value"], json!("1"), "{after}");
    assert!(crate::auth::consent::json_storage_enabled(&app));

    // 全量查询也要一致。
    let all = dispatch(&app, "settings.get", &json!({}))
        .await
        .expect("get all");
    assert_eq!(all["settings"][key], json!("1"), "{all}");
}

/// 轮换成功：access 与 refresh 一次换掉，版本只前进一格。
#[test]
fn rotation_replaces_both_tokens_in_one_write() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-aaaa-0001-0002-000000000007";
    persist_login(
        &app,
        &payload(uid, "old-access", Some("old-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let secret_id = secret_id_of(&app, &acc.id);
    let before = store.load(&secret_id).expect("load");

    let out = persist_rotated_tokens(
        &app,
        &acc,
        acc.auth_epoch,
        "new-access",
        Some("new-refresh"),
        None,
    )
    .expect("rotate");
    assert_eq!(out["state"], json!("valid"), "{out}");

    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("new-access"));
    assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("new-refresh"));
    let after = store.load(&secret_id).expect("load");
    assert_eq!(after.revision, before.revision + 1, "一次替换只该推进一格");
    // 没有留下第二份文件。
    assert_eq!(secret_ids(&app), vec![secret_id]);
}

/// P0 回归：轮换提交时 SQLite 引用已经被迁走 —— 不许写旧文件，更不许报成功。
///
/// 确定性交错：`owned::load`（验归属 + 取快照）→ **别的进程把 ref 迁到 G**
/// → 迟到的 `commit_rotated_json` 才发现版本对不上。
#[test]
fn rotation_supersedes_when_the_authorization_moves_mid_flight() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-0000-0001-0002-0000000000g1";
    persist_login(
        &app,
        &payload(uid, "old-access", Some("old-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    let row = own_row(&app, &acc.id);
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let f_id = secret_id_of(&app, &acc.id);

    // ① 轮换前那一刻：验过归属的快照（这正是 persist_rotated_json 做的事）。
    let verified = crate::auth::owned::load(&app, &acc, &row).expect("verified");

    // ② 另一个进程把引用迁到 G，旧文件 F **保留**（迁移/换源之后可能出现这种局面）。
    let other = store
        .create(crate::credentials::localjson::SecretDraft::oauth(
            ids::auth_id(&acc.id, ids::KIND_OWN),
            "workbuddy",
            "moved-access",
        ))
        .expect("other secret");
    let other_ref = crate::credentials::reference::local_json_ref(&other.secret_id).expect("ref");
    let mut moved = row.clone();
    moved.access_ref = other_ref.clone();
    moved.refresh_ref = Some(other_ref.clone());
    assert!(app
        .with_db(|db| db.replace_authorization_cas(&moved, row.version, &row.access_ref))
        .expect("cas"));
    let g_path = store.root().join(format!("{}.json", other.secret_id));
    let g_before = std::fs::read(&g_path).expect("read g");

    // ③ 迟到的轮换提交：必须 superseded，且不能把新令牌留在 F 里。
    let out = commit_rotated_json(
        &app,
        &row,
        &store,
        &f_id,
        &verified,
        "late-access",
        Some("late-refresh"),
        None,
    )
    .expect("superseded 不是错误");

    assert_eq!(out["state"], json!("superseded"), "{out}");
    assert_eq!(out["reason"], json!("authorization_moved"), "{out}");
    assert_eq!(out["fileReverted"], json!(true), "{out}");
    assert_eq!(out["orphanSecret"], json!(false), "{out}");

    let f_after = store.load(&f_id).expect("load f");
    assert_eq!(
        f_after.credentials.access_token.as_deref(),
        Some("old-access"),
        "旧文件里不能留下新令牌"
    );
    assert_eq!(
        f_after.credentials.refresh_token.as_deref(),
        Some("old-refresh")
    );
    assert_eq!(
        std::fs::read(&g_path).expect("read g"),
        g_before,
        "G 不许被改"
    );
    assert_eq!(own_row(&app, &acc.id).access_ref, other_ref, "行必须还是 G");
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("moved-access"));
}

/// P0 回归：**版本变了 ≠ 引用变了** —— 只更新过状态的账号不许被回滚成旧令牌。
///
/// 交错：`owned::load` 取到快照 → 另一个流程只调了 `record_status`（版本 +1，引用仍是 F）
/// → 迟到的轮换提交。此时回滚会把服务端已作废的旧 refresh 写回去，账号反而更坏。
#[test]
fn rotation_keeps_the_new_tokens_when_only_the_row_version_changes() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-0000-0001-0002-0000000000g3";
    persist_login(
        &app,
        &payload(uid, "old-access", Some("old-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    let row = own_row(&app, &acc.id);
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let f_id = secret_id_of(&app, &acc.id);
    let verified = crate::auth::owned::load(&app, &acc, &row).expect("verified");

    // 另一个流程只更新了状态：版本 +1，`access_ref` 一个字符都没变。
    assert!(app
        .with_db(|db| db.set_authorization_status(&row.id, row.version, "valid", None, None))
        .expect("status"));

    let out = commit_rotated_json(
        &app,
        &row,
        &store,
        &f_id,
        &verified,
        "new-access",
        Some("new-refresh"),
        None,
    )
    .expect("superseded 不是错误");

    assert_eq!(out["state"], json!("superseded"), "{out}");
    assert_eq!(
        out["reason"],
        json!("authorization_version_changed"),
        "{out}"
    );
    assert_eq!(out["fileReverted"], json!(false), "不许回滚：{out}");
    assert_eq!(out["fileUpdated"], json!(true), "{out}");

    // 新令牌必须留在 F 里 —— 回滚等于毁掉唯一有效的 refresh。
    let f = store.load(&f_id).expect("load");
    assert_eq!(f.credentials.access_token.as_deref(), Some("new-access"));
    assert_eq!(f.credentials.refresh_token.as_deref(), Some("new-refresh"));
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("new-access"));
}

/// P0 回归：状态提交失败时**不许**回报 `state=valid`。
#[test]
fn rotation_never_reports_valid_when_the_status_cannot_be_written() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-0000-0001-0002-0000000000g2";
    persist_login(
        &app,
        &payload(uid, "old-access", Some("old-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    app.with_db(|db| {
        db.exec_raw(
            "CREATE TRIGGER no_auth_update BEFORE UPDATE ON account_authorizations
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
    })
    .expect("install trigger");

    let out = persist_rotated_tokens(
        &app,
        &acc,
        acc.auth_epoch,
        "new-access",
        Some("new-refresh"),
        None,
    )
    .expect("返回 invalid 而不是抛错");

    assert_eq!(out["state"], json!("invalid"), "{out}");
    assert_eq!(
        out["error"]["kind"],
        json!("rotation_not_committed"),
        "{out}"
    );
    assert_eq!(out["needsRelogin"], json!(false), "{out}");
    // 新令牌是真的可用（旧 refresh 已被服务端作废），所以保留在文件里。
    assert_eq!(out["fileUpdated"], json!(true), "{out}");
    app.with_db(|db| db.exec_raw("DROP TRIGGER no_auth_update;"))
        .expect("drop trigger");
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("new-access"));
}

/// 并发轮换：后到的一方拿着旧版本，必须 superseded，而不是把人家的新令牌盖掉。
#[test]
fn a_concurrent_rotation_is_reported_as_superseded() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-aaaa-0001-0002-000000000008";
    persist_login(
        &app,
        &payload(uid, "old-access", Some("old-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    let row = own_row(&app, &acc.id);
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let secret_id = secret_id_of(&app, &acc.id);
    let snapshot = crate::auth::owned::load(&app, &acc, &row).expect("verified snapshot");

    // 另一个进程先写了同一个文件（它看到了同一个 revision）。
    store
        .replace_credentials(
            &secret_id,
            snapshot.revision,
            crate::credentials::localjson::Credentials::oauth("other-access", "other-refresh"),
            None,
        )
        .expect("另一个写入者");

    // 我拿着旧的 revision 来写：必须被拒，且不标失效。
    let out = commit_rotated_json(
        &app,
        &row,
        &store,
        &secret_id,
        &snapshot,
        "new-access",
        Some("new-refresh"),
        None,
    )
    .expect("superseded 不是错误");
    assert_eq!(out["state"], json!("superseded"), "{out}");
    assert_eq!(out["reason"], json!("credential_revision_changed"));
    assert_eq!(
        own_access(&app, &acc.id).as_deref(),
        Some("other-access"),
        "盘上必须是先写成功的那一份"
    );
    assert_eq!(own_row(&app, &acc.id).status, "valid", "不许标失效");
}

/// 同一份授权重新登录复用同一个文件：不留孤儿文件，也不换引用。
#[test]
fn a_second_login_reuses_the_same_secret_file() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-aaaa-0001-0002-000000000009";
    persist_login(
        &app,
        &payload(uid, "first-access", Some("first-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    let first_ref = own_row(&app, &acc.id).access_ref;
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let secret_id = secret_id_of(&app, &acc.id);
    let first_revision = store.load(&secret_id).expect("load").revision;

    persist_login(
        &app,
        &payload(uid, "second-access", Some("second-refresh")),
        WbVariant::Cn,
    )
    .expect("re-login");

    let acc = account(&app, uid);
    assert_eq!(
        own_row(&app, &acc.id).access_ref,
        first_ref,
        "重新登录不该换文件"
    );
    assert_eq!(
        secret_ids(&app),
        vec![secret_id.clone()],
        "不该留下孤儿文件"
    );
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("second-access"));
    assert_eq!(
        own_refresh(&app, &acc.id).as_deref(),
        Some("second-refresh")
    );
    assert!(store.load(&secret_id).expect("load").revision > first_revision);
}

/// 已有 JSON 授权重新登录、写入失败：旧的一对必须原样还在。
#[test]
fn an_existing_json_login_keeps_the_old_pair_when_the_write_fails() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-aaaa-0001-0002-00000000000a";
    persist_login(
        &app,
        &payload(uid, "old-access", Some("old-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    let secret_id = secret_id_of(&app, &acc.id);
    let before = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
        .load(&secret_id)
        .expect("load");

    crate::credentials::localjson::test_inject_fault(
        crate::credentials::localjson::FaultPoint::TempFile,
    );
    persist_login(
        &app,
        &payload(uid, "new-access", Some("new-refresh")),
        WbVariant::Cn,
    )
    .expect_err("写不进去就该失败");
    crate::credentials::localjson::test_clear_faults();

    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("old-access"));
    assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("old-refresh"));
    let after = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
        .load(&secret_id)
        .expect("load");
    assert_eq!(after.revision, before.revision, "失败不该推进版本");
    assert_eq!(secret_ids(&app), vec![secret_id]);
}

/// 引用与状态必须一次提交：库拒绝写入时，两个字段都不能动。
#[test]
fn record_grant_keeps_refs_and_status_in_one_write() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-aaaa-0001-0002-00000000000b";
    let account_id = seed_legacy_own(&app, uid, "old-access", Some("old-refresh"));

    // 准备一份可以指过去的 JSON（模拟凭据已经落盘）。
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let secret = store
        .create(
            crate::credentials::localjson::SecretDraft::oauth(
                ids::auth_id(&account_id, ids::KIND_OWN),
                "workbuddy",
                "json-access",
            )
            .with_refresh_token("json-refresh"),
        )
        .expect("create");
    let reference = crate::credentials::reference::local_json_ref(&secret.secret_id).expect("ref");

    // 让这次 UPSERT 失败：引用与状态必须一起被拒。
    app.with_db(|db| {
        db.exec_raw(
            "CREATE TRIGGER no_auth_update BEFORE UPDATE ON account_authorizations
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
    })
    .expect("install trigger");

    // 这条用例模拟"写凭据时这一行已存在"的时序：先人为造一行，再拿它的版本做 CAS。
    let expected = own_row(&app, &account_id);
    let err = match service::record_grant(
        &app,
        &account_id,
        uid,
        ids::KIND_OWN,
        None,
        Some((&reference, Some(&reference))),
        &service::GrantExpectation::Present {
            version: expected.version,
            access_ref: expected.access_ref.clone(),
        },
    ) {
        Ok(_) => panic!("库写不进去就该失败"),
        Err(err) => err,
    };
    assert_eq!(err.code, crate::rpc::E_STORAGE);

    // 一个字段都没动：没有"引用换了但状态还是旧的"这种中间态。
    let row = own_row(&app, &account_id);
    assert_eq!(row.storage_kind(), "keychain", "引用不许先被改掉");
    assert_eq!(row.status, "valid", "状态不许先被改掉");
    assert_eq!(own_access(&app, &account_id).as_deref(), Some("old-access"));
}

/// P0-1 回归：引用指到**别人的**文件时，读 / 重登 / 删除都必须拒绝，且不碰对方。
///
/// 这条是"凭据永远属于正确账号"的核心证据：文件权限 0600 挡不住业务层串号。
#[tokio::test]
async fn a_reference_to_another_accounts_secret_is_refused_on_every_path() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid_a = "6f1c2d94-0000-0001-0002-0000000000b1";
    let uid_b = "6f1c2d94-0000-0001-0002-0000000000b2";
    persist_login(
        &app,
        &payload(uid_a, "access-a", Some("refresh-a")),
        WbVariant::Cn,
    )
    .expect("login a");
    persist_login(
        &app,
        &payload(uid_b, "access-b", Some("refresh-b")),
        WbVariant::Cn,
    )
    .expect("login b");
    let a = account(&app, uid_a);
    let b = account(&app, uid_b);
    let b_secret = secret_id_of(&app, &b.id);
    let b_path = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths)
        .root()
        .join(format!("{b_secret}.json"));
    let b_bytes = std::fs::read(&b_path).expect("read b");

    // 人为把 A 的引用改成 B 的文件（模拟迁移出错 / 库损坏）。
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE account_authorizations
                    SET access_ref = 'localjson:v1:{b_secret}',
                        refresh_ref = 'localjson:v1:{b_secret}'
                  WHERE account_id = '{}' AND kind = 'own'",
            a.id
        ))
    })
    .expect("tamper");

    // ① 读：拒绝，绝不会把 B 的令牌返回给 A。
    let read = service::read_access(&app, &a, ids::KIND_OWN);
    assert!(read.is_err(), "不能把 B 的令牌返回给 A：{read:?}");
    let refresh = service::read_own_refresh(&app, &a);
    assert!(refresh.is_err(), "{refresh:?}");
    match broker::resolve(&app, &a).expect("resolve 不抛错") {
        broker::Outcome::Resolved(resolved) => {
            panic!("不许解析出令牌：{}", resolved.access)
        }
        broker::Outcome::Unavailable { reason, .. } => {
            assert_eq!(reason, "credential_ownership_mismatch", "{reason}");
        }
    }

    // ② 重登：拒绝，且 B 的文件一个字节都没变。
    let err = persist_login(
        &app,
        &payload(uid_a, "new-a", Some("new-a-refresh")),
        WbVariant::Cn,
    )
    .expect_err("引用不属于自己，重登必须失败");
    assert_eq!(
        err.data.as_ref().unwrap()["kind"],
        json!("conflict"),
        "{err:?}"
    );
    assert_eq!(
        std::fs::read(&b_path).expect("read b"),
        b_bytes,
        "B 的凭据被改写了"
    );

    // ③ 单份删除：拒绝（一个文件都不删）。
    let report = service::delete_authorization(&app, &a.id, ids::KIND_OWN).expect("report");
    assert!(!report.record_deleted);
    assert_eq!(report.secret_error.as_deref(), Some("conflict"));
    assert!(
        report.removed_secrets.is_empty(),
        "{:?}",
        report.removed_secrets
    );
    assert!(b_path.exists());

    // ④ 整账号删除：拒绝，账号保留。
    let out = dispatch(&app, "accounts.delete", &json!({ "id": a.id }))
        .await
        .expect("delete");
    assert_eq!(out["deleted"], json!(false), "{out}");
    assert_eq!(out["reason"], json!("secret_ownership_mismatch"), "{out}");
    assert!(
        app.with_db(|db| db.get_account(&a.id)).unwrap().is_some(),
        "拒绝删除时账号必须保留"
    );

    // ⑤ B 完全不受影响。
    assert_eq!(own_access(&app, &b.id).as_deref(), Some("access-b"));
    assert_eq!(own_refresh(&app, &b.id).as_deref(), Some("refresh-b"));
}

/// P0-2 回归：首次并发登录时，晚到的那个不许插入，也不许留下有效令牌孤儿文件。
///
/// 场景：两个进程都在"还没有 own 行"时各写了一份文件，然后先后落库。
#[test]
fn a_lost_first_insert_race_leaves_no_orphan_file() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-0000-0001-0002-0000000000c1";
    let account_id = crate::storage::account_id("workbuddy", uid);
    app.with_db(|db| {
        db.upsert_account(&crate::storage::AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.to_string(),
            provider: Some("workbuddy".to_string()),
            ..Default::default()
        })
    })
    .expect("account");

    // 竞争窗口：两边都看到"没有这一行"，各自落了一份文件。
    let first = service::write_own(
        &app,
        &account_id,
        "workbuddy",
        "first-access",
        Some("first-refresh"),
        None,
    )
    .expect("first write");
    let second = service::write_own(
        &app,
        &account_id,
        "workbuddy",
        "second-access",
        Some("second-refresh"),
        None,
    )
    .expect("second write");
    assert_eq!(secret_ids(&app).len(), 2, "竞争窗口里确实有两份文件");

    // 先到者插入成功。
    service::record_grant(
        &app,
        &account_id,
        uid,
        ids::KIND_OWN,
        None,
        Some((first.access_ref(), first.refresh_ref())),
        first.expectation(),
    )
    .expect("first wins");

    // 后到者：只允许 INSERT，所以必须被打回。
    let err = match service::record_grant(
        &app,
        &account_id,
        uid,
        ids::KIND_OWN,
        None,
        Some((second.access_ref(), second.refresh_ref())),
        second.expectation(),
    ) {
        Ok(_) => panic!("后来者不许覆盖先到者"),
        Err(err) => err,
    };
    assert!(is_stale_snapshot(&err), "{err:?}");

    // 清理只能删掉"没人引用"的那一份。
    second.discard_unreferenced(&app);

    let row = own_row(&app, &account_id);
    assert_eq!(row.access_ref, first.access_ref());
    assert_eq!(
        secret_ids(&app),
        vec![secret_id_of(&app, &account_id)],
        "不许留下未被引用的有效凭据文件"
    );
    assert_eq!(
        own_access(&app, &account_id).as_deref(),
        Some("first-access")
    );
}

/// P0-2 回归：写凭据之后、落库之前，别人把引用换掉了 —— 迟到的落库不许覆盖。
#[test]
fn a_late_login_does_not_overwrite_a_newer_reference() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-0000-0001-0002-0000000000c2";
    persist_login(
        &app,
        &payload(uid, "old-access", Some("old-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);

    // ① 我读到这一行、写好凭据（此时行还指着旧文件）。
    let late = service::write_own(
        &app,
        &acc.id,
        "workbuddy",
        "late-access",
        Some("late-refresh"),
        None,
    )
    .expect("late write");

    // ② 另一个流程把引用换成了另一份文件。
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let other = store
        .create(crate::credentials::localjson::SecretDraft::oauth(
            ids::auth_id(&acc.id, ids::KIND_OWN),
            "workbuddy",
            "other-access",
        ))
        .expect("other secret");
    let other_ref = crate::credentials::reference::local_json_ref(&other.secret_id).expect("ref");
    let row = own_row(&app, &acc.id);
    let mut moved = row.clone();
    moved.access_ref = other_ref.clone();
    moved.refresh_ref = Some(other_ref.clone());
    assert!(app
        .with_db(|db| db.replace_authorization_cas(&moved, row.version, &row.access_ref))
        .unwrap());

    // ③ 我再落库：CAS 对不上，放弃。
    let err = match service::record_grant(
        &app,
        &acc.id,
        uid,
        ids::KIND_OWN,
        None,
        Some((late.access_ref(), late.refresh_ref())),
        late.expectation(),
    ) {
        Ok(_) => panic!("旧快照不许覆盖新引用"),
        Err(err) => err,
    };
    assert!(is_stale_snapshot(&err), "{err:?}");

    late.discard_unreferenced(&app);
    let after = own_row(&app, &acc.id);
    assert_eq!(after.access_ref, other_ref, "行必须还是新的那份");
    assert_eq!(secret_ids(&app), vec![other.secret_id.clone()]);
}

/// P1-3 回归：own 行出现别的种类的引用时，**显式失败**，不静默重建、不猜。
#[test]
fn an_unknown_own_reference_is_refused_instead_of_rebuilt() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-0000-0001-0002-0000000000d1";
    let account_id = seed_legacy_own(&app, uid, "old-access", Some("old-refresh"));
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE account_authorizations
                    SET access_ref = 'magpie:v1:workbuddy#{uid}'
                  WHERE account_id = '{account_id}' AND kind = 'own'"
        ))
    })
    .expect("make it dirty");

    let err = persist_login(
        &app,
        &payload(uid, "new-access", Some("new-refresh")),
        WbVariant::Cn,
    )
    .expect_err("引用不可用就该显式失败");
    assert_eq!(
        err.data.as_ref().unwrap()["kind"],
        json!("credential_ref_unusable"),
        "{err:?}"
    );
    assert!(
        secret_ids(&app).is_empty(),
        "不许顺手新建一份 JSON 把坏引用盖过去"
    );
    // 引用本身也没被改。
    assert_eq!(own_row(&app, &account_id).storage_kind(), "magpie");
}

/// P1-3 回归：未知引用不能退到钥匙串（后端全拒，退过去就会变成 denied）。
#[test]
fn an_unknown_own_reference_does_not_fall_back_to_the_keychain() {
    let (app, _dir) = test_app(SecureStore::failing());
    let uid = "6f1c2d94-0000-0001-0002-0000000000d2";
    let account_id = crate::storage::account_id("workbuddy", uid);
    app.with_db(|db| {
        db.upsert_account(&crate::storage::AccountInput {
            platform: "workbuddy".to_string(),
            uid: uid.to_string(),
            provider: Some("workbuddy".to_string()),
            ..Default::default()
        })?;
        db.upsert_authorization(&crate::storage::AuthorizationRecord {
            id: ids::auth_id(&account_id, ids::KIND_OWN),
            account_id: account_id.clone(),
            kind: ids::KIND_OWN.to_string(),
            access_ref: "magpie:v1:workbuddy#someone".to_string(),
            refresh_ref: None,
            auth_domain: None,
            token_expires_at: None,
            status: "valid".to_string(),
            version: 0,
            key_migration_state: "ready".to_string(),
            source_key: None,
            last_used_at: None,
            last_error: None,
            created_at: crate::clock::now_rfc3339(),
            updated_at: crate::clock::now_rfc3339(),
        })
    })
    .expect("seed");

    let acc = account(&app, uid);
    let err = service::read_own_refresh(&app, &acc).expect_err("必须报错");
    assert_eq!(
        err.data.as_ref().unwrap()["kind"],
        json!("credential_ref_unusable"),
        "{err:?}"
    );
    let err = service::read_access(&app, &acc, ids::KIND_OWN).expect_err("读 access 同样不许回退");
    assert_eq!(
        err.data.as_ref().unwrap()["kind"],
        json!("conflict"),
        "own 行写着 magpie 引用是串号，不是「没凭据」：{err:?}"
    );
}

/// 上线前闸门：没确认过明文风险时，新登录仍然写钥匙串。
#[test]
fn new_logins_stay_on_the_keychain_until_the_plaintext_risk_is_confirmed() {
    let (app, _dir) = test_app_without_consent(SecureStore::memory());
    let uid = "6f1c2d94-0000-0001-0002-0000000000e1";
    let out = persist_login(
        &app,
        &payload(uid, "k-access", Some("k-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    assert_eq!(out["credentialStorage"], json!("keychain"), "{out}");
    assert_eq!(out["jsonConsentRequired"], json!(true), "{out}");

    let acc = account(&app, uid);
    assert_eq!(own_row(&app, &acc.id).storage_kind(), "keychain");
    assert!(secret_ids(&app).is_empty(), "没确认之前不许落明文");
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("k-access"));

    // 确认之后只影响**新建**的授权：已有 keychain 行不动（那是 CRED-4 的事）。
    crate::auth::consent::set_json_storage(&app, true).expect("consent");
    let uid2 = "6f1c2d94-0000-0001-0002-0000000000e2";
    let out = persist_login(
        &app,
        &payload(uid2, "j-access", Some("j-refresh")),
        WbVariant::Cn,
    )
    .expect("login 2");
    assert_eq!(out["credentialStorage"], json!("localjson"), "{out}");
    assert_eq!(out.get("jsonConsentRequired"), None);
    let acc2 = account(&app, uid2);
    assert_eq!(own_row(&app, &acc2.id).storage_kind(), "localjson");
    assert_eq!(secret_ids(&app).len(), 1);
    // 老账号仍是老的：不因为打开开关就被隐性迁移。
    assert_eq!(own_row(&app, &acc.id).storage_kind(), "keychain");
}

/// `health` 要把"是否需要告知"暴露给界面，否则没人能弹这个框。
#[test]
fn health_reports_the_plaintext_consent_state() {
    let (app, _dir) = test_app_without_consent(SecureStore::memory());
    let report = health(&app);
    assert_eq!(
        report["credentials"]["jsonStorageEnabled"],
        json!(false),
        "{report}"
    );
    assert_eq!(report["credentials"]["consentRequired"], json!(true));
    assert!(report["credentials"]["risk"]
        .as_str()
        .unwrap()
        .contains("明文"));

    crate::auth::consent::set_json_storage(&app, true).expect("consent");
    let report = health(&app);
    assert_eq!(report["credentials"]["jsonStorageEnabled"], json!(true));
    assert_eq!(report["credentials"]["consentRequired"], json!(false));
}

/// 全新 JSON 授权：登录 + 读取全程不碰钥匙串（后端把一切操作都拒了）。
#[test]
fn a_json_only_account_never_touches_the_keychain() {
    let (app, _dir) = test_app(SecureStore::failing());
    let uid = "6f1c2d94-aaaa-0001-0002-00000000000c";
    persist_login(
        &app,
        &payload(uid, "json-access", Some("json-refresh")),
        WbVariant::Cn,
    )
    .expect("登录不该被钥匙串挡住");

    let acc = account(&app, uid);
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("json-access"));
    assert_eq!(own_refresh(&app, &acc.id).as_deref(), Some("json-refresh"));
    match broker::resolve(&app, &acc).expect("resolve") {
        broker::Outcome::Resolved(resolved) => {
            assert_eq!(resolved.kind, "own");
            assert_eq!(resolved.access, "json-access");
        }
        broker::Outcome::Unavailable { reason, .. } => panic!("不该不可用：{reason}"),
    }
}

/// PR-CRED-2 的核心不变量：Magpie 导入**一个字节都不进钥匙串**。
///
/// 旧实现先写钥匙串再写库，写库失败要回滚；现在只登记 `magpie:v1:` 引用，
/// 连"回滚"这个动作都不存在了 —— 失败时天然没有半成品。
#[test]
fn magpie_import_writes_no_keychain_entry_even_when_storage_fails() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-7777-0001-0002-000000000001";
    import_logins(&app, vec![magpie_login(uid, "borrowed-1")], &[]).expect("import");
    let acc = account(&app, uid);

    assert!(
        magpie_access(&app, &acc.id).is_none(),
        "导入不得把 Magpie 令牌复制进钥匙串"
    );
    let row = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .expect("query")
        .expect("row");
    assert_eq!(row.storage_kind(), "magpie");
    assert_eq!(
        row.access_ref,
        format!("magpie:v1:workbuddy#{uid}"),
        "只登记来源引用"
    );
    assert!(row.refresh_ref.is_none(), "借用授权永远没有刷新令牌");

    app.with_db(|db| {
        db.exec_raw(
            "CREATE TRIGGER no_auth_update BEFORE UPDATE ON account_authorizations
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
    })
    .expect("install trigger");

    let err = match service::save_magpie(&app, &acc.id, uid, &format!("workbuddy#{uid}"), None) {
        Ok(_) => panic!("库里写不进去就该失败"),
        Err(err) => err,
    };
    assert_eq!(err.code, crate::rpc::E_STORAGE);
    assert!(
        magpie_access(&app, &acc.id).is_none(),
        "失败路径同样不写钥匙串"
    );
}

/// 旅行侧：选中的是自有授权但刷新不成（没有 refresh），要兜底到另一份现成的 access。
/// 这条路径不发网络请求，所以能在单测里跑。
#[test]
fn travel_resync_falls_back_to_the_other_authorization() {
    use crate::travel::scheduler::AppTokens;
    use crate::travel::service::Tokens;

    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-7777-0001-0002-000000000002";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    // 借用授权的令牌现在从 Magpie 文件现读，单测里用线程局部的登录态代替。
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    // 自有授权有 access、没有 refresh：refresh 一定失败，且不需要联网
    persist_login(&app, &payload(uid, "own-access", None), WbVariant::Cn).expect("login");
    broker::set_preferred_source(&app, "own").expect("prefer own");
    let acc = account(&app, uid);

    let tokens = AppTokens { app: &app };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let result = runtime.block_on(tokens.resync(&acc));

    assert_eq!(result.as_deref(), Ok("borrowed"), "主路失败要兜底到另一份");
    let want_ref = format!("magpie:v1:workbuddy#{uid}");
    assert_eq!(
        magpie_ref(&app, &acc.id).as_deref(),
        Some(want_ref.as_str()),
        "兜底只读，不许改另一份的引用"
    );
    assert!(
        magpie_access(&app, &acc.id).is_none(),
        "兜底也不会把令牌复制进钥匙串"
    );
    let own = app
        .with_db(|db| db.find_authorization(&acc.id, "own"))
        .unwrap()
        .unwrap();
    assert_eq!(own.status, "invalid", "自有授权缺 refresh 应标失效");
}

/// 两份都拿不出来时才报错，不会无限重试。
#[test]
fn travel_resync_reports_when_both_sources_are_unusable() {
    use crate::travel::scheduler::AppTokens;
    use crate::travel::service::Tokens;

    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-7777-0001-0002-000000000003";
    persist_login(&app, &payload(uid, "own-access", None), WbVariant::Cn).expect("login");
    broker::set_preferred_source(&app, "own").expect("prefer own");
    let acc = account(&app, uid);

    let tokens = AppTokens { app: &app };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let result = runtime.block_on(tokens.resync(&acc));
    assert!(result.is_err(), "没有可用来源就该报错，而不是返回空令牌");
}

// -----------------------------------------------------------------------
// 删除
// -----------------------------------------------------------------------

/// 动态 Magpie 关联的删除**不碰钥匙串**：我们只持有一条引用，令牌在
/// Magpie 自己的文件里。用"所有删除都失败"的后端来证明它没被调用。
#[test]
fn deleting_a_dynamic_magpie_authorization_never_touches_the_keychain() {
    let (app, _dir) = test_app(SecureStore::faulty(&[], &["workbuddy/", "legacy"]));
    let uid = "6f1c2d94-9999-0001-0002-000000000003";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    // 旧安装遗留的别名：删除动态关联时**必须保留**（回滚要用）。
    let (legacy_access_key, legacy_refresh_key) = ids::legacy_keys(uid);
    app.secure
        .set(&legacy_access_key, b"legacy-access")
        .unwrap();
    app.secure
        .set(&legacy_refresh_key, b"legacy-refresh")
        .unwrap();
    // 武装之后任何 delete 都会返回 denied：只要删除路径碰了钥匙串就会被抓到。
    app.secure.arm();

    let report = service::delete_authorization(&app, &acc.id, "magpie").expect("delete");
    assert!(report.record_deleted, "不得因钥匙串被拒而失败");
    assert!(
        report.removed_keys.is_empty() && report.failed_keys.is_empty(),
        "动态关联的删除不许碰钥匙串：{:?} / {:?}",
        report.removed_keys,
        report.failed_keys
    );
    let after = account(&app, uid);
    assert!(after.auth_mirror_auth_id.is_none(), "镜像必须清空");
    assert_eq!(after.auth_status, "unknown");
    assert!(
        app.with_db(|db| db.find_authorization(&acc.id, "magpie"))
            .unwrap()
            .is_none(),
        "授权行必须删掉"
    );
    // 为回滚保留的旧别名一个都不能少。
    assert_eq!(
        read_key(&app, &legacy_access_key).as_deref(),
        Some("legacy-access")
    );
    assert_eq!(
        read_key(&app, &legacy_refresh_key).as_deref(),
        Some("legacy-refresh")
    );
}

/// 仍是 `keychain:` 引用的旧 Magpie 行：按旧安全规则处理，删除时连旧别名一起清。
#[test]
fn deleting_a_legacy_keychain_magpie_row_still_purges_the_alias() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-000000000004";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    // 退回 v19 的形状：引用指向钥匙串。
    let key = ids::access_key(&acc.id, ids::KIND_MAGPIE);
    app.secure.set(&key, b"legacy-token").unwrap();
    app.with_db(|db| {
        db.exec_raw(&format!(
            "UPDATE account_authorizations SET access_ref = 'keychain:{key}'
                  WHERE account_id = '{}' AND kind = 'magpie'",
            acc.id
        ))
    })
    .expect("rewind");
    let (legacy_access_key, legacy_refresh_key) = ids::legacy_keys(uid);
    app.secure
        .set(&legacy_access_key, b"legacy-access")
        .unwrap();
    app.secure
        .set(&legacy_refresh_key, b"legacy-refresh")
        .unwrap();

    let report = service::delete_authorization(&app, &acc.id, "magpie").expect("delete");
    assert!(report.record_deleted, "{:?}", report.failed_keys);
    assert!(
        report
            .removed_keys
            .iter()
            .any(|key| key.ends_with("/access") && key.contains(uid)),
        "旧别名 access 应在删除范围内：{:?}",
        report.removed_keys
    );
    assert!(legacy_access(&app, uid).is_none(), "旧别名 access 必须清掉");
    assert!(
        read_key(&app, &ids::legacy_keys(uid).1).is_none(),
        "旧别名 refresh 必须清掉"
    );

    let acc = account(&app, uid);
    assert!(acc.auth_mirror_auth_id.is_none());
    assert_eq!(acc.auth_status, "unknown");
}

/// P0-5：整账号删除也要按引用算。Magpie-only 动态账号必须 **0 次**钥匙串调用。
#[tokio::test]
async fn deleting_a_magpie_only_account_never_touches_the_keychain() {
    // 后端"所有删除都失败"：只要删账号碰到钥匙串，就会进 failedKeys / 删不掉账号。
    let (app, _dir) = test_app(SecureStore::faulty(&[], &["workbuddy/", "legacy"]));
    let uid = "6f1c2d94-9999-0001-0002-000000000021";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    // 旧安装遗留的别名：动态账号删除时原样保留（回滚要用）。
    let (legacy_access_key, legacy_refresh_key) = ids::legacy_keys(uid);
    app.secure
        .set(&legacy_access_key, b"legacy-access")
        .unwrap();
    app.secure
        .set(&legacy_refresh_key, b"legacy-refresh")
        .unwrap();
    app.secure.arm();

    let out = dispatch(&app, "accounts.delete", &json!({ "id": acc.id }))
        .await
        .expect("delete");
    assert_eq!(out["deleted"], json!(true), "{out}");
    assert_eq!(out["failedKeys"], json!([]), "不许碰钥匙串：{out}");
    assert_eq!(out["orphanCredentials"], json!(false), "{out}");

    assert!(
        app.with_db(|db| db.get_account(&acc.id)).unwrap().is_none(),
        "账号必须删掉"
    );
    assert_eq!(
        read_key(&app, &legacy_access_key).as_deref(),
        Some("legacy-access"),
        "回滚用的旧别名不许删"
    );
    assert_eq!(
        read_key(&app, &legacy_refresh_key).as_deref(),
        Some("legacy-refresh")
    );
    // Magpie 原文件（这里用注入的登录态代表）不能被动过。
    assert_eq!(magpie::read_logins().expect("logins").len(), 1);
    magpie::test_set_logins(None);
}

/// 老账号（`keychain:` 自有）+ Magpie：钥匙串那一侧仍按旧规则清理干净。
#[tokio::test]
async fn deleting_a_mixed_account_purges_the_keychain_side() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-000000000022";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let account_id = seed_legacy_own(&app, uid, "own-access", Some("own-refresh"));
    let acc = account(&app, uid);
    assert_eq!(acc.id, account_id);
    let (legacy_access_key, legacy_refresh_key) = ids::legacy_keys(uid);
    app.secure
        .set(&legacy_access_key, b"legacy-access")
        .unwrap();
    app.secure
        .set(&legacy_refresh_key, b"legacy-refresh")
        .unwrap();

    let out = dispatch(&app, "accounts.delete", &json!({ "id": acc.id }))
        .await
        .expect("delete");
    assert_eq!(out["deleted"], json!(true), "{out}");
    assert_eq!(out["failedKeys"], json!([]), "{out}");

    assert!(own_access(&app, &acc.id).is_none(), "自有 access 必须清掉");
    assert!(own_refresh(&app, &acc.id).is_none());
    assert!(legacy_access(&app, uid).is_none(), "旧别名必须清掉");
    assert!(read_key(&app, &ids::legacy_keys(uid).1).is_none());
    magpie::test_set_logins(None);
}

/// 老账号：清理失败时账号必须保留，否则没有 id 可以重试。
#[tokio::test]
async fn a_failed_purge_keeps_the_account_for_retry() {
    let (app, _dir) = test_app(SecureStore::faulty(&[], &["workbuddy/"]));
    let uid = "6f1c2d94-9999-0001-0002-000000000023";
    let account_id = seed_legacy_own(&app, uid, "own-access", Some("own-refresh"));
    let acc = account(&app, uid);
    assert_eq!(acc.id, account_id);
    app.secure.arm();

    let out = dispatch(&app, "accounts.delete", &json!({ "id": acc.id }))
        .await
        .expect("delete");
    assert_eq!(out["deleted"], json!(false), "{out}");
    assert!(
        !out["failedKeys"].as_array().unwrap().is_empty(),
        "删不掉的键必须可见：{out}"
    );
    assert!(
        app.with_db(|db| db.get_account(&acc.id)).unwrap().is_some(),
        "清理失败要保留账号行，否则无法重试"
    );
}

/// P1-6：迟到的 resync 不许覆盖别人刚改好的来源键。
#[test]
fn a_late_resync_does_not_overwrite_a_newer_magpie_link() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-000000000024";
    magpie::test_set_logins(Some(vec![magpie_login(uid, "borrowed")]));
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    let acc = account(&app, uid);

    // resync 的"读快照"时刻。
    let snapshot = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .unwrap();

    // 另一个流程（重新导入 / 用户操作）先把来源键换成别的。
    let newer_key = format!("workbuddy#{uid}");
    service::save_magpie(&app, &acc.id, uid, &newer_key, None).expect("newer save");
    let newer = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .unwrap();
    assert!(newer.version > snapshot.version);

    // 迟到的写入拿着旧快照：必须被 CAS 拒掉。
    let err = match service::save_magpie_guarded(
        &app,
        &acc.id,
        uid,
        &format!("workbuddy#{uid}"),
        None,
        Some(service::MagpieGuard::of(&snapshot)),
    ) {
        Ok(_) => panic!("旧快照不许覆盖"),
        Err(err) => err,
    };
    assert!(
        is_stale_snapshot(&err),
        "要归类成 superseded：{:?}",
        err.data
    );

    let after = app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .unwrap();
    assert_eq!(after.access_ref, newer.access_ref);
    assert_eq!(after.version, newer.version);
    magpie::test_set_logins(None);
}

#[test]
fn deleting_one_authorization_leaves_the_other_intact() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-000000000001";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);

    let report = service::delete_authorization(&app, &acc.id, "own").expect("delete");
    assert!(report.record_deleted);
    assert!(own_access(&app, &acc.id).is_none());
    assert!(own_refresh(&app, &acc.id).is_none());
    let want_ref = format!("magpie:v1:workbuddy#{uid}");
    assert_eq!(
        magpie_ref(&app, &acc.id).as_deref(),
        Some(want_ref.as_str()),
        "另一份授权必须原样保留"
    );
    assert!(app
        .with_db(|db| db.find_authorization(&acc.id, "own"))
        .unwrap()
        .is_none());
    assert!(app
        .with_db(|db| db.find_authorization(&acc.id, "magpie"))
        .unwrap()
        .is_some());
}

/// 账号的授权全是 JSON / Magpie：账号删除不产生任何钥匙串键，只删自己的文件。
#[test]
fn deleting_a_json_and_magpie_account_purges_nothing() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-000000000002";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);

    let rows = app.with_db(|db| db.list_authorizations(&acc.id)).unwrap();
    let plan = service::account_purge_plan(&rows, &acc);
    assert!(
        plan.is_empty(),
        "全是动态 / JSON 引用，不该产生任何钥匙串键：{plan:?}"
    );

    // access 与 refresh 指向同一个文件，只能删一次；归属校验必须全部通过。
    let secrets = service::account_secret_plan(&app, &acc, &rows);
    assert!(secrets.refused.is_empty(), "{secrets:?}");
    assert_eq!(secrets.owned.len(), 1, "{secrets:?}");
    assert!(service::purge_secrets(&app, &secrets.owned).is_empty());
    assert!(secret_ids(&app).is_empty(), "凭据文件应当被删掉");
    assert!(own_access(&app, &acc.id).is_none());
}

/// 单份授权删除：只删自己的文件，不碰钥匙串。
#[test]
fn deleting_a_json_authorization_removes_only_the_file() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-00000000000d";
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    let secret_id = secret_id_of(&app, &acc.id);

    let report = service::delete_authorization(&app, &acc.id, ids::KIND_OWN).expect("delete");
    assert!(report.record_deleted, "{:?}", report.record_error);
    assert_eq!(report.removed_secrets, vec![secret_id.clone()]);
    assert!(
        report.removed_keys.is_empty() && report.failed_keys.is_empty(),
        "JSON 授权删除不许碰钥匙串：{:?} / {:?}",
        report.removed_keys,
        report.failed_keys
    );
    assert!(report.failed_secrets.is_empty());
    assert!(secret_ids(&app).is_empty());
    assert!(app
        .with_db(|db| db.find_authorization(&acc.id, ids::KIND_OWN))
        .unwrap()
        .is_none());
}

/// 凭据文件删不掉（被换成软链）时**不删记录**：否则再也找不到这次失败。
#[cfg(unix)]
#[test]
fn a_failed_secret_delete_keeps_the_row() {
    let (app, dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-00000000000e";
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    let store = crate::credentials::localjson::LocalJsonStore::for_paths(&app.paths);
    let path = store
        .root()
        .join(format!("{}.json", secret_id_of(&app, &acc.id)));

    // 把文件换成指向别处的软链：删除必须拒绝，而不是顺着软链删到外面。
    let outside = dir.path().join("outside.json");
    std::fs::rename(&path, &outside).expect("move");
    std::os::unix::fs::symlink(&outside, &path).expect("symlink");

    let report = service::delete_authorization(&app, &acc.id, ids::KIND_OWN).expect("report");
    assert!(!report.record_deleted, "文件没删掉就不能删记录");
    // 归属校验先失败（`unsafe`：路径是软链），所以这一步连"删除失败"都不算 —— 是拒绝。
    assert_eq!(
        report.secret_error.as_deref(),
        Some("unsafe"),
        "{:?}",
        report.secret_error
    );
    assert_eq!(
        report.failed_secrets.len(),
        0,
        "{:?}",
        report.failed_secrets
    );
    assert!(report.removed_keys.is_empty(), "JSON 授权不该碰钥匙串");
    assert!(
        app.with_db(|db| db.find_authorization(&acc.id, ids::KIND_OWN))
            .unwrap()
            .is_some(),
        "记录必须留着，否则无法重试"
    );
    assert!(outside.exists(), "拒绝删软链时也不能删到目标文件");
}

/// 记录删除失败（凭据已删）：如实报告，并且**重试能成功**。
///
/// 这是「先凭据、后记录」这个顺序必须付的代价，所以要证明它是可恢复的：
/// 文件已经没了，行还指着它 —— 读会得到 not_found，而删除本身是幂等的。
#[test]
fn a_failed_record_delete_is_reported_and_can_be_retried() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-000000000010";
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);

    app.with_db(|db| {
        db.exec_raw(
            "CREATE TRIGGER no_auth_delete BEFORE DELETE ON account_authorizations
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
    })
    .expect("install trigger");

    let report = service::delete_authorization(&app, &acc.id, ids::KIND_OWN).expect("report");
    assert!(!report.record_deleted);
    assert_eq!(
        report.record_error.as_deref(),
        Some("record_delete_failed"),
        "{:?}",
        report.record_error
    );
    assert_eq!(report.removed_secrets.len(), 1, "凭据文件这一步已经完成");
    assert!(secret_ids(&app).is_empty(), "文件确实已经删了");
    assert!(
        app.with_db(|db| db.find_authorization(&acc.id, ids::KIND_OWN))
            .unwrap()
            .is_some(),
        "行还在，所以还能重试"
    );

    // 去掉注入后重试：删一个不存在的文件是幂等的，整份删除应当完成。
    app.with_db(|db| db.exec_raw("DROP TRIGGER no_auth_delete;"))
        .expect("drop trigger");
    let retry = service::delete_authorization(&app, &acc.id, ids::KIND_OWN).expect("retry");
    assert!(retry.record_deleted, "{:?}", retry.record_error);
    assert!(retry.failed_secrets.is_empty());
    assert!(app
        .with_db(|db| db.find_authorization(&acc.id, ids::KIND_OWN))
        .unwrap()
        .is_none());
}

/// 整账号删除：记录删不掉时不谎报成功，账号保留（有 id 才能重试）。
#[tokio::test]
async fn a_failed_account_delete_is_reported_and_can_be_retried() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-000000000011";
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);

    app.with_db(|db| {
        db.exec_raw(
            "CREATE TRIGGER no_account_delete BEFORE DELETE ON accounts
                 BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )
    })
    .expect("install trigger");

    let out = dispatch(&app, "accounts.delete", &json!({ "id": acc.id }))
        .await
        .expect("delete");
    assert_eq!(out["deleted"], json!(false), "{out}");
    assert_eq!(out["recordError"], json!("account_delete_failed"), "{out}");
    assert!(
        app.with_db(|db| db.get_account(&acc.id)).unwrap().is_some(),
        "账号必须保留，否则没有 id 可以重试"
    );

    app.with_db(|db| db.exec_raw("DROP TRIGGER no_account_delete;"))
        .expect("drop trigger");
    let retry = dispatch(&app, "accounts.delete", &json!({ "id": acc.id }))
        .await
        .expect("retry");
    assert_eq!(retry["deleted"], json!(true), "{retry}");
    assert!(app.with_db(|db| db.get_account(&acc.id)).unwrap().is_none());
    assert!(secret_ids(&app).is_empty());
}

/// 整账号删除：JSON-only 账号在所有删除操作都被拒的后端上也要成功。
#[tokio::test]
async fn deleting_a_json_only_account_never_touches_the_keychain() {
    let (app, _dir) = test_app(SecureStore::faulty(&["workbuddy/"], &["workbuddy/"]));
    let uid = "6f1c2d94-9999-0001-0002-00000000000f";
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    app.secure.arm();

    let out = dispatch(&app, "accounts.delete", &json!({ "id": acc.id }))
        .await
        .expect("delete");
    assert_eq!(out["deleted"], json!(true), "{out}");
    assert_eq!(out["failedKeys"], json!([]), "不许碰钥匙串：{out}");
    assert_eq!(out["failedSecrets"], json!([]), "{out}");
    assert!(app.with_db(|db| db.get_account(&acc.id)).unwrap().is_none());
    assert!(secret_ids(&app).is_empty(), "凭据文件要跟着账号一起删");
}

/// 只剩一份时删它才算"最后一份"；删的是两份里的一份，别名要留给另一份。
#[test]
fn deleting_one_of_two_keeps_the_legacy_alias() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-000000000005";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);

    service::delete_authorization(&app, &acc.id, "own").expect("delete");
    let acc = account(&app, uid);
    assert_eq!(acc.auth_source, "magpie", "镜像应切到还活着的那份");
    assert!(
        legacy_access(&app, uid).is_none(),
        "镜像切到借用授权后也不写别名（宁可旧版本读不到，也不再碰钥匙串）"
    );
    assert!(
        read_key(&app, &ids::legacy_keys(uid).1).is_none(),
        "镜像是借用授权，别名下不能留 refresh"
    );
}

#[tokio::test]
async fn changing_the_preferred_source_resyncs_every_mirror() {
    let (app, _dir) = test_app(SecureStore::memory());
    let uid = "6f1c2d94-9999-0001-0002-000000000006";
    import_logins(&app, vec![magpie_login(uid, "borrowed")], &[]).expect("import");
    persist_login(
        &app,
        &payload(uid, "own-access", Some("own-refresh")),
        WbVariant::Cn,
    )
    .expect("login");
    let acc = account(&app, uid);
    assert_eq!(acc.auth_source, "magpie");
    assert!(
        legacy_access(&app, uid).is_none(),
        "借用授权不写旧别名：那是又一次隐性钥匙串写入"
    );

    // 走真正的 RPC 路径：settings.set 而不是直接改 broker
    let out = dispatch(
        &app,
        "settings.set",
        &json!({ "key": "auth_preferred_source", "value": "own" }),
    )
    .await
    .expect("settings.set");
    assert_eq!(out["mirrorErrors"], json!([]), "{out}");

    let acc = account(&app, uid);
    assert_eq!(acc.auth_source, "own", "镜像列要立刻跟着换");
    // 两份来源都不是 keychain 引用，所以别名两侧都不该被写。
    assert!(
        legacy_access(&app, uid).is_none(),
        "JSON 授权不写旧别名：{}",
        acc.auth_source
    );
    assert!(read_key(&app, &ids::legacy_keys(uid).1).is_none());
    // 切换之后仍然读得到自有授权（走 JSON）。
    assert_eq!(own_access(&app, &acc.id).as_deref(), Some("own-access"));
}

/// 老账号：删除失败必须报出来，且**记录保留**（否则再也找不到这次失败）。
#[test]
fn failed_key_deletion_is_reported_and_record_survives() {
    let (app, _dir) = test_app(SecureStore::faulty(&[], &["/own/access"]));
    let uid = "6f1c2d94-9999-0001-0002-000000000003";
    let account_id = seed_legacy_own(&app, uid, "own-access", Some("own-refresh"));
    app.secure.arm();

    let report = service::delete_authorization(&app, &account_id, ids::KIND_OWN).expect("report");
    assert_eq!(report.failed_keys.len(), 1);
    assert!(!report.record_deleted, "删不干净就不能声称删除成功");
    assert!(
        app.with_db(|db| db.find_authorization(&account_id, ids::KIND_OWN))
            .unwrap()
            .is_some(),
        "记录必须留着，否则再也找不到这次失败"
    );
}

// -----------------------------------------------------------------------
// 旧快照与状态守卫
// -----------------------------------------------------------------------

#[test]
fn stale_snapshot_covers_both_rejection_kinds() {
    assert!(super::is_stale_snapshot(&superseded("epoch_changed")));
    assert!(super::is_stale_snapshot(
        &RpcError::new(crate::rpc::E_AUTH, "own").with_data(json!({ "kind": "not_magpie_source" }))
    ));
    assert!(!super::is_stale_snapshot(
        &RpcError::new(crate::rpc::E_AUTH, "magpie 令牌过期")
            .with_data(json!({ "kind": "magpie_token_expired" }))
    ));
    assert!(!super::is_stale_snapshot(&RpcError::internal("boom")));
}

// -----------------------------------------------------------------------
// RPC 收口
// -----------------------------------------------------------------------

#[test]
fn secure_key_allowlist_only_accepts_the_three_shapes() {
    for key in [
        "workbuddy/0123456789abcdef/own/access",
        "workbuddy/0123456789abcdef/own/refresh",
        "workbuddy/0123456789abcdef/magpie/access",
    ] {
        assert!(super::validate_secure_key(key).is_ok(), "{key}");
    }
    for key in [
        // 不存在这个键
        "workbuddy/0123456789abcdef/magpie/refresh",
        // 旧 UID 格式：只给 Rust 内部迁移用
        "workbuddy/6f1c2d94-1111-2222/access",
        // 位数不对
        "workbuddy/abc/own/access",
        // 探针键
        "__probe__1234",
        "anything/else",
    ] {
        assert!(super::validate_secure_key(key).is_err(), "{key} 不该被接受");
    }
}

#[test]
fn secure_rpc_is_closed_unless_the_switch_is_on() {
    // CI 与本地都不该设这个变量；真设了就把断言让开，避免误报。
    if std::env::var("AICC_ALLOW_SECURE_RPC").as_deref() == Ok("1") {
        return;
    }
    let err = super::require_secure_rpc("secure.set").expect_err("默认必须关闭");
    assert_eq!(
        err.code,
        crate::rpc::METHOD_NOT_FOUND,
        "默认应当 method_not_found"
    );
}

#[test]
fn only_the_known_setting_key_is_accepted() {
    assert!(super::require_setting_key(broker::PREFERRED_SETTING).is_ok());
    assert!(super::require_setting_key("auth_something_else").is_err());
}
