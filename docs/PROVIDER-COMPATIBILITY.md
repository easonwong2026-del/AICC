# 服务商兼容性矩阵

调查日期：2026-10-20。本机 Magpie **0.1.1296**（`v0.1.1296`，2026-10-20 发布），网关 `127.0.0.1:3425`。

判定口径：

- **已验证**：本机 Magpie 实际返回该服务商额度，AICC 采集器/Widget/APK 用真实数据跑通。
- **部分验证**：只在 Magpie 源码或社区插件里确认了额度实现，本机没有登录态，没有取到真实额度行。
- **未验证**：只有认证或模型能力，没查到额度输出。
- AICC 侧一律走同一套通用模型；「AICC 支持」列写的是**展示能力**，不代表该服务商已在本机验证。

## 一、契约来源

| 项目 | 证据 |
| --- | --- |
| 额度结构 | [`internal/provider/quotas.go`](https://github.com/yetone/magpie/blob/main/internal/provider/quotas.go)：`Quota`（provider/name/kind/plan/user/windows/balance/balanceSplit/error/asOf/readDeferred/readAt/until/renew/resets/lastServedAt/last/from/alias/seat）、`QuotaSpan`（name/used/remaining/resetsAt/display/amount/limit/unit/pool/unlimited） |
| 路由 | [`internal/gateway/gateway.go`](https://github.com/yetone/magpie/blob/main/internal/gateway/gateway.go)：`GET /v1/magpie/quotas`、`GET /v1/magpie/quotas/history`、`GET /v1/magpie/quotas/cards`、`POST /v1/magpie/quotas/refresh` |
| 缓存读 | `?kept=1` 走 `provider.KeptQuotaReport`，不请求任何供应商，也不等待读取 |
| 刷新语义 | `?provider=<id>&user=<user>` 可选；同一卡片 30 秒内最多重读一次；未知名称为空操作，返回 200 + 当前列表 |
| 插件目录 | [`magpie-community/plugins/registry.json`](https://github.com/magpie-community/plugins/blob/main/registry.json)，共 32 个条目，其中 24 个声明了 provider id |

## 二、额度类型

| `kind` | 含义 | AICC 展示规则 |
| --- | --- | --- |
| `subscription` | 登录账号的订阅窗口（数量不限，窗口名由服务商决定） | 每个窗口一张条目：优先 `display`，其次剩余百分比；无数据不伪造 0% |
| `plan` | 用 Key 购买的套餐/积分计划（含 `until`/`renew`） | 同订阅窗口，另保留套餐名与到期/续费语义 |
| `balance` | API Key 或账户余额（`balance` + `balanceSplit`） | 直接显示金额与拆分，**绝不**换算成百分比 |

## 三、逐项矩阵

| 服务商 | Provider ID | 来源 | 额度能力（源码证据） | 接口结果 | 失败情况 | AICC 支持 |
| --- | --- | --- | --- | --- | --- | --- |
| Codex | `codex` | 内置 | 订阅：`5 hours` / `7 days`；`resets`（重置次数）；`until` | **已验证**：本机 1 个 Team 账号，2 个窗口 + 2 次重置 | 未登录时整行缺失；token 失效时 `error` 行 | 已实现 |
| WorkBuddy | `workbuddy`、`workbuddy-ai` | 社区插件 `@magpie-community/opencode-workbuddy-auth` | 订阅：`Credits` 窗口，带 `display`/`amount`/`limit`/`unit=credits` | **已验证**：本机 3 个账号，`2.27 / 4723` 等真实文本 | 插件未登录时无该行 | 已实现 |
| Antigravity | `antigravity` | 内置 | 订阅：按池拆分的多窗口（`pool`=Gemini / Claude & GPT），窗口名随刷新变化（`Gemini 3.8 Flash (High)`、`7 days`、`5 hours`） | **已验证**：本机 1 个 Google AI Pro 账号，一次刷新出现 7 个窗口（含重名窗口） | 上游 429 时该行 `error` | 已实现 |
| Kiro | `kiro` | 社区插件 `@magpie-community/opencode-kiro-auth` | 订阅（`internal/provider/kiro*.go`） | **部分验证**：插件与 provider 源码存在额度实现；本机未登录 | 未登录 → 无行 | 已实现，待验证 |
| ZCode / GLM Coding Plan | `zcode` | 社区插件 `@magpie-community/opencode-zcode-auth` | 套餐（`zcode_team.go`、`planquota.go`）：GLM 席位、Start Plan | **部分验证**：源码有套餐与席位额度；本机未登录 | 席位与 Key 同源时会合并为一张卡（`seat`） | 已实现，待验证 |
| MiniMax Code | `minimax-code`、`minimax-code-global` | 社区插件 `@magpie-community/opencode-minimax-auth` | 订阅/签到额度（`minimax_checkin.go`） | **部分验证**：源码有签到额度；本机未登录 | 未登录 → 无行 | 已实现，待验证 |
| Command Code | `commandcode-plan` | 社区插件 `@magpie-community/opencode-commandcode-auth` | 套餐 + Key 积分（`commandcode_plan.go`、`commandcode_key_credits_test.go`） | **部分验证**：源码有套餐与积分；本机未配置 | 无 Key → 无行 | 已实现，待验证 |
| API Key 余额类 | 各服务商 | 内置 | `balance` + `balanceSplit`（`balance.go`、`key_allowance.go`） | **部分验证**：源码与结构体确认；本机没有配置带余额的 Key | 上游失败 → `error`，`text` 为 `—` | 已实现，待验证 |
| Qoder | `qoder`、`qoder-cn` | 社区插件 | 订阅（插件目录） | **未验证** | — | 已实现（通用） |
| Devin | `devin` | 社区插件 | 认证/模型 | **未验证** | — | 已实现（通用） |
| Zed | `zed` | 社区插件 | 订阅（Zed plan） | **未验证** | — | 已实现（通用） |
| Cursor | `cursor` | 社区插件 | 认证/模型 | **未验证** | — | 已实现（通用） |
| Grok | `grok` | 社区插件 | 认证/模型 | **未验证** | — | 已实现（通用） |
| Factory | `factory` | 社区插件 | 认证/模型 | **未验证** | — | 已实现（通用） |
| Cline | `cline` | 社区插件 | 认证/模型 | **未验证** | — | 已实现（通用） |
| Trae | `trae-cn`、`trae-global` | 社区插件 | 认证/模型 | **未验证** | — | 已实现（通用） |
| Kimi Work | `kimi-work` | 社区插件 | 会员共享用量 | **未验证** | — | 已实现（通用） |
| OpenCode Zen Free | `opencode-zen-free` | 社区插件 | 免费模型（无额度语义） | **未验证** | — | 已实现（无额度时显示「无额度」） |
| MiMo | `mimo-app` | 社区插件 | 认证/模型 | **未验证** | — | 已实现（通用） |
| AutoClaw | `autoclaw`、`autoclaw-cn` | 社区插件 | 认证/模型 | **未验证** | — | 已实现（通用） |
| Open Design / Raccoon | `opendesign`、`raccoon` | 社区插件 | 认证/模型 | **未验证** | — | 已实现（通用） |
| 其他未知 Provider | 任意 | 内置或社区 | 未知 | 未验证 | — | 已实现：按 id 生成名称，不丢弃 |

> **不要**把本表当作「所有社区插件都已验证额度」。除 Codex、WorkBuddy、Antigravity 外，本轮都没有真实额度行可对照；
> 它们的显示能力来自同一套通用模型，而非逐家适配。

## 四、AICC 不做什么

- 不读取任何服务商的凭据文件、Cookie、token 或 `providers.json`。
- 不实现供应商登录、账号切换、额度重置或模型路由。
- 不在 Magpie 没有返回额度时编造 0%、100% 或「剩余 X 天」。
- 不按 provider id 白名单过滤；新增服务商只要 Magpie 返回 `subscription`/`plan`/`balance` 行就会进入展示流程。

## 五、复现验证

```bash
# 本机真实额度（不请求上游）
curl -s 'http://127.0.0.1:3425/v1/magpie/quotas?kept=1' | python3 -m json.tool | head -40

# 全量刷新（每张卡 30 秒内最多一次）
curl -s -X POST 'http://127.0.0.1:3425/v1/magpie/quotas/refresh' | python3 -m json.tool | head -20

# AICC 采集器
python3 -c "from collectors import magpie; s=magpie.collect(); print(s['state'], s['providers'], len(s['accounts']))"
```
