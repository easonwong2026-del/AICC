# Qoder 双版本多账号 · 真机验收记录

> 对应 PR #70。记录的是**这台机器上真实跑出来的结果**，不写"应该没问题"。
> 技术证据与方案取舍见 `docs/QODER_MULTI_PROFILE_SPIKE.md`。

- 机器：`rakmacmini`（Apple Silicon / macOS）
- 时间：2026-11-13
- 被测二进制：`dist/mac/AICC.app/Contents/Helpers/aicc-account-worker`（与发布物同一个 release 构建）
- 真实账号：`e***@gmail.com`（Qoder International，v0.5.7）
- Qoder CN：**本机未安装**，CN 侧未做真机验收（见 §4）

## 1. 环境与迁移

| 项 | 结果 |
| --- | --- |
| 迁移前库 | `user_version=10`，`accounts` 里 4 个 workbuddy |
| 迁移 | `storage.init` 一次升到 `11`（只新增 `qoder_profiles` / `qoder_runtime`，不重建 `accounts`） |
| 迁移后数据 | workbuddy 4 条、授权 4 条、签到记录 4 条、成长记录 28 条，**全部保留** |
| 完整性 | `PRAGMA quick_check` 正常（`storage.status` 报 `integrityOk`） |

迁移前已用 `sqlite3 .backup` 备份到 `/tmp/aicc-accept/accounts-before.sqlite`。

## 2. 两个版本识别

`qoder.editions.detect` 实测输出（节选）：

| 字段 | Qoder CN | Qoder International |
| --- | --- | --- |
| installed | false | **true** |
| appPath | — | `/Applications/Qoder.app` |
| bundleId | `com.qodercn.app` | `com.qoder.app` |
| clientVersion | — | `0.5.7` |
| 数据目录 | 无 | `~/Library/Application Support/com.qoder.app.stable` |
| 客户端当前登录 | — | `e***@gmail.com`（读客户端自己的状态文件） |
| verifiedOnThisMachine | false | true |
| parallelInstances / switchMode | `no` / `snapshot` | `no` / `snapshot` |

## 3. 安全收尾后的当前状态（2026-11-13 晚）

**真实切换已停用**，本轮没有做任何新的真实账号快照写操作。

| 能力 | 状态 | 说明 |
| --- | --- | --- |
| 版本检测 `editions.detect` | 可用 | International 实测；CN 标注未复验，`parallelInstances=unknown` |
| 扫描 `profiles.scan` | 可用 | 只读 live + 复制快照，live 零改动 |
| 导入 `profiles.import` | 可用 | 只登记已有快照目录 |
| 认证检查 `profiles.inspect` | 可用 | 只读快照，不碰 live |
| 保存快照 `profiles.capture` | 可用 | 只写快照目录；身份证明不了就拒绝 |
| 切换 `profiles.open` / `activate` | **永久拒绝** | 没有任何环境变量开关；写入路径已从交付二进制里移除（`#[cfg(test)]`） |
| 关闭 `profiles.close` | 可用 | 只对"命令行带本 Profile 标记"的进程发信号 |
| 恢复 `profiles.recover` | 可用（显式） | 必须先确认客户端已退出；`Unknown` / 运行中一律拒绝 |

原因见 `docs/QODER_MULTI_PROFILE_SPIKE.md` §7：`main.sqlite` 里存在按 `account_id`
分区的账号数据（agent 定义、工作角色、协作运行时凭据），只换两个认证文件无法隔离。

### 3.0 本轮复核对写入路径的处理

复核指出三个问题，均已封闭（细节见 SPIKE §9）：

