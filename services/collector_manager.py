"""Run independent collectors concurrently while serving their last snapshots."""

from __future__ import annotations

import inspect
import threading
import time
from datetime import datetime
from pathlib import Path

from providers.base import CacheStore, CallableProvider, Provider, DEFAULT_PROVIDER_TIMEOUT


def _accepts_force(collector) -> bool:
    """Keep older third-party providers working while forwarding force when supported."""
    try:
        parameters = inspect.signature(collector).parameters.values()
    except (TypeError, ValueError):
        return True
    return any(
        parameter.name == "force" or parameter.kind == inspect.Parameter.VAR_KEYWORD
        for parameter in parameters
    )


class CollectorSlot:
    """Small mutable record without the import cost of dataclasses."""

    __slots__ = (
        "provider", "collect", "interval", "timeout", "value", "running", "worker_alive",
        "started_monotonic", "generation", "last_attempt", "last_success", "error",
        "timed_out", "duration_ms", "consecutive_failures", "accepts_force",
    )

    def __init__(self, provider: Provider) -> None:
        self.provider = provider
        self.collect = provider.refresh
        self.accepts_force = _accepts_force(self.collect)
        self.interval = max(30, provider.interval)
        self.timeout = max(1.0, float(getattr(provider, "timeout", DEFAULT_PROVIDER_TIMEOUT)))
        self.value = provider.status()
        self.running = False
        self.worker_alive = False
        self.started_monotonic = 0.0
        self.generation = 0
        self.last_attempt = 0.0
        self.last_success = 0.0
        self.error: str | None = None
        self.timed_out = False
        self.duration_ms: int | None = None
        self.consecutive_failures = 0


class CollectorManager:
    def __init__(self, definitions: dict[str, Provider | tuple]) -> None:
        self._condition = threading.Condition()
        self._slots = {}
        for name, definition in definitions.items():
            provider = definition if hasattr(definition, "refresh") else self._legacy_provider(name, definition)
            self._slots[name] = CollectorSlot(provider)
            self._slots[name].value = provider.status()

    @staticmethod
    def _legacy_provider(name: str, definition: tuple) -> Provider:
        collect, interval, initial = definition
        return CallableProvider(
            name,
            collect,
            interval,
            initial,
            CacheStore(Path(".")),
            timeout=DEFAULT_PROVIDER_TIMEOUT,
        )

    def snapshot(self, *, force: bool = False, wait_seconds: float = 0.0) -> tuple[dict, dict]:
        started: set[str] = set()
        with self._condition:
            now = time.monotonic()
            self._expire_locked(now)
            for name, slot in self._slots.items():
                if (
                    not slot.worker_alive
                    and (force or not slot.last_attempt or now - slot.last_attempt >= slot.interval)
                ):
                    slot.running = True
                    slot.worker_alive = True
                    slot.started_monotonic = now
                    slot.last_attempt = now
                    slot.generation += 1
                    generation = slot.generation
                    started.add(name)
                    threading.Thread(
                        target=self._run,
                        args=(name, slot, generation, force),
                        name=f"collect-{name}",
                        daemon=True,
                    ).start()
            deadline = time.monotonic() + max(0.0, wait_seconds)
            while started and any(self._slots[name].running for name in started):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                self._condition.wait(min(remaining, self._next_timeout_locked()))
                self._expire_locked(time.monotonic())
            return self._values_locked(), self._metadata_locked()

    def invalidate(self, *names: str) -> None:
        with self._condition:
            targets = names or tuple(self._slots)
            for name in targets:
                if name in self._slots:
                    self._slots[name].last_attempt = 0.0

    def _run(self, name: str, slot: CollectorSlot, generation: int, force: bool) -> None:
        started = time.monotonic()
        try:
            value = slot.collect(force=force) if slot.accepts_force else slot.collect()
            if not isinstance(value, dict):
                raise TypeError("collector did not return an object")
        except Exception as error:  # collectors are an isolation boundary
            with self._condition:
                slot.worker_alive = False
                slot.duration_ms = round((time.monotonic() - started) * 1000)
                if generation != slot.generation:
                    self._condition.notify_all()
                    return
                slot.error = f"{type(error).__name__}: {error}"[:160]
                slot.running = False
                slot.timed_out = False
                slot.consecutive_failures += 1
                self._condition.notify_all()
            return
        with self._condition:
            slot.worker_alive = False
            slot.duration_ms = round((time.monotonic() - started) * 1000)
            if generation != slot.generation:
                self._condition.notify_all()
                return
            slot.value = value
            slot.error = None
            slot.last_success = time.time()
            slot.running = False
            slot.timed_out = False
            slot.consecutive_failures = 0
            self._condition.notify_all()

    def _next_timeout_locked(self) -> float:
        active = [
            max(0.01, slot.timeout - (time.monotonic() - slot.started_monotonic))
            for slot in self._slots.values()
            if slot.worker_alive and slot.running
        ]
        return min(active) if active else 0.25

    def _expire_locked(self, now: float) -> None:
        for slot in self._slots.values():
            if not slot.worker_alive or not slot.running:
                continue
            if now - slot.started_monotonic < slot.timeout:
                continue
            slot.running = False
            slot.timed_out = True
            slot.error = f"Timeout after {slot.timeout:.1f}s"
            slot.consecutive_failures += 1
            slot.generation += 1

    def _values_locked(self) -> dict:
        values = {}
        for name, slot in self._slots.items():
            # WorkBuddy's age/stale fields are derived from wall time and need
            # to stay current even while its next collection is still inside
            # the interval. Other providers retain the legacy slot snapshot.
            values[name] = slot.provider.status() if name == "workbuddy" else slot.value.copy()
        return values

    def _metadata_locked(self) -> dict:
        now = time.time()
        result = {}
        for name, slot in self._slots.items():
            result[name] = {
                "state": (
                    "refreshing" if slot.running
                    else "timeout" if slot.timed_out
                    else "error" if slot.error
                    else "ready" if slot.last_success
                    else "pending"
                ),
                "last_success": (
                    datetime.fromtimestamp(slot.last_success).astimezone()
                    .strftime("%Y-%m-%d %H:%M:%S") if slot.last_success else None
                ),
                "age_seconds": max(0, round(now - slot.last_success)) if slot.last_success else None,
                "error": slot.error,
                "refresh_seconds": round(slot.interval),
                "timeout_seconds": round(slot.timeout, 1),
                "duration_ms": slot.duration_ms,
                "consecutive_failures": slot.consecutive_failures,
            }
        return result

    def health(self) -> dict:
        with self._condition:
            metadata = self._metadata_locked()
            result = {}
            for name, slot in self._slots.items():
                try:
                    provider_health = slot.provider.health()
                except Exception as error:  # health reporting must never break the server
                    provider_health = {"ok": False, "state": "error", "error": str(error)[:160]}
                item = metadata[name].copy()
                item.update(provider_health)
                item["provider"] = name
                item["ok"] = bool(provider_health.get("ok")) and item["state"] not in ("error", "timeout")
                result[name] = item
            return result
