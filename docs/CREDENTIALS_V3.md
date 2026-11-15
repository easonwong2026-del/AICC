# 凭据存储 v3：本地 JSON + 动态 Magpie

> 状态：**CRED-1 已合并**（`LocalJsonStore` + 引用解析 + 文件系统边界加固）；
> **CRED-2 已交付待验收**（SQLite v20 + Magpie 动态关联 + 统一 Broker 解析）。
> CRED-3/4/5 尚未开始：自有登录仍写钥匙串，旧 Keychain 迁移也还没做 —— 本文件随每个 PR 更新。
>
> 基线：`main` @ `3d8f23c`（#82 已合并），数据库 Schema **v20**。
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

## 3. 引用方案（本 PR 交付）

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

## 7. 本 PR 的范围

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

## 8. 五个 PR 的顺序

| PR | 内容 | 状态 |
| --- | --- | --- |
| CRED-1 | JSON 存储底层 + 引用解析 | 已合并（#82） |
| CRED-2 | SQLite v20 + Magpie 动态授权 | **本 PR** |
| CRED-3 | 自有登录 JSON 化（四平台） | 待开始 |
| CRED-4 | 旧 Keychain 显式迁移（备份 / 核对 / 回滚） | 待开始 |
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

## 9. 风险

1. **明文**：见 §1。任何"顺手把 secrets 打进诊断包 / CI artifact"的改动都是事故。
2. **半迁移状态**：库里有 `localjson:` 引用但文件不在时，必须报 `not_found` 并提示重新登录，
   绝不能回退钥匙串 —— 那会让"免弹窗"变成"偶尔弹一次"，比稳定弹窗更难排查。
3. **降级**：不支持 v20 的旧版本读不了 `localjson:` 引用。回退必须用迁移前的数据库备份。
4. **MSRV**：跨进程锁用 `std::fs::File::try_lock`（Rust 1.89 起稳定），
   `account-worker/Cargo.toml` 的 `rust-version` 已随之提到 1.89。
