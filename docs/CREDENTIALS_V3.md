# 凭据存储 v3：本地 JSON + 动态 Magpie

> 状态：**CRED-1 已合并**（`LocalJsonStore` + 引用解析 + 文件系统边界加固）；
> **CRED-2 已合并**（SQLite v20 + Magpie 动态关联 + 统一 Broker 解析）；
> **CRED-3a 已合并**（WorkBuddy 自有授权 JSON 化，**默认关闭**，见 §8ter）；
> **CRED-3-UI 已合并**（明文的告知 / 确认与开关，见 §8quater）；
> **CRED-3b 已交付待验收**（Qoder 设备授权 / PAT 的存储分派、读取与删除，见 §8quinquies）。
> CRED-4（旧 Keychain 迁移）/ CRED-5（免弹窗收口）尚未开始。
>
> 基线：`main` @ `8da2e80`（#86 已合并），数据库 Schema **v21**（CRED-3b 复审为 `qoder_authorizations` 增加 `auth_version`）。
> 范围：只动凭据的**存取方式**，不动账号模型、签到 / 成长 / 旅行 / Credits 业务。

## 1. 为什么要换掉钥匙串

后台助手是 ad-hoc 签名的可执行文件，每次重新构建 cdhash 就变，macOS 于是重新询问
"允许 AICC 访问钥匙串"。用户点过"始终允许"也没用：下一次构建再来一次。结果不只是
弹窗烦人 —— 授权框是**阻塞**的，Worker 的 RPC 队列会被一起卡住，签到、额度、旅行
全部停摆。

本地明文 JSON 换掉的正是这条链：文件读写不需要系统授权，也就没有弹窗和阻塞。
代价必须说清楚：**这是明文存储**，`0700` / `0600` 挡得住同机其他用户，挡不住同一个
macOS 用户权限下的恶意程序。首次使用要向用户明示，建议开 FileVault，不要把
`secrets/` 放进云同步或未加密备份。

## 2. 目标架构

```text
Magpie 授权             AICC 自有授权
（原始 JSON，动态读）     （浏览器 / 扫码 / PAT）
        │                       │
        │                   本地 JSON（一份授权一个文件）
        └──────────┬────────────┘
                   ▼
          统一 Credential Broker
        （默认 Magpie 优先，自有兜底）
                   ▼
        SQLite v20：账号、来源、引用、状态
```

两种授权的边界是硬的：

| 项目 | Magpie 授权 | AICC 自有授权 |
| --- | --- | --- |
| 凭据所有者 | Magpie | AICC |
| access | 每次现读，不持久化 | 保存 |
| refresh | **严禁读取 / 复制 / 刷新** | 保存登录实际返回的值 |
| 落盘位置 | `~/.config/magpie/plugin-auth.json` | `<data>/secrets/v1/<uuid>.json` |

## 3. 引用方案（CRED-1 交付）

SQLite 只存引用串。三种方案由**显式前缀**决定，没有"猜"的余地：

| 引用 | 含义 | 读失败能否兜底到钥匙串 |
| --- | --- | --- |
| `keychain:<key>` | 旧钥匙串凭据（未迁移） | 它就是钥匙串 |
| `localjson:v1:<id>` | AICC 自有 JSON | **不能**（否则迁移后还会弹窗） |
| `magpie:v1:<source-key>` | Magpie 动态关联 | **不能** |

`CredentialRef::parse` 对任何不认识的前缀直接报错 —— 包括 `localjson:v2:`、
`magpie:` 这种"看起来像"的写法。裸键（忘写前缀）同样被拒。

槽位必须由调用方显式指定：`CredentialSlot::Access` / `Refresh`。同一份 WorkBuddy
凭据里 access 与 refresh 共存，靠"读第一个非空字段"区分就会把 refresh 当 access 用。

## 4. 存储布局与文件结构

```text
~/Library/Application Support/AICC-Dashboard/accounts/
├── accounts.sqlite
├── secrets/
│   └── v1/
│       ├── .lock              # 跨进程锁（0600）
│       ├── <uuid>.json        # 一份独立授权一个文件（0600）
│       └── ...
├── backups/
└── logs/
```

```json
{
  "schemaVersion": 1,
  "secretId": "3f2a…（随机 UUID，同时是文件名）",
  "authorizationId": "对应 account_authorizations.id",
  "platform": "workbuddy",
  "source": "own",
  "credentialKind": "oauth",
  "revision": 0,
  "expiresAt": "2026-12-01T00:00:00Z",
  "credentials": { "accessToken": "…", "refreshToken": "…" }
}
```

硬约束：

* 文件名必须是随机 UUID，**不**采用 UID / 邮箱 / 昵称。
* 同一账号的浏览器授权与 PAT 各占一个文件，互不覆盖。
* `PAT` 是独立类型（`credentialKind: "pat"`），不允许携带 refresh，也不允许自动刷新。
* 反序列化用 `deny_unknown_fields`：多出来的字段直接报错，避免"字段放错位置被静默忽略"。

## 5. 写入保证

| 保证 | 实现 |
| --- | --- |
| 权限 | 目录 `0700`（每次操作顺带收紧），文件 `0600`（显式 chmod，防 umask） |
| 目录不可被替换 | `secrets` 与 `v1` 两级目录逐个 `symlink_metadata` 校验：软链 / 非目录 / 属主不是当前用户 / 组或其他可写 一律拒绝，**且先校验再 chmod** |
| 无半截文件 | 同目录 `.tmp-*` 写完 → `fsync` → `rename` 原子替换 → 目录 `fsync` |
| 并发不互相盖 | `secrets/v1/.lock` advisory 锁：`O_NOFOLLOW` 打开、校验属主、`nlink == 1`（挡硬链接）、fd↔路径同一 inode、`fchmod` 收紧到 `0600`；等锁有上限（默认 2s），超时报 `lock_timeout` |
| 旧快照不覆盖新令牌 | `revision` 检查：对不上返回 `conflict`，一个字节都不写 |
| 归属不可变 | `write()` 额外比对 `authorizationId` / `platform` / `source` / `credentialKind`，不一致返回 `conflict`；换归属只能新建凭据 |
| 崩溃可恢复 | 残留 `.tmp-*` 由 `sweep_orphans` 清理；目标文件永远是"旧内容或新内容" |
| access + refresh 成组 | `replace_credentials` 在锁内一次性换两值 |
| 不误判"没有凭据" | `Denied` / `Unsafe` / `Corrupt` 与 `NotFound` 分开上报 |

