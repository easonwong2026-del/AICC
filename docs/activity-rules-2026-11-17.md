# 成长活跃规则取证（PR-ACT-1，2026-11-17）

四个国内版账号的实测取证。目的只有一个：在写"成长活跃管理"之前，先用服务端数据
回答"签到到底算不算活跃"，而不是照直觉做功能。

采集方式：`bash scripts/collect-activity-evidence.sh`（只读，四个 GET，复用
`growth.rewards.evidence`）。原始响应已脱敏落盘在
`docs/evidence/activity-2026-11-17T224040-*.json`。

## 一、结论（先看这四条）

1. **签到不计入活跃，也不点亮热力墙。** 四个账号 11-13 起连续 5 天签到成功
   （本地 `checkin_records` 全是 `already`），但其中三个账号今天的
   `/heatmap.today.score` 仍是 0、`is_active=false`、连登 0 天。
2. **连登天数只由活跃点亮。** 唯一有连登的账号（卡里，17 天）恰好是最近 30 天
   每天都 `score > 0` 的那个；其余三个账号历史活跃天数分别是 1 / 6 / 79 天，
   但都不连续，连登都是 0。
3. **官方口径是"客户端内至少一次有效对话或任务"。** WorkBuddy 官方活动条款原文：
   "用户在一个活动日内，通过 WorkBuddy 或 CodeBuddy 客户端发起至少一次有效 AI
   对话或任务，即视为当日活跃……网页端产生的对话或任务不计入。" 条款同时明确
   脚本、插件、模拟器、接口调用刷取属于取消资格情形。
4. **AICC 不做、也不能做自动活跃。** 唯一可合规做的事：只读判断"今天亮了没有"，
   没亮就提醒用户去官方客户端做一个真实任务，然后回查确认。

## 二、四账号实测（服务端时间 2026-11-17 22:40 CST）

| 账号 | 本地签到（11-13 起） | 今日 `is_active` | 今日 `score` | 连登 `streak.days` | 下一档 | 历史活跃天数 | 最近活跃日 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 卡里 `1d32cc97` | 5 天全成功 | **true** | 105 | **17** | 28d，还差 11 天 | 150 | 11-17（今天） |
| SAMSUNG 技术服务 `ab809e1e` | 5 天全成功 | false | 0 | 0 | 7d，还差 6 天 | 79 | 11-13 |
| 高亮 `d3de2ea3` | 5 天全成功 | false | 0 | 0 | 7d，还差 7 天 | 6 | 10-25 |
| 一锅端 `c4255d2b` | 5 天全成功 | false | 0 | 0 | 7d，还差 7 天 | 1 | 09-17 |

同一天的 `/streak` 回执里，卡里 `tier_7d_status` / `tier_14d_status` 都是 `claimed`，
`tier_28d_status=locked`；其余三个账号三档全 `locked`，`month_total_days` 为 0/0/1。
**签到记录与连登档位完全脱钩**——这是这轮取证最硬的一条证据。

## 三、接口事实（可以直接照着写代码）

### `/v2/activity/growth/heatmap`

| 字段 | 含义 | 注意 |
| --- | --- | --- |
| `data.today.date` | **服务端自己的"今天"** | 界面的"今天"必须用它，不能用本机日期 |
| `data.today.is_active` | 今天是否已点亮 | 唯一权威的活跃判定 |
| `data.today.score` | 今日热度分 | 0 与 `is_active=false` 同时出现 |
| `data.today.status_text` | 给用户看的一句话 | 未活跃时是"开始使用以点亮今日热力墙" |
| `data.cells[]` | 365 天 `{date, score, has_new_buddy}` | 算"最近 N 天活跃几天"用这个 |
| `data.updated_at` | 该账号最后一次产生行为的时间 | 今天更新过但 `is_active=false` → 疑似统计延迟 |
| `data.range` | `{days, start_date, end_date}` | `end_date` 通常等于 `today.date` |

### `/v2/activity/growth/streak`

| 字段 | 含义 |
| --- | --- |
| `data.streak.days` | 当前连登天数 |
| `data.streak.next_tier` | `7d` / `14d` / `28d` / 空 |
| `data.streak.next_tier_remaining` | 距下一档还差几天，**服务端算好的** |
| `data.streak.month_total_days` | 本周期内累计活跃天数 |
| `data.streak.makeup_dates[]` | 已补签的日期 |
| `data.launch_date` | 账号首次进入成长中心的日子 |
| `data.timezone` | `Asia/Shanghai`，服务端日期用它 |
| `data.redemption_status.tier_{7,14,28}d_status` | `locked` / `claimable` / `claimed` / … |
| `data.makeup_cards.{balance,max}` | 补签卡余额与上限 |

