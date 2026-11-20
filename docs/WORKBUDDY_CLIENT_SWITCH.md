# WorkBuddy 客户端账号切换（WB-SWITCH-1）

一句话：**把 AICC 里已经授权过的账号写进 WorkBuddy 客户端的登录态，必要时重启客户端；
失败按快照逐字节还原，并把"到底恢没恢复"如实告诉用户。**

这份文档是这一步的唯一口径。界面文案、RPC 返回、数据库字段都以这里为准。

---

## 1. 边界（先说不能做什么）

| 能力 | 状态 | 原因 |
| --- | --- | --- |
| 国内版（`provider = workbuddy`）客户端切换 | **已接通** | 认证文件结构、退出标记、启停方式都实测过 |
| 国际版（`workbuddy_ai`） | **未开放** | 认证文件、客户端行为都没有实测证据，不拿用户登录态冒险 |
| Magpie 来源账号 | **未开放** | 只有 access、没有 refresh；切过去之后无法续期 |
| 只有 access 的自有授权 | **未开放** | 同上，缺刷新凭据 |
| 复制聊天会话 / 迁移 `main.sqlite` | **不做** | 只换"登录是谁"，不搬业务数据（Qoder Profile 的教训：只换认证文件 ≠ 账号隔离） |
| 伪造活跃、模拟对话 | **不做** | 官方条款里属于取消资格情形，AICC 只做只读判断 + 提醒 |

`plan()`（`account-worker/src/workbuddy_switch/service.rs`）是唯一的可切换判定入口，
它只读库、**不读令牌**（读令牌会弹钥匙串授权框）。界面显示的"为什么不能切"就是它给出的
`reason`，不要在前端另写一套判断。

稳定原因码：

| code | 含义 | 界面动作 |
| --- | --- | --- |
| `ok` | 可以切 | 显示「切换到此账号」 |
| `live` | 客户端当前登录的就是它 | 显示"就是当前账号"，不给按钮 |
| `unsupported_provider` | 国际版 | 说明未开放 |
| `client_not_installed` | 没找到客户端 | 说明先安装 |
| `no_credential` | 没有任何授权 | 引导重新授权 |
| `missing_refresh` | 自有授权缺 refresh | 引导重新用浏览器登录 |
| `magpie_no_refresh` | 只有 Magpie 关联 | 说明暂不支持 |

---

## 2. 客户端认证文件长什么样（2026-11 实测）

路径（国内版）：

```
~/Library/Application Support/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop.info
同目录还有 workbuddy-desktop.info.logged-out   ← 退出标记（存在即视为未登录）
```

结构：

```json
{
  "auth":    { "accessToken": {"$wbEncrypted":1, "...": "..."},
               "refreshToken": {"$wbEncrypted":1, "...": "..."},
               "tokenType": "Bearer", "domain": "www.workbuddy.cn",
               "expiresAt": 1797552874249, "lastRefreshTime": 1795... },
  "account":  { "uid": "...", "nickname": {"$wbEncrypted":1, ...}, "uin": "...", ... },
  "accounts":    [ { "uid": "...", "nickname": {"$wbEncrypted":1, ...} } ],
  "allAccounts": [ 同上 ]
}
```

三条要点：

1. **令牌只有当前账号那一份**（`auth.accessToken` / `auth.refreshToken`）。
   `accounts` / `allAccounts` 里只有身份字段，没有令牌 —— 所以"切换"必须重写 `auth`。
2. `nickname` / `phoneNumber` 是 AES-GCM 信封，密钥只在客户端运行时里。
   AICC **不解密、不覆盖**：目标账号如果已经在客户端列表里，原样复用那一条账号对象。
3. AICC 写出去的是**明文** access / refresh（自有 OAuth 拿到的就是明文）。
   客户端启动后会自己重新封装 —— 所以 `live.tokenForm` 从 `plaintext` 变成 `envelope`
   本身就是"客户端读进去并接受了"的强证据，见 §3 的校验。

> 写文件走 `write_private()`：临时文件 `0600` → fsync → rename → fsync 目录。
> 里面是明文令牌，权限不能靠 umask 碰运气。

---

## 3. 切换流程

```text
1. 预检       账号存在 / 国内版 / 客户端已安装 / 自有授权带 refresh / 没有未完成切换
2. 活跃取证   目标账号 growth /heatmap + /streak（只读，失败只记一句原因）
3. 关客户端   osascript 优雅退出 → 超时 pkill → 等进程消失（20s）
4. 快照       关掉之后再读一次认证文件，逐字节存到 <数据目录>/workbuddy-switch/<id>.json
5. 写 journal phase=prepared（open_slot=1）
6. 写认证     合并 account + auth，保留客户端已有的账号对象与其它顶层字段
7. 启动       open -a，等进程起来
8. 回读校验   至少观察 2.5 秒，最多 25 秒
9. 活跃取证   再查一次
10. 收尾      phase=verified / unconfirmed，释放 open_slot
```

