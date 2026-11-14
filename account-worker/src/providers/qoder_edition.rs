//! Qoder 的两个平台变体：CN 与 International（Global）。
//!
//! **这两个是独立产品，不是换个名字的同一份界面。** 它们各自有自己的安装包、
//! Bundle ID、用户数据目录和钥匙串条目（证据见 `docs/QODER_MULTI_PROFILE_SPIKE.md`）。
//! 本模块只负责"哪个版本长什么样、装在哪里、数据落在哪"，**不做认证解析**——
//! 解析分别在 `providers::qoder`（CN）与 `providers::qoder_global`（International）。
//!
//! 识别顺序固定为：Bundle ID > 可执行文件路径 > 目录名。
//! 仅凭 App 名字或图标判断版本是不可接受的（PRD §3.1）。

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::paths::dirs_home;

/// 覆盖 App 搜索根的环境变量（冒号分隔），测试与非常规安装位置用。
pub const APP_DIRS_ENV: &str = "AICC_QODER_APP_DIRS";
/// 覆盖 Application Support 根的环境变量，测试用。
pub const SUPPORT_DIR_ENV: &str = "AICC_QODER_SUPPORT_DIR";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QoderEdition {
    /// 国内版。**本机未安装过，识别参数来自既有实现与上游参考实现**，
    /// 见 SPIKE 文档 §2.2，未经真机复验。
    Cn,
    /// 国际版。本机实测：Bundle `com.qoder.app`，v0.5.7。
    Global,
}

impl QoderEdition {
    pub const ALL: [QoderEdition; 2] = [QoderEdition::Cn, QoderEdition::Global];

    /// 落库、RPC、界面统一用这个字符串，禁止用模糊的 `qoder`。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cn => "qoder_cn",
            Self::Global => "qoder_global",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "qoder_cn" | "cn" => Some(Self::Cn),
            "qoder_global" | "global" | "qoder_international" | "international" => {
                Some(Self::Global)
            }
            _ => None,
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Cn => "Qoder CN",
            Self::Global => "Qoder International",
        }
    }

    /// 用户数据目录名前缀（`~/Library/Application Support/` 下）。
    pub fn data_dir_prefix(self) -> &'static str {
        match self {
            Self::Cn => "com.qodercn.app",
            Self::Global => "com.qoder.app",
        }
    }

    pub fn bundle_id(self) -> &'static str {
        match self {
            Self::Cn => "com.qodercn.app",
            Self::Global => "com.qoder.app",
        }
    }

    pub fn keychain_service(self) -> &'static str {
        match self {
            Self::Cn => "Qoder CN App Safe Storage",
            Self::Global => "Qoder App Safe Storage",
        }
    }

    pub fn keychain_account(self) -> &'static str {
        match self {
            Self::Cn => "Qoder CN App Key",
            Self::Global => "Qoder App Key",
        }
    }

    /// 该版本的识别/认证参数是否在本机验证过。
    pub fn verified_on_this_machine(self) -> bool {
        matches!(self, Self::Global)
    }

    /// 是否能"同时跑多个账号"。
    ///
    /// - International：实测**不能**——客户端忽略 `--user-data-dir`，
    ///   一个 OS 用户只有一份 userData，第二个实例被单实例锁挡回。
    /// - CN：没实测过，答案是**未知**，不能写成"已确认不支持"。
    pub fn parallel_instances_label(self) -> &'static str {
        match self {
            Self::Global => "no",
            Self::Cn => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledApp {
    pub edition: &'static str,
    pub app_path: String,
    pub bundle_id: String,
    pub version: Option<String>,
    pub executable: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditionDetection {
    pub edition: &'static str,
    pub display_name: &'static str,
    pub bundle_id: &'static str,
    pub keychain_service: &'static str,
    pub keychain_account: &'static str,
    pub data_dir_prefix: &'static str,
    pub installed: bool,
    pub app_path: Option<String>,
    pub client_version: Option<String>,
    pub executable: Option<String>,
    pub data_dirs: Vec<String>,
    pub parallel_instances: &'static str,
    /// 该版本的识别参数是否在本机复验过。
    pub verified_on_this_machine: bool,
}

/// App 搜索根：默认 `/Applications` 与 `~/Applications`。
pub fn app_roots() -> Vec<PathBuf> {
    if let Some(value) = std::env::var_os(APP_DIRS_ENV) {
        let raw = value.to_string_lossy().to_string();
        let roots: Vec<PathBuf> = raw
            .split(':')
            .filter(|p| !p.trim().is_empty())
            .map(PathBuf::from)
            .collect();
        if !roots.is_empty() {
            return roots;
        }
    }
    vec![
        PathBuf::from("/Applications"),
        dirs_home().join("Applications"),
    ]
}

pub fn support_root() -> PathBuf {
    match std::env::var_os(SUPPORT_DIR_ENV) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => dirs_home().join("Library").join("Application Support"),
    }
}

/// 扫描搜索根，按 Bundle ID 找某个版本的客户端。
///
/// **不看 App 名字**：改名、换图标、放错目录都不影响识别，唯一判据是
/// `Contents/Info.plist` 里的 `CFBundleIdentifier`。
pub fn find_app(edition: QoderEdition) -> Option<InstalledApp> {
    for root in app_roots() {
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "app").unwrap_or(false))
            .collect();
        candidates.sort();

        for path in candidates {
            let Some(plist) = read_info_plist(&path) else {
                continue;
            };
            if plist.bundle_id.as_deref() != Some(edition.bundle_id()) {
                continue;
            }
            let executable = plist
                .executable
                .clone()
                .map(|name| path.join("Contents").join("MacOS").join(name))
                .filter(|p| p.is_file())
                .map(|p| p.to_string_lossy().to_string());
            return Some(InstalledApp {
                edition: edition.as_str(),
                app_path: path.to_string_lossy().to_string(),
                bundle_id: edition.bundle_id().to_string(),
                version: plist.version,
                executable,
            });
        }
    }
    None
}

