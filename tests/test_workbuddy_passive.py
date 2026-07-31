import json
import shutil
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from collectors import workbuddy
from services.cdp import CdpError
from providers.base import CacheStore
from providers.workbuddy import WorkBuddyProvider
from services.collector_manager import CollectorManager


class WorkBuddyPassiveTests(unittest.TestCase):
    def setUp(self):
        workbuddy._last_attempt = 0.0
        workbuddy._account_cache = None
        workbuddy._account_target = None
        workbuddy._account_last_error = False
        workbuddy._account_last_error_code = None
        workbuddy._account_last_diag = None

    def test_same_target_is_read_again_after_refresh_interval(self):
        raw = {"usageLeft": "6000", "refreshAt": None}
        updated = {"usageLeft": "5238", "refreshAt": None}
        with patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "_save_account_cache"), \
             patch.object(workbuddy, "target_identity_localhost", return_value="session-1"), \
             patch.object(workbuddy, "evaluate_localhost", side_effect=[raw, updated]) as evaluate, \
             patch.object(workbuddy.time, "monotonic", side_effect=[100.0, 219.0, 221.0]), \
             patch.object(workbuddy.time, "time", return_value=1_000.0):
            first = workbuddy._get_account_usage()
            cached = workbuddy._get_account_usage()
            refreshed = workbuddy._get_account_usage()

        self.assertEqual(first["points"], 6000)
        self.assertEqual(cached["points"], 6000)
        self.assertEqual(refreshed["points"], 5238)
        self.assertEqual(evaluate.call_count, 2)

    def test_manual_refresh_bypasses_interval_on_same_target(self):
        raw = {"usageLeft": "6000", "refreshAt": None}
        updated = {"usageLeft": "5238", "refreshAt": None}
        with patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "_save_account_cache"), \
             patch.object(workbuddy, "target_identity_localhost", return_value="session-1"), \
             patch.object(workbuddy, "evaluate_localhost", side_effect=[raw, updated]) as evaluate, \
             patch.object(workbuddy.time, "monotonic", return_value=100.0), \
             patch.object(workbuddy.time, "time", return_value=1_000.0):
            workbuddy._get_account_usage()
            result = workbuddy._get_account_usage(force=True)

        self.assertEqual(result["points"], 5238)
        self.assertEqual(evaluate.call_count, 2)

    def test_failed_read_keeps_last_successful_balance(self):
        with patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "_save_account_cache"), \
             patch.object(workbuddy, "target_identity_localhost", return_value="session-1"), \
             patch.object(workbuddy, "evaluate_localhost", side_effect=[{"usageLeft": "6000"}, CdpError("read failed")]), \
             patch.object(workbuddy.time, "monotonic", side_effect=[100.0, 221.0]), \
             patch.object(workbuddy.time, "time", return_value=1_000.0):
            first = workbuddy._get_account_usage()
            second = workbuddy._get_account_usage(force=True)

        self.assertEqual(first["points"], 6000)
        self.assertEqual(second["points"], 6000)
        self.assertEqual(second["balance_updated_epoch"], 1_000.0)
        self.assertEqual(second["balance_state"], "Cached")

    def test_expired_cache_is_marked_stale(self):
        cache = {"points": 5238, "balance_updated_epoch": 100.0}
        with patch.object(workbuddy, "_load_account_cache", return_value=cache), \
             patch.object(workbuddy, "target_identity_localhost", side_effect=CdpError("closed")), \
             patch.object(workbuddy.time, "monotonic", return_value=100.0), \
             patch.object(workbuddy.time, "time", return_value=501.0):
            result = workbuddy._get_account_usage()

        self.assertEqual(result["points"], 5238)
        self.assertTrue(result["balance_stale"])
        self.assertEqual(result["balance_age_seconds"], 401)
        self.assertEqual(result["balance_state"], "Cached")

    def test_closed_bridge_is_passive_and_does_not_evaluate_or_start(self):
        with patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "target_identity_localhost", side_effect=CdpError("closed")), \
             patch.object(workbuddy, "evaluate_localhost") as evaluate, \
             patch.object(workbuddy.time, "monotonic", return_value=100.0):
            self.assertIsNone(workbuddy._get_account_usage())

        evaluate.assert_not_called()

    def test_balance_number_formats_are_normalised(self):
        for raw, expected in (("5238", 5238), ("5,238", 5238), ("5，238", 5238), ("5238.5", 5238.5)):
            with self.subTest(raw=raw):
                self.assertEqual(workbuddy._normalise_account_usage({"usageLeft": raw})["points"], expected)
        self.assertEqual(workbuddy._normalise_account_usage({"balance": "5 238"})["points"], 5238)

    def test_bridge_failure_does_not_expose_manual_fallback_points(self):
        with tempfile.TemporaryDirectory() as directory, \
             patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "target_identity_localhost", side_effect=CdpError("WorkBuddy monitoring bridge is not running")):
            result = workbuddy.collect(
                {"points": 8520, "used_points": 1480, "reset_text": "Update from WorkBuddy"},
                database_path=Path(directory) / "missing.db",
            )

        self.assertIsNone(result["points"])
        self.assertIsNone(result["used_points"])
        self.assertEqual(result["balance_state"], "Unavailable")
        self.assertTrue(result["balance_stale"])
        self.assertEqual(result["balance_error_code"], "bridge_unavailable")
        self.assertEqual(result["usage_source"], "WorkBuddy unavailable")

    def test_expression_uses_safe_api_fallback_and_never_reads_page_body(self):
        expression = workbuddy.ACCOUNT_EXPRESSION.lower()
        self.assertIn("account_api_invalid", expression)
        self.assertIn("auth:getaccountusage", expression)
        self.assertIn("__apiprobe", expression)
        self.assertIn("waitforbalance", expression)
        self.assertNotIn("document.body", expression)
        self.assertNotIn("localstorage", expression)
        self.assertNotIn("sessionstorage", expression)
        self.assertNotIn("cookie", expression)
        self.assertNotIn("authorization", expression)

    def test_workbuddy_5_3_invoke_channel_returns_usage(self):
        if shutil.which("node") is None:
            self.skipTest("Node.js is not installed")
        result = self._run_expression(
            api_function=None,
            api_invoke="async (command) => {"
            " globalThis.__cmd = command;"
            " return {usageLeft: '5,238', usageTotal: '12000', usageUsed: '6762', refreshAt: null};"
            " }",
            button_text="",
        )
        self.assertEqual(result["usageLeft"], "5238")
        self.assertEqual(result["usageTotal"], "12000")
        self.assertEqual(result["source"], "account-api")

    def test_workbuddy_5_3_daemon_rpc_returns_usage(self):
        if shutil.which("node") is None:
            self.skipTest("Node.js is not installed")
        daemon = (
            "(port) => {"
            "  port.onmessage = (event) => {"
            "    const payload = event.data;"
            "    if (!payload || payload.kind !== 'message' || !payload.json) return;"
            "    const frame = payload.json;"
            "    if (frame.channel !== 'auth:getAccountUsage') {"
            "      port.postMessage({kind: 'message', json: {"
            "        id: frame.id, type: 'error', error: {message: 'no handler for ' + frame.channel}"
            "      }});"
            "      return;"
            "    }"
            "    port.postMessage({kind: 'message', json: {"
            "      id: frame.id, type: 'response',"
            "      result: {usageLeft: '5,238', usageTotal: '12000', usageUsed: '6762', refreshAt: null}"
            "    }});"
            "  };"
            "  port.start();"
            "  port.postMessage({kind: 'open', sessionId: 'test-session'});"
            "}"
        )
        result = self._run_expression(api_function=None, api_invoke=None, button_text="", api_daemon=daemon)
        self.assertEqual(result["usageLeft"], "5238")
        self.assertEqual(result["usageTotal"], "12000")
        self.assertEqual(result["usageUsed"], "6762")
        self.assertEqual(result["source"], "account-api")

    def test_failure_includes_api_probe_diagnostics(self):
        if shutil.which("node") is None:
            self.skipTest("Node.js is not installed")
        result = self._run_expression(api_function=None, api_invoke=None, button_text="")
        self.assertEqual(result["__errorCode"], "account_api_unavailable")
        self.assertIn("invoke", result["__apiProbe"]["hostKeys"])
        self.assertTrue(all("token" not in key.lower() for key in result["__apiProbe"]["hostKeys"]))
        self.assertIn("daemon", result["__apiError"])

    def test_collector_surfaces_api_diagnostics_in_error_payload(self):
        raw = {
            "__errorCode": "account_api_unavailable",
            "__apiProbe": {"page": "file:///renderer/index.html", "hostKeys": ["invoke"]},
            "__apiError": "boom",
        }
        with patch.object(workbuddy, "_load_account_cache", return_value=None), \
             patch.object(workbuddy, "target_identity_localhost", return_value="session-1"), \
             patch.object(workbuddy, "evaluate_localhost", return_value=raw), \
             patch.object(workbuddy.time, "monotonic", return_value=100.0):
            result = workbuddy.collect({}, database_path=Path("/tmp/missing-workbuddy.db"))

        self.assertEqual(result["balance_error_code"], "account_api_unavailable")
        self.assertEqual(result["balance_diagnostic"]["__apiProbe"]["hostKeys"], ["invoke"])

    def test_api_invalid_result_falls_back_to_same_line_dom_balance(self):
        if shutil.which("node") is None:
            self.skipTest("Node.js is not installed")
        result = self._run_expression(
            "async () => ({points: 'not-a-balance'})",
            "积分余额 刷新 5,238",
        )
        self.assertEqual(result["usageLeft"], "5238")
        self.assertEqual(result["source"], "account-menu")

    def test_api_exception_falls_back_to_adjacent_dom_nodes(self):
        if shutil.which("node") is None:
            self.skipTest("Node.js is not installed")
        result = self._run_expression(
            "async () => { throw new Error('account API unavailable'); }",
            "积分余额",
            parent_text="积分余额",
            sibling_text="5，238",
        )
        self.assertEqual(result["usageLeft"], "5238")
        self.assertEqual(result["source"], "account-menu")

    def _run_expression(self, api_function, button_text, parent_text="", sibling_text="", api_invoke=None, api_daemon=None):
        expression = json.dumps(workbuddy.ACCOUNT_EXPRESSION)
        button_text = json.dumps(button_text)
        parent_text = json.dumps(parent_text)
        sibling_text = json.dumps(sibling_text)
        legacy_api = api_function if api_function is not None else "null"
        invoke_api = api_invoke if api_invoke is not None else "null"
        daemon_api = api_daemon if api_daemon is not None else "null"
        script = f"""
globalThis.workbuddyDesktop = {{ authGetAccountUsage: {legacy_api}, invoke: {invoke_api} }};
globalThis.window = {{
  postMessage: (data, origin, ports) => {{
    if (!data || data.type !== 'workbuddy:open-local-daemon-transport-port') return;
    const daemon = {daemon_api};
    if (typeof daemon === 'function') daemon(ports[0]);
  }}
}};
const sibling = {{ tagName: 'SPAN', innerText: {sibling_text}, textContent: {sibling_text},
  parentElement: null, nextElementSibling: null,
  getAttribute: () => null, getBoundingClientRect: () => ({{width: 10, height: 10}}) }};
const parent = {{ tagName: 'DIV', innerText: {parent_text}, textContent: {parent_text},
  parentElement: null, nextElementSibling: sibling,
  getAttribute: () => null, getBoundingClientRect: () => ({{width: 10, height: 10}}) }};
const button = {{ tagName: 'BUTTON', innerText: {button_text}, textContent: {button_text},
  parentElement: parent, nextElementSibling: sibling,
  getAttribute: () => null, getBoundingClientRect: () => ({{width: 10, height: 10}}), click: () => {{}} }};
globalThis.getComputedStyle = () => ({{display: 'block', visibility: 'visible'}});
globalThis.document = {{ querySelectorAll: (selector) => selector.includes('button') ? [button] : [] }};
Promise.resolve(eval({expression})).then((result) => process.stdout.write(JSON.stringify(result)));
"""
        completed = subprocess.run(["node", "-e", script], capture_output=True, text=True, timeout=8, check=True)
        return json.loads(completed.stdout)

    def test_workbuddy_refresh_forwards_force(self):
        provider = WorkBuddyProvider(CacheStore(Path(".")), lambda: {}, {})
        with patch("providers.workbuddy.collect", return_value={"points": 5238}) as collect:
            provider.refresh(force=True)
        collect.assert_called_once_with({}, force=True)

    def test_provider_status_recomputes_balance_age_without_collecting(self):
        provider = WorkBuddyProvider(
            CacheStore(Path(".")),
            lambda: {},
            {"points": 5238, "balance_updated_epoch": 100.0, "balance_state": "Connected"},
        )
        with patch.object(workbuddy.time, "time", return_value=501.0):
            result = provider.status()

        self.assertEqual(result["balance_age_seconds"], 401)
        self.assertTrue(result["balance_stale"])
        self.assertEqual(result["balance_state"], "Cached")

    def test_provider_does_not_expose_old_manual_initial_points(self):
        provider = WorkBuddyProvider(
            CacheStore(Path(".")),
            lambda: {},
            {"points": 8520, "used_points": 1480, "reset_text": "Manual"},
        )
        result = provider.status()

        self.assertIsNone(result["points"])
        self.assertIsNone(result["used_points"])
        self.assertEqual(result["balance_state"], "Unavailable")

    def test_concurrent_workbuddy_refreshes_are_coalesced(self):
        started = threading.Event()
        release = threading.Event()
        calls = 0

        def collect(_fallback, force=False):
            nonlocal calls
            calls += 1
            started.set()
            release.wait(1)
            return {"points": 5238}

        with tempfile.TemporaryDirectory() as directory, patch("providers.workbuddy.collect", side_effect=collect):
            provider = WorkBuddyProvider(CacheStore(Path(directory)), lambda: {}, {})
            manager = CollectorManager({"workbuddy": provider})
            manager.snapshot(force=True)
            self.assertTrue(started.wait(1))
            manager.snapshot(force=True)
            release.set()

        self.assertEqual(calls, 1)

    def test_closed_bridge_keeps_cache_and_allows_next_session_probe(self):
        cache = {"points": 88, "balance_updated_epoch": 900.0}
        with patch.object(workbuddy, "_load_account_cache", return_value=cache), \
             patch.object(workbuddy, "target_identity_localhost", side_effect=CdpError("closed")), \
             patch.object(workbuddy.time, "monotonic", return_value=100.0), \
             patch.object(workbuddy.time, "time", return_value=1_000.0):
            result = workbuddy._get_account_usage()

        self.assertEqual(result["points"], 88)
        self.assertIsNone(workbuddy._account_target)


if __name__ == "__main__":
    unittest.main()
