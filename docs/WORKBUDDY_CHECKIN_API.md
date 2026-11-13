# WorkBuddy 每日签到接口证据（PR #69 · M0）

本文件记录 `POST /v2/billing/meter/*` 这一族接口**我们知道什么、凭什么知道、还差什么**。
写代码之前先看这里，别把"上游这么写"当成"服务端这么答"。

证据分三级：

| 级别 | 含义 |
| --- | --- |
| **A 实测** | AICC 自己用真实账号打过，看过原始响应 |
| **B 交叉** | 上游代码 + 上游测试 + AICC 已有的间接观测互相印证，但没有 AICC 自己的原始报文 |
| **C 未测** | 只有代码里的假设，没有观测；改动时按"可能不对"处理 |

> **本轮没有执行任何真实 `daily-checkin`。** 原因写在 §5，属于 PRD §9 红线第 10 条。

---

## 1. 端点

域名：`https://www.codebuddy.cn`（`WbVariant::Cn.api_endpoint()`，与成长中心同域）。

| 用途 | 路径 | 方法 | 请求体 | 证据 |
| --- | --- | --- | --- | --- |
| 查今日状态（首选） | `/v2/billing/meter/checkin-activity-status` | POST | `{}` | **B** |
| 查今日状态（旧） | `/v2/billing/meter/checkin-status` | POST | `{}` | **B** |
| 执行每日签到 | `/v2/billing/meter/daily-checkin` | POST | `{}` | **B（形态）/ C（响应）** |

三条都在 `CHECKIN_API_PREFIX = "/v2/billing/meter"` 之下（`wb-switch-core/src/modules/config.rs`）。
国内版只有这一种路径形态，**没有** `/activity/growth` 那种"去掉 v2"的候选。

### 请求头

直接用 `wb_switch_core::modules::account::build_auth_headers(payload)`：

```
Authorization: Bearer <access_token>
Accept: application/json
Content-Type: application/json
X-User-Id: <uid>
```

`payload` 只放 `access_token` / `uid` / `domain` / `variant`，**不放 refresh token**。
这是 PR #66 定的边界：AICC 借用 Magpie 的 access，任何情况下都不刷新它。

### 状态查询为什么是 POST

服务端把"查状态"做成了 POST，但它**不改签到状态**：重复调用结果一致，
且 AICC 在真实账号上跑过（见 §2），没有产生任何签到记录变化。
所以 AICC 把它当只读接口使用，并额外用两道保险：

- `checkin.status` RPC 只调它，**没有**提交能力（构造上：那条路径里没有 `submit`）；
- 自动调度只在真正要签到时才走提交流程，且提交前再查一次。

---

## 2. 状态响应

### 2.1 观测到的形态

```json
{
  "code": 0,
  "data": {
    "today_checked_in": true,
    "streak_days": 42,
    "daily_credit": 100
  }
}
```

字段位置：`data.today_checked_in`（也接受驼峰 `data.todayCheckedIn`）。

| 结论 | 级别 | 依据 |
| --- | --- | --- |
| 状态查询路径可用、POST `{}`、Bearer 认证可用 | **A** | 2026-11-13 第一轮验收：在签名 App 里对 4 个真实账号逐个查询成功（见 §2.1.1） |
| 今日状态在 `data.today_checked_in`（或驼峰） | **A** | 同上：4 个账号的「已签到 / 未签到」与 WorkBuddy 官方页面逐一对上；若这个键读不到，界面会显示「状态未知」而不是确认态 |
| `streak_days` / `daily_credit` 在 `data` 下 | **B** | 上游代码同款读法；本轮界面没有核对这两个数字，PR #63 曾记录真实值 `streakDays=42 · dailyCredit=100` |
| 两条候选路径里**实际生效的那一条** | **C** | 需要看 `checkin.status` 每行的 `path` 字段才能确定（`checkin-activity-status` 还是回退到 `checkin-status`）。解析器两条都试，只在 404 时退让 |
| `today_credit`（今天已得积分）存在 | **C** | 代码里读了，但没有任何观测确认它会出现 |
| 缺 `today_checked_in` 时服务端可能省略该键 | **C** | 未观测；正因为不确定，AICC 把它当 `unknown` 处理 |

