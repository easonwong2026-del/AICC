# AICC UI V5：设置中心重构（Phase 2B）

## 0. 基线与边界

- 基线：`origin/main` `35e8ce6`（PR #115–#117 已合并）。
- 基线 CI：run `44882304525`（`CI 35e8ce6c`）结果为 `success`；`changes` / `test` / `lint` / `rust` / `swift-test` / `macos-build` / `android` / `android-visual` / `macos-dmg` 九个 job 全部成功。
- 分支：`feat/ui-v5-settings-center`，独立 PR，不自动合并、不打 tag、不改版本号。
- 本轮只改 SwiftUI 设置窗口、设置页派生模型与必要测试。不改 Rust Worker、SQLite schema、JSON-RPC 协议、凭据格式与迁移状态机、轮换状态机、签到/成长/旅行/额度业务、Python、Android。
- 唯一允许的 Swift 业务侧改动：`AccountWorkerManager.setPreferredAuthSource` 在助手未运行时恢复原值（PRD §6 P1-1 明确要求）。其余改动均为视图与纯展示模型。

## 1. 设置系统审计

### 1.1 代码事实

| 事实 | 位置 | 结论 |
|---|---|---|
| 设置窗口由 AppKit `NSWindow` + `NSHostingController` 管理，默认 580×560、最小 560×520 | `Services/SettingsWindowCoordinator.swift:92-99` | 保持 AppKit 窗口管理，不切换 SwiftUI `Settings` scene |
| 入口只有一处：主控制台右上角齿轮 → `openSettings` → `presentSettings()` | `AICCApp.swift:536, 606-608` | 保留；重复点击复用同一窗口 |
| 关闭窗口后 1 秒才调用 `onClose`，App 随后可能退出（未开桥接时） | `SettingsWindowCoordinator.swift:119-129`、`AICCApp.swift:610-620` | 关闭生命周期行为不得改变 |
| 语言/主题变化通过 `.id(SettingsPresentationIdentity)` 重建整棵 SwiftUI 树 | `SettingsWindowCoordinator.swift:17` | 保留；新结构也必须在 `.id` 下无状态丢失问题 |
| 设置页所有数据来自三个共享单例：`AppSettings.shared`、`ServerManager.shared`、`AccountWorkerManager.shared` | `SettingsView.swift:6-24` | 新结构继续复用单例，视觉渲染通过 fixture 注入 |
| `AppSettings.themeMode` / `languageCode` 只有数据层，**没有任何 UI 入口** | `Models/SettingsData.swift:41-58` | 不新增外观/语言设置，避免"数据层有字段就加功能" |
| 旧设置页单文件 440 行、单个 `Form`、8 个 Section | `Settings/SettingsView.swift` | 新结构拆为 4 分类 + 3 个详情页 |

### 1.2 旧设置页真实渲染（改造前）

由临时离屏渲染脚本生成，真实 `SettingsView` + 真实 SwiftUI 布局，固定样本、不写任何设置：

- `docs/console-v5/settings/before/settings-before-580x560-light.png`
- `docs/console-v5/settings/before/settings-before-580x560-dark.png`
- `docs/console-v5/settings/before/settings-before-580x2200-light.png`（完整高度，审计用）

审计确认的问题：

1. 580×560 首屏只能看到「Magpie 介绍 / Magpie 连接 / 桌面小组件」三组，墨水屏、凭据、迁移、诊断完全在滚动区以下。
2. 580 宽下「首选授权」的 segmented 控件被右边界裁切（"自有优先"右侧描边不可见）。
3. 8 个 Section 中 6 个含大段机制说明；`旧凭据迁移` 一节 4 段说明 + 5 个按钮 + 1 个折叠区，与"日常设置"同页。
4. 危险操作（重置证书身份、撤销全部设备）与普通按钮同一行、同一样式，只靠 `role: .destructive` 区分。
5. `诊断` 一节把「查看日志 / 项目与更新 / 退出 AICC」三个不同性质的动作放在一行。
6. 「助手状态」直接暴露 `未找到 aicc-account-worker，App 可能未正确构建` 这类构建期信息，未区分"未运行 / 运行中 / 协议不匹配"。

### 1.3 完整设置项映射（旧 → 新）

风险等级：**高** = 改安全边界或不可逆；**中** = 会影响运行中的服务或后台任务；**低** = 只影响展示或只读。

