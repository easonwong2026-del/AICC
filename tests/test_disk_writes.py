import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from collectors import deepseek, workbuddy
from services.codex_monitor import CodexMonitor


class DiskWriteTests(unittest.TestCase):
    def test_deepseek_does_not_append_unchanged_balance(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "history.json"
            balance = [{"currency": "CNY", "total_balance": "10.00"}]
            deepseek.update_usage(path, balance)
            deepseek.update_usage(path, balance)
            snapshots = json.loads(path.read_text(encoding="utf-8"))["snapshots"]
        self.assertEqual(len(snapshots), 1)

    def test_workbuddy_unchanged_recent_cache_is_not_rewritten(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "workbuddy.json"
            original = {"points": 10, "reset_text": "Auto", "balance_updated_epoch": 100}
            path.write_text(json.dumps(original), encoding="utf-8")
            value = {"points": 10, "reset_text": "Auto", "balance_updated_epoch": 110}
            with patch.object(workbuddy, "ACCOUNT_CACHE_PATH", path), patch.object(workbuddy.time, "time", return_value=120):
                workbuddy._save_account_cache(value)
            saved = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(saved["balance_updated_epoch"], 100)

    def test_codex_unchanged_recent_cache_is_not_rewritten(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "codex.json"
            path.write_text(json.dumps({"weekly": {"remaining": 90}, "updated_epoch": 100}), encoding="utf-8")
            monitor._cache_path = path
            with patch("services.codex_monitor.time.time", return_value=120):
                monitor._save_cache({"weekly": {"remaining": 90}, "updated_epoch": 120})
            saved = json.loads(path.read_text(encoding="utf-8"))
        self.assertEqual(saved["updated_epoch"], 100)


if __name__ == "__main__":
    unittest.main()
