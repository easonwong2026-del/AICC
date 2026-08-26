import threading
import time
import unittest
from unittest.mock import Mock, patch

from services.codex_monitor import CodexMonitor
from services.collector_manager import CollectorManager


class ResourceUsageTests(unittest.TestCase):
    def test_collector_slots_do_not_allocate_instance_dicts(self):
        manager = CollectorManager({"one": (lambda force=False: {}, 60, 1, {})})
        slot = manager._slots["one"]
        self.assertFalse(hasattr(slot, "__dict__"))

    def test_collectors_run_independently_with_bounded_wait(self):
        release = threading.Event()

        def slow(force=False):
            release.wait(1)
            return {"status": "slow"}

        manager = CollectorManager({
            "slow": (slow, 60, 1, {"status": "cached"}),
            "fast": (lambda force=False: {"status": "ready"}, 60, 1, {}),
        })
        started = time.monotonic()
        values, metadata = manager.snapshot(force=True, wait_seconds=0.05)
        elapsed = time.monotonic() - started
        release.set()
        self.assertLess(elapsed, 0.2)
        self.assertEqual(values["fast"]["status"], "ready")
        self.assertEqual(values["slow"]["status"], "cached")
        self.assertEqual(metadata["slow"]["state"], "refreshing")

    def test_concurrent_refreshes_are_coalesced(self):
        release = threading.Event()
        calls = 0

        def collect(force=False):
            nonlocal calls
            calls += 1
            release.wait(1)
            return {"ok": True}

        manager = CollectorManager({"one": (collect, 60, 1, {})})
        manager.snapshot(force=True)
        manager.snapshot(force=True)
        release.set()
        self.assertEqual(calls, 1)

    def test_stale_collector_snapshot_is_not_marked_ready(self):
        manager = CollectorManager({
            "codex": (
                lambda force=False: {"available": True, "state": "Connecting", "stale": True},
                60,
                1,
                {},
            ),
        })

        values, metadata = manager.snapshot(force=True, wait_seconds=1.0)

        self.assertTrue(values["codex"]["stale"])
        self.assertEqual(metadata["codex"]["state"], "stale")
        self.assertIsNone(metadata["codex"]["last_success"])

    def test_only_latest_rate_limit_request_is_retained(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._last_request = 0.0
        monitor._rate_limit_request_id = None
        monitor._send = Mock(side_effect=[10, 11])
        monitor._request_limits()
        monitor._request_limits()
        self.assertEqual(monitor._rate_limit_request_id, 11)

    def test_idle_monitor_releases_child_process(self):
        process = Mock()
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._process = process
        monitor._started = True
        monitor._lock = threading.Lock()
        monitor._status = {"available": True, "state": "Connected"}
        monitor._last_access = 0.0
        monitor._idle_seconds = 30
        monitor._stop_process = Mock()
        with patch("services.codex_monitor.time.sleep"), \
             patch("services.codex_monitor.time.monotonic", return_value=31):
            monitor._refresh_loop(process)
        self.assertIsNone(monitor._process)
        self.assertFalse(monitor._started)
        self.assertEqual(monitor._status["state"], "Cached")
        monitor._stop_process.assert_called_once_with(process)


if __name__ == "__main__":
    unittest.main()
