"""Read-only Magpie companion bridge. Account management belongs to Magpie."""

import hmac
import json
import os
import platform
import secrets
import socket
import sys
import threading
import time
import uuid
from collections import OrderedDict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

from collectors import magpie

ROOT = Path(__file__).resolve().parent
SERVER_STARTED_AT = time.time()
SERVER_INSTANCE_ID = str(uuid.uuid4())
DISCOVERY_MAGIC = b"AI_EINK_DISCOVER"
DATA_PATH = magpie.DATA_ROOT / "status.json"
_lock = threading.Lock()
_snapshot = None
_token = secrets.token_urlsafe(32)
_pair_code = None
_pair_deadline = 0
_pair_attempts = 0
_rates = OrderedDict()
_rate_lock = threading.Lock()


def version():
    return (ROOT / "VERSION").read_text(encoding="utf-8").strip()


def host_id():
    path = DATA_PATH.parent / "host_id"
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        with path.open("x", encoding="utf-8") as output:
            output.write(uuid.uuid4().hex)
    except FileExistsError:
        pass
    return path.read_text(encoding="utf-8").strip()


def server_identity():
    return {"protocol": "aicc", "companion": True, "version": version(), "host_id": host_id(),
            "server_instance_id": SERVER_INSTANCE_ID, "hostname": socket.gethostname(),
            "platform": "macOS" if platform.system() == "Darwin" else platform.system()}


def live_health_payload():
    payload = {**server_identity(), "ok": True, "status": "live", "pairing_required": True,
               "pid": os.getpid(), "ppid": os.getppid(), "started_at": SERVER_STARTED_AT}
    if os.environ.get("AICC_BUILD"):
        payload["build"] = os.environ["AICC_BUILD"]
    return payload


def refresh_status():
    global _snapshot
    try:
        snapshot = magpie.collect(force=True)
    except Exception:
        snapshot = magpie.load_cache() or {"source": "Magpie", "accounts": [], "stale": True, "available": False}
    with _lock:
        _snapshot = snapshot


def load_status():
    with _lock:
        snapshot = _snapshot or magpie.load_cache() or {"source": "Magpie", "accounts": [], "stale": True, "available": False}
        snapshot = json.loads(json.dumps(snapshot))
    now = time.time()
    for account in snapshot.get("accounts", []):
        read = account.get("read_at")
        account["stale"] = bool(account.get("stale") or read is None or now - read >= 300)
    accounts = snapshot.get("accounts", [])
    snapshot["stale"] = not accounts or all(account["stale"] for account in accounts)
    for account in accounts:
        account.pop("read_at", None)
    return {"codex": snapshot, "fetched_at": now, "codex_account_display_mode": "all"}


def allow_request(address, kind, limit):
    now = time.monotonic()
    key = (address, kind)
    with _rate_lock:
        start, count = _rates.get(key, (now, 0))
        if now - start >= 60:
            start, count = now, 0
        _rates[key] = (start, count + 1)
        _rates.move_to_end(key)
        while len(_rates) > 512:
            _rates.popitem(last=False)
        return count < limit


class DashboardHandler(BaseHTTPRequestHandler):
    server_version = "AICC Companion"
    sys_version = ""

    def setup(self):
        super().setup()
        self.connection.settimeout(10)

    def is_local_request(self):
        origin = self.headers.get("Origin")
        host = urlparse("//" + self.headers.get("Host", "")).hostname
        return (self.client_address[0] in ("127.0.0.1", "::1")
                and host in ("127.0.0.1", "localhost", "::1") and origin is None)

    def authorized(self):
        supplied = self.headers.get("Authorization", "")
        return hmac.compare_digest(supplied.encode(), ("Bearer " + _token).encode())

    def do_GET(self):
        if not allow_request(self.client_address[0], "read", 60):
            return self.send_json({"error": "Too many requests"}, 429)
        path = urlparse(self.path).path
        if path == "/api/health/live":
            return self.send_json(live_health_payload())
        if path == "/api/health/ready" and self.is_local_request():
            return self.send_json({"ok": True, "status": "ready"})
        if path != "/api/status":
            return self.send_json({"error": "Not found"}, 404)
        if not self.authorized():
            return self.send_json({"error": "Pair this device in AICC settings"}, 401)
        return self.send_json(load_status())

    def do_POST(self):
        global _pair_code, _pair_deadline, _pair_attempts
        path = urlparse(self.path).path
        if not allow_request(self.client_address[0], "pair", 10):
            return self.send_json({"error": "Too many requests"}, 429)
        if path == "/api/shutdown" and self.is_local_request():
            self.send_json({"ok": True, "pid": os.getpid()})
            threading.Thread(target=self.server.shutdown, daemon=True).start()
            return
        if path == "/api/pairing" and self.is_local_request():
            with _lock:
                _pair_code = f"{secrets.randbelow(100000000):08d}"
                _pair_deadline = time.monotonic() + 120
                _pair_attempts = 0
                code = _pair_code
            return self.send_json({"code": code, "expires_seconds": 120})
        if path != "/api/pair":
            return self.send_json({"error": "Read-only bridge"}, 403)
        try:
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 256:
                raise ValueError()
            document = json.loads(self.rfile.read(length))
            code = document.get("code")
            if not isinstance(code, str) or len(code) != 8 or not code.isascii() or not code.isdigit():
                raise ValueError()
        except (ValueError, TypeError, AttributeError):
            return self.send_json({"error": "Invalid pairing code"}, 400)
        with _lock:
            _pair_attempts += 1
            valid = (_pair_code is not None and time.monotonic() < _pair_deadline and _pair_attempts <= 5
                     and hmac.compare_digest(code, _pair_code))
            if valid:
                _pair_code = None
        if not valid:
            return self.send_json({"error": "Pairing code expired or incorrect"}, 403)
        return self.send_json({"token": _token, "host_id": host_id()})

    def send_json(self, payload, status=200):
        body = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        pass


class DashboardServer(ThreadingHTTPServer):
    allow_reuse_address = True
    daemon_threads = True


def start_discovery(http_port):
    def serve():
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as channel:
                channel.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                channel.bind(("0.0.0.0", int(os.environ.get("EINK_DISCOVERY_PORT", "8766"))))
                while True:
                    request, address = channel.recvfrom(512)
                    if request.strip() == DISCOVERY_MAGIC:
                        payload = {"name": "AICC Companion", "port": http_port, **server_identity()}
                        channel.sendto(json.dumps(payload).encode(), address)
        except OSError:
            pass
    threading.Thread(target=serve, daemon=True).start()


def main():
    global _token
    if os.environ.get("AICC_TOKEN_STDIN") == "1":
        supplied = sys.stdin.readline(256).strip()
        if len(supplied) < 32:
            raise SystemExit("Missing bridge credential")
        _token = supplied
    server = DashboardServer((os.environ.get("EINK_HOST", "0.0.0.0"), int(os.environ.get("EINK_PORT", "8765"))), DashboardHandler)
    start_discovery(server.server_port)

    def poll():
        while True:
            refresh_status()
            time.sleep(300)

    def watchdog():
        parent = int(os.environ["AICC_PARENT_PID"])
        while True:
            time.sleep(3)
            try:
                os.kill(parent, 0)
            except OSError:
                os._exit(0)

    threading.Thread(target=poll, daemon=True).start()
    if os.environ.get("AICC_PARENT_PID"):
        threading.Thread(target=watchdog, daemon=True).start()
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
