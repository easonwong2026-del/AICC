import copy
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from collectors import magpie
from services.collector_manager import CollectorManager
import server

FIXTURE = json.loads((Path(__file__).parent / "fixtures/magpie_quotas.json").read_text())
NOW = magpie.timestamp("2026-10-14T13:32:00+08:00")


class MagpieTests(unittest.TestCase):
    def test_real_report_keeps_accounts_separate_and_masks_identity(self):
        result = magpie.parse_report(FIXTURE, NOW)
        self.assertEqual(len(result["accounts"]), 2)
        self.assertEqual(result["accounts"][0]["weekly"]["remaining"], 88)
        self.assertEqual(result["accounts"][1]["weekly"]["remaining"], 59)
        self.assertIsNone(result["weekly"])
        self.assertIsNone(result["active_account_id"])
        self.assertFalse(any(account["active"] for account in result["accounts"]))
        self.assertNotIn("@", json.dumps(result))
        self.assertFalse(result["stale"])
        reverse = copy.deepcopy(FIXTURE)
        reverse["data"].reverse()
        self.assertEqual(result["accounts"][0]["id"], magpie.parse_report(reverse, NOW)["accounts"][1]["id"])

    def test_unknown_invalid_and_old_windows_are_never_fresh_zeroes(self):
        for value in (None, True, "bad", float("nan"), float("inf")):
            report = copy.deepcopy(FIXTURE)
            report["data"][0]["windows"][0]["used"] = value
            self.assertIsNone(magpie.parse_report(report, NOW)["accounts"][0]["five_hour"]["remaining"])
        report = copy.deepcopy(FIXTURE)
        report["data"][0]["error"] = "credential-secret"
        result = magpie.parse_report(report, NOW)
        self.assertTrue(result["accounts"][0]["stale"])
        self.assertNotIn("credential-secret", json.dumps(result))
        self.assertTrue(magpie.parse_report(FIXTURE, NOW + 600)["stale"])
        report["data"][0].pop("readAt")
        self.assertTrue(magpie.parse_report(report, NOW)["stale"])

    def test_single_empty_and_bad_shape(self):
        single = {"object": "list", "data": FIXTURE["data"][:1]}
        self.assertEqual(magpie.parse_report(single, NOW)["weekly"]["remaining"], 88)
        self.assertFalse(magpie.parse_report({"object": "list", "data": []}, NOW)["available"])
        for bad in ({}, [], {"object": "list", "data": None}):
            with self.assertRaises(ValueError):
                magpie.parse_report(bad, NOW)
        duplicate = copy.deepcopy(FIXTURE)
        duplicate["data"].append(duplicate["data"][0])
        with self.assertRaisesRegex(ValueError, "Duplicate"):
            magpie.parse_report(duplicate, NOW)

    def test_force_refresh_uses_documented_path_and_cache_is_separate(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(magpie, "CACHE_PATH", Path(directory) / "magpie.json"), \
                patch.object(magpie, "_request", return_value=FIXTURE) as request:
            magpie.collect(force=True)
            self.assertEqual(request.call_args_list[0].args, ("/v1/magpie/quotas/refresh?provider=codex", "POST"))
            self.assertEqual(request.call_args_list[1].args, ("/v1/magpie/quotas?kept=1",))
            self.assertTrue(magpie.load_cache()["stale"])
            request.side_effect = ValueError("Magpie temporarily unavailable")
            with self.assertRaises(ValueError):
                magpie.collect()
            self.assertEqual(len(magpie.load_cache()["accounts"]), 2)

    def test_remote_credentials_and_non_loopback_urls_are_rejected_before_network(self):
        for url in ("https://example.com:3425", "http://127.0.0.1:3425@evil.invalid", "http://user:key@127.0.0.1:3425",
                    "http://localhost:3425", "http://127.0.0.1:bad", "http://127.0.0.1:3425/private"):
            with patch.dict(os.environ, {"AICC_MAGPIE_URL": url}), patch.object(magpie, "build_opener") as opener:
                with self.assertRaisesRegex(ValueError, "local HTTP"):
                    magpie._request("/v1/magpie/quotas")
                opener.assert_not_called()
        self.assertIsNone(magpie.NoRedirect().redirect_request(None, None, 302, "", {}, "https://example.com"))

    def test_auto_prefers_magpie_and_failure_does_not_switch_accounts(self):
        previous = server._collector_manager
        try:
            with patch.dict(os.environ, {"AICC_CODEX_SOURCE": "auto"}), patch.object(magpie, "installed", return_value=True), \
                    patch.object(magpie, "load_cache", return_value=None), patch.object(server.monitor, "status") as legacy, \
                    patch.object(magpie, "collect", side_effect=ValueError("Magpie temporarily unavailable")):
                server._collector_manager = None
                slot = server.collector_manager()._slots["codex"]
                self.assertEqual(slot.value["source"], "Magpie")
                with self.assertRaises(ValueError):
                    slot.collect()
                legacy.assert_not_called()
        finally:
            server._collector_manager = previous
        manager = CollectorManager({"codex": (lambda force=False: (_ for _ in ()).throw(ValueError("Unavailable")),
                                              120, 1, magpie.parse_report(FIXTURE, NOW))})
        failed = manager.snapshot(force=True, wait_seconds=2)[0]["codex"]
        self.assertTrue(failed["stale"])
        self.assertTrue(all(account["stale"] for account in failed["accounts"]))
