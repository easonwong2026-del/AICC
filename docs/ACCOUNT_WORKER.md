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
| `providers.qoder_global.presence` / `inspect` | 国际版同名的只读检查，**与 CN 分开**：自己的路径、自己的钥匙串条目 | 否 |
| `qoder.editions.detect` | 报两个版本各自的 Bundle ID、安装路径、客户端版本、数据目录、是否允许并行实例、是否在本机复验过 | 否 |
| `qoder.profiles.list` | 列出 Profile（可带 `edition` 过滤）；附带快照是否就绪、live 当前是否就是这个账号、运行状态；**只读**：未完成切换只放在 `pendingSwitches` 里报告，不恢复 | 否 |
| `qoder.profiles.scan` | 把客户端当前登录的账号登记成 Profile：复制一份快照，**不移动、不覆盖、不要求重新登录** | 否 |
| `qoder.profiles.create` | 新建 Profile（AICC 托管的空快照目录） | 否 |
| `qoder.profiles.import` | 登记一份**外部**账号快照目录（必须有 `auth.v1.dat`；`..`、已登记目录、AICC 管理目录内、以及**客户端 live 目录本身**都会被拒）。登记后 `managed=false`，即**只读**：`capture` 不会向它写任何文件 | 否 |
| `qoder.profiles.adopt` | 把外部只读快照**复制**进 AICC 管理目录（`qoder/profiles/<id>/`），逐文件校验哈希后才改成 `managed=true`；原目录一个字节都不改，失败即删除半成品 | 否 |
| `qoder.profiles.inspect` | 用对应版本的适配器解密**快照**，写回指纹/到期/身份；**不回传令牌** | 否 |
| `qoder.profiles.capture` | 把客户端此刻的登录态按**文件组事务**存进快照（暂存 → 校验 → 上一代备份 → 提交 → 全组校验；失败整体回退）；无法证明 live 属于该 Profile 时拒绝覆盖；**只写 AICC 管理的 `qoder/profiles/<id>/`**：外部导入（`managed=false`）返回 `profile_read_only`，目录不是 AICC 分配的、与 live 重叠、或目标本身是符号链接，都直接拒绝 | 否 |
| `qoder.profiles.rename` | 只改 AICC 里的显示名 | 否 |
| `qoder.profiles.open` | **本阶段永久拒绝**：返回 `switchEnabled:false, permanent:true`，不探测进程、不建备份、不读写 live（没有任何环境变量开关） | 否 |
| `qoder.profiles.close` | 只关 AICC 自己拉起的进程；`force` 才升级 SIGKILL | 否 |
| `qoder.profiles.activate` | 与 `open` 相同，**永久拒绝** | 否 |
| `qoder.profiles.remove` | 移除登记；`purge=true` 才删**AICC 快照**，客户端数据目录永远不动 | 否 |
| `qoder.profiles.status` | 运行状态汇总（`running` / `stopped` / `unknown` 三态，每次都重新核实 PID）；**只读**，未完成切换放在 `pendingSwitches` 里 | 否 |
| `qoder.profiles.recover` | **显式**恢复未完成切换：必须能证明对应客户端已退出（`pgrep` 退出码 1），`Running` / `Unknown` / 找不到客户端都拒绝；任一条恢复失败返回错误 + `blocked` 明细 | 否 |
| `auth.workbuddy.start` / `poll` / `cancel` | 官方 OAuth：发起、轮询、取消（PR1b-1）。`start` 接受 `variant`（`cn` / `ai`，**严格解析**，非法值直接报参数错）；返回值带 `provider`，轮询成功后账号按**会话档位**落 `accounts.provider`。同一 URL 既能在浏览器打开也能渲染成二维码 | 是 |
| `auth.workbuddy.refresh` | 刷新令牌；`auth_source=magpie` 的账号改为重新同步 | 是 |
| `auth.workbuddy.status` | 用当前选用的授权探测一次登录态（只读）：先 `Valid/Unauthorized/Transient` 分类，再过 401 才切源 | 是 |
| `providers.magpie.accounts` | 列出 Magpie 里已登录的**四个 provider**（`workbuddy` / `workbuddy-ai` / `qoder` / `qoder-cn`）账号，逐条带 `platform` / `provider` / `edition` / `importTarget`，并标出哪些已导入；**不含令牌** | 否 |
| `auth.magpie.import` | 按平台分流：WorkBuddy（CN/AI）把 access 搬进钥匙串 + `accounts`；Qoder（CN/Global）只登记 `qoder_accounts` + `qoder_authorizations`（`magpie` 来源**不复制令牌**） | 否 |
| `qoder.accounts.list` | Qoder CN / Global 的**平台服务账号** + 各自授权来源（`magpie` / `own_browser`）；与 `qoder.profiles.list` 是两件事 | 否 |
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
| `quota.board` | 四平台额度看板（`days` / `provider` / `accountId` / `refresh`）。WorkBuddy CN/AI 走官方只读接口，**Qoder 两版零请求返回 `unverified`**；`refresh=false` 时优先用 `quota_snapshots` 缓存 | 是（只读语义） |
| `quota.history` | 某账号的额度快照历史（`days` 默认 30，上限 90），按时间升序 | 否 |

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

