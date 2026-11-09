//! 端到端：真的把助手当子进程跑起来，用 JSONL 说话。
//!
//! 这一层是最有价值的回归——Rust 单测覆盖不到"stdout 混进日志"这类问题。

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use serde_json::{json, Value};

const BIN: &str = env!("CARGO_BIN_EXE_aicc-account-worker");

struct Session {
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    stdout: BufReader<std::process::ChildStdout>,
    _data_dir: tempfile::TempDir,
}

impl Session {
    fn start() -> Self {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let mut child = Command::new(BIN)
            // 刻意不传 WB_SWITCH_HOME：助手应自行把它收进数据目录。
            .env_remove("WB_SWITCH_HOME")
            .env("AICC_ACCOUNT_DATA_DIR", data_dir.path())
            .env("AICC_VERSION", "2.12.2-test")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("启动助手失败");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            _data_dir: data_dir,
        }
    }

    fn send(&mut self, value: Value) {
        let line = serde_json::to_string(&value).expect("序列化");
        let stdin = self.stdin.as_mut().expect("stdin 已关闭");
        writeln!(stdin, "{line}").expect("写 stdin");
        stdin.flush().expect("flush");
    }

    /// 关掉管道，模拟宿主 App 退出。
    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        let n = self.stdout.read_line(&mut line).expect("读 stdout");
        assert!(n > 0, "助手没有输出就退出了");
        serde_json::from_str(&line).expect("stdout 必须只有 JSON，不能混日志")
    }

    fn call(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        self.read()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn health_round_trip_over_stdio() {
    let mut session = Session::start();
    let response = session.call(1, "health", json!({}));

    assert_eq!(response["id"], json!(1));
    assert_eq!(response["jsonrpc"], json!("2.0"));

    let result = &response["result"];
    assert_eq!(result["protocol"], json!(1), "协议版本必须是 1");
    assert_eq!(result["worker"]["aiccVersion"], json!("2.12.2-test"));
    assert_eq!(result["upstream"]["version"], json!("0.1.62"));
    assert!(
        result["database"]["ok"].as_bool().unwrap_or(false),
        "数据库应初始化成功"
    );
    assert_eq!(result["database"]["schemaVersion"], json!(1));
    assert_eq!(
        result["paths"]["wbSwitchHomeOwned"],
        json!(true),
        "助手应自行接管 WB_SWITCH_HOME"
    );
    let wb_home = result["paths"]["wbSwitchHome"].as_str().unwrap_or_default();
    assert!(
        wb_home.ends_with("/wb-switch-home"),
        "WB_SWITCH_HOME 必须落在数据目录下，实际：{wb_home}"
    );
    assert!(
        !wb_home.contains(".wb-switch"),
        "不能指向用户自己的 ~/.wb-switch"
    );
    assert!(result["today"].as_str().unwrap().len() == 10);
    assert_eq!(result["status"], json!("ok"));
}

#[test]
fn ping_then_shutdown_exits_cleanly() {
    let mut session = Session::start();

    let ping = session.call(1, "ping", json!({}));
    assert_eq!(ping["result"]["pong"], json!(true));

    let shutdown = session.call(2, "worker.shutdown", json!({}));
    assert_eq!(shutdown["result"]["shuttingDown"], json!(true));

    // 关掉 stdin 之后进程必须自己退出，不能挂着。
    session.close_stdin();
    let status = session.child.wait().expect("等待退出");
    assert!(status.success(), "正常关闭应返回 0，实际 {status:?}");
}

#[test]
fn unknown_method_returns_method_not_found_and_keeps_running() {
    let mut session = Session::start();

    let error = session.call(1, "nope.nothing", json!({}));
    assert_eq!(error["error"]["code"], json!(-32601));

    // 出错不能把进程带走。
    let ping = session.call(2, "ping", json!({}));
    assert_eq!(ping["result"]["pong"], json!(true));
}

#[test]
fn invalid_json_returns_parse_error() {
    let mut session = Session::start();
    {
        let stdin = session.stdin.as_mut().expect("stdin");
        writeln!(stdin, "{{{{ not json").expect("写");
        stdin.flush().expect("flush");
    }
    let response = session.read();
    assert_eq!(response["error"]["code"], json!(-32700));
    assert_eq!(response["id"], Value::Null);
}

#[test]
fn account_lifecycle_over_rpc() {
    let mut session = Session::start();

    let created = session.call(
        1,
        "accounts.upsert",
        json!({"platform": "qoder_cn", "uid": "u-1", "nickname": "测试", "checkinEnabled": true}),
    );
    let id = created["result"]["id"]
        .as_str()
        .expect("返回 id")
        .to_string();
    assert_eq!(created["result"]["checkinEnabled"], json!(true));

    // 重复添加同一 platform+uid 必须幂等。
    let again = session.call(
        2,
        "accounts.upsert",
        json!({"platform": "qoder_cn", "uid": "u-1", "nickname": "改名"}),
    );
    assert_eq!(again["result"]["id"], json!(id));
    assert_eq!(again["result"]["nickname"], json!("改名"));

    let listed = session.call(3, "accounts.list", json!({"platform": "qoder_cn"}));
    assert_eq!(listed["result"]["accounts"].as_array().unwrap().len(), 1);

    let deleted = session.call(4, "accounts.delete", json!({"id": id}));
    assert_eq!(deleted["result"]["deleted"], json!(true));

    let missing = session.call(5, "accounts.get", json!({"id": id}));
    assert_eq!(missing["error"]["code"], json!(1003));
}

#[test]
fn bad_params_are_rejected_with_specific_code() {
    let mut session = Session::start();
    let response = session.call(
        1,
        "accounts.upsert",
        json!({"platform": "codex", "uid": "x"}),
    );
    assert_eq!(response["error"]["code"], json!(-32602));
}

#[test]
fn notifications_get_no_response() {
    let mut session = Session::start();
    // 不带 id 的请求不应产生响应；紧接着的 ping 必须是第 1 条响应。
    session.send(json!({"jsonrpc": "2.0", "method": "ping", "params": {}}));
    let ping = session.call(7, "ping", json!({}));
    assert_eq!(ping["id"], json!(7));
}
