# WorkBuddy 客户端账号切换（WB-SWITCH-1 / #112 Magpie 临时模式）

一句话：**把 AICC 里授权过的账号写进 WorkBuddy 客户端的登录态，必要时重启客户端；
失败按快照逐字节还原，并把"到底恢没恢复"如实告诉用户。**

两种授权，两种生命周期：

| 模式 | 写进客户端的令牌 | 谁续期 | 结束条件 |
| --- | --- | --- | --- |
| `own_refresh`（AICC 自有授权） | access + refresh | WorkBuddy 自己刷新 | 用户退出或切走 |
| `magpie_access_temporary`（Magpie 借用） | **只有** access | 没人续期 | access 过期 |

> 本文档是这两种模式的唯一口径。界面文案、RPC 返回、数据库字段都以这里为准。

---

## 1. 边界

| 能力 | 状态 | 原因 |
| --- | --- | --- |
| 国内版 + 自有授权（有 refresh） | **已接通** | 长期登录，客户端自己续期 |
| 国内版 + Magpie 关联（只有 access） | **已接通（临时）** | 借来的 access 必然过期，到期前必须恢复 |
| 国际版（`workbuddy_ai`） | **未开放** | 认证文件、客户端行为都没有实测证据 |
| 复制聊天会话 / 迁移 `main.sqlite` | **不做** | 只换"登录是谁"，不搬业务数据 |
| 接管 / 同步 Magpie 的 refresh | **不做** | 那是 Magpie 的凭据；两边各自刷新会互相踢下线 |
| 伪造活跃、模拟对话 | **不做** | 官方条款里属于取消资格情形 |

稳定原因码（Rust `workbuddy_switch::plan::reason`，界面按这个分支，不解析中文）：

| code | 含义 | 下一步 |
| --- | --- | --- |
| `ok` | 可以切 | 按钮 |
| `live` | 客户端当前登录的就是它 | 不显示按钮 |
| `unsupported_provider` | 国际版 | 暂不开放 |
| `client_not_installed` | 没找到客户端 | 先安装 |
| `no_credential` | 没有任何授权 | 重新授权 |
| `missing_refresh` | 自有授权缺 refresh | 重新用浏览器登录 |
| `magpie_unavailable` | Magpie 文件读不出来 | 确认 Magpie 在运行并已登录 |
| `magpie_source_missing` | 文件里没有这条来源键 | 重新导入 / 在 Magpie 重新登录 |
| `magpie_uid_mismatch` | 来源键对应的 UID 不是这个账号 | 重新导入 |
| `magpie_platform_mismatch` | 那条登录态不属于国内版 | — |
| `magpie_access_empty` | access 是空串 | 在 Magpie 重新登录 |
| `magpie_access_expired` | access 已过期 | 在 Magpie 重新登录 |
| `magpie_expiry_unknown` | 没写 `expires`，不猜 | — |
| `magpie_too_short` | 剩余有效期低于门槛 | 换账号 / 等 Magpie 刷新 |
| `temporary_pending` | 还有一次临时登录没恢复 | 先「恢复原账号」 |
| `live_changed` | 客户端当前登录已不是这一轮的目标 | 确认后再恢复 |

`plan::resolve()` 是**唯一**的可切换判定入口：`status` 用它给界面答案，`switch` 用它决定
实际写哪份令牌。界面显示 `magpie_access_temporary`、实际却用了自有授权，是这一层最不能犯的错。

### 来源选择与 Broker 一致

账号可以同时有 own / magpie 两份授权，`auth_preferred_source`（默认 `magpie`）决定优先用哪份。
顺序完全照抄 `auth::broker`：先首选、后另一份；首选不可用时用另一份并在
`reasonDetail` / `Plan.degraded` 里写明"首选不可用，已改用 X"。

**这意味着**：一个既有自有授权又有 Magpie 关联的账号，默认会被判成临时模式。
想长期登录，把设置里的首选来源改成 `own`（或直接不导入 Magpie 那份）。

---

## 2. 客户端认证文件（2026-11 实测）

```
~/Library/Application Support/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info
同目录 workbuddy-desktop.info.logged-out   ← 退出标记（存在即视为未登录）
```

```json
{
  "auth":    { "accessToken": "…", "refreshToken": "…", "tokenType": "Bearer",
               "domain": "www.workbuddy.cn", "expiresAt": 1797552874249, … },
  "account":  { "uid": "…", "nickname": {"$wbEncrypted":1, …}, "uin": "…", … },
  "accounts":    [ … ],   // 客户端自己的多账号列表：**只有身份字段，没有令牌**
  "allAccounts": [ … ]
}
```

三条要点：