### 2.1.1 实测记录（2026-11-13 · 第一轮只读验收）

条件：`~/Applications/AICC.app`（PR #69 构建），4 个真实 WorkBuddy 账号，用户点了「刷新全部状态」。

| 观测 | 结果 |
| --- | --- |
| 4 个账号状态读取 | 全部成功，无 `unknown`、无 `failed` |
| 第三方软件（Magpie）已签到的账号 | 正确显示「服务端已签到，本次未提交」，奖励列 `—` |
| 本地签到记录为空时 | 结论完全来自服务端，**没有**因为"本地没记录"就当成未签到 |
| 产生的 `daily-checkin` 写请求 | **0 次** |

这一轮顺带证明了设计里最关键的一条：**签到不是 AICC 做的，AICC 也不会去补一次。**
界面上那句「可能由 Magpie 或官方客户端完成」是推测语气，但"不提交"是硬事实 ——
因为提交分支只从 `today_checked_in == false` 进入，而这一路根本没走到。

仍未验证的仍然是**写接口**（§3 全表）：服务端真实的成功/奖励字段名、重复签到的回执文案、
429/5xx 的具体形态。所以 §3 的级别一条都没有升。

### 2.2 AICC 的判定规则（比上游严格）

```
data.today_checked_in == true   → already（不提交）
data.today_checked_in == false  → 可以提交
键缺失 / 类型不对               → unknown（**绝不提交**）
```

上游 `wb-switch-core` 这里是 `.unwrap_or(false)`：接口一改结构就会变成每天
重复签到一次。AICC 不复用那段逻辑（`account-worker/src/checkin/api.rs`
`status_from_response` 返回 `Option<bool>`），并有一个专门的用例
`a_missing_boolean_is_unknown_not_false` 钉住它。

---

## 3. 执行签到

### 3.1 请求

```
POST /v2/billing/meter/daily-checkin
{}
```

没有参数、没有日期字段（日期由服务端按账号时区决定）。**AICC 不传日期** ——
传错日期就是给"补签昨天"开门，而 PRD 明确不做补签。

### 3.2 成功判定

| 情形 | AICC 的处理 | 级别 |
| --- | --- | --- |
| `code == 0` 或 `200` | 视为"已受理"，**必须回查**才记 `claimed` | B |
| 回查 `today_checked_in == true` | `claimed`，奖励取回执里的数字 | B |
| 回查没确认 | `unknown`，**不记奖励、不记成功** | B（规则）/ C（真实是否会发生） |
| 业务码非 0，文案含"已签到"/"重复"/"repeat" | `already`（幂等回执，不是失败） | **B** |
| `code == 401` | `unauthorized`，停这个账号 | B |
| `code == 403` | `forbidden`，**不当授权失效**（可能是网关 10085） | B |
| `code == 429` | `rate_limited`，30 分钟冷却，不重试 | C |
| `5xx` / `code == -1`（传输错误） | 回查一次；确认已签 → `claimed`，否则 `unknown` | B（规则） |
| 超时（15s） | 同上：**先回查，不重发** | B（规则） |

### 3.3 奖励字段

`data` 下按顺序找：`credit` → `credits` → `today_credit` → `todayCredit` → `reward_credit`。

**没有就存 NULL**，不填 100、不按历史值推断。界面显示 `—`。
原因：成长中心的实测已经证明"服务端不一定回数额"（`/profile` 根本没有积分字段），
凭空补一个数字就是造假余额。

| 结论 | 级别 |
| --- | --- |
| `data.credit` 是奖励字段 | **C** |
| 重复签到时奖励为 0 / 缺省 | **C** |

这一条必须用一次真实签到校准（验收第二轮），届时按实际键名收敛上面的候选列表。

---

## 4. 错误分类与"不许做的事"

```
404        → RouteMissing  → 允许换另一条状态路径（仅此一处换路）
401        → Unauthorized  → 停这个账号，换源逻辑交给 auth::broker / probe
403        → Forbidden     → 记录，不改授权状态
429        → RateLimited   → 冷却 30 分钟，不重试
5xx        → Server        → 回查一次，不重发写请求
-1 / 超时  → Network       → 回查一次，不重发写请求
其它非 0   → Business      → 记录原始 message（脱敏后进 task_executions）
```

