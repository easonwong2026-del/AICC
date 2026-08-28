#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERVER_ROOT="${SERVER_ROOT:-$ROOT}"
BUNDLE_SERVER="${BUNDLE_SERVER:-0}"
SIGNING_IDENTITY="${AICC_SIGNING_IDENTITY:--}"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"

# Required server directories for BUNDLE_SERVER=1
REQUIRED_SERVER_DIRS=("collectors" "services" "web")
REQUIRED_SERVER_FILES=("macos/start-workbuddy-monitored.sh")

APP_NAME="AICC"
APP_DIR="$ROOT/dist/mac/$APP_NAME.app"
CONTENTS="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS/MacOS"
RESOURCES_DIR="$CONTENTS/Resources"
SOURCE_DIR="$ROOT/macos/MenuBarApp/Sources"
WIDGET_SOURCE_DIR="$ROOT/macos/Widget"
WIDGET_APP_DIR="$CONTENTS/PlugIns/AICCWidget.appex"
WIDGET_MACOS_DIR="$WIDGET_APP_DIR/Contents/MacOS"
WIDGET_INFO="$WIDGET_APP_DIR/Contents/Info.plist"
WIDGET_ENTITLEMENTS="$WIDGET_SOURCE_DIR/entitlements.plist"
BUILD_VERSION="$(plutil -extract CFBundleVersion raw -o - "$ROOT/macos/MenuBarApp/Info.plist")"

echo "=== Building AICC SwiftUI App ==="
echo "Root: $ROOT"

# Clean and create bundle structure
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR" "$WIDGET_MACOS_DIR"

# Copy Info.plist
cp "$ROOT/macos/MenuBarApp/Info.plist" "$CONTENTS/Info.plist"
plutil -insert CFBundleShortVersionString -string "$VERSION" "$CONTENTS/Info.plist"

# Copy localized resources
if [ -d "$ROOT/macos/MenuBarApp/Resources" ]; then
  cp -R "$ROOT/macos/MenuBarApp/Resources/." "$RESOURCES_DIR/"
fi

# Bundle server if requested
if [[ "$BUNDLE_SERVER" == "1" ]]; then
  BUNDLED_SERVER_DIR="$RESOURCES_DIR/Server"
  # Validate required server directories exist
  for dir in "${REQUIRED_SERVER_DIRS[@]}"; do
    if [ ! -d "$ROOT/$dir" ]; then
      echo "ERROR: Required server directory not found: $ROOT/$dir" >&2
      exit 1
    fi
  done
  for file in "${REQUIRED_SERVER_FILES[@]}"; do
    if [ ! -f "$ROOT/$file" ]; then
      echo "ERROR: Required bundled server file not found: $ROOT/$file" >&2
      exit 1
    fi
  done

  mkdir -p "$BUNDLED_SERVER_DIR"
  cp "$ROOT/server.py" "$ROOT/VERSION" "$BUNDLED_SERVER_DIR/"
  cp -R "$ROOT/collectors" "$ROOT/services" "$ROOT/web" "$BUNDLED_SERVER_DIR/"
  mkdir -p "$BUNDLED_SERVER_DIR/macos"
  cp "$ROOT/macos/start-workbuddy-monitored.sh" "$BUNDLED_SERVER_DIR/macos/"
  chmod 755 "$BUNDLED_SERVER_DIR/macos/start-workbuddy-monitored.sh"
  if [ ! -x "$BUNDLED_SERVER_DIR/macos/start-workbuddy-monitored.sh" ]; then
    echo "ERROR: Bundled WorkBuddy reconnect script is not executable" >&2
    exit 1
  fi
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

# Choose compatible SDK
SDK_PATH=""
for candidate in \
  /Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk \
  /Library/Developer/CommandLineTools/SDKs/MacOSX26.sdk \
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
  -framework WidgetKit \
  -module-cache-path /tmp/swift-module-cache \
  -Xlinker -rpath -Xlinker /usr/lib/swift \
  "${SWIFT_FILES[@]}" \
  -o "$MACOS_DIR/$APP_NAME"

echo "=== Building Widget extension ==="
WIDGET_FILES=(
  "$WIDGET_SOURCE_DIR/WidgetStatus.swift"
  "$WIDGET_SOURCE_DIR/AICCWidget.swift"
  "$WIDGET_SOURCE_DIR/RefreshWidgetIntent.swift"
)
for file in "${WIDGET_FILES[@]}"; do
  if [ ! -f "$file" ]; then
    echo "ERROR: Widget source not found: $file" >&2
    exit 1
  fi
done
if [ ! -f "$WIDGET_SOURCE_DIR/Info.plist" ] || [ ! -f "$WIDGET_ENTITLEMENTS" ]; then
  echo "ERROR: Widget metadata or entitlements missing" >&2
  exit 1
fi
cp "$WIDGET_SOURCE_DIR/Info.plist" "$WIDGET_INFO"
plutil -replace CFBundleShortVersionString -string "$VERSION" "$WIDGET_INFO"
plutil -replace CFBundleVersion -string "$BUILD_VERSION" "$WIDGET_INFO"
xcrun swiftc \
  -parse-as-library \
  -application-extension \
  -module-name AICCWidget \
  -O \
  -sdk "$SDK_PATH" \
  -target arm64-apple-macosx14.0 \
  -framework WidgetKit \
  -framework SwiftUI \
  -framework AppIntents \
  -framework Foundation \
  -module-cache-path /tmp/swift-module-cache \
  -Xlinker -e \
  -Xlinker _NSExtensionMain \
  "${WIDGET_FILES[@]}" \
  -o "$WIDGET_MACOS_DIR/AICCWidget"

echo "=== Signing ==="
if [[ "$SIGNING_IDENTITY" == "-" ]]; then
  codesign --force --sign - --entitlements "$WIDGET_ENTITLEMENTS" "$WIDGET_APP_DIR"
else
  codesign --force --options runtime --timestamp \
    --entitlements "$WIDGET_ENTITLEMENTS" \
    --sign "$SIGNING_IDENTITY" "$WIDGET_APP_DIR"
fi
codesign --verify --strict "$WIDGET_APP_DIR"

if [[ "$SIGNING_IDENTITY" == "-" ]]; then
  codesign --force --sign - "$APP_DIR"
else
  codesign --force --options runtime --timestamp \
    --entitlements "$ROOT/macos/entitlements.plist" \
    --sign "$SIGNING_IDENTITY" "$APP_DIR"
fi
test -x "$WIDGET_MACOS_DIR/AICCWidget"
test "$(plutil -extract CFBundleShortVersionString raw -o - "$WIDGET_INFO")" = "$VERSION"
test "$(plutil -extract CFBundleVersion raw -o - "$WIDGET_INFO")" = "$BUILD_VERSION"
codesign --verify --strict "$WIDGET_APP_DIR"
codesign --verify --deep --strict "$APP_DIR"

echo "=== Done ==="
echo "$APP_DIR"
echo "$WIDGET_APP_DIR"
