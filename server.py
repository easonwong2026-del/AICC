"""Read-only Magpie companion bridge. Account management belongs to Magpie.

The bridge serves one generic quota snapshot (`schema_version: 3`): every
provider Magpie reports, every account, every window, plan credit and balance.
A pinned `format=2` view keeps the pre-3.0 `codex` object available for older
readers; it is derived from the same snapshot, never from a second code path.
"""

import hmac
import json
import os
import platform
import secrets
import socket
import ssl
import sys
import threading
import time
import uuid
from collections import OrderedDict
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from collectors import magpie

import bridge_tls

ROOT = Path(__file__).resolve().parent
SERVER_STARTED_AT = time.time()
SERVER_INSTANCE_ID = str(uuid.uuid4())
DISCOVERY_MAGIC = b"AI_EINK_DISCOVER"
DATA_PATH = magpie.DATA_ROOT / "status.json"
READ_LIMIT_PER_MINUTE = 60
PAIR_LIMIT_PER_MINUTE = 10
# 配对失败按**来源 IP** 计数：别的设备猜错 5 次不该让合法设备的码作废。
PAIR_FAILURES_PER_WINDOW = 5
PAIR_FAILURE_WINDOW_SECONDS = 120
MAX_PAIR_FAILURE_ENTRIES = 512
# 默认只听回环：局域网访问必须由用户在设置里明确打开。
DEFAULT_HOST = "127.0.0.1"
LAN_HOST = "0.0.0.0"
EMPTY_SNAPSHOT = {"schema_version": magpie.SCHEMA_VERSION, "source": "Magpie", "available": False,
                  "state": "Empty", "stale": True, "providers": [], "accounts": [],
                  "updated_at": None, "updated_epoch": None, "refresh_seconds": magpie.REFRESH_SECONDS}
_lock = threading.Lock()
_snapshot = None
_token = secrets.token_urlsafe(32)
_pair_code = None
_pair_deadline = 0
_pair_failures = OrderedDict()
_rates = OrderedDict()
_rate_lock = threading.Lock()
# 局域网 HTTPS 真的起来之后才有内容（端口、指纹、证书到期时间）；
# 没起来就是 None，`bind` 与 `lan_serving` 都以此为准，不看环境变量。
_lan_info = None


def version():
    return (ROOT / "VERSION").read_text(encoding="utf-8").strip()


def host_id():
    """Stable identity of this Mac, created once and never rewritten.

    PR-B pairs devices against this id and a certificate, so a second value is
    worse than a wrong one: two ids mean two devices believe they paired with
    two different Macs.  Creation therefore happens under an advisory lock, and
    the file is replaced atomically — a reader can never see a half-written id,
    and a wedged empty file gets healed instead of being overwritten by every
    caller in turn.
    """
    path = DATA_PATH.parent / "host_id"
    path.parent.mkdir(parents=True, exist_ok=True)
    try:
        guard = _host_id_lock(path)
    except OSError:
        guard = None
    try:
        try:
            value = path.read_text(encoding="utf-8").strip()
        except (FileNotFoundError, OSError):
            value = ""
        if value:
            return value
        value = uuid.uuid4().hex
        temporary = path.with_name("host_id.tmp")
        try:
            with temporary.open("w", encoding="utf-8") as output:
                output.write(value)
                output.flush()
                os.fsync(output.fileno())
            os.replace(temporary, path)
        except OSError:
            return value
        return value
    finally:
        if guard is not None:
            guard.close()


def _host_id_lock(path):
    """Cross-process lock file; `close()` releases it."""
    import fcntl

    handle = os.open(str(path.with_name("host_id.lock")), os.O_CREAT | os.O_RDWR, 0o600)
    fcntl.flock(handle, fcntl.LOCK_EX)
    return _FdGuard(handle)


class _FdGuard:
    def __init__(self, descriptor):
        self.descriptor = descriptor

    def close(self):
        import fcntl

        try:
            fcntl.flock(self.descriptor, fcntl.LOCK_UN)
        finally:
            os.close(self.descriptor)


def requested_host():
    """`EINK_HOST` 只认两个值：回环，或明确的局域网通配。

    拼错、留空、写成主机名，一律按**回环**处理 —— 宁可连不上，
    也不要因为一个笔误把额度数据绑到意料之外的网卡上。
    """
    return LAN_HOST if (os.environ.get("EINK_HOST") or "").strip() == LAN_HOST else DEFAULT_HOST