### 为什么快照必须在**关客户端之后**

客户端退出时会做一次收尾写入，可能把刚刷新过的令牌写回文件。关之前拍的那份，
恢复回去可能是一份已经作废的 refresh。所以第 4 步在第 3 步之后。

### 校验为什么是三态而不是"成功/失败"

| 观察到的情况 | 结论 | 动作 |
| --- | --- | --- |
| 文件里 UID = 目标 + 无退出标记 + 进程在跑（≥2.5s） | **confirmed**（`verified`） | 收尾，报成功 |
| 文件里 UID ≠ 目标，或出现退出标记 | **rejected** | **自动回滚**，报失败 + `restored` |
| 文件已是目标，但 25 秒内没确认到进程 | **unconfirmed** | **不回滚**，`phase=unconfirmed`，让用户确认 |

第三态的理由：冷启动慢是常态。把一次大概率成功的切换回滚掉，比"晚点再确认"更糟；
但也不能把"文件写对了"说成"客户端接受了"。

---

## 4. 失败与恢复

### 4.1 自动回滚（切换过程中）

触发条件：启动失败、客户端把文件改回别的账号、客户端写退出标记。

回滚动作，顺序固定：

1. 关掉可能已经起来的客户端；
2. 把快照**逐字节**写回认证文件（不重建，避免丢字段）；
3. 按切换前的状态恢复退出标记（原来有就写回，原来没有就删掉）；
4. 如果切换前客户端在跑，按原状重新启动；
5. 落 `phase=restored`（恢复成功）或 `phase=failed`（恢复也失败）。

写入本身失败（`write_private` 报错）时文件一个字节都没改，走 `finish_without_write`：
重启客户端、落 `restored`、错误里 `restored=true`。

**错误里必须带 `restored`**：`true` = 登录态没变；`false` = 需要用户手动登录。
界面把这两种分开显示，不许合并成"切换失败"。

### 4.2 显式恢复（用户点按钮）

`workbuddy.client.restore`：不传 `journalId` 时按"未完成 → 最近一次成功切换"挑一条。

- 恢复前必须校验快照 sha256；被改动过就拒绝（`kind=snapshot_corrupt`），
  绝不用一份来源不明的文件覆盖登录态。
- 没有任何可恢复记录时返回 `restored=false, reason=nothing_to_restore`（**不是错误**）。

### 4.3 崩溃恢复

`workbuddy_switch_journal.open_slot` 上有唯一索引：**未完成的切换至多一条**。
AICC 或助手在切换中间被杀掉时，下次 `workbuddy.client.status` 会带 `pendingSwitch`，
界面显示"上一次切换没走完（已写入，未确认）"+「恢复…」。恢复成功即释放槽位。

未完成期间不允许再发起切换（`E_SWITCH` / `kind=switch_busy`）：
否则"当前登录该还原到哪一份"就说不清了。

---

## 5. 接口

| 方法 | 参数 | 返回 | 何时调用 |
| --- | --- | --- | --- |
| `workbuddy.client.status` | 无 | `client` / `live` / `candidates[]` / `pendingSwitch` / `lastSwitch` / `recent[]` | 只读；进账号页、切换后刷新 |
| `workbuddy.client.switch` | `accountId`、`restart`（默认 true，传 false 直接拒绝）、`observeActivity`（默认 true） | `changed` / `phase` / `verified` / `verifyDetail` / `previousUid` / `activityBefore` / `activityAfter` | 用户点「切换到此账号」 |
| `workbuddy.client.restore` | `journalId`（可选） | `restored` / `uid` / `detail` | 用户点「恢复…」 |

- `live.uid` 是**唯一可信的"当前登录"**。AICC 账号列表里选中了谁、查询优先用哪份授权，
  都不能替代它。
- `switch` 超时预算：关进程 20s + 校验 25s + 两次活跃查询，Swift 侧给 180s。
- 错误码：`E_SWITCH = 1008`（已有未完成切换）；其余失败用 `INTERNAL`，靠 `data.kind` 分类：
  `switch_failed` / `close_failed` / `no_snapshot` / `snapshot_corrupt` / `switch_busy`。

---

## 6. 数据库（v25）

```sql
workbuddy_switch_journal(
  id, target_account_id, target_uid,
  previous_uid, previous_running, previous_marker,
  phase,            -- prepared / written / launched / verified / unconfirmed / failed / restored
  open_slot,        -- 1 = 未完成；NULL = 已收尾（唯一索引保证至多一条）
  snapshot_path, snapshot_sha256, token_form, detail,
  activity_before, activity_after,   -- 只作证据，不参与判定
  created_at, updated_at
)
```

快照文件默认在 `~/Library/Application Support/AICC-Dashboard/accounts/workbuddy-switch/`。
**不要手动删**：`restore` 依赖它；确认不需要恢复后再清。