1. **令牌只有当前账号那一份**（`auth.accessToken` / `auth.refreshToken`），
   所以"切换"必须重写 `auth`。
2. `nickname` / `phoneNumber` 是 AES-GCM 信封，密钥只在客户端运行时里。
   AICC **不解密、不覆盖**：目标账号如果已经在客户端列表里，原样复用那条账号对象。
3. AICC 写出去的是**明文** access（自有 OAuth 拿到的就是明文）。
   `live.tokenForm` 从 `plaintext` 变 `envelope` 是"客户端读进去并重新封装了"的强证据。

### 2.1 Access-only 的 `auth` 段（本 PR 新增）

```json
{
  "accessToken": "…",
  "tokenType": "Bearer",
  "domain": "www.codebuddy.cn",
  "lastRefreshTime": 1795…,
  "scope": "openid profile offline_access email",
  "notBeforePolicy": 0,
  "sessionState": "",
  "expiresAt": 1795…,
  "expiresIn": 2700
}
```

**没有** `refreshToken` / `refreshExpiresAt` / `refreshExpiresIn` —— 连空串都不写。

为什么单独实现（`build_access_only_auth`）而不复用上游 `build_auth_obj`：

- `build_auth_obj` 在缺 refresh 时会写 `"refreshToken": ""` 并补 `refreshExpiresAt`。
  "没有刷新令牌"和"刷新令牌是空串"是两件事：后者会让客户端去做一次必然失败的刷新，
  而且看起来像"这份登录可以续期"。
- 更要紧的是安全：如果只是"把 refresh 留空"而漏了某个字段，上一账号的 refresh
  就可能被带到目标账号上。独立构造函数让"没有 refresh 字段"成为**结构保证**，
  并有单测钉住（`access_only_auth_has_no_refresh_fields`、
  `temporary_switch_writes_access_only_and_never_leaks_the_old_refresh`）。

写文件仍走 `write_private()`：临时文件 `0600` → fsync → rename → fsync 目录。

---

## 3. 切换流程

```text
1. 预检    账号存在 / 国内版 / 客户端已安装 / 没有未完成切换 / 没有待还原的临时登录
2. 判定    plan::resolve → 模式(own|magpie) + 到期时间 + 剩余时间（Magpie 现读文件）
3. 取料    own：读库里的 access+refresh；magpie：**现读 Magpie 文件**，只取 access
4. 活跃取证 目标账号 growth /heatmap + /streak（只读）
5. 关客户端 osascript quit → 超时 pkill → 等进程消失（20s）
6. 快照    关掉之后再读认证文件，逐字节存到 <数据目录>/workbuddy-switch/<id>.json
7. journal phase=prepared（open_slot=1）
8. 写认证  复用客户端账号对象，替换 auth + account
9. 启动    open -a，等进程起来
10. 回读   至少观察 2.5s，最多 25s
11. 服务端探针 一次只读请求，记录 authAccepted
12. 收尾   phase=verified/unconfirmed，释放 open_slot；临时登录 restore_state=pending
```

### 校验三态

| 观察到 | 结论 | 动作 |
| --- | --- | --- |
| UID=目标 + 无退出标记 + 进程在跑（≥2.5s） | **confirmed**（`verified`） | 收尾，报成功 |
| UID≠目标，或出现退出标记 | **rejected** | **自动回滚**，报失败 + `restored` |
| 文件已是目标，但没确认到进程 | **unconfirmed** | **不回滚**，让用户确认 |

第三态的理由：冷启动慢是常态。把一次大概率成功的切换回滚掉，比"晚点再确认"更糟；
但也不能把"文件写对了"说成"客户端接受了"。

### 客户端成功 ≠ 服务端接受

回读只能证明"客户端停在这个 UID"。令牌是否被服务端接受由**一次只读探针**单独记录：

| `authAccepted` | 含义 |
| --- | --- |
| `accepted` | 探针拿到正常响应 |
| `rejected` | 401/403（明确的认证失败） |
| `unknown` | 网络/超时/判不出来 —— **"没问到"不是"被拒绝"** |

探针不写任何状态、不刷令牌、不改授权行（复用 `auth::probe`，与 `auth.workbuddy.status` 同一条路）。

---

## 4. 临时授权的有效期

| 常量 | 值 | 用途 |
| --- | --- | --- |
| `MIN_SWITCH_REMAINING_SECS` | **30 分钟** | 低于此值禁止临时切换 |
| `RESTORE_SAFETY_MARGIN_SECS` | **15 分钟** | 建议恢复截止 = 到期 − 15 分钟 |

两条都是**设计取舍，不是实测结论**，所以做成常量，后续按真机数据调整。