def lan_enabled():
    return requested_host() == LAN_HOST


def discovery_enabled():
    """设备发现只在"允许局域网"时才有意义，也必须显式打开。"""
    return lan_enabled() and os.environ.get("EINK_DISCOVERY") == "1"


def _pair_window(address):
    now = time.monotonic()
    start, count = _pair_failures.get(address, (now, 0))
    if now - start >= PAIR_FAILURE_WINDOW_SECONDS:
        start, count = now, 0
    return start, count


def pair_blocked(address):
    """这个来源 IP 在本窗口内的失败次数是否已经用完。

    判据是**来源**：别的设备猜错不该让合法设备的配对码作废 —— 那是一个
    局域网内任何人都能触发的拒绝服务。
    """
    with _lock:
        start, count = _pair_window(address)
        _pair_failures[address] = (start, count)
        _pair_failures.move_to_end(address)
        while len(_pair_failures) > MAX_PAIR_FAILURE_ENTRIES:
            _pair_failures.popitem(last=False)
        return count >= PAIR_FAILURES_PER_WINDOW


def note_pair_failure(address):
    with _lock:
        start, count = _pair_window(address)
        _pair_failures[address] = (start, count + 1)
        _pair_failures.move_to_end(address)


def clear_pair_failures(address):
    with _lock:
        _pair_failures.pop(address, None)


def server_identity():
    identity = {"protocol": "aicc", "companion": True, "version": version(), "host_id": host_id(),
                "server_instance_id": SERVER_INSTANCE_ID, "hostname": socket.gethostname(),
                "platform": "macOS" if platform.system() == "Darwin" else platform.system(),
                # 监听范围要能被宿主校验：升级后如果旧进程还绑在 0.0.0.0，
                # App 只看 version/build 会认它"兼容"而继续复用，局域网暴露就留在原地。
                "bind": "lan" if lan_enabled() else "loopback",
                # bind 说"想不想"，lan_serving 说"成没成"：证书缺失时两者不一致，
                # 界面必须能把这件事说出来，而不是显示已开启却连不上。
                "lan_serving": _lan_info is not None,
                "protocol_version": bridge_tls.PROTOCOL_VERSION,
                "status_schema": magpie.SCHEMA_VERSION}
    if _lan_info is not None:
        identity["https"] = _lan_info
    return identity


def live_health_payload():
    payload = {**server_identity(), "ok": True, "status": "live", "pairing_required": True,
               "pid": os.getpid(), "ppid": os.getppid(), "started_at": SERVER_STARTED_AT}
    if os.environ.get("AICC_BUILD"):
        payload["build"] = os.environ["AICC_BUILD"]
    return payload


def refresh_status(force=True):
    """Read Magpie once.  One provider failing must not blank the others."""
    global _snapshot
    try:
        snapshot = magpie.collect(force=force)
    except Exception as error:
        # The collector already logged stage and error type; say once more that
        # we are serving cache, so an empty card can be traced to a failed read.
        print(f"[bridge] serving cache after {type(error).__name__}: {error}", file=sys.stderr)
        snapshot = magpie.load_cache() or dict(EMPTY_SNAPSHOT)
    with _lock:
        _snapshot = snapshot
    return snapshot


def snapshot_copy():
    with _lock:
        snapshot = _snapshot or magpie.load_cache() or dict(EMPTY_SNAPSHOT)
    return json.loads(json.dumps(snapshot))


def age_accounts(snapshot, now=None):
    """Recompute freshness for the moment of reading, per account."""
    now = time.time() if now is None else now
    accounts = snapshot.get("accounts", [])
    for account in accounts:
        read = account.get("read_at")
        if read is None or now - read >= magpie.MAX_AGE_SECONDS or read > now + 60:
            account["stale"] = True
    # A snapshot the collector already marked stale (an error, a deferred read,
    # or a cache fallback) stays stale: freshness is only ever added, never
    # claimed back from a failed read.
    snapshot["stale"] = (not accounts or bool(snapshot.get("stale"))
                         or all(account["stale"] for account in accounts))
    snapshot["state"] = "Empty" if not accounts else ("Cached" if snapshot["stale"] else "Connected")
    return snapshot


_warned_cards = set()


