//! Qoder 双版本账号管理的业务层（本阶段：只读管理 + 认证快照）。
//!
//! ## 四条硬约束
//!
//! 1. **不确定就停**：客户端在不在跑、进程是不是我们启动的、live 属于哪个账号——
//!    任何一项判断不出来，都直接拒绝，不写 live 的任何一个字节。
//! 2. **不写 live**：`main.sqlite` / `Local State` / Cookies 里还有账号绑定数据，
//!    只切两个认证文件**不等于**完整 Profile 隔离（见 SPIKE §7）。
//!    因此 `open` / `activate` **永久拒绝**，没有任何环境变量或调试开关能打开它；
//!    本阶段只开放版本识别、扫描、导入、认证检查、保存快照。
//! 3. **只读就是只读**：`list` / `status` 只报告未完成的切换日志，不执行恢复。
//!    恢复是显式动作，且必须先确认客户端已退出。
//! 4. **快照按组提交**：保存快照只写 AICC 自己的目录，但两个认证文件必须一起成功
//!    或一起回退；覆盖前还要能证明"当前 live 就是这个 Profile"。
//!
//! 切换事务的代码留在 `switch.rs`，但**只在测试构建里存在**（`#[cfg(test)]`），
//! 交付的二进制里没有任何"开始切换"的入口。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
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

/// 故障注入点只在测试构建里生效，交付的二进制里恒为 false。
#[cfg(test)]
fn fault_hit(point: &str) -> bool {
    switch::fault::hit(point)
}

#[cfg(not(test))]
fn fault_hit(_point: &str) -> bool {
    false
}

/// 正常退出的等待窗口。
const TERM_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

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
/// **恒为 `false`，没有任何运行时开关。**
///
/// 上一版用 `AICC_QODER_ENABLE_SWITCH=1` + `verified_on_this_machine()` 两个条件，
/// 但后者对 International 是编译期常量 `true`——等于一个环境变量就能绕过全部安全结论。
/// 在 `main.sqlite` / `Local State` / Cookies 里的账号数据被真正隔离之前，
/// 这条写入路径必须彻底封闭：要恢复切换，只能在代码里补上完整隔离 + 重新真机验收，
/// 不能靠外部配置。
pub fn switch_capability(_edition: QoderEdition) -> (bool, &'static str) {
    (
        false,
        "只替换认证文件无法隔离 main.sqlite / Local State / Cookies 中的账号数据；\
         本阶段仅支持双版本识别、扫描、导入、认证检查与快照保存",
    )
}

// ---------------------------------------------------------------------------
// 文件层
// ---------------------------------------------------------------------------