| # | 旧设置项 | 实际状态来源 | 修改方法（原有绑定） | 副作用 | 风险 | 新位置 |
|---:|---|---|---|---|---|---|
| 1 | 本机端口 | `AppSettings.magpiePort`（UserDefaults `magpiePort`，默认 3425，1…65535） | `TextField(value:format:)` 直接绑定 | 发布 Widget 设置；重读服务商；连接状态复位；清空配对码；桥接运行中则重启服务（`restartServer`） | 中 | 连接与设备 · Magpie |
| 2 | 检查连接 | 只读：`WidgetStatusLoader.load(port:)` | 按钮 → 异步探测 | 更新连接文案与服务商列表；发布 Widget 设置 | 低 | 连接与设备 · Magpie |
| 3 | 打开 Magpie | 无状态 | `NSWorkspace.open(/Applications/magpie.app)` | 启动外部 App | 低 | 连接与设备 · Magpie |
| 4 | 显示数量 | `AppSettings.widgetCards`（`widgetCards`，旧键 `widgetAccounts` 迁移） | `Picker` 绑定 | `publishWidgetSettings` + `WidgetCenter.reloadAllTimelines()` | 低 | 常规 · 桌面小组件 |
| 5 | 显示额度 | `AppSettings.widgetSelection`（`[String]`，空 = 自动） | 按钮 → `QuotaSelectionView` sheet，应用时写回 | 同上 + 顺序即显示顺序 | 低 | 常规 · 桌面小组件 |
| 6 | 启用墨水屏桥接 | `AppSettings.bridgeEnabled`（`bridgeEnabled`） | `Toggle` 绑定 | 开：`startMonitoring()`；关：`stopMonitoring()` + `stopServer(.user)` + `loginAtLaunch.setEnabled(false)`；清空配对码 | 高 | 连接与设备 · 墨水屏 |
| 7 | 桥接状态 | `ServerManager.isServerRunning` | 只读 | — | — | 连接与设备 · 墨水屏 |
| 8 | 运行时错误 | `ServerManager.runtimeError`（内置 Python 运行时缺失等） | 只读 | — | — | 连接与设备 · 墨水屏（异常行） |
| 9 | 允许局域网设备连接 | `AppSettings.bridgeLANEnabled`（默认关，老版本升级后也为关） | `Toggle` 绑定 | 清空配对码；桥接开启时先 `ensureBridgeTls()` 再 `restartServer()`（真实重启进程，改监听范围） | 高 | 连接与设备 · 墨水屏 |
| 10 | HTTPS 服务 | `ServerManager.lanServing`（`health.live.lan_serving`） | 只读 | — | — | 连接与设备 · 墨水屏 |
| 11 | 证书有效期 | `AccountWorkerManager.bridgeTls.notAfter` | 只读 | — | — | 连接与设备 · 设备安全 |
| 12 | 证书指纹（分组四位） | `bridgeTls.groupedFingerprint`（来自 `tls.ensure` / `tls.reset` 返回的 SPKI） | 只读 | — | — | 连接与设备 · 设备安全 |
| 13 | 复制指纹 | 同上 | `NSPasteboard.setString(tls.spki)` | 写系统剪贴板 | 低 | 连接与设备 · 设备安全 |
| 14 | 重置证书身份 | Worker RPC `tls.reset` | 按钮 → `resetBridgeTls()` → 成功后 `restartServer()` | 换根证书；所有已配对设备必须重新确认新指纹；端口重启；清空配对码 | 高 | 连接与设备 · 设备安全（详情内 + 独立确认） |
| 15 | TLS 错误 | `bridgeTlsError` | 只读 | — | — | 连接与设备 · 设备安全（异常行） |
| 16 | LAN 已开但监听未起 | `bridgeLANEnabled && !lanServing` | 只读警示 | — | — | 连接与设备 · 墨水屏（异常行） |
| 17 | 生成配对码 | 桥接 HTTP `POST http://127.0.0.1:8765/api/pairing` | 按钮 → 同步 HTTP，2 分钟本地倒计时清除 | 服务器生成一次性配对码 | 低 | 连接与设备 · 墨水屏 |
| 18 | 登录时启动桥接 | `LaunchAtLoginService.isEnabled`（`SMAppService.mainApp.status`） | `Toggle` → `setEnabled` | 注册/注销登录项；失败保留错误 | 中 | 连接与设备 · 墨水屏 |
| 19 | 撤销所有已配对设备 | `BridgeCredential` 文件 + 旧钥匙串条目 | 按钮 → 确认 → 停服务（等退出）→ `BridgeCredential.revoke()` → 重启监控 | 旧配对凭证立即失效，所有墨水屏需重新配对；失败提示钥匙串权限 | 高 | 连接与设备 · 设备安全（危险区） |
| 20 | 助手状态 | `AccountWorkerManager.state`（stopped/starting/running(health)/failed） | 只读 | — | — | 高级与诊断 · 账号助手 |
| 21 | 检查助手 | 同上 | 按钮 → `refresh()` | 可能启动/回收内嵌助手进程，读 health | 低 | 高级与诊断 · 账号助手 |
| 22 | 首选授权 | `preferredAuthSource`（助手 `app_settings`，`magpie`/`own`） | Segmented Picker → `setPreferredAuthSource` | 写助手设置；失败回滚 | 中 | 授权与存储 · 授权来源 |
| 23 | 授权设置错误 | `authSettingsError` | 只读 | — | — | 授权与存储（就近）；账号助手错误在高级 |
| 24 | 账号轮换时区 | 助手轮换计划 `plan.timezone`；可选值 `limits.supportedTimezones` | Picker → `workbuddyRotationSave(params:["timezone"])` | 写轮换计划；失败重新读取 | 中 | 高级与诊断 · WorkBuddy 轮换 |
| 25 | 轮换规则文本 | `lateStartToleranceSeconds` / `restoreReserveSeconds` | 只读（后端常量） | — | — | 高级与诊断 · WorkBuddy 轮换 |
| 26 | 轮换设置错误 | `rotationSettingsError` | 只读 | — | — | 高级与诊断 · WorkBuddy 轮换（异常行） |
| 27 | 凭据当前方式 | `health.credentials.jsonEnabled` → `headline` | 只读 | — | — | 授权与存储 |
| 28 | 凭据来源分布 | `CredentialStoragePolicy.summary`（本地 JSON / 钥匙串 / Magpie 条数） | 只读 | — | — | 授权与存储 |
| 29 | 启用本地 JSON | 助手设置 `credentials_json_consent` | 按钮 → `PlaintextCredentialSheet` → `setJSONCredentialStorage(true)` | 之后**新建**自有授权写明文 JSON；不迁移既有；失败留驻 sheet | 高 | 授权与存储（风险 sheet 保留） |
| 30 | 改回钥匙串 | 同上 | 按钮 → `confirmationDialog` → `setJSONCredentialStorage(false)` | 只影响之后新建授权；已写 JSON 的继续可用，不自动迁回 | 高 | 授权与存储 |
| 31 | 明文同意缺失提示 | `CredentialStoragePolicy.consentRequired` | 只读 | — | — | 授权与存储 |
| 32 | 凭据设置错误 | `authSettingsError` | 只读 | — | — | 授权与存储（异常行） |
| 33 | 钥匙串自检结果 | `secureStoreSummary` | 只读 | — | — | 授权与存储 |
| 34 | 测试钥匙串 | Worker RPC `secure.status` | 按钮 → `checkSecureStore()` | 读写一次钥匙串探针，**会弹系统授权框** | 中 | 授权与存储（需确认提示） |
| 35 | 仍在钥匙串 N 条 | `CredentialMigrationSummary.migratable` | 只读 | — | — | 凭据管理详情 |
| 36 | 已是本地 JSON N 条 | `.alreadyJSON` | 只读 | — | — | 凭据管理详情 |
| 37 | 已迁移（有记录）N 条 | `credentialMigration.migratedRecords` | 只读 | — | — | 凭据管理详情 |
| 38 | 由 Magpie 管理 N 条 | `.magpie` | 只读 | — | — | 凭据管理详情 |
| 39 | 借用授权仍指向钥匙串 + 重新关联 | `.needsRelink` | 按钮 → 确认 → `relinkBorrowedCredentials()` | **只改引用**，不复制令牌、不读令牌、不删旧条目 | 中 | 凭据管理详情 |
| 40 | 引用异常 N 条 | `.blocked` | 只读 | — | — | 凭据管理详情 |
| 41 | 旧键搬运失败 + 重试 | `keyMigration.failed` | 按钮 → 确认 → `retryKeychainKeyMigration()` | 读一次旧钥匙串，可能弹授权框；失败不影响已有授权 | 中 | 凭据管理详情 |
| 42 | 可回滚 N 条 | `.rollbackable` | 只读 | — | — | 凭据管理详情 |
| 43 | 最近核验结论 | `credentialVerifyReport` | 只读 | — | — | 凭据管理详情 |
| 44 | 刷新盘点 | Worker RPC `credentials.migration.inventory` | 按钮 → `refreshCredentialMigration()` | 只读：列引用与摘要，不读令牌 | 低 | 凭据管理详情 |
| 45 | 迁移全部 | `credentials.migration.run` | 按钮 → 确认 → `migrateCredentials()` | 先备份（不含令牌）再逐条搬；原始钥匙串不删；产生明文副本 | 高 | 凭据管理详情（危险区） |
| 46 | 核验 | `credentials.migration.verify` | 按钮 → `verifyCredentials()` | 读回本地 JSON 比对归属与指纹，扫描孤儿文件 | 低 | 凭据管理详情 |
| 47 | 回滚 | `credentials.migration.rollback` | 按钮 → 确认 → `rollbackCredentialMigration()` | 只回滚本次迁移且未被重新授权的副本；删不掉会留 cleanup_pending | 高 | 凭据管理详情（危险区） |
| 48 | 操作结论 notice | `credentialMigrationNotice` | 只读 | — | — | 凭据管理详情 |
| 49 | 逐条状态 | `credentialMigration.items` | 只读（可展开） | — | — | 凭据管理详情 |
| 50 | 查看日志 | `AICCPaths.logsDirectory` | 按钮 → `NSWorkspace.open` | 打开 Finder | 低 | 高级与诊断 · 诊断工具 |
| 51 | 项目与更新 | GitHub releases 固定 URL | 按钮 → `NSWorkspace.open` | 打开浏览器 | 低 | 常规 · 项目与更新 |
| 52 | 退出 AICC | 无状态 | 按钮 → `NSApp.terminate(nil)` | 退出 App；桥接关闭时会停服务 | 中 | 高级与诊断 · 退出 |

