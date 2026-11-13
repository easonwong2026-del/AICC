# 账号中心助手（aicc-account-worker）

AICC 的账号管理与自动签到的业务核心。它是 `AICC.app/Contents/Helpers/` 里的一个
独立可执行文件，由主程序通过本机管道驱动。

- 分支：`feature/account-worker-pr1a`
- 阶段：**PR1a —— 基础架构**（不含真实账号联调）
- 上游核心：`changexbc/workbuddy-switch` @ `22b318c`，`crates/wb-switch-core` v0.1.62（MIT）

```
AICC.app
├── MacOS/AICC                      SwiftUI 主程序（原有）
├── PlugIns/AICCWidget.appex        WidgetKit（原有，未改动）
├── Resources/Server                Python Magpie 桥接（原有，未改动）
└── Helpers/aicc-account-worker     ← 本次新增（Rust）
```

## 1. 为什么是 Rust，而不是 Go

PRD 原本规划用 Go 新写业务核心。实际调研后改为复用上游的纯 Rust 核心库：

| 能力 | 上游 `wb-switch-core` 已有 | 自研 Go 要重做 |
| --- | --- | --- |
| 多账号库 / 分组 / 导入导出 | ✅ `account.rs`、`export_import.rs` | 全部 |
| WorkBuddy 认证文件读写 | ✅ `auth_file.rs` | 全部 + 逆向加密信封 |
| 扫码登录 | ✅ `oauth.rs` | 全部 + 联调 |
| 签到 / 成长中心 / 补签 / 抽奖 | ✅ `checkin.rs` | 全部 + 逐一验证接口 |
| Buddy 旅行 | ✅ `travel.rs` | 全部 |
| 积分查询与统计 | ✅ `credits.rs` | 全部 |

上游是纯逻辑 crate，不依赖 Tauri，可以静态链进我们自己的二进制。
语言换了，PRD 的架构约束一条没破：内嵌 helper、stdio JSON-RPC、不开端口、SQLite、Keychain。

## 2. 协议

stdin/stdout，一行一条 JSON-RPC 2.0。**stdout 只放协议消息**，日志全部走 stderr。
不监听任何 TCP 端口。

