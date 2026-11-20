# WorkBuddy 夜间账号轮换（WB-ROTATE-1 / #113）

一句话：**按用户设的时间窗依次切换若干个 WorkBuddy 登录账号，并在窗口结束时
回到这一轮最初登录的那个** —— 不是回到"上一个"。

```text
22:30 原本登录 A   →  23:00 切 B  →  00:00 切 C  →  01:00 切 D  →  07:00 回 A
```

之所以能回 A，是因为**本轮开始时的登录态被单独存了一份**（`origin_*`），
而不是靠 `workbuddy_switch_journal` 的"回到上一站"。后者在 A→B→C→D 之后只能带回 C。

> 本文档是这一功能的唯一口径。界面文案、RPC 字段、数据库列都以这里为准。

---

## 1. 边界

| 能做的 | 不能做的（以及为什么） |
| --- | --- |
| 定时切换**国内版** WorkBuddy 的登录账号 | 国际版（`workbuddy_ai`）：认证文件与客户端行为都没有实测证据 |
| 自有长期授权（access + refresh）与 Magpie 临时授权（只有 access） | 替 Magpie 刷新令牌：那是 Magpie 的凭据，两边一起刷会互相踢下线 |
| 结束时按快照逐字节恢复**最初**账号 | 恢复中途"上一个"账号：语义不同，会让 D→C→B→A 变成 D→C |
| 切换前后只读查询活跃状态 | 伪造活跃、自动发对话、调内部写接口：官方条款里属于取消资格 |
| 用户在场时的短时验证（「立即执行一轮」） | 无人值守情况下"无论如何都要切完"：宁可跳过，不硬来 |

**默认关闭**。只有用户在面板里显式打开，才会有任何自动操作。

---

## 2. 为什么不复用 `switch_journal`

`workbuddy_switch_journal`（v25/v26）的语义是**一次切换**：`previous_uid` 是"上一站"，
`restore` 是"回上一站"。这对单个手动切换是对的；对一夜多跳是错的：

| 需求 | journal 能给的 | 夜间轮换要的 |
| --- | --- | --- |
| 结束回到哪 | 上一站（D→C） | 这一轮最初（D→A） |
| 记录粒度 | 一次切换 | 一轮（含多步） |
| 一天执行次数 | 无约束 | 一天最多一轮（数据库唯一索引） |

所以 v27 另建三张表，`origin_*` 与 journal **完全分开**：

```text
workbuddy_rotation_plan   用户配置（单行）
workbuddy_rotation_run    一轮：窗口、最初账号与快照、状态、光标
workbuddy_rotation_step   这一轮里的一个账号：模式、结果、活跃证据
```

`run.origin_snapshot_*` 指向的是一份**独立复制**的快照文件
（`<数据目录>/workbuddy-switch/rotation/origin-*.json`），
不是 journal 快照的引用 —— 那些 journal 会被后续切换结算、更新，不应牵连这一轮。

### 快照怎么拍

沿用切换本身的机制：**先关客户端，再读认证文件**。客户端退出时会把内存里的最新
令牌写回文件；不关它就可能在半夜存下一份马上过期的旧令牌。

```text
预检（有没有可切目标）
  → 关客户端
  → 读 workbuddy-desktop.info（逐字节）
  → 存 origin 快照（0600 + sha256）
  → 启动客户端
```

**一个可切目标都没有时不拍快照、不关客户端**：一次纯跳过的轮次不该让用户多等一次重启。

---

## 3. 状态机

```text
scheduled ──► prechecking ──► switching ──► dwelling ──┐
                   │              │              │      │
                   ├─► skipped    ├─► restoring ◄┘      │
                   └─► failed     │                     │
                                  └─► recovery_required ◄┘
restoring ──► completed / recovery_required / failed
```

| 状态 | 含义 | 谁推进 |
| --- | --- | --- |
| `scheduled` | 本轮已建立，尚未预检 | tick |
| `prechecking` | 读客户端登录、算每个账号能不能切、拍原账号快照 | tick |
| `switching` | 正在切 `cursor` 指向的账号 | tick |
| `dwelling` | 已切过去，在停留 | tick |
| `restoring` | 正在写回原账号快照 | tick |
| `completed` | 正常收尾（含"用户自己切回去了"、人工恢复成功） | — |
| `skipped` | 本轮没执行（迟到 / 没登录 / 全都不可切） | — |
| `failed` | 预检或执行阶段彻底失败 | — |
| `recovery_required` | 该恢复但不敢自动覆盖，等用户确认 | 用户 |

