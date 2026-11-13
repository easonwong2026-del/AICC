//! Qoder 多账号 Profile 的业务层（认证快照顺序切换）。
//!
//! ## 三条硬约束
//!
//! 1. **不确定就停**：客户端在不在跑、进程是不是我们启动的、live 属于哪个账号——
//!    任何一项判断不出来，都直接拒绝，不写 live 的任何一个字节。
//! 2. **先备份再覆盖**：切换走 `switch.rs` 的文件组事务（备份 → 暂存 → 校验 → 提交），
//!    中途失败或进程崩溃都能按备份恢复。
//! 3. **切换默认关闭**：`main.sqlite` / `Local State` / Cookies 里还有账号绑定数据，
//!    只切两个认证文件**不等于**完整 Profile 隔离（见 SPIKE §7）。
//!    默认只开放扫描、导入、认证检查、保存快照；切换要显式打开工程开关。
//!
//! 关于"保存快照"（capture）：它只写 AICC 自己的快照目录，不碰 live；
//! 但仍然要求能证明"当前 live 就是这个 Profile"，否则拒绝覆盖已有快照。

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::providers::qoder;
use crate::providers::qoder_edition::{self, QoderEdition};
use crate::providers::qoder_global;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::qoder_profiles::{ProfileInsert, ProfilePatch, ProfileRecord, RuntimeRecord};

use super::process::{self, Liveness, Running};
use super::switch;

/// 工程开关：**默认关闭**。只有在"认证快照 ≠ 完整 Profile 隔离"这件事被解决、
/// 并重新做过真机验证之后，才允许把它打开。
pub const SWITCH_ENV: &str = "AICC_QODER_ENABLE_SWITCH";

/// 启动后等客户端就绪的窗口。
const VERIFY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
/// 正常退出的等待窗口。
const TERM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);
/// 启动后等客户端把 live 认证写出来的窗口。
const AUTH_SETTLE: std::time::Duration = std::time::Duration::from_millis(1500);

// ---------------------------------------------------------------------------
// 参数与校验
// ---------------------------------------------------------------------------

fn param_str<'a>(params: &'a Value, key: &str) -> Option<&'a str> {
    params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

fn param_bool(params: &Value, key: &str) -> Option<bool> {
    params.get(key).and_then(Value::as_bool)
}

fn parse_edition(params: &Value) -> RpcResult<QoderEdition> {
    let raw = param_str(params, "edition").ok_or_else(|| {
        RpcError::invalid_params("必须显式指定 edition（qoder_cn / qoder_global）")
    })?;
    QoderEdition::parse(raw).ok_or_else(|| {
        RpcError::invalid_params(format!(
            "edition 只支持 qoder_cn / qoder_global，收到 {raw}"
        ))
    })
}

fn valid_profile_id(id: &str) -> bool {
    let Some(rest) = id.strip_prefix("qp_") else {
        return false;
    };
    rest.len() == 12 && rest.chars().all(|c| c.is_ascii_hexdigit())
}

fn require_profile(app: &App, id: &str) -> RpcResult<ProfileRecord> {
    if !valid_profile_id(id) {
        return Err(RpcError::invalid_params(format!("profileId 非法：{id}")));
    }
    app.with_db(|db| db.get_profile(id))?
        .ok_or_else(|| RpcError::not_found(format!("profile 不存在：{id}")))
}

fn new_profile_id() -> String {
    let digest = Sha256::digest(uuid::Uuid::new_v4().as_bytes());
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("qp_{hex}")
}

fn profiles_root(app: &App) -> PathBuf {
    app.paths.data_dir.join("qoder").join("profiles")
}

fn snapshot_dir(app: &App, profile_id: &str) -> PathBuf {
    profiles_root(app).join(profile_id)
}

fn is_inside_managed_root(app: &App, dir: &Path) -> bool {
    let Ok(root) = profiles_root(app).canonicalize() else {
        return false;
    };
    match dir.canonicalize() {
        Ok(target) => target.starts_with(&root),
        Err(_) => false,
    }
}

fn edition_of(profile: &ProfileRecord) -> RpcResult<QoderEdition> {
    QoderEdition::parse(&profile.edition)
        .ok_or_else(|| RpcError::internal(format!("profile edition 非法：{}", profile.edition)))
}

// ---------------------------------------------------------------------------
// 切换能力闸门（P0-4 / P0-5）
// ---------------------------------------------------------------------------

/// 这一版能不能做真实的快照切换。
///
/// 两个条件必须同时满足：
/// 1. 该版本在本机做过认证+切换验证（CN 目前没有）；
/// 2. 显式打开了工程开关（因为 2 个文件的快照无法隔离 main.sqlite 里的账号数据）。
pub fn switch_capability(edition: QoderEdition) -> (bool, &'static str) {
    if !edition.verified_on_this_machine() {
        return (
            false,
            "该版本未在本机完成认证与切换验证（CN 客户端未安装），不开放快照写操作",
        );
    }
    match std::env::var(SWITCH_ENV).ok().as_deref() {
        Some("1") => (true, ""),
        _ => (
            false,
            "认证快照不等于完整 Profile 隔离：main.sqlite / Local State / Cookies 仍属同一账号体系；\
             切换默认关闭，仅开放扫描、导入、认证检查与保存快照",
        ),
    }
}

// ---------------------------------------------------------------------------
// 文件层
// ---------------------------------------------------------------------------

fn auth_path_in(dir: &Path) -> PathBuf {
    dir.join(qoder::AUTH_FILE)
}

fn live_has_auth(profile: &ProfileRecord) -> bool {
    auth_path_in(Path::new(&profile.live_dir)).is_file()
}

/// 当前 live 认证属于哪个 Profile（按快照整文件哈希）。判不出来返回 None。
fn profile_owning_live(
    app: &App,
    edition: QoderEdition,
    live: &Path,
) -> RpcResult<Option<ProfileRecord>> {
    let Some(hash) = switch::file_sha256(&auth_path_in(live)) else {
        return Ok(None);
    };
    let profiles = app.with_db(|db| db.list_profiles(Some(edition.as_str())))?;
    for profile in profiles {
        let snapshot = Path::new(&profile.snapshot_dir);
        if switch::file_sha256(&auth_path_in(snapshot)).as_deref() == Some(hash.as_str()) {
            return Ok(Some(profile));
        }
    }
    Ok(None)
}

fn client_email(edition: QoderEdition) -> Option<String> {
    qoder_edition::read_client_status(edition).and_then(|s| s.email)
}

// ---------------------------------------------------------------------------
// 认证检查（只读快照）
// ---------------------------------------------------------------------------

fn inspect_snapshot(edition: QoderEdition, snapshot: &Path) -> Value {
    match edition {
        QoderEdition::Cn => qoder::inspect_dir(snapshot),
        QoderEdition::Global => qoder_global::inspect_dir(snapshot),
    }
}

fn classify_status(report: &Value) -> (&'static str, &'static str) {
    let state = report.get("state").and_then(Value::as_str).unwrap_or("");
    match state {
        "authenticated" => {
            let expired = report
                .get("expiresAt")
                .and_then(Value::as_str)
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                .map(|dt| dt < chrono::Utc::now())
                .unwrap_or(false);
            if expired {
                ("expired", "令牌已过期")
            } else {
                ("valid", "认证有效")
            }
        }
        "missing_auth_file" | "missing_auth_dir" => ("missing", "快照里没有认证文件"),
        "keychain_denied" => ("denied", "钥匙串授权被拒绝"),
        "keychain_unavailable" => ("unknown", "钥匙串条目不可用"),
        "token_missing" => ("invalid", "认证文件里没有令牌"),
        "unsupported_format" | "decrypt_failed" => ("invalid", "认证格式无法解析"),
        "ambiguous_auth_dir" => ("unknown", "存在多个候选目录"),
        _ => ("unknown", "认证状态未知"),
    }
}

// ---------------------------------------------------------------------------
// 运行状态
// ---------------------------------------------------------------------------

