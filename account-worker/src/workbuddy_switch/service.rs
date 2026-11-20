//! WorkBuddy 客户端账号切换的编排。
//!
//! ## 一句话
//!
//! 备份 → 关客户端 → 写认证 → 起客户端 → **回读确认客户端真的用上了**；
//! 任何一步失败都按备份逐字节还原，并把结果如实报给调用方。
//!
//! ## 两种授权，两种生命周期
//!
//! | 模式 | 写进客户端的令牌 | 谁续期 | 结束条件 |
//! | --- | --- | --- | --- |
//! | [`plan::Mode::OwnRefresh`] | 自有 access + refresh | WorkBuddy 自己刷新 | 用户退出或切走 |
//! | [`plan::Mode::MagpieTemporary`] | **只有** Magpie 的 access | 没人续期 | access 过期 |
//!
//! 临时模式不碰 Magpie 的 refresh（那是它的凭据），所以必然过期。因此：
//! 切换前按 Magpie 文件里的 `expires` 算剩余时间，不够就拒绝；切换成功后
//! `restore_state` 一直是 `pending`，界面必须持续提供「恢复原账号」，直到用户真的恢复。
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
//! ## 客户端成功 ≠ 服务端接受
//!
//! 回读只能证明"客户端把文件读进去并停在这个 UID"。令牌是否被服务端接受是另一件事，
//! 由一次只读探针单独记录（`authAccepted`）。没有证据时如实返回 `unknown`。

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};
use wb_switch_core::modules::auth_file;

use crate::app::App;
use crate::auth::probe::{self, Probe};
use crate::auth::service as auth_service;
use crate::providers::magpie;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::wb_switch::{
    NewSwitchJournal, SwitchJournal, PHASE_FAILED, PHASE_LAUNCHED, PHASE_RESTORED,
    PHASE_UNCONFIRMED, PHASE_VERIFIED, PHASE_WRITTEN, RESTORE_DONE,
};
use crate::storage::AccountRecord;

use super::client::{self, ClientControl};
use super::plan::{self, Mode, Plan, PlanOutcome};

/// 关客户端 / 恢复时各自允许的等待。
const CLOSE_TIMEOUT_SECS: i64 = 20;
const ROLLBACK_CLOSE_SECS: i64 = 15;

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
    /// 切换后用只读探针确认服务端是否接受这份 access。失败不影响切换结论。
    pub verify_server: bool,
}

/// 写进客户端的凭据。**两种模式的差别就在这里**。
pub enum TargetCredential {
    /// 自有授权：access + refresh，客户端会自己续期。
    OwnRefresh { access: String, refresh: String },
    /// Magpie：只有 access。**不碰 refresh**，也因此必然过期。
    MagpieAccess { access: String },
}

impl TargetCredential {
    fn access(&self) -> &str {
        match self {
            Self::OwnRefresh { access, .. } | Self::MagpieAccess { access } => access,
        }
    }
}

/// 写进客户端的材料。
pub struct TargetSession {
    pub uid: String,
    pub nickname: Value,
    pub domain: String,
    pub expires_at_ms: Option<i64>,
    pub credential: TargetCredential,
}

/// 整体状态：客户端、当前登录、每个账号能不能切、有没有未完成的切换、有没有待还原的临时登录。
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
        let (switchable, reason, detail, mode, access_expires_at_ms, remaining) =
            match plan::resolve(app, account, installed, live.uid.as_deref()) {
                PlanOutcome::Ready(plan) => (
                    true,
                    plan::reason::OK,
                    plan.degraded.clone().unwrap_or_default(),
                    plan.mode.as_str(),
                    plan.access_expires_at_ms,
                    plan.remaining_secs,
                ),
                PlanOutcome::Blocked { reason, detail } => (false, reason, detail, "", None, None),
            };
        candidates.push(json!({
            "accountId": account.id,
            "uid": account.uid,
            "nickname": account.nickname,
            "provider": account.provider,
            "switchable": switchable,
            "reason": reason,
            "reasonDetail": detail,
            // `own_refresh` / `magpie_access_temporary`；不可切时是空串。
            "mode": mode,
            "temporary": mode == Mode::MagpieTemporary.as_str(),
            "accessExpiresAt": access_expires_at_ms.map(ms_to_rfc3339),
            "remainingSeconds": remaining,
        }));
    }

    let open = app.with_db(|db| db.open_wb_switch())?;
    let latest = app.with_db(|db| db.latest_wb_switch())?;
    let recent = app.with_db(|db| db.wb_switch_journals(5))?;
    let temporary_restore = app.with_db(|db| db.pending_temporary_restore())?;

    // 临时登录是否正作用在当前登录账号上：是的话界面要标"临时登录"，并且一直给恢复入口。
    let live_temporary = temporary_restore
        .as_ref()
        .filter(|journal| live.uid.as_deref() == Some(journal.target_uid.as_str()));
    let restore_available = open.is_some() || temporary_restore.is_some();

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
            "tokenForm": live.token_form,
            "tokenFingerprint": live.token_fingerprint,
            "accountsInFile": live.accounts_len,
            "sizeBytes": live.size,
            "modifiedAtMs": live.modified_ms,
            "sha256": live.sha256,
            "error": live.parse_error,
            // 当前这份登录是临时借来的：界面必须显示到期时间与"恢复原账号"。
            "temporary": live_temporary.is_some(),
            "temporaryAccountId": live_temporary.map(|journal| journal.target_account_id.clone()),
            "accessExpiresAt": live_temporary
                .and_then(|journal| journal.access_expires_at.clone()),
            "remainingSeconds": live_temporary
                .and_then(|journal| remaining_seconds(journal.access_expires_at.as_deref())),
            // 建议最晚什么时候恢复：到期前留出安全余量。
            "safeRestoreUntil": live_temporary
                .and_then(|journal| safe_restore_at(journal.access_expires_at.as_deref())),
        },
        "candidates": candidates,
        "pendingSwitch": open,
        "temporaryRestore": temporary_restore,
        "restoreAvailable": restore_available,
        "lastSwitch": latest,
        "recent": recent,
    }))
}

