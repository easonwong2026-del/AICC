"""WorkBuddy provider adapter."""

from __future__ import annotations

from typing import Any, Callable

from collectors.workbuddy import _with_stale_state, collect
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
        self._status = _initial_status(initial)

    def status(self) -> dict[str, Any]:
        value = self._status.copy()
        if value.get("balance_updated_epoch") is not None:
            return _with_stale_state(value) or value
        return value

    def health(self) -> dict[str, Any]:
        value = self.status()
        result = {
            "provider": self.name,
            "ok": value.get("points") is not None,
            "state": value.get("balance_state", "pending"),
        }
        if value.get("balance_error_code"):
            result["error_code"] = value["balance_error_code"]
            result["error"] = value.get("balance_error")
        return result

    def refresh(self, force: bool = False) -> dict[str, Any]:
        value = collect(self._fallback(), force=force)
        self._status = value.copy()
        return value

    def cache(self) -> CacheStore:
        return self._cache


def _initial_status(value: dict[str, Any]) -> dict[str, Any]:
    """Do not expose pre-2.4.3 manual points before a real read succeeds."""
    result = value.copy()
    if result.get("balance_updated_epoch") is not None:
        return result
    for key in ("points", "total_points", "cycle_used_points", "used_points", "reset_text"):
        result[key] = None
    result.update({
        "balance_state": "Unavailable",
        "balance_stale": True,
        "balance_updated_at": None,
        "balance_age_seconds": None,
        "usage_source": "WorkBuddy unavailable",
    })
    result.pop("balance_error_code", None)
    result.pop("balance_error", None)
    return result
