# AICC Provider 架构

## 目标

AICC 2.5.0（P1）引入“内置可插拔 Provider”架构：新增 AI Agent、额度、积分或本地服务时，不需要再为每个平台分别硬编码 Python API 字段、Registry 条目、Swift 数据模型、SwiftUI 专属卡片、设置开关、排序逻辑、状态显示或诊断入口。

本阶段**不包含**：第三方插件商店、在线安装插件、动态执行仓库外 Python 文件、任意 Shell 插件、插件签名系统、第三方代码自动下载、Android 全面动态化、OpenCodex 分支维护。

## 两层数据结构

### 第一层：Provider 原始状态

现有 Provider（codex / workbuddy / deepseek / system）继续负责：

- 数据采集
- 本地缓存
- 健康状态
- 刷新
- 错误回退
- 专属诊断（例如 WorkBuddy 的 `balance_error_code` / `balance_error` / `balance_diagnostic`）
- 专属操作

保留接口：

```python
provider.status()
provider.refresh(force=False)
provider.health()
provider.cache()
```

P1 **不重构、不替换、不破坏** WorkBuddy 2.4.4 已打通的真实采集链路：`127.0.0.1:9223` CDP bridge、主 renderer 页面选择、DevTools/tdoc/MCP 内嵌页面排除、MessageChannel daemon transport、`auth:getAccountUsage` RPC、`usageLeft/usageTotal/usageUsed/refreshAt`、120 秒自动更新、手动 `force=true`、bridge 自动恢复、最近成功缓存与专属诊断字段。

### 第二层：Provider Manifest

在原始 Provider 外层增加统一规范化层（`providers/manifest.py`）：

- 每个 Provider 通过 `render_manifest(provider_id, provider, metadata)` 生成 schema-v1 Manifest；
- Manifest 只包含展示所需的安全字段，**绝不直接暴露**完整原始响应、诊断对象或认证材料；
- 未知字段、未知 Metric 类型、未知 Action 类型会被丢弃或安全降级，不会污染 UI。

适配方式（二选一）：

1. **插件式接口（推荐）**：新 Provider 在类上实现 `manifest(self, metadata)`，返回待校验的 Manifest 片段；
2. **命名适配器**：`MANIFEST_ADAPTERS` 注册 `provider_id -> adapter(status, metadata)`，用于采集器必须保持零改动的场景（codex、workbuddy、deepseek、system 均使用此方式）。

没有适配器的 Provider 使用保守的通用适配器（仅从 `points/balance/remaining/credits/value/total` 白名单键生成主 Metric）。

## Provider 生命周期

1. **注册**：`build_provider_registry()` 构造内置 Provider（示例 Provider 仅当 `AICC_DEV_PROVIDERS=1` 时注册）；
2. **采集**：`CollectorManager` 以独立线程并行刷新，超时/异常互相隔离；`refresh_one(name)` 支持只刷新单个 Provider；
3. **展示**：`/api/providers` 对每个 Provider 执行 `render_manifest()`，单个 Provider 失败只返回该 Provider 的 `error` Manifest，不破坏整个列表；
4. **操作**：前端只携带 `provider_id + action.kind`，后端 Action 白名单映射执行；Manifest 禁止提供 `endpoint / shell_command / executable / script_path / external_url`。

## API 契约

| 方法 | 路径 | 说明 | 权限 |
| --- | --- | --- | --- |
| GET | `/api/providers` | schema-v1 Provider 列表（按 sort_order 排序） | 局域网可读 |
| GET | `/api/providers/<id>` | 单个 Provider Manifest | 局域网可读 |
| POST | `/api/providers/<id>/refresh` | 强制刷新单个 Provider 并返回 Manifest | 仅 localhost |
| POST | `/api/providers/<id>/actions/<kind>` | 白名单 Action（refresh / reconnect / diagnostics） | 仅 localhost |
| GET | `/api/status` | 旧格式状态（保持完全兼容） | 局域网可读 |
| POST | `/api/refresh` | 全量强制刷新（保持兼容） | 仅 localhost |
| POST | `/api/workbuddy/reconnect` | WorkBuddy 重连（保持兼容） | 仅 localhost |

`/api/providers` 示例：

