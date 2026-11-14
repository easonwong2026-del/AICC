# WorkBuddy 额度与用量接口（只读）

实测时间：**2026-11-14**，本机真实账号各一次，凭据未落盘、未进日志。
结论只覆盖下表列出的两个端点；**表外的一切都是未验证**。

## 1. 端点

| 档位 | 基址 | 摘要 | 用量 |
| --- | --- | --- | --- |
| 国内版 | `https://www.codebuddy.cn` | `POST /billing/meter/get-user-resource-summary` | `POST /billing/meter/get-user-request-usage` |
| 国际版 | `https://www.workbuddy.ai` | 同上 | 同上 |

- 两版**同一套路径**，只有基址不同；请求/响应结构一致。
- `/v2/billing/meter/get-user-resource-summary` 实测 **404**（`{"error_msg":"404 Route Not Found"}`）。
  带 `/v2` 的只有签到 / 成长那一组，额度这组没有前缀。
- 请求头：`Authorization: Bearer <access>`、`X-User-Id: <uid>`、`X-Domain: <账号的 auth_domain>`、
  `Content-Type: application/json`。

## 2. 摘要响应

```
{"code":0,"msg":"OK","requestId":"...","data":{
  "Packages":[{"PackageCode":"...","CycleTotalCapacity":"4913",
               "CycleRemainCapacity":"4597.75000075","CycleUsedCapacity":"315.24999925",
               "CycleFrozenCapacity":"0","CapacityUnit":"credits","TotalCount":67}],
  "CreditGroups":[...], "TotalCredits":"5197.75000075",
  "SubscriptionPackageCode":"", "SubscriptionPackageName":"体验版",
  "IsPaidUser":false, "IsProtectedPriceUser":false, "ProTrialStatus":0}}
```

三个必须按原样处理的事实：

1. **数值是字符串**，且带小数（`"4597.75000075"`）。解析成 `f64` 时不能四舍五入到整数。
2. **`CapacityUnit` 会写 `credit` 或 `credits`**（同一账号的两个包分别是两种写法）。
   两者是同一笔账，归一后**必须相加**；`tokens` 与 `credits` 是真不同单位，**永不相加**。
3. **`TotalCredits` 是"可用额度合计"，不是总容量**。国内版实测：三个包
   4913/500/100（共 **5513**），剩余 4597.75/500/100，而 `TotalCredits` = **5197.75**
   —— 正好等于剩余之和。所以它只能当 `remaining` 的兜底，拿来当 total 会把
   "还能用多少"写成"一共有多少"。总容量一律由 `Packages[].CycleTotalCapacity` 求和。
4. **没有到期时间**。摘要里没有任何 `expire*` 字段，所以额度行的 `expiresAt` 一律
   是未知，不拿订阅到期时间去猜。

## 2.1 必须带 `User-Agent`（否则 403）

国内版网关在**缺少 `User-Agent`** 时返回：

```
403 {"code":10085,"msg":"请求不合法，如有疑问请联系客服"}
```

实测对照（同一 token、同一路径）：`curl` 默认 UA → 200；`curl -A ""` → 403；
`curl -A "AICC/2.12.2"` → 200。因此适配器固定发送 `User-Agent: WorkBuddy`
（与 `growth/api.rs` 同一个值）。国际版不校验，但两个站点用同一个值，避免出现第二套约定。

## 3. 用量响应

```
{"code":0,"data":{"total":108,"data":[
  {"requestId":"...","credit":3,"model":"...","client":"WorkBuddy",
   "requestTime":"2026-11-14 12:23:49","agentPurpose":"conversation",
   "appType":"...","subscribeType":"...","input":"<用户 prompt 原文>","inputTrunc":"..."}]}}
```

- 请求体：`{startTime:"YYYY-MM-DD 00:00:00", endTime:"YYYY-MM-DD 23:59:59", pageNum:1, pageSize:N}`。
- **计量单位是 credit，不是 token**。这个接口没有 input/output token 计数。
  想要 token 维度只能走本地会话文件（上游 `token_stats`），二者不能混算。
- 行里的 `input` / `inputTrunc` 是**用户 prompt 原文**。worker 一行都不读，
  返回值里只有 `credit` / `model` / `requestTime` / `requestId`。
- `credit` 为负的行不计（与上游同判据）。

## 4. 本次没有验证的

| 项目 | 状态 |
| --- | --- |
| 额度到期时间 | 响应里没有这个字段 → 界面显示未知 |
| 分页取全量历史 | 只取第一页（`pageSize=3000`），超出时标 `truncated` |
| 国际版 refresh token | 仍然关闭（与 PR #72 一致） |
| 企业 / 团队计费口径 | 未测 |
| 额度变化的服务端推送 | 不存在，只能轮询 |

## 5. 代码位置

- 适配器：`account-worker/src/quota/workbuddy.rs`
- 看板组装：`account-worker/src/quota/mod.rs`
- 缓存表：`quota_snapshots`（迁移 v14）
