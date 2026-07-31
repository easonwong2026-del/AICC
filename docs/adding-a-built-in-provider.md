# 如何新增一个内置 Provider

## 最少需要做什么

1. 新建 `providers/<id>/__init__.py`，实现 Provider 协议（`status` / `refresh` / `health` / `cache`）；
2. 在类上实现 `manifest(self, metadata)`，输出展示字段；
3. 在 `providers/registry.py` 的 `build_provider_registry()` 注册一行；
4. 完成。

**不需要**修改：Dashboard 核心代码、Swift 数据模型、SwiftUI 卡片、设置开关、排序逻辑、状态显示、诊断入口。

## Provider 类模板

```python
"""providers/example/__init__.py"""

from providers.base import CacheStore, DEFAULT_PROVIDER_INTERVAL, DEFAULT_PROVIDER_TIMEOUT


class MyProvider:
    name = "myprovider"
    interval = DEFAULT_PROVIDER_INTERVAL
    timeout = DEFAULT_PROVIDER_TIMEOUT

    def __init__(self, cache: CacheStore) -> None:
        self._cache = cache
        self._status = {"points": 1000, "state": "connected"}

    def status(self):
        return self._status.copy()

    def refresh(self, force: bool = False):
        return self._status.copy()

    def health(self):
        return {"provider": self.name, "ok": True, "state": "connected"}

    def cache(self):
        return self._cache

    def manifest(self, metadata):
        """唯一的展示适配层：只输出白名单字段。"""
        return {
            "display_name": "My Provider",
            "category": "credits",           # credits / quota / system / generic
            "icon": "sparkles",
            "state": "connected",            # connected / cached / unavailable / error / pending / disabled
            "available": True,
            "stale": False,
            "updated_at": metadata.get("last_success"),
            "sort_order": 150,               # 越小越靠前
            "capabilities": ["refresh"],
            "metrics": [
                {
                    "key": "points",
                    "label": "剩余积分",
                    "value": self._status["points"],
                    "value_type": "number",   # number/percentage/currency/text/status/duration
                    "format": "decimal",      # integer/decimal/percent/currency/plain/compact
                    "unit": "积分",
                    "primary": True,
                }
            ],
            "actions": [
                {"id": "refresh", "label": "刷新", "kind": "refresh", "local_only": True}
            ],
        }
```

## 注册

```python
# providers/registry.py
from providers.myprovider import MyProvider

providers["myprovider"] = MyProvider(cache)
```

如果新 Provider 只在开发/测试模式启用：

```python
if os.environ.get("AICC_DEV_PROVIDERS") == "1":
    providers["myprovider"] = MyProvider(cache)
```

## 采集器必须零改动时

如果 Provider 的采集器是冻结链路（例如 WorkBuddy），不要改采集器，在 `providers/manifest.py` 的 `MANIFEST_ADAPTERS` 注册命名适配器：

```python
MANIFEST_ADAPTERS["myprovider"] = _myprovider_manifest
```

适配器签名：`(status: dict, metadata: dict) -> dict`，返回与 `manifest()` 相同的展示字段。适配器只能读取状态字段，最终输出还会经过 `_finalize()` 白名单过滤。

## 校验与安全

- Provider ID 必须匹配 `[a-z0-9][a-z0-9_-]{0,63}`；
- Manifest 不能包含 HTML/JS/CSS/SwiftUI 代码、Shell 命令、Python 文件路径、远程 URL；
- Action 只能是 `refresh` / `reconnect` / `diagnostics` 且由后端白名单执行；
- 不要把你的 Token、Cookie、API Key、完整账户对象或诊断内部对象放进 Manifest；
- 无有效数据时 `value` 传 `null`（UI 显示“暂不可用”），不要伪造 0；
- 长数字让 UI 处理：默认千位分隔、自动减少小数位，除非明确指定 `format: "compact"`。

## 验证

```bash
python3 -B -m unittest tests.test_manifest -v
python3 -m flake8 --max-line-length=140 --exclude=__pycache__,.git,dist,android,web .
bash scripts/smoke-test-swift-core.sh
```

新增测试建议：Manifest 符合 Schema v1、不泄露诊断字段、单个 Provider 失败不影响 `/api/providers`、Action 只能命中白名单。
