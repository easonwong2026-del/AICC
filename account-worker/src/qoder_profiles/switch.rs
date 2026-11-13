//! 认证快照切换：文件组事务（备份 → 暂存 → 校验 → 提交 → 回滚）。
//!
//! ## 为什么不是"两个文件 cp 一下"
//!
//! Qoder 的账号状态是**一组文件**（`auth.v1.dat` + `auth-profile-overlays.v1.dat`），
//! 切换时必须当成一个整体：
//!
//! ```text
//! begin   备份 live 组（含清单 hash）→ 写 journal(prepared)
//! stage   把快照组写成 live 目录里的 .aicc-*.tmp，逐个校验 hash
//! commit  逐个 rename 到正式位置，全部成功后再校验一次 → journal(committed)
//! 任一步失败 ──▶ rollback：用备份把 live 组恢复原样
//! ```
//!
//! 关键约束：
//!
//! 1. **备份先落盘**：`begin` 成功之前不允许动 live 的任何字节。
//! 2. **快照完整性**：`stage` 会拿快照文件的 sha256 和库里记的 `snapshot_hash`
//!    对比，快照被改坏/截断时**在覆盖 live 之前**就失败。
//! 3. **崩溃可恢复**：journal 在库里，重启后 `recover_pending` 会把所有
//!    `prepared` / `staged` 的切换按备份回滚。
//! 4. **并发互斥**：同一版本同时只允许一条未完成 journal（唯一索引）。
//!
//! ⚠️ 这一层保证的是"认证文件组的切换是原子的"，**不等于完整的 Profile 隔离**：
//! `main.sqlite`、`Local State`、Cookies、CLI 状态里还有账号绑定数据，
//! 见 `docs/QODER_MULTI_PROFILE_SPIKE.md` §7。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::app::App;
use crate::rpc::{RpcError, RpcResult};
use crate::storage::qoder_profiles::ProfileRecord;
use crate::storage::qoder_switch::{
    SwitchJournal, PHASE_CLEARED, PHASE_COMMITTED, PHASE_ROLLED_BACK, PHASE_STAGED,
};

/// 参与切换的文件组（相对 live/snapshot 目录）。
///
/// 只有这两个文件是"当前登录态"本身；其余（Cookies / Local State / main.sqlite）
/// 属于客户端业务数据，**故意不在这里**——它们也不该被当成"可以安全共享"，
/// 所以切换功能整体默认关闭，见 `docs/QODER_MULTI_PROFILE_SPIKE.md` §7。
pub const GROUP_FILES: &[&str] = &["auth.v1.dat", "auth-profile-overlays.v1.dat"];

const MANIFEST: &str = "manifest.json";
const TMP_PREFIX: &str = ".aicc-";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ManifestEntry {
    present: bool,
    sha256: Option<String>,
    size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackupManifest {
    live_dir: String,
    files: BTreeMap<String, ManifestEntry>,
}

#[derive(Debug)]
pub struct PreparedSwitch {
    pub journal: SwitchJournal,
}

// ---------------------------------------------------------------------------
// 测试用故障注入（生产构建里不存在）
// ---------------------------------------------------------------------------

#[cfg(test)]
pub mod fault {
    use std::sync::Mutex;

    static POINT: Mutex<Option<&'static str>> = Mutex::new(None);

    pub fn set(point: Option<&'static str>) {
        *POINT.lock().unwrap_or_else(|e| e.into_inner()) = point;
    }

    pub fn hit(point: &str) -> bool {
        let guard = POINT.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_deref() == Some(point)
    }
}

#[cfg(test)]
fn fault_hit(_point: &str) -> bool {
    fault::hit(_point)
}

#[cfg(not(test))]
fn fault_hit(_point: &str) -> bool {
    false
}

// ---------------------------------------------------------------------------
// 基础工具
// ---------------------------------------------------------------------------

/// 整文件 sha256（hex）。文件不存在返回 `None`。
pub fn file_sha256(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(hex(&Sha256::digest(&bytes)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn tmp_path(live: &Path, name: &str) -> PathBuf {
    live.join(format!("{TMP_PREFIX}{name}.tmp"))
}

/// 复制到临时文件 → 校验 → rename，避免客户端读到半个文件。
fn copy_atomic(src: &Path, dst: &Path, expect_sha: Option<&str>) -> RpcResult<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| RpcError::internal(format!("创建目录失败：{e}")))?;
    }
    let staged = dst.with_extension("aicc-tmp");
    std::fs::copy(src, &staged).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        RpcError::internal(format!("复制 {} 失败：{e}", src.display()))
    })?;
    if let Some(expect) = expect_sha {
        let actual = file_sha256(&staged)
            .ok_or_else(|| RpcError::internal(format!("读不到 {} 的哈希", staged.display())))?;
        if actual != expect {
            let _ = std::fs::remove_file(&staged);
            return Err(RpcError::internal(format!(
                "复制后校验失败：{} 期望 {expect} 实际 {actual}",
                staged.display()
            )));
        }
    }
    std::fs::rename(&staged, dst).map_err(|e| {
        let _ = std::fs::remove_file(&staged);
        RpcError::internal(format!("落盘 {} 失败：{e}", dst.display()))
    })?;
    Ok(())
}

