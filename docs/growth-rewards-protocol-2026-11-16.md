# 成长中心奖励协议差异报告（PR #95，2026-11-16）

三个写动作 —— 连登奖励兑换（`redeem`）、活动抽奖（`lottery/draw`）、补签卡（`makeup-cards/use`）
—— 都会**消耗账号的一次机会或一张卡**。这份文档只回答一件事：**哪些事实有证据、哪些还是推断**。

PR #95 不接线、不发这三个请求。它只做三件事：把协议事实钉死（本文档）、修掉奖励模型里
两处会误判的设计（`growth/streak_rewards.rs`）、给这三个动作建本地操作账本（`storage/reward_ops.rs`）。

## 一、证据等级

四类证据**不能互相替代**，后面每一条结论都标了等级。

| 等级 | 含义 | 能不能作为接线依据 |
| --- | --- | --- |
| **A** | AICC 自己用真实账号实测（含具体响应） | 能 |
| **B** | 第三方项目**声称实测**，并给出可核对的服务端原话 | 能作参考，接线前补 A |
| **C** | 从第三方源码推断（常量、字段名），没有服务端原话 | 不能 |
| **D** | 纯推测 / 从页面按钮猜的 | 不能 |

本机**没有**为 PR #95 制造任何真实写操作：不消耗抽奖次数、不动补签卡、不兑换档位。
本阶段所有结论来自 AICC 已有实测记录 + 第三方源码，`#96` 接线前必须补一次 A 级证据。

## 二、三个动作的协议对照

### 2.1 连登奖励兑换 `POST /redeem`

| 项 | 88lin `signin.py` | jichie `streak.mjs` | wb-switch | AICC 现状 | 结论 |
| --- | --- | --- | --- | --- | --- |
| 路径 | `/v2/activity/growth/redeem` | `{base}/activity/growth/redeem` | **未实现** | `{base}/activity/growth/redeem`（自行拼） | 前缀用 AICC 已实测的 `/v2/activity/growth`（A） |
| body | `{"tier":"7d","client_token":"u-<uuid>"}` | `{"tier","client_token": randomUUID()}` | — | `{"tier":"7d","client_token"}` | 一致（B） |
| `tier` 取值 | **实测**：`"starter"`/`"7"`/`7` 分别得到 `unknown tier`/`unknown tier`/`invalid request`，只有 `"7d"` 会 200 | `["7d","14d","28d"]` | — | 同 | `"7d"/"14d"/"28d"`（B，两项目一致） |
| 档位状态字段 | `/redeem/summary` 的 `starter_status`/`advanced_status`/`legendary_status` | `/streak` 的 `redemption_status.tier_7d_status`（**实测**；`tiers[]` 数组与 `{7d:...}` 都是猜的，拿不到值） | — | `/redeem/summary` 的 `tier_{7,14,28}d_status` | **两种都要认**（AICC 实测过 summary 平铺形态；jichie 实测 streak 形态） |
| 成功到账字段 | `credit_granted`/`energy_granted`/`cards_granted`/`chances_granted` | 只读 `data.credit` | — | 读 `credits/credit/reward_credit` | **`*_granted` 优先**（B）；88lin 明确写"直接读 `credit` 恒为空" |
| 未解锁档位 | **403 + 文案含"天数不足"**，且必须**先于** 401/403 鉴权判定处理 | status 为 `locked` 就不发请求 | — | 403 一律判 `Unauthorized` | 403 是**业务拒绝**，不能当授权失效（B，AICC 现状是错的） |
| 已兑换 | `status in (claimed, locked)` 跳过 | 同 | — | 无 | 本地不再发起（B） |
| 参数级拒绝 | 400 + 文案含 `tier` 且含 unknown/unsupported/invalid → **参数校验阶段**拒绝，服务端没兑换任何东西，可退回天数重试一次 | 无 | — | 无 | 单独一类 `InvalidRequest`，只有它允许"换写法重试"（B） |

