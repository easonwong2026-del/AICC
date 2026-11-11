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
    _magpie_dir: Option<tempfile::TempDir>,
}

impl Session {
    fn start() -> Self {
        Self::spawn(None)
    }

    /// 带一份假的 Magpie 配置启动：`plugin-auth.json` + `plugin-providers.json`。
    fn with_magpie() -> Self {
        let magpie = tempfile::tempdir().expect("magpie tempdir");
        std::fs::write(
            magpie.path().join("plugin-auth.json"),
            json!({
                "codex": {"access": "codex-token", "uid": "codex-uid"},
                "workbuddy": {
                    "access": "a".repeat(1620),
                    "refresh": "r".repeat(1622),
                    "uid": "6f1c2d94-1111-2222-3333-444455556666",
                    "domain": "www.workbuddy.cn",
                    "expires": 1_795_354_934_547i64
                }
            })
            .to_string(),
        )
        .expect("写 plugin-auth");
        std::fs::write(
            magpie.path().join("plugin-providers.json"),
            json!([{
                "id": "workbuddy",
                "signedIn": true,
                "accounts": [{"key": "workbuddy", "accountId": "卡里"}]
            }])
            .to_string(),
        )
        .expect("写 plugin-providers");
        Self::spawn(Some(magpie))
    }

