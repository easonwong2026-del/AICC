# AICC UI V5 Phase 3：额度看板与今日任务交互精简（PR-B）

## 0. 基线与边界

- 基线：`origin/main` `88358879db06c6428c2c49426f39eeb28c9ad2de`（PR #119 已合并）。
- 基线 CI：run `44970957086`（`CI 8835887`）结论 **success**，9 个 job 全部 success：
  `changes / test / lint / rust / swift-test / macos-build / android / android-visual / macos-dmg`。
- 分支：`feat/ui-v5-quota-tasks-polish`，PR 标题 `feat(ui): v5-quota-and-daily-tasks-polish`，保持 Open。
- 继承：`docs/UI_V5.md`、`docs/UI_V5_QUOTA.md`、`docs/UI_V5_TASKS.md`、`docs/UI_V5_SETTINGS.md`、
  `docs/UI_V5_AUTOMATION.md`、`ConsoleTheme.swift`、`ConsoleComponents.swift`。
- 本轮只改：额度视图、今日任务视图、纯展示模型、UI 交互状态协调、视觉样本、测试与文档。
  **零改动**：`account-worker/`、Rust、SQLite schema、JSON-RPC、授权与凭据、签到/成长/旅行状态机、
  额度采集与缓存算法、自动计划调度、WorkBuddy 客户端轮换、Python、Android、`VERSION`。
- 没有新增颜色、字号、间距或第二套状态组件；列宽仍是页面局部常量。

## 1. Step 1：原页面问题清单（先审计，再修改）

### 1.1 两个「刷新」的真实调用路径

| 入口 | 实际调用 | 网络 | 写 | 操作后更新的状态 |
|---|---|---|---|---|
| 页头刷新图标（额度页） | `MainConsoleView.refreshCurrentPage` → `.quota` → `store.refreshQuota(force: true)` / `refreshWorkerQuota(force: true)` | Magpie `GET /v1/magpie/quotas?kept=1` + `POST /v1/magpie/refresh`；或 worker `quota.board {refresh:true}` | 否 | `snapshot` / `quotaError` / `workerQuotaRows` / `workerQuotaError` / `lastQuotaRefresh` |
| 页面内「刷新」（额度页） | 同上，逐字相同 | 同上 | 否 | 同上 |
| 7 / 30 天切换 | `refreshWorkerQuota(force: false, days:)` | worker `quota.board {days, refresh:false}`（助手优先用快照） | 否 | `workerQuotaRows` / `workerQuotaWindowDays` / `workerQuotaError` |
| 空 / 错误状态按钮 | `refreshQuota(force: true)` / `refreshWorkerQuota(force: true)` | 同上 | 否 | 同上 |

结论：额度页的两个「刷新」是**同一个方法的同一次调用**，没有任何语义差别。
7/30 天确实不同（只重算统计窗口），但它不是刷新按钮，而是筛选控件。

| 入口 | 实际调用 | 网络 | 写 | 说明 |
|---|---|---|---|---|
| 页头刷新（任务页 · 签到） | `loadCheckinAuto()` + `loadCheckin(cached: true)` | 否（只读本地库/助手状态） | 否 | 与页面「检查状态」不是同一件事 |
| 页头刷新（任务页 · 成长） | `loadGrowthAuto()` | 否 | 否 | 只重读开关与下次检查时间 |
| 页头刷新（任务页 · 旅行） | `loadTravel()` | 否（本地快照） | 否 | 与页面「重读快照」逐字相同 |
| 页头刷新（任务页 · Credits） | `refreshQoderCredits(silent: true)` | 否（`qoder.credits.auto.status` 读本地） | 否 | 与页面「刷新」同一调用，只差一次提示清理 |
| 签到「检查状态」 | `refreshCheckinStatus()` → `loadCheckin(cached: false)` | 是：每账号一次只读 `checkin.status` | 否 | 独立价值：只有它会问服务端 |
| 成长「扫描」 | `loadGrowth()` | 是：`growth.scan`（9 个只读 GET × N 账号） | 否 | 独立价值 |
| 旅行「重读快照」 | `loadTravel()` | 否 | 否 | 与页头重复 |
| Credits「刷新」 | `refreshQoderCredits()` | 否 | 否 | 与页头重复 |

### 1.2 逐项问题与处置

