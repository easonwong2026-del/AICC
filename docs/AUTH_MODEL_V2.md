# 授权模型 v2：一账号多来源授权（PR #66 设计稿）

> 状态：**M0 设计稿已定稿；M1 / M2 / M3 已按本文实现**（迁移 + 只读 Broker、双写 + 探测 + 删除、
> 设置 / UI / RPC 收口），全部提交在 PR #66 里。v8 仍留给后续独立 PR。
>
> 基线：`main` @ `72e6426`（PR #63 / #64 / #65 已合并）。
> 范围：**只做 WorkBuddy**。Qoder 不接；Buddy 旅行状态机（PR #64）不动；不执行真实签到 / 派出 / 领奖。

## 0. 六条不变量（G1–G6）

后面每一节、每个测试都挂在这六条上。任何一条被破坏，PR #66 不予验收。

| ID | 不变量 | 一句话判据 | 主要落点 |
| --- | --- | --- | --- |
| **G1** | **授权隔离**：一个 WorkBuddy 账号可同时持有 `own` 与 `magpie` 两份授权，凭据、版本、状态各自独立，互不覆盖 | 两份同时存在时，登录 / 导入 / 刷新 / 删除任意一份，另一份的钥匙串条目与状态**字节不变** | M1 表结构、M2 双写 |
| **G2** | **迁移窗口确定**：本轮数据库只有 **v6、v7**；v8 不在本 PR | `PRAGMA user_version` 只会出现 5 → 7；代码里不存在 v8 迁移 | M1 §5 |
| **G3** | **回滚可达**：旧 Keychain 键保留、`accounts` 镜像同步、迁移幂等且可重试 | 换成 PR #65 的二进制打开迁移后的库，4 个真实账号的凭据仍能读、旅行数据仍在 | M1 §5.4、§10 |
| **G4** | **Keychain 不在 SQL 里迁移**：SQL 迁移只建表与回填行，凭据搬运由 Rust 状态机完成 | 迁移期间钥匙串被拒 → 账号仍可用旧键，`health` 报 `failed` 且可重试；库不会留下"已迁移"的假象 | M1 §5.3 |
| **G5** | **授权 ID 确定**：`sha256(account_id + NUL + kind)` 前 8 字节，16 位 hex，由 Rust 回填时直接生成 | 同一账号同一 kind 反复重算得到同一 ID；库里不存在 `seed-*` | M1 §5.2 |
| **G6** | **状态派生**：认证状态以 `account_authorizations` 为准，账号的 `auth_status` / `auth_source` 是派生结果，旧库字段只作兼容镜像 | 授权行与镜像列不一致时以授权行为准；镜像写入必须携带来源授权 ID，禁止"来源 own、指向 magpie access" | M2 §7 |

## 1. 现在为什么不够用

PR #65 只做了小范围安全修复：关掉 RPC 明文回传、修好 `auth_source` 扫码后不变、给删除 / 导入 /
刷新加归属检查。**一个账号仍然只能有一份凭据**——这是所有别扭的根源。

| 现象 | 根因 |
| --- | --- |
| 从 Magpie 导入的账号再扫码登录，`auth_source` 要么撒谎、要么令牌被旧值覆盖 | 一个 uid 只对应一份钥匙串条目（`workbuddy/<uid>/access`），来源是单值列 |
| 想"自有优先、Magpie 兜底"做不到 | 没有地方同时存两份 access，只能二选一 |
| Magpie 重新登录后，AICC 里那份不会自动更新 | 导入只在用户手动点的时候跑一次 |
| 删除账号时按 uid 清凭据 | 没有"这条凭据属于哪份授权"的概念，只能猜 |
| 401 之后不知道该切源还是该刷新 | `get_checkin_status` 把 HTTP 语义压成 `ok:false` + 文案，401/429/超时无法区分 |

## 2. 目标与不做的事

**目标（按优先级）**

1. 一个 WorkBuddy 账号可以有多份授权（自有扫码 / 从 Magpie 借用），互不覆盖（G1）。
2. 首选来源可配置，默认 **Magpie**；首选不可用时按策略兜底到另一份。
3. `accounts.id` 不变，`checkin_records` / `travel_states` / `travel_events` /
   `credit_snapshots` 的外键一行都不用改，历史原样保留。
4. 迁移可回滚、可重试、中断可恢复（G3）。

**明确不做**

