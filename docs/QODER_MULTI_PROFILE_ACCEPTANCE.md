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
| 切换 `profiles.open` / `activate` | **默认拒绝** | 需要 `AICC_QODER_ENABLE_SWITCH=1` 且版本已复验；CN 永远不满足 |
| 关闭 `profiles.close` | 可用 | 只对"命令行带本 Profile 标记"的进程发信号 |
| 恢复 `profiles.recover` | 可用 | 只处理崩溃遗留的 journal |

原因见 `docs/QODER_MULTI_PROFILE_SPIKE.md` §7：`main.sqlite` 里存在按 `account_id`
分区的账号数据（agent 定义、工作角色、协作运行时凭据），只换两个认证文件无法隔离。

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

0. **隔离未成立 → 切换默认关闭**（新增）：`main.sqlite` 的账号绑定数据无法用两个认证文件隔离；
   要恢复切换，先完成 SPIKE §7.2 的最小安全快照设计并重新做真机验证。

1. **Qoder CN 未安装**：CN 的 Bundle ID、数据目录、钥匙串条目名、`--user-data-dir`
   行为都还没有真机复验。代码按既有实现与官方发行标识处理，并在
   `verifiedOnThisMachine=false` 与界面文案里明确标注。
2. **`qoder.profiles.inspect` 未拿到明文**：钥匙串条目 ACL 绑定创建者，第一次读会弹
   系统授权框；自动化终端里点不到那个框（等待 4 分钟超时）。因此
   `authFingerprint` / `expiresAt` 仍为空，快照状态停在"待检查"。
   这一步需要在 GUI 里点一次"始终允许"，之后不再弹。
3. **进程探测现在是三态**：`pgrep` 不可用/退出码异常 → `unknown`，界面显示"状态未知"，
   此时**禁止任何 live 写操作**；`ps` 读不到命令行时用 `kill -0` 兜底，
   只有拿到 ESRCH 才判"已退出"。归属判定只认 `AICC_QODER_PROFILE` 标记。
4. 切换依赖"客户端已退出"。如果用户在切换的瞬间手动打开客户端，
   AICC 会检测到非自己启动的进程并**拒绝**切换（有单测覆盖），但无法阻止用户
   在写文件的那几百毫秒里抢先启动。

## 5. 人工补验步骤（在有 GUI 的会话里做）

1. 打开 `~/Applications/AICC.app` → 「AI 账号」，确认能看到
   WorkBuddy / Qoder CN / Qoder International 三段。
2. 点 Qoder International 的 `e***@gmail.com` 卡片 → 「检查授权」，
   在钥匙串弹窗点「始终允许」→ 应显示"认证有效"与到期时间。
3. 点「添加账号」建一个空 Profile → 「登录并保存」→ 在官方客户端里登录另一个账号
   → 回到 AICC 点「保存当前登录」。
4. 两张卡片来回切 5 次，确认每次 `activeProfileId` 跟着变、客户端里显示的是对应账号。
5. 退出 AICC 再打开，确认 Profile 列表、当前账号、运行状态都还对得上。