| # | 用户操作 | 实际调用 | 网络 | 写 | 更新 | 现在的问题 | 处置 | 如何证明没丢 |
|---:|---|---|---|---|---|---|---|---|
| Q1 | 额度页内「刷新」 | `refreshQuota/refreshWorkerQuota(force:true)` | 是 | 否 | 整份额度 | 与页头刷新完全重复；两个同名按钮没有区别 | **删除页内按钮**，页头保留唯一入口 | `QuotaRefreshPlan.pageRefresh` 单测 + 页头仍能刷新（截图/代码） |
| Q2 | 额度页头刷新 | 同上 | 是 | 否 | 整份额度 | help 只写「刷新当前页」，不说明是重新采集 | 改为按页文案 `ConsolePage.refreshHint` | `testRefreshHintsSeparateQuotaFromTasks` |
| Q3 | 额度摘要「N 个账号 · M 个服务商 · 更新于」 | 无 | 否 | 否 | — | 账号数在分组标题再说一次；窗口天数在工具栏已经有；「旧快照 N」在每行状态再说一次 | **删除计数与徽章**，只保留「Magpie 快照 MM-dd HH:mm」 | 截图 `quota-accounts-*` / `console-quota-*` |
| Q4 | Magpie → 账号额度切换 | `nav.quotaSource` | 否 | 否 | 渲染哪张表 | 切换后可能带着另一个来源的 provider 显示空表 | `selectQuotaSource`：只换表 + 清跨来源筛选 | `testSwitchingSourceKeepsWindowAndRowsIndependent` |
| Q5 | 7 / 30 天 | `refreshWorkerQuota(force:false, days:)` | 否（优先快照） | 否 | 用量窗口 | 与刷新并排，容易被当成同一种「刷新」 | 保留控件，help 写清「只重算用量窗口，不重新采集」 | `QuotaRefreshPlan.windowHint` + 单测 |
| Q6 | 额度空状态按钮（原「刷新」） | `refresh* (force:true)` | 是 | 否 | 整份额度 | 与页头同名 | 改名「读取额度」，作为上下文重试 | `state-empty-quota-*` 截图 |
| Q7 | 任务页头刷新 | 见 1.1 | 否 | 否 | 当前域本地状态 | help 与页面按钮无法区分 | 页头 help 按页说明「重读本地 / 不联网」 | 单测 + 截图 tooltip 文案 |
| Q8 | 旅行「重读快照」 | `loadTravel()` | 否 | 否 | 本地快照 | 与页头同一调用，两个入口 | **删除页面按钮**，仅留页头 | `testOnlyWriteActionsMapToCheckinRun` 之外的路径表（§3） |
| Q9 | Credits「刷新」 | `refreshQoderCredits()` | 否 | 否 | 自动领取状态 | 同上 | **删除页面按钮**，仅留页头 | 同上 |
| Q10 | 已完成行「立即签到」 | `checkin.run` | 是 | **是**（先查后写） | 状态/记录 | 「已完成」还挂着可点的写按钮，诱导重复执行 | 改为「复查」（只读 `checkin.status`） | `testCompletedCheckinOnlyOffersReadOnlyRecheck` + `checkin-all-done-*` |
| Q11 | 待确认行「立即签到」 | `checkin.run` | 是 | **是** | 状态/记录 | 服务端没结论时仍可再提交一次 | 改为「复查」，结果明确前不提交 | 同上（unknown → recheck） |
| Q12 | 授权失效行「立即签到（灰）」 | 无 | 否 | 否 | — | 只有灰按钮，没有出路 | 改为「去登录」（纯导航到 AI 账号） | 同上（login，rpc 为空） |
| Q13 | 网络失败行「立即签到」 | `checkin.run` | 是 | **是** | 状态/记录 | 文案不区分「首次」与「重试」 | 改为「重试」，help 说明仍先查后写 | `checkin-network-failed-*` |
| Q14 | 批量「签到未完成账号」 | `checkin.runPending` | 是 | **是** | 全体 | 无（保留） | 不改 | 截图 + 代码零 diff |
| Q15 | Qoder 自动领取开关 | `qoder.credits.auto.set` | 是 | **是**（设置） | 开关状态 | `!supportsAuto || isBusy` 会**阻断关闭** | `canToggleAuto`：只拦「新开启」与执行中 | `testCreditsAutoToggleKeepsDisablePathAfterLosingEligibility` + `credits-revoked-*` |
| Q16 | 同上，成功提示 | 同上 | — | — | 提示 | 先乐观提示「已开启」，失败后靠下次刷新纠正 | 写成功后**回读**，只有服务端状态一致才说成功 | 代码 + §5 |
| Q17 | 成长摘要「已领取 / 待处理」 | 无 | 否 | 否 | 摘要 | 直接读 `scan.counts`，与表格行是两套来源 | 改为展示行派生 | `testGrowthHeadlineDerivesFromDisplayRows` |
| Q18 | 失败但 `errorKind=unauthorized` | `checkin.run` | 是 | 是（旧） | 状态 | 显示「执行失败」，实际需要重新登录 | 状态改为「需要登录」，隐藏写入口 | `testCheckinUnsupportedAndExpiredCannotStartWrites` |
| Q19 | 960 宽任务页工具栏 | 无 | 否 | 否 | — | 三个分段控件写死帧宽，文字被压出边界（Qoder CN 被「每日签到」压住、Buddy 旅行被截断、检查状态被挤成小图标） | 分段控件改用自然宽度；窄窗口用 `ViewThatFits` 折成两行 | `console-checkin-960x640-{light,dark}` |