- 不接 Qoder（`qoder_cn` 仍是单来源，不进 `account_authorizations`）。
- 不改变 PR #64 的旅行状态机、调度节奏、幂等规则。
- 不执行真实签到、派出、领奖；不对 4 个真实账号做破坏性测试；不写 Magpie 的凭据文件。
- 不读、不复制、不刷新 Magpie 的 refresh token（硬约束，贯穿全 PR）。
- v8 的旧凭据清理由**后续独立 PR** 完成（见 §5.5）。

## 3. 表结构（v6）

```sql
CREATE TABLE account_authorizations (
    id                  TEXT PRIMARY KEY,      -- G5：sha256(account_id \0 kind)[0..8] → 16 hex
    account_id          TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    kind                TEXT NOT NULL CHECK (kind IN ('own', 'magpie')),
    access_ref          TEXT NOT NULL,         -- keychain:workbuddy/<account_id>/<kind>/access
    refresh_ref         TEXT,                  -- 只有 own 可能有；magpie 恒为 NULL
    auth_domain         TEXT,
    token_expires_at    TEXT,
    status              TEXT NOT NULL DEFAULT 'unknown'
                        CHECK (status IN ('unknown', 'valid', 'expired', 'invalid', 'denied')),
    version             INTEGER NOT NULL DEFAULT 0,   -- 单份授权的写版本（替代账号级 epoch 的粒度）
    key_migration_state TEXT NOT NULL DEFAULT 'pending'
                        CHECK (key_migration_state IN ('pending', 'migrating', 'ready', 'failed')),
    source_key          TEXT,                  -- Magpie 侧 key（workbuddy#<uid>）；own 为 NULL
    last_used_at        TEXT,
    last_error          TEXT,                  -- 只存分类与脱敏文本，永不存令牌
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL,
    -- magpie 授权不允许有 refresh 引用：约束写在表上，比"记得别写"可靠
    CHECK (kind <> 'magpie' OR refresh_ref IS NULL),
    UNIQUE (account_id, kind)
);

CREATE INDEX IF NOT EXISTS idx_auth_account ON account_authorizations (account_id, status);
CREATE INDEX IF NOT EXISTS idx_auth_pending ON account_authorizations (key_migration_state);
```

`UNIQUE (account_id, kind)` 是幂等的来源：重复导入 = **更新**已有的 magpie 行，永远不会长第二行；
扫码登录同理，own 行只会被覆盖。

`refresh_ref` 对 magpie 永远是 NULL，而且代码里**没有**写入路径：导入流程会先确认该行没有 refresh
引用，再落 access。`CHECK` 是最后一道闸，不是唯一一道。

`version` 与 `accounts.auth_epoch` 的关系：`version` 是**单份授权**的写版本，任何对这份授权的写入
（凭据、状态、元数据）都 +1；`auth_epoch` 保留为账号级兼容机制（旧二进制 / 旧快照仍然读它）。
两者都比较，任一不符即拒绝写入。

## 4. 钥匙串键布局

| | 现在（v5 及以前） | v2 |
| --- | --- | --- |
| 自有 access | `workbuddy/<uid>/access` | `workbuddy/<account_id>/own/access` |
| 自有 refresh | `workbuddy/<uid>/refresh` | `workbuddy/<account_id>/own/refresh` |
| 借用 access | `workbuddy/<uid>/access`（同一个键） | `workbuddy/<account_id>/magpie/access` |
| 借用 refresh | 不应该有（PR #65 会主动删） | **不存在这个键** |

`account_id` 是 16 位 hex，键总长 40 字符左右，满足 `SecureStore::validate_account`。
`magpie/refresh` 这个键在代码里不出现、在 RPC 白名单里也不出现（§9）。

## 5. 迁移（G2 / G3 / G4 / G5）

沿用现有 `PRAGMA user_version` + 事务机制（`storage/mod.rs::MIGRATIONS`），本轮扩成
**SQL 步骤 + Rust 步骤**两种，同一版本号内的两步在**同一个事务**里执行。

### 5.1 版本号规划：v5 → v6 → v7，没有 v8

