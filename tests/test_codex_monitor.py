import tempfile
import threading
import unittest
import json
from unittest.mock import MagicMock
from pathlib import Path
from unittest.mock import patch

from services.codex_monitor import CodexMonitor


class CodexMonitorTests(unittest.TestCase):
    def test_concurrent_rate_limit_calls_share_pending_request(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._io_lock = threading.Lock()
        monitor._request_id = 0
        monitor._rate_limit_request_id = None
        monitor._process = MagicMock()
        monitor._request_limits()
        monitor._request_limits()
        self.assertEqual(monitor._process.stdin.write.call_count, 1)
        self.assertEqual(monitor._rate_limit_request_id, 1)

    def test_immediate_rate_limit_response_is_registered_before_write(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._lock = threading.Lock()
        monitor._io_lock = threading.Lock()
        monitor._request_id = 0
        monitor._rate_limit_request_id = None
        monitor._initialize_request_id = None
        monitor._fresh_event = threading.Event()
        monitor._status = {"state": "Connected"}
        monitor._started = True
        written = threading.Event()
        applied = threading.Event()
        payload = {}

        class Stdin:
            def write(self, line):
                payload.update(json.loads(line))

            def flush(self):
                written.set()
                applied.wait(1)

        class Process:
            stdin = Stdin()

            @property
            def stdout(self):
                def lines():
                    if written.wait(1):
                        yield json.dumps({"id": payload["id"], "result": {"primary": {"usedPercent": 1}}})
                return lines()

        process = Process()
        monitor._process = process

        def apply(result):
            applied.set()
        with patch.object(monitor, "_apply_limits", side_effect=apply), \
                patch.object(monitor, "_schedule_restart"), patch.object(monitor, "_stop_process"):
            reader = threading.Thread(target=monitor._read_stdout, args=(process,))
            reader.start()
            monitor._request_limits()
            reader.join(2)
        self.assertTrue(applied.is_set())
        self.assertFalse(reader.is_alive())

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

    def test_find_windows_primary_300_secondary_10080(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        payload = {
            "rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 10, "windowDurationMins": 300},
                "secondary": {"usedPercent": 20, "windowDurationMins": 10080},
            }
        }
        windows = monitor._find_windows(payload)
        self.assertEqual(windows["five_hour"]["usedPercent"], 10)
        self.assertEqual(windows["weekly"]["usedPercent"], 20)

    def test_find_windows_primary_10080_maps_to_weekly(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        payload = {
            "rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 15, "windowDurationMins": 10080},
            }
        }
        windows = monitor._find_windows(payload)
        self.assertNotIn("five_hour", windows)
        self.assertEqual(windows["weekly"]["usedPercent"], 15)

    def test_find_windows_secondary_300_maps_to_five_hour_not_weekly(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        payload = {
            "rateLimits": {
                "limitId": "codex",
                "secondary": {"usedPercent": 25, "windowDurationMins": 300},
            }
        }
        windows = monitor._find_windows(payload)
        self.assertEqual(windows["five_hour"]["usedPercent"], 25)
        self.assertNotIn("weekly", windows)

    def test_find_windows_prefers_main_rate_limits_over_other_buckets(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        payload = {
            "rateLimits": {
                "limitId": "codex",
                "primary": {"usedPercent": 5, "windowDurationMins": 300},
                "secondary": {"usedPercent": 12, "windowDurationMins": 10080},
            },
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "primary": {"usedPercent": 5, "windowDurationMins": 300},
                    "secondary": {"usedPercent": 12, "windowDurationMins": 10080},
                },
                "review": {
                    "limitId": "review",
                    "primary": {"usedPercent": 90, "windowDurationMins": 10080},
                },
            },
        }
        windows = monitor._find_windows(payload)
        self.assertEqual(windows["five_hour"]["usedPercent"], 5)
        self.assertEqual(windows["weekly"]["usedPercent"], 12)
        buckets = monitor._extract_limit_buckets(payload)
        self.assertEqual(len(buckets), 2)
        self.assertEqual([b["id"] for b in buckets], ["codex", "review"])

    def test_find_windows_legacy_fallback_when_duration_missing(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        payload = {
            "primary": {"usedPercent": 30},
            "secondary": {"usedPercent": 40},
        }
        windows = monitor._find_windows(payload)
        self.assertEqual(windows["five_hour"]["usedPercent"], 30)
        self.assertEqual(windows["weekly"]["usedPercent"], 40)

    def test_sparse_update_preserves_weekly_quota(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._lock = threading.Lock()
        monitor._refresh_seconds = 60
        monitor._last_success_epoch = 0.0
        monitor._restart_attempts = 0
        monitor._status = {
            "source": "test",
            "five_hour": {"remaining": 80, "duration_minutes": 300},
            "weekly": {"remaining": 63, "duration_minutes": 10080},
        }
        with tempfile.TemporaryDirectory() as directory:
            monitor._cache_path = Path(directory) / "cache.json"
            # Sparse update with only 5-hour
            monitor._apply_limits({
                "primary": {"usedPercent": 10, "windowDurationMins": 300}
            })
        self.assertEqual(monitor._status["five_hour"]["remaining"], 90)
        self.assertEqual(monitor._status["weekly"]["remaining"], 63)

    def test_duration_always_takes_precedence_over_slot_name(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        payload = {
            "primary": {"usedPercent": 50, "windowDurationMins": 10080},
            "secondary": {"usedPercent": 10, "windowDurationMins": 300},
        }
        windows = monitor._find_windows(payload)
        self.assertEqual(windows["five_hour"]["usedPercent"], 10)
        self.assertEqual(windows["weekly"]["usedPercent"], 50)

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

    def test_legacy_mode_does_not_overwrite_pool_cache(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch("services.codex_monitor.DATA_ROOT", Path(directory)):
            pool = Path(directory) / "codex_last_success.json"
            pool.write_text('{"schema_version":2,"source":"OpenCodex","accounts":[]}', encoding="utf-8")
            monitor = CodexMonitor()
            self.assertEqual(monitor._cache_path.name, "codex_app_server_last_success.json")
            monitor._save_cache({"weekly": {"remaining": 50}, "updated_epoch": 100})
            self.assertEqual(json.loads(pool.read_text())["schema_version"], 2)

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
