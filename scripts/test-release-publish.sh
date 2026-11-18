#!/bin/bash
# 发布脚本的 Mock 测试（PR #105 复审要求）。
#
# 只测三件在真实发版里最贵的事：
#   1. `--dry-run` 在还没有 Tag 的时候也能跑完并给出计划；
#   2. 资产上传失败时**不碰**自动更新清单，也不提交、不推送；
#   3. 实跑时 Tag 缺失会先失败，一个 API 请求都不发。
#
# 做法：临时目录里造一个"仓库"（含 VERSION / 发布说明 / 假 DMG / 假 Info.plist），
# 用一个假 `curl` 拦掉网络，git 走真实的本地 bare 仓库（origin 用 insteadOf 映射），
# 这样 commit / push / diff 全是真执行，只有 HTTP 是假的。
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
    if [[ "${FAKE_RELEASE_EXISTS:-1}" == "1" ]]; then emit '{"id":900}'; else emit '{"message":"Not Found"}' 404; fi ;;
  *assets?name=*)
    name="${url##*name=}"
    if [[ "$name" == *.dmg ]]; then code="${FAKE_UPLOAD_DMG_STATUS:-201}"; else code="${FAKE_UPLOAD_SHA_STATUS:-201}"; fi
    if [[ "$code" == 2* ]]; then printf '%s\n' "$name" >> "${FAKE_STATE}/assets"; fi
    emit '{"id":1}' "$code" ;;
  */releases/900/assets)
    body="[]"
    if [[ -s "${FAKE_STATE}/assets" ]]; then
      body="$(python3 -c 'import json,sys; print(json.dumps([{"name":n.strip()} for n in open(sys.argv[1]) if n.strip()]))' "${FAKE_STATE}/assets")"
    fi
    emit "$body" ;;
  */releases/900)
    emit '{"id":900,"published_at":"2026-11-18T00:00:00Z"}' ;;
  */releases)
    emit '{"id":900}' 201 ;;
  *)
    emit '' 200 ;;
esac
FAKE
chmod +x "$TMP/bin/curl"
export PATH="$TMP/bin:$PATH"

# ---------------------------------------------------------------- repo maker
# make_repo NAME → 回声临时仓库路径；同时把 bare 远端放在 $TMP/NAME-remote.git
make_repo() {
  local name="$1" dir="$TMP/$1" remote="$TMP/$1-remote.git"
  mkdir -p "$dir/scripts" "$dir/docs/releases" "$dir/dist/mac/AICC.app/Contents" "$dir/updates"
  cp "$SRC_ROOT/scripts/release-publish.sh" "$dir/scripts/"
  printf '9.9.9\n' > "$dir/VERSION"
  cat > "$dir/dist/mac/AICC.app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>CFBundleVersion</key><string>99</string></dict></plist>
PLIST
  printf '# notes\n' > "$dir/docs/releases/9.9.9.md"
  head -c 2048 /dev/urandom > "$dir/dist/AICC-9.9.9.dmg"
  printf '{"version":"0.0.0","build":"0","minimumSystemVersion":"14.0"}\n' > "$dir/updates/aicc-update.json"
  git -C "$dir" init -q -b main
  git -C "$dir" config user.email t@example.com
  git -C "$dir" config user.name tester
  git -C "$dir" config commit.gpgsign false
  git -C "$dir" add -A
  git -C "$dir" commit -q -m base
  git init -q --bare "$remote"
  git -C "$dir" remote add origin "git@github.com:easonwong2026-del/AICC.git"
  # 让 push / ls-remote 真的走本地 bare 仓库
  git -C "$dir" config "url.$remote.insteadOf" "git@github.com:easonwong2026-del/AICC.git"
  git -C "$dir" push -q origin main
  echo "$dir"
}

head_of() { git -C "$1" rev-parse HEAD; }
manifest_of() { cat "$1/updates/aicc-update.json"; }

# ---------------------------------------------------------------- case 1
echo "case 1: dry-run without a tag"
DIR="$(make_repo dry)"
BEFORE_HEAD="$(head_of "$DIR")"
BEFORE_MANIFEST="$(manifest_of "$DIR")"
OUT="$("$DIR/scripts/release-publish.sh" --dry-run 2>&1)"; CODE=$?
check "exit 0" "[[ $CODE -eq 0 ]]"
check "mentions the missing tag" "grep -q 'not created yet' <<<\"\$OUT\""
check "prints the plan" "grep -q 'upload AICC-9.9.9.dmg' <<<\"\$OUT\""
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"
check "manifest untouched" "[[ \"$(manifest_of "$DIR")\" == \"$BEFORE_MANIFEST\" ]]"

