#!/bin/bash
# Smoke test for bundled AICC server.
# Runs server.py on an isolated port, checks key endpoints,
# then cleans up. Intended for CI and pre-release validation.
set -euo pipefail

PORT="${AICC_SMOKE_PORT:-18765}"
DISCOVERY_PORT="${AICC_SMOKE_DISCOVERY_PORT:-18766}"
BASE_URL="http://127.0.0.1:$PORT"
SERVER_DIR="${1:-dist/mac/AICC.app/Contents/Resources/Server}"
TIMEOUT_SEC=30
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-smoke.XXXXXX")"
LOG_FILE="$TEMP_ROOT/server.log"
PID=""

cleanup() {
  local rc=$?
  trap - EXIT INT TERM
  if [[ -n "$PID" ]] && kill -0 "$PID" 2>/dev/null; then
    kill "$PID" 2>/dev/null || true
    wait "$PID" 2>/dev/null || true
  fi
  if [[ "$rc" -ne 0 && -f "$LOG_FILE" ]]; then
    tail -40 "$LOG_FILE" >&2 || true
  fi
  rm -rf "$TEMP_ROOT"
  exit "$rc"
}
trap cleanup EXIT INT TERM

# ---- Validation: server directory ----
echo "=== Smoke Test: Bundled AICC Server ==="
echo "Server dir: $SERVER_DIR"

if [ ! -f "$SERVER_DIR/server.py" ]; then
  echo "FAIL: server.py not found in $SERVER_DIR" >&2
  exit 1
fi

for dir in providers collectors services web; do
  if [ ! -d "$SERVER_DIR/$dir" ]; then
    echo "FAIL: $dir/ not found in $SERVER_DIR" >&2
    exit 1
  fi
done
if [ ! -x "$SERVER_DIR/macos/start-workbuddy-monitored.sh" ]; then
  echo "FAIL: bundled WorkBuddy reconnect script not found or not executable" >&2
  exit 1
fi

# ---- Start server ----
echo "Starting server on port $PORT ..."
EINK_PORT="$PORT" \
EINK_DISCOVERY_PORT="$DISCOVERY_PORT" \
EINK_HOST=127.0.0.1 \
EINK_DATA_DIR="$TEMP_ROOT/data" \
PYTHONDONTWRITEBYTECODE=1 \
python3 -B "$SERVER_DIR/server.py" > "$LOG_FILE" 2>&1 &
PID=$!

# Wait for server to become ready
ELAPSED=0
while [ $ELAPSED -lt $TIMEOUT_SEC ]; do
  if curl -sf "$BASE_URL/api/health/live" > /dev/null 2>&1; then
    echo "Server started after ${ELAPSED}s"
    break
  fi
  sleep 1
  ELAPSED=$((ELAPSED + 1))
done

if [ $ELAPSED -ge $TIMEOUT_SEC ]; then
  echo "FAIL: Server did not start within ${TIMEOUT_SEC}s" >&2
  tail -20 "$LOG_FILE" >&2
  exit 1
fi

# ---- Health endpoints ----
echo "--- /api/health/live ---"
LIVE=$(curl -sf "$BASE_URL/api/health/live") || { echo "FAIL: /api/health/live"; exit 1; }
echo "$LIVE"
VERSION=$(tr -d '[:space:]' < "$SERVER_DIR/VERSION")
echo "$LIVE" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('ok') is True; assert d.get('status') == 'live'; assert d.get('version') == '$VERSION'" || {
  echo "FAIL: /api/health/live payload unexpected" >&2; exit 1; }

echo ""
echo "--- /api/health/ready ---"
READY=$(curl -sf "$BASE_URL/api/health/ready") || { echo "FAIL: /api/health/ready"; exit 1; }
echo "$READY"
echo "$READY" | python3 -c "import sys,json; d=json.load(sys.stdin); assert d.get('status') in ('healthy','degraded')" || {
  echo "FAIL: /api/health/ready status unexpected" >&2; exit 1; }

echo ""
echo "--- /api/status ---"
STATUS=$(curl -sf "$BASE_URL/api/status") || { echo "FAIL: /api/status"; exit 1; }
echo "$STATUS" | python3 -m json.tool | head -20 || echo "(raw, truncated)"
echo "$STATUS" | python3 -c "import sys,json; d=json.load(sys.stdin); assert 'codex' in d, 'missing codex'; assert 'deepseek' in d, 'missing deepseek'; assert 'system' in d, 'missing system'" || {
  echo "FAIL: /api/status missing expected providers" >&2; exit 1; }

# ---- Clean shutdown ----
echo ""
echo "=== All smoke tests passed ==="