fn auth_path_in(dir: &Path) -> PathBuf {
    dir.join(qoder::AUTH_FILE)
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
    // 保存快照做到一半（有日志）时，快照可能只有半套文件，不能当成就绪。
    let incomplete = capture_journal_path(snapshot).exists();
    let (switching, reason) = edition_of(profile)
        .map(switch_capability)
        .unwrap_or((false, "版本未知"));
    value["runtime"] = runtime;
    value["active"] = json!(active);
    value["snapshotReady"] = json!(snapshot_hash.is_some() && !incomplete);
    value["snapshotIncomplete"] = json!(incomplete);
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

/// 保存快照的提交日志。放在快照目录里，进程崩了也能知道上一份在哪。
const CAPTURE_JOURNAL: &str = ".aicc-capture.json";

#[derive(Debug, Serialize, Deserialize)]
struct CaptureJournal {
    id: String,
    /// 提交前快照里已经存在的文件（恢复时据此决定"搬回去"还是"删掉"）。
    previous: Vec<String>,
    /// 上一份快照的哈希，恢复后用来校验。
    previous_hashes: BTreeMap<String, String>,
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn capture_journal_path(snapshot: &Path) -> PathBuf {
    snapshot.join(CAPTURE_JOURNAL)
}

fn capture_scratch(snapshot: &Path, kind: &str, id: &str) -> PathBuf {
    snapshot.join(format!(".aicc-capture-{kind}-{id}"))
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> RpcResult<()> {
    let body = serde_json::to_vec_pretty(value)
        .map_err(|e| RpcError::internal(format!("序列化快照日志失败：{e}")))?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, body).map_err(|e| RpcError::internal(format!("写快照日志失败：{e}")))?;
    std::fs::rename(&tmp, path).map_err(|e| RpcError::internal(format!("落盘快照日志失败：{e}")))
}

fn cleanup_capture(snapshot: &Path, id: &str) {
    let _ = std::fs::remove_dir_all(capture_scratch(snapshot, "new", id));
    let _ = std::fs::remove_dir_all(capture_scratch(snapshot, "old", id));
    let _ = std::fs::remove_file(capture_journal_path(snapshot));
}

/// 按日志把快照恢复成上一份完整版本。**恢复不了就报错，不假装成功。**
fn restore_capture(snapshot: &Path, journal: &CaptureJournal) -> RpcResult<()> {
    let previous_dir = capture_scratch(snapshot, "old", &journal.id);
    for name in switch::GROUP_FILES {
        let target = snapshot.join(name);
        let backup = previous_dir.join(name);
        if journal.previous.iter().any(|n| n == name) {
            if backup.is_file() {
                let _ = std::fs::remove_file(&target);
                std::fs::rename(&backup, &target)
                    .map_err(|e| RpcError::internal(format!("恢复快照 {name} 失败：{e}")))?;
            } else if switch::file_sha256(&target).as_ref() != journal.previous_hashes.get(*name) {
                // 备份不在、正式文件也不是上一份：这份快照已经无法证明是完整的。
                return Err(RpcError::internal(format!(
                    "恢复快照失败：上一份的 {name} 既不在备份目录里，也不在原位（{}）",
                    previous_dir.display()
                )));
            }
        } else if target.exists() {
            std::fs::remove_file(&target)
                .map_err(|e| RpcError::internal(format!("清理半成品快照 {name} 失败：{e}")))?;
        }
    }
    for name in &journal.previous {
        let want = journal.previous_hashes.get(name);
        let got = switch::file_sha256(&snapshot.join(name));
        if want != got.as_ref() {
            return Err(RpcError::internal(format!(
                "恢复快照后校验失败：{name} 期望 {want:?} 实际 {got:?}"
            )));
        }
    }
    Ok(())
}

/// 上一次保存如果被中断，先把快照恢复到上一份完整版本。
///
/// 返回被修复的日志 id（没有残留时返回 `None`）。这个函数只动 AICC 自己的
/// 快照目录，**永远不碰 live**，所以可以在写快照前无条件调用。
fn heal_capture(snapshot: &Path) -> RpcResult<Option<String>> {
    let path = capture_journal_path(snapshot);
    let Ok(raw) = std::fs::read(&path) else {
        return Ok(None);
    };
    let journal: CaptureJournal = serde_json::from_slice(&raw)
        .map_err(|e| RpcError::internal(format!("快照保存日志损坏（{}）：{e}", path.display())))?;
    restore_capture(snapshot, &journal)?;
    cleanup_capture(snapshot, &journal.id);
    Ok(Some(journal.id))
}

/// 把 live 的认证文件组存进快照。
///
/// 覆盖已有快照前必须能证明"live 现在就是这个 Profile"：
/// 邮箱一致，或 live 与快照逐字节一致。证明不了就拒绝。
///
/// 提交是**文件组事务**：新内容先写进暂存目录并逐个校验，然后写日志，
/// 再把上一份快照整体挪进 `old/`、新文件挪进正式位置，最后全组校验。
/// 任一步失败都按日志把上一份完整快照搬回来——不会出现"auth 是新的、
/// overlay 还是旧的"这种半套快照。
fn capture_into(app: &App, profile: &ProfileRecord) -> RpcResult<Value> {
    let live = PathBuf::from(&profile.live_dir);
    let snapshot = PathBuf::from(&profile.snapshot_dir);
    let edition = edition_of(profile)?;
    let live_auth = auth_path_in(&live);
    if !live_auth.is_file() {
        return Ok(json!({ "captured": false, "reason": "live 目录里没有 auth.v1.dat" }));
    }

    std::fs::create_dir_all(&snapshot)
        .map_err(|e| RpcError::internal(format!("创建快照目录失败：{e}")))?;
    // 上一次保存被中断 → 先恢复到上一份完整快照，再谈这次。
    let healed = heal_capture(&snapshot)?;

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

    let id = format!(
        "cp_{}",
        &hex(&Sha256::digest(uuid::Uuid::new_v4().as_bytes()))[..16]
    );
    let staging = capture_scratch(&snapshot, "new", &id);
    std::fs::create_dir_all(&staging)
        .map_err(|e| RpcError::internal(format!("创建快照暂存目录失败：{e}")))?;

    // 1. 暂存 + 逐个校验。全部成功之前不碰正式文件。
    let stage = (|| -> RpcResult<Vec<String>> {
        let mut present = Vec::new();
        for (index, name) in switch::GROUP_FILES.iter().enumerate() {
            let src = live.join(name);
            if !src.is_file() {
                continue;
            }
            if fault_hit("capture_stage_second") && index == 1 {
                return Err(RpcError::internal("注入故障：暂存第二个快照文件失败"));
            }
            let dst = staging.join(name);
            std::fs::copy(&src, &dst)
                .map_err(|e| RpcError::internal(format!("复制 {name} 到快照失败：{e}")))?;
            let want = switch::file_sha256(&src);
            let got = switch::file_sha256(&dst);
            if want.is_none() || want != got {
                return Err(RpcError::internal(format!("快照 {name} 复制后校验失败")));
            }
            present.push((*name).to_string());
        }
        if !staging.join(switch::GROUP_FILES[0]).is_file() {
            return Err(RpcError::internal(
                "live 里的 auth.v1.dat 读不到，无法保存快照",
            ));
        }
        Ok(present)
    })();

    let present = match stage {
        Ok(present) => present,
        Err(err) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(err);
        }
    };
    let _ = present;

    // 2. 记录上一份快照（提交前落盘，崩溃后靠它恢复）。
    let mut previous = Vec::new();
    let mut previous_hashes = BTreeMap::new();
    for name in switch::GROUP_FILES {
        if let Some(hash) = switch::file_sha256(&snapshot.join(name)) {
            previous.push((*name).to_string());
            previous_hashes.insert((*name).to_string(), hash);
        }
    }
    let journal = CaptureJournal {
        id: id.clone(),
        previous,
        previous_hashes,
    };
    if let Err(err) = write_json_atomic(&capture_journal_path(&snapshot), &journal) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err);
    }

    // 3. 提交：旧的一代挪进 old/，新的一代挪进正式位置，最后整组校验。
    let previous_dir = capture_scratch(&snapshot, "old", &id);
    let commit = (|| -> RpcResult<()> {
        std::fs::create_dir_all(&previous_dir)
            .map_err(|e| RpcError::internal(format!("创建快照备份目录失败：{e}")))?;
        // 先把上一代整体挪走（任何一步失败都还能按日志搬回来）。
        for name in switch::GROUP_FILES {
            let target = snapshot.join(name);
            if target.exists() {
                std::fs::rename(&target, previous_dir.join(name))
                    .map_err(|e| RpcError::internal(format!("备份上一份快照 {name} 失败：{e}")))?;
            }
        }
        // 再把新的一代挪到正式位置。
        for (index, name) in switch::GROUP_FILES.iter().enumerate() {
            if fault_hit("capture_commit_second") && index == 1 {
                return Err(RpcError::internal("注入故障：提交第二个快照文件失败"));
            }
            let staged = staging.join(name);
            if staged.is_file() {
                std::fs::rename(&staged, snapshot.join(name))
                    .map_err(|e| RpcError::internal(format!("落盘快照 {name} 失败：{e}")))?;
            }
        }
        for name in switch::GROUP_FILES {
            let want = switch::file_sha256(&live.join(name));
            let got = switch::file_sha256(&snapshot.join(name));
            if want != got {
                return Err(RpcError::internal(format!(
                    "保存后校验失败：{name} 期望 {want:?} 实际 {got:?}"
                )));
            }
        }
        Ok(())
    })();

    if let Err(err) = commit {
        return match restore_capture(&snapshot, &journal) {
            Ok(()) => {
                cleanup_capture(&snapshot, &id);
                Err(err)
            }
            Err(restore_err) => Err(RpcError::internal(format!(
                "{}；且恢复上一份快照也失败：{}",
                err.message, restore_err.message
            ))),
        };
    }

    cleanup_capture(&snapshot, &id);

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
    Ok(json!({
        "captured": true,
        "profile": updated.to_json(),
        "healedCapture": healed,
    }))
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

