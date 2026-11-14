# 主控制台 UI V3

本文是主控制台 **V3 信息架构与视觉规范**的权威说明。旧文档 [CONSOLE.md](CONSOLE.md) 里的页面结构已被本文取代；
数据链路、RPC 名称与验收方式仍以那边为准。

- 适用范围：`AICC.app` 主窗口（左侧四页 + 右上角设置齿轮）
- 不在本文范围：菜单栏行为、小组件、墨水屏 APK、账号助手内部协议
- 视觉证据：`docs/console-v3/`（33 张），可复跑脚本见 §7

---

## 1. 一句话结构

左侧固定四页，右上是**窗口级**标题栏（页面标题 + 刷新 + 设置齿轮），每个页面自己提供一条工具栏；
内容区按页面决定是否滚动。账号页需要双栏铺满高度，所以不再套统一 `ScrollView`。

| 页 | 职责 | 明确不放在这里 |
| --- | --- | --- |
| 总览 | 全局状态、异常、最近记录 | 登录表单、完整额度列表、全部账号 |
| AI 账号 | 账号列表、授权来源、账号详情、本机配置 | 额度图表、完整签到任务 |
| 额度看板 | Magpie 额度窗口 / 官方账号额度 | 登录表单、领取 POST |
| 签到任务 | 今日摘要、平台与任务切换、执行与历史 | 账号凭据管理、无关平台 |

窗口：默认 1120×760，最小 960×640，侧栏 202pt，页面边距 20pt。

---

## 2. 四页结构

### 2.1 总览

三层，自上而下：

1. **状态摘要**：服务账号 / 授权正常 / 今日完成 / 需要处理。
   宽窗四张横排，窄窗自动折成 2×2（`ViewThatFits`），不强行四连排。
2. **服务平台**：WorkBuddy CN、WorkBuddy AI、Qoder CN、Qoder International 四张卡。
   每张只有：平台名、账号数、正常数、异常数、最近同步时间。点击带着平台筛选跳到 AI 账号页。
3. **最近记录 | 待处理**：左列来自真实的签到 / 旅行 / Credits / 成长扫描结果，右列是授权异常与失败任务，
   每一行都能跳到对应页面并选中目标账号。

口径（写在 `ConsoleFleetSummary`，有单测）：

| 数字 | 来源 | 说明 |
| --- | --- | --- |
| 服务账号 | `accounts`（WorkBuddy 两个 provider）+ `qoder_accounts` | **不含**本机 Qoder 配置 |
| 授权正常 | WorkBuddy `authStatus == valid` 且令牌未过期；Qoder 有可用自有凭据 | Magpie 关联不算有效 |
| 今日完成 / 待完成 / 失败 | `tasks.overview`（本地记录） | 未查询过的项显示 `—`，不显示 0 |
| 需要处理 | 账号授权异常 + 失败任务 | 与"授权正常"卡的下标同一口径 |

### 2.2 AI 账号

- **左栏 244pt**：分组列表（WorkBuddy CN / WorkBuddy AI / Qoder CN / Qoder International / 本机配置）。
  行内只有平台图标、昵称、脱敏 UID 或邮箱、一个状态点；超过 20 个账号独立滚动。
- **工具栏**：自适应搜索框 + 平台筛选菜单（不再用固定 420pt 的分段控件）+ 账号数 + 「本机配置」+「添加账号」。
- **右栏详情**四块：基本资料 / 授权来源 / 可用能力 / 更多操作。
  授权来源逐份列出（自有 / Magpie / PAT），状态与到期时间各自独立；Magpie 关联只写「已关联」，不写「已授权」。
  没有接口支持的能力不渲染入口。

统一添加账号（唯一主入口）：`ActiveConsoleSheet` 单一路由 → 选平台 → 选方式 → 复用现有组件。

