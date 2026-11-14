# Qoder 每日 Credits 领取（PR #75）

更新时间：**2026-11-14**。本文只回答三件事：**做了什么、依据是什么、哪些还没验证**。

## 1. 一句话结论

| 能力 | Qoder CN | Qoder International |
| --- | --- | --- |
| 只读查询今日活动 | **已实现**，本地协议与解析有单测；**真实凭据尚未跑通** | **未实现**，零请求 |
| 手动领取 | **已实现**（含超时回查、幂等去重、失败分类） | 界面不显示领取按钮 |
| 自动领取 | **已实现**，每账号独立开关、默认关闭、20 分钟限速 | 开关禁用 |
| 额度看板 | 显示"当日奖励"与活动状态，**不伪造余额**；授权来源取本次真实使用的那一份 | `unverified` + 原因 |

"未验证"是**第三种状态**：不是"没有这个功能"，也不是"功能坏了"，而是
**AICC 还没拿到一份带真实凭据的成功响应**。在拿到之前，任何"领取成功"的
本地测试都只证明代码路径正确。

## 2. 依据（不是猜的）

- 社区参考实现：`88lin/workbuddy-auto-signin` → `qoder_signin.py`（MIT）。
  它调用的两个接口就是下面这两个，且明确记录了 `Cosy-ClientType` 的必要性。
- 官方域可达性：本机实测 `openapi.qoder.com.cn` / `qoder.cn` / `qoder.com`
  均可连通（404 于根路径，说明服务在）。
- **没有**证据的部分：International 是否也有同形活动接口、PAT 是否有权调用
  `/sash/...`、活动刷新时刻的真实规则。

## 3. 协议（只有 CN）

```text
GET  https://openapi.qoder.com.cn/sash/api/v1/me/campaigns
POST https://openapi.qoder.com.cn/sash/api/v1/me/campaigns/{campaignId}/claim
```

请求头：

| 头 | 值 | 为什么要写死 |
| --- | --- | --- |
| `Authorization` | `Bearer <device token 或 PAT>` | 优先浏览器授权，PAT 兜底 |
| `Cosy-ClientType` | `10` | **缺了服务端返回空活动列表**（不是错误），最阴的一种失败 |
| `User-Agent` | `Qoder/claim` | 与参考实现一致 |
| `Accept` | `application/json` | — |

参考实现还带了一组 `Cosy-Machine*` 风控头（来自桌面客户端的 `runtime-info`）。
AICC **不伪造这些值**：拿不到就是拿不到，不猜。如果真实请求因此被拒，结论
应该如实写成"缺客户端风控头，被服务端拒绝"，而不是继续伪造到成功。

### 3.1 为什么 International 是零请求

"把 `openapi.qoder.com.cn` 换成 `openapi.qoder.sh`" 是本项目最该避免的一类错误：
两个站点是独立产品，路径、鉴权、活动模型都可能不同。没有一份真实成功响应
之前，AICC 不发请求、不显示领取按钮、只在看板上写清原因。
要解阻，需要有人用 International 账号抓到一次真实的 `campaigns` 响应。

## 4. 判定规则

| 判定 | 依据 |
| --- | --- |
| 是否在活动窗口内 | 服务端 `startAt` / `endAt`（Unix 秒），**不在本地推算 10:00** |
| 是否可领 | `actionType == "CLAIM_BENEFIT"` 且 `claimStatus == "CLAIMABLE"` 且窗口内 |
| 是否已领 | `claimStatus == "CLAIMED"` 且窗口内 |
| 领取成功 | 服务端返回 `status == "CLAIMED"`；`replayed = true` 记为"已领过" |
| 奖励数量 | `benefit.amount`；**缺失就是未知，不写 0** |

"每天 100 Credits" 只是参考实现对某个活动的描述，AICC 不把它写死：
显示的数量永远来自服务端当天返回。

## 5. 失败处理

