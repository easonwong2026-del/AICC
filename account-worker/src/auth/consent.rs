//! 明文 JSON 存储的**首次告知**闸门（PR-CRED-3a 复审）。
//!
//! PRD §4.3 要求"首次使用必须明确提示风险"。在告知界面落地之前，新登录**不**默认
//! 写明文：没有确认过就一直走原来的钥匙串路径，行为与 #84 完全一致。
//!
//! 这条闸门只管一件事：**要不要新建一个明文文件**。已经存在的 JSON 行照旧读写 ——
//! 那个文件早就落盘了，再退回钥匙串既不会减少暴露，还会让同一份授权分裂成两处。

use crate::app::App;
use crate::rpc::RpcResult;

/// 设置项：用户已经读过明文存储的风险说明并同意。
pub const JSON_CONSENT_SETTING: &str = "credentials_json_consent";

/// 是否允许把**新**令牌写成明文 JSON。默认 `false`。
pub fn json_storage_enabled(app: &App) -> bool {
    matches!(
        app.with_db(|db| db.get_setting(JSON_CONSENT_SETTING)),
        Ok(Some(value)) if is_true(&value)
    )
}

/// 记下用户的决定。
pub fn set_json_storage(app: &App, enabled: bool) -> RpcResult<()> {
    app.with_db(|db| db.set_setting(JSON_CONSENT_SETTING, if enabled { "1" } else { "0" }))
}

/// 给界面看的现状：`consentRequired` 为真时，前端必须先弹说明再让用户确认。
pub fn status(app: &App) -> serde_json::Value {
    let enabled = json_storage_enabled(app);
    serde_json::json!({
        "jsonStorageEnabled": enabled,
        "consentRequired": !enabled,
        "settingKey": JSON_CONSENT_SETTING,
        "risk": "本地 JSON 是明文存储：文件权限 0600 只能挡住同机其他用户，\
                 挡不住同一个 macOS 用户下的其他程序。建议开启 FileVault，\
                 并且不要把凭据目录放进云同步或未加密备份。",
    })
}

fn is_true(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[cfg(test)]
mod tests {
    use super::is_true;

    #[test]
    fn only_explicit_affirmatives_enable_it() {
        for yes in ["1", "true", "true ", " YES ", "on"] {
            assert!(is_true(yes), "{yes}");
        }
        for no in ["0", "false", "", "no", "off", "随便"] {
            assert!(!is_true(no), "{no}");
        }
    }
}
