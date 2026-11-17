# 奖励接口 A 级取证（PR #100，2026-11-17）

本轮只做两件事：**把四个只读接口的真实响应抓下来**（A 级），以及**实现但暂不执行**
受控写探针。真实抽奖 / 兑换 / 补签的 POST 仍未发生一次。

## 一、证据怎么来的

`scripts/collect-rewards-evidence.sh` → worker 的 dev-only RPC `growth.rewards.evidence`
→ 四个 GET（`/streak`、`/redeem/summary`、`/heatmap`、`/lottery/chances`）
→ 响应在 worker 内脱敏 → 落到 `docs/evidence/`。

采集环境：本机真实账号库（5 个 WorkBuddy CN 账号），客户端版本 `cn`，前缀 `/v2/activity/growth`。

**脱敏规则**（`growth/evidence.rs::sanitize_json`，有单测）：

| 规则 | 处理 |
| --- | --- |
| 键名含 `token`/`secret`/`password`/`cookie`/`authorization`/`bearer`/`session`/`refresh`/`credential`/`private` | 整个键丢掉，值替换为 `<redacted-key>` |
| 键名是 `email` / `phone` / `mobile` / `nickname` | 值打码 `s**m` / `王**名`（两个字全遮 `**`） |
| 任意长字符串 | `rpc::sanitize` 抹掉 ≥32 位令牌串，再截断 160 字符 |
| 数组 / 深度 | 最多 800 项、8 层 |

不含 Cookie、Authorization、Token，也不是 HAR。

## 二、四个接口都成功（A）

| 接口 | 方法 | 状态 | 业务码 | 生效前缀 |
| --- | --- | --- | --- | --- |
| `/streak` | GET | 2xx | 0 | `/v2/activity/growth` |
| `/redeem/summary` | GET | 2xx | 0 | `/v2/activity/growth` |
| `/heatmap` | GET | 2xx | 0 | `/v2/activity/growth` |
| `/lottery/chances` | GET | 2xx | 0 | `/v2/activity/growth` |

成功时 `httpStatus` 记为 `null` 而不是编一个 200：api 层只校验了 2xx，没把具体状态码透出来。

## 三、字段结论（11 项全部命中，等级从 B/C 升到 A）

以「卡里」（`1d32cc97c9a3ec26`，连登 16 天）那次采集为例：

| 想知道 | 真实位置 | 角色 | 类型 | 样例 | 等级 |
| --- | --- | --- | --- | --- | --- |
| 抽奖**余额** | `lotteryChances:data.balance` | balance | number | `1` | **A** |
| 档位配置·积分 | `streak:data.redemption_status.tiers[].credit` | **configuration** | number | `0` | **A** |
| 档位配置·能量 | `streak:data.redemption_status.tiers[].energy` | **configuration** | number | `2` | **A** |
| 补签卡 | `streak:data.makeup_cards.balance` / `.max` | balance | number | `4` / `4` | **A** |
| 7 天档位状态 | `streak:data.redemption_status.tier_7d_status` **与** `redeemSummary:data.starter_status` | status | string | `claimed`（两边一致） | **A** |
| 14 天档位状态 | `tier_14d_status` / `advanced_status` | status | string | `claimed` | **A** |
| 28 天档位状态 | `tier_28d_status` / `legendary_status` | status | string | `locked` | **A** |
| 服务端日期 | `heatmap:data.today.date` | date | string | `2026-11-17` | **A** |
| 活动上线日 | `streak:data.launch_date` | date | string | `2026-06-17` | **A** |
| 热力图格子 | `heatmap:data.cells[]`（365 条，`2025-11-18`→`2026-11-17`） | calendar | array | `{"date":"2026-11-17","score":44,"has_new_buddy":false}` | **A** |

## 四、这轮取证改写了三个假设

### 1. `redemption_status.tiers[]` 是**奖励说明表**，不是"已发放"

```json
{"tier":"7d","days":7,"credit":0,"energy":2,"cards":1,"chances":1}
```

