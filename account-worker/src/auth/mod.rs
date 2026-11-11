//! 多来源授权（PR #66）。
//!
//! 一个 WorkBuddy 账号可以同时持有两份授权：
//!
//! * `own` —— AICC 自己扫码拿到、自己持有 refresh；
//! * `magpie` —— 从 Magpie 借来的 access，**永远没有 refresh**。
//!
//! 三个子模块各管一件事：
//! * [`ids`]：授权 ID 与钥匙串键名（G5，纯计算）；
//! * [`migrate`]：旧键 → 新键的搬运状态机（G4，可中断、可重试、只复制不删除）；
//! * [`broker`]：只读取用（首选来源 + 兜底，不写库）。

pub mod broker;
pub mod ids;
pub mod migrate;
pub mod probe;
pub mod service;
