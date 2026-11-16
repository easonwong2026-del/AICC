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
        server._pair_failures.clear()
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

        # 同一来源连续猜错，被限速（429）—— 但配对码本身没作废。
        code = self.request("POST", "/api/pairing")[1]["code"]
        wrong = "00000000" if code != "00000000" else "11111111"
        for _ in range(server.PAIR_FAILURES_PER_WINDOW):
            self.assertEqual(self.request("POST", "/api/pair", {"code": wrong})[0], 403)
        self.assertEqual(self.request("POST", "/api/pair", {"code": code})[0], 429)

        # 重新生成码 = 新一轮，失败计数清零，合法设备立刻能配上。
        # 这里同时清掉分钟级请求计数：本条测的是"失败计数"，不是"每分钟请求数"，
        # 否则会被 10 次/分钟的通用限流挡在前面。
        server._rates.clear()
        code = self.request("POST", "/api/pairing")[1]["code"]
        self.assertEqual(self.request("POST", "/api/pair", {"code": code})[0], 200)

        for code in (None, [], "１２３４５６７８", "123", "<secret>"):
            self.assertEqual(self.request("POST", "/api/pair", {"code": code})[0], 400)
        with patch.object(server, "_token", "rotated-credential"):
            self.assertEqual(self.request("GET", "/api/status", headers={"Authorization": "Bearer old"})[0], 401)

    def test_pair_failures_are_counted_per_source_ip(self):
        """别的设备猜错，不该让合法设备的配对码失效。"""
        for _ in range(server.PAIR_FAILURES_PER_WINDOW):
            server.note_pair_failure("10.0.0.5")
        self.assertTrue(server.pair_blocked("10.0.0.5"))
        self.assertFalse(
            server.pair_blocked("10.0.0.6"),
            "另一个来源必须还能配对",
        )

        # 窗口过期后自动放开，不需要人工清表。
        server._pair_failures["10.0.0.5"] = (
            time.monotonic() - server.PAIR_FAILURE_WINDOW_SECONDS - 1,
            server.PAIR_FAILURES_PER_WINDOW,
        )
        self.assertFalse(server.pair_blocked("10.0.0.5"))

        # 成功配对清掉自己的计数。
        server.note_pair_failure("10.0.0.7")
        server.clear_pair_failures("10.0.0.7")
        self.assertFalse(server.pair_blocked("10.0.0.7"))

    def test_bridge_listens_on_loopback_unless_lan_is_explicitly_enabled(self):
        with patch.dict("os.environ", {}, clear=True):
            self.assertEqual(server.requested_host(), "127.0.0.1")
            self.assertFalse(server.lan_enabled())
            self.assertFalse(server.discovery_enabled(), "默认连发现都不开")

        with patch.dict("os.environ", {"EINK_HOST": "0.0.0.0"}):
            self.assertEqual(server.requested_host(), "0.0.0.0")
            self.assertTrue(server.lan_enabled())
            self.assertFalse(server.discovery_enabled(), "局域网开着也要显式打开发现")

        with patch.dict("os.environ", {"EINK_HOST": "0.0.0.0", "EINK_DISCOVERY": "1"}):
            self.assertTrue(server.discovery_enabled())

        # 拼错的值一律按回环处理：宁可连不上，也别绑到意料之外的网卡。
        for wrong in ("localhost", "192.168.1.5", "", "0.0.0.0.0", "::"):
            with patch.dict("os.environ", {"EINK_HOST": wrong}, clear=True):
                self.assertEqual(server.requested_host(), "127.0.0.1", repr(wrong))

    def test_host_id_survives_concurrent_first_creation(self):
        path = Path(self.directory.name) / "host_id"
        results = []
        barrier = threading.Barrier(8)

        def worker():
            barrier.wait()
            results.append(server.host_id())

        threads = [threading.Thread(target=worker) for _ in range(8)]
        for thread in threads:
            thread.start()
        for thread in threads:
            thread.join()

        self.assertEqual(len(results), 8)
        self.assertEqual(len(set(results)), 1, f"并发创建必须收敛到同一个 id：{results}")
        self.assertTrue(results[0], "不许返回空 host_id")
        self.assertEqual(results[0], path.read_text(encoding="utf-8").strip())

    def test_legacy_view_skips_a_cached_card_without_id_or_label(self):
        """缓存来自旧版本时，缺字段的卡片要被跳过，而不是让 /api/status 变成 500。"""
        server._snapshot = {
            "state": "Connected",
            "stale": False,
            "accounts": [
                {"provider": "codex", "kind": "subscription", "id": "a1", "account_label": "Main",
                 "items": [{"name": "7 days", "remaining": 50, "reset": None}]},
                {"provider": "codex", "kind": "subscription",
                 "items": [{"name": "7 days", "remaining": 1}]},
                {"provider": "codex", "kind": "subscription", "id": "a3",
                 "items": [{"name": "5 hours", "remaining": 10}]},
            ],
        }
        status, legacy = self.request("GET", "/api/status?format=2", headers=self.paired_headers())
        self.assertEqual(status, 200)
        ids = [account["id"] for account in legacy["codex"]["accounts"]]
        self.assertEqual(ids, ["a1"], f"只有完整的卡片能被展示：{legacy}")

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
