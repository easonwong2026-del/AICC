# 奖励状态只读层（PR #96，2026-11-16）

#95 把协议与账本钉死了，但界面还拿不到"这个账号现在能领什么、还有几次抽奖、哪天断了"。
这一层就是补这个洞，并且**只读**：抽奖 / 兑换 / 补签的真实 POST 要等 AICC 自己拿到
A 级接口证据（见 `growth-rewards-protocol-2026-11-16.md` §八）。

## 一、新增 RPC

`growth.rewards.status`（`handlers.rs`）—— 参数只有 `accountIds`（可选，空数组=全部）：

```json
{
  "accounts": [{
    "accountId": "…", "nickname": "…",
    "state": "ok | partial | unauthorized | missing_credential | unsupported | failed",
    "streakDays": 3,
    "tiers": [
      {"tier":"7d","days":7,"status":"locked","state":"locked","claimable":false},
      {"tier":"14d","days":14,"status":"available","state":"claimable","claimable":true},
      {"tier":"28d","days":28,"status":null,"state":"unknown","claimable":false}
    ],
    "lotteryChances": 2,
    "makeup": {
      "balance": 2, "max": 4, "today": "2026-11-16",
      "candidates": ["2026-11-15"], "note": "no_broken_day"
    },
    "awaitingVerification": 1,
    "recentOperations": [{"phase":"unknown","outcome":"indeterminate","attempt":1,"open":true}],
    "errors": [],
    "scannedAt": "…", "readOnly": true
  }],
  "queriedAt": "…", "readOnly": true
}
```

用的是和 `growth.scan` 同一套凭据解析（`auth::broker`）、同一条"国际版不支持"跳过规则，
四个 GET 并发拉取：`/streak`、`/redeem/summary`、`/lottery/chances`、`/heatmap`。

两个字段需要特别注意：

| 字段 | 语义 |
| --- | --- |
| `awaitingVerification` | 数字 = 本地账本里等回查的条数；**`null` = 账本读不到**（数据库打不开/查询失败），此时 `errors` 里一定有一条 `field: "ledger"`，`state` 降级为 `partial` |
| `accountIds` 参数 | 缺省 / `null` / `[]` = 全部账号；**类型不对（字符串、数字数组、对象）直接 `-32602`**，不会被静默当成"全部账号" |

## 二、三个必须做对的地方

### 1. 档位状态：三个来源合成，但"空"不等于"可领"

实测见过三种位置：

| 位置 | 形态 | 证据 |
| --- | --- | --- |
| `/streak` | `redemption_status.tier_7d_status` | B（jichie 生产实测） |
| `/redeem/summary` | `tier_7d_status` | A（AICC 自己读过） |
| `/redeem/summary` | `starter_status` / `advanced_status` / `legendary_status` | B（88lin 读法） |

`tier_states()` 按上表顺序取第一个**非空**值。空字符串继续往后找，不是答案。

状态映射只认有证据的写法：

| 原始值 | 界面状态 | 可领 |
| --- | --- | --- |
| `claimed` / `received` / `taken` | `claimed` | 否 |
| `locked` / `not_reached` | `locked` | 否 |
| `ended` / `expired` / `closed` / `finished` | `ended` | 否 |
| `available` / `ready` / `claimable` / `unlocked` / `can_claim` | `claimable` | 是 |
| 其它任何值、空、缺字段 | `unknown` | **否** |

最后一行是关键：服务端哪天加一个新状态，我们宁可不显示"可领"，也不能把不可领的档位
画成可领。88lin 的两个反向事故（"永远不兑换"与"对三档无脑 POST"）都出在这一行。

### 2. 可补签日期：`makeup_dates` 是历史，不是待办

`makeup_candidates(streak, heatmap)` 完全照官方前端 `canMakeup`：

```text
start = max(当月 1 号, launch_date, 2026-06-17)
候选 = cells[] 里  score == 0  且  start <= day < today  且  day ∉ makeup_dates
排序 = 最近的在前
today = /heatmap 的 today.date（服务端日期，不用本机时区）
```

结构异常**整段停止**，每种都返回一个可解释的原因：

| 原因 | 触发 | 用户看到 |
| --- | --- | --- |
| `missing_today` / `bad_today` | 没有或非法 `today.date` | 活跃日历缺少可用的服务端日期 |
| `missing_launch` / `bad_launch` | 没有或非法 `launch_date` | 连登状态缺少活动上线日期 |
| `bad_makeup_dates` | `makeup_dates` 不是数组 | 已补签记录格式异常 |
| `missing_cells` / `bad_cell` | 没有 `cells[]`、分数非法、同一天两个分数 | 活跃日历格式异常 |