fn executable_for(profile: &ProfileRecord) -> RpcResult<PathBuf> {
    let edition = edition_of(profile)?;
    let app_path = profile
        .app_path
        .clone()
        .or_else(|| qoder_edition::find_app(edition).map(|a| a.app_path))
        .ok_or_else(|| {
            RpcError::not_found(format!(
                "{} 的客户端路径未知，请先安装对应版本",
                profile.display_name
            ))
        })?;

    let as_path = PathBuf::from(&app_path);
    if as_path.extension().map(|e| e == "app").unwrap_or(false) {
        let plist = qoder_edition::read_info_plist(&as_path).ok_or_else(|| {
            RpcError::invalid_params(format!("读不出来 {} 的 Info.plist", as_path.display()))
        })?;
        if plist.bundle_id.as_deref() != Some(edition.bundle_id()) {
            return Err(RpcError::invalid_params(format!(
                "客户端 {} 的 Bundle ID 是 {}，与 Profile 版本 {} 要求的 {} 不一致，拒绝启动",
                as_path.display(),
                plist.bundle_id.unwrap_or_else(|| "未知".into()),
                edition.as_str(),
                edition.bundle_id()
            )));
        }
    }
    qoder_edition::resolve_executable(&app_path).map_err(RpcError::invalid_params)
}

fn runtime_state(app: &App, profile: &ProfileRecord) -> RpcResult<Value> {
    let record = app.with_db(|db| db.get_runtime(&profile.profile_id))?;
    runtime_state_with(app, profile, record)
}

/// 核实运行状态。**只有活着且有证据说明"是我们这个 Profile"才算 running。**
fn runtime_state_with(
    app: &App,
    profile: &ProfileRecord,
    record: Option<RuntimeRecord>,
) -> RpcResult<Value> {
    let Some(record) = record else {
        return Ok(json!({ "state": "stopped", "pid": Value::Null }));
    };
    match process::describe(record.pid) {
        Some(info) => {
            if process::owned_by(&info, &profile.profile_id) {
                Ok(json!({
                    "state": "running",
                    "pid": record.pid,
                    "launchedAt": record.launched_at,
                    "verified": true,
                }))
            } else {
                app.with_db(|db| db.clear_runtime(&profile.profile_id))?;
                Ok(json!({
                    "state": "unknown",
                    "pid": record.pid,
                    "verified": false,
                    "reason": "进程还活着，但命令行里没有本 Profile 的归属标记",
                }))
            }
        }
        None => match process::liveness(record.pid) {
            Liveness::Gone => {
                app.with_db(|db| db.clear_runtime(&profile.profile_id))?;
                Ok(json!({ "state": "stopped", "pid": Value::Null, "reason": "进程已退出" }))
            }
            Liveness::Alive | Liveness::Unknown(_) => Ok(json!({
                "state": "unknown",
                "pid": record.pid,
                "verified": false,
                "reason": "读不到命令行，无法确认这个 PID 还是不是我们的客户端",
            })),
        },
    }
}

fn profile_json(app: &App, profile: &ProfileRecord) -> RpcResult<Value> {
    let mut value = profile.to_json();
    let runtime = runtime_state(app, profile)?;
    let live = Path::new(&profile.live_dir);
    let snapshot = Path::new(&profile.snapshot_dir);
    let live_hash = switch::file_sha256(&auth_path_in(live));
    let snapshot_hash = switch::file_sha256(&auth_path_in(snapshot));
    let active = live_hash.is_some() && live_hash == snapshot_hash;
    let (switching, reason) = edition_of(profile)
        .map(switch_capability)
        .unwrap_or((false, "版本未知"));
    value["runtime"] = runtime;
    value["active"] = json!(active);
    value["snapshotReady"] = json!(snapshot_hash.is_some());
    value["snapshotHash"] = json!(snapshot_hash);
    value["switchEnabled"] = json!(switching);
    value["switchBlockedReason"] = json!(if switching {
        Value::Null
    } else {
        json!(reason)
    });
    value["editionName"] = json!(edition_of(profile)
        .map(|e| e.display_name())
        .unwrap_or("Qoder"));
    Ok(value)
}

// ---------------------------------------------------------------------------
// 保存快照（只写快照目录，不碰 live）
// ---------------------------------------------------------------------------

/// 把 live 的认证文件组存进快照。
///
/// 覆盖已有快照前必须能证明"live 现在就是这个 Profile"：
/// 邮箱一致，或 live 与快照逐字节一致。证明不了就拒绝。
fn capture_into(app: &App, profile: &ProfileRecord) -> RpcResult<Value> {
    let live = PathBuf::from(&profile.live_dir);
    let snapshot = PathBuf::from(&profile.snapshot_dir);
    let edition = edition_of(profile)?;
    let live_auth = auth_path_in(&live);
    if !live_auth.is_file() {
        return Ok(json!({ "captured": false, "reason": "live 目录里没有 auth.v1.dat" }));
    }

    let live_email = client_email(edition);
    let live_hash = switch::file_sha256(&live_auth);
    let snapshot_hash = switch::file_sha256(&auth_path_in(&snapshot));
    let has_snapshot = snapshot_hash.is_some();

    let identity_proven = match (profile.email.as_deref(), live_email.as_deref()) {
        (Some(a), Some(b)) => a == b,
        _ => live_hash.is_some() && live_hash == snapshot_hash,
    };
    if has_snapshot && !identity_proven {
        return Ok(json!({
            "captured": false,
            "identityMismatch": true,
            "reason": "无法证明 live 当前登录的就是这个 Profile，拒绝覆盖已有快照",
        }));
    }

    // 先写临时文件，两个文件都成功后再一起 rename，避免留下半个快照。
    std::fs::create_dir_all(&snapshot)
        .map_err(|e| RpcError::internal(format!("创建快照目录失败：{e}")))?;
    for name in switch::GROUP_FILES {
        let src = live.join(name);
        let staged = snapshot.join(format!(".{name}.tmp"));
        if src.is_file() {
            std::fs::copy(&src, &staged)
                .map_err(|e| RpcError::internal(format!("复制 {name} 到快照失败：{e}")))?;
            if switch::file_sha256(&staged) != switch::file_sha256(&src) {
                let _ = std::fs::remove_file(&staged);
                return Err(RpcError::internal(format!("快照 {name} 复制后校验失败")));
            }
        }
    }
    for name in switch::GROUP_FILES {
        let staged = snapshot.join(format!(".{name}.tmp"));
        if staged.is_file() {
            std::fs::rename(&staged, snapshot.join(name))
                .map_err(|e| RpcError::internal(format!("落盘快照 {name} 失败：{e}")))?;
        }
    }

    let new_hash = switch::file_sha256(&auth_path_in(&snapshot));
    let patch = ProfilePatch {
        email: profile.email.clone().or(live_email),
        snapshot_hash: new_hash,
        auth_status: Some(if profile.auth_status == "valid" {
            "valid".to_string()
        } else {
            "unknown".to_string()
        }),
        ..Default::default()
    };
    let updated = app.with_db(|db| db.update_profile(&profile.profile_id, &patch))?;
    Ok(json!({ "captured": true, "profile": updated.to_json() }))
}

// ---------------------------------------------------------------------------
// RPC 入口
// ---------------------------------------------------------------------------

pub fn editions_detect() -> Value {
    let editions: Vec<Value> = QoderEdition::ALL
        .iter()
        .map(|edition| {
            let mut value =
                serde_json::to_value(qoder_edition::detect(*edition)).unwrap_or(Value::Null);
            let status = qoder_edition::read_client_status(*edition);
            value["loggedInEmail"] = json!(status.as_ref().and_then(|s| s.email.clone()));
            value["loggedIn"] = json!(status.as_ref().map(|s| s.logged_in).unwrap_or(false));
            // 快照切换模型：两版都不支持并行实例；CN 的取值是"未复验"，不是"已确认不支持"。
            value["parallelInstances"] = json!(edition.parallel_instances_label());
            value["switchMode"] = json!("snapshot");
            let (enabled, reason) = switch_capability(*edition);
            value["switchEnabled"] = json!(enabled);
            value["switchBlockedReason"] = json!(if enabled { Value::Null } else { json!(reason) });
            value
        })
        .collect();
    json!({ "editions": editions })
}