所有推进都是 **`state` 的 CAS**：`UPDATE ... WHERE id=? AND state=?`。
两个 worker 同时 tick 时只有一个能赢，另一个拿到 0 行、这一轮什么都不做。

`open_slot`（唯一索引）仍然是最后一道闸：同一时刻只允许一条未完成的**切换**。

### 步骤状态

`planned` → `switching` → `done` / `dwelling`（写入未确认时）/ `failed` / `skipped` / `already_live`。

`already_live` 是"客户端当前登录的就是它"：不写文件、不拍快照、只留一条记录。

---

## 4. 时间窗

| 规则 | 实现 |
| --- | --- |
| 明确时区 | 只接受 `Asia/Shanghai` / `UTC`（没有 `chrono-tz`，手写 DST 一定会错；保存时直接拒绝其它值） |
| 跨午夜 | `end <= start` 视为次日结束；`active_window` 同时看"昨天开始、今天结束" |
| 星期 | 按**窗口开始那天**的星期判断 |
| 一天最多一轮 | `UNIQUE(plan_id, plan_date)`，插入冲突 = 今天已经跑过 |
| 迟到不补跑 | 超过开始时间 10 分钟（`LATE_START_TOLERANCE_SECS`）→ 建一条 `skipped`，**同一晚不再尝试** |
| 窗口不够 | **开始之前**就算"现在 + 停留 + 恢复余量"，不够就 `skipped`，连客户端都不关 |
| 关闭开关 | 停止后续所有轮次 + **立刻**把还在跑的那一轮恢复到最初账号（`restoring`），两者不可分割 |
| 用户在用 | 拍原账号快照**之前**就推迟（`prechecking` 状态等下一跳）；等不起就跳过，不关客户端 |

`RESTORE_RESERVE_SECS = 5 分钟`：恢复要关客户端、写文件、等它起来。
`USER_ACTIVE_IDLE_SECS = 120`：HID 空闲低于 2 分钟视为"用户在键盘前"。

**窗口检查独立于空闲检查**：等待用户、休眠、进程重启都会让 `now` 往前跳，
所以每次准备开始新的一跳，都用当前时间重新算一次"现在 + 停留 + 恢复余量"；
不够就恢复本轮最初账号，不切入一个已经结束的窗口。

**同一次 tick 内也要复核**：入口的 `now` 在预检（一次真实 HTTP 探针）和活跃查询
之后已经过时。所以真正动客户端之前再读一次 `deps.clock.now_ms()`，超窗就把这一步
记成 `skipped / window_elapsed_during_wait` 并进入恢复 —— 而不是拿着入口的时间戳
去切一个已经过期的窗口。

### 关闭开关 ≠ 只是"别再开新的"

| 动作 | 后续计划 | 当前这一轮 |
| --- | --- | --- |
| 关闭开关 | 停止（明天也不会跑） | **立即恢复最初账号**，然后 `completed` |
| 面板「暂停并恢复原账号」 | 不动开关（明天照常） | 立即恢复最初账号 |

两者对**当轮**的行为一样，区别只在"明天还跑不跑"。之所以让"关闭"也立刻恢复：
把客户端留在某个轮换账号上过夜，是这条链路上最坏的结果。
界面文案与这张表一致（关闭后显示"并把账号恢复到…"，没恢复成功就说没成功）。

**停止请求必须先落库**（v29 `rotation_run.stop_requested`）。`stop()` 拿不到跨进程锁时
只返回 `busy` 并把请求丢掉，下一次心跳会以为没人要停 —— 而手动轮次本来就有
"开关关了也继续跑"的豁免，于是会接着切下一个账号。现在的顺序是：

```text
找到活动轮次 → 写 stop_requested=1 → 抢锁
  拿不到 → 返回 stop_requested（"已请求暂停，当前操作结束后会自动恢复"）
  拿到   → 立刻恢复并收尾
```

`advance` 最前面检查这个标记：一旦为真，**优先于手动轮次的豁免**，直接进 `restoring`。

**手动发起的一轮不受开关影响**：`rotation_run.trigger`（v28）区分 `scheduled` / `manual`。
关闭开关只会收尾自动轮次；用户点「立即执行一轮」起来的那一轮有自己的窗口，
会按停留时长走完再恢复。这个标记存在数据库里，助手重启后仍然有效。