### 1.4 旧页面生命周期（必须等价保留）

| 时机 | 旧行为 | 新结构 |
|---|---|---|
| 首次出现 | `loginAtLaunch.refresh()`；`accountWorker.refresh()` | shell 根 `onAppear`，只跑一次（guard + 任务标记） |
| 首次挂载 | 读 Widget 服务商列表；`loadAuthPreferences()`；`loadRotationSettings()` | shell 根一次性只读加载，分类切换不重跑 |
| `bridgeEnabled` 变化 | 启停监控/服务，关闭时同时取消登录启动 | shell 根 `onChange`，与旧代码逐行等价 |
| `bridgeLANEnabled` 变化 | 清配对码；桥接开启时 ensureTls + restart | 同上 |
| `magpiePort` 变化 | reload widgets + 重读 providers + 复位连接 + 清配对码 + 桥接重启 | 同上 |
| `widgetCards` / `widgetProvider` / `widgetSelection` 变化 | `reloadWidgets()` | 同上 |
| 关闭窗口 | 1 秒后 `onClose`；未开桥接且无窗口则退出 | 不变（coordinator 未改） |

## 2. 新信息架构

### 2.1 四个分类

| 分类 | 内容 | 设计理由 |
|---|---|---|
| 常规 | 桌面小组件（显示数量、显示额度选择）、项目与更新 | 日常只碰这两项；一屏内完成 |
| 连接与设备 | Magpie（端口/检查/打开）、墨水屏（桥接开关、运行状态、LAN、登录启动、配对码）、设备安全（HTTPS 状态、指纹、复制、重置身份、撤销设备） | 三类连接对象分开；安全动作集中在「设备安全」组并放页面底部 |
| 授权与存储 | 首选授权来源、凭据存储方式（钥匙串/本地 JSON）、钥匙串自检、凭据管理入口 | 主页只回答"现在用哪种存储、有多少要处理"；迁移/核验/回滚/重新关联进详情 |
| 高级与诊断 | 账号助手（状态/检查）、WorkBuddy 轮换时区与规则、TLS 高级诊断、日志、退出 | 技术信息与低频动作下沉，减少日常干扰 |

### 2.2 不重复原则

- 同一控件只出现在一个分类：`首选授权` 只在授权与存储；`轮换时区` 只在高级与诊断；`TLS 指纹` 只在连接与设备的设备安全组（高级与诊断只显示证书代数/有效期/错误，不重复指纹）。
- 助手状态与错误：`authSettingsError` 贴近触发它的控件显示；`state.failed` 的进程级错误只在高级与诊断显示。

### 2.3 窗口与自适应

| 尺寸 | 行为 |
|---|---|
| 960×720（宽） | 左导航 152 固定 + 右侧内容最大宽度 640，增加留白 |
| 760×620（默认） | 左导航 152 + 内容自适应 |
| 560×520（最小） | 左导航收起为顶部紧凑 Picker，内容单列；不允许隐藏任何设置入口 |

