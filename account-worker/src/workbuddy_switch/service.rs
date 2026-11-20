//! WorkBuddy 客户端账号切换的编排。
//!
//! ## 一句话
//!
//! 备份 → 关客户端 → 写认证 → 起客户端 → **回读确认客户端真的用上了**；
//! 任何一步失败都按备份逐字节还原，并把结果如实报给调用方。
//!
//! ## 为什么 AICC 要自己做回滚
//!
//! 上游 `switch_account` 的顺序是「关进程 → 写认证 → 启动」，写认证或启动失败时
//! **不会**把文件改回去。桌面端用户在半夜被切到一个起不来的账号，第二天打开
//! 客户端就是"未登录"——这不能接受。所以这里每一步都落 journal，失败立刻用快照
//! 还原，并且还原之后**重新启动客户端**（不然用户面对的是一个被关掉的 App）。
//!
//! ## "当前登录账号"从哪来
//!
//! 只从客户端的 `workbuddy-desktop.info` 读。AICC 自己的账号库里"选中了谁"、
//! 查询接口优先用哪份授权，都**不能**代替这个状态。所以 [`status`] 的 `live.uid`
//! 是唯一可信来源，界面上那句"当前登录：X"必须用它。
//!
//! ## 已知边界（写进 `docs/WORKBUDDY_CLIENT_SWITCH.md`）
//!
//! - **只有同时持有 access + refresh 的账号可切换**。Magpie 来源只有 access，
//!   写进去之后客户端没有可用来续期的凭据，过期即掉线 —— 所以先禁用，等实测。
//! - 只支持国内版。国际版的认证文件与客户端行为都没有实测证据。
//! - 写出去的是**明文令牌**（AICC 自有 OAuth 拿到的就是明文），客户端启动后会
//!   自己重新封装。是否被接受由 [`verify`] 回读判断，不靠假设。

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use wb_switch_core::modules::auth_file;

use crate::app::App;
use crate::auth::service as auth_service;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::wb_switch::{
    NewSwitchJournal, SwitchJournal, PHASE_FAILED, PHASE_LAUNCHED, PHASE_RESTORED,
    PHASE_UNCONFIRMED, PHASE_VERIFIED, PHASE_WRITTEN,
};
use crate::storage::AccountRecord;

use super::client::{self, ClientControl};

/// 关客户端 / 恢复时各自允许的等待。
const CLOSE_TIMEOUT_SECS: i64 = 20;
const ROLLBACK_CLOSE_SECS: i64 = 15;

/// 切换目标不可用的稳定分类。界面按这个显示原因，不要解析中文。
pub mod reason {
    pub const OK: &str = "ok";
    /// 客户端当前登录的就是它。
    pub const LIVE: &str = "live";
    pub const UNSUPPORTED_PROVIDER: &str = "unsupported_provider";
    pub const CLIENT_NOT_INSTALLED: &str = "client_not_installed";
    pub const NO_CREDENTIAL: &str = "no_credential";
    /// 自有授权里没有 refresh。
    pub const MISSING_REFRESH: &str = "missing_refresh";
    /// 只有 Magpie 那份授权，天然没有 refresh。
    pub const MAGPIE_NO_REFRESH: &str = "magpie_no_refresh";
}

/// 路径注入点：生产用系统真实路径，测试用临时目录。
#[derive(Debug, Clone)]
pub struct SwitchEnv {
    /// 客户端认证文件（`workbuddy-desktop.info`）。
    pub auth_path: PathBuf,
    /// 切换快照目录（AICC 自己的数据目录下）。
    pub snapshot_dir: PathBuf,
}

impl SwitchEnv {
    pub fn system(app: &App) -> Self {
        Self {
            auth_path: wb_switch_core::modules::variant::WbVariant::Cn.auth_file_path(),
            snapshot_dir: app.paths.data_dir.join("workbuddy-switch"),
        }
    }
}

pub struct SwitchRequest {
    pub account_id: String,
    pub restart: bool,
    pub observe_activity: bool,
}

/// 写进客户端的材料。`access` / `refresh` 用 `Value`：AICC 目前只会拿到明文串，
/// 但上游也支持原样回写信封，这里不为将来设限。
pub struct TargetSession {
    pub uid: String,
    pub nickname: Value,
    pub access: Value,
    pub refresh: Value,
    pub domain: String,
    pub expires_at_ms: Option<i64>,
}

/// 这个账号能不能切，以及为什么不能。纯读库，**不读令牌**（读令牌会弹钥匙串授权）。
pub fn plan(
    app: &App,
    account: &AccountRecord,
    live_uid: Option<&str>,
    installed: bool,
) -> (bool, &'static str) {
    if account.platform != "workbuddy" || account.provider != "workbuddy" {
        return (false, reason::UNSUPPORTED_PROVIDER);
    }
    if live_uid == Some(account.uid.as_str()) {
        return (false, reason::LIVE);
    }
    if !installed {
        return (false, reason::CLIENT_NOT_INSTALLED);
    }
    let rows = match app.with_db(|db| db.list_authorizations(&account.id)) {
        Ok(rows) => rows,
        Err(_) => return (false, reason::NO_CREDENTIAL),
    };
    let own = rows.iter().find(|row| row.kind == "own");
    match own {
        // 有自有授权但缺 refresh：登录材料不完整，写进去只能撑到 access 过期。
        Some(row) if row.refresh_ref.is_none() => (false, reason::MISSING_REFRESH),
        Some(_) => (true, reason::OK),
        None if rows.iter().any(|row| row.kind == "magpie") => (false, reason::MAGPIE_NO_REFRESH),
        None => (false, reason::NO_CREDENTIAL),
    }
}