| 版本 | 内容 | 归属 |
| --- | --- | --- |
| v1–v4 | 账号 / 签到 / 积分 / 旅行 | 已发布 |
| **v5** | `accounts.auth_epoch` | 已发布（PR #65） |
| **v6** | 建 `account_authorizations` + Rust 逐行回填 | 本 PR |
| **v7** | `accounts.auth_mirror_auth_id` + 首次镜像同步（双写起点） | 本 PR |
| v8 | 清理旧钥匙串键、下线镜像列 | **后续独立 PR，不在 #66** |

**`Db::open()` 会连续执行本版本包含的所有迁移。** v6 与 v7 拆号是为了让 diff 可评审、
让每一步能单独回滚，**不代表运行时能观察到 v6 之后、v7 之前的中间态**：同一个进程启动时会
把 5 → 6 → 7 一次跑完，任何 RPC 都看不到"只有 v6"的库。因此：

- 不要在 v7 里假设"v6 已经跑了一段时间、Keychain 一定迁移完了"——它没有；
- 迁移逻辑必须支持"首次启动时 v6 与 v7 无间隔连续执行"；
- 测试里的 v5→v6 观察窗口是**测试构造的**，不是产品路径。

### 5.2 v6 —— 建表 + Rust 回填

SQL 步骤只有 DDL（§3）。回填由 Rust 步骤完成，因为它要做三件 SQL 不该做的事：
算确定性 ID、读 `accounts` 的历史列、按 kind 决定 `refresh_ref` 是 NULL 还是引用。

回填规则（每个 `platform = 'workbuddy'` 的账号一行）：

| 字段 | own | magpie |
| --- | --- | --- |
| `id` | `sha256(account_id + NUL + 'own')[0..8]` → 16 hex | 同公式，`kind = 'magpie'` |
| `access_ref` | `keychain:workbuddy/<account_id>/own/access` | `keychain:workbuddy/<account_id>/magpie/access` |
| `refresh_ref` | `keychain:workbuddy/<account_id>/own/refresh` | **NULL** |
| `auth_domain` / `token_expires_at` / `status` | 复制 `accounts` 同名列 | 同左 |
| `version` | 0 | 0 |
| `key_migration_state` | `pending` | `pending` |
| `source_key` | NULL | NULL（首次导入 / resync 时补 `workbuddy#<uid>`） |

`kind` 取 `COALESCE(NULLIF(auth_source, ''), 'own')`：v3 的列默认值就是 `own`，
空值只可能来自更早的脏数据，按 own 处理并让 `health` 报出来。

**`access_ref` / `refresh_ref` 记的是"目标位置"，不是"已经在那里的内容"。** 在
`key_migration_state = 'ready'` 之前，读取一律走兼容路径（§5.3）。这一点必须写死在代码里，
否则迁移中断后会出现"库里说 own/access，钥匙串里还没有这个键"的组合，读凭据直接失败。

**ID 必须是最终值。** 不生成 `seed-<account_id>` 这类临时 ID 再在 v7 改写：

- 临时 ID 会让"授权 ID"在同一个库的生命周期里出现两种形态，任何按 ID 写下的日志 / 事件 /
  在途请求都会指向一个不存在的行；
- v6/v7 连续执行（§5.1）意味着临时 ID 的存活时间可能接近 0，付出的复杂度换不到任何观察窗口；
- 重算公式足够简单，回填时直接算，幂等性由 `UNIQUE (account_id, kind)` + `INSERT ... ON CONFLICT
  DO NOTHING` 保证，不需要"先起个名字再改名"。

`accounts.id` 与所有旅行 / 签到历史**一行不改**：回填只读 `accounts`，不写它。

### 5.3 Keychain 自动迁移（G4）

**Keychain 不能在 SQL 里迁移。** SQL 迁移是纯数据结构操作，跑在数据库事务里；钥匙串是另一个
系统、没有事务、会弹系统授权框、可能被用户拒绝。把两者塞进一个事务，结果是"库回滚了、钥匙串写了一半"
——比不迁移更糟。所以：

- **SQL 侧**只负责建表、回填行、把 `key_migration_state` 置为 `pending`；
- **Rust 侧**在启动时（迁移完成后）跑一个可中断、可重入的搬运器，逐行推进状态机。

状态机：

```
pending ──► migrating ──► ready        读到旧凭据并写成功
                 │
                 └─────► failed         读取被拒 / 写入失败 / 内容不可用
                            │
                            └──► pending（下一次启动重试，可反复）
```

搬运规则：

