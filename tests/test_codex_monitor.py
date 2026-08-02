import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from services.codex_monitor import CodexMonitor


class CodexMonitorTests(unittest.TestCase):
    def test_missing_reset_credits_is_not_reported_as_zero(self):
        result = CodexMonitor._normalise_reset_credits(None)
        self.assertFalse(result["provided"])
        self.assertIsNone(result["available_count"])

    def test_reset_credit_count_is_authoritative(self):
        result = CodexMonitor._normalise_reset_credits({
            "availableCount": 3,
            "credits": [
                {"status": "available", "expiresAt": 1_800_000_000},
                {"status": "available", "expiresAt": 1_700_000_000},
            ],
        })
        self.assertTrue(result["provided"])
        self.assertEqual(result["available_count"], 3)
        self.assertEqual(result["detail_count"], 2)
        self.assertTrue(result["details_limited"])
        self.assertIsNotNone(result["next_expiry"])

    def test_multiple_limit_buckets_are_preserved(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        buckets = monitor._extract_limit_buckets({
            "rateLimitsByLimitId": {
                "main": {"limitName": "Main", "primary": {"usedPercent": 25, "windowDurationMins": 300}},
                "review": {"limitName": "Review", "primary": {"usedPercent": 40, "windowDurationMins": 10080}},
            }
        })
        self.assertEqual([item["name"] for item in buckets], ["Main", "Review"])
        self.assertEqual(buckets[0]["windows"][0]["remaining"], 75)

    def test_sparse_update_keeps_previous_reset_credit_summary(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._lock = threading.Lock()
        monitor._refresh_seconds = 60
        monitor._last_success_epoch = 0.0
        monitor._restart_attempts = 0
        monitor._status = {
            "source": "test",
            "reset_credits": {"provided": True, "available_count": 2},
        }
        with tempfile.TemporaryDirectory() as directory:
            monitor._cache_path = Path(directory) / "cache.json"
            monitor._apply_limits({"primary": {"usedPercent": 10, "windowDurationMins": 300}})
        self.assertEqual(monitor._status["reset_credits"]["available_count"], 2)
        self.assertEqual(monitor._status["five_hour"]["remaining"], 90)

    def test_old_cache_gets_new_optional_fields(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._cache_path = None
        monitor._status = {}
        monitor._last_success_epoch = 0.0
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "cache.json"
            path.write_text('{"weekly":{"remaining":62},"updated_epoch":1}', encoding="utf-8")
            monitor._cache_path = path
            monitor._load_cache()
        self.assertEqual(monitor._status["limit_buckets"], [])
        self.assertFalse(monitor._status["reset_credits"]["provided"])

    def test_connecting_without_cache_is_reported_stale(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._lock = threading.Lock()
        monitor._status = {"available": False, "state": "Connecting"}
        monitor._last_success_epoch = 0.0
        monitor._fresh_event = threading.Event()
        monitor._refresh_seconds = 60
        monitor._fresh_event.set()

        with patch.object(monitor, "start"):
            result = monitor.status()

        self.assertTrue(result["stale"])


if __name__ == "__main__":
    unittest.main()