## 2. 操作映射

| 旧 | 新 | 类型 |
|---|---|---|
| 额度页内「刷新」 | — | 删除（与页头同一调用） |
| 额度页头刷新 | 保留（唯一入口），help 改成「向数据源重新读一次（只读，不会领取）」 | 保留 + 改名 |
| 额度摘要「N 个账号 · M 个服务商 · 更新于 … / 旧快照 N」 | 「Magpie 快照 MM-dd HH:mm」 | 合并 / 下沉 |
| 空状态「刷新」 | 「读取额度」 | 改名 |
| 旅行「重读快照」 | — （页头刷新即为同一调用） | 删除 |
| Credits「刷新」 | — （页头刷新即为同一调用） | 删除 |
| 「立即签到」 | 待签到 →「立即签到」；失败 →「重试」；已完成/待确认 →「复查」；授权失效 →「去登录」；不支持/执行中 →「—」 | 拆分 |
| 状态「执行失败」+ 详情红字 | `errorKind=unauthorized` 时统一为「需要登录」 | 合并 |
| 成长摘要读 `scan.counts` | 读展示行 `claimedTaskCount` / `pendingTaskCount` | 下沉到展示模型 |

**保留下来的唯一主按钮**：签到是「签到未完成账号（N）」，成长是自动开启时的「接单 / 领奖」，
旅行是行内「领取」，Credits 是行内「立即领取」。本轮没有新增主按钮。

## 3. 刷新 / 检查 / 写操作的调用对照

| 动作 | RPC / 网络 | 性质 | 界面文案 |
|---|---|---|---|
| 页头刷新（额度） | Magpie HTTP 或 `quota.board{refresh:true}` | 只读重新采集 | 图标 + `ConsolePage.refreshHint` |
| 7 / 30 天 | `quota.board{refresh:false, days}` | 只读、优先快照 | 控件（help 说明不重新采集） |
| 页头刷新（任务） | `checkin.auto.status` / 本地库 / `travel.summary` / `qoder.credits.auto.status` | 只读本地 | 图标 + 「重读本地状态（不联网）」 |
| 签到「检查状态」 | `checkin.status{cached:false}` | 只读联网 | 「检查状态」 |
| 签到行「复查 / 检查」 | `checkin.status{cached:false, accountIds:[id]}` | 只读单账号 | 「复查」/「检查」 |
| 签到行「立即签到 / 重试」 | `checkin.run`（服务端先查后写） | 写 | 「立即签到」/「重试」 |
| 签到行「去登录」 | 无 | 纯导航 | 「去登录」 |
| 成长「扫描」 | `growth.scan` | 只读联网 | 「扫描」 |
| 成长「接单 / 领奖」 | `growth.auto.tick{force:true}` | 自动开启时可能写 | 「接单 / 领奖」（关闭时是「只读检查」） |
| 旅行行内刷新 | `travel.refresh` | 只读联网 | 图标 |
| 旅行「派出 / 领取」 | `travel.depart` / `travel.claim` | 写 | 「派出」/「领取」 |
| Credits「检查今日」 | `qoder.credits.status` | 只读联网 | 「检查今日」 |
| Credits「立即领取」 | `qoder.credits.claim` | 写 | 「立即领取」（仅确认可领后亮起） |
| Credits 自动开关 | `qoder.credits.auto.set` | 写设置 | 开关 |