| 方法 | 说明 | 联网 |
| --- | --- | --- |
| `ping` | 存活探针 | 否 |
| `health` | 版本、路径、数据库、平台状态、方法表 | 否 |
| `env.info` | 数据目录、`WB_SWITCH_HOME` 实际值、认证文件位置 | 否 |
| `worker.shutdown` | 优雅退出 | 否 |
| `storage.init` / `storage.status` | 迁移、行数、完整性 | 否 |
| `accounts.list` / `get` / `upsert` / `delete` | 账号库 CRUD（`upsert` 是补丁语义：没传的字段保持原值；建行与写字段在同一个事务里，失败整条回滚）。`list`/`get` 额外带 `authorizations`；授权字段是派生镜像，`upsert` 传了也不生效 | 否 |
| `groups.list` / `upsert` | 分组 | 否 |
| `secure.status` | 钥匙串读写删探针（会触发一次系统授权） | 否 |
| `secure.set` / `get` / `delete` | **默认关闭**（`method_not_found`）。`AICC_ALLOW_SECURE_RPC=1` 才启用，且只接受 `workbuddy/<16 hex>/{own/access,own/refresh,magpie/access}` | 否 |
| `settings.get` / `settings.set` | 全局设置；目前只有 `auth_preferred_source`（`magpie` \| `own`，默认 `magpie`） | 否 |
| `auth.authorization.delete` | 删除**一份**授权（`{accountId, kind}`），只清它自己的键与记录 | 否 |
| `providers.workbuddy.inspect` | 读认证文件并**如实**报告能否解密 | 否 |
| `providers.workbuddy.oauth` | OAuth 能力说明（PR1a 未接线） | 否 |
| `providers.qoder.presence` | 只查配置目录/文件，不碰钥匙串 | 否 |
| `providers.qoder.inspect` | 解密 `auth.v1.dat` 并给令牌指纹 | 否 |
| `auth.workbuddy.start` / `poll` / `cancel` | 官方 OAuth 扫码：发起、轮询、取消（PR1b-1） | 是 |
| `auth.workbuddy.refresh` | 刷新令牌；`auth_source=magpie` 的账号改为重新同步 | 是 |
| `auth.workbuddy.status` | 用当前选用的授权探测一次登录态（只读）：先 `Valid/Unauthorized/Transient` 分类，再过 401 才切源 | 是 |
| `providers.magpie.accounts` | 列出 Magpie 里已登录的 WorkBuddy 账号（**不含令牌**） | 否 |
| `auth.magpie.import` | 把 Magpie 的 access token 搬进 AICC 钥匙串 | 否 |
| `travel.summary` | 旅行快照汇总（账号 + 开关 + 运行信息），**只读本地库** | 否 |
| `travel.events` | 某账号的旅行事件日志（派出 / 领取 / 失败） | 否 |
| `travel.setEnabled` | 每账号的自动旅行开关（默认关） | 否 |
| `travel.refresh` | 查一次真实旅行状态；**纯只读**，看到「已到达」也不领取 | 是 |
| `travel.depart` | 手动派出（服务端每日上限仍然生效） | 是 |
| `travel.claim` | 手动领取奖励（只有 arrived 才提交） | 是 |
| `travel.auto.status` | 调度器信息：承载进程、间隔、上次 tick | 否 |
| `travel.auto.tick` | 立刻跑一轮自动检查（调试 / 手动触发） | 是 |
| `checkin.status` | 今日签到状态（只读）：默认向服务端查每个账号，`cached` 时只读本地 | 是（只读语义） |
| `checkin.run` | 用户显式签到单账号：先查状态，已签到就返回 `already` | 是（仅未签到时） |
| `checkin.runPending` | 用户显式批量签到：只处理服务端确认未签到的账号 | 是（仅未签到时） |
| `checkin.setEnabled` | 每账号自动签到开关（默认关）；**不动 `auth_epoch`** | 否 |
| `checkin.auto.status` | 计划时间、下次执行、尝试次数与每账号状态 | 否 |
| `checkin.auto.tick` | 跑一轮自动签到；`force` 只跳过时间判断，不绕过开关与每日次数上限 | 是（开关打开时） |
| `checkin.records` | 签到历史与每次尝试（`checkin_records` + `task_executions`） | 否 |

`health` **故意不做**钥匙串探针：否则每次启动 App 都会弹一次系统授权框。
需要验证凭据存储时显式调 `secure.status`（设置页有按钮）。探针的读写删三步全成功才报 `ok`，
清理失败会以 `kind=cleanup_failed` 返回 false——写入成功但删不掉说明授权有问题，不能算通过。

`health.status` 只有在**数据库能打开且完整性自检通过**时才是 `ok`；
任何一项失败都降级为 `degraded` 并带 `degradedReason`。

Swift 侧的协议常量在 `macos/MenuBarApp/Sources/Models/AccountWorkerProtocol.swift`，
`protocolVersion` 必须与 `account-worker/src/version.rs` 一致；不一致时 App 拒绝使用该助手。

## 3. 数据与隔离

| 位置 | 内容 |
| --- | --- |
| `~/Library/Application Support/AICC-Dashboard/accounts/` | 数据根（可被 `AICC_ACCOUNT_DATA_DIR` 覆盖） |
| `.../accounts/accounts.sqlite` | 账号库 |
| `.../accounts/backups/` | 备份（PR2 使用） |
| `.../accounts/logs/` | 日志 |
| `.../accounts/wb-switch-home/.wb-switch/` | 上游核心自己的存储 |

**`WB_SWITCH_HOME` 的语义**：它只隔离上游的存储目录。WorkBuddy 客户端的认证文件
（`~/Library/Application Support/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info`）
属于客户端，必须留在真实主目录 —— 上游原实现把两者都挂在 `home_dir()` 上，
AICC 打了 3 行补丁拆开（见 `account-worker/vendor/VENDOR.md`，回归测试
`account-worker/tests/vendor_paths.rs`）。用户自己的 `~/.wb-switch` 不会被读写。

`QODER_AUTH_DIR` 逐账号指定。没有显式指定时**只在恰好一个**候选目录时才自动采用，
多个候选直接返回 `ambiguous_auth_dir` 要求指定 —— 不允许"永远先挑第一个"。

## 4. 凭据与安全

- AICC 自己的凭据进钥匙串，service = `com.aieink.dashboard.menubar.account-worker`。
  SQLite 只存引用，不存明文。
