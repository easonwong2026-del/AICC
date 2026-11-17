# 四账号活跃交叉诊断与验收（PR #104，2026-11-17）

本轮的验收证据。命令一条：`bash scripts/check-activity.sh`（只读，每账号两个 GET）。

## 一、实测输出（服务端今天 2026-11-17，22:54 CST）

| 账号 | 今日活跃 | 签到 | 连登 | 下一档 | 结论 | 谁来做 |
| --- | --- | --- | --- | --- | --- | --- |
| 卡里 | 是 · 105 分 | `already` | 17 天 | 28d · 还差 11 天 | `active` | 不用管 |
| SAMSUNG 技术服务 | 否 | `already` | 0 | 7d · 还差 6 天 | `checkin_only` | 用户（客户端做一次真实任务） |
| 高亮 | 否 | `already` | 0 | 7d · 还差 7 天 | `checkin_only` | 用户（客户端做一次真实任务） |
| 一锅端 | 否 | `already` | 0 | 7d · 还差 7 天 | `checkin_only` | 用户（客户端做一次真实任务） |

汇总：总 4 · 已活跃 1 · 待处理 3 · 未知 0 · 需授权 0。

四个账号的签到记录都是当天 `already`，活跃只有 1 个。**签到自动跑通、活跃不会自动发生**，
这就是这一轮要暴露的事实。

## 二、哪些能自动，哪些必须人工

| 环节 | 能自动吗 | 依据 |
| --- | --- | --- |
| 读今日活跃 / 连登 / 下一档 | **能**（只读 GET） | 本轮已实现并实测 |
| 签到 | **能**（已有 #69） | 四账号 11-13 起每天成功 |
| 产生"有效活跃" | **不能** | 官方条款要求客户端内真实对话/任务；脚本刷取属取消资格情形 |
| 回查是否点亮 | **能** | 点「刷新」重新 GET，服务端为准 |
| 补签 | 技术上能，**本轮不做** | 消耗补签卡，属写操作，走 #101 的二次确认链路 |
| 领 7/14/28 档奖励 | 能，但需人工确认 | #101 已接线，二次确认 + 账本 |

结论：**活跃状态可以每天自动呈现，活跃本身只能由人去客户端做一次真实使用。**
AICC 的价值是把"哪个账号需要动手"这件事压缩成一屏。

## 三、六项可靠性验收

| 项 | 怎么验的 | 结果 |
| --- | --- | --- |
| 跨日 | 单测 `a_rollover_to_the_next_day_invalidates_yesterdays_checkin`：同一条签到记录，服务端日期翻到 11-18 | 昨天的 `already` 不再算今天，结论降为 `checkin_missing` |
| 离线 | 单测 `an_offline_read_is_unknown_and_recovers_on_the_next_attempt` | 读不到 → `state=failed` + `unknown`（不催人）；下次读到即恢复 `active` |
| 授权失效 | 单测 `a_401_is_reported_as_an_authorization_problem` | 401 → `needs_auth`，不会伪装成"未活跃" |
| 错峰执行 | 代码约束：worker 的 RPC 在 `main.rs` 的 `select!` 里**串行** `handle_line`，一轮读账号之间也串行 | 同一进程不可能并发打两轮；总览自动读一次 + 手动刷新由 `activityLoading` 挡住重入 |
| 重复操作防护 | 单测 `reading_twice_is_idempotent` + 该 RPC 无任何写操作 | 两次读取结论相同；不写库、不改授权 |
| 活跃未知处理 | 单测 `the_diagnosis_matrix_never_claims_activity_without_evidence`（27 种组合）+ `a_missing_heatmap_is_unknown_not_inactive` | 没有 `is_active=true` 一律不报 `active`；未知不标 `needsAction` |

### 不自行增加连续天数

`streakDays` 只来自 `/streak`；热力图只用来数"最近 7 天活跃几天"。
单测 `streak_is_never_counted_locally_from_the_heatmap`：即使热力图里连续三天有分，
`/streak` 没回来时 `streakDays` 仍是 `None`，界面显示"—"。

## 四、这一轮没有验证的

1. **CodeBuddy IDE / CLI 是否点亮国内站热力墙**——没跑过，不在界面上声称支持。
   验证步骤写在 `docs/activity-rules-2026-11-17.md` §六。
2. **服务端"有效活跃"的判定阈值**（一次对话够不够、不同任务类型是否等价）——
   只能观察到 `score` 的数值差异，看不出规则。
3. **跨设备一致性**：本机只有一个客户端环境，没验证多端同时使用时的计数行为。

## 五、复现

```bash
cargo build --manifest-path account-worker/Cargo.toml
bash scripts/check-activity.sh                # 四账号交叉诊断
bash scripts/collect-activity-evidence.sh     # 原始响应（脱敏）落 docs/evidence/
cargo test --manifest-path account-worker/Cargo.toml growth::activity
bash scripts/smoke-test-console-visual.sh     # 53 张，含 activity-block 两张
```
