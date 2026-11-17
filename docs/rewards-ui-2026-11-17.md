# 账号详情「奖励」区块（PR #98，2026-11-17）

#96 把只读数据带回来了，但界面上看不到。这一层把 `growth.rewards.status` 接到
**账号详情**里，并且明确不提供任何写操作。

## 一、放在哪里

`ConsoleAccountsView.workbuddyDetail` 的「可用能力」与「更多操作」之间，新增一个
`ConsoleDetailGroup(title: "奖励", detail: "只读 · 不会消耗抽奖次数或补签卡")`，
内容由 `ConsoleRewardsBlock` 渲染。国际版账号没有成长中心，这一块直接不出现。

**不自动查询**：进入账号详情不会偷偷打四个接口。用户点「刷新」才查一次，
与「成长中心扫描」保持同一套约定。查过一次后缓存在 `ConsoleStore.rewardStates[accountId]`。

## 二、显示什么

| 区域 | 内容 | 数据来源 |
| --- | --- | --- |
| 头部 | 摘要（连登 / 抽奖次数 / 补签卡 / 可领档位数）+ 状态徽章 + 刷新 | `summaryText`、`stateLabel` |
| 三档 | 7 / 14 / 28 天各一格，显示「已领取 / 可领取 / 未达成 / 已结束 / 状态未知」 | `tiers[].state` |
| 指标 | 连登天数、抽奖次数、补签卡（`2 / 4 张`）、服务端日期 | `streakDays`、`lotteryChances`、`makeup` |
| 可补签 | `可补签 N 天：11-16、11-15 …`，并注明"这些日期由服务端日历算出，点补签前会再查一次" | `makeup.candidates` |
| 提示 | 未核验条数、账本读不到、日历异常原因 | `awaitingVerification`、`makeup.note` |
| 历史 | 最近 5 条本地账本记录（动作 / 目标 / 阶段 / 时间） | `recentOperations` |

三档格子的配色只看 `state`：可领=绿、已领=灰、未达成=更浅的灰、未知=黄、已结束=灰。

## 三、三个按钮现在是禁用的

抽奖 / 兑换连登奖励 / 补签，三个按钮全部 `disabled`，并有一行说明：

> 这三个动作会消耗次数或补签卡，接口证据到位前保持禁用；接好后每一次都会先记本地账本、
> 再回查服务端。

理由：三个写接口只有 B 级证据（第三方实测），AICC 自己没抓到过真实响应。
放一个点了没反应的按钮比不放更糟 —— 用户会以为"点了没生效"，真实原因是"还没接线"。
**#98 不打开任何 POST。**

## 四、三处"不能把未知说成 0"的界面约定

后端已经把它们区分开了，界面必须跟着区分，否则前面白做：

| 情况 | 后端值 | 界面 |
| --- | --- | --- |
| 账本读不到 | `awaitingVerification = null` | 红色提示「本地奖励账本读取失败，无法确认有没有未核验的操作」；**不显示 0 条** |
| 账本里有待核验 | `awaitingVerification = 2` | 黄色提示「有 2 条操作还没核验完，AICC 会先回查再动」 |
| 补签卡余额没查到 | `makeup.balance = null` | `—` + 「补签卡余额没查到，稍后重试」 |
| 补签卡确认 0 张 | `makeup.balance = 0` | `0 张` + 「补签卡已用完」 |

摘要行同理：查不到的项不出现，`summaryText` 只拼有值的部分，全空时显示 `—`。

## 五、Mock 与视觉回归

- `RewardStatusResult.mock(accountId:)`：固定样本（7d 已领 / 14d 可领 / 28d 未达成、
  1 次抽奖、2 张卡、2 天可补签、1 条待核验、2 条历史）。只用于预览、单测与视觉回归，
  值不代表任何真实账号。
- `ConsoleVisualFixture` 新增 `rewardStates` / `rewardFailures`，样本挂在 `acc-wb-1` 上。
- 新增两个离屏渲染场景：
  `accounts-rewards-detail-1120x760-light`、`accounts-rewards-detail-1120x760-dark`。
  这是这个区块唯一可复核的视觉证据（`scripts/smoke-test-console-visual.sh`）。