fn backup_root(app: &App) -> PathBuf {
    app.paths.data_dir.join("qoder").join("switch-backups")
}

fn prune_backups(app: &App, keep: usize) {
    let root = backup_root(app);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return;
    };
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((mtime, e.path()))
        })
        .collect();
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in dirs.into_iter().skip(keep) {
        let _ = std::fs::remove_dir_all(path);
    }
}

// ---------------------------------------------------------------------------
// 事务
// ---------------------------------------------------------------------------

/// 备份 live 文件组并写 journal。**这一步之后才允许动 live。**
pub fn begin(app: &App, profile: &ProfileRecord) -> RpcResult<PreparedSwitch> {
    let live = PathBuf::from(&profile.live_dir);
    if !live.is_dir() {
        return Err(RpcError::not_found(format!(
            "客户端数据目录不存在：{}",
            live.display()
        )));
    }
    let id = format!(
        "sw_{}",
        &hex(&Sha256::digest(uuid::Uuid::new_v4().as_bytes()))[..16]
    );
    let backup_dir = backup_root(app).join(&id);
    std::fs::create_dir_all(&backup_dir)
        .map_err(|e| RpcError::internal(format!("创建备份目录失败：{e}")))?;

    let mut files = BTreeMap::new();
    for name in GROUP_FILES {
        let src = live.join(name);
        if src.is_file() {
            if fault_hit("backup_first") && *name == GROUP_FILES[0] {
                let _ = std::fs::remove_dir_all(&backup_dir);
                return Err(RpcError::internal("注入故障：备份第一个文件失败"));
            }
            let sha = file_sha256(&src);
            let size = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
            let dst = backup_dir.join(name);
            copy_atomic(&src, &dst, sha.as_deref()).inspect_err(|_| {
                let _ = std::fs::remove_dir_all(&backup_dir);
            })?;
            files.insert(
                (*name).to_string(),
                ManifestEntry {
                    present: true,
                    sha256: sha,
                    size,
                },
            );
        } else {
            files.insert(
                (*name).to_string(),
                ManifestEntry {
                    present: false,
                    sha256: None,
                    size: 0,
                },
            );
        }
    }

    let manifest: BackupManifest = BackupManifest {
        live_dir: live.to_string_lossy().to_string(),
        files,
    };
    let manifest_path = backup_dir.join(MANIFEST);
    let body = serde_json::to_vec_pretty(&manifest)
        .map_err(|e| RpcError::internal(format!("写清单失败：{e}")))?;
    std::fs::write(&manifest_path, body)
        .map_err(|e| RpcError::internal(format!("写清单失败：{e}")))?;

    let journal = SwitchJournal {
        id: id.clone(),
        profile_id: profile.profile_id.clone(),
        edition: profile.edition.clone(),
        phase: "prepared".into(),
        live_dir: live.to_string_lossy().to_string(),
        backup_dir: backup_dir.to_string_lossy().to_string(),
        files: GROUP_FILES.iter().map(|s| (*s).to_string()).collect(),
        created_at: crate::clock::now_rfc3339(),
        updated_at: crate::clock::now_rfc3339(),
    };
    if let Err(err) = app.with_db(|db| db.begin_switch(&journal)) {
        let _ = std::fs::remove_dir_all(&backup_dir);
        return Err(err);
    }
    let _ = manifest;
    Ok(PreparedSwitch { journal })
}

