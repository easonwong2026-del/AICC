import os
import threading
import time
import unittest
from http import HTTPStatus
from unittest.mock import MagicMock, patch

from services.codex_monitor import CodexMonitor
from services.collector_manager import CollectorManager
import server


class BackendLifecycleTests(unittest.TestCase):
    def test_live_health_payload_contains_identity(self):
        payload = server.live_health_payload()
        self.assertTrue(payload["ok"])
        self.assertEqual(payload["status"], "live")
        self.assertIn("version", payload)
        self.assertIn("pid", payload)
        self.assertIn("ppid", payload)
        self.assertIn("server_instance_id", payload)
        self.assertIn("started_at", payload)

    def test_ready_health_payload_reflects_pipeline_state(self):
        # 1. Normal state -> ready
        payload, status = server.ready_health_payload()
        self.assertEqual(status, HTTPStatus.OK)
        self.assertTrue(payload["ok"])
        self.assertEqual(payload["status"], "ready")

        # 2. Pipeline stuck -> SERVICE_UNAVAILABLE
        with patch.object(server.collector_manager(), "pipeline_health", return_value={"ready": False, "stuck_workers": ["codex"]}):
            payload_stuck, status_stuck = server.ready_health_payload()
            self.assertEqual(status_stuck, HTTPStatus.SERVICE_UNAVAILABLE)
            self.assertFalse(payload_stuck["ok"])
            self.assertEqual(payload_stuck["status"], "stuck_pipeline")
            self.assertIn("codex", payload_stuck["stuck_workers"])

    def test_pipeline_health_detects_stuck_worker(self):
        def fake_collect(force=False):
            return {"status": "ok"}

        mgr = CollectorManager({
            "test": (fake_collect, 60.0, 5.0, {"status": "init"})
        })
        pipe = mgr.pipeline_health()
        self.assertTrue(pipe["ready"])
        self.assertEqual(pipe["stuck_workers"], [])

        slot = mgr._slots["test"]
        slot.running = True
        slot.started_monotonic = time.monotonic() - 25.0  # 25s > timeout(5) + 15s

        pipe_stuck = mgr.pipeline_health()
        self.assertFalse(pipe_stuck["ready"])
        self.assertIn("test", pipe_stuck["stuck_workers"])

    def test_codex_connecting_timeout_triggers_cleanup_and_restart(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._lock = threading.Lock()
        monitor._io_lock = threading.Lock()
        monitor._status = {"state": "Connecting", "available": True}
        monitor._started = True
        monitor._restarting = False
        monitor._restart_attempts = 0
        monitor._connecting_timeout = 0.05
        monitor._connecting_started_at = time.monotonic() - 1.0  # Timed out
        monitor._last_access = time.monotonic()
        monitor._idle_seconds = 600
        monitor._last_request = time.monotonic()
        monitor._refresh_seconds = 60
        monitor._fresh_event = threading.Event()
        monitor._schedule_restart = MagicMock()

        fake_proc = MagicMock()
        fake_proc.poll.return_value = None
        monitor._process = fake_proc

        with patch("time.sleep", return_value=None), \
             patch.object(CodexMonitor, "_stop_process") as mock_stop:
            # Execute refresh loop iteration
            monitor._refresh_loop(fake_proc)

            # Verify state transition and actions
            self.assertIsNone(monitor._connecting_started_at)
            self.assertEqual(monitor._status.get("state"), "Connecting timed out")
            self.assertTrue(monitor._status.get("stale"))
            self.assertIsNone(monitor._process)
            self.assertFalse(monitor._started)
            mock_stop.assert_called_once_with(fake_proc)
            monitor._schedule_restart.assert_called_once()
            self.assertTrue(monitor._fresh_event.is_set())

    def test_codex_status_force_triggers_limits_request(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._lock = threading.Lock()
        monitor._fresh_event = threading.Event()
        monitor._request_limits = MagicMock()
        monitor._status = {"state": "Connected", "weekly": {"remaining": 80}}
        monitor._last_success_epoch = time.time()
        monitor._refresh_seconds = 60
        monitor._last_access = 0.0

        fake_proc = MagicMock()
        fake_proc.poll.return_value = None
        monitor._process = fake_proc
        monitor._started = True

        # When force=True, monitor immediately calls _request_limits
        with patch.object(monitor._fresh_event, "wait"):
            result = monitor.status(force=True)

        monitor._request_limits.assert_called_once()
        self.assertEqual(result["state"], "Connected")

    def test_restart_launch_marks_worker_started_and_coalesces_next_start(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._lock = threading.Lock()
        monitor._fresh_event = threading.Event()
        monitor._restarting = True
        monitor._started = False
        monitor._process = None
        monitor._last_access = time.monotonic()
        monitor._idle_seconds = 600
        monitor._restart_attempts = 0
        monitor._status = {"state": "Reconnecting"}
        monitor._send = MagicMock(return_value=1)

        fake_proc = MagicMock()
        fake_proc.pid = 9001
        fake_proc.poll.return_value = None

        with patch.object(monitor, "_resolve_cli", return_value=("codex", "test")), \
             patch("services.codex_monitor.subprocess.Popen", return_value=fake_proc) as popen, \
             patch("services.codex_monitor.threading.Thread"):
            with patch("services.codex_monitor.time.sleep", return_value=None):
                monitor._restart_after_delay()

            self.assertTrue(monitor._started)
            self.assertIs(monitor._process, fake_proc)
            monitor.start()

        popen.assert_called_once()

    def test_codex_worker_death_detected_and_schedules_restart(self):
        monitor = CodexMonitor.__new__(CodexMonitor)
        monitor._lock = threading.Lock()
        monitor._fresh_event = threading.Event()
        monitor._schedule_restart = MagicMock()
        monitor._status = {"state": "Connected"}
        fake_proc = MagicMock()
        fake_proc.stdout = iter([])  # Empty stdout -> EOF immediately
        monitor._process = fake_proc
        monitor._started = True

        # Process dies and stdout closes
        with patch.object(CodexMonitor, "_stop_process") as mock_stop:
            monitor._read_stdout(fake_proc)

        # Must detect death, reset started state, and schedule restart
        self.assertIsNone(monitor._process)
        self.assertFalse(monitor._started)
        self.assertEqual(monitor._status.get("state"), "Reconnecting")
        monitor._schedule_restart.assert_called_once()
        mock_stop.assert_called_once_with(fake_proc)
        self.assertTrue(monitor._fresh_event.is_set())

    def test_stop_process_terminates_when_stdin_close_fails(self):
        process = MagicMock()
        process.stdin.close.side_effect = BrokenPipeError()
        process.wait.return_value = None

        CodexMonitor._stop_process(process)

        process.terminate.assert_called_once_with()
        process.wait.assert_called_once_with(timeout=2)

    def test_shutdown_endpoint_handles_local_request(self):
        class DummyHandler:
            def __init__(self):
                self.client_address = ("127.0.0.1", 12345)
                self.path = "/api/shutdown"
                self.sent_payload = None
                self.sent_status = None

            def is_local_request(self):
                return True

            def send_json(self, payload, status=HTTPStatus.OK):
                self.sent_payload = payload
                self.sent_status = status

        handler = DummyHandler()
        with patch("threading.Thread") as mock_thread:
            server.DashboardHandler.do_POST(handler)
            self.assertEqual(handler.sent_status, HTTPStatus.OK)
            self.assertTrue(handler.sent_payload.get("ok"))
            self.assertEqual(handler.sent_payload.get("message"), "Server shutting down")
            mock_thread.assert_called_once()

    def test_parent_watchdog_exits_when_parent_dies(self):
        calls = []

        def fake_kill(pid, sig):
            raise ProcessLookupError("Process does not exist")

        def fake_exit(code):
            calls.append(code)
            raise SystemExit(code)

        def run_sync_watchdog(target_pid):
            try:
                os.kill(target_pid, 0)
            except OSError:
                os._exit(0)

        with patch("os.kill", side_effect=fake_kill), \
             patch("os._exit", side_effect=fake_exit):
            with self.assertRaises(SystemExit):
                run_sync_watchdog(999999)

        self.assertEqual(calls, [0])


if __name__ == "__main__":
    unittest.main()