日历里**没出现的日期不推测为断签**（缺项 ≠ 零分）；`score` 为负、同一天两个分数都直接停。
余额分三种情况：`no_cards`（**确认** 0 张）、`balance_unknown`（余额没查到）、
`no_broken_day`（本来就没断）。把"没查到"说成"没有卡"就是错误结论 —— 用户抽屉里
可能还有卡（复审 P1）。

### 3. 回查判定：看不出来就不下结论

`growth/reward_ledger.rs` 新增纯函数：

```rust
Observation::Applied | Unchanged | ProvablyNotExecuted | Rejected | Inconclusive
        ↓ verdict()
Verdict::Confirmed | StillUnknown | NotApplied | Rejected
```

| 观察 | 结论 | 账本阶段 | verified |
| --- | --- | --- | --- |
| 目标确实变了（次数少了/档位 claimed/日期进了 makeup_dates） | `Confirmed` | `confirmed` | 是 |
| **前后一致**（次数还是那个次数） | `StillUnknown` | `unknown` | 否 |
| 服务端明确说"没这条操作 / 未受理" | `NotApplied` | `failed` | **是（必须）** |
| 服务端明确拒绝 | `Rejected` | `rejected` | 是 |
| 查询失败/字段缺失/自相矛盾 | `StillUnknown` | `unknown` | 否 |

两条都不能省：

1. **"没变化"不是"没执行"**（复审 P0）。写请求本机超时、随后 GET 到的次数与原值相同，
   完全可能是响应还在路上、读接口走了缓存、或另一个客户端刚补了一次。判成 `failed`
   会释放目标，下一轮生成新的 `client_token` —— 同一次用户意图扣两次。所以
   `Unchanged` 只能 → `StillUnknown`。
2. **"看不出来"更不是"没执行"**。`Inconclusive` 同样只能 → `StillUnknown`。

只有服务端亲口说"没有这条操作"（`ProvablyNotExecuted`）才允许落 `failed`。
`failed` 在存储层还额外要求 `verified == true`，两道门都必须过。

## 三、发送次数上限落到存储层（复审遗留）

之前 `MAX_SEND_ATTEMPTS` 只活在 `plan()` 里，调用方不问 `plan()` 就能无限重发。
现在：

```sql
UPDATE reward_operations
   SET phase = 'sent', attempt = attempt + 1, updated_at = ?
 WHERE id = ? AND phase IN ('pending','ready') AND attempt < 3
```

次数检查与状态切换**在同一条 UPDATE** 里，所以两次并发发送不可能都成功。
被拒时错误文案区分两种情况：「已经发满次数」与「不在待发送状态」。

`growth::reward_ledger::MAX_SEND_ATTEMPTS` 现在直接引用
`storage::reward_ops::MAX_SEND_ATTEMPTS`，两处不会再漂移。

## 四、测试

新增 22 条（单测 20 + RPC 集成 2），全量 845 单测 + 28 集成：

| 组 | 覆盖 |
| --- | --- |
| 档位 | 空值/未知值不默认可领；三来源优先级；空串不挡住后面的值；两份响应都缺时三档仍齐全 |
| 补签候选 | 当月窗口、排除今天、排除上个月、活动起始下限、已补签排除、日历缺项不推测、六种结构异常 |
| 补签视图 | `no_cards`（确认 0 张）/ `balance_unknown`（余额没查到）/ `no_broken_day` / `calendar_missing`；日历坏了仍显示余额 |
| 回查 | `Applied→confirmed`、`Unchanged→unknown`（**不释放**）、`Inconclusive→unknown`、`ProvablyNotExecuted→failed(verified)`、`Rejected→rejected` |
| 存储上限 | 第 1~3 次可发，第 4 次被同一条 UPDATE 拒绝且状态保持 `ready` |
| RPC | 无账号时返回 `accounts: []`；传写形状参数（`draw`/`redeem`）也不产生任何动作 |

## 五、这一层明确不做什么

- **不发任何 POST**：`growth.rewards.status` 只有 4 个 GET；
- **不自动收敛账本**：`awaitingVerification` 只是提示，真正回查写盘留给 #97 的编排；
- **不做界面**：按决定，奖励信息放在账号详情的「奖励」区块，随 #97 一起做；
- **不触发真实抽奖/兑换/补签**，也不消耗任何次数或补签卡。

## 六、下一步（#97）

1. 账号详情的「奖励」区块：三档灯 + 抽奖次数 + 可补签日期 + 执行历史；
2. 手动抽奖 / 兑换 / 补签的 UI 与确认流程（先用 Mock 与只读数据驱动）；
3. 回查编排：把 `verdict()` 接上 `resolve_reward_op`，把 `awaitingVerification` 收敛掉；
4. 真实 POST 的启用条件：AICC 自己抓到一次真实响应（A 级），且用户逐次授权。