def _warn_bad_card(card_id):
    """每张坏卡片只吼一次，不刷日志（面板每 60 秒就轮询一次）。"""
    key = str(card_id)[:32]
    if key in _warned_cards:
        return
    if len(_warned_cards) < 64:
        _warned_cards.add(key)
    print("[bridge] skipping a cached account without id/label", file=sys.stderr)


def legacy_view(snapshot):
    """The pre-3.0 `codex` object, built from the generic cards.

    Only local Codex subscriptions have a place in it; every other provider,
    plan and balance is dropped, which is why new readers ask for schema 3.

    Cards come from a cache file that an older build (or a hand edit) may have
    written, so a card without `id` / `account_label` must be skipped, not
    raise: a KeyError here turns `/api/status?format=2` into a 500 and the
    e-ink panel shows an error instead of "no data".
    """
    windows = {"7 days": ("weekly", 10080), "5 hours": ("five_hour", 300)}
    accounts = []
    for card in snapshot.get("accounts", []):
        if card.get("provider") != "codex" or card.get("kind") != "subscription" or card.get("from"):
            continue
        card_id = card.get("id")
        label = card.get("account_label")
        if not isinstance(card_id, str) or not card_id or not isinstance(label, str) or not label:
            _warn_bad_card(card_id)
            continue
        entry = {"id": card_id, "label": f"Codex · {label}", "plan": card.get("plan"),
                 "active": False, "needs_reauth": False, "stale": card.get("stale", True),
                 "read_at": card.get("read_at")}
        for item in card.get("items", []) or []:
            if not isinstance(item, dict):
                continue
            mapped = windows.get(item.get("name"))
            if mapped is None:
                continue
            entry[mapped[0]] = {"remaining": item.get("remaining"), "duration_minutes": mapped[1],
                                "reset": item.get("reset")}
        if not any(key in entry for key in ("weekly", "five_hour")):
            continue
        accounts.append(entry)
    codex = {"schema_version": 2, "source": "Magpie", "available": bool(accounts),
             "state": snapshot.get("state") if accounts else "No Codex accounts",
             "stale": snapshot.get("stale", True), "accounts": accounts}
    if len(accounts) == 1:
        codex["weekly"] = accounts[0].get("weekly")
        codex["five_hour"] = accounts[0].get("five_hour")
    return {"codex": codex, "fetched_at": time.time(), "codex_account_display_mode": "all"}


def requested_schema(handler, query):
    """Explicit version negotiation: `format` query, else header, else 3."""
    for candidate in (query.get("format", [None])[0], handler.headers.get("X-AICC-Schema")):
        if candidate == "2":
            return 2
        if candidate == "3":
            return 3
    return magpie.SCHEMA_VERSION


def load_status(schema=magpie.SCHEMA_VERSION):
    snapshot = age_accounts(snapshot_copy())
    if schema == 2:
        return legacy_view(snapshot)
    return {**snapshot, "fetched_at": time.time()}


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
        if not allow_request(self.client_address[0], "read", READ_LIMIT_PER_MINUTE):
            return self.send_json({"error": "Too many requests"}, 429)
        parsed = urlparse(self.path)
        path = parsed.path
        if path == "/api/health/live":
            return self.send_json(live_health_payload())
        if path == "/api/health/ready" and self.is_local_request():
            return self.send_json({"ok": True, "status": "ready"})
        if path != "/api/status":
            return self.send_json({"error": "Not found"}, 404)
        if not self.authorized():
            return self.send_json({"error": "Pair this device in AICC settings"}, 401)
        query = parse_qs(parsed.query)
        schema = requested_schema(self, query)
        return self.send_json(load_status(schema), headers={"X-AICC-Schema": str(schema)})

    def do_POST(self):
        global _pair_code, _pair_deadline
        path = urlparse(self.path).path
        address = self.client_address[0]
        if not allow_request(address, "pair", PAIR_LIMIT_PER_MINUTE):
            return self.send_json({"error": "Too many requests"}, 429)
        if path == "/api/shutdown" and self.is_local_request():
            self.send_json({"ok": True, "pid": os.getpid()})
            threading.Thread(target=self.server.shutdown, daemon=True).start()
            return
        if path == "/api/pairing" and self.is_local_request():
            with _lock:
                _pair_code = f"{secrets.randbelow(100000000):08d}"
                _pair_deadline = time.monotonic() + 120
                # 新码 = 新一轮：旧码积累的失败计数不该继续拖累新码，
                # 否则用户被锁住之后连"重新生成"都救不回来。
                _pair_failures.clear()
                code = _pair_code
            return self.send_json({"code": code, "expires_seconds": 120})
        if path != "/api/pair":
            # Network devices never reach Magpie's refresh, account or routing APIs.
            return self.send_json({"error": "Read-only bridge"}, 403)
        return attempt_pairing(self)

    def send_json(self, payload, status=200, headers=None):
        body = json.dumps(payload, ensure_ascii=False, separators=(",", ":")).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("X-Content-Type-Options", "nosniff")
        for name, value in (headers or {}).items():
            self.send_header(name, value)
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        pass


