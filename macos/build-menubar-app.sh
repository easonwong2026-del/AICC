#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERVER_ROOT="${SERVER_ROOT:-$ROOT}"
APP_NAME="AI E-Ink Dashboard"
APP_DIR="$ROOT/dist/mac/$APP_NAME.app"
CONTENTS="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCES_DIR="$CONTENTS/Resources"
SOURCE_DIR="$ROOT/macos/MenuBarApp/Sources"

rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp "$ROOT/macos/MenuBarApp/Info.plist" "$CONTENTS/Info.plist"
printf '%s\n' "$SERVER_ROOT" > "$RESOURCES_DIR/ServerRoot.txt"
python3 -B "$ROOT/macos/make-app-icon.py" "$RESOURCES_DIR/AppIcon.icns"

clang \
  -Os \
  -fobjc-arc \
  -mmacosx-version-min=13.0 \
  -framework Cocoa \
  "$SOURCE_DIR/main.m" \
  -o "$MACOS_DIR/$APP_NAME"

plutil -lint "$CONTENTS/Info.plist"
codesign --force --deep --sign - "$APP_DIR" >/dev/null

echo "$APP_DIR"