| 源（旧键） | 目标（新键） | 条件 |
| --- | --- | --- |
| `workbuddy/<uid>/access` | `workbuddy/<account_id>/own/access` | 该行 `kind = own` |
| `workbuddy/<uid>/refresh` | `workbuddy/<account_id>/own/refresh` | 该行 `kind = own` |
| `workbuddy/<uid>/access` | `workbuddy/<account_id>/magpie/access` | 该行 `kind = magpie` |
| `workbuddy/<uid>/refresh` | **不迁移** | magpie 永不持有 refresh |

**"搬完了"的判定必须可信**（评审 P0）：

| 情况 | 结果 |
| --- | --- |
| 旧键有内容 → 写新键成功 | `ready` |
| 旧键没有、新键已有相同内容 | `ready`（幂等重跑） |
| 旧键没有、新键也没有，且授权标 `valid` / `unknown` | **`failed` + `credential_missing`**：不允许显示成"迁移完成" |
| 旧键没有、新键也没有，授权本来就是 `expired` / `invalid` / `denied` | `ready`（本来就不该有凭据） |
| 读新键被拒 / 后端异常 | **中止，不写**（不知道现状就不盲写） |
| 新键已有内容但与旧键不同 | **`failed` + `conflict`**，不覆盖（来源无法确认） |

四条硬规则：

1. **只复制已存在的授权，不凭空创建第二套凭据。** 旧键里没有 refresh，就不去造一个；
   账号只有 magpie 一行，就不生成 own 行。
2. **读取失败 ≠ 凭据不存在。** `NotFound` 才是"没有"，`Denied` / 后端异常要进 `failed` 并保留原状态。
   把拒绝读成"没有"，下一轮就会认为迁移完成。
3. **中断可恢复、重复执行幂等。** 目标键已存在且内容一致 → 直接 `ready`；写入中途崩溃 → 下次
   重跑同一行（`migrating` 视为 `pending` 处理），不产生副作用。
4. **旧键不删除。** 这一步只做复制。删除是 v8 的事（§5.5），而且要在确认新键可读之后。

`health` 报告（不含任何明文）：

```json
"credentialMigration": {
  "pending": 0, "migrating": 0, "ready": 4, "failed": 0,
  "lastError": null,
  "failures": [],
  "retryable": false
}
```

`failures` 按失败原因分组（`[{message, count}]`）：光有 `failed: 1` 分不清是
"钥匙串被拒"还是"两边都没有凭据"，而这两种情况的处理动作完全不同。

失败时 `lastError.kind` 取 `denied` / `unavailable` / `mismatch` / `write_failed` 之一，
`message` 过 `rpc::sanitize`，不含键内容。

**迁移未完成时新版仍可用**：读凭据的入口按 `key_migration_state` 决定读新键还是旧键——
`ready` 读新键，其余读旧键（旧键存在即用）。于是"没迁完"只影响性能与 `health` 展示，
不影响签到 / 旅行 / 刷新。

### 5.4 v7 —— 双写起点 + 镜像指向

SQL 步骤：

```sql
ALTER TABLE accounts ADD COLUMN auth_mirror_auth_id TEXT;
```

Rust 步骤：为每个账号选出"当前镜像来源"（回填出来的那一行），写入 `auth_mirror_auth_id`，
并把 `accounts.auth_ref` / `auth_status` / `auth_source` / `token_expires_at` 按该行同步一次。

为什么要多这一列：G6 要求"旧键与镜像必须匹配同一来源，不得出现 Own 状态却指向 Magpie access
的组合"。只靠 `auth_source` 一个字符串无法证明"这个状态来自哪一份授权"，也就无法在授权行变更后
判断镜像是否已经过期。`auth_mirror_auth_id` 让镜像**可追溯**：镜像写入必须带上来源授权 ID，
读的时候能校验它是否仍指向同一行。

v7 之后进入双写期：所有授权操作走 Broker / Authorization Service（§6、§7），
`accounts.*` 的镜像列由授权变更驱动更新，不再由调用方直接写（`accounts.upsert` 仍接受该字段，
但内部按"镜像"处理，见 §7.3）。

### 5.5 v8 —— 后续独立 PR

**不在本 PR。** 内容（供后续参考，不实现）：

- 删除旧钥匙串键 `workbuddy/<uid>/access|refresh`，只在确认新键可读、且 `key_migration_state = ready`
  时执行；
- 下线镜像列（或标 `deprecated` 保留一版）。

