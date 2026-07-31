import json
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request, urlopen

import server
from providers.base import CacheStore
from providers.example import ExampleProvider
from providers.registry import ProviderRegistry


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


class FakeManager:
    def __init__(self):
        self.invalidated = []
        self.snapshots = []
        self.refreshed = []

    def snapshot(self, **kwargs):
        self.snapshots.append(kwargs)
        values = {
            "codex": {"weekly": {"remaining": 92}, "available": True, "state": "Connected"},
            "deepseek": {"status": "Online", "balances": [{"currency": "CNY", "total_balance": "128.50"}], "usage": []},
            "workbuddy": {"points": 12, "balance_state": "Connected", "balance_stale": False},
            "system": {"status": "Online"},
        }
        metadata = {
            name: {"state": "ready", "last_success": "2026-07-31 19:20:00", "age_seconds": 5}
            for name in values
        }
        return values, metadata

    def refresh_one(self, name, **kwargs):
        self.refreshed.append(name)
        values, metadata = self.snapshot(**kwargs)
        return values.get(name, {}), metadata.get(name, {})

    def invalidate(self, *names):
        self.invalidated.extend(names)


class ProviderApiTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.original_data_path = server.DATA_PATH
        self.original_manager = server._collector_manager
        self.original_registry = server._provider_registry
        server.DATA_PATH = Path(self.temporary.name) / "status.json"
        server._collector_manager = FakeManager()
        providers = {
            "codex": FakeProvider("codex", {"weekly": {"remaining": 92}, "available": True}),
            "deepseek": FakeProvider("deepseek", {"status": "Online", "balances": []}),
            "workbuddy": FakeProvider("workbuddy", {"points": 12, "balance_state": "Connected"}),
            "system": FakeProvider("system", {"status": "Online"}),
        }
        server._provider_registry = ProviderRegistry(providers)
        server._rate_windows.clear()
        self.httpd = server.DashboardServer(("127.0.0.1", 0), server.DashboardHandler)
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()
        self.base = f"http://127.0.0.1:{self.httpd.server_port}"

    def tearDown(self):
        self.httpd.shutdown()
        self.httpd.server_close()
        self.thread.join(timeout=2)
        server.DATA_PATH = self.original_data_path
        server._collector_manager = self.original_manager
        server._provider_registry = self.original_registry
        self.temporary.cleanup()

    def test_providers_list_returns_sorted_schema_v1(self):
        with urlopen(self.base + "/api/providers", timeout=2) as response:
            payload = json.load(response)
        self.assertEqual(payload["schema_version"], 1)
        self.assertIn("updated_at", payload)
        ids = [item["id"] for item in payload["providers"]]
        self.assertEqual(ids, ["codex", "workbuddy", "deepseek", "system"])
        for item in payload["providers"]:
            self.assertEqual(
                set(item),
                {"id", "display_name", "category", "icon", "state", "available", "stale",
                 "updated_at", "sort_order", "capabilities", "metrics", "actions"},
            )
        workbuddy = next(item for item in payload["providers"] if item["id"] == "workbuddy")
        self.assertEqual(workbuddy["display_name"], "WorkBuddy")
        self.assertEqual(workbuddy["sort_order"], 20)
        self.assertTrue(workbuddy["metrics"][0]["primary"])
        self.assertEqual(
            workbuddy["actions"],
            [
                {"id": "refresh", "label": "刷新 WorkBuddy", "kind": "refresh", "local_only": True},
                {"id": "reconnect", "label": "重连 WorkBuddy", "kind": "reconnect", "local_only": True},
                {"id": "diagnostics", "label": "WorkBuddy 诊断", "kind": "diagnostics", "local_only": True},
            ],
        )

    def test_single_provider_endpoint(self):
        with urlopen(self.base + "/api/providers/deepseek", timeout=2) as response:
            payload = json.load(response)
        self.assertEqual(payload["id"], "deepseek")
        self.assertEqual(payload["category"], "credits")
        self.assertEqual(payload["state"], "connected")

    def test_unknown_provider_returns_404(self):
        for path in ("/api/providers/nope", "/api/providers/Bad_ID", "/api/providers/a/b"):
            with self.assertRaises(HTTPError) as caught:
                urlopen(self.base + path, timeout=2)
            self.assertEqual(caught.exception.code, 404)

    def test_provider_refresh_endpoint(self):
        request = Request(self.base + "/api/providers/codex/refresh", method="POST")
        with urlopen(request, timeout=2) as response:
            payload = json.load(response)
        self.assertEqual(payload["id"], "codex")
        self.assertEqual(server._collector_manager.refreshed, ["codex"])
        with self.assertRaises(HTTPError) as caught:
            urlopen(Request(self.base + "/api/providers/nope/refresh", method="POST"), timeout=2)
        self.assertEqual(caught.exception.code, 404)

    def test_action_whitelist_rejects_unknown_kinds(self):
        request = Request(self.base + "/api/providers/workbuddy/actions/evil", method="POST")
        with self.assertRaises(HTTPError) as caught:
            urlopen(request, timeout=2)
        self.assertEqual(caught.exception.code, 400)
        payload = json.loads(caught.exception.read().decode("utf-8"))
        self.assertIn("not available", payload["error"])

    def test_reconnect_action_only_for_workbuddy(self):
        request = Request(self.base + "/api/providers/codex/actions/reconnect", method="POST")
        with self.assertRaises(HTTPError) as caught:
            urlopen(request, timeout=2)
        self.assertEqual(caught.exception.code, 400)

    def test_reconnect_action_runs_bundled_script(self):
        request = Request(self.base + "/api/providers/workbuddy/actions/reconnect", method="POST")
        with patch("server.subprocess.run") as run:
            run.return_value = type(
                "Completed",
                (),
                {"returncode": 0, "stdout": "AICC_WORKBUDDY_READY\n", "stderr": ""},
            )()
            with urlopen(request, timeout=2) as response:
                payload = json.load(response)
        self.assertEqual(payload["id"], "workbuddy")
        self.assertIn("--ensure", run.call_args.args[0])
        self.assertIn("workbuddy", server._collector_manager.invalidated)

    def test_diagnostics_action_returns_whitelisted_fields_only(self):
        request = Request(self.base + "/api/providers/workbuddy/actions/diagnostics", method="POST")
        with urlopen(request, timeout=2) as response:
            payload = json.load(response)
        self.assertEqual(payload["provider"], "workbuddy")
        diagnostics = payload["diagnostics"]
        self.assertEqual(set(diagnostics), {"collection", "health"})
        self.assertEqual(
            set(diagnostics["collection"]),
            {"state", "last_success", "age_seconds"},
        )
        self.assertEqual(set(diagnostics["health"]), {"ok", "state"})
        self.assertNotIn("balance_diagnostic", json.dumps(payload))

    def test_refresh_action_uses_provider_refresh(self):
        request = Request(self.base + "/api/providers/deepseek/actions/refresh", method="POST")
        with urlopen(request, timeout=2) as response:
            payload = json.load(response)
        self.assertEqual(payload["id"], "deepseek")
        self.assertIn("deepseek", server._collector_manager.refreshed)

    def test_single_provider_failure_does_not_break_the_list(self):
        def boom_status():
            raise RuntimeError("collector exploded")

        broken = FakeProvider("broken", {})
        broken.status = boom_status
        server._provider_registry = ProviderRegistry({
            "codex": FakeProvider("codex", {"weekly": {"remaining": 92}, "available": True}),
            "broken": broken,
        })
        with urlopen(self.base + "/api/providers", timeout=2) as response:
            payload = json.load(response)
        ids = [item["id"] for item in payload["providers"]]
        self.assertEqual(ids, ["codex", "broken"])
        broken_item = next(item for item in payload["providers"] if item["id"] == "broken")
        self.assertEqual(broken_item["state"], "error")
        self.assertFalse(broken_item["available"])
        self.assertEqual(broken_item["metrics"], [])
        self.assertEqual(payload["providers"][0]["id"], "codex")

    def test_status_contract_is_unchanged(self):
        with urlopen(self.base + "/api/status", timeout=2) as response:
            payload = json.load(response)
        self.assertEqual(payload["codex"]["weekly"]["remaining"], 92)
        self.assertEqual(payload["workbuddy"]["points"], 12)
        self.assertEqual(payload["system"]["status"], "Online")
        self.assertIn("collection", payload)

    def test_write_operations_are_local_only(self):
        handler = server.DashboardHandler.__new__(server.DashboardHandler)
        handler.client_address = ("192.168.1.5", 4242)
        self.assertFalse(handler.is_local_request())
        handler.client_address = ("127.0.0.1", 4242)
        self.assertTrue(handler.is_local_request())
        handler.client_address = ("::1", 4242)
        self.assertTrue(handler.is_local_request())

    def test_dev_example_provider_appears_in_api_when_enabled(self):
        with patch.dict("os.environ", {"AICC_DEV_PROVIDERS": "1"}):
            cache = CacheStore(Path(self.temporary.name))
            providers = {
                "codex": FakeProvider("codex", {"weekly": {"remaining": 92}, "available": True}),
                "workbuddy": FakeProvider("workbuddy", {"points": 12, "balance_state": "Connected"}),
                "deepseek": FakeProvider("deepseek", {"status": "Online", "balances": []}),
                "system": FakeProvider("system", {"status": "Online"}),
                "example": ExampleProvider(cache),
            }
            server._provider_registry = ProviderRegistry(providers)
            with urlopen(self.base + "/api/providers", timeout=2) as response:
                payload = json.load(response)
        ids = [item["id"] for item in payload["providers"]]
        self.assertEqual(ids[-1], "example")
        example = next(item for item in payload["providers"] if item["id"] == "example")
        self.assertEqual(example["display_name"], "Example Provider")
        self.assertEqual(example["metrics"][0]["value"], 12_345.67)


if __name__ == "__main__":
    unittest.main()
