#!/bin/bash
# Negative and positive tests for scripts/verify-symbols.sh.
#
# The whole point of that script is to fail when the symbols do not belong to
# the App, so the interesting cases are the failures. Each one is produced by
# damaging a *copy* of the real archive, never the archive itself.
#
#   bash scripts/test-verify-symbols.sh
#
# Skips when there is no build to test against (so it is safe on a fresh clone).

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP="$ROOT/dist/mac/AICC.app"
SYMBOLS="$ROOT/dist/symbols"
DMG="$ROOT/dist/AICC-2.13.0.dmg"
VERIFY="$ROOT/scripts/verify-symbols.sh"

if [[ ! -d "$APP" ]] || [[ ! -d "$SYMBOLS" ]]; then
  echo "SKIP: 没有构建产物（先跑 bash macos/build-aicc-swiftui.sh）"
  exit 0
fi

ARCHIVE="$(ls -d "$SYMBOLS"/* 2>/dev/null | sort | tail -1)"
if [[ -z "$ARCHIVE" ]] || [[ ! -f "$ARCHIVE/manifest.txt" ]]; then
  echo "SKIP: $SYMBOLS 下没有带 manifest 的归档"
  exit 0
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/aicc-symbol-test.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0
ok()  { echo "ok   $*"; PASS=$(( PASS + 1 )); }
bad() { echo "FAIL $*" >&2; FAIL=$(( FAIL + 1 )); }

expect_ok() {  # expect_ok <描述> <命令...>
  local what="$1"; shift
  if "$@" > "$WORK/out.log" 2>&1; then ok "$what"; else
    bad "${what}（应当通过却失败）"; tail -3 "$WORK/out.log" >&2
  fi
}

expect_fail() {  # expect_fail <描述> <期望出现的关键字> <命令...>
  local what="$1" needle="$2"; shift 2
  if "$@" > "$WORK/out.log" 2>&1; then
    bad "${what}（应当失败却通过）"
  elif grep -q "$needle" "$WORK/out.log"; then
    ok "$what"
  else
    bad "${what}（报错信息里没有 '$needle'）"; tail -3 "$WORK/out.log" >&2
  fi
}

# --- 1. the real pair must pass ---------------------------------------------
expect_ok "真实 App + 对应归档：通过" \
  bash "$VERIFY" --app "$APP" --symbols "$ARCHIVE"

# --- 2. a dSYM from another binary must be rejected -------------------------
MIXED="$WORK/mixed"
cp -R "$ARCHIVE" "$MIXED"
rm -rf "$MIXED/AICC.dSYM"
cp -R "$MIXED/AICCWidget.dSYM" "$MIXED/AICC.dSYM"
expect_fail "把 Widget 的 dSYM 冒充成 App 的：拒绝" "dSYM UUID" \
  bash "$VERIFY" --app "$APP" --symbols "$MIXED"

# --- 3. a stale manifest for another build must be rejected -----------------
# Same version/build, different UUID: exactly what "rebuild the same version"
# produces.
STALE="$WORK/stale"
cp -R "$ARCHIVE" "$STALE"
# awk 重写而不是 sed -i：BSD/GNU 的 -i 语义不同，这里没必要踩这个坑。
awk 'BEGIN { OFS = " " }
     $1 == "binary" && $2 == "AICC" {
       print "binary", $2, $3, "00000000-0000-0000-0000-000000000000", "deadbeef"; next
     }
     { print }' "$STALE/manifest.txt" > "$STALE/manifest.tmp"
mv "$STALE/manifest.tmp" "$STALE/manifest.txt"
expect_fail "manifest 记录的是另一次构建的 UUID：拒绝" "manifest" \
  bash "$VERIFY" --app "$APP" --symbols "$STALE"

# --- 4. a tampered binary must be rejected even with a matching UUID --------
TAMPERED="$WORK/tampered.app"
cp -R "$APP" "$TAMPERED"
printf 'x' >> "$TAMPERED/Contents/MacOS/AICC"
expect_fail "二进制内容被改动（UUID 不变、sha256 变）：拒绝" "sha256" \
  bash "$VERIFY" --app "$TAMPERED" --symbols "$ARCHIVE"

# --- 5. no manifest at all --------------------------------------------------
BARE="$WORK/bare"
mkdir -p "$BARE"
cp -R "$ARCHIVE/AICC.dSYM" "$BARE/" 2>/dev/null || true
expect_fail "归档里没有 manifest：拒绝" "manifest" \
  bash "$VERIFY" --app "$APP" --symbols "$BARE"

# --- 6. a real address must resolve to file:line ----------------------------
# The reviewer's point: "有 100 行 .debug_line" only proves the table exists.
# This proves it can answer the question a crash report actually asks.
resolve_check() {  # resolve_check <name>
  local name="$1" dw addr resolved
  dw="$ARCHIVE/$name.dSYM/Contents/Resources/DWARF/$name"
  [[ -f "$dw" ]] || return 1
  # 从"未裁剪原件"取一个真实函数地址（第一个 T 是 __TEXT 基址，跳过）。
  addr="$(nm -n "$ARCHIVE/$name.unstripped" 2>/dev/null | grep ' T ' | sed -n 2p | awk '{ print $1 }')"
  [[ -n "$addr" ]] || return 1
  resolved="$(atos -o "$dw" -arch arm64 -l 0x100000000 "0x$addr" 2>/dev/null | head -1)"
  [[ "$resolved" == *".swift:"* ]] || return 1
  echo "     $name → $resolved"
}

if resolve_check AICC > "$WORK/out.log" 2>&1; then
  ok "atos 能把真实地址解析到源码行：$(sed -n 's/^ *//p' "$WORK/out.log" | head -1)"
else
  bad "atos 无法把地址解析到 .swift 行号"; cat "$WORK/out.log" >&2
fi

# --- 7. DMG hash mismatch ---------------------------------------------------
if [[ -f "$DMG" ]]; then
  expect_ok "真实 DMG + manifest：通过" \
    bash "$VERIFY" --app "$APP" --symbols "$ARCHIVE" --dmg "$DMG"
  expect_fail "换一个 DMG 来对：拒绝" "DMG sha256" \
    bash "$VERIFY" --app "$APP" --symbols "$ARCHIVE" --dmg "$APP/Contents/MacOS/AICC"
else
  echo "SKIP: 没有 dist/AICC-2.13.0.dmg，跳过 DMG 用例"
fi

echo
if [[ "$FAIL" -gt 0 ]]; then
  echo "FAILED: $FAIL 项失败，$PASS 项通过" >&2
  exit 1
fi
echo "PASS: $PASS 项全部通过（含 5 个必须拒绝的负向用例）"
