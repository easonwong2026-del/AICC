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

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
cd "$ROOT"

OWNER="easonwong2026-del"
REPO="AICC"
API="https://api.github.com/repos/$OWNER/$REPO"
DRY_RUN=0
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=1

VERSION="$(tr -d '[:space:]' < VERSION)"
TAG="v$VERSION"
DMG="dist/AICC-$VERSION.dmg"
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

git rev-parse -q --verify "refs/tags/$TAG" >/dev/null || die "tag $TAG does not exist locally"
if ! git ls-remote --exit-code --tags origin "$TAG" >/dev/null 2>&1; then
  die "tag $TAG is not on origin — run: git push origin $TAG"
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
  TOKEN="$(printf 'protocol=https\nhost=github.com\n\n' | git credential fill 2>/dev/null | sed -n 's/^password=//p')"
  [[ -n "$TOKEN" ]] || die "no github.com credential in the keychain — see docs/RELEASING.md"
  curl -sS --max-time 20 -o /dev/null -w '' -H "Authorization: Bearer $TOKEN" "$API" \
    || die "the stored credential cannot read $OWNER/$REPO"
fi

say "version $VERSION, macOS build $BUILD, tag $TAG"

# ---- release -------------------------------------------------------------
RELEASE_ID="$(curl -sS --max-time 20 "$API/releases/tags/$TAG" \
  | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("id", ""))' 2>/dev/null || true)"

if [[ -z "$RELEASE_ID" ]]; then
  say "creating release $TAG"
  if [[ $DRY_RUN -eq 1 ]]; then
    echo "    POST $API/releases (name=AICC $VERSION / macOS Build $BUILD, body=$NOTES)"
  else
    python3 - "$VERSION" "$BUILD" "$NOTES" > /tmp/aicc-release.json <<'PY'
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
    RELEASE_ID="$(curl -sS -X POST "$API/releases" \
      -H "Authorization: Bearer $TOKEN" -H "Accept: application/vnd.github+json" \
      -H "Content-Type: application/json" --data-binary @/tmp/aicc-release.json \
      | python3 -c 'import json,sys; d=json.load(sys.stdin); print(d.get("id") or sys.exit(d.get("message")))')"
    rm -f /tmp/aicc-release.json
    say "release id $RELEASE_ID"
  fi
else
  say "release $TAG already exists (id $RELEASE_ID), reusing it"
fi

# ---- assets --------------------------------------------------------------
for FILE in "$DMG" "$SHA"; do
  NAME="$(basename "$FILE")"
  [[ -f "$FILE" ]] || { say "skip $NAME (not built)"; continue; }
  PRESENT="False"
  if [[ -n "$RELEASE_ID" ]]; then
    AUTH=()
    [[ -n "$TOKEN" ]] && AUTH=(-H "Authorization: Bearer $TOKEN")
    PRESENT="$(curl -sS --max-time 20 ${AUTH[@]+"${AUTH[@]}"} "$API/releases/$RELEASE_ID/assets" \
      | python3 -c "import json,sys; print(any(a['name']==sys.argv[1] for a in json.load(sys.stdin)))" "$NAME" 2>/dev/null || echo False)"
  fi
  if [[ "$PRESENT" == "True" ]]; then
    say "asset $NAME already attached, skipping"
    continue
  fi
  if [[ $DRY_RUN -eq 1 ]]; then
    echo "    upload $NAME to release ${RELEASE_ID:-<new>}"
    continue
  fi
  say "uploading $NAME"
  case "$NAME" in
    *.dmg) CONTENT_TYPE="application/x-apple-diskimage" ;;
    *)     CONTENT_TYPE="application/octet-stream" ;;
  esac
  curl -sS -X POST "https://uploads.github.com/repos/$OWNER/$REPO/releases/$RELEASE_ID/assets?name=$NAME" \
    -H "Authorization: Bearer $TOKEN" -H "Content-Type: $CONTENT_TYPE" \
    --data-binary "@$FILE" -o /dev/null -w '    http=%{http_code}\n'
done

# ---- update manifest -----------------------------------------------------
if [[ $DRY_RUN -eq 0 ]]; then
  PUBLISHED="$(curl -sS --max-time 20 "$API/releases/$RELEASE_ID" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["published_at"])')"
  python3 - "$VERSION" "$BUILD" "$PUBLISHED" <<'PY'
import json, pathlib, sys
version, build, published = sys.argv[1:4]
path = pathlib.Path("updates/aicc-update.json")
current = json.loads(path.read_text(encoding="utf-8"))
updated = {
    "version": version,
    "build": build,
    "minimumSystemVersion": current.get("minimumSystemVersion", "14.0"),
    "downloadURL": f"https://github.com/easonwong2026-del/AICC/releases/download/v{version}/AICC-{version}.dmg",
    "releaseNotesURL": f"https://github.com/easonwong2026-del/AICC/releases/tag/v{version}",
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