pub fn list(app: &App, params: &Value) -> RpcResult<Value> {
    let edition = match param_str(params, "edition") {
        Some(raw) => Some(
            QoderEdition::parse(raw)
                .ok_or_else(|| RpcError::invalid_params("edition 只支持 qoder_cn / qoder_global"))?
                .as_str(),
        ),
        None => None,
    };
    // 有未完成切换就先按备份恢复（没有 journal 时零写入）。
    let recovered = switch::recover_pending(app, edition)?;
    let profiles = app.with_db(|db| db.list_profiles(edition))?;
    let mut out = Vec::with_capacity(profiles.len());
    for profile in &profiles {
        out.push(profile_json(app, profile)?);
    }
    let active = out
        .iter()
        .find(|p| p["active"] == json!(true))
        .and_then(|p| p["profileId"].as_str())
        .map(str::to_string);
    Ok(json!({
        "profiles": out,
        "count": out.len(),
        "activeProfileId": active,
        "recovered": recovered,
    }))
}

/// 扫描：把客户端当前登录的账号登记成 Profile（复制一份快照，不动 live）。
pub fn scan(app: &App, params: &Value) -> RpcResult<Value> {
    let only = param_str(params, "edition")
        .map(|raw| {
            QoderEdition::parse(raw)
                .ok_or_else(|| RpcError::invalid_params("edition 只支持 qoder_cn / qoder_global"))
        })
        .transpose()?;

    let mut created = Vec::new();
    let mut skipped = Vec::new();
    let mut without_client = Vec::new();

    for edition in QoderEdition::ALL {
        if let Some(only) = only {
            if only != edition {
                continue;
            }
        }
        let detection = qoder_edition::detect(edition);
        if !detection.installed {
            without_client.push(edition.as_str());
        }
        let Some(live) = qoder_edition::find_data_dirs(edition).into_iter().next() else {
            continue;
        };
        if !auth_path_in(&live).is_file() {
            skipped.push(json!({
                "edition": edition.as_str(),
                "reason": "no_auth_in_live_dir",
                "liveDir": live.to_string_lossy(),
            }));
            continue;
        }
        if let Some(existing) = profile_owning_live(app, edition, &live)? {
            skipped.push(json!({
                "edition": edition.as_str(),
                "reason": "already_registered",
                "profileId": existing.profile_id,
            }));
            continue;
        }

        let profile_id = new_profile_id();
        let snapshot = snapshot_dir(app, &profile_id);
        std::fs::create_dir_all(&snapshot)
            .map_err(|e| RpcError::internal(format!("创建快照目录失败：{e}")))?;
        let display_name =
            client_email(edition).unwrap_or_else(|| edition.display_name().to_string());
        let insert = ProfileInsert {
            profile_id: profile_id.clone(),
            edition: edition.as_str().to_string(),
            display_name,
            live_dir: live.to_string_lossy().to_string(),
            snapshot_dir: snapshot.to_string_lossy().to_string(),
            app_bundle_id: Some(detection.bundle_id.to_string()),
            app_path: detection.app_path.clone(),
            client_version: detection.client_version.clone(),
            auth_file: Some(auth_path_in(&snapshot).to_string_lossy().to_string()),
            snapshot_hash: None,
            auth_status: "unknown".to_string(),
            managed: true,
        };
        let record = app.with_db(|db| db.insert_profile(&insert))?;
        let captured = capture_into(app, &record)?;
        let refreshed = require_profile(app, &profile_id)?;
        created.push(json!({
            "profile": profile_json(app, &refreshed)?,
            "captured": captured["captured"],
        }));
    }

    Ok(json!({
        "created": created,
        "skipped": skipped,
        "editionsWithoutClient": without_client,
    }))
}

pub fn create(app: &App, params: &Value) -> RpcResult<Value> {
    let edition = parse_edition(params)?;
    let detection = qoder_edition::detect(edition);
    let Some(live) = qoder_edition::find_data_dirs(edition).into_iter().next() else {
        return Err(RpcError::not_found(format!(
            "没有找到 {} 的客户端数据目录，请先安装并启动一次客户端",
            edition.display_name()
        )));
    };

    let profile_id = new_profile_id();
    let snapshot = snapshot_dir(app, &profile_id);
    std::fs::create_dir_all(&snapshot)
        .map_err(|e| RpcError::internal(format!("创建快照目录失败：{e}")))?;

    let display_name = param_str(params, "displayName")
        .map(str::to_string)
        .unwrap_or_else(|| edition.display_name().to_string());
    let insert = ProfileInsert {
        profile_id: profile_id.clone(),
        edition: edition.as_str().to_string(),
        display_name,
        live_dir: live.to_string_lossy().to_string(),
        snapshot_dir: snapshot.to_string_lossy().to_string(),
        app_bundle_id: Some(detection.bundle_id.to_string()),
        app_path: detection.app_path.clone(),
        client_version: detection.client_version.clone(),
        auth_file: Some(auth_path_in(&snapshot).to_string_lossy().to_string()),
        snapshot_hash: None,
        auth_status: "missing".to_string(),
        managed: true,
    };
    let record = app.with_db(|db| db.insert_profile(&insert))?;
    let (enabled, reason) = switch_capability(edition);
    Ok(json!({
        "profile": profile_json(app, &record)?,
        "switchEnabled": enabled,
        "switchBlockedReason": if enabled { Value::Null } else { json!(reason) },
        "hint": "Profile 已创建但还没有认证快照：先进官方客户端登录，再点「保存当前登录」。",
    }))
}

pub fn import(app: &App, params: &Value) -> RpcResult<Value> {
    let edition = parse_edition(params)?;
    let raw = param_str(params, "snapshotDir")
        .or_else(|| param_str(params, "userDataDir"))
        .ok_or_else(|| RpcError::invalid_params("必须提供 snapshotDir（含 auth.v1.dat 的目录）"))?;
    let source = PathBuf::from(raw);
    if !source.is_absolute() {
        return Err(RpcError::invalid_params(format!(
            "snapshotDir 必须是绝对路径：{raw}"
        )));
    }
    if raw.split('/').any(|part| part == "..") {
        return Err(RpcError::invalid_params(
            "snapshotDir 不允许包含 .. 路径片段",
        ));
    }
    if !source.is_dir() {
        return Err(RpcError::invalid_params(format!("目录不存在：{raw}")));
    }
    if !auth_path_in(&source).is_file() {
        return Err(RpcError::invalid_params(
            "该目录里没有 auth.v1.dat，不能当作账号快照导入",
        ));
    }
    if app
        .with_db(|db| db.find_profile_by_snapshot_dir(raw))?
        .is_some()
    {
        return Err(RpcError::invalid_params("该目录已登记为 Profile"));
    }
    let Some(live) = qoder_edition::find_data_dirs(edition).into_iter().next() else {
        return Err(RpcError::not_found(format!(
            "没有找到 {} 的客户端数据目录，请先安装并启动一次客户端",
            edition.display_name()
        )));
    };
    let detection = qoder_edition::detect(edition);
    let display_name = param_str(params, "displayName")
        .map(str::to_string)
        .unwrap_or_else(|| edition.display_name().to_string());

    let insert = ProfileInsert {
        profile_id: new_profile_id(),
        edition: edition.as_str().to_string(),
        display_name,
        live_dir: live.to_string_lossy().to_string(),
        snapshot_dir: source.to_string_lossy().to_string(),
        app_bundle_id: Some(detection.bundle_id.to_string()),
        app_path: detection.app_path.clone(),
        client_version: detection.client_version.clone(),
        auth_file: Some(auth_path_in(&source).to_string_lossy().to_string()),
        snapshot_hash: switch::file_sha256(&auth_path_in(&source)),
        auth_status: "unknown".to_string(),
        managed: false,
    };
    let record = app.with_db(|db| db.insert_profile(&insert))?;
    Ok(json!({ "profile": profile_json(app, &record)? }))
}