为什么只在 404 换路：401/403/429/5xx 是服务端的**真实回答**，
换个路径再问一遍只会把真因藏起来，多打一次无意义的请求。
这条规则在 #67 的成长中心已经验证过，签到沿用同一套。

---

## 5. 本轮实测边界（必须如实记录）

| 想做的事 | 结果 | 原因 |
| --- | --- | --- |
| 在真实账号上跑 `checkin.status`（只读） | **已完成（2026-11-13，App 进程内）** | 4 个真实账号全部读到，第三方已签到的账号被正确识别，零写请求。见 §2.1.1 |
| 用 `target/debug/aicc-account-worker` 做同样的只读校验 | **不可行** | 未签名二进制读钥匙串会被 ACL 拦住并无限等待（与 PR #60 同类）。只读校验只能在签名 App 里做 |
| 真实库迁移到 v10 | **已完成** | 用 `.backup` 副本验证：v9 → v10，4 账号 / 22 条成长记录 / 33 条旅行事件全在 |
| `daily-checkin` 真实 POST | **0 次** | PRD §9 红线：不经用户明确操作不发写请求；验收当天 4 个账号都已被第三方签到 |

结论：**读路径 A 级、写路径仍 C 级**。

读路径的证据已经闭环（不然界面上不可能出现「已签到」），但它只证明了
"查得准"，没有证明"签得对"。写路径要等一次真实验收：

1. 在第三方软件签到之前刷新状态（必须看到服务端明确说未签到）；
2. 点「立即签到」；
3. 确认服务端变已签到、界面出现真实奖励数字；
4. 再点一次，确认变成「今天已经签过了」且服务端没有收到第二次 `daily-checkin`。

第 3 步拿到真实回执后，把 §3.2 / §3.3 里对应的 B、C 升上来，并收敛奖励字段名。

---

## 6. 与上游的差异（改动前先读）

| 项 | 上游 `wb-switch-core` | AICC |
| --- | --- | --- |
| 缺 `today_checked_in` | 当 `false`，继续提交 | 当 `unknown`，**不提交** |
| 状态查询失败 | 直接进 `Submit` 分支重试 | 分类后决定；授权类直接停 |
| 国际版 | 有 `statusUnsupported` 分支 | 直接 `unsupported`，不发任何请求 |
| 令牌刷新 | 401 时可能刷新 refresh token | 从不刷新；Magpie 来源只允许重新同步 access |
| 记录 | 写它自己的 `checkin_log.json` | 写 `checkin_records` + `task_executions` |
| 时间窗口 | `checkin_start` / `checkin_end` 本地时区 | 固定北京时间 09:00 + 账号错峰，不看机器时区 |

最后一条是重点：上游用 `chrono::Local`，机器时区一改，"今天"就变了。
AICC 统一用 `clock::beijing_date()` / `beijing_now()`。

---

## 7. 状态机与调度

### 7.1 状态（`checkin/state.rs`）

| 状态 | 含义 | 会不会写服务端 | 落 `checkin_records.status` |
| --- | --- | --- | --- |
| `disabled` | 该账号没开自动签到 | 否（连状态都不查） | `skipped` |
| `unchecked` | 服务端确认今天未签到 | 会（下一步就提交） | 不写（中间态） |
| `already` | 服务端确认今天已签到 | 否 | `already` |
| `claimed` | 提交后回查确认成功 | 是 | `claimed` |
| `unknown` | 说不清 / 回查没确认 | 否 | `failed` + `error_kind` |
| `unauthorized` | 授权失效 | 否 | `failed` + `error_kind` |
| `no_credential` | 没有可用授权 | 否 | `failed` + `error_kind` |
| `failed` | 明确失败（403 / 429 / 5xx / 业务码） | 否 | `failed` + `error_kind` |
| `unsupported` | 平台/档位不支持 | 否 | `skipped` |
| `busy` | 同账号已有执行在跑 | 否 | 不写（并发噪声） |
| `stopped` | 提交前发现开关被关 | 否 | `skipped` |

`checkin_records` 的 CHECK 只允许四个值，所以细分类放 `error_kind` 列，
不为多几个枚举值去重建一张已发布的表。

