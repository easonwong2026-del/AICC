"""DeepSeek failures preserve money values and never expose credentials."""
import io
import json
import socket
import ssl
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError, URLError

import server
from collectors import deepseek
from services.collector_manager import CollectorManager


BALANCE = {"currency": "CNY", "total_balance": "58.39"}


class DeepSeekRegressionTests(unittest.TestCase):
    def collect(self, payload=None, error=None):
        response = io.BytesIO(json.dumps(payload).encode())
        with patch.object(deepseek, "load_api_key", return_value="fixture-secret-never-log"), \
                patch.object(deepseek, "open_balance", side_effect=error, return_value=response):
            result = deepseek.collect()
        self.assertNotIn("fixture-secret-never-log", json.dumps(result))
        return result

    def test_live_and_invalid_responses(self):
        result = self.collect({"is_available": True, "balance_infos": [BALANCE]})
        self.assertEqual(result["status"], "Online")
        self.assertEqual(result["balances"][0]["total_balance"], "58.39")
        self.assertFalse(result["stale"])
        self.assertGreater(result["last_success"], 0)
        for payload in (None, [], {}, {"is_available": True, "balance_infos": []},
                        {"is_available": True, "balance_infos": [{"currency": "CNY", "total_balance": "NaN"}]}):
            self.assertEqual(self.collect(payload)["error_code"], "invalid_response")

    def test_safe_error_classification(self):
        cases = [(TimeoutError("fixture-secret-never-log"), "timeout"),
                 (URLError(socket.gaierror(-2, "secret")), "dns_error"),
                 (URLError(ssl.SSLError("secret")), "tls_error"),
                 (URLError(ConnectionRefusedError("secret")), "connection_error")]
        cases += [(HTTPError(deepseek.BALANCE_URL, code, "secret", {}, None), f"http_{code}")
                  for code in (401, 403, 429, 500, 503)]
        for error, code in cases:
            with self.subTest(code=code):
                self.assertEqual(self.collect(error=error)["error_code"], code)

    def test_proxy_configuration_is_reloaded_each_request(self):
        with patch.object(deepseek, "getproxies", side_effect=[{"https": "http://127.0.0.1:1"}, {}]), \
                patch.object(deepseek, "build_opener") as build:
            deepseek.open_balance(deepseek.Request(deepseek.BALANCE_URL))
            deepseek.open_balance(deepseek.Request(deepseek.BALANCE_URL))
        self.assertEqual(build.call_args_list[0].args[0].proxies, {"https": "http://127.0.0.1:1"})
        self.assertEqual(build.call_args_list[1].args[0].proxies, {})

    def test_last_known_good_failure_and_recovery(self):
        values = iter([{"status": "Online", "balances": [BALANCE], "stale": False},
                       deepseek.failure("connection_error", "Connection error"),
                       {"status": "Online", "balances": [{**BALANCE, "total_balance": "58.38"}], "stale": False}])
        manager = CollectorManager({"deepseek": (lambda force=False: next(values), 300, 8, {})})
        live, _ = manager.snapshot(force=True, wait_seconds=1)
        stale, meta = manager.snapshot(force=True, wait_seconds=1)
        self.assertEqual(stale["deepseek"]["balances"], [BALANCE])
        self.assertTrue(stale["deepseek"]["stale"])
        self.assertEqual(stale["deepseek"]["last_success"], live["deepseek"]["last_success"])
        self.assertEqual(meta["deepseek"]["state"], "error")
        recovered, _ = manager.snapshot(force=True, wait_seconds=1)
        self.assertFalse(recovered["deepseek"]["stale"])
        self.assertNotIn("error_code", recovered["deepseek"])
        self.assertEqual(recovered["deepseek"]["balances"][0]["total_balance"], "58.38")

    def test_no_previous_success_and_age(self):
        manager = CollectorManager({"deepseek": (
            lambda force=False: deepseek.failure("connection_error", "Connection error"), 300, 8, {})})
        values, _ = manager.snapshot(force=True, wait_seconds=1)
        self.assertEqual(values["deepseek"]["balances"], [])
        self.assertIsNone(values["deepseek"]["last_success"])
        self.assertTrue(values["deepseek"]["stale"])
        slot = manager._slots["deepseek"]
        slot.value = {"status": "Online", "balances": [BALANCE], "stale": False}
        slot.error = None
        slot.last_success = time.time() - 601
        self.assertTrue(manager._values_locked()["deepseek"]["stale"])

    def test_timeout_and_exception_retain_balance(self):
        for error in (RuntimeError("safe"),):
            def collect(force=False):
                raise error
            manager = CollectorManager({"deepseek": (collect, 300, 8, {
                "balances": [BALANCE], "last_success": time.time(), "status": "Online"})})
            values, _ = manager.snapshot(force=True, wait_seconds=1)
            self.assertTrue(values["deepseek"]["stale"])
            self.assertEqual(values["deepseek"]["balances"], [BALANCE])
            slot = manager._slots["deepseek"]
            slot.worker_alive = slot.running = True
            slot.started_monotonic = time.monotonic() - 9
            manager._expire_locked(time.monotonic())
            self.assertEqual(manager._values_locked()["deepseek"]["error_code"], "timeout")

    def test_server_restart_restores_persisted_deepseek(self):
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(server, "DATA_PATH", Path(directory) / "status.json"), \
                patch.object(server, "_collector_manager", None), \
                patch.object(server, "collect_deepseek", return_value=deepseek.failure("connection_error", "Connection error")):
            server.save_status({"deepseek": {"status": "Online", "balances": [BALANCE], "last_success": time.time()}})
            manager = server.collector_manager()
            self.assertEqual(manager._slots["deepseek"].value["balances"], [BALANCE])
            self.assertTrue(manager._slots["deepseek"].value["stale"])
            slot = manager._slots["deepseek"]
            manager._run("deepseek", slot, slot.generation, True)
            self.assertEqual(manager._values_locked()["deepseek"]["balances"], [BALANCE])


if __name__ == "__main__":
    unittest.main()
