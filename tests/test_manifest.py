import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from providers.base import CacheStore
from providers.example import ExampleProvider
from providers.manifest import (
    PROVIDER_ID_PATTERN,
    SCHEMA_VERSION,
    error_manifest,
    render_manifest,
)
from providers.registry import build_provider_registry


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


class ManifestSchemaTests(unittest.TestCase):
    def test_codex_manifest_schema_v1(self):
        provider = FakeProvider("codex", {
            "weekly": {"remaining": 92, "reset": "7月17日"},
            "five_hour": {"remaining": 65, "reset": "14:27"},
            "available": True,
            "stale": False,
            "state": "Connected",
        })
        manifest = render_manifest("codex", provider, {"last_success": "2026-07-31 19:00:00"})
        self.assertEqual(manifest["id"], "codex")
        self.assertEqual(manifest["display_name"], "Codex")
        self.assertEqual(manifest["state"], "connected")
        self.assertTrue(manifest["available"])
        self.assertEqual(manifest["sort_order"], 10)
        self.assertEqual(manifest["capabilities"], ["refresh"])
        primaries = [metric for metric in manifest["metrics"] if metric["primary"]]
        self.assertEqual(
            [metric["key"] for metric in primaries],
            ["five_hour_remaining", "weekly_remaining"],
        )
        weekly = next(metric for metric in manifest["metrics"] if metric["key"] == "weekly_remaining")
        self.assertEqual(weekly["value"], 92)
        self.assertEqual(weekly["value_type"], "percentage")
        self.assertEqual(weekly["format"], "percent")
        self.assertEqual(weekly["unit"], "%")
        for metric in manifest["metrics"]:
            self.assertEqual(
                set(metric),
                {"key", "label", "value", "value_type", "format", "unit", "primary"},
            )

    def test_workbuddy_manifest_never_leaks_diagnostics(self):
        status = {
            "points": 5343.37,
            "auto_used_credits": 126,
            "total_points": 10000,
            "balance_state": "Connected",
            "balance_stale": False,
            "balance_updated_at": "2026-07-31 19:19:50",
            "balance_error_code": "secret-code",
            "balance_error": "secret message",
            "balance_diagnostic": {"__apiProbe": {"page": "file:///secret"}},
            "usage_source": "daemon-rpc",
        }
        manifest = render_manifest("workbuddy", FakeProvider("workbuddy", status), {})
        serialized = json.dumps(manifest, ensure_ascii=False)
        self.assertNotIn("secret-code", serialized)
        self.assertNotIn("secret message", serialized)
        self.assertNotIn("__apiProbe", serialized)
        self.assertNotIn("balance_diagnostic", serialized)
        self.assertNotIn("balance_error_code", serialized)
        self.assertNotIn("usage_source", serialized)
        points = next(metric for metric in manifest["metrics"] if metric["key"] == "points")
        self.assertEqual(points["value"], 5343.37)
        self.assertTrue(points["primary"])
        self.assertEqual(manifest["state"], "connected")
        self.assertTrue(manifest["available"])
        self.assertEqual(manifest["updated_at"], "2026-07-31 19:19:50")
        self.assertEqual(
            [action["kind"] for action in manifest["actions"]],
            ["refresh", "reconnect", "diagnostics"],
        )
        for action in manifest["actions"]:
            self.assertEqual(set(action), {"id", "label", "kind", "local_only"})

    def test_workbuddy_cached_manifest_keeps_points_and_age(self):
        status = {
            "points": 5343.37,
            "balance_state": "Cached",
            "balance_stale": True,
            "balance_age_seconds": 400,
            "balance_updated_at": "2026-07-31 19:00:00",
        }
        manifest = render_manifest("workbuddy", FakeProvider("workbuddy", status), {})
        self.assertEqual(manifest["state"], "cached")
        self.assertTrue(manifest["stale"])
        self.assertTrue(manifest["available"])
        points = next(metric for metric in manifest["metrics"] if metric["key"] == "points")
        self.assertEqual(points["value"], 5343.37)
        ages = [metric for metric in manifest["metrics"] if metric["key"] == "cache_age"]
        self.assertEqual(ages[0]["value"], 400)
        self.assertEqual(ages[0]["value_type"], "duration")

    def test_workbuddy_unavailable_manifest_has_null_points(self):
        status = {
            "points": None,
            "balance_state": "Unavailable",
            "balance_error_code": "bridge_unavailable",
        }
        manifest = render_manifest("workbuddy", FakeProvider("workbuddy", status), {})
        self.assertEqual(manifest["state"], "error")
        self.assertFalse(manifest["available"])
        points = next(metric for metric in manifest["metrics"] if metric["key"] == "points")
        self.assertIsNone(points["value"])
        self.assertTrue(points["primary"])
        self.assertNotIn("balance_error", json.dumps(manifest))

    def test_deepseek_manifest_currency_and_usage(self):
        status = {
            "status": "Online",
            "balances": [{"currency": "CNY", "total_balance": "128.50", "granted_balance": "0", "topped_up_balance": "128.50"}],
            "usage": [{"currency": "CNY", "used_today": "2.34"}],
        }
        manifest = render_manifest("deepseek", FakeProvider("deepseek", status), {})
        self.assertEqual(manifest["state"], "connected")
        balance = next(metric for metric in manifest["metrics"] if metric["key"] == "balance")
        self.assertEqual(balance["value"], 128.5)
        self.assertEqual(balance["value_type"], "currency")
        self.assertEqual(balance["format"], "currency")
        self.assertEqual(balance["unit"], "CNY")
        used = next(metric for metric in manifest["metrics"] if metric["key"] == "used_today")
        self.assertEqual(used["value"], 2.34)

    def test_system_manifest(self):
        manifest = render_manifest(
            "system",
            FakeProvider("system", {"status": "Online", "label": "mac"}),
            {},
        )
        self.assertEqual(manifest["state"], "connected")
        self.assertEqual(manifest["capabilities"], [])
        self.assertEqual(manifest["actions"], [])
        self.assertEqual(manifest["metrics"][0]["key"], "status")

    def test_generic_fallback_for_unknown_provider(self):
        manifest = render_manifest(
            "mystery",
            FakeProvider("mystery", {"points": "99", "state": "Online", "unit": "pt"}),
            {},
        )
        self.assertEqual(manifest["category"], "generic")
        self.assertEqual(manifest["state"], "connected")
        self.assertEqual(manifest["metrics"][0]["value"], 99)
        self.assertEqual(manifest["metrics"][0]["unit"], "pt")

    def test_unknown_metric_types_safely_downgrade(self):
        provider = FakeProvider("codex", {"weekly": {"remaining": 92}, "available": True})
        provider.manifest = lambda metadata: {
            "display_name": "Evil",
            "state": "connected",
            "metrics": [
                {"key": "a", "label": "A", "value": 1, "value_type": "html", "format": "javascript", "primary": True},
                {"key": "b", "label": "B", "value": "5 > 3", "value_type": "text", "format": "plain"},
            ],
            "actions": [
                {"id": "evil", "label": "Run", "kind": "shell", "shell_command": "rm -rf /", "endpoint": "http://x"},
            ],
        }
        manifest = render_manifest("codex", provider, {})
        first = manifest["metrics"][0]
        self.assertEqual(first["value_type"], "text")
        self.assertEqual(first["format"], "plain")
        self.assertEqual(manifest["actions"], [])
        serialized = json.dumps(manifest)
        self.assertNotIn("html", serialized)
        self.assertNotIn("shell_command", serialized)
        self.assertNotIn("endpoint", serialized)

    def test_manifest_caps_metrics_and_strips_executable_fields(self):
        provider = FakeProvider("codex", {"weekly": {"remaining": 92}, "available": True})
        provider.manifest = lambda metadata: {
            "display_name": "X",
            "state": "connected",
            "metrics": [
                {"key": f"m{i}", "label": f"M{i}", "value": i, "value_type": "number", "format": "integer"}
                for i in range(20)
            ],
            "actions": [
                {"id": f"a{i}", "label": f"A{i}", "kind": "refresh"} for i in range(12)
            ],
        }
        manifest = render_manifest("codex", provider, {})
        self.assertLessEqual(len(manifest["metrics"]), 12)
        self.assertLessEqual(len(manifest["actions"]), 8)
        self.assertEqual({action["kind"] for action in manifest["actions"]}, {"refresh"})

    def test_provider_id_validation(self):
        for valid in ("codex", "a", "a-b_c", "x" * 64):
            self.assertRegex(valid, PROVIDER_ID_PATTERN)
        for invalid in ("Codex", "-bad", "_bad", "bad!", "a" * 65, "a/b", "", "a b"):
            self.assertIsNone(PROVIDER_ID_PATTERN.fullmatch(invalid))

    def test_error_manifest_is_schema_valid(self):
        manifest = error_manifest("codex", RuntimeError("boom"))
        self.assertEqual(manifest["state"], "error")
        self.assertFalse(manifest["available"])
        self.assertEqual(manifest["metrics"], [])
        self.assertEqual(set(manifest), {
            "id", "display_name", "category", "icon", "state", "available", "stale",
            "updated_at", "sort_order", "capabilities", "metrics", "actions",
        })

    def test_example_provider_uses_plug_in_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            provider = ExampleProvider(CacheStore(Path(directory)))
            manifest = render_manifest("example", provider, {"last_success": "2026-07-31 19:20:00"})
        self.assertEqual(manifest["display_name"], "Example Provider")
        self.assertEqual(manifest["category"], "credits")
        self.assertEqual(manifest["icon"], "sparkles")
        self.assertEqual(manifest["sort_order"], 200)
        points = next(metric for metric in manifest["metrics"] if metric["key"] == "points")
        self.assertEqual(points["value"], 12_345.67)
        self.assertTrue(points["primary"])
        self.assertEqual(manifest["updated_at"], "2026-07-31 19:20:00")

    def test_example_provider_is_dev_mode_only(self):
        with tempfile.TemporaryDirectory() as directory:
            registry = build_provider_registry(Path(directory), {}, lambda: {})
            self.assertNotIn("example", registry.as_dict())
            with patch.dict(os.environ, {"AICC_DEV_PROVIDERS": "1"}):
                dev_registry = build_provider_registry(Path(directory), {}, lambda: {})
            self.assertIn("example", dev_registry.as_dict())

    def test_schema_version_constant(self):
        self.assertEqual(SCHEMA_VERSION, 1)


if __name__ == "__main__":
    unittest.main()