每档描述的是"这一档**发什么**"，不是"已经发了什么"。7 天档 `credit` 为 0、能量 2、
补签卡 1、抽奖 1 —— 与 88lin 里那份奖励文案完全对上（B 级升 A 级）。

**这意味着**：兑换成功后仍然只能靠响应里的 `*_granted` 判断实发数值，不能拿这张表当回执。

### 2. 档位状态是**两套并存且一致**，不是"二选一"

`/streak` 的 `redemption_status.tier_7d_status` 与 `/redeem/summary` 的 `starter_status`
在同一次采集里同时存在、值相同。协议文档 §八 那条"并存性未核对"的悬案到此关闭。

现在的实现"先 streak、后 summary 的 `tier_Nd`、最后 `starter/advanced/legendary`"仍然正确，
但可以再补一条交叉校验：两处不一致时该报警，而不是静默取第一个。

### 3. `/heatmap` 的 `today` 是对象，不是字符串

`{date, is_active, score, ...}` —— 现有代码取 `today.date` 是对的（A 级确认），
但任何直接读 `today` 当字符串的写法都会拿到 object。

另外 `cells[]` 里还有 `has_new_buddy` 字段；`redeem_summary` 里有 `starter_count` /
`advanced_count` / `legendary_count` / `month_total_days` / `remaining_days` / `total_consumed`
——这些我们目前都没用，先记下来，将来做"档位进度条"会需要。

## 五、受控 Probe（已实现，**没有执行**）

`growth.rewards.probe`，只在 `cfg(debug_assertions)` 下注册（release 构建的 match 里没有这条，
`health.methods` 里也不会出现）。

安全设计逐条对应：

| 要求 | 实现 |
| --- | --- |
| 必须 `confirm=true` | 缺省或 false 直接 `-32602`，文案写明"会消耗一次真实机会" |
| action 严格枚举 | `ProbeAction::parse` 只认 `lottery_draw` / `redeem` / `makeup_card` |
| 一次只一个动作 | 一个 RPC = 一个 Command = 一个账本目标 |
| 先 `open_reward_op` 再 `mark_sent` 再发 | `decide()` → `mark_reward_op_sent()` → `api::post()`，顺序写死 |
| 超时 / 5xx / 未知 → unknown | `status_for_error()` 把传输失败映射成 0 → `classify` → `Indeterminate` → `unknown` |
| 禁止自动重发 | 同一目标第二次调用只会得到 `Recheck`（有测试） |
| 响应先脱敏再落盘 | 复用 `sanitize_json` |
| 不能绕开账本 / 凭据门禁 | handler 里先 `credentials::gate::begin(app)`；状态变更全部走 `reward_ops` |
| 幂等键只钉一次 | `set_reward_op_token` 只在 `client_token IS NULL` 时成功（有测试） |

## 六、测试

新增 16 条（Rust）：

| 组 | 覆盖 |
| --- | --- |
| 脱敏 | 凭据键丢弃；邮箱/昵称打码；长令牌串抹除+截断；数组 800 上限与深度上限；finding 的 sample 也脱敏 |
| 字段定位 | 嵌套路径与 `[]` 数组路径；找不到就标 `missing` 且不升级等级 |
| 探针动作 | 三个枚举值；`redeem` 必须带 `7d/14d/28d`；`makeup_card` 必须带日期 |
| 探针秩序 | 第一次 `Send` 且 token 已钉入账本；第二次 `Recheck`；超时后 `Recheck`；并发只有一个线程能把发送记进账本 |
| 幂等键 | 只能钉一次 |

全量：**877 单测 + 30 集成**。

## 七、还没验证的（仍是 C 级）

1. 抽奖成功响应的字段（积分/能量/奖品名/是否需要地址）；
2. 兑换成功响应是否带 `*_granted`；
3. 补签成功响应与卡数变化；
4. 三个写接口的 403/400 文案是否与第三方记录一致。