/// 整体状态：客户端、当前登录、每个账号能不能切、有没有未完成的切换。
pub fn status(app: &App, control: &dyn ClientControl, env: &SwitchEnv) -> RpcResult<Value> {
    let installed = control.installed();
    let running = installed.then(|| control.is_running());
    let live = client::read_live(&env.auth_path);
    let accounts = app.with_db(|db| db.list_accounts(&Default::default()))?;

    let mut candidates: Vec<Value> = Vec::new();
    for account in &accounts {
        if account.platform != "workbuddy" {
            continue;
        }
        let (switchable, why) = plan(app, account, live.uid.as_deref(), installed);
        candidates.push(json!({
            "accountId": account.id,
            "uid": account.uid,
            "nickname": account.nickname,
            "provider": account.provider,
            "switchable": switchable,
            "reason": why,
        }));
    }

    let open = app.with_db(|db| db.open_wb_switch())?;
    let latest = app.with_db(|db| db.latest_wb_switch())?;
    let recent = app.with_db(|db| db.wb_switch_journals(5))?;

    Ok(json!({
        "ok": true,
        "variant": control.variant().as_str(),
        "client": {
            "installed": installed,
            "appPath": control.app_path().to_string_lossy(),
            "running": running,
            "authFile": env.auth_path.to_string_lossy(),
            "authFileExists": live.exists,
            "logoutMarker": live.logout_marker,
        },
        "live": {
            "uid": live.uid,
            "loggedIn": live.logged_in(),
            // 令牌形态只是诊断信息：`plaintext` 说明上一次切换写的是明文，
            // 客户端还没重新封装。
            "tokenForm": live.token_form,
            "tokenFingerprint": live.token_fingerprint,
            "accountsInFile": live.accounts_len,
            "sizeBytes": live.size,
            "modifiedAtMs": live.modified_ms,
            "sha256": live.sha256,
            "error": live.parse_error,
        },
        "candidates": candidates,
        "pendingSwitch": open,
        "lastSwitch": latest,
        "recent": recent,
    }))
}

/// 切换。成功返回 `{ok:true, changed:true, verified:…, journalId:…, activityBefore/After}`；
/// 失败返回错误，`data` 里带 `restored` —— 界面必须把这两个状态分开显示。
pub async fn switch(
    app: &App,
    request: &SwitchRequest,
    control: &dyn ClientControl,
    env: &SwitchEnv,
) -> RpcResult<Value> {
    if !request.restart {
        return Err(RpcError::invalid_params(
            "账号切换必须重启 WorkBuddy：客户端运行时会用内存里的登录态覆盖认证文件",
        ));
    }

    let account = app
        .with_db(|db| db.get_account(&request.account_id))?
        .ok_or_else(|| RpcError::not_found(format!("账号不存在：{}", request.account_id)))?;
    if account.platform != "workbuddy" {
        return Err(RpcError::invalid_params("目前只支持 WorkBuddy 账号切换"));
    }
    if account.provider != "workbuddy" {
        return Err(RpcError::invalid_params(
            "WorkBuddy AI（国际版）的客户端切换尚未接通，AICC 没有改动任何文件",
        ));
    }
    if !control.installed() {
        return Err(
            RpcError::not_found("未找到 WorkBuddy 客户端，无法切换").with_data(json!({
                "kind": reason::CLIENT_NOT_INSTALLED,
            })),
        );
    }

    let rows = app.with_db(|db| db.list_authorizations(&account.id))?;
    let own = rows.iter().find(|row| row.kind == "own").ok_or_else(|| {
        let why = plan(app, &account, None, true).1;
        RpcError::new(
            crate::rpc::E_AUTH,
            "该账号没有 AICC 自有授权，不能用于客户端切换",
        )
        .with_data(json!({ "kind": why }))
    })?;
    if own.refresh_ref.is_none() {
        return Err(RpcError::new(
            crate::rpc::E_AUTH,
            "该授权只有访问令牌、没有刷新令牌，切过去之后无法续期；请用浏览器登录重新授权",
        )
        .with_data(json!({ "kind": reason::MISSING_REFRESH })));
    }

    // 未完成的切换必须先处理：否则"当前登录"到底该还原到哪一份就说不清了。
    if app.with_db(|db| db.open_wb_switch())?.is_some() {
        return Err(RpcError::new(
            crate::rpc::E_SWITCH,
            "已有一条未完成的账号切换，先恢复或等它结束",
        )
        .with_data(json!({ "kind": "switch_busy" })));
    }

    let before = client::read_live(&env.auth_path);
    if before.uid.as_deref() == Some(account.uid.as_str()) && !before.logout_marker {
        return Ok(json!({
            "ok": true,
            "changed": false,
            "alreadyLive": true,
            "target": { "accountId": account.id, "uid": account.uid, "nickname": account.nickname },
        }));
    }

    let target = target_session(app, &account, before.json.as_ref())?;

    // 活跃取证放在关客户端之前：它要联网，客户端这时还能正常用。
    let activity_before = if request.observe_activity {
        Some(activity_snapshot(app, &account.id).await)
    } else {
        None
    };

    let outcome = apply(app, &account, &target, control, env)?;

    let activity_after = if request.observe_activity {
        Some(activity_snapshot(app, &account.id).await)
    } else {
        None
    };
    if activity_before.is_some() || activity_after.is_some() {
        let _ = app.with_db(|db| {
            db.set_wb_switch_activity(
                &outcome.journal_id,
                activity_before.as_ref(),
                activity_after.as_ref(),
            )
        });
    }

    log_note(&format!(
        "WorkBuddy 账号切换：{} → {}，phase={}",
        outcome.previous_uid.as_deref().unwrap_or("—"),
        account.uid,
        outcome.phase
    ));

    Ok(json!({
        "ok": true,
        "changed": true,
        "journalId": outcome.journal_id,
        "phase": outcome.phase,
        "verified": outcome.verified,
        "verifyDetail": outcome.verify_detail,
        "clientRunning": outcome.running,
        "clientRewroteFile": outcome.client_rewrote,
        "target": {
            "accountId": account.id,
            "uid": account.uid,
            "nickname": account.nickname,
        },
        "previousUid": outcome.previous_uid,
        "snapshotSha256": outcome.snapshot_sha,
        "activityBefore": activity_before,
        "activityAfter": activity_after,
    }))
}