- 内容区独立 `ScrollView`，导航不滚动；页面水平边距 24（比控制台 28 略紧，因窗口更窄）。
- 行高统一 32–36；分组用段标题 + 留白 + 细分隔线，不嵌套卡片。
- 危险操作固定在分组末尾，与普通按钮之间加分隔与说明，不放在同一行。

## 3. 组件结构

```
SettingsRootView（既有，未改）
└── SettingsView（改造为 shell）
    ├── SettingsCategoryNavigation（宽窗左侧：图标 + 名称，无副标题 / 窄窗顶部）
    └── SettingsDetailScrollView
        ├── GeneralSettingsSection
        ├── ConnectionSettingsSection
        ├── CredentialSettingsSection
        └── AdvancedSettingsSection
独立详情：
├── BridgeSecurityDetailView（证书指纹 / 有效期 / 重置身份 / 撤销设备）
├── CredentialMigrationDetailView（异常 → 检查 → 更多操作）
├── PlaintextCredentialSheet（既有，仅换 V5 样式）
└── QuotaSelectionView（既有，仅换 V5 样式）
```

复用 `ConsoleTheme` / `ConsoleType` / `ConsoleComponents` 的语义：
`ConsoleSectionHeader`、`ConsoleListSurface`、`ConsoleStatusDot`、`ConsoleInlineNotice`、
`ConsolePrimaryButtonStyle`、`ConsoleSecondaryButtonStyle`、`ConsoleIconButtonStyle`、`ConsoleEmptyState`。

设置页新增的只有布局级组件（不新增配色/字号 Token）：

- `SettingsGroup`：段标题 + 行容器 + 可选脚注。
- `SettingsRow`：`标签 / 说明 / 右侧控件`，统一最小高度与对齐基线。
- `SettingsFootnote`：12pt 次要说明，只在需要时出现。
- `SettingsDangerZone`：危险动作区（分隔线 + 风险说明 + 右置危险按钮）。

## 4. 文案删减清单

| 旧文案 | 新处理 |
|---|---|
| 「Magpie 显示伴侣」+ 两行产品介绍 | 删除。页面标题只写「设置」 |
| 「账号、模型和调用由 Magpie 管理…」 | 删除 |
| 「可以勾选具体窗口（例如只显示 Gemini 的 7 天额度），勾选顺序即显示顺序。」 | 折叠进「显示额度」行的 help |
| 「小尺寸最多 2 张、中尺寸最多 4 张…」 | 进 help |
| 「小组件独立读取 Magpie，关闭 AICC 后仍可更新…」 | 进 help |
| 「默认只监听本机（127.0.0.1）…」 | 改为 LAN 行下的一行状态：「仅本机」 |
| 首选授权来源 4 行机制说明 | 改为行内一句「只影响后台请求，不影响客户端登录」，其余进 help |
| 「助手是内嵌在本 App 内的独立可执行文件…」 | 进高级与诊断 help |
| 「账号管理已移到主控制台…」 | 删除（导航已有） |
| 轮换规则两行 | 保留为一行，完整规则进「轮换规则」help |
| 凭据存储 3 段说明 | 主页只留一行：「新授权写入 X；已有授权不受影响」 |
| 迁移规则 2 段 | 全部移入凭据管理详情顶部「说明」折叠 |
| 证书指纹 2 行解释 | 移入设备安全详情 help |
| 「重置会换掉根证书…」 | 保留在确认框（必须） |
| 「桥接不会在没有证书时退回明文」 | 保留为 LAN 打开且监听未起时的异常说明 |
| 「客户端登录的切换与恢复在…操作」 | 删除（主控制台可见） |
| Worker / RPC / Snapshot / cdhash / health | 普通界面不出现；仅高级诊断的原始错误行 |

保留的强制文案（不得删减）：

- 明文 JSON 风险与显式同意（sheet 内 5 条）。
- 重置证书身份的影响（所有已配对设备需重新确认）。
- 撤销设备的后果（需重新配对）。
- 迁移前备份、原始钥匙串不删除、回滚边界。
- 重新关联不复制/不读取/不删除令牌。
- 重试旧键搬运可能弹系统授权框。
- 切换回钥匙串不迁移已有 JSON。
- 关闭桥接会停止服务并取消登录启动。

## 5. 安全敏感操作保留矩阵

| 操作 | 原确认方式 | 新确认方式 | 变化 |
|---|---|---|---|
| 启用本地 JSON | 560×520 风险 sheet，5 条说明，默认按钮「我已了解，启用」 | 同 sheet（V5 样式），说明不减 | 仅视觉 |
| 改回钥匙串 | `confirmationDialog` + 说明「已写成 JSON 的不受影响」 | 同语义 dialog | 仅视觉 |
| 迁移全部 | `confirmationDialog`（先备份、可回滚） | 同语义 dialog，入口在凭据管理详情 | 位置变化 |
| 回滚 | `confirmationDialog`（只回滚本次迁移） | 同上 | 位置变化 |
| 重新关联 | `confirmationDialog`（只改引用） | 同上 | 位置变化 |
| 重试旧键搬运 | `confirmationDialog`（会弹授权框） | 同上 | 位置变化 |
| 撤销所有设备 | `confirmationDialog`（需重新配对） | 同语义 dialog | 仅视觉 |
| 重置证书身份 | **无确认，直接执行** | 新增独立确认：说明换根证书 + 设备需重新确认 + 执行中禁用 | **修复 P0-2** |
| 关闭桥接 | 无确认（关闭 Toggle 即停服务） | 保留原语义（不新增确认，避免改变日常行为），但行内说明写清后果 | 不变 |
| 退出 AICC | 无确认 | 保留 | 不变 |

## 6. UI 状态一致性修复

