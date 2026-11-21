# AICC UI V5：任务中心与自动轮换配置（PR A）

## 0. 基线与边界

- 基线：`origin/main` `4600677`（PR #118 已合并）。
- 分支：`feat/ui-v5-automation-center`，独立 PR，保持 Open。
- 本轮只改 SwiftUI 信息架构与交互：任务中心加「自动计划」模式，轮换拥有一套独立管理界面。
- **不改**：Rust Worker、SQLite schema、JSON-RPC、轮换调度器、凭据读写、Magpie 文件、Python、Android、版本号。
- 五类任务的调度、资格判定、写操作语义完全沿用既有实现；界面不新增隐式自动执行。

## 1. 原自动化入口审计

审计对象是「谁在什么时候自动执行、状态从哪来、哪些能改」。

| 任务 | 调度与状态来源 | 可修改 | 只读 | 资格 | 执行中锁定 |
|---|---|---|---|---|---|
| WorkBuddy 每日签到 | `checkin.autoStatus`（`enabled` / `enabledAccounts` / `planTime` / `nextRunAt` / `running`） | 每账号 `checkin.setEnabled` | 今日状态、最近结果 | 仅 WorkBuddy 国内版；需可用凭据 | 不允许改该账号开关 |
| 成长中心 | `growth.autoStatus`（`enabled` / `intervalSeconds` / `nextTickAt` / `running`） | 全局 `growth.setAuto(enabled)` | 扫描结果、任务状态 | 国内版账号 | `growthActing` 时按钮禁用 |
| Buddy 旅行 | `travel.summary.auto`（`enabledAccounts` / `busy` / 间隔） | 每账号 `travel.setEnabled` | 快照、状态、奖励 | 国内版账号；需 Buddy 资格（后端判定） | `travelActing` 时禁用该行 |
| Qoder 每日 Credits | `qoder.credits.autoStatus`（`supported` / `autoEnabled` / `lastRun`） | 每账号 `setAuto` | 今日可领、最近结果 | `supported == true`；不支持的账号后端拒绝 | `qoderCreditsBusy` 时禁用该行 |
| WorkBuddy 客户端轮换 | `workbuddy.rotation.status`（plan / schedule / current / lastRun / history / limits） | `rotation.save`、`rotation.setEnabled`、`stop`、`restoreOrigin` | `rotation.status`、`rotation.dryRun` | 国内版账号；凭据预检；客户端空闲 | `wbRotationBusy` / 当前轮次非终态 |

关键事实（决定界面能说什么）：

- 签到与成长是**全局计划 + 每账号开关**两层，不能合成一个总开关。
- 旅行的"已开启"是**账号数**，不是全局布尔。
- Credits 的"支持"与"开启"是两个维度，不支持的账号不能计入"已开启"。
- 轮换的 `recovery_required` 会被后端排除在活动轮次之外：`current == nil`、只有 `lastRun` 停在待处理。界面必须认 `attentionRun`，否则警示会消失。

## 2. 五类任务的状态映射

`Models/AutomationCenter.swift` 的 `AutomationPlanBuilder` 负责把 store 状态翻译成一行：

| 域 | 已开启的判定 | 摘要 | 未知 / 失败 |
|---|---|---|---|
| 签到 | `enabled && enabledAccounts > 0` | `5/8 个账号已开启` | `checkinAutoLoaded` / `checkinAutoError` |
| 成长 | `enabled` | `已开启 · 每天检查一次` | `growthAutoLoaded` / `growthAutoError` |
| 旅行 | `enabledAccounts > 0` | `3 个账号已开启` | `travelAuto == nil` + `travelError` |
| Credits | `supported && autoEnabled` 的数量 | `1/2 个账号已开启` | `qoderCreditsLoaded` / `qoderCreditsError` |
| 轮换 | `plan.enabled` | `已开启 · 2 个账号` | `wbRotationStatus == nil` + `wbRotationError` |

三条硬规则：

1. **未读取 ≠ 已关闭**。`unknown` / `loading` / `failed` / `off` 是四个不同状态，分别显示「未知 / 读取中 / 读取失败 / 未开启」。
2. **不支持 ≠ 未开启**。Credits 没有支持账号时显示「暂无支持账号」，不是「未开启」。
3. **读取失败不吞掉**。签到与成长的自动状态原本静默失败（"次要信息"），本轮只加了一个只读错误字段用于显示，不改变原流程。

## 3. 任务中心导航