## 六、测试

`macos/MenuBarApp/Tests/RewardModelsTests.swift`（23 条）：

| 组 | 覆盖 |
| --- | --- |
| 档位 | 三档映射；未知状态 → `状态未知` 且不可领；缺字段不崩 |
| 账本 | `null` → 不显示成 0 且给红字；`0` → 安静；`2` → 一句话 |
| 补签 | 余额未知 ≠ 没有卡；日历异常翻译成人话；候选顺序保留 |
| 历史 | 阶段翻译（`unknown`=待核验、`frozen`=已冻结、`failed`=确认未执行）；`open` 计数 |
| 汇总 | 未知不写 0；有值时的完整摘要；状态标签 |
| 整包 | 按账号取；Mock 自身可被解析；空 payload 不造假行 |
| 刷新失败 | 有缓存 + 失败 → 出提示；成功 → 无提示；无缓存 + 失败 → 不出"上次数据"；读取时间缺失 → 报未知 |

## 七、这一层不做什么

- 不发任何 POST（三个按钮禁用，`ConsoleStore` 里只有 `growth.rewards.status` 一个调用）；
- 不做批量视图：奖励信息只出现在单账号详情里，避免一屏一屏的数字；
- 不做自动轮询：一次查询 4 个 GET，必须由用户触发；
- 不改 `#97` 的 CI 配置，也不碰成长中心页（`ConsoleGrowthSection` 保持原样）。

## 八、下一步（#99 候选）

1. 真实写接口的逐次授权流程（抽奖 / 兑换 / 补签各一次确认，带 `client_token` 展示）；
2. 把 `verdict()` 接上 `resolve_reward_op`，让「待核验」能自动收敛；
3. 三档兑换做成"逐档确认"，补签做成"选日期 + 卡数确认"。

## 九、渲染证据

`scripts/smoke-test-console-visual.sh` 输出 50 张图（原 45 张 + 5 张奖励相关），
归档在 `docs/images/rewards-ui/`：

| 图 | 内容 |
| --- | --- |
| `block-light.png` | 奖励区块单独渲染，浅色 |
| `block-dark.png` | 奖励区块单独渲染，深色 |
| `account-detail-tall-light.png` | 账号详情长图，「奖励」位于「可用能力」与「更多操作」之间 |

复核过的几点：三档格子在深色下边框仍可辨；"待核验"用黄色徽章、"已到账"用绿色，
不把未定状态画成失败；三个禁用按钮在两种外观下都能看出是禁用。

## 十、复审修复：刷新失败不再静默（2026-11-17）

**问题**：`loadReward()` 失败时**不清空**缓存（这是对的），但 `ConsoleRewardsBlock.body`
里 `if let state` 优先于 `failure`，于是第二次刷新失败时界面继续显示旧数据，一个字都不提。

**修法**：

| 变化 | 位置 |
| --- | --- |
| 新增 `RewardRefreshNotice`（模型层，可单测）：只有"有缓存 + 本次失败"同时成立才存在 | `RewardModels.swift` |
| 旧数据顶部加黄色横幅：「刷新失败，以下为上次成功读取的数据」+ 失败原因 + 上次成功读取时间 | `ConsoleRewardsSection.swift` |
| 头部固定显示「上次读取 …」（来自 `scannedAt`），没有失败时也显示，避免把昨天的数据当实时 | 同上 |
| 旧数据 + 失败时状态徽章从「已读取」换成「显示旧数据」（黄色），不与红字自相矛盾 | 同上 |
| 再次刷新成功：`rewardFailures[accountId] = nil`（原有行为），横幅与徽章自动消失 | `ConsoleStore.loadReward` |

新增 4 条单测：有缓存+失败出提示、成功不出提示、无缓存+失败不出"上次数据"、读取时间缺失报未知。
视觉回归新增 `rewards-block-stale-760x560-light`（放在 `docs/images/rewards-ui/block-stale-light.png`）。