```json
{
  "schema_version": 1,
  "updated_at": "2026-07-31 19:20:00",
  "providers": [
    {
      "id": "workbuddy",
      "display_name": "WorkBuddy",
      "category": "credits",
      "icon": "wand.and.stars",
      "state": "connected",
      "available": true,
      "stale": false,
      "updated_at": "2026-07-31 19:19:50",
      "sort_order": 20,
      "capabilities": ["refresh", "reconnect", "diagnostics"],
      "metrics": [
        {"key": "points", "label": "剩余积分", "value": 5343.37, "value_type": "number", "format": "decimal", "unit": "积分", "primary": true},
        {"key": "used_today", "label": "今日使用", "value": 126, "value_type": "number", "format": "decimal", "unit": "积分", "primary": false}
      ],
      "actions": [
        {"id": "reconnect", "label": "重连 WorkBuddy", "kind": "reconnect", "local_only": true}
      ]
    }
  ]
}
```

完整字段说明见 [provider-schema-v1.md](provider-schema-v1.md)。

## Action 白名单

第一阶段的 Action 由后端白名单映射执行：

| kind | 说明 | 可用 Provider |
| --- | --- | --- |
| `refresh` | 强制刷新该 Provider | 全部可刷新 Provider |
| `reconnect` | 重连 WorkBuddy bridge（运行打包脚本） | 仅 workbuddy |
| `diagnostics` | 返回安全诊断（截断、脱敏） | workbuddy（后续可扩展） |

`diagnostics` 只返回 `collection` 元数据（state / last_success / age_seconds / error 截断等）与 `health` 摘要，**不返回** `balance_diagnostic` 内部对象或原始账户数据。

## 安全边界

1. Provider ID 必须匹配 `[a-z0-9][a-z0-9_-]{0,63}`；
2. Manifest 不允许包含可执行代码（HTML/JS/SwiftUI/CSS/Shell/Python 路径）；
3. 不允许通过 API 指定任意 Python 文件路径或加载仓库外代码；
4. 不允许 Provider 下发任意远程 URL；
5. 写操作（刷新、Action、重连）仅允许 localhost；
6. Manifest 剥离 Token、Cookie、API Key、Authorization、WorkBuddy 完整账户对象、`balance_diagnostic` 完整内部对象与用户聊天内容；
7. 错误信息截断（默认 80 字符、诊断错误 160 字符）并脱敏；
8. WorkBuddy daemon RPC 仍只返回白名单余额字段；
9. 动态 Action 只能命中白名单映射。

## 设置迁移

旧设置：

```json
{
  "menuBarShowWorkBuddy": true,
  "menuBarShowDeepSeek": false
}
```

首次升级自动迁移为：

```json
{
  "provider_order": ["codex", "workbuddy", "deepseek", "system"],
  "hidden_providers": ["deepseek"]
}
```

迁移只在 `providerOrderData` 不存在时执行一次，不会丢失用户的卡片显示、自动刷新、语言、OpenCodex 或 WorkBuddy 配置。旧的 `menuBarShow*` 键保留兼容，但不再新增逐 Provider 开关。

## Android / Poke4S 后续动态化方案

P1 保持 Android 完全兼容：`GET /api/status` 原字段不变，新增字段不参与解析，现有 APK 继续正常显示 Codex、WorkBuddy、DeepSeek、System。

下一阶段建议：

1. **数据层**：改用 `GET /api/providers` 作为唯一数据源，本地建立 `ProviderSummary` 等价模型（Gson 忽略未知字段）；
2. **渲染层**：Canvas 布局按 `category` 分发：`credits` 走积分/余额卡片、`quota` 走百分比卡片、`system` 走只读卡片；新 Provider 无需改 Java 代码；
3. **排序与隐藏**：复用 `sort_order`，隐藏集合存 SharedPreferences；
4. **操作**：仅对本机入口暴露刷新/Action 按钮（Android 端默认只读）；
5. **兼容期**：双数据源并存，`/api/providers` 不可用时回退 `/api/status` 固定布局，直到所有旧版本服务器下线。

## Schema 版本升级原则

- `schema_version` 递增代表不可兼容变更（字段删除/语义变化）；
- 兼容新增（可选字段、新 value_type、新 action kind）**不递增版本**；
- 客户端必须忽略未知字段；未知 Metric 类型降级为普通文本；
- 后端必须同时保留旧 `/api/status`，直到所有已发布客户端完成迁移。
