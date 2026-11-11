//! 平台适配层。
//!
//! PR1a 只做**本地**能力：读认证文件 / 判断登录态 / 报告解密能力。
//! 所有联网动作（签到、领取、OAuth）都在 PR1b 之后，且必须用真实账号验证。

pub mod magpie;
pub mod qoder;
pub mod workbuddy;
pub mod workbuddy_auth;