/// 把快照文件组暂存成 live 目录里的临时文件，并校验完整性。
pub fn stage(
    app: &App,
    prepared: &PreparedSwitch,
    snapshot: &Path,
    expect_snapshot_hash: Option<&str>,
) -> RpcResult<()> {
    let live = PathBuf::from(&prepared.journal.live_dir);
    let auth = snapshot.join(GROUP_FILES[0]);
    if !auth.is_file() {
        return Err(RpcError::not_found(format!(
            "快照里没有 {}：{}",
            GROUP_FILES[0],
            snapshot.display()
        )));
    }
    let actual = file_sha256(&auth).ok_or_else(|| RpcError::internal("读不到快照的哈希"))?;
    if let Some(expect) = expect_snapshot_hash {
        if actual != expect {
            return Err(RpcError::invalid_params(format!(
                "目标快照已损坏（哈希 {actual} ≠ 记录 {expect}），拒绝覆盖 live"
            )));
        }
    }

    let mut staged_any = false;
    for (index, name) in GROUP_FILES.iter().enumerate() {
        if fault_hit("stage_second") && index == 1 {
            return Err(RpcError::internal("注入故障：暂存第二个文件失败"));
        }
        let src = snapshot.join(name);
        let dst = tmp_path(&live, name);
        if src.is_file() {
            let sha = file_sha256(&src);
            copy_atomic(&src, &dst, sha.as_deref())?;
            staged_any = true;
        } else if dst.exists() {
            let _ = std::fs::remove_file(&dst);
        }
    }
    if !staged_any {
        return Err(RpcError::internal("没有任何文件被暂存"));
    }
    app.with_db(|db| db.set_switch_phase(&prepared.journal.id, PHASE_STAGED))?;
    Ok(())
}

/// 提交：把暂存文件逐个 rename 到位；任何一步失败按备份回滚。
pub fn commit(app: &App, prepared: &PreparedSwitch, snapshot: &Path) -> RpcResult<()> {
    let live = PathBuf::from(&prepared.journal.live_dir);
    let mut renamed: Vec<String> = Vec::new();

    let outcome = (|| -> RpcResult<()> {
        for (index, name) in GROUP_FILES.iter().enumerate() {
            if fault_hit("commit_second") && index == 1 {
                return Err(RpcError::internal("注入故障：提交第二个文件失败"));
            }
            let staged = tmp_path(&live, name);
            let target = live.join(name);
            if staged.is_file() {
                std::fs::rename(&staged, &target).map_err(|e| {
                    RpcError::internal(format!("提交 {} 失败：{e}", target.display()))
                })?;
                renamed.push((*name).to_string());
            } else if target.is_file() {
                // 快照里没有这个文件（例如 overlays）：按快照删除。
                std::fs::remove_file(&target).map_err(|e| {
                    RpcError::internal(format!("删除 {} 失败：{e}", target.display()))
                })?;
            }
        }
        // 全组校验：live 必须和快照逐字节一致。
        for name in GROUP_FILES {
            let want = file_sha256(&snapshot.join(name));
            let got = file_sha256(&live.join(name));
            if want != got {
                return Err(RpcError::internal(format!(
                    "提交后校验失败：{name} 期望 {want:?} 实际 {got:?}"
                )));
            }
        }
        Ok(())
    })();

    match outcome {
        Ok(()) => {
            app.with_db(|db| db.set_switch_phase(&prepared.journal.id, PHASE_COMMITTED))?;
            app.with_db(|db| db.finish_switch(&prepared.journal.id))?;
            for name in GROUP_FILES {
                let _ = std::fs::remove_file(tmp_path(&live, name));
            }
            prune_backups(app, 20);
            Ok(())
        }
        Err(err) => {
            let _ = renamed;
            let _ = rollback(app, &prepared.journal);
            Err(err)
        }
    }
}

