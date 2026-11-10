# 授权模型 v2 设计（PR #66 的迁移方案）

PR #65 只做了「小范围安全修复」：关掉 RPC 明文回传、修好 `auth_source` 在扫码后
不变的问题、给删除/导入/刷新加上归属检查。**一个账号仍然只能有一份凭据**——
这是所有别扭的根源，本文件是把它拆开的方案。

- 状态：**设计稿，未实现**。PR #66 才落地。
- 前置：PR #65（凭据归属检查）必须先合并，否则迁移会踩到"来源和令牌不一致"的旧数据。

## 1. 现在为什么不够用

| 现象 | 根因 |
| --- | --- |
| 从 Magpie 导入的账号再扫码登录，`auth_source` 要么撒谎、要么令牌被旧值覆盖 | 一个 uid 只对应一份钥匙串条目（`workbuddy/<uid>/access`），来源是个单值列 |
| 想"自有优先、Magpie 兜底"做不到 | 没有地方同时存两份 access，只能二选一 |
| Magpie 重新登录后，AICC 里那份不会自动更新 | 导入只在用户手动点的时候跑一次 |
| 删除账号时按 uid 清凭据 | 没有"这条凭据属于哪份授权"的概念，只能猜 |

## 2. 目标（按优先级）

1. **一个账号可以有多份授权**（自有扫码 / 从 Magpie 借用），互不覆盖。
2. **现有 `accounts.id` 不变**：`sha256(platform \0 uid)[0..16]`。
   于是 `checkin_records` / `travel_states` / `travel_events` / `credit_snapshots`
   的外键一行都不用改，历史签到与旅行记录原样保留。
3. 选择策略可解释：自有优先、Magpie 兜底、失效才降级。
4. 迁移可回滚：任何一步失败都不留半个库。

## 3. 表结构

```sql
CREATE TABLE account_authorizations (
    id               TEXT PRIMARY KEY,          -- sha256(account_id \0 kind)[0..16]，确定性、可重算
    account_id       TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    kind             TEXT NOT NULL CHECK (kind IN ('own', 'magpie')),
    access_ref       TEXT NOT NULL,             -- keychain:workbuddy/<account_id>/<kind>/access
    refresh_ref      TEXT,                      -- 只有 own 可能有；magpie 恒为 NULL
    auth_domain      TEXT,
    token_expires_at TEXT,
    status           TEXT NOT NULL DEFAULT 'unknown'
                     CHECK (status IN ('unknown', 'valid', 'expired', 'invalid', 'denied')),
    source_key       TEXT,                      -- Magpie 的 key（workbuddy#2ntprc）；own 为 NULL
    last_used_at     TEXT,
    last_error       TEXT,
    created_at       TEXT NOT NULL,
    updated_at       TEXT NOT NULL,
    UNIQUE (account_id, kind)
);

CREATE INDEX idx_auth_account ON account_authorizations (account_id, status);
```

`UNIQUE (account_id, kind)` 是幂等的来源：重复导入 = **更新**已有的 magpie 行，
永远不会长第二行；扫码登录同理，own 行只会被覆盖。

`refresh_ref` 对 magpie 永远是 NULL，并且代码里**没有**写入路径 —— 这比"记得别写"
可靠：导入流程会先显式删掉该授权下可能残留的 refresh。

## 4. 钥匙串键

| | 现在 | v2 |
| --- | --- | --- |
| 自有 | `workbuddy/<uid>/access` `…/refresh` | `workbuddy/<account_id>/own/access` `…/own/refresh` |
| 借用 | `workbuddy/<uid>/access`（同一个键！） | `workbuddy/<account_id>/magpie/access` |

`account_id` 是 16 位 hex，键总长约 40 字符，满足 `SecureStore::validate_account`。

迁移采用**复制后保留**：新键写成功后旧键原样留着（只读），下一个版本再删。
这样回滚旧二进制时凭据还在。

## 5. 迁移步骤

沿用现有 `PRAGMA user_version` + 事务机制（`storage/mod.rs::MIGRATIONS`），分三次提交：

### v5 —— 建表 + 回填（不改 `accounts` 列）

```sql
-- 每个账号按现有 auth_source 生成一行；auth_ref 里的键直接沿用（暂不换键）。
INSERT OR IGNORE INTO account_authorizations
    (id, account_id, kind, access_ref, refresh_ref, auth_domain, token_expires_at,
     status, source_key, created_at, updated_at)
SELECT
    'seed-' || a.id, a.id, COALESCE(NULLIF(a.auth_source, ''), 'own'),
    COALESCE(a.auth_ref, 'keychain:workbuddy/' || a.uid || '/access'),
    NULL, a.auth_domain, a.token_expires_at,
    a.auth_status, NULL,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM accounts a
WHERE a.platform = 'workbuddy';
```