pub fn inspect(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let profile = require_profile(app, id)?;
    let edition = edition_of(&profile)?;
    let snapshot = PathBuf::from(&profile.snapshot_dir);
    let report = inspect_snapshot(edition, &snapshot);
    let (status, summary) = classify_status(&report);

    let mut conflict: Option<String> = None;
    let uid = report
        .get("uid")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(uid) = &uid {
        if let Some(other) = app.with_db(|db| db.find_profile_by_identity(edition.as_str(), uid))? {
            if other.profile_id != profile.profile_id {
                conflict = Some(other.profile_id);
            }
        }
    }

    let patch = ProfilePatch {
        auth_status: Some(status.to_string()),
        auth_fingerprint: report
            .get("tokenFingerprint")
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_at: report
            .get("expiresAt")
            .and_then(Value::as_str)
            .map(str::to_string),
        account_uid: if conflict.is_none() { uid } else { None },
        email: report
            .get("email")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or(profile.email.clone()),
        ..Default::default()
    };
    let updated = app.with_db(|db| db.update_profile(&profile.profile_id, &patch))?;

    Ok(json!({
        "profile": profile_json(app, &updated)?,
        "report": report,
        "summary": summary,
        "identityConflict": conflict,
    }))
}

pub fn rename(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let name = param_str(params, "displayName")
        .ok_or_else(|| RpcError::invalid_params("必须提供 displayName"))?;
    if name.chars().count() > 60 {
        return Err(RpcError::invalid_params("displayName 最多 60 个字符"));
    }
    require_profile(app, id)?;
    let updated = app.with_db(|db| {
        db.update_profile(
            id,
            &ProfilePatch {
                display_name: Some(name.to_string()),
                ..Default::default()
            },
        )
    })?;
    Ok(json!({ "profile": profile_json(app, &updated)? }))
}

/// 保存当前客户端登录态到该 Profile 的快照（只写快照，不碰 live）。
pub fn capture(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let profile = require_profile(app, id)?;
    capture_into(app, &profile)
}

// ---------------------------------------------------------------------------
// 切换（默认关闭）
// ---------------------------------------------------------------------------

fn refuse_switch(edition: QoderEdition) -> RpcResult<()> {
    let (enabled, reason) = switch_capability(edition);
    if enabled {
        return Ok(());
    }
    Err(RpcError::invalid_params(format!(
        "{} 的快照切换当前不可用：{reason}",
        edition.display_name()
    ))
    .with_data(json!({ "switchEnabled": false, "reason": reason })))
}

/// 切换前的进程闸门：只有"确认没在跑"或"跑的是本 Profile 且已退出"才继续。
fn ensure_client_stopped(
    app: &App,
    profile: &ProfileRecord,
    exe: &Path,
) -> RpcResult<Option<Value>> {
    match process::probe(exe) {
        Running::Stopped => Ok(None),
        Running::Unknown(reason) => Err(RpcError::internal(format!(
            "无法确认客户端是否在运行（{reason}），为安全起见不修改认证文件"
        ))),
        Running::Running(pids) => {
            let record = app.with_db(|db| db.get_runtime(&profile.profile_id))?;
            let Some(record) = record.filter(|r| pids.contains(&r.pid)) else {
                return Err(RpcError::invalid_params(format!(
                    "{} 正在运行（PID {}），但不是 AICC 为这个 Profile 启动的。请先手动退出客户端。",
                    edition_of(profile)?.display_name(),
                    pids.iter().map(i64::to_string).collect::<Vec<_>>().join("、")
                )));
            };
            match process::describe(record.pid) {
                Some(info) if process::owned_by(&info, &profile.profile_id) => {
                    let closed = close_owned(app, profile, exe, false)?;
                    Ok(Some(closed))
                }
                _ => Err(RpcError::invalid_params(format!(
                    "PID {} 存活但无法确认归属（缺少 {} 标记），不会发信号；请手动退出客户端",
                    record.pid,
                    process::OWNER_ENV
                ))),
            }
        }
    }
}