| 问题 | 位置 | 修复 |
|---|---|---|
| 首选授权保存失败时，若助手未运行，`preferredAuthSource` 已被乐观更新但未恢复 | `AccountWorkerManager.setPreferredAuthSource` | `guard` 失败分支恢复 `previous` 并记错误 |
| 重置证书身份无确认 | `SettingsView.lanStatus` | 新增 `TLSResetFlow` 状态机：idle → confirming → running → done/failed；只有 confirm 才调用 `tls.reset` |
| 证书已换但服务重启失败时提示不明确 | `resetIdentity` | 明确区分「身份已替换，但端口仍是旧证书」与「身份替换失败」，两者都不说"成功" |
| 助手未运行与协议不匹配混为一条 failed 文案 | 高级与诊断 | 拆成「未运行 / 启动中 / 运行中 / 版本不匹配 / 启动失败」五种展示 |
| 设置页加载、失败、未知不可区分 | 凭据存储、轮换设置、凭据盘点 | 引入 `SettingsLoadState`（unknown/loading/loaded/failed），未知不显示为 0 |

## 7. 实施结果

### 7.1 文件与职责

| 文件 | 变化 | 职责 |
|---|---|---|
| `Settings/SettingsView.swift` | 重写（440 → 90 行） | shell：自适应导航 + 内容滚动 + 生命周期 `onChange` |
| `Settings/SettingsModel.swift` | 新增 | 交互状态与动作落地；决策全部委托纯模型 |
| `Settings/SettingsComponents.swift` | 新增 | `SettingsPageHeader` / `SettingsGroup` / `SettingsRow` / `SettingsValueRow` / `DangerRow` / 导航 |
| `Settings/SettingsSections.swift` | 新增 | 四个分类的内容 |
| `Settings/SettingsDialogs.swift` | 新增 | 三个 sheet + 三个确认框（集中核对文案） |
| `Settings/CredentialMigrationDetailView.swift` | 新增（替换 `CredentialMigrationSection.swift`） | 凭据管理详情：摘要 → 异常 → 检查 → 更多操作 |
| `Settings/BridgeSecurityDetailView.swift` | 新增 | 设备安全详情：指纹、有效期、重置身份、撤销设备 |
| `Settings/SettingsWorkerPresentation.swift` | 新增 | `AccountWorkerManager.State` → 五种展示状态 |
| `Settings/PlaintextCredentialSheet.swift` | 重写（仅样式） | 明文风险与显式同意（5 条说明一字未删） |
| `Settings/QuotaSelectionView.swift` | 重写（仅样式） | 额度窗口选择（顺序、自动、失效项语义不变） |
| `Models/SettingsCenter.swift` | 新增 | 分类、设置项映射、状态派生、动作计划、TLS 流程、导航、窗口尺寸 |
| `AccountCenter/AccountWorkerManager.swift` | 最小修复 | `setPreferredAuthSource` 失败回滚 |
| `Services/SettingsWindowCoordinator.swift` | 改尺寸 | 760×620 / min 560×520，`SettingsRootView` 注入 model |
| `Services/LaunchAtLoginService.swift` | 视觉注入 | 只在双 flag 构建里存在 |
| `macos/SettingsVisual/*` | 新增 | 设置页离屏渲染入口与脱敏样本 |
| `scripts/smoke-test-settings-visual.sh` | 新增 | 51 张真实 SwiftUI 渲染 |
| `macos/MenuBarApp/Tests/SettingsCenterTests.swift` | 新增 | 30 个纯模型测试 |
| `Package.swift` | 补齐 | 把 `SettingsCenter` / `CredentialMigrationModels` 纳入 AICCCore；把已存在但未编译的 `CredentialMigrationModelsTests` 真正跑起来 |

### 7.2 架构要点

- **生命周期集中**：`bridgeEnabled` / `bridgeLANEnabled` / `magpiePort` / Widget 的 `onChange` 全部挂在 shell。旧实现只有一个 Form；拆页后如果把这些放进分页里，用户切走再改开关就再也不会生效。
- **分类切换是纯导航**：`SettingsNavigation.select` 只改一个 `@Published` 值并写 UserDefaults。`SettingsTrigger` 里没有"导航"这个 case，所以结构上不可能因为切页启停服务。
- **一次性只读加载**：`SettingsLoadGate` 保证助手健康检查、首选授权、轮换设置、凭据盘点只跑一次；切分类不重复请求。旧实现把 `loadAuthPreferences()` 与 `refresh()` 放在同一批 `.task`，助手还没起来时读取会静默 return，界面停在默认值——现在先 `await refresh()` 再并行读三份。
- **自适应**：宽度 < 700 时左导航收成顶部分类选择器（4 个分类全在）；≥ 700 时左侧 176 固定，内容最大 640 居中。任何尺寸都不隐藏设置入口。
- **不再嵌套卡片**：一个段 = 一个段标题 + 一个单层行容器 + 可选脚注。危险动作在同一容器末尾，用分隔线、风险说明和红色描边按钮区分。

### 7.3 窗口尺寸

| 场景 | 尺寸 | 结果 |
|---|---|---|
| 最小 | 560×520 | 顶部分段选择器；单列；无水平溢出 |
| 默认 | 760×620 | 左导航 176 + 内容 584；首屏可见各分类主要设置 |
| 宽 | 960×720 | 内容 640 居中，左右各 72 留白；不拉长行宽 |

## 8. 视觉验收（真实 SwiftUI 离屏渲染）

工具：`bash scripts/smoke-test-settings-visual.sh`
输出：`/private/tmp/aicc-settings-qa`，55 张 PNG。
样本：`macos/SettingsVisual/SettingsVisualState.swift`，固定且脱敏；不含真实令牌、指纹或账号。
安全：双 flag 构建里 `SettingsModel.loadOnce()` 直接返回，视图不挂任何写操作；`tls.reset` / 撤销设备 / 迁移 / LAN 修改都不会被调用。

### 8.1 覆盖