---

## 7. 切换对"活跃"到底有没有用（实测协议，**尚未完成**）

### 7.1 已知事实

`docs/activity-rules-2026-11-17.md` 已经证明：**签到成功 ≠ 今日活跃**。
四个账号连续签到 5 天，其中三个热力墙仍是 `score=0 / is_active=false`。

但"**仅切换登录 = 计入活跃**"这件事，**本项目没有任何证据**。上游 WorkBuddy Switch
的"切换成功"也不包含服务端活跃校验。所以下面的实测是这一步的必做项。

### 7.2 步骤（需要一个当天尚未活跃的账号）

1. 挑目标账号：账号页 → 活跃页确认它今天 `is_active=false` 且 `score=0`。
2. 记录基线：`workbuddy.client.status` 的 `live.uid`（切换前是谁）+
   `growth.activity.status` 里目标账号的 `todayActive` / `streakDays`。
3. 在账号页点「切换到此账号」并等它报"已确认切换"。
4. **不做任何其它操作**，等 2 分钟，点活跃页「刷新」，记录一次。
5. 再等 10 分钟，再刷新一次，记录。
6. 打开客户端真实用一次（发一条对话），再刷新一次作为对照。
7. 把三个时点的 `is_active` / `score` / `streak.days` 记进下表。

> 切换时 AICC 会自动把 `activityBefore` / `activityAfter` 写进 journal，
> 这是 T+0 的证据；T+2min / T+10min 需要手动点刷新。

### 7.3 判定

| 观察 | 结论 | 下一步 |
| --- | --- | --- |
| T+0 起 `is_active` 变 true 且 `score > 0` | **登录即计入活跃** | 定时切换可以直接围绕"切号 + 打开客户端"做 |
| 只在 T+10min 后才 true | 计入，但有延迟 | 定时任务要留回查窗口，别当场判失败 |
| 一直 false，用一次对话后 true | **登录不计入**，真实使用才计入 | 定时切换只是"换号"，活跃问题要另说；界面不许暗示切换能点亮热力墙 |
| 一直 false，用对话也不变 | 活跃与客户端行为无关（另有规则） | 停止在这条路上投入，回到"只读判断 + 提醒" |

**禁止**把"自动换号已完成"写成"活跃问题已解决"。这两件事在证据不足时必须分开说。

### 7.4 干扰项

- 服务端日期按 `Asia/Shanghai`，00:00–08:00 之间会先翻页；跨日测试要避开。
- 目标账号当天可能已经在别处活跃过（手机端、网页端）——先确认基线是 `false`。
- `/heatmap` 的 `updated_at` 只是服务端最后更新时间，**不能**当作"刚发生了活跃"。

---

## 8. 未验证项与风险（诚实清单）

| 项 | 现状 | 影响 |
| --- | --- | --- |
| 明文令牌是否被所有客户端版本接受 | 只有"回读确认"证据，没有"服务端会话"证据 | 个别版本可能要求重新登录；回滚可用 |
| 账号不在客户端 `allAccounts` 里时 | 只能用 AICC 的信息拼一个最小账号对象（缺 `uin` / `avatarUrl` / `uniqueUserId`） | 头像与 UIN 可能显示为空，不影响登录 |
| 授权没有到期时间（`token_expires_at` 为空） | 不写 `expiresAt`，`expiresIn=0` | 客户端会立刻刷新一次令牌（正常行为，会重写文件）；刷新失败则会掉线 |
| 客户端启动很慢（>25s） | 落 `unconfirmed`，不回滚 | 界面要提示"打开客户端后刷新状态" |
| 切换会中断正在进行的对话 | 已知 | 二次确认里写明；夜间计划要处理"还在执行任务" |
| 写客户端目录的权限 | 需要「App 管理」或「完全磁盘访问」 | 写入失败会报权限错误并保持原登录态 |
| 国际版 | 完全未接 | 界面显示"未开放"，不要试 |

---

## 9. 手工验收清单

```bash
# 1) 只看不写：能读到"当前登录是谁"
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"workbuddy.client.status","params":{}}' \
  | AICC_ACCOUNT_DATA_DIR=$(mktemp -d) account-worker/target/debug/aicc-account-worker

# 2) 全量单测（含切换/回滚/恢复，全部用假客户端，不会碰真实 WorkBuddy）
env -u https_proxy -u http_proxy \
  ~/.cargo/bin/cargo test --manifest-path account-worker/Cargo.toml

# 3) 真机：在 App 里切换一个账号，确认
#    - 切换后 live.uid 变成目标账号
#    - 客户端重启后仍停在目标账号（不是被改回去）
#    - 点「恢复…」能回到原账号，且客户端重新启动
```

真机验收时**不要**在 AICC 正在被使用的时间段做（切换会退出客户端）。
