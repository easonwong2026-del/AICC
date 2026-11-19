#!/bin/bash
# Isolation test for scripts/audit-python-imports.sh.
#
# Claim under test: the audit runs the real bridge but reaches **no** real
# endpoint — no Magpie on 3425, no LAN listener, no inherited credentials or
# parent-pid watchdog.
#
# How it is proven rather than asserted:
#
#   1. The caller's shell is poisoned with exactly the variables that would
#      break isolation (EINK_HOST=0.0.0.0, EINK_DISCOVERY=1, a real-looking
#      AICC_MAGPIE_URL, AICC_TOKEN_STDIN, AICC_PARENT_PID, proxies). If the
#      child inherited any of them, the canary or the LAN check would trip.
#   2. A canary socket listens on 127.0.0.1:3425 and counts connections.
#   3. The audit's mock gateway must receive the refresh POST, which proves the
#      quota traffic went there and not to the canary.
#
#   bash scripts/test-audit-python-imports.sh
#
# Skips the canary half (and says so) when something already occupies 3425.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
AUDIT="$ROOT/scripts/audit-python-imports.sh"
RUNTIME="${AICC_RUNTIME:-$ROOT/dist/mac/AICC.app/Contents/Resources/Python}"

[[ -x "$RUNTIME/bin/python3" ]] || { echo "SKIP: 没有内嵌运行时（${RUNTIME}），先构建一次 macOS app" >&2; exit 0; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/aicc-imports-test.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

CANARY="$ROOT/scripts/canary_loopback_port.py"
PYTHON_BIN="$RUNTIME/bin/python3"

free_port() {
  "$PYTHON_BIN" -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()'
}

echo "==> canary 计数机制（三种状态）"

# 1. idle: bound, nobody connects -> 0, written without any exit hook
PORT_IDLE="$(free_port)"
LOG_IDLE="$WORK/canary-idle.log"
"$PYTHON_BIN" -B "$CANARY" "$PORT_IDLE" "$LOG_IDLE" &
CANARY_IDLE_PID=$!
sleep 0.5
IDLE_VALUE="$(head -1 "$LOG_IDLE" 2>/dev/null || true)"
if [[ "$IDLE_VALUE" == "0" ]]; then
  echo "ok   canary 空闲：立即写出 0（不依赖 atexit）"
else
  echo "FAIL canary 空闲时应为 0，实际 '$IDLE_VALUE'" >&2
  FAIL_CANARY=1
fi

# 2. one deliberate connection -> 1, updated per accept
"$PYTHON_BIN" -c 'import socket,sys; socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=3).close()' "$PORT_IDLE"
sleep 0.4
BUSY_VALUE="$(head -1 "$LOG_IDLE" 2>/dev/null || true)"
if [[ "$BUSY_VALUE" == "1" ]]; then
  echo "ok   canary 计数：故意连接一次后为 1"
else
  echo "FAIL canary 应记录 1，实际 '$BUSY_VALUE'" >&2
  FAIL_CANARY=1
fi
kill "$CANARY_IDLE_PID" 2>/dev/null || true
wait "$CANARY_IDLE_PID" 2>/dev/null || true

# 3. occupied port -> unavailable, never a fake 0
"$PYTHON_BIN" - "$WORK" <<'PYHOLD' &
import socket, sys, time
server = socket.socket()
server.bind(("127.0.0.1", 0))
server.listen(1)
with open(sys.argv[1] + "/occupied-port", "w") as handle:
    handle.write(str(server.getsockname()[1]))
time.sleep(30)
PYHOLD
HOLDER_PID=$!
for _ in $(seq 1 50); do [[ -s "$WORK/occupied-port" ]] && break; sleep 0.1; done
PORT_TAKEN="$(cat "$WORK/occupied-port" 2>/dev/null || echo "")"
LOG_TAKEN="$WORK/canary-taken.log"
"$PYTHON_BIN" -B "$CANARY" "$PORT_TAKEN" "$LOG_TAKEN"
TAKEN_VALUE="$(head -1 "$LOG_TAKEN" 2>/dev/null || true)"
if [[ "$TAKEN_VALUE" == "unavailable" ]]; then
  echo "ok   canary 端口被占用：明确写 unavailable（不伪造 0）"
else
  echo "FAIL 端口被占时应为 unavailable，实际 '$TAKEN_VALUE'" >&2
  FAIL_CANARY=1
fi
kill "$HOLDER_PID" 2>/dev/null || true
wait "$HOLDER_PID" 2>/dev/null || true

# 4. a hit on the canary must fail the audit
echo "==> 故意打一次 canary，审计必须失败"
PORT_HIT="$(free_port)"
set +e
AICC_AUDIT_TEST_PROBE_CANARY=1 AICC_AUDIT_TEST_CANARY_PORT="$PORT_HIT" \
  bash "$AUDIT" --seconds 4 --out "$WORK/out-hit" > "$WORK/hit.log" 2>&1
HIT_STATUS=$?
set -e
if [[ "$HIT_STATUS" -ne 0 ]] && grep -q '个连接打到了' "$WORK/hit.log"; then
  echo "ok   canary 命中 1 次时审计非零退出"
else
  echo "FAIL canary 命中时审计应失败（status=${HIT_STATUS}）" >&2
  tail -5 "$WORK/hit.log" >&2
  FAIL_CANARY=1
fi

echo
echo "==> 用「被污染」的环境跑审计"
EINK_HOST=0.0.0.0 \
EINK_DISCOVERY=1 \
AICC_MAGPIE_URL=http://127.0.0.1:3425 \
AICC_TOKEN_STDIN=1 \
AICC_PARENT_PID=1 \
AICC_ACCOUNT_DATA_DIR=/tmp/should-not-be-used \
http_proxy=http://127.0.0.1:9 \
https_proxy=http://127.0.0.1:9 \
  bash "$AUDIT" --seconds 6 --out "$WORK/out" > "$WORK/audit.log" 2>&1 || {
    echo "FAIL: 审计本身失败" >&2
    tail -20 "$WORK/audit.log" >&2
    exit 1
  }

REPORT="$WORK/out/python-imports.md"
[[ -f "$REPORT" ]] || { echo "FAIL: 没有生成报告" >&2; exit 1; }

FAIL=${FAIL_CANARY:-0}
check() {  # check <描述> <命令...>
  local what="$1"; shift
  if "$@"; then
    echo "ok   $what"
  else
    echo "FAIL $what" >&2
    FAIL=$(( FAIL + 1 ))
  fi
}

check "mock 收到了 refresh POST（额度流量被重定向）" \
  grep -q 'POST /v1/magpie/quotas/refresh' "$REPORT"
check "真实 Magpie 端口 canary 命中 0（或端口被占时明确标 unavailable）" \
  grep -qE 'canary 命中 \*\*(0|unavailable)\*\*' "$REPORT"
check "没有 LAN 监听（EINK_HOST=0.0.0.0 未被继承）" \
  grep -q 'LAN 监听行 0 条' "$REPORT"
check "报告声明了 env -i 白名单而不是笼统的「不联网」" \
  grep -q 'env -i' "$REPORT"

# No leftovers: the mock/canary/driver must all be gone. If `ps` itself is
# unavailable (sandboxed shells cannot always enumerate processes), say so
# instead of silently passing.
if PS_SNAPSHOT="$(ps -axo pid=,command= 2>/dev/null)"; then
  LEFTOVERS="$(printf '%s\n' "$PS_SNAPSHOT" | grep -F "$WORK" | grep -v grep || true)"
  check "没有遗留子进程" test -z "$LEFTOVERS"
else
  echo "SKIP 无法枚举进程（ps 不可用），残留检查未执行"
fi

if grep -q 'canary 命中 \*\*unavailable\*\*' "$REPORT"; then
  echo "NOTE: 3425 已被占用（可能 Magpie 在运行），canary 无法放置探针；已退化为「流量全部落到 mock」的单侧证据" >&2
fi

echo
if [[ "$FAIL" -gt 0 ]]; then
  echo "FAILED: $FAIL 项失败" >&2
  exit 1
fi
echo "PASS: 隔离校验全部通过"
