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

    def test_pending_force_starts_at_watchdog_without_old_worker_returning(self):
        old_started, force_started, old_release, force_release = (threading.Event() for _ in range(4))
        calls, results = [], []

        def collect(force=False):
            calls.append(force)
            (force_started if force else old_started).set()
            # No timeout: the old worker cannot finish until test cleanup.
            (force_release if force else old_release).wait()
            return {"value": "new" if force else "old"}

        manager = CollectorManager({"p": (collect, 60, 1, {"value": "initial"})})
        manager.snapshot(wait_seconds=0)
        self.assertTrue(old_started.wait(1))
        waiter = threading.Thread(target=lambda: results.append(manager.snapshot(force=True, wait_seconds=4)))
        waiter.start()
        try:
            self._wait_until(lambda: manager._slots["p"].pending_force)
            self.assertTrue(force_started.wait(2), "watchdog must start force before old thread returns")
            self.assertTrue(waiter.is_alive(), "caller must wait for the force result")
            self.assertFalse(old_release.is_set())
            force_release.set()
            waiter.join(2)
            self.assertFalse(waiter.is_alive())
            self.assertEqual(results[0][0]["p"], {"value": "new"})
            self.assertEqual(results[0][1]["p"]["state"], "ready")
            before = manager._slots["p"].duration_ms
            old_release.set()
            self._wait_until(lambda: not any(t.name == "collect-p" for t in threading.enumerate()))
            self.assertEqual(manager.snapshot()[0]["p"], {"value": "new"})
            self.assertEqual(manager._slots["p"].duration_ms, before)
            self.assertEqual(calls, [False, True])
        finally:
            old_release.set()
            force_release.set()
            waiter.join(2)

    def test_concurrent_force_callers_wait_for_one_worker(self):
        started, release, second_entered = (threading.Event() for _ in range(3))
        calls, results = [], []

        def collect(force=False):
            calls.append(force)
            started.set()
            release.wait()
            return {"value": "new"}

        manager = CollectorManager({"p": (collect, 60, 3, {"value": "old"})})
        first = threading.Thread(target=lambda: results.append(manager.snapshot(force=True, wait_seconds=4)))

        def second_call():
            second_entered.set()
            results.append(manager.snapshot(force=True, wait_seconds=4))

        second = threading.Thread(target=second_call)
        first.start()
        try:
            self.assertTrue(started.wait(1))
            second.start()
            self.assertTrue(second_entered.wait(1))
            second.join(0.1)
            self.assertTrue(second.is_alive(), "second force must not return the old snapshot")
            release.set()
            first.join(2)
            second.join(2)
            self.assertEqual(calls, [True])
            self.assertEqual(len(results), 2)
            for values, metadata in results:
                self.assertEqual(values["p"], {"value": "new"})
                self.assertEqual(metadata["p"]["state"], "ready")
        finally:
            release.set()
            first.join(2)
            if second.ident is not None:
                second.join(2)

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
