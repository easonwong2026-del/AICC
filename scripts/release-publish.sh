#!/bin/bash
set -euo pipefail

# Publish the release for the version in VERSION: create the GitHub release,
# attach the DMG and its checksum, then point the update manifest at it.
#
# Usage:
#   bash scripts/release-publish.sh --dry-run   # print what would happen
#   bash scripts/release-publish.sh             # do it
#
# Needs, once per machine:
#   1. an SSH key registered on GitHub, so `git push` works
#      (see docs/RELEASING.md)
#   2. a personal access token in the macOS keychain for github.com, so the
#      REST calls work:
#      read -rs T; printf 'protocol=https\nhost=github.com\nusername=<login>\npassword=%s\n\n' "$T" | git credential-osxkeychain store
#
# Every step is idempotent: an existing release is reused, an asset that is
# already attached is skipped, and a manifest that already matches is not
# committed again.  Re-running after a partial failure is safe.
#
# 三条安全约束（2026-11-18 复审）：
#   1. **查询失败不许当成“不存在”**：Release 查询只有明确 404 才继续创建；
#      资产列表查询必须 200 且是合法 JSON，否则立刻停下。
#   2. **上传失败不许动清单**：任一资产非 2xx 直接退出，改清单前还要回查资产确实在。
#   3. **Tag 必须指向正确提交**：本地 Tag 解析出的 commit 必须等于 HEAD，
#      origin 上的同一个 Tag 也必须指向同一 commit。只“存在”不够。
#   `--dry-run` 不要求 Tag 存在——它的用途就是打 Tag 之前先看计划。

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$ROOT"

OWNER="easonwong2026-del"
REPO="AICC"
# Follow the repository this checkout actually points at, so a fork publishes
# to itself instead of the upstream project.  Falls back to the defaults above.
REMOTE="$(git config --get remote.origin.url 2>/dev/null || true)"
if [[ "$REMOTE" =~ github\.com[:/]+([^/]+)/([^/]+)$ ]]; then
  OWNER="${BASH_REMATCH[1]}"
  REPO="${BASH_REMATCH[2]%.git}"
fi
# 基址可覆盖：Mock 测试用它们把请求拦在本地，生产环境不用设。
API="${AICC_RELEASE_API_BASE:-https://api.github.com}/repos/$OWNER/$REPO"
UPLOAD="${AICC_RELEASE_UPLOAD_BASE:-https://uploads.github.com}/repos/$OWNER/$REPO"
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

VERSION="$(tr -d '[:space:]' < VERSION)"
TAG="v$VERSION"
APP_NAME="AICC"
DMG="dist/$APP_NAME-$VERSION.dmg"
SHA="$DMG.sha256"
NOTES="docs/releases/$VERSION.md"
APP_PLIST="dist/mac/AICC.app/Contents/Info.plist"

die() { echo "error: $*" >&2; exit 1; }
say() { echo "==> $*"; }

[[ -f "$DMG" ]] || die "missing $DMG — run: bash macos/build-release-mac.sh"
[[ -f "$NOTES" ]] || die "missing $NOTES — write the release notes first"
[[ -f "$APP_PLIST" ]] || die "missing $APP_PLIST — build the app first"
BUILD="$(plutil -extract CFBundleVersion raw -o - "$APP_PLIST")"
[[ -n "$BUILD" ]] || die "cannot read CFBundleVersion"

# 发布前最后一道门：符号必须与这个 DMG 是同一次构建。
# 没有它，线上崩溃只能拿到地址；记错版本号的符号比没有符号更糟——它会
# 把崩溃指到错误的代码行。校验内容：App 内二进制 ↔ dSYM ↔ manifest ↔ DMG hash。
SYMBOLS_DIR="$ROOT/dist/symbols/$VERSION-$BUILD"
[[ -f "$SYMBOLS_DIR/manifest.txt" ]] \
  || die "missing $SYMBOLS_DIR/manifest.txt — 没有符号清单就不能发布（build-dmg.sh 会生成）"
bash "$ROOT/scripts/verify-symbols.sh" \
  --app "$ROOT/dist/mac/$APP_NAME.app" \
  --symbols "$SYMBOLS_DIR" \
  --dmg "$DMG" >/dev/null \
  || die "符号归档与 $DMG 对不上，拒绝发布（手工复核：bash scripts/verify-symbols.sh --app dist/mac/$APP_NAME.app --symbols $SYMBOLS_DIR --dmg ${DMG}）"
say "symbols: $(basename "$SYMBOLS_DIR") 与 DMG 一致"

