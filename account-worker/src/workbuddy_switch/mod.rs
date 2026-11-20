//! WorkBuddy **客户端**账号切换（不是 Qoder Profile 切换，也不是授权轮换）。
//!
//! 分三层，任何一层都不做"顺手多做一点"：
//!
//! | 层 | 负责 | 不负责 |
//! | --- | --- | --- |
//! | [`client`] | 读认证文件、原子写、启停客户端 | 判断该不该切 |
//! | [`service`] | 可切换判定、切换编排、校验、回滚、journal | 解析 UI 文案 |
//! | `handlers` | 参数校验与协议形状 | 任何业务判断 |
//!
//! 边界写在 `docs/WORKBUDDY_CLIENT_SWITCH.md`：只支持国内版；只有同时持有
//! access + refresh 的自有授权可切换；写完必须回读确认，不能"写了就算成功"。

pub mod client;
pub mod plan;
pub mod service;

pub use client::SystemControl;
pub use service::{restore, status, switch, SwitchEnv, SwitchRequest};