- 基础：4 分类 × 3 尺寸（560×520 / 760×620 / 960×720）× 浅色 / 深色 = 24 张。
- 连接与设备：Magpie 连接失败、桥接仅本机、LAN 正常、LAN 未监听、运行时缺失、已生成配对码（含最小窗）、登录项失败、证书未准备、证书重置失败、证书已换未重启。
- 授权与存储：本地 JSON、需要迁移（未核验）、首选授权保存失败（长错误）、助手未运行（禁用态）。
- 高级与诊断：助手启动失败（深色）、协议不匹配、轮换读取失败。
- 弹层：凭据管理（浅/深/空/有异常/展开）、设备安全（浅/深）、明文风险（浅/深/失败）、额度选择错误态。

### 8.2 仓库内保存

- `docs/console-v5/settings/before/`：3 张（旧页 580×560 浅/深 + 580×2200 全貌）。
- `docs/console-v5/settings/after/`：31 张（核心页 × 浅深色、两套边界尺寸、关键状态与弹层）。
- 其余 24 张由 CI Artifact 承载，不进仓库。

### 8.3 逐轮人工检查与修复

| 轮次 | 发现 | 处理 |
|---|---|---|
| 1 | 左导航摘要被截断（"Magpie、墨水屏与…"） | 摘要缩短为 4–6 字：小组件与更新 / Magpie 与墨水屏 / 授权来源与凭据 / 助手、轮换、日志 |
| 1 | 960 宽下内容左对齐、右侧大片空白不均 | 内容块改为居中（最大 640），只增加留白 |
| 1 | 证书指纹 12pt 等宽在 760 下折成两行且末行只剩一组 | 降到 11pt 等宽，一行放下 |
| 1 | "重置 / 撤销"按钮与普通按钮同色（ButtonStyle 内部覆盖了外部红色） | 新增 `SettingsDestructiveButtonStyle`：红字 + 淡红描边 + 淡红按下态 |
| 1 | "待处理"一行塞了 4 段文字、贴到卡片右边缘 | 收敛为「N 条待迁移 · M 项需处理」，明细进详情 |
| 1 | 助手状态把整条 health 摘要塞到右侧并染绿 | 右侧只显示"运行中"；摘要进标题下方；正常状态不再着色 |
| 1 | 桥接失败时，同一件事在脚注和红色提示里各说一遍 | 失败原因只留行内提示；关闭态的说明移到"启用桥接"行 |
| 1 | LAN 未监听时，"运行状态"和"局域网访问"说了同一句话 | 运行状态改为「运行中 · 仅本机」，原因只在提示条里说一次 |
| 2 | 凭据存储组把"助手未运行"写成"这个版本还没有…" | `SettingsCredentialView.resolve` 增加 `helperRunning`：暂时读不到 vs 老助手不支持分开 |
| 2 | 老助手会显示一个永远点不动的开关 | 老助手显示"不可用"文字；助手只是没跑起来时保留禁用按钮 |
| 2 | 首选授权失败的错误在两个分组各显示一次 | 凭据存储改用独立的 `credentialStorageError` |
| 2 | 保存失败提示在明文弹层的滚动区底部，用户看不到 | 失败提示固定到按钮上方；弹层高度 520 → 560，五条说明一屏可见 |
| 2 | 560×520 下配对码被挤到滚动区以下 | 配对码上移到局域网之前，并与按钮合并为一行；首屏可见完整 6 位数字 |
| 2 | 额度选择弹层读不到列表时，底部仍提示"2 项已不存在"且"应用"可点 | 只在有真实列表时提示；加载 / 失败时禁用"应用"（否则等于清空选择） |
| 3 | 深色下"管理凭据"弹层的标题与段标题几乎不可见 | 弹层根视图补 `ConsoleTheme.canvas` 背景，动态色才会按 dark 解析 |
| 3 | 助手启动失败只显示"助手未启动"，丢掉"找不到 helper" | 保留最多 3 行；协议不匹配补"请更新 AICC 或重新安装" |
| 3 | 证书已换但端口未重启时，运行状态仍是绿色"运行中" | 改为「运行中 · 证书未生效」+ warning，与橙色提示一致 |
| 3 | 两张状态截图与基础图完全相同（迁移提示、证书未准备） | 迁移提示只保留在详情（主页不重复）；证书未准备改用 960×720，让设备安全组可见 |
| 3 | `项目与更新` 分组标题与行标题重复 | 分组标题改「关于」 |
| 3 | 常规页 960×720 下方大片空白 | 接受：常规只放日常两项；不为此新增原本没有的设置项 |

### 8.4 三尺寸 × 浅深色检查结论

- 560×520：顶部分类选择器四个入口完整；无横向溢出；表格行不换行错位；配对码、首选授权 segmented、启用 JSON 按钮均完整。
- 760×620：左导航 176；首屏可见各分类的主要设置；配对码 / 检查连接 / 管理凭据按钮不被裁切。
- 960×720：内容 640 居中；证书指纹一行；危险按钮与普通按钮视觉分离。
- 浅色：白卡 + #F6F6F7 画布；正常状态为次要文字色，异常才用红/橙。
- 深色：卡片 #262628 / 画布 #1B1B1D；正文 #F5F5F7；提示条为深色软底；危险按钮红字可辨。
- 滚动：每个分类独立滚动，导航与页头不滚；弹层内容超高时滚动，底部按钮固定。

## 9. 测试

`macos/MenuBarApp/Tests/SettingsCenterTests.swift`，30 个用例，覆盖 PRD §9 的 14 项：