/// 切换里"动文件 + 动进程"的那一半。**不联网**，因此可以被完整单测。
#[derive(Debug)]
struct ApplyOutcome {
    journal_id: String,
    phase: String,
    verified: bool,
    verify_detail: Option<String>,
    running: bool,
    client_rewrote: bool,
    previous_uid: Option<String>,
    snapshot_sha: String,
}

fn apply(
    app: &App,
    account: &AccountRecord,
    target: &TargetSession,
    control: &dyn ClientControl,
    env: &SwitchEnv,
) -> RpcResult<ApplyOutcome> {
    let running_before = control.is_running();
    if running_before {
        control.close(CLOSE_TIMEOUT_SECS).map_err(|err| {
            RpcError::internal(format!("关闭 WorkBuddy 失败，未改动任何认证文件：{err}"))
                .with_data(json!({ "kind": "close_failed" }))
        })?;
    }

    // 关闭之后再拍快照：客户端退出时可能把刚刷新过的令牌写回文件。
    let snapshot = match std::fs::read(&env.auth_path) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        Ok(_) => {
            if running_before {
                let _ = control.launch();
            }
            return Err(RpcError::internal(
                "客户端认证文件是空的，没有可恢复的登录状态；请先在客户端里登录一次",
            )
            .with_data(json!({ "kind": "no_snapshot" })));
        }
        Err(err) => {
            if running_before {
                let _ = control.launch();
            }
            return Err(
                RpcError::internal(format!("读取客户端认证文件失败，未做切换：{err}"))
                    .with_data(json!({ "kind": "no_snapshot" })),
            );
        }
    };

    // 快照内容就是"要恢复成什么样"，所以后面一切都以它为准（不再用关客户端之前的读法）。
    let snapshot_root: Option<Value> = serde_json::from_slice(&snapshot).ok();
    let Some(snapshot_root) = snapshot_root else {
        // 文件不是我们能读懂的结构：宁可不动，也不要在上面拼一个"看起来合理"的登录态。
        if running_before {
            let _ = control.launch();
        }
        return Err(RpcError::internal(
            "客户端认证文件结构未识别，未做切换；请先在客户端里登录一次再试",
        )
        .with_data(json!({ "kind": "unrecognized_auth_file" })));
    };
    let previous_uid = client::uid_of(&snapshot_root);
    let previous_marker = client::logout_marker_path(&env.auth_path).exists();

    let journal_id = uuid::Uuid::new_v4().to_string();
    let snapshot_path = env.snapshot_dir.join(format!("{journal_id}.json"));
    let snapshot_sha = client::sha256_hex(&snapshot);
    if let Err(err) = client::write_private(&snapshot_path, &snapshot) {
        if running_before {
            let _ = control.launch();
        }
        return Err(RpcError::internal(format!(
            "保存切换快照失败，未做切换：{err}"
        )));
    }

    let opened = app.with_db(|db| {
        db.begin_wb_switch(&NewSwitchJournal {
            id: &journal_id,
            target_account_id: &account.id,
            target_uid: &account.uid,
            previous_uid: previous_uid.as_deref(),
            previous_running: running_before,
            previous_marker,
            snapshot_path: &snapshot_path.to_string_lossy(),
            snapshot_sha256: &snapshot_sha,
            token_form: Some("plaintext"),
        })
    });
    if let Err(err) = opened {
        let _ = std::fs::remove_file(&snapshot_path);
        if running_before {
            let _ = control.launch();
        }
        return Err(err);
    }

    let session = build_session(Some(&snapshot_root), target);
    let text = match serde_json::to_string_pretty(&session) {
        Ok(text) => text,
        Err(err) => {
            return Err(finish_without_write(
                app,
                control,
                &journal_id,
                running_before,
                format!("生成认证内容失败，未改动客户端：{err}"),
            ))
        }
    };
    if let Err(err) = client::write_private(&env.auth_path, text.as_bytes()) {
        return Err(finish_without_write(
            app,
            control,
            &journal_id,
            running_before,
            format!("写入客户端认证文件失败，登录态未变：{err}"),
        ));
    }
    app.with_db(|db| db.set_wb_switch_phase(&journal_id, PHASE_WRITTEN, None))?;

    if let Err(err) = control.launch() {
        let restored = rollback(
            app,
            control,
            env,
            &journal_id,
            running_before,
            &snapshot,
            previous_marker,
            previous_uid.as_deref(),
            &format!("启动 WorkBuddy 失败：{err}"),
        );
        return Err(switch_failed(
            format!("启动 WorkBuddy 失败：{err}"),
            restored,
            previous_uid.as_deref(),
            &journal_id,
        ));
    }
    app.with_db(|db| db.set_wb_switch_phase(&journal_id, PHASE_LAUNCHED, None))?;

    let verdict = verify(control, &env.auth_path, &account.uid, &text);
    if let Some(failure) = verdict.rejected {
        let restored = rollback(
            app,
            control,
            env,
            &journal_id,
            running_before,
            &snapshot,
            previous_marker,
            previous_uid.as_deref(),
            &failure,
        );
        return Err(switch_failed(
            failure,
            restored,
            previous_uid.as_deref(),
            &journal_id,
        ));
    }

    let phase = if verdict.confirmed {
        PHASE_VERIFIED
    } else {
        PHASE_UNCONFIRMED
    };
    app.with_db(|db| db.close_wb_switch(&journal_id, phase, verdict.detail.as_deref()))?;

    Ok(ApplyOutcome {
        journal_id,
        phase: phase.to_string(),
        verified: verdict.confirmed,
        verify_detail: verdict.detail,
        running: verdict.running,
        client_rewrote: verdict.client_rewrote,
        previous_uid,
        snapshot_sha,
    })
}

