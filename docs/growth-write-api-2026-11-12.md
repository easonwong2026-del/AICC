# 成长中心写接口实测记录（2026-11-12）

PR #68 新增的两个写操作 —— 接单与领奖 —— 的真实形态。**下面每一条都来自实测**，
标注了证据等级；没验证的地方也写清楚，不留"大概是这样"。

## 一、探测方法

写操作不能随便试，所以先用**零副作用探针**确认路由存在，再做真实领取（用户授权）：

| 探针 | 请求 | 副作用 |
|---|---|---|
| 空数组接单 | `POST /tasks/accept {"task_codes": []}` | 无（没有任务被接） |
| 幽灵领奖 | `POST /tasks/__aicc_probe__/claim {}` | 无（不存在的 code 领不到任何奖励） |
| 幽灵接单 | `POST /tasks/accept {"task_codes": ["__aicc_probe__"]}` | 无（同上） |
| 真实领取 | 对 2 个 `completed` 任务各领 1 次 | 各到账 100 积分 + 5 能量（用户授权） |
| 重复领取 | 对已领过的任务再领 1 次 | 无（幂等回执） |

账号：`1d32cc97c9a3ec26`（卡里，探针）、`d3de2ea3626c2b44`（高亮）、
`ab809e1e7cf247c6`（SAMSUNG），均为 Magpie 授权。

## 二、接单 `POST /v2/activity/growth/tasks/accept`

### 请求

```http
POST /v2/activity/growth/tasks/accept
Authorization: Bearer <access>
X-User-Id: <uid>
X-Domain: www.workbuddy.cn
Content-Type: application/json

{"task_codes": ["a", "b"]}
```

- 字段是**复数** `task_codes`。空数组时服务端回的错就是最好的证明：
  `400 {"code":400,"msg":"task_codes is empty"}` —— 它读的就是这个字段名。
- 每批 20 个（沿用上游实现的分片口径，也是我们自己在客户端截断的上限）。

### 响应（证据 A：实测）

**幽灵任务**（HTTP 200，整体 `code: 0`，但这一条是 error）：

```json
{
  "code": 0,
  "msg": "OK",
  "requestId": "83ce9b36-715e-4214-8d9f-6c4d547a1e4b",
  "data": {
    "results": [
      {"task_code": "__aicc_probe__", "status": "error", "message": "task not found"}
    ]
  }
}
```

**这条最关键**：HTTP 200 + `code 0` **不代表接单成功**。真正的结果在
`data.results[]` 里逐条给 `status: "ok" | "error"`，`message` 是服务端原话
（例如 `prerequisite not met: first_buddy (no buddy instance found)`）。

成功条目的形态（`status: "ok"`）**未单独实测**——我们只在真实接单时才会看到，
而真实接单由用户在 AICC 里开开关触发。证据等级：**交叉参考 88lin 的
`signin.py`（从桌面端 H5 的 growthSpace chunk 逆向）+ 我们自己实测到的
error 条目结构**，两者共用同一个 `results[]` 数组。

`results` 缺失时（例如整体 4xx）实现按"未知"处理，不当成功。

## 三、领奖 `POST /v2/activity/growth/tasks/{task_code}/claim`

### 请求

```http
POST /v2/activity/growth/tasks/Expert_lighthouse/claim
Content-Type: application/json

{}
```

请求体是空对象；`task_code` 直接进路径（我们只放行 `[A-Za-z0-9_-.]+`）。

### 响应（证据 A：实测，三种情形都拿到了）

**首次领取成功**：

```json
{
  "code": 0, "msg": "OK", "requestId": "2bccd2b7-...",
  "data": {
    "already_claimed": false,
    "credit": 100,
    "energy": 5,
    "reward_popup_button_text": "收下奖励，前往Lighthouse",
    "reward_popup_redirect_url": "https://cloud.tencent.com/act/pro/workbuddy-lighthouse"
  }
}
```

**重复领取（幂等）**：

```json
{"code": 0, "msg": "OK", "data": {"already_claimed": true, "credit": 0, "energy": 0}}
```

**未完成 / 不存在的任务**：

```json
{"code": 400, "msg": "task not completed"}
```

⚠️ 注意最后这个：对一个**根本不存在的 task_code** 也返回 `task not completed`，
不是 `not found`。说明服务端只判断"这个 code 当前有没有可领的完成状态"。
所以 `task not completed` 不能当"任务不存在"，只能当"现在别领"。

### 服务端状态变化（实测）

| 时刻 | `accept_status` |
|---|---|
| 领取前 | `completed` |
| 领取后 | `claimed` |

重复领取**不会**把状态改回去，也不会重复发奖（`credit: 0`, `energy: 0`）。

## 四、证据等级汇总

| 结论 | 等级 | 依据 |
|---|---|---|
| accept 路由存在、方法是 POST、认证走 Bearer | A 实测 | 空数组返回 `400 task_codes is empty`（不是 404/405/401） |
| accept 请求字段名是 `task_codes`（复数） | A 实测 | 服务端错误文案自己给出 |
| accept 结果在 `data.results[]`，逐条 `status`/`message` | A 实测 | 幽灵任务拿到完整 error 条目 |
| accept 成功条目形态 | B 交叉 | 88lin 逆向 + 同结构 error 条目 |
| claim 路由存在、方法是 POST、body 为 `{}` | A 实测 | 幽灵任务返回 `400 task not completed` |
| claim 成功响应含 `already_claimed`/`credit`/`energy` | A 实测 | 两次真实领取 |
| claim 幂等：重复领 `already_claimed: true` 且不发奖 | A 实测 | 同一任务连领两次 |
| claim 成功后 `accept_status` 变 `claimed` | A 实测 | 领后重扫 |
| 领奖实发数值以响应为准 | A 实测 | 本次列表 100 积分 / 5 能量，实发也是 100 / 5；实现仍只认响应里的数值，不拿列表兜底 |
| 429 / 5xx 的具体文案 | C 未测 | 未制造限流；实现按 `retryable` 分类处理，不重发 |

## 五、实现上的对应

| 证据 | 代码 |
|---|---|
| HTTP 200 不等于成功 | `rewards::parse_accept` 只看 `data.results[]` |
| `already_claimed` 是幂等回执 | `rewards::parse_claim` → `ClaimOutcome::AlreadyClaimed` |
| `task not completed` 是"现在别领" | `ClaimOutcome::Rejected`，记 rejected 不算失败 |
| 领取后状态必须复核 | `manager` 每次 claim 后重新拉 `/tasks`，只有 `claimed` 才计入到账 |
| 超时可能已经生效 | claim 超时记 `unknown` 并回查，**不重发** |
| 一次最多接 20 个 | `api::ACCEPT_BATCH` |
