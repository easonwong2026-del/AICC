# PR-C 范围与现状（Qoder 账号闭环 + WorkBuddy 奖励）

本文只做**接口调查与需求收口**，不改代码。所有结论都带证据位置，便于复核。

## 一、现状：数据库里真实存在什么

只读查询 `~/Library/Application Support/AICC-Dashboard/accounts/accounts.sqlite`（2026-11-16 17:20）：

| 表 | 内容 |
| --- | --- |
| `qoder_accounts` | 4 条：`qoder_global` ×2、`qoder_cn` ×2 |
| `qoder_credit_runs` | 4 条，全部来自 CN 账号 |
| `qoder_profiles` | 1 条 |
| `qoder_authorizations` | **空**（当前 Qoder 凭据不走这张表） |

四条账号的实际状态：

| edition | nickname | email | credits_auto | auth_epoch |
| --- | --- | --- | --- | --- |
| qoder_global | `easonwong2026@gmail.com` | 空 | 0 | **0** |
| qoder_global | `rakliang2026@gmail.com` | 空 | 0 | **0** |
| qoder_cn | `01a1d9e6-c126-…`（uid） | 空 | 1 | 2 |
| qoder_cn | `nick3871551154` | 空 | 1 | 2 |

读法：

1. **两个 Global 账号从没完成过认证**：`auth_epoch = 0`，昵称直接是邮箱（`qoder_identity` 解析失败后的兜底），额度自动领取为 0。这就是之前说的"两个异常账号"。
2. **两个 CN 账号是活的**：`auth_epoch = 2`、自动领取开启，且 `qoder_credit_runs` 有成功记录：

```
qa_2f4076cb9818  ok       100 credits  http 200  auth_source=own_browser  manual  2026-11-15T14:06:46Z
qa_625c109ab9a7  already  100 credits            auth_source=own_browser  auto    2026-11-15T14:07:26Z
qa_2f4076cb9818  already  100 credits                     own_browser     auto    2026-11-15T16:00:30Z
qa_625c109ab9a7  already  100 credits                     own_browser     auto    2026-11-15T16:00:30Z
```

也就是说 **C2（Credits 查询与领取核验）在 CN 侧已经有真实成功证据**；Global 侧没有任何一次运行记录。

## 二、代码里已经有的能力

`account-worker` 现有 RPC（`handlers.rs`）：

```
qoder.accounts.list / delete
qoder.auth.start / poll / cancel
qoder.auth.pat.save / check / remove
qoder.credits.status / claim / runs / auto.{status,setEnabled,tick}
qoder.editions.detect
qoder.profiles.*  （扫描/导入/inspect/capture/recover；切换已永久关闭）
```

成长中心（`growth.*`）目前是**只读**：`growth.scan` 拉取 tasks / profile / energy / streak / heatmap / redeem summary / lottery chances / buddy quota / travel status，`growth.auto.*` 只做扫描调度。
`lottery_chances`、`redeem` 的 7/14/28 天档位都只是**读出来展示**，没有任何"抽奖"或"兑换"的写接口。

## 三、按你的优先级拆解

| 项 | 现状 | 本轮要做 |
| --- | --- | --- |
| C1 Qoder 账号导入/登录/状态闭环 | CN 已闭环；Global 两个账号导入但未认证 | 逐条定位 Global 认证失败点（登录方式？区域？回调？），修到能拿到 profile 与额度 |
| C2 Credits 查询与领取 | CN 有真实成功记录；Global 无记录 | Global 认证打通后补一次真实核验 |
| C3 WorkBuddy 抽奖 | 只有 `lottery_chances` 只读字段 | **先取证**：抽奖请求的真实 URL / 方法 / 请求体 / 响应 / 频率限制 |
| C4 7/14/28 天连登奖励兑换 | 只有 `redeem.tier_{7,14,28}d_status` 只读 | **先取证**：兑换请求的真实形态与前置条件 |
| C5 补签卡 | 只读 `makeup_cards.balance/max` | 取证后才能谈操作保护 |

C3/C4/C5 都涉及**写**操作（消耗次数、改变账号状态），在拿到真实请求证据前不写一行实现代码 —— 靠页面按钮猜参数会做出"看起来能跑、实际扣错东西"的功能。

## 四、C3/C4 取证方案（需要你选一个）

1. **抓真实请求（推荐）**：在 Mac 上用 mitmproxy/Charles 抓一次你手动点"抽奖 / 领取连登奖励"的请求，我只提取 URL、方法、body 结构、响应码与关键字段（不保存 cookie、不保存任何账号凭据）。
   - 需要你同意在本机装/跑代理并信任其根证书；
   - 只需要一次成功 + 一次失败（例如次数用完）的样本。
2. **由我先做只读侦察**：用现有 `growth.scan` 的同一套鉴权，访问已知的只读接口（`/redeem/summary`、`/lottery/chances`），确认字段与状态机，写接口仍然等证据。
   - 不需要代理，但**无法**得到写接口的请求形态。

## 五、需要你决定的三件事

1. **C1 先修哪一个 Global 账号？** 建议先 `easonwong2026@gmail.com`（两条都 `auth_epoch=0`，先修一条能形成对比基线）。
2. **Global 账号当初是怎么导入的？** 是浏览器授权没走完，还是走了 PAT？这决定是修回调还是修 PAT 校验 —— 如果你记得当时的操作，能省掉一轮猜测。
3. **C3/C4 取证走方案 1 还是 2？** 方案 1 我能拿到完整证据但需要你操作一次；方案 2 只能确认只读侧。

## 六、不做什么

- 不同时改凭据、桥接、成长中心三个模块（PR-B 刚收口，先稳住）。
- 不为了凑功能去猜写接口参数。
- 不动已完成的 CRED-5 凭据系统与墨屏 UI。
