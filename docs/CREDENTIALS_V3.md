# 凭据存储 v3：本地 JSON + 动态 Magpie

> 状态：**CRED-1 已合并**（`LocalJsonStore` + 引用解析 + 文件系统边界加固）；
> **CRED-2 已合并**（SQLite v20 + Magpie 动态关联 + 统一 Broker 解析）；
> **CRED-3a 已交付待验收**（WorkBuddy 自有授权 JSON 化，**默认关闭**，见 §8ter）。
> CRED-3b（Qoder 两种授权）/ CRED-4（旧 Keychain 迁移）/ CRED-5（免弹窗收口）尚未开始。
>
> 基线：`main` @ `6adde70`（#84 已合并），数据库 Schema **v20**。
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
| CRED-1 | JSON 存储底层 + 引用解析 | 已合并（#82） |
| CRED-2 | SQLite v20 + Magpie 动态授权 | **本 PR** |
| CRED-3a | WorkBuddy 自有授权 JSON 化 | **本 PR** |
| CRED-3b | Qoder CN / International 浏览器、设备授权、PAT JSON 化 | 待开始（等 3a 合并） |
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

## 9. 风险

1. **明文**：见 §1。任何"顺手把 secrets 打进诊断包 / CI artifact"的改动都是事故。
2. **半迁移状态**：库里有 `localjson:` 引用但文件不在时，必须报 `not_found` 并提示重新登录，
   绝不能回退钥匙串 —— 那会让"免弹窗"变成"偶尔弹一次"，比稳定弹窗更难排查。
3. **降级**：不支持 v20 的旧版本读不了 `localjson:` 引用。回退必须用迁移前的数据库备份。
4. **MSRV**：跨进程锁用 `std::fs::File::try_lock`（Rust 1.89 起稳定），
   `account-worker/Cargo.toml` 的 `rust-version` 已随之提到 1.89。
