"""Run independent collectors concurrently while serving their last snapshots."""

from __future__ import annotations

import threading
import time
from datetime import datetime


class CollectorSlot:
    """Small mutable record without the import cost of dataclasses."""

    __slots__ = ("collect", "interval", "value", "running", "last_attempt", "last_success", "error")

    def __init__(self, collect, interval: float, value: dict) -> None:
        self.collect = collect
        self.interval = interval
        self.value = value
        self.running = False
        self.last_attempt = 0.0
        self.last_success = 0.0
        self.error: str | None = None


class CollectorManager:
    def __init__(self, definitions: dict[str, tuple]) -> None:
        self._condition = threading.Condition()
        self._slots = {
            name: CollectorSlot(collect=collect, interval=max(5, interval), value=initial.copy())
            for name, (collect, interval, initial) in definitions.items()
        }

    def snapshot(self, *, force: bool = False, wait_seconds: float = 0.0) -> tuple[dict, dict]:
        started: set[str] = set()
        with self._condition:
            now = time.monotonic()
            for name, slot in self._slots.items():
                if not slot.running and (force or not slot.last_attempt or now - slot.last_attempt >= slot.interval):
                    slot.running = True
                    slot.last_attempt = now
                    started.add(name)
                    threading.Thread(
                        target=self._run,
                        args=(name, slot),
                        name=f"collect-{name}",
                        daemon=True,
                    ).start()
            deadline = time.monotonic() + max(0.0, wait_seconds)
            while started and any(self._slots[name].running for name in started):
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                self._condition.wait(remaining)
            return self._values_locked(), self._metadata_locked()

    def invalidate(self, *names: str) -> None:
        with self._condition:
            targets = names or tuple(self._slots)
            for name in targets:
                if name in self._slots:
                    self._slots[name].last_attempt = 0.0

    def _run(self, name: str, slot: CollectorSlot) -> None:
        try:
            value = slot.collect()
            if not isinstance(value, dict):
                raise TypeError("collector did not return an object")
        except Exception as error:  # collectors are an isolation boundary
            with self._condition:
                slot.error = f"{type(error).__name__}: {error}"[:160]
                slot.running = False
                self._condition.notify_all()
            return
        with self._condition:
            slot.value = value
            slot.error = None
            slot.last_success = time.time()
            slot.running = False
            self._condition.notify_all()

    def _values_locked(self) -> dict:
        return {name: slot.value.copy() for name, slot in self._slots.items()}

    def _metadata_locked(self) -> dict:
        now = time.time()
        result = {}
        for name, slot in self._slots.items():
            result[name] = {
                "state": "refreshing" if slot.running else "error" if slot.error else "ready" if slot.last_success else "pending",
                "last_success": datetime.fromtimestamp(slot.last_success).astimezone().strftime("%Y-%m-%d %H:%M:%S") if slot.last_success else None,
                "age_seconds": max(0, round(now - slot.last_success)) if slot.last_success else None,
                "error": slot.error,
                "refresh_seconds": round(slot.interval),
            }
        return result