/// 未完成的切换日志（**只读**）。
///
/// `list` / `status` 只把它报出来，绝不在这里执行恢复：恢复会写 live 认证文件，
/// 而打开账号页面不应该产生任何写入。
fn pending_switches(app: &App, edition: Option<&str>) -> RpcResult<Value> {
    let journals = app.with_db(|db| db.active_switches(edition))?;
    let items: Vec<Value> = journals
        .iter()
        .map(|journal| {
            json!({
                "journalId": journal.id,
                "profileId": journal.profile_id,
                "edition": journal.edition,
                "phase": journal.phase,
                "createdAt": journal.created_at,
            })
        })
        .collect();
    Ok(json!({
        "count": items.len(),
        "items": items,
        "hint": if items.is_empty() {
            Value::Null
        } else {
            json!("存在未完成的切换记录：确认客户端已完全退出后再调用 qoder.profiles.recover 恢复，只读操作不会自动恢复")
        },
    }))
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
    // 只读：有未完成切换也只报告，不恢复（恢复见 `recover`）。
    let pending = pending_switches(app, edition)?;
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
        "pendingSwitches": pending,
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
    if capture_journal_path(&snapshot).exists() {
        return Err(RpcError::invalid_params(
            "该 Profile 上一次保存快照没有完成，快照可能只有半套文件；             请重新「保存当前登录」修复后再检查",
        ));
    }
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

fn refuse_switch(edition: QoderEdition) -> RpcError {
    let (_, reason) = switch_capability(edition);
    RpcError::invalid_params(format!(
        "{} 的账号切换未开放：{reason}",
        edition.display_name()
    ))
    .with_data(json!({
        "switchEnabled": false,
        "permanent": true,
        "reason": reason,
    }))
}

/// 打开（切换到）指定 Profile。
///
/// **本阶段永久拒绝。** 没有环境变量、没有调试开关：调用进来只做参数校验，
/// 不探测进程、不建备份、不读也不写 live 的任何一个字节。
///
/// 以后要在代码层面补上完整隔离（`main.sqlite` 的账号表、Cookies、
/// Local Storage 等）并重新做真机验收，才可能恢复这条路径。
pub fn open(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let profile = require_profile(app, id)?;
    Err(refuse_switch(edition_of(&profile)?))
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

/// 切换（等价于 `open`）：本阶段同样永久拒绝。
pub fn activate(app: &App, params: &Value) -> RpcResult<Value> {
    let id = param_str(params, "profileId")
        .ok_or_else(|| RpcError::invalid_params("必须提供 profileId"))?;
    let profile = require_profile(app, id)?;
    Err(refuse_switch(edition_of(&profile)?))
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
    let pending = pending_switches(app, edition)?;
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
        "pendingSwitches": pending,
    }))
}

