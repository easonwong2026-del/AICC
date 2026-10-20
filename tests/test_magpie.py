import copy
import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from collectors import magpie

FIXTURE = json.loads((Path(__file__).parent / "fixtures/magpie_quotas.json").read_text())
LIVE = json.loads((Path(__file__).parent / "fixtures/magpie_live_shape.json").read_text())
NOW = magpie.timestamp("2026-10-14T13:32:00+08:00")


def find(snapshot, provider, kind="subscription", index=0):
    rows = [card for card in snapshot["accounts"] if card["provider"] == provider and card["kind"] == kind]
    return rows[index]


class ReportTests(unittest.TestCase):
    def test_codex_subscription_keeps_every_window_and_its_reset(self):
        card = find(magpie.parse_report(FIXTURE, NOW), "codex")
        self.assertEqual([item["name"] for item in card["items"]], ["5 hours", "7 days"])
        self.assertEqual(card["items"][0]["remaining"], 91)
        self.assertEqual(card["items"][1]["remaining"], 88)
        self.assertEqual(card["plan"], "team")
        self.assertEqual((card["resets_count"], card["last"], card["renew"]), (2, True, None))
        self.assertFalse(card["stale"])

    def test_workbuddy_credits_keep_the_vendors_own_count(self):
        card = find(magpie.parse_report(FIXTURE, NOW), "workbuddy")
        item = card["items"][0]
        self.assertEqual(item["name"], "Credits")
        self.assertEqual(item["display"], "2.27 / 4723")
        self.assertEqual((item["amount"], item["limit"], item["unit"]), (2.27, 4723, "credits"))
        self.assertAlmostEqual(item["remaining"], 99.95, places=2)
        self.assertEqual(card["plan"], "Free")

    def test_plan_cards_accept_any_window_name(self):
        snapshot = magpie.parse_report(FIXTURE, NOW)
        card = find(snapshot, "zcode", "plan")
        self.assertEqual(card["items"][0]["name"], "prompts")
        self.assertEqual(card["items"][0]["display"], "35 / 40")
        self.assertEqual(card["renew"], "auto")
        self.assertEqual(card["until"], magpie.timestamp("2026-11-20T00:00:00Z"))

    def test_balance_card_is_never_turned_into_a_percentage(self):
        card = find(magpie.parse_report(FIXTURE, NOW), "deepseek", "balance")
        self.assertEqual(card["balance"], "12.34 USD")
        self.assertEqual([part["text"] for part in card["balance_parts"]], ["10.00 USD", "2.34 USD"])
        self.assertEqual(card["items"], [])
        self.assertIsNone(card["balance_parts"][0].get("percent"))

    def test_mixed_providers_keep_source_order_and_are_counted_once(self):
        snapshot = magpie.parse_report(FIXTURE, NOW)
        self.assertEqual(snapshot["providers"], ["codex", "workbuddy", "antigravity", "zcode", "deepseek", "brandnew-vendor"])
        self.assertEqual(len(snapshot["accounts"]), len(FIXTURE["data"]))
        self.assertEqual(snapshot["state"], "Connected")
        self.assertEqual(snapshot["schema_version"], 3)

    def test_unknown_provider_is_kept_and_named(self):
        card = find(magpie.parse_report(FIXTURE, NOW), "brandnew-vendor")
        self.assertEqual(card["provider_name"], "Brandnew Vendor")
        self.assertEqual(card["items"][0]["name"], "Monthly")
        self.assertEqual(magpie.provider_name("my_own-provider"), "My Own Provider")

    def test_missing_and_odd_values_are_unknown_not_zero(self):
        report = copy.deepcopy(FIXTURE)
        report["data"][0]["windows"] = [
            {"name": "5 hours", "used": None},
            {"name": "7 days", "used": "12"},
            {"name": "unlimited", "unlimited": True},
            {"used": 5},
            "not-a-window",
        ]
        card = find(magpie.parse_report(report, NOW), "codex")
        self.assertEqual([item["name"] for item in card["items"]], ["5 hours", "7 days", "unlimited"])
        self.assertIsNone(card["items"][0]["remaining"])
        self.assertIsNone(card["items"][1]["remaining"])
        self.assertTrue(card["items"][2]["unlimited"])
        self.assertIsNone(card["items"][2]["remaining"])
        self.assertIsNone(card["items"][0]["reset"])

    def test_empty_windows_keep_the_card(self):
        report = {"object": "list", "data": [{"provider": "workbuddy", "kind": "subscription",
                                             "user": "empty@example.invalid", "windows": []}]}
        snapshot = magpie.parse_report(report, NOW)
        self.assertEqual(len(snapshot["accounts"]), 1)
        self.assertEqual(snapshot["accounts"][0]["items"], [])

    def test_errors_deferred_and_old_reads_go_stale_without_leaking_text(self):
        snapshot = magpie.parse_report(FIXTURE, NOW)
        broken = find(snapshot, "codex", index=2)
        self.assertTrue(broken["failed"])
        self.assertTrue(broken["stale"])
        self.assertNotIn("invalid_grant", json.dumps(snapshot))
        self.assertTrue(find(snapshot, "workbuddy", index=2)["deferred"])
        self.assertFalse(snapshot["stale"])
        old = magpie.parse_report(FIXTURE, NOW + 600)
        self.assertTrue(old["stale"])
        self.assertEqual(old["state"], "Cached")
        cached = copy.deepcopy(FIXTURE)
        cached["data"][0]["asOf"] = "2026-10-14T13:00:00+08:00"
        self.assertTrue(find(magpie.parse_report(cached, NOW), "codex")["stale"])
        reset_passed = copy.deepcopy(FIXTURE)
        reset_passed["data"][0]["windows"][0]["resetsAt"] = "2026-10-14T13:00:00+08:00"
        self.assertTrue(find(magpie.parse_report(reset_passed, NOW), "codex")["stale"])

    def test_invalid_percentages_and_timestamps_become_unknown(self):
        for value in (True, "42", float("nan"), float("inf"), -1, 101, None):
            report = copy.deepcopy(FIXTURE)
            report["data"][0]["windows"][0] = {"name": "5 hours", "used": value, "remaining": value}
            item = find(magpie.parse_report(report, NOW), "codex")["items"][0]
            self.assertIsNone(item["remaining"], value)
            self.assertIsNone(item["used"], value)
        report = copy.deepcopy(FIXTURE)
        report["data"][0]["readAt"] = "not-a-time"
        report["data"][0]["windows"][0]["resetsAt"] = "2026-13-40T99:99:99Z"
        card = find(magpie.parse_report(report, NOW), "codex")
        self.assertIsNone(card["read_at"])
        self.assertIsNone(card["items"][0]["reset"])
        self.assertTrue(card["stale"])

    def test_duplicate_rows_keep_the_newer_reading(self):
        report = copy.deepcopy(FIXTURE)
        duplicate = copy.deepcopy(report["data"][0])
        duplicate["readAt"] = "2026-10-14T13:31:00+08:00"
        duplicate["windows"][1]["remaining"] = 42
        report["data"].append(duplicate)
        snapshot = magpie.parse_report(report, NOW)
        self.assertEqual(len(snapshot["accounts"]), len(FIXTURE["data"]))
        self.assertEqual(find(snapshot, "codex")["items"][1]["remaining"], 42)

    def test_broken_envelope_is_rejected(self):
        for bad in ({}, [], {"object": "list", "data": None}, {"object": "other", "data": []}):
            with self.assertRaises(ValueError):
                magpie.parse_report(bad, NOW)

    def test_bad_rows_do_not_take_other_providers_down(self):
        report = copy.deepcopy(FIXTURE)
        report["data"] = [None, {"provider": "", "kind": "subscription"}, {"provider": "x", "kind": "mystery"},
                          "row"] + report["data"]
        snapshot = magpie.parse_report(report, NOW)
        self.assertEqual(len(snapshot["accounts"]), len(FIXTURE["data"]))

    def test_identities_are_masked_and_stable(self):
        snapshot = magpie.parse_report(FIXTURE, NOW)
        encoded = json.dumps(snapshot)
        self.assertNotIn("@", encoded)
        self.assertNotIn("example.invalid", encoded)
        self.assertNotIn("one@example", encoded)
        for account in snapshot["accounts"]:
            self.assertTrue(account["id"].startswith("magpie-"))
            self.assertNotIn(" ", account["account_label"])
        reverse = copy.deepcopy(FIXTURE)
        reverse["data"].reverse()
        self.assertEqual({card["id"] for card in snapshot["accounts"]},
                         {card["id"] for card in magpie.parse_report(reverse, NOW)["accounts"]})

    def test_live_capture_shape_parses_with_dynamic_windows(self):
        snapshot = magpie.parse_report(LIVE, NOW)
        self.assertEqual(snapshot["providers"], ["codex", "workbuddy", "antigravity"])
        self.assertEqual(sum(1 for card in snapshot["accounts"] if card["provider"] == "workbuddy"), 3)
        antigravity = find(snapshot, "antigravity")
        self.assertEqual(len(antigravity["items"]), 7)
        names = [item["name"] for item in antigravity["items"]]
        self.assertIn("Gemini 3.8 Flash (High)", names)
        self.assertEqual(names.count("7 days"), 2)
        codex = find(snapshot, "codex")
        self.assertEqual(codex["resets_count"], 2)
        self.assertEqual(codex["until"], magpie.timestamp("2027-07-01T04:07:05Z"))
        self.assertNotIn("@", json.dumps(snapshot))


class TransportTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.cache = patch.object(magpie, "CACHE_PATH", Path(self.directory.name) / "magpie.json")
        self.cache.start()

    def tearDown(self):
        self.cache.stop()
        self.directory.cleanup()

    def test_forced_read_refreshes_every_provider_with_one_request(self):
        with patch.object(magpie, "_request", return_value=FIXTURE) as request:
            magpie.collect(force=True)
            request.assert_called_once_with("/v1/magpie/quotas/refresh", "POST")
        with patch.object(magpie, "_request", return_value=FIXTURE) as request:
            magpie.collect(force=True, provider="workbuddy")
            request.assert_called_once_with("/v1/magpie/quotas/refresh?provider=workbuddy", "POST")

    def test_passive_read_asks_for_nothing_upstream(self):
        with patch.object(magpie, "_request", return_value=FIXTURE) as request:
            magpie.collect()
            request.assert_called_once_with("/v1/magpie/quotas?kept=1")

    def test_refresh_failure_falls_back_to_the_kept_read(self):
        with patch.object(magpie, "_request", return_value=FIXTURE):
            magpie.collect()
        with patch.object(magpie, "_request", side_effect=[ValueError("boom"), FIXTURE]) as request:
            snapshot = magpie.collect(force=True)
        self.assertEqual(request.call_args_list[1].args, ("/v1/magpie/quotas?kept=1",))
        self.assertEqual(len(snapshot["accounts"]), len(FIXTURE["data"]))
        with patch.object(magpie, "_request", side_effect=ValueError("boom")):
            with self.assertRaises(ValueError):
                magpie.collect()
        self.assertTrue(magpie.load_cache()["stale"])
        self.assertEqual(len(magpie.load_cache()["accounts"]), len(FIXTURE["data"]))

    def test_cache_is_atomic_private_and_schema_gated(self):
        with patch.object(magpie, "_request", return_value=FIXTURE):
            magpie.collect()
        written = magpie.CACHE_PATH
        self.assertTrue(written.exists())
        self.assertEqual(written.stat().st_mode & 0o777, 0o600)
        self.assertEqual([path.name for path in written.parent.iterdir()], [written.name])
        self.assertEqual(magpie.load_cache()["state"], "Cached")
        written.write_text(json.dumps({"source": "Magpie", "schema_version": 2, "accounts": []}))
        self.assertIsNone(magpie.load_cache())
        written.write_text("{not json")
        self.assertIsNone(magpie.load_cache())

    def test_remote_credentials_and_non_loopback_urls_are_rejected_before_network(self):
        for url in ("https://example.com:3425", "http://127.0.0.1:3425@evil.invalid",
                    "http://user:key@127.0.0.1:3425", "http://localhost:3425",
                    "http://127.0.0.1:bad", "http://127.0.0.1:3425/private", "http://127.0.0.1:3425?x=1"):
            with patch.dict(os.environ, {"AICC_MAGPIE_URL": url}), patch.object(magpie, "build_opener") as opener:
                with self.assertRaisesRegex(ValueError, "local HTTP"):
                    magpie._request("/v1/magpie/quotas")
                opener.assert_not_called()
        self.assertIsNone(magpie.NoRedirect().redirect_request(None, None, 302, "", {}, "https://example.com"))

    def test_refresh_policy_lives_in_one_place(self):
        self.assertEqual(magpie.REFRESH_SECONDS, magpie.MAX_AGE_SECONDS)
        self.assertIsInstance(magpie.REFRESH_SECONDS, int)


class ConcurrencyTests(unittest.TestCase):
    def test_a_failing_provider_leaves_the_others_in_place(self):
        """Magpie reports a provider's failure as a row, not as a failed request."""
        report = copy.deepcopy(FIXTURE)
        for row in report["data"]:
            if row["provider"] == "workbuddy":
                row["error"] = "provider down"
        snapshot = magpie.parse_report(report, NOW)
        self.assertTrue(all(card["failed"] for card in snapshot["accounts"] if card["provider"] == "workbuddy"))
        healthy = [card for card in snapshot["accounts"] if card["provider"] == "codex" and card["items"]]
        self.assertTrue(healthy)
        self.assertFalse(any(card["failed"] for card in healthy))
        self.assertNotEqual(snapshot["state"], "Empty")

    def test_repeated_reads_are_independent(self):
        first = magpie.parse_report(FIXTURE, NOW)
        second = magpie.parse_report(FIXTURE, NOW + 1)
        self.assertEqual(first, second)


if __name__ == "__main__":
    unittest.main()
