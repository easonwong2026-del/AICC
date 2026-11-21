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
计划（概览）      自动执行 [开启… / 关闭] / 参与账号 / 计划时间 / 下次运行 / 这一轮或最近结果 / 客户端登录
                  状态（仅执行中、待处理、读取失败时出现）
                  [设置计划…]
                  执行操作 ▸（立即执行 / 暂停并恢复 / 恢复最初账号）
计划（编辑）      第 1/3 步 · 参与账号
                  第 2/3 步 · 执行时间（星期、起止、停留、时区）
                  第 3/3 步 · 预演与保存
                  [取消] [关闭自动轮换] [上一步] [下一步] / [仅保存] [保存并开启]
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
| 3 预演与保存 | 账号顺序、窗口、停留、时区、结束动作 + 预演按钮与结果 | **预演只针对已保存计划**；草稿有改动时按钮禁用，先「仅保存」 |

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
- **确认关闭 = 真的关闭窗口**；取消 = 留在原处并保留内容。
- **确认切 tab = 切到执行记录**；取消 = 回到「计划」的编辑页，不会停在执行记录。
- 保存执行中禁止关闭/切 tab（按钮禁用 + `RotationLeavePolicy` 忽略），避免丢状态。
- 底部常驻「有未保存的修改」提示。

## 5. 旧功能 → 新入口映射

| 旧入口 | 新位置 | 说明 |
|---|---|---|
| 侧栏「签到任务」 | 侧栏「任务中心」 | 只改标题 |
| 任务中心四个域 | 今日任务（默认模式） | 内容、表格、手动操作未改 |
| — | 自动计划（新） | 五类自动化集中查看 |
| 账号页「WorkBuddy 登录与自动轮换」 | 同一按钮 → 轮换管理 | 落点改为概览 |
| 账号页客户端卡「管理 / 去处理」 | 轮换管理（`去处理` → 恢复异常焦点） | 仍是同一个界面 |
| 总览「轮换未恢复 / 状态无法确认」 | 轮换管理（`recovery` 焦点） | 打开即见恢复警告与按钮 |
| 设置 → 高级与诊断 → 账号轮换时区 | **改为只读** | 显示服务端当前值和「去哪里改」；唯一编辑入口是轮换管理，避免两处各改一份 |
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
| 有缓存但最新读取失败 | 行显示「上次：已开启 … · 当前无法确认」，警告色；不展示过期「下次执行」 |
| 关闭计划时这一轮还在收尾 | 状态「正在停止」，自动执行「未开启」，等轮次终态后才能再开启 |
| 恢复失败但计划已关闭 | 仍然警告「需要恢复」，摘要写「已关闭 · 需要恢复」，不反推计划仍在运行 |
| 保存失败 | 停在编辑页、保留表单、底部红条说明「设置没有改动」，不调用 enable |
| 启用失败 | 回读服务端状态后显示失败，不显示「已开启」 |
| 恢复异常 | `attentionRun` 优先于 `current`；`current == nil` 也要出现警告 |
| 停止请求 | `stop_requested` 显示「已请求停止，会在安全收尾后恢复」，不宣称已恢复 |
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

危险操作仍各自确认：**首次开启（关闭→开启）**、立即执行、暂停并恢复、恢复最初账号、放弃未保存修改。

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
| 轮换概览（缓存在、最新读取失败） | `rotation-stale-760x620-light/dark` |
| 轮换概览（需要恢复，计划开启/关闭） | `rotation-recovery-760x620-light/dark`、`rotation-recovery-closed-760x620-light/dark` |
| 关闭后正在收尾 | `rotation-disable-waiting-760x620-light/dark` |
| 执行记录 | `rotation-history-760x620-light` |
| 编辑第 1/2/3 步 | `rotation-setup-*` |
| 草稿未保存（预演禁用） | `rotation-preview-dirty-760x620-light/dark` |
| 已开启 + 未保存修改（仍可直接关闭） | `rotation-setup-enabled-dirty-760x620-light` |
| 保存失败 / 启用失败 | `rotation-save-failed-*` / `rotation-enable-failed-*` |
| 无可用账号 | `rotation-setup-no-account-760x620-light` |

系统 `alert` / `confirmationDialog` 无法真实离屏渲染，因此没有伪造截图；确认文案与触发条件由模型测试和源码核验覆盖（`RotationSaveFlow`、`RotationFeedbackText`、`runNow` / `restore` 的 alert）。

仓库内保存 32 张（`docs/console-v5/automation/after/`）；完整 137 张由 CI Artifact `console-ui-visuals` 承载。

改造前对照（旧单页弹层，来自 #115 验收集）：

