//! vendored 上游补丁的回归。
//!
//! AICC 用 `WB_SWITCH_HOME` 把上游存储收进自己的数据目录，
//! 但 WorkBuddy 客户端的认证文件必须仍然指向真实用户主目录
//! （见 `account-worker/vendor/VENDOR.md`）。这两条一旦被上游同步冲掉，
//! 表现会是"添加账号后读不到登录态"，很难查，所以钉死在这里。

use wb_switch_core::modules::{config, variant::WbVariant};

#[test]
fn wb_switch_home_moves_store_but_not_client_auth_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sandbox = dir.path().join("wb-home");

    std::env::set_var(config::HOME_ENV_VAR, &sandbox);

    // 上游自己的存储必须跟着走。
    assert_eq!(config::home_dir(), sandbox);
    assert_eq!(config::store_dir(), sandbox.join(".wb-switch"));

    // 客户端认证文件不能跟着走。
    let auth = WbVariant::Cn.auth_file_path();
    let text = auth.to_string_lossy().to_string();
    assert!(
        !text.contains("wb-home"),
        "认证文件被 WB_SWITCH_HOME 带偏了：{text}"
    );
    assert!(
        text.contains("CodeBuddyExtension/Data/Public/auth"),
        "认证文件路径结构不对：{text}"
    );

    std::env::remove_var(config::HOME_ENV_VAR);

    // 清掉环境变量后回到真实主目录，且两档位同目录不同文件。
    let restored = WbVariant::Cn.auth_file_path();
    assert_eq!(restored.parent(), WbVariant::Ai.auth_file_path().parent());
    assert_ne!(restored, WbVariant::Ai.auth_file_path());
}

#[test]
fn store_dir_falls_back_to_real_home_without_override() {
    std::env::remove_var(config::HOME_ENV_VAR);
    let store = config::store_dir();
    assert!(
        store.ends_with(".wb-switch"),
        "默认存储目录异常：{}",
        store.display()
    );
}