| 平台 | 方式 | 复用的组件 |
| --- | --- | --- |
| WorkBuddy CN / AI | 浏览器登录（同会话含二维码） | `AddWorkbuddyAccountSheet` |
| WorkBuddy CN / AI | 从 Magpie 导入 | `MagpieImportSheet` |
| Qoder CN / International | 浏览器授权 | `QoderDeviceAuthSheet` |
| Qoder CN / International | PAT 备用授权 | `QoderPatSheet` |

Qoder 没有 Magpie 导入通道（Magpie 只提供 WorkBuddy 令牌），所以不显示该入口。

**Qoder 服务账号删除**（`qoder.accounts.delete`，PR #77）走独立确认面板：

- 删：AICC 保存的凭据、授权元数据、领取记录、额度快照
- 不删：Magpie 配置、Qoder 客户端本机 Profile
- 不撤销服务端 PAT（要到 Qoder 官方账户页操作）
- 删除期间禁用同一账号的重复删除；失败保留账号并就地重试

**本机 Qoder 配置**单独分类、只读：扫描、识别版本、查看认证状态与快照、保存当前登录、重命名、移除。
账号切换仍永久关闭；未完成的切换恢复只在有记录时提示，且必须显式确认。

### 2.3 额度看板

顶部来源切换，两套数据**不合并、不相加**：

- **Magpie 额度**：`WidgetStatusLoader` + `QuotaSnapshot.slots`，按服务商分组，支持服务商筛选，显示缓存/过期标记。
- **账号额度**：`quota.board`（余额）+ `quota.history`（用量，7/30 天）。
  WorkBuddy 显示剩余/总额/用量；Qoder CN 显示 Credits **活动状态**（不是余额），并跳转签到任务页；
  Qoder International 显示未验证，不发请求。

额度页只做看、筛、刷、跳转。**没有任何领取 POST**。

### 2.4 签到任务

1. **今日摘要**：`tasks.overview` 的完成 / 待完成 / 自动任务 / 最近失败，附最近失败明细。
2. **平台**：WorkBuddy CN、Qoder CN（只有这两家真有签到类任务）。
3. **任务类型**：WorkBuddy → 每日签到 / 成长中心 / Buddy 旅行；Qoder → 每日 Credits。
   一次只渲染一个分区，复用 `ConsoleCheckinSection` / `ConsoleGrowthSection` /
   `ConsoleTravelSection` / `ConsoleCreditsSection`。

按钮语义（不允许混用）：

| 文案 | 行为 |
| --- | --- |
| 检查状态 / 检查今日 | 只读查询 |
| 立即签到 / 立即签到未完成账号 | 执行签到（先查再写） |
| 领取 Credits | 领取 POST，且需本次会话的只读检查确认可领后才可点 |
| 派出 / 领取旅行奖励 | 对应写操作 |
| 自动执行开关 | 只改该账号的任务开关 |

切页、切平台、切任务类型、点刷新都**不会**隐式触发任何写操作。

---

## 3. 组件清单

| 组件 | 文件 | 用途 |
| --- | --- | --- |
| `ConsoleTheme` / `ConsoleType` | `Console/ConsoleTheme.swift` | 唯一令牌来源：尺寸、语义色、字号 |
| `ConsoleCard` | `Console/ConsoleComponents.swift` | 白底 + 细边框 + 12pt 圆角，无阴影 |
| `ConsoleStatTile` | 同上 | 统计卡（标题 / 数值 / 说明） |
| `ConsoleBadge` / `ConsoleStatusDot` | 同上 | 状态徽章与状态点 |
| `ConsoleInlineNotice` | 同上 | 页面级 / 卡片级内联提示（信息 / 警告 / 错误 + 重试） |
| `ConsoleEmptyState` | 同上 | 空状态：说明原因 + 下一步 |
| `ConsoleLoadingRow` | 同上 | 局部加载，不锁死整页 |
| `ConsoleDetailGroup` / `ConsoleInfoRow` | 同上 | 详情分组与键值行 |
| `ConsoleSearchField` / `ConsoleFilterMenu` | 同上 | 自适应搜索、紧凑筛选菜单 |
| `ConsolePageScaffold` | `Console/MainConsoleView.swift` | 页面骨架：工具栏固定、内容可选滚动 |
| `ConsoleNavigation` | `Console/ConsoleNavigation.swift` | 页面内导航上下文（选中项、筛选、来源、任务类型） |
| `ActiveConsoleSheet` | `Console/ConsoleAddAccountSheet.swift` | 全页唯一的 sheet 路由状态 |