class DashboardServer(ThreadingHTTPServer):
    allow_reuse_address = True
    daemon_threads = True


def attempt_pairing(handler):
    """POST /api/pair, shared by the loopback and the LAN listener.

    The attempt counter is keyed on the source address either way, so a device
    on the LAN cannot burn through the code any faster than one on the host.
    """
    global _pair_code
    address = handler.client_address[0]
    try:
        length = int(handler.headers.get("Content-Length", "0"))
        if not 0 < length <= 256:
            raise ValueError()
        document = json.loads(handler.rfile.read(length))
        code = document.get("code")
        if not isinstance(code, str) or len(code) != 8 or not code.isascii() or not code.isdigit():
            raise ValueError()
    except (ValueError, TypeError, AttributeError):
        return handler.send_json({"error": "Invalid pairing code"}, 400)
    if pair_blocked(address):
        return handler.send_json({"error": "Too many attempts, wait a moment"}, 429)
    with _lock:
        valid = (_pair_code is not None and time.monotonic() < _pair_deadline
                 and hmac.compare_digest(code, _pair_code))
        if valid:
            _pair_code = None
    if not valid:
        note_pair_failure(address)
        return handler.send_json({"error": "Pairing code expired or incorrect"}, 403)
    clear_pair_failures(address)
    granted = {"token": _token, "host_id": host_id(), "protocol_version": bridge_tls.PROTOCOL_VERSION}
    # 设备要能验证之后的每一次连接，就需要一个信任锚；自签根不在任何系统信任库里，
    # 所以配对响应把根证书一起给它。这里的信任仍然来自用户人工核对过的指纹。
    try:
        granted.update(bridge_tls.pairing_material())
    except bridge_tls.TlsUnavailable:
        pass
    return handler.send_json(granted)


class LanHandler(DashboardHandler):
    """What a network device may reach: health, pairing, read-only status.

    This class deliberately lists its routes instead of inheriting the host's
    routing table: `/api/pairing`, `/api/shutdown` and `/api/health/ready` are
    loopback-only by design, and copying every HTTP route onto HTTPS is exactly
    how an account-management endpoint ends up exposed to the whole subnet.
    """

    def is_local_request(self):
        return False

    def do_GET(self):
        if not allow_request(self.client_address[0], "read", READ_LIMIT_PER_MINUTE):
            return self.send_json({"error": "Too many requests"}, 429)
        parsed = urlparse(self.path)
        if parsed.path == "/api/health/live":
            return self.send_json(live_health_payload())
        if parsed.path != "/api/status":
            return self.send_json({"error": "Not available over LAN"}, 403)
        if not self.authorized():
            return self.send_json({"error": "Pair this device in AICC settings"}, 401)
        schema = requested_schema(self, parse_qs(parsed.query))
        return self.send_json(load_status(schema), headers={"X-AICC-Schema": str(schema)})

    def do_POST(self):
        path = urlparse(self.path).path
        if not allow_request(self.client_address[0], "pair", PAIR_LIMIT_PER_MINUTE):
            return self.send_json({"error": "Too many requests"}, 429)
        if path != "/api/pair":
            return self.send_json({"error": "Read-only bridge"}, 403)
        return attempt_pairing(self)


class LanServer(ThreadingHTTPServer):
    """HTTPS-only listener; the socket is wrapped by bridge_tls, not here."""
    allow_reuse_address = True
    daemon_threads = True

    def handle_error(self, request, client_address):
        # A device that aborts mid-handshake (wrong pin, plaintext probe, sleep)
        # is an expected event on a network listener, not a server fault.
        error = sys.exc_info()[1]
        if isinstance(error, (ssl.SSLError, ConnectionResetError, BrokenPipeError, OSError)):
            return
        super().handle_error(request, client_address)