/// 该版本在 `Application Support` 下的用户数据目录，按修改时间倒序。
pub fn find_data_dirs(edition: QoderEdition) -> Vec<PathBuf> {
    let root = support_root();
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let prefix = edition.data_dir_prefix();
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with(prefix) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if !looks_like_user_data(&path) {
            continue;
        }
        let mtime = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        found.push((mtime, path));
    }
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Electron 的 userData 至少会有其中一两个标记文件；空目录、缓存目录不算。
fn looks_like_user_data(dir: &Path) -> bool {
    ["auth.v1.dat", "Local State", "Preferences", "main.sqlite"]
        .iter()
        .any(|name| dir.join(name).exists())
}

pub fn detect(edition: QoderEdition) -> EditionDetection {
    let app = find_app(edition);
    let data_dirs: Vec<String> = find_data_dirs(edition)
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    EditionDetection {
        edition: edition.as_str(),
        display_name: edition.display_name(),
        bundle_id: edition.bundle_id(),
        keychain_service: edition.keychain_service(),
        keychain_account: edition.keychain_account(),
        data_dir_prefix: edition.data_dir_prefix(),
        installed: app.is_some(),
        app_path: app.as_ref().map(|a| a.app_path.clone()),
        client_version: app.as_ref().and_then(|a| a.version.clone()),
        executable: app.and_then(|a| a.executable),
        data_dirs,
        parallel_instances: edition.parallel_instances_label(),
        verified_on_this_machine: edition.verified_on_this_machine(),
    }
}

/// 把 App 路径解析成真正要 exec 的文件。
///
/// `.app` 走 `Contents/MacOS/<CFBundleExecutable>`；其它情况按"本身就是可执行文件"
/// 处理——测试用的 mock 客户端就是这种形态，不允许为了测试在线上代码里另开分支。
pub fn resolve_executable(app_path: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(app_path);
    if !path.exists() {
        return Err(format!("客户端路径不存在：{}", path.display()));
    }
    if path.extension().map(|e| e == "app").unwrap_or(false) {
        let plist = read_info_plist(&path)
            .ok_or_else(|| format!("读不出 {} 的 Info.plist", path.display()))?;
        let name = plist
            .executable
            .ok_or_else(|| format!("{} 的 Info.plist 没有 CFBundleExecutable", path.display()))?;
        let exe = path.join("Contents").join("MacOS").join(name);
        if !exe.is_file() {
            return Err(format!("可执行文件不存在：{}", exe.display()));
        }
        return Ok(exe);
    }
    if path.is_dir() {
        return Err(format!(
            "{} 是目录但不是 .app 包，无法确定可执行文件",
            path.display()
        ));
    }
    Ok(path)
}

