//! Qoder 客户端进程：探测、归属、启动、关闭、对账。
//!
//! 这一层的核心原则是**"不知道就停下来"**：
//!
//! | 探测结果 | 含义 | 允许做什么 |
//! | --- | --- | --- |
//! | `Stopped` | 有确凿证据说明客户端没在跑（`pgrep` 明确返回"无匹配"） | 可以进入切换流程 |
//! | `Running(pids)` | 有进程在跑 | 只有确认是本 Profile 的进程才发信号 |
//! | `Unknown` | 探测工具本身失败（spawn 失败 / 退出码异常） | **禁止**任何 live 认证写入、禁止发信号 |
//!
//! 同理，`liveness(pid)` 只有在拿到"进程不存在"的确凿证据时才返回 `Gone`；
//! `ps` 读不到不等于进程死了，必须再用 `kill -0` 拿 ESRCH 才算数。
//!
//! 归属判定只认启动时注入的 `AICC_QODER_PROFILE=<profile_id>`：
//! "可执行文件路径相同" 不再算证据——同一份 App 用户也可能自己开一个。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 启动时注入的归属标记。值 = profile_id。
pub const OWNER_ENV: &str = "AICC_QODER_PROFILE";
/// 测试注入的假 `ps`（按 BSD/macOS 的参数风格调用）。
pub const PS_BIN_ENV: &str = "AICC_PS_BIN";
/// 测试注入的假 `pgrep`。
pub const PGREP_BIN_ENV: &str = "AICC_PGREP_BIN";
/// 测试注入的假 `kill`。
pub const KILL_BIN_ENV: &str = "AICC_KILL_BIN";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessInfo {
    pub pid: i64,
    pub argv: Vec<String>,
    /// 命令行里解析出的归属 Profile（`AICC_QODER_PROFILE=` 的值）。
    pub owner_profile: Option<String>,
}

/// "客户端在不在跑"的三态。**不要把它压成 bool。**
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Running {
    Stopped,
    Running(Vec<i64>),
    Unknown(String),
}

/// 某个 PID 的存活三态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Liveness {
    Alive,
    Gone,
    Unknown(String),
}

fn tool(env_key: &str, default: &str) -> PathBuf {
    match std::env::var_os(env_key) {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(default),
    }
}

fn ps_bin() -> PathBuf {
    tool(PS_BIN_ENV, "/bin/ps")
}

fn pgrep_bin() -> PathBuf {
    tool(PGREP_BIN_ENV, "/usr/bin/pgrep")
}

fn kill_bin() -> PathBuf {
    tool(KILL_BIN_ENV, "/bin/kill")
}

/// 读进程命令行。读不到返回 `None`（**不代表进程已经退出**）。
pub fn describe(pid: i64) -> Option<ProcessInfo> {
    if pid <= 1 {
        return None;
    }
    let args = pid.to_string();
    let output = run_ps(&["-p", &args, "-E", "-ww", "-o", "args="])
        .or_else(|| run_ps(&["-p", &args, "-ww", "-o", "args="]))?;
    let line = output.trim();
    if line.is_empty() {
        return None;
    }
    let argv: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    if argv.is_empty() {
        return None;
    }
    let prefix = format!("{OWNER_ENV}=");
    let owner_profile = argv
        .iter()
        .find_map(|arg| arg.strip_prefix(&prefix).map(str::to_string))
        .filter(|v| !v.is_empty());
    Some(ProcessInfo {
        pid,
        argv,
        owner_profile,
    })
}

fn run_ps(args: &[&str]) -> Option<String> {
    let output = Command::new(ps_bin())
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

/// 客户端是否在运行。`pgrep` 明确"无匹配"(exit 1) 才算 `Stopped`；
/// 其它任何异常都是 `Unknown`，调用方必须停下来。
pub fn probe(executable: &Path) -> Running {
    let output = match Command::new(pgrep_bin())
        .arg("-f")
        .arg(executable.to_string_lossy().to_string())
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(output) => output,
        Err(err) => return Running::Unknown(format!("pgrep 无法执行：{err}")),
    };

    match output.status.code() {
        Some(1) => Running::Stopped,
        Some(0) => {
            let pids: Vec<i64> = String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| line.trim().parse::<i64>().ok())
                .filter(|pid| *pid > 1)
                .collect();
            if pids.is_empty() {
                // 退出码说"找到了"，输出却是空的：两边矛盾，不能当没跑。
                Running::Unknown("pgrep 报告有匹配但没给出 PID".into())
            } else {
                Running::Running(pids)
            }
        }
        other => Running::Unknown(format!(
            "pgrep 退出码异常：{other:?}（stderr: {}）",
            String::from_utf8_lossy(&output.stderr).trim()
        )),
    }
}