88lin 源码里那条历史教训值得抄下来（`signin.py:1517`）：
> 2026-09-11 曾据"传天数返回 invalid request"误判 tier 要收数字，导致兑换模块连坏三个版本
> —— "业务拒绝"的错误文案不能反推参数格式正确，必须拿到一次成功响应才算验证过。

兑换奖励官方文案（2026-09 活动版本，仅用于展示兜底，`signin.py:1678`）：
`7d` = +2 能量 +1 补登卡 +1 次抽奖；`14d` = +50 积分 +3 能量 +1 补登卡 +1 次抽奖；
`28d` = +150 积分 +5 能量 +1 补登卡 +1 次抽奖。**到账以响应里的 `*_granted` 为准**。

### 2.2 活动抽奖 `POST /lottery/draw`

| 项 | 88lin | jichie | AICC 现状 | 结论 |
| --- | --- | --- | --- | --- |
| 次数查询 | `GET /lottery/chances` → `data.balance`（**A 级：AICC 也实测过这个已存在**） | `GET /lottery/summary` → `data.chances` | `GET /lottery/chances` | 用 `chances`；`summary` 作为未验证的备选，**不接线** |
| 路径 | `/v2/activity/growth/lottery/draw` | `{base}/activity/growth/lottery/draw` | 同 | 一致（B） |
| body | `{"client_token":"u-<uuid>"}` | `{"client_token": randomUUID()}` | 同 | 一致（B） |
| 缺 `client_token` | **400（实测过）** | — | — | 必带（B） |
| 无次数 | 400 + `insufficient lottery chance balance` —— 常态，不是故障 | `chances=0` 不发请求 | 归到 `NotEligible`（尚未区分） | 单独一类 `NoChance`（B） |
| 中奖字段 | `prize_name` / `prize`；实物奖另有 `need_address`/`require_address` | `data.name` / `data.prize` + `data.credit` | 无 | 四个键都读，实物奖要提示填地址（B） |
| 失败后剩余次数 | 失败即**停手**，不硬闯完 | 同（`break`） | — | 一次抽奖一个操作，失败即停（B） |

抽奖响应里**没有** `*_granted` 系列字段的证据：88lin 读的是 `prize_name`/`prize`，
jichie 读 `data.name`/`data.prize`/`data.credit`。两个项目都只拿了奖品名，
所以 `#96` 接线必须先拿一次真实抽奖响应（A 级），再决定积分/能量从哪读。

### 2.3 补签卡 `POST /makeup-cards/use`

| 项 | 88lin | jichie | AICC 现状 | 结论 |
| --- | --- | --- | --- | --- |
| 路径 | `/v2/activity/growth/makeup-cards/use` | `{base}/activity/growth/makeup-cards/use` | 同 | 一致（B） |
| body | `{"target_date":"YYYY-MM-DD"}`，**无** `client_token` | 同 | 同 | 一致（B），补签不带幂等键 |
| 待补日期来源 | `/heatmap` 的 `cells[]`：`score == 0` | `data.cells`，`score === 0` | 未实现 | heatmap `cells[]`（A：AICC 已读过 heatmap；B：两项目一致） |
| 日期边界 | `start = max(当月 1 号, launch_date, 2026-06-17)`，`day < today`，`today` 取服务端 `heatmap.today.date` | 只补"昨日"一格 | 未实现 | 服务端日期为准，不用本机时区（B） |
| `makeup_dates` | **是"已补签"历史，不是待办清单**（`docs/plans/makeup-compatibility.md` 的根因就是误用） | 未使用 | 无 | 必须排除，不能当候选（B） |
| 卡余额 | `makeup_cards` 既可能是 `{"balance":n}` 也可能是裸数字 | `data.makeup_cards.balance` | `read_makeup_cards` 两种都认 | 继续两种都认（A） |
| 已无需补 | 400 + `date is not broken, no makeup needed` → 按"无需操作"处理，不算失败 | 拒绝即静默跳过 | 无 | 单独一类 `NothingToMakeup`（B） |
| 每轮上限 | 官方规则补登卡上限 4 张；每轮最多 1 张 | 每日最多补昨日 1 张 | 无 | 每轮 1 张，卡上限 4（B） |

