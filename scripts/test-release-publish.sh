#!/bin/bash
# 发布脚本的 Mock 测试（PR #105 复审要求）。
#
# 测的是"失败时绝不能有副作用"，以及几条发布前必须成立的约束：
#   1. `--dry-run` 在还没有 Tag 时也能跑完并给出计划；
#   2. Release 查询失败（401/500/坏 JSON）必须停，只有 404 才允许创建；
#   3. 资产列表查询失败必须停，不能当成"没有资产"；
#   4. 资产上传失败不碰清单，不提交、不推送；
#   5. 实跑要求 Tag 存在、指向 HEAD，且 origin 上一致；
#   6. 正常路径能传完两个资产、写清单、提交并推送。
#
# 做法：临时目录里造一个"仓库"（VERSION / 发布说明 / 假 DMG / 假 Info.plist），
# 用一个假 `curl` 拦掉网络，git 走真实的本地 bare 仓库（`url.insteadOf` 把
# github SSH 映射到本地），所以 commit / push / diff / ls-remote 全是真执行。
set -uo pipefail

SRC_ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/aicc-release-test.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); printf '  ok   %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf '  FAIL %s\n' "$1"; }
check() { if eval "$2"; then ok "$1"; else bad "$1"; fi; }

# ---------------------------------------------------------------- fake curl
mkdir -p "$TMP/bin"
cat > "$TMP/bin/curl" <<'FAKE'
#!/bin/bash
# 极简 curl：只实现 release-publish.sh 用到的几种调用。
out=""; fmt=""; method="GET"; url=""
args=("$@"); i=0
while (( i < ${#args[@]} )); do
  case "${args[$i]}" in
    -X) method="${args[$((i+1))]}"; i=$((i+2)); continue ;;
    -o) out="${args[$((i+1))]}"; i=$((i+2)); continue ;;
    -w) fmt="${args[$((i+1))]}"; i=$((i+2)); continue ;;
    -H|--max-time|--data-binary) i=$((i+2)); continue ;;
    http*) url="${args[$i]}" ;;
  esac
  i=$((i+1))
done

printf '%s %s\n' "$method" "$url" >> "${FAKE_LOG:-/dev/null}"

emit() {
  local body="$1" code="${2:-200}"
  [[ -n "$out" && "$out" != "/dev/null" ]] && printf '%s' "$body" > "$out"
  # 只有 -w '%{http_code}' 需要只回状态码；其它情况把 body 打到 stdout，
  # 脚本会用管道读它（release GET / assets GET / POST releases）。
  if [[ "$fmt" == "%{http_code}" ]]; then
    printf '%s' "$code"
  else
    printf '%s' "$body"
  fi
  return 0
}

case "$url" in
  */releases/tags/*)
    st="${FAKE_RELEASE_LOOKUP_STATUS:-200}"
    if [[ "$st" == "200" ]]; then
      body="${FAKE_RELEASE_LOOKUP_BODY-}"
      [[ -n "$body" ]] || body='{"id":900}'
      emit "$body" 200
    else
      emit '{"message":"boom"}' "$st"
    fi ;;
  *assets?name=*)
    name="${url##*name=}"
    if [[ "$name" == *.dmg ]]; then code="${FAKE_UPLOAD_DMG_STATUS:-201}"; else code="${FAKE_UPLOAD_SHA_STATUS:-201}"; fi
    if [[ "$code" == 2* ]]; then printf '%s\n' "$name" >> "${FAKE_STATE}/assets"; fi
    emit '{"id":1}' "$code" ;;
  */releases/900/assets)
    st="${FAKE_ASSETS_STATUS:-200}"
    if [[ "$st" != "200" ]]; then emit '{"message":"boom"}' "$st"; exit 0; fi
    if [[ -n "${FAKE_ASSETS_BODY:-}" ]]; then emit "$FAKE_ASSETS_BODY" 200; exit 0; fi
    body="[]"
    if [[ -s "${FAKE_STATE}/assets" ]]; then
      body="$(python3 -c 'import json,sys; print(json.dumps([{"name":n.strip()} for n in open(sys.argv[1]) if n.strip()]))' "${FAKE_STATE}/assets")"
    fi
    emit "$body" ;;
  */releases/900)
    emit '{"id":900,"published_at":"2026-11-18T00:00:00Z"}' ;;
  */releases)
    emit '{"id":900}' "${FAKE_CREATE_STATUS:-201}" ;;
  *)
    emit '' 200 ;;
esac
FAKE
chmod +x "$TMP/bin/curl"