本 PR 的代码里**不出现 v8 迁移**，`LATEST_VERSION` 停在 7。

### 5.6 回滚路径（G3）

| 层级 | 手段 | 结果 |
| --- | --- | --- |
| 数据库 | 每条迁移在事务里，失败整体回滚；成功则 `user_version` 前进 | 不存在半个库 |
| 钥匙串 | 只复制、不删除，旧键原样保留 | 旧二进制照常读旧键 |
| 账号镜像 | v7 同步 `auth_ref` / `auth_status` / `auth_source` / `token_expires_at` | 旧二进制看到"大致正确"的状态 |
| 重试 | `key_migration_state` 可 `failed → pending`；迁移 `INSERT ... ON CONFLICT DO NOTHING` | 反复启动不产生重复授权 |
| 二进制回退 | 换回 PR #65 的二进制 | 多出的两张结构（表 + 列）被忽略；4 个账号凭据仍可读 |

回退验证是验收项之一（§11 #8）：用 v7 的库 + 旧二进制跑一次 `auth.workbuddy.status`，
必须仍能读到凭据并返回 `valid`。

## 6. Credential Broker（M1，只读）

```rust
pub enum Source { Own, Magpie }

pub struct Resolved {
    pub authorization_id: String,
    pub kind: Source,
    pub access: String,        // 明文只在内存里，不出 Broker
    pub expires_at: Option<String>,
    pub degraded: Option<Degrade>,  // 首选不可用、这次用了兜底
}
```

规则：

1. **默认首选 Magpie**（`app_settings.auth_preferred_source`，默认 `magpie`）。
2. 候选 = 该账号 `status` 可用且未过期（`token_expires_at > now + 5min`）的授权行；
   首选来源在前，同来源内取 `last_used_at` 更晚的。
3. 首选没有可用行时**读时**兜底到另一份，不改库、不改 `status`（一次取用，不留痕）。
4. 读取一份授权**绝不写另一份**：Broker 只做 `SecureStore::get`，不带任何 `set` / `delete`。
5. 能回答"这次用的是哪个来源"（`Resolved.kind` + `degraded`），供 UI 与日志使用。
6. **M1 阶段不切换生产任务写入路径。** 签到 / 旅行的取令牌仍走现有 `AppTokens`；
   Broker 先只被 `health`、`auth.workbuddy.status` 的只读路径使用，M2 才接写入与任务。

Magpie 的 refresh 在 Broker 里没有字段、没有分支——结构上就取不到。

## 7. 凭据写入翻转与双授权协作（M2）

### 7.1 v7 双写

所有授权操作改走统一 Authorization Service（Broker 的写侧）：

| 动作 | 只写这一行 | 行为 |
| --- | --- | --- |
| Magpie 导入 / resync | `magpie` | 只写 `magpie/access`；确认该行 `refresh_ref` 为 NULL；不碰 own |
| AICC 扫码登录 | `own` | 写 `own/access` + `own/refresh`；不碰 magpie |
| 刷新 | `own` | 只有 own 允许用 refresh 刷新；magpie 走"重新从 Magpie 读一次 access" |
| 删除单份授权 | 指定行 | 只删该行对应的键与记录（§7.4） |

- **Magpie 的 refresh 永不读取、不复制、不刷新**：Broker 没有字段，导入路径没有分支，
  表上有 `CHECK`，测试里有一条专门的断言。
- **单份授权独立 `version`**；写入前比对 `version` 与账号 `auth_epoch`，任一不符即
  `superseded`，一个字节都不写。
- **更新必须有锁和版本校验**：沿用 `AUTH_WRITE_LOCK`（进程内）+ 行版本比对。
  锁内重读授权行，再写入。
- **部分写入失败必须记录、恢复或降级，不得宣称成功**：沿用 `CredentialSnapshot` 语义
  （`NotFound` 才算"本来没有"，`Denied` 中止写入），回滚不干净报 `rollback_incomplete`；
  状态写 `failed` + `last_error`，`health` 可见。

### 7.2 授权有效性探测

现有 `get_checkin_status` 已经把 HTTP 语义丢掉（失败只回 `ok:false` + 文案，而且 404 会回落
到第二个路径，连状态码属于哪次请求都说不清），**不能**用 `ok:false` 驱动授权切换。

新增内部探测（不经 RPC，不返回明文）：

