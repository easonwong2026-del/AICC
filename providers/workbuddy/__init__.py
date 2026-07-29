"""WorkBuddy provider adapter."""

from __future__ import annotations

from typing import Any, Callable

from collectors.workbuddy import collect
from providers.base import DEFAULT_PROVIDER_INTERVAL, DEFAULT_PROVIDER_TIMEOUT, CacheStore


class WorkBuddyProvider:
    name = "workbuddy"
    interval = DEFAULT_PROVIDER_INTERVAL
    timeout = DEFAULT_PROVIDER_TIMEOUT

    def __init__(
        self,
        cache: CacheStore,
        fallback: Callable[[], dict[str, Any]],
        initial: dict[str, Any],
    ) -> None:
        self._cache = cache
        self._fallback = fallback
        self._status = initial.copy()

    def status(self) -> dict[str, Any]:
        return self._status.copy()

    def health(self) -> dict[str, Any]:
        value = self.status()
        return {
            "provider": self.name,
            "ok": value.get("points") is not None,
            "state": value.get("balance_state", "pending"),
        }

    def refresh(self) -> dict[str, Any]:
        value = collect(self._fallback())
        self._status = value.copy()
        return value

    def cache(self) -> CacheStore:
        return self._cache
