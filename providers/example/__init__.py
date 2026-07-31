"""Built-in example provider for validating the dynamic provider framework.

Registered only when ``AICC_DEV_PROVIDERS=1`` so it never pollutes the
production UI.  It demonstrates the plug-in manifest interface: the provider
implements ``manifest(metadata)`` and needs no Swift model, no SwiftUI card,
and no settings switch to appear in the dashboard.
"""

from __future__ import annotations

from typing import Any

from providers.base import DEFAULT_PROVIDER_INTERVAL, DEFAULT_PROVIDER_TIMEOUT, CacheStore


class ExampleProvider:
    name = "example"
    interval = DEFAULT_PROVIDER_INTERVAL
    timeout = DEFAULT_PROVIDER_TIMEOUT

    def __init__(self, cache: CacheStore) -> None:
        self._cache = cache
        self._status: dict[str, Any] = {
            "points": 12_345.67,
            "used_today": 126,
            "state": "connected",
        }

    def status(self) -> dict[str, Any]:
        return self._status.copy()

    def health(self) -> dict[str, Any]:
        return {"provider": self.name, "ok": True, "state": "connected"}

    def refresh(self, force: bool = False) -> dict[str, Any]:
        return self._status.copy()

    def cache(self) -> CacheStore:
        return self._cache

    def manifest(self, metadata: dict[str, Any]) -> dict[str, Any]:
        """Plug-in manifest interface: the only code a new provider needs."""
        return {
            "display_name": "Example Provider",
            "category": "credits",
            "icon": "sparkles",
            "state": "connected",
            "available": True,
            "stale": False,
            "updated_at": metadata.get("last_success"),
            "sort_order": 200,
            "capabilities": ["refresh"],
            "metrics": [
                {
                    "key": "points",
                    "label": "示例积分",
                    "value": self._status["points"],
                    "value_type": "number",
                    "format": "decimal",
                    "unit": "积分",
                    "primary": True,
                },
                {
                    "key": "used_today",
                    "label": "今日使用",
                    "value": self._status["used_today"],
                    "value_type": "number",
                    "format": "decimal",
                    "unit": "积分",
                    "primary": False,
                },
            ],
            "actions": [
                {"id": "refresh", "label": "刷新示例 Provider", "kind": "refresh", "local_only": True},
            ],
        }