# 假 dwarfdump：UUID 与 .debug_line 行数都给固定值，让 verify-symbols.sh 能通过。
# 真实的 UUID 语义由 scripts/test-verify-symbols.sh 用真产物覆盖，这里只测发布流程。
cat > "$TMP/bin/dwarfdump" <<'FAKE'
#!/bin/bash
case "$1" in
  --uuid)
    echo "UUID: 11111111-1111-1111-1111-111111111111 (arm64) ${2:-}"
    ;;
  --debug-line)
    i=0; while (( i < 150 )); do echo "0x00000000: Fake.swift:1"; i=$(( i + 1 )); done
    ;;
esac
FAKE
chmod +x "$TMP/bin/dwarfdump"
export PATH="$TMP/bin:$PATH"

# ---------------------------------------------------------------- repo maker
make_repo() {
  local name="$1" dir="$TMP/$1" remote="$TMP/$1-remote.git"
  mkdir -p "$dir/scripts" "$dir/docs/releases" "$dir/dist/mac/AICC.app/Contents" "$dir/updates"
  cp "$SRC_ROOT/scripts/release-publish.sh" "$dir/scripts/"
  # 发布脚本的符号硬门会调用它；fixture 里要有一份，否则连正常路径都过不去。
  cp "$SRC_ROOT/scripts/verify-symbols.sh" "$dir/scripts/"
  printf '9.9.9\n' > "$dir/VERSION"
  cat > "$dir/dist/mac/AICC.app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleVersion</key><string>99</string></dict></plist>
PLIST
  printf '# notes\n' > "$dir/docs/releases/9.9.9.md"
  head -c 2048 /dev/urandom > "$dir/dist/AICC-9.9.9.dmg"

  # 假 App 二进制 + 假符号归档：release-publish.sh 现在会强制校验符号与包的一致性。
  local macos="$dir/dist/mac/AICC.app/Contents/MacOS"
  local widget="$dir/dist/mac/AICC.app/Contents/PlugIns/AICCWidget.appex/Contents/MacOS"
  local symbols="$dir/dist/symbols/9.9.9-99"
  mkdir -p "$macos" "$widget" "$symbols/AICC.dSYM" "$symbols/AICCWidget.dSYM"
  printf 'fake-host-binary' > "$macos/AICC"
  printf 'fake-widget-binary' > "$widget/AICCWidget"
  {
    echo "format=1"
    echo "version=9.9.9"
    echo "build=99"
    printf 'binary AICC %s %s %s\n' "Contents/MacOS/AICC" \
      "11111111-1111-1111-1111-111111111111" "$(shasum -a 256 "$macos/AICC" | awk '{ print $1 }')"
    printf 'binary AICCWidget %s %s %s\n' "Contents/PlugIns/AICCWidget.appex/Contents/MacOS/AICCWidget" \
      "11111111-1111-1111-1111-111111111111" "$(shasum -a 256 "$widget/AICCWidget" | awk '{ print $1 }')"
    printf 'dmg_sha256 %s\n' "$(shasum -a 256 "$dir/dist/AICC-9.9.9.dmg" | awk '{ print $1 }')"
  } > "$symbols/manifest.txt"
  printf '{"version":"0.0.0","build":"0","minimumSystemVersion":"14.0"}\n' > "$dir/updates/aicc-update.json"
  git -C "$dir" init -q -b main
  git -C "$dir" config user.email t@example.com
  git -C "$dir" config user.name tester
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" add -A
  git -C "$dir" commit -q -m base
  git init -q --bare "$remote"
  git -C "$dir" remote add origin "git@github.com:easonwong2026-del/AICC.git"
  git -C "$dir" config "url.$remote.insteadOf" "git@github.com:easonwong2026-del/AICC.git"
  git -C "$dir" push -q origin main
  echo "$dir"
}

tag_head() { git -C "$1" tag -a v9.9.9 -m test && git -C "$1" push -q origin v9.9.9; }
head_of() { git -C "$1" rev-parse HEAD; }
manifest_of() { cat "$1/updates/aicc-update.json"; }
fresh_state() { export FAKE_STATE="$TMP/$1-state"; rm -rf "$FAKE_STATE"; mkdir -p "$FAKE_STATE"; }