def start_discovery(https_port):
    """UDP 发现只在「允许局域网 + 显式打开发现」时才跑。

    以前它无条件绑 `0.0.0.0:8766`，等于桥接开关关着也在局域网里应答；
    现在关掉局域网访问就连发现一起停，端口真正收回去。

    广播本身没有认证能力，所以它只负责"这里有个服务器"，附带的 SPKI 指纹
    只是**给人看**的候选值：设备必须让用户把它和 Mac 设置页上的完整指纹
    对一遍才能建立信任，绝不能因为广播里带着指纹就自动接受（那是把信任
    建立在一个谁都能伪造的 UDP 包上）。
    """
    if not discovery_enabled():
        return False

    def serve():
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as channel:
                channel.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                channel.bind((LAN_HOST, int(os.environ.get("EINK_DISCOVERY_PORT", "8766"))))
                while True:
                    request, address = channel.recvfrom(512)
                    if request.strip() == DISCOVERY_MAGIC:
                        # 广播要小：设备侧一个 512 字节的包就该装得下。整份 identity
                        # 带上 sans / TLS 诊断后会超过 MTU 上限，设备解析直接失败。
                        identity = server_identity()
                        payload = {
                            "name": "AICC Companion",
                            "port": https_port,
                            "protocol": identity["protocol"],
                            "protocol_version": identity["protocol_version"],
                            "host_id": identity["host_id"],
                            "hostname": identity["hostname"],
                            "platform": identity["platform"],
                            "version": identity["version"],
                        }
                        if _lan_info is not None:
                            payload["https"] = {
                                "port": https_port,
                                "protocol_version": _lan_info["protocol_version"],
                                "spki": _lan_info["spki"],
                            }
                        channel.sendto(json.dumps(payload).encode(), address)
        except OSError:
            pass
    threading.Thread(target=serve, daemon=True).start()
    return True


def lan_bootstrap():
    """Load certificate material for the LAN listener.  Never raises.

    Returning `(None, None)` is the fail-closed path: the caller then serves
    loopback only.  There is deliberately no "continue in plaintext" branch.
    """
    global _lan_info
    if not lan_enabled():
        return None, None
    try:
        material = bridge_tls.load_identity()
        context = bridge_tls.build_context(material)
    except bridge_tls.TlsUnavailable as error:
        # 失败必须"关"，不能退化成明文：用户以为开了局域网，
        # 实际把额度数据摊在网段上，比连不上危险得多。
        print(f"[bridge] LAN requested but TLS unavailable, keeping loopback only: {error}", file=sys.stderr)
        return None, None
    _lan_info = bridge_tls.summary(material, context)
    print(f"[bridge] https {LAN_HOST}:{bridge_tls.LAN_PORT} "
          f"{json.dumps(bridge_tls.diagnostics(context), sort_keys=True)}", file=sys.stderr)
    return material, context


def main():
    global _token
    if os.environ.get("AICC_TOKEN_STDIN") == "1":
        supplied = sys.stdin.readline(256).strip()
        if len(supplied) < 32:
            raise SystemExit("Missing bridge credential")
        _token = supplied

    # 本机管理接口永远是回环上的明文 HTTP：它只对本机进程有意义，
    # 公网/局域网都不该看到它，也不需要证书。
    loopback = DashboardServer((DEFAULT_HOST, int(os.environ.get("EINK_PORT", "8765"))), DashboardHandler)
    lan_server = None

    material, context = lan_bootstrap()
    if context is not None:
        lan_server = LanServer((LAN_HOST, bridge_tls.LAN_PORT), LanHandler)
        lan_server.socket = context.wrap_socket(lan_server.socket, server_side=True)
        threading.Thread(target=lan_server.serve_forever, daemon=True).start()
        start_discovery(bridge_tls.LAN_PORT)

    def poll():
        while True:
            refresh_status()
            time.sleep(magpie.REFRESH_SECONDS)

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
        loopback.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        loopback.server_close()
        if lan_server is not None:
            lan_server.shutdown()
            lan_server.server_close()


if __name__ == "__main__":
    main()
