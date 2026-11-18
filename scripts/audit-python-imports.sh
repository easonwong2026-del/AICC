#!/bin/bash
# Report which standard-library files the bridge actually loads.
#
# The bridge ships a full CPython standard library (11 MiB of source). Knowing
# what a real run touches is the only honest basis for pruning it: guessing by
# module name is how you ship an App that breaks the first time someone pairs a
# device at 3 a.m.
#
# How it measures: the interpreter is started with a `sitecustomize` that
# installs an audit hook on the `import` event and records the resolved file of
# every module in `sys.modules` at exit. The real `server.py` runs for a few
# seconds, is interrupted with SIGINT, and the recorded set is diffed against
# the files on disk.
#
# ## Isolation (why this cannot touch the machine you are using)
#
# `server.py` is not a passive importable module: `main()` starts a poll thread
# that refreshes quotas, and it can open a LAN listener. So the child gets a
# **constructed** environment (`env -i`, whitelist only) instead of inheriting
# the caller's:
#
#   * EINK_HOST=127.0.0.1      -> `lan_enabled()` is false: no LAN TLS listener
#   * EINK_DISCOVERY=0         -> no mDNS/UDP announcement
#   * EINK_DATA_DIR/EINK_CERT_DIR under a temp dir -> no user data, no user
#     pairing certificates are read
#   * AICC_MAGPIE_URL=http://127.0.0.1:<mock port> -> quota traffic goes to a
#     local mock, never to the Magpie on 3425
#   * AICC_TOKEN_STDIN, AICC_PARENT_PID, proxies, TLS overrides: **cleared**,
#     because inheriting any of them changes which code paths run
#
# Everything cleared is cleared by `env -i`, not by a list of `unset`s that
# could go stale. After the run the script *proves* the redirection worked
# rather than asserting it: the mock must have received at least one request,
# and a canary socket on 127.0.0.1:3425 (when that port is free) must have
# received none. If either check fails, the audit fails.
#
# Usage:
#   bash scripts/audit-python-imports.sh
#   bash scripts/audit-python-imports.sh --runtime dist/mac/AICC.app/Contents/Resources/Python
#   bash scripts/audit-python-imports.sh --seconds 10 --out dist/audit
#
# Outputs: python-imports.md, python-imports.csv

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUNTIME="${AICC_RUNTIME:-$ROOT/dist/mac/AICC.app/Contents/Resources/Python}"
SERVER_DIR="${AICC_SERVER_DIR:-$ROOT/dist/mac/AICC.app/Contents/Resources/Server}"
SECONDS_TO_RUN=6
OUT="$ROOT/dist/audit"
MAGPIE_PORT=3425

usage() {
  cat <<'USAGE'
Usage: bash scripts/audit-python-imports.sh [OPTIONS]

Optional:
  --runtime <dir>   Bundled runtime (default: dist/mac/AICC.app/Contents/Resources/Python)
  --server <dir>    Directory holding server.py (default: the bundled one)
  --seconds <n>     How long to let the bridge run (default: 6)
  --out <dir>       Output directory (default: dist/audit)
  -h, --help        This message

The bridge child is fully isolated: loopback only, LAN/discovery off, temp data
and certificate directories, quota traffic redirected to a local mock. It never
talks to the Magpie on port 3425 and never reads your pairing material.
USAGE
  exit 0
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --runtime) RUNTIME="$2"; shift 2 ;;
    --server)  SERVER_DIR="$2"; shift 2 ;;
    --seconds) SECONDS_TO_RUN="$2"; shift 2 ;;
    --out)     OUT="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "Unknown argument: $1" >&2; usage ;;
  esac
done