判定顺序（任何一步不满足就拒绝，原因码见 §1）：

1. access 为空 → `magpie_access_empty`
2. 没有 `expires` → `magpie_expiry_unknown`（不猜）
3. 已过期 → `magpie_access_expired`
4. 剩余 < 30 分钟 → `magpie_too_short`
5. 其余 → 允许，并把 `accessExpiresAt` / `remainingSeconds` / `safeRestoreUntil` 一并返回

切换时**再读一次** Magpie 文件：计划阶段到真正写文件之间可能已经刷新或退登。

---

## 5. 失败与恢复

### 5.1 自动回滚（切换过程中）

触发：启动失败、客户端把文件改回别的账号、客户端写退出标记。

1. 关掉可能已经起来的客户端；
2. 把快照**逐字节**写回（不重建，避免丢字段）；
3. 按切换前状态恢复退出标记；
4. 切换前在跑就按原状重新启动；
5. `phase=restored`（恢复成功）或 `failed`（恢复也失败）。

写入本身失败时文件一个字节都没改，走 `finish_without_write`：重启客户端、落 `restored`。

**错误里必须带 `restored`**：`true` = 登录态没变；`false` = 需要手动登录。

### 5.2 显式恢复

`workbuddy.client.restore { journalId?, force? }`：

- 不传 id 时按「未完成切换 → 待还原的临时登录 → 最近一次成功切换」挑一条。
- 先校验快照 sha256；被改动过就拒绝（`snapshot_corrupt`）。
- **客户端当前登录的既不是目标账号、也不是原账号**（用户手动换过）时默认拒绝
  （`live_changed`，带 `currentUid`）；界面确认后用 `force=true` 再调一次。
- 客户端因为 access 过期而登出（文件缺失/无 uid）时**不需要 force**：那正是要救的场景。
- 成功 → `restore_state=done`；失败 → 保持 `pending`，可重试。

### 5.3 崩溃恢复与两把"锁"

| 字段 | 管什么 | 释放时机 |
| --- | --- | --- |
| `open_slot`（唯一索引） | **切换正在执行**（prepared/written/launched） | 收尾时置 NULL |
| `restore_state=pending` | **临时授权还没还原** | 恢复成功（或切回原账号）→ `done` |

刻意分开：临时登录成功后 `open_slot` 立刻释放（不再并发阻塞），但 `restore_state`
一直是 `pending`，界面必须持续给「恢复原账号」。

同时只允许一条 `pending` 的临时记录；在它未还原时：
- 切回**那个原账号** → 允许（等价于恢复），旧记录标 `done`；
- 切别的账号 → 拒绝（`temporary_pending`），否则旧快照会被悄悄取代。

---

## 6. 接口

| 方法 | 参数 | 关键返回 |
| --- | --- | --- |
| `workbuddy.client.status` | 无 | `client` / `live`（含 `temporary`、`accessExpiresAt`、`remainingSeconds`）/ `candidates[]`（`switchable`、`reason`、`reasonDetail`、`mode`、`temporary`、`accessExpiresAt`、`remainingSeconds`）/ `pendingSwitch` / `temporaryRestore` / `restoreAvailable` / `lastSwitch` |
| `workbuddy.client.switch` | `accountId`、`restart`(默认 true)、`observeActivity`(默认 true)、`verifyServer`(默认 true) | `changed`、`phase`、`verified`、`mode`、`temporary`、`accessExpiresAt`、`remainingSeconds`、`safeRestoreUntil`、`authAccepted`、`activityBefore/After` |
| `workbuddy.client.restore` | `journalId?`、`force?` | `restored`、`uid`、`detail`、`clientRunning` |

- `live.uid` 是**唯一可信的"当前登录"**。
- `switch` 超时预算：关进程 20s + 校验 25s + 探针 + 两次活跃查询，Swift 侧给 180s。
- 错误码：`E_SWITCH=1008`（busy / pending / live_changed）；`E_AUTH`（凭据问题），
  分类看 `data.kind`（就是 §1 的原因码）。

---

## 7. 数据库（v25 + v26）

```sql
workbuddy_switch_journal(
  id, target_account_id, target_uid,
  previous_uid, previous_running, previous_marker,
  phase,            -- prepared/written/launched/verified/unconfirmed/failed/restored
  open_slot,        -- 1 = 切换在执行（唯一索引，至多一条）
  snapshot_path, snapshot_sha256, token_form, detail,
  -- v26
  auth_source,      -- own / magpie
  temporary,        -- 1 = 临时登录
  access_expires_at,-- RFC3339
  access_fingerprint, -- access 的 SHA-256 前 8 位（**不存令牌**）
  source_key,       -- Magpie 来源键
  restore_state,    -- pending / done
  auth_accepted,    -- accepted / rejected / unknown
  activity_before, activity_after,
  created_at, updated_at
)
```