> 残余窗口：目录与锁文件的校验是"lstat → chmod/open"，理论上仍有极短的
> TOCTOU 窗口。这里没有做 `openat` 全链路（std 不提供），因为前提是攻击者已经能在
> 该用户的 `0700` 目录里改名——那已经等价于同一用户权限下的完全控制。
>
> 平台差异：`O_NOFOLLOW` 与 `ELOOP` 的数值在 macOS / Linux 上不同，两者都按
> `target_os` 取值，不写死单一平台。凭据文件刻意**不**要求 `nlink == 1`
> （`rsync --link-dest` 之类会正常产生硬链接，那份也是 `0600` 同一属主），
> 只有锁文件要求——我们会 `fchmod` + `flock` 它，改动会落到共享 inode 上。

## 6. 读路径的硬规定

* 读之前先验 `secrets` / `v1` 两级目录：目录是软链直接 `unsafe`，**不读**目录之外的 JSON。
* 符号链接、非普通文件、属主不是当前用户、权限过宽（组 / 其他可访问）一律 `unsafe`，拒绝读。
* 文件内 `secretId` 与文件名不一致 → `corrupt`。
* 更高的 `schemaVersion` → `unsupported`，不猜字段。
* JSON 解析失败只报行列号：`serde` 的原始消息会把字段值抄进错误里，那可能是令牌。
* 坏文件不参与读取，但**不删、不覆盖**；`scan()` 把它单独列进 `broken`，健康文件照常可用。

## 7. CRED-1 的范围

**做了**：`src/credentials/reference.rs`（引用解析）、`src/credentials/localjson.rs`
（存储）、35 个测试（并发写、revision 冲突、锁超时、注入写失败、rename 前中断、
文件损坏、权限过宽、符号链接、目录穿越、Token 不泄漏等）。

其中文件系统边界部分是 #82 复审后补的（两个 P0 + 一个 P1）：

| 问题 | 修法 | 回归测试 |
| --- | --- | --- |
| `secrets` 父目录未校验，且先 chmod 再验 | 目录链逐个 lstat，先拒软链 / 非目录 / 非本人属主，再 chmod，chmod 后复查 | `secrets_dir_symlink_is_refused_and_the_target_is_untouched` |
| `v1` 是软链时 `load()` 仍能读到外部 JSON | 读路径也走 `validate_dirs()` | `schema_dir_symlink_is_refused_even_on_a_direct_load` |
| `.lock` 未 no-follow，还会 chmod 目标 | `O_NOFOLLOW` + 属主校验 + fd↔路径 inode 比对 + `fchmod` | `lock_file_symlink_is_refused_and_the_target_is_untouched` |
| `write()` 可改写授权归属 | 归属字段不可变，不一致返回 `conflict` | `write_cannot_change_the_authorization_identity` |
| 并发测试只是线程 | 保留线程用例并改名，新增真实 4 进程竞争 | `concurrent_writers_across_processes_let_exactly_one_win` |

**没做**（后续 PR）：任何真实账号迁移、SQLite v20、Broker 接入、登录路径改造、
旧 Keychain 迁移 UI、Worker 免弹窗收口。本 PR 不改变生产默认行为，也不创建
`secrets/` 目录 —— 没有任何调用方。

## 8. 阶段顺序

| PR | 内容 | 状态 |
| --- | --- | --- |
| CRED-1 | JSON 存储底层 + 引用解析 | 已合并（#82、#83） |
| CRED-2 | SQLite v20 + Magpie 动态授权 | 已合并（#84） |
| CRED-3a | WorkBuddy 自有授权 JSON 化 | 已合并（#85） |
| CRED-3-UI | 明文的告知 / 确认与开关 | 已合并（#86） |
| CRED-3b | Qoder CN / International 设备授权 + PAT + Credits 读取 | **本 PR**（见 §8quinquies） |
| CRED-4 | 旧 Keychain 显式迁移（备份 / 核对 / 回滚） | 待开始（两平台都 JSON 化之后） |
| CRED-5 | Worker 免弹窗收口 + 端到端验收 | 待开始 |

## 8bis. CRED-2：Magpie 动态关联

**做了什么**

| 项 | 落点 |
| --- | --- |
| SQLite **v20**：`account_authorizations.storage_kind` + `ref_updated_at` | `storage/authz.rs::migrate_v20` |
| 迁移**只回填、不改引用**（不猜归属） | 同上 |
| Broker 按引用分派：`keychain` / `localjson` / `magpie` | `auth/broker.rs::access_for` |
| Magpie 现读：平台 + provider + UID + 有效期校验 | `auth/magpie_link.rs` |
| 导入只登记 `magpie:v1:<source_key>`，**零钥匙串写入** | `auth/service.rs::save_magpie` |
| 旧别名键只给仍是 `keychain` 引用的授权写 | `auth/service.rs::sync_mirror` |
| 显式重新关联 `auth.magpie.relink`（幂等、留旧键） | `auth/relink.rs` |
| `health.authorizations.storage` 按引用统计三种存储 | `handlers.rs::authorization_fleet` |

**几条硬规则**

1. `save_magpie` 不再接收 access：令牌只存在于 Magpie 的文件里，AICC 一行都不存。
2. 来源键是硬要求。没有 `source_key` 就无法确定对应哪条登录态，宁可报
   `magpie_source_key_missing` 也不猜。
3. 读路径分三类失败：`missing_credential`（真没有）、`link_mismatch`（串号，
   拿错账号/平台）、`denied`（读不了）。串号**不会**被兜底成"没凭据"。
4. 重新关联只改 `access_ref` / `source_key` / `storage_kind`，`version` +1；
   旧钥匙串条目保留，删不删留给用户（CRED-4）。
5. `storage_kind` 列是**审计留痕**：Rust 侧一律由 `access_ref` 前缀推导，
   统计 SQL 也用前缀算，列漂移不会影响判断。

**本阶段没做**（照 PRD 留给后续）

- 自有授权（浏览器 / 扫码 / PAT）仍在钥匙串 —— CRED-3 换成 `localjson:v1:`。
- 旧 Keychain 批量迁移与 UI —— CRED-4。
- Worker 全链路的免弹窗收口与端到端验收 —— CRED-5。
- `auth.magpie.relink` 还没有界面入口；当前由 RPC 显式调用。

**复审后补的边界（#84 第二轮）**

| 问题 | 修法 |
| --- | --- |
| 旧 `tokenExpiresAt` 挡住动态新令牌 | `is_usable` 对 `magpie` 引用直接放行，有效性交给文件判；`describe` 增 `effectiveStatus` / `recoverable`，库里过期但文件有效时界面显示"可恢复" |
| 重同步按 UID 裸匹配、把文件不可读误写成 `expired` | 先认 `source_key`，缺 key 时要求「平台 + provider + UID」唯一匹配；文件不可读返回 `state=unavailable` 且**不改状态**；只有真的过期才写 `expired` |
| 重新关联缺并发保护 | `rebind_authorization_ref` 带 CAS：`id + kind='magpie' + version + access_ref` 全中才写，否则 `superseded`，不重试 |
| 删除动态关联仍访问钥匙串 | `delete_authorization` 对 `magpie` 引用不再生成任何钥匙串键（含旧别名），旧 `keychain:` 行仍按旧规则清理 |
| 诊断分类混在一起 | 新增 `CredentialErrorKind::Unavailable`（→ `magpie_unavailable`）；未知引用前缀记为 `unknown` 而不是 `keychain` |