### 7.2 执行顺序（不许调换）

```
平台检查 → 开关（自动轮次）→ 账号级互斥 → 取授权
   → 查状态 ─┬ Some(true)  → already，结束
             ├ None        → unknown，结束（不提交）
             └ Some(false) → 继续
   → 再问一次开关（WriteGate）→ 关了就 stopped，结束
   → 提交 ─┬ 成功回执 → 回查 ─┬ Some(true) → claimed
           │                    └ 其它      → unknown
           ├ already 回执 → already
           └ 超时/5xx → 回查一次（不许重发）→ claimed / unknown
           └ 401/403/429/业务码 → failed / unauthorized
```

### 7.3 调度（`checkin/scheduler.rs`）

- 计划时间：**北京时间每天 09:00**，每个账号加一个由 account_id 算出的固定偏移
  （0–300 秒）。同一台机器上顺序稳定，重启不变。
- 心跳复用 worker 已有的 60 秒 tick；不新增进程、不开端口、不注册第二个定时器。
- 迟到补做：09:00 之后才打开 App，当天仍会补一次；已经签过就跳过；
  **绝不补昨天**（请求里不带日期，服务端只认当天）。
- 一天最多 4 次尝试（含失败重试），退避 60s → 300s → 1800s，
  429 直接 30 分钟冷却。尝试次数从 `task_executions` 数，App 重启不会重新数。
- 授权失效 / 缺少凭据：当天不再试，直接把 `next_at` 推到明天。
- 多账号：串行，账号之间 800ms 间隔。
- `force` **只**代表"现在没到计划时间也跑一次"。它不绕过账号开关，
  也**不绕过一天 4 次的上限** —— 否则连点几下「立即检查」就能变成无上限重试。
  用例 `force_cannot_bypass_the_daily_attempt_cap` /
  `force_still_runs_an_account_that_has_attempts_left` 一正一反钉住这条边界。

### 7.4 为什么签到开关不能走 `accounts.upsert`

`upsert_account` 每次写入都会 `auth_epoch = auth_epoch + 1`（迁移 v5，用来让在途的旧
凭据快照作废）。点一下签到开关就把旅行 / 成长中心的在途快照全废掉，属于开关不该有的副作用。
所以新增 `Db::set_checkin_enabled()`，只更新 `checkin_enabled` + `updated_at`，
并有用例 `the_switch_does_not_touch_the_credential_epoch` 与 stdio 端到端用例
`checkin_switch_over_rpc_does_not_bump_the_credential_epoch` 钉住。

---

## 8. 数据库变更

**v10**（只追加两列，不改 v1–v9，不占用保留给 #66 的 v8）：

```sql
ALTER TABLE checkin_records ADD COLUMN error_kind TEXT;
ALTER TABLE checkin_records ADD COLUMN execution_source TEXT;
```

| 表 | 用途 | 一天几行 |
| --- | --- | --- |
| `checkin_records` | 当天最终结论（`claimed` / `already` / `failed` / `skipped`）+ 奖励 + 来源 | **1**（`UNIQUE(account_id, local_date)`） |
| `task_executions`（复用） | 每次尝试的细节：`task_kind='daily_checkin'`、`error_kind`、`next_retry_at` | N |
| `accounts.last_result` / `last_checkin_at` | 账号列表上那一行文字 | 1 |

写入规则（`storage/checkin.rs` 的 `upsert_checkin_record`）：

1. 已经 `claimed` 的一天，后面来的 `already` / `failed` / `skipped` **一律不覆盖**；
2. 已经 `claimed` 的一天，再次 `claimed` 但没拿到数额 → **保留旧数额**，不抹成 NULL；
3. 只有服务端确认过的结论（`claimed` / `already`）才写 `accounts.last_result`；
4. 奖励缺失存 NULL，不填 0 或历史值；
5. 不删历史记录。

升级验证：用真实库的 `.backup` 副本跑过 v9 → v10，`user_version` 到 10、`integrityOk`、
4 个账号 / 22 条成长记录 / 33 条旅行事件全部保留（用例
`upgrading_from_v9_keeps_accounts_records_and_growth_history` 在 CI 里复现这一条）。

回退：v10 只多两列，旧二进制不看它们即可，不需要脚本。
