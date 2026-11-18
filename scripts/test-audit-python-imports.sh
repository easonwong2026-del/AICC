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

[[ -x "$RUNTIME/bin/python3" ]] || { echo "SKIP: 没有内嵌运行时（$RUNTIME），先构建一次 macOS app" >&2; exit 0; }

WORK="$(mktemp -d "${TMPDIR:-/tmp}/aicc-imports-test.XXXXXX")"
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

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

FAIL=0
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

# No leftovers: the mock/canary/driver must all be gone.
LEFTOVERS="$(ps -axo pid=,command= | grep -F "$WORK" | grep -v grep || true)"
check "没有遗留子进程" test -z "$LEFTOVERS"

if grep -q 'canary 命中 \*\*unavailable\*\*' "$REPORT"; then
  echo "NOTE: 3425 已被占用（可能 Magpie 在运行），canary 无法放置探针；已退化为「流量全部落到 mock」的单侧证据" >&2
fi

echo
if [[ "$FAIL" -gt 0 ]]; then
  echo "FAILED: $FAIL 项失败" >&2
  exit 1
fi
echo "PASS: 隔离校验全部通过"