```rust
pub enum Probe { Valid, Unauthorized, Transient }

pub struct ProbeReport {
    pub outcome: Probe,
    pub http_status: Option<u16>,
    pub business_code: Option<i64>,
    pub detail: Option<String>,   // 已脱敏
}
```

实现要点：

- 用 `config::http_request_raw`（保留状态码的底层请求）打一次轻量鉴权请求，
  **不用** `get_checkin_status`；
- 分类：

| 情况 | 分类 | 是否切源 |
| --- | --- | --- |
| 2xx 且业务码正常 | `Valid` | 否 |
| 401 | `Unauthorized` | **是**（唯一触发条件） |
| 403 | **按真实响应语义判断**（见下） | 视分类 |
| 429 / 5xx / 超时 / 连接错误 / `code = -1` | `Transient` | 否 |
| 其它 4xx | `Transient`（保守） | 否 |

**判定顺序**：传输失败（`status = 0`）→ 429 → 5xx → 401 → 403 → 2xx 业务码 → 其它。
429 / 5xx 先于响应体里的 `code = 401`：限流响应即使 body 写了 401，说的也是
"这次没问出来"，不是"令牌不能用"。

- **403 必须依据 WorkBuddy 的真实响应语义分类，不能默认视为认证失败。** WorkBuddy 的 403
  既可能是"令牌无权 / 失效"，也可能是网关指纹拦截（`10085`）或活动未开放。判定依据是响应体里的
  业务码与文案（`is_unauthorized` 等上游已有判据），不是 HTTP 码本身。分类不出来时**归为
  `Transient`**：宁可这次不切源，也不能把一份还能用的授权标成失效。
- **只有明确的授权不可用（`Unauthorized`）触发备用授权选择。**
- **暂时性失败不能标记认证失效，也不能造成重复任务执行**：`Transient` 只写 `last_error`，
  不动 `status`，不切源；旅行 / 签到侧沿用现有的失败退避与幂等（同一天 / 同一程不重复提交）。

探测结果里**不含**令牌、Cookie、请求头。

### 7.3 状态派生与兼容镜像（G6）

- 事实来源：`account_authorizations.status` / `token_expires_at` / `last_used_at`。
- 账号总体状态 = **最终选中的那份有效授权**派生：有 `valid` 且未过期 → 账号 `valid`；
  两份都过期 → `expired`；两份都 `invalid` / `denied` → `invalid`。
- `accounts.auth_status` / `auth_source` / `token_expires_at` / `auth_ref` 继续维护，
  作为**兼容镜像**，让 PR #65 的二进制仍能读到当前有效授权。
- 镜像写入必须同时写 `auth_mirror_auth_id`：镜像只能来自**同一行**授权。
  `auth_source = own` 却指向 `magpie/access` 的组合在代码里无法构造，
  测试里有一条断言专门钉住（`mirror_and_source_cannot_disagree`）。
- `accounts.list` / `accounts.get` 的 `authSource` 返回**当前选用的来源**（派生值），
  协议形状不变；新增 `authorizations: [...]` 数组（§8）。

### 7.4 删除

新增类型化 RPC：

```
auth.authorization.delete { accountId, kind }
```

- 只清除**指定授权**的独立钥匙串条目（`workbuddy/<account_id>/<kind>/access`，
  own 再加 `/refresh`）与关联记录；
- 不删其它授权、不删账号、不动签到 / 旅行历史；
- 删除失败必须可见且可恢复：Keychain 删除失败不回滚记录（记录留着才能重试），
  返回 `credential_delete_failed` + 剩余键数量，`health` 报出来。

完整账号删除（`accounts.delete`）要清理 **5 条键**：

1. `workbuddy/<account_id>/own/access`
2. `workbuddy/<account_id>/own/refresh`
3. `workbuddy/<account_id>/magpie/access`
4. `workbuddy/<uid>/access`（旧别名，G3 保留的那条）
5. `workbuddy/<uid>/refresh`（旧别名）

任一键清理失败都要进返回值（`failedKeys`，只给键名不给值）与日志，不允许 `let _ =`；
账号行删除后仍失败的，记一条 `orphan_credential` 到 `health`，供下次启动重试。

## 8. 设置、UI 与 RPC（M3）

### 8.1 全局设置

`app_settings` 表已存在但从未被读写。本轮实现真实读写：