---

## 5. 两种授权模式

写进客户端的登录按**固定优先级**挑（与手动切换同一个 `plan::resolve`）：

1. 自有授权且带 refresh → `own_refresh`（客户端自己续期）；
2. 否则 Magpie 可用 → `magpie_access_temporary`（借来的 access，会过期）。

`auth_preferred_source` **不影响**这里：它只决定 AICC 读签到 / 额度 / 成长时用哪份令牌。

### 5.1 自有授权：真的探一次，而不是看数据库

**「库里有 `refresh_ref`」不等于「这份授权今天还能用」。** WorkBuddy 客户端自己刷新
时会轮换 refresh token，AICC 手里那份可能已经作废。

所以每一跳之前：

1. 从凭据库读出 access（只读，不写、不刷）；
2. 发一次**只读**探针；
3. `rejected`（401/403）→ **跳过这个账号**，记 `own_grant_rejected`，界面提示重新授权；
4. `unknown`（网络/超时）→ 允许继续，但 step 的 `authPrecheck` 如实记 `unknown`；
5. 读不到令牌 → 跳过，记 `own_grant_unreadable`。

**不会**为了"把这一晚跑完"去覆盖用户授权、静默换来源或自动重新登录。

### 5.2 Magpie 临时授权

- 每次切换前**现读** `plugin-auth.json`，严格匹配 `source_key` + UID + 国内版 provider；
- **不读、不复制、不修改** Magpie 的 refresh token；
- 算剩余有效期，要求覆盖 `停留时长 + 恢复余量`，不够就跳过（`magpie_too_short_for_dwell`）；
- 原始 Magpie 文件只读：`docs/WORKBUDDY_CLIENT_SWITCH.md` §10 有"运行前后字节一致"的单测；
- access 过期时不伪造、不续期，直接跳过。

### 5.3 Refresh Token 风险（如实说明）

| 事实 | 影响 |
| --- | --- |
| 客户端用自有授权登录后会自己刷新，可能**轮换** refresh token | AICC 手里那份会变旧 |
| 变旧之后，AICC 的签到 / 额度 / 成长查询会失效 | 下一跳的预检会发现 401 → 跳过并提示重新授权 |
| 客户端当前的登录**不受影响** | 它手里是最新的 |
| 无法完全避免 | 只要"客户端用自有授权登录"，这件事就可能发生；能做的只有**发现并停下** |

---

## 6. 用户在场与打断

| 场景 | 行为 |
| --- | --- |
| 用户在键盘前（HID 空闲 < 2 分钟） | **拍快照之前**就推迟（这一轮停在 `prechecking`）；等不起就整轮跳过，客户端一次都不关 |
| 推迟之后用户离开 | 下一次 tick 正常开始，不需要用户操作 |
| 已在停留中、用户回来了 | 停留照旧；到点该切下一个时若仍在用，就推迟那一跳（不关客户端） |
| 用户中途手动换了账号 | **立即停止**整轮，且**不覆盖**他的选择（`completed` + 说明） |
| 用户中途自己切回最初账号 | 停止，视为已收尾 |
| 客户端因 access 过期而登出（`uid` 为空） | **不算**用户干预：这正是要救的场景，继续恢复 |
| 用户点「暂停并恢复原账号」 | 立刻走恢复流程，失败则 `recovery_required` |

"用户正在跑长任务"**无法可靠判断**：AICC 只看进程存在，不能据此认为客户端空闲。
所以打开计划时界面明确提示"切换会中断客户端里正在进行的对话"，默认保守（见上表第一行）。

---

## 7. Mac 休眠与 AICC 退出

| 场景 | 实际行为 |
| --- | --- |
| 屏幕关闭、系统仍运行 | 心跳继续（60 秒一次），计划正常推进 |
| Mac 真正休眠 | 心跳停止。**不会**在日志里记成"已执行" |
| 醒来时已过开始时间 10 分钟 | 本轮 `skipped`，**不补跑**；同一晚不会重试 |
| 醒来时仍在窗口内 | 继续按当前时间推进（可能少切几个账号） |
| AICC 退出 | **调度器停止**。没有 launchd、没有常驻代理 |
| AICC 重启 | 读 `active_rotation_run`：还在窗口内就接着推进；窗口已过就恢复原账号 |