| 问题 | 处理 | 验证 |
| --- | --- | --- |
| 环境变量仍能重新打开 Global 的切换 | `switch_capability()` 恒 false，不再读环境变量；`begin/stage/commit` 只编进测试 | `switch_is_permanently_off_even_with_env_flag`、`open_and_activate_refuse_and_write_nothing` |
| `list` / `status` 会顺带恢复 live | 改成只报告 `pendingSwitches`；恢复走显式 `recover` + 进程退出确认 | `list_and_status_report_pending_switch_without_restoring`、`recover_refuses_*`、`recover_restores_only_after_confirmed_stop` |
| `capture` 逐文件 rename，可能留半套快照 | 改成暂存 + 上一代备份 + journal + 全组校验，失败整体回退 | `capture_stage_failure_*`、`capture_commit_failure_*`、`capture_heals_an_interrupted_commit` |

这一轮**没有对真实账号执行任何写操作**：live `auth.v1.dat` 哈希未变（`710d6e08`），
真实库里 `qoder_switch_journal` 为 0 条；只做了 schema 迁移 v11 → v12（新增切换日志表与
`qoder_profiles.snapshot_hash` 列）。

### 3.0.1 International 钥匙串授权（本轮完成）

之前 `inspect` 一直停在"待检查"，原因是钥匙串条目的 ACL **只信任 Qoder 自己**：

```text
acl 0: desc="Qoder App Safe Storage" trusted: [/Applications/Qoder.app]
partition: teamid:B6U242QL73
```

`SecKeychainItemCopyAccess` 读到的是路径式信任列表（base64 解出来就是
`/Applications/Qoder.app\0`），没有 `security`、也没有 AICC。

本轮在 GUI 会话里对
`~/Applications/AICC.app/Contents/Helpers/aicc-account-worker` 的请求点了「始终允许」，
之后 ACL 变成：

```text
acl 0: trusted: [
  /Users/rakmacmini/Applications/AICC.app/Contents/Helpers/aicc-account-worker,
  /Applications/Qoder.app
]
partition: teamid:B6U242QL73, cdhash:47ac964e27a9baccf0b49e562c8ed543be4e3d7c
```

紧接着重跑 `qoder.profiles.inspect`，**不用再弹窗**，返回真实数据：

| 字段 | 值 |
| --- | --- |
| `authStatus` | `valid`（快照指纹与 live 一致） |
| `tokenFingerprint` | `688d92f2` |
| `expiresAt` | `2026-12-12T03:39:39Z` |
| `accountUid` | `01a1afdc-c495-78ad-8a5c-9123e17e8708` |
| `email` | `e***@gmail.com` |
| `snapshotHash` | `710d6e08390c24162c70008a72bd9a1678264c574c2bf921a7a2315fd15531eb` |

这说明：认证解密链路（Safe Storage 口令 → `v10` 信封 → AES-128-CBC → JWT 字段）
在真实账号上是通的，只是**第一次必须由人点一次授权**。

> ⚠️ 本机没有可用签名身份（`security find-identity -p codesigning` = 0 个），
> AICC 是 ad-hoc 签名，授权记录里带的是 helper 的 cdhash
> （`47ac964e279abaccf0b49e562c8ed543be4e3d7c`）。
> 同一份源码重建 cdhash 不变，授权仍然有效（本轮已实测）；
> **代码一变 cdhash 就变，需要重新点一次「始终允许」**。
> 等有稳定签名证书后应改成按 designated requirement 授权。

## 3.1 上一轮（收尾前）的真机往返记录（历史）

下面这段是在**加闸门之前**的构建上做的，保留下来作为"快照机制本身可用"的证据；
当前版本不会再执行这些写操作。

### A. 原有账号保护

`qoder.profiles.scan` 把当前客户端登录的账号登记成 Profile：

- `captured: true`，`snapshotReady: true`，`active: true`
- live 的 `auth.v1.dat` **一个字节没动**（扫描前后 sha256 前缀都是 `710d6e08`）
- 快照目录：`…/AICC-Dashboard/accounts/qoder/profiles/qp_cd8a06b84e70/`

### B. 快照切换往返（强隔离验证）

新建一个**空快照**的 Profile（`隔离验证-B`），然后：