/// 客户端自己写在磁盘上的登录状态（不是凭据，只用来判断"现在是谁登录着"）。
///
/// International 实测路径：`~/.qoder/.qoder-app-status.json`，字段
/// `logged_in / name / email / product / version`。CN 的对应文件没有在本机验证过，
/// 因此这里只对 International 返回数据，CN 一律 `None`——宁可不匹配，也不猜。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientStatus {
    pub logged_in: bool,
    pub name: Option<String>,
    pub email: Option<String>,
}

/// 覆盖状态文件路径的环境变量，测试用。
pub const STATUS_JSON_ENV: &str = "AICC_QODER_STATUS_JSON";

pub fn read_client_status(edition: QoderEdition) -> Option<ClientStatus> {
    let path = match std::env::var_os(STATUS_JSON_ENV) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => match edition {
            QoderEdition::Global => dirs_home().join(".qoder").join(".qoder-app-status.json"),
            QoderEdition::Cn => return None,
        },
    };
    let raw = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    Some(ClientStatus {
        logged_in: value
            .get("logged_in")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        name: value
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        email: value
            .get("email")
            .and_then(|v| v.as_str())
            .map(str::to_string),
    })
}

#[derive(Debug, Default, Clone)]
pub struct InfoPlist {
    pub bundle_id: Option<String>,
    pub version: Option<String>,
    pub executable: Option<String>,
}

/// 读 `Contents/Info.plist`。
///
/// Qoder 的 plist 是 XML 文本，直接按 key 顺序取值即可；
/// 万一哪天变成二进制 plist，这里返回 None，调用方按"识别失败"处理，
/// 而不是拿目录名猜版本。
pub fn read_info_plist(app: &Path) -> Option<InfoPlist> {
    let path = app.join("Contents").join("Info.plist");
    let raw = std::fs::read_to_string(&path).ok()?;
    if !raw.trim_start().starts_with("<?xml") && !raw.contains("<plist") {
        return None;
    }
    Some(InfoPlist {
        bundle_id: plist_string(&raw, "CFBundleIdentifier"),
        version: plist_string(&raw, "CFBundleShortVersionString"),
        executable: plist_string(&raw, "CFBundleExecutable"),
    })
}

