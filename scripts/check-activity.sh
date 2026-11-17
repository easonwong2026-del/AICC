#!/usr/bin/env bash
# 四账号活跃交叉诊断（PR-ACT-3，只读）。
#
# 调用 `growth.activity.status`（每账号两个 GET），打印一张能直接贴进验收报告的
# 表：今天的活跃、签到、连登、下一档，以及"这一步谁来做"。
#
# 用法：
#   bash scripts/check-activity.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKER="${WORKER:-$ROOT/account-worker/target/debug/aicc-account-worker}"

if [[ ! -x "$WORKER" ]]; then
  echo "缺少 worker：$WORKER" >&2
  echo "先跑：cargo build --manifest-path account-worker/Cargo.toml" >&2
  exit 1
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/aicc-activity-check.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"growth.activity.status","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"worker.shutdown","params":{}}' \
  | "$WORKER" 2>/dev/null | head -1 > "$TMP/report.json"

python3 - "$TMP/report.json" <<'PY'
import json, sys

payload = json.load(open(sys.argv[1]))
if payload.get("error"):
    print("RPC 失败：", payload["error"].get("message"), file=sys.stderr)
    raise SystemExit(1)
report = payload.get("result") or {}
accounts = report.get("accounts") or []
summary = report.get("summary") or {}

def who(diagnosis):
    code = diagnosis.get("code")
    if code == "active":
        return "不用管"
    if code == "checkin_only":
        return "用户（客户端做一次真实任务）"
    if code == "checkin_missing":
        return "AICC（跑一次签到）"
    if code == "stale_stats":
        return "等（稍后刷新）"
    if code == "needs_auth":
        return "用户（重新授权）"
    return "用户（先确认凭据）"

def cells(account):
    checkin = account.get("checkin") or {}
    checkin_text = checkin.get("status") or "无记录"
    active = account.get("todayActive")
    active_text = "是" if active is True else ("否" if active is False else "—")
    score = account.get("todayScore")
    if active is True and score is not None:
        active_text += f" ({score})"
    streak = account.get("streakDays")
    streak_text = "—" if streak is None else str(streak)
    remaining = account.get("nextTierRemaining")
    tier = account.get("nextTier")
    next_text = "—" if tier is None else f"{tier} 还差 {remaining if remaining is not None else '?'}"
    return [account.get("nickname") or account.get("accountId"), active_text,
            checkin_text, streak_text, next_text]

header = ["账号", "今日活跃", "签到", "连登", "下一档", "结论", "谁来做"]
rows = []
for account in accounts:
    rows.append(cells(account) + [
        account.get("diagnosis", {}).get("code", "?"),
        who(account.get("diagnosis") or {}),
    ])

widths = [0] * len(header)
for row in [header] + rows:
    for index, value in enumerate(row):
        widths[index] = max(widths[index], sum(2 if ord(ch) > 127 else 1 for ch in str(value)))
fmt = "  ".join("{:<%d}" % w for w in widths)
print(fmt.format(*header))
print("-" * (sum(widths) + 2 * (len(widths) - 1)))
for row in rows:
    print(fmt.format(*row))
print()
print(f"服务端今天：{accounts[0].get('serverToday') if accounts else '—'}")
print(f"汇总：总 {summary.get('total', 0)} · 已活跃 {summary.get('active', 0)} · "
      f"待处理 {summary.get('pending', 0)} · 未知 {summary.get('unknown', 0)} · "
      f"需授权 {summary.get('needsAuth', 0)}")
print(f"读取时间：{report.get('fetchedAt')}")
print("只读：签到与活跃分开算；未活跃不代表签到失败。")
PY