**复审第二轮（#84 第三次提交）**

| 问题 | 修法 |
| --- | --- |
| `accounts.delete` 无条件碰钥匙串 | 先读授权快照再算清理计划：`keychain:` 行按自己的引用清、动态行**一个键都不产生**、旧别名只在"还有钥匙串凭据或根本没有授权行"时才清；顺序改为**先清凭据再删库**，清理失败保留账号以便重试（不产生无法按 id 重试的孤儿） |
| `save_magpie` 无跨进程 CAS | `save_magpie_guarded(..., MagpieGuard)` 用 `version + access_ref` 做 CAS（`replace_authorization_cas`），竞争返回 `superseded`；resync 传入"读文件之前"那一行的快照 |

**已知代价**

借用授权不再写旧别名键，所以 PR #65 时代的二进制看不到"当前借用令牌"。
回退到旧版本必须用迁移前的数据库备份（PRD §7.4 已接受）。

## 8ter. CRED-3a：WorkBuddy 自有授权 JSON 化

**落点矩阵**（`auth/service.rs::write_own`）

| 账号现状 | 写哪里 | 回滚方式 |
| --- | --- | --- |
| 还没有自有授权行 | 新建 `<uuid>.json`，`localjson:v1:<uuid>` | 删掉刚建的文件 |
| 已经是 `localjson:` | **覆盖同一个文件**（一份授权一个文件，不留孤儿） | 把旧 access/refresh 写回去（带 revision 校验） |
| 还是 `keychain:`（CRED-4 之前的老账号） | 维持钥匙串写入，一个字段都不迁移 | 快照恢复 |

**三条硬规则**

1. **引用与状态一次提交。** `record_grant` 用单条 UPSERT 同时写 `access_ref` /
   `refresh_ref` / `status` / `token_expires_at` / `storage_kind` / `version+1`。
   先写引用再写状态会留下「新凭据 + 旧状态」或「新状态 + 旧引用」，后者会让库永远
   指着一份不是这次登录写出来的文件。
2. **删除顺序固定为「先凭据、后记录」。** 反过来的话，文件名是随机 UUID，行一删就再也
   定位不到那个文件，只能扫目录找孤儿，而孤儿里躺着的是**有效令牌**。现在这一侧最坏
   是「文件已删、行还在」：读得到 `not_found`，界面提示重新登录，而删除可以原样重试
   （删文件、删行都幂等）。行删除失败会返回 `recordError = record_delete_failed`，
   不谎报成功。
3. **老账号不迁移。** 只要行的引用还是 `keychain:`，读写都照旧：`write_own` 走原快照
   路径、`read_access` / `read_own_refresh` 走钥匙串、删除按旧规则连旧别名一起清。
   CRED-3 一个字节都不搬 —— 迁移是 CRED-4 的显式动作。

**刷新路径**（`handlers.rs::persist_rotated_json` / `commit_rotated_json`）

* `replace_credentials` 在锁内一次换掉 access + refresh，不存在"新 access + 旧 refresh"；
* 写失败 → 文件保持旧内容（原子替换），但服务端已作废旧 refresh，所以行标 `invalid`；
* `revision` 冲突 → `superseded`（不覆盖、不标失效），盘上那份是别人先写成功的新令牌。

**删除清理矩阵**

| 引用 | 钥匙串键 | 本地文件 | 旧别名 |
| --- | --- | --- | --- |
| `keychain:` | 自己的 access / refresh | — | 最后一份时清 |
| `localjson:v1:` | **不产生任何键** | 删自己那个文件 | 不清（从没写过） |
| `magpie:v1:` | **不产生任何键** | — | 不清（CRED-4 的回滚依据） |

`DeleteReport` 分列 `removedKeys` / `removedSecrets` / `failedKeys` / `failedSecrets` /
`recordError`；`accounts.delete` 的响应同样分列 `failedKeys` 与 `failedSecrets`。

**本阶段没做**

- Qoder 的浏览器 / 设备授权、PAT（`credits` 读取路径）—— CRED-3b。
- 旧 Keychain 批量迁移与 UI —— CRED-4。
- Worker 全链路免弹窗收口与端到端验收 —— CRED-5。

**已知限制**

钥匙串与 SQLite 之间没有跨存储事务（`#84` 复审已记录）。可能出现的中间态只有一种：
凭据已删、记录还在。两个删除步骤都是幂等的，重试即收敛；CRED-4/5 会补一致性检查与
恢复提示。

**复审后补的边界（#85 第二轮）**

| 问题 | 修法 |
| --- | --- |
| 引用只是 UUID，没校验文件属于这一行 | `auth/owned.rs`：读 / 刷新 / 覆盖写 / 删除四个入口统一 `load` + 校验 `authorizationId` / `platform` / `source` / `credentialKind`，不符返回 `conflict`，**不返回令牌、不写、不删**；整账号删除逐行验归属（`SecretPlan { owned, refused }`），有 `refused` 就整次拒绝 |
| 首次并发登录两个 UPSERT 都成功 → 孤儿文件 | `write_own` 把"当时看到的是不是已有行"记进 `GrantExpectation`：没有行只允许 `INSERT ... DO NOTHING`，有行走 `version + access_ref` CAS；输的那一方只删"没有任何行引用"的文件（`count_authorizations_referencing`） |
| 非法引用被静默重建 / 回退钥匙串 | `write_own` 遇到 `magpie:` / `unknown` 直接报 `credential_ref_unusable`；`read_own_refresh` 只有明确 `keychain:` 才走钥匙串；Broker 额外校验"引用种类配得上授权种类" |
| 明文落盘早于风险告知 | 新登录**默认不写 JSON**：`credentials_json_consent` 未置位时继续写钥匙串，`health.credentials` 暴露 `consentRequired`，登录响应带 `credentialStorage` / `jsonConsentRequired` |

**复审第三轮（#85 第三次提交）**

