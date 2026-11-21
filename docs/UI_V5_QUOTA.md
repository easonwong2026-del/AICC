# AICC UI V5 Phase 2A-1：额度看板

## 0. 基线与边界

- 分支：`feat/ui-v5-quota-dashboard`，从 UI V5 Phase 1 最新 HEAD `e672f5c`（PR #115，未合并）。
- PR base：暂为 `feat/ui-v5-design-system-overview-accounts`；#115 合并后改为 `main` 并重新核查 CI。
- 继承规范：`docs/UI_V5.md`、`ConsoleTheme.swift`、`ConsoleComponents.swift`、`docs/console-v5/after/` 已验收截图。
- 本轮只改额度视图、必要的纯展示模型、视觉样本与文档。Rust / Python / Android / SQLite / RPC / 授权 / 签到 / 轮换 / 版本号零改动。

## 1. Step 1：旧额度页审计与映射

### 数据字段

| 来源 | 旧界面展示 | V5 处理 |
|---|---|---|
| Magpie | 服务商、账号标签、套餐、窗口名、剩余值/百分比、剩余进度条、总量、重置时间、整卡状态（失败/跳过/缓存） | 表头：账号 / 窗口 / 剩余额度 / 已用 / 重置 / 状态；每窗口一行，账号首行突出、续行弱化 |
| Magpie 余额卡 | 余额文本 + 分项（如赠送） | 一行「余额」，分项作为剩余额度下的小字；无重置/已用则显示 — |
| 账号额度 | 账号、套餐、剩余值+单位、剩余进度条、总额、用量（请求数 / credits / 窗口天数 / 截断）、用量错误、凭据来源、采集时间、行状态、每日奖励块 | 表头：账号 / 剩余额度 / 用量 / 更新 / 状态；用量显示为「N 次请求」+「credits · 近 N 天」，每日奖励放账号副行，绝不进剩余列 |
| 账号额度 | 每日奖励：状态、可领数量、活动窗口截止、去签到任务 | 账号副行短标签（可领/已领取/无/暂不支持/需要登录），整行可点跳签到；完整说明与窗口截止进 help |
| 两种来源 | 页面级「实时/缓存」、服务商数、账号数、更新时间 | Magpie 摘要保留账号/服务商/更新时间与缓存徽章；账号额度摘要保留账号数/窗口天数/旧快照数量；无数据时摘要不重复正文 |

### 操作入口

- 来源切换：Magpie 额度 / 账号额度（保留）。
- Magpie 服务商筛选（保留）；账号额度平台筛选（新增，消费既有 `nav.quotaProvider`，使账号详情「查看额度」的上下文真正生效）。
- 7 / 30 天用量窗口切换（保留，仍只调用 `refreshWorkerQuota(force:false, days:)`）。
- 刷新（保留，primary）。
- 空/错误状态的重试（保留）。
- 每日奖励跳签到任务（保留，改为账号副行可点标签）。

### 状态与错误

| 状态 | 旧展示 | V5 展示 |
|---|---|---|
| 正常 | Magpie「实时」+ 账号「正常」文字 | 只留 5pt 绿色状态点；不写字 |
| 缓存/旧快照 | Magpie 卡片「缓存」、账号「过期/缓存」 | 行内「旧快照」/「缓存」+ 页面摘要「缓存」「旧快照 N」 |
| 读取失败 | 整卡错误文案 | 行状态「读取失败」，错误原文进 help；页面级失败显示居中的可重试空状态 |
| 本轮跳过 | 「本轮跳过」 | 行状态「本轮跳过」 |
| 未验证 | 长句「未验证（该平台没有已证实的接口…）」 | 「暂不支持」+ help |
| 授权失效 | 「授权不可用，需要重新登录」 | 「需要登录」+ help |
| 未知额度 | 有时以空值/旧卡片呈现 | 剩余列「未知」或「—」，绝不显示 0 |
| 用量单独失败 | 卡片内红色长句 | 用量列「读取失败」警告色，余额保持原值，原文进 help |

### 可删除的说明文字

- 「账号级余额与用量。每日奖励不是余额，不与余额相加。」→ 删除，改为每日奖励副行与 help。
- 「未验证（该平台没有已证实的接口，未发送请求）」→ 「暂不支持」+ help。
- 「领取操作只在签到任务页执行」→ 删除，跳转标签自身说明目标。
- 「活动窗口截止 …（服务端时间）」→ 进每日奖励 help，不占行宽。
- 每个账号一张卡、卡内重复套餐徽章与状态说明 → 合并为单层表格。

