//! `aicc-account-worker` —— AICC 内嵌的账号与签到辅助进程。
//!
//! 传输：stdin/stdout 上的 JSON-RPC 2.0，一行一条消息（JSONL）。
//! **stdout 只放协议消息**，任何人类可读的日志都走 stderr，否则会把协议打碎。
//! 不监听任何 TCP 端口（PRD §9.3）。

mod app;
mod clock;
mod handlers;
mod paths;
mod providers;
mod rpc;
mod securestore;
mod storage;
mod travel;
mod version;

use std::io::{BufRead, Write};
use std::sync::Arc;

use serde_json::{json, Value};

use crate::app::App;
use crate::paths::Paths;
use crate::rpc::{Request, RpcError};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!(
                "aicc-account-worker {} protocol {} (wb-switch-core {})",
                version::WORKER_VERSION,
                version::PROTOCOL_VERSION,
                version::UPSTREAM_CORE_VERSION
            );
        }
        Some("--help") | Some("-h") => print_help(),
        Some("--self-test") => {
            let runtime = runtime();
            std::process::exit(runtime.block_on(self_test()))
        }
        Some(other) if other.starts_with('-') => {
            eprintln!("未知参数：{other}");
            print_help();
            std::process::exit(64);
        }
        _ => serve(),
    }
}

fn print_help() {
    println!(
        "aicc-account-worker {}\n\
         \n\
         用法：\n\
         \x20 aicc-account-worker            以 JSON-RPC over stdio 提供服务（AICC 内部调用）\n\
         \x20 aicc-account-worker --self-test 不联网自检：建库、迁移、读写账号、探针凭据存储\n\
         \x20 aicc-account-worker --version   打印版本\n\
         \n\
         环境变量：\n\
         \x20 AICC_ACCOUNT_DATA_DIR  覆盖数据目录（默认 ~/Library/Application Support/AICC-Dashboard/accounts）\n\
         \x20 WB_SWITCH_HOME         上游核心存储目录（默认 <数据目录>/wb-switch-home）\n\
         \x20 QODER_AUTH_DIR         Qoder CN 配置目录（多账号必须逐账号指定）",
        version::WORKER_VERSION
    );
}

/// 单线程 runtime：RPC 逐条处理，登录轮询是短请求，够用且省资源。
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build tokio runtime")
}

fn serve() {
    let runtime = runtime();
    let paths = Paths::resolve();
    if let Err(err) = paths.ensure_dirs() {
        log(&format!("数据目录创建失败：{err}"));
    }
    let app = Arc::new(App::new(paths));
    log(&format!(
        "worker {} 启动，protocol {}，数据目录 {}",
        version::WORKER_VERSION,
        version::PROTOCOL_VERSION,
        app.paths.data_dir.display()
    ));

    // stdin 的读取放在独立线程：主线程要同时等「协议请求」和「调度心跳」，
    // 阻塞式 read_line 会把心跳饿死（自动旅行就成了摆设）。
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let reader = std::thread::Builder::new()
        .name("aicc-stdin".to_string())
        .spawn(move || {
            let stdin = std::io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                match stdin.lock().read_line(&mut line) {
                    Ok(0) => break, // 上游关掉了管道 → 正常退出
                    Ok(_) => {
                        if tx.send(line.clone()).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        log(&format!("读取 stdin 失败：{err}"));
                        break;
                    }
                }
            }
        })
        .ok();

    let stdout = std::io::stdout();
    let mut output = std::io::BufWriter::new(stdout.lock());

    runtime.block_on(async {
        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
            travel::service::TICK_INTERVAL_SECS,
        ));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        // interval 的第一次 tick 立刻返回；先消费掉，让第一轮检查在一个周期之后。
        ticker.tick().await;

        loop {
            tokio::select! {
                incoming = rx.recv() => {
                    let Some(line) = incoming else { break };
                    if handle_line(&app, &line, &mut output).await {
                        break;
                    }
                }
                _ = ticker.tick() => {
                    // 放到独立任务里跑：一轮检查可能要几十秒，不能把 RPC 卡住。
                    let app = Arc::clone(&app);
                    tokio::task::spawn(async move {
                        let report = travel::scheduler::run_tick(&app).await;
                        let status = report.get("status").and_then(Value::as_str).unwrap_or("?");
                        if status == "ok" {
                            log(&format!(
                                "旅行检查完成：{} 个账号，实际检查 {}",
                                report.get("accounts").and_then(Value::as_i64).unwrap_or(0),
                                report.get("checked").and_then(Value::as_i64).unwrap_or(0)
                            ));
                        }
                    });
                }
            }
        }
    });

    let _ = output.flush();
    if let Some(handle) = reader {
        // 不等它：stdin 可能还阻塞着 read_line，进程退出会一并回收。
        drop(handle);
    }
}