/// 恢复：把某一轮切换之前的登录态放回去（自动回滚也是走这套）。
///
/// 不带 `journal_id` 时按"未完成 → 最近一次成功的切换"顺序挑一条。
pub fn restore(
    app: &App,
    journal_id: Option<&str>,
    control: &dyn ClientControl,
    env: &SwitchEnv,
) -> RpcResult<Value> {
    let journal = match journal_id {
        Some(id) => app
            .with_db(|db| db.wb_switch_journal(id))?
            .ok_or_else(|| RpcError::not_found(format!("切换记录不存在：{id}")))?,
        None => match app.with_db(|db| db.open_wb_switch())? {
            Some(open) => open,
            None => {
                let recent = app.with_db(|db| db.wb_switch_journals(10))?;
                match recent.into_iter().find(|row| {
                    row.previous_uid.is_some()
                        && matches!(
                            row.phase.as_str(),
                            PHASE_VERIFIED | PHASE_UNCONFIRMED | PHASE_FAILED
                        )
                }) {
                    Some(row) => row,
                    None => {
                        return Ok(json!({
                            "ok": true,
                            "restored": false,
                            "reason": "nothing_to_restore",
                            "message": "没有可恢复的切换记录",
                        }))
                    }
                }
            }
        },
    };

    let previous_uid = journal.previous_uid.clone();
    let snapshot = read_snapshot(&journal)?;

    let running = control.is_running();
    if running {
        control.close(ROLLBACK_CLOSE_SECS).map_err(|err| {
            RpcError::internal(format!("关闭 WorkBuddy 失败，未恢复任何文件：{err}"))
                .with_data(json!({ "kind": "close_failed" }))
        })?;
    }
    client::write_private(&env.auth_path, &snapshot).map_err(|err| {
        RpcError::internal(format!("写回切换前的认证文件失败：{err}"))
            .with_data(json!({ "kind": "restore_write_failed", "journalId": journal.id }))
    })?;
    apply_marker(&env.auth_path, journal.previous_marker);
    let launched = if journal.previous_running {
        control.launch().is_ok()
    } else {
        true
    };

    let after = client::read_live(&env.auth_path);
    let ok = after.uid == previous_uid && !after.logout_marker;
    let detail = if ok {
        "已恢复到切换前的登录态".to_string()
    } else {
        format!(
            "已写回快照，但客户端当前登录是 {}（目标 {}）",
            after.uid.as_deref().unwrap_or("读不到"),
            previous_uid.as_deref().unwrap_or("—")
        )
    };
    app.with_db(|db| {
        db.close_wb_switch(
            &journal.id,
            if ok { PHASE_RESTORED } else { PHASE_FAILED },
            Some(&detail),
        )
    })?;
    log_note(&format!(
        "WorkBuddy 账号恢复：journal={}，uid={}，ok={ok}",
        journal.id,
        previous_uid.as_deref().unwrap_or("—")
    ));

    Ok(json!({
        "ok": true,
        "restored": ok,
        "journalId": journal.id,
        "uid": previous_uid,
        "clientRunning": control.is_running(),
        "launched": launched,
        "verified": after.uid == previous_uid,
        "detail": detail,
    }))
}

// ---------------------------------------------------------------------------
// 内部
// ---------------------------------------------------------------------------

struct Verdict {
    confirmed: bool,
    running: bool,
    client_rewrote: bool,
    /// 客户端明确拒绝了这个登录态（必须是目标账号之外的结果）。
    rejected: Option<String>,
    detail: Option<String>,
}

/// 回读确认。**只有三种结局**：确认、明确被拒绝、说不清。
///
/// - 明确被拒绝（UID 变了 / 出现退出标记）→ 调用方回滚。
/// - 说不清（客户端没起来）→ 不回滚，落 `unconfirmed` 让用户决定；
///   把一次大概率的成功回滚掉，比"晚点再确认"更糟。
fn verify(
    control: &dyn ClientControl,
    auth_path: &Path,
    target_uid: &str,
    written: &str,
) -> Verdict {
    let written_sha = client::sha256_hex(written.as_bytes());
    let (settle_ms, budget_ms) = control.verify_window();
    let started = std::time::Instant::now();
    let mut saw_running = false;
    let mut client_rewrote = false;

    loop {
        let running = control.is_running();
        saw_running |= running;
        let live = client::read_live(auth_path);
        if live.sha256.as_deref() != Some(written_sha.as_str()) {
            client_rewrote = true;
        }
        if live.logout_marker {
            return Verdict {
                confirmed: false,
                running,
                client_rewrote,
                rejected: Some("客户端写下了退出标记，说明它没有接受这次登录".to_string()),
                detail: None,
            };
        }
        if live.uid.as_deref() != Some(target_uid) {
            return Verdict {
                confirmed: false,
                running,
                client_rewrote,
                rejected: Some(format!(
                    "客户端当前登录变成了 {}，不是目标账号",
                    live.uid.as_deref().unwrap_or("读不到")
                )),
                detail: None,
            };
        }
        let elapsed = started.elapsed().as_millis() as u64;
        if saw_running && elapsed >= settle_ms {
            return Verdict {
                confirmed: true,
                running,
                client_rewrote,
                rejected: None,
                detail: Some(if client_rewrote {
                    "客户端已启动并重写了认证文件（说明它接受了这次登录）".to_string()
                } else {
                    "客户端已启动，认证文件与写入内容一致".to_string()
                }),
            };
        }
        if elapsed >= budget_ms {
            return Verdict {
                confirmed: false,
                running,
                client_rewrote,
                rejected: None,
                detail: Some(format!(
                    "认证文件已是目标账号，但 {} 秒内没有确认到客户端进程；请手动打开客户端后点「刷新状态」",
                    budget_ms.div_ceil(1000)
                )),
            };
        }
        control.sleep_ms(400);
    }
}