| PRD 要求 | 用例 |
|---|---|
| 1 四分类与原设置项目映射 | `testEverySettingItemBelongsToExactlyOneCategory`、`testEveryCategoryHasItemsAndATitle`、`testDangerousItemsLiveInTheExpectedCategories` |
| 2 导航切换不触发写操作 | `testCategorySwitchOnlyChangesSelection`、`testLoadGateRunsOnceAcrossCategoryChanges` |
| 3 窗口关闭/重开与选择状态 | `testCategorySelectionSurvivesWindowReopen` |
| 4 语言/主题变更 | `testPresentationIdentityChangesTriggerRebuildWithoutTouchingCategory` |
| 5 LAN 生命周期调用 | `testBridgeToggleEffectsMatchTheOriginalLifecycle`、`testLANToggleAlwaysRestartsAndPreparesACertificateFirst`、`testPortAndWidgetChangesReloadTheRightThings` |
| 6 TLS 重置必须确认 | `testTLSResetCannotRunWithoutConfirmation`、`testTLSResetConfirmationStartsExactlyOneRun`、`testTLSResetCancelReturnsToIdleWithoutRunning` |
| 7 重置失败与重启失败显示 | `testTLSResetDistinguishesIdentityFailureFromRestartFailure` |
| 8 JSON 风险确认 | `testCredentialViewKeepsConsentRequirementVisible`、`testCredentialViewMarksPlaintextStorageAsWarning`、`testCredentialViewSeparatesHelperDownFromLegacyHelper` |
| 9 凭据存储保存失败 | `testFailedCredentialSwitchNeverReportsSuccess` |
| 10 迁移/回滚/重新关联入口可达 | `testMigrationEntryStaysInCredentialCategoryAfterMove`、`testCredentialMaintenanceIsReachableFromTheCredentialCategory` |
| 11 授权来源失败回滚 | `testPreferredAuthRollsBackWhenSaveFails` |
| 12 Widget 变更后刷新 | `testWidgetChangeRepublishesAndReloadsTimelines` |
| 13 加载/错误/未知 | `testLoadStateKeepsUnknownApartFromFailure` |
| 14 禁用与重复点击 | `testBusyFlowBlocksRepeatedConfirmation` |
| 文案预算 | `testNoticeBriefDropsTrailingDetail` |

另外补了桥接状态派生（`testBridgePhaseOrdering`、`testBridgePhaseNeverCallsLoopbackNormalWhenLANIsSilentlyMissing`）、LAN 文案、助手状态、凭据摘要与窗口尺寸契约。

`CredentialMigrationModelsTests`（12 个用例）此前已存在于仓库但**没有进 testTarget**，本轮一并纳入，凭据盘点解析从此真正受测。

### 9.1 CI 发现并修复的两处

| 现象 | 根因 | 处理 |
|---|---|---|
| `swift-test` 的「Render companion settings acceptance images」失败 | `macos/Widget/DashboardVisualSmokeMain.swift` 调用无参 `SettingsView()`；设置页现在需要 model | 给它构造 `SettingsModel` 并渲染新的 760×620；它守的「桥接开关 → 关窗退出」断言保留 |
| 修完上一条后 `swift test` 编译失败 | 新纳入的 `CredentialMigrationModelsTests` 引用 `AccountWorkerManager`，而后者被排除在 AICCCore 之外 | 把 `reasonSummary` 移到 `CredentialMigrationModels` 的 `CredentialRelinkSummary`，Manager 转发；测试改测纯函数 |

### 9.2 结果

- CI run `44889194707`（HEAD `2bd3fd1`）**completed success**：
  `changes` / `test` / `lint` / `rust` / `swift-test` / `macos-build` / `android` / `android-visual` 全部 success。
- `swift-test` 内：`swift test` 通过（含本 PR 的 30 + 12 个新用例），设置页 51 张渲染并上传 artifact `settings-ui-visuals`。
- `macos-dmg` 在 PR 上按设计跳过。
- 本机无 Xcode/XCTest，无法 `swift test`；额外用一份 XCTest 替身跑过 `SettingsCenterTests`（170 个断言）与 `CredentialMigrationModelsTests`（42 个断言），均 0 失败。

## 10. 未验证的真实环境情况

- 未在真实 `NSWindow` 里手动点击验证（本轮验收全部是真实 SwiftUI 视图的离屏渲染，不是网页模拟）。窗口复用、最小化恢复、关闭后退出仍由原有 `SettingsWindowCoordinator` 负责，未改。
- 未用真实墨水屏 / Android 设备配对；`tls.reset` 与撤销设备的真实链路未执行（离屏渲染只覆盖执行后的界面状态）。
- 未在真实钥匙串上切换 JSON / 钥匙串存储；`credentials.migration.*` 未真实迁移。
- 未在真实 Magpie 上做连接探测；连接成功/失败的文案由固定样本覆盖。
- 深色 / 浅色是通过 `NSAppearance` + `preferredColorScheme` 渲染；系统"增强对比度""减少透明度"未单独验收。
- 语言切换只验证了 `.id(SettingsPresentationIdentity)` 的重建路径；本轮新增文案全部为中文硬编码，与设置页原有做法一致（原有设置页也没有本地化字符串）。

## 11. 第二轮收口：文案预算与减法清单

第一轮建立了四分类结构，但默认页面仍然常驻大量说明。第二轮只做信息减法，不改业务逻辑、不改安全确认。

### 11.1 默认页面的文案预算

| 位置 | 允许 | 不允许 |
|---|---|---|
| 左侧导航 | 图标 + 一行分类名 | 副标题、摘要、计数 |
| 页面 | 一行标题 | 解释页面用途的副标题 |
| 分组 | 一个短标题 | 默认显示的脚注、机制说明 |
| 普通设置行 | 名称 + 控件 / 当前值 | 介绍这段设置的段落 |
| 异常 | 一句话原因 + 处理入口 | 与正常状态并列的长解释 |
| 操作结果 | 一句摘要（≤44 字） | 常驻的完整日志与备份路径 |
| 危险操作 | 确认弹窗内完整影响说明 | 默认页面上预演一遍 |

### 11.2 删除（默认页面不再出现）

