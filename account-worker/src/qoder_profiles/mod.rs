//! Qoder 多账号 Profile（PR #70）。
//!
//! 目录分工：
//!
//! - `manager`：登记 / 检查 / 起停 / 清理的业务流程；
//! - `process`：真实进程的启动、核实、关闭与对账；
//! - 存储层在 `storage::qoder_profiles`（v11 新增两张表）；
//! - 各版本的认证解析在 `providers::qoder`（CN）与 `providers::qoder_global`。

pub mod manager;
pub mod process;