- 一个 WorkBuddy 账号可以有两份授权（PR #66），键按来源分开：
  `workbuddy/<account_id>/own/access`、`…/own/refresh`、`…/magpie/access`。
  **`magpie/refresh` 这个键不存在**，也不允许被创建；`magpie` 授权在表上有
  `CHECK (kind <> 'magpie' OR refresh_ref IS NULL)`。旧的 `workbuddy/<uid>/access|refresh`
  只作为兼容别名继续同步，等 v8 的独立 PR 再删。
- 通用 `secure.*` 默认不挂载：能写 stdin 的进程本来就能覆盖凭据，这条通道不该长期存在。
  受控开发 / 迁移时设 `AICC_ALLOW_SECURE_RPC=1`，并且仍受键名白名单约束（旧 UID 键、
  `magpie/refresh`、探针键都拒绝）。
- 对外只返回**指纹**（sha256 前 8 hex）与长度。**`secure.get` 不再回明文**
  （PR #65 关掉了 `reveal`）：能写 stdin 的进程就能读钥匙串，这个能力不该出现在协议里。
  需要明文的内部调用（查登录态 / 刷新 / 旅行）是 Rust 里直接 `app.secure.get()`，不经过 RPC。
- 所有日志与错误消息过 `rpc::sanitize`：长度 ≥32 且形如令牌的连续串（字母数字与 `. _ ~ + - =`）
  一律打码。`/` 刻意不算令牌字符，否则整条路径会被打码、日志失去诊断价值。
- 写凭据用钥匙串的**原位更新**（`SecItemAdd` → 命中重复项改走 `SecItemUpdate`），
  不再"先删再加"：删除成功而写入失败会永久丢掉旧凭据。
- 助手是 ad-hoc 签名的嵌套可执行文件，每次重新构建签名都会变，macOS 可能重新询问
  钥匙串授权。返回码区分 `denied` 与 `not_found`，UI 上分别提示。
- **会读 Magpie 的 `access`，但绝不读 / 不写它的 `refresh`**（见 §5.5）。理由不是洁癖：
  refresh 是单份有状态凭据，AICC 与 Magpie 各自刷新会互相把对方踢下线。
- 不向 Widget / 墨水屏暴露账号信息。

## 5. 与 PRD 不一致、已按实际能力修正的三处

| PRD 原文 | 实际情况 | 处理 |
| --- | --- | --- |
| §9.2「WorkBuddy Token 存储在 Keychain 或加密凭据库」 | 认证文件 `workbuddy-desktop.info` 是嵌套结构，`auth.accessToken` / `auth.refreshToken` / `account.nickname` / `account.phoneNumber` 等字段各自是 AES-GCM 信封（`{"$wbEncrypted":1,"envelope":{suite,keyId,nonce,ciphertext,authTag}}`）。密钥由客户端自己的 Electron 原生模块 `electron_browser_workbuddy_storage.loggerGet()` 提供，**既不在钥匙串也不在文件里** | PR1a 只报告 `decryptable:false` + `requires` + 信封字段路径，不假装解开。要拿明文只能走 OAuth 或起 WorkBuddy 运行时当凭据助手（PR1b） |
| §5.2「认证数据读取：Qoder CN 自带运行时」 | 不需要。`auth.v1.dat` = `v10` + AES-128-CBC（IV = 16 个空格，PKCS7），密钥 = PBKDF2-HMAC-SHA1(钥匙串口令, `saltysalt`, 1003, 16B)。钥匙串口令是**机器级**的 | Rust 直接算，不依赖 Qoder 进程，少一层依赖 |
| §4「签到接口未定」 | WorkBuddy：`POST /v2/billing/meter/checkin-activity-status`、`POST /v2/billing/meter/daily-checkin`、`/v2/activity/growth/*`；登录：`https://www.codebuddy.cn/v2/plugin/auth/state?platform=workbuddy` → 轮询 `/auth/token?state=` + `/login/account?state=` | 契约已明确，PR1b 直接接线 |

### 另外一处值得记的发现

