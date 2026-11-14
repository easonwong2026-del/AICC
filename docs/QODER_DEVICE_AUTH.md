# Qoder 设备授权（PR #73）

不装 Qoder 客户端，也能用浏览器给 AICC 授权一个 **Qoder 平台账号**。
CN 与 International 是**两个独立产品**：域名、client id、钥匙串键全部按版本隔离，
同一 UID 落在两个站是两行账号（`qoder_accounts` 的唯一键是 `(edition, uid)`）。

协议照抄客户端自己的 CLI（`@qoder-ai/qodercli` / `@qodercn-ai/qoderclicn`），
下表是我们实际实现的形态。

## 1. 站点参数（唯一事实来源：`providers/qoder_edition.rs`）

| 项 | Qoder CN | Qoder International |
| --- | --- | --- |
| 登录页 origin | `https://qoder.cn` | `https://qoder.com` |
| openapi（轮询 / userinfo） | `https://openapi.qoder.com.cn` | `https://openapi.qoder.sh` |
| api（网关，#74 用） | `https://gateway.qoder.com.cn` | `https://api3.qoder.sh` |
| device 授权 client id | `e883ade2-e6e3-4d6d-adf7-f92ceff5fdcb` | `732aef47-9cf2-46a2-95fe-4cebb5d0d1fa` |
| `redirect_uri` | 空（CLI 不发） | 空（CLI 不发） |

`edition` 只接受 `qoder_cn` / `qoder_global`；含糊的 `qoder` 一律拒绝，
**不回落**到任何一个站（`QoderEdition::parse` 对 `"qoder"` 返回 `None`）。

## 2. 流程

### 2.1 发起（不联网）

```
verifier   = base64url(random 64B)              # 86 字符
challenge  = base64url(sha256(verifier))        # 43 字符，challenge_method=S256
nonce      = uuid v4
machine_id = uuid v4

GET(浏览器) {web}/device/selectAccounts
    ?challenge=<challenge>&challenge_method=S256
    &nonce=<nonce>&machine_id=<machine_id>&client_id=<site client id>
```

参数全是 base64url / uuid，天然 URL 安全，不需要额外编码；两站都不带 `redirect_uri`。
这一步只登记内存会话，所以助手离线时也能把登录页开出来。

### 2.2 轮询

```
GET {openapi}/api/v1/deviceToken/poll?nonce=<nonce>&verifier=<verifier>&challenge_method=S256
Accept: application/json
```

- **只有 HTTP 200 且 `token` 非空**才算完成（`device_token_payload`）。
- 其它情况——4xx、`200` 但没有 token、网络抖动（status 0）——都按"还没点授权"处理，
  继续等；这与 CLI 的 `catch {} // a hiccup: ask again` 一致。上限是 15 分钟 TTL。
- 响应形态：`{token, refresh_token, user_id, expires_at|expires_in}`（顶层）；
  `data` 包裹只是兼容分支。

### 2.3 身份

```
GET {openapi}/api/v1/userinfo
Authorization: Bearer <device token>
```

- uid 优先取轮询响应的 `user_id`，缺失才回落到 userinfo 的 `id / user_id / uid`；
  两边都没有就**不许入库**（`profile_missing`）。
- `name` / `nickname` / `display_name` 取一个用；`email` / `mail` 同理。
- userinfo 挂了但轮询有 uid：仍然入库，只是没有昵称与邮箱。

### 2.4 落库

| 东西 | 去哪 |
| --- | --- |
| device token | 钥匙串 `qoder/<account_id>/own/access` |
| refresh token | 钥匙串 `qoder/<account_id>/own/refresh`（只存不用） |
| 账号身份 | `qoder_accounts`（`edition` + `uid`） |
| 授权来源 | `qoder_authorizations`，`source=own_browser`、`secret_ref=keychain:qoder/…/own/access`、`fingerprint`、`token_expires_at` |

顺序：先写钥匙串（唯一需要显式回滚的一步），再写库；库写失败就把旧凭据恢复回去，
不留"钥匙串里有令牌但没有授权行"的半套状态。SQLite 里**没有任何令牌列**。

## 3. 会话生命周期

与 WorkBuddy 浏览器登录同一套纪律（#72 的教训）：

| 事件 | 行为 |
| --- | --- |
| 提交门禁 | `begin_commit` 原子把 `Pending` 抢成 `Committing` 并返回会话里的 `edition`；抢不到（取消 / 过期 / 已结束）就**拒绝落库**，没有默认值兜底 |
| 取消 | 只对 `Pending` 生效；已进入 `Committing` 的取消落空，不会出现"界面已取消但账号已写入" |
| TTL | 15 分钟（与 CLI 一致）；等待期间过期由门禁判 `Expired` |
| 终态 | `complete` / `fail` 只覆盖 `Pending` / `Committing`，不会把已取消的会话"抢救"回来 |

## 4. PAT 备用授权（同一 PR 第二轮）

浏览器授权依赖 Qoder 的登录页与回调，遇到 SSO / 网络策略就走不通。PAT 是第二条路：
用户在**自己站点**的账户页生成一张长期令牌，粘进 AICC。

| 项 | 值 |
| --- | --- |
| 生成入口（CN） | `https://qoder.cn/account/integrations` |
| 生成入口（International） | `https://qoder.com/account/integrations` |
| 官方文档 | <https://docs.qoder.com/cli/authentication> |
| 验身份 | `GET {openapi}/api/v1/userinfo`（`Authorization: Bearer <PAT>`） |
| 钥匙串键 | `qoder/<account_id>/pat/token` |
| 授权行 | `qoder_authorizations`，`source = own_pat` |
| 刷新 | **不做**。PAT 失效只能由用户重新生成 |

### 4.1 两条来源互不覆盖