| 问题 | 修法 |
| --- | --- |
| 轮换写（`persist_rotated_json`）绕过归属校验 | 写之前也走 `owned::load`；`commit_rotated_json` 改成吃"已验证快照"，`revision` 与旧 refresh 都从它取。串号返回 `credential_ownership_mismatch` 且**不**标 `needsRelogin`（重新登录只会把坏引用盖成一条新授权） |
| `settings.get` 单键读不到新设置 | `broker::read_setting` 认 `credentials_json_consent`，返回 `"1"` / `"0"`；全量查询复用同一个 `stored_value()` |
| 跨进程证据只有线程级 | 新增离线 harness `src/cred_harness.rs`：两个**真进程**共享临时 SQLite + `secrets/`，文件握手摆出竞争窗口，父进程断言最终状态 |

**跨进程 harness 的两个场景**

| 用例 | 断言 |
| --- | --- |
| 并发首次登录 | 恰好一个 `won` / 一个 `superseded`；DB 只有一条 own 授权；`secrets/` 只剩 1 份且就是被引用的那份，内容属于赢家 |
| 迟到的登录（另一个进程） | 引用被挪走后旧快照写入被拒（`superseded`）；行仍是新引用；迟到者写的旧文件被清掉，不留有效令牌 |

harness 不联网、不使用真实令牌（`SecureStore::memory` + 临时目录），跑一次约 0.1s。

**复审第四轮（#85 第四次提交）**

| 问题 | 修法 |
| --- | --- |
| 轮换写完文件后不校验 SQLite 引用是否还在 | `commit_rotated_json` 在文件写完后用 `record_status` 做 `version` CAS：失败就把文件写回旧内容并返回 `superseded`（`fileReverted` / `orphanSecret` 如实上报），绝不报 `valid` |
| `record_status` 的返回值被 `let _ =` 吞掉 | 分三支处理：`Ok(true)` → valid；`Ok(false)` → 再读一次行决定（见下）；`Err` → `rotation_not_committed`（文件保留新令牌，因为旧 refresh 已被服务端作废，回滚只会让账号更坏） |
| **CAS 失败一律当成"引用被迁走"** | `Ok(false)` 之后再读一次授权行：仍指向同一份文件 → **保留新令牌**，返回 `authorization_version_changed`；引用换了 / 行没了 → 才回滚并返回 `authorization_moved`；行读不出来 → 保留新令牌，返回 `rotation_state_unknown` |
| 老 keychain 行同样"写完就报成功" | 钥匙串分支也检查 `record_status` 的 CAS 结果 |

**为什么是"写完再撤销"而不是"先查一次"**

文件写和 SQLite 提交跨两个存储，没有事务可用。写前查一次挡不住写后迁移；真正的判据是
**写完之后这次提交还能不能成立**：

```text
owned::load（验归属，取 revision R）
      ↓   ← 另一个进程 CAS 把引用从 F 迁到 G（F 保留）
写 F（R → R+1）                       ← 这一步无法阻止
record_status（CAS on version）→ false
      ↓
把 F 写回旧内容（R+1 → R+2），返回 superseded
```

F 已经不被任何行引用，没人会读到那半秒的新令牌；最终状态是"F 里是旧值、G 未被碰过、
行指向 G、界面收到 superseded"。回归：`rotation_supersedes_when_the_authorization_moves_mid_flight`
（进程内确定性交错）与 harness 的 `a_rotation_in_another_process_reports_superseded_after_the_reference_moves`（真双进程）。

**但"版本变了"不等于"引用变了"**：另一支只更新状态、仍指向 F 时，把 F 回滚成旧令牌等于
毁掉服务端之后唯一有效的 refresh。所以 `Ok(false)` 之后必须再读一次行：

| 重新读取的结果 | 处理 | 回应 |
| --- | --- | --- |
| 仍指向 F | 保留新令牌（回滚风险更大） | `superseded` / `authorization_version_changed` / `fileReverted=false` |
| 指向 G 或行已删 | 撤销对 F 的写入 | `superseded` / `authorization_moved` / `fileReverted` + `orphanSecret` |
| 读不出来 | 保留新令牌，不声称成功 | `invalid` / `rotation_state_unknown` |

回归：`rotation_keeps_the_new_tokens_when_only_the_row_version_changes`。

### 归属校验为什么必须在业务层

文件 `0600` 只回答"别的用户能不能读"，回答不了"这个文件是不是这条授权的"。引用写错
（迁移出错、并发覆盖、库损坏）时，按 UUID 直接读就会把**另一个账号的有效令牌**当成自己的。
四个入口全部校验，且错误分类为 `conflict`（`credential_ownership_mismatch`），不是
`not_found` —— 后者会被上层当成"没凭据，请重新登录"，把串号糊过去。

### 并发首次登录的收敛方式

```text
进程 A: 读库(无 own 行) → 写 file-A → INSERT ... DO NOTHING ✔
进程 B: 读库(无 own 行) → 写 file-B → INSERT ... DO NOTHING ✘ → superseded
                                        → 删 file-B（无任何行引用）→ 返回 superseded
```

已有行则走 `version + access_ref` CAS：谁先把引用改掉，后到的旧快照写不进去。
**不回滚赢家的文件**：清理前先查 `count_authorizations_referencing`，被引用就一个字节都不动。

### 明文存储的开关

| 状态 | 新登录写哪 | 界面提示 |
| --- | --- | --- |
| `credentials_json_consent` 未设置（默认） | 钥匙串（与 #84 一致） | `health.credentials.consentRequired = true`，登录响应 `jsonConsentRequired = true` |
| 置为 `1` | 新建 `<uuid>.json` | `jsonStorageEnabled = true` |

这条闸门只管**新建**：已有 JSON 行照旧读写（文件早就落盘了，退回钥匙串不会减少暴露，
还会让同一份授权分裂成两处）。老 `keychain:` 行不因开关变化而迁移。

## 8quater. CRED-3-UI：明文的告知、确认与开关

助手侧只提供"能不能写明文"的判据，**是否同意由用户在界面上决定**。这一阶段没有改任何
凭据读写逻辑，只把开关接到人。

| 界面 | 位置 | 行为 |
| --- | --- | --- |
| 凭据存储 | 设置 → 凭据存储 | 显示当前方式（钥匙串 / 本地 JSON）、各来源条数、`钥匙串自检` |
| 启用 | 同上 →「启用本地 JSON…」 | 先弹风险说明；点「我已了解，启用」才 `settings.set credentials_json_consent=1` |
| 风险说明 | `PlaintextCredentialSheet` | 明文落盘、0600 的边界、FileVault、备份 / 云同步、只影响新建授权、可随时关闭、Qoder 与第三方钥匙串不受影响 |
| 拒绝 | 同上 →「仍使用钥匙串」 | **什么都不写**：助手默认就是钥匙串，不需要"拒绝"这个状态 |
| 关闭 | 已启用时的「改回钥匙串…」 | 写 `0`；只影响之后新建的授权，已写成 JSON 的继续可用 |