尺寸与字号（`ConsoleTheme`）：

| 项 | 值 |
| --- | --- |
| 页面边距 / 标题栏高 | 20pt / 58pt |
| 卡片圆角 / 内边距 / 间距 | 12 / 14 / 12pt |
| 控件高度 | 28pt |
| 页面标题 / 分组标题 | 19pt semibold / 13pt semibold |
| 正文 / 辅助 | 12pt / 11pt 起（**不再出现 10pt 及以下**） |

状态色：品牌青绿 `#328F80`（深色 `#45B3A0`）、正常绿、警告琥珀、错误红、未验证中性灰。
颜色只表达状态，平台之间不用不同底色区分。

---

## 4. 交互规则

1. **一个页面一个主操作**：账号页「添加账号」、额度页「刷新额度」、任务页按上下文。总览没有必须的主操作。
2. **弹层单路由**：所有流程由 `ActiveConsoleSheet` 呈现；不并行声明多个 sheet，不嵌套业务 sheet。
   每个弹层的内容区独立滚动，标题与底部按钮始终可见。
3. **错误分层**：账号级错误在账号详情；任务级错误在对应任务卡；页面级用内联提示；不因单个账号失败遮住整页。
4. **刷新按页**：`MainConsoleView.refreshCurrentPage()` 只拉当前页需要的数据；
   启动时先确保账号助手就绪，再**并行**拉四份数据（Magpie 慢不再拖住账号列表）。
5. **文案收口**：运行规则、接口前缀、所有权说明这类长文本进帮助气泡，首屏只留一行可操作信息。

QA 钩子（截图与验收脚本用，功能与手工操作等价）：

| 启动参数 | 作用 |
| --- | --- |
| `-AICCConsolePage overview\|accounts\|quota\|checkin` | 直接打开某一页 |
| `-AICCConsoleAccount workbuddy:<id>` / `qoderService:<id>` / `qoderProfile:<id>` | 预选账号 |
| `-AICCConsoleQuotaSource magpie\|accounts` | 额度页来源 |
| `-AICCConsoleCheckinPlatform workbuddy\|qoder` | 任务页平台 |
| `-AICCConsoleWorkbuddyTask checkin\|growth\|travel` | WorkBuddy 任务类型 |
| `-AICCOpenAddAccount 1` / `-AICCOpenMagpieImport 1` | 直接拉起添加账号 / Magpie 导入 |

---

## 5. 业务入口映射

| 原入口 | V3 位置 |
| --- | --- |
| 扫码 / 浏览器登录 WorkBuddy | AI 账号 → 添加账号 → WorkBuddy → 浏览器登录 |
| 从 Magpie 导入 | AI 账号 → 添加账号 → WorkBuddy → 从 Magpie 导入 |
| Qoder 浏览器授权 / PAT | AI 账号 → 添加账号 → Qoder → 浏览器授权 / PAT；或在账号详情里「重新授权」 |
| Qoder 删除服务账号（PR #77） | AI 账号 → 选中账号 → 更多操作 → 从 AICC 移除账号 |
| 扫描 / 导入本机 Qoder 配置 | AI 账号 → 本机配置 |
| Magpie 额度卡 | 额度看板 → Magpie 额度 |
| 官方账号额度 / 用量 | 额度看板 → 账号额度（7 / 30 天） |
| 每日签到 / 成长 / 旅行 / Credits | 签到任务 → 平台 → 任务类型 |
| 任务失败与需重新登录 | 总览 → 待处理（点击跳转到对应账号） |

---

## 6. 回归清单

布局（每个尺寸都要过）：