/// 切换。成功返回 `{ok:true, changed:true, verified:…, journalId:…}`；
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

    // 未完成的切换必须先处理：否则"当前登录"到底该还原到哪一份就说不清了。
    if app.with_db(|db| db.open_wb_switch())?.is_some() {
        return Err(RpcError::new(
            crate::rpc::E_SWITCH,
            "已有一条未完成的账号切换，先恢复或等它结束",
        )
        .with_data(json!({ "kind": "switch_busy" })));
    }

    let before = client::read_live(&env.auth_path);
    let outcome = plan::resolve(app, &account, control.installed(), before.uid.as_deref());

    // 「客户端已经登录的就是它」不是错误：界面按 `changed:false` 处理。
    if let PlanOutcome::Blocked { reason, .. } = &outcome {
        if *reason == plan::reason::LIVE && before.logged_in() {
            return Ok(json!({
                "ok": true,
                "changed": false,
                "alreadyLive": true,
                "target": { "accountId": account.id, "uid": account.uid, "nickname": account.nickname },
            }));
        }
    }
    let plan = match outcome {
        PlanOutcome::Ready(plan) => plan,
        PlanOutcome::Blocked { reason, detail } => {
            let code = if reason == plan::reason::NO_CREDENTIAL
                || reason == plan::reason::MISSING_REFRESH
                || reason.starts_with("magpie_")
            {
                crate::rpc::E_AUTH
            } else {
                crate::rpc::E_SWITCH
            };
            return Err(RpcError::new(code, detail).with_data(json!({ "kind": reason })));
        }
    };

    let target = target_session(app, &account, &plan, before.json.as_ref())?;

    // 活跃取证放在关客户端之前：它要联网，客户端这时还能正常用。
    let activity_before = if request.observe_activity {
        Some(activity_snapshot(app, &account.id).await)
    } else {
        None
    };

    let applied = apply(app, &account, &plan, &target, control, env)?;

    let auth_accepted = if request.verify_server {
        let outcome = probe_server(&account, target.credential.access(), &target.domain).await;
        let _ = app.with_db(|db| db.set_switch_auth_accepted(&applied.journal_id, &outcome.0));
        Some(outcome)
    } else {
        None
    };

    let activity_after = if request.observe_activity {
        Some(activity_snapshot(app, &account.id).await)
    } else {
        None
    };
    if activity_before.is_some() || activity_after.is_some() {
        let _ = app.with_db(|db| {
            db.set_wb_switch_activity(
                &applied.journal_id,
                activity_before.as_ref(),
                activity_after.as_ref(),
            )
        });
    }

    // 新的一次切换已经成功，更早的临时记录就此结算：
    // 「恢复原账号」永远指"回到最近一次切换之前那一站"，不会攒出一堆待还原条目。
    let _ = app.with_db(|db| db.settle_pending_temporary_except(&applied.journal_id));

    log_note(&format!(
        "WorkBuddy 账号切换：{} → {}（{}），phase={}",
        applied.previous_uid.as_deref().unwrap_or("—"),
        account.uid,
        plan.mode.as_str(),
        applied.phase
    ));

    Ok(json!({
        "ok": true,
        "changed": true,
        "journalId": applied.journal_id,
        "phase": applied.phase,
        "verified": applied.verified,
        "verifyDetail": applied.verify_detail,
        "clientRunning": applied.running,
        "clientRewroteFile": applied.client_rewrote,
        "mode": plan.mode.as_str(),
        "temporary": plan.temporary(),
        "accessExpiresAt": plan.access_expires_at_ms.map(ms_to_rfc3339),
        "remainingSeconds": plan.remaining_secs,
        "safeRestoreUntil": plan.safe_restore_until_ms.map(ms_to_rfc3339),
        // 客户端层面已确认；服务端层面对这份 access 的结论单独给。
        "authAccepted": auth_accepted.as_ref().map(|(state, _)| state.clone()).unwrap_or_else(|| "unknown".to_string()),
        "authAcceptedDetail": auth_accepted.and_then(|(_, detail)| detail),
        "target": {
            "accountId": account.id,
            "uid": account.uid,
            "nickname": account.nickname,
        },
        "previousUid": applied.previous_uid,
        "snapshotSha256": applied.snapshot_sha,
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
    plan: &Plan,
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

    let access_fingerprint = crate::securestore::fingerprint(target.credential.access().as_bytes());
    let access_expires_at = plan.access_expires_at_ms.map(ms_to_rfc3339);
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
            auth_source: if plan.mode == Mode::OwnRefresh {
                "own"
            } else {
                "magpie"
            },
            temporary: plan.temporary(),
            access_expires_at: access_expires_at.as_deref(),
            access_fingerprint: Some(&access_fingerprint),
            source_key: plan.source_key.as_deref(),
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
/// 不带 `journal_id` 时按「未完成的切换 → 待还原的临时登录 → 最近一次成功切换」挑一条。
///
/// `force` 只影响一件事：当前客户端登录的既不是目标账号、也不是原账号（用户手动换过），
/// 默认拒绝，避免静默覆盖用户的新登录态；界面确认后可以 `force=true` 再调一次。
pub fn restore(
    app: &App,
    journal_id: Option<&str>,
    force: bool,
    control: &dyn ClientControl,
    env: &SwitchEnv,
) -> RpcResult<Value> {
    let journal = match journal_id {
        Some(id) => app
            .with_db(|db| db.wb_switch_journal(id))?
            .ok_or_else(|| RpcError::not_found(format!("切换记录不存在：{id}")))?,
        None => match pick_restorable(app)? {
            Some(row) => row,
            None => {
                return Ok(json!({
                    "ok": true,
                    "restored": false,
                    "reason": "nothing_to_restore",
                    "message": "没有可恢复的切换记录",
                }))
            }
        },
    };

    let previous_uid = journal.previous_uid.clone();
    let snapshot = read_snapshot(&journal)?;

    // 用户手动换了别的账号：默认不动，交回去让界面确认。
    let live = client::read_live(&env.auth_path);
    if !force && live.logged_in() && live.uid.as_deref() != Some(journal.target_uid.as_str()) {
        return Err(RpcError::new(
            crate::rpc::E_SWITCH,
            format!(
                "客户端当前登录的是 {}，不是这一轮切换的目标账号；恢复会覆盖它，需要确认",
                live.uid
                    .as_deref()
                    .map(|uid| format!("「{}」", short_uid(uid)))
                    .unwrap_or_else(|| "另一个账号".to_string())
            ),
        )
        .with_data(json!({
            "kind": plan::reason::LIVE_CHANGED,
            "journalId": journal.id,
            "currentUid": live.uid,
            "targetUid": journal.target_uid,
        })));
    }

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
        )?;
        // 恢复成功就把"临时登录"这件事收掉；失败则保持 pending，允许再试。
        if ok {
            db.set_restore_state(&journal.id, RESTORE_DONE)?;
        }
        Ok(())
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

/// 没指定 journal 时挑一条：未完成切换优先，其次待还原的临时登录，最后是最近一次成功切换。
fn pick_restorable(app: &App) -> RpcResult<Option<SwitchJournal>> {
    if let Some(open) = app.with_db(|db| db.open_wb_switch())? {
        return Ok(Some(open));
    }
    if let Some(temporary) = app.with_db(|db| db.pending_temporary_restore())? {
        return Ok(Some(temporary));
    }
    let recent = app.with_db(|db| db.wb_switch_journals(10))?;
    Ok(recent.into_iter().find(|row| {
        row.previous_uid.is_some()
            && matches!(
                row.phase.as_str(),
                PHASE_VERIFIED | PHASE_UNCONFIRMED | PHASE_FAILED
            )
            && row.restore_state != RESTORE_DONE
    }))
}

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

/// 取出目标账号的登录材料。**Magpie 模式现读文件**，不用数据库里的旧值。
fn target_session(
    app: &App,
    account: &AccountRecord,
    plan: &Plan,
    existing: Option<&Value>,
) -> RpcResult<TargetSession> {
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

    match plan.mode {
        Mode::OwnRefresh => {
            let access = auth_service::read_access(app, account, "own")?
                .filter(|token| !token.trim().is_empty())
                .ok_or_else(|| {
                    RpcError::new(crate::rpc::E_AUTH, "读取自有授权失败：没有访问令牌")
                        .with_data(json!({ "kind": plan::reason::NO_CREDENTIAL }))
                })?;
            let refresh = auth_service::read_own_refresh(app, account)?
                .filter(|token| !token.trim().is_empty())
                .ok_or_else(|| {
                    RpcError::new(crate::rpc::E_AUTH, "读取自有授权失败：没有刷新令牌")
                        .with_data(json!({ "kind": plan::reason::MISSING_REFRESH }))
                })?;
            let expires_at_ms = account
                .token_expires_at
                .as_deref()
                .and_then(auth_service::parse_rfc3339)
                .map(|time| time.timestamp_millis())
                .or(plan.access_expires_at_ms);
            Ok(TargetSession {
                uid: account.uid.clone(),
                nickname,
                domain,
                expires_at_ms,
                credential: TargetCredential::OwnRefresh { access, refresh },
            })
        }
        Mode::MagpieTemporary => {
            // 现读一次：计划阶段到真正写文件之间，Magpie 可能已经刷新或退登。
            let (access, expires_at_ms) = magpie_access_now(account, plan)?;
            Ok(TargetSession {
                uid: account.uid.clone(),
                nickname,
                domain,
                expires_at_ms: Some(expires_at_ms),
                credential: TargetCredential::MagpieAccess { access },
            })
        }
    }
}

/// 现读 Magpie 文件，拿 access 与到期时间。**不读 refresh**。
fn magpie_access_now(account: &AccountRecord, plan: &Plan) -> RpcResult<(String, i64)> {
    let source_key = plan.source_key.clone().ok_or_else(|| {
        RpcError::new(crate::rpc::E_AUTH, "这条 Magpie 授权没有来源键")
            .with_data(json!({ "kind": plan::reason::MAGPIE_SOURCE_MISSING }))
    })?;
    let logins = magpie::read_logins().map_err(|err| {
        RpcError::new(
            crate::rpc::E_AUTH,
            format!(
                "Magpie 登录态不可读（{}）",
                crate::rpc::sanitize(&err.message())
            ),
        )
        .with_data(json!({ "kind": plan::reason::MAGPIE_UNAVAILABLE }))
    })?;
    let login =
        crate::auth::magpie_link::find_link(&logins, account, &source_key, Some(magpie::now_ms()))
            .map_err(|problem| {
                RpcError::new(
                    crate::rpc::E_AUTH,
                    format!("Magpie 关联不可用：{}", problem.as_str()),
                )
                .with_data(json!({ "kind": problem.as_str() }))
            })?;
    if login.access_token.trim().is_empty() {
        return Err(RpcError::new(crate::rpc::E_AUTH, "Magpie 访问令牌是空的")
            .with_data(json!({ "kind": plan::reason::MAGPIE_ACCESS_EMPTY })));
    }
    let expires_at = login.expires_at_ms.ok_or_else(|| {
        RpcError::new(crate::rpc::E_AUTH, "Magpie 没有提供访问令牌的有效期")
            .with_data(json!({ "kind": plan::reason::MAGPIE_EXPIRY_UNKNOWN }))
    })?;
    Ok((login.access_token.clone(), expires_at))
}

/// 生成写进客户端的完整 JSON。**沿用客户端已有的结构**，只替换认证与当前账号。
///
/// 复用的意义：客户端里的账号对象带着 `uin` / `avatarUrl` / `uniqueUserId` /
/// 昵称信封等字段，凭 AICC 库里那点信息重建只会得到一个缺字段的账号。
///
/// 自有授权走上游的 `build_auth_obj`（access + refresh）；Magpie 临时授权走
/// [`build_access_only_auth`]，**绝不把上一账号的 refresh 留给目标账号**。
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
        "token_type": "Bearer",
        "domain": target.domain,
        "profile_raw": Value::Object(profile),
    });
    if let TargetCredential::OwnRefresh { access, refresh } = &target.credential {
        acc["access_token"] = json!(access);
        acc["refresh_token"] = json!(refresh);
    } else {
        acc["access_token"] = json!(target.credential.access());
    }
    if let Some(expires_at) = target.expires_at_ms {
        acc["expiresAt"] = json!(expires_at);
    }

    let mut account_obj = auth_file::build_account_obj(&acc);
    if let Some(map) = account_obj.as_object_mut() {
        // `build_account_obj` 只在缺失时补 `lastLogin`：旧列表项是 false 时要盖掉。
        map.insert("lastLogin".to_string(), Value::Bool(true));
    }
    let auth_obj = match &target.credential {
        TargetCredential::OwnRefresh { .. } => auth_file::build_auth_obj(&acc),
        TargetCredential::MagpieAccess { .. } => {
            build_access_only_auth(&acc["access_token"], &target.domain, target.expires_at_ms)
        }
    };

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

/// Access-only 的 `auth` 段：**只有 access，没有任何 refresh 字段**。
///
/// 刻意不用 `build_auth_obj`：它在缺 refresh 时会写一个空串 `refreshToken`
/// 并补 `refreshExpiresAt`，那是"没有刷新令牌"和"刷新令牌是空"两件不同的事。
/// 对一个只借来 access 的临时登录，写空 refresh 会让客户端去做一次必然失败的刷新，
/// 更糟的是让人误以为这份登录可以续期。
///
/// 已知风险（`docs/WORKBUDDY_CLIENT_SWITCH.md` §8）：客户端遇到"没有 refreshToken"
/// 会怎么处理**没有实测证据**。保守预期是"用到 access 过期为止"，到期后可能被登出。
/// 所以临时模式必须配合 `restore_state=pending` 与到期提醒，不能当成长期登录。
pub fn build_access_only_auth(access: &Value, domain: &str, expires_at_ms: Option<i64>) -> Value {
    let now = wb_switch_core::modules::config::now_ms();
    let mut auth = Map::new();
    auth.insert("accessToken".to_string(), access.clone());
    auth.insert("tokenType".to_string(), json!("Bearer"));
    auth.insert("domain".to_string(), json!(domain));
    auth.insert("lastRefreshTime".to_string(), json!(now));
    auth.insert(
        "scope".to_string(),
        json!("openid profile offline_access email"),
    );
    auth.insert("notBeforePolicy".to_string(), json!(0));
    auth.insert("sessionState".to_string(), json!(""));
    if let Some(expires_at) = expires_at_ms {
        auth.insert("expiresAt".to_string(), json!(expires_at));
        auth.insert(
            "expiresIn".to_string(),
            json!(((expires_at - now) / 1000).max(0)),
        );
    }
    Value::Object(auth)
}

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

/// 一次只读探针：服务端到底认不认这份 access。
///
/// 只回答"这份令牌能不能用"，**不写任何状态、不刷令牌、不改授权行**。
/// 网络抖动一律 `unknown` —— 不能把"没问到"说成"被拒绝"。
async fn probe_server(
    account: &AccountRecord,
    access: &str,
    domain: &str,
) -> (String, Option<String>) {
    let report = probe::probe(
        wb_switch_core::modules::variant::WbVariant::Cn,
        access,
        &account.uid,
        domain,
    )
    .await;
    match report.outcome {
        Probe::Valid => ("accepted".to_string(), None),
        Probe::Unauthorized => (
            "rejected".to_string(),
            Some(format!(
                "服务端拒绝了这份令牌（HTTP {}）",
                report
                    .http_status
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "?".to_string())
            )),
        ),
        Probe::Transient => (
            "unknown".to_string(),
            Some("网络或服务端瞬时失败，未能确认服务端是否接受".to_string()),
        ),
    }
}