快照默认在 `~/Library/Application Support/AICC-Dashboard/accounts/workbuddy-switch/`。
**不要手动删**：`restore` 依赖它。

---

## 8. 未验证项与风险（诚实清单）

| 项 | 现状 | 影响 |
| --- | --- | --- |
| 客户端遇到"没有 `refreshToken`"会怎么处理 | **没有实测证据** | 保守预期：用到 access 过期为止，之后可能被登出（这正是临时模式的定位） |
| 客户端会不会在 Access-only 下立刻尝试刷新 | 未知 | 若尝试，缺少 refresh 会失败；可能触发一次登出。**首次真机必须验证** |
| 明文令牌是否被所有版本接受 | 只有"回读确认"证据 | 首次切换若被拒会走回滚 |
| 服务端是否会因换设备/换 IP 让 access 提前失效 | 未知 | 到期时间只是乐观上界 |
| 账号不在客户端 `allAccounts` 里 | 只能拼最小账号对象 | 头像 / UIN 可能为空 |
| 客户端启动很慢（>25s） | 落 `unconfirmed`，不回滚 | 提示"打开客户端后刷新状态" |
| 切换会中断正在进行的对话 | 已知 | 二次确认里写明 |
| 写客户端目录权限 | 需要「App 管理」或「完全磁盘访问」 | 写入失败会保持原登录态 |
| 国际版 | 完全未接 | 显示"未开放" |

### 到期后会发生什么（预期，未实测）

- `live.uid` 可能仍然显示目标账号（客户端不主动清理），但任何服务端请求都会 401；
- 也可能客户端自己写下 `.logged-out` 并把文件清空；
- **两种情况下 `restore` 都能把原账号写回去**：它只依赖快照与 `previous_uid`，
  不依赖当前登录是否有效。

---

## 9. 真机验收步骤（必须用户在在场时做）

前置：Magpie 里已登录一个**国内版** WorkBuddy 账号，且它在 AICC 账号列表里只有 Magpie 关联。

1. 在 AICC「AI 账号」页选中该账号，确认按钮是「**临时**切换到此账号」，
   旁边显示"只借用 Magpie 的访问令牌 · 剩余 X"。
2. 记下**当前** WorkBuddy 登录账号（左栏顶部那行）。
3. 记录 `~/.config/magpie/plugin-auth.json` 的 sha256：
   `shasum -a 256 ~/.config/magpie/plugin-auth.json`
4. 点「临时切换到此账号」→ 确认弹层（会写明有效期与"不会交出刷新令牌"）。
5. 观察 WorkBuddy 重启；AICC 应报"已切换 / 临时登录有效至 …（剩余 …）"。
6. 在客户端里确认登录的是目标账号；做一次真实操作（发一句话），确认服务端接受。
7. 再取一次 Magpie 文件的 sha256 —— **必须与第 3 步完全一致**。
8. 点左栏「恢复原账号…」→ 确认；客户端应重启并回到第 2 步那个账号。
9. 复查：AICC 左栏不再显示"临时登录"，`cat` 客户端认证文件里 `account.uid` 是原账号。
10. 若第 5 步后客户端立刻登出：说明 Access-only 不被接受 —— 记录现象，仍按第 8 步恢复。

**禁止**把"模拟客户端测试通过"写成"真机验证成功"。

---

## 10. 本 PR 能回答 / 不能回答的问题

| 问题 | 结论 | 依据 |
| --- | --- | --- |
| Magpie access-only 为什么**可能**被接受 | 客户端认证文件里 access 是独立字段，没有 refresh 也能构成一个"有令牌的登录" | 文件结构 + 代码测试 |
| 是否**真的**被接受 | **未知** | 需要 §9 第 5–6 步 |
| 客户端会不会自动登出 | **未知** | 同上 |
| 到期后客户端表现 | **未知**，两种预期都写在 §8 | 同上 |
| 原账号能否可靠恢复 | **能**（代码层保证）：快照按 sha256 校验、逐字节写回、登出状态下也可恢复 | `restoring_a_temporary_login_puts_the_previous_account_back` 等 6 个用例 |
| Magpie 原始登录态是否保持不变 | **是**：只 `read_logins`，从不写；有单测比对文件字节 | `reading_magpie_logins_never_rewrites_the_file` |
| 有没有 refresh 泄漏 / 刷新冲突 | **没有**：Magpie 路径不读 refresh；Access-only 结构上不含 refresh 字段 | 两个单测 + 独立构造函数 |
