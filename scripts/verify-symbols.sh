#!/bin/bash
# Prove that an archive of symbols belongs to *this* App.
#
# The failure this exists to prevent: a build produces an App, a second build
# produces a DMG, both are "2.13.0 / build 25", and the symbols kept for crash
# triage belong to the first one. Everything looks fine until someone tries to
# symbolicate a real crash and gets addresses instead of function names.
#
# A UUID match is the only thing that ties them together, so this script refuses
# to pass unless every link agrees:
#
#   manifest  ->  binary under test  ->  dSYM next to the manifest
#
# It also refuses to accept a dSYM that only has symbol names: a dSYM without a
# line table cannot tell you which line crashed.
#
# Usage:
#   bash scripts/verify-symbols.sh --app <AICC.app> --symbols <dist/symbols/2.13.0-25>
#   bash scripts/verify-symbols.sh --app <mounted>/AICC.app --symbols <dir> --dmg <file>
#
# Exit codes: 0 all good, 1 mismatch or missing evidence, 2 bad usage.

set -euo pipefail

APP=""
SYMBOLS=""
DMG=""
MIN_LINES=100

usage() {
  cat <<'USAGE'
Usage: bash scripts/verify-symbols.sh --app <AICC.app> --symbols <dir> [--dmg <file>]

  --app      App bundle to check (a build output, or one mounted from a DMG)
  --symbols  Symbol archive directory produced by build-aicc-swiftui.sh
  --dmg      Optional: also check that the manifest's dmg_sha256 matches this file
  -h, --help This message

Fails (exit 1) when: the manifest is missing, a binary's UUID differs from the
dSYM, a dSYM has fewer than 100 lines of .debug_line, or the DMG hash differs.
USAGE
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --app)     APP="$2"; shift 2 ;;
    --symbols) SYMBOLS="$2"; shift 2 ;;
    --dmg)     DMG="$2"; shift 2 ;;
    -h|--help) usage ;;
    *) echo "Unknown argument: $1" >&2; usage ;;
  esac
done

fail() { echo "FAIL: $*" >&2; exit 1; }

[[ -n "$APP" ]] || usage
[[ -n "$SYMBOLS" ]] || usage
[[ -d "$APP" ]] || fail "没有这个 App：$APP"
[[ -d "$SYMBOLS" ]] || fail "没有这个符号目录：$SYMBOLS"

MANIFEST="$SYMBOLS/manifest.txt"
[[ -f "$MANIFEST" ]] || fail "$SYMBOLS 里没有 manifest.txt —— 无法证明这些符号属于这个 App"

uuid_of() {
  dwarfdump --uuid "$1" 2>/dev/null | awk 'NR == 1 { print $2 }'
}

sha_of() {
  shasum -a 256 "$1" 2>/dev/null | awk '{ print $1 }'
}

checked=0
while read -r tag name relpath want_uuid want_sha; do
  [[ "$tag" == "binary" ]] || continue
  [[ -n "$relpath" ]] || fail "manifest 行不完整：$tag $name"

  binary="$APP/$relpath"
  [[ -f "$binary" ]] || fail "${name}：App 内找不到 $relpath"

  got_uuid="$(uuid_of "$binary")"
  got_sha="$(sha_of "$binary")"
  [[ -n "$got_uuid" ]] || fail "${name}：读不出二进制 UUID"

  [[ "$got_uuid" == "$want_uuid" ]] \
    || fail "${name}：二进制 UUID $got_uuid ≠ manifest ${want_uuid}（符号不是这次构建的）"
  [[ "$got_sha" == "$want_sha" ]] \
    || fail "${name}：二进制 sha256 与 manifest 不符（同一个 UUID 但内容不同，属异常）"

  dsym="$SYMBOLS/$name.dSYM"
  [[ -d "$dsym" ]] || fail "${name}：缺少 $name.dSYM"
  dsym_uuid="$(uuid_of "$dsym")"
  [[ "$dsym_uuid" == "$want_uuid" ]] \
    || fail "${name}：dSYM UUID $dsym_uuid ≠ 二进制 $want_uuid"

  rows="$(dwarfdump --debug-line "$dsym" 2>/dev/null | wc -l | tr -d ' ')"
  [[ "${rows:-0}" -ge "$MIN_LINES" ]] \
    || fail "${name}：dSYM 只有 ${rows} 行 .debug_line，无法定位到源码行"

  echo "ok   $name  uuid=$got_uuid  sha256=${got_sha:0:12}…  debug_line=${rows} 行"
  checked=$(( checked + 1 ))
done < "$MANIFEST"

[[ "$checked" -gt 0 ]] || fail "manifest 里没有 binary 记录，什么都没验"

if [[ -n "$DMG" ]]; then
  [[ -f "$DMG" ]] || fail "没有这个 DMG：$DMG"
  want="$(awk '$1 == "dmg_sha256" { print $2; exit }' "$MANIFEST")"
  [[ -n "$want" ]] || fail "manifest 里没有 dmg_sha256，无法证明符号对应这个 DMG"
  got="$(sha_of "$DMG")"
  [[ "$got" == "$want" ]] || fail "DMG sha256 $got ≠ manifest $want"
  echo "ok   DMG   sha256=${got:0:12}…  与 manifest 一致"
fi

echo "PASS: $checked 个二进制的 UUID / sha256 / dSYM 行号全部对应（${SYMBOLS}）"
