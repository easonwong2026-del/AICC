#!/usr/bin/env bash
# 采集"成长活跃"证据：每个国内版账号的 /streak + /heatmap（只读）。
#
# 与 collect-rewards-evidence.sh 的区别：
#   - 那个脚本一次一个账号，用来做协议取证；
#   - 这个脚本一次跑完所有国内版账号，并直接打印一张对照表，
#     回答的是"今天哪个账号亮了热力墙、连登几天、下次到档还差几天"。
#
# 只发 GET（复用 growth.rewards.evidence 的四个只读接口），不改任何服务端状态。
#
# 用法：
#   bash scripts/collect-activity-evidence.sh                # 全部国内版账号
#   bash scripts/collect-activity-evidence.sh <accountId>…   # 指定账号
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKER="${WORKER:-$ROOT/account-worker/target/debug/aicc-account-worker}"
OUT_DIR="$ROOT/docs/evidence"

if [[ ! -x "$WORKER" ]]; then
  echo "缺少 worker：$WORKER" >&2
  echo "先跑：cargo build --manifest-path account-worker/Cargo.toml" >&2
  exit 1
fi

TMP="$(mktemp -d "${TMPDIR:-/tmp}/aicc-activity.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

# 1) 取账号列表（只读本地库，不联网）。
printf '%s\n' '{"jsonrpc":"2.0","id":1,"method":"accounts.list","params":{}}' \
  | "$WORKER" 2>/dev/null | head -1 > "$TMP/accounts.json"

if [[ $# -gt 0 ]]; then
  IDS=("$@")
else
  IDS=()
  while IFS= read -r id; do
    [[ -n "$id" ]] && IDS+=("$id")
  done < <(python3 - "$TMP/accounts.json" <<'PY'
import json, sys
payload = json.load(open(sys.argv[1]))
for account in payload.get("result", {}).get("accounts", []):
    if (account.get("provider") or "workbuddy").lower() in ("workbuddy", "workbuddy_cn"):
        print(account.get("id", ""))
PY
)
fi

if [[ ${#IDS[@]} -eq 0 ]]; then
  echo "没有找到 WorkBuddy 国内版账号" >&2
  exit 1
fi

mkdir -p "$OUT_DIR"
STAMP="$(date '+%Y-%m-%dT%H%M%S')"

# 2) 逐个账号采集。账号之间串行：同一时刻只跑一个扫描，避免令牌刷新撞车。
for id in "${IDS[@]}"; do
  printf '%s\n' \
    "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"growth.rewards.evidence\",\"params\":{\"confirm\":true,\"accountId\":\"$id\"}}" \
    '{"jsonrpc":"2.0","id":2,"method":"worker.shutdown","params":{}}' \
    | "$WORKER" 2>/dev/null | head -1 > "$TMP/$id.json"
  echo "采集 $id"
done

# 3) 一张表说清四个账号现在的状态。
python3 - "$TMP" "$OUT_DIR" "$STAMP" <<'PY'
import json, os, sys

tmp, out_dir, stamp = sys.argv[1], sys.argv[2], sys.argv[3]

rows = []
saved = []
for name in sorted(os.listdir(tmp)):
    if name in ("accounts.json",) or not name.endswith(".json"):
        continue
    payload = json.load(open(os.path.join(tmp, name)))
    report = (payload.get("result") or {})
    if payload.get("error"):
        rows.append((name[:-5], "error", payload["error"].get("message", "")[:40], "", "", "", ""))
        continue
    endpoints = {e["name"]: e for e in report.get("endpoints", [])}
    path = os.path.join(out_dir, f"activity-{stamp}-{name[:-5]}.json")
    with open(path, "w") as fh:
        json.dump(report, fh, ensure_ascii=False, indent=1)
    saved.append(path)

    def body(key):
        entry = endpoints.get(key) or {}
        return entry.get("body") or {}

    streak = ((body("streak").get("data") or {}).get("streak") or {})
    today = ((body("heatmap").get("data") or {}).get("today") or {})
    cards = ((body("streak").get("data") or {}).get("makeup_cards") or {})
    rows.append((
        name[:-5][:16],
        today.get("date", "—"),
        "是" if today.get("is_active") else "否",
        str(today.get("score", "—")),
        str(streak.get("days", "—")),
        f"{streak.get('next_tier', '—')} / 还差 {streak.get('next_tier_remaining', '—')} 天",
        str(cards.get("balance", "—")),
    ))

widths = [16, 12, 6, 6, 6, 22, 6]
header = ("账号", "服务端日期", "活跃", "分数", "连登", "下一档", "补签")
fmt = "  ".join("{:<%d}" % w for w in widths)
print()
print(fmt.format(*header))
print("-" * (sum(widths) + 2 * (len(widths) - 1)))
for row in rows:
    print(fmt.format(*row))
print()
print(f"原始响应：{out_dir}/activity-{stamp}-<account>.json（共 {len(saved)} 份，已脱敏）")
print("注意：'活跃=否' 只代表服务端热力墙今天没被点亮，不代表签到失败。")
PY