数据来源只有一处：`health.credentials`（开关与风险文案）+ `health.authorizations.storage`
（各来源条数）。界面不自己推断，也不缓存"用户点过"的本地值 —— 关掉助手再打开，显示的是
助手真实状态。

**老助手兼容**：两个字段缺失时 `CredentialStoragePolicy.parse` 返回 nil，界面显示
"这个版本的内嵌助手还没有本地 JSON 存储"，而不是一个点不动的开关。

**验证**

| 手段 | 结果 |
| --- | --- |
| `xcrun swiftc -typecheck`（全量 Sources + Widget 共享源码） | 通过，无新增 warning |
| `swift test`（CI：`swift-test`） | 新增 3 条：解析、标题 / 摘要、老助手缺字段 |
| `bash scripts/smoke-test-dashboard-visual.sh` | 设置页渲染通过（新增段在浅色 / 深色下均可读） |
| `AICC_ALLOW_ADHOC=1 bash macos/build-aicc-swiftui.sh` | App + Widget + 内嵌助手签名与版本校验通过 |
| 构建产物 RPC 走查 | `settings.get` → `0`；`set 1` → `health.credentials.jsonStorageEnabled = true` |

> 本机没有 Xcode / XCTest，`swift test` 只能靠 CI；本地用 `swiftc -typecheck` + 离屏渲染
> 覆盖编译与版式，两者的边界已在上面写清。

## 8quinquies. CRED-3b：Qoder 自有授权 JSON 化

Qoder 有**两条**自有授权来源，各占 `qoder_authorizations` 的一行：设备授权
（`own_browser`，OAuth）与 PAT（`own_pat`）。本期把两者都接入 `LocalJsonStore`，
并让 Credits 的读取与账号删除**按引用**工作。**没有**批量迁移旧凭据（那是 CRED-4）。

### 落点规则（只看这一行现在是什么）

| 行的状态 | 新令牌写到哪里 |
| --- | --- |
| 还没有这一行，且用户没同意明文 | 钥匙串（与 CRED-3b 之前完全一致） |
| 还没有这一行，且已同意明文 | 新建 JSON 文件（随机 UUID） |
| 已是 `localjson:v1:` | 覆盖同一文件（`revision` 校验，归属不可变） |
| 仍是 `keychain:` | 继续写钥匙串，**不隐式迁移** |
| 引用是 magpie / 未知前缀 / 指向别的键 | 显式拒绝（`unsupported` / `conflict`），不回退 |

同一账号的设备授权与 PAT 是**两个文件、两个引用**，互不覆盖：删掉 PAT 不影响设备授权，
反之亦然。

### 文件内容

| 字段 | 设备授权 | PAT |
| --- | --- | --- |
| `platform` | 实际 edition：`qoder_cn` / `qoder_global` | 同左 |
| `source` | `own` | `own` |
| `credentialKind` | `oauth` | `pat` |
| `authorizationId` | `qoder_auth_id(account, "own_browser")` | `qoder_auth_id(account, "own_pat")` |
| `accessToken` | 接口实际返回的 device token | 用户粘贴的 PAT |
| `refreshToken` | 只在服务端真的返回时保存 | **恒为空**（PAT 不刷新） |
| `expiresAt` | 以接口响应为准 | 恒为空（失效靠 401 发现） |

### 条件提交：不加列

`qoder_authorizations` 没有 WorkBuddy 那样的 `version` 列。沿用既有字段做守卫，
**不新增迁移**：

| 场景 | 判定 |
| --- | --- |
| 首次授权，两进程同时写 | `INSERT`，唯一键冲突 → `None`（放弃，不覆盖） |
| 重复授权 / PAT 更新 | `UPDATE ... WHERE secret_ref IS <写凭据前看到的那条>` |
| 引用被别人迁走 | 同上，`secret_ref` 对不上 → `None` |
| 删号后同 UID 重建 | `EXISTS(... AND generation = ? AND auth_epoch = ?)`，单看 epoch 挡不住 |

`None` 的收尾分两种：**新建**的文件只在"没有任何行引用它"时删掉（`qoder_authorizations_referencing`）；
**覆盖写**的那份保持不动（盘上那份本来就属于这条授权，里面是刚拿到的有效令牌）。

### 四条生产路径

| 文件 | 改动 |
| --- | --- |
| `providers/qoder_secret.rs`（新） | 落点决策、归属校验（4 个判据）、写入 / 回滚 / 删除、条件提交封装 |
| `providers/qoder_auth.rs::persist` | 令牌按引用分流；授权行改为条件提交；失败回滚凭据 + 新建账号 |
| `providers/qoder_pat.rs::persist / read_secret / remove` | 同上；读取按行分派，删除只清这一来源 |
| `credits.rs::resolve_token` | 按 `secret_ref` 读凭据；读失败**不换来源**，错误保留分类（`not_found` / `conflict` / `denied` …） |
| `providers/qoder_admin.rs::delete` | 先读授权快照制定清理计划，再删凭据、删行；JSON-only 账号全程不碰钥匙串 |

### 存储前后行为对照

| 行为 | CRED-3b 之前 | CRED-3b 之后 |
| --- | --- | --- |
| 新设备授权（同意明文） | 钥匙串 `qoder/<id>/own/access|refresh` | 一个 JSON 文件；钥匙串零访问 |
| 新 PAT（同意明文） | 钥匙串 `qoder/<id>/pat/token` | 另一个 JSON 文件；不含 refresh |
| 未同意明文 | 钥匙串 | 钥匙串（不变） |
| 老账号（已授权） | 钥匙串 | 钥匙串（**不迁移**，CRED-4 显式做） |
| Credits 读取 | 固定按账号键名读钥匙串 | 按授权行 `secret_ref` 分派 |
| 引用坏了 | 读不到就报错 | `not_found` / `conflict` 显式分类，仍不回落 |
| 删号 | 无条件删三条固定钥匙串键 | 按引用清理；引用不可信时**拒绝删除** |
| JSON-only 账号删号 | 会碰三次钥匙串（可能弹框） | 零次钥匙串访问 |
| 并发首次授权 | 后写覆盖先写 | 只有一个成功，失败方不留孤儿令牌文件 |

### 已知限制

1. **不做迁移**：CRED-3b 之前登录的账号，凭据仍在钥匙串，读取时仍可能触发一次系统授权。
2. **服务端不下发 refresh 时保留旧值**（与钥匙串路径一致）：如果服务端换了会话，
   旧的 refresh 会留在文件里直到下一次真的下发。
3. **PAT 不刷新**：401 只能提示用户重新生成；AICC 没有轮换它的能力。
4. **一个账号一份凭据一个文件**：不接受"一份文件装多平台"，换账号必须新建。

### 复审修复（P0-1 ~ P0-4）

首轮验收提出四项并发 / 删除缺陷，均已在本期修复，**没有新增 SQLite 列**。

