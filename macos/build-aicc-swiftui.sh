#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERVER_ROOT="${SERVER_ROOT:-$ROOT}"
BUNDLE_SERVER="${BUNDLE_SERVER:-0}"

APP_NAME="AICC"
APP_DIR="$ROOT/dist/mac/$APP_NAME.app"
CONTENTS="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCES_DIR="$CONTENTS/Resources"
SOURCE_DIR="$ROOT/macos/MenuBarApp/Sources"

echo "=== Building AICC SwiftUI App ==="
echo "Root: $ROOT"

# Clean and create bundle structure
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

# Copy Info.plist
cp "$ROOT/macos/MenuBarApp/Info.plist" "$CONTENTS/Info.plist"

# Bundle server if requested
if [[ "$BUNDLE_SERVER" == "1" ]]; then
  BUNDLED_SERVER_DIR="$RESOURCES_DIR/Server"
  mkdir -p "$BUNDLED_SERVER_DIR"
  cp "$ROOT/server.py" "$ROOT/VERSION" "$ROOT/PACKAGE.json" "$BUNDLED_SERVER_DIR/"
  cp -R "$ROOT/collectors" "$ROOT/services" "$ROOT/web" "$BUNDLED_SERVER_DIR/"
  printf '%s\n' "@resources/Server" > "$RESOURCES_DIR/ServerRoot.txt"
else
  printf '%s\n' "$SERVER_ROOT" > "$RESOURCES_DIR/ServerRoot.txt"
fi

# Generate app icon
python3 -B "$ROOT/macos/make-app-icon.py" "$RESOURCES_DIR/AppIcon.icns"

# Collect all Swift source files
SWIFT_FILES=()
while IFS= read -r -d '' file; do
  SWIFT_FILES+=("$file")
done < <(find "$SOURCE_DIR" -name "*.swift" -print0)

echo "Swift sources: ${#SWIFT_FILES[@]} files"
for f in "${SWIFT_FILES[@]}"; do echo "  $f"; done

# Choose compatible SDK (prefer 15.4 which works with current CLT)
SDK_PATH=""
for candidate in \
  /Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk \
  /Library/Developer/CommandLineTools/SDKs/MacOSX15.sdk \
  /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk; do
  if [ -d "$candidate" ]; then
    SDK_PATH="$candidate"
    break
  fi
done

if [ -z "$SDK_PATH" ]; then
  SDK_PATH="$(xcrun --sdk macosx --show-sdk-path)"
fi

echo "SDK: $SDK_PATH"
echo "=== Compiling ==="

xcrun swiftc \
  -parse-as-library \
  -O \
  -sdk "$SDK_PATH" \
  -target arm64-apple-macosx14.0 \
  -framework SwiftUI \
  -framework AppKit \
  -framework Foundation \
  -module-cache-path /tmp/swift-module-cache \
  -Xlinker -rpath -Xlinker /usr/lib/swift \
  "${SWIFT_FILES[@]}" \
  -o "$MACOS_DIR/$APP_NAME"

echo "=== Signing ==="
codesign --force --deep --sign - "$APP_DIR" 2>/dev/null || true

echo "=== Done ==="
echo "$APP_DIR"
