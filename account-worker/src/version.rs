//! 版本与协议常量。
//!
//! `PROTOCOL_VERSION` 与 Swift 侧 `AccountWorkerProtocol.protocolVersion` 必须一致；
//! 不一致时 Swift 侧拒绝接管进程并提示升级，避免"半个新协议"。

/// stdio JSON-RPC 协议版本。改动消息结构就要 +1。
pub const PROTOCOL_VERSION: u32 = 1;

/// Worker 自身版本。
pub const WORKER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// AICC 主程序版本由宿主动态注入（`AICC_VERSION` 环境变量）。
///
/// 不用 `option_env!`：那是编译期取值，cargo 不会因为环境变量变化而重编，
/// 很容易在增量构建里留下过期版本号。
pub fn aicc_version() -> String {
    std::env::var("AICC_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// vendored 上游核心的固定 commit，`health` 会回报，便于核对发布物来源。
pub const UPSTREAM_CORE_COMMIT: &str = "22b318c1f44b335c2222e444abcc2d0f74e160f7";
pub const UPSTREAM_CORE_VERSION: &str = "0.1.62";