| 步骤 | 实测结果 |
| --- | --- |
| `qoder.profiles.open(B)` | `state=running`、`requiresLogin=true`、`savedPreviousProfile=qp_cd8a06b84e70` |
| 此时 live 认证 | **不存在**（被整份挪到 `profiles/.live-backup/<时间戳>/`，客户端进登录页） |
| `qoder.profiles.open(A)` | `state=running`、`authenticated=true`、`usedSnapshot=true` |
| 此时 live 认证 | 恢复为原内容（sha256 前缀回到 `710d6e08`） |
| `qoder.profiles.status` | `running=1`、`activeProfileId=qp_cd8a06b84e70` |
| `qoder.profiles.remove(B, purge)` | `purged=true`，**只删 AICC 快照**；返回 `liveDirUntouched` |

结论：两个账号的认证**没有互相覆盖**；切走时当前账号被存回自己的 Profile，
切回时原样写回 live，客户端接受该快照（指纹一致）。

### C. 关闭与收尾

- `qoder.profiles.close(A)` 返回 `capture.captured=true`：退出前把最新登录态存回快照。
- 机器收尾后没有任何 Qoder 进程残留，live `auth.v1.dat` 仍是原始内容。

## 4. 未完成 / 限制（必须如实记录）

0. **隔离未成立 → 切换永久关闭**：`main.sqlite` 的账号绑定数据无法用两个认证文件隔离；
   要恢复切换，先完成 SPIKE §7.2 的最小安全快照设计，再在代码里重新开放并做真机验证。

1. **Qoder CN 未安装**：CN 的 Bundle ID、数据目录、钥匙串条目名、`--user-data-dir`
   行为都还没有真机复验。代码按既有实现与官方发行标识处理，并在
   `verifiedOnThisMachine=false` 与界面文案里明确标注。
2. **钥匙串授权与 cdhash 绑定**：已在本机点过「始终允许」，`inspect` 可读到真实指纹与到期
   （见 §3.0.1）。同一份源码重建授权仍在；**代码变更后 cdhash 会变，需要重新授权**。
   有稳定签名证书后应改成按 designated requirement 授权。
3. **进程探测现在是三态**：`pgrep` 不可用/退出码异常 → `unknown`，界面显示"状态未知"，
   此时**禁止任何 live 写操作**；`ps` 读不到命令行时用 `kill -0` 兜底，
   只有拿到 ESRCH 才判"已退出"。归属判定只认 `AICC_QODER_PROFILE` 标记。
4. **恢复路径的真实崩溃没有在真机上演练**：只在单测里用 journal + 备份模拟
   "提交到一半崩溃"。真实环境的断电/强杀恢复仍属未验证行为。
5. **Qoder 官方多账号机制未调查**：`auth-profile-overlays.v1.dat` 可能承载官方的
   账号切换语义，但目前没有证据表明它能完成安全隔离；留到下一阶段专门研究。

## 5. 人工补验步骤（在有 GUI 的会话里做）

1. 打开 `~/Applications/AICC.app` → 「AI 账号」，确认能看到
   WorkBuddy / Qoder CN / Qoder International 三段，并且页面顶部写明"只读管理、切换未开放"。
2. 点 Qoder International 的 `e***@gmail.com` 卡片 → 「检查授权」，
   在钥匙串弹窗点「始终允许」→ 应显示"认证有效"、Token 指纹与到期时间。
3. 点「添加账号」建一个空 Profile → **手动**打开 Qoder 客户端登录另一个账号
   → 回到 AICC 点「保存当前登录」→ 快照应变为就绪，live 不变。
4. 主按钮应始终是灰的「切换未开放」；右键菜单里不应出现「打开」/「切换」。
5. 退出 AICC 再打开，确认 Profile 列表、认证状态、`live` 当前账号都还对得上。

> 真机多账号切换**不在本轮验收范围**：在完整隔离方案落地前，验收只包含
> 识别、扫描、导入、认证检查、保存快照，以及"切换被拒绝"这一负向结论。