**没有独立调度服务**：`aicc-account-worker` 是 `AICC.app` 的子进程，这是既有架构。
需要"App 关了也跑"必须引入 launchd LaunchAgent，那是另一件事（见 §12），本轮不做。

### 并发：三层，各管一件事

| 层 | 机制 | 挡住什么 |
| --- | --- | --- |
| 进程内 | 线程局部 `BusyGuard` | 同一进程里 tick 与 RPC 抢着操作客户端 |
| 跨进程 | `<数据目录>/workbuddy-switch/rotation.lock` 上的 `flock`（`try_lock`，非阻塞） | **两个 worker 进程**同时进 `prechecking` / `switching` |
| 数据库 | `state` 的 CAS + `open_slot` 唯一索引 | 同一状态被推进两次、两条切换同时存在 |

只用前两层里的任何一层都不够：

- 只有进程内互斥 → 两个 worker 各自持锁，会各关一次客户端；
- 只有 CAS → 它只保证"状态不重复迁移"，挡不住两个进程**同时执行同一个状态**。

`flock` 而不是锁文件里的 pid：进程崩溃时内核自动释放，不需要"清理陈旧锁"，
也不会因为 pid 复用判错。锁文件残留无害。

真实并发测试：`two_concurrent_ticks_switch_only_once` 用两个线程共享同一份数据目录
（线程局部的 `BusyGuard` 互不可见），断言只有一个 tick 真正执行、只切一次、只产生一条 `done`。

---

## 8. 崩溃恢复

崩溃时 `flock` 由内核释放，不需要人工清锁；重启后从 `active_rotation_run` 接着判定。

判定顺序（`advance` + `reconcile`）：

1. 读客户端**实际**登录（`live.uid`），不信 AICC 的选中项；
2. 判本轮是否还在窗口内（`window_end` + 是否迟到）；
3. 若有一个 `switching` 的步骤：
   - 客户端已是目标账号 → 视为成功，进入停留，**不重复切**；
   - 客户端仍是最初账号 → 记该步失败，继续下一个；
   - 客户端是第三个账号 → `recovery_required`，不猜；
4. 需要恢复时先校验快照 sha256：
   - 文件不在 → `snapshot_missing`，`recovery_required`；
   - sha 不符 → `snapshot_corrupt`，`recovery_required`（**不删**损坏的快照，留给人工判断）；
   - 客户端当前既不是最初账号、也不是本轮最后切到的账号，且不是登出 → `live_changed`，**不覆盖**；
5. 恢复失败保持 `recovery_required`，可重试；
6. **人工把故障排除后恢复成功**（`restoreOrigin`）会把 `recovery_required` 收口成 `completed`，
   状态页、历史记录与 `restore_result` 一起更新——`recovery_required` 是"等人处理"，
   不是"永远结束"。

---

## 9. 活跃：只取证，不造假

用户提这个功能的初衷是"改善多个账号的活跃"，但**没有证据**证明"切换成功 = 服务端活跃成功"。
所以本功能只做三件事：

1. 切换**前**只读查一次目标账号的活跃状态（`activity_before`）；
2. 停留**结束**再查一次（`activity_after`）—— 不在刚切完就查，那时服务端还没记上；
3. 三个结论**分开记**：`step.status`（切换成功）、`authPrecheck`（授权有效）、`activityAfter`（今日活跃）。

界面上"今日活跃 / 连续天数 / 查询时间"分别展示；查询失败写"活跃查不到"，**不写"未活跃"**。

禁止：自动发对话、伪造活跃事件、调未授权接口、把签到成功当活跃成功。

如果实测证明"切换并不产生活跃"，照实报告并停止围绕这个目标加功能。

---

## 10. 接口

| 方法 | 参数 | 说明 |
| --- | --- | --- |
| `workbuddy.rotation.status` | — | 计划、下一轮时间、当前轮、最近 5 轮 |
| `workbuddy.rotation.save` | `timezone?` `weekdays?` `startMinutes?` `endMinutes?` `accountIds?` `dwellSeconds?` `enabled?` | 只覆盖出现的字段 |
| `workbuddy.rotation.setEnabled` | `enabled` | 打开前校验计划；缺账号直接拒绝 |
| `workbuddy.rotation.dryRun` | — | 只演算：会切谁、跳过谁、为什么；**不写库、不动客户端** |
| `workbuddy.rotation.runNow` | `confirm: true` | 立即跑一轮（占用当天名额），用于用户在场的短时验证 |
| `workbuddy.rotation.stop` | — | 暂停并恢复原账号 |
| `workbuddy.rotation.restoreOrigin` | `runId?` `force?` | 手动恢复最初账号 |

