#!/bin/bash
# Guard tests for scripts/measure-resources.sh.
#
# The point is not "does the report look right" — it is "can this script quit
# the App you are using without asking". Every case below runs against a
# synthetic process table (AICC_MEASURE_PS_FILE) and a fake lifecycle log
# (AICC_MEASURE_TEST_LOG), so no real App is started or stopped.
#
#   bash scripts/test-measure-resources.sh
#
# Exits non-zero on the first failed assertion.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/scripts/measure-resources.sh"
BUNDLE_ID="com.example.aicc.measuretest"

WORK="$(mktemp -d "${TMPDIR:-/tmp}/aicc-measure-test.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

PASS=0
FAIL=0
ok()   { echo "ok   $*"; PASS=$(( PASS + 1 )); }
bad()  { echo "FAIL $*" >&2; FAIL=$(( FAIL + 1 )); }

# Two fake bundles that claim the same bundle id at different paths.
make_bundle() {
  local path="$1"
  mkdir -p "$path/Contents"
  cat > "$path/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>$BUNDLE_ID</string>
  <key>CFBundleShortVersionString</key><string>0.0.1</string>
  <key>CFBundleVersion</key><string>1</string>
</dict></plist>
PLIST
}

OURS="$WORK/ours/AICC.app"
OTHER="$WORK/other/AICC.app"
make_bundle "$OURS"
make_bundle "$OTHER"

# Fixtures: a process table holds pid, ppid, command.
ps_with() {  # ps_with <pid> <bundle path> > file
  printf '  %s   1 %s/Contents/MacOS/AICC\n' "$1" "$2"
}

PS_OURS="$WORK/ps-ours.txt"
ps_with 4242 "$OURS" > "$PS_OURS"

PS_OTHER="$WORK/ps-other.txt"
ps_with 9999 "$OTHER" > "$PS_OTHER"

PS_BOTH="$WORK/ps-both.txt"
{ ps_with 4242 "$OURS"; ps_with 9999 "$OTHER"; } > "$PS_BOTH"

PS_EMPTY="$WORK/ps-empty.txt"
: > "$PS_EMPTY"

# Optional per-test knobs, read here so they are passed explicitly instead of
# relying on `VAR=x func` semantics.
QUIT_RESULT="ok"
PS_AFTER_QUIT=""

run() {  # run <log file> [args...]
  local log="$1"; shift
  : > "$log"
  AICC_MEASURE_PS_FILE="${PS_FILE:?}" \
  AICC_MEASURE_TEST_LOG="$log" \
  AICC_MEASURE_QUIT_RESULT="$QUIT_RESULT" \
  AICC_MEASURE_PS_FILE_AFTER_QUIT="$PS_AFTER_QUIT" \
    bash "$SCRIPT" "$@" > "$WORK/stdout.log" 2>&1
}

# --- 1. sample never quits or opens -----------------------------------------
PS_FILE="$PS_OURS"
if run "$WORK/log-sample.txt" --mode sample --app "$OURS" --samples 1 --interval 0 \
     --out "$WORK/out-sample"; then
  if [[ ! -s "$WORK/log-sample.txt" ]]; then
    ok "sample：不调用 quit/open"
  else
    bad "sample 动了生命周期：$(cat "$WORK/log-sample.txt")"
  fi
else
  bad "sample 在纯观察模式下退出码非 0"
fi

# --- 2. launch without consent ----------------------------------------------
PS_FILE="$PS_OURS"
set +e
run "$WORK/log-noconsent.txt" --mode launch --app "$OURS" --out "$WORK/out2"
STATUS=$?
set -e
if [[ "$STATUS" -ne 0 && ! -s "$WORK/log-noconsent.txt" ]]; then
  ok "launch 无 --allow-lifecycle-actions：拒绝且未退出任何进程"
else
  bad "launch 无授权时的行为不对（status=$STATUS, log=$(cat "$WORK/log-noconsent.txt" 2>/dev/null))"
fi

# --- 3. launch with lifecycle consent but no real-data consent ---------------
PS_FILE="$PS_OURS"
set +e
run "$WORK/log-nodata.txt" --mode launch --app "$OURS" --out "$WORK/out3" --allow-lifecycle-actions
STATUS=$?
set -e
if [[ "$STATUS" -ne 0 && ! -s "$WORK/log-nodata.txt" ]]; then
  ok "launch 缺 --allow-real-data：拒绝且未退出任何进程"
else
  bad "launch 缺真实数据授权时的行为不对（status=$STATUS）"
fi

# --- 4. another install path is running -------------------------------------
PS_FILE="$PS_OTHER"
set +e
run "$WORK/log-foreign.txt" --mode launch --app "$OURS" --out "$WORK/out4" \
  --allow-lifecycle-actions --allow-real-data
STATUS=$?
set -e
if [[ "$STATUS" -ne 0 && ! -s "$WORK/log-foreign.txt" ]]; then
  ok "其他安装位置在运行时：拒绝，且没有 quit/open"
else
  bad "外来实例未被拒绝（status=$STATUS, log=$(cat "$WORK/log-foreign.txt" 2>/dev/null))"
fi

# --- 5. quit times out -> fail closed, never open ---------------------------
PS_FILE="$PS_OURS"
set +e
QUIT_RESULT="timeout"
run "$WORK/log-timeout.txt" --mode launch --app "$OURS" \
  --out "$WORK/out5" --allow-lifecycle-actions --allow-real-data --settle-timeout 2
STATUS=$?
set -e
if [[ "$STATUS" -ne 0 ]] && grep -qx "quit $BUNDLE_ID" "$WORK/log-timeout.txt" \
   && ! grep -q '^open ' "$WORK/log-timeout.txt"; then
  ok "退出失败：fail closed，没有 open"
else
  bad "退出超时后的行为不对（status=$STATUS, log=$(cat "$WORK/log-timeout.txt" 2>/dev/null))"
fi

# --- 6. happy path: quit then open, exactly once each -----------------------
PS_FILE="$PS_OURS"
set +e
QUIT_RESULT="ok"
PS_AFTER_QUIT="$PS_EMPTY"
run "$WORK/log-ok.txt" --mode launch --app "$OURS" \
  --out "$WORK/out6" --allow-lifecycle-actions --allow-real-data --settle-timeout 2 --samples 1 --interval 0
STATUS=$?
set -e
QUITS=$(grep -c '^quit ' "$WORK/log-ok.txt" 2>/dev/null || true)
OPENS=$(grep -c '^open ' "$WORK/log-ok.txt" 2>/dev/null || true)
if [[ "${QUITS:-0}" -eq 1 && "${OPENS:-0}" -eq 1 ]]; then
  ok "授权后：quit 一次、open 一次"
else
  bad "正常路径的动作次数不对（quit=${QUITS:-0} open=${OPENS:-0}, status=$STATUS）"
fi

# --- 7. exit mode without consent ------------------------------------------
PS_FILE="$PS_OURS"
set +e
run "$WORK/log-exit-noconsent.txt" --mode exit --app "$OURS" --out "$WORK/out7"
STATUS=$?
set -e
if [[ "$STATUS" -ne 0 && ! -s "$WORK/log-exit-noconsent.txt" ]]; then
  ok "exit 无授权：拒绝"
else
  bad "exit 无授权时的行为不对（status=$STATUS）"
fi

echo
if [[ "$FAIL" -gt 0 ]]; then
  echo "FAILED: $FAIL 项失败，$PASS 项通过" >&2
  exit 1
fi
echo "PASS: $PASS 项全部通过（未启动或退出任何真实 App）"
