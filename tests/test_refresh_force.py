"""Concurrency tests for fixed collector force refresh semantics."""

import threading
import time
import unittest

from services.collector_manager import CollectorManager


class ForceRefreshTests(unittest.TestCase):
    def _wait_until(self, predicate, timeout=3.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return
            time.sleep(0.01)
        raise AssertionError("condition not met in time")

    def test_idle_snapshot_runs_force_immediately(self):
        calls = []

        def collect(force=False):
            calls.append(force)
            return {"force": force}

        manager = CollectorManager({"p": (collect, 60.0, 1.0, {})})
        values, metadata = manager.snapshot(force=True, wait_seconds=1.0)

        self.assertEqual(calls, [True])
        self.assertTrue(values["p"]["force"])
        self.assertEqual(metadata["p"]["state"], "ready")

    def test_force_request_during_normal_refresh_runs_after_it(self):
        started = threading.Event()
        release = threading.Event()
        calls = []
        calls_lock = threading.Lock()

        def collect(force=False):
            with calls_lock:
                calls.append(force)
            started.set()
            if not force:
                release.wait(2.0)
            return {"force": force}

        manager = CollectorManager({"p": (collect, 60.0, 1.0, {})})
        normal_result = {}

        def run_normal():
            normal_result["values"], normal_result["metadata"] = manager.snapshot(
                force=False, wait_seconds=3.0
            )

        normal_thread = threading.Thread(target=run_normal)
        normal_thread.start()
        self._wait_until(lambda: started.is_set() and manager._slots["p"].running)

        force_result = {}

        def run_force():
            force_result["values"], force_result["metadata"] = manager.snapshot(
                force=True, wait_seconds=3.0
            )

        force_thread = threading.Thread(target=run_force)
        force_thread.start()
        self._wait_until(lambda: manager._slots["p"].pending_force)
        release.set()
        normal_thread.join(timeout=4.0)
        force_thread.join(timeout=4.0)

        self.assertFalse(normal_thread.is_alive())
        self.assertFalse(force_thread.is_alive())
        self.assertEqual(calls, [False, True])
        self.assertTrue(force_result["values"]["p"]["force"])
        self.assertEqual(force_result["metadata"]["p"]["state"], "ready")
        self.assertTrue(normal_result["values"]["p"]["force"])

    def test_repeated_forced_snapshots_merge_and_keep_one_thread(self):
        started = threading.Event()
        release = threading.Event()
        calls = []
        calls_lock = threading.Lock()

        def collect(force=False):
            with calls_lock:
                calls.append(force)
            started.set()
            release.wait(2.0)
            return {"force": force}

        manager = CollectorManager({"p": (collect, 60.0, 1.0, {})})
        manager.snapshot(force=True, wait_seconds=0)
        self.assertTrue(started.wait(1.0))
        for _ in range(20):
            manager.snapshot(force=True, wait_seconds=0)

        self.assertEqual([thread.name for thread in threading.enumerate()].count("collect-p"), 1)
        release.set()
        self._wait_until(lambda: not manager._slots["p"].running and not manager._slots["p"].worker_alive)
        self.assertEqual(calls, [True])

    def test_refresh_after_timeout_starts_fresh_force_worker(self):
        calls = []
        calls_lock = threading.Lock()

        def collect(force=False):
            with calls_lock:
                calls.append(force)
                call_number = len(calls)
            if call_number == 1:
                time.sleep(1.6)
            return {"force": force, "call": call_number}

        # CollectorSlot clamps timeouts to a 1.0s minimum.
        manager = CollectorManager({"p": (collect, 60.0, 0.8, {})})
        _, metadata = manager.snapshot(force=True, wait_seconds=1.4)
        self.assertEqual(metadata["p"]["state"], "timeout")

        values, second_metadata = manager.snapshot(force=True, wait_seconds=2.0)
        self.assertEqual(calls, [True, True])
        self.assertEqual(values["p"]["call"], 2)
        self.assertEqual(second_metadata["p"]["state"], "ready")
        self._wait_until(lambda: not any(t.name == "collect-p" for t in threading.enumerate()))
        values, _ = manager.snapshot(wait_seconds=0)
        self.assertEqual(values["p"]["call"], 2)

    def test_collectors_refresh_independently(self):
        started = {name: threading.Event() for name in ("first", "second")}
        release = threading.Event()
        calls = {name: [] for name in ("first", "second")}
        calls_lock = threading.Lock()

        def make_collect(name):
            def collect(force=False):
                with calls_lock:
                    calls[name].append(force)
                started[name].set()
                if not force:
                    release.wait(2.0)
                return {"force": force}

            return collect

        manager = CollectorManager({
            "first": (make_collect("first"), 60.0, 1.0, {}),
            "second": (make_collect("second"), 60.0, 1.0, {}),
        })
        normal_thread = threading.Thread(target=lambda: manager.snapshot(force=False, wait_seconds=3.0))
        normal_thread.start()
        self._wait_until(lambda: all(event.is_set() for event in started.values()))

        force_thread = threading.Thread(target=lambda: manager.snapshot(force=True, wait_seconds=3.0))
        force_thread.start()
        self._wait_until(lambda: all(manager._slots[name].pending_force for name in started))
        release.set()
        normal_thread.join(timeout=4.0)
        force_thread.join(timeout=4.0)

        self.assertFalse(normal_thread.is_alive())
        self.assertFalse(force_thread.is_alive())
        self.assertEqual(calls["first"], [False, True])
        self.assertEqual(calls["second"], [False, True])


if __name__ == "__main__":
    unittest.main()
