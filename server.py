#!/usr/bin/env python3
"""Dependency-free local server for AICC — AI Status Center."""

from __future__ import annotations

import json
import mimetypes
import os
import socket
import subprocess
import threading
import time
from collections import deque
from datetime import datetime, timezone
from http import HTTPStatus
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

from collectors.deepseek import collect as collect_deepseek
from collectors.system import collect as collect_system
from collectors.workbuddy import collect as collect_workbuddy, initial_status
from services.codex_monitor import monitor
from services.collector_manager import (
    DEFAULT_COLLECTOR_INTERVAL,
    DEFAULT_COLLECTOR_TIMEOUT,
    CollectorManager,
)

ROOT = Path(__file__).resolve().parent
DATA_ROOT = Path(os.environ.get("EINK_DATA_DIR", ROOT / "data")).expanduser()
DATA_PATH = DATA_ROOT / "status.json"
WEB_ROOT = Path(os.environ.get("EINK_WEB_ROOT", ROOT / "web")).expanduser()
COLLECTOR_WAIT_SECONDS = max(0, min(5, float(os.environ.get("COLLECTOR_WAIT_SECONDS", "3.2"))))
SERVER_STARTED_AT = time.time()
mimetypes.add_type("application/manifest+json", ".webmanifest")
DEFAULT_STATUS = {
    "codex": {"five_hour": {"remaining": 83, "reset": "14:27"}, "weekly": {"remaining": 97, "reset": "7月17日"}, "source": "Manual"},
    "workbuddy": {
        "points": None,
        "balance_state": "Unavailable",
        "balance_stale": True,
        "usage_source": "WorkBuddy unavailable",
    },
}
DISCOVERY_MAGIC = b"AI_EINK_DISCOVER"
_collector_manager: CollectorManager | None = None
_rate_lock = threading.Lock()
_status_write_lock = threading.Lock()
_rate_windows: dict[tuple[str, str], deque[float]] = {}


def allow_request(address: str, category: str, limit: int) -> bool:
    now = time.monotonic()
    key = (address, category)
    with _rate_lock:
        window = _rate_windows.setdefault(key, deque())
        while window and now - window[0] >= 60:
            window.popleft()
        if len(window) >= limit:
            return False
        window.append(now)
        if len(_rate_windows) > 512:
            for stale_key in [item for item, values in _rate_windows.items() if not values or now - values[-1] >= 60]:
                _rate_windows.pop(stale_key, None)
        return True