| 编号 | 问题 | 修法 | 错误时的副作用 |
| --- | --- | --- | --- |
| P0-1 | 登录写凭据时**重读**授权行，两次观察可能不同：输家把赢家的文件当成"自己的"覆盖写，随后提交撞唯一键报冲突 —— 赢家行还 `valid`，令牌已被改写 | `qoder_secret::Observation`：登录流程只调用一次 `observe()`，"写哪里"与"怎么提交"来自同一份快照；`write()` 不再自己查库 | 输家只可能**新建**一个文件；提交失败后仅在"没有任何行引用"时删掉它，赢家的文件与 `revision` 一字未动 |
| P0-2 | `Absent` 预期不带生命周期：账号删号重建后 `account_id` 相同、`generation` 变了，旧登录仍能 INSERT | `Absent { lifecycle }` + `INSERT ... SELECT ... WHERE EXISTS(generation, auth_epoch)`；`Ok(0)` 一律当竞争失败（**不能**把 `Ok(_)` 当成功，SQLite 的 `WHERE` 不成立只插 0 行） | 旧登录被拒，新账号里不会出现旧会话的授权，`auth_epoch` 也不会被推进 |
| P0-3 | 删号计划只在 scheme 层判断，串号要等逐份删除时才发现；Magpie-only 账号还会走"三条固定键"兜底去碰钥匙串 | 计划阶段就 `load_json` 逐份校验四个归属字段（`NotFound` 视为可重试）；兜底条件收紧为"授权表**一行都没有**" | 任一份校验失败 → 整次拒绝：合法凭据内容与 `revision` 不变、自动领取开关与生命周期不变、账号与行都在 |
| P0-4 | 单独移除 PAT 先删凭据、再无核验删行：并发保存新 PAT 后，旧移除会删掉新令牌 / 新行 | 顺序改为「① 观察一次 → ② 带核验删行（`secret_ref` + `generation` + `auth_epoch`）→ ③ 按版本删凭据（`delete_at_revision` / 钥匙串字节比对）→ ④ 失败时把行以 `missing` 放回」；界面拿到 `authorizationChanged` | 交错的移除**什么都不删**：新 PAT 原样可读、行还在；删凭据失败则行以 `missing` 恢复，可重试且幂等 |

删除顺序的代价写在 `qoder_pat::remove` 的文档里：② 与 ③ 之间进程被强杀会留下一个
没有行引用的文件。这是**有意**选的——宁可留一份 `0600` 的孤儿文件（可被后续核对扫到），
也不能删掉用户刚保存的新令牌。

### 第二轮复审修复（P0-A / P0-B / P1-C / P1-D）

| 编号 | 问题 | 修法 | 错误时的副作用 |
| --- | --- | --- | --- |
| P0-A | 移除 PAT 时"行已删、文件才被发现重写"（`revision` 变了），旧实现记一行日志就返回 `removed: true`：盘上留下一份**有效却无任何引用的令牌**，再点一次也找不到它 | 该分支改为把行以 `missing` 放回（可定位、可重试、Credits 不用），返回 `removed: false` + `pending_cleanup: true`；RPC 新增 `pendingCleanup` | 不假报成功；文件仍被一行（停用状态）引用，重试即可收尾 |
| P0-B | 删号按计划清完凭据后，若另一进程此时新存了一份授权（它读到推进后的 epoch，可合法提交），直接删账号行会让那份文件变成孤儿 | 删账号行改为**带生命周期条件**（`delete_qoder_account_if_current`，`generation` + 我们自己推进后的 `auth_epoch`）。不成立 → 拒绝删除、保留账号与那份新授权，返回 `authorization_changed_during_delete`；计划内的行按 `generation` 标 `missing` | 删除成功时不存在计划外凭据；竞争时态可重试，且每个留在盘上的文件都有行引用 |
| P1-C | `missing` 只是文案，`credits::resolve_token` 仍会选它 → 失败的删除会"生效一半" | `usable` 排除 `missing`；只有 `missing` 行时给出"上次删除没完成、已停用"的明确提示 | 手动/自动领取都不会发请求（测试断言 `claimed_ids` 为空） |
| P1-D | 补偿性恢复只查外键，可能把旧引用写进"同 UID 删号重建"的新账号 | `restore_qoder_authorization` 增加生命周期参数，`INSERT ... WHERE EXISTS(generation)`；只比 `generation`（删行本身就会推进 epoch，比 epoch 永远不成立）；唯一键存在时直接放弃 | 身份已变化时不恢复、只留痕；绝不覆盖更新的授权 |

### 第三轮复审修复（删除计划带版本）

| 问题 | 修法 |
| --- | --- |
| 整账号删除的计划只记 `(row, kind)`，执行时用**无条件删除**。若另一进程在"读计划之后、删文件之前"对**同一来源**重新授权（覆盖写同一文件 → `revision + 1`），新令牌会被删掉；删账号行的 CAS 之后才发现 epoch 变了，令牌已经回不来 | 计划阶段就通过 `observe_for_delete` 记下每份凭据的版本（JSON `revision` / 钥匙串字节）；执行时一律走 `delete_guarded` / `delete_keychain_guarded`，版本不符（`conflict`）→ **立即停手**，返回 `credential_changed_during_delete`。`missing` 只标"这次确实删掉、且行仍指着旧引用"的来源（`set_qoder_auth_status_if_generation` 同时核验 `generation` 与 `secret_ref`） |

`secretsDeleted` 的计数含义随之明确为**按来源计的凭据份数**（设备授权的 access + refresh 算一份）。

### 第四轮复审修复（钥匙串原子性）

JSON 的读-比-删天然原子（`delete_at_revision` 在跨进程锁内比对 `revision`）。
**钥匙串没有原子的 compare-and-delete**：`get` 与 `delete` 是两次 API 调用，
上一版靠"删之前再读一次"，读与删之间仍可被写入插队。本轮统一到一把锁上：

| 问题 | 修法 |
| --- | --- |
| 钥匙串"读 → 比 → 删"之间没有互斥，可能删掉刚写进来的新令牌 | 复用凭据目录的 `.lock`（`LocalJsonStore::lock_exclusive`）：**写入**（`write_keychain`：拍快照 + 写 access + 写 refresh）、**回滚**（`QoderWrite::restore`）、**读-比-删**（`delete_guarded`）与**旧安装兜底**（`delete_keychain_group`）全部持同一把锁。拿不到锁就 `lock_timeout`，一个字节都不动 |
| 设备授权只比主键 access，随后连 refresh 一起删 | `DeleteGuard::Keychain` 改为**整组键的取值快照**（`Vec<(key, Option<bytes>)>`）：任一键不符（含"计划时不存在、现在出现"）即 `conflict`；一致才整组删除 |
| 同一来源重新授权复用同一个 `secret_ref`，`missing` 会标脏新授权 | 标记条件从「`generation` + `secret_ref`」改为「`generation` + `secret_ref` + **`fingerprint`**」。`fingerprint` 是每份凭据的令牌指纹，重新授权必换；不用 `auth_epoch`——它是账号级的，别的来源新增授权也会推进它 |
| 旧安装（无授权行）的三条键按"键"计数 | `plan.legacy` 按**份**分组（`own/access` + `own/refresh` 为一份，`pat/token` 为一份），整组比对 + 整组删除在同一把锁里；`secretsDeleted` 与有授权行的账号同口径 |