PYTHON="$RUNTIME/bin/python3"
[[ -x "$PYTHON" ]] || { echo "ERROR: no interpreter at $PYTHON" >&2; exit 1; }
[[ -f "$SERVER_DIR/server.py" ]] || { echo "ERROR: no server.py in $SERVER_DIR" >&2; exit 1; }
mkdir -p "$OUT"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/aicc-imports.XXXXXX")"
MOCK_PID=""
CANARY_PID=""
cleanup() {
  if [[ -n "$MOCK_PID" ]] && kill -0 "$MOCK_PID" 2>/dev/null; then kill "$MOCK_PID" 2>/dev/null || true; wait "$MOCK_PID" 2>/dev/null || true; fi
  if [[ -n "$CANARY_PID" ]] && kill -0 "$CANARY_PID" 2>/dev/null; then kill "$CANARY_PID" 2>/dev/null || true; wait "$CANARY_PID" 2>/dev/null || true; fi
  rm -rf "$WORK"
}
trap cleanup EXIT

HOOK_DIR="$WORK/hook"
DATA_DIR="$WORK/data"
CERT_DIR="$WORK/certs"
HOME_DIR="$WORK/home"
TMP_DIR="$WORK/tmp"
mkdir -p "$HOOK_DIR" "$DATA_DIR" "$CERT_DIR" "$HOME_DIR" "$TMP_DIR"

# ---- mock gateway -----------------------------------------------------------

MOCK_LOG="$WORK/mock-requests.log"
: > "$MOCK_LOG"

cat > "$WORK/mock_magpie.py" <<'PY'
"""A stand-in for Magpie that records every request and never calls a vendor."""
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

LOG = os.environ["AICC_MOCK_LOG"]
BODY = json.dumps({"object": "list", "data": []}).encode()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def _record(self, method):
        with open(LOG, "a") as handle:
            handle.write("%s %s\n" % (method, self.path))

    def _reply(self):
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(BODY)))
        self.end_headers()
        self.wfile.write(BODY)

    def do_GET(self):
        self._record("GET")
        self._reply()

    def do_POST(self):
        length = int(self.headers.get("Content-Length") or 0)
        if length:
            self.rfile.read(length)
        self._record("POST")
        self._reply()

    def log_message(self, *args):
        pass  # keep the audit output clean


HTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
PY

free_port() {
  "$PYTHON" - <<'PY'
import socket
with socket.socket() as sock:
    sock.bind(("127.0.0.1", 0))
    print(sock.getsockname()[1])
PY
}

MOCK_PORT="$(free_port)"
AICC_MOCK_LOG="$MOCK_LOG" "$PYTHON" -B "$WORK/mock_magpie.py" "$MOCK_PORT" &
MOCK_PID=$!
disown "$MOCK_PID" 2>/dev/null || true

# A canary on the real Magpie port: if anything still aims at 3425, it connects
# here instead of at a user's Magpie. When Magpie is actually running the port
# is taken and the canary is reported as unavailable rather than silently
# passing.
CANARY_LOG="$WORK/canary.log"
echo "unavailable" > "$CANARY_LOG"
cat > "$WORK/canary.py" <<'PY'
"""Listen on 127.0.0.1:<port> and count connections. Writes the count on exit."""
import atexit
import os
import socket
import sys

LOG = os.environ["AICC_CANARY_LOG"]
count = 0
atexit.register(lambda: open(LOG, "w").write("%d\n" % count))
with socket.socket() as server:
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 0)
    try:
        server.bind(("127.0.0.1", int(sys.argv[1])))
    except OSError:
        open(LOG, "w").write("unavailable\n")
        sys.exit(0)
    server.listen(8)
    while True:
        try:
            conn, _ = server.accept()
        except OSError:
            break
        count += 1
        conn.close()
PY

AICC_CANARY_LOG="$CANARY_LOG" "$PYTHON" -B "$WORK/canary.py" "$MAGPIE_PORT" &
CANARY_PID=$!
disown "$CANARY_PID" 2>/dev/null || true
sleep 1

# ---- the measured child -----------------------------------------------------

cat > "$HOOK_DIR/sitecustomize.py" <<'PY'
"""Record every imported module and the file it came from."""
import atexit
import os
import sys

_loaded = set()


def _hook(event, args):
    if event == "import" and args and isinstance(args[0], str):
        _loaded.add(args[0])


