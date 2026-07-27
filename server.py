#!/usr/bin/env python3
"""Dependency-free local server for the Poke4S AI e-ink display."""

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
from collectors.workbuddy import collect as collect_workbuddy
from services.codex_monitor import monitor as codex_monitor
from services.collector_manager import CollectorManager

ROOT = Path(__file__).resolve().parent
DATA_ROOT = Path(os.environ.get("EINK_DATA_DIR", ROOT / "data")).expanduser()
DATA_PATH = DATA_ROOT / "status.json"
DEEPSEEK_HISTORY_PATH = DATA_ROOT / "deepseek_history.json"
WEB_ROOT = Path(os.environ.get("EINK_WEB_ROOT", ROOT / "web")).expanduser()
COLLECTOR_WAIT_SECONDS = max(0, min(5, float(os.environ.get("COLLECTOR_WAIT_SECONDS", "3.2"))))
MAX_POST_BYTES = 16 * 1024
mimetypes.add_type("application/manifest+json", ".webmanifest")
DEFAULT_STATUS = {
    "codex": {"five_hour": {"remaining": 83, "reset": "14:27"}, "weekly": {"remaining": 97, "reset": "7月17日"}, "source": "Manual"},
    "workbuddy": {"points": 8520, "used_points": 1480, "reset_text": "Update from WorkBuddy"},
}
DISCOVERY_MAGIC = b"AI_EINK_DISCOVER"
_collector_manager: CollectorManager | None = None
_rate_lock = threading.Lock()
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
        _collector_manager = CollectorManager({
            "codex": (codex_monitor.status, 60, fallback.get("codex", {})),
            "deepseek": (lambda: collect_deepseek(DEEPSEEK_HISTORY_PATH), 300, {"status": "Loading", "balances": []}),
            "workbuddy": (lambda: collect_workbuddy(persisted_status().get("workbuddy", {})), 60, fallback.get("workbuddy", {})),
            "system": (collect_system, 60, {"status": "Loading"}),
        })
    return _collector_manager


def load_status(force: bool = False) -> dict:
    values, metadata = collector_manager().snapshot(force=force, wait_seconds=COLLECTOR_WAIT_SECONDS)
    data = persisted_status()
    data.update(values)
    data["collection"] = metadata
    data["updated_at"] = datetime.now(timezone.utc).astimezone().strftime("%Y-%m-%d %H:%M")
    return data


def save_status(data: dict) -> None:
    DATA_PATH.parent.mkdir(parents=True, exist_ok=True)
    temporary = DATA_PATH.with_suffix(".json.tmp")
    temporary.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, DATA_PATH)


def version() -> str:
    try:
        return (ROOT / "VERSION").read_text(encoding="utf-8").strip()
    except OSError:
        return "dev"


class DashboardHandler(SimpleHTTPRequestHandler):
    server_version = "AI-EInk/2.2.2"
    sys_version = ""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(WEB_ROOT), **kwargs)

    def do_GET(self) -> None:
        if not allow_request(self.client_address[0], "read", 120):
            self.send_json({"error": "Too many requests"}, HTTPStatus.TOO_MANY_REQUESTS)
            return
        path = urlparse(self.path).path
        if path == "/api/status":
            return self.send_json(load_status())
        if path == "/api/codex/status":
            return self.send_json(codex_monitor.status())
        if path == "/api/health":
            return self.send_json({"ok": True, "version": version()})
        self.path = "/settings.html" if path == "/settings" else "/index.html" if path == "/" else self.path
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
            script = ROOT / "macos" / "start-workbuddy-monitored.sh"
            subprocess.Popen(
                ["/bin/bash", str(script), "--ensure"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
            collector_manager().invalidate("workbuddy")
            self.send_json({"ok": True, "state": "reconnecting"}, HTTPStatus.ACCEPTED)
            return
        if path == "/api/refresh":
            self.send_json(load_status(force=True))
            return
        if path != "/api/status":
            self.send_error(HTTPStatus.NOT_FOUND)
            return
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if length < 0 or length > MAX_POST_BYTES:
                self.send_json({"error": "Request body too large"}, HTTPStatus.REQUEST_ENTITY_TOO_LARGE)
                return
            payload = json.loads(self.rfile.read(length).decode("utf-8"))
            data = persisted_status()
            if isinstance(payload.get("workbuddy"), dict):
                workbuddy = payload["workbuddy"]
                clean = data.setdefault("workbuddy", {})
                for name in ("points", "used_points"):
                    if name in workbuddy:
                        clean[name] = max(0, float(workbuddy[name]))
                if "reset_text" in workbuddy:
                    clean["reset_text"] = str(workbuddy["reset_text"])[:60]
            save_status(data)
            collector_manager().invalidate("workbuddy")
            self.send_json(load_status(force=True))
        except (ValueError, UnicodeDecodeError, json.JSONDecodeError) as error:
            self.send_json({"error": str(error)}, HTTPStatus.BAD_REQUEST)

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
        self.send_header("Content-Security-Policy", "default-src 'self'; img-src 'self' data:; object-src 'none'; frame-ancestors 'none'")
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
                    payload = json.dumps({"name": "AI E-Ink Dashboard", "port": http_port}).encode("utf-8")
                    channel.sendto(payload, address)
        except OSError as error:
            print(f"Discovery disabled: {error}")

    threading.Thread(target=serve, name="eink-discovery", daemon=True).start()


def _periodic_save(interval: int = 30) -> None:
    """定时将收集器数据写入 status.json，供菜单栏等文件读取方使用。"""
    while True:
        time.sleep(interval)
        try:
            values, _ = collector_manager().snapshot(force=False, wait_seconds=0)
            data = persisted_status()
            data.update(values)
            save_status(data)
        except Exception:
            pass

def main() -> None:
    port = int(os.environ.get("EINK_PORT", "8765"))
    server = DashboardServer((os.environ.get("EINK_HOST", "0.0.0.0"), port), DashboardHandler)
    start_discovery(port)
    collector_manager().snapshot(force=True, wait_seconds=0)
    threading.Thread(target=_periodic_save, daemon=True).start()
    print(f"AI E-Ink Dashboard: http://localhost:{port}")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nStopped.")


if __name__ == "__main__":
    main()