| RPC | 参数 | 返回 |
| --- | --- | --- |
| `settings.get` | `{ key? }` | 省略 key 返回全部已知设置；指定 key 返回单个 |
| `settings.set` | `{ key, value }` | 校验后持久化，返回生效值 |

首个设置项：

| key | 取值 | 默认 | 校验 |
| --- | --- | --- | --- |
| `auth_preferred_source` | `magpie` \| `own` | `magpie` | 其它值 → `invalid_params` |

未知 key 的 `settings.set` 一律拒绝（不给"随便存点什么"的口子）。
Swift 设置页加入全局授权优先级选项（Magpie 优先 / 自有优先），**暂不做账号级覆盖**。

### 8.2 AI 账号界面

现有账号页（`ConsoleAccountsView`）显示：

- Magpie 是否已授权 / Own 是否已授权（两枚独立徽章）；
- 各自的认证状态与到期时间；
- 当前使用来源（派生）与首选来源（设置）；
- 授权不可用的原因（`last_error` 的分类文案，不含令牌）。

**本 PR 不增加单套授权删除按钮**，只保留类型化 RPC（§7.4）。

### 8.3 RPC 安全收口

通用 `secure.set` / `secure.get` / `secure.delete` **默认关闭**：

| 条件 | 行为 |
| --- | --- |
| `AICC_ALLOW_SECURE_RPC` 未设 / 非 `1` | 三个方法一律 `method_not_found` |
| `= 1`（受控开发 / 迁移） | 启用，但仍校验键名与操作 |

启用时允许的键结构**仅限**：

```
workbuddy/<16hex>/own/access
workbuddy/<16hex>/own/refresh
workbuddy/<16hex>/magpie/access
```

- **禁止 `workbuddy/<u16hex>/magpie/refresh`**（不存在这个键，也不允许创建）；
- 旧 UID 格式键（`workbuddy/<uid>/access|refresh`）**仅限 Rust 内部迁移操作**，
  RPC 不接受（`invalid_params`）；
- `__probe__*` 只由内部 `secure.status` 使用，不提供任意探针键写入能力；
- 所有 RPC、日志、错误响应不得泄露 Token：`secure.get` 仍只回 `fingerprint` / `length` /
  `revealed: false`，`sanitize` 覆盖所有错误文案。

内部逻辑（签到 / 旅行 / 刷新 / 迁移）走 `SecureStore`，不经 RPC，因此关掉这三个方法
不影响任何功能——回归用例要证明这一点。

## 9. 协议变化一览

| 方法 | 变化 |
| --- | --- |
| `accounts.list` / `accounts.get` | 新增 `authorizations: [{ id, kind, status, tokenExpiresAt, sourceKey, keyMigrationState, lastUsedAt, lastError }]`；`authSource` 保留（改为派生值） |
| `accounts.upsert` | 忽略调用方传来的 `authStatus` / `authSource` / 授权引用：它们是派生镜像，只由服务写（G6） |
| `auth.workbuddy.status` | 返回 `authSource` / `authorizationId` / `probe.{outcome,httpStatus,businessCode}`；暂时性失败返回 `state: "transient"` 且不写状态 |
| `settings.get` / `settings.set` | 新增 |
| `auth.authorization.delete` | 新增（类型化删除） |
| `secure.set/get/delete` | 默认关闭；启用时键名白名单 |
| `health` | 新增 `credentialMigration`、`authorizations` 汇总（不含明文） |
| 其它 | 形状不变 |

## 10. 失败与降级策略

| 情况 | 处理 |
| --- | --- |
| 首选是 magpie，探测 `Unauthorized` | 重新读 Magpie 的 `plugin-auth.json` 取最新 access（**不刷新**）；取不到 → own 兜底；两条都不行 → `expired` |
| 首选是 own，探测 `Unauthorized` | 用 own 的 refresh 刷新；刷新失败 → 该行 `invalid`，降级到 magpie |
| 探测 `Transient`（429 / 5xx / 超时 / 403 无法归类） | 不改状态、不切源，只记 `last_error`；任务侧按既有退避处理，不重复执行 |
| 旅行 / 签到拿到 401 | 按**当前选中的来源**恢复一次：own → refresh，magpie → 重新读文件；主路明确失败后**只读**地试另一份仍在有效期内的 access，不再发第二次请求 |
| 旅行 / 签到拿到 429 / 5xx / 超时 | 不换源、不进 6 小时令牌冷却，按普通退避重试；不重复派出 / 领取 |
| 两条都失效 | 账号 `expired`，界面提示"重新扫码，或去 Magpie 登录" |
| 只有 magpie 且令牌临期 | 不做后台保活；使用前检查，过期即提示 |
| 只有 own 且 refresh 缺失 | 不降级（没有别的来源），标 `invalid` 等重新登录 |
| Keychain 读被拒（`Denied`） | 该授权 `denied`，不当"不存在"；账号不因此变 invalid，`health` 报 `denied` |
| Keychain 迁移未完成 | 读旧键；账号照常可用（G4） |
| 迁移中断 / 重复启动 | 状态机重入，`INSERT ... ON CONFLICT DO NOTHING`，不产生重复行（G3） |

