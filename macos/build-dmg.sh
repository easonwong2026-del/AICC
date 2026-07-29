#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="AICC"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"
DMG_NAME="$APP_NAME-$VERSION.dmg"
APP_DIR="$ROOT/dist/mac/$APP_NAME.app"
STAGING_DIR="$ROOT/dist/dmg"
RELEASE_DIR="${RELEASE_DIR:-$ROOT/dist}"
DMG_PATH="$RELEASE_DIR/$DMG_NAME"
TEMP_DMG="$ROOT/dist/$DMG_NAME.tmp.dmg"

BUNDLE_SERVER=1 "$ROOT/macos/build-aicc-swiftui.sh" >/dev/null

rm -rf "$STAGING_DIR" "$TEMP_DMG" "$DMG_PATH"
mkdir -p "$STAGING_DIR" "$RELEASE_DIR"
cp -R "$APP_DIR" "$STAGING_DIR/"
ln -s /Applications "$STAGING_DIR/Applications"

hdiutil create \
  -volname "$APP_NAME $VERSION" \
  -srcfolder "$STAGING_DIR" \
  -ov \
  -format UDZO \
  "$TEMP_DMG" >/dev/null
mv "$TEMP_DMG" "$DMG_PATH"
rm -rf "$STAGING_DIR"

echo "$DMG_PATH"