只读入口与写入口在文案上分开；`CheckinRowAction.rpc` 把这条契约写进类型，测试钉住
「只读动作只能是 `checkin.status`」。

## 4. 原业务功能保全矩阵

| 能力 | 位置 | 本轮状态 | 证据 |
|---|---|---|---|
| Magpie 额度 / 账号额度两条链路 | 额度页 | 保留，仍不相加 | `testCreditsNeverBecomeBalance` 等既有用例 + 截图 |
| 服务商 / 平台筛选 | 额度页 | 保留（切来源清空跨来源筛选） | 截图 `quota-accounts-filter-qoder`、`quota-magpie-filter-claude` |
| 7 / 30 天窗口 | 额度页 | 保留，语义写清 | `testWindowChangeIsLocalRecalculationOnly` |
| 0 / 未知 / 缓存 / 旧快照 / 授权失败 | 额度页 | 保留，未知不给绿点 | `testUnknownRemainingIsNotZeroAndNotLive` 等 |
| 每日 Credits 跳签到 | 额度页账号副行 | 保留 | 截图 |
| 单账号签到 / 批量签到 | 今日任务 | 保留（文案按状态分流） | 代码 + 截图 |
| 签到自动开关 | 今日任务 | 保留（资格规则未改） | 既有 `testCheckinAutoToggleRespectsEligibility` |
| 成长扫描 / 自动管理 / 接单领奖 | 今日任务 | 保留 | 既有用例 + 截图 |
| 旅行派出 / 领取 / 行内刷新 / 自动开关 | 今日任务 | 保留 | 既有用例 + 截图 |
| Credits 检查 / 领取 / 自动开关 | 今日任务 | 保留（开关资格修正） | 新用例 + 截图 |
| 奖励预览→确认→执行→回查 | 账号详情 | **零 diff** | `git diff` 不含 `ConsoleRewardsSection.swift` |
| 自动计划中心 / 轮换管理 | 任务页 · 自动计划 | **零 diff** | `git diff` 不含相关文件；`automation-*` 截图重渲染无变化 |
| 危险操作确认弹层 | 全局 | 未改 | 同上 |

## 5. Qoder Credits 自动开关边界

| 场景 | 允许开启 | 允许关闭 | 开关表现 | help |
|---|---|---|---|---|
| 支持 + 未开启 | ✅ | — | 正常 | 「每天按服务端活动窗口检查一次」 |
| 支持 + 已开启 | — | ✅ | 正常 | 「关闭后不再自动领取」 |
| **不支持 + 已开启（失去资格）** | ❌ | ✅ | 正常，可关 | 同上 |
| 不支持 + 未开启 | ❌ | — | 45% 透明 + 禁用 | 「该版本没有已证实的 Credits 接口，无法开启自动领取」 |
| 执行中（任意） | ❌ | ❌ | 45% + 禁用 | 「正在执行，结束后再切换」 |

提交后的真实状态：`setQoderCreditsAuto` 现在无论成功失败都重读一次 `qoder.credits.auto.status`；
只有回读到的值与请求一致才提示「已开启 / 已关闭」，否则提示「服务端没有确认这次修改，已重新读取真实状态」。
后端接口、调度与存储**未改**。

## 6. 前后真实截图（真实 SwiftUI 离屏渲染）

工具：`bash scripts/smoke-test-console-visual.sh`（本轮 144 张，CI Artifact `console-ui-visuals`）。
基线图来自 `origin/main` `8835887` 的同一脚本渲染（137 张），归档在 `docs/console-v5/polish/before/`。

