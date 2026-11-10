# wb-switch-core (vendored)

`aicc-account-worker` 的业务核心直接复用上游 `wb-switch-core`，**不引入 Tauri 应用**，
只保留纯 Rust 的账号 / 认证 / 签到 / 成长中心 / 旅行逻辑。

| 项 | 值 |
| --- | --- |
| 上游 | https://github.com/changexbc/workbuddy-switch |
| 子目录 | `crates/wb-switch-core`（对应到本地 `vendor/crates/wb-switch-core`） |
| 固定 commit | `22b318c1f44b335c2222e444abcc2d0f74e160f7`（`release/0.1.62`，2026-10-12） |
| crate 版本 | `0.1.62` |
| 布局 | 刻意与上游一致：`vendor/crates/wb-switch-core` + `vendor/scripts/` |
| 许可 | MIT，见 `crates/wb-switch-core/LICENSE`（Copyright (c) 2026 wb-switch） |

## 为什么保留 `vendor/scripts/`

`vendor/crates/wb-switch-core/src/modules/codebuddy_cli.rs` 用
`include_str!("../../../../scripts/codebuddy-cli-helper.cjs")` 把上游仓库根目录的
`scripts/codebuddy-cli-helper.cjs` 编译进二进制。这个相对路径往上四层就是仓库根，
所以本地目录必须镜像上游的层级（`vendor/` 充当"仓库根"），否则 crate 直接编译不过。
同步上游时这个文件要一起拷。

## 为什么 vendor 而不是 git 依赖

1. 构建必须可复现：固定到单一 commit，不随上游 main 漂移。
2. 离线 / 受限网络下也能构建（AICC 的发版机不保证能访问 github.com）。
3. 需要一处最小补丁（见下），补丁必须和代码一起被评审。

## 相对上游的改动（全部改动都标注了 `AICC 补丁`）

### 1. `config::client_home_dir()` —— 认证文件不随 `WB_SWITCH_HOME` 迁移

上游 `home_dir()` 同时被用于两件事：wb-switch 自己的存储目录（`~/.wb-switch`），
以及 WorkBuddy 客户端的认证文件路径（`~/Library/Application Support/CodeBuddyExtension/...`）。

AICC 必须用 `WB_SWITCH_HOME` 把账号库隔离到 AICC 的 Application Support，
**但绝不能连带把 WorkBuddy 认证文件的查找路径搬走** —— 否则读不到用户已经登录的账号。

补丁内容：

- `src/modules/config.rs` 新增 `pub fn client_home_dir()`：始终返回真实主目录，忽略 `WB_SWITCH_HOME`。
- `src/modules/variant.rs` `auth_file_path()` 改用 `client_home_dir()`。
- `src/modules/variant.rs` 对应用例的断言同步改为 `client_home_dir()`。

补丁只有 3 处、行为单一，回归由 `account-worker/tests/vendor_paths.rs` 兜住。

上游若合并同类修复，同步时可以直接丢弃这段补丁。

### 2. `travel.rs` 的接口原语改为 `pub` —— 只改可见性，不改行为

AICC 的旅行编排（`account-worker/src/travel`）按**账号库 + 钥匙串**驱动，而上游
`run_travel_cycle()` / `depart_travel_for_account()` 读的是它自己的 `~/.wb-switch` 账号
与 `travel_cache.json`，还会在令牌过期时调 refresh —— 这两点 AICC 都不能接受
（账号在 SQLite、令牌在钥匙串、绝不调用 refresh）。

所以只把**无状态的那一层**开放出来复用，调度与持久化仍留在 AICC：

| 符号 | 用途 |
| --- | --- |
| `fetch_travel_config` / `fetch_travel_status` | GET `/activity/growth/buddy/travel/{config,status}` 的封装 |
| `depart_travel` / `claim_travel` | POST `{depart,claim}`，含错误文案归一 |
| `decide_travel_action` + `TravelAction` | 官方状态机：`arrived→claim`、`traveling→wait`、`idle+daily_limit→stop`、`idle→depart` |
| `classify_depart_error` + `DepartClass` | 「已在旅行 / 今日上限 / 无 Buddy / 地点不可用」的分类 |
| `parse_travel_state` / `is_unauthorized` | 状态解析与 401/403 判定 |

`TravelAction` 额外 derive 了 `Clone, Copy`（便于在 match 外复用），除此之外**没有逻辑改动**，
diff 全是 `fn` → `pub fn`。

**为什么不会碰到 refresh**：`travel_request` 只有在账号 Value 里存在非空 `refresh_token`
字段时才会刷新。AICC 构造的 payload 只有 `access_token` / `uid` / `domain` / `variant`，
回归用例 `travel::tests::payload_never_carries_refresh_token` 卡住这一点。

### 未改动但需要知情的点

`variant.rs` 的**应用数据根**（`variant.rs` 中 `home_dir()` 的其它调用）、
`codebuddy_*` / `limits` / `session` 等模块的客户端路径，仍然跟随 `home_dir()`。
PR1a 只用到 WorkBuddy 认证文件读取，所以不影响；等 PR2 需要读客户端会话目录时，
必须逐个确认这些路径是否也该走 `client_home_dir()`。

## 同步上游的步骤

```bash
UPSTREAM=git@github.com:changexbc/workbuddy-switch.git
COMMIT=<new sha>
rm -rf /tmp/wbs && git clone --depth 1 --filter=blob:none --sparse "$UPSTREAM" /tmp/wbs
git -C /tmp/wbs fetch --depth 1 origin "$COMMIT" && git -C /tmp/wbs checkout "$COMMIT"
git -C /tmp/wbs sparse-checkout set crates/wb-switch-core LICENSE
# 单文件不能用 cone 模式 sparse-checkout，单独取
mkdir -p /tmp/wbs/scripts
curl -H "Accept: application/vnd.github.raw" \
  -o /tmp/wbs/scripts/codebuddy-cli-helper.cjs \
  "https://api.github.com/repos/changexbc/workbuddy-switch/contents/scripts/codebuddy-cli-helper.cjs?ref=$COMMIT"

rm -rf account-worker/vendor/crates/wb-switch-core/src
cp -R /tmp/wbs/crates/wb-switch-core/src account-worker/vendor/crates/wb-switch-core/
cp /tmp/wbs/crates/wb-switch-core/Cargo.toml account-worker/vendor/crates/wb-switch-core/
cp /tmp/wbs/LICENSE account-worker/vendor/crates/wb-switch-core/
cp /tmp/wbs/scripts/codebuddy-cli-helper.cjs account-worker/vendor/scripts/
# 重新施加本文件第 2 节的补丁，然后：
cargo test --manifest-path account-worker/Cargo.toml
```

同步后必须更新本文件的 commit 与版本号，并跑一次 `scripts/smoke-test-account-worker.sh`。
