# Provider Manifest Schema v1

## 顶层响应（GET /api/providers）

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `schema_version` | int | 当前为 `1` |
| `updated_at` | string? | `%Y-%m-%d %H:%M:%S`（本地时区） |
| `providers` | array | `ProviderSummary` 列表，按 `sort_order` 升序 |

## ProviderSummary

每个 Provider 至少包含：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | `[a-z0-9][a-z0-9_-]{0,63}` |
| `display_name` | string | 展示名（≤60 字符） |
| `category` | enum | `credits` / `quota` / `system` / `generic` |
| `icon` | string? | SF Symbol 名（≤40 字符） |
| `state` | enum | `connected` / `cached` / `unavailable` / `error` / `pending` / `disabled` / `unknown` |
| `available` | bool | 是否有有效数据 |
| `stale` | bool | 是否为过期缓存 |
| `updated_at` | string? | 最近成功时间（≤32 字符） |
| `sort_order` | int | 排序权重，越小越靠前 |
| `capabilities` | array | `refresh` / `reconnect` / `diagnostics` |
| `metrics` | array | `ProviderMetric`（≤12 个） |
| `actions` | array | `ProviderAction`（≤8 个） |

## ProviderMetric

每个 Metric 至少包含：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `key` | string | 唯一键（≤40 字符） |
| `label` | string | 展示标签（≤60 字符） |
| `value` | number / string / bool / null | 数据值；无效时必须是 `null`，**禁止伪造 0** |
| `value_type` | enum | 见下 |
| `format` | enum | 见下 |
| `unit` | string? | 单位（≤16 字符），例如 `积分`、`%`、`CNY` |
| `primary` | bool | 是否为主 Metric（每 Provider 最多 2 个） |

### value_type

第一阶段允许：`number`、`percentage`、`currency`、`text`、`status`、`duration`。

未知 `value_type` 一律安全降级为 `text`。

### format

第一阶段允许：`integer`、`decimal`、`percent`、`currency`、`plain`、`compact`。

未知 `format` 一律降级为 `plain`。

### 显示规则

- 默认千位分隔；最多保留必要小数位；
- 不使用科学计数法；不直接截断；
- 长数字先减少小数位，而不是无限缩小字号；
- 极长数字只有在 `format: "compact"` 时才使用 `1.23M` 风格；
- 金额与积分不得混淆单位。

## ProviderAction

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `id` | string | 动作唯一键（≤40 字符） |
| `label` | string | 展示标签（≤60 字符） |
| `kind` | enum | `refresh` / `reconnect` / `diagnostics` |
| `local_only` | bool | 是否仅 localhost 可执行（默认 true） |

**禁止**出现在 Action 中：`endpoint`、`shell_command`、`executable`、`script_path`、`external_url`。

## 内置 Provider 默认排序

| id | sort_order |
| --- | --- |
| codex | 10 |
| workbuddy | 20 |
| deepseek | 30 |
| system | 90 |
| example（仅开发模式） | 200 |
| 其他 | 100 |

## 示例：WorkBuddy 缓存状态

```json
{
  "id": "workbuddy",
  "display_name": "WorkBuddy",
  "category": "credits",
  "state": "cached",
  "available": true,
  "stale": true,
  "metrics": [
    {"key": "points", "label": "剩余积分", "value": 5343.37, "value_type": "number", "format": "decimal", "unit": "积分", "primary": true},
    {"key": "used_today", "label": "今日使用", "value": 126, "value_type": "number", "format": "decimal", "unit": "积分", "primary": false},
    {"key": "cache_age", "label": "缓存年龄", "value": 400, "value_type": "duration", "format": "plain", "unit": "秒", "primary": false}
  ]
}
```

## 安全校验（后端强制）

- Provider ID 正则校验；
- 所有字符串截断（标签 60、单位 16、时间戳 32）；
- 非有限数值（NaN/Infinity）转 `null`；
- 未知字段直接丢弃（包括 Token、Cookie、API Key、完整账户对象、`balance_diagnostic`）；
- 错误信息截断并脱敏。
