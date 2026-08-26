import json
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.request import Request, urlopen

import server


class FakeManager:
    def __init__(self):
        self.invalidated = []
        self.snapshots = []

    def snapshot(self, **kwargs):
        self.snapshots.append(kwargs)
        values = {
            "codex": {"state": "Connected"},
            "deepseek": {"status": "Online", "balances": []},
            "workbuddy": {"points": 12},
            "system": {"status": "Online"},
        }
        metadata = {name: {"state": "ready", "last_success": "now"} for name in values}
        return values, metadata

    def invalidate(self, *names):
        self.invalidated.extend(names)


class ServerIntegrationTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.original_data_path = server.DATA_PATH
        self.original_manager = server._collector_manager
        server.DATA_PATH = Path(self.temporary.name) / "status.json"
        server._collector_manager = FakeManager()
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
        self.temporary.cleanup()

    def test_health_and_status_have_security_headers(self):
        with urlopen(self.base + "/api/health", timeout=2) as response:
            payload = json.load(response)
            self.assertTrue(payload["ok"])
            self.assertEqual(response.headers["X-Content-Type-Options"], "nosniff")
            self.assertEqual(response.headers["X-Frame-Options"], "DENY")
        with urlopen(self.base + "/api/status", timeout=2) as response:
            payload = json.load(response)
            self.assertEqual(payload["workbuddy"]["points"], 12)
            self.assertIn("collection", payload)

    def test_health_layers_are_non_refreshing_and_versioned(self):
        with urlopen(self.base + "/api/health/live", timeout=2) as response:
            live = json.load(response)
        self.assertEqual(live["status"], "live")
        self.assertEqual(live["version"], server.version())

        with urlopen(self.base + "/api/health/ready", timeout=2) as response:
            ready = json.load(response)
        self.assertIn(ready["status"], {"healthy", "degraded"})
        self.assertEqual(server._collector_manager.invalidated, [])

    def test_manual_refresh_forces_provider_refresh(self):
        request = Request(self.base + "/api/refresh", method="POST")
        with urlopen(request, timeout=2) as response:
            self.assertEqual(response.status, 200)
        self.assertTrue(server._collector_manager.snapshots[-1]["force"])

    def test_workbuddy_reconnect_uses_bundled_script(self):
        request = Request(self.base + "/api/workbuddy/reconnect", method="POST")
        with patch("server.subprocess.run") as run:
            run.return_value = type(
                "Completed",
                (),
                {"returncode": 0, "stdout": "AICC_WORKBUDDY_READY\n", "stderr": ""},
            )()
            with urlopen(request, timeout=2) as response:
                self.assertEqual(response.status, 200)
                self.assertEqual(json.load(response)["state"], "bridge_ready")
        self.assertIn("--ensure", run.call_args.args[0])
        self.assertIn("start-workbuddy-monitored.sh", run.call_args.args[0][1])
        self.assertIn("workbuddy", server._collector_manager.invalidated)

    def test_workbuddy_reconnect_reports_failure(self):
        request = Request(self.base + "/api/workbuddy/reconnect", method="POST")
        with patch("server.subprocess.run") as run:
            run.return_value = type(
                "Completed",
                (),
                {
                    "returncode": 1,
                    "stdout": "",
                    "stderr": "boom\nAICC_WORKBUDDY_FAIL:timeout\n",
                },
            )()
            with self.assertRaises(HTTPError) as caught:
                urlopen(request, timeout=2)
            self.assertEqual(caught.exception.code, 502)
            payload = json.loads(caught.exception.read().decode("utf-8"))
        self.assertEqual(payload["error_code"], "timeout")
        self.assertFalse(payload["ok"])

    def test_workbuddy_monitor_heals_bridge_and_refreshes_collector(self):
        with patch("server.subprocess.run") as run:
            run.return_value = type(
                "Completed",
                (),
                {"returncode": 0, "stdout": "WorkBuddy bridge auto-healed.\n", "stderr": ""},
            )()
            healed = server._run_workbuddy_monitor_once()
        self.assertTrue(healed)
        self.assertIn("--monitor", run.call_args.args[0])
        self.assertIn("workbuddy", server._collector_manager.invalidated)
        self.assertTrue(server._collector_manager.snapshots[-1]["force"])

    def test_workbuddy_monitor_does_nothing_when_bridge_is_already_up(self):
        with patch("server.subprocess.run") as run:
            run.return_value = type(
                "Completed",
                (),
                {"returncode": 0, "stdout": "", "stderr": ""},
            )()
            healed = server._run_workbuddy_monitor_once()
        self.assertFalse(healed)
        self.assertEqual(server._collector_manager.invalidated, [])

    def test_unavailable_workbuddy_snapshot_clears_old_manual_points(self):
        server.save_status({"workbuddy": {"points": 8520, "used_points": 1480}})
        server._collector_manager = type(
            "UnavailableManager",
            (),
            {
                "snapshot": lambda _self, **_kwargs: (
                    {"workbuddy": {"points": None, "balance_state": "Unavailable"}},
                    {},
                )
            },
        )()
        result = server.load_status(force=True)
        self.assertIsNone(result["workbuddy"]["points"])

    def test_legacy_settings_route_is_removed(self):
        with self.assertRaises(HTTPError) as caught:
            urlopen(self.base + "/settings", timeout=2)
        self.assertEqual(caught.exception.code, 404)

    def test_status_post_no_longer_accepts_manual_workbuddy_writes(self):
        server.save_status({"workbuddy": {"points": 45, "used_points": 3, "reset_text": "keep"}})
        body = json.dumps({"workbuddy": {"points": 45, "used_points": 3, "reset_text": "ok"}}).encode()
        request = Request(self.base + "/api/status", data=body, headers={"Content-Type": "application/json"}, method="POST")
        with self.assertRaises(HTTPError) as caught:
            urlopen(request, timeout=2)
        self.assertEqual(caught.exception.code, 404)
        saved = json.loads(server.DATA_PATH.read_text(encoding="utf-8"))
        self.assertEqual(saved["workbuddy"]["points"], 45)
        self.assertEqual(saved["workbuddy"]["reset_text"], "keep")
        self.assertEqual(server._collector_manager.invalidated, [])

    def test_remote_write_check(self):
        handler = server.DashboardHandler.__new__(server.DashboardHandler)
        handler.client_address = ("192.168.1.10", 1234)
        self.assertFalse(handler.is_local_request())

    def test_rate_limiter_is_bounded_per_window(self):
        with patch("server.time.monotonic", return_value=10):
            self.assertTrue(server.allow_request("client", "read", 2))
            self.assertTrue(server.allow_request("client", "read", 2))
            self.assertFalse(server.allow_request("client", "read", 2))


if __name__ == "__main__":
    unittest.main()
