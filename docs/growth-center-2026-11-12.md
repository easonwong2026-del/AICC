# 成长中心实测记录（2026-11-12）

PR #67 的只读联调结果。**所有数据来自 4 个真实账号**，不是文档推测。
下一次动这块代码前先读这一页，能省掉一轮"字段名猜错"的返工。

## 方法与范围

```
dist/mac/AICC.app/Contents/Helpers/aicc-account-worker
  → {"method":"growth.scan","params":{}}
```

只发 GET：`/tasks` `/profile` `/energy` `/streak` `/heatmap`
`/redeem/summary` `/lottery/chances` `/buddy/quota` `/buddy/travel/status`。
没有 accept / claim / use / draw / open 任何一种写请求。

账号均为 Magpie 借用授权（`auth_source = magpie`），账号库 schema v7。

## 一、路径形态（已确认）

| 项 | 结论 |
|---|---|
| 域名 | `https://www.codebuddy.cn` |
| **生效路径** | **`/v2/activity/growth/...`** |
| 备用路径 | `/activity/growth/...` —— 本域名下不走，仅作 404 兜底保留 |

社区资料里两种形态都存在，本域名实测是**带 v2** 的那种。兜底只在
路由不存在（404）时触发；401 / 403 / 429 / 5xx 一律不退，那些是服务端的真实回答。

## 二、各接口的真实字段

### `GET /profile` —— 成长档案，**没有积分**

```json
{"completed": 15, "first_visit": false, "level": 15,
 "level_icon": "https://.../badge-05.png", "level_name": "",
 "max_level": false, "total": 19}
```

⚠️ 这里最容易出错：成长中心页上那个数字是**等级**，不是积分。
`credit` 在 #67 阶段恒为空，界面显示「—」；积分只能等 #68 拿领奖回执累计。

### `GET /energy`

```json
{"balance": 3, "total_consumed": 130, "total_earned": 133}
```

### `GET /streak`

```json
{"launch_date": "2026-06-17",
 "makeup_cards": {"balance": 4, "max": 4},
 "streak": {"days": 12, "makeup_dates": [], "month_consumed_days": 0,
            "month_total_days": 12, "next_tier": "14d", "next_tier_remaining": 2},
 "redemption_status": {"remaining_days": 12, "tier_7d_count": 1,
                       "tier_7d_status": "claimed", "tier_14d_status": "locked",
                       "tier_28d_status": "locked", "tiers": [...]},
 "timezone": "Asia/Shanghai"}
```

`makeup_dates` 是**已补登**记录，不是待补日期 —— 这是 `makeup-compatibility.md`
反复强调的那一点。

### `GET /redeem/summary`

```json
{"starter_status": "claimed", "advanced_status": "locked", "legendary_status": "locked",
 "starter_count": 1, "advanced_count": 0, "legendary_count": 0,
 "month_total_days": 12, "remaining_days": 12, "total_consumed": 0}
```

`starter / advanced / legendary` 对应 7 / 14 / 28 天。

### `GET /heatmap`

`{"today": {"date": "2026-11-12", "is_active": true, "score": 40, "status_text": "..."}, "cells": [365 项], "range": ..., "updated_at": ...}`

### `GET /lottery/chances`

```json
{"balance": 0}
```

### `GET /buddy/quota`

```json
{"affordable": 0, "balance": 3, "cost_per_open": 10, "max_open_count": 5}
```

`balance` 与 `/energy` 的 `balance` 是**同一个计数器**（已核对，可互为兜底）。
`affordable` 是"按当前能量能开几个"，不是"有几次机会"。

### `GET /buddy/travel/status`

```json
{"state": "idle", "record_id": 0, "arrive_at": 0, "daily_limit_reached": true,
 "location": null, "buddy_id": ..., "depart_at": ..., "duration_hours": ...,
 "reward_credit": ..., "letter": ..., "server_now": ..., "use_deeplink": ...}
```

⚠️ 空闲时 `record_id` 与 `arrive_at` 都是 **0**，`location` 是 null。
直接透传会让界面出现「0 号行程」。扫描器现在把 0 归一化成"没有"。

### `GET /tasks` 的任务项

```json
{"task_code": "expert_use_douyinyin", "title": "体验「抖音策略师」",
 "task_desc": "在专家页搜索并召唤「抖音策略师」，完成一次对话。",
 "description": "", "accept_status": "not_accepted", "locked": false,
 "progress": null, "reward_credit": 50, "reward_energy": 5,
 "has_reward": true, "reward_buddy": false, "is_new": false, "is_pinned": false,
 "jump_url": "https://www.workbuddy.cn/work/launch/?deeplink=...",
 "task_type": "single", "valid_start": null, "valid_end": "2026-11-13T23:59:00+08:00",
 "icon_url": "https://...", "badge_name": "", "claimed_button_style": "gray",
 "template_id_fixed": 0}
```

要点：

- **没有 `status` 字段**，只有 `accept_status`。
- `progress` 大量为 `null` —— 多数任务是"做没做"，没有进度条。有进度的（如
  `Expert_team_use_3`）给 `{"current": 1, "target": 3}`。
- `task_type` 目前只见到 `single`。

## 三、四账号实况

