"""Local system health provider adapter."""

from __future__ import annotations

from typing import Any

from collectors.system import collect
from providers.base import CacheStore


class SystemProvider:
    name = "system"
    interval = 60.0

    def __init__(self, cache: CacheStore) -> None:
        self._cache = cache
        self._status: dict[str, Any] = {"status": "Loading"}

    def status(self) -> dict[str, Any]:
        return self._status.copy()

    def health(self) -> dict[str, Any]:
        value = self.status()
        return {
            "provider": self.name,
            "ok": value.get("status") == "Online",
            "state": value.get("status", "pending"),
        }

    def refresh(self) -> dict[str, Any]:
        value = collect()
        self._status = value.copy()
        return value

    def cache(self) -> CacheStore:
        return self._cache
