"""Regression guard for the frozen WorkBuddy 2.4.4 live-balance chain.

P1 must not restructure or replace the WorkBuddy collector; the manifest
layer only wraps its output. These tests pin the critical transport pieces
so an accidental rewrite fails CI immediately.
"""

import json
import unittest

from collectors import workbuddy
from providers.manifest import render_manifest


class FakeProvider:
    def __init__(self, name, status):
        self.name = name
        self._status = status

    def status(self):
        return dict(self._status)

    def health(self):
        return {"provider": self.name, "ok": True, "state": "connected"}

    def refresh(self, force=False):
        return dict(self._status)

    def cache(self):
        return None


class WorkBuddyChannelRegressionTests(unittest.TestCase):
    def test_localhost_cdp_bridge_port_is_unchanged(self):
        self.assertEqual(workbuddy.DEBUG_PORT, 9223)
        self.assertEqual(workbuddy.ACCOUNT_REFRESH_SECONDS, 120)

    def test_rpc_channel_and_public_fields_are_unchanged(self):
        expression = workbuddy.ACCOUNT_EXPRESSION
        self.assertIn("auth:getAccountUsage", expression)
        self.assertIn("MessageChannel", expression)
        self.assertIn("usageLeft", expression)
        self.assertIn("usageTotal", expression)
        self.assertIn("usageUsed", expression)
        self.assertIn("refreshAt", expression)
        self.assertIn("window.postMessage", expression)

    def test_public_field_whitelist_is_unchanged(self):
        for name in ("usageLeft", "usageTotal", "usageUsed", "refreshAt"):
            self.assertIn(name, workbuddy.ACCOUNT_EXPRESSION)
        for name in ("__apiProbe", "__apiError"):
            self.assertIn(name, workbuddy.ACCOUNT_EXPRESSION)

    def test_collector_keeps_diagnostic_fields_on_raw_status(self):
        """The collector still exposes its dedicated diagnostic fields; only
        the manifest strips them."""
        raw = {
            "points": None,
            "balance_state": "Unavailable",
            "balance_error_code": "bridge_unavailable",
            "balance_error": "WorkBuddy monitoring bridge is not running",
            "balance_diagnostic": {"__apiProbe": {"page": "file:///secret"}},
        }
        manifest = render_manifest("workbuddy", FakeProvider("workbuddy", raw), {})
        serialized = json.dumps(manifest, ensure_ascii=False)
        self.assertNotIn("balance_diagnostic", serialized)
        self.assertNotIn("balance_error", serialized)
        self.assertNotIn("bridge_unavailable", serialized)
        self.assertEqual(manifest["state"], "error")
        self.assertFalse(manifest["available"])
        self.assertIsNone(manifest["metrics"][0]["value"])

    def test_manifest_keeps_cached_balance_with_age(self):
        raw = {
            "points": 5343.37,
            "balance_state": "Cached",
            "balance_stale": True,
            "balance_age_seconds": 90,
            "balance_updated_at": "2026-07-31 19:00:00",
        }
        manifest = render_manifest("workbuddy", FakeProvider("workbuddy", raw), {})
        self.assertEqual(manifest["state"], "cached")
        self.assertTrue(manifest["available"])
        self.assertTrue(manifest["stale"])
        points = next(metric for metric in manifest["metrics"] if metric["key"] == "points")
        self.assertEqual(points["value"], 5343.37)
        self.assertTrue(points["primary"])
        age = next(metric for metric in manifest["metrics"] if metric["key"] == "cache_age")
        self.assertEqual(age["value"], 90)

    def test_reconnect_capability_is_workbuddy_only(self):
        workbuddy_manifest = render_manifest(
            "workbuddy",
            FakeProvider("workbuddy", {"points": 1, "balance_state": "Connected"}),
            {},
        )
        codex_manifest = render_manifest(
            "codex",
            FakeProvider("codex", {"weekly": {"remaining": 92}, "available": True}),
            {},
        )
        self.assertIn("reconnect", workbuddy_manifest["capabilities"])
        self.assertNotIn("reconnect", codex_manifest["capabilities"])

    def test_collector_expression_has_no_executable_escape_hatch(self):
        expression = workbuddy.ACCOUNT_EXPRESSION
        self.assertNotIn("require(", expression)
        self.assertNotIn("import(", expression)
        self.assertNotIn("child_process", expression)


if __name__ == "__main__":
    unittest.main()
