# 四账号活跃交叉诊断与验收（PR #104，2026-11-17；2026-11-18 修订）

本轮的验收证据。命令一条：`bash scripts/check-activity.sh`
（只读：每账号 `/heatmap` + `/streak`，本地拿不出成功签到记录时再读一次签到状态）。

## 一、实测输出（服务端今天 2026-11-18，08:19 CST）

| 账号 | 今日活跃 | 今日签到 | 连登 | 下一档 | 结论 | 谁来做 |
| --- | --- | --- | --- | --- | --- | --- |
| 卡里 | 是 · 8 分 | 已签（服务端） | 18 天 | 28d · 还差 10 天 | `active` | 不用管 |
| SAMSUNG 技术服务 | 否 | 已签（服务端） | 0 | 7d · 还差 6 天 | `checkin_only` | 用户（客户端做一次真实任务） |
| 高亮 | 否 | 已签（服务端） | 0 | 7d · 还差 7 天 | `checkin_only` | 用户（客户端做一次真实任务） |
| 一锅端 | 否 | 已签（服务端） | 0 | 7d · 还差 7 天 | `checkin_only` | 用户（客户端做一次真实任务） |

汇总：总 4 · 已活跃 1 · 待处理 3 · 未知 0 · 需授权 0。

### 这轮跑出来的一个真 bug

四个账号在 11-18 凌晨**都没有 AICC 本地签到记录**（本地最新是 11-17），
但服务端签到接口的 `checkin_dates[0]` 是 `2026-11-18`。

- 旧实现：本地没记录 → 直接判 `checkin_missing`（"签到没跑"）——**误判**；
- 新实现：本地拿不出成功记录时多读一次签到状态接口，`checkin_dates` 含服务端今天
  → `confirmed(server)` → 结论降为 `checkin_only`（签到确实做了，只是今天还没活跃）。

这条正是"签到状态误判"要修的核心：**AICC 没有记录 ≠ 没签到**。

## 二、哪些能自动，哪些必须人工

| 环节 | 能自动吗 | 依据 |
| --- | --- | --- |
| 读今日活跃 / 连登 / 下一档 | **能**（只读 GET） | 本轮实现并实测 |
| 签到 | **能**（#69 已有） | 四账号 11-13 起每天成功 |
| 判定今日签到（含"别处签过"） | **能**（只读签到状态接口） | 本轮修的就是这条 |
| 产生"有效活跃" | **不能** | 要客户端真实使用；脚本刷属取消资格情形 |
| 回查是否点亮 | **能** | 点「刷新」重新 GET，服务端为准 |
| 补签 | 技术上能，本轮不做 | 消耗补签卡且是写操作，走 #101 二次确认 |
| 领 7/14/28 档奖励 | 能，但需人工确认 | #101 已接线，二次确认 + 账本 |

结论：**状态可以每天自动呈现，活跃本身只能由人去客户端做一次真实使用。**
AICC 的价值是把"哪个账号需要动手、动手前该先做什么"压缩成一屏。

## 三、十项可靠性验收