`(account_id, source)` 唯一，所以 PAT 与 `own_browser` 是**两行**：

- 删掉 PAT 不动浏览器授权，反之亦然；
- 浏览器 token 过期不会把 PAT 标成 expired；
- 同一账号的 Magpie 关联（`source = magpie`，无凭据）完全独立。

`own_pat` 是 v16 新加的取值。SQLite 改不了 `CHECK`，所以 v16 **重建了
`qoder_authorizations`**（先建新表、搬数据、再改名）。迁移是 Rust 步骤而不是纯 SQL：
两步都先检测再动手，重复执行必须是空操作。

### 4.2 "保存成功" ≠ "登录完成"

落库前会真打一次 `userinfo`，拿不到 uid 就**不入库**（`profile_missing`）。
但 `userinfo` 通得过只证明"这张令牌属于这个账号"，**不证明**它能调用 Credits 接口。
所以 RPC 返回体把两件事分开：

```json
{ "saved": true, "identity": "verified", "credits": "unknown",
  "warning": "PAT 已保存并通过身份验证；它是否能领取 Credits 需要单独检测" }
```

界面同样只说"已保存并验证身份"，后续权限由额度页的检测接口回答。

### 4.3 失败类型与下一步

| `error.kind` | 含义 | 界面下一步 |
| --- | --- | --- |
| `malformed` | 空 / 太短 / 带空格换行 | 重新复制 |
| `rejected` | 401：撤销、过期，或**不属于这个站点** | 确认站点后重新生成 |
| `forbidden` | 403：权限范围不够 | 勾选账号读取权限后重新生成 |
| `profile_missing` | 200 但没有 uid | 换浏览器授权 |
| `network` | 超时 / 连接失败 | 重试，**令牌不用换** |

**不做任何跨站回落**：CN 的 PAT 发到 International 只会 401，换域名重试等于把令牌
发到错误的站点。

### 4.4 凭据生命周期的两个方向都要能收拾干净

| 时刻 | 顺序 | 失败时 |
| --- | --- | --- |
| 保存 | 账号 → **读旧凭据** → 钥匙串 → 授权行 | 三个失败点走同一条清理：读旧凭据失败（`Snapshot::capture`）、钥匙串写失败、授权行写失败，都把「本次新建且没有任何来源」的账号删掉；写失败另加还原钥匙串快照 |
| 删除 | **钥匙串 → 授权行** | 钥匙串删不掉：**保留授权行并报错**（`secret_delete_failed`），让用户重试；下次进来凭据已是 `NotFound` 就视为删干净，继续删行 |

反过来（先删库再删钥匙串）会留下一份"库里看不见、用户也删不掉"的孤儿凭据。
写入方向反过来（先写库再写钥匙串）会留下一个没有授权的空账号 —— 两种都要避免。

### 4.4 本轮不做

PAT 不参与自动刷新、不写进任何配置或日志、不在 Android / Widget 侧提供输入入口。
`qoder_profiles` 那条本机客户端快照线仍然只读。

### 4.5 状态更新要核对**生命周期 + 授权版本**

一次 `userinfo` 要几秒，期间用户可能已经换了一份新 PAT。旧结果回来把新授权标成
`invalid`，用户看到的是"刚存就失效"，而真正失效的是那张已被替换的旧令牌。

更隐蔽的一种：这段时间里账号可能被**删掉又用同一个 UID 登录**。账号 id 是
`(edition, uid)` 的确定值，重建后一模一样；新行的 `auth_epoch` 从 0 开始，
与旧请求记下的值相同——只比 `auth_epoch` 会判定"没变化"，把新账号的凭据写脏。

所以状态写入核验 `QoderLifecycle = (generation, auth_epoch)`（v19）：
`set_qoder_auth_status_if_current(..., &lifecycle)`，一条 `UPDATE … WHERE EXISTS`
同时要求两者相等，对不上就什么都不写。

另外 **403 不改状态**：它是权限范围问题，把令牌标成 `invalid` 会让用户白去重新生成
一张本来没问题的令牌。

## 5. 本阶段不做（明确排除）

| 不做 | 原因 |
| --- | --- |
| `/api/v1/me/jobToken` 交换 | 换出来的是 chat token，AICC 不聊天；device token 才是 #74 领 Credits 要用的凭据 |
| token 刷新 | 只存不用。刷新涉及轮换与并发互斥，单独评审 |
| 客户端登录态读写 | `qoder_profiles` 那条线（#70）只读，与本流程互不影响 |
| VPC / 企业专属域 | 只在 `qoder.cn` / `qoder.com` 两个公有站点上实现；企业域需要额外探测规则 |
| QwenWork | 第三个站点，不在 #73 范围 |

## 6. 验收

1. CN 与 International 各自发起，登录页分别落在 `qoder.cn` / `qoder.com`，client id 不同。
2. 同一 UID 在两个站 → 两行账号、两套钥匙串键，互不覆盖。
3. 取消 / 过期 / 不存在的会话，轮询返回终态且**不写库、不发网络**。
4. 未完成授权前 `qoder.accounts.list` 不出现新账号。
5. 令牌不出现在 SQLite、RPC 响应、日志里；对外只有 `hasSecret` 与指纹。
6. PAT：保存后 `qoder.accounts.list` 里该账号出现 `own_pat` 一行；删除后只剩原来源；
   错误 PAT（跨站 / 被撤销）返回 `rejected` 且**不写库、不写钥匙串**。

**待真机**：真实账号的浏览器换票、`userinfo` 实际字段、`token_expires_at` 的实际形态、
真实 PAT 的 `userinfo` 权限范围。
在拿到真实响应之前，本模块的任何"成功"都只代表本地状态机正确，不代表服务端已验证。
