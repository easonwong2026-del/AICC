#!/bin/bash
set -euo pipefail
SERVER_DIR="${1:-dist/mac/AICC.app/Contents/Resources/Server}"
test -f "$SERVER_DIR/server.py"
test -f "$SERVER_DIR/collectors/magpie.py"
test ! -e "$SERVER_DIR/collectors/workbuddy.py"
test ! -e "$SERVER_DIR/services/codex_monitor.py"
python3 - "$SERVER_DIR" <<'PY'
import http.client
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import tempfile
import time

root = Path(sys.argv[1]).resolve()
with socket.socket() as listener:
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
with tempfile.TemporaryDirectory() as directory:
    token = secrets.token_urlsafe(32)
    env = dict(os.environ, EINK_HOST="127.0.0.1", EINK_PORT=str(port), EINK_DATA_DIR=directory,
               AICC_TOKEN_STDIN="1", AICC_MAGPIE_URL="http://127.0.0.1:1", EINK_DISCOVERY_PORT="0")
    process = subprocess.Popen([sys.executable, "-B", str(root / "server.py")], stdin=subprocess.PIPE,
                               stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, env=env)
    try:
        process.stdin.write((token + "\n").encode())
        process.stdin.close()
        for _ in range(50):
            try:
                connection = http.client.HTTPConnection("127.0.0.1", port, timeout=1)
                connection.request("GET", "/api/health/live")
                response = connection.getresponse()
                assert json.loads(response.read())["companion"] is True
                connection.close()
                break
            except OSError:
                time.sleep(.1)
        else:
            raise AssertionError("Bridge did not start")
        for route, headers, expected in [
            ("/api/status", {}, 401),
            ("/api/status", {"Authorization": "Bearer " + token}, 200),
            ("/api/health/ready", {}, 200),
            ("/", {}, 404)]:
            connection = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
            connection.request("GET", route, headers=headers)
            response = connection.getresponse()
            assert response.status == expected, route
            payload = response.read()
            assert token.encode() not in payload
            connection.close()
        print("Bundled bridge: stdin credential, authentication, offline Magpie and no dashboard passed")
    finally:
        process.terminate()
        process.wait(timeout=5)
PY