def persisted_status() -> dict:
    DATA_PATH.parent.mkdir(parents=True, exist_ok=True)
    if not DATA_PATH.exists():
        save_status(DEFAULT_STATUS)
    try:
        return json.loads(DATA_PATH.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return DEFAULT_STATUS.copy()


def collector_manager() -> CollectorManager:
    global _collector_manager
    if _collector_manager is None:
        fallback = persisted_status()

        def collect_codex(force: bool = False) -> dict:
            return monitor.status()

        def collect_deepseek_value(force: bool = False) -> dict:
            return collect_deepseek(DATA_ROOT / "deepseek_history.json")

        def collect_workbuddy_value(force: bool = False) -> dict:
            return collect_workbuddy(
                persisted_status().get("workbuddy", {}),
                force=force,
            )

        def collect_system_value(force: bool = False) -> dict:
            return collect_system()

        _collector_manager = CollectorManager({
            "codex": (
                collect_codex,
                DEFAULT_COLLECTOR_INTERVAL,
                DEFAULT_COLLECTOR_TIMEOUT,
                fallback.get("codex", {}),
            ),
            "deepseek": (
                collect_deepseek_value,
                300.0,
                DEFAULT_COLLECTOR_TIMEOUT,
                {"status": "Loading", "balances": []},
            ),
            "workbuddy": (
                collect_workbuddy_value,
                DEFAULT_COLLECTOR_INTERVAL,
                DEFAULT_COLLECTOR_TIMEOUT,
                initial_status(fallback.get("workbuddy", {})),
            ),
            "system": (
                collect_system_value,
                DEFAULT_COLLECTOR_INTERVAL,
                DEFAULT_COLLECTOR_TIMEOUT,
                {"status": "Loading"},
            ),
        })
    return _collector_manager


def load_status(force: bool = False) -> dict:
    values, metadata = collector_manager().snapshot(force=force, wait_seconds=COLLECTOR_WAIT_SECONDS)
    data = persisted_status()
    data.update(values)
    data["collection"] = metadata
    data["updated_at"] = datetime.now(timezone.utc).astimezone().strftime("%Y-%m-%d %H:%M")
    return data


def _workbuddy_reconnect() -> tuple[dict, HTTPStatus]:
    """Run the bundled WorkBuddy bridge setup script and report the result."""
    script = ROOT / "macos" / "start-workbuddy-monitored.sh"
    if not script.is_file():
        return (
            {"error": "WorkBuddy reconnect is unavailable in the bundled runtime"},
            HTTPStatus.NOT_IMPLEMENTED,
        )
    try:
        completed = subprocess.run(
            ["/bin/bash", str(script), "--ensure"],
            capture_output=True,
            text=True,
            timeout=50,
        )
    except subprocess.TimeoutExpired:
        collector_manager().invalidate("workbuddy")
        return (
            {"ok": False, "state": "failed", "error_code": "timeout",
             "error": "WorkBuddy reconnect timed out"},
            HTTPStatus.BAD_GATEWAY,
        )
    collector_manager().invalidate("workbuddy")
    if completed.returncode == 0:
        return {"ok": True, "state": "bridge_ready"}, HTTPStatus.OK
    error_code = "unknown"
    for line in reversed((completed.stderr or "").splitlines()):
        if line.startswith("AICC_WORKBUDDY_FAIL:"):
            error_code = line.split(":", 1)[1].strip()
            break
    detail = (completed.stderr or completed.stdout or "").strip().splitlines()
    message = detail[-1] if detail else "WorkBuddy reconnect failed"
    return (
        {"ok": False, "state": "failed", "error_code": error_code, "error": message},
        HTTPStatus.BAD_GATEWAY,
    )


def save_status(data: dict) -> bool:
    DATA_PATH.parent.mkdir(parents=True, exist_ok=True)
    with _status_write_lock:
        try:
            existing = json.loads(DATA_PATH.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            existing = None
        if existing == data:
            return False
        temporary = DATA_PATH.with_suffix(".json.tmp")
        temporary.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        os.replace(temporary, DATA_PATH)
        return True


def version() -> str:
    try:
        return (ROOT / "VERSION").read_text(encoding="utf-8").strip()
    except OSError:
        return "dev"


class DashboardHandler(SimpleHTTPRequestHandler):
    server_version = f"AICC/{version()}"
    sys_version = ""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(WEB_ROOT), **kwargs)

    def do_GET(self) -> None:
        if not allow_request(self.client_address[0], "read", 120):
            self.send_json({"error": "Too many requests"}, HTTPStatus.TOO_MANY_REQUESTS)
            return
        path = urlparse(self.path).path
        if path == "/api/health/live":
            return self.send_json({"ok": True, "status": "live", "version": version()})
        if path == "/api/health/ready":
            payload = health_payload()
            status = HTTPStatus.OK if payload["status"] != "unhealthy" else HTTPStatus.SERVICE_UNAVAILABLE
            return self.send_json(payload, status)
        if path == "/api/status":
            return self.send_json(load_status())
        if path == "/api/codex/status":
            values, _ = collector_manager().snapshot(wait_seconds=COLLECTOR_WAIT_SECONDS)
            return self.send_json(values.get("codex", {}))
        if path == "/api/health":
            return self.send_json(health_payload())
        if path == "/":
            self.path = "/index.html"
        return super().do_GET()

    def do_POST(self) -> None:
        if not allow_request(self.client_address[0], "write", 20):
            self.send_json({"error": "Too many requests"}, HTTPStatus.TOO_MANY_REQUESTS)
            return
        path = urlparse(self.path).path
        if not self.is_local_request():
            self.send_json({"error": "Write operations are local-only"}, HTTPStatus.FORBIDDEN)
            return
        if path == "/api/workbuddy/reconnect":
            payload, status = _workbuddy_reconnect()
            return self.send_json(payload, status)
        if path == "/api/refresh":
            self.send_json(load_status(force=True))
            return
        self.send_error(HTTPStatus.NOT_FOUND)

    def send_json(self, payload: dict, status: HTTPStatus = HTTPStatus.OK) -> None:
        body = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def end_headers(self) -> None:
        self.send_header("X-Content-Type-Options", "nosniff")
        self.send_header("X-Frame-Options", "DENY")
        self.send_header("Referrer-Policy", "no-referrer")
        self.send_header("Content-Security-Policy", "default-src 'self'; img-src 'self' data:; object-src 'none'; frame-ancestors 'none'")  # noqa: E501
        super().end_headers()

    def is_local_request(self) -> bool:
        return self.client_address[0] in ("127.0.0.1", "::1")

    def log_message(self, format: str, *args) -> None:
        if os.environ.get("EINK_ACCESS_LOG") == "1":
            super().log_message(format, *args)


class DashboardServer(ThreadingHTTPServer):
    allow_reuse_address = True
    daemon_threads = True


def start_discovery(http_port: int) -> None:
    discovery_port = int(os.environ.get("EINK_DISCOVERY_PORT", "8766"))

    def serve() -> None:
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as channel:
                channel.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                channel.bind(("0.0.0.0", discovery_port))
                while True:
                    request, address = channel.recvfrom(512)
                    if request.strip() != DISCOVERY_MAGIC:
                        continue
                    payload = json.dumps({"name": "AICC Dashboard", "port": http_port}).encode("utf-8")
                    channel.sendto(payload, address)
        except OSError as error:
            print(f"Discovery disabled: {error}")

    threading.Thread(target=serve, name="eink-discovery", daemon=True).start()


def _periodic_save(interval: int = 120) -> None:
    """定时将收集器数据写入 status.json，供菜单栏等文件读取方使用。"""
    while True:
        time.sleep(interval)
        try:
            values, _ = collector_manager().snapshot(force=False, wait_seconds=0)
            data = persisted_status()
            data.update(values)
            save_status(data)
        except Exception as error:
            if os.environ.get("EINK_ACCESS_LOG") == "1":
                print(f"Periodic status save failed: {type(error).__name__}: {error}")


def _workbuddy_monitor_loop(interval: int = 60) -> None:
    """Auto-heal the WorkBuddy debugging bridge once per WorkBuddy process.

    This restores the 2.3.x monitor behavior (previously a LaunchAgent) without
    needing a LaunchAgent or Automation permission: when WorkBuddy is running
    without the localhost bridge, the monitor restarts it once with the
    debugging flag. WorkBuddy is never launched when it is not running.
    """
    while True:
        time.sleep(interval)
        if os.environ.get("WORKBUDDY_AUTO_HEAL", "1") != "1":
            continue
        _run_workbuddy_monitor_once()


def _run_workbuddy_monitor_once() -> bool:
    script = ROOT / "macos" / "start-workbuddy-monitored.sh"
    if not script.is_file():
        return False
    try:
        completed = subprocess.run(
            ["/bin/bash", str(script), "--monitor"],
            capture_output=True,
            text=True,
            timeout=55,
        )
        if completed.returncode == 0 and "auto-healed" in completed.stdout:
            collector_manager().invalidate("workbuddy")
            collector_manager().snapshot(force=True, wait_seconds=0)
            print("WorkBuddy bridge auto-healed; collector refreshed.")
            return True
    except Exception as error:
        if os.environ.get("EINK_ACCESS_LOG") == "1":
            print(f"WorkBuddy monitor failed: {type(error).__name__}: {error}")
    return False


def _cache_health() -> dict:
    try:
        location = DATA_PATH if DATA_PATH.exists() else DATA_ROOT
        parent = location if location.is_dir() else location.parent
        writable = parent.exists() and os.access(parent, os.W_OK)
        modified = DATA_PATH.stat().st_mtime if DATA_PATH.exists() else None
        return {
            "writable": writable,
            "exists": DATA_PATH.exists(),
            "age_seconds": max(0, round(time.time() - modified)) if modified else None,
        }
    except OSError as error:
        return {"writable": False, "exists": False, "error": str(error)[:160]}


def _collector_health(values: dict, metadata: dict) -> dict:
    result = {}
    for name in ("codex", "workbuddy", "deepseek", "system"):
        value = values.get(name, {})
        item = metadata.get(name, {}).copy()
        collection_state = item.get("state")
        if name == "codex":
            collector_ok = bool(value.get("available"))
            state = value.get("state", "pending")
        elif name == "workbuddy":
            collector_ok = value.get("points") is not None
            state = value.get("balance_state", "pending")
            if value.get("balance_error_code"):
                item["error_code"] = value["balance_error_code"]
                item["error"] = value.get("balance_error")
        elif name == "deepseek":
            state = value.get("status", "pending")
            collector_ok = state in ("Online", "Not configured")
        else:
            state = value.get("status", "pending")
            collector_ok = state == "Online"
        item.update({"provider": name, "state": state})
        item["ok"] = (
            bool(collector_ok)
            and collection_state is not None
            and collection_state not in ("error", "timeout", "stale")
        )
        result[name] = item
    return result


def health_payload() -> dict:
    cache = _cache_health()
    try:
        manager = collector_manager()
        values, metadata = manager.snapshot(wait_seconds=0)
        provider_items = _collector_health(values, metadata)
    except Exception as error:  # health must explain a broken scheduler without raising
        return {
            "ok": False,
            "status": "unhealthy",
            "version": version(),
            "uptime_seconds": round(max(0, time.time() - SERVER_STARTED_AT)),
            "cache": cache,
            "providers": {},
            "error": f"{type(error).__name__}: {error}"[:160],
        }

    required = {"codex", "workbuddy", "system"}
    required_failed = any(
        not provider_items.get(name, {}).get("ok", False)
        for name in required
    )
    any_failed = any(not item.get("ok", False) for item in provider_items.values())
    if not cache.get("writable", False):
        state = "unhealthy"
    elif required_failed or any_failed:
        state = "degraded"
    else:
        state = "healthy"

    return {
        "ok": state == "healthy",
        "status": state,
        "version": version(),
        "uptime_seconds": round(max(0, time.time() - SERVER_STARTED_AT)),
        "cache": cache,
        "providers": provider_items,
    }


def main() -> None:
    port = int(os.environ.get("EINK_PORT", "8765"))
    server = DashboardServer((os.environ.get("EINK_HOST", "0.0.0.0"), port), DashboardHandler)
    start_discovery(port)
    collector_manager().snapshot(force=True, wait_seconds=0)
    threading.Thread(target=_periodic_save, args=(120,), daemon=True).start()
    threading.Thread(target=_workbuddy_monitor_loop, args=(60,), daemon=True).start()
    print(f"AICC Dashboard: http://localhost:{port}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped.")


if __name__ == "__main__":
    main()