降级是**读时**决策，不写库：`auth_source` 是"当前首选授权的 kind"，由 `accounts.list` 现算。

## 11. 测试矩阵（本 PR 必须全绿）

全部离线：隔离临时库 + 模拟 Keychain（`SecureStore::memory/faulty*`）+ Mock 网络。

| # | 用例 | 覆盖 |
| --- | --- | --- |
| 1 | 空库初始化；v4→v5→v6/v7；v5→v6/v7 | G2 |
| 2 | 迁移中断、Keychain 读取拒绝、写入失败及重试 | G3 / G4 |
| 3 | 两套授权并存；重复扫码 / 重复导入幂等 | G1 / G5 |
| 4 | Magpie 优先、Own 优先、认证失效后的正确兜底 | G6 |
| 5 | HTTP 401 / 403 / 429 / 5xx / 超时分类 | §7.2 |
| 6 | 旧快照不能覆盖新授权（`version` + `auth_epoch`） | G1 |
| 7 | 单份授权删除、完整账号删除（5 条键）、失败恢复 | §7.4 |
| 8 | 旧版 PR #65 二进制回退兼容（新库 + 旧二进制读凭据） | G3 |
| 9 | 自动旅行原有用例全部通过（18 项） | 不回归 |
| 10 | RPC 脱敏；通用 `secure.*` 默认关闭；白名单键 | §8.3 |
| 11 | 真实库副本迁移前后：账号 / 旅行事件 / 旅行快照 / 签到记录数量一致 | G3 |

不做的测试：真实账号破坏性操作、真实 Magpie 凭据文件写入、真实签到 / 派出 / 领奖。

## 12. 交付与里程碑

| 里程碑 | 内容 | 提交 | 依赖 |
| --- | --- | --- | --- |
| M0 | 本文档（G1–G6、v6/v7 规划、回滚路径） | `docs(auth): ...` | — |
| M1 | 迁移器（SQL + Rust 步骤）、v6/v7、Keychain 搬运状态机、只读 Broker | `feat(auth): ...` | M0 |
| M2 | 双写、探测接口、状态派生与镜像、单份删除 | `feat(auth): ...` | M1 |
| M3 | `settings.*`、Swift 设置与账号页、RPC 收口 | `feat(auth): ...` | M2 |

每个里程碑的提交说明包含：改动文件与核心设计、测试结果、已知风险、回滚路径、下一里程碑依赖。

CI 门槛：PR #66 的 7 项检查（`test` / `lint` / `rust` / `swift-test` / `macos-build` /
`android` / `android-visual`）全部成功。**不自动合并。**

## 13. 风险

| 风险 | 缓解 |
| --- | --- |
| 回填时 `auth_ref` 为空（早期数据） | 按 `account_id` 现算目标键；仍不可用时建行并标 `status = unknown`，不阻塞迁移 |
| 钥匙串写入新键失败 | 保留旧键、该行 `failed`，账号降级为"仅旧键可用"，`health` 报原因 |
| 403 误判为认证失败 | 默认归 `Transient`；只有明确 `Unauthorized` 才切源 |
| 双写期两份状态不一致 | 以授权行为准 + `auth_mirror_auth_id` 校验；`health` 对比镜像与授权 |
| 旧二进制回退后看不到新键 | 旧键保留一个版本；镜像列同步 |
| 一账号多行的历史脏数据 | `UNIQUE (platform, uid)` 已防住；删除按 `account_id` |
| 迁移期并发启动两个 worker | 进程内锁 + `user_version` 事务 + `UNIQUE` 约束；跨进程场景留到后续 |