/// 取 `<key>Name</key>` 后面第一个 `<string>…</string>`。
fn plist_string(xml: &str, key: &str) -> Option<String> {
    let needle = format!("<key>{key}</key>");
    let start = xml.find(&needle)? + needle.len();
    let rest = &xml[start..];
    let open = rest.find("<string>")? + "<string>".len();
    let close = rest[open..].find("</string>")? + open;
    let value = rest[open..close].trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::lock as lock_env;

    fn write_app(root: &Path, name: &str, bundle: &str, version: &str) -> PathBuf {
        let app = root.join(name);
        let contents = app.join("Contents");
        std::fs::create_dir_all(contents.join("MacOS")).unwrap();
        std::fs::write(
            contents.join("Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>{bundle}</string>
  <key>CFBundleExecutable</key><string>Qoder</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
</dict></plist>"#
            ),
        )
        .unwrap();
        std::fs::write(contents.join("MacOS").join("Qoder"), "#!/bin/sh\n").unwrap();
        app
    }

    #[test]
    fn editions_have_distinct_identifiers() {
        assert_ne!(
            QoderEdition::Cn.bundle_id(),
            QoderEdition::Global.bundle_id()
        );
        assert_ne!(
            QoderEdition::Cn.keychain_service(),
            QoderEdition::Global.keychain_service()
        );
        assert_ne!(
            QoderEdition::Cn.data_dir_prefix(),
            QoderEdition::Global.data_dir_prefix()
        );
    }

    #[test]
    fn parse_rejects_vague_platform() {
        assert!(QoderEdition::parse("qoder").is_none());
        assert_eq!(QoderEdition::parse("qoder_cn"), Some(QoderEdition::Cn));
        assert_eq!(
            QoderEdition::parse("qoder_global"),
            Some(QoderEdition::Global)
        );
        assert_eq!(
            QoderEdition::parse("International"),
            Some(QoderEdition::Global)
        );
    }

    #[test]
    fn identifies_edition_by_bundle_id_not_name() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        // 名字叫 "Qoder.app" 但 Bundle ID 是 CN 的：必须按 Bundle 认成 CN。
        write_app(tmp.path(), "Qoder.app", "com.qodercn.app", "1.2.3");
        write_app(tmp.path(), "Weird Name.app", "com.qoder.app", "0.5.7");
        std::env::set_var(APP_DIRS_ENV, tmp.path().to_string_lossy().to_string());

        let cn = find_app(QoderEdition::Cn).expect("cn");
        assert_eq!(cn.app_path, tmp.path().join("Qoder.app").to_string_lossy());

        let global = find_app(QoderEdition::Global).expect("global");
        assert_eq!(
            global.app_path,
            tmp.path().join("Weird Name.app").to_string_lossy()
        );
        assert_eq!(global.version.as_deref(), Some("0.5.7"));

        std::env::remove_var(APP_DIRS_ENV);
    }

    #[test]
    fn data_dirs_match_only_own_prefix() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("com.qoder.app.stable")).unwrap();
        std::fs::write(tmp.path().join("com.qoder.app.stable/auth.v1.dat"), b"v10").unwrap();
        std::fs::create_dir_all(tmp.path().join("com.qodercn.app")).unwrap();
        std::fs::write(tmp.path().join("com.qodercn.app/auth.v1.dat"), b"v10").unwrap();
        std::fs::create_dir_all(tmp.path().join("com.qoder.installer")).unwrap();
        std::env::set_var(SUPPORT_DIR_ENV, tmp.path().to_string_lossy().to_string());

        let global = find_data_dirs(QoderEdition::Global);
        assert_eq!(global.len(), 1);
        assert!(global[0].ends_with("com.qoder.app.stable"));

        let cn = find_data_dirs(QoderEdition::Cn);
        assert_eq!(cn.len(), 1);
        assert!(cn[0].ends_with("com.qodercn.app"));

        std::env::remove_var(SUPPORT_DIR_ENV);
    }

    #[test]
    fn empty_dirs_are_not_profiles() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("com.qoder.app.empty")).unwrap();
        std::env::set_var(SUPPORT_DIR_ENV, tmp.path().to_string_lossy().to_string());
        assert!(find_data_dirs(QoderEdition::Global).is_empty());
        std::env::remove_var(SUPPORT_DIR_ENV);
    }

    #[test]
    fn resolve_executable_handles_app_and_plain_binary() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let app = write_app(tmp.path(), "Qoder.app", "com.qoder.app", "0.5.7");
        let exe = resolve_executable(&app.to_string_lossy()).unwrap();
        assert!(exe.ends_with("Contents/MacOS/Qoder"));

        let script = tmp.path().join("mock-qoder");
        std::fs::write(&script, "#!/bin/sh\n").unwrap();
        assert_eq!(
            resolve_executable(&script.to_string_lossy()).unwrap(),
            script
        );
    }

    #[test]
    fn parallel_capability_is_not_guessed_for_cn() {
        // International 实测不能并行；CN 没装过，只能是 unknown。
        assert_eq!(QoderEdition::Global.parallel_instances_label(), "no");
        assert_eq!(QoderEdition::Cn.parallel_instances_label(), "unknown");
        assert!(QoderEdition::Global.verified_on_this_machine());
        assert!(!QoderEdition::Cn.verified_on_this_machine());
    }
}