- 所有会动客户端的接口超时给 180 秒（关客户端 20s + 校验 25s + 恢复）。
- 响应里**没有**任何令牌：只有 UID、时间、状态与原因码。
- `status` 里的 `scheduler.runsWhileAppClosed = false` 是**事实**，界面据此写文案。

---

## 11. 数据库（v27）

```sql
workbuddy_rotation_plan(
  id, enabled, timezone, weekdays, start_minutes, end_minutes,
  account_ids, dwell_seconds, created_at, updated_at
)
workbuddy_rotation_run(
  id, plan_id, plan_date, timezone, window_start, window_end,
  origin_account_id, origin_uid, origin_snapshot_path, origin_snapshot_sha256,
  origin_marker, origin_running,
  state, cursor, current_journal_id, last_target_account_id, last_target_uid,
  restore_attempts, restore_result, detail, created_at, updated_at
)
-- UNIQUE(plan_id, plan_date)：一天最多一轮
workbuddy_rotation_step(
  id, run_id, seq, account_id, uid, mode, planned_at, started_at, ended_at,
  status, journal_id, skip_reason, detail, auth_precheck,
  activity_before, activity_after, created_at, updated_at
)
-- UNIQUE(run_id, seq)：一轮里每个位置只有一个步骤
```

快照在 `<数据目录>/workbuddy-switch/rotation/origin-*.json`，`0600`、原子写、sha256 校验。
**不要手动删**：恢复依赖它。

---

## 12. 本轮没做（以及为什么）

| 项 | 原因 |
| --- | --- |
| launchd 常驻调度 | 需要长期驻留服务、额外的安装/卸载与失败可见性设计；先在 App 内跑通 |
| 多套计划（工作日 / 周末不同） | 表已留 `id` 列；当前只有一套配置，减少认知负担 |
| 每账号单独停留时长 | 统一停留时长已能表达"人均几分钟"；单独设置会让窗口演算复杂一倍 |
| 国际版 | 认证文件结构未验证 |
| 自动"补跑"错过的轮次 | 会让第二天一次性切好几次，风险大于收益 |
| UI V4 精简 | 本轮只加必要区块；方案见 PR 描述 |

---

## 13. 验证清单（区分"测过"与"没测过"）

### 自动化（确定性，CI 可跑）

Fake Clock + Fake Client + 注入式预检与活跃取证，**不联网、不碰真实客户端**：

| 用例 | 覆盖 |
| --- | --- |
| `one_account_switches_and_comes_back` | 单账号切换 + 恢复 |
| `three_accounts_rotate_and_return_to_the_original` | A→B→C→**A** |
| `magpie_access_that_cannot_cover_the_dwell_is_skipped` | Magpie 剩余不足跳过 |
| `a_rejected_own_grant_is_skipped_and_the_client_is_restored` | 自有授权 401 安全停止 |
| `an_unreadable_own_grant_is_skipped_too` | 读不到令牌跳过 |
| `a_second_tick_in_the_same_night_does_not_start_another_run` | 重复触发只执行一次 |
| `a_manual_switch_during_the_night_stops_the_plan_without_overwriting` | 手动换号不覆盖 |
| `a_crash_after_writing_the_target_is_reconciled_on_restart` | 崩溃后识别并继续 |
| `waking_up_late_skips_instead_of_catching_up` | 休眠错过不补跑 |
| `a_corrupt_origin_snapshot_is_refused_and_kept_for_a_human` | 快照损坏拒绝恢复 |
| `a_missing_origin_snapshot_is_a_human_decision_not_a_guess` | 快照丢失不猜 |
| `an_expired_temporary_login_can_still_be_restored` | 临时授权过期仍能恢复 |
| `tokens_never_appear_in_status_or_history` | 不泄漏令牌 |
| `an_activity_probe_failure_does_not_block_the_restore` | 活跃查询失败不影响恢复 |
| `dry_run_touches_nothing` | 预演零副作用 |
| `stopping_a_run_puts_the_original_account_back` | 暂停即恢复 |
| `a_disabled_plan_never_touches_the_client` | 关闭时零动作 |
| `a_target_that_is_already_live_is_not_switched_again` | 已是当前账号不重复切 |
| `a_client_that_rejects_the_login_does_not_wedge_the_run` | 客户端拒绝不会卡死 |
| 窗口 / 时区 / 星期 / 跨午夜 | 时间计算 |
| `starting_a_run_waits_when_the_user_is_active` | 用户在用时**不关客户端**，状态停在预检；离开后正常开始 |
| `a_window_too_short_never_starts_a_hop` | 窗口不够一个停留 → 跳过，客户端零动作 |
| `a_busy_user_postpones_the_next_hop` | 停留到点但用户在忙 → 推迟，不切走 |
| `near_the_window_end_it_restores_instead_of_forcing_another_switch` | 窗口快结束 → 恢复而不是强切 |
| `disabling_the_plan_mid_run_restores_the_original_account` | 关开关 → 立刻回到最初账号，之后不再开新轮次 |
| `a_pending_run_is_still_finished_when_the_switch_is_already_off` | 开关已关但轮次遗留 → 仍会收尾 |
| `activity_after_is_recorded_only_after_the_dwell_ends` | 停留结束前不取证 |
| `two_concurrent_ticks_switch_only_once` | **两个线程**同时 tick，只执行一次 |
| `a_stop_request_survives_a_busy_lock` | 锁被占时按暂停：请求落库，锁放开后只收尾不切下一个 |
| `a_slow_probe_cannot_switch_into_a_closed_window` | 活跃查询"耗时" 9 小时：复核用当下时间，不切 C |
| `a_manual_run_keeps_going_after_the_plan_is_switched_off` | 关闭自动计划后，手动一轮仍走完 B→C→A |
| `the_window_is_rechecked_after_waiting_for_the_user` | 等待用户后已超窗 → 不切 C，直接恢复 A |
| `a_successful_manual_restore_reopens_a_recovery_required_run` | 人工恢复成功 → `recovery_required` 收口为 `completed` |

