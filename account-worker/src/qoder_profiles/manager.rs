//! Qoder 多账号 Profile 的业务层（快照切换模型）。
//!
//! ## 为什么不是"每个账号一个 userDataDir"
//!
//! M0 实测（`docs/QODER_MULTI_PROFILE_SPIKE.md` §4）：Qoder 客户端**忽略**
//! `--user-data-dir`，一个 OS 用户只有一份 `userData`。所以并行隔离实例做不到，
//! 只能"同一时刻一个客户端 + 快照切账号"：
//!
//! ```text
//! 关闭客户端 → 把 live 认证存回它所属的 Profile
//!            → 把目标 Profile 的快照写回 live
//!            → 启动客户端 → 用文件指纹核对登录的确实是目标账号
//! ```
//!
//! 这一层保证三件事：
//! 1. **不会丢账号**：任何覆盖 live 之前，先把当前 live 存回它所属的 Profile；
//!    找不到归属就整份备份到 `profiles/.live-backup/<时间戳>/`。
//! 2. **不会串账号**：身份对不上（邮箱不一致）时宁可放弃捕获，也不覆盖快照。
//! 3. **不会误杀进程**：只对 AICC 自己拉起、且命令行核实过的 PID 发信号。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::providers::qoder;
use crate::providers::qoder_edition::{self, QoderEdition};
use crate::providers::qoder_global;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::qoder_profiles::{ProfileInsert, ProfilePatch, ProfileRecord, RuntimeRecord};

use super::process;

const VERIFY_TIMEOUT: Duration = Duration::from_secs(5);
const TERM_TIMEOUT: Duration = Duration::from_secs(8);
/// 启动后等客户端把 live 认证写出来（或确认没写）的窗口。
const AUTH_SETTLE: Duration = Duration::from_millis(1500);

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

/// Profile ID 形态固定：`qp_` + 12 位十六进制。
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

fn live_backup_root(app: &App) -> PathBuf {
    profiles_root(app).join(".live-backup")
}

/// 目录是否在 AICC 自己的 profiles 根下。**永久删除只允许碰这里**。
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
// 文件层：哈希、复制、备份、还原
// ---------------------------------------------------------------------------

/// 认证文件的整体哈希（sha256 前 8 hex）。
///
/// 用它判断"live 现在是谁"不需要钥匙串，也不接触令牌明文；
/// 它与 `auth_fingerprint`（令牌指纹）是两件事，前者比对来源，后者给用户看。
fn file_hash(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    if bytes.is_empty() {
        return None;
    }
    let digest = Sha256::digest(&bytes);
    Some(digest.iter().take(4).map(|b| format!("{b:02x}")).collect())
}

fn auth_path_in(dir: &Path) -> PathBuf {
    dir.join(qoder::AUTH_FILE)
}

fn overlays_path_in(dir: &Path) -> PathBuf {
    dir.join("auth-profile-overlays.v1.dat")
}

fn copy_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(from, to).map(|_| ())
}

/// 把 live 的认证（以及账号 overlay）复制进快照目录。
fn copy_live_into_snapshot(live: &Path, snapshot: &Path) -> std::io::Result<bool> {
    let auth = auth_path_in(live);
    if !auth.is_file() {
        return Ok(false);
    }
    std::fs::create_dir_all(snapshot)?;
    copy_file(&auth, &auth_path_in(snapshot))?;
    let overlays = overlays_path_in(live);
    if overlays.is_file() {
        copy_file(&overlays, &overlays_path_in(snapshot))?;
    }
    Ok(true)
}

/// 把快照写回 live。先写临时文件再 rename，避免客户端读到半个文件。
fn restore_snapshot(snapshot: &Path, live: &Path) -> std::io::Result<()> {
    let source = auth_path_in(snapshot);
    if !source.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "快照里没有 auth.v1.dat",
        ));
    }
    std::fs::create_dir_all(live)?;
    let staged = live.join(".aicc-auth.v1.dat.tmp");
    std::fs::copy(&source, &staged)?;
    std::fs::rename(&staged, auth_path_in(live))?;
    let overlays = overlays_path_in(snapshot);
    if overlays.is_file() {
        let staged_overlay = live.join(".aicc-overlays.tmp");
        std::fs::copy(&overlays, &staged_overlay)?;
        std::fs::rename(&staged_overlay, overlays_path_in(live))?;
    }
    Ok(())
}

/// 把 live 认证整份挪走（给"登录新账号"腾位置）。**不删除**，只移动。
fn park_live_auth(app: &App, live: &Path) -> std::io::Result<Option<PathBuf>> {
    let auth = auth_path_in(live);
    if !auth.is_file() {
        return Ok(None);
    }
    let stamp = crate::clock::now_rfc3339().replace([':', '.'], "-");
    let target = live_backup_root(app).join(stamp);
    std::fs::create_dir_all(&target)?;
    std::fs::rename(&auth, target.join(qoder::AUTH_FILE))?;
    let overlays = overlays_path_in(live);
    if overlays.is_file() {
        let _ = std::fs::rename(&overlays, target.join("auth-profile-overlays.v1.dat"));
    }
    Ok(Some(target))
}