`#96/#97` 必须自己算候选日期（本机只读校验），**服务端确认前不消耗卡**。

## 三、参考项目能复用什么

| 项目 | 实际实现 | 对 AICC 的价值 |
| --- | --- | --- |
| `88lin/workbuddy-auto-signin` | `run_growth()` 全流程：旅行礼物 → 任务领奖 → 补登 → 连登兑换 → 抽奖 → Buddy 盲盒 → 派出 | **协议参考（B 级）**：错误文案分类、`*_granted` 字段、tier 取值、补签候选算法 |
| `jichie/magpie-workbuddy-tasks` | `fetchStreakState()` + `runStreakSteward()`：状态三路 GET → 补签 → 礼包 → 兑换 → 抽奖 | **架构参考**：先查状态再动作、"状态空就绝不硬闯"、全局 ≥1s 节流、企业版/API key 直接跳过 |
| `changexbc/workbuddy-switch` | `checkin.rs` 是 `/billing/meter/daily-checkin`（**不是**成长中心）；`travel.rs` 是 Buddy 旅行 | **无写路径可复用**。只有旅行模块与 AICC 已有实现重叠，`vendor/crates/wb-switch-core` 里已经用了 |

`wb-switch` 的 `checkin.rs` 有一个和本 PR 直接相关的正确做法（`checkin.rs:168`）：
> 路径候选回落**只有 404** 才继续下一个候选；401/403/10085/`code=-1` 原样返回 ——
> 它们不是路径问题，回落只会掩盖真因并多打一次无意义的请求。

AICC `growth/api.rs` 已经是这个行为（只有 `RouteMissing` 退让），本 PR 保持并把它写进契约。

## 四、URL 构造契约（修掉一处真实缺陷）

**现状缺陷**：`streak_rewards::request(base, ...)` 自己再拼 `/activity/growth/xxx`。
调用方若传进 `https://www.codebuddy.cn/v2/activity/growth`，拼出来是
`.../v2/activity/growth/activity/growth/redeem` —— 上一版测试自己就把这个双重拼接断言进去了
（`streak_rewards.rs` 旧测试里 base 是 `.../growth-v2/`，期望值是 `.../growth-v2/activity/growth/redeem`）。

**契约**（唯一一条，写在 `growth/api.rs`）：

```
URL = WORKBUDDY_API_ENDPOINT + prefix + suffix
prefix ∈ { "/v2/activity/growth", "/activity/growth" }   （候选，只有 404 才换）
suffix  = "/redeem" | "/lottery/draw" | "/makeup-cards/use" | "/tasks" | ...
```

- `suffix` **永远以 `/` 开头、永远不含 `activity/growth`**，由 `api::endpoint()` 统一拼。
- `api::endpoint()` 是唯一的拼接点；`streak_rewards` 只产出 `suffix + body`，不碰域名与前缀。
- 测试断言：任何 suffix 都不含 `activity`，`endpoint()` 对四个前缀组合都只出现一次成长前缀。

## 五、结果分类契约（修掉第二处缺陷）

**现状缺陷**（`streak_rewards.rs:115-169`）：

1. `401` 与 `403` 合并成 `Unauthorized` → 一次"连登天数不足"会让账号被标成授权失效去换源。
2. 任何不认识的业务码都落到 `NotEligible` → 活动结束、账号被限制、参数错都会被说成"没资格"。
3. 到账字段读 `credits/credit/reward_credit`，而兑换的实发字段是 `*_granted`。

**新契约**（`Outcome`，`growth/streak_rewards.rs`）：