实测客户端认证文件里已经有 `accounts` 与 `allAccounts` 数组（含 uid / uin / 加密后的昵称与手机号），
说明**客户端自己就保存了多账号列表**，但令牌类字段（`auth.accessToken`）只有当前账号一份。
这对 PR2 的账号模型有直接影响：添加账号可能不需要自己造一套"独立 profile"，
而是要么走 OAuth 拿新令牌，要么按客户端已有的列表做切换。PR1a 只把这个结构记下来，
并要求 `providers.workbuddy.inspect` 报出 `accountsArrayCount`，不做任何写入。

## 5.5 借用 Magpie 的登录态（PR1b-1）

Magpie 自己完成了 WorkBuddy OAuth，把结果明文放在 `~/.config/magpie/plugin-auth.json`：
每个账号一条 `access` / `refresh` / `uid` / `domain` / `expires`。昵称不在这个文件里，
在 `plugin-providers.json` 的 `accounts[].accountId`（按 `key` 对齐，如 `workbuddy#2ntprc`）。

AICC 的取法是**只读文件、只借 access**：

| 项 | 处理 |
| --- | --- |
| `access` | 复制进 AICC 钥匙串（`workbuddy/<uid>/access`） |
| `refresh` | **不读、不存**。AICC 不做 refresh，过期后重新从 Magpie 读一次 |
| `uid` / `domain` / `expires` | 落库（`token_expires_at` / `auth_domain`） |
| 账号来源 | `auth_source = 'magpie'`，界面卡片上标 `Magpie` |

这样两边共用的是无状态 access token（多端同时用是正常行为），不会出现"谁刷新谁把对方踢掉"。
代价是：Magpie 里删了账号，AICC 这条只能在令牌过期后变成"已过期"，不会立刻消失——这是刻意的，
不给 Magpie 增加任何耦合。

`auth.magpie.import` 的边界：

- 已存在且 `auth_source = 'own'` 的账号**跳转不覆盖**（不能把独立授权降级成借用）；
- 令牌已过期的跳过；
- 写入失败（钥匙串拒绝）只跳过该条，不整批失败；
- 返回值里没有任何令牌。

> 钥匙串条目的 ACL 绑定创建它的二进制。AICC 是 ad-hoc 签名，**每次重新构建签名都会变**，
> 所以升级后第一次读凭据 macOS 可能弹一次授权框，点"始终允许"即可。也因此导入动作
> 必须由 App 自己做（由 App 创建条目），命令行跑助手会触发一次额外授权。

## 5.6 Buddy 旅行（PR1c）

细节见 [`docs/BUDDY_TRAVEL.md`](BUDDY_TRAVEL.md)，这里只记与助手相关的三条：

- 调度器**在助手进程里**（60 秒 tick，15 分钟查/领、30 分钟派出窗口）。助手是 `AICC.app`
  的子进程，所以 **App 退出 = 自动旅行停止**；`travel.auto.status` 与界面都如实这么写。
- 只管 access：payload 只有 `access_token` / `uid` / `domain` / `variant`；401 时只重新从
  Magpie 取一份 access（一次机会），不调用 refresh。
- 三个手动动作互不越界：`refresh` 只读、`depart` 只派、`claim` 只领。避免「点刷新顺手把奖领了」。

运行态落在 `travel_states` / `travel_events` 两张表（迁移 v4），开关是
`accounts.travel_enabled`，与 `checkin_enabled` 互相独立。

### 凭据写入的互斥与版本（迁移 v5 → v6/v7）

`accounts.auth_epoch` 是账号级凭据版本号（v5）；v6 起真正的粒度落到了
`account_authorizations.version`（**单份授权**）。两者都保留，写入前都校验：

- 扫码登录、导入、resync、刷新轮换、删除都在进程内 `AUTH_WRITE_LOCK` 里；
- 写入前按 id 重读账号与授权行，`auth_epoch` / `version` 任一不符 → 拒绝（`superseded`）；
- 该账号没有 magpie 授权时，resync 拒绝（`not_magpie_source`）。

自动旅行拿着几秒前的账号快照去做 resync 时，这两种情况都会被拦下，不会覆盖用户
刚扫码拿到的自有授权。`superseded` 与 `not_magpie_source` 在旅行侧都算"旧快照"，
不算失败：直接用手上现有的令牌继续，下一轮自然读到新账号，不会给账号打 6 小时冷却。

其余细节见 `docs/AUTH_MODEL_V2.md`：