/// 目标 Profile 还没有快照（要登录新账号）：把 live 的认证文件组挪进备份，
/// 客户端下次启动会进登录页。备份已经由 `begin` 落盘，这里只是移走原文件。
pub fn clear_live(app: &App, prepared: &PreparedSwitch) -> RpcResult<()> {
    let live = PathBuf::from(&prepared.journal.live_dir);
    for name in GROUP_FILES {
        let path = live.join(name);
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| RpcError::internal(format!("清空 live {name} 失败：{e}")))?;
        }
    }
    app.with_db(|db| db.set_switch_phase(&prepared.journal.id, PHASE_CLEARED))?;
    app.with_db(|db| db.finish_switch(&prepared.journal.id))?;
    Ok(())
}

/// 按备份把 live 组恢复原样。**恢复失败要如实报错，不能假装成功。**
pub fn rollback(app: &App, journal: &SwitchJournal) -> RpcResult<Value> {
    let live = PathBuf::from(&journal.live_dir);
    let backup_dir = PathBuf::from(&journal.backup_dir);
    let manifest_path = backup_dir.join(MANIFEST);
    let raw = std::fs::read(&manifest_path).map_err(|e| {
        RpcError::internal(format!(
            "读不到备份清单（{}）：{e}",
            manifest_path.display()
        ))
    })?;
    let manifest: BackupManifest = serde_json::from_slice(&raw)
        .map_err(|e| RpcError::internal(format!("备份清单损坏：{e}")))?;

    let mut restored: Vec<String> = Vec::new();
    let mut removed: Vec<String> = Vec::new();
    for (name, entry) in &manifest.files {
        let target = live.join(name);
        if entry.present {
            let src = backup_dir.join(name);
            copy_atomic(&src, &target, entry.sha256.as_deref())?;
            restored.push(name.clone());
        } else if target.exists() {
            std::fs::remove_file(&target)
                .map_err(|e| RpcError::internal(format!("恢复时删除 {name} 失败：{e}")))?;
            removed.push(name.clone());
        }
    }
    // 清掉可能残留的暂存文件。
    for name in GROUP_FILES {
        let _ = std::fs::remove_file(tmp_path(&live, name));
    }
    // 校验恢复结果。
    for (name, entry) in &manifest.files {
        let got = file_sha256(&live.join(name));
        if got != entry.sha256 {
            return Err(RpcError::internal(format!(
                "回滚后校验失败：{name} 期望 {:?} 实际 {got:?}",
                entry.sha256
            )));
        }
    }
    app.with_db(|db| db.set_switch_phase(&journal.id, PHASE_ROLLED_BACK))?;
    app.with_db(|db| db.finish_switch(&journal.id))?;
    Ok(json!({ "restored": restored, "removed": removed }))
}