def _dump():
    path = os.environ.get("AICC_IMPORT_LOG")
    if not path:
        return
    rows = []
    # Union of the audit hook's view and sys.modules: `encodings` and friends
    # are imported by the interpreter before sitecustomize runs, so the hook
    # alone would report them as untouched and prune a live dependency.
    for name in sorted(set(_loaded) | set(sys.modules)):
        module = sys.modules.get(name)
        origin = getattr(module, "__file__", None)
        if origin is None:
            continue  # builtin / frozen: no file on disk to prune
        rows.append("%s\t%s" % (name, os.path.realpath(origin)))
    with open(path, "w") as handle:
        handle.write("\n".join(rows) + "\n")


sys.addaudithook(_hook)
atexit.register(_dump)
PY

cat > "$WORK/drive.py" <<'PY'
"""Run server.py as __main__ and interrupt it after a few seconds.

An external `kill -INT` does not reliably reach the interpreter, so the timer
lives inside the child.
"""
import os
import runpy
import signal
import sys
import threading

target = os.environ["AICC_AUDIT_TARGET"]
delay = float(os.environ.get("AICC_AUDIT_SECONDS", "6"))


def _stop():
    os.kill(os.getpid(), signal.SIGINT)


threading.Timer(delay, _stop).start()
# `python3 server.py` puts the script's directory on sys.path; runpy.run_path
# does not. Without this the bridge dies on `from collectors import magpie`
# and the audit would measure a failed startup instead of a real one.
sys.path.insert(0, os.path.dirname(os.path.abspath(target)))
sys.argv = [target]
runpy.run_path(target, run_name="__main__")
PY

IMPORT_LOG="$WORK/imports.tsv"

echo "==> 采集导入表（${SECONDS_TO_RUN}s；桥接子进程用白名单环境启动）"
echo "    额度流量 → mock 127.0.0.1:$MOCK_PORT · 真实端口 canary :$MAGPIE_PORT"

# `env -i` is the whole point: nothing is inherited, so AICC_TOKEN_STDIN,
# AICC_PARENT_PID, EINK_HOST=0.0.0.0, EINK_DISCOVERY=1, *_proxy and any TLS
# overrides from the caller simply do not exist in this process.
set +e
/usr/bin/env -i \
  PATH=/usr/bin:/bin:/usr/sbin:/sbin \
  HOME="$HOME_DIR" \
  TMPDIR="$TMP_DIR" \
  EINK_PORT="$(free_port)" \
  EINK_HOST=127.0.0.1 \
  EINK_DISCOVERY=0 \
  EINK_DATA_DIR="$DATA_DIR" \
  EINK_CERT_DIR="$CERT_DIR" \
  AICC_MAGPIE_URL="http://127.0.0.1:$MOCK_PORT" \
  AICC_IMPORT_LOG="$IMPORT_LOG" \
  AICC_AUDIT_TARGET="$SERVER_DIR/server.py" \
  AICC_AUDIT_SECONDS="$SECONDS_TO_RUN" \
  PYTHONPATH="$HOOK_DIR" \
  "$PYTHON" -B "$WORK/drive.py" > "$WORK/bridge.log" 2>&1
BRIDGE_STATUS=$?
set -e

sleep 1
if [[ -n "$MOCK_PID" ]] && kill -0 "$MOCK_PID" 2>/dev/null; then kill "$MOCK_PID" 2>/dev/null || true; wait "$MOCK_PID" 2>/dev/null || true; fi
if [[ -n "$CANARY_PID" ]] && kill -0 "$CANARY_PID" 2>/dev/null; then kill "$CANARY_PID" 2>/dev/null || true; wait "$CANARY_PID" 2>/dev/null || true; fi
sleep 0.5

if [[ ! -s "$IMPORT_LOG" ]]; then
  echo "ERROR: 没有采到导入表，桥接可能启动失败。日志：" >&2
  sed -n '1,20p' "$WORK/bridge.log" >&2
  exit 1
fi

