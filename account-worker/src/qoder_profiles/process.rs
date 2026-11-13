//! Qoder 客户端进程：启动、核实、关闭、对账。
//!
//! 三条铁律（PRD §7）：
//!
//! 1. **不认"启动成功"这四个字**。`Command::spawn` 成功只说明 fork/exec 没报错，
//!    必须再读一次真实进程，确认它还在跑。
//! 2. **只碰自己拉起来的进程**。归属判定看启动时注入的环境变量标记；
//!    `ps -E` 不可用时退回"可执行文件路径一致 + 运行记录里有我们写下的 PID"。
//! 3. **PID 会复用**。AICC 重启后旧 PID 可能已经属于别的程序，必须重新核实，
//!    核实不上就当没在跑。
//!
//! 注意：Qoder 忽略 `--user-data-dir`（M0 实测），所以这里**不再**靠
//! "命令行里有没有某个目录"来认亲。

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 启动时注入的归属标记。值 = profile_id。
pub const OWNER_ENV: &str = "AICC_QODER_PROFILE";
/// 测试注入的假 `ps`（按 BSD/macOS 的参数风格调用）。
pub const PS_BIN_ENV: &str = "AICC_PS_BIN";
/// 测试注入的假 `pgrep`。
pub const PGREP_BIN_ENV: &str = "AICC_PGREP_BIN";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: i64,
    /// 完整命令行（含 argv[0]）。
    pub argv: Vec<String>,
    /// 命令行里出现了归属标记（说明是本进程注入的环境变量）。
    pub has_owner_marker: bool,
}

impl ProcessInfo {
    /// 取 `--flag=value` 形式参数的值。只有测试用得到。
    #[cfg(test)]
    pub fn arg_after(&self, prefix: &str) -> Option<&str> {
        self.argv
            .iter()
            .find_map(|arg| arg.strip_prefix(prefix))
            .filter(|v| !v.is_empty())
    }
}

fn ps_bin() -> PathBuf {
    match std::env::var_os(PS_BIN_ENV) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from("/bin/ps"),
    }
}

fn pgrep_bin() -> PathBuf {
    match std::env::var_os(PGREP_BIN_ENV) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from("/usr/bin/pgrep"),
    }
}

/// 读进程命令行。读不到（进程不存在 / 权限不足 / ps 不可用）一律返回 `None`。
pub fn describe(pid: i64) -> Option<ProcessInfo> {
    if pid <= 1 {
        return None;
    }
    let with_env = run_ps(&["-p", &pid.to_string(), "-E", "-ww", "-o", "args="]);
    let output = match with_env {
        Some(text) => text,
        None => run_ps(&["-p", &pid.to_string(), "-ww", "-o", "args="])?,
    };
    let line = output.trim();
    if line.is_empty() {
        return None;
    }
    let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    if argv.is_empty() {
        return None;
    }
    Some(ProcessInfo {
        pid,
        argv,
        has_owner_marker: line.contains(&format!("{OWNER_ENV}=")),
    })
}

