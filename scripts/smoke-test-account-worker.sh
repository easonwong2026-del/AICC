#!/bin/bash
# 账号中心助手冒烟：不联网、不碰真实钥匙串、不写用户数据目录。
#
# 覆盖：
#   1. --self-test（建库、迁移、账号读写、凭据探针）
#   2. JSON-RPC over stdio 往返（health / shutdown）
#   3. WB_SWITCH_HOME 隔离（不得创建 ~/.wb-switch）
#
# 用法：smoke-test-account-worker.sh [可执行文件路径]
#   默认取 account-worker/target/release/aicc-account-worker，其次取 PATH。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${1:-}"

if [[ -z "$BIN" ]]; then
  for candidate in \
    "$ROOT/account-worker/target/release/aicc-account-worker" \
    "$ROOT/account-worker/target/debug/aicc-account-worker"; do
    if [[ -x "$candidate" ]]; then BIN="$candidate"; break; fi
  done
fi
[[ -n "$BIN" ]] || { echo "FAIL: 找不到 aicc-account-worker，先 cargo build" >&2; exit 1; }
[[ -x "$BIN" ]] || { echo "FAIL: $BIN 不可执行" >&2; exit 1; }

echo "== self-test =="
SELF_TEST_OUT="$("$BIN" --self-test)"
echo "$SELF_TEST_OUT"
SELF_TEST_OUT="$SELF_TEST_OUT" python3 - <<'PY'
import json, os, sys
report = json.loads(os.environ["SELF_TEST_OUT"])
if not report.get("ok"):
    print("FAIL: self-test 未通过", file=sys.stderr)
    sys.exit(1)
names = {c["name"] for c in report["checks"]}
required = {"storage.init", "accounts.upsert", "accounts.list", "secure.status", "health"}
missing = required - names
if missing:
    print(f"FAIL: 缺少检查项 {sorted(missing)}", file=sys.stderr)
    sys.exit(1)
PY

echo "== stdio JSON-RPC =="
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

RPC_OUT="$(
  printf '%s\n%s\n' \
    '{"jsonrpc":"2.0","id":1,"method":"ping"}' \
    '{"jsonrpc":"2.0","id":2,"method":"worker.shutdown"}' \
  | AICC_ACCOUNT_DATA_DIR="$TMP/data" "$BIN"
)"
RPC_OUT="$RPC_OUT" python3 - <<'PY'
import json, os, sys
lines = [json.loads(l) for l in os.environ["RPC_OUT"].splitlines() if l.strip()]
if len(lines) != 2:
    print(f"FAIL: 期望 2 条响应，实际 {len(lines)}", file=sys.stderr)
    sys.exit(1)
ping, shutdown = lines
if ping.get("id") != 1 or not ping.get("result", {}).get("pong"):
    print(f"FAIL: ping 响应异常 {ping}", file=sys.stderr)
    sys.exit(1)
if ping["result"].get("protocol") != 1:
    print("FAIL: 协议版本不是 1", file=sys.stderr)
    sys.exit(1)
if not shutdown.get("result", {}).get("shuttingDown"):
    print(f"FAIL: shutdown 响应异常 {shutdown}", file=sys.stderr)
    sys.exit(1)
PY

echo "== 未知方法必须报错而不是静默 =="
ERR_OUT="$(
  printf '%s\n' '{"jsonrpc":"2.0","id":9,"method":"nope.nothing"}' \
  | AICC_ACCOUNT_DATA_DIR="$TMP/data" "$BIN" || true
)"
ERR_OUT="$ERR_OUT" python3 - <<'PY'
import json, os, sys
line = json.loads(os.environ["ERR_OUT"].strip().splitlines()[0])
if line.get("error", {}).get("code") != -32601:
    print(f"FAIL: 未知方法应返回 -32601，实际 {line}", file=sys.stderr)
    sys.exit(1)
PY

echo "== WB_SWITCH_HOME 隔离 =="
[[ -d "$TMP/data" ]] || { echo "FAIL: 数据目录没有创建" >&2; exit 1; }
[[ -d "$TMP/data/wb-switch-home" ]] || { echo "FAIL: 未把 WB_SWITCH_HOME 收进数据目录" >&2; exit 1; }
# 用户自己的 wb-switch 数据不允许被创建或写入。
if [[ -d "$HOME/.wb-switch" ]]; then
  NEWEST="$(find "$HOME/.wb-switch" -newer "$TMP" -print -quit 2>/dev/null || true)"
  [[ -z "$NEWEST" ]] || { echo "FAIL: 本次运行写到了 $HOME/.wb-switch：$NEWEST" >&2; exit 1; }
  echo "提示: 用户已有 ~/.wb-switch，本次运行未触碰"
fi

echo "OK: account worker smoke passed"