### 第五轮复审修复（版本号、回滚条件、部分删除、旧键存在性）

| 编号 | 问题 | 修法 |
| --- | --- | --- |
| **P0-A** | 旧安装（无授权行）的计划只记录**当时存在**的键。旧账号本来只有 access，重新登录补上了 refresh —— 计划看不到这个新键，比对仍然只比 access，于是删掉新授权正在用的 access | `PlannedLegacy` 改成 `Vec<(String, Option<Vec<u8>>)>`：不存在的键也入计划（值 `None`）。`delete_keychain_group` 在同一把锁里**连存在性一起比对**：观察时没有、现在有了 → `conflict`，整组一个字节都不删 |
| **P0-B** | `QoderWrite::restore` 拿到锁后**无条件**写回旧快照。A 写完等提交期间 B 写入并提交成功，A 的提交报错 → A 的回滚把 B 的令牌覆盖成旧值 | 回滚前（锁内）逐键比对"盘上还是不是我这次写进去的值"：不同 → **放弃回滚**并留痕；新建文件的回滚也先确认没有任何授权行引用它 |
| **P1-C** | `fingerprint` 不是版本号：它是令牌的 SHA-256 前 8 位。重新保存**同一串** PAT / access 时摘要不变，删号失败后的收尾会把这份新授权标成 `missing`（令牌还在、Credits 却不用了） | 数据库 **v21** 给 `qoder_authorizations` 加 `auth_version`（每次提交 +1，首插为 1）。`missing` 的守卫改成「`generation` + `auth_version`」；`delete_qoder_authorization_if_current` 也从 `auth_epoch` 改为 `auth_version`（更精确：别的来源写入不再阻止本来源删除） |
| **P1-D** | 钥匙串整组删除中途失败时，已删的键没上报。access 删掉、refresh 失败后，行还是 `valid`，而 Credits 已经读不到 access | `CredentialError` 增加 `partial`：整组删除在已经删掉一部分后失败 → 带 `partial` 返回。删号流程据此把这一份也计入"已删来源"，行被标 `missing`（Credits 不再使用），并可重试 |

顺带修了一个被 P1-C 的测试暴露出来的可用性缺陷：JSON 授权行还在、文件已经被删（删号被并发打断后就是这种状态）时，
`write_json_in_place` 过去直接报"凭据不存在"，用户重新授权永远失败。现在这种情形按"新建一份"处理 ——
否则并发保护等于把重新登录功能关掉了。

### 第六轮复审修复（统一凭据事务协议）

前五轮都是逐条补 guard，这一轮改成**结构性收口**：所有 mutate 路径进入同一个
锁域，凭据与数据库不再可能各自为政。

#### 锁顺序与状态机

```text
          ┌─────────────────────── SecretTxn（.lock，跨进程 flock）───────────────────────┐
persist   │ observe(DB) → write(Keychain|JSON) → commit(DB, CAS) → [失败] restore/cleanup │
remove    │ observe → observe_for_delete → 删行(CAS) → 确认没被重建 → 删凭据              │
delete    │ plan(DB+凭据预验证) → 关自动领取 → bump epoch → 逐份删 → 删账号行(CAS)         │
          └──────────────────────────────────────────────────────────────────────────────┘
```

- **只允许 `.lock` → SQLite**，不存在反向顺序：持锁期间只调 SQLite 与钥匙串，
  而 SQLite 事务内部不会再取 `.lock`。
- `FileLock` 现在**同线程可重入、跨线程互斥、跨进程 flock**。可重入是必需的：
  `LocalJsonStore` 的每个写方法都会自己取锁，外层一旦持锁，没有重入就会在自己
  的 flock 上等到超时。

#### 修复点

| 编号 | 问题 | 修法 |
| --- | --- | --- |
| **P0-1** | `write_keychain` 只锁"写"，`commit` 在锁外提交：A 写、B 写并提交、A 提交失败 → 行归 B，钥匙串是 A | 四条 mutate 路径整段持 `SecretTxn`，写-提交-回滚不可分割 |
| **P0-2** | JSON 原地覆盖同理（文件是 B、行记 A） | 同上；`Ok(None)` 现在几乎只可能来自"账号被删"这种真实冲突 |
| **P0-3** | PAT 移除在"删行后、删凭据前"可被重建插队；Keychain 键名固定、PAT 可能同值，旧 guard 会通过 | 移除整段持锁；**并在删凭据前再读一次行**，发现被重建就停手（`authorization_changed`） |
| **P0-4** | `auth_version` 首次插入固定为 1，删行重建后复用旧值 → 在途旧操作可能命中新实例 | 初值与补偿恢复都取 `账号 auth_epoch + 1`：epoch 只增不减，版本在账号内严格单调 |
| **P1-1** | `cleanup_created_account` 只按 `created: bool` + "无授权行"判断，可能删掉并发重建的账号 | 改成单条 `DELETE ... WHERE id=? AND generation=? AND NOT EXISTS(授权行)`，三条判断原子完成 |
| **P1-2** | 部分删除失败后的收敛路径没有正向验证 | `a_relogin_after_a_half_finished_delete_brings_the_account_back`：missing 之后重新授权必须回到 valid 且 Credits 可用 |
| **P2** | `authStorage` 在响应里重复写 3 次；两处注释仍描述 `auth_epoch` 守卫 | 去重为每响应一次；注释改述行版本 |

#### 读写一致性

读路径（`credits::resolve_token`）**不进**这把锁：它只读令牌、不写任何状态，
而写入永远是"先写凭据、后提交行"，所以读到的最坏情况是"上一版令牌"，
不会是半份。这一点在 `qoder_secret` 的模块注释里写明。

### 验证