/// 失败回滚：把快照逐字节写回去，并按原状恢复退出标记与客户端进程。
fn rollback(
    app: &App,
    control: &dyn ClientControl,
    env: &SwitchEnv,
    journal_id: &str,
    previous_running: bool,
    snapshot: &[u8],
    previous_marker: bool,
    previous_uid: Option<&str>,
    cause: &str,
) -> bool {
    let _ = control.close(ROLLBACK_CLOSE_SECS);
    let written = client::write_private(&env.auth_path, snapshot).is_ok();
    apply_marker(&env.auth_path, previous_marker);
    if previous_running {
        let _ = control.launch();
    }
    let after = client::read_live(&env.auth_path);
    let ok = written && after.uid.as_deref() == previous_uid && !after.logout_marker;
    let detail = if ok {
        format!("{cause}；已恢复到切换前的登录态")
    } else {
        format!("{cause}；恢复失败，请手动在客户端里登录")
    };
    let _ = app.with_db(|db| {
        db.close_wb_switch(
            journal_id,
            if ok { PHASE_RESTORED } else { PHASE_FAILED },
            Some(&detail),
        )
    });
    log_note(&format!("WorkBuddy 账号切换失败：{detail}"));
    ok
}

/// 写入之前就失败：客户端已经被关掉，但认证文件一个字节都没动。
fn finish_without_write(
    app: &App,
    control: &dyn ClientControl,
    journal_id: &str,
    previous_running: bool,
    detail: String,
) -> RpcError {
    if previous_running {
        let _ = control.launch();
    }
    let _ = app.with_db(|db| db.close_wb_switch(journal_id, PHASE_RESTORED, Some(&detail)));
    RpcError::internal(detail).with_data(json!({
        "kind": "switch_failed",
        "restored": true,
        "journalId": journal_id,
    }))
}

fn switch_failed(
    message: String,
    restored: bool,
    previous_uid: Option<&str>,
    journal_id: &str,
) -> RpcError {
    RpcError::internal(message).with_data(json!({
        "kind": "switch_failed",
        "restored": restored,
        "previousUid": previous_uid,
        "journalId": journal_id,
    }))
}

/// 快照完整性：文件在、sha 对得上，才允许覆盖客户端登录态。
fn read_snapshot(journal: &SwitchJournal) -> RpcResult<Vec<u8>> {
    let path = PathBuf::from(&journal.snapshot_path);
    let bytes = std::fs::read(&path).map_err(|err| {
        RpcError::internal(format!("读取切换快照失败：{err}"))
            .with_data(json!({ "kind": "snapshot_missing", "journalId": journal.id }))
    })?;
    let sha = client::sha256_hex(&bytes);
    if sha != journal.snapshot_sha256 {
        return Err(RpcError::internal(
            "切换快照与记录不一致（可能被改动过），拒绝用它覆盖客户端登录态",
        )
        .with_data(json!({ "kind": "snapshot_corrupt", "journalId": journal.id })));
    }
    Ok(bytes)
}

fn apply_marker(auth_path: &Path, present: bool) {
    let marker = client::logout_marker_path(auth_path);
    if present {
        let _ = std::fs::write(&marker, "logged-out");
    } else {
        match std::fs::remove_file(&marker) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => eprintln!("[switch] 清理退出标记失败：{err}"),
        }
    }
}

/// 取出目标账号的登录材料。只走 `own` 授权：Magpie 那份天然没有 refresh。
fn target_session(
    app: &App,
    account: &AccountRecord,
    existing: Option<&Value>,
) -> RpcResult<TargetSession> {
    let access = auth_service::read_access(app, account, "own")?
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            RpcError::new(crate::rpc::E_AUTH, "读取自有授权失败：没有访问令牌")
                .with_data(json!({ "kind": reason::NO_CREDENTIAL }))
        })?;
    let refresh = auth_service::read_own_refresh(app, account)?
        .filter(|token| !token.trim().is_empty())
        .ok_or_else(|| {
            RpcError::new(crate::rpc::E_AUTH, "读取自有授权失败：没有刷新令牌")
                .with_data(json!({ "kind": reason::MISSING_REFRESH }))
        })?;

    // 昵称优先沿用客户端自己那份（可能是信封，解开才有值；不要用明文覆盖）。
    let nickname = existing
        .and_then(|root| client::find_account_entry(root, &account.uid))
        .and_then(|entry| entry.get("nickname").cloned())
        .filter(|value| !value.is_null())
        .unwrap_or_else(|| Value::String(account.nickname.clone()));

    let domain = account
        .auth_domain
        .clone()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            existing
                .and_then(|root| root.get("auth")?.get("domain")?.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| {
            wb_switch_core::modules::variant::WbVariant::Cn
                .codebuddy_domain()
                .to_string()
        });

    let expires_at_ms = account
        .token_expires_at
        .as_deref()
        .and_then(auth_service::parse_rfc3339)
        .map(|time| time.timestamp_millis());

    Ok(TargetSession {
        uid: account.uid.clone(),
        nickname,
        access: Value::String(access),
        refresh: Value::String(refresh),
        domain,
        expires_at_ms,
    })
}