# ---------------------------------------------------------------- case 1
echo "case 1: dry-run without a tag"
DIR="$(make_repo dry)"
BEFORE_HEAD="$(head_of "$DIR")"
BEFORE_MANIFEST="$(manifest_of "$DIR")"
OUT="$("$DIR/scripts/release-publish.sh" --dry-run 2>&1)"; CODE=$?
check "exit 0" "[[ $CODE -eq 0 ]]"
check "mentions the missing tag" "grep -q 'not created yet' <<<'$OUT'"
check "plans both uploads" "grep -q 'upload AICC-9.9.9.dmg.sha256' <<<'$OUT'"
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"
check "manifest untouched" "[[ \"$(manifest_of "$DIR")\" == \"$BEFORE_MANIFEST\" ]]"

# ---------------------------------------------------------------- case 2
echo "case 2: real run, DMG upload fails"
DIR="$(make_repo badcdn)"; tag_head "$DIR"
BEFORE_HEAD="$(head_of "$DIR")"; BEFORE_MANIFEST="$(manifest_of "$DIR")"
fresh_state badcdn
OUT="$(FAKE_LOG="$TMP/badcdn.log" AICC_RELEASE_TOKEN=dummy FAKE_UPLOAD_DMG_STATUS=500 \
  "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "reports HTTP 500" "grep -q 'HTTP 500' <<<'$OUT'"
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"
check "manifest untouched" "[[ \"$(manifest_of "$DIR")\" == \"$BEFORE_MANIFEST\" ]]"

# ---------------------------------------------------------------- case 3
echo "case 3: real run, checksum upload fails"
DIR="$(make_repo badsha)"; tag_head "$DIR"
BEFORE_HEAD="$(head_of "$DIR")"; BEFORE_MANIFEST="$(manifest_of "$DIR")"
fresh_state badsha
OUT="$(FAKE_LOG="$TMP/badsha.log" AICC_RELEASE_TOKEN=dummy FAKE_UPLOAD_SHA_STATUS=502 \
  "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "reports HTTP 502" "grep -q 'HTTP 502' <<<'$OUT'"
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"
check "manifest untouched" "[[ \"$(manifest_of "$DIR")\" == \"$BEFORE_MANIFEST\" ]]"

# ---------------------------------------------------------------- case 4
echo "case 4: real run without a tag"
DIR="$(make_repo notag)"
BEFORE_HEAD="$(head_of "$DIR")"
LOG="$TMP/notag.log"; : > "$LOG"
OUT="$(FAKE_LOG="$LOG" AICC_RELEASE_TOKEN=dummy "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "says the tag is missing" "grep -q 'does not exist locally' <<<'$OUT'"
check "no HTTP calls" "[[ ! -s \"$LOG\" ]]"
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"

# ---------------------------------------------------------------- case 5
echo "case 5: release lookup fails with 500"
DIR="$(make_repo rel500)"; tag_head "$DIR"
BEFORE_HEAD="$(head_of "$DIR")"; BEFORE_MANIFEST="$(manifest_of "$DIR")"
fresh_state rel500
OUT="$(FAKE_LOG="$TMP/rel500.log" AICC_RELEASE_TOKEN=dummy FAKE_RELEASE_LOOKUP_STATUS=500 \
  "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "refuses to guess" "grep -q 'refusing to guess' <<<'$OUT'"
check "never creates a release" "! grep -q '^POST https://api.github.com.*/releases\$' \"$TMP/rel500.log\""
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"
check "manifest untouched" "[[ \"$(manifest_of "$DIR")\" == \"$BEFORE_MANIFEST\" ]]"

# ---------------------------------------------------------------- case 6
echo "case 6: release lookup returns invalid JSON"
DIR="$(make_repo relbadjson)"; tag_head "$DIR"
fresh_state relbadjson
OUT="$(FAKE_LOG="$TMP/relbadjson.log" AICC_RELEASE_TOKEN=dummy \
  FAKE_RELEASE_LOOKUP_BODY='{"nope":' "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "says JSON is unusable" "grep -q 'no usable JSON' <<<'$OUT'"

# ---------------------------------------------------------------- case 7
echo "case 7: asset list fails with 500"
DIR="$(make_repo assets500)"; tag_head "$DIR"
BEFORE_HEAD="$(head_of "$DIR")"; BEFORE_MANIFEST="$(manifest_of "$DIR")"
fresh_state assets500
OUT="$(FAKE_LOG="$TMP/assets500.log" AICC_RELEASE_TOKEN=dummy FAKE_ASSETS_STATUS=500 \
  "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "says the list query failed" "grep -q 'asset list query failed with HTTP 500' <<<'$OUT'"
check "no upload attempted" "! grep -q 'assets?name=' \"$TMP/assets500.log\""
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"
check "manifest untouched" "[[ \"$(manifest_of "$DIR")\" == \"$BEFORE_MANIFEST\" ]]"