| # | 项 | 怎么验的 | 结果 |
| --- | --- | --- | --- |
| 1 | 无本地签到记录、服务端已签到 | 实测四账号 + 单测 `a_server_confirmed_checkin_without_a_local_record_is_not_a_missing_checkin` | `confirmed(server)` → `checkin_only`，不再误报 |
| 2 | 本地 `failed/unknown/disabled` 不误报成功 | 单测 `no_non_success_status_is_ever_treated_as_checked_in`（6 种状态） | 一律不进 `confirmed` |
| 3 | 服务端跨日、旧日期记录失效 | 单测 `a_rollover_invalidates_the_previous_days_checkin` | 昨天的 `already` 不挂到今天 |
| 4 | `updated_at` 当天变化但无活跃证据 | 单测 `a_fresh_updated_at_alone_proves_nothing` | 结论仍是 `checkin_only`，不算延迟 |
| 5 | 热力图已点亮、连登未更新 | 单测 `a_lit_heatmap_without_streak_data_does_not_invent_streak` | `active` + 连登 `—`，不推算 |
| 6 | 无授权 / 401 / 网络失败 | 单测 `a_401_is_reported_as_an_authorization_problem`、`a_network_failure_is_unknown_not_inactive` | `needs_auth` / `unknown`，都不算"未活跃" |
| 7 | 多账号按 `accountId` 对应 | 单测 `multiple_accounts_keep_their_own_identity` + RPC 集成测试 | 各自归属，汇总计数正确 |
| 8 | 连登与剩余天数只取服务端 | 单测 `next_tier_remaining_is_server_truth_not_arithmetic`（0 天 / 还差 6 天） | 不做减法 |
| 9 | 连续刷新没有任何写操作 | RPC 集成测试 `activity_status_is_registered_and_read_only`（连调两次）+ 源码守卫断言无 `growth.activity.write` | 只读，无写接口 |
| 11 | 本地 `claimed` / `already` 都必须算已签 | 单测 `both_stored_success_states_confirm_without_asking_the_server` + `claimed_and_already_are_the_only_success_states` | 两种状态都 `confirmed(local)`，且不依赖探针结论 |
| 12 | 状态查询不许回退到上游探针 | 单测 `the_activity_module_uses_the_controlled_status_query`（源码守卫）+ `checkin::api` 的 5 条请求纪律用例 | 只用 `query_status_tolerant`；401/403/429/5xx/超时都只发一次 |
| 10 | 正式构建不含调试探针 | 单测 `dev_only_probes_stay_behind_debug_assertions`（读 `handlers.rs` 断言 `#[cfg(debug_assertions)]`） | 探针只在 debug 注册 |

另有 `the_diagnosis_matrix_never_claims_activity_without_evidence`：45 种
（活跃三态 × 分数三档 × 签到五态）组合里，没有 `is_active = true` 一律不报 `active`。

### 复审 P0 的两条修复（本轮）

1. **`claimed` 漏判**：本地成功状态只认存储层常量 `claimed` / `already`；
   本地成功时**零额外请求**（`a_claimed_local_record_needs_no_server_verdict`）。
2. **不受控的状态查询**：换掉上游 `get_checkin_status`（非成功即换路、401/429/5xx
   都重问），改用 `checkin::api::query_status_tolerant`：只读语义 `POST {}`、
   仅 404 fallback 一次、不刷令牌、不发 `daily-checkin`。`checkin::api` 新增 5 条
   带 Mock 发送层的用例，断言"成功只发 1 次、404 才发 2 次、其它错误只发 1 次"。
3. **信号冲突不选边**：`checkin_dates` 与显式布尔冲突 → `Indeterminate` → 未确认。

## 四、这一轮没有验证的

1. **CodeBuddy IDE / CLI 是否点亮国内站热力墙**：本阶段已取消该项研究（架构决策见
   `docs/activity-rules-2026-11-17.md` §六），不在界面上声称支持。
2. **服务端"有效活跃"的判定阈值**（一次对话够不够、任务类型是否等价）：只能观察到
   `score` 数值差异，看不出规则。
3. **多设备同时使用**：本机只有一个客户端环境，没验证多端计数行为。
4. **签到状态接口的日期口径**：靠 `checkin_dates` 对齐，未取得接口文档；若将来字段
   改名，`confirmed(server)` 会退回 `unconfirmed`（不会误报成功）。

## 五、复现

```bash
cargo build --manifest-path account-worker/Cargo.toml
bash scripts/check-activity.sh                # 四账号交叉诊断
bash scripts/collect-activity-evidence.sh     # 判定所需字段摘要落 docs/evidence/
cargo test --manifest-path account-worker/Cargo.toml growth::activity   # 35 条
bash scripts/smoke-test-console-visual.sh     # 53 张，含 activity-block 两张
```