/// 打开（切换到）指定 Profile。**默认被闸门挡住**，见 `SWITCH_ENV`。
pub fn open(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let profile = require_profile(app, id)?;
    let edition = edition_of(&profile)?;

    // 0. 闸门：未验证/未开启时，一个字节都不动，也不去关客户端。
    refuse_switch(edition)?;

    let exe = executable_for(&profile)?;
    let live = PathBuf::from(&profile.live_dir);
    if !live.is_dir() {
        return Err(RpcError::not_found(format!(
            "客户端数据目录不存在：{}",
            live.display()
        )));
    }

    // 1. 先把上次崩溃留下的未完成切换恢复掉。
    let recovered = switch::recover_pending(app, Some(edition.as_str()))?;

    // 2. 进程闸门。
    if let Some(closed) = ensure_client_stopped(app, &profile, &exe)? {
        if closed["state"] == "unknown" {
            return Err(RpcError::internal(
                "关闭上一个实例时无法确认它是否真的退出，已停止切换",
            ));
        }
    }

    // 3. live 归属：属于别的 Profile 就必须先成功保存，失败即中止。
    let saved_into: Option<String> = if live_has_auth(&profile) {
        match profile_owning_live(app, edition, &live)? {
            Some(owner) if owner.profile_id != profile.profile_id => {
                match capture_into(app, &owner) {
                    Ok(result) if result["captured"] == json!(true) => Some(owner.profile_id),
                    Ok(result) => {
                        return Err(RpcError::internal(format!(
                            "当前 live 属于 {}，但保存失败（{}），已停止切换，live 保持原样",
                            owner.display_name,
                            result["reason"].as_str().unwrap_or("未知原因")
                        )))
                    }
                    Err(err) => {
                        return Err(RpcError::internal(format!(
                            "当前 live 属于 {}，保存失败（{}），已停止切换，live 保持原样",
                            owner.display_name, err.message
                        )))
                    }
                }
            }
            _ => None,
        }
    } else {
        None
    };

    // 4. 文件组事务：备份 → 暂存 → 校验 → 提交。
    let snapshot = PathBuf::from(&profile.snapshot_dir);
    let snapshot_ready = auth_path_in(&snapshot).is_file();
    if snapshot_ready && profile.snapshot_hash.is_none() {
        return Err(RpcError::invalid_params(
            "该 Profile 的快照没有完整性记录（snapshotHash），请先「保存当前登录」重建快照",
        ));
    }

    let prepared = switch::begin(app, &profile)?;
    let commit_result = (|| -> RpcResult<()> {
        if snapshot_ready {
            switch::stage(app, &prepared, &snapshot, profile.snapshot_hash.as_deref())?;
        } else {
            // 目标没有快照 = 要登录新账号：把 live 那组文件挪进备份，客户端会进登录页。
            switch::clear_live(app, &prepared)?;
        }

        // 提交前再确认一次：切换窗口内客户端不能被外部拉起来。
        match process::probe(&exe) {
            Running::Stopped => {}
            Running::Running(pids) => {
                return Err(RpcError::invalid_params(format!(
                    "切换过程中检测到客户端被外部启动（PID {}），已回滚",
                    pids.iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join("、")
                )))
            }
            Running::Unknown(reason) => {
                return Err(RpcError::internal(format!(
                    "提交前无法确认客户端状态（{reason}），已回滚"
                )))
            }
        }

        if snapshot_ready {
            switch::commit(app, &prepared, &snapshot)?;
        }
        Ok(())
    })();

    if let Err(err) = commit_result {
        let _ = switch::rollback(app, &prepared.journal);
        return Err(err);
    }

    // 5. 启动 + 核实。
    let pid = process::spawn(&exe, &[], &profile.profile_id)
        .map_err(|e| RpcError::internal(format!("启动 {} 失败：{e}", edition.display_name())))?;
    let deadline = std::time::Instant::now() + VERIFY_TIMEOUT;
    let mut alive = false;
    while std::time::Instant::now() < deadline {
        match process::liveness(i64::from(pid)) {
            Liveness::Alive => {
                alive = true;
                break;
            }
            Liveness::Gone => break,
            Liveness::Unknown(_) => {
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
    }
    if alive {
        std::thread::sleep(AUTH_SETTLE);
    }

    let live_hash = switch::file_sha256(&auth_path_in(&live));
    let snapshot_hash = switch::file_sha256(&auth_path_in(&snapshot));
    let authenticated = live_hash.is_some() && live_hash == snapshot_hash;
    let verified = alive
        && process::describe(i64::from(pid))
            .map(|info| process::owned_by(&info, &profile.profile_id))
            .unwrap_or(false);

    let now = crate::clock::now_rfc3339();
    app.with_db(|db| {
        db.set_runtime(&RuntimeRecord {
            profile_id: profile.profile_id.clone(),
            pid: i64::from(pid),
            executable: exe.to_string_lossy().to_string(),
            launched_at: now.clone(),
        })?;
        db.update_profile(
            &profile.profile_id,
            &ProfilePatch {
                last_opened_at: Some(now),
                app_path: Some(exe.to_string_lossy().to_string()),
                auth_status: Some(if authenticated {
                    if profile.auth_status == "valid" {
                        "valid".to_string()
                    } else {
                        "unknown".to_string()
                    }
                } else if snapshot_ready {
                    profile.auth_status.clone()
                } else {
                    "missing".to_string()
                }),
                ..Default::default()
            },
        )?;
        Ok(())
    })?;

    Ok(json!({
        "state": if alive { "running" } else { "unverified" },
        "pid": pid,
        "verified": verified,
        "authenticated": authenticated,
        "requiresLogin": !authenticated,
        "savedPreviousProfile": saved_into,
        "usedSnapshot": snapshot_ready,
        "recovered": recovered,
        "mode": "snapshot-switch",
        "reason": if alive {
            Value::Null
        } else {
            json!("进程未在超时内出现（可能被单实例锁挡回）")
        },
    }))
}

fn close_owned(app: &App, profile: &ProfileRecord, exe: &Path, force: bool) -> RpcResult<Value> {
    let Some(record) = app.with_db(|db| db.get_runtime(&profile.profile_id))? else {
        return Ok(json!({ "state": "stopped", "alreadyStopped": true }));
    };
    match process::describe(record.pid) {
        Some(info) if process::owned_by(&info, &profile.profile_id) => {}
        Some(_) => {
            app.with_db(|db| db.clear_runtime(&profile.profile_id))?;
            return Ok(json!({
                "state": "unknown",
                "alreadyStopped": false,
                "reason": "进程还活着，但命令行里没有本 Profile 的归属标记，未发信号",
            }));
        }
        None => {
            return match process::liveness(record.pid) {
                Liveness::Gone => {
                    app.with_db(|db| db.clear_runtime(&profile.profile_id))?;
                    Ok(json!({ "state": "stopped", "alreadyStopped": true }))
                }
                Liveness::Alive | Liveness::Unknown(_) => Ok(json!({
                    "state": "unknown",
                    "alreadyStopped": false,
                    "reason": "读不到命令行，无法确认归属，未发信号；请手动退出客户端",
                })),
            };
        }
    }

    process::signal(record.pid, false);
    if !process::wait_gone(record.pid, TERM_TIMEOUT) {
        if !force {
            return Err(RpcError::new(
                crate::rpc::E_STORAGE,
                format!(
                    "{} 在 {} 秒内没有拿到可靠的退出证据，需要显式确认强制结束",
                    profile.display_name,
                    TERM_TIMEOUT.as_secs()
                ),
            )
            .with_data(json!({ "pid": record.pid, "stillRunning": true })));
        }
        process::signal(record.pid, true);
        if !process::wait_gone(record.pid, std::time::Duration::from_secs(3)) {
            return Err(RpcError::internal(
                "强制结束之后仍然拿不到退出证据，已停止后续操作",
            ));
        }
    }
    app.with_db(|db| db.clear_runtime(&profile.profile_id))?;
    let _ = exe;
    Ok(json!({ "state": "stopped", "forced": force }))
}

pub fn close(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let force = param_bool(params, "force").unwrap_or(false);
    let profile = require_profile(app, id)?;
    let exe = executable_for(&profile)?;

    let stopped = close_owned(app, &profile, exe.as_path(), force)?;
    if stopped["state"] != "stopped" {
        return Ok(json!({ "close": stopped, "state": stopped["state"], "capture": Value::Null }));
    }
    // 退出后客户端可能刚刷新过令牌，趁现在存回快照（只写快照目录）。
    let captured = capture_into(app, &profile)
        .unwrap_or_else(|e| json!({ "captured": false, "reason": e.message }));
    Ok(json!({ "close": stopped, "capture": captured, "state": "stopped" }))
}

pub fn activate(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let profile = require_profile(app, id)?;
    let opened = open(app, params)?;
    Ok(json!({
        "activated": profile.profile_id,
        "edition": profile.edition,
        "opened": opened,
        "state": opened["state"],
        "mode": "snapshot-switch",
    }))
}

pub fn remove(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let purge = param_bool(params, "purge").unwrap_or(false);
    let profile = require_profile(app, id)?;

    if runtime_state(app, &profile)?["state"] == "running" {
        return Err(RpcError::invalid_params(
            "Profile 正在运行，请先关闭再删除（运行中禁止删除）",
        ));
    }

    let snapshot = PathBuf::from(&profile.snapshot_dir);
    let mut purged = false;
    if purge {
        if !is_inside_managed_root(app, &snapshot) {
            return Err(RpcError::invalid_params(format!(
                "拒绝删除 AICC 管理目录之外的快照：{}",
                snapshot.display()
            )));
        }
        if snapshot.exists() {
            std::fs::remove_dir_all(&snapshot)
                .map_err(|e| RpcError::internal(format!("删除快照失败：{e}")))?;
            purged = true;
        }
    }

    app.with_db(|db| db.clear_runtime(&profile.profile_id))?;
    let deleted = app.with_db(|db| db.delete_profile(&profile.profile_id))?;
    Ok(json!({
        "removed": deleted,
        "purged": purged,
        "snapshotDir": profile.snapshot_dir,
        "liveDirUntouched": profile.live_dir,
    }))
}

pub fn status(app: &App, params: &Value) -> RpcResult<Value> {
    let edition = param_str(params, "edition")
        .map(|raw| {
            QoderEdition::parse(raw)
                .ok_or_else(|| RpcError::invalid_params("edition 只支持 qoder_cn / qoder_global"))
        })
        .transpose()?
        .map(|e| e.as_str());
    let recovered = switch::recover_pending(app, edition)?;
    let profiles = app.with_db(|db| db.list_profiles(edition))?;
    let runtimes: std::collections::HashMap<String, RuntimeRecord> = app
        .with_db(|db| db.list_runtime())?
        .into_iter()
        .map(|record| (record.profile_id.clone(), record))
        .collect();
    let mut running = 0usize;
    let mut unknown = 0usize;
    let mut active: Option<String> = None;
    let mut items = Vec::with_capacity(profiles.len());
    for profile in &profiles {
        let state = runtime_state_with(app, profile, runtimes.get(&profile.profile_id).cloned())?;
        match state["state"].as_str() {
            Some("running") => running += 1,
            Some("unknown") => unknown += 1,
            _ => {}
        }
        let live_hash = switch::file_sha256(&auth_path_in(Path::new(&profile.live_dir)));
        let snapshot_hash = switch::file_sha256(&auth_path_in(Path::new(&profile.snapshot_dir)));
        let is_active = live_hash.is_some() && live_hash == snapshot_hash;
        if is_active {
            active = Some(profile.profile_id.clone());
        }
        items.push(json!({
            "profileId": profile.profile_id,
            "edition": profile.edition,
            "displayName": profile.display_name,
            "active": is_active,
            "runtime": state,
        }));
    }
    Ok(json!({
        "profiles": items,
        "running": running,
        "unknown": unknown,
        "activeProfileId": active,
        "recovered": recovered,
    }))
}

/// 显式恢复未完成切换（启动时/排障时调用）。没有未完成记录时零写入。
pub fn recover(app: &App, params: &Value) -> RpcResult<Value> {
    let edition = param_str(params, "edition")
        .map(|raw| {
            QoderEdition::parse(raw)
                .ok_or_else(|| RpcError::invalid_params("edition 只支持 qoder_cn / qoder_global"))
        })
        .transpose()?
        .map(|e| e.as_str());
    let reports = switch::recover_pending(app, edition)?;
    Ok(json!({ "recovered": reports, "count": reports.len() }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;
    use crate::test_env::lock as lock_env;

    const MOCK_CLIENT: &str = r#"#!/bin/sh
f="$AICC_MOCK_STATE/line.$$"
printf '%s' "$0" > "$f"
for a in "$@"; do printf ' %s' "$a" >> "$f"; done
printf ' AICC_QODER_PROFILE=%s' "$AICC_QODER_PROFILE" >> "$f"
trap 'rm -f "$f"; exit 0' TERM
i=0
while [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done
rm -f "$f"
"#;

    /// 假 ps：只回放 mock 客户端自己写下的命令行。
    const MOCK_PS: &str = r#"#!/bin/sh
pid=""
prev=""
for a in "$@"; do
  if [ "$prev" = "-p" ]; then pid="$a"; fi
  prev="$a"
done
if [ -f "$AICC_MOCK_STATE/line.$pid" ]; then cat "$AICC_MOCK_STATE/line.$pid"; echo; else exit 1; fi
"#;

    /// 假 pgrep：由 `$AICC_MOCK_STATE/mode` 决定行为。
    /// stopped → exit 1（确认没跑）；state → 回放现存实例；external → 回放外部 PID；
    /// seq → 第一次 stopped，之后 external（模拟"切换窗口里被外部启动"）。
    const MOCK_PGREP: &str = r#"#!/bin/sh
mode=$(cat "$AICC_MOCK_STATE/mode" 2>/dev/null)
[ -z "$mode" ] && mode=stopped
case "$mode" in
  stopped) exit 1;;
  external) cat "$AICC_MOCK_STATE/external.pid" 2>/dev/null; exit 0;;
  seq)
    n=$(cat "$AICC_MOCK_STATE/seq" 2>/dev/null)
    [ -z "$n" ] && n=0
    n=$((n+1)); echo "$n" > "$AICC_MOCK_STATE/seq"
    if [ "$n" -ge 2 ]; then cat "$AICC_MOCK_STATE/external.pid"; exit 0; fi
    exit 1;;
  *)
    found=0
    for f in "$AICC_MOCK_STATE"/line.*; do
      [ -f "$f" ] || continue
      basename "$f" | sed 's/^line\.//'
      found=1
    done
    [ "$found" = "1" ] || exit 1
    ;;
esac
"#;

    fn write_script(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    struct Sandbox {
        _dir: tempfile::TempDir,
        app: App,
        support: PathBuf,
        state: PathBuf,
    }

    fn sandbox() -> Sandbox {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let paths = Paths {
            data_dir: data.clone(),
            db_path: data.join("accounts.sqlite"),
            backups_dir: data.join("backups"),
            logs_dir: data.join("logs"),
            wb_switch_home: data.join("wb-switch-home"),
            wb_home_owned: true,
        };
        for d in [
            &paths.data_dir,
            &paths.backups_dir,
            &paths.logs_dir,
            &paths.wb_switch_home,
        ] {
            std::fs::create_dir_all(d).unwrap();
        }
        let support = dir.path().join("support");
        let state = dir.path().join("mock-state");
        std::fs::create_dir_all(&support).unwrap();
        std::fs::create_dir_all(&state).unwrap();
        Sandbox {
            _dir: dir,
            app: App::with_secure(paths, SecureStore::memory()),
            support,
            state,
        }
    }

    struct Fixture;

    fn install_both(sb: &Sandbox, dir: &Path) -> PathBuf {
        let apps = dir.join("apps");
        std::fs::create_dir_all(&apps).unwrap();
        for (name, bundle, version) in [
            ("Qoder CN.app", "com.qodercn.app", "9.9.9"),
            ("Qoder.app", "com.qoder.app", "0.5.7"),
        ] {
            let app = apps.join(name);
            let contents = app.join("Contents");
            std::fs::create_dir_all(contents.join("MacOS")).unwrap();
            std::fs::write(
                contents.join("Info.plist"),
                format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>{bundle}</string>
  <key>CFBundleExecutable</key><string>Qoder</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
                ),
            )
            .unwrap();
            write_script(&contents.join("MacOS").join("Qoder"), MOCK_CLIENT);
        }

        let ps = dir.join("ps");
        write_script(&ps, MOCK_PS);
        let pgrep = dir.join("pgrep");
        write_script(&pgrep, MOCK_PGREP);

        std::env::set_var(
            qoder_edition::APP_DIRS_ENV,
            apps.to_string_lossy().to_string(),
        );
        std::env::set_var(
            qoder_edition::SUPPORT_DIR_ENV,
            sb.support.to_string_lossy().to_string(),
        );
        std::env::set_var(process::PS_BIN_ENV, ps.to_string_lossy().to_string());
        std::env::set_var(process::PGREP_BIN_ENV, pgrep.to_string_lossy().to_string());
        std::env::set_var("AICC_MOCK_STATE", sb.state.to_string_lossy().to_string());
        apps
    }

    fn set_mode(sb: &Sandbox, mode: &str) {
        std::fs::write(sb.state.join("mode"), mode).unwrap();
    }

    fn set_status_json(sb: &Sandbox, email: &str) {
        let path = sb.state.join("status.json");
        std::fs::write(
            &path,
            format!(r#"{{"logged_in":true,"name":"Tester","email":"{email}"}}"#),
        )
        .unwrap();
        std::env::set_var(
            qoder_edition::STATUS_JSON_ENV,
            path.to_string_lossy().to_string(),
        );
    }

    fn seed_live(sb: &Sandbox, name: &str, auth: &str) -> PathBuf {
        let dir = sb.support.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Local State"), b"{}").unwrap();
        std::fs::write(dir.join(qoder::AUTH_FILE), auth.as_bytes()).unwrap();
        dir
    }

    fn enable_switch() {
        std::env::set_var(super::SWITCH_ENV, "1");
    }

    fn create_profile(app: &App, edition: &str, name: &str) -> Value {
        super::create(app, &json!({ "edition": edition, "displayName": name })).unwrap()
    }

    fn live_hash(dir: &Path) -> Option<String> {
        switch::file_sha256(&dir.join(qoder::AUTH_FILE))
    }

    // ---- 1. pgrep 失败：零写入 ----

    #[test]
    fn unknown_process_state_blocks_switch_without_writing() {
        let _guard = lock_env();
        enable_switch();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-AUTH");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let id = super::list(&sb.app, &json!({})).unwrap()["profiles"][0]["profileId"]
            .as_str()
            .unwrap()
            .to_string();

        // pgrep 直接不可用 → Running::Unknown。
        std::env::set_var(process::PGREP_BIN_ENV, "/nonexistent/pgrep");
        let before = live_hash(&live);
        let err = super::open(&sb.app, &json!({ "profileId": id })).unwrap_err();
        assert!(
            err.message.contains("无法确认客户端"),
            "msg={}",
            err.message
        );

        assert_eq!(live_hash(&live), before, "live 认证不允许被改动");
        assert!(sb
            .app
            .with_db(|db| db.active_switches(None))
            .unwrap()
            .is_empty());
        assert!(!super::profiles_root(&sb.app)
            .join("switch-backups")
            .exists());
    }

    // ---- 2. ps 失败但进程仍存活：禁止切换 ----

    #[test]
    fn alive_process_without_readable_cmdline_blocks_switch() {
        let _guard = lock_env();
        enable_switch();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-AUTH");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let id = super::list(&sb.app, &json!({})).unwrap()["profiles"][0]["profileId"]
            .as_str()
            .unwrap()
            .to_string();

        // pgrep 说"有一个 PID"，但 ps 读不到它的命令行；kill -0 能看到本测试进程。
        std::fs::write(
            sb.state.join("external.pid"),
            std::process::id().to_string(),
        )
        .unwrap();
        set_mode(&sb, "external");
        sb.app
            .with_db(|db| {
                db.set_runtime(&RuntimeRecord {
                    profile_id: id.clone(),
                    pid: i64::from(std::process::id()),
                    executable: "/nonexistent/Qoder".into(),
                    launched_at: "t".into(),
                })?;
                Ok(())
            })
            .unwrap();

        let before = live_hash(&live);
        let err = super::open(&sb.app, &json!({ "profileId": id })).unwrap_err();
        assert!(
            err.message.contains("无法确认归属") || err.message.contains("无法确认"),
            "msg={}",
            err.message
        );
        assert_eq!(live_hash(&live), before);
        assert!(sb
            .app
            .with_db(|db| db.active_switches(None))
            .unwrap()
            .is_empty());
    }

    // ---- 3. 捕获身份不一致：原 live 不变 ----

    #[test]
    fn capture_refuses_identity_mismatch_and_keeps_live() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let profile = super::list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let id = profile["profileId"].as_str().unwrap().to_string();
        let snapshot = PathBuf::from(profile["snapshotDir"].as_str().unwrap());
        let snapshot_before = switch::file_sha256(&snapshot.join(qoder::AUTH_FILE));

        // live 换成了 B 的账号，但 Profile 记的是 A。
        set_status_json(&sb, "b@example.com");
        std::fs::write(live.join(qoder::AUTH_FILE), b"LIVE-B").unwrap();
        let before = live_hash(&live);

        let result = super::capture(&sb.app, &json!({ "profileId": id })).unwrap();
        assert_eq!(result["captured"], false);
        assert_eq!(result["identityMismatch"], true);
        assert_eq!(live_hash(&live), before, "live 不能被改");
        assert_eq!(
            switch::file_sha256(&snapshot.join(qoder::AUTH_FILE)),
            snapshot_before,
            "已有快照不能被覆盖"
        );
    }

    // ---- 4. 捕获 I/O 失败：禁止进入 restore ----

    #[test]
    fn capture_io_failure_aborts_before_restore() {
        let _guard = lock_env();
        enable_switch();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let a = super::list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let a_id = a["profileId"].as_str().unwrap().to_string();
        let a_snapshot = PathBuf::from(a["snapshotDir"].as_str().unwrap());

        // 造一个 B Profile，并让 A 的快照目录不可写，模拟"保存当前账号失败"。
        let b = create_profile(&sb.app, "qoder_global", "B");
        let b_id = b["profile"]["profileId"].as_str().unwrap().to_string();
        let b_snapshot = PathBuf::from(b["profile"]["snapshotDir"].as_str().unwrap());
        std::fs::write(b_snapshot.join(qoder::AUTH_FILE), b"SNAP-B").unwrap();
        sb.app
            .with_db(|db| {
                db.update_profile(
                    &b_id,
                    &ProfilePatch {
                        snapshot_hash: switch::file_sha256(&b_snapshot.join(qoder::AUTH_FILE)),
                        ..Default::default()
                    },
                )?;
                Ok(())
            })
            .unwrap();

        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&a_snapshot).unwrap().permissions();
        perms.set_mode(0o500);
        std::fs::set_permissions(&a_snapshot, perms).unwrap();

        // live 仍是 A 的认证，切到 B 之前必须先把它存回 A —— 保存会失败。
        let before = live_hash(&live);
        let err = super::open(&sb.app, &json!({ "profileId": b_id })).unwrap_err();

        let mut perms = std::fs::metadata(&a_snapshot).unwrap().permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&a_snapshot, perms).unwrap();

        assert!(err.message.contains("保存失败"), "msg={}", err.message);
        assert_eq!(live_hash(&live), before, "restore 必须被禁止");
        assert_eq!(
            std::fs::read_to_string(live.join(qoder::AUTH_FILE)).unwrap(),
            "LIVE-A"
        );
        let _ = a_id;
    }

    // ---- 7. 目标快照损坏：不能覆盖 live ----

    #[test]
    fn corrupt_target_snapshot_never_overwrites_live() {
        let _guard = lock_env();
        enable_switch();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let a = super::list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let a_id = a["profileId"].as_str().unwrap().to_string();

        // 造 B 的快照，并把记录的哈希改坏（模拟快照被截断/篡改）。
        let b = create_profile(&sb.app, "qoder_global", "B");
        let b_id = b["profile"]["profileId"].as_str().unwrap().to_string();
        let b_snapshot = PathBuf::from(b["profile"]["snapshotDir"].as_str().unwrap());
        std::fs::write(b_snapshot.join(qoder::AUTH_FILE), b"SNAP-B").unwrap();
        sb.app
            .with_db(|db| {
                db.update_profile(
                    &b_id,
                    &ProfilePatch {
                        snapshot_hash: Some("0000000000".into()),
                        ..Default::default()
                    },
                )?;
                Ok(())
            })
            .unwrap();

        let before = live_hash(&live);
        let err = super::open(&sb.app, &json!({ "profileId": b_id })).unwrap_err();
        assert!(err.message.contains("损坏"), "msg={}", err.message);
        assert_eq!(live_hash(&live), before);
        assert_eq!(
            std::fs::read_to_string(live.join(qoder::AUTH_FILE)).unwrap(),
            "LIVE-A"
        );
        let _ = a_id;
    }

    // ---- 8. 未完成 journal：先恢复，再继续；互斥本身由唯一索引保证 ----

    #[test]
    fn pending_journal_is_recovered_then_switch_completes() {
        let _guard = lock_env();
        enable_switch();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let a = super::list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let a_id = a["profileId"].as_str().unwrap().to_string();

        let b = create_profile(&sb.app, "qoder_global", "B");
        let b_id = b["profile"]["profileId"].as_str().unwrap().to_string();
        let b_snapshot = PathBuf::from(b["profile"]["snapshotDir"].as_str().unwrap());
        std::fs::write(b_snapshot.join(qoder::AUTH_FILE), b"SNAP-B").unwrap();
        sb.app
            .with_db(|db| {
                db.update_profile(
                    &b_id,
                    &ProfilePatch {
                        snapshot_hash: switch::file_sha256(&b_snapshot.join(qoder::AUTH_FILE)),
                        ..Default::default()
                    },
                )?;
                Ok(())
            })
            .unwrap();

        // 模拟上一次切换崩溃：留下一条 prepared journal。
        // 同一时刻第二条 journal 会被唯一索引挡住（见 storage::qoder_switch 的测试）。
        let prepared = switch::begin(&sb.app, &require_profile(&sb.app, &a_id).unwrap()).unwrap();
        let before = live_hash(&live);

        // open 会先把崩溃残留恢复掉，再完成本次切换。
        let opened = super::open(&sb.app, &json!({ "profileId": b_id })).unwrap();
        assert_eq!(opened["state"], "running", "opened={opened}");
        assert_eq!(opened["recovered"].as_array().unwrap().len(), 1);
        assert_eq!(
            std::fs::read_to_string(live.join(qoder::AUTH_FILE)).unwrap(),
            "SNAP-B"
        );
        assert_ne!(live_hash(&live), before);
        assert!(sb
            .app
            .with_db(|db| db.active_switches(None))
            .unwrap()
            .is_empty());
        let _ = prepared;
    }

    // ---- 9. 切换窗口被外部启动：安全停止 ----

    #[test]
    fn external_start_during_switch_rolls_back() {
        let _guard = lock_env();
        enable_switch();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let a = super::list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let a_id = a["profileId"].as_str().unwrap().to_string();

        let b = create_profile(&sb.app, "qoder_global", "B");
        let b_id = b["profile"]["profileId"].as_str().unwrap().to_string();
        let b_snapshot = PathBuf::from(b["profile"]["snapshotDir"].as_str().unwrap());
        std::fs::write(b_snapshot.join(qoder::AUTH_FILE), b"SNAP-B").unwrap();
        sb.app
            .with_db(|db| {
                db.update_profile(
                    &b_id,
                    &ProfilePatch {
                        snapshot_hash: switch::file_sha256(&b_snapshot.join(qoder::AUTH_FILE)),
                        ..Default::default()
                    },
                )?;
                Ok(())
            })
            .unwrap();

        // 第一次 probe 说"没在跑"，第二次（提交前）说"有外部进程"。
        std::fs::write(
            sb.state.join("external.pid"),
            std::process::id().to_string(),
        )
        .unwrap();
        set_mode(&sb, "seq");

        let before = live_hash(&live);
        let err = super::open(&sb.app, &json!({ "profileId": b_id })).unwrap_err();
        assert!(err.message.contains("外部启动"), "msg={}", err.message);
        assert_eq!(live_hash(&live), before, "必须回滚到原账号");
        assert_eq!(
            std::fs::read_to_string(live.join(qoder::AUTH_FILE)).unwrap(),
            "LIVE-A"
        );
        assert!(sb
            .app
            .with_db(|db| db.active_switches(None))
            .unwrap()
            .is_empty());
        let _ = a_id;
    }

    // ---- 10. CN 未复验：拒绝真实切换 ----

    #[test]
    fn cn_switch_is_refused_until_verified() {
        let _guard = lock_env();
        enable_switch();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        seed_live(&sb, "com.qodercn.app", "CN-LIVE");
        set_mode(&sb, "stopped");
        let created = create_profile(&sb.app, "qoder_cn", "CN 账号");
        let id = created["profile"]["profileId"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(created["switchEnabled"], false);

        let before = live_hash(&sb.support.join("com.qodercn.app"));
        let err = super::open(&sb.app, &json!({ "profileId": id })).unwrap_err();
        assert!(
            err.message.contains("未在本机完成认证与切换验证"),
            "msg={}",
            err.message
        );
        assert_eq!(live_hash(&sb.support.join("com.qodercn.app")), before);
    }

    // ---- 其余回归 ----

    #[test]
    fn switch_is_disabled_by_default_even_for_verified_edition() {
        let _guard = lock_env();
        std::env::remove_var(super::SWITCH_ENV);
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        seed_live(&sb, "com.qoder.app.stable", "LIVE-A");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let profile = super::list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        assert_eq!(profile["switchEnabled"], false);
        let reason = profile["switchBlockedReason"].as_str().unwrap_or("");
        assert!(
            reason.contains("不等于完整 Profile 隔离"),
            "reason={reason}"
        );

        let id = profile["profileId"].as_str().unwrap().to_string();
        let err = super::open(&sb.app, &json!({ "profileId": id })).unwrap_err();
        assert!(err.message.contains("不可用"), "msg={}", err.message);
    }

    #[test]
    fn scan_captures_current_account_without_touching_live() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A");
        set_status_json(&sb, "me@example.com");

        let before = std::fs::read(live.join(qoder::AUTH_FILE)).unwrap();
        let result = super::scan(&sb.app, &json!({})).unwrap();
        assert_eq!(result["created"].as_array().unwrap().len(), 1);
        assert_eq!(result["created"][0]["captured"], true);
        assert_eq!(
            result["created"][0]["profile"]["displayName"],
            "me@example.com"
        );
        assert_eq!(std::fs::read(live.join(qoder::AUTH_FILE)).unwrap(), before);

        let profile = super::list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let snapshot = PathBuf::from(profile["snapshotDir"].as_str().unwrap());
        assert_eq!(
            std::fs::read(snapshot.join(qoder::AUTH_FILE)).unwrap(),
            before
        );
        assert_eq!(profile["snapshotReady"], true);

        // 重复扫描不重复登记。
        let again = super::scan(&sb.app, &json!({})).unwrap();
        assert!(again["created"].as_array().unwrap().is_empty());
    }

    #[test]
    fn import_rejects_missing_auth_and_traversal() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        seed_live(&sb, "com.qoder.app.stable", "LIVE-A");

        let empty = sb._dir.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        let err = super::import(
            &sb.app,
            &json!({ "edition": "qoder_global", "snapshotDir": empty.to_string_lossy() }),
        )
        .unwrap_err();
        assert!(err.message.contains("auth.v1.dat"));

        let err = super::import(
            &sb.app,
            &json!({ "edition": "qoder_global", "snapshotDir": "/tmp/../etc" }),
        )
        .unwrap_err();
        assert!(err.message.contains(".."));
    }

    #[test]
    fn purge_deletes_only_snapshot() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let profile = super::list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let id = profile["profileId"].as_str().unwrap().to_string();
        let snapshot = PathBuf::from(profile["snapshotDir"].as_str().unwrap());

        let removed = super::remove(&sb.app, &json!({ "profileId": id, "purge": true })).unwrap();
        assert_eq!(removed["purged"], true);
        assert!(!snapshot.exists());
        assert!(live.join(qoder::AUTH_FILE).is_file());
    }

    #[test]
    fn inspect_never_returns_token() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        seed_live(&sb, "com.qoder.app.stable", "v10LIVE");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let profile = super::list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let id = profile["profileId"].as_str().unwrap().to_string();
        let snapshot = PathBuf::from(profile["snapshotDir"].as_str().unwrap());
        std::fs::write(snapshot.join(qoder::AUTH_FILE), b"v99garbagegarbage").unwrap();

        let report = super::inspect(&sb.app, &json!({ "profileId": id })).unwrap();
        assert_eq!(report["profile"]["authStatus"], "invalid");
        assert!(!contains_key(&report, "token"));
    }

    #[test]
    fn status_reports_unknown_process_state_instead_of_running() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        seed_live(&sb, "com.qoder.app.stable", "LIVE-A");
        set_status_json(&sb, "a@example.com");
        super::scan(&sb.app, &json!({})).unwrap();
        let id = super::list(&sb.app, &json!({})).unwrap()["profiles"][0]["profileId"]
            .as_str()
            .unwrap()
            .to_string();

        // ps 读不到、但 kill -0 能看到本测试进程 → unknown（不是 running，也不是 stopped）。
        sb.app
            .with_db(|db| {
                db.set_runtime(&RuntimeRecord {
                    profile_id: id.clone(),
                    pid: i64::from(std::process::id()),
                    executable: "/nonexistent/Qoder".into(),
                    launched_at: "t".into(),
                })?;
                Ok(())
            })
            .unwrap();
        let state = super::status(&sb.app, &json!({})).unwrap();
        assert_eq!(state["running"], 0);
        assert_eq!(state["unknown"], 1);
        let runtime = &state["profiles"][0]["runtime"];
        assert_eq!(runtime["state"], "unknown");
        assert_eq!(runtime["verified"], false);
        sb.app.with_db(|db| db.clear_runtime(&id)).unwrap();
    }

    fn contains_key(value: &Value, needle: &str) -> bool {
        match value {
            Value::Object(map) => map
                .iter()
                .any(|(k, v)| k.to_ascii_lowercase() == needle || contains_key(v, needle)),
            Value::Array(items) => items.iter().any(|v| contains_key(v, needle)),
            _ => false,
        }
    }
}