| 判定 | 触发条件 | 改授权状态？ | 可重发？ | 关闭账本？ | 账本阶段 |
| --- | --- | --- | --- | --- | --- |
| `Granted` | 2xx + `code` 为 0/200，且解析到实发数值或 ok 标记 | 否 | 否 | 是 | `confirmed` |
| `AlreadyClaimed` | `already` / `already_claimed` / 403+已领取 | 否 | 否 | 是 | `rejected` |
| `Rejected` | 明确常态拒绝：`TierLocked`(403+天数不足) / `NoChance` / `NothingToMakeup` / `ActivityEnded` | 否 | 否 | 是 | `rejected` |
| `Unauthorized` | 401 | **是**（唯一一个） | 否 | 是 | `rejected` |
| `Forbidden` | 403 但文案认不出来（风控 / 受限） | 否 | 否 | **否 —— 冻结** | `frozen` |
| `RateLimited` | 429 | 否 | 退避后（回查后） | 否 → 回查 | `unknown` |
| `ServerError` | 5xx | 否 | **否**（可能已生效） | 否 → 回查 | `unknown` |
| `Indeterminate` | 无响应 / 超时（`http_status == 0`） | 否 | **否** | 否 → 回查 | `unknown` |
| `Unknown` | 其余一切 | 否 | 否 | 否 → 回查 | `unknown` |
| `RouteMissing` | 404 | 否 | **是**（换前缀，服务端什么都没做） | 否 | `ready` |
| `InvalidRequest` | 400 + tier 不认识（参数校验阶段） | 否 | **是**（换写法，服务端什么都没做） | 否 | `ready` |

四条硬规则：

1. **只有 `Unauthorized` 能改账号凭据状态。** `Forbidden`/`Rejected`/`Unknown` 一律不许。
2. **`Unknown` 不是成功**，也不是"没资格"——它要求回查服务端，不许原地重发。
3. **`data.results[]` 出现时一律 `Unknown`**：那是接单/领奖的逐条信封，这三个动作没有
   "逐条结果"的语义，读不懂就不许当成功（沿用 `rewards.rs` 的教训）。
4. **认不出来的 403 冻结，不当作已完成。** 关掉之后下一轮又会重新提交，等于拿同一份
   授权反复撞风控（复审 P0-2）。

## 六、幂等与 `client_token` 契约

- 生成方式：`u-<uuid v4>`，与官方前端一致（88lin `_client_token`，B 级）。服务端只做去重，不校验格式。
- 一次**逻辑操作**只有一个 token，重发必须复用；**跨进程重启也要复用**（存在账本里）。
- 补签不带 token，幂等键是 `(account_id, "makeup_card", target_date)`。
- 抽奖没有"目标"，一次抽奖就是一条独立操作：同一个账号同时**只允许一条未决的抽奖记录**，
  否则两个进程会各抽一次。
- 写请求超时 / 进程崩溃后：**先回查服务端，不许直接生成新 token 重发**（账本状态机保证）。

## 七、操作账本（v24）

表 `reward_operations`（`account-worker/src/storage/reward_ops.rs`）：

| 列 | 用途 |
| --- | --- |
| `id` | 操作 id（uuid） |
| `account_id` | 账号 |
| `action` | `redeem` / `lottery_draw` / `makeup_card` |
| `target` | 档位 `7d`/`14d`/`28d`、补签日期；抽奖为空串 |
| `client_token` | 幂等键（补签为 NULL） |
| `phase` | 见状态机 |
| `attempt` | 已经发出去几次 |
| `outcome` | 判定名（`granted`/`rejected`/…），原始枚举名以便线上核对 |
| `grant_json` | 到账数值 JSON（credits/energy/cards/chances/prize） |
| `server_state` | 回查到的服务端状态快照（档位状态 / 次数 / 卡余额） |
| `detail` | 脱敏说明（≤200 字，`rpc::sanitize`） |
| `created_at`/`updated_at`/`verified_at` | 时间戳 |

状态机（复审后收紧：**结果不明不许关闭**）：