- `docs/console-v5/after/rotation-sheet-560x660-light.png`
- `docs/console-v5/after/rotation-recovery-560x660-light.png`

## 10. 测试与 CI

### 10.1 测试

| 文件 | 用例 | 覆盖 |
|---|---:|---|
| `AutomationCenterTests` | 33 | 五类状态派生（未知/读取中/失败/关闭/开启/执行中/**缓存+失败**）、Credits 只计支持账号、轮换概览（未配置/关闭/开启/恢复异常/正在停止/未知）、恢复异常按 `plan.enabled` 区分、`planKey` 覆盖全部预演字段、预演态四条分支、草稿脏判定与排序稳定、`params` 键与顺序、保存并开启四条分支、关闭不需要保存、离开策略（关闭/切 tab/取消编辑 × 脏/保存中）、星期与时间文案 |
| `RotationManagerModelTests` | 21 | **生产控制路径**：关闭不保存无效表单、执行中仍可关闭、关闭失败回读、开启先保存再启用、保存失败不启用、Worker 不可用不伪装成功、首次开启确认（取消零请求/确认后才执行/已开启不再问）、放弃修改后真正关闭、取消关闭保留内容、切 tab 先确认且取消回原页、无修改直接离开、保存中拒绝导航、预演与草稿一致性、焦点落点 |
| `WorkbuddyRotationFeedbackTests` | 8 | 执行/停止文案语义；关闭计划后「另一操作占锁 / 停止请求 / 已恢复 / 恢复失败 / 无事可停」分别反馈；`stop_requested` 不得出现「已恢复」 |
| `ConsoleNavigationTests` | 7 | 三个入口共用一次性请求、token 每次变化、任务中心模式切换不产生请求、自动计划的管理路由 |

本机没有 XCTest，使用一份 XCTest 替身编译并**真跑**了本次受影响的三套测试：
`AutomationCenterTests` + `RotationManagerModelTests` + `WorkbuddyRotationFeedbackTests` 共 62 个用例（334 断言），
以及 `SettingsCenterTests` + `ConsoleNavigationTests` 共 41 个用例（216 断言），全部 0 失败。
最终以 CI `swift-test` 为准。

### 10.2 UI 操作 → 生产代码绑定

| 界面操作 | 代码路径 |
|---|---|
| 概览「关闭」/「开启…」 | `RotationManagerView.autoRunRow` → `RotationManagerModel.requestPlanEnabled` → `setPlanEnabled` → `RotationPlanControl.setPlanEnabled` → `ConsoleStore.setWorkbuddyRotationEnabled` |
| 编辑底部「关闭自动轮换」 | 同上，`dirty` 为 true 时仍只走 `[.apply]` |
| 「保存并开启」（首次） | `requestSaveAndEnable` → `pendingEnable = .saveAndEnable` → alert → `confirmFirstEnable()` → `performSave(enable: true)` |
| 「仅保存」 | `requestSaveOnly` → `performSave(enable: false)` → `RotationSaveFlow.steps == [.save]` |
| 第三步「预演已保存计划」 | `onPreview` → `previewSavedPlan`（`preview.canPreview` 为 false 时不发 RPC）→ `ConsoleStore.previewWorkbuddyRotation` → 记录 `wbRotationDryRunPlanKey` |
| 关闭窗口 / 切 tab | `requestClose` / `requestTab` → `RotationLeavePolicy.decide` → `pendingLeave` → `confirmLeave` / `cancelLeave` |
| 概览开关失败 | `RotationPlanControl` 返回 `.applyFailed` → 已 `reload` → `ConsoleStore` 的 notice 显示真实原因 |

系统 `alert` / `confirmationDialog` 无法离屏渲染，因此没有伪造截图；文案与触发条件由上述模型测试覆盖。

### 10.3 CI

- HEAD `0ca1d0d`，run `44912283147` **completed success**：
  `changes` / `swift-test` / `macos-build` 全部 success；
  `rust` / `android` / `test` / `lint` / `android-visual` 按路径过滤 skipped；`macos-dmg` 在 PR 上按设计跳过。
- `swift-test` 内：`swift test` 通过，控制台 137 张与设置 59 张离屏渲染并上传 artifact（`console-ui-visuals` / `settings-ui-visuals`）。
- 基线：上一轮 `7d02a95` 的 run `44907673299` 也是 success。

## 11. 尚未验证的真实客户端场景

- 真实 WorkBuddy 客户端的一轮完整切换与恢复（本轮不操作真实客户端）。
- 真实唤醒/休眠、客户端正在使用时的空闲判定。
- 真实保存失败与启用失败的服务端返回（界面路径用模型测试覆盖）。
- 真实 Magpie 临时授权剩余时长的预演警告。
- 系统原生弹窗的实际点击顺序无法离屏渲染；只由模型测试 + 绑定不允许在 `set(false)` 里取消来保证。

## 12. 最终交互安全收口

本轮只修交互与状态一致性，不改视觉结构、调度、RPC、存储。

### 12.1 P0-1 重新可以关闭自动轮换

- 概览新增「自动执行」行：`开启… / 关闭` + 当前值。
- 关闭直接调用既有 `workbuddy.rotation.setEnabled(false)`；**关闭永远不保存表单**（`RotationPlanControl.steps(enabled:false,*) == [.apply]`）。
- 编辑页底部也提供「关闭自动轮换」，不受未保存/无效草稿阻挡。
- 关闭后按服务端 `stopResult` 分开反馈：

| stopResult | 文案 | 失败？ |
|---|---|---|
| `nothing_to_stop` | 计划已关闭；当前没有正在执行的一轮。 | 否 |
| `busy` | 计划已关闭；另一个轮换操作仍在执行，结束后会恢复原账号。 | 否 |
| `stop_requested` / `restoring` | 计划已关闭；已请求停止这一轮，会在当前操作安全收尾后恢复原账号。 | 否 |
| `restored == true` | 计划已关闭，并且已恢复原账号。 | 否 |
| `restored == false` | 计划已关闭，但原账号没能自动恢复；请点「恢复最初账号…」并确认。 | 是 |

- Worker 不可用：`setWorkbuddyRotationEnabled` 返回 false → 回读状态 → 显示失败；开关不会看起来变过。
- 本轮还在收尾时「开启…」禁用，避免和 stop 语义打架。

### 12.2 P0-2 预演不再冒充草稿验证

服务端 `rotation.dry_run` 读 SQLite 里的**已保存计划**，不接受草稿。界面现在：

1. 第三步标题区分「已保存的配置」/「未保存的配置（本地摘要）」。
2. 草稿有改动（账号、顺序、时间、停留、时区任一变化，由 `planKey` 判定）时：
   - 预演按钮禁用；
   - 不展示任何旧的预演结果；
   - 明确写「服务端预演只读已保存的计划，先仅保存，再预演」；
   - 若计划已开启，追加「保存会立即改变正在生效的配置」。
3. 只有 `dryRunPlanKey == 已保存计划.planKey` 才展示结果，并注明「不代表真实切换一定成功」。
4. 保存后 `planKey` 变化，旧结果自动过期；改回原值仍然有效。
5. 不为预演偷偷保存：预演前不调用 `save`。

### 12.3 P1-1 首次开启确认

- 计划本来关闭时，「开启…」与「保存并开启」都先弹确认：
  关闭并重启 WorkBuddy、可能影响正在使用的工作、AICC 未运行/Mac 休眠不能保证执行、恢复失败需人工处理。
- 取消不保存、不启用（测试断言 `calls.isEmpty`）。
- 计划已开启时保存不再弹确认。

### 12.4 P1-2 读取失败保留缓存但不冒充正常

- `AutomationState` 增加 `.stale(previous:reason:)`：显示「上次：…」，摘要追加「当前无法确认」，色调 warning。
- 缓存中的「下次执行」不再显示。
- 无缓存 + 失败仍是「读取失败」；读取中不显示为「未开启」。
- 覆盖签到、成长、旅行、Credits、轮换五类。

### 12.5 P1-3 离开编辑

`RotationLeavePolicy`（关闭 / 切 tab / 取消编辑 × 脏 / 保存中）：

| 情况 | 行为 |
|---|---|
| 无修改 | 直接关闭或切换 |
| 有修改 | 先确认；取消保持原样 |
| 确认关闭 | `closeRequested` → 视图 `dismiss()`，真正关窗 |
| 确认切 tab | 退出编辑并切到目标 tab |
| 保存中 | 忽略一切导航（按钮也禁用） |

绑定的 `set(false)` 不做取消动作，避免 SwiftUI 在按钮动作前清掉待办。

### 12.6 P1-4 恢复异常不再反推计划在跑

`RotationOverview.compactSummary` 按 `plan.enabled` 输出「已开启 · 需要恢复」或「已关闭 · 需要恢复」；
顶部横幅也写「（计划已关闭）」。另补「已关闭 · 正在停止」状态。

### 12.7 P2 时区入口

设置 → 高级与诊断 → 账号轮换时区改为只读：显示服务端值 + 「在任务中心 → 自动计划 → 客户端轮换里修改」。
删除 `SettingsModel.saveRotationTimezone`，确保没有第二个写入口。后端存储与计算未改。