HEAD_SHA="$(git rev-parse HEAD)"

# HTTP GET 的统一出口：body 写文件、回显状态码；传输出错返回 000。
# 调用方必须自己判断状态码——把失败当“不存在”正是复审要修的问题。
api_get() {
  local url="$1" out="$2"
  curl -sS --max-time 20 -o "$out" -w '%{http_code}' "$url" 2>/dev/null || echo 000
}

# ---- tag -----------------------------------------------------------------
# 实跑要求：Tag 存在、指向 HEAD、origin 上一致。dry-run 只报告状态。
REMOTE_TAG_SHA=""
if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
  TAG_SHA="$(git rev-parse "$TAG^{commit}")"
  REMOTE_TAGS="$(git ls-remote origin "refs/tags/$TAG" "refs/tags/$TAG^{}" 2>/dev/null || true)"
  if [[ -n "$REMOTE_TAGS" ]]; then
    REMOTE_TAG_SHA="$(printf '%s\n' "$REMOTE_TAGS" | python3 -c '
import sys
wanted = "refs/tags/" + sys.argv[1]
plain = peeled = ""
for line in sys.stdin:
    parts = line.split()
    if len(parts) != 2:
        continue
    sha, ref = parts
    if ref == wanted + "^{}":
        peeled = sha
    elif ref == wanted:
        plain = sha
print(peeled or plain)
' "$TAG")"
  fi
fi

if [[ $DRY_RUN -eq 0 ]]; then
  [[ -n "${TAG_SHA:-}" ]] || die "tag $TAG does not exist locally"
  [[ "$TAG_SHA" == "$HEAD_SHA" ]] \
    || die "tag $TAG points to $TAG_SHA, not HEAD $HEAD_SHA — 发布会指向错误的提交"
  [[ -n "$REMOTE_TAG_SHA" ]] || die "tag $TAG is not on origin — run: git push origin $TAG"
  [[ "$REMOTE_TAG_SHA" == "$HEAD_SHA" ]] \
    || die "origin tag $TAG points to $REMOTE_TAG_SHA, not HEAD $HEAD_SHA"
  say "tag $TAG -> $HEAD_SHA (local and origin agree)"
else
  if [[ -z "${TAG_SHA:-}" ]]; then
    say "tag $TAG not created yet — a real run requires it locally and on origin"
  elif [[ "$TAG_SHA" != "$HEAD_SHA" ]]; then
    say "tag $TAG points to $TAG_SHA, HEAD is $HEAD_SHA — a real run would refuse"
  else
    say "tag $TAG -> $HEAD_SHA"
  fi
fi

# The checksum file ships next to the DMG; regenerate it if it is missing or
# stale so the two can never disagree.
EXPECTED_SHA="$(shasum -a 256 "$DMG" | awk '{print $1}')"
if [[ ! -f "$SHA" || "$(cat "$SHA")" != "$EXPECTED_SHA" ]]; then
  say "writing $(basename "$SHA")"
  [[ $DRY_RUN -eq 1 ]] || printf '%s\n' "$EXPECTED_SHA" > "$SHA"
fi

TOKEN=""
if [[ $DRY_RUN -eq 0 ]]; then
  TOKEN="${AICC_RELEASE_TOKEN:-$(printf 'protocol=https\nhost=github.com\n\n' | git credential fill 2>/dev/null | sed -n 's/^password=//p')}"
  [[ -n "$TOKEN" ]] || die "no github.com credential in the keychain — see docs/RELEASING.md"
  TOKEN_CHECK="$(mktemp)"
  CODE="$(curl -sS --max-time 20 -o "$TOKEN_CHECK" -w '%{http_code}' \
    -H "Authorization: Bearer $TOKEN" "$API" 2>/dev/null || echo 000)"
  rm -f "$TOKEN_CHECK"
  [[ "$CODE" == "200" ]] || die "the stored credential cannot read $OWNER/$REPO (HTTP $CODE)"
fi

say "version $VERSION, macOS build $BUILD, tag $TAG"

# ---- release -------------------------------------------------------------
# 只有 404 才算“还没有 Release”。401/403/429/5xx/超时/坏 JSON 都必须停下：
# 把它们当成不存在，会在错误前提下建 Release。
RELEASE_ID=""
REL_BODY="$(mktemp)"
REL_CODE="$(api_get "$API/releases/tags/$TAG" "$REL_BODY")"
case "$REL_CODE" in
  200)
    RELEASE_ID="$(python3 - "$REL_BODY" <<'PY'