| 项 | 手段 |
| --- | --- |
| 跨进程覆盖写后行与文件一致（P0-1/P0-2） | `qoder_secret::tests::concurrent_rewrites_keep_the_row_and_the_file_in_sync`：4 进程同时覆盖同一来源，断言 **行的 fingerprint == 盘上令牌的 fingerprint == 读路径拿到的令牌** |
| 同来源并发首次授权 | `..._concurrent_first_grants_leave_exactly_one_secret_and_one_row`（1 行 / 1 文件 / 行指向该文件） |
| 锁可重入（结构前提） | `localjson::tests::the_lock_is_reentrant_within_one_thread`；跨线程仍互斥：`lock_contention_surfaces_as_a_bounded_error` |
| 删除必须真的持锁 | `qoder_secret::tests::the_keychain_delete_actually_waits_for_the_cross_process_lock`（另一线程持锁 → `lock_timeout`，一个字节未动） |
| PAT 删行后被重建（P0-3） | `qoder_pat::tests::a_rebuilt_pat_row_is_not_deleted_by_the_older_removal`（**同一串** PAT 重建：行仍 valid、PAT 可读、Credits 可用） |
| 版本不可复用（P0-4） | `storage::qoder_accounts::tests::a_reinserted_row_never_reuses_an_old_auth_version` |
| 失败清理不误删（P1-1） | `..._cleanup_refuses_a_rebuilt_account_or_one_with_authorizations` |
| 部分删除后的恢复（P1-2） | `qoder_admin::tests::a_relogin_after_a_half_finished_delete_brings_the_account_back` |
| v21 迁移 | `storage::tests::v21_adds_an_authorization_version_without_touching_existing_rows` |
| 旧账号补 refresh 后停手（P0-A） | `qoder_admin::tests::a_relogin_that_adds_a_refresh_key_blocks_the_legacy_delete`（`deleted == 0`、access 与 refresh 都在） |
| 迟到回滚不覆盖赢家（P0-B） | `qoder_secret::tests::a_late_rollback_never_overwrites_a_newer_write`（回滚返回 false、盘上是 `B-TOKEN`）；反向 `a_rollback_still_restores_when_the_token_is_still_mine` |
| 同一串令牌重登（P1-C） | `qoder_admin::tests::a_relogin_with_the_same_token_keeps_the_new_row_valid`（断言指纹不变、`auth_version` 前进、行仍 `valid`、Credits 可读） |
| 半删凭据不许 `valid`（P1-D） | `qoder_admin::tests::a_half_deleted_keychain_credential_is_not_left_valid`（`faulty` 只让 refresh 删除失败 → 行标 `missing`、重试删净） |
| v21 迁移 | `storage::tests::v21_adds_an_authorization_version_without_touching_existing_rows`（老行从 0 起、回拨重跑是空操作） |
| 钥匙串读-比-删真的持锁 | `qoder_secret::tests::the_keychain_delete_actually_waits_for_the_cross_process_lock`（外部持锁 → `lock_timeout` 且一个字节未动） |
| 比对后写入新令牌 | `..._a_keychain_secret_rewritten_after_the_guard_is_refused`（新令牌完整保留） |
| 只更新 refresh | `..._a_refresh_only_update_also_blocks_the_delete`（两把键都不删） |
| 删凭据后、删号前重登 | `qoder_admin::tests::a_relogin_after_the_credential_was_deleted_keeps_the_new_row_usable`（新授权保持 `valid`、Credits 可读） |
| 旧安装按份计数 | `qoder_admin::tests::a_legacy_keychain_only_account_counts_two_credentials`（三条键 → 两份） |
| 同来源重登（设备授权） | `qoder_admin::tests::a_same_source_relogin_during_delete_keeps_the_new_token`（`rev 0` 计划 → 重登写 `rev 1` → 停手；新令牌可读、`revision == 1`、行仍 valid、重试删净） |
| 同来源重登（PAT） | `..._a_same_source_pat_relogin_during_delete_keeps_the_new_pat`（设备授权已删并标 `missing`，新 PAT 保持 `valid` 且可读） |
| P0-A 假成功 | `qoder_pat::tests::a_rewritten_secret_leaves_a_retryable_record_and_never_reports_success`（行以 `missing` 回来、Credits 拒绝使用、重试收尾） |
| P0-B 删号竞争 | `qoder_admin::tests::a_new_authorization_during_delete_aborts_without_orphaning_it`（拒绝删除；磁盘上每份文件都有行引用；重试删净） |
| P1-C missing | `credits::tests::a_missing_authorization_is_never_used_by_credits`（`resolve_token` 拒绝 + 领取零请求） |
| P1-D 补偿 | `storage::qoder_accounts::tests::a_restored_authorization_is_marked_missing_and_retryable`（换 generation 后恢复落空） |
| P0-1 确定性交错 | `qoder_secret::tests::a_stale_absent_observation_never_overwrites_the_winner`（赢家令牌与 `revision` 原样可读） |
| P0-2 生命周期 | `storage::qoder_accounts::tests::an_absent_commit_from_a_deleted_account_is_rejected`、`..._after_another_source_moved_the_epoch` |
| P0-3 先验后删 | `qoder_admin::tests::a_foreign_secret_aborts_the_whole_delete_before_anything_is_removed`、`a_magpie_link_is_counted_but_never_deleted`（钥匙串删除全失败的后端下仍成功） |
| P0-4 并发移除 | `qoder_pat::tests::a_remove_racing_a_new_pat_keeps_the_new_one`（钥匙串）、`..._new_json_pat_keeps_the_new_file_contents`、`removing_after_a_half_finished_delete_still_converges`、`storage::…::a_guarded_delete_refuses_a_newer_authorization` |
| 存储分派与归属 | `providers::qoder_secret::tests`（19 条，含全拒钥匙串后端） |
| 条件提交 | `storage::qoder_accounts::tests`（4 条，含删号重建） |
| 读取与降级 | `credits::tests`（JSON 设备授权 / JSON PAT / 文件缺失不换来源 / 归属错配） |
| 删除 | `providers::qoder_admin::tests`（含 JSON-only、混合、拒绝删除、失败可重试） |
| 真实跨进程竞争 | `concurrent_first_grants_leave_exactly_one_secret_and_one_row`（4 个子进程） |
| 全量 | `cargo fmt --check` / `cargo check --locked --all-targets` / `cargo test --locked` |

## 9. 风险

1. **明文**：见 §1。任何"顺手把 secrets 打进诊断包 / CI artifact"的改动都是事故。
2. **半迁移状态**：库里有 `localjson:` 引用但文件不在时，必须报 `not_found` 并提示重新登录，
   绝不能回退钥匙串 —— 那会让"免弹窗"变成"偶尔弹一次"，比稳定弹窗更难排查。
3. **降级**：不支持 v20 的旧版本读不了 `localjson:` 引用。回退必须用迁移前的数据库备份。
4. **MSRV**：跨进程锁用 `std::fs::File::try_lock`（Rust 1.89 起稳定），
   `account-worker/Cargo.toml` 的 `rust-version` 已随之提到 1.89。
