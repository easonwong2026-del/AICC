"""Shared Provider protocol and cache location for AICC data sources."""

from __future__ import annotations

from pathlib import Path
from typing import Any, Callable, Protocol


class CacheStore:
    """Namespaced view over the single AICC runtime data directory."""

    def __init__(self, root: Path) -> None:
        self.root = root.expanduser()

    def file(self, name: str) -> Path:
        return self.root / name

    def ensure(self) -> Path:
        self.root.mkdir(parents=True, exist_ok=True)
        return self.root


class Provider(Protocol):
    name: str
    interval: float

    def status(self) -> dict[str, Any]:
        """Return the most recent in-memory or cached value without refreshing."""

    def health(self) -> dict[str, Any]:
        """Return a small health summary without starting a provider refresh."""

    def refresh(self) -> dict[str, Any]:
        """Perform one provider refresh and return its normalized status."""

    def cache(self) -> CacheStore:
        """Return the provider's shared cache store."""


class CallableProvider:
    """Adapter used while migrating existing function-based collectors."""

    def __init__(
        self,
        name: str,
        collect: Callable[[], dict[str, Any]],
        interval: float,
        initial: dict[str, Any],
        cache: CacheStore,
    ) -> None:
        self.name = name
        self.interval = interval
        self._collect = collect
        self._status = initial.copy()
        self._cache = cache

    def status(self) -> dict[str, Any]:
        return self._status.copy()

    def health(self) -> dict[str, Any]:
        value = self.status()
        state = value.get("state", value.get("status", "unknown"))
        return {"provider": self.name, "ok": bool(value), "state": str(state)}

    def refresh(self) -> dict[str, Any]:
        value = self._collect()
        if not isinstance(value, dict):
            raise TypeError("provider did not return an object")
        self._status = value.copy()
        return value

    def cache(self) -> CacheStore:
        return self._cache