# ---------------------------------------------------------------- case 8
echo "case 8: local tag points to another commit"
DIR="$(make_repo tagoflocal)"
git -C "$DIR" tag -a v9.9.9 -m test          # tag 落在 base
echo tweak >> "$DIR/docs/releases/9.9.9.md"
git -C "$DIR" commit -q -am "second"         # HEAD 前进一格
git -C "$DIR" push -q origin v9.9.9
BEFORE_HEAD="$(head_of "$DIR")"
fresh_state tagoflocal
OUT="$(FAKE_LOG="$TMP/tagoflocal.log" AICC_RELEASE_TOKEN=dummy "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "explains the mismatch" "grep -q 'not HEAD' <<<'$OUT'"
check "no HTTP calls" "[[ ! -s \"$TMP/tagoflocal.log\" ]]"
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"

# ---------------------------------------------------------------- case 9
echo "case 9: origin tag points to another commit"
DIR="$(make_repo tagofremote)"; tag_head "$DIR"
echo tweak >> "$DIR/docs/releases/9.9.9.md"
git -C "$DIR" commit -q -am "second"
git -C "$DIR" tag -f -a v9.9.9 -m moved      # 本地重打，origin 还是旧提交
fresh_state tagofremote
OUT="$(FAKE_LOG="$TMP/tagofremote.log" AICC_RELEASE_TOKEN=dummy "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "names origin" "grep -q 'origin tag v9.9.9' <<<'$OUT'"
check "no HTTP calls" "[[ ! -s \"$TMP/tagofremote.log\" ]]"

# ---------------------------------------------------------------- case 10
echo "case 10: happy path"
DIR="$(make_repo happy)"; tag_head "$DIR"
BEFORE_HEAD="$(head_of "$DIR")"
fresh_state happy
OUT="$(FAKE_LOG="$TMP/happy.log" AICC_RELEASE_TOKEN=dummy "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exit 0" "[[ $CODE -eq 0 ]]"
check "uploads both assets" "[[ \$(grep -c 'assets?name=' \"$TMP/happy.log\") -eq 2 ]]"
check "manifest updated" "python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d[\"version\"]==\"9.9.9\" and d[\"build\"]==\"99\" else 1)' '$DIR/updates/aicc-update.json'"
check "one new commit" "[[ \"$(head_of "$DIR")\" != \"$BEFORE_HEAD\" ]]"
check "pushed to origin main" "[[ \"$(git -C "$TMP/happy-remote.git" rev-parse main)\" == \"$(head_of "$DIR")\" ]]"

# ---------------------------------------------------------------- case 11
echo "case 11: no symbol manifest -> refuse to publish"
DIR="$(make_repo nosymbols)"; tag_head "$DIR"
rm -f "$DIR/dist/symbols/9.9.9-99/manifest.txt"
BEFORE_HEAD="$(head_of "$DIR")"
BEFORE_MANIFEST="$(manifest_of "$DIR")"
fresh_state nosymbols
OUT="$(FAKE_LOG="$TMP/nosymbols.log" AICC_RELEASE_TOKEN=dummy "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "names the missing manifest" "grep -q 'manifest.txt' <<<'$OUT'"
check "no HTTP calls" "[[ ! -s \"$TMP/nosymbols.log\" ]]"
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"
check "manifest untouched" "[[ \"$(manifest_of "$DIR")\" == \"$BEFORE_MANIFEST\" ]]"

# ---------------------------------------------------------------- case 12
echo "case 12: symbol archive belongs to another build -> refuse"
DIR="$(make_repo wrongsymbols)"; tag_head "$DIR"
python3 - "$DIR/dist/symbols/9.9.9-99/manifest.txt" <<'PYEOF'
import sys
path = sys.argv[1]
lines = []
for line in open(path):
    if line.startswith("binary AICC "):
        parts = line.split()
        parts[4] = "deadbeef"
        line = " ".join(parts) + "\n"
    lines.append(line)
open(path, "w").writelines(lines)
PYEOF
fresh_state wrongsymbols
OUT="$(FAKE_LOG="$TMP/wrongsymbols.log" AICC_RELEASE_TOKEN=dummy "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "says the symbols do not match" "grep -q '对不上' <<<'$OUT'"
check "no uploads" "! grep -q 'assets?name=' \"$TMP/wrongsymbols.log\" 2>/dev/null"

# ---------------------------------------------------------------- summary
echo
echo "release-publish mock tests: $PASS passed, $FAIL failed"
[[ $FAIL -eq 0 ]]
