#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"
PACKAGE_VERSION="$(python3 -c 'import json, sys; print(json.load(open(sys.argv[1], encoding="utf-8"))["version"])' "$ROOT/PACKAGE.json")"
PLIST_VERSION="$(plutil -extract CFBundleShortVersionString raw -o - "$ROOT/macos/MenuBarApp/Info.plist")"

[[ "$VERSION" == "$PACKAGE_VERSION" ]] || {
  echo "Version mismatch: VERSION=$VERSION PACKAGE.json=$PACKAGE_VERSION" >&2
  exit 1
}
[[ "$VERSION" == "$PLIST_VERSION" ]] || {
  echo "Version mismatch: VERSION=$VERSION Info.plist=$PLIST_VERSION" >&2
  exit 1
}

if [[ "${1:-}" == "--dmg" ]]; then
  test -s "$ROOT/dist/AICC-$VERSION.dmg"
fi

echo "Version $VERSION is consistent."