fn live_dir_for(edition: QoderEdition) -> Option<PathBuf> {
    qoder_edition::find_data_dirs(edition).into_iter().next()
}

/// 当前 live 认证属于哪个 Profile（按文件指纹）。判不出来就返回 None。
fn profile_owning_live(
    app: &App,
    edition: QoderEdition,
    live: &Path,
) -> RpcResult<Option<ProfileRecord>> {
    let Some(hash) = file_hash(&auth_path_in(live)) else {
        return Ok(None);
    };
    let profiles = app.with_db(|db| db.list_profiles(Some(edition.as_str())))?;
    for profile in profiles {
        if file_hash(&auth_path_in(Path::new(&profile.snapshot_dir))).as_deref()
            == Some(hash.as_str())
        {
            return Ok(Some(profile));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// 认证检查
// ---------------------------------------------------------------------------

fn inspect_snapshot(edition: QoderEdition, snapshot: &Path) -> Value {
    match edition {
        QoderEdition::Cn => qoder::inspect_dir(snapshot),
        QoderEdition::Global => qoder_global::inspect_dir(snapshot),
    }
}

/// 把适配器结论翻译成 `auth_status`。拿不到会话就如实分类。
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

fn runtime_state_with(
    app: &App,
    profile: &ProfileRecord,
    record: Option<RuntimeRecord>,
) -> RpcResult<Value> {
    let Some(record) = record else {
        return Ok(json!({ "state": "stopped", "pid": Value::Null }));
    };
    let Some(info) = process::describe(record.pid) else {
        // `ps` 读不到时用 `kill -0` 兜底：活着就报"运行中（未核实命令行）"，
        // 但归属未确认，关闭时不会发信号。
        if process::is_alive(record.pid) {
            return Ok(json!({
                "state": "running",
                "pid": record.pid,
                "launchedAt": record.launched_at,
                "verified": false,
                "reason": "读不到命令行（ps 不可用），只确认进程还活着",
            }));
        }
        app.with_db(|db| db.clear_runtime(&profile.profile_id))?;
        return Ok(json!({ "state": "stopped", "pid": Value::Null, "reason": "进程已退出" }));
    };
    let exe = PathBuf::from(&record.executable);
    if !process::is_owned(&info, &exe) {
        app.with_db(|db| db.clear_runtime(&profile.profile_id))?;
        return Ok(json!({
            "state": "stopped",
            "pid": Value::Null,
            "reason": "运行记录与真实进程不匹配，已丢弃旧记录",
        }));
    }
    Ok(json!({
        "state": "running",
        "pid": record.pid,
        "launchedAt": record.launched_at,
        "verified": true,
    }))
}

fn profile_json(app: &App, profile: &ProfileRecord) -> RpcResult<Value> {
    let mut value = profile.to_json();
    let runtime = runtime_state(app, profile)?;
    let live = Path::new(&profile.live_dir);
    let snapshot = Path::new(&profile.snapshot_dir);
    let live_hash = file_hash(&auth_path_in(live));
    let snapshot_hash = file_hash(&auth_path_in(snapshot));
    // live 现在是不是这个 Profile：指纹一致即"当前登录的就是它"。
    let active = live_hash.is_some() && live_hash == snapshot_hash;
    value["runtime"] = runtime;
    value["active"] = json!(active);
    value["liveAuthHash"] = json!(live_hash);
    value["snapshotHash"] = json!(snapshot_hash);
    value["snapshotReady"] = json!(snapshot_hash.is_some());
    value["editionName"] = json!(edition_of(profile)
        .map(|e| e.display_name())
        .unwrap_or("Qoder"));
    Ok(value)
}

// ---------------------------------------------------------------------------
// 快照捕获
// ---------------------------------------------------------------------------

fn capture_into(app: &App, profile: &ProfileRecord) -> RpcResult<Value> {
    let live = PathBuf::from(&profile.live_dir);
    let snapshot = PathBuf::from(&profile.snapshot_dir);
    let edition = edition_of(profile)?;

    if !auth_path_in(&live).is_file() {
        return Ok(json!({ "captured": false, "reason": "live 目录里没有 auth.v1.dat" }));
    }

    // 身份对不上就不覆盖：宁可少存一次，也不把 A 的凭据写进 B 的快照。
    let live_email = qoder_edition::read_client_status(edition).and_then(|s| s.email);
    if let (Some(snapshot_email), Some(live_email)) =
        (profile.email.as_deref(), live_email.as_deref())
    {
        if snapshot_email != live_email {
            return Ok(json!({
                "captured": false,
                "reason": format!("live 当前登录的是 {live_email}，与 Profile 记录的 {snapshot_email} 不一致"),
                "identityMismatch": true,
            }));
        }
    }

    let copied = copy_live_into_snapshot(&live, &snapshot)
        .map_err(|e| RpcError::internal(format!("写入快照失败：{e}")))?;
    if !copied {
        return Ok(json!({ "captured": false, "reason": "live 目录里没有 auth.v1.dat" }));
    }

    let patch = ProfilePatch {
        email: profile.email.clone().or(live_email),
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
            // 快照切换模型：任何版本都不支持并行实例。
            value["parallelInstances"] = json!("no");
            value["switchMode"] = json!("snapshot");
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
    Ok(json!({ "profiles": out, "count": out.len(), "activeProfileId": active }))
}

/// 扫描本机已有的客户端配置，把"当前登录的这个账号"登记成 Profile。
///
/// **只读 live，然后复制一份快照**：不移动、不覆盖、不要求重新登录。
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
        let Some(live) = live_dir_for(edition) else {
            continue;
        };
        let live_hash = file_hash(&auth_path_in(&live));
        if live_hash.is_none() {
            skipped.push(json!({
                "edition": edition.as_str(),
                "reason": "no_auth_in_live_dir",
                "liveDir": live.to_string_lossy(),
            }));
            continue;
        }
        // 已经登记过同一个 live 账号就不再重复建。
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
        let display_name = qoder_edition::read_client_status(edition)
            .and_then(|s| s.email)
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
    let Some(live) = live_dir_for(edition) else {
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
        auth_status: "missing".to_string(),
        managed: true,
    };
    let record = app.with_db(|db| db.insert_profile(&insert))?;
    Ok(json!({
        "profile": profile_json(app, &record)?,
        "hint": "Profile 已创建但还没有认证快照：点「打开并登录」会在客户端里登录，登录后 AICC 自动存一份快照。",
    }))
}

/// 导入：现在导入的是"一份 auth.v1.dat 所在的目录"（以前备份出来的账号目录）。
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
    let Some(live) = live_dir_for(edition) else {
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

/// 保存当前客户端登录态到该 Profile 的快照。
pub fn capture(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let profile = require_profile(app, id)?;
    capture_into(app, &profile)
}

/// 打开（切换到）指定 Profile。
pub fn open(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let profile = require_profile(app, id)?;
    let edition = edition_of(&profile)?;
    let exe = executable_for(&profile)?;
    let live = PathBuf::from(&profile.live_dir);
    if !live.is_dir() {
        return Err(RpcError::not_found(format!(
            "客户端数据目录不存在：{}",
            live.display()
        )));
    }

    // 1. 客户端必须先停下来：快照切换是"换文件 + 重启"，不是并行。
    let running = process::find_running(&exe);
    if !running.is_empty() {
        let record = app.with_db(|db| db.get_runtime(&profile.profile_id))?;
        let ours = record
            .as_ref()
            .map(|r| running.contains(&r.pid))
            .unwrap_or(false);
        if ours
            && file_hash(&auth_path_in(&live))
                == file_hash(&auth_path_in(Path::new(&profile.snapshot_dir)))
        {
            return Ok(json!({
                "state": "running",
                "pid": record.map(|r| r.pid),
                "alreadyRunning": true,
                "authenticated": true,
            }));
        }
        if !ours {
            return Err(RpcError::invalid_params(format!(
                "{} 正在运行（PID {}），但不是 AICC 启动的。请先手动退出客户端再切换账号。",
                edition.display_name(),
                running
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join("、")
            )));
        }
        close_owned(app, &profile, exe.as_path(), false)?;
    }

    // 2. 先把当前 live 存回它所属的 Profile，绝不直接覆盖。
    let mut saved_into: Option<String> = None;
    if let Some(owner) = profile_owning_live(app, edition, &live)? {
        if owner.profile_id != profile.profile_id {
            let result = capture_into(app, &owner)?;
            if result["captured"] == json!(true) {
                saved_into = Some(owner.profile_id);
            }
        }
    } else if auth_path_in(&live).is_file() {
        // 归属不明：整份挪到备份，宁可多留一份也不删。
        park_live_auth(app, &live)
            .map_err(|e| RpcError::internal(format!("备份 live 认证失败：{e}")))?;
    }

    // 3. 写回目标快照；没有快照就把 live 腾空，让客户端进登录页。
    let snapshot = PathBuf::from(&profile.snapshot_dir);
    let has_snapshot = auth_path_in(&snapshot).is_file();
    if has_snapshot {
        restore_snapshot(&snapshot, &live)
            .map_err(|e| RpcError::internal(format!("写回快照失败：{e}")))?;
    } else if auth_path_in(&live).is_file() {
        park_live_auth(app, &live)
            .map_err(|e| RpcError::internal(format!("清空 live 认证失败：{e}")))?;
    }

    // 4. 启动并核实真实进程。
    let pid = process::spawn(&exe, &[], &profile.profile_id)
        .map_err(|e| RpcError::internal(format!("启动 {} 失败：{e}", edition.display_name())))?;
    let deadline = Instant::now() + VERIFY_TIMEOUT;
    let mut alive = false;
    while Instant::now() < deadline {
        if process::describe(i64::from(pid)).is_some() || process::is_alive(i64::from(pid)) {
            alive = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(150));
    }

    std::thread::sleep(AUTH_SETTLE);
    let live_hash = file_hash(&auth_path_in(&live));
    let snapshot_hash = file_hash(&auth_path_in(&snapshot));
    let authenticated = live_hash.is_some() && live_hash == snapshot_hash;

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
                // 指纹一致只证明"客户端接受了这份快照"，不等于令牌没过期；
                // 所以不冒认 valid，交给 inspect 去解一次再定。
                auth_status: Some(if authenticated {
                    if profile.auth_status == "valid" {
                        "valid".to_string()
                    } else {
                        "unknown".to_string()
                    }
                } else if has_snapshot {
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
        "verified": alive && process::describe(i64::from(pid)).is_some(),
        "authenticated": authenticated,
        "requiresLogin": !authenticated,
        "savedPreviousProfile": saved_into,
        "usedSnapshot": has_snapshot,
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
    let owned = process::describe(record.pid)
        .map(|info| process::is_owned(&info, exe))
        .unwrap_or(false);
    if !owned {
        app.with_db(|db| db.clear_runtime(&profile.profile_id))?;
        return Ok(
            json!({ "state": "stopped", "alreadyStopped": true, "reason": "进程已不在或不属于 AICC" }),
        );
    }
    process::signal(record.pid, false);
    if !process::wait_gone(record.pid, TERM_TIMEOUT) {
        if !force {
            return Err(RpcError::new(
                crate::rpc::E_STORAGE,
                format!(
                    "{} 在 {} 秒内没有退出，需要显式确认强制结束",
                    profile.display_name,
                    TERM_TIMEOUT.as_secs()
                ),
            )
            .with_data(json!({ "pid": record.pid, "stillRunning": true })));
        }
        process::signal(record.pid, true);
        let _ = process::wait_gone(record.pid, Duration::from_secs(3));
    }
    app.with_db(|db| db.clear_runtime(&profile.profile_id))?;
    Ok(json!({ "state": "stopped", "forced": force }))
}

pub fn close(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let force = param_bool(params, "force").unwrap_or(false);
    let profile = require_profile(app, id)?;
    let exe = executable_for(&profile)?;

    let stopped = close_owned(app, &profile, exe.as_path(), force)?;
    // 退出后客户端可能刚刷新过令牌，趁现在存回快照。
    let captured = capture_into(app, &profile).unwrap_or_else(|_| json!({ "captured": false }));
    Ok(json!({ "close": stopped, "capture": captured, "state": "stopped" }))
}

/// 切换 = 打开目标 Profile（open 内部已经包含"先关后写"）。
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
    let profiles = app.with_db(|db| db.list_profiles(edition))?;
    let runtimes: std::collections::HashMap<String, RuntimeRecord> = app
        .with_db(|db| db.list_runtime())?
        .into_iter()
        .map(|record| (record.profile_id.clone(), record))
        .collect();
    let mut running = 0usize;
    let mut active: Option<String> = None;
    let mut items = Vec::with_capacity(profiles.len());
    for profile in &profiles {
        let state = runtime_state_with(app, profile, runtimes.get(&profile.profile_id).cloned())?;
        if state["state"] == "running" {
            running += 1;
        }
        let live_hash = file_hash(&auth_path_in(Path::new(&profile.live_dir)));
        let snapshot_hash = file_hash(&auth_path_in(Path::new(&profile.snapshot_dir)));
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
    Ok(json!({ "profiles": items, "running": running, "activeProfileId": active }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;
    use crate::test_env::lock as lock_env;

    /// 假客户端：把真实 argv 写进 `$AICC_MOCK_STATE/<pid>`，收到 TERM 就删掉再退出。
    const MOCK_CLIENT: &str = r#"#!/bin/sh
f="$AICC_MOCK_STATE/$1"
true
"#;

    /// 更实用的假客户端：自己记 PID，写完就 sleep。
    const MOCK_CLIENT_SCRIPT: &str = r#"#!/bin/sh
f="$AICC_MOCK_STATE/$$"
printf '%s' "$0" > "$f"
for a in "$@"; do printf ' %s' "$a" >> "$f"; done
printf ' AICC_QODER_PROFILE=%s' "$AICC_QODER_PROFILE" >> "$f"
trap 'rm -f "$f"; exit 0' TERM
i=0
while [ $i -lt 200 ]; do sleep 0.05; i=$((i+1)); done
rm -f "$f"
"#;

    /// 假 ps：把该 pid 的真实命令行回放出来。
    const MOCK_PS: &str = r#"#!/bin/sh
pid=""
prev=""
for a in "$@"; do
  if [ "$prev" = "-p" ]; then pid="$a"; fi
  prev="$a"
done
if [ -f "$AICC_MOCK_STATE/$pid" ]; then cat "$AICC_MOCK_STATE/$pid"; echo; else exit 1; fi
"#;

    /// 假 pgrep：把 state 目录里现存的 PID 全部吐出来（模拟"客户端正在跑"）。
    const MOCK_PGREP: &str = r#"#!/bin/sh
for f in "$AICC_MOCK_STATE"/*; do
  [ -f "$f" ] || continue
  basename "$f"
done
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
        Sandbox {
            _dir: dir,
            app: App::with_secure(paths, SecureStore::memory()),
        }
    }

    fn fake_app(root: &Path, name: &str, bundle: &str, version: &str) -> PathBuf {
        let app = root.join(name);
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
        write_script(&contents.join("MacOS").join("Qoder"), MOCK_CLIENT_SCRIPT);
        app
    }

    struct Fixture {
        apps: PathBuf,
        support: PathBuf,
        state: PathBuf,
    }

    fn install_both(dir: &Path) -> Fixture {
        let apps = dir.join("apps");
        let support = dir.join("support");
        let state = dir.join("mock-state");
        for d in [&apps, &support, &state] {
            std::fs::create_dir_all(d).unwrap();
        }
        fake_app(&apps, "Qoder CN.app", "com.qodercn.app", "9.9.9");
        fake_app(&apps, "Qoder.app", "com.qoder.app", "0.5.7");

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
            support.to_string_lossy().to_string(),
        );
        std::env::set_var(process::PS_BIN_ENV, ps.to_string_lossy().to_string());
        std::env::set_var(process::PGREP_BIN_ENV, pgrep.to_string_lossy().to_string());
        std::env::set_var("AICC_MOCK_STATE", state.to_string_lossy().to_string());

        Fixture {
            apps,
            support,
            state,
        }
    }

    fn set_status_json(dir: &Path, email: &str) {
        let path = dir.join("status.json");
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

    fn seed_live(fixture: &Fixture, name: &str, auth: &str) -> PathBuf {
        let dir = fixture.support.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Local State"), b"{}").unwrap();
        std::fs::write(dir.join(qoder::AUTH_FILE), auth.as_bytes()).unwrap();
        dir
    }

    fn create_profile(app: &App, edition: &str, name: &str) -> Value {
        create(app, &json!({ "edition": edition, "displayName": name })).unwrap()
    }

    #[test]
    fn editions_detect_reports_snapshot_mode() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        install_both(dir.path());
        let value = editions_detect();
        let editions = value["editions"].as_array().unwrap();
        assert_eq!(editions.len(), 2);
        for edition in editions {
            assert_eq!(edition["parallelInstances"], "no");
            assert_eq!(edition["switchMode"], "snapshot");
        }
        let global = editions
            .iter()
            .find(|e| e["edition"] == "qoder_global")
            .unwrap();
        assert_eq!(global["installed"], true);
        assert_eq!(global["clientVersion"], "0.5.7");
        let cn = editions
            .iter()
            .find(|e| e["edition"] == "qoder_cn")
            .unwrap();
        assert_eq!(cn["verifiedOnThisMachine"], false);
    }

    #[test]
    fn create_makes_profile_with_empty_snapshot() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        seed_live(&fixture, "com.qoder.app.stable", "v10AAAA");
        let sb = sandbox();

        let a = create_profile(&sb.app, "qoder_global", "工作账号");
        let b = create_profile(&sb.app, "qoder_global", "测试账号");
        assert_ne!(a["profile"]["profileId"], b["profile"]["profileId"]);
        assert_ne!(a["profile"]["snapshotDir"], b["profile"]["snapshotDir"]);
        assert_eq!(a["profile"]["authStatus"], "missing");
        // 同一版本共享 live 目录，这是快照模型的预期行为。
        assert_eq!(a["profile"]["liveDir"], b["profile"]["liveDir"]);
        assert!(Path::new(a["profile"]["snapshotDir"].as_str().unwrap()).is_dir());
    }

    #[test]
    fn cross_edition_profiles_are_independent() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        seed_live(&fixture, "com.qoder.app.stable", "v10GLOBAL");
        seed_live(&fixture, "com.qodercn.app", "v10CN");
        let sb = sandbox();

        let cn = create_profile(&sb.app, "qoder_cn", "CN");
        let global = create_profile(&sb.app, "qoder_global", "GL");
        assert_eq!(cn["profile"]["edition"], "qoder_cn");
        assert_eq!(global["profile"]["edition"], "qoder_global");
        assert_ne!(cn["profile"]["liveDir"], global["profile"]["liveDir"]);
        assert_eq!(
            list(&sb.app, &json!({ "edition": "qoder_cn" })).unwrap()["count"],
            1
        );
        assert_eq!(
            list(&sb.app, &json!({ "edition": "qoder_global" })).unwrap()["count"],
            1
        );
    }

    #[test]
    fn scan_registers_current_account_without_touching_live() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        let live = seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "me@example.com");
        let sb = sandbox();

        let before = std::fs::read(live.join(qoder::AUTH_FILE)).unwrap();
        let result = scan(&sb.app, &json!({})).unwrap();
        assert_eq!(result["created"].as_array().unwrap().len(), 1);
        let created = &result["created"][0];
        assert_eq!(created["captured"], true);
        assert_eq!(created["profile"]["displayName"], "me@example.com");

        // live 原文件一个字节没动，快照里是同样内容。
        assert_eq!(std::fs::read(live.join(qoder::AUTH_FILE)).unwrap(), before);
        let snapshot = PathBuf::from(created["profile"]["snapshotDir"].as_str().unwrap());
        assert_eq!(
            std::fs::read(snapshot.join(qoder::AUTH_FILE)).unwrap(),
            before
        );

        // 再扫一次不重复登记。
        let again = scan(&sb.app, &json!({})).unwrap();
        assert!(again["created"].as_array().unwrap().is_empty());
        assert_eq!(again["skipped"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn scan_skips_edition_without_auth() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        seed_live(&fixture, "com.qoder.app.stable", "");
        std::fs::remove_file(
            fixture
                .support
                .join("com.qoder.app.stable")
                .join(qoder::AUTH_FILE),
        )
        .unwrap();
        let sb = sandbox();
        let result = scan(&sb.app, &json!({})).unwrap();
        assert!(result["created"].as_array().unwrap().is_empty());
    }

    #[test]
    fn import_rejects_traversal_and_missing_auth() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        let sb = sandbox();

        let empty = dir.path().join("empty-snapshot");
        std::fs::create_dir_all(&empty).unwrap();
        let err = import(
            &sb.app,
            &json!({ "edition": "qoder_global", "snapshotDir": empty.to_string_lossy() }),
        )
        .unwrap_err();
        assert!(err.message.contains("auth.v1.dat"), "msg={}", err.message);

        let err = import(
            &sb.app,
            &json!({ "edition": "qoder_global", "snapshotDir": "/tmp/../etc" }),
        )
        .unwrap_err();
        assert!(err.message.contains(".."));
    }

    #[test]
    fn invalid_profile_ids_are_rejected() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        install_both(dir.path());
        let sb = sandbox();
        for bad in ["../etc", "qp_xyz", "qp_123", "qp_0123456789abc"] {
            let e = inspect(&sb.app, &json!({ "profileId": bad })).unwrap_err();
            assert!(e.message.contains("非法"), "bad={bad} msg={}", e.message);
        }
    }

    #[test]
    fn open_restores_snapshot_and_verifies_real_process() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        let live = seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "me@example.com");
        let sb = sandbox();

        scan(&sb.app, &json!({})).unwrap();
        let profile = list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let id = profile["profileId"].as_str().unwrap().to_string();

        let opened = open(&sb.app, &json!({ "profileId": id })).unwrap();
        assert_eq!(opened["state"], "running", "opened={opened}");
        assert_eq!(opened["authenticated"], true);
        assert_eq!(opened["mode"], "snapshot-switch");

        // 假客户端启动后没有额外参数（快照切换不靠 --user-data-dir）。
        let pid = opened["pid"].as_i64().unwrap();
        let recorded = std::fs::read_to_string(fixture.state.join(pid.to_string())).unwrap();
        assert!(!recorded.contains("--user-data-dir"), "recorded={recorded}");

        let closed = close(&sb.app, &json!({ "profileId": id })).unwrap();
        assert_eq!(closed["state"], "stopped");
        assert_eq!(status(&sb.app, &json!({})).unwrap()["running"], 0);
        // live 文件仍在，且没被改成空。
        assert_eq!(
            std::fs::read_to_string(live.join(qoder::AUTH_FILE)).unwrap(),
            "v10LIVE"
        );
    }

    #[test]
    fn switching_restores_other_snapshot_and_saves_previous() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        let live = seed_live(&fixture, "com.qoder.app.stable", "v10ACCOUNT-A");
        set_status_json(dir.path(), "a@example.com");
        let sb = sandbox();

        scan(&sb.app, &json!({})).unwrap();
        let a = list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let a_id = a["profileId"].as_str().unwrap().to_string();
        let a_snapshot = PathBuf::from(a["snapshotDir"].as_str().unwrap());

        // 造第二个账号的 Profile：快照里直接放 B 的认证。
        let b = create_profile(&sb.app, "qoder_global", "B 账号");
        let b_id = b["profile"]["profileId"].as_str().unwrap().to_string();
        let b_snapshot = PathBuf::from(b["profile"]["snapshotDir"].as_str().unwrap());
        std::fs::write(b_snapshot.join(qoder::AUTH_FILE), b"v10ACCOUNT-B").unwrap();

        let switched = open(&sb.app, &json!({ "profileId": b_id })).unwrap();
        assert_eq!(switched["state"], "running", "switched={switched}");
        assert_eq!(switched["authenticated"], true);
        assert_eq!(
            switched["savedPreviousProfile"],
            json!(a_id),
            "切换前必须先把当前账号存回它自己的 Profile"
        );

        // live 变成 B，A 的快照保持原样。
        assert_eq!(
            std::fs::read_to_string(live.join(qoder::AUTH_FILE)).unwrap(),
            "v10ACCOUNT-B"
        );
        assert_eq!(
            std::fs::read_to_string(a_snapshot.join(qoder::AUTH_FILE)).unwrap(),
            "v10ACCOUNT-A"
        );
        let active = list(&sb.app, &json!({})).unwrap()["activeProfileId"].clone();
        assert_eq!(active, json!(b_id));

        close(&sb.app, &json!({ "profileId": b_id })).unwrap();
    }

    #[test]
    fn capture_refuses_when_identity_mismatches() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "a@example.com");
        let sb = sandbox();
        scan(&sb.app, &json!({})).unwrap();

        // live 换成了另一个账号，Profile 记的是 a@example.com → 拒绝覆盖。
        set_status_json(dir.path(), "b@example.com");
        let id = list(&sb.app, &json!({})).unwrap()["profiles"][0]["profileId"]
            .as_str()
            .unwrap()
            .to_string();
        let result = capture(&sb.app, &json!({ "profileId": id })).unwrap();
        assert_eq!(result["captured"], false);
        assert_eq!(result["identityMismatch"], true);
    }

    #[test]
    fn open_refuses_when_client_runs_outside_aicc() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "a@example.com");
        let sb = sandbox();
        scan(&sb.app, &json!({})).unwrap();
        let id = list(&sb.app, &json!({})).unwrap()["profiles"][0]["profileId"]
            .as_str()
            .unwrap()
            .to_string();

        // 假设用户手动起了一个客户端：state 目录里有它的记录，但 AICC 没有运行记录。
        std::fs::write(
            fixture.state.join("99999"),
            "/x/Qoder AICC_QODER_PROFILE=qp_other",
        )
        .unwrap();
        let err = open(&sb.app, &json!({ "profileId": id })).unwrap_err();
        assert!(err.message.contains("正在运行"), "msg={}", err.message);
        std::fs::remove_file(fixture.state.join("99999")).unwrap();
    }

    #[test]
    fn running_profile_cannot_be_removed() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "a@example.com");
        let sb = sandbox();
        scan(&sb.app, &json!({})).unwrap();
        let id = list(&sb.app, &json!({})).unwrap()["profiles"][0]["profileId"]
            .as_str()
            .unwrap()
            .to_string();
        open(&sb.app, &json!({ "profileId": id })).unwrap();

        let err = remove(&sb.app, &json!({ "profileId": id })).unwrap_err();
        assert!(err.message.contains("运行中"));
        close(&sb.app, &json!({ "profileId": id })).unwrap();
    }

    #[test]
    fn purge_deletes_only_aicc_snapshot() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        let live = seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "a@example.com");
        let sb = sandbox();
        scan(&sb.app, &json!({})).unwrap();
        let profile = list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let id = profile["profileId"].as_str().unwrap().to_string();
        let snapshot = PathBuf::from(profile["snapshotDir"].as_str().unwrap());

        let removed = remove(&sb.app, &json!({ "profileId": id, "purge": true })).unwrap();
        assert_eq!(removed["purged"], true);
        assert!(!snapshot.exists(), "快照目录应被删除");
        assert!(
            live.join(qoder::AUTH_FILE).is_file(),
            "客户端数据目录绝不能被删"
        );
    }

    #[test]
    fn switching_to_profile_without_snapshot_parks_live_auth() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        let live = seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "a@example.com");
        let sb = sandbox();

        // 不 scan，直接建一个空快照的 Profile。
        let created = create_profile(&sb.app, "qoder_global", "新账号");
        let id = created["profile"]["profileId"]
            .as_str()
            .unwrap()
            .to_string();

        let opened = open(&sb.app, &json!({ "profileId": id })).unwrap();
        assert_eq!(opened["requiresLogin"], true);
        assert_eq!(opened["usedSnapshot"], false);
        // live 认证被整份挪到备份，客户端会进登录页；原文件不在但也没被删除内容。
        assert!(!live.join(qoder::AUTH_FILE).exists());
        let backups = std::fs::read_dir(profiles_root(&sb.app).join(".live-backup")).unwrap();
        assert!(backups.count() >= 1, "必须留一份 live 备份");
        close(&sb.app, &json!({ "profileId": id })).unwrap();
    }

    #[test]
    fn dead_pid_invalidates_runtime_record() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "a@example.com");
        let sb = sandbox();
        scan(&sb.app, &json!({})).unwrap();
        let id = list(&sb.app, &json!({})).unwrap()["profiles"][0]["profileId"]
            .as_str()
            .unwrap()
            .to_string();

        // 一个几乎不可能存在的 PID：必须被判定"已退出"并清掉记录。
        sb.app
            .with_db(|db| {
                db.set_runtime(&RuntimeRecord {
                    profile_id: id.clone(),
                    pid: 999_999,
                    executable: "/nonexistent/Qoder".into(),
                    launched_at: "2026-01-01T00:00:00Z".into(),
                })?;
                Ok(())
            })
            .unwrap();

        let state = status(&sb.app, &json!({})).unwrap();
        assert_eq!(state["running"], 0);
        assert!(sb.app.with_db(|db| db.get_runtime(&id)).unwrap().is_none());
    }

    #[test]
    fn alive_pid_without_readable_command_line_reports_unverified() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "a@example.com");
        let sb = sandbox();
        scan(&sb.app, &json!({})).unwrap();
        let id = list(&sb.app, &json!({})).unwrap()["profiles"][0]["profileId"]
            .as_str()
            .unwrap()
            .to_string();

        // ps 读不到（假 ps 里没有这个 PID），但 kill -0 能看到本测试进程：
        // 必须报 running + verified=false，并且**不**丢弃记录。
        sb.app
            .with_db(|db| {
                db.set_runtime(&RuntimeRecord {
                    profile_id: id.clone(),
                    pid: i64::from(std::process::id()),
                    executable: "/nonexistent/Qoder".into(),
                    launched_at: "2026-01-01T00:00:00Z".into(),
                })?;
                Ok(())
            })
            .unwrap();

        let state = status(&sb.app, &json!({})).unwrap();
        assert_eq!(state["running"], 1);
        let runtime = &state["profiles"][0]["runtime"];
        assert_eq!(runtime["state"], "running");
        assert_eq!(runtime["verified"], false);
        assert!(sb.app.with_db(|db| db.get_runtime(&id)).unwrap().is_some());
        sb.app.with_db(|db| db.clear_runtime(&id)).unwrap();
    }

    #[test]
    fn inspect_reports_bad_format_and_never_leaks_token() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "a@example.com");
        let sb = sandbox();
        scan(&sb.app, &json!({})).unwrap();
        let profile = list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let id = profile["profileId"].as_str().unwrap().to_string();
        let snapshot = PathBuf::from(profile["snapshotDir"].as_str().unwrap());
        std::fs::write(snapshot.join(qoder::AUTH_FILE), b"v99garbagegarbage").unwrap();

        let report = inspect(&sb.app, &json!({ "profileId": id })).unwrap();
        assert_eq!(report["profile"]["authStatus"], "invalid");
        assert!(!contains_key(&report, "token"));
        assert_eq!(report["report"]["authenticated"], false);
    }

    #[test]
    fn classify_status_covers_known_states() {
        assert_eq!(
            classify_status(&json!({"state":"authenticated","expiresAt":"2000-01-01T00:00:00Z"})).0,
            "expired"
        );
        assert_eq!(
            classify_status(&json!({"state":"authenticated","expiresAt":"2999-01-01T00:00:00Z"})).0,
            "valid"
        );
        assert_eq!(
            classify_status(&json!({"state":"keychain_denied"})).0,
            "denied"
        );
        assert_eq!(
            classify_status(&json!({"state":"unsupported_format"})).0,
            "invalid"
        );
        assert_eq!(
            classify_status(&json!({"state":"something-new"})).0,
            "unknown"
        );
    }

    #[test]
    fn rename_is_local_only() {
        let _guard = lock_env();
        let dir = tempfile::tempdir().unwrap();
        let fixture = install_both(dir.path());
        let live = seed_live(&fixture, "com.qoder.app.stable", "v10LIVE");
        set_status_json(dir.path(), "a@example.com");
        let sb = sandbox();
        create_profile(&sb.app, "qoder_global", "旧名字");
        let id = list(&sb.app, &json!({})).unwrap()["profiles"][0]["profileId"]
            .as_str()
            .unwrap()
            .to_string();

        let renamed = rename(
            &sb.app,
            &json!({ "profileId": id, "displayName": "新名字" }),
        )
        .unwrap();
        assert_eq!(renamed["profile"]["displayName"], "新名字");
        assert!(live.join(qoder::AUTH_FILE).is_file());
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
