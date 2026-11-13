//! 平台适配层。
//!
//! 只做**本地**能力：读认证文件 / 判断登录态 / 报告解密能力 / 管理本地 profile。
//! 所有联网动作（签到、领取、OAuth）都不在这里，且必须用真实账号验证。
//!
//! Qoder 有两个独立变体，各自一个适配器：
//! - `qoder`        → Qoder CN
//! - `qoder_global` → Qoder International
//!
//! 二者共用 `qoder_crypto`（算法）与 `qoder_identity`（身份字段提取），
//! 但**不共用**密钥、路径与钥匙串条目。

pub mod magpie;
pub mod qoder;
pub mod qoder_crypto;
pub mod qoder_edition;
pub mod qoder_global;
pub mod qoder_identity;
pub mod workbuddy;
pub mod workbuddy_auth;
