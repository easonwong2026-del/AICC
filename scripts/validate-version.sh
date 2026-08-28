#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"
APP_PLIST="$ROOT/dist/mac/AICC.app/Contents/Info.plist"
WIDGET_PLIST="$ROOT/dist/mac/AICC.app/Contents/PlugIns/AICCWidget.appex/Contents/Info.plist"

[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  echo "Invalid VERSION: $VERSION" >&2
  exit 1
}
[[ -f "$APP_PLIST" ]] || {
  echo "Built app metadata not found: $APP_PLIST" >&2
  exit 1
}
[[ -f "$WIDGET_PLIST" ]] || {
  echo "Built widget metadata not found: $WIDGET_PLIST" >&2
  exit 1
}

PLIST_VERSION="$(plutil -extract CFBundleShortVersionString raw -o - "$APP_PLIST")"
BUILD_VERSION="$(plutil -extract CFBundleVersion raw -o - "$APP_PLIST")"

[[ "$VERSION" == "$PLIST_VERSION" ]] || {
  echo "Version mismatch: VERSION=$VERSION generated Info.plist=$PLIST_VERSION" >&2
  exit 1
}
[[ -n "$BUILD_VERSION" ]] || {
  echo "Generated Info.plist is missing CFBundleVersion" >&2
  exit 1
}

WIDGET_VERSION="$(plutil -extract CFBundleShortVersionString raw -o - "$WIDGET_PLIST")"
WIDGET_BUILD="$(plutil -extract CFBundleVersion raw -o - "$WIDGET_PLIST")"
[[ "$VERSION" == "$WIDGET_VERSION" ]] || {
  echo "Version mismatch: VERSION=$VERSION generated Widget Info.plist=$WIDGET_VERSION" >&2
  exit 1
}
[[ "$BUILD_VERSION" == "$WIDGET_BUILD" ]] || {
  echo "Build mismatch: App=$BUILD_VERSION generated Widget Info.plist=$WIDGET_BUILD" >&2
  exit 1
}

if [[ "${1:-}" == "--dmg" ]]; then
  test -s "$ROOT/dist/AICC-$VERSION.dmg"
elif [[ $# -gt 0 ]]; then
  echo "Usage: bash scripts/validate-version.sh [--dmg]" >&2
  exit 2
fi

echo "Version $VERSION matches generated Info.plist (build $BUILD_VERSION)."