```
任务中心（原「签到任务」，ConsolePage.checkin）
├── 今日任务  ← 原页面内容，未改表格与操作
│   ├── 平台：WorkBuddy CN / Qoder CN
│   └── 任务：每日签到 / 成长中心 / Buddy 旅行（Qoder：每日 Credits）
└── 自动计划  ← 新增
    └── 五行：签到 / 成长 / 旅行 / Credits / 客户端轮换
        └── 「管理」→ 对应今日任务域（轮换 → 轮换管理界面）
```

- 模式是 `ConsoleNavigation.taskCenterMode`，页面内导航，不增加第五个一级页面。
- 切换模式只改导航状态：没有 `.task` 依赖模式，自动化页只做一次只读补读（轮换状态为空时）。
- 五个「管理」入口都落到真实可编辑的地方，没有"看起来能管"的假入口。

## 4. 自动轮换管理界面

`RotationManagerView`（760×620）替换原来的 633 行单页弹层。

### 4.1 结构

```
自动轮换                                  [关闭]
[计划] [执行记录]
──────────────
计划（概览）      状态 / 参与账号 / 计划时间 / 下次运行 / 这一轮或最近结果 / 客户端登录
                  [设置计划…]
                  执行操作 ▸（立即执行 / 暂停并恢复 / 恢复最初账号）
计划（编辑）      第 1/3 步 · 参与账号
                  第 2/3 步 · 执行时间（星期、起止、停留、时区）
                  第 3/3 步 · 预演与保存
                  [取消] [上一步] [下一步] / [仅保存] [保存并开启]
执行记录          筛选（全部/成功/跳过/失败）+ 日期/账号/切换/活跃
```

### 4.2 概览只回答日常问题

- 未配置时只显示「还没有设置自动轮换」+「设置轮换计划…」，执行区隐藏。
- 读取失败时状态是「读取失败」、计划时间是 `—`、主按钮禁用、提示里带「重试」。
- 恢复异常时：顶部橙色警告 + 原因 + 「恢复最初账号…」按钮，执行区自动展开。
- 日常不展示状态机字段、锁、CAS、RPC 名称。

### 4.3 配置流程

| 步骤 | 内容 | 约束 |
|---|---|---|
| 1 参与账号 | 只列 WorkBuddy 国内版；勾选 + 顺序 + 上下移；授权失效的账号显示原因且不可勾选 | 已选中的失效账号仍可取消 |
| 2 执行时间 | 星期 / 开始 / 结束 / 每账号停留 / 时区 | 时区在这里可改，不再要求先去设置页；跨午夜显示「次日」 |
| 3 预演与保存 | 账号顺序、窗口、停留、时区、结束动作 + 预演按钮与结果 | 预演是只读 dry-run |

### 4.4 保存并开启的执行顺序

`RotationSaveFlow.run`：

1. `save`（有改动才执行）。
2. 保存失败 → **立即停止**，不调用 enable，保留表单内容。
3. `setEnabled(true)`。
4. 启用失败 → **回读服务端状态**，返回 `enableFailed`，界面不显示"已开启"。
5. 成功后回读，返回 `enabled`，退回概览。

「仅保存」只执行第 1 步，不开启。

### 4.5 编辑与刷新

- 服务端状态刷新只在**没有编辑**时重置草稿；编辑中的表单不会被覆盖。
- 关闭或切到「执行记录」时若有未保存修改，先弹「放弃未保存的修改？」。
- 底部常驻「有未保存的修改」提示。

## 5. 旧功能 → 新入口映射

| 旧入口 | 新位置 | 说明 |
|---|---|---|
| 侧栏「签到任务」 | 侧栏「任务中心」 | 只改标题 |
| 任务中心四个域 | 今日任务（默认模式） | 内容、表格、手动操作未改 |
| — | 自动计划（新） | 五类自动化集中查看 |
| 账号页「WorkBuddy 登录与夜间轮换」 | 同一按钮 → 轮换管理 | 落点改为概览 |
| 账号页客户端卡「管理 / 去处理」 | 轮换管理（`去处理` → 恢复异常焦点） | 仍是同一个界面 |
| 总览「轮换未恢复 / 状态无法确认」 | 轮换管理（`recovery` 焦点） | 打开即见恢复警告与按钮 |
| 设置 → 高级与诊断 → 账号轮换时区 | 保留 | 与计划同一份后端数据；轮换管理里也能改同一字段 |
| 旧弹层的一次性导航信号 | `ConsoleNavigation.requestRotationManager(focus:)` | 带 token，同一次请求只消费一次 |

## 6. 只读与写请求边界

**只读**（打开页面、切模式、切 tab、预演、刷新状态）：

- `checkin.status` / `checkin.autoStatus`
- `growth.autoStatus`
- `travel.summary`
- `qoder.credits.autoStatus`
- `workbuddy.rotation.status` / `rotation.dryRun`
- `accounts.list`