回填的 `id` 先给一个临时确定性值（`seed-<account_id>`），v6 换成正式公式
`sha256(account_id \0 kind)[0..16]`，保持"可重算、可幂等"。

### v6 —— 双写期

- 读写全部走 `account_authorizations`；钥匙串写新键（复制，不删旧键）。
- `accounts.auth_source` / `auth_ref` / `token_expires_at` 降级成**只读镜像**：
  每次授权变化时同步写一次，供旧版本回退时仍能看到"大致正确"的状态。

### v7 —— 收尾

- 删掉镜像列（或标 `deprecated` 保留一版）。
- 删除旧钥匙串键（`workbuddy/<uid>/access|refresh`），只在确认新键存在时执行。

## 6. 选择与失败策略（双模式智能协作）

```
候选 = 该账号所有 status = valid 且 token_expires_at > now + 5min 的授权
排序 = own 优先，其次 last_used_at 更晚的
```

| 情况 | 处理 |
| --- | --- |
| 首选是 own | 401 → 用 own 的 refresh 刷新；刷新失败 → 标 invalid，降到 magpie |
| 首选是 magpie | 401 → 重新读 `plugin-auth.json` 取最新 access（**不刷新**）；取不到 → 标 expired |
| 两条都失效 | 账号 `auth_status = expired`，界面提示"重新扫码，或去 Magpie 登录" |
| 只有 magpie 且令牌临期 | 不做后台保活；只在使用前检查，过期即提示 |

降级是**读时**决策，不写库：`auth_source` 不再是账号属性，而是"当前首选授权的 kind"，
由 `accounts.list` 现算后返回，老界面照样拿到一个字符串。

## 7. 兼容性与影响面

| 项 | 影响 |
| --- | --- |
| `accounts.id` | 不变。签到 / 旅行 / 积分表外键不动 |
| 旅行调度 | 只换凭据来源（`AppTokens::access` 变成"按策略取首选授权"），调度逻辑不动 |
| 签到 | 同上；`checkin_records` 不涉及凭据 |
| RPC | `accounts.list` 增加 `authorizations: [{id, kind, status, tokenExpiresAt, sourceKey}]`；`authSource` 保留 |
| 登录 / 导入 / 删除 / 刷新 | 全部改成"按授权"操作：登录写 own 行、导入写 magpie 行、删除按授权逐个删键、刷新按授权类型选路径 |

## 8. 风险与回滚

| 风险 | 缓解 |
| --- | --- |
| 回填时 `auth_ref` 为空（早期数据） | 回填用 `COALESCE` 兜底成旧键规范；仍取不到就建行但标 `status = unknown`，不阻塞迁移 |
| 钥匙串写入在新键上失败 | 保留旧键、该授权标 `denied`，账号降级为"仅旧键可用"，`health` 报 degraded 原因 |
| 迁移中断 | 沿用事务 + `user_version`，失败整体回滚 |
| 回退旧二进制 | v6 之前可直接回退（镜像列还在、旧键还在）；v7 之后需先跑一次"键还原"脚本 |
| 同一 uid 出现两行账号 | 现有 `UNIQUE(platform, uid)` 防住；删除路径已按 `account_id` 而非 uid 清理（PR #65） |

## 9. 明确不做

- 不做跨平台账号合并（`qoder_cn` 与 `workbuddy` 仍是两条账号）。
- 不做 refresh token 集中托管或后台保活。
- 不改旅行调度的节奏与状态机（PR #64 的语义保持不变）。
- 不读取、不复制 Magpie 的 refresh token —— 这条在 v2 里依然是硬约束。

## 10. PR #66 的验收清单（预告）

1. 迁移在真实库副本上跑通，`accounts.id` 前后一致，签到/旅行记录条数不变。
2. 同一账号同时存在 own + magpie 两份授权：401 时按策略切换，且钥匙串里两份都在。
3. 导入不再覆盖 own；扫码不再覆盖 magpie 的刷新令牌；删除只删自己那条。
4. 回退测试：用 v5/v6 的二进制打开 v6 数据库仍可用（镜像列未删）。
5. 不联网的 Mock 测试覆盖选择策略的全部分支。
