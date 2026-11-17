# 正式奖励写操作（PR #101，2026-11-17）

从探针走到正式功能：三个写接口各自成 RPC，界面按钮可用，但**每一次都要重新查余额 + 用户确认**。

## 一、为什么要和探针分开

| | 取证探针 `growth.rewards.probe` | 正式写操作 |
| --- | --- | --- |
| 构建 | 仅 `cfg(debug_assertions)` | 正式构建可用 |
| 一次性 | 同一目标只能用一次 | 可反复发起（结果由服务端定） |
| 入口 | 一个 RPC + action 枚举 | `draw` / `redeem` / `makeup` 三个方法 |
| 共同点 | 都会：先查余额 → 用户确认 → 写账本 → 只发一次 POST → 结果回查 | 同左 |

探针的一次性守卫（P0-4）保留在探针里，正式路径走 `decide_with(.., one_shot = false)`，
两条路共用同一套账本与发送实现，不存在第二套写入逻辑。

## 二、三个 RPC

| 方法 | 参数 | 说明 |
| --- | --- | --- |
| `growth.rewards.draw` | `accountId`, `confirm` | 抽奖 |
| `growth.rewards.redeem` | `accountId`, `target`(7d/14d/28d), `confirm` | 连登兑换 |
| `growth.rewards.makeup` | `accountId`, `target`(YYYY-MM-DD), `confirm` | 补签 |

`confirm` **不是 true 时只返回预览**：账号、动作、目标、当前余额、预计消耗，`sent=false`。
真的要发时按这个顺序：

```text
resolve 凭据 → readiness(重新 GET 余额/资格) → 不满足就不发
            → open_reward_op(+token 同插入) → mark_sent
            → post_exact(固定前缀, 一次) → classify → resolve(账本)
```

## 三、结果四态

| 账本阶段 | 界面 | 文案基调 |
| --- | --- | --- |
| `confirmed` | `succeeded` | 已到账 |
| `rejected` | `rejected` | 服务端拒绝 / 已经领过 |
| `unknown` | `pending` | 结果未确定，会先回查，**不会重发** |
| `frozen` | `frozen` | 已冻结，需要人工核对 |

`pending` 与 `frozen` 在界面上**不是失败**：一个是等回查，一个是等人。
把它们写成"失败"会诱导用户再点一次。

## 四、界面

- 三个按钮只在真正具备条件时可点（余额 > 0 / 有可领档位 / 有断签日且有卡）；
- 点击弹确认框，写明**账号、目标、当前余额、预计消耗、不可撤销**；
- 确认后按钮进入禁用态（`acting`），结果以一条彩色横幅呈现，可「知道了」关闭；
- 每次结果之后自动重读状态（成功看新余额，未定看账本提示）。

## 五、连登为 0 的解释（验收重点）

实测三个账号服务端返回 `streak.days = 0`，而它们的每日签到是成功的。
两套判定，不能互相证明。界面在连登为 0 时固定显示：

> 服务端返回连登 0 天：成长中心的活跃累计与每日签到是两套判定，签到成功不会让它 +1

这样用户看到的是"解释"，不是"签到失败"。

## 六、这一版没做的事

- 没有对真实账号发出任何写请求：三个 RPC 的 confirm 路径未被真实调用过；
- 未做「批量抽奖」「自动兑换」——每次操作都必须人点；
- 补签一次只补一天（多选留给后续）。

## 七、测试

Rust：`880+` 单测，新增写操作状态映射、文案（未知不叫失败）、成本文案。
Swift：新增写结果四态、`needsAttention`、预览不算已发送、连登解释 6 条。

## 八、安装包与实测步骤（2026-11-17 构建）

产物：`dist/AICC-2.12.2.dmg`（18 MB，ad-hoc 签名，`hdiutil verify` 通过）。
内含的 worker 已确认含有 `growth.rewards.draw`，**不含** `growth.rewards.probe` /
`growth.rewards.evidence` —— 取证 RPC 确实没进正式构建（字符串检查）。

### 实测步骤（不消耗任何奖励）

1. 挂载 dmg，把 AICC 拖到 `~/Applications`（**不要放 `/Applications`**：这台机器上那目录会卡在 dyld）。
2. 启动，进「主控制台 → 账号」，选一个 WorkBuddy 国内版账号。
3. 详情里找到「奖励」，点「刷新」：
   - 应看到三档状态、抽奖次数、补签卡与可补签日期；
   - 连登为 0 的账号应显示"成长中心活跃与每日签到是两套判定"那句解释。
4. 点「抽奖」→ 确认框：应写明账号、当前次数、消耗 1 次、不可撤销。**此时点取消**，验证没有发出任何请求。
5. 若要真实验证写路径（**会消耗「卡里」唯一一次机会**），需另行明确授权；本包构建后未执行。

### 已知限制

- ad-hoc 签名，换构建会丢小组件实例（需重新添加）。
- 本包来自 `feat/rewards-actions`（#101），不是 main。

## 九、复审 P0/P1 修复（2026-11-17 第二轮）

| 编号 | 问题 | 修复 |
| --- | --- | --- |
| P0 | `decide_with(one_shot=false)` 里守卫条件没带 `one_shot`，正式抽奖一次后会被永久 Blocked | 条件补上 `one_shot &&`；回归测试 `the_product_path_may_repeat_a_finished_draw`（正式路径第二次是 Send，探针仍是 Blocked） |
| P0 | `pending` 没有真正的回查闭环 | 新增 `growth.rewards.recheck`：发送前把基线（余额/档位状态）写进账本 `server_state`；回查时 GET 当前状态对比，**只有基线变了**才 `confirmed`，没变保持 `unknown` |
| P1 | 补签日期只校验格式 | `readiness()` 对补签改为同时取 `/streak` + `/heatmap`，用 `makeup_candidates` 校验目标日期**确实在候选里**，不在就不发 |
| P1 | 确认框用旧缓存余额 | **未完成**：RPC 侧每次发送前都会重查（权威判断在服务端），但确认框显示的数字仍来自上一次刷新 |
| — | `cargo fmt --check` 失败 | 已 `cargo fmt`，`--check` 通过 |

回查语义与 `verdict()` 一致：看不动就不关账。测试 `recheck_only_closes_when_the_baseline_moved`。