fn ms_to_rfc3339(ms: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(ms)
        .map(|at| at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_default()
}

/// 从 journal 里的 RFC3339 到期时间算剩余秒数。读不到就是 `None`（未知 ≠ 已过期）。
fn remaining_seconds(expires_at: Option<&str>) -> Option<i64> {
    let at = expires_at.and_then(auth_service::parse_rfc3339)?;
    Some((at.timestamp_millis() - magpie::now_ms()).max(0) / 1000)
}

/// 建议恢复截止 = access 到期 − [`plan::RESTORE_SAFETY_MARGIN_SECS`]。
fn safe_restore_at(expires_at: Option<&str>) -> Option<String> {
    let at = expires_at.and_then(auth_service::parse_rfc3339)?;
    Some(ms_to_rfc3339(
        at.timestamp_millis() - plan::RESTORE_SAFETY_MARGIN_SECS * 1000,
    ))
}

fn short_uid(uid: &str) -> String {
    uid.chars().take(8).collect()
}

/// `Display` 适配：错误文案里要一个短 UID，不想在 format! 里套函数。
struct WorkbuddyUidLabel<'a>(&'a str);

impl std::fmt::Display for WorkbuddyUidLabel<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", short_uid(self.0))
    }
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
            domain: "www.codebuddy.cn".to_string(),
            expires_at_ms: Some(1_800_000_000_000),
            credential: TargetCredential::OwnRefresh {
                access: "AT-B".to_string(),
                refresh: "RT-B".to_string(),
            },
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
            domain: "www.codebuddy.cn".to_string(),
            expires_at_ms: None,
            credential: TargetCredential::OwnRefresh {
                access: "AT".to_string(),
                refresh: "RT".to_string(),
            },
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

    fn own_plan() -> Plan {
        Plan {
            mode: Mode::OwnRefresh,
            source_key: None,
            access_expires_at_ms: None,
            safe_restore_until_ms: None,
            remaining_secs: None,
            degraded: None,
        }
    }

    fn magpie_plan() -> Plan {
        Plan {
            mode: Mode::MagpieTemporary,
            source_key: Some("workbuddy".to_string()),
            access_expires_at_ms: Some(1_800_000_000_000),
            safe_restore_until_ms: Some(1_800_000_000_000),
            remaining_secs: Some(3600),
            degraded: None,
        }
    }

    fn target() -> TargetSession {
        TargetSession {
            uid: "uid-b".to_string(),
            nickname: json!("新账号"),
            domain: "www.codebuddy.cn".to_string(),
            expires_at_ms: Some(1_800_000_000_000),
            credential: TargetCredential::OwnRefresh {
                access: "AT-B".to_string(),
                refresh: "RT-B".to_string(),
            },
        }
    }

    /// 正常路径：写进去、客户端接受、journal 收尾为 verified，未完成槽位释放。
    #[test]
    fn apply_confirms_when_the_client_accepts_the_login() {
        let (h, control) = harness(true, Launch::Works);
        let outcome = apply(&h.app, &h.account, &own_plan(), &target(), &control, &h.env).unwrap();

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
        let err = apply(&h.app, &h.account, &own_plan(), &target(), &control, &h.env).unwrap_err();

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
        let err = apply(&h.app, &h.account, &own_plan(), &target(), &control, &h.env).unwrap_err();
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
        let outcome = apply(&h.app, &h.account, &own_plan(), &target(), &control, &h.env).unwrap();

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
        let outcome = apply(&h.app, &h.account, &own_plan(), &target(), &control, &h.env).unwrap();
        let journal = h
            .app
            .with_db(|db| db.wb_switch_journal(&outcome.journal_id))
            .unwrap()
            .unwrap();
        std::fs::write(&journal.snapshot_path, b"{}").unwrap();

        let err = restore(&h.app, Some(&outcome.journal_id), false, &control, &h.env).unwrap_err();
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
        let outcome = apply(&h.app, &h.account, &own_plan(), &target(), &control, &h.env).unwrap();
        // 恢复时客户端应当按上一次记录的"原本在跑"重新启动。
        let value = restore(&h.app, Some(&outcome.journal_id), false, &control, &h.env).unwrap();

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
        let value = restore(&h.app, None, false, &control, &h.env).unwrap();
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
                    auth_source: "own",
                    temporary: false,
                    access_expires_at: None,
                    access_fingerprint: None,
                    source_key: None,
                })
            })
            .unwrap();

        let err = apply(&h.app, &h.account, &own_plan(), &target(), &control, &h.env).unwrap_err();
        assert_eq!(err.data.unwrap()["kind"], "switch_busy");
        assert_eq!(
            std::fs::read_to_string(&h.env.auth_path).unwrap(),
            h.original,
            "被拒时不能动客户端文件"
        );
    }

    // ---- Magpie access-only 临时切换（PR #112）----

    use crate::storage::authz::AuthorizationRecord;

    fn grant(
        app: &App,
        account_id: &str,
        kind: &str,
        source_key: Option<&str>,
        with_refresh: bool,
    ) {
        let now = crate::clock::now_rfc3339();
        let record = AuthorizationRecord {
            id: format!("{account_id}-{kind}"),
            account_id: account_id.to_string(),
            kind: kind.to_string(),
            access_ref: if kind == "magpie" {
                format!("magpie:v1:{}", source_key.unwrap_or("workbuddy"))
            } else {
                "localjson:v1:own".to_string()
            },
            refresh_ref: if kind == "own" && with_refresh {
                Some("localjson:v1:own".to_string())
            } else {
                None
            },
            auth_domain: Some("www.codebuddy.cn".to_string()),
            token_expires_at: None,
            status: "valid".to_string(),
            version: 0,
            key_migration_state: "ready".to_string(),
            source_key: source_key.map(str::to_string),
            last_used_at: None,
            last_error: None,
            created_at: now.clone(),
            updated_at: now,
        };
        app.with_db(|db| db.upsert_authorization(&record)).unwrap();
    }

    fn login(
        key: &str,
        uid: &str,
        provider: &str,
        access: &str,
        expires_in_secs: Option<i64>,
    ) -> magpie::MagpieLogin {
        magpie::MagpieLogin {
            key: key.to_string(),
            provider: magpie::provider_of(provider).expect("provider"),
            uid: uid.to_string(),
            nickname: None,
            domain: "www.codebuddy.cn".to_string(),
            access_token: access.to_string(),
            expires_at_ms: expires_in_secs.map(|secs| magpie::now_ms() + secs * 1000),
        }
    }

    fn resolve(h: &Harness) -> PlanOutcome {
        plan::resolve(&h.app, &h.account, true, Some("uid-a"))
    }

    fn ready(h: &Harness) -> Plan {
        match resolve(h) {
            PlanOutcome::Ready(plan) => plan,
            PlanOutcome::Blocked { reason, detail } => panic!("应当可切，实际 {reason}：{detail}"),
        }
    }

    fn blocked(h: &Harness) -> (&'static str, String) {
        match resolve(h) {
            PlanOutcome::Ready(_) => panic!("应当被拒绝，实际可切"),
            PlanOutcome::Blocked { reason, detail } => (reason, detail),
        }
    }

    /// 1) 有效期充足时，Magpie 账号给出「临时」计划，且知道还能用多久。
    #[test]
    fn magpie_access_is_accepted_when_fresh() {
        let (h, _control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-MAG",
            Some(3600),
        )]));

        let plan = ready(&h);
        assert_eq!(plan.mode, Mode::MagpieTemporary);
        assert!(plan.temporary());
        assert_eq!(plan.source_key.as_deref(), Some("workbuddy"));
        assert!(plan.remaining_secs.unwrap_or(0) >= 3590);
        assert!(plan.access_expires_at_ms.is_some());
        assert!(plan.safe_restore_until_ms.unwrap() < plan.access_expires_at_ms.unwrap());
        magpie::test_reset();
    }

    /// 1b) 同时有自有授权和 Magpie 关联时，**用自有授权**（长期），不是借来的 access。
    ///
    /// 第一版照抄了 `auth_preferred_source`（默认 magpie），结果双授权账号全被标成
    /// "临时登录"，与用户"我明明有 AICC 登录权限"的认知冲突。
    #[test]
    fn own_long_lived_login_wins_over_a_borrowed_access() {
        let (h, _control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        grant(&h.app, &h.account.id, "own", None, true);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-MAG",
            Some(3600),
        )]));

        let plan = ready(&h);
        assert_eq!(plan.mode, Mode::OwnRefresh);
        assert!(!plan.temporary());
        assert!(plan.degraded.is_none(), "首选就是自有授权，不该有降级说明");
        magpie::test_reset();
    }

    /// 自有授权缺 refresh 时才退回临时模式，并且要说明原因。
    #[test]
    fn a_refreshless_own_grant_falls_back_to_temporary_with_a_reason() {
        let (h, _control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "own", None, false);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-MAG",
            Some(3600),
        )]));

        let plan = ready(&h);
        assert_eq!(plan.mode, Mode::MagpieTemporary);
        let why = plan.degraded.unwrap_or_default();
        assert!(why.contains("自有授权不可用"), "{why}");
        assert!(why.contains("临时"), "{why}");
        magpie::test_reset();
    }

    /// 2) 同一个 provider 的多个账号：来源键必须一一对应，不能"取第一条"。
    #[test]
    fn source_keys_are_not_interchangeable_between_accounts() {
        let (h, _control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy#b"), false);
        magpie::test_set_logins(Some(vec![
            login("workbuddy#a", "uid-a", "workbuddy", "AT-A", Some(3600)),
            login("workbuddy#b", "uid-b", "workbuddy", "AT-B", Some(3600)),
        ]));
        assert_eq!(ready(&h).source_key.as_deref(), Some("workbuddy#b"));

        // 把来源键指到另一个账号的那条：UID 对不上，必须拒绝。
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy#a"), false);
        let (why, detail) = blocked(&h);
        assert_eq!(why, plan::reason::MAGPIE_UID_MISMATCH);
        assert!(detail.contains("UID"), "{detail}");
        magpie::test_reset();
    }

    /// 3) 只有 UID 对得上还不够，平台/provider 也必须一致。
    #[test]
    fn an_international_login_is_refused_for_the_cn_account() {
        let (h, _control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy-ai"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy-ai",
            "uid-b",
            "workbuddy-ai",
            "AT-AI",
            Some(3600),
        )]));
        let (why, _) = blocked(&h);
        assert_eq!(why, plan::reason::MAGPIE_PLATFORM_MISMATCH);
        magpie::test_reset();
    }

    /// 4) 已过期 / 剩余不足 / 没写有效期：三种都必须拒绝，且不能猜。
    #[test]
    fn expired_short_and_unknown_expiry_are_all_refused() {
        let (h, _control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);

        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT",
            Some(-60),
        )]));
        assert_eq!(blocked(&h).0, plan::reason::MAGPIE_ACCESS_EXPIRED);

        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT",
            Some(600),
        )]));
        let (why, detail) = blocked(&h);
        assert_eq!(why, plan::reason::MAGPIE_TOO_SHORT);
        assert!(detail.contains("分钟"), "{detail}");
        assert!(detail.contains("30"), "要说明门槛是多久：{detail}");

        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT",
            None,
        )]));
        assert_eq!(blocked(&h).0, plan::reason::MAGPIE_EXPIRY_UNKNOWN);

        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "   ",
            Some(3600),
        )]));
        assert_eq!(blocked(&h).0, plan::reason::MAGPIE_ACCESS_EMPTY);
        magpie::test_reset();
    }

    /// 5) Magpie 文件读不出来 ≠ 没有授权：原因要分开，界面才知道该让用户去做什么。
    #[test]
    fn an_unreadable_magpie_file_is_reported_as_such() {
        let (h, _control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_unreadable();
        let (why, detail) = blocked(&h);
        assert_eq!(why, plan::reason::MAGPIE_UNAVAILABLE);
        assert!(detail.contains("Magpie"), "{detail}");
        magpie::test_reset();
    }

    /// 6) Access-only 的 auth 段：只有 access，**一个 refresh 字段都没有**。
    #[test]
    fn access_only_auth_has_no_refresh_fields() {
        let auth = build_access_only_auth(
            &json!("AT-MAG"),
            "www.codebuddy.cn",
            Some(magpie::now_ms() + 3_600_000),
        );
        assert_eq!(auth["accessToken"], "AT-MAG");
        assert_eq!(auth["tokenType"], "Bearer");
        assert!(auth.get("refreshToken").is_none(), "{auth}");
        assert!(auth.get("refreshExpiresAt").is_none(), "{auth}");
        assert!(auth.get("refreshExpiresIn").is_none(), "{auth}");
        assert!(auth["expiresIn"].as_i64().unwrap() > 3_000);
    }

    /// 7) 关键一条：临时切换写出去的文件里**不能有上一账号的 refresh**。
    #[test]
    fn temporary_switch_writes_access_only_and_never_leaks_the_old_refresh() {
        let (h, control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-MAG",
            Some(3600),
        )]));

        let plan = ready(&h);
        let before = client::read_live(&h.env.auth_path);
        let target = target_session(&h.app, &h.account, &plan, before.json.as_ref()).unwrap();
        assert!(matches!(
            target.credential,
            TargetCredential::MagpieAccess { .. }
        ));
        apply(&h.app, &h.account, &plan, &target, &control, &h.env).unwrap();

        let text = std::fs::read_to_string(&h.env.auth_path).unwrap();
        assert!(
            !text.contains("RT-OLD"),
            "上一账号的 refresh 泄漏了：{text}"
        );
        let written: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(written["auth"]["accessToken"], "AT-MAG");
        assert!(written["auth"].get("refreshToken").is_none(), "{written}");
        assert_eq!(written["account"]["uid"], "uid-b");
        magpie::test_reset();
    }

    /// 8) journal 要记下"这是临时登录、什么时候过期"，但**不记令牌**。
    #[test]
    fn the_journal_records_the_temporary_window_without_tokens() {
        let (h, control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-SECRET-VALUE",
            Some(3600),
        )]));
        let plan = ready(&h);
        let before = client::read_live(&h.env.auth_path);
        let target = target_session(&h.app, &h.account, &plan, before.json.as_ref()).unwrap();
        let outcome = apply(&h.app, &h.account, &plan, &target, &control, &h.env).unwrap();

        let journal = h
            .app
            .with_db(|db| db.wb_switch_journal(&outcome.journal_id))
            .unwrap()
            .unwrap();
        assert_eq!(journal.auth_source.as_deref(), Some("magpie"));
        assert!(journal.temporary);
        assert_eq!(
            journal.restore_state,
            crate::storage::wb_switch::RESTORE_PENDING
        );
        assert!(journal.access_expires_at.is_some());
        assert_eq!(
            journal.access_fingerprint.as_ref().map(String::len),
            Some(8)
        );
        assert_eq!(journal.source_key.as_deref(), Some("workbuddy"));
        // 令牌本身一个字都不许落库。
        let dump = serde_json::to_string(&journal).unwrap();
        assert!(!dump.contains("AT-SECRET-VALUE"), "{dump}");
        magpie::test_reset();
    }

    /// 9) 临时登录成功后必须还能回到原账号（即使客户端已经登出）。
    #[test]
    fn restoring_a_temporary_login_puts_the_previous_account_back() {
        let (h, control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-MAG",
            Some(3600),
        )]));
        let plan = ready(&h);
        let before = client::read_live(&h.env.auth_path);
        let target = target_session(&h.app, &h.account, &plan, before.json.as_ref()).unwrap();
        let outcome = apply(&h.app, &h.account, &plan, &target, &control, &h.env).unwrap();

        // 模拟"客户端因 access 过期被登出"：文件没了。
        std::fs::remove_file(&h.env.auth_path).unwrap();

        let value = restore(&h.app, Some(&outcome.journal_id), false, &control, &h.env).unwrap();
        assert_eq!(value["restored"], true);
        assert_eq!(
            std::fs::read_to_string(&h.env.auth_path).unwrap(),
            h.original,
            "必须逐字节回到切换前"
        );
        let journal = h
            .app
            .with_db(|db| db.wb_switch_journal(&outcome.journal_id))
            .unwrap()
            .unwrap();
        assert_eq!(
            journal.restore_state,
            crate::storage::wb_switch::RESTORE_DONE
        );
        magpie::test_reset();
    }

    /// 10) 用户手动切到别的账号后，恢复不能静默覆盖；确认（force）才动手。
    #[test]
    fn restore_refuses_to_overwrite_a_manually_changed_login() {
        let (h, control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-MAG",
            Some(3600),
        )]));
        let plan = ready(&h);
        let before = client::read_live(&h.env.auth_path);
        let target = target_session(&h.app, &h.account, &plan, before.json.as_ref()).unwrap();
        let outcome = apply(&h.app, &h.account, &plan, &target, &control, &h.env).unwrap();

        // 用户自己在客户端里换成了 uid-c。
        std::fs::write(&h.env.auth_path, auth_json("uid-c", "AT-C")).unwrap();

        let err = restore(&h.app, Some(&outcome.journal_id), false, &control, &h.env).unwrap_err();
        assert_eq!(err.data.clone().unwrap()["kind"], "live_changed");
        assert_eq!(
            client::read_live(&h.env.auth_path).uid.as_deref(),
            Some("uid-c"),
            "拒绝时不能动文件"
        );

        let forced = restore(&h.app, Some(&outcome.journal_id), true, &control, &h.env).unwrap();
        assert_eq!(forced["restored"], true);
        assert_eq!(
            std::fs::read_to_string(&h.env.auth_path).unwrap(),
            h.original
        );
        magpie::test_reset();
    }

    /// 11) 连续临时切换不会攒下一堆"待还原"：只保留最近一次的记录。
    ///
    /// 第一版在这里用的是"按最新 pending 结算旧记录"，结果最新那条正是刚写入的自己，
    /// 旧记录一条都清不掉 —— 真机连续切两次后，别的账号全被 `temporary_pending` 挡住。
    #[test]
    fn only_the_latest_temporary_switch_stays_pending() {
        let (h, control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-MAG",
            Some(3600),
        )]));
        let plan = ready(&h);
        let before = client::read_live(&h.env.auth_path);
        let target = target_session(&h.app, &h.account, &plan, before.json.as_ref()).unwrap();

        let first = apply(&h.app, &h.account, &plan, &target, &control, &h.env).unwrap();
        // 第二次切换（同一套夹具下等价于"又切了一次"）。
        let second = apply(&h.app, &h.account, &plan, &target, &control, &h.env).unwrap();
        h.app
            .with_db(|db| db.settle_pending_temporary_except(&second.journal_id))
            .unwrap();

        let pending = h
            .app
            .with_db(|db| db.pending_temporary_restore())
            .unwrap()
            .expect("最近一次仍待还原");
        assert_eq!(pending.id, second.journal_id);
        assert_ne!(pending.id, first.journal_id);

        let first_row = h
            .app
            .with_db(|db| db.wb_switch_journal(&first.journal_id))
            .unwrap()
            .unwrap();
        assert_eq!(
            first_row.restore_state,
            crate::storage::wb_switch::RESTORE_DONE,
            "更早的临时记录必须被结算，不能一直 pending"
        );
        magpie::test_reset();
    }

    /// 12) AICC 重启后（新连接同一份库）仍能认出这次临时登录。
    #[test]
    fn the_pending_temporary_login_survives_a_restart() {
        let (h, control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-MAG",
            Some(3600),
        )]));
        let plan = ready(&h);
        let before = client::read_live(&h.env.auth_path);
        let target = target_session(&h.app, &h.account, &plan, before.json.as_ref()).unwrap();
        apply(&h.app, &h.account, &plan, &target, &control, &h.env).unwrap();
        drop(control);

        // 另一个连接 = 进程重启后的视角。
        let mut reopened = Db::open(&h.app.paths.db_path).unwrap();
        reopened.migrate().unwrap();
        let pending = reopened
            .pending_temporary_restore()
            .unwrap()
            .expect("应当还在");
        assert_eq!(pending.target_uid, "uid-b");
        assert_eq!(pending.previous_uid.as_deref(), Some("uid-a"));
        magpie::test_reset();
    }

    /// 9b) 自有授权缺 refresh 时走 Magpie，**完全不碰自有凭据**：
    /// 把它做成读不出来的坏引用也不影响。
    #[test]
    fn magpie_switch_never_reads_the_own_refresh() {
        let (h, control) = harness(true, Launch::Works);
        // own 行故意指向一个不存在的本地文件：只要被读一次就会报错。
        let now = crate::clock::now_rfc3339();
        h.app
            .with_db(|db| {
                db.upsert_authorization(&AuthorizationRecord {
                    id: format!("{}-own", h.account.id),
                    account_id: h.account.id.clone(),
                    kind: "own".to_string(),
                    // 缺 refresh → 计划阶段就落到 Magpie；access 引用也是坏的，
                    // 只要有任何一处真去读了自有凭据，这个用例就会失败。
                    access_ref: "localjson:v1:does-not-exist".to_string(),
                    refresh_ref: None,
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
            .unwrap();
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-MAG",
            Some(3600),
        )]));

        // 自有授权缺 refresh → 临时模式，取凭据时只读 Magpie 文件。
        let plan = ready(&h);
        assert_eq!(plan.mode, Mode::MagpieTemporary);
        assert!(plan
            .degraded
            .as_deref()
            .unwrap_or_default()
            .contains("自有授权不可用"));
        let before = client::read_live(&h.env.auth_path);
        let target = target_session(&h.app, &h.account, &plan, before.json.as_ref()).unwrap();
        assert_eq!(target.credential.access(), "AT-MAG");

        // 走完整个写入也不该碰到那份坏引用。
        apply(&h.app, &h.account, &plan, &target, &control, &h.env).unwrap();
        magpie::test_reset();
    }

    /// 14) status 要把"模式 / 临时 / 能不能恢复"一次说清，界面不再自己推断。
    #[test]
    fn status_reports_mode_and_restore_availability() {
        let (h, control) = harness(true, Launch::Works);
        grant(&h.app, &h.account.id, "magpie", Some("workbuddy"), false);
        magpie::test_set_logins(Some(vec![login(
            "workbuddy",
            "uid-b",
            "workbuddy",
            "AT-MAG",
            Some(3600),
        )]));

        let before = status(&h.app, &control, &h.env).unwrap();
        assert_eq!(before["restoreAvailable"], false);
        assert_eq!(before["temporaryRestore"], Value::Null);
        let candidate = before["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["accountId"] == h.account.id)
            .unwrap();
        assert_eq!(candidate["switchable"], true);
        assert_eq!(candidate["mode"], "magpie_access_temporary");
        assert_eq!(candidate["temporary"], true);
        assert!(candidate["accessExpiresAt"].is_string());
        assert!(candidate["remainingSeconds"].as_i64().unwrap() > 3000);

        // 切过去之后：标记临时登录 + 一直可恢复。
        let plan = ready(&h);
        let live = client::read_live(&h.env.auth_path);
        let target = target_session(&h.app, &h.account, &plan, live.json.as_ref()).unwrap();
        apply(&h.app, &h.account, &plan, &target, &control, &h.env).unwrap();

        let after = status(&h.app, &control, &h.env).unwrap();
        assert_eq!(after["restoreAvailable"], true);
        assert_eq!(after["live"]["temporary"], true);
        assert_eq!(after["live"]["uid"], "uid-b");
        assert!(after["live"]["accessExpiresAt"].is_string());
        assert_eq!(
            after["temporaryRestore"]["targetUid"], "uid-b",
            "待还原的临时登录必须能被界面找到"
        );
        magpie::test_reset();
    }

    /// 13) 读 Magpie 文件是只读的：不能顺手改写别人的登录态。
    #[test]
    fn reading_magpie_logins_never_rewrites_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin-auth.json");
        let body = json!({
            "workbuddy": {
                "access": "AT-MAG",
                "refresh": "RT-MAGPIE",
                "uid": "uid-b",
                "domain": "www.codebuddy.cn",
                "expires": magpie::now_ms() + 3_600_000,
            }
        })
        .to_string();
        std::fs::write(&path, &body).unwrap();

        let logins = magpie::read_logins_from(dir.path()).unwrap();
        assert_eq!(logins.len(), 1);
        assert_eq!(logins[0].access_token, "AT-MAG");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            body,
            "读取不能改动 Magpie 的文件"
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