| 场景 | 改造前 | 改造后 |
|---|---|---|
| 额度 · 账号额度 1120 浅色 | `polish/before/quota-accounts-1120x760-light.png` | `quota/after/quota-accounts-1120x760-light.png` |
| 额度 · 账号额度 1120 深色 | `polish/before/quota-accounts-1120x760-dark.png` | `quota/after/quota-accounts-1120x760-dark.png` |
| 额度 · 账号额度 960 | `polish/before/quota-accounts-960x640-light.png` | `quota/after/quota-accounts-960x640-light.png` |
| 额度 · 账号额度 1440 深色 | `polish/before/quota-accounts-1440x900-dark.png` | `quota/after/quota-accounts-1440x900-dark.png` |
| 额度 · Magpie 1120 / 960 | `polish/before/console-quota-1120x760-light.png`、`...-960x640-light.png` | `quota/after/console-quota-1120x760-light.png`、`...-960x640-light.png` |
| 额度 · 空状态 | `polish/before/state-empty-quota-light.png` | `quota/after/state-empty-quota-light.png` |
| 任务 · 全部已完成 | `polish/before/checkin-all-done-1120x760-light.png` | `tasks/after/checkin-all-done-1120x760-light.png` |
| 任务 · 1120 常态 | `polish/before/console-checkin-1120x760-light.png` | `tasks/after/console-checkin-1120x760-light.png` |
| 任务 · 960 工具栏 | `polish/before/console-checkin-960x640-light.png` | `tasks/after/console-checkin-960x640-light.png` |
| 任务 · 异常态 | `polish/before/checkin-attention-1120x760-light.png` | `tasks/after/checkin-attention-1120x760-light.png` |
| Credits · 常态 | `polish/before/checkin-credits-1120x760-light.png`、`credits-960x640-light.png` | `tasks/after/checkin-credits-1120x760-light.png`、`credits-960x640-light.png` |
| 旅行 · 960 | `polish/before/travel-960x640-light.png` | `tasks/after/travel-960x640-light.png` |

逐项复核：

- 额度页工具栏少了「刷新」，页头图标仍在；表头上方不再出现「N 个账号 · 近 7 天用量」。
- 960 下 `123456789.50 credits` 与 `0 credits` 都完整显示，无省略号、无重叠。
- 已完成行由「立即签到」变成「复查」；授权失效行是「需要登录 + 去登录」。
- 960 任务工具栏折成两行：三个分段控件完整可见，`检查状态` 不再被挤成图标。
- 旅行 / Credits 页面不再有第二个本地刷新按钮。

## 7. 状态与异常截图

| 状态 | 截图 |
|---|---|
| 0 / 未知 / 缓存 / 超长数值同屏 | `quota/after/quota-accounts-zero-unknown-1120x760-{light,dark}.png`、`...-960x640-light.png` |
| Magpie 旧快照 | `quota/after/quota-magpie-stale-1120x760-{light,dark}.png` |
| 额度读取失败 / 加载 | `quota/after/quota-accounts-error-1120x760-light.png`、`quota-accounts-loading-...` |
| 签到 · 全部已完成 | `tasks/after/checkin-all-done-1120x760-light.png` |
| 签到 · 授权失效 / 待确认 / 不支持 | `tasks/after/checkin-attention-1120x760-{light,dark}.png` |
| 签到 · 网络失败（重试） | `tasks/after/checkin-network-failed-1120x760-{light,dark}.png` |
| 签到 · 单个执行中 / 批量执行中 | `tasks/after/checkin-executing-...`、`checkin-batching-...` |
| Credits · 支持 / 不支持 | `tasks/after/checkin-credits-1120x760-light.png`、`credits-limited-1120x760-light.png` |
| Credits · 已开启但失去资格 | `tasks/after/credits-revoked-1120x760-{light,dark}.png` |
| 成长 · 自动开启 / 仅手动 / 未扫描 | `tasks/after/checkin-growth-...`、`growth-readonly-...`、`growth-unscanned-...` |
| 旅行 · 进行中 / 空 | `tasks/after/travel-1120x760-{light,dark}.png`、`travel-empty-1120x760-light.png` |
| 任务 · 空 / 错误 / 加载 | `tasks/after/state-{empty,error,loading}-checkin-{light,dark}.png` |

## 8. 测试

`swift test --package-path macos/MenuBarApp`（CI `swift-test`）。本机只有 Command Line Tools，
没有 XCTest，本地只做 `xcrun swiftc -typecheck`（全量 Sources + Widget 依赖），测试结果以 CI 为准。

新增/更新用例：

