"""Local HTTPS endpoint for the POKE4S TLS probe (not shipped).

Serves the two things the probe checks, over TLS:

  * ``/api/health/live`` -> 200 with a small JSON body
  * anything else        -> 401 (the bridge requires a bearer token there)

Reachable from the device through ``adb reverse tcp:8767 tcp:8767``, so the
Mac never has to listen on a LAN interface for this test.

Usage::

    python3 android/tls-probe/server.py            # needs certs/server.pem + key
    python3 android/tls-probe/server.py --port 8767

Python 3.10+ is required for TLS 1.3 (the system Python 3.9 that ships with the
Command Line Tools links LibreSSL and cannot speak it, which is itself relevant
to the PR-B decision: the App hosts the bridge with that interpreter).
"""

import argparse
import json
import ssl
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_CERT_DIR = HERE / "certs"

HEALTH = {
    "protocol": "aicc",
    "companion": True,
    "status": "live",
    "bind": "loopback",
    "status_schema": 3,
    "probe_server": True,
}


class Handler(BaseHTTPRequestHandler):
    server_version = "AICC TLS Probe"
    sys_version = ""

    def do_GET(self):
        path = self.path.split("?", 1)[0]
        if path == "/api/health/live":
            self._json(200, HEALTH)
        elif path == "/api/status":
            if self.headers.get("Authorization", "").startswith("Bearer "):
                self._json(200, {"accounts": [], "probe_server": True})
            else:
                self._json(401, {"error": "Pair this device in AICC settings"})
        else:
            self._json(404, {"error": "Not found"})

    def _json(self, status, payload):
        body = json.dumps(payload, ensure_ascii=False).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)
        print(f"[tls-probe] {self.client_address[0]} {self.path} -> {status} "
              f"tls={self.request_version}/{self.connection.version()}", flush=True)

    def log_message(self, *args):
        pass


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8767)
    # Two instances: certs/ is signed by the CA the probe trusts, certs/ca2/ is
    # signed by a CA it must reject. Running both makes the negative case
    # airtight instead of relying on what the device's trust store happens to hold.
    parser.add_argument("--cert-dir", default=str(DEFAULT_CERT_DIR))
    args = parser.parse_args()

    cert_dir = Path(args.cert_dir)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.minimum_version = ssl.TLSVersion.TLSv1_2
    context.load_cert_chain(certfile=str(cert_dir / "server.pem"),
                            keyfile=str(cert_dir / "server.key"))

    server = ThreadingHTTPServer((args.host, args.port), Handler)
    server.socket = context.wrap_socket(server.socket, server_side=True)
    print(f"[tls-probe] listening on https://{args.host}:{args.port} certs={cert_dir}",
          flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