```text
        ┌──────── 发送前先落盘 ────────┐
(无记录) ─▶ pending ─(mark_sent)─▶ sent ──┬─▶ confirmed   ← 明确到账（释放目标）
                                          ├─▶ rejected    ← 常态拒绝/幂等/401（释放目标）
                                          ├─▶ unknown     ← 超时/5xx/429/未知（继续占用）
                                          ├─▶ frozen      ← 未知 403/风控（继续占用，等人工）
                                          └─▶ ready       ← 404/tier 参数错（继续占用，可同 token 重发）
ready ─(mark_sent，同一个 token)─▶ sent
unknown ─(回查仍不确定)─▶ unknown ─(回查确认没生效 + verified)─▶ failed（释放目标）
frozen  ─(人工核对)─▶ confirmed / rejected / failed(verified)
```

`failed` 是**唯一**需要 `verified == true` 才能落盘的阶段：它的含义是"回查确认这次请求
确实没生效"，不是"出错了"。超时、5xx、回查仍不确定一律落 `unknown`。

跨进程保护的三层：

1. **部分唯一索引**（DB 层，最关键）：
   `UNIQUE (account_id, action, target) WHERE phase IN ('pending','ready','sent','unknown','frozen')`
   —— 同一账号同一目标**物理上不可能**有两条未决记录，绕过代码也挡得住。
2. **`BEGIN IMMEDIATE` 事务**：选/插在同一次写事务里，配合 `busy_timeout=5s`（WAL 已开），
   两个进程只会有一个 `Created`，另一个拿到 `Existing` 并被迫走回查。
3. **token 唯一索引**：`(account_id, action, client_token)`，防止同一 token 落在两条记录上。

`plan()`（`growth/reward_ledger.rs`）是唯一的决策入口：

| 账本现状 | 下一步 |
| --- | --- |
| 没有未决记录 | `Insert`：生成新 token，先落盘再发 |
| `pending` + `attempt == 0` | `Resend`：沿用已有 token（崩溃在发送前） |
| `pending` + `attempt > 0` | `Recheck`：状态不明，先查服务端 |
| `sent` / `unknown` | `Recheck`：**不许发**，先查服务端（**次数用尽也要回查**） |
| `ready` 且 `attempt < 3` | `Resend`：同一个 token 再发一次 |
| `ready` 且 `attempt >= 3` | `Blocked`：没有可查的东西，只能停手 |
| `frozen` 或阶段不认识 | `Blocked`：停下，等人工 |

**次数上限只约束"再发一次"，绝不约束只读回查**（复审 P1-1）：否则第 3 次发送之后
结果就永远查不到，记录会一直悬在 `sent` 上。

### 7.1 存储边界（复审 P1-3）

`resolve_reward_op` 不信任调用方，三件事在存储层强制：

| 项 | 规则 |
| --- | --- |
| `detail` | `rpc::sanitize` 脱敏 + 截断 200 字 |
| `grant` / `server_state` | 只接受 **JSON 对象**；只保留扁平标量（数字/布尔/短字符串） |
| 嵌套对象 / 数组 / null | 丢弃 —— 原始响应不属于账本 |
| 键名 | ≤32 字符、ASCII；含 `token`/`secret`/`password`/`cookie`/`authorization`/`bearer`/`refresh`/`access` 一律丢弃 |
| 值长度 | 字符串截断 64 字符；整体超过 512 字节就丢字段 |

### 7.2 账号删除与重新导入（复审 P1-2）

WorkBuddy 账号 id 是 `(platform, uid)` 的确定性哈希，删号后用同一个 UID 重新导入会
**复用同一个 id**。所以 `delete_account` 在**一个事务**里同时清 `quota_snapshots`、
`reward_operations` 和 `accounts` 行：否则新账号一进来就继承旧账号那条未决记录，
界面会显示"还有一个没查完的抽奖"。

**刻意不用 `auth_epoch` 做生命周期隔离**：它每次重新授权都会 +1，而"重新授权"恰恰
不该释放一个可能已经发出去的写操作 —— 那正是重复消耗的入口。

## 八、本 PR 之后的未验证项（`#96` 接线前必须补）