## 2. Step 2：实现

- `ConsoleQuotaView.swift`：单层列表容器 + 每来源一个表格；无嵌套卡片。
- `Sources/Models/QuotaPresentation.swift`：纯展示模型，统一状态、剩余/单位、用量、更新时间、每日奖励标签与 help；视图不自行拼业务文案。
- `ConsoleStore.applyVisualFixture`：离屏样本补齐 `loadingQuota` / `loadingWorkerQuota`（仅视觉测试编译）。
- `ConsoleNavigation`：新增 `AICCConsoleQuotaProvider` QA 钩子（仅影响截图初始状态）。
- 复用 `ConsoleTheme` / `ConsoleType` / `ConsoleListSurface` / `ConsoleShareBar` / `ConsoleBadge` / `ConsoleEmptyState`，未新增第二套 Token。
- 列宽为页面局部网格常量（在 960 下按 696pt 内容宽计算），不进入全局设计 Token。

### 960×640 实测布局

- Magpie：账号弹性列（最小 140）+ 窗口 96 + 剩余 116 + 已用 88 + 重置 96 + 状态 72 + 间距/内边距，全部落在内容宽内。
- 账号：账号弹性列 + 剩余 128 + 用量 166 + 更新 92 + 状态 72。
- 数字和单位分别最小缩放；长名称中间省略 + help；重置时间压缩为 `MM-dd HH:mm`；状态文字最小缩放。

## 3. 业务边界核对

1. **不相加**：Magpie 与账号额度使用两个独立数组和两张表；页面不存在合计。
2. **不混单位**：`remainingText` / `unit` 原样显示，余额卡 `¥`、百分比、credits 各占一行/一格；无任何求和。
3. **7/30 天**：仍调用原 `refreshWorkerQuota(days:)`；用量文案使用 `usage.windowDays`，余额口径不变。
4. **来源/筛选/刷新/降级**：来源切换、Magpie 服务商筛选、账号平台筛选、7/30、刷新、空状态重试全部保留。
5. **缓存 vs 实时**：`QuotaDisplayState` 区分 live / cached / stale / failed / deferred / unverified / unauthorized；页面摘要与行级同时可见。
6. **未知不为 0**：`remaining == nil` 显示「未知」或「—」；测试覆盖。
7. **每日 Credits 不是余额**：只出现在账号副行，remaining 列不读取 credits；测试覆盖。
8. **后端零改动**：未修改额度 RPC、参数、缓存策略或任何 Rust/Python 文件。

## 4. Step 3：真实截图

- 工具：`bash scripts/smoke-test-console-visual.sh`（真实 SwiftUI 离屏渲染）。
- 本轮最终渲染 92 张；仓库保留 Phase 2A-1 对照集：
  - `docs/console-v5/quota/before/`：6 张（旧额度卡栅格，3 尺寸 × 浅深色，取自 `e8ed661`）。
  - `docs/console-v5/quota/after/`：Magpie 与账号额度各 3 尺寸 × 浅深色，外加旧快照、30 天、长名称、筛选、加载/空/错误状态。
- 完整批量图由 CI Artifact `console-ui-visuals`（swift-test job）承载。

## 5. Step 4：测试与验证

- 纯展示测试 `QuotaPresentationTests`：未知不为 0、未验证非失败、旧快照优先、用量窗口、用量失败不遮余额、每日奖励不冒充余额、长数字完整、Magpie 百分比/计数/余额/失败/多窗口、每日奖励状态。
- 视觉检查：960/1120/1440 × 浅深色；Magpie、账号额度、长名称、不同单位、旧快照、空/加载/错误、7/30、平台筛选。
- 工程：Swift 编译；`swift test` 交给 CI；`AICC_ALLOW_ADHOC=1 SKIP_RUST_HELPER=1 bash macos/build-aicc-swiftui.sh` 整包构建；CI 全绿后报告。

## 6. 尚未验证 / 已知差异

- **账号额度没有重置时间字段**：`quota.board` 不提供重置时间，因此账号表用「更新」列展示采集时间，不伪造重置；Magpie 表保留「重置」。
- 真实网络下的 Magpie / 账号额度未在本轮调用；验收使用固定脱敏样本。分页/超长列表仅由 7 行样本覆盖，未做数百行滚动压测。
- Qoder 每日奖励的完整活动窗口截止只在 help 中，不占表格列宽。
