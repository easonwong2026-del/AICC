import json
import os
import subprocess
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from collectors import codex
from services.collector_manager import CollectorManager
import server


FIXTURE = json.loads((Path(__file__).parent / "fixtures/opencodex_pool.json").read_text())
CURRENT = {"activeId": "acct-2", "account": {"id": "acct-2"}}


class CodexPoolTests(unittest.TestCase):
    def test_source_policy_never_falls_back_after_ocx_failure(self):
        previous = server._collector_manager
        try:
            with patch.object(server, "persisted_status", return_value={}), \
                    patch.object(codex, "load_cache", return_value=None), \
                    patch.object(codex, "resolve_executable", return_value="/tmp/ocx"), \
                    patch.object(codex, "collect", side_effect=ValueError("Temporarily unavailable")) as ocx, \
                    patch.object(server.monitor, "status", return_value={"source": "Codex app-server"}) as legacy, \
                    patch.dict(os.environ, {"AICC_CODEX_SOURCE": "auto"}):
                server._collector_manager = None
                manager = server.collector_manager()
                self.assertEqual(manager._slots["codex"].value["source"], "OpenCodex")
                with self.assertRaisesRegex(ValueError, "unavailable"):
                    manager._slots["codex"].collect(force=True)
                ocx.assert_called_once_with(force=True)
                legacy.assert_not_called()
            with patch.object(server, "persisted_status", return_value={}), \
                    patch.object(codex, "resolve_executable", return_value=None), \
                    patch.object(server.monitor, "status", return_value={"source": "Codex app-server"}) as legacy, \
                    patch.dict(os.environ, {"AICC_CODEX_SOURCE": "auto"}):
                server._collector_manager = None
                manager = server.collector_manager()
                self.assertEqual(manager._slots["codex"].collect()["source"], "Codex app-server")
                legacy.assert_called_once_with(force=False)
        finally:
            server._collector_manager = previous
        with patch.object(codex, "resolve_executable", return_value=None):
            with self.assertRaisesRegex(ValueError, "OpenCodex not installed"):
                codex.collect()

    def test_real_shape_one_two_and_n_accounts_keep_quotas_separate(self):
        for count in (1, 2, 7):
            document = {"accounts": [{**FIXTURE["accounts"][i % 2], "id": f"acct-{i}"} for i in range(count)]}
            result = codex.parse_pool(document, {"activeId": "acct-0"})
            self.assertEqual(len(result["accounts"]), count)
            self.assertEqual(result["weekly"], result["accounts"][0]["weekly"])
            self.assertEqual(result["accounts"][0]["weekly"]["remaining"], 95)
            if count > 1:
                self.assertEqual(result["accounts"][1]["weekly"]["remaining"], 59)
                self.assertEqual(result["accounts"][1]["five_hour"]["remaining"], 100)

    def test_active_account_and_auto_mode(self):
        result = codex.parse_pool(FIXTURE, CURRENT)
        self.assertEqual(result["selection_mode"], "pinned")
        self.assertEqual(result["active_account_id"], "acct-2")
        self.assertEqual(result["weekly"]["remaining"], 59)
        auto = codex.parse_pool(FIXTURE, {"activeId": None})
        self.assertEqual(auto["selection_mode"], "auto")
        self.assertIsNone(auto["weekly"])
        self.assertFalse(any(row["active"] for row in auto["accounts"]))

    def test_unknown_reauth_invalid_values_and_empty_pool(self):
        row = {**FIXTURE["accounts"][0], "quota": None, "needsReauth": True, "email": "plain@example.com"}
        result = codex.parse_pool({"accounts": [row]}, {"activeId": "acct-1"})
        self.assertIsNone(result["weekly"]["remaining"])
        self.assertTrue(result["accounts"][0]["needs_reauth"])
        self.assertIsNone(result["accounts"][0]["email"])
        row["quota"] = {"fiveHourPercent": 22, "fiveHourResetAt": "2026-09-30T12:34:00Z"}
        aliased = codex.parse_pool({"accounts": [row]}, {"activeId": "acct-1"})
        self.assertEqual(aliased["five_hour"]["remaining"], 78)
        self.assertIsNotNone(aliased["five_hour"]["reset"])
        for value in (float("nan"), float("inf"), None, "bad"):
            row["quota"] = {"weeklyPercent": value, "shortPercent": 150, "shortResetAt": "bad"}
            parsed = codex.parse_pool({"accounts": [row]}, {"activeId": "acct-1"})
            self.assertIsNone(parsed["weekly"]["remaining"])
            self.assertEqual(parsed["five_hour"]["remaining"], 0)
            self.assertIsNone(parsed["five_hour"]["reset"])
        self.assertEqual(codex.parse_pool({"accounts": []}, {"activeId": None})["accounts"], [])
        with self.assertRaises(ValueError):
            codex.parse_pool({}, CURRENT)

    def test_commands_cache_and_failure_recovery(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(codex, "CACHE_PATH", Path(directory) / "codex.json"), \
                patch.object(codex, "resolve_executable", return_value="/tmp/ocx"), \
                patch.object(codex.subprocess, "run") as run:
            run.side_effect = lambda args, **kwargs: subprocess.CompletedProcess(
                args, 0, json.dumps(CURRENT if "current" in args else FIXTURE), "")
            normal = codex.collect()
            self.assertEqual(run.call_args_list[0].args[0], ["/tmp/ocx", "account", "list", "openai", "--quota", "--json"])
            fresh = codex.collect(force=True)
            self.assertEqual(run.call_args_list[2].args[0], ["/tmp/ocx", "account", "refresh", "openai", "--json"])
            self.assertEqual(fresh["accounts"], normal["accounts"])
            self.assertEqual(codex.load_cache(codex.CACHE_PATH)["accounts"][0]["stale"], True)
            run.side_effect = subprocess.TimeoutExpired("ocx", 30)
            with self.assertRaisesRegex(ValueError, "unavailable"):
                codex.collect(force=True)
            self.assertEqual(len(codex.load_cache(codex.CACHE_PATH)["accounts"]), 2)
            run.side_effect = lambda args, **kwargs: subprocess.CompletedProcess(
                args, 0, json.dumps(CURRENT if "current" in args else {"accounts": FIXTURE["accounts"][:1]}), "")
            codex.collect()
            self.assertEqual(len(codex.load_cache(codex.CACHE_PATH)["accounts"]), 1)
            run.reset_mock()
            run.side_effect = lambda args, **kwargs: subprocess.CompletedProcess(args, 0, '{"accounts":[]}', "")
            self.assertEqual(codex.collect()["accounts"], [])
            self.assertEqual(run.call_count, 1)
            codex.CACHE_PATH.write_text('{"weekly":{"remaining":50}}')
            self.assertIsNone(codex.load_cache(codex.CACHE_PATH))

    def test_concurrent_force_calls_run_one_pool_refresh(self):
        entered = threading.Event()
        release = threading.Event()
        commands = []

        def command(_executable, args, _env):
            commands.append(args)
            if args[0] == "refresh":
                entered.set()
                release.wait(2)
            return CURRENT if args[0] == "current" else FIXTURE
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(codex, "CACHE_PATH", Path(directory) / "codex.json"), \
                patch.object(codex, "resolve_executable", return_value="/tmp/ocx"), \
                patch.object(codex, "_command", side_effect=command):
            manager = CollectorManager({"codex": (codex.collect, 60, 3, {"source": "OpenCodex"})})
            first = threading.Thread(target=lambda: manager.snapshot(force=True, wait_seconds=2))
            second = threading.Thread(target=lambda: manager.snapshot(force=True, wait_seconds=2))
            first.start()
            self.assertTrue(entered.wait(1))
            second.start()
            time.sleep(0.05)
            release.set()
            first.join(3)
            second.join(3)
            self.assertFalse(first.is_alive() or second.is_alive())
            self.assertEqual(commands.count(["refresh", "openai"]), 1)

    def test_manager_failure_is_stale_and_next_success_clears_it(self):
        calls = 0

        def collect(force=False):
            nonlocal calls
            calls += 1
            if calls == 2:
                raise ValueError("Temporarily unavailable")
            return codex.parse_pool(FIXTURE, CURRENT)
        manager = CollectorManager({"codex": (collect, 60, 1, {"source": "OpenCodex", "accounts": []})})
        self.assertFalse(manager.snapshot(force=True, wait_seconds=2)[0]["codex"]["stale"])
        failed = manager.snapshot(force=True, wait_seconds=2)[0]["codex"]
        self.assertTrue(failed["stale"])
        self.assertEqual(failed["state"], "Temporarily unavailable")
        self.assertTrue(all(row["stale"] for row in failed["accounts"]))
        self.assertFalse(manager.snapshot(force=True, wait_seconds=2)[0]["codex"]["stale"])


if __name__ == "__main__":
    unittest.main()