/// 生成写进客户端的完整 JSON。**沿用客户端已有的结构**，只替换认证与当前账号。
///
/// 复用的意义：客户端里的账号对象带着 `uin` / `avatarUrl` / `uniqueUserId` /
/// 昵称信封等字段，凭 AICC 库里那点信息重建只会得到一个缺字段的账号。
pub fn build_session(existing: Option<&Value>, target: &TargetSession) -> Value {
    let mut root: Map<String, Value> = existing
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    let existing_list = existing.map(client::account_list).unwrap_or_default();
    let entry = existing_list
        .iter()
        .find(|item| item.get("uid").and_then(Value::as_str) == Some(target.uid.as_str()))
        .and_then(Value::as_object)
        .cloned();

    let mut profile = entry.unwrap_or_default();
    profile.insert("uid".to_string(), Value::String(target.uid.clone()));
    profile.insert("nickname".to_string(), target.nickname.clone());
    profile.insert("lastLogin".to_string(), Value::Bool(true));

    let mut acc = json!({
        "uid": target.uid,
        "nickname": target.nickname,
        "access_token": target.access,
        "refresh_token": target.refresh,
        "token_type": "Bearer",
        "domain": target.domain,
        "profile_raw": Value::Object(profile),
    });
    if let Some(expires_at) = target.expires_at_ms {
        acc["expiresAt"] = json!(expires_at);
    }

    let mut account_obj = auth_file::build_account_obj(&acc);
    if let Some(map) = account_obj.as_object_mut() {
        // `build_account_obj` 只在缺失时补 `lastLogin`：旧列表项是 false 时要盖掉。
        map.insert("lastLogin".to_string(), Value::Bool(true));
    }
    let auth_obj = auth_file::build_auth_obj(&acc);

    let mut all: Vec<Value> = existing_list
        .into_iter()
        .filter(|item| item.get("uid").and_then(Value::as_str) != Some(target.uid.as_str()))
        .collect();
    all.push(account_obj.clone());

    root.insert("account".to_string(), account_obj);
    root.insert("auth".to_string(), auth_obj);
    root.insert("accounts".to_string(), Value::Array(all.clone()));
    root.insert("allAccounts".to_string(), Value::Array(all));
    Value::Object(root)
}

/// 目标账号的服务端活跃快照（只读）。失败只记一句原因，绝不影响切换本身。
pub async fn activity_snapshot(app: &App, account_id: &str) -> Value {
    match crate::growth::activity::fleet(app, &[account_id.to_string()]).await {
        Ok(fleet) => match fleet.accounts.first() {
            Some(account) => json!({
                "state": account.state,
                "diagnosis": account.diagnosis.code,
                "serverDate": account.server_today,
                "todayActive": account.today_active,
                "todayScore": account.today_score,
                "streakDays": account.streak_days,
                "nextTier": account.next_tier,
                "nextTierRemaining": account.next_tier_remaining,
                "fetchedAt": account.fetched_at,
            }),
            None => json!({ "state": "no_account", "error": "没有可查询的国内版账号" }),
        },
        Err(err) => json!({ "state": "probe_failed", "error": err.message }),
    }
}

