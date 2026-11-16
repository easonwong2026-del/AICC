# PR-C 范围与现状（Qoder 账号闭环 + WorkBuddy 奖励）

本文只做**接口调查与需求收口**，不改代码。所有结论都带证据位置，便于复核。

## 一、现状：数据库里真实存在什么

只读查询 `~/Library/Application Support/AICC-Dashboard/accounts/accounts.sqlite`（2026-11-16 18:40）。

### 1.1 账号与授权

`qoder_accounts` 4 条，`qoder_authorizations` **5 条**（不是空表 —— 早前那版文档写错了，原因见 1.4）：

| account | edition | auth_epoch | 授权来源 |
| --- | --- | --- | --- |
| `qa_2f4076cb9818` | qoder_cn | 2 | `magpie`（qoder-cn） + **`own_browser` → `localjson:v1:d6667035…`**（valid，auth_version 1） |
| `qa_625c109ab9a7` | qoder_cn | 2 | **`own_browser` → `localjson:v1:6e0c7342…`**（valid，auth_version 1） |
| `qa_ff0e54f3e8b0` | qoder_global | **0** | **只有 `magpie`**（source_key `qoder`），`secret_ref` 为 NULL |
| `qa_f74f2617d3d9` | qoder_global | **0** | **只有 `magpie`**（source_key `qoder#6j8tuq`），`secret_ref` 为 NULL |

两个 Global 账号的真实状态因此不是"浏览器授权没走完"，而是——**AICC 从来没为它们建立过自有凭据**：
它们只是从 Magpie 导入的关联（`magpie` 行），AICC 手里没有可发请求的令牌。
`credits::resolve_token` 遇到这种账号会直接返回「这个账号只有 Magpie 关联，没有 AICC 自己的凭据」，
一个请求都不发（`account-worker/src/credits.rs:120-190`）。

`auth_epoch = 0` **不能**单独用来判断"从未认证"：v16 迁移给这一列加的是
`NOT NULL DEFAULT 0`（`storage/qoder_accounts.rs:70`），老行全部是 0。
能说明问题的是上面的`来源`列：只有 `magpie` 行、没有 `own_browser` / `own_pat`。

### 1.2 历史 Credits 与今天的可用性

`qoder_credit_runs` 4 条，全部来自 CN 账号：

```
qa_2f4076cb9818  ok       100 credits  http 200  own_browser  manual  2026-11-15T14:06:46Z
qa_625c109ab9a7  already  100 credits            own_browser  auto    2026-11-15T14:07:26Z
qa_2f4076cb9818  already  100 credits                     own_browser  auto    2026-11-15T16:00:30Z
qa_625c109ab9a7  already  100 credits                     own_browser  auto    2026-11-15T16:00:30Z
```

这证明 **CN 侧的领取链路曾经真实成功过**；但"历史成功"不等于"今天可用"，
今天的可用性要看 1.3 的凭据引用是否仍然成立。Global 侧一条运行记录都没有。

### 1.3 凭据引用与迁移记录

`credential_migrations` 3 条，与 `secrets/v1/` 下 3 个 JSON 文件一一对应：

| 迁移 id | migrated_ref | 文件 |
| --- | --- | --- |
| `qoder:qa_2f4076cb9818:own_browser` | `localjson:v1:d6667035…` | 374 B |
| `qoder:qa_625c109ab9a7:own_browser` | `localjson:v1:6e0c7342…` | 374 B |
| `workbuddy:d69d4a7b1460b46d:own` | `localjson:v1:5078a797…` | 2422 B |

CN 两个账号的 `qoder_authorizations.secret_ref` 与迁移记录的 `migrated_ref` **一致**，
说明引用是自洽的，凭据文件也在盘上。这一层不需要"修"，只需要在真正跑一次领取时验证。

### 1.4 更正说明

上一版文档写了「`qoder_authorizations` 空表」和「两个 Global 从没完成过认证」，两处都不准确：

- 空表结论来自一条**列名写错的 SQL**（用了不存在的 `platform` 列），查询报错后输出为空，被当成了"没有行"。
  实际有 5 行。
- `auth_epoch = 0` 是 v16 迁移的默认值，不能当作"从未登录"的证据。

结论改为：**两个 Global 账号当前没有 AICC 自有授权，只有 Magpie 关联**；历史导入方式仍待核实
（Magpie 行存在，说明至少经过 Magpie；`Qoder.app` 客户端曾登录过第一个账号，但那与 AICC 的授权无关）。

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

## 四、C3/C4 取证：第三方参考 + 待核实

第三方项目 `magpie-workbuddy-tasks` 的 `engine/streak.mjs` 已经实现过这三项写操作：

| 功能 | 路径（第三方） | 请求体 |
| --- | --- | --- |
| 连登奖励兑换 | `POST /activity/growth/redeem` | `{"tier":"14d","client_token":"<uuid>"}` |
| 活动抽奖 | `POST /activity/growth/lottery/draw` | `{"client_token":"<uuid>"}` |
| 补签卡使用 | `POST /activity/growth/makeup-cards/use` | `{"target_date":"YYYY-MM-DD"}` |

**这是参考，不是结论。** AICC 自己的实测前缀是 `/v2/activity/growth`（`growth/api.rs:29` 里两个前缀都试过），
域名、前缀、当前协议是否与第三方一致，必须用本机账号的实际请求确认。

> 2026-11-16 更新：三项目的对照、证据等级与分类契约已整理到
> [`growth-rewards-protocol-2026-11-16.md`](growth-rewards-protocol-2026-11-16.md)（PR #95）。
> 该 PR 只做协议核验 + 模型修复 + 操作账本，**没有任何写请求**；下面的取证顺序仍然有效。

取证顺序（按你的决定）：

1. 先用**只读**接口确认抽奖次数、7/14/28 天档位、可领取状态（`/redeem/summary`、`/lottery/chances`），
   与第三方源码的字段名对照；
2. 再由你在浏览器里手动点一次真实操作，用开发者工具记录：路径、方法、脱敏后的 body 结构、响应码、奖励结果；
3. 若必须抓 HTTPS，才临时上代理，用完**立即移除代理证书**，不动 AICC 已完成的 TLS 验证；
4. 没有可领取的奖励时不强行测试写操作 —— 不为凑证据多消耗一次机会。

在拿到第 2 步的证据前，不实现任何写接口。

## 五、C1 执行计划（已定）

先修 `easonwong2026@gmail.com`（`qa_ff0e54f3e8b0`），第二个 Global 账号暂不动。

1. 跑一次 Qoder International **官方浏览器设备授权**（`qoder.auth.start` → 用户在浏览器确认 → `qoder.auth.poll`）；
2. 按结果定位：
   - `userinfo` 成功但账号/授权行没落库 → 查数据库提交与凭据存储；
   - 轮询一直不成功 → 查登录域名、设备授权与轮询响应；
   - 保存成功但 Credits 不可用 → 查 Credits 协议与权限（**当前 `capability(Global) = None`，
     即 International 根本没有适配器，见 `qoder_credits.rs:49`），这不算登录失败；
3. 浏览器授权确不可行时才退到 PAT。

不做：删除已有账号、重建账号库、改 CRED-5。

## 六、不做什么

- 不同时改凭据、桥接、成长中心三个模块（PR-B 刚收口，先稳住）。
- 不为了凑功能去猜写接口参数。
- 不动已完成的 CRED-5 凭据系统与墨屏 UI。