这四项都只能靠一次真实写请求拿到 —— 等你指定账号与逐次授权。

## 八、下一步

1. 你指定一个 WorkBuddy CN 账号，并明确"允许消耗一次抽奖机会"；
2. 我用 `growth.rewards.probe` 跑一次 `lottery_draw`，把脱敏响应归档；
3. 按响应把 `GRANT_KEYS` 补齐，再接正式 UI 按钮；
4. 兑换、补签各自独立授权一次。

## 九、复审 P0 修复（2026-11-17 第二轮）

### P0-1 取证改为**权威路径**取值

原来按字段名全局搜索，所以先命中了 `streak:...tiers[].chances`（档位奖励配置），
把**余额 0 次**的账号显示成"还有 1 次"。现在每条 finding 声明 `source = 接口:路径`，
只按这条链取值（`lookup_path`，`[]` 才穿透数组），并新增 `role` 区分语义：

| role | 含义 | 例子 |
| --- | --- | --- |
| `balance` | 账号当前余额 | `lotteryChances:data.balance` |
| `configuration` | 档位奖励配置（每档发什么） | `streak:data.redemption_status.tiers[].chances` |
| `status` / `date` / `calendar` | 状态、日期、日历 | `...tier_7d_status`、`data.today.date`、`data.cells[]` |

**真实数据验证**：`d3de2ea3626c2b44` 现在 `balance=0`（配置仍为 1），不再误判。
回归测试：`a_tier_configuration_value_never_counts_as_the_balance`、
`a_finding_only_looks_at_its_authoritative_path`、`a_missing_path_does_not_borrow_another_endpoints_value`。

### P0-2 探针只发一次 POST

新增 `api::post_exact()`：固定 `VERIFIED_PREFIX = /v2/activity/growth`，走 `send_once`，
**没有 `for prefix in PREFIXES` 的 404 退让**。探针只调它。
源码守门测试 `the_probe_post_never_falls_back_to_the_other_prefix` 盯住"别改回 `post()`"。

### P0-3 幂等键原子化

- token 在 `open_reward_op` **之前**生成、随 INSERT 一起写入 —— 不存在"行已落盘、token 还没写"的窗口；
- 删除了 `set_reward_op_token`（分两步提交正是并发空窗的来源）；
- `mark_reward_op_sent` 增加 `AND (action='makeup_card' OR client_token 非空)`：缺 token 的抽奖/兑换**在存储层就发不出去**。
- 测试：`a_tokenless_draw_cannot_be_marked_as_sent`、`a_makeup_operation_may_send_without_a_token`、
  并发测试 `concurrent_probes_cannot_both_mark_the_send`。

### P0-4 探针一次性

`decide()` 先查这个目标**有没有已结束的记录**：有就 `Blocked`，绝不新建。
仍开着的记录继续走同一个 attempt（回查/重发），那是同一次操作。
测试：`a_completed_probe_blocks_a_second_one`（confirmed 后再调必须 blocked，且不新建记录）、
`the_one_shot_guard_is_scoped_to_its_own_target`。正式 UI 将来的多次抽奖不走这条路径。

### 附加保护

| 保护 | 实现 |
| --- | --- |
| 发送前重新 GET 余额 | `preview()` / `execute()` 先跑 `readiness()`；`judge_lottery` / `judge_tier` / `judge_makeup` 三个纯函数决定"发不发" |
| 余额 0 不发送 | `judge_*(Some(0))` 与 `judge_*(None)` 都是 `ready=false` |
| 展示再授权 | `confirm` 不为 true 时只返回预览：账号、动作、余额、预计消耗，`sent=false` |
| 取证 RPC 只在 dev | `the_dev_only_rpcs_stay_behind_debug_assertions` 检查两条 RPC 前必须有 `#[cfg(debug_assertions)]` |
| 证据文件隐私扫描 | `committed_evidence_files_carry_no_credentials` 扫 `docs/evidence/*.json` |
