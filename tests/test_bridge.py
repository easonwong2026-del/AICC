import copy
import http.client
import json
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

import server
from collectors import magpie

FIXTURE = json.loads((Path(__file__).parent / "fixtures/magpie_quotas.json").read_text())
NOW = magpie.timestamp("2026-10-14T13:32:00+08:00")


class BridgeTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = patch.object(server, "DATA_PATH", Path(self.directory.name) / "status.json")
        self.path.start()
        self.cache = patch.object(magpie, "CACHE_PATH", Path(self.directory.name) / "magpie.json")
        self.cache.start()
        server._rates.clear()
        server._snapshot = None
        server._pair_code = None
        self.http = server.DashboardServer(("127.0.0.1", 0), server.DashboardHandler)
        self.worker = threading.Thread(target=self.http.serve_forever, daemon=True)
        self.worker.start()

    def tearDown(self):
        self.http.shutdown()
        self.http.server_close()
        self.worker.join()
        self.path.stop()
        self.cache.stop()
        self.directory.cleanup()

    def request(self, method, path, body=None, headers=None):
        connection = http.client.HTTPConnection("127.0.0.1", self.http.server_port, timeout=3)
        connection.request(method, path, json.dumps(body) if body is not None else None, headers or {})
        response = connection.getresponse()
        status, payload = response.status, json.loads(response.read())
        connection.close()
        return status, payload

    def pair(self):
        code = self.request("POST", "/api/pairing")[1]["code"]
        return self.request("POST", "/api/pair", {"code": code})[1]

    def paired_headers(self):
        return {"Authorization": "Bearer " + self.pair()["token"]}

    def test_pair_once_then_read_only_and_no_browser_management(self):
        self.assertEqual(self.request("GET", "/api/status")[0], 401)
        live = self.request("GET", "/api/health/live")[1]
        self.assertEqual(live["companion"], True)
        self.assertEqual(live["status_schema"], 3)
        paired = self.pair()
        self.assertEqual(self.request("POST", "/api/pair", {"code": "00000000"})[0], 403)
        headers = {"Authorization": "Bearer " + paired["token"]}
        self.assertEqual(self.request("GET", "/api/status", headers=headers)[0], 200)
        for route in ("/api/refresh", "/api/display-snapshot", "/api/workbuddy/reconnect",
                      "/api/magpie/quotas/refresh", "/api/accounts"):
            self.assertEqual(self.request("POST", route, headers=headers)[0], 403)
        self.assertEqual(self.request("POST", "/api/pairing", headers={"Origin": "https://evil.invalid"})[0], 403)
        self.assertEqual(self.request("POST", "/api/shutdown", headers={"Host": "evil.invalid"})[0], 403)
        self.assertEqual(self.request("GET", "/collectors/magpie.py")[0], 404)

    def test_pair_expiry_attempt_limit_and_credential_rotation(self):
        code = self.request("POST", "/api/pairing")[1]["code"]
        server._pair_deadline = time.monotonic() - 1
        self.assertEqual(self.request("POST", "/api/pair", {"code": code})[0], 403)
        code = self.request("POST", "/api/pairing")[1]["code"]
        server._pair_attempts = 5
        self.assertEqual(self.request("POST", "/api/pair", {"code": code})[0], 403)
        for code in (None, [], "１２３４５６７８", "123", "<secret>"):
            self.assertEqual(self.request("POST", "/api/pair", {"code": code})[0], 400)
        with patch.object(server, "_token", "rotated-credential"):
            self.assertEqual(self.request("GET", "/api/status", headers={"Authorization": "Bearer old"})[0], 401)

    def test_status_is_the_generic_snapshot(self):
        server._snapshot = magpie.parse_report(FIXTURE, time.time())
        status, payload = self.request("GET", "/api/status", headers=self.paired_headers())
        self.assertEqual(status, 200)
        self.assertEqual(payload["schema_version"], 3)
        self.assertEqual(payload["source"], "Magpie")
        self.assertIn("codex", payload["providers"])
        self.assertIn("deepseek", payload["providers"])
        self.assertEqual(len(payload["accounts"]), len(FIXTURE["data"]))
        self.assertIn("fetched_at", payload)
        encoded = json.dumps(payload)
        self.assertNotIn("@", encoded)
        self.assertNotIn("invalid_grant", encoded)
        kinds = {card["kind"] for card in payload["accounts"]}
        self.assertEqual(kinds, {"subscription", "plan", "balance"})
        self.assertNotIn("codex", payload)

    def test_legacy_format_is_an_explicit_derived_view(self):
        server._snapshot = magpie.parse_report(FIXTURE, time.time())
        headers = self.paired_headers()
        status, legacy = self.request("GET", "/api/status?format=2", headers=headers)
        self.assertEqual(status, 200)
        self.assertIn("codex", legacy)
        self.assertNotIn("accounts", legacy)
        self.assertNotIn("providers", legacy)
        self.assertEqual(legacy["codex"]["schema_version"], 2)
        self.assertTrue(all(account["label"].startswith("Codex · ") for account in legacy["codex"]["accounts"]))
        self.assertTrue(all("workbuddy" not in json.dumps(account) for account in legacy["codex"]["accounts"]))
        self.assertEqual(len(legacy["codex"]["accounts"]), 2)
        by_header = self.request("GET", "/api/status", headers={**headers, "X-AICC-Schema": "2"})[1]
        self.assertEqual(by_header, legacy | {"fetched_at": by_header["fetched_at"]})
        generic = self.request("GET", "/api/status", headers={**headers, "X-AICC-Schema": "3"})[1]
        self.assertEqual(generic["schema_version"], 3)

    def test_cached_accounts_age_independently_and_source_does_not_switch(self):
        server._snapshot = magpie.parse_report(FIXTURE, NOW)
        server._snapshot["accounts"][0]["read_at"] = time.time() - magpie.MAX_AGE_SECONDS - 1
        server._snapshot["accounts"][1]["read_at"] = time.time()
        server._snapshot["accounts"][1]["stale"] = False
        displayed = server.load_status()
        self.assertTrue(displayed["accounts"][0]["stale"])
        self.assertFalse(displayed["accounts"][1]["stale"])
        self.assertFalse(displayed["stale"])
        self.assertEqual(displayed["state"], "Connected")
        before = copy.deepcopy(server._snapshot)
        server.load_status()
        self.assertEqual(server._snapshot, before)
        with patch.object(magpie, "collect", side_effect=ValueError("private-secret")), \
                patch.object(magpie, "load_cache", return_value=magpie.mark_stale(before)):
            server.refresh_status()
        fallback = server.load_status()
        self.assertEqual(fallback["source"], "Magpie")
        self.assertTrue(fallback["stale"])
        self.assertEqual(fallback["state"], "Cached")
        self.assertNotIn("private-secret", json.dumps(fallback))

    def test_empty_and_missing_cache_is_offline_not_zero(self):
        status = server.load_status()
        self.assertEqual(status["state"], "Empty")
        self.assertFalse(status["available"])
        self.assertEqual(status["accounts"], [])
        legacy = server.load_status(2)
        self.assertFalse(legacy["codex"]["available"])

    def test_network_reads_never_collect_and_rate_table_is_bounded(self):
        with patch.object(magpie, "collect") as collect:
            for _ in range(10):
                server.load_status()
            collect.assert_not_called()
        for index in range(1000):
            server.allow_request(str(index), "read", 60)
        self.assertEqual(len(server._rates), 512)
        for _ in range(60):
            self.assertTrue(server.allow_request("client", "read", 60))
        self.assertFalse(server.allow_request("client", "read", 60))

    def test_refresh_policy_is_shared_not_per_client(self):
        self.assertEqual(magpie.REFRESH_SECONDS, server.magpie.REFRESH_SECONDS)
        status = self.request("GET", "/api/status", headers=self.paired_headers())[1]
        self.assertEqual(status["refresh_seconds"], magpie.REFRESH_SECONDS)


if __name__ == "__main__":
    unittest.main()
