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
        Self::spawn(None, true)
    }

    /// 不打开 `AICC_ALLOW_SECURE_RPC`：验证通用 secure.* 默认关闭。
    fn start_without_secure_rpc() -> Self {
        Self::spawn(None, false)
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
        Self::spawn(Some(magpie), true)
    }

    fn spawn(magpie: Option<tempfile::TempDir>, allow_secure_rpc: bool) -> Self {
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
        if allow_secure_rpc {
            command.env("AICC_ALLOW_SECURE_RPC", "1");
        } else {
            command.env_remove("AICC_ALLOW_SECURE_RPC");
        }
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

// ---------------------------------------------------------------------------
// PR #65：凭据归属与 RPC 脱敏
// ---------------------------------------------------------------------------

/// 假 Magpie 里的 uid，与 `Session::with_magpie` 写死的一致。
const MAGPIE_UID: &str = "6f1c2d94-1111-2222-3333-444455556666";

fn access_key(uid: &str) -> String {
    format!("workbuddy/{uid}/access")
}

/// 新键：`workbuddy/<account_id>/<kind>/<which>`。
fn scoped_key(account_id: &str, kind: &str, which: &str) -> String {
    format!("workbuddy/{account_id}/{kind}/{which}")
}

/// `secure.get` 只回指纹：就算调用方明确要 `reveal`，也不能拿到明文。
#[test]
fn secure_get_never_reveals_plaintext() {
    let mut session = Session::start();
    let secret = "super-secret-access-token-value";

    let key = scoped_key("0123456789abcdef", "own", "access");
    let set = session.call(1, "secure.set", json!({"key": key, "value": secret}));
    assert_eq!(set["result"]["stored"], json!(true), "{set}");

    let plain = session.call(2, "secure.get", json!({"key": key}));
    assert_eq!(plain["result"]["exists"], json!(true));
    assert_eq!(plain["result"]["length"], json!(secret.len()));

    let asked = session.call(3, "secure.get", json!({"key": key, "reveal": true}));
    assert_eq!(asked["result"]["revealed"], json!(false), "{asked}");
    assert!(
        asked["result"].get("value").is_none(),
        "reveal 不能再回明文：{asked}"
    );
    let text = asked.to_string();
    assert!(!text.contains(secret), "响应里出现了明文：{text}");

    // 旧 UID 键与 magpie/refresh 都不在白名单里。
    let legacy = session.call(
        4,
        "secure.set",
        json!({"key": access_key(MAGPIE_UID), "value": "x"}),
    );
    assert_eq!(legacy["error"]["code"], json!(-32602), "{legacy}");
    let forbidden = session.call(
        5,
        "secure.set",
        json!({"key": scoped_key("0123456789abcdef", "magpie", "refresh"), "value": "x"}),
    );
    assert_eq!(forbidden["error"]["code"], json!(-32602), "{forbidden}");

    session.call(6, "worker.shutdown", json!({}));
}

/// 重复导入：不新增账号、不产生第二行，第二次仍然更新同一个账号。
#[test]
fn repeated_magpie_import_is_idempotent() {
    let mut session = Session::with_magpie();

    let first = session.call(1, "auth.magpie.import", json!({}));
    assert_eq!(first["result"]["imported"], json!(1), "{first}");
    let second = session.call(2, "auth.magpie.import", json!({}));
    assert_eq!(second["result"]["imported"], json!(1), "{second}");

    let list = session.call(3, "accounts.list", json!({}));
    assert_eq!(list["result"]["accounts"].as_array().unwrap().len(), 1);
    let account = &list["result"]["accounts"][0];
    assert_eq!(account["authSource"], json!("magpie"));
    assert_eq!(account["uid"], json!(MAGPIE_UID));

    session.call(4, "worker.shutdown", json!({}));
}

/// 导入只新增借用授权：账号的昵称、签到开关这些用户字段不能被覆盖。
#[test]
fn import_adds_magpie_authorization_and_keeps_account_fields() {
    let mut session = Session::with_magpie();

    let created = session.call(
        1,
        "accounts.upsert",
        json!({
            "platform": "workbuddy",
            "uid": MAGPIE_UID,
            "nickname": "我自己登录的",
            // 授权字段是派生镜像：调用方写了也不生效
            "authStatus": "valid",
            "authSource": "own",
        }),
    );
    assert_eq!(created["result"]["nickname"], json!("我自己登录的"));
    assert_eq!(
        created["result"]["authorizations"],
        json!([]),
        "没有扫码就没有授权行：来源不该由 upsert 编出来"
    );

    let imported = session.call(2, "auth.magpie.import", json!({}));
    assert_eq!(imported["result"]["imported"], json!(1), "{imported}");

    let list = session.call(3, "accounts.list", json!({}));
    let account = &list["result"]["accounts"][0];
    assert_eq!(account["nickname"], json!("我自己登录的"), "昵称不能被覆盖");
    let kinds: Vec<&str> = account["authorizations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, vec!["magpie"], "导入只写借用授权");

    session.call(4, "worker.shutdown", json!({}));
}

/// 只有借用授权的账号刷新时必须走"重新同步"，不能去刷 Magpie 的 refresh。
#[test]
fn refresh_of_a_borrowed_account_resyncs_from_magpie() {
    let mut session = Session::with_magpie();
    session.call(1, "auth.magpie.import", json!({}));

    let list = session.call(2, "accounts.list", json!({}));
    let id = list["result"]["accounts"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let refreshed = session.call(3, "auth.workbuddy.refresh", json!({"id": id}));
    assert_eq!(refreshed["result"]["state"], json!("valid"), "{refreshed}");
    assert_eq!(refreshed["result"]["source"], json!("magpie"));
    assert_eq!(refreshed["result"]["resynced"], json!(true));

    let after = session.call(4, "accounts.get", json!({"id": id}));
    assert_eq!(after["result"]["authSource"], json!("magpie"));
    assert_eq!(
        after["result"]["authorizations"].as_array().unwrap().len(),
        1,
        "借用账号只有一份授权"
    );

    session.call(5, "worker.shutdown", json!({}));
}

/// 删除账号时要把它自己的凭据清干净。
#[test]
fn deleting_an_account_purges_its_credentials() {
    let mut session = Session::with_magpie();
    session.call(1, "auth.magpie.import", json!({}));

    let list = session.call(2, "accounts.list", json!({}));
    let id = list["result"]["accounts"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let magpie_key = scoped_key(&id, "magpie", "access");
    let before = session.call(3, "secure.get", json!({"key": magpie_key}));
    assert_eq!(before["result"]["exists"], json!(true), "{before}");

    session.call(4, "accounts.delete", json!({"id": id}));

    let after = session.call(5, "secure.get", json!({"key": magpie_key}));
    assert_eq!(
        after["error"]["code"],
        json!(1002),
        "凭据应该跟着账号一起删掉：{after}"
    );

    session.call(6, "worker.shutdown", json!({}));
}

// ---------------------------------------------------------------------------
// PR #66：多来源授权
// ---------------------------------------------------------------------------

/// 通用 secure.* 默认关闭：不给任何进程一条"任意键读写钥匙串"的通道。
#[test]
fn secure_rpc_is_closed_by_default() {
    let mut session = Session::start_without_secure_rpc();
    for (id, method) in [(1, "secure.set"), (2, "secure.get"), (3, "secure.delete")] {
        let reply = session.call(
            id,
            method,
            json!({"key": scoped_key("0123456789abcdef", "own", "access"), "value": "x"}),
        );
        assert_eq!(
            reply["error"]["code"],
            json!(-32601),
            "{method} 默认必须 method_not_found：{reply}"
        );
    }
    // 探针仍然可用：它不涉及任意键写入。
    let probe = session.call(4, "secure.status", json!({}));
    assert_eq!(probe["result"]["ok"], json!(true), "{probe}");
    session.call(5, "worker.shutdown", json!({}));
}

/// 首选来源可读可写，非法值被拒，未知 key 被拒。
#[test]
fn settings_round_trip_and_validation() {
    let mut session = Session::start();

    let default = session.call(1, "settings.get", json!({}));
    assert_eq!(
        default["result"]["settings"]["auth_preferred_source"],
        json!("magpie"),
        "默认 Magpie 优先"
    );

    let set = session.call(
        2,
        "settings.set",
        json!({"key": "auth_preferred_source", "value": "own"}),
    );
    assert_eq!(set["result"]["value"], json!("own"), "{set}");
    let read = session.call(3, "settings.get", json!({"key": "auth_preferred_source"}));
    assert_eq!(read["result"]["value"], json!("own"));

    let bad = session.call(
        4,
        "settings.set",
        json!({"key": "auth_preferred_source", "value": "qoder"}),
    );
    assert_eq!(bad["error"]["code"], json!(-32602), "{bad}");

    let unknown = session.call(
        5,
        "settings.set",
        json!({"key": "not_a_setting", "value": "x"}),
    );
    assert_eq!(unknown["error"]["code"], json!(-32602), "{unknown}");

    session.call(6, "worker.shutdown", json!({}));
}

/// `auth.authorization.delete` 只删指定的那一份。
#[test]
fn authorization_delete_only_removes_the_requested_kind() {
    let mut session = Session::with_magpie();
    session.call(1, "auth.magpie.import", json!({}));

    let list = session.call(2, "accounts.list", json!({}));
    let id = list["result"]["accounts"][0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let key = scoped_key(&id, "magpie", "access");

    let gone = session.call(
        3,
        "auth.authorization.delete",
        json!({"accountId": id, "kind": "magpie"}),
    );
    assert_eq!(gone["result"]["deleted"], json!(true), "{gone}");

    let after = session.call(4, "accounts.get", json!({"id": id}));
    assert_eq!(
        after["result"]["authorizations"].as_array().unwrap().len(),
        0,
        "指定的那份授权应当被删掉"
    );
    assert!(
        session.call(5, "secure.get", json!({"key": key}))["error"]["code"] == json!(1002),
        "凭据也要一起删"
    );

    // 账号本身还在（不是删账号）。
    let list = session.call(6, "accounts.list", json!({}));
    assert_eq!(list["result"]["accounts"].as_array().unwrap().len(), 1);

    let missing = session.call(
        7,
        "auth.authorization.delete",
        json!({"accountId": id, "kind": "own"}),
    );
    assert_eq!(missing["error"]["code"], json!(1003), "{missing}");

    session.call(8, "worker.shutdown", json!({}));
}

// ---------------------------------------------------------------------------
// PR #69：每日签到
// ---------------------------------------------------------------------------

/// 开关是一次普通写入：**不许推进 auth_epoch**（PRD §2 的那个坑）。
#[test]
fn checkin_switch_over_rpc_does_not_bump_the_credential_epoch() {
    let mut session = Session::start();
    let created = session.call(
        1,
        "accounts.upsert",
        json!({"platform": "workbuddy", "uid": "wb-checkin-1", "nickname": "卡里"}),
    );
    let id = created["result"]["id"].as_str().expect("id").to_string();
    let epoch = created["result"]["authEpoch"].clone();

    let on = session.call(
        2,
        "checkin.setEnabled",
        json!({"accountId": id, "enabled": true}),
    );
    assert_eq!(on["result"]["enabled"], json!(true), "{on}");
    assert_eq!(on["result"]["auto"]["enabledAccounts"], json!(1));

    let after = session.call(3, "accounts.get", json!({"id": id}));
    assert_eq!(after["result"]["checkinEnabled"], json!(true));
    assert_eq!(
        after["result"]["authEpoch"], epoch,
        "开关不是凭据变更，不许动 auth_epoch"
    );

    let off = session.call(
        4,
        "checkin.setEnabled",
        json!({"accountId": id, "enabled": false}),
    );
    assert_eq!(off["result"]["enabled"], json!(false));
    assert_eq!(off["result"]["auto"]["enabledAccounts"], json!(0));

    let missing = session.call(
        5,
        "checkin.setEnabled",
        json!({"accountId": "nope", "enabled": true}),
    );
    assert_eq!(missing["error"]["code"], json!(1003), "{missing}");

    session.call(6, "worker.shutdown", json!({}));
}

/// `cached: true` 的只读状态：不联网、不带令牌、没有账号也不是错误。
#[test]
fn cached_checkin_status_is_offline_and_token_free() {
    let mut session = Session::with_magpie();
    session.call(1, "auth.magpie.import", json!({}));

    let reply = session.call(2, "checkin.status", json!({"cached": true}));
    let accounts = reply["result"]["accounts"].as_array().expect("accounts");
    assert_eq!(accounts.len(), 1);
    assert_eq!(reply["result"]["cached"], json!(true));
    assert_eq!(reply["result"]["counts"]["checked"], json!(0));
    assert_eq!(
        accounts[0]["state"],
        json!("unknown"),
        "本地没记录就是未知，不猜"
    );

    let text = reply.to_string();
    assert!(
        !text.contains(&"a".repeat(200)),
        "不许把 access token 带进响应"
    );
    assert!(!text.contains("refresh"), "不许出现 refresh");

    session.call(3, "worker.shutdown", json!({}));
}

/// 没开任何账号的自动签到时，一轮 tick 立刻返回，不发请求。
#[test]
fn checkin_tick_without_enabled_accounts_is_idle() {
    let mut session = Session::start();
    let reply = session.call(1, "checkin.auto.tick", json!({}));
    assert_eq!(
        reply["result"]["skipped"],
        json!("no_accounts_enabled"),
        "{reply}"
    );
    let status = session.call(2, "checkin.auto.status", json!({}));
    assert_eq!(status["result"]["planTime"], json!("09:00"));
    assert_eq!(status["result"]["enabled"], json!(false));
    session.call(3, "worker.shutdown", json!({}));
}

/// 签到记录是空的就返回空数组，不编造历史。
#[test]
fn checkin_records_start_empty() {
    let mut session = Session::start();
    let created = session.call(
        1,
        "accounts.upsert",
        json!({"platform": "workbuddy", "uid": "wb-checkin-2"}),
    );
    let id = created["result"]["id"].as_str().expect("id").to_string();

    let reply = session.call(2, "checkin.records", json!({"accountId": id}));
    assert_eq!(reply["result"]["records"].as_array().unwrap().len(), 0);
    assert_eq!(reply["result"]["attempts"].as_array().unwrap().len(), 0);

    // 参数不全会给明确的 invalid_params，而不是 panic 或空成功。
    let bad = session.call(3, "checkin.run", json!({}));
    assert_eq!(bad["error"]["code"], json!(-32602), "{bad}");

    session.call(4, "worker.shutdown", json!({}));
}

// ---------------------------------------------------------------------------
// PR #73：Qoder 设备授权（CN / International 独立）
// ---------------------------------------------------------------------------

/// 发起阶段**不联网**：只生成 PKCE 并返回对应站点的登录页。
/// 含糊的 edition 不能被默认成任何一个站；取消后轮询立刻是终态。
#[test]
/// PR #77：删号走真实子进程。未知账号必须是明确的 `not_found`，
/// 而不是"删了 0 行也算成功"。
#[test]
fn qoder_account_delete_reports_unknown_accounts() {
    let mut session = Session::start();
    let reply = session.call(
        1,
        "qoder.accounts.delete",
        json!({"accountId": "qa_missing"}),
    );
    assert!(reply.get("error").is_some(), "{reply}");
    assert_eq!(reply["error"]["code"], json!(1003), "{reply}");
    assert!(
        reply["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("Qoder 账号不存在"),
        "{reply}"
    );

    // 空参数也要被当成参数错误，不能默认删掉什么。
    let bad = session.call(2, "qoder.accounts.delete", json!({}));
    assert_eq!(bad["error"]["code"], json!(-32602), "{bad}");
}

fn qoder_device_auth_start_is_offline_and_edition_scoped() {
    let mut session = Session::start();

    let cn = session.call(1, "qoder.auth.start", json!({"edition": "qoder_cn"}));
    assert_eq!(cn["result"]["edition"], json!("qoder_cn"), "{cn}");
    assert_eq!(cn["result"]["displayName"], json!("Qoder CN"), "{cn}");
    let cn_url = cn["result"]["verificationUri"]
        .as_str()
        .expect("verificationUri")
        .to_string();
    assert!(
        cn_url.starts_with("https://qoder.cn/device/selectAccounts?"),
        "{cn_url}"
    );
    for part in [
        "challenge_method=S256",
        "client_id=e883ade2",
        "machine_id=",
        "nonce=",
        // 两站的 CLI 都不带 redirect_uri。
        "redirect_uri",
    ] {
        let present = cn_url.contains(part);
        assert_eq!(
            present,
            part != "redirect_uri",
            "参数 {part} 与预期不符：{cn_url}"
        );
    }
    // 二维码内容就是同一个地址，不允许另给一份。
    assert_eq!(cn["result"]["qrPayload"], cn["result"]["verificationUri"]);

    let login_id = cn["result"]["loginId"]
        .as_str()
        .expect("loginId")
        .to_string();
    let cancelled = session.call(2, "qoder.auth.cancel", json!({"loginId": login_id}));
    assert_eq!(cancelled["result"]["cancelled"], json!(true), "{cancelled}");
    // 重复取消是幂等的 false。
    let again = session.call(3, "qoder.auth.cancel", json!({"loginId": login_id}));
    assert_eq!(again["result"]["cancelled"], json!(false), "{again}");

    let global = session.call(4, "qoder.auth.start", json!({"edition": "qoder_global"}));
    let global_url = global["result"]["verificationUri"]
        .as_str()
        .expect("verificationUri");
    assert!(
        global_url.starts_with("https://qoder.com/device/selectAccounts?"),
        "{global_url}"
    );
    assert!(global_url.contains("client_id=732aef47"), "{global_url}");
    assert_ne!(
        global["result"]["loginId"],
        json!(login_id),
        "每次发起都是新会话"
    );

    // 含糊的 `qoder` 不能被默认成国内版或国际版。
    let bad = session.call(5, "qoder.auth.start", json!({"edition": "qoder"}));
    assert_eq!(bad["error"]["code"], json!(-32602), "{bad}");

    // 已取消的会话：轮询直接终态，不发网络、不落库。
    let polled = session.call(6, "qoder.auth.poll", json!({"loginId": login_id}));
    assert_eq!(polled["result"]["state"], json!("cancelled"), "{polled}");
    // 不存在的会话也一样。
    let unknown = session.call(7, "qoder.auth.poll", json!({"loginId": "qd1_missing"}));
    assert_eq!(unknown["result"]["state"], json!("finished"), "{unknown}");

    let accounts = session.call(8, "qoder.accounts.list", json!({}));
    assert_eq!(
        accounts["result"]["accounts"],
        json!([]),
        "没有完成授权就不该有账号"
    );
    session.call(9, "worker.shutdown", json!({}));
}