/// 处理一行输入。返回 true 表示该退出了。
async fn handle_line<W: Write>(app: &Arc<App>, line: &str, output: &mut W) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    let (payload, is_notification) = match parse_request(trimmed) {
        Ok(v) => v,
        Err(err) => {
            write_line(output, &rpc::response_err(None, &err));
            return false;
        }
    };

    let Request {
        id, method, params, ..
    } = payload;
    let id_is_null = id.is_none();
    let result = handlers::dispatch(app, &method, &params).await;

    // 通知（没有 id）不回包，避免给调用方制造"多出来的响应"。
    if !is_notification && !id_is_null {
        let response = match result {
            Ok(value) => rpc::response_ok(id, value),
            Err(err) => {
                log(&format!("{method} 失败：{err}"));
                rpc::response_err(id, &err)
            }
        };
        write_line(output, &response);
    }

    if method == "worker.shutdown" {
        log("收到 worker.shutdown，正常退出");
        return true;
    }
    false
}

fn parse_request(line: &str) -> Result<(Request, bool), RpcError> {
    let value: Value = serde_json::from_str(line)
        .map_err(|err| RpcError::new(rpc::PARSE_ERROR, format!("不是合法 JSON：{err}")))?;
    if let Some(batch) = value.as_array() {
        // 批量请求不在 PR1a 范围内，明确拒绝而不是静默忽略。
        return Err(RpcError::new(
            rpc::INVALID_REQUEST,
            format!("暂不支持批量请求（收到 {} 条）", batch.len()),
        ));
    }
    let is_notification = value.get("id").is_none() || value.get("id") == Some(&Value::Null);
    let request: Request = serde_json::from_value(value)
        .map_err(|err| RpcError::new(rpc::INVALID_REQUEST, format!("请求结构非法：{err}")))?;
    Ok((request, is_notification))
}

fn write_line<W: Write>(out: &mut W, value: &Value) {
    match serde_json::to_string(value) {
        Ok(text) => {
            let _ = out.write_all(text.as_bytes());
            let _ = out.write_all(b"\n");
            let _ = out.flush();
        }
        Err(err) => log(&format!("序列化响应失败：{err}")),
    }
}

/// 日志只走 stderr，且过一遍敏感词过滤。
fn log(message: &str) {
    eprintln!("[aicc-account-worker] {}", rpc::sanitize(message));
}

/// 自检：不联网、不碰真实钥匙串、不影响用户数据目录。
async fn self_test() -> i32 {
    let dir = std::env::temp_dir().join(format!("aicc-worker-selftest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::env::set_var(paths::DATA_DIR_ENV, &dir);

    let paths = Paths::resolve();
    if let Err(err) = paths.ensure_dirs() {
        println!(
            "{}",
            json!({ "ok": false, "stage": "paths", "error": err.to_string() })
        );
        return 1;
    }
    let app = Arc::new(App::with_secure(paths, securestore::SecureStore::memory()));

    let mut checks: Vec<Value> = Vec::new();
    let mut push = |name: &str, ok: bool, detail: Value| {
        checks.push(json!({ "name": name, "ok": ok, "detail": detail }));
    };

    match handlers::dispatch(&app, "storage.init", &json!({})).await {
        Ok(v) => push("storage.init", true, v),
        Err(err) => push("storage.init", false, json!(err.to_string())),
    }

    let upsert = handlers::dispatch(
        &app,
        "accounts.upsert",
        &json!({ "platform": "workbuddy", "uid": "selftest", "nickname": "自检账号", "checkinEnabled": true }),
    )
    .await;
    push(
        "accounts.upsert",
        upsert.is_ok(),
        upsert.unwrap_or_else(|e| json!(e.to_string())),
    );

    let list = handlers::dispatch(&app, "accounts.list", &json!({})).await;
    let count = list
        .as_ref()
        .ok()
        .and_then(|v| v.get("accounts"))
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    push("accounts.list", count == 1, json!({ "count": count }));

    // 旅行：不联网，只验证迁移、开关与快照结构（自动旅行是否真的跑起来要靠联调）。
    let travel = handlers::dispatch(&app, "travel.summary", &json!({})).await;
    let travel_has_auto = travel
        .as_ref()
        .ok()
        .and_then(|v| v.get("auto"))
        .and_then(|a| a.get("owner"))
        .and_then(Value::as_str)
        .map(|owner| owner.contains("aicc-account-worker"))
        .unwrap_or(false);
    push(
        "travel.summary",
        travel_has_auto,
        travel.unwrap_or_else(|e| json!(e.to_string())),
    );

    let probe = handlers::dispatch(&app, "secure.status", &json!({})).await;
    let probe_ok = probe
        .as_ref()
        .ok()
        .and_then(|v| v.get("ok"))
        .and_then(Value::as_bool)
        .unwrap_or(false);
    push(
        "secure.status",
        probe_ok,
        probe.unwrap_or_else(|e| json!(e.to_string())),
    );

    let health = handlers::dispatch(&app, "health", &json!({})).await;
    let health_ok = health
        .as_ref()
        .ok()
        .and_then(|v| v.get("status"))
        .and_then(Value::as_str)
        .map(|s| s == "ok")
        .unwrap_or(false);
    push("health", health_ok, json!(health.is_ok()));

    let _ = std::fs::remove_dir_all(&dir);

    let ok = checks.iter().all(|c| c["ok"] == json!(true));
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "ok": ok,
            "worker": version::WORKER_VERSION,
            "protocol": version::PROTOCOL_VERSION,
            "checks": checks,
        }))
        .unwrap_or_default()
    );
    if ok {
        0
    } else {
        1
    }
}