**写**（只在用户明确点击后）：

- `checkin.setEnabled`、`growth.setAuto`、`travel.setEnabled`、`qoder.credits.setAuto`
- `rotation.save`、`rotation.setEnabled`、`rotation.stop`、`rotation.restoreOrigin`、`rotation.runNow`

切换「今日任务 / 自动计划」、切换「计划 / 执行记录」、打开轮换管理都不会发写请求。

## 7. 状态一致性检查

| 场景 | 处理 |
|---|---|
| 读不到轮换状态 | 状态「读取失败」、计划时间 `—`、主按钮禁用、提示 + 重试；不显示「未开启」 |
| 读不到签到 / 成长自动状态 | 行显示「读取失败」+ 原因；不显示「未开启」 |
| 保存失败 | 停在编辑页、保留表单、底部红条说明「设置没有改动」，不调用 enable |
| 启用失败 | 回读服务端状态后显示失败，不显示「已开启」 |
| 恢复异常 | `attentionRun` 优先于 `current`；`current == nil` 也要出现警告 |
| 停止请求 | `stop_requested` 显示「当前操作结束后会自动恢复」，不宣称已恢复 |
| 未配置 | 「还没有设置自动轮换」+ 设置按钮，执行区隐藏 |
| 配置后关闭 | 概览显示「未开启」但与「未配置」区分（仍显示账号数与计划时间） |

## 8. 原有安全规则保留清单

1. 轮换仅适用于既有支持范围（WorkBuddy 国内版 + 凭据预检）。
2. 客户端空闲检查保留：未知空闲按"可能正在使用"处理，由后端判定。
3. 切换与手动操作共用互斥锁（`wbRotationBusy` / 后端执行权）。
4. 本轮最初账号的独立快照保留，恢复只用快照。
5. 窗口截止与恢复余量保留，界面只读展示（`lateStartToleranceSeconds` / `restoreReserveSeconds`）。
6. `stop_requested` 语义保留：停止是请求，等当前切换安全收尾。
7. 手动切换与轮换冲突规则保留（后端判定，界面不覆盖）。
8. 每日轮次上限保留：「立即执行一轮」的确认文案写明占用当天名额。
9. 恢复失败状态与人工确认保留：`restoreOrigin(force:)` 仍需确认。
10. 不自动发送聊天、不伪造使用行为。
11. 离屏测试不操作真实客户端。

危险操作仍各自确认：立即执行、暂停并恢复、恢复最初账号、放弃未保存修改、轮换开启（保存并开启的语义已在流程内保证顺序）。

## 9. 截图

工具：`bash scripts/smoke-test-console-visual.sh`（真实 SwiftUI 离屏渲染）。

本轮新增/替换场景：

| 场景 | 文件 |
|---|---|
| 自动计划 960/1120/1440 × 浅深色 | `automation-*` |
| 自动计划（状态读取失败） | `automation-degraded-1120x760-light` |
| 轮换概览（执行中） | `rotation-plan-760x620-light/dark` |
| 轮换概览（已关闭） | `rotation-off-760x620-light` |
| 轮换概览（未配置） | `rotation-empty-760x620-light` |
| 轮换概览（读取失败） | `rotation-error-760x620-light` |
| 轮换概览（需要恢复） | `rotation-recovery-760x620-light/dark` |
| 执行记录 | `rotation-history-760x620-light` |
| 编辑第 1/2/3 步 | `rotation-setup-*` |
| 保存失败 / 启用失败 | `rotation-save-failed-*` / `rotation-enable-failed-*` |
| 无可用账号 | `rotation-setup-no-account-760x620-light` |

系统 `alert` / `confirmationDialog` 无法真实离屏渲染，因此没有伪造截图；确认文案与触发条件由模型测试和源码核验覆盖（`RotationSaveFlow`、`RotationFeedbackText`、`runNow` / `restore` 的 alert）。

仓库内保存 17 张（`docs/console-v5/automation/after/`）；完整 126 张由 CI Artifact `console-ui-visuals` 承载。

改造前对照（旧单页弹层，来自 #115 验收集）：

- `docs/console-v5/after/rotation-sheet-560x660-light.png`
- `docs/console-v5/after/rotation-recovery-560x660-light.png`

## 10. 测试与 CI

（待 CI 完成后补齐）

## 11. 尚未验证的真实客户端场景

- 真实 WorkBuddy 客户端的一轮完整切换与恢复（本轮不操作真实客户端）。
- 真实唤醒/休眠、客户端正在使用时的空闲判定。
- 真实保存失败与启用失败的服务端返回（界面路径用模型测试覆盖）。
- 真实 Magpie 临时授权剩余时长的预演警告。