| 用例 | 覆盖 |
|---|---|
| `testQuotaRefreshEntryMapsToServerReadPerSource` | 额度刷新入口对应真实刷新方法（两个来源都是重新读） |
| `testWindowChangeIsLocalRecalculationOnly` | 7/30 只重算窗口，不是刷新 |
| `testRefreshHintsSeparateQuotaFromTasks` | 页头刷新在额度页是重新采集、任务页是重读本地 |
| `testSwitchingSourceKeepsWindowAndRowsIndependent` | 切来源不改统计口径、清跨来源筛选 |
| `testMagpieResetKeepsFullTextInHelp` | 重置时间列压缩后，原文仍在 help |
| `testCompletedCheckinOnlyOffersReadOnlyRecheck` | 已完成/待确认/待签到/失败/授权失效/不支持/执行中 七种操作 |
| `testOnlyWriteActionsMapToCheckinRun` | 只读动作绝不会映射到 `checkin.run` |
| `testGrowthHeadlineDerivesFromDisplayRows` | 摘要与列表同源（counts 故意写错也不影响） |
| `testCreditsAutoToggleKeepsDisablePathAfterLosingEligibility` | 失去资格仍可关、未开启不可开、执行中不可切 |
| 既有用例 | 状态映射、奖励入口、旅行阶段、自动计划中心等回归（零改动） |

## 9. CI

- 本轮 PR 的 `CI`（9 个 job）结果见 PR 检查页；`changes` 的路径过滤会让无关 job skipped，
  出现时会在 PR 正文与最终回复里写明原因（本轮改动了 `macos/`，`swift-test`/`macos-build` 必跑）。

## 10. 修改文件与业务零改动证据

| 文件 | 变更 |
|---|---|
| `Sources/Models/QuotaPresentation.swift` | `spokenLabel`、`resetHelp`、`QuotaRefreshIntent` / `QuotaRefreshPlan` |
| `Sources/Models/TaskCenterPresentation.swift` | `CheckinRowAction`、签到状态/动作/help、成长计数、Credits 开关资格 |
| `Sources/Models/ConsoleNavigation.swift` | `ConsolePage.refreshHint`、`selectQuotaSource` |
| `Sources/Console/ConsoleQuotaView.swift` | 删除重复刷新与重复统计、快照时间、a11y、重置 help |
| `Sources/Console/MainConsoleView.swift` | 页头 help 按页、额度刷新走统一计划 |
| `Sources/Console/ConsoleCheckinView.swift` | 窄窗口工具栏折行、只保留联网只读入口、成长摘要改签名 |
| `Sources/Console/ConsoleCheckinSection.swift` | 操作列按 `CheckinRowAction` 分流 |
| `Sources/Console/ConsoleCreditsSection.swift` | 开关按 `canToggleAuto` 禁用与 help |
| `Sources/Console/ConsoleStore.swift` | 新增只读 `recheckCheckin`；`setQoderCreditsAuto` 回读确认 |
| `Sources/Console/ConsoleComponents.swift` | 0% 进度条不再画 2pt 最短宽度 |
| `ConsoleVisual/ConsoleVisualSmokeMain.swift` | 新增 3 组样本 + 7 张渲染 |
| `Tests/*` | 9 个新用例 + 2 处语义修正 |
| `docs/console-v5/*` | 关键截图刷新与 `polish/before` 基线 |

```bash
# 业务零改动（应输出空）
git diff --stat origin/main -- account-worker server.py collectors tests android \
  macos/Widget macos/MenuBarApp/Sources/Services VERSION
```

## 11. 尚未验证的真实账号操作

- **真实网络**：额度重新采集、签到「复查」、签到提交、成长扫描/接单领奖、旅行派出/领取、
  Credits 查询/领取都没有在真实账号上执行（本轮禁止写操作）。
- **VoiceOver**：状态点的 `accessibilityLabel/Value` 已加，但未在真机 VoiceOver 下逐行朗读验证。
- **服务端拒绝开关**：`setQoderCreditsAuto` 的回读分支只在代码层验证，没有构造服务端拒绝的真实请求。
- **单账号 `checkin.status` 的返回形状**：按整队返回的同一结构解析（`accounts[]`），
  已在代码里对「返回里没有这个账号」做了兜底提示，但未用真实 worker 验证。