| 账号 | 等级 | 完成 | 能量 | 连签 | 补签卡 | 抽奖 | 旅行 |
|---|---|---|---|---|---|---|---|
| 卡里 | Lv.15 | 15/19 | 3 | 12 天 | 4 | 0 | idle |
| 一锅端 | Lv.5 | 5/19 | 18 | 0 | 0 | 0 | idle |
| 高亮 | Lv.11 | 11/19 | 8 | 0 | 0 | 0 | traveling（record 54931469） |
| SAMSUNG 技术服务 | Lv.10 | 10/19 | 5 | 0 | 0 | 0 | traveling（record 54931459） |

总览：21 项可接受 · 2 项待领奖 · 8 项需人工 · **0 项待识别** · 34 能量。

各账号状态分布：

| 账号 | claimed | accepted | in_progress | not_accepted | completed |
|---|---|---|---|---|---|
| 卡里 | 15 | 0 | 1 | 3 | 0 |
| 一锅端 | 5 | 5 | 2 | 7 | 0 |
| 高亮 | 10 | 3 | 1 | 4 | 1 |
| SAMSUNG | 9 | 1 | 1 | 7 | 1 |

## 四、任务目录（19 项，卡里视角）

| task_code | 标题 | 完成条件 | 状态 | 奖励 | 分类 |
|---|---|---|---|---|---|
| `playbook_prompt` | 探索优秀灵感 | 体验1款「灵感」优秀案例。 | claimed | +100 | realAction |
| `RichMeow_Chat` | 桌面端对话1次 | 下载并使用桌面端，即可解锁限定款 Buddy 盲盒 | claimed | +100 | realAction |
| `Library_read` | 体验「资料库」 | 前往「资料库」，读完《WorkBuddy 资料库介绍》 | claimed | +100 | realAction |
| `Expert_lighthouse` | 体验「腾讯轻量云」专家 | 召唤专家，在对话框上方**授权连接器**后对话一次 | claimed | +100 | **manual** |
| `Buddy_App` | 体验 WorkBuddy「发现应用」 | 升级 5.5.3+，点左上角「发现应用」进任意 Buddy 应用 | claimed | +100 | realAction |
| `wb_wechat_oa_subscribe_task` | 关注「腾讯 WorkBuddy」官方公众号 | 关注满 24 小时 | claimed | +100 | **manual** |
| `WorkBuddy_Valorant` | 体验「无畏契约拼豆专家」 | 搜索并召唤，完成一次对话 | claimed | +100 | realAction |
| `expert_use_maidehao` | 体验「中国电商运营专家」 | 同上 | claimed | +50 | realAction |
| `expert_use_douyinyin` | 体验「抖音策略师」 | 同上 | not_accepted | +50 | realAction |
| `expert_use_shangjia_xuanchuan` | 体验「商家宣传页与海报设计师」 | 同上 | not_accepted | +50 | realAction |
| `expert_use_bowangwang` | 体验「直播电商教练」 | 同上 | not_accepted | +50 | realAction |
| `skill_share` | 分享一个技能给好友 | 技能详情页「分享」发给好友 | claimed | +100 | realAction |
| `Expert_team_use_3` | 召唤 3 次专家团 | 召唤并使用 3 个不同的专家团队 | in_progress（1/3） | +100 | realAction |
| `first_buddy` | 领取一只 Buddy | 新建 1 个任务并发起对话 | claimed | +300 | realAction |
| `chat_5` | 和 AI 聊天 5 次 | 累计对话 5 次 | claimed | +100 | realAction |
| `skill_1` | 尝鲜热门技能 | 安装并使用 1 个技能 | claimed | +100 | realAction |
| `expert_5` | 召唤 5 次专家 | 召唤 5 位不同专家并对话 | claimed | +100 | realAction |
| `template_5` | 使用 5 个模板 | 新建任务并使用 5 个不同模板 | claimed | +100 | realAction |
| `automation_1` | 设置自动化任务 | 设置 1 个自动化任务 | claimed | +100 | realAction |

两处 **manual 判对了**：`Expert_lighthouse` 的条件里有"授权连接器"，
`wb_wechat_oa_subscribe_task` 是关注公众号。这两类都不该由 AICC 代做。

## 五、对 #68 / #69 的输入

1. **可自动化的任务数为 0**。19 个任务里没有一个属于"系统管理型"——
   接受任务和领奖是动作，不是任务本身。所以 #68 的价值全在
   「接受未接受 + 领取已完成」，而 #69 的真实操作适配器一个都跑不掉。
2. **真实操作任务高度集中在专家/技能/模板/自动化**：`expert_use_*`（4 个）、
   `expert_5`、`Expert_team_use_3`、`skill_1`、`skill_share`、`template_5`、
   `automation_1`、`chat_5`、`RichMeow_Chat`、`Library_read`、`playbook_prompt`、
   `Buddy_App`。要自动化就得对着这些做适配器，而不是虚构事件上报。
3. **`first_buddy` 奖励 300 积分**，是单项最高；`valid_end` 有到期时间，
   过期任务会从列表消失（本次 4 个账号都是 19 项，暂无缺失）。
4. **2 个待领奖样本**（`Expert_lighthouse` +100、`Expert_team_use_3` +100）
   可以直接用来验证 #68 的领奖幂等：领完再扫，`accept_status` 应该变 `claimed`。
5. `daily_limit_reached: true` 且 `state: idle` 说明当天已派过 —— 开盲盒前必须看这个，
   避免在"旅行中"或"今天已用完"时开盒子（#69 的互斥条件）。