- [ ] 960×640 / 1120×760 / 1440×900 下无水平溢出、无被裁按钮
- [ ] 弹层的关闭与确认按钮始终可见，内容超高时内部滚动
- [ ] 账号 > 20 个仍可独立滚动与筛选
- [ ] 长昵称、长邮箱、长 UID 不破坏列表与详情
- [ ] 深浅色下对比一致；分段控件与开关使用品牌色

功能：

- [ ] 四平台账号列表、浏览器登录、PAT、Magpie 导入
- [ ] Qoder 删除成功 / 失败重试 / 删除期间禁用
- [ ] 本机配置只读管理（扫描 / 保存 / 重命名 / 移除）
- [ ] Magpie 额度多服务商、缓存与过期标记、服务商筛选
- [ ] 账号额度余额与用量、7/30 天切换、Qoder 活动状态与未验证提示
- [ ] 签到 / 成长 / 旅行 / Credits 的检查与执行分离、自动开关、历史记录
- [ ] 设置入口、Widget 构建、墨水屏 APK 构建未被改动

---

## 7. 视觉证据与复跑

```bash
bash scripts/smoke-test-console-visual.sh   # 输出 /private/tmp/aicc-console-qa/*.png
```

脚本用真实 SwiftUI 视图离屏渲染（`-D AICC_VISUAL_TEST`），**不需要屏幕录制权限**，CI 也会跑：

- 4 页 × 3 尺寸 × 深浅色 = 24 张
- 关键状态：账号详情（Qoder）、本机配置详情、账号额度、Credits 任务、成长任务、
  添加账号（平台 / 方式）、PAT 输入、删除确认
- 空状态与错误状态（总览 / 账号 / 任务 × 深浅色）

样本数据只在 `AICC_VISUAL_TEST` 构建里存在（`ConsoleStore.applyVisualFixture`），
生产构建不编译这段代码，界面不会出现假数据。

`docs/console-v3/` 里提交的 33 张是 24 张基础图 + 9 张关键状态图，均由该脚本产出。

`docs/console-v3/real-run/` 是真机运行截图（`screencapture -l <window>`，1120×760）：

| 文件 | 说明 |
| --- | --- |
| `before-accounts-1120x760-light.png` | 改造前（`main@ca253b7`）：420pt 分段筛选把工具栏挤爆，「扫…」「…」被截断，两条说明横幅占掉首屏 |
| `before-overview-1120x760-light.png` | 改造前总览：占位符「待接入」、重复的「查看账号」按钮 |
| `after-{overview,accounts,quota,checkin}-1120x760-light.png` | 改造后四页的真实运行结果（同一台机器、同一份账号库） |

> 「改造前」两张是用 `main@ca253b7` 的代码 + 固定样本离屏渲染的，避免把桌面上的其它窗口带进仓库。

---

## 8. 已知限制

- **Qoder International Credits 未验证**：没有已证实的接口，界面只显示未验证，不发请求。
- **Magpie 没有 Qoder 导入通道**：Qoder 只能用浏览器授权或 PAT。
- **Qoder 账号切换永久关闭**：`main.sqlite` 未按账号隔离，只保留识别 / 扫描 / 导入 / 快照能力。
- **WorkBuddy 账号删除没有后端接口**：详情页不提供该按钮，避免假入口。
- **WorkBuddy 单份授权移除没有接口**：只能重新授权覆盖，或用 Magpie 侧操作。
- **首屏耗时**：账号数约 5–8 秒（助手进程启动 + 健康检查）；`tasks.overview`（今日完成/待完成）
  在本机实测要 15–20 秒，期间显示「读取本地记录中」而不是 0；Qoder 列表同理，期间显示「尚未同步」。
  这是助手侧的单线程 RPC 耗时，UI 侧已把最便宜的汇总排在 Qoder 目录扫描之前。
- 桌面小组件在 ad-hoc 签名换构建后会丢实例，需要重新添加（与 UI 无关，见 RELEASING.md）。