MODULE_COUNT=$(wc -l < "$IMPORT_LOG" | tr -d ' ')
MOCK_REQUESTS=$(wc -l < "$MOCK_LOG" | tr -d ' ')
MOCK_POSTS=$(grep -c '^POST' "$MOCK_LOG" 2>/dev/null || true)
MOCK_POSTS=${MOCK_POSTS:-0}
CANARY_HITS=$(head -1 "$CANARY_LOG" 2>/dev/null || true)
CANARY_HITS=${CANARY_HITS:-unavailable}
LAN_LINES=$(grep -c 'https 0\.0\.0\.0' "$WORK/bridge.log" 2>/dev/null || true)
LAN_LINES=${LAN_LINES:-0}

echo "    模块 $MODULE_COUNT 个（桥接退出码 ${BRIDGE_STATUS}）"
echo "    mock 收到 $MOCK_REQUESTS 个请求（POST ${MOCK_POSTS}）· canary 命中 $CANARY_HITS · LAN 监听行 $LAN_LINES"

# ---- isolation assertions ---------------------------------------------------
#
# These fail the run: an audit whose traffic escaped to the real Magpie, or
# that opened a LAN listener, is not evidence for anything.

if [[ "$MOCK_REQUESTS" -lt 1 ]]; then
  echo "FAIL: mock 网关一个请求都没收到 —— 无法证明额度刷新被重定向，审计作废" >&2
  sed -n '1,20p' "$WORK/bridge.log" >&2
  exit 1
fi
if [[ "$MOCK_POSTS" -lt 1 ]]; then
  echo "FAIL: mock 没有收到 POST /v1/magpie/quotas/refresh，refresh 路径没被覆盖" >&2
  exit 1
fi
if [[ "$CANARY_HITS" != "unavailable" && "$CANARY_HITS" -gt 0 ]]; then
  echo "FAIL: 有 $CANARY_HITS 个连接打到了 127.0.0.1:${MAGPIE_PORT}（真实 Magpie 端口）" >&2
  exit 1
fi
if [[ "$LAN_LINES" -gt 0 ]]; then
  echo "FAIL: 桥接日志里出现了 LAN 监听，隔离失效" >&2
  grep 'https 0.0.0.0' "$WORK/bridge.log" >&2
  exit 1
fi
echo "==> 隔离校验通过：额度流量全部落在 mock（canary ${CANARY_HITS}），无 LAN 监听"

# ---- report -----------------------------------------------------------------

LIB_DIR="$RUNTIME/lib"
[[ -d "$LIB_DIR" ]] || { echo "ERROR: 找不到标准库目录 $LIB_DIR" >&2; exit 1; }

USED_LIST="$WORK/used.txt"
awk -F'\t' '{ print $2 }' "$IMPORT_LOG" | sort -u > "$USED_LIST"

TOTAL_FILES=$(find "$LIB_DIR" -name '*.py' | wc -l | tr -d ' ')
TOTAL_KB=$(du -sk "$LIB_DIR" | awk '{print $1}')

UNUSED_TSV="$WORK/unused.tsv"
: > "$UNUSED_TSV"
while IFS= read -r file; do
  real="$(python3 -c 'import os,sys; print(os.path.realpath(sys.argv[1]))' "$file")"
  if ! grep -qxF "$real" "$USED_LIST"; then
    size=$(stat -f%z "$file")
    printf '%s\t%d\n' "${file#"$LIB_DIR"/}" "$size" >> "$UNUSED_TSV"
  fi
done < <(find "$LIB_DIR" -name '*.py' | sort)

UNUSED_COUNT=$(wc -l < "$UNUSED_TSV" | tr -d ' ')
UNUSED_KB=$(awk -F'\t' '{ s += $2 } END { printf "%d", s / 1024 }' "$UNUSED_TSV")

CSV="$OUT/python-imports.csv"
MD="$OUT/python-imports.md"

{
  echo "module,file"
  awk -F'\t' '{ printf "%s,%s\n", $1, $2 }' "$IMPORT_LOG"
} > "$CSV"