## 9. 四平台账号导入（v13，PR #71）

### 9.1 账号存放位置

| 平台 | 表 | 身份键 | 依赖客户端 |
| --- | --- | --- | --- |
| WorkBuddy 国内版 | `accounts`（`provider='workbuddy'`） | `platform + uid` | 否 |
| WorkBuddy AI 国际版 | `accounts`（`provider='workbuddy_ai'`） | 同上 | 否 |
| Qoder CN | `qoder_accounts` + `qoder_authorizations` | `edition + uid` | 否 |
| Qoder Global | 同上（`edition='qoder_global'`） | 同上 | 否 |
| Qoder 本机客户端 | `qoder_profiles` | `profile_id` | **是** |

两条线刻意分开：

- `qoder_profiles` 是**本机客户端**的认证快照，用来回答"本机现在登录的是谁"；
- `qoder_accounts` 是**平台服务**上的账号，网页授权和 Magpie 导入都只写这里。

所以**不装 Qoder 客户端也能把账号导进来**，界面上一个账号可能只有服务账号、没有本机快照。

### 9.2 provider 与 UID 冲突

`accounts.platform` 的 CHECK 只有 `workbuddy` / `qoder_cn`（v1 的既有约束，不重建），
WorkBuddy 国内版与国际版共用 `platform='workbuddy'`，靠 `provider` 区分服务。

代价写清楚：UNIQUE 仍然是 `(platform, uid)`。**同一个 UID 在两个站点都存在时，
第二个导入会返回 `uid_conflict` 并跳过**，不会自动合并、也不会改写已有账号。
真出现这种情况再评估是否重建主表。

### 9.3 授权来源与所有权

| 来源 | 存什么 | 谁刷新 |
| --- | --- | --- |
| `magpie`（WorkBuddy） | access token 进钥匙串，`magpie/refresh` 键**不存在** | Magpie |
| `magpie`（Qoder） | 只登记 `source_key`，**不复制令牌** | Magpie |
| `own_browser`（Qoder） | 令牌在钥匙串，表里只有 `secret_ref` 与指纹 | AICC（单账号互斥） |

`qoder_authorizations` 上 `(account_id, source)` 唯一：重导只更新元数据，
另一个来源的行完全不动。`magpie` 来源带 `secret_ref` 会被直接拒绝。

### 9.4 版本隔离（WorkBuddy AI 不进入国内版流程）

签到、成长、旅行、授权检查是**国内版独有的任务**（国际版产品里没有这几项）：端点写死在
`copilot.tencent.com` / `www.codebuddy.cn`，请求头与签到日界线也是国内版的。
WorkBuddy AI 是另一个站点，拿它的授权请求这些接口，轻则 401，重则把国际版
令牌发到国内域名。因此这些入口一律以 `AccountRecord::supports_workbuddy_cn_apis()`
（`platform == "workbuddy" && provider == "workbuddy"`）为准，**只看 platform 不算数**。

| 入口 | 行为 |
| --- | --- |
| `checkin::manager::run_account(_with)` | 取凭据**之前**返回 `unsupported`，`error_kind = unsupported_provider` |
| `checkin::scheduler::enabled_accounts` | 国际版不进自动轮次 |
| `checkin::scheduler::auto_status` | **不列出国际版**（没有这个任务，列出来只是噪音） |
| `checkin::scheduler::still_enabled`（POST 前最后一道） | 开关 **和** 版本都要满足 |
| `growth::scan_account` | 直接返回 `state=unsupported`，**连凭据都不取** |
| `growth::scheduler` / `growth::manager::run_account` | 调度跳过 + 写入口再挡一次 |
| `travel::scheduler` / `travel::service` | 标 `unsupported`，不写旅行状态 |
| `workbuddy_account_status` | 返回 `state=unsupported`，不探测、不改授权状态 |
| `growth::scan_fleet` / `travel::run_tick` | 直接跳过国际版，不产生结果行 |
| 账号卡 | `provider=workbuddy_ai` 显示为 **WorkBuddy AI**，**不显示签到一栏** |