import json, sys
try:
    data = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    sys.exit(2)
if not isinstance(data, dict) or not data.get("id"):
    sys.exit(3)
print(data["id"])
PY
)" || { rm -f "$REL_BODY"; die "release lookup returned HTTP 200 but no usable JSON"; }
    say "release $TAG already exists (id $RELEASE_ID), reusing it"
    ;;
  404)
    RELEASE_ID=""
    ;;
  *)
    rm -f "$REL_BODY"
    if [[ $DRY_RUN -eq 1 ]]; then
      say "release lookup failed (HTTP $REL_CODE) — dry-run cannot tell whether it exists"
      RELEASE_ID=""
    else
      die "release lookup failed with HTTP $REL_CODE — refusing to guess"
    fi
    ;;
esac
rm -f "$REL_BODY"

if [[ -z "$RELEASE_ID" ]]; then
  if [[ $DRY_RUN -eq 1 ]]; then
    say "would create release $TAG"
    echo "    POST $API/releases (name=AICC $VERSION / macOS Build $BUILD, body=$NOTES)"
  else
    say "creating release $TAG"
    REQ="$(mktemp)"
    python3 - "$VERSION" "$BUILD" "$NOTES" > "$REQ" <<'PY'
import json, sys
version, build, notes = sys.argv[1], sys.argv[2], sys.argv[3]
print(json.dumps({
    "tag_name": f"v{version}",
    "name": f"AICC {version} / macOS Build {build}",
    "body": open(notes, encoding="utf-8").read(),
    "draft": False,
    "prerelease": False,
}, ensure_ascii=False))
PY
    RESP="$(mktemp)"
    CODE="$(curl -sS --max-time 60 -o "$RESP" -w '%{http_code}' -X POST "$API/releases" \
      -H "Authorization: Bearer $TOKEN" -H "Accept: application/vnd.github+json" \
      -H "Content-Type: application/json" --data-binary "@$REQ" 2>/dev/null || echo 000)"
    rm -f "$REQ"
    if [[ "$CODE" != 2* ]]; then
      head -c 300 "$RESP" >&2 || true
      rm -f "$RESP"
      die "creating the release failed with HTTP $CODE"
    fi
    RELEASE_ID="$(python3 - "$RESP" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
print(data["id"])
PY
)" || { rm -f "$RESP"; die "creating the release returned no id"; }
    rm -f "$RESP"
    say "release id $RELEASE_ID"
  fi
fi

# ---- assets --------------------------------------------------------------
# 先一次拿到资产列表：查询失败必须停下，不能把 401/500/超时/坏 JSON 当成“没有资产”。
# bash 3.2 没有 mapfile，用 while read 往数组里塞。
ASSET_NAMES=()
if [[ -n "$RELEASE_ID" && $DRY_RUN -eq 0 ]]; then
  ASSETS_BODY="$(mktemp)"
  CODE="$(curl -sS --max-time 20 -o "$ASSETS_BODY" -w '%{http_code}' \
    -H "Authorization: Bearer $TOKEN" "$API/releases/$RELEASE_ID/assets" 2>/dev/null || echo 000)"
  if [[ "$CODE" != "200" ]]; then
    rm -f "$ASSETS_BODY"
    die "asset list query failed with HTTP $CODE — refusing to guess"
  fi
  NAMES="$(python3 - "$ASSETS_BODY" <<'PY'
import json, sys
try:
    data = json.load(open(sys.argv[1], encoding="utf-8"))
except Exception:
    sys.exit(2)
if not isinstance(data, list):
    sys.exit(3)
for asset in data:
    name = asset.get("name")
    if name:
        print(name)
PY
)" || { rm -f "$ASSETS_BODY"; die "asset list is not valid JSON"; }
  rm -f "$ASSETS_BODY"
  while IFS= read -r line; do
    [[ -n "$line" ]] && ASSET_NAMES+=("$line")
  done <<< "$NAMES"
fi

has_asset() {
  local needle="$1" name
  for name in ${ASSET_NAMES[@]+"${ASSET_NAMES[@]}"}; do
    [[ "$name" == "$needle" ]] && return 0
  done
  return 1
}