BY_DIR="$WORK/by-dir.tsv"
awk -F'\t' '
  {
    path = $1
    size = $2
    sub(/^python[0-9.]+\//, "", path)
    if (path ~ /\//) { split(path, parts, "/"); key = parts[1] "/" }
    else { key = "(python3.x 顶层 .py)" }
    s[key] += size
    c[key]++
  }
  END { for (k in s) printf "%s\t%d\t%d\n", k, s[k], c[k] }
' "$UNUSED_TSV" | sort -t$'\t' -k2,2 -rn > "$BY_DIR"

mib() { awk -v b="$1" 'BEGIN { printf "%.2f", b / 1048576 }'; }

{
  echo "# 桥接实际导入的 Python 模块"
  echo
  echo "| 项 | 值 |"
  echo "| --- | --- |"
  echo "| 采集时间 | $(date '+%Y-%m-%d %H:%M:%S %Z') |"
  echo "| 运行时 | \`$RUNTIME\` |"
  echo "| 入口 | \`$SERVER_DIR/server.py\` |"
  echo "| 运行时长 | ${SECONDS_TO_RUN}s 后 SIGINT |"
  echo "| 有磁盘文件的模块 | $MODULE_COUNT |"
  echo "| 标准库 \`.py\` 总数 | $TOTAL_FILES |"
  echo "| 标准库目录体积 | $TOTAL_KB KiB |"
  echo "| 本次未加载的 \`.py\` | $UNUSED_COUNT |"
  echo "| 其中未被加载的字节数 | $UNUSED_KB KiB（$(mib $(( UNUSED_KB * 1024 ))) MiB） |"
  echo
  echo "## 隔离校验（不是声明，是断言）"
  echo
  echo "| 项 | 实测 |"
  echo "| --- | --- |"
  echo "| 桥接子进程环境 | \`env -i\` 白名单：不继承 \`AICC_TOKEN_STDIN\`、\`AICC_PARENT_PID\`、\`EINK_HOST\`、\`EINK_DISCOVERY\`、\`*_proxy\` 与任何 TLS 覆盖 |"
  echo "| 监听地址 | 强制 \`EINK_HOST=127.0.0.1\`，日志中 LAN 监听行 $LAN_LINES 条 |"
  echo "| mDNS/发现 | 强制 \`EINK_DISCOVERY=0\` |"
  echo "| 数据目录 / 证书 | \`EINK_DATA_DIR\`、\`EINK_CERT_DIR\` 均指向临时目录，不读用户配对材料 |"
  echo "| 额度流量 | 全部落在本地 mock：$MOCK_REQUESTS 个请求（其中 POST ${MOCK_POSTS}） |"
  echo "| 真实 Magpie 端口 $MAGPIE_PORT | canary 命中 **$CANARY_HITS**（\`unavailable\` 表示该端口已被占，无法放探针） |"
  echo
  echo "mock 实际收到的请求："
  echo
  echo '```'
  sed -e 's/^/  /' "$MOCK_LOG" | head -20
  echo '```'
  echo
  echo "## 未加载的 .py 按目录汇总"
  echo
  echo "| 目录 | KiB | 文件数 |"
  echo "| --- | ---: | ---: |"
  while IFS=$'\t' read -r dir bytes count; do
    printf '| `%s` | %s | %s |\n' "$dir" "$(( bytes / 1024 ))" "$count"
  done < "$BY_DIR"
  echo
  echo "## 已加载的模块（$MODULE_COUNT 条）"
  echo
  echo '```'
  awk -F'\t' '{ print $1 }' "$IMPORT_LOG" | sort -u | paste -sd ' ' - | fold -s -w 100
  echo '```'
  echo
  echo "> 这份清单只覆盖「跑起来会加载」的模块，而且只覆盖 mock 数据下的默认路径。"
  echo "> 运行时按需导入的分支（错误处理、罕见编码、证书校验失败）不会全部出现，"
  echo "> 所以**未加载 ≠ 可删除**：删任何一个之前都要有对应的行为测试。"
} > "$MD"

echo
echo "报告：$MD"
echo "清单：$CSV"
echo "未加载：$UNUSED_COUNT 个 .py，$UNUSED_KB KiB"