/// 恢复所有未完成的切换（重启后调用）。
pub fn recover_pending(app: &App, edition: Option<&str>) -> RpcResult<Vec<Value>> {
    let journals = app.with_db(|db| db.active_switches(edition))?;
    let mut reports = Vec::new();
    for journal in journals {
        let result = rollback(app, &journal);
        match result {
            Ok(value) => reports.push(json!({
                "journalId": journal.id,
                "profileId": journal.profile_id,
                "edition": journal.edition,
                "phase": journal.phase,
                "recovered": true,
                "detail": value,
            })),
            Err(err) => reports.push(json!({
                "journalId": journal.id,
                "profileId": journal.profile_id,
                "edition": journal.edition,
                "phase": journal.phase,
                "recovered": false,
                "error": err.message,
            })),
        }
    }
    Ok(reports)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Paths;
    use crate::securestore::SecureStore;
    use crate::storage::qoder_profiles::ProfileInsert;
    use crate::test_env::lock as lock_env;

    struct Sandbox {
        _dir: tempfile::TempDir,
        app: App,
        live: PathBuf,
        snapshot: PathBuf,
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
        let live = dir.path().join("live");
        let snapshot = dir.path().join("snapshot");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::create_dir_all(&snapshot).unwrap();
        Sandbox {
            _dir: dir,
            app: App::with_secure(paths, SecureStore::memory()),
            live,
            snapshot,
        }
    }

    fn profile(sb: &Sandbox, id: &str) -> ProfileRecord {
        let snapshot = sb.snapshot.join(id);
        std::fs::create_dir_all(&snapshot).unwrap();
        let insert = ProfileInsert {
            profile_id: id.into(),
            edition: "qoder_global".into(),
            display_name: "t".into(),
            live_dir: sb.live.to_string_lossy().to_string(),
            snapshot_dir: snapshot.to_string_lossy().to_string(),
            app_bundle_id: None,
            app_path: None,
            client_version: None,
            auth_file: Some(snapshot.join("auth.v1.dat").to_string_lossy().to_string()),
            snapshot_hash: None,
            auth_status: "unknown".into(),
            managed: true,
        };
        sb.app.with_db(|db| db.insert_profile(&insert)).unwrap()
    }

    fn write(path: PathBuf, body: &[u8]) {
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn begin_backs_up_group_and_writes_manifest() {
        let _guard = lock_env();
        let sb = sandbox();
        write(sb.live.join("auth.v1.dat"), b"LIVE-AUTH");
        write(
            sb.live.join("auth-profile-overlays.v1.dat"),
            b"LIVE-OVERLAY",
        );
        let p = profile(&sb, "qp_aaaaaaaaaaaa");

        let prepared = begin(&sb.app, &p).unwrap();
        assert!(Path::new(&prepared.journal.backup_dir)
            .join("auth.v1.dat")
            .is_file());
        assert_eq!(
            file_sha256(&Path::new(&prepared.journal.backup_dir).join("auth.v1.dat")),
            file_sha256(&sb.live.join("auth.v1.dat"))
        );
        // 备份清单落盘，且每个文件都在。
        let manifest_path = Path::new(&prepared.journal.backup_dir).join(MANIFEST);
        let manifest: BackupManifest =
            serde_json::from_slice(&std::fs::read(manifest_path).unwrap()).unwrap();
        assert_eq!(manifest.files.len(), GROUP_FILES.len());
        assert_eq!(
            sb.app.with_db(|db| db.active_switches(None)).unwrap().len(),
            1
        );
    }

    #[test]
    fn commit_replaces_group_atomically() {
        let _guard = lock_env();
        let sb = sandbox();
        write(sb.live.join("auth.v1.dat"), b"LIVE-AUTH");
        let p = profile(&sb, "qp_bbbbbbbbbbbb");
        let snap = PathBuf::from(&p.snapshot_dir);
        write(snap.join("auth.v1.dat"), b"SNAP-AUTH");
        write(snap.join("auth-profile-overlays.v1.dat"), b"SNAP-OVERLAY");
        let snap_hash = file_sha256(&snap.join("auth.v1.dat"));

        let prepared = begin(&sb.app, &p).unwrap();
        stage(&sb.app, &prepared, &snap, snap_hash.as_deref()).unwrap();
        commit(&sb.app, &prepared, &snap).unwrap();

        assert_eq!(
            std::fs::read(sb.live.join("auth.v1.dat")).unwrap(),
            b"SNAP-AUTH"
        );
        assert_eq!(
            std::fs::read(sb.live.join("auth-profile-overlays.v1.dat")).unwrap(),
            b"SNAP-OVERLAY"
        );
        assert!(sb
            .app
            .with_db(|db| db.active_switches(None))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn corrupt_snapshot_is_rejected_before_touching_live() {
        let _guard = lock_env();
        let sb = sandbox();
        write(sb.live.join("auth.v1.dat"), b"LIVE-AUTH");
        let p = profile(&sb, "qp_cccccccccccc");
        let snap = PathBuf::from(&p.snapshot_dir);
        write(snap.join("auth.v1.dat"), b"CORRUPTED");
        let prepared = begin(&sb.app, &p).unwrap();

        // 库里记录的快照哈希是"原来的那份"，现在文件被改过。
        let err = stage(&sb.app, &prepared, &snap, Some("deadbeef")).unwrap_err();
        assert!(err.message.contains("损坏"), "msg={}", err.message);
        assert_eq!(
            std::fs::read(sb.live.join("auth.v1.dat")).unwrap(),
            b"LIVE-AUTH"
        );
        rollback(&sb.app, &prepared.journal).unwrap();
    }

    #[test]
    fn stage_failure_keeps_live_untouched() {
        let _guard = lock_env();
        let sb = sandbox();
        write(sb.live.join("auth.v1.dat"), b"LIVE-AUTH");
        let p = profile(&sb, "qp_dddddddddddd");
        let snap = PathBuf::from(&p.snapshot_dir);
        write(snap.join("auth.v1.dat"), b"SNAP-AUTH");
        write(snap.join("auth-profile-overlays.v1.dat"), b"SNAP-OVERLAY");
        let prepared = begin(&sb.app, &p).unwrap();

        fault::set(Some("stage_second"));
        let err = stage(&sb.app, &prepared, &snap, None).unwrap_err();
        fault::set(None);
        assert!(err.message.contains("注入故障"));
        assert_eq!(
            std::fs::read(sb.live.join("auth.v1.dat")).unwrap(),
            b"LIVE-AUTH"
        );
        rollback(&sb.app, &prepared.journal).unwrap();
    }

    #[test]
    fn commit_failure_rolls_back_both_files() {
        let _guard = lock_env();
        let sb = sandbox();
        write(sb.live.join("auth.v1.dat"), b"LIVE-AUTH");
        write(
            sb.live.join("auth-profile-overlays.v1.dat"),
            b"LIVE-OVERLAY",
        );
        let p = profile(&sb, "qp_eeeeeeeeeeee");
        let snap = PathBuf::from(&p.snapshot_dir);
        write(snap.join("auth.v1.dat"), b"SNAP-AUTH");
        write(snap.join("auth-profile-overlays.v1.dat"), b"SNAP-OVERLAY");
        let snap_hash = file_sha256(&snap.join("auth.v1.dat")).unwrap();

        let prepared = begin(&sb.app, &p).unwrap();
        stage(&sb.app, &prepared, &snap, Some(&snap_hash)).unwrap();

        fault::set(Some("commit_second"));
        let err = commit(&sb.app, &prepared, &snap).unwrap_err();
        fault::set(None);
        assert!(err.message.contains("注入故障"));
        // 两个文件都必须回到原值。
        assert_eq!(
            std::fs::read(sb.live.join("auth.v1.dat")).unwrap(),
            b"LIVE-AUTH"
        );
        assert_eq!(
            std::fs::read(sb.live.join("auth-profile-overlays.v1.dat")).unwrap(),
            b"LIVE-OVERLAY"
        );
        assert!(sb
            .app
            .with_db(|db| db.active_switches(None))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn recovery_after_crash_restores_live() {
        let _guard = lock_env();
        let sb = sandbox();
        write(sb.live.join("auth.v1.dat"), b"ORIGINAL");
        let p = profile(&sb, "qp_ffffffffffff");
        let prepared = begin(&sb.app, &p).unwrap();

        // 模拟"进程在 stage 之后崩溃"：live 已被改写，journal 仍是 staged。
        write(sb.live.join("auth.v1.dat"), b"HALF-WRITTEN");
        sb.app
            .with_db(|db| db.set_switch_phase(&prepared.journal.id, PHASE_STAGED))
            .unwrap();

        let reports = recover_pending(&sb.app, None).unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0]["recovered"], true);
        assert_eq!(
            std::fs::read(sb.live.join("auth.v1.dat")).unwrap(),
            b"ORIGINAL"
        );
        assert!(sb
            .app
            .with_db(|db| db.active_switches(None))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn concurrent_switch_is_rejected() {
        let _guard = lock_env();
        let sb = sandbox();
        write(sb.live.join("auth.v1.dat"), b"A");
        let p1 = profile(&sb, "qp_111111111111");
        let p2 = profile(&sb, "qp_222222222222");
        let first = begin(&sb.app, &p1).unwrap();
        let err = begin(&sb.app, &p2).unwrap_err();
        assert!(err.message.contains("未完成的切换"), "msg={}", err.message);
        rollback(&sb.app, &first.journal).unwrap();
    }
}
