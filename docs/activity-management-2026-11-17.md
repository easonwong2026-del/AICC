# 四账号成长活跃管理（PR #103，2026-11-17）

在 PR-ACT-1 的取证结论上做的一层只读功能：**每天看一眼四个账号今天亮了没有**，
没亮的去官方客户端做一个真实任务，回来点一下刷新确认。

不做「自动活跃」。理由写在 `docs/activity-rules-2026-11-17.md` §七：
官方条款把脚本 / 接口刷取活跃列为取消资格情形，AICC 不去踩这条线。

## 一、后端：`growth.activity.status`

| | |
| --- | --- |
| 方法 | `growth.activity.status` |
| 参数 | `accountIds?: string[]`（省略 = 所有国内版账号） |
| 请求 | 每账号 **2 个 GET**：`/heatmap`、`/streak`；账号之间串行 |
| 写操作 | 无。响应里固定带 `"readOnly": true` |
| 超时 | 客户端给 60 秒（4 账号 8 个请求） |

响应：

```jsonc
{
  "accounts": [
    {
      "accountId": "…", "nickname": "…", "state": "ok",
      "serverToday": "2026-11-17",
      "todayActive": true, "todayScore": 105, "todayNote": "今日已活跃，继续点亮成长热力墙",
      "streakDays": 17, "nextTier": "28d", "nextTierRemaining": 11, "monthActiveDays": 17,
      "tiers": [{ "tier": "7d", "days": 7, "status": "claimed", "state": "claimed", "claimable": false }],
      "lastActiveDate": "2026-11-17", "activeDays7": 7,
      "updatedAt": "2026-11-17T22:38:04+08:00",
      "checkin": { "status": "already", "checkedAt": "2026-11-17T01:03:01Z",
                   "localDate": "2026-11-17", "source": "auto" },
      "diagnosis": { "code": "active", "label": "今日已活跃", "needsAction": false },
      "errors": [], "fetchedAt": "…"
    }
  ],
  "summary": { "total": 4, "active": 1, "pending": 3, "unknown": 0, "needsAuth": 0 },
  "fetchedAt": "…", "readOnly": true
}
```

### 诊断码（顺序即优先级）

| code | 判定依据 | 界面动作 |
| --- | --- | --- |
| `active` | `today.is_active = true` | 不用管 |
| `unknown` | `/heatmap` 没读到 | 显示"—"，**不催** |
| `checkin_missing` | 服务端今天没有签到记录 | 去补签到（签到页） |
| `stale_stats` | 未活跃，但 `updated_at` 落在服务端今天 | 提示"稍后回查"，**不催** |
| `checkin_only` | 签到在，活跃确实没有 | **去客户端做一个真实任务** |
| `needs_auth` | 凭据缺失 / 401 | 重新授权 |
| `unsupported` | 国际版账号 | 不出现（列表里已过滤） |

`checkin_missing` 与 `checkin_only` 分开，是因为用户要做的动作完全不同：
前者是签到没跑，后者是签到跑了但没用客户端。

## 二、三条实现约束（每条都有测试守着）

1. **签到记录只在等于服务端今天时才算数。** 判据是 `checkin.local_date == heatmap.today.date`，
   不是本机日期。跨日之后昨天的 `already` 不能当今天（`yesterdays_checkin_does_not_count_for_today`）。
2. **`nextTierRemaining` 原样透传。** 实测 SAMSUNG 是 `连登 0 / 7d / 还差 6`；
   本地减法会算出 7（`next_tier_remaining_is_server_truth_not_arithmetic`）。
3. **`todayActive = nil` ≠ 未活跃。** 没读到时 `diagnosis = unknown`、
   `needsAction = false`，界面显示"—"（`a_missing_heatmap_is_unknown_not_inactive`）。

## 三、界面：总览页第二层

`ConsoleActivityBlock` 放在统计卡下面、服务平台上面——这是"每天看一眼"的位置。

| 列 | 内容 |
| --- | --- |
| 账号 | 昵称 + 第二行：`近 7 天活跃 N 天 · 上次 11-13`（延迟时换成"今天有行为未落库，稍后回查"） |
| 状态 | 四字短标签（已活跃 / 未活跃 / 签到未跑 / 待落库 / 需授权 / 未知）；完整句子在 tooltip |
| 今日活跃 | `已活跃 · 105 分` / `未活跃` / `—` |
| 今日签到 | `今日已签（自动）` / `签到失败（手动）` / `今天没有签到记录` |
| 连登 | `连登 17 天` |
| 下一档 | `28 天档 · 还差 11 天`（服务端数字；tooltip 里给三档全貌） |
| 右侧 | 需要处理的账号给「打开客户端」；其余留空 |

页脚三行：待处理账号数 →（若有延迟）延迟账号数 → 怎么做 + 不切换账号的提醒。

### 为什么这一块没有"一键活跃"

- 活跃只能由客户端里的真实使用产生；AICC 能做的只有打开客户端；
- 「打开客户端」是 `NSWorkspace` 打开本机 `WorkBuddy.app`（找不到时按钮禁用并说明），
  **不切换账号**：仓库没有账号切换能力（`docs/UI_CONSOLE_V3.md` §账号切换永久关闭），
  所以界面上明确写"打开后请确认当前登录的是要补的那个账号"；
- 没有自动轮询：一次四个账号 8 个 GET。**打开总览时自动读一次**（每次启动仅一次，
  结果缓存在 store 里），之后只靠「刷新」——这一块的存在意义就是"打开就能看一眼"，
  让用户先点一下再看反而违背目的；但绝不定时轮询。

## 四、顺手修掉的一句旧话

PR #98 的奖励区块在连登为 0 时写的是"签到与活跃是否同一计数，尚无确证"。
现在有确证了，改成：

> 连登 0 天是服务端返回的事实：签到只记账，不点亮热力墙；连登要在 WorkBuddy 客户端里真实使用一次才会增长

对应测试 `testASeroStreakExplainsThatCheckinIsADifferentSignal` 同步改断言（旧断言要求出现"尚无确证"）。

## 五、测试

- Rust `growth::activity`：**12 条**（判定矩阵、跨日、未知、延迟、周窗口、汇总、档位复用）。
- Swift `ActivityModelsTests`：**24 条**（解码、三态文案、签到与活跃不混用、下一档不减法、
  短标签长度、汇总只写非零项、mock 自洽）。
- 真实数据核对：四个账号一次查询得到 `已活跃 1 / 待处理 3`，与取证结论一致。

## 六、视觉回归

`bash scripts/smoke-test-console-visual.sh` → 53 张（原 51 张 + 2 张）。

新增：

- `activity-block-1000x420-light.png`
- `activity-block-1000x420-dark.png`

另外 `console-overview-{960x640,1120x760,1440x900}-{light,dark}` 六张里已经含这一块，
用来确认加了一整块之后总览不溢出、统计卡不被挤变形。

样本 `ActivityFleetState.mock()` 覆盖四种现状：已活跃 / 签到在但未活跃 / 疑似延迟 / 没问到。
值不代表任何真实账号。

## 七、这一层不做什么

- 不发任何 POST，不伪造 `chat_request_send` 之类的事件；
- 不自动补签（有成本且有写操作，走 PR #101 的二次确认链路）；
- 不做"每天自动发一条对话"的伪活跃；
- 不在账号详情里重复一遍活跃信息：详情看奖励，总览看活跃。