并发：`state` CAS + `open_slot` 唯一索引 + 进程内互斥（单线程 runtime 的线程局部标志）。
测试里用"连续两次 tick"验证幂等；真正的双进程并发由数据库约束兜底（与 #111/#112 同一套）。

### 只经过 Fake Client 测试（**没有**真机验证）

- 真实的"关客户端 → 拍快照 → 写认证 → 回读"整夜流程；
- Mac 真实休眠/唤醒后的行为（只验证了逻辑分支）；
- 真实 Magpie 文件在整夜运行中的字节不变（单测用临时目录 + 注入，未跑真机整夜）；
- 用户在深夜真的在敲键盘时的推迟行为。

### 需要用户在场的真机验证（短时）

**一天只有一轮计划名额**（`UNIQUE(plan_id, plan_date)`，手动与自动共用），所以顺序要提前排好，
不能临时加：第 4、5 项各需要另一天。

| # | 场景 | 占当天名额 | 怎么做 | 记录什么 |
| --- | --- | --- | --- | --- |
| 1 | 手动切换 A→B→A（自有长期授权） | **否** | 账号页「切换到此账号」（走 journal，不产生轮换记录） | 客户端是否重启到 B、恢复后是否回到 A |
| 2 | Magpie 临时登录 → 恢复 | **否** | 同上切到 Magpie 账号，几分钟内点「恢复原账号…」 | 到期提示是否可见；`~/.config/magpie/plugin-auth.json` sha256 前后一致 |
| 3 | 完整 A→B→C→A | **第 1 轮** | 「立即执行一轮」，2 账号 × 5 分钟；**停留阶段退出并重开 AICC** | 自然结束并回到 A；重开后轮次继续推进；执行记录里切换与活跃分开 |
| 4 | 手动换号打断 | **第 2 轮（次日）** | 切到 B 之后在客户端手动换到 D | 计划立即停止；不覆盖 D；状态页提示需要确认 |
| 5 | 中途关闭开关 | **第 3 轮（再次日）** | 停留中把开关关掉 | 「已关闭，并且已恢复原账号」；之后不再开新轮次 |

补充：

- 第 3 步之前先「预演」，确认"会切谁 / 跳过谁"；手动一轮**不需要打开自动计划**（`trigger=manual` 不受开关影响）。
- 第 3 步开始前记下当前登录账号，并 `shasum -a 256 ~/.config/magpie/plugin-auth.json`；结束后再取一次。
- 每轮结束把自动计划保持关闭，直到五项都记录完。

**禁止**在没有用户在场时开启无人值守的整夜计划。
