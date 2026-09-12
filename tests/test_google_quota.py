import json
import subprocess
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from collectors import google
from services.collector_manager import CollectorManager
import server


def report(windows, updated=None):
    return {"reports": [{"provider": "other"}, {"provider": "google-antigravity", "quota": {
        "customWindows": windows, "updatedAt": updated or time.time() * 1000}}]}


class GoogleQuotaTests(unittest.TestCase):
    def test_windows_are_selected_by_label_and_used_percent_is_converted(self):
        data = report([{"label": "Cla", "percent": 90}, {"label": "Gem", "percent": "22", "resetAt": 1788003600000},
                       {"label": "Gem (Weekly)", "percent": 51, "resetAt": "2026-09-18T00:00:00Z"}])
        value = google.parse_quota(data)
        self.assertEqual(value["weekly"]["remaining"], 49)
        self.assertEqual(value["five_hour"]["remaining"], 78)
        self.assertNotEqual(value["weekly"]["reset"], value["five_hour"]["reset"])
        self.assertFalse(value["stale"])
        self.assertEqual(google.epoch(1788003600000), 1788003600)

    def test_missing_invalid_and_zero_windows(self):
        for used, expected in [(-10, 100), (150, 0), (100, 0)]:
            value = google.parse_quota(report([{"label": "Gem", "percent": used}]))
            self.assertEqual(value["five_hour"]["remaining"], expected)
            self.assertNotIn("weekly", value)
        for percent in [None, "bad", "NaN", True, float("inf")]:
            with self.assertRaises(ValueError):
                google.parse_quota(report([{"label": "Gem", "percent": percent}]))
        for document in [{}, [], report([]), report([None])]:
            with self.assertRaises(ValueError):
                google.parse_quota(document)
        value = google.parse_quota(report([{"label": "Gem (Weekly)", "percent": 0}], time.time() - 301))
        self.assertTrue(value["stale"])

    def test_command_force_and_errors_never_start_proxy(self):
        body = json.dumps(report([{"label": "Gem", "percent": 1}]))
        with patch.object(google, "resolve_executable", return_value="/tmp/path with spaces/ocx"), patch.object(google.subprocess, "run") as run:
            run.return_value = subprocess.CompletedProcess([], 0, stdout=body)
            google.collect(force=True)
            self.assertEqual(run.call_args.args[0], ["/tmp/path with spaces/ocx", "provider", "quota", "--refresh", "--json"])
            google.collect()
            self.assertNotIn("--refresh", run.call_args.args[0])
            run.return_value = subprocess.CompletedProcess([], 1, stdout="", stderr="Proxy is not running")
            with self.assertRaisesRegex(ValueError, "not running"):
                google.collect()
            run.side_effect = subprocess.TimeoutExpired("ocx", 8)
            with self.assertRaisesRegex(ValueError, "unavailable"):
                google.collect()

    def test_saved_executable_with_spaces(self):
        with tempfile.TemporaryDirectory(prefix="google quota ") as directory:
            executable = Path(directory) / "ocx"
            executable.touch()
            executable.chmod(0o700)
            with patch.object(google.sys, "platform", "darwin"), patch.dict(google.os.environ, {"AICC_OCX_PATH": ""}), patch.object(google.subprocess, "run") as run:
                run.return_value.stdout = str(executable) + "\n"
                self.assertEqual(google.resolve_executable(), str(executable))

    def test_collector_ttl_force_failure_retention_and_expiry(self):
        fresh = google.parse_quota(report([{"label": "Gem (Weekly)", "percent": 15}]))
        with patch.object(google, "collect", side_effect=[fresh, ValueError("offline")]) as collect:
            manager = CollectorManager({"google": (collect, 300, 10, {})})
            values, _ = manager.snapshot(wait_seconds=1)
            first = values["google"]
            manager.snapshot(wait_seconds=1)
            self.assertEqual(collect.call_count, 1)
            values, metadata = manager.snapshot(force=True, wait_seconds=1)
            self.assertEqual(collect.call_count, 2)
            self.assertTrue(values["google"]["stale"])
            self.assertEqual(values["google"]["weekly"], first["weekly"])
            self.assertEqual(values["google"]["updated_epoch"], first["updated_epoch"])
            self.assertEqual(metadata["google"]["state"], "error")
            with patch("services.collector_manager.time.time", return_value=first["updated_epoch"] + 86401):
                self.assertNotIn("weekly", manager.snapshot()[0]["google"])

    def test_google_only_changes_invalidate_display_revision(self):
        class Manager:
            remaining = 80
            def snapshot(self, **kwargs):
                return {"google": {"weekly": {"remaining": self.remaining}}}, {}
        manager = Manager()
        with tempfile.TemporaryDirectory() as directory, patch.object(server, "DATA_PATH", Path(directory) / "status.json"), patch.object(server, "collector_manager", return_value=manager):
            first = server.load_status()
            manager.remaining = 75
            second = server.load_status()
            self.assertNotEqual(first["display_revision"], second["display_revision"])
            self.assertEqual(server.persisted_status()["google"]["weekly"]["remaining"], 75)


if __name__ == "__main__":
    unittest.main()