`accounts.upsert` 未传 `provider` 时**继承库里已有值**：改备注不会把国际版
账号降级成国内版，也不会误报 `uid_conflict`。

## 10. WorkBuddy 双站点登录（PR #72）

国内版（`copilot.tencent.com` / `www.codebuddy.cn`）与国际版（`www.workbuddy.ai`）
是**两个站点**：OAuth 的 `platform` 参数、API 域名、`X-Domain` 都不一样。AICC 用
`accounts.platform = "workbuddy"` + `accounts.provider`（`workbuddy` / `workbuddy_ai`）
表示它们，这是 v13 定下的模型。

| 环节 | 行为 |
| --- | --- |
| 发起 `auth.workbuddy.start` | `variant` 决定站点；不会因为"看起来像"而回落 |
| 登录页 | `verificationUri` 同一份：界面主按钮用系统浏览器打开，次选渲染成二维码 |
| 登录地址校验 | `is_trusted_login_url`：主机交给标准 `url` 解析，只接受**本站点域名下的 https**，并**拒绝反斜杠与 userinfo**（浏览器把 `\` 当 `/`，`evil.com\@官方域` 会被打开成 `evil.com`）；界面打开前与编码二维码前再各挡一次 |
| 轮询 | 会话绑死档位；`poll` 从**会话**取档位决定 provider，不看请求参数 |
| 落库门禁 | `begin_commit` 原子抢占：只有仍 `Pending` 的会话能落库，并把会话里**不可变**的档位带出；抢不到（取消 / 过期 / 已结束）就拒绝写库，**没有"取不到版本就默认国内版"的兜底** |
| 取消 / 超时 | `cancel` 只对 `Pending` 生效；已进 `Committing` 的会话取消落空，不会出现"界面已取消但账号已写入" |
| 令牌落库 | access / refresh 进钥匙串（按 uid 隔离），`accounts.provider` 按档位写 |
| domain 校验 | `matches_domain` 拒绝跨档响应（国际版登录返回国内 domain 直接失败） |
| UID 冲突 | 同一 UID 已在另一站点 → `uid_conflict`，凭据回滚，账号行不改写 |
| 刷新 `auth.workbuddy.refresh` | **国际版仍返回 unsupported**：刷新端点没有真机证据，不拿好授权冒险 |
| 签到 / 成长 / 旅行 / 授权检查 | 国际版**没有这些任务**，永久 `unsupported_provider`；列表里也不出现 |

界面：添加账号面板顶部有 `WorkBuddy` / `WorkBuddy AI` 分段选择，切换即重开会话；
账号卡按 provider 显示独立名称与图标，国际版**不显示签到一栏**（产品里没有签到，不是"暂不支持"）。

## 11. 四平台额度看板（v14）

一段话：额度是**观测数据**，不是业务状态。所以它不进 `accounts`，只在
`quota_snapshots`（v14 建表，v15 去掉外键）里留快照，读取路径完全不碰账号与授权。

**为什么 v15 要改 v14 的表**：v14 最初把 `account_id` 建成指向 `accounts(id)` 的外键，
但四平台里 WorkBuddy 账号在 `accounts`、Qoder 服务账号在 `qoder_accounts` ——
一张"通用"缓存表没法同时对两张表建外键。SQLite 改不了外键，所以 v15 是一步
**按需重建**的 Rust 迁移：检测到还有外键就建新表搬数据，新建库上是空操作。
代价是没有级联：`delete_account` / `delete_qoder_account` 各加一条显式清快照
（账号 id 是 uid 的确定性哈希，重加同一账号会复用同一 id，残留快照会被当成"刚取过"），
外加 90 天裁剪兜底。

| 平台 | 数据来源 | 状态 |
| --- | --- | --- |
| `workbuddy`（国内版） | `www.codebuddy.cn` 官方只读接口 | `ok`（真实账号实测） |
| `workbuddy_ai`（国际版） | `www.workbuddy.ai` 官方只读接口 | `ok`（真实账号实测） |
| `qoder_cn` / `qoder_global` | **无** | `unverified`，零请求 |

接口细节与实测证据见 `docs/WORKBUDDY_QUOTA_API.md`；Qoder 的阻塞依据见
`docs/QODER_QUOTA_BLOCKER.md`。真机界面截图见
`docs/verification/quota-board-m4-2026-11-14.png`（5 个真实账号，钥匙串直读，非注入令牌）。

统一模型（`quota.board` 每行）：

| 字段 | 语义 |
| --- | --- |
| `accountId` / `provider` / `nickname` | 账号身份 |
| `source` | 这次用的是哪份授权（`own` / `magpie` / Qoder 的 `magpie` `own_browser`） |
| `status` | `ok` / `unauthorized` / `rate_limited` / `error` / `unverified` |
| `unit` / `total` / `used` / `remaining` | **单位原样带出，不同单位不相加**；未知一律 `null`，不是 0 |
| `usage` / `usageError` | 用量与它的失败原因。**用量失败不影响余额**，两件事分开显示 |
| `cached` / `stale` / `capturedAt` | 来自缓存 / 来自旧快照 / 快照时间 |
| `plan` / `isPaid` | 订阅名与是否付费（服务端字段直出） |
| `usage` | `windowDays` / `requests` / `credits` / `truncated` / 按模型的明细 |
| `syncAt` / `capturedAt` / `error` | 抓取时间、快照时间、失败原因（已过 `sanitize`） |

七条硬规则：

1. **未知不当 0**：`Packages` 为空、**某个包缺 `CycleTotalCapacity` / `CycleUsedCapacity`**、
   平台未接入，都返回 `null`。聚合是**按字段完整性**做的——三个字段各自累计，
   有一个参与计数的包缺该字段，这一项整体为 `null`。用 `unwrap_or(0)` 会把
   "不知道"写成"确实是 0"，界面就会画成"额度用完了"。
2. **部分成功**：额度与用量是两次独立请求。额度成功、用量 429/超时/解析失败时，
   余额照常返回，失败只写进 `usageError`。
3. **缓存 5 分钟 + 窗口必须一致**：`refresh=false`、快照在 300 秒内、**且快照里
   `usage.windowDays` 等于本次请求的 `days`** 才算命中。窗口不一致时**余额复用、
   用量按新窗口重取**——7 天的消耗绝不能被 30 天的查询拿去复用（实测：
   `days=7` 是 97 次 / 624.11 credit，`days=30` 重取后是 396 次 / 2346.34 credit）。
   上次用量取数失败（`usage=null`）也不算命中，下一轮会重试用量。
4. **刷新失败不丢数据**：有旧快照时返回旧数据并标 `status="stale"` + `stale=true`
   + `error`（本次失败原因）；没有旧快照才是真正的失败行。失败行**不落库**——
   一次临时 429 不该变成历史记录。
5. **批量有界**：3 路并发（`MAX_CONCURRENT`）、整轮 20 秒硬预算（`BOARD_BUDGET`），
   超预算的账号直接返回"缓存 + 过期"或 `timeout`，不拖累已经拿到的账号。
   Swift 侧 RPC 超时 45 秒，**必须大于**助手的预算。
   **钥匙串读取也在预算内**：`broker::resolve` 是同步阻塞调用，跑在 `spawn_blocking`
   里并套 `timeout_at`。首次授权弹窗时调用方最多等 20 秒就降级返回，
   不会把整轮看板挂住；但那个被阻塞的线程会一直到用户回答弹窗为止——
   这是 macOS 钥匙串的行为，无法取消。
6. **快照写入原子核验身份**：写入是同一条 `INSERT … SELECT … WHERE EXISTS`，
   只有账号仍存在、`provider` 未变、`auth_epoch` 未变才落库。取数有两三次网络往返，
   期间账号可能被删除或重新授权；账号 id 是 uid 的哈希，重新导入会复用同一个 id，
   一条晚到的快照会被当成"这个新账号刚取过"。账号删除时另有一条显式清理。
7. **只读**：`quota.board` / `quota.history` 不写 `last_used_at`、不改 `auth_status`、
   不刷新令牌。取数失败只影响这一行。
