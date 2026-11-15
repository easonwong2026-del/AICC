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
//! * [`broker`]：只读取用（首选来源 + 兜底，不写库）；
//! * [`magpie_link`]：Magpie 动态关联（按 `source_key` 现读，不复制令牌，PR-CRED-2）；
//! * [`relink`]：把旧的钥匙串引用改成动态关联（显式、幂等、不猜归属）；
//! * [`owned`]：自有 JSON 凭据的归属校验（引用只是 UUID，得拿文件里的字段对一遍）；
//! * [`consent`]：明文 JSON 的首次告知闸门（没确认过就继续走钥匙串）。

pub mod broker;
pub mod consent;
pub mod ids;
pub mod magpie_link;
pub mod migrate;
pub mod owned;
pub mod probe;
pub mod relink;
pub mod service;
