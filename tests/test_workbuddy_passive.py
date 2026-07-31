import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from collectors import workbuddy
from services.cdp import CdpError
from providers.base import CacheStore
from providers.workbuddy import WorkBuddyProvider
from services.collector_manager import CollectorManager


class WorkBuddyPassiveTests(unittest.TestCase):
    def setUp(self):
        workbuddy._last_attempt = 0.0
        workbuddy._account_cache = None
        workbuddy._account_target = None
        workbuddy._account_last_error = False

    def test_same_target_is_read_again_after_refresh_interval(self):
        raw = {"usageLeft": "6000", "refreshAt": None}
        updated = {"usageLeft": "5238", "refreshAt": None}
        with patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "_save_account_cache"), \
             patch.object(workbuddy, "target_identity_localhost", return_value="session-1"), \
             patch.object(workbuddy, "evaluate_localhost", side_effect=[raw, updated]) as evaluate, \
             patch.object(workbuddy.time, "monotonic", side_effect=[100.0, 219.0, 221.0]), \
             patch.object(workbuddy.time, "time", return_value=1_000.0):
            first = workbuddy._get_account_usage()
            cached = workbuddy._get_account_usage()
            refreshed = workbuddy._get_account_usage()

        self.assertEqual(first["points"], 6000)
        self.assertEqual(cached["points"], 6000)
        self.assertEqual(refreshed["points"], 5238)
        self.assertEqual(evaluate.call_count, 2)

    def test_manual_refresh_bypasses_interval_on_same_target(self):
        raw = {"usageLeft": "6000", "refreshAt": None}
        updated = {"usageLeft": "5238", "refreshAt": None}
        with patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "_save_account_cache"), \
             patch.object(workbuddy, "target_identity_localhost", return_value="session-1"), \
             patch.object(workbuddy, "evaluate_localhost", side_effect=[raw, updated]) as evaluate, \
             patch.object(workbuddy.time, "monotonic", return_value=100.0), \
             patch.object(workbuddy.time, "time", return_value=1_000.0):
            workbuddy._get_account_usage()
            result = workbuddy._get_account_usage(force=True)

        self.assertEqual(result["points"], 5238)
        self.assertEqual(evaluate.call_count, 2)

    def test_failed_read_keeps_last_successful_balance(self):
        with patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "_save_account_cache"), \
             patch.object(workbuddy, "target_identity_localhost", return_value="session-1"), \
             patch.object(workbuddy, "evaluate_localhost", side_effect=[{"usageLeft": "6000"}, CdpError("read failed")]), \
             patch.object(workbuddy.time, "monotonic", side_effect=[100.0, 221.0]), \
             patch.object(workbuddy.time, "time", return_value=1_000.0):
            first = workbuddy._get_account_usage()
            second = workbuddy._get_account_usage(force=True)

        self.assertEqual(first["points"], 6000)
        self.assertEqual(second["points"], 6000)
        self.assertEqual(second["balance_updated_epoch"], 1_000.0)
        self.assertEqual(second["balance_state"], "Cached")

    def test_expired_cache_is_marked_stale(self):
        cache = {"points": 5238, "balance_updated_epoch": 100.0}
        with patch.object(workbuddy, "_load_account_cache", return_value=cache), \
             patch.object(workbuddy, "target_identity_localhost", side_effect=CdpError("closed")), \
             patch.object(workbuddy.time, "monotonic", return_value=100.0), \
             patch.object(workbuddy.time, "time", return_value=501.0):
            result = workbuddy._get_account_usage()

        self.assertEqual(result["points"], 5238)
        self.assertTrue(result["balance_stale"])
        self.assertEqual(result["balance_age_seconds"], 401)
        self.assertEqual(result["balance_state"], "Cached")

    def test_closed_bridge_is_passive_and_does_not_evaluate_or_start(self):
        with patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "target_identity_localhost", side_effect=CdpError("closed")), \
             patch.object(workbuddy, "evaluate_localhost") as evaluate, \
             patch.object(workbuddy.time, "monotonic", return_value=100.0):
            self.assertIsNone(workbuddy._get_account_usage())

        evaluate.assert_not_called()

    def test_balance_number_formats_are_normalised(self):
        for raw, expected in (("5238", 5238), ("5,238", 5238), ("5，238", 5238), ("5238.5", 5238.5)):
            with self.subTest(raw=raw):
                self.assertEqual(workbuddy._normalise_account_usage({"usageLeft": raw})["points"], expected)

    def test_workbuddy_refresh_forwards_force(self):
        provider = WorkBuddyProvider(CacheStore(Path(".")), lambda: {}, {})
        with patch("providers.workbuddy.collect", return_value={"points": 5238}) as collect:
            provider.refresh(force=True)
        collect.assert_called_once_with({}, force=True)

    def test_provider_status_recomputes_balance_age_without_collecting(self):
        provider = WorkBuddyProvider(
            CacheStore(Path(".")),
            lambda: {},
            {"points": 5238, "balance_updated_epoch": 100.0, "balance_state": "Connected"},
        )
        with patch.object(workbuddy.time, "time", return_value=501.0):
            result = provider.status()

        self.assertEqual(result["balance_age_seconds"], 401)
        self.assertTrue(result["balance_stale"])
        self.assertEqual(result["balance_state"], "Cached")

    def test_concurrent_workbuddy_refreshes_are_coalesced(self):
        started = threading.Event()
        release = threading.Event()
        calls = 0

        def collect(_fallback, force=False):
            nonlocal calls
            calls += 1
            started.set()
            release.wait(1)
            return {"points": 5238}

        with tempfile.TemporaryDirectory() as directory, patch("providers.workbuddy.collect", side_effect=collect):
            provider = WorkBuddyProvider(CacheStore(Path(directory)), lambda: {}, {})
            manager = CollectorManager({"workbuddy": provider})
            manager.snapshot(force=True)
            self.assertTrue(started.wait(1))
            manager.snapshot(force=True)
            release.set()

        self.assertEqual(calls, 1)

    def test_closed_bridge_keeps_cache_and_allows_next_session_probe(self):
        cache = {"points": 88, "balance_updated_epoch": 900.0}
        with patch.object(workbuddy, "_load_account_cache", return_value=cache), \
             patch.object(workbuddy, "target_identity_localhost", side_effect=CdpError("closed")), \
             patch.object(workbuddy.time, "monotonic", return_value=100.0), \
             patch.object(workbuddy.time, "time", return_value=1_000.0):
            result = workbuddy._get_account_usage()

        self.assertEqual(result["points"], 88)
        self.assertIsNone(workbuddy._account_target)


if __name__ == "__main__":
    unittest.main()