/// 显式恢复未完成的切换。
///
/// 三条规则（P0 复核要求）：
///
/// 1. **只在显式调用时执行**：`list` / `status` 不会顺带写 live；
/// 2. **必须先确认客户端已退出**：`pgrep` 说还在跑 → 拒绝；探测不确定 → 拒绝；
///    客户端装在哪都找不到 → 拒绝（连"是否退出"都无法证明）；
/// 3. **恢复失败不静默**：任何一条恢复不了，整个调用返回错误并带上明细。
pub fn recover(app: &App, params: &Value) -> RpcResult<Value> {
    let edition = param_str(params, "edition")
        .map(|raw| {
            QoderEdition::parse(raw)
                .ok_or_else(|| RpcError::invalid_params("edition 只支持 qoder_cn / qoder_global"))
        })
        .transpose()?
        .map(|e| e.as_str());
    let journals = app.with_db(|db| db.active_switches(edition))?;

    let mut recovered = Vec::new();
    let mut blocked = Vec::new();
    for journal in &journals {
        let Some(edition) = QoderEdition::parse(&journal.edition) else {
            blocked.push(json!({
                "journalId": journal.id,
                "profileId": journal.profile_id,
                "edition": journal.edition,
                "error": "日志里的版本无法识别，拒绝恢复",
            }));
            continue;
        };

        // 只有能证明"客户端确实没在跑"才允许动 live。
        let executable = qoder_edition::find_app(edition)
            .and_then(|app| qoder_edition::resolve_executable(&app.app_path).ok());
        let Some(executable) = executable else {
            blocked.push(json!({
                "journalId": journal.id,
                "profileId": journal.profile_id,
                "edition": journal.edition,
                "error": "找不到该版本的客户端，无法确认它已经退出，拒绝恢复",
            }));
            continue;
        };
        match process::probe(&executable) {
            Running::Stopped => {}
            Running::Running(pids) => {
                blocked.push(json!({
                    "journalId": journal.id,
                    "profileId": journal.profile_id,
                    "edition": journal.edition,
                    "error": format!(
                        "客户端正在运行（PID {}），请先完全退出再恢复",
                        pids.iter().map(i64::to_string).collect::<Vec<_>>().join("、")
                    ),
                }));
                continue;
            }
            Running::Unknown(reason) => {
                blocked.push(json!({
                    "journalId": journal.id,
                    "profileId": journal.profile_id,
                    "edition": journal.edition,
                    "error": format!("无法确认客户端是否已退出（{reason}），拒绝恢复"),
                }));
                continue;
            }
        }

        match switch::rollback(app, journal) {
            Ok(detail) => recovered.push(json!({
                "journalId": journal.id,
                "profileId": journal.profile_id,
                "edition": journal.edition,
                "phase": journal.phase,
                "recovered": true,
                "detail": detail,
            })),
            Err(err) => blocked.push(json!({
                "journalId": journal.id,
                "profileId": journal.profile_id,
                "edition": journal.edition,
                "error": err.message,
            })),
        }
    }

    if !blocked.is_empty() {
        return Err(RpcError::internal(format!(
            "{} 条未完成的切换未能恢复，live 目录保持原样；请按明细处理后重试",
            blocked.len()
        ))
        .with_data(json!({ "recovered": recovered, "blocked": blocked })));
    }

    Ok(json!({
        "recovered": recovered,
        "count": recovered.len(),
        "pending": journals.len(),
    }))
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
    /// stopped → exit 1（唯一能代表"确认没跑"的退出码）；
    /// external → 回放一个外部 PID；state → 回放现存实例。
    const MOCK_PGREP: &str = r#"#!/bin/sh
mode=$(cat "$AICC_MOCK_STATE/mode" 2>/dev/null)
[ -z "$mode" ] && mode=stopped
case "$mode" in
  stopped) exit 1;;
  external) cat "$AICC_MOCK_STATE/external.pid" 2>/dev/null; exit 0;;
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

    fn install_both(sb: &Sandbox, dir: &Path) {
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

    fn seed_live(sb: &Sandbox, name: &str, auth: &str, overlay: &str) -> PathBuf {
        let dir = sb.support.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Local State"), b"{}").unwrap();
        std::fs::write(dir.join(qoder::AUTH_FILE), auth.as_bytes()).unwrap();
        std::fs::write(dir.join("auth-profile-overlays.v1.dat"), overlay.as_bytes()).unwrap();
        dir
    }

    fn create_profile(app: &App, edition: &str, name: &str) -> Value {
        super::create(app, &json!({ "edition": edition, "displayName": name })).unwrap()
    }

    fn sha(path: &Path) -> Option<String> {
        switch::file_sha256(path)
    }

    fn register_current(sb: &Sandbox) -> (String, PathBuf) {
        super::scan(&sb.app, &json!({})).unwrap();
        let profile = super::list(&sb.app, &json!({})).unwrap()["profiles"][0].clone();
        let id = profile["profileId"].as_str().unwrap().to_string();
        let snapshot = PathBuf::from(profile["snapshotDir"].as_str().unwrap());
        (id, snapshot)
    }

    // ---- 1. 环境变量绕不过闸门 ----

    #[test]
    fn switch_is_permanently_off_even_with_env_flag() {
        let _guard = lock_env();
        std::env::set_var("AICC_QODER_ENABLE_SWITCH", "1");
        let (global, reason) = switch_capability(QoderEdition::Global);
        let (cn, _) = switch_capability(QoderEdition::Cn);
        let detected = editions_detect();
        std::env::remove_var("AICC_QODER_ENABLE_SWITCH");

        assert!(!global, "国际版也必须永久关闭");
        assert!(!cn);
        assert!(reason.contains("无法隔离"), "reason={reason}");
        for edition in detected["editions"].as_array().unwrap() {
            assert_eq!(edition["switchEnabled"], false, "{edition}");
        }
    }

    #[test]
    fn open_and_activate_refuse_and_write_nothing() {
        let _guard = lock_env();
        std::env::set_var("AICC_QODER_ENABLE_SWITCH", "1");
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A", "OVERLAY-A");
        set_status_json(&sb, "a@example.com");
        let (id, _) = register_current(&sb);
        let before = sha(&live.join(qoder::AUTH_FILE));

        let opened = super::open(&sb.app, &json!({ "profileId": id }));
        let activated = super::activate(&sb.app, &json!({ "profileId": id }));
        std::env::remove_var("AICC_QODER_ENABLE_SWITCH");

        for outcome in [opened, activated] {
            let err = outcome.unwrap_err();
            assert!(err.message.contains("未开放"), "msg={}", err.message);
        }
        assert_eq!(
            sha(&live.join(qoder::AUTH_FILE)),
            before,
            "live 一个字节都不能动"
        );
        assert!(
            !sb.app.paths.data_dir.join("qoder/switch-backups").exists(),
            "拒绝时不允许创建备份目录"
        );
        assert!(sb
            .app
            .with_db(|db| db.active_switches(None))
            .unwrap()
            .is_empty());
        assert!(sb.app.with_db(|db| db.list_runtime()).unwrap().is_empty());
    }

    // ---- 2. 只读操作不恢复 live ----

    #[test]
    fn list_and_status_report_pending_switch_without_restoring() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "ORIGINAL", "ORIGINAL-OVERLAY");
        set_status_json(&sb, "a@example.com");
        let (id, _) = register_current(&sb);

        // 模拟"切换走到一半崩溃"：journal 还在，live 已被改写。
        let profile = require_profile(&sb.app, &id).unwrap();
        let prepared = switch::begin(&sb.app, &profile).unwrap();
        std::fs::write(live.join(qoder::AUTH_FILE), b"HALF-WRITTEN").unwrap();

        let listed = super::list(&sb.app, &json!({})).unwrap();
        assert_eq!(listed["pendingSwitches"]["count"], 1, "list 只报告，不恢复");
        let stated = super::status(&sb.app, &json!({})).unwrap();
        assert_eq!(stated["pendingSwitches"]["count"], 1);

        assert_eq!(
            std::fs::read(live.join(qoder::AUTH_FILE)).unwrap(),
            b"HALF-WRITTEN",
            "只读接口不允许写 live"
        );
        assert_eq!(
            sb.app.with_db(|db| db.active_switches(None)).unwrap().len(),
            1,
            "journal 也不允许被只读接口清掉"
        );
        let _ = prepared;
    }

    // ---- 3. 恢复必须先确认客户端退出 ----

    #[test]
    fn recover_refuses_when_process_state_is_unknown() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "ORIGINAL", "ORIGINAL-OVERLAY");
        set_status_json(&sb, "a@example.com");
        let (id, _) = register_current(&sb);
        let profile = require_profile(&sb.app, &id).unwrap();
        switch::begin(&sb.app, &profile).unwrap();
        std::fs::write(live.join(qoder::AUTH_FILE), b"HALF-WRITTEN").unwrap();

        // pgrep 不可执行 → Unknown。
        let broken = sb._dir.path().join("no-such-pgrep");
        std::env::set_var(process::PGREP_BIN_ENV, broken.to_string_lossy().to_string());
        let err = super::recover(&sb.app, &json!({})).unwrap_err();
        assert!(err.message.contains("未能恢复"), "msg={}", err.message);

        assert_eq!(
            std::fs::read(live.join(qoder::AUTH_FILE)).unwrap(),
            b"HALF-WRITTEN",
            "状态未知时禁止恢复"
        );
        assert_eq!(
            sb.app.with_db(|db| db.active_switches(None)).unwrap().len(),
            1
        );
    }

    #[test]
    fn recover_refuses_while_client_is_running() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "ORIGINAL", "ORIGINAL-OVERLAY");
        set_status_json(&sb, "a@example.com");
        let (id, _) = register_current(&sb);
        let profile = require_profile(&sb.app, &id).unwrap();
        switch::begin(&sb.app, &profile).unwrap();
        std::fs::write(live.join(qoder::AUTH_FILE), b"HALF-WRITTEN").unwrap();

        std::fs::write(sb.state.join("external.pid"), "424242").unwrap();
        set_mode(&sb, "external");
        let err = super::recover(&sb.app, &json!({})).unwrap_err();
        assert!(err.message.contains("未能恢复"), "msg={}", err.message);
        assert_eq!(
            std::fs::read(live.join(qoder::AUTH_FILE)).unwrap(),
            b"HALF-WRITTEN"
        );
    }

    #[test]
    fn recover_restores_only_after_confirmed_stop() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "ORIGINAL", "ORIGINAL-OVERLAY");
        set_status_json(&sb, "a@example.com");
        let (id, _) = register_current(&sb);
        let profile = require_profile(&sb.app, &id).unwrap();
        switch::begin(&sb.app, &profile).unwrap();
        std::fs::write(live.join(qoder::AUTH_FILE), b"HALF-WRITTEN").unwrap();

        set_mode(&sb, "stopped");
        let result = super::recover(&sb.app, &json!({})).unwrap();
        assert_eq!(result["count"], 1);
        assert_eq!(
            std::fs::read(live.join(qoder::AUTH_FILE)).unwrap(),
            b"ORIGINAL"
        );
        assert!(sb
            .app
            .with_db(|db| db.active_switches(None))
            .unwrap()
            .is_empty());
    }

    // ---- 4. 保存快照的文件组原子性 ----

    #[test]
    fn capture_stage_failure_keeps_previous_snapshot() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A", "OVERLAY-A");
        set_status_json(&sb, "a@example.com");
        let (id, snapshot) = register_current(&sb);

        std::fs::write(live.join(qoder::AUTH_FILE), b"LIVE-A2").unwrap();
        std::fs::write(live.join("auth-profile-overlays.v1.dat"), b"OVERLAY-A2").unwrap();

        switch::fault::set(Some("capture_stage_second"));
        let err = super::capture(&sb.app, &json!({ "profileId": id })).unwrap_err();
        switch::fault::set(None);
        assert!(err.message.contains("注入故障"));
        assert_eq!(
            std::fs::read(snapshot.join(qoder::AUTH_FILE)).unwrap(),
            b"LIVE-A"
        );
        assert_eq!(
            std::fs::read(snapshot.join("auth-profile-overlays.v1.dat")).unwrap(),
            b"OVERLAY-A"
        );
        assert!(!capture_journal_path(&snapshot).exists());
    }

    #[test]
    fn capture_commit_failure_restores_both_files() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A", "OVERLAY-A");
        set_status_json(&sb, "a@example.com");
        let (id, snapshot) = register_current(&sb);

        std::fs::write(live.join(qoder::AUTH_FILE), b"LIVE-A2").unwrap();
        std::fs::write(live.join("auth-profile-overlays.v1.dat"), b"OVERLAY-A2").unwrap();

        switch::fault::set(Some("capture_commit_second"));
        let err = super::capture(&sb.app, &json!({ "profileId": id })).unwrap_err();
        switch::fault::set(None);
        assert!(err.message.contains("注入故障"));
        assert_eq!(
            std::fs::read(snapshot.join(qoder::AUTH_FILE)).unwrap(),
            b"LIVE-A",
            "第一个文件也必须回到上一份"
        );
        assert_eq!(
            std::fs::read(snapshot.join("auth-profile-overlays.v1.dat")).unwrap(),
            b"OVERLAY-A"
        );
        assert!(!capture_journal_path(&snapshot).exists());
        assert_eq!(
            std::fs::read(live.join(qoder::AUTH_FILE)).unwrap(),
            b"LIVE-A2",
            "live 从不参与保存"
        );
    }

    #[test]
    fn capture_heals_an_interrupted_commit() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A", "OVERLAY-A");
        set_status_json(&sb, "a@example.com");
        let (id, snapshot) = register_current(&sb);

        // 手工制造"提交到一半崩溃"：上一代在 old/，正式位置只有半套新文件。
        let journal = CaptureJournal {
            id: "cp_deadbeefdeadbeef".into(),
            previous: vec![qoder::AUTH_FILE.to_string()],
            previous_hashes: BTreeMap::from([(
                qoder::AUTH_FILE.to_string(),
                sha(&snapshot.join(qoder::AUTH_FILE)).unwrap(),
            )]),
        };
        let old = capture_scratch(&snapshot, "old", &journal.id);
        std::fs::create_dir_all(&old).unwrap();
        std::fs::rename(snapshot.join(qoder::AUTH_FILE), old.join(qoder::AUTH_FILE)).unwrap();
        std::fs::write(snapshot.join(qoder::AUTH_FILE), b"PARTIAL-WRITE").unwrap();
        write_json_atomic(&capture_journal_path(&snapshot), &journal).unwrap();

        // live 换成同账号的新一代，然后保存：必须先修复上一份，再整体提交。
        std::fs::write(live.join(qoder::AUTH_FILE), b"LIVE-A2").unwrap();
        std::fs::write(live.join("auth-profile-overlays.v1.dat"), b"OVERLAY-A2").unwrap();
        let result = super::capture(&sb.app, &json!({ "profileId": id })).unwrap();
        assert_eq!(result["captured"], true);
        assert_eq!(result["healedCapture"], json!("cp_deadbeefdeadbeef"));
        assert_eq!(
            std::fs::read(snapshot.join(qoder::AUTH_FILE)).unwrap(),
            b"LIVE-A2"
        );
        assert_eq!(
            std::fs::read(snapshot.join("auth-profile-overlays.v1.dat")).unwrap(),
            b"OVERLAY-A2"
        );
        assert!(!capture_journal_path(&snapshot).exists());
        assert!(!old.exists());
    }

    // ---- 5. 回归：识别、扫描、导入、检查、删除 ----

    #[test]
    fn scan_captures_current_account_without_touching_live() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A", "OVERLAY-A");
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
        assert_eq!(profile["switchEnabled"], false);
        assert!(profile["switchBlockedReason"]
            .as_str()
            .unwrap()
            .contains("无法隔离"));

        let again = super::scan(&sb.app, &json!({})).unwrap();
        assert!(again["created"].as_array().unwrap().is_empty());
    }

    #[test]
    fn cross_edition_profiles_are_independent() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        seed_live(&sb, "com.qoder.app.stable", "GL", "GL-OV");
        seed_live(&sb, "com.qodercn.app", "CN", "CN-OV");
        let cn = create_profile(&sb.app, "qoder_cn", "CN");
        let global = create_profile(&sb.app, "qoder_global", "GL");
        assert_ne!(cn["profile"]["liveDir"], global["profile"]["liveDir"]);
        assert_ne!(
            cn["profile"]["snapshotDir"],
            global["profile"]["snapshotDir"]
        );
        assert_eq!(
            super::list(&sb.app, &json!({ "edition": "qoder_cn" })).unwrap()["count"],
            1
        );
    }

    #[test]
    fn import_rejects_missing_auth_and_traversal() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        seed_live(&sb, "com.qoder.app.stable", "LIVE-A", "OVERLAY-A");

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
    fn invalid_profile_ids_are_rejected() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        for bad in ["../etc", "qp_xyz", "qp_123", "qp_0123456789abc"] {
            let err = super::inspect(&sb.app, &json!({ "profileId": bad })).unwrap_err();
            assert!(
                err.message.contains("非法"),
                "bad={bad} msg={}",
                err.message
            );
        }
    }

    #[test]
    fn capture_refuses_identity_mismatch_and_keeps_live() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A", "OVERLAY-A");
        set_status_json(&sb, "a@example.com");
        let (id, snapshot) = register_current(&sb);
        let snapshot_before = sha(&snapshot.join(qoder::AUTH_FILE));

        set_status_json(&sb, "b@example.com");
        std::fs::write(live.join(qoder::AUTH_FILE), b"LIVE-B").unwrap();
        let before = sha(&live.join(qoder::AUTH_FILE));

        let result = super::capture(&sb.app, &json!({ "profileId": id })).unwrap();
        assert_eq!(result["captured"], false);
        assert_eq!(result["identityMismatch"], true);
        assert_eq!(sha(&live.join(qoder::AUTH_FILE)), before);
        assert_eq!(sha(&snapshot.join(qoder::AUTH_FILE)), snapshot_before);
    }

    #[test]
    fn purge_deletes_only_snapshot() {
        let _guard = lock_env();
        let sb = sandbox();
        install_both(&sb, sb._dir.path());
        let live = seed_live(&sb, "com.qoder.app.stable", "LIVE-A", "OVERLAY-A");
        set_status_json(&sb, "a@example.com");
        let (id, snapshot) = register_current(&sb);

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
        seed_live(&sb, "com.qoder.app.stable", "v10LIVE", "v10OVERLAY");
        set_status_json(&sb, "a@example.com");
        let (id, snapshot) = register_current(&sb);
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
        seed_live(&sb, "com.qoder.app.stable", "LIVE-A", "OVERLAY-A");
        set_status_json(&sb, "a@example.com");
        let (id, _) = register_current(&sb);

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
        assert_eq!(state["profiles"][0]["runtime"]["state"], "unknown");
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