for FILE in "$DMG" "$SHA"; do
  NAME="$(basename "$FILE")"
  if [[ $DRY_RUN -eq 1 ]]; then
    # dry-run 不落 sha 文件，但计划里必须列出它，否则看起来像“只传 DMG”。
    echo "    upload $NAME to release ${RELEASE_ID:-<new>}"
    continue
  fi
  [[ -f "$FILE" ]] || die "missing $NAME — the release assets must both exist"
  if has_asset "$NAME"; then
    say "asset $NAME already attached, skipping"
    continue
  fi
  say "uploading $NAME"
  case "$NAME" in
    *.dmg) CONTENT_TYPE="application/x-apple-diskimage" ;;
    *)     CONTENT_TYPE="application/octet-stream" ;;
  esac
  BODY="$(mktemp)"
  CODE="$(curl -sS --max-time 300 -X POST "$UPLOAD/releases/$RELEASE_ID/assets?name=$NAME" \
    -H "Authorization: Bearer $TOKEN" -H "Content-Type: $CONTENT_TYPE" \
    --data-binary "@$FILE" -o "$BODY" -w '%{http_code}' 2>/dev/null || echo 000)"
  if [[ "$CODE" != 2* ]]; then
    echo "    body: $(head -c 300 "$BODY" 2>/dev/null)" >&2
    rm -f "$BODY"
    # 上传没成功就直接退出：绝不能继续去改自动更新清单。
    die "upload $NAME failed with HTTP $CODE"
  fi
  rm -f "$BODY"
  ASSET_NAMES+=("$NAME")
  say "  uploaded $NAME (HTTP $CODE)"
done

# ---- verify assets before touching the manifest --------------------------
if [[ $DRY_RUN -eq 0 ]]; then
  VERIFY_BODY="$(mktemp)"
  CODE="$(curl -sS --max-time 20 -o "$VERIFY_BODY" -w '%{http_code}' \
    -H "Authorization: Bearer $TOKEN" "$API/releases/$RELEASE_ID/assets" 2>/dev/null || echo 000)"
  if [[ "$CODE" != "200" ]]; then
    rm -f "$VERIFY_BODY"
    die "post-upload asset check failed with HTTP $CODE"
  fi
  VERIFIED="$(python3 - "$VERIFY_BODY" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
for asset in data:
    name = asset.get("name")
    if name:
        print(name)
PY
)" || { rm -f "$VERIFY_BODY"; die "post-upload asset list is not valid JSON"; }
  rm -f "$VERIFY_BODY"
  for FILE in "$DMG" "$SHA"; do
    NAME="$(basename "$FILE")"
    if ! printf '%s\n' "$VERIFIED" | grep -Fxq "$NAME"; then
      die "asset $NAME is missing from release $RELEASE_ID — manifest left untouched"
    fi
  done
  say "both assets verified on the release"
fi

# ---- update manifest -----------------------------------------------------
if [[ $DRY_RUN -eq 0 ]]; then
  REL_BODY="$(mktemp)"
  CODE="$(api_get "$API/releases/$RELEASE_ID" "$REL_BODY")"
  if [[ "$CODE" != "200" ]]; then
    rm -f "$REL_BODY"
    die "release metadata query failed with HTTP $CODE"
  fi
  PUBLISHED="$(python3 - "$REL_BODY" <<'PY'
import json, sys
data = json.load(open(sys.argv[1], encoding="utf-8"))
published = data.get("published_at")
if not published:
    sys.exit(2)
print(published)
PY
)" || { rm -f "$REL_BODY"; die "release has no published_at — refusing to write a manifest without it"; }
  rm -f "$REL_BODY"
  python3 - "$VERSION" "$BUILD" "$PUBLISHED" "$OWNER" "$REPO" <<'PY'
import json, pathlib, sys
version, build, published, owner, repo = sys.argv[1:6]
path = pathlib.Path("updates/aicc-update.json")
current = json.loads(path.read_text(encoding="utf-8"))
updated = {
    "version": version,
    "build": build,
    "minimumSystemVersion": current.get("minimumSystemVersion", "14.0"),
    "downloadURL": f"https://github.com/{owner}/{repo}/releases/download/v{version}/AICC-{version}.dmg",
    "releaseNotesURL": f"https://github.com/{owner}/{repo}/releases/tag/v{version}",
    "publishedAt": published,
}
if updated == current:
    print("manifest already current")
else:
    path.write_text(json.dumps(updated, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print("manifest updated")
PY
  if git diff --quiet -- updates/aicc-update.json; then
    say "nothing to commit"
  else
    git add updates/aicc-update.json
    git commit -q -m "chore(release): point the update manifest at v$VERSION"
    git push origin HEAD:main
    say "manifest pushed"
  fi
else
  echo "    update updates/aicc-update.json for $VERSION / build $BUILD, commit and push"
fi

say "done: https://github.com/$OWNER/$REPO/releases/tag/$TAG"