- 旧键 → 新键的搬运是 Rust 状态机（`pending/migrating/ready/failed`），
  `ready` 之前读旧键，所以迁移没完成时签到 / 旅行照常；
- 兼容镜像（`accounts.auth_status/auth_source/token_expires_at/auth_ref`）与
  `auth_mirror_auth_id` 一起写，并且旧别名键同步指向同一份授权；
- 探测用 `Valid / Unauthorized / Transient` 三分类，只有明确的 401 触发切源，
  429 / 5xx / 超时 / 判不出来的 403 不改状态。

## 5.7 WorkBuddy 每日签到（PR #69）

签到是**独立模块**（`account-worker/src/checkin/`），不并入成长中心自动接单：
用户可以只开签到，不碰旅行和成长中心。

三条铁律，改动前先读：

1. **服务端是唯一事实来源。** 每轮先查 `today_checked_in`，已签到就跳过写请求；
   缺这个布尔值一律 `unknown`、**不提交**（上游用 `unwrap_or(false)`，AICC 不复用）。
2. **只有回查确认才记成功。** HTTP 200 / 业务码 0 都只算"已受理"，
   回查 `today_checked_in == true` 才落 `claimed` 与奖励；奖励缺失存 NULL。
3. **签到开关不走 `accounts.upsert`。** `upsert` 每次都推进 `auth_epoch`，
   点一下开关会把在途的凭据快照全废掉。用 `Db::set_checkin_enabled()`。

接口证据（含未验证项）、状态机、调度参数与数据库变更全部在
[`docs/WORKBUDDY_CHECKIN_API.md`](WORKBUDDY_CHECKIN_API.md)。

数据库到 **v10**：`checkin_records` 加 `error_kind` / `execution_source` 两列，
每次尝试复用 `task_executions`（`task_kind = 'daily_checkin'`）。

## 6. PR1a 交付范围

已交付：

- Rust 助手骨架、JSON-RPC over stdio、超时与进程退出处理
- 上游核心编译进产物（`wb-switch-core` 全量编译通过）
- SQLite 初始化 + 迁移 + 账号/分组 CRUD
- 钥匙串凭据存取 + 探针
- Qoder 本地认证解密（含错误分类）与 WorkBuddy 信封能力上报
- 构建脚本内嵌 + 独立签名（Widget 扩展签名不受影响）
- Swift 侧客户端 / 生命周期 / 设置页状态行
- CI：`cargo fmt` / `cargo check` / `cargo test` / 助手冒烟 / DMG 内含助手校验

**未交付**（等真实账号，PR1b）：

- WorkBuddy 扫码登录接线与 3 个真实账号联调
- Qoder 多账号 profile 隔离实机验证（`--user-data-dir`；PRD §3.3 的硬门槛）
- 签到 / 成长中心 / 旅行 / Credits 的实际领取
- 调度器、后台常驻、通知（PR4）
- 账号管理 UI 列表、卡片、筛选（PR2）

## 7. 常用命令

```bash
# 构建助手（调试）
cargo build --manifest-path account-worker/Cargo.toml

# 全量测试
cargo test --manifest-path account-worker/Cargo.toml

# 不联网自检（建库/迁移/账号读写/凭据探针，用内存钥匙串）
account-worker/target/debug/aicc-account-worker --self-test

# 协议冒烟 + WB_SWITCH_HOME 隔离
bash scripts/smoke-test-account-worker.sh

# 构建 App（会顺带编译助手并内嵌、签名）
bash macos/build-aicc-swiftui.sh

# 只调 Swift 界面时跳过 Rust
SKIP_RUST_HELPER=1 bash macos/build-aicc-swiftui.sh
```

本机验证时**装到 `~/Applications`**，`/Applications` 在这台机器上会卡在 `_dyld_start`（见 `HANDOFF.md`）。

## 8. 已知限制

- 数据库操作是单线程串行的（JSON-RPC 逐条处理）。并发调度在 PR4，届时再引入
  每账号串行的任务队列，而不是把连接池塞进现在这层。
- `cargo build` 会在 `account-worker/target/` 留产物，已在 `.gitignore` 里排除。
- 上游 `codebuddy_*` / `limits` / `session` 等模块仍把客户端路径挂在 `home_dir()` 上；
  PR1a 用不到，PR2 读会话目录前必须逐个确认是否也要走 `client_home_dir()`。
