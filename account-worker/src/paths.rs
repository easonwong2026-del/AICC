//! 数据目录解析。
//!
//! 账号中心的数据与 Magpie 的 `status.json` 完全分开，落在
//! `~/Library/Application Support/AICC-Dashboard/accounts/`（PRD §9.1）。
//!
//! `WB_SWITCH_HOME` 把上游核心的存储（`~/.wb-switch`）一并收进这个目录，
//! 避免碰到用户自己那套 wb-switch 数据。上游的 `home_dir()` 会读这个变量，
//! 所以必须在调用任何 `wb_switch_core` 函数之前设好。

use std::path::PathBuf;

/// 覆盖数据根的环境变量（测试和沙箱用）。
pub const DATA_DIR_ENV: &str = "AICC_ACCOUNT_DATA_DIR";
/// 上游核心的家目录覆盖变量。
pub const WB_HOME_ENV: &str = "WB_SWITCH_HOME";

#[derive(Debug, Clone)]
pub struct Paths {
    pub data_dir: PathBuf,
    pub db_path: PathBuf,
    pub backups_dir: PathBuf,
    pub logs_dir: PathBuf,
    pub wb_switch_home: PathBuf,
    /// 是否由本进程写入了 `WB_SWITCH_HOME`（false = 外部已指定，保持原样）。
    pub wb_home_owned: bool,
}

impl Paths {
    /// 解析路径并把 `WB_SWITCH_HOME` 定下来。进程启动时调用一次。
    pub fn resolve() -> Self {
        let data_dir = match std::env::var_os(DATA_DIR_ENV) {
            Some(v) if !v.is_empty() => PathBuf::from(v),
            _ => default_data_dir(),
        };

        let (wb_switch_home, wb_home_owned) = match std::env::var_os(WB_HOME_ENV) {
            Some(v) if !v.is_empty() => (PathBuf::from(v), false),
            _ => {
                let home = data_dir.join("wb-switch-home");
                std::env::set_var(WB_HOME_ENV, &home);
                (home, true)
            }
        };

        Self {
            db_path: data_dir.join("accounts.sqlite"),
            backups_dir: data_dir.join("backups"),
            logs_dir: data_dir.join("logs"),
            data_dir,
            wb_switch_home,
            wb_home_owned,
        }
    }

    /// 建目录。`health` 不该因为目录缺失就失败，所以顶层调用要忽略错误并回报。
    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.data_dir)?;
        std::fs::create_dir_all(&self.backups_dir)?;
        std::fs::create_dir_all(&self.logs_dir)?;
        std::fs::create_dir_all(&self.wb_switch_home)?;
        Ok(())
    }
}

fn default_data_dir() -> PathBuf {
    dirs_home()
        .join("Library")
        .join("Application Support")
        .join("AICC-Dashboard")
        .join("accounts")
}

/// 真实用户主目录。这里不能用 `WB_SWITCH_HOME`——它可能已被改成沙箱路径。
pub fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}