fn run_ps(args: &[&str]) -> Option<String> {
    let output = Command::new(ps_bin())
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// 归属判定。
///
/// - 命令行里带 `AICC_QODER_PROFILE=` 标记 → 一定是 AICC 拉的；
/// - 没有标记（`ps -E` 不可用）→ 要求 argv[0] 与记录的可执行文件一致。
///
/// 运行记录本来就是"我们 spawn 时写下的 PID"，再加可执行文件一致已经足够，
/// 而且绝不会误伤用户手动启动的另一份客户端（它没有运行记录）。
pub fn is_owned(info: &ProcessInfo, executable: &Path) -> bool {
    if info.has_owner_marker {
        return true;
    }
    info.argv
        .first()
        .map(|arg0| arg0 == &executable.to_string_lossy())
        .unwrap_or(false)
}

/// 找出当前正在跑的同一份客户端。切换账号前必须先确认它已经退出。
///
/// 用 `pgrep -f <可执行文件绝对路径>`，只匹配这个路径。
pub fn find_running(executable: &Path) -> Vec<i64> {
    let output = Command::new(pgrep_bin())
        .arg("-f")
        .arg(executable.to_string_lossy().to_string())
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let Ok(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<i64>().ok())
        .filter(|pid| *pid > 1)
        .collect()
}

pub fn spawn(executable: &Path, args: &[String], profile_id: &str) -> io::Result<u32> {
    let mut command = Command::new(executable);
    command
        .args(args)
        .env(OWNER_ENV, profile_id)
        // 主机进程（尤其是从终端/IDE 继承来的环境）可能带着
        // `ELECTRON_RUN_AS_NODE=1`，那会让 Electron 客户端退化成 node，
        // 一行代码都没跑就退出。隔离启动时必须把这几个变量摘掉。
        .env_remove("ELECTRON_RUN_AS_NODE")
        .env_remove("ELECTRON_NO_ATTACH_CONSOLE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let child = command.spawn()?;
    Ok(child.id())
}

/// 发信号。`force = SIGKILL`，否则 `SIGTERM`。
pub fn signal(pid: i64, force: bool) -> bool {
    let flag = if force { "-KILL" } else { "-TERM" };
    Command::new("/bin/kill")
        .arg(flag)
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 只探测"这个 PID 还在不在"，不读命令行。
///
/// 有些环境（沙箱、受限终端）禁掉了 `ps`，但 `kill -0` 仍可用。
/// 这种情况下我们只能确认"进程活着"，不能确认"它是我们那个实例"，
/// 调用方必须把 `verified` 标成 false，并且**不许**据此发终止信号。
pub fn is_alive(pid: i64) -> bool {
    if pid <= 1 {
        return false;
    }
    Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 等进程消失。超时返回 false（调用方决定要不要升级到 SIGKILL）。
pub fn wait_gone(pid: i64, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if describe(pid).is_none() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::lock as lock_env;

    fn info(argv: &[&str], marker: bool) -> ProcessInfo {
        ProcessInfo {
            pid: 100,
            argv: argv.iter().map(|s| s.to_string()).collect(),
            has_owner_marker: marker,
        }
    }

    fn write_script(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn owner_marker_wins() {
        let i = info(&["/Applications/Qoder.app/Contents/MacOS/Qoder"], true);
        assert!(is_owned(&i, Path::new("/whatever")));
    }

    #[test]
    fn exe_path_must_match_without_marker() {
        let exe = Path::new("/Applications/Qoder.app/Contents/MacOS/Qoder");
        assert!(is_owned(
            &info(&["/Applications/Qoder.app/Contents/MacOS/Qoder"], false),
            exe
        ));
        assert!(!is_owned(
            &info(&["/Applications/Other.app/Contents/MacOS/Other"], false),
            exe
        ));
    }

    #[test]
    fn describe_uses_injected_ps() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("ps");
        write_script(
            &stub,
            "#!/bin/sh\necho '/fake/Qoder --flag AICC_QODER_PROFILE=qp_1'\n",
        );
        std::env::set_var(PS_BIN_ENV, stub.to_string_lossy().to_string());
        let described = describe(4242).expect("stub output");
        std::env::remove_var(PS_BIN_ENV);

        assert_eq!(described.pid, 4242);
        assert!(described.has_owner_marker);
        assert_eq!(
            described.argv.first().map(String::as_str),
            Some("/fake/Qoder")
        );
        assert!(described.arg_after("--user-data-dir=").is_none());
    }

    #[test]
    fn dead_pid_has_no_description() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("ps");
        write_script(&stub, "#!/bin/sh\nexit 1\n");
        std::env::set_var(PS_BIN_ENV, stub.to_string_lossy().to_string());
        assert!(describe(4242).is_none());
        std::env::remove_var(PS_BIN_ENV);
    }

    #[test]
    fn find_running_parses_pgrep_output() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("pgrep");
        write_script(&stub, "#!/bin/sh\nprintf '111\\n222\\nnot-a-pid\\n'\n");
        std::env::set_var(PGREP_BIN_ENV, stub.to_string_lossy().to_string());
        let pids = find_running(Path::new("/fake/Qoder"));
        std::env::remove_var(PGREP_BIN_ENV);
        assert_eq!(pids, vec![111, 222]);
    }

    #[test]
    fn alive_probe_rejects_bogus_pids() {
        // 0/1 与不可能存在的 PID 一律当"不在"。
        assert!(!is_alive(0));
        assert!(!is_alive(1));
        assert!(!is_alive(999_999));
    }

    #[test]
    fn find_running_returns_empty_when_pgrep_fails() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("pgrep");
        write_script(&stub, "#!/bin/sh\nexit 1\n");
        std::env::set_var(PGREP_BIN_ENV, stub.to_string_lossy().to_string());
        assert!(find_running(Path::new("/fake/Qoder")).is_empty());
        std::env::remove_var(PGREP_BIN_ENV);
    }
}