fn log_note(message: &str) {
    eprintln!("[aicc-account-worker] {}", crate::rpc::sanitize(message));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Db;

    #[test]
    fn build_session_reuses_the_clients_own_account_object() {
        let existing = json!({
            "auth": {"accessToken": {"$wbEncrypted": 1, "envelope": "x"}, "domain": "www.workbuddy.cn"},
            "account": {"uid": "uid-a", "nickname": {"$wbEncrypted": 1, "envelope": "a"}},
            "allAccounts": [{
                "uid": "uid-b",
                "nickname": {"$wbEncrypted": 1, "envelope": "b"},
                "uin": "10001",
                "uniqueUserId": "unique-b",
                "lastLogin": false,
            }],
        });
        let target = TargetSession {
            uid: "uid-b".to_string(),
            nickname: existing["allAccounts"][0]["nickname"].clone(),
            access: json!("AT-B"),
            refresh: json!("RT-B"),
            domain: "www.codebuddy.cn".to_string(),
            expires_at_ms: Some(1_800_000_000_000),
        };

        let session = build_session(Some(&existing), &target);
        assert_eq!(session["account"]["uid"], "uid-b");
        assert_eq!(session["account"]["uin"], "10001", "客户端已有字段必须保留");
        assert_eq!(session["account"]["uniqueUserId"], "unique-b");
        assert_eq!(session["account"]["lastLogin"], true, "旧值 false 要被盖掉");
        assert!(
            session["account"]["nickname"].is_object(),
            "昵称信封不能被明文覆盖"
        );
        assert_eq!(session["auth"]["accessToken"], "AT-B");
        assert_eq!(session["auth"]["refreshToken"], "RT-B");
        assert_eq!(session["auth"]["domain"], "www.codebuddy.cn");
        // 列表里同一 uid 不能出现两份。
        let list = session["allAccounts"].as_array().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["uid"], "uid-b");
        assert_eq!(session["accounts"], session["allAccounts"]);
    }

    #[test]
    fn build_session_from_nothing_still_produces_a_usable_shape() {
        let target = TargetSession {
            uid: "uid-new".to_string(),
            nickname: json!("新账号"),
            access: json!("AT"),
            refresh: json!("RT"),
            domain: "www.codebuddy.cn".to_string(),
            expires_at_ms: None,
        };
        let session = build_session(None, &target);
        assert_eq!(session["account"]["uid"], "uid-new");
        assert_eq!(session["account"]["nickname"], "新账号");
        assert_eq!(session["auth"]["tokenType"], "Bearer");
        assert_eq!(session["allAccounts"].as_array().unwrap().len(), 1);
    }

    // ---- 切换编排（进程与文件都用假的，绝不碰真实客户端）----

    use crate::paths::Paths;
    use crate::storage::AccountInput;
    use std::cell::{Cell, RefCell};

    #[derive(Clone, PartialEq)]
    enum Launch {
        /// 客户端启动并接受登录态。
        Works,
        /// 客户端启动后把文件改回另一个账号（模拟"不接受这份令牌"）。
        RewritesTo(&'static str),
        /// 客户端写退出标记。
        LogsOut,
        /// launch 返回成功但进程没起来。
        NeverStarts,
    }

    struct FakeControl {
        auth_path: PathBuf,
        app_path: PathBuf,
        installed: bool,
        running: Cell<bool>,
        /// 只在**第一次**启动时生效；回滚后的重启一律视为"正常启动"，
        /// 否则测试会分不清"客户端拒绝了"和"回滚把文件写坏了"。
        mode: RefCell<Option<Launch>>,
        launches: Cell<u32>,
        closes: Cell<u32>,
    }

    impl FakeControl {
        fn new(auth_path: &Path, running: bool, mode: Launch) -> Self {
            Self {
                auth_path: auth_path.to_path_buf(),
                app_path: PathBuf::from("/Applications/WorkBuddy.app"),
                installed: true,
                running: Cell::new(running),
                mode: RefCell::new(Some(mode)),
                launches: Cell::new(0),
                closes: Cell::new(0),
            }
        }
    }

    fn auth_json(uid: &str, access: &str) -> String {
        serde_json::to_string_pretty(&json!({
            "auth": {
                "accessToken": access,
                "refreshToken": "RT-OLD",
                "tokenType": "Bearer",
                "domain": "www.workbuddy.cn",
            },
            "account": {"uid": uid, "nickname": "旧账号"},
            "allAccounts": [{"uid": uid, "nickname": "旧账号"}],
        }))
        .unwrap()
    }

    impl ClientControl for FakeControl {
        fn variant(&self) -> wb_switch_core::modules::variant::WbVariant {
            wb_switch_core::modules::variant::WbVariant::Cn
        }
        fn app_path(&self) -> PathBuf {
            self.app_path.clone()
        }
        fn installed(&self) -> bool {
            self.installed
        }
        fn is_running(&self) -> bool {
            self.running.get()
        }
        fn close(&self, _timeout_secs: i64) -> Result<(), String> {
            self.closes.set(self.closes.get() + 1);
            self.running.set(false);
            Ok(())
        }
        fn launch(&self) -> Result<(), String> {
            self.launches.set(self.launches.get() + 1);
            let mode = self.mode.borrow_mut().take();
            match mode {
                Some(Launch::RewritesTo(uid)) => {
                    std::fs::write(&self.auth_path, auth_json(uid, "AT-OTHER")).unwrap();
                    self.running.set(true);
                }
                Some(Launch::LogsOut) => {
                    std::fs::write(client::logout_marker_path(&self.auth_path), "logged-out")
                        .unwrap();
                    self.running.set(false);
                }
                Some(Launch::NeverStarts) => self.running.set(false),
                // `Works` 与"回滚后的重启"都走这里。
                _ => self.running.set(true),
            }
            Ok(())
        }
        fn sleep_ms(&self, _ms: u64) {}
        fn verify_window(&self) -> (u64, u64) {
            (0, 120)
        }
    }

    struct Harness {
        _dir: tempfile::TempDir,
        app: App,
        env: SwitchEnv,
        account: AccountRecord,
        original: String,
    }

    fn harness(running: bool, mode: Launch) -> (Harness, FakeControl) {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let paths = Paths {
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            data_dir: data.clone(),
            wb_home_owned: true,
        };
        let app = App::with_secure(paths, crate::securestore::SecureStore::memory());
        app.with_db(|db| db.migrate().map(|_| ())).unwrap();
        let id = app
            .with_db(|db| {
                db.upsert_account(&AccountInput {
                    platform: "workbuddy".to_string(),
                    uid: "uid-b".to_string(),
                    nickname: Some("新账号".to_string()),
                    ..Default::default()
                })
            })
            .unwrap()
            .id;
        let account = app.with_db(|db| db.get_account(&id)).unwrap().unwrap();

        let auth_path = dir.path().join("workbuddy-desktop.info");
        let original = auth_json("uid-a", "AT-OLD");
        std::fs::write(&auth_path, &original).unwrap();

        let env = SwitchEnv {
            auth_path: auth_path.clone(),
            snapshot_dir: data.join("workbuddy-switch"),
        };
        let control = FakeControl::new(&auth_path, running, mode);
        (
            Harness {
                _dir: dir,
                app,
                env,
                account,
                original,
            },
            control,
        )
    }

    fn target() -> TargetSession {
        TargetSession {
            uid: "uid-b".to_string(),
            nickname: json!("新账号"),
            access: json!("AT-B"),
            refresh: json!("RT-B"),
            domain: "www.codebuddy.cn".to_string(),
            expires_at_ms: Some(1_800_000_000_000),
        }
    }

    /// 正常路径：写进去、客户端接受、journal 收尾为 verified，未完成槽位释放。
    #[test]
    fn apply_confirms_when_the_client_accepts_the_login() {
        let (h, control) = harness(true, Launch::Works);
        let outcome = apply(&h.app, &h.account, &target(), &control, &h.env).unwrap();

        assert!(outcome.verified, "{:?}", outcome.verify_detail);
        assert_eq!(outcome.phase, PHASE_VERIFIED);
        assert_eq!(outcome.previous_uid.as_deref(), Some("uid-a"));
        assert_eq!(control.closes.get(), 1, "切换前应当关一次客户端");
        assert_eq!(control.launches.get(), 1);

        let live = client::read_live(&h.env.auth_path);
        assert_eq!(live.uid.as_deref(), Some("uid-b"));
        assert_eq!(live.token_form, Some("plaintext"));

        let journal = h
            .app
            .with_db(|db| db.wb_switch_journal(&outcome.journal_id))
            .unwrap()
            .unwrap();
        assert_eq!(journal.phase, PHASE_VERIFIED);
        assert!(h.app.with_db(|db| db.open_wb_switch()).unwrap().is_none());
    }

    /// 客户端不接受（把文件改回原账号）→ 必须回滚到原样的字节，并释放槽位。
    #[test]
    fn a_rejecting_client_triggers_a_byte_exact_rollback() {
        let (h, control) = harness(true, Launch::RewritesTo("uid-a"));
        let err = apply(&h.app, &h.account, &target(), &control, &h.env).unwrap_err();

        let data = err.data.clone().expect("失败必须带恢复结果");
        assert_eq!(data["kind"], "switch_failed");
        assert_eq!(data["restored"], true);
        assert_eq!(
            std::fs::read_to_string(&h.env.auth_path).unwrap(),
            h.original,
            "回滚必须逐字节还原，不能重建"
        );
        assert!(control.launches.get() >= 2, "回滚后要重新启动客户端");
        assert!(h.app.with_db(|db| db.open_wb_switch()).unwrap().is_none());
    }

    /// 客户端写退出标记同样算拒绝。
    #[test]
    fn a_logout_marker_is_treated_as_rejection() {
        let (h, control) = harness(true, Launch::LogsOut);
        let err = apply(&h.app, &h.account, &target(), &control, &h.env).unwrap_err();
        assert_eq!(err.data.unwrap()["restored"], true);
        assert!(
            !client::logout_marker_path(&h.env.auth_path).exists(),
            "回滚要清掉标记"
        );
        assert_eq!(
            std::fs::read_to_string(&h.env.auth_path).unwrap(),
            h.original
        );
    }

    /// 进程没起来：**不回滚**，落 unconfirmed，让用户自己确认。
    #[test]
    fn a_client_that_never_starts_is_unconfirmed_not_rolled_back() {
        let (h, control) = harness(true, Launch::NeverStarts);
        let outcome = apply(&h.app, &h.account, &target(), &control, &h.env).unwrap();

        assert!(!outcome.verified);
        assert_eq!(outcome.phase, PHASE_UNCONFIRMED);
        assert_eq!(
            client::read_live(&h.env.auth_path).uid.as_deref(),
            Some("uid-b"),
            "文件已经写成目标账号，不该被回滚"
        );
        let journal = h
            .app
            .with_db(|db| db.wb_switch_journal(&outcome.journal_id))
            .unwrap()
            .unwrap();
        assert_eq!(journal.phase, PHASE_UNCONFIRMED);
        assert!(h.app.with_db(|db| db.open_wb_switch()).unwrap().is_none());
    }

    /// 快照被改动过 → 恢复必须拒绝，不能用一份来源不明的文件覆盖登录态。
    #[test]
    fn restore_refuses_a_tampered_snapshot() {
        let (h, control) = harness(true, Launch::Works);
        let outcome = apply(&h.app, &h.account, &target(), &control, &h.env).unwrap();
        let journal = h
            .app
            .with_db(|db| db.wb_switch_journal(&outcome.journal_id))
            .unwrap()
            .unwrap();
        std::fs::write(&journal.snapshot_path, b"{}").unwrap();

        let err = restore(&h.app, Some(&outcome.journal_id), &control, &h.env).unwrap_err();
        assert_eq!(err.data.unwrap()["kind"], "snapshot_corrupt");
        assert_eq!(
            client::read_live(&h.env.auth_path).uid.as_deref(),
            Some("uid-b"),
            "拒绝恢复时不能动客户端文件"
        );
    }

    /// 显式恢复：把切换前的登录态放回去。
    #[test]
    fn restore_puts_the_previous_login_back() {
        let (h, control) = harness(true, Launch::Works);
        let outcome = apply(&h.app, &h.account, &target(), &control, &h.env).unwrap();
        // 恢复时客户端应当按上一次记录的"原本在跑"重新启动。
        let value = restore(&h.app, Some(&outcome.journal_id), &control, &h.env).unwrap();

        assert_eq!(value["restored"], true);
        assert_eq!(value["uid"], "uid-a");
        assert_eq!(
            std::fs::read_to_string(&h.env.auth_path).unwrap(),
            h.original
        );
    }

    /// 没有可恢复的记录时不该报错，如实返回"没事可做"。
    #[test]
    fn restore_with_nothing_to_do_is_not_an_error() {
        let (h, control) = harness(true, Launch::Works);
        let value = restore(&h.app, None, &control, &h.env).unwrap();
        assert_eq!(value["restored"], false);
        assert_eq!(value["reason"], "nothing_to_restore");
    }

    /// 未完成切换会占住槽位：第二次必须被拒，不能并行改两个账号。
    #[test]
    fn a_pending_switch_blocks_a_second_one() {
        let (h, control) = harness(true, Launch::Works);
        // 模拟"进程在切换中间被杀掉"：库里留着一条未完成的记录，文件还是原账号。
        h.app
            .with_db(|db| {
                db.begin_wb_switch(&NewSwitchJournal {
                    id: "j-interrupted",
                    target_account_id: &h.account.id,
                    target_uid: "uid-b",
                    previous_uid: Some("uid-a"),
                    previous_running: true,
                    previous_marker: false,
                    snapshot_path: "/tmp/snapshot.json",
                    snapshot_sha256: "sha",
                    token_form: Some("plaintext"),
                })
            })
            .unwrap();

        let err = apply(&h.app, &h.account, &target(), &control, &h.env).unwrap_err();
        assert_eq!(err.data.unwrap()["kind"], "switch_busy");
        assert_eq!(
            std::fs::read_to_string(&h.env.auth_path).unwrap(),
            h.original,
            "被拒时不能动客户端文件"
        );
    }

    #[test]
    fn db_is_available_for_journal_tests() {
        // 只是钉住 `SwitchEnv::system` 依赖的 `App` 目录形状，防将来改路径改漏。
        let dir = tempfile::tempdir().unwrap();
        let mut db = Db::open(&dir.path().join("accounts.sqlite")).unwrap();
        db.migrate().unwrap();
        assert!(db.open_wb_switch().unwrap().is_none());
    }
}