/// 某个 PID 还在不在。
///
/// `ps` 读不到时会再用 `kill -0`：拿到 "No such process" 才算 `Gone`，
/// 其它情况（权限不足、kill 不可用）都返回 `Unknown` —— **宁可不结束，也不误判。**
pub fn liveness(pid: i64) -> Liveness {
    if pid <= 1 {
        return Liveness::Gone;
    }
    if describe(pid).is_some() {
        return Liveness::Alive;
    }
    let output = match Command::new(kill_bin())
        .arg("-0")
        .arg(pid.to_string())
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(output) => output,
        Err(err) => return Liveness::Unknown(format!("kill -0 无法执行：{err}")),
    };
    if output.status.success() {
        return Liveness::Alive;
    }
    let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
    if stderr.contains("no such process") {
        return Liveness::Gone;
    }
    Liveness::Unknown(format!(
        "kill -0 失败但没有明确的'进程不存在'证据：{}",
        stderr.trim()
    ))
}

/// 归属判定：只认启动时注入的标记，且必须与目标 Profile 一致。
pub fn owned_by(info: &ProcessInfo, profile_id: &str) -> bool {
    info.owner_profile.as_deref() == Some(profile_id)
}

/// 发信号。`force = SIGKILL`，否则 `SIGTERM`。
///
/// **调用前必须已经确认归属**（`owned_by`），这个函数自己不做判断。
pub fn signal(pid: i64, force: bool) -> bool {
    let flag = if force { "-KILL" } else { "-TERM" };
    Command::new(kill_bin())
        .arg(flag)
        .arg(pid.to_string())
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 等进程消失。**只有拿到 `Gone` 才返回 true**，超时或探测不确定都返回 false。
pub fn wait_gone(pid: i64, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match liveness(pid) {
            Liveness::Gone => return true,
            Liveness::Alive | Liveness::Unknown(_) => {}
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

    fn write_script(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    fn info(argv: &[&str], owner: Option<&str>) -> ProcessInfo {
        ProcessInfo {
            pid: 100,
            argv: argv.iter().map(|s| s.to_string()).collect(),
            owner_profile: owner.map(str::to_string),
        }
    }

    #[test]
    fn ownership_requires_marker_not_just_path() {
        let exe = "/Applications/Qoder.app/Contents/MacOS/Qoder";
        // 同一份可执行文件、没有标记 → 不是我们的。
        assert!(!owned_by(&info(&[exe], None), "qp_1"));
        // 标记属于别的 Profile → 也不是我们的。
        assert!(!owned_by(&info(&[exe], Some("qp_2")), "qp_1"));
        assert!(owned_by(&info(&[exe], Some("qp_1")), "qp_1"));
    }

    #[test]
    fn describe_parses_owner_marker() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("ps");
        write_script(
            &stub,
            "#!/bin/sh\necho '/fake/Qoder AICC_QODER_PROFILE=qp_abc'\n",
        );
        std::env::set_var(PS_BIN_ENV, stub.to_string_lossy().to_string());
        let described = describe(4242).expect("stub output");
        std::env::remove_var(PS_BIN_ENV);
        assert_eq!(described.owner_profile.as_deref(), Some("qp_abc"));
        assert!(owned_by(&described, "qp_abc"));
        assert!(!owned_by(&described, "qp_other"));
    }

    #[test]
    fn pgrep_no_match_is_confirmed_stopped() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("pgrep");
        write_script(&stub, "#!/bin/sh\nexit 1\n");
        std::env::set_var(PGREP_BIN_ENV, stub.to_string_lossy().to_string());
        let state = probe(Path::new("/fake/Qoder"));
        std::env::remove_var(PGREP_BIN_ENV);
        assert_eq!(state, Running::Stopped);
    }

    #[test]
    fn pgrep_error_is_unknown_not_stopped() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("pgrep");
        write_script(&stub, "#!/bin/sh\necho boom >&2\nexit 3\n");
        std::env::set_var(PGREP_BIN_ENV, stub.to_string_lossy().to_string());
        let state = probe(Path::new("/fake/Qoder"));
        std::env::remove_var(PGREP_BIN_ENV);
        assert!(matches!(state, Running::Unknown(_)), "got {state:?}");
    }

    #[test]
    fn pgrep_missing_binary_is_unknown() {
        let _guard = lock_env();
        std::env::set_var(PGREP_BIN_ENV, "/nonexistent/pgrep");
        let state = probe(Path::new("/fake/Qoder"));
        std::env::remove_var(PGREP_BIN_ENV);
        assert!(matches!(state, Running::Unknown(_)), "got {state:?}");
    }

    #[test]
    fn pgrep_found_but_empty_output_is_unknown() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let stub = tmp.path().join("pgrep");
        write_script(&stub, "#!/bin/sh\nexit 0\n");
        std::env::set_var(PGREP_BIN_ENV, stub.to_string_lossy().to_string());
        let state = probe(Path::new("/fake/Qoder"));
        std::env::remove_var(PGREP_BIN_ENV);
        assert!(matches!(state, Running::Unknown(_)), "got {state:?}");
    }

    #[test]
    fn liveness_reports_gone_only_with_esrch() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let ps = tmp.path().join("ps");
        write_script(&ps, "#!/bin/sh\nexit 1\n");
        let kill = tmp.path().join("kill");
        write_script(
            &kill,
            "#!/bin/sh\necho 'kill: 4242: No such process' >&2\nexit 1\n",
        );
        std::env::set_var(PS_BIN_ENV, ps.to_string_lossy().to_string());
        std::env::set_var(KILL_BIN_ENV, kill.to_string_lossy().to_string());
        let state = liveness(4242);
        std::env::remove_var(PS_BIN_ENV);
        std::env::remove_var(KILL_BIN_ENV);
        assert_eq!(state, Liveness::Gone);
    }

    #[test]
    fn liveness_unknown_when_kill_fails_without_esrch() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let ps = tmp.path().join("ps");
        write_script(&ps, "#!/bin/sh\nexit 1\n");
        let kill = tmp.path().join("kill");
        write_script(
            &kill,
            "#!/bin/sh\necho 'operation not permitted' >&2\nexit 1\n",
        );
        std::env::set_var(PS_BIN_ENV, ps.to_string_lossy().to_string());
        std::env::set_var(KILL_BIN_ENV, kill.to_string_lossy().to_string());
        let state = liveness(4242);
        std::env::remove_var(PS_BIN_ENV);
        std::env::remove_var(KILL_BIN_ENV);
        assert!(matches!(state, Liveness::Unknown(_)), "got {state:?}");
    }

    #[test]
    fn liveness_alive_when_kill_0_succeeds() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let ps = tmp.path().join("ps");
        write_script(&ps, "#!/bin/sh\nexit 1\n");
        let kill = tmp.path().join("kill");
        write_script(&kill, "#!/bin/sh\nexit 0\n");
        std::env::set_var(PS_BIN_ENV, ps.to_string_lossy().to_string());
        std::env::set_var(KILL_BIN_ENV, kill.to_string_lossy().to_string());
        assert_eq!(liveness(4242), Liveness::Alive);
        std::env::remove_var(PS_BIN_ENV);
        std::env::remove_var(KILL_BIN_ENV);
    }

    #[test]
    fn wait_gone_never_returns_true_on_unknown() {
        let _guard = lock_env();
        let tmp = tempfile::tempdir().unwrap();
        let ps = tmp.path().join("ps");
        write_script(&ps, "#!/bin/sh\nexit 1\n");
        let kill = tmp.path().join("kill");
        write_script(&kill, "#!/bin/sh\nexit 2\n");
        std::env::set_var(PS_BIN_ENV, ps.to_string_lossy().to_string());
        std::env::set_var(KILL_BIN_ENV, kill.to_string_lossy().to_string());
        let gone = wait_gone(4242, Duration::from_millis(300));
        std::env::remove_var(PS_BIN_ENV);
        std::env::remove_var(KILL_BIN_ENV);
        assert!(!gone, "探测不确定时不能报告'已退出'");
    }
}
