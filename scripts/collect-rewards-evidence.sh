#!/usr/bin/env bash
# 采集成长中心四个只读接口的真实响应（PR #100）。
#
# 只做 GET：/streak、/redeem/summary、/heatmap、/lottery/chances。
# 输出写到 docs/evidence/<日期>-<account>.json，响应体已在 worker 内脱敏
# （凭据键丢弃、邮箱/昵称打码、长串截断）。
#
# 用法：
#   bash scripts/collect-rewards-evidence.sh                # 自动挑第一个 CN 账号
#   bash scripts/collect-rewards-evidence.sh <accountId>    # 指定账号
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKER="${WORKER:-$ROOT/account-worker/target/debug/aicc-account-worker}"
OUT_DIR="$ROOT/docs/evidence"
ACCOUNT="${1:-}"

if [[ ! -x "$WORKER" ]]; then
  echo "缺少 worker：$WORKER" >&2
  echo "先跑：cargo build --manifest-path account-worker/Cargo.toml" >&2
  exit 1
fi

# 采集只读写真实库：不设置 AICC_ACCOUNT_DATA_DIR，走默认数据目录。
TMP="$(mktemp -d "${TMPDIR:-/tmp}/aicc-evidence.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

# 1) 找账号。
if [[ -z "$ACCOUNT" ]]; then
  printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"accounts.list","params":{}}' \
    | "$WORKER" 2>/dev/null | head -1 > "$TMP/accounts.json"
  ACCOUNT="$(python3 - "$TMP/accounts.json" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1]))
accounts = payload.get("result", {}).get("accounts", [])
for account in accounts:
    provider = (account.get("provider") or "workbuddy").lower()
    if provider in ("workbuddy", "workbuddy_cn"):
        print(account.get("id", ""))
        break
PY
)"
fi

if [[ -z "$ACCOUNT" ]]; then
  echo "没有找到 WorkBuddy 国内版账号；请显式传入 accountId" >&2
  exit 1
fi

echo "账号：$ACCOUNT"

# 2) 采集。confirm=true 是硬要求：这条 RPC 会真的发四个请求。
printf '%s\n' \
  "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"growth.rewards.evidence\",\"params\":{\"confirm\":true,\"accountId\":\"$ACCOUNT\"}}" \
  '{"jsonrpc":"2.0","id":2,"method":"worker.shutdown","params":{}}' \
  | "$WORKER" 2>/dev/null | head -1 > "$TMP/report.json"

mkdir -p "$OUT_DIR"
STAMP="$(date '+%Y-%m-%dT%H%M%S')"
TARGET="$OUT_DIR/$STAMP-${ACCOUNT}.json"

python3 - "$TMP/report.json" "$TARGET" "$ACCOUNT" <<'PY'
import json, sys, datetime

source, target, account = sys.argv[1], sys.argv[2], sys.argv[3]
reply = json.load(open(source))
if "error" in reply:
    raise SystemExit(f"采集失败：{reply['error']}")

report = reply["result"]
report["accountId"] = report.get("accountId") or account
report["collectedAt"] = report.get("capturedAt") or datetime.datetime.utcnow().isoformat() + "Z"
report["source"] = "scripts/collect-rewards-evidence.sh"
report["note"] = "真实响应已脱敏：凭据键丢弃、邮箱/昵称打码、长串截断；不含 Cookie/Authorization。"
json.dump(report, open(target, "w"), ensure_ascii=False, indent=2)
print(f"写入 {target}")
confirmed = [f for f in report.get("findings", []) if f.get("verdict") == "confirmed"]
missing = [f["field"] for f in report.get("findings", []) if f.get("verdict") != "confirmed"]
print(f"字段命中 {len(confirmed)} 项；未命中：{', '.join(missing) if missing else '无'}")
PY

echo "完成：$TARGET"