| 情况 | 行为 |
| --- | --- |
| 401 | 立刻停手，把该来源标 `invalid`，**不重试** |
| 403 | 同上（有凭据但没权限） |
| 429 / 5xx / 网络 | 查询（GET）按 1.5s、4s 退避重试两次；**领取（POST）不重发** |
| 领取超时 | **先回查活动列表**：服务端已经领了就记 `already`，没领到才记 `network` |
| 200 但 `status != CLAIMED` | 判为失败，不报告成"已领取" |
| 结构不认识 | `failed` + `响应结构不认识`，**不当成"没有活动"**（见 §5.1） |
| 403 | **不改授权状态**：权限范围 / 风控，不是令牌被撤销（见 §5.2） |
| 401 | 确认授权版本未变才标 `invalid`；版本已变（用户换了 PAT）则不写 |

自动领取的额外约束：

- 每账号开关，**默认关闭**；未验证的版本直接拒绝开启（`invalid_params`）；
- 两次尝试至少间隔 **20 分钟**（`force` 只跳过这一条，不绕过开关）；
- 当天已经成功过（`ok` / `already`，按**北京时间**比较日期）就不再动；
- 同一活动周期内成功过一次，不再提交 POST（`qoder_credit_claimed`）。

### 5.1 200 + 看不懂 ≠ 今天没有活动

`parse_campaigns` 只有在**响应里确实有 `campaigns` 且它是空数组**时才返回空列表。
其余全部是 `unparsable`：

| 情况 | 判定 |
| --- | --- |
| `{}`、`{"data":{}}`、没有 `campaigns` 字段 | 错误 |
| `campaigns` 不是数组 | 错误 |
| 某条活动没有 `campaignId` | 错误（整份作废，避免"可领 0 个"的假结论） |
| `code` 非 0 / `success == false` | 错误，带上业务码或 message |
| `campaigns: []` | **空列表**（这才是真的没活动） |

早先的版本对任何异常结构都返回空数组，于是 200 + 业务失败 + 结构变更都会被
显示成"今天没有活动"——用户会以为今天确实没奖励，而真相是接口变了。

### 5.2 401 与 403 不是一回事

- **401**：这份凭据不被接受了。标 `invalid`，但必须先核对 `auth_epoch`——
  请求要几秒，期间用户可能已经换了新 PAT，旧结果不能把新授权标脏。
- **403**：有凭据但没权限（PAT 没勾对应范围、或触发风控）。**不改授权状态**：
  标成 `invalid` 会让用户白去重新生成一张本来没问题的令牌。

### 5.3 手动与自动共用一把账号级锁

两侧同时触发（用户点「立即领取」的那一秒心跳也跑起来）会让同一个活动被提交两次。
服务端幂等会让其中一个变成 `replayed`，但"到底提交了几次"就说不清了。

- 拿不到锁的调用返回 `state = "busy"`，**不是错误**，自动那条直接跳过；
- 自动化在拿锁之后**再确认一次开关**：这期间用户可能刚关掉；
- 去重按**单个活动**判断，不是整批：一批里有一项已领，不影响其它项提交。

### 5.4 每次 POST 前再问一遍

查询活动与提交领取之间隔着一次网络往返。这段时间里用户可能关掉自动开关、
删掉账号，或者换了一份新授权。任一发生，都不该再用旧快照里的活动去提交。

`pre_post_stop()` 在每个 POST 之前重新确认三件事，任一不满足就**停下并写明原因**：

| 情况 | 判定 | 结果 |
| --- | --- | --- |
| 账号已删除 | `qoder_auth_epoch` 读不到 | `reason = account_removed` |
| 授权被替换 / 重新保存 | `auth_epoch` 与请求发起时不同 | `reason = authorization_changed` |
| 自动开关被关（仅自动路径） | `credits_auto_enabled = 0` | `reason = auto_disabled` |

手动路径不受开关影响（用户自己点的，本来就代表"现在就要"）。

### 5.5 去重键包含活动周期（v18）

`campaignId` 由服务端生成，**同一个 id 可能在不同周期被复用**。只按 id 去重时，
次日会被当成"已经领过"而领不到。

去重键改为 `(account_id, campaign_id, period_start)`：