| 位置 | 删掉的文案 |
|---|---|
| 导航 | 「小组件与更新 / Magpie 与墨水屏 / 授权来源与凭据 / 助手、轮换、日志」四行副标题 |
| 全部页面 | 「桌面小组件与更新。」「Magpie、墨水屏桥接与证书。」「授权来源、凭据存放位置与旧凭据。」「账号助手、轮换时区、证书与日志。」四个 subtitle |
| 常规 | 「小组件独立读取 Magpie，关闭 AICC 后仍可更新。」 |
| 常规 | 「在 GitHub 上查看发布说明与安装包」 |
| 连接与设备 | 「端口与 Magpie 设置里的一致；改动会重启桥接并刷新小组件数据源。」 |
| 连接与设备 | 「仅使用小组件时无需桥接，关闭窗口即退出 AICC。」 |
| 连接与设备 | 完整的「证书有效期 / 证书指纹 / 重置会换掉根证书…」常驻块（进详情） |
| 连接与设备 | 「在墨水屏上输入，2 分钟内有效」改为只在有码时显示「2 分钟内有效」 |
| 授权与存储 | 「保存位置：钥匙串 7 · Magpie 2」（属来源分布，已移入详情并改名「盘点」） |
| 授权与存储 | 「之后创建的授权默认写入钥匙串。」「之后创建的授权写入本机 JSON（明文）…」 |
| 授权与存储 | 「尚未确认明文存储的风险：在启用之前…」 |
| 授权与存储 | 「查看逐条状态、迁移、核验与回滚」 |
| 授权与存储 | 钥匙串自检的 `keychain，12 ms`（可用时只说「可用」） |
| 管理凭据 | 副标题「旧钥匙串授权的盘点、迁移、核验与回滚」 |
| 管理凭据 | 「这些操作会做什么」折叠说明块 |
| 管理凭据 | 重复的分组标题「操作 / 盘点」与同义计数 |
| 管理凭据 | 常驻的备份路径（改为展开区的「完整结果」） |
| 管理凭据 | 页脚「不会显示任何令牌内容。」与底部第二个「完成」按钮 |
| 高级与诊断 | 「助手是内嵌在 App 内的独立进程，只通过本机管道通信，不开放网络端口。」 |
| 高级与诊断 | 完整的轮换规则段落（改为「账号轮换时区」的 help） |
| 高级与诊断 | 整个「TLS 诊断」分组（证书信息只在设备安全详情里有一份） |
| 高级与诊断 | 「重新检查助手进程与协议版本」「在 Finder 中打开 AICC 的日志目录」「关闭主控制台、设置窗口与账号助手」 |
| 高级与诊断 | 助手状态里的 `数据库正常 · WorkBuddy 已登录 · Qoder 已安装`（只留版本与账号数） |

### 11.3 隐藏（进 help / 详情 / 展开区）

| 内容 | 去处 |
|---|---|
| 首选授权为什么不影响客户端登录、两种模式的取令牌方式 | 「首选授权」行的 help 图标 |
| 小尺寸/中尺寸的卡片上限、勾选顺序 | 「显示数量」「显示额度」的 help |
| LAN 默认只听回环、不打开就连不上 | 「局域网访问」的 help |
| 轮换时区的固定偏移、不补跑、恢复余量 | 「账号轮换时区」的 help |
| 证书指纹、有效期、复制、重置身份、撤销设备 | 设备安全详情（独立弹层，520×520） |
| 迁移/回滚的完整规则、盘点明细、逐条状态、完整备份路径 | 管理凭据 → 更多操作 |
| 明文存储的 5 条风险 | 点「启用本地 JSON…」时的风险弹层（一字未删） |

### 11.4 保留（必须在默认页面可见）

- 首选授权（Magpie 优先 / 自有优先）。
- 当前凭据方式，以及「启用本地 JSON…」/「改回钥匙串…」。
- 待处理数量与「管理凭据…」。
- 桥接开关、运行状态、配对码、局域网访问、登录时启动、设备安全入口。
- 一条异常一行：局域网已允许但 HTTPS 未起、运行时缺失、证书未生效、登录项失败、助手启动失败、轮换读取失败、凭据保存失败、首选授权保存失败。
- 操作结果摘要（迁移/核验/复制指纹）。
- 迁移/回滚/重新关联/重试四个入口（在「需要处理」与「更多操作」里）。
- 全部确认弹窗：TLS 重置、撤销设备、启用 JSON、改回钥匙串、迁移、回滚、重新关联、重试旧键。

### 11.5 结构变化

- 设备安全从「常驻 5 行 + 2 个危险按钮」变成主页一行 `HTTPS 状态 + 详情…`；重置与撤销进入 520×520 详情。
- 桥接关闭时设备安全行显示「已关闭」，入口仍可达，但不展开任何证书信息。
- 管理凭据从「6 行盘点 + 5 个按钮 + 折叠」重排为：摘要 → 需要处理 → 检查 → 更多操作（迁移/回滚/盘点/逐条/完整结果）。
- 管理凭据只有右上角一个「关闭」；底部改为结果条（无按钮）。
- 检查行右侧常驻核验结论（`正常 5` / `异常 2`），失败仍标红。
- 「诊断」组收纳日志与退出，避免再分出两个只有一行的分组。

### 11.6 本轮截图

- 改造前（第一轮成品）：`docs/console-v5/settings/before-simplify/`（连接、授权、管理凭据 3 张）。
- 改造后：`docs/console-v5/settings/after/`（31 张，含 560/760/960 × 浅深色、异常态、设备安全详情、管理凭据四态）。
- 完整 55 张由 CI Artifact `settings-ui-visuals` 承载。

### 11.7 检查结论（560×520 / 760×620 / 960×720 × 浅色 / 深色）

- 连接与设备（桥接关闭）：两行设置 + 设备安全入口，页面无一句解释文字；无裁切。
- 连接与设备（桥接开启）：6 行，异常只出现一条橙色提示；指纹不再占用日常页面。
- 授权与存储：三组五行以内；待处理与「管理凭据」同一行；钥匙串自检只说「可用」。
- 管理凭据：打开即看到「N 项需处理」与三个异常入口；展开「更多操作」后自动滚到迁移/回滚；备份路径只在展开区。
- 常规 / 高级与诊断：各 2 个与 3 个分组，无脚注。
- 深色：所有正文与危险按钮对比度保持；弹层有 `ConsoleTheme.canvas` 背景，不再出现灰字压深底。
