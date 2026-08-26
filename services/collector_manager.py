"""Run independent collectors concurrently while serving their last snapshots."""

from __future__ import annotations

import threading
import time
from datetime import datetime

from collectors.workbuddy import _with_stale_state


DEFAULT_COLLECTOR_INTERVAL = 120.0
DEFAULT_COLLECTOR_TIMEOUT = 8.0


class CollectorSlot:
    """Small mutable record without the import cost of dataclasses."""

    __slots__ = (
        "collect", "interval", "timeout", "value", "running", "worker_alive",
        "running_force", "pending_force", "started_monotonic", "generation",
        "last_attempt", "last_success", "error", "timed_out", "duration_ms",
        "consecutive_failures", "snapshot_stale",
    )

    def __init__(self, collect, interval: float, timeout: float, initial: dict) -> None:
        self.collect = collect
        self.interval = max(30.0, float(interval))
        self.timeout = max(1.0, float(timeout))
        self.value = initial.copy()
        self.snapshot_stale = bool(self.value.get("stale"))
        self.running = False
        self.worker_alive = False
        self.running_force = False
        self.pending_force = False
        self.started_monotonic = 0.0
        self.generation = 0
        self.last_attempt = 0.0
        self.last_success = 0.0
        self.error: str | None = None
        self.timed_out = False
        self.duration_ms: int | None = None
        self.consecutive_failures = 0


class CollectorManager:
    def __init__(self, definitions: dict[str, tuple]) -> None:
        self._condition = threading.Condition()
        self._slots = {}
        for name, (collect, interval, timeout, initial) in definitions.items():
            self._slots[name] = CollectorSlot(collect, interval, timeout, initial)

    def snapshot(self, *, force: bool = False, wait_seconds: float = 0.0) -> tuple[dict, dict]:
        started: set[str] = set()
        with self._condition:
            now = time.monotonic()
            self._expire_locked(now)
            for name, slot in self._slots.items():
                if (not slot.worker_alive or not slot.running) and (
                    force or not slot.last_attempt or now - slot.last_attempt >= slot.interval
                ):
                    self._start_worker_locked(name, slot, force)
                    started.add(name)
                elif force and slot.worker_alive and slot.running and not slot.pending_force:
                    # A force snapshot must still produce at least one
                    # force=True collection. If the running worker is a
                    # normal (non-force) run, defer the force request and
                    # wait for the follow-up run.
                    slot.pending_force = True
                    if not slot.running_force:
                        started.add(name)
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

    def _start_worker_locked(self, name: str, slot: CollectorSlot, force: bool) -> None:
        """Start one collector worker for a slot. Callers hold the lock."""
        now = time.monotonic()
        slot.running = True
        slot.worker_alive = True
        slot.running_force = force
        slot.started_monotonic = now
        slot.last_attempt = now
        slot.generation += 1
        generation = slot.generation
        threading.Thread(
            target=self._run,
            args=(name, slot, generation, force),
            name=f"collect-{name}",
            daemon=True,
        ).start()

    def _start_pending_force_locked(
        self, name: str, slot: CollectorSlot, generation: int, force: bool
    ) -> None:
        """Honor a deferred force request after a worker finishes.

        A normal run that finished while a force request was pending is
        followed immediately by one force=True run. A force run satisfies
        every request that arrived during it (merged), so it is never
        followed by another worker. A stale worker (timeout or superseded
        generation) only acts when no newer worker owns the slot.
        """
        if not slot.pending_force:
            return
        if generation != slot.generation:
            if slot.running or slot.worker_alive:
                return
            slot.pending_force = False
            self._start_worker_locked(name, slot, force=True)
            return
        if force:
            slot.pending_force = False
            return
        slot.pending_force = False
        self._start_worker_locked(name, slot, force=True)

    def _run(self, name: str, slot: CollectorSlot, generation: int, force: bool) -> None:
        started = time.monotonic()
        try:
            value = slot.collect(force=force)
            if not isinstance(value, dict):
                raise TypeError("collector did not return an object")
        except Exception as error:  # collectors are an isolation boundary
            with self._condition:
                slot.duration_ms = round((time.monotonic() - started) * 1000)
                if generation == slot.generation:
                    slot.worker_alive = False
                    slot.error = f"{type(error).__name__}: {error}"[:160]
                    slot.running = False
                    slot.timed_out = False
                    slot.consecutive_failures += 1
                elif not slot.running:
                    # Stale worker winding down after a timeout with no newer
                    # worker: release ownership so a later refresh can start.
                    slot.worker_alive = False
                self._start_pending_force_locked(name, slot, generation, force)
                self._condition.notify_all()
            return
        with self._condition:
            slot.duration_ms = round((time.monotonic() - started) * 1000)
            if generation == slot.generation:
                slot.worker_alive = False
                slot.value = value
                slot.snapshot_stale = bool(value.get("stale"))
                slot.error = None
                slot.running = False
                slot.timed_out = False
                if not slot.snapshot_stale:
                    slot.last_success = time.time()
                    slot.consecutive_failures = 0
            elif not slot.running:
                slot.worker_alive = False
            self._start_pending_force_locked(name, slot, generation, force)
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
            # the interval. Other collectors retain the slot snapshot.
            value = slot.value.copy()
            if name == "workbuddy" and slot.value.get("balance_updated_epoch") is not None:
                value = _with_stale_state(slot.value) or value
            values[name] = value
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
                    else "stale" if slot.snapshot_stale
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