# ---------------------------------------------------------------- case 2
echo "case 2: real run, DMG upload fails"
DIR="$(make_repo badcdn)"
git -C "$DIR" tag -a v9.9.9 -m test
git -C "$DIR" push -q origin v9.9.9
BEFORE_HEAD="$(head_of "$DIR")"
BEFORE_MANIFEST="$(manifest_of "$DIR")"
export FAKE_STATE="$TMP/badcdn-state"; mkdir -p "$FAKE_STATE"
OUT="$(FAKE_LOG="$TMP/badcdn.log" AICC_RELEASE_TOKEN=dummy FAKE_UPLOAD_DMG_STATUS=500 \
  "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "reports HTTP 500" "grep -q 'HTTP 500' <<<\"\$OUT\""
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"
check "manifest untouched" "[[ \"$(manifest_of "$DIR")\" == \"$BEFORE_MANIFEST\" ]]"

# ---------------------------------------------------------------- case 3
echo "case 3: real run, checksum upload fails"
DIR="$(make_repo badsha)"
git -C "$DIR" tag -a v9.9.9 -m test
git -C "$DIR" push -q origin v9.9.9
BEFORE_HEAD="$(head_of "$DIR")"
BEFORE_MANIFEST="$(manifest_of "$DIR")"
export FAKE_STATE="$TMP/badsha-state"; mkdir -p "$FAKE_STATE"
OUT="$(FAKE_LOG="$TMP/badsha.log" AICC_RELEASE_TOKEN=dummy FAKE_UPLOAD_SHA_STATUS=502 \
  "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "reports HTTP 502" "grep -q 'HTTP 502' <<<\"\$OUT\""
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"
check "manifest untouched" "[[ \"$(manifest_of "$DIR")\" == \"$BEFORE_MANIFEST\" ]]"

# ---------------------------------------------------------------- case 4
echo "case 4: real run without a tag"
DIR="$(make_repo notag)"
BEFORE_HEAD="$(head_of "$DIR")"
LOG="$TMP/notag.log"; : > "$LOG"
OUT="$(FAKE_LOG="$LOG" AICC_RELEASE_TOKEN=dummy "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exits non-zero" "[[ $CODE -ne 0 ]]"
check "says the tag is missing" "grep -q 'does not exist locally' <<<\"\$OUT\""
check "no HTTP calls" "[[ ! -s \"$LOG\" ]]"
check "no commit" "[[ \"$(head_of "$DIR")\" == \"$BEFORE_HEAD\" ]]"

# ---------------------------------------------------------------- case 5
echo "case 5: happy path"
DIR="$(make_repo happy)"
git -C "$DIR" tag -a v9.9.9 -m test
git -C "$DIR" push -q origin v9.9.9
BEFORE_HEAD="$(head_of "$DIR")"
export FAKE_STATE="$TMP/happy-state"; mkdir -p "$FAKE_STATE"
OUT="$(FAKE_LOG="$TMP/happy.log" AICC_RELEASE_TOKEN=dummy "$DIR/scripts/release-publish.sh" 2>&1)"; CODE=$?
check "exit 0" "[[ $CODE -eq 0 ]]"
check "uploads both assets" "[[ \$(grep -c 'assets?name=' "$TMP/happy.log") -eq 2 ]]"
check "manifest updated" "python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(0 if d[\"version\"]==\"9.9.9\" and d[\"build\"]==\"99\" else 1)' '$DIR/updates/aicc-update.json'"
check "one new commit" "[[ \"$(head_of "$DIR")\" != \"$BEFORE_HEAD\" ]]"
check "pushed to origin main" "[[ \"$(git -C "$TMP/happy-remote.git" rev-parse main)\" == \"$(head_of "$DIR")\" ]]"

# ---------------------------------------------------------------- summary
echo
echo "release-publish mock tests: $PASS passed, $FAIL failed"
[[ $FAIL -eq 0 ]]
