"""Concurrency tests for single-provider force refresh semantics (0.4).

Manual refresh must always produce at least one force=True collection while
keeping at most one live worker per provider and never growing threads with
repeated clicks.
"""

import threading
import time
import unittest

from services.collector_manager import CollectorManager


class GatedProvider:
    """Provider whose refresh duration is controllable per test."""

    def __init__(self, name="p", delay=0.2, timeout=1.0):
        self.name = name
        self.interval = 60.0
        self.timeout = timeout
        self.delay = delay
        self.calls = []
        self._lock = threading.Lock()
        self._status = {"v": 0}

    def status(self):
        with self._lock:
            return dict(self._status)

    def refresh(self, force=False):
        with self._lock:
            self.calls.append(force)
        time.sleep(self.delay)
        with self._lock:
            self._status = {"v": len(self.calls), "force": force}
            return dict(self._status)

    def health(self):
        return {"ok": True, "state": "connected"}


class ForceRefreshTests(unittest.TestCase):
    def _wait_until(self, predicate, timeout=3.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.01)
        raise AssertionError("condition not met in time")

    def test_refresh_one_without_worker_runs_force_immediately(self):
        provider = GatedProvider(delay=0.05)
        manager = CollectorManager({"p": provider})
        values, metadata = manager.refresh_one("p", wait_seconds=1.0)
        self.assertEqual(provider.calls, [True])
        self.assertEqual(values["force"], True)
        self.assertEqual(metadata["state"], "ready")

    def test_force_request_during_normal_refresh_runs_after_it(self):
        provider = GatedProvider(delay=0.15)
        manager = CollectorManager({"p": provider})
        snapshot_result = {}

        def run_snapshot():
            snapshot_result["values"], snapshot_result["metadata"] = manager.snapshot(
                force=False, wait_seconds=3.0
            )

        thread = threading.Thread(target=run_snapshot)
        thread.start()
        self._wait_until(lambda: manager._slots["p"].running)
        # The normal (non-force) worker is live; a manual refresh must be
        # deferred and followed by exactly one force=True run.
        values, metadata = manager.refresh_one("p", wait_seconds=3.0)
        thread.join(timeout=4.0)
        self.assertEqual(provider.calls, [False, True])
        self.assertEqual(values["force"], True)
        self.assertEqual(metadata["state"], "ready")
        self.assertEqual(snapshot_result["values"]["p"]["force"], True)

    def test_repeated_force_clicks_merge_into_one_worker(self):
        provider = GatedProvider(delay=0.15)
        manager = CollectorManager({"p": provider})
        # Start one force worker without waiting for it.
        manager.refresh_one("p", wait_seconds=0)
        self._wait_until(lambda: manager._slots["p"].running)
        # Many clicks while it is running must merge, not spawn workers.
        for _ in range(8):
            manager.refresh_one("p", wait_seconds=0)
        self._wait_until(lambda: not manager._slots["p"].running)
        self.assertEqual(provider.calls, [True])

    def test_refresh_after_timeout_starts_fresh_force_worker(self):
        # CollectorSlot clamps timeouts to a 1.0s minimum.
        provider = GatedProvider(delay=1.6, timeout=0.8)
        manager = CollectorManager({"p": provider})
        _, metadata = manager.refresh_one("p", wait_seconds=1.4)
        self.assertEqual(metadata["state"], "timeout")
        # Let the follow-up collection finish quickly.
        provider.delay = 0.1
        # The stale thread may still be alive; the slot must be reclaimable
        # and a fresh force=True run must start immediately.
        values, second_metadata = manager.refresh_one("p", wait_seconds=2.0)
        self.assertEqual(provider.calls, [True, True])
        self.assertEqual(values["force"], True)
        self.assertEqual(second_metadata["state"], "ready")

    def test_providers_refresh_independently(self):
        first = GatedProvider("first", delay=0.15)
        second = GatedProvider("second", delay=0.15)
        manager = CollectorManager({"first": first, "second": second})
        manager.snapshot(force=False, wait_seconds=0)
        self._wait_until(
            lambda: manager._slots["first"].running and manager._slots["second"].running
        )
        manager.refresh_one("first", wait_seconds=3.0)
        self.assertEqual(first.calls, [False, True])
        self.assertEqual(second.calls, [False])

    def test_thread_count_stays_bounded_under_click_storm(self):
        provider = GatedProvider(delay=0.3)
        manager = CollectorManager({"p": provider})
        manager.refresh_one("p", wait_seconds=0)
        self._wait_until(lambda: manager._slots["p"].running)
        for _ in range(20):
            manager.refresh_one("p", wait_seconds=0)

        def collect_threads():
            return [t for t in threading.enumerate() if t.name == "collect-p"]

        # At most one live collector thread despite 21 refresh clicks.
        self._wait_until(lambda: len(collect_threads()) == 1)
        self.assertLessEqual(len(collect_threads()), 1)
        self._wait_until(lambda: not manager._slots["p"].running)
        time.sleep(0.05)
        self.assertEqual(provider.calls, [True])
        self.assertEqual(collect_threads(), [])


if __name__ == "__main__":
    unittest.main()