| 项 | 当前等级 | 补证方式 |
| --- | --- | --- |
| 三个写路径在 AICC 前缀下的真实存在性 | B | 零副作用探针（不存在的 tier / 过去日期的补签请按用户授权） |
| 抽奖响应的积分/能量字段 | C | 一次真实抽奖（用户授权后） |
| 兑换响应是否含 `makeup_cards.balance` | C | 一次真实兑换 |
| 补签成功响应的余额字段 | C | 一次真实补签 |
| 429 / 5xx 的服务端原话 | D | 不制造；按 `retryable` 分类处理 |
| `/lottery/summary` 是否存在 | C | 只读探测，可与 `/lottery/chances` 对账 |
| `/redeem/summary` 的 `starter_status` 与 `tier_7d_status` 是否同时存在 | A（两者分别被实测过，但没见过同一次响应） | 一次只读 GET 对账 |

## 九、代码对应

| 契约 | 代码 |
| --- | --- |
| 唯一 URL 拼接点 | `growth/api.rs::endpoint` |
| 三个动作的形态（suffix + body） | `growth/streak_rewards.rs::call` |
| 结果分类 11 类 + 四条硬规则 | `growth/streak_rewards.rs::classify` |
| **判定 → 账本阶段的唯一翻译** | `growth/streak_rewards.rs::Outcome::ledger_phase` |
| token 规则 | `growth/streak_rewards.rs::client_token` |
| 下一步决策 | `growth/reward_ledger.rs::plan` |
| 账本持久化 + 终态/脱敏/清理 | `storage/reward_ops.rs`、`storage/mod.rs` v24 |
| 不发请求 | 三个动作**没有任何**调用 `api::post` 的代码路径（`#96` 才接） |

## 十、复审修复（2026-11-16 第二轮，本节对应复审意见逐条）

| 编号 | 问题 | 修复 | 回归测试 |
| --- | --- | --- | --- |
| P0-1 | `failed` 可当终态，超时后账本被关闭，下一轮生成新 token | `failed` 必须 `verified == true`（回查确认没生效）；超时/5xx/未知一律 `unknown`；`unknown` 与 `frozen` 继续占着唯一索引 | `an_indeterminate_result_never_frees_the_target`、`failed_requires_a_verified_recheck` |
| P0-2 | 未知 403 被当终态，风控被反复触发 | `Outcome::terminal()` 不再包含 `Forbidden`；`ledger_phase()` 映射到 `frozen`；`requires_attention()` 只给 `Forbidden` | `an_unknown_403_is_frozen_rather_than_retried`、`a_frozen_operation_keeps_the_target_and_blocks_new_tokens` |
| P1-1 | 次数上限挡住回查 | `plan()` 先判 `sent`/`unknown` → `Recheck`，上限只作用于 `ready` 的重发 | `the_attempt_cap_blocks_sending_but_never_blocks_a_recheck`、`a_ready_operation_resends_with_the_same_token_until_the_cap` |
| P1-2 | 删号不清账本，同 UID 重导入继承旧记录 | `delete_account` 在一个事务里清 `quota_snapshots` + `reward_operations` + `accounts` | `deleting_an_account_purges_its_reward_ledger`、`a_reimported_account_starts_with_a_clean_ledger` |
| P1-3 | `detail` 未在存储层脱敏，回执可存原始响应 | 存储边界强制脱敏/截断；`grant`/`server_state` 只收扁平标量，凭据键与嵌套结构丢弃 | `detail_is_sanitised_and_truncated_at_the_storage_boundary`、`raw_server_payloads_cannot_be_stored`、`a_payload_that_is_not_an_object_is_dropped_instead_of_stored_raw` |

新增 `ready` 阶段的理由：404 / tier 参数错这两种"服务端明确没执行"的情况必须允许
用**同一个 token** 再发一次（换前缀、换写法是正常协议行为）。它仍然占着目标，
所以并发下不会变成两次操作；次数上限对它生效，因为此时没有可回查的东西。
