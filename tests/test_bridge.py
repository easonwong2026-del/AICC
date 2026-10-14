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

    def test_pair_once_then_read_only_and_no_browser_management(self):
        self.assertEqual(self.request("GET", "/api/status")[0], 401)
        self.assertEqual(self.request("GET", "/api/health/live")[1]["companion"], True)
        code = self.request("POST", "/api/pairing")[1]["code"]
        status, paired = self.request("POST", "/api/pair", {"code": code})
        self.assertEqual(status, 200)
        self.assertEqual(self.request("POST", "/api/pair", {"code": code})[0], 403)
        headers = {"Authorization": "Bearer " + paired["token"]}
        self.assertEqual(self.request("GET", "/api/status", headers=headers)[0], 200)
        for route in ("/api/refresh", "/api/display-snapshot", "/api/workbuddy/reconnect"):
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

    def test_cached_accounts_age_independently_and_source_does_not_switch(self):
        fixture = json.loads((Path(__file__).parent / "fixtures/magpie_quotas.json").read_text())
        result = magpie.parse_report(fixture, magpie.timestamp("2026-10-14T13:32:00+08:00"))
        result["accounts"][0]["read_at"] = time.time() - 301
        result["accounts"][1]["read_at"] = time.time()
        server._snapshot = result
        displayed = server.load_status()["codex"]
        self.assertTrue(displayed["accounts"][0]["stale"])
        self.assertFalse(displayed["accounts"][1]["stale"])
        self.assertFalse(displayed["stale"])
        self.assertNotIn("@", json.dumps(displayed))
        before = copy.deepcopy(result)
        server.load_status()
        self.assertEqual(result, before)
        with patch.object(magpie, "collect", side_effect=ValueError("private-secret")), \
                patch.object(magpie, "load_cache", return_value=magpie.mark_stale(result)):
            server.refresh_status()
        self.assertEqual(server.load_status()["codex"]["source"], "Magpie")
        self.assertTrue(server.load_status()["codex"]["stale"])
        self.assertNotIn("private-secret", json.dumps(server.load_status()))

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


if __name__ == "__main__":
    unittest.main()
