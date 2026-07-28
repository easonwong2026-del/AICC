"""DeepSeek provider adapter."""

from __future__ import annotations

from typing import Any

from collectors.deepseek import collect
from providers.base import CacheStore


class DeepSeekProvider:
    name = "deepseek"
    interval = 300.0

    def __init__(self, cache: CacheStore) -> None:
        self._cache = cache
        self._status: dict[str, Any] = {"status": "Loading", "balances": []}

    def status(self) -> dict[str, Any]:
        return self._status.copy()

    def health(self) -> dict[str, Any]:
        value = self.status()
        return {
            "provider": self.name,
            "ok": value.get("status") in ("Online", "Not configured"),
            "state": value.get("status", "pending"),
        }

    def refresh(self) -> dict[str, Any]:
        value = collect(self._cache.file("deepseek_history.json"))
        self._status = value.copy()
        return value

    def cache(self) -> CacheStore:
        return self._cache