## 四、两个"不要自己算"的地方

1. **`next_tier_remaining` 不等于 `next_tier - streak.days`。**
   实测 SAMSUNG 是 `streak.days=0` + `next_tier=7d` + `next_tier_remaining=6`，
   而同状态的 高亮/一锅端 是 7。服务端的计数口径比"连登天数"多一层（当天已计入周期，
   但还没点亮），本地减法会算出 7，与界面上的 6 打架。
   **界面只显示服务端给的数字**，不做减法。
2. **跨日边界以 `today.date` 为准。** 本机 22:40（CST）与服务端一致，但 00:00–08:00
   之间本机日期可能已经翻页而服务端还没结算。判断"今天的签到记录还在不在"
   要用签到记录的 `local_date == today.date`，不能用 `Date()` 减一天。

## 五、四种失败原因怎么区分（能区分的与不能区分的）

| 现象 | 可观测依据 | AICC 的判定 |
| --- | --- | --- |
| 授权缺失 / 失效 | `broker::resolve` 拿不到凭据，或接口 401 | `needs_auth` |
| 自动签到未执行 | 本地 `checkin_records` 里没有 `local_date == 服务端今天` 的成功行 | `checkin_missing` |
| 签到成功但活跃未触发 | 签到行在，但 `is_active=false` 且 `updated_at` 不是今天 | `checkin_only`（**最常见**） |
| 服务端统计延迟 | 签到行在，`is_active=false`，但 `updated_at` 是今天 | `stale_stats`（提示"可能延迟，稍后回查"） |
| 账号身份错误 | **只读接口区分不出来** | 见下 |

身份错误这一条要诚实：`/heatmap` 和 `/streak` 都不回 `uid`，AICC 只能看到
"这份 access 对应的成长档案"。当前能做的只有：授权探测（`auth.probe`）失败即视为
凭据问题；以及"本地签到天天成功、但 `month_total_days` 长期为 0"这种反常组合
提示"凭据可能属于另一个账号"。**不声称能自动判定身份错误。**

## 六、CodeBuddy IDE / CLI 是否计入（未验证）

- 官方条款写的是"WorkBuddy 或 CodeBuddy 客户端"，**没有区分 IDE、CLI 还是桌面端**；
- 第三方逆向资料称桌面端会发一条 `chat_request_send` 事件点亮连登，属于灰色实现，
  AICC **不会**采用；
- AICC 侧**没有验证过** CodeBuddy IDE / CLI 能否点亮国内站热力墙，因此：
  **不在界面上声称支持，也不把它当作自动活跃手段。**

验证方法（需要用户授权并亲自操作，AICC 只做只读观察）：

1. 选一个当前 `is_active=false` 的账号（高亮 / 一锅端 / SAMSUNG 任一）；
2. 在该账号下用 CodeBuddy IDE 或 CLI 发起一次真实的、自己确实需要的任务；
3. 30 分钟后跑 `bash scripts/collect-activity-evidence.sh <accountId>`，只看
   `is_active` 与 `score` 是否从 0 变正；
4. 结论写回本文档；**没跑过就不要写"支持"**。

## 七、合规边界（写进代码注释的那一条）

国际站条款明确把"脚本、外挂、插件、模拟器、接口调用、批量注册刷取活跃或奖励"
列为取消资格情形。国内站条款原文本轮没有取到，但同一产品线的口径不会更宽。

因此 AICC 的实现边界：

- 只读 `GET /heatmap`、`GET /streak`，**不写任何活跃事件**；
- 不构造 `chat_request_send`，不模拟客户端行为，不批量发送对话；
- 用户要活跃，就在官方客户端里做一件真实的事；AICC 负责提醒和回查。

## 八、这一轮不做什么

- 不自动补签（补签卡要花，且属于写操作，已有 PR #101 的二次确认链路）；
- 不做"每天自动发一条对话"这种伪活跃；
- 不在总览页自动轮询：活跃状态按用户点「刷新」查一次（每账号 2 个 GET）。
