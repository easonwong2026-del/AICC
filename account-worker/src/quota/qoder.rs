//! Qoder 两版的额度适配器：**只读能力尚未验证，一个请求都不发**。
//!
//! 为什么这里什么都没有（2026-11-14 实测，记录在 `docs/QODER_QUOTA_BLOCKER.md`）：
//!
//! - 两版都有官方 OIDC（国际 `https://qoder.com`，国内 issuer `https://qoder.cn`），
//!   但 token 端点只声明 `client_secret_post` / `private_key_jwt`，**没有 `none`**；
//!   AICC 手上没有 Qoder 侧注册的 client_id，拿不到 access token。
//! - Qoder 的 Credits / 每日领取 / 成长任务属于**账号级业务接口**，与 OIDC 的
//!   userinfo 是两套东西；没有公开文档，也没有可复现的成功请求。
//!
//! 所以本模块只回答"为什么没有数"，绝不返回 `0`：0 会被当成"额度用完了"。
//! 一旦有人拿到真实成功响应，把 `verified()` 改成查表、补一个解析函数即可，
//! 上层 (`super::board`) 与界面都不用改。

use serde_json::{json, Value};

/// Qoder 账号的标识方式：`qoder_cn` / `qoder_global`。
pub const PROVIDERS: &[&str] = &["qoder_cn", "qoder_global"];

/// 未验证的能力行。`status = unverified`、`quota = null`，并附上阻塞原因。
pub fn unverified_row(
    account_id: &str,
    provider: &str,
    nickname: &str,
    source: &str,
    detail: &str,
) -> Value {
    json!({
        "accountId": account_id,
        "provider": provider,
        "nickname": nickname,
        "source": source,
        "status": "unverified",
        "unit": Value::Null,
        "total": Value::Null,
        "used": Value::Null,
        "remaining": Value::Null,
        "plan": Value::Null,
        "isPaid": Value::Null,
        "usage": Value::Null,
        "syncAt": crate::clock::now_rfc3339(),
        "error": detail,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_qoder_editions_are_known() {
        assert!(PROVIDERS.contains(&"qoder_cn"));
        assert!(PROVIDERS.contains(&"qoder_global"));
        assert!(!PROVIDERS.contains(&"workbuddy"));
        assert!(!PROVIDERS.contains(&"qoder"));
    }

    #[test]
    fn unverified_row_never_reports_zero_quota() {
        let row = unverified_row("qa_test", "qoder_cn", "测试", "own_browser", "阻塞原因");
        assert_eq!(row["status"], "unverified");
        // 未知额度必须是 null，绝不能是 0：0 会被界面读成"用完了"。
        assert!(row["total"].is_null());
        assert!(row["remaining"].is_null());
        assert!(row["used"].is_null());
        assert_eq!(row["error"], "阻塞原因");
    }
}