| 场景 | 是否算领过 |
| --- | --- |
| 同一 id、同一 `startAt`，已 `ok`/`already` | 算（不重复提交） |
| 同一 id、`startAt` 不同（次日） | **不算**（允许领） |
| 服务端没给 `startAt` | **不算**（宁可多一次幂等 POST，也不能挡住合法领取） |

## 6. 存储（v17 / v18）

| 对象 | 位置 |
| --- | --- |
| 自动开关 | `qoder_accounts.credits_auto_enabled`（默认 0） |
| 领取账 | `qoder_credit_runs`（含 `period_start` / `period_end`，账号删掉则级联删除） |
| 额度缓存 | 复用 `quota_snapshots`（v14/v15），**不另造一套** |

`qoder_credit_runs` 只记"发过什么、服务端怎么回"，**不缓存活动状态**：
能不能领永远看实时响应。`credits` 可空，未知奖励不写 0。

迁移是 Rust 步骤而不是纯 SQL：先检测 `credits_auto_enabled` 是否存在再决定
是否 `ALTER`，`user_version` 被回拨后重复执行不会炸在 `duplicate column name`。

### 6.1 快照并发安全

`accounts.auth_epoch`（v5）对 Qoder 不适用——Qoder 账号在 `qoder_accounts`。
v16 给那张表加了同名字段，授权写入/删除都会 +1；额度写入前原子核验
`(id, edition, auth_epoch)`：

```sql
INSERT INTO quota_snapshots (...) SELECT ...
WHERE EXISTS (SELECT 1 FROM qoder_accounts
              WHERE id = ?2 AND edition = ?3 AND auth_epoch = ?7)
```

覆盖四种情况：账号被删（无父行）、重新授权（版本变）、同 uid 重新导入（版本变）、
CN/International 串号（`edition` 不匹配）。四种都有单测。

## 7. 界面

| 位置 | 显示什么 |
| --- | --- |
| 额度页 · Qoder 卡片 | 当日状态（可领/已领/无活动/未支持）、窗口截止、「检查」「领取今日」 |
| 签到任务页 · Qoder 段 | 每账号自动开关、检查/领取、最近一条与最近 5 条记录 |

按钮可点性与状态严格对应：只有 `state == "claimable"` 时"领取今日"才可点；
未支持的版本按钮置灰并给出原因，不放一个永远失败的空任务。

## 8. 真实验收矩阵

| 验收项 | 需要的证据 | 当前状态 |
| --- | --- | --- |
| CN 查询活动 | 脱敏后的请求（URL + 头名）与真实 `campaigns` 结构 | **unverified**：无真实凭据 |
| CN 手动领取 | 服务端回执 `status=CLAIMED` + 回查后 `claimStatus=CLAIMED` | **unverified** |
| CN 自动领取 | `qoder_credit_runs` 里一条 `trigger=auto` 的 `ok`，且同周期无重复 POST | **unverified**（依赖上两项） |
| PAT 能否领 Credits | 用 PAT 跑通一次 `campaigns` | **unverified** |
| International 活动接口 | 一份真实响应 | **blocked**：无证据，零请求 |
| 额度看板 | 一张"可领/已领"的截图 + 对应 `quota_snapshots` 行 | 本地渲染已验，真实数据待验 |

跑真实验收时不需要把令牌交给任何人：

1. 在 AICC 里用浏览器授权或 PAT 添加一个 Qoder CN 账号；
2. 额度页点「检查」——若返回 `unauthorized` / `error`，把**脱敏后**的错误文案
   记下来；
3. 若返回 `claimable`，点「领取今日」，再看 `qoder.credits.runs` 的 outcome；
4. 需要自动化时打开该账号的自动开关，第二天检查 `trigger=auto` 的记录。

## 9. 明确不做

- 不伪造 `Cosy-Machine*` 风控头；
- 不把国内版接口换域名套到 International；
- 不在 Android / Widget 侧实现登录、PAT 输入与自动领取；
- 不为"刷任务"构造任何无意义请求。
