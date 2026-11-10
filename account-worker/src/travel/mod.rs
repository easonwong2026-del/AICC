//! Buddy 旅行（派猫猫旅行）：把「检查资格 → 派出 → 等到达 → 领奖励」变成
//! AICC 自己的一等能力，而不是靠外部工具。
//!
//! 分层（改代码前先看这里，别把职责搞混）：
//!
//! | 文件 | 职责 | 有没有 IO |
//! | --- | --- | --- |
//! | `state.rs` | 状态机、标签映射、冲突识别、退避曲线 | 无（纯函数，可单测） |
//! | `api.rs` | 请求封装（复用上游）+ 20 秒硬超时 + `Transport` 注入点 | 网络 |
//! | `service.rs` | 一个账号一轮「查 → 决定 → 执行 → 出新快照」 | 网络 + 凭据 |
//! | `scheduler.rs` | 常驻 tick、单账号互斥、快照落库 | 网络 + 数据库 |
//!
//! 三条不可协商的边界：
//!
//! 1. 只用 access token；401 时重新从 Magpie 取 access，**绝不调用 refresh**。
//! 2. 不读写 Magpie 的账号、签到记录或旅行配置，只读它的 `plugin-auth.json`。
//! 3. 调度器活在 `AICC.app` 的子进程里 —— App 退出即停，界面不许暗示还在跑。

pub mod api;
pub mod scheduler;
pub mod service;
pub mod state;

#[cfg(test)]
mod tests;