## 七、真实数据核验（2026-11-16，A 级）

用本机真实账号库跑了一次 `growth.rewards.status`（**全是 GET**，没有抽奖/兑换/补签）：

| 账号 | streak | 档位 | 抽奖次数 | 补签卡 | 候选 | note |
| --- | --- | --- | --- | --- | --- | --- |
| 卡里 `1d32cc97c9a3ec26` | 16 | 7d claimed / 14d claimed / 28d locked | 1 | 4 | 0 | `no_broken_day` |
| 高亮 `d3de2ea3626c2b44` | 0 | 7d/14d/28d locked | 0 | 0 | 15 | `no_cards` |
| 一锅端 `c4255d2bcfcd10a7` | 0 | 7d/14d/28d locked | 0 | 0 | 15 | `no_cards` |
| SAMSUNG `ab809e1e7cf247c6` | 0 | 7d/14d/28d locked | 0 | 0 | 14 | `no_cards` |

这三条结论因此从 B 升到 **A**：

1. `/streak` 或 `/redeem/summary` 里确实有 `claimed` / `locked` 两种状态，且"卡里"7d/14d 已领、
   28d 未解锁 —— 与连登 16 天一致；
2. 候选日期只覆盖**当月**（2026-11-01 起）且不含今天，说明服务端 `today.date` 与
   `launch_date` 边界确实生效；
3. 三个 `no_cards` 账号的候选日期列表非空而余额为 0 —— 这正是"有断签但没卡"，
   界面要能说清楚，而不是显示"没有可补签日期"。

顺带说明一个副作用：这次 CLI 调用打开了真实账号库，`Db::open` 因此把 schema 从
**v23 迁到 v24**（只建 `reward_operations` 空表 + 索引）。`integrity_check ok`、
`accounts` 仍 5 行、`growth_runs` 29 行未动。这是 main 上已合并的迁移，正式构建下次启动
也会做同一件事；旧构建读到 `user_version=24` 会跳过全部迁移，不会降级失败。

## 八、复审修复（2026-11-17）

| 编号 | 问题 | 修复 | 回归测试 |
| --- | --- | --- | --- |
| P0 | `Observation::NotApplied` 被定义为"操作前后状态一致"并直接落 `failed(verified)`。超时后 GET 没变化不等于服务端没执行（响应延迟、缓存、并发改动都能造成同样的观察），一旦落 `failed` 就释放目标，下一轮生成新 `client_token` | 观察值拆成三个：`Applied` / `Unchanged` / `ProvablyNotExecuted`。`Unchanged` → `StillUnknown`（继续占着），只有 `ProvablyNotExecuted`（服务端明确说没这条操作/未受理）才 → `NotApplied` → `failed(verified)`。`failed` 仍在存储层被 `verified` 二次校验 | `an_unchanged_state_is_still_unknown_not_failed`、`only_provable_non_execution_lands_on_failed` |
| P1 | 补签卡余额 `None` 被 `unwrap_or(0)` 当成 0，界面会提示"没有补签卡" | 拆成 `no_cards`（确认 0 张）与 `balance_unknown`（没查到），`Some(n>0)` 时不给任何理由 | `makeup_state_explains_why_there_is_nothing_to_do` 增加余额未知与有卡两组断言 |

## 九、复审修复（2026-11-17 第二轮）

| 优先级 | 问题 | 修复 | 回归测试 |
| --- | --- | --- | --- |
| P1 | `attach_ledger` 在本地库读取失败时静默保留 `awaitingVerification = 0`；`reward_states` 在账号列表读取失败时返回空数组 | `awaitingVerification` 改为 `Option<usize>`：读不到就是 `null`，同时压一条 `field: "ledger"` 的错误、`state` 降级为 `partial`（已有的 `unauthorized` 不被覆盖）。`reward_states` 返回 `RpcResult`，账号列表读不到时向上抛错，不再伪装成"你没有账号" | `an_unreadable_ledger_is_unknown_not_zero`、`an_unreadable_ledger_degrades_an_otherwise_ok_state`、`an_unreadable_ledger_does_not_mask_a_more_specific_state` |
| P1 | `accountIds` 类型不对被 `and_then(as_array).unwrap_or_default()` 吞成空数组，等于意外扫描全部账号 | 新增 `optional_string_list()`：非数组 → `-32602`，数组里非字符串 → `-32602`，`null`/缺省 → 全部。`growth.scan` 与 `growth.rewards.status` 共用它 | `rewards_status_rejects_malformed_account_ids`、`growth_scan_rejects_malformed_account_ids` |

顺带修正 `growth.scan` 的同一处输入校验 —— 它之前有一模一样的解析写法。
