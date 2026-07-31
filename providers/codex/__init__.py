"""Codex provider adapter."""

from __future__ import annotations

from typing import Any

from providers.base import DEFAULT_PROVIDER_INTERVAL, DEFAULT_PROVIDER_TIMEOUT, CacheStore
from services.codex_monitor import monitor


class CodexProvider:
    name = "codex"
    interval = DEFAULT_PROVIDER_INTERVAL
    timeout = DEFAULT_PROVIDER_TIMEOUT

    def __init__(self, cache: CacheStore, initial: dict[str, Any]) -> None:
        self._cache = cache
        self._status = initial.copy()

    def status(self) -> dict[str, Any]:
        return self._status.copy()

    def health(self) -> dict[str, Any]:
        value = self.status()
        return {
            "provider": self.name,
            "ok": bool(value.get("available")),
            "state": value.get("state", "pending"),
        }

    def refresh(self, force: bool = False) -> dict[str, Any]:
        value = monitor.status()
        self._status = value.copy()
        return value

    def cache(self) -> CacheStore:
        return self._cache