    fn spawn(magpie: Option<tempfile::TempDir>) -> Self {
        let data_dir = tempfile::tempdir().expect("tempdir");
        let mut command = Command::new(BIN);
        command
            // 刻意不传 WB_SWITCH_HOME：助手应自行把它收进数据目录。
            .env_remove("WB_SWITCH_HOME")
            .env("AICC_ACCOUNT_DATA_DIR", data_dir.path())
            .env("AICC_VERSION", "2.12.2-test")
            // 测试不碰真实钥匙串：并行跑不互相抢，也不在开发机上留假令牌。
            .env("AICC_SECURE_STORE", "memory")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        match &magpie {
            Some(dir) => {
                command.env("MAGPIE_CONFIG_DIR", dir.path());
            }
            None => {
                // 别读到开发机自己的 Magpie 数据。
                command.env("MAGPIE_CONFIG_DIR", data_dir.path().join("no-magpie"));
            }
        }
        let mut child = command.spawn().expect("启动助手失败");
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Self {
            child,
            stdin: Some(stdin),
            stdout,
            _data_dir: data_dir,
            _magpie_dir: magpie,
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
    // 断言与 health 自报的 latestSchemaVersion 一致，避免每次加迁移都要改数字。
    assert_eq!(
        result["database"]["schemaVersion"], result["database"]["latestSchemaVersion"],
        "schema 迁移要跟到最新版本"
    );
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

    // 重复添加同一 platform+uid 必须幂等，且没传的字段要保持原值。
    let again = session.call(
        2,
        "accounts.upsert",
        json!({"platform": "qoder_cn", "uid": "u-1", "nickname": "改名"}),
    );
    assert_eq!(again["result"]["id"], json!(id));
    assert_eq!(again["result"]["nickname"], json!("改名"));
    assert_eq!(
        again["result"]["checkinEnabled"],
        json!(true),
        "没传 checkinEnabled 不能把签到开关重置成 false"
    );

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

/// Magpie 导入的预览：能列出账号、带昵称，但**绝不能**把令牌带出来。
#[test]
fn magpie_preview_over_stdio_never_leaks_tokens() {
    let mut session = Session::with_magpie();

    let reply = session.call(1, "providers.magpie.accounts", json!({}));
    let result = &reply["result"];
    assert_eq!(result["available"], json!(true), "{reply}");
    let accounts = result["accounts"].as_array().expect("accounts 数组");
    assert_eq!(accounts.len(), 1, "codex 那条不算 WorkBuddy");
    assert_eq!(accounts[0]["nickname"], json!("卡里"));
    assert_eq!(accounts[0]["imported"], json!(false));
    assert_eq!(accounts[0]["expired"], json!(false));

    let text = reply.to_string();
    assert!(
        !text.contains(&"a".repeat(64)),
        "access token 泄漏到 RPC：{text}"
    );
    assert!(
        !text.contains(&"r".repeat(64)),
        "refresh token 泄漏到 RPC：{text}"
    );

    session.call(2, "worker.shutdown", json!({}));
}

/// 没有 Magpie 时必须给出可读原因，而不是报错崩掉。
#[test]
fn magpie_preview_without_magpie_is_unavailable() {
    let mut session = Session::start();
    let reply = session.call(1, "providers.magpie.accounts", json!({}));
    assert_eq!(reply["result"]["available"], json!(false), "{reply}");
    assert_eq!(reply["result"]["kind"], json!("not_installed"));
    session.call(2, "worker.shutdown", json!({}));
}

/// 旅行汇总：只读本地快照，带出账号与开关，且**不含令牌**。
/// 顺手把开关（默认关闭 → 打开 → 关闭）一起验了，两次导入会互相抢钥匙串。
#[test]
fn travel_summary_is_local_and_token_free() {
    let mut session = Session::with_magpie();
    let imported = session.call(1, "auth.magpie.import", json!({}));
    assert_eq!(imported["result"]["imported"], json!(1), "{imported}");

    let reply = session.call(2, "travel.summary", json!({}));
    let result = &reply["result"];
    let accounts = result["accounts"].as_array().expect("accounts 数组");
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0]["travelEnabled"], json!(false), "默认必须关闭");
    assert_eq!(accounts[0]["travel"], json!(null), "没查过就没有快照");
    assert_eq!(
        result["auto"]["runsOnlyWhileAppRunning"],
        json!(true),
        "必须如实说明调度依赖 AICC.app 常驻"
    );
    assert!(
        result["auto"]["owner"]
            .as_str()
            .unwrap_or_default()
            .contains("aicc-account-worker"),
        "要说清承载进程：{reply}"
    );
    assert_eq!(result["auto"]["departIntervalSecs"], json!(1800));
    assert_eq!(result["auto"]["claimIntervalSecs"], json!(900));

    let text = reply.to_string();
    assert!(
        !text.contains(&"a".repeat(64)),
        "access token 泄漏到旅行汇总：{text}"
    );

    session.call(3, "worker.shutdown", json!({}));
}

/// 开关：不存在的账号要报 not_found，存在则真的落库。
#[test]
fn travel_toggle_requires_an_existing_account() {
    let mut session = Session::with_magpie();
    let missing = session.call(
        1,
        "travel.setEnabled",
        json!({"id": "nope", "enabled": true}),
    );
    assert_eq!(missing["error"]["code"], json!(1003), "{missing}");

    session.call(2, "auth.magpie.import", json!({}));
    let summary = session.call(3, "travel.summary", json!({}));
    let id = summary["result"]["accounts"][0]["accountId"]
        .as_str()
        .expect("accountId")
        .to_string();

    let toggled = session.call(4, "travel.setEnabled", json!({"id": id, "enabled": true}));
    assert_eq!(toggled["result"]["travelEnabled"], json!(true), "{toggled}");

    let after = session.call(5, "travel.summary", json!({}));
    assert_eq!(after["result"]["accounts"][0]["travelEnabled"], json!(true));
    assert_eq!(after["result"]["auto"]["enabledAccounts"], json!(1));

    let off = session.call(6, "travel.setEnabled", json!({"id": id, "enabled": false}));
    assert_eq!(off["result"]["travelEnabled"], json!(false));
    session.call(7, "worker.shutdown", json!({}));
}

/// 没有任何账号开自动旅行时，一轮检查必须立刻返回 idle，不发请求。
#[test]
fn travel_tick_without_enabled_accounts_is_idle() {
    let mut session = Session::start();
    let reply = session.call(1, "travel.auto.tick", json!({}));
    assert_eq!(reply["result"]["status"], json!("idle"), "{reply}");
    assert_eq!(reply["result"]["reason"], json!("no_enabled_accounts"));
    session.call(2, "worker.shutdown", json!({}));
}
