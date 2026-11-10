#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERVER_ROOT="${SERVER_ROOT:-$ROOT}"
BUNDLE_SERVER="${BUNDLE_SERVER:-0}"
SIGNING_IDENTITY="${AICC_SIGNING_IDENTITY:--}"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"

# Required server directories for BUNDLE_SERVER=1
REQUIRED_SERVER_DIRS=("collectors")
REQUIRED_SERVER_FILES=("collectors/magpie.py")

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
  cp -R "$ROOT/collectors" "$BUNDLED_SERVER_DIR/"
  # CI syntax/tests may leave bytecode; distribute source only.
  find "$BUNDLED_SERVER_DIR" -type d -name __pycache__ -prune -exec rm -rf {} +
  find "$BUNDLED_SERVER_DIR" -type f \( -name "*.pyc" -o -name "*.pyo" \) -delete
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
# Shared Widget settings and the direct Magpie loader are used by the host refresh link.
SWIFT_FILES+=(
  "$WIDGET_SOURCE_DIR/WidgetConfiguration.swift"
  "$WIDGET_SOURCE_DIR/WidgetStatus.swift"
)

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
  -module-name AICC \
  -D AICC_HOST \
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
  "$SOURCE_DIR/Models/QuotaModel.swift"
  "$SOURCE_DIR/Models/UsageModel.swift"
  "$SOURCE_DIR/Models/WidgetPreferences.swift"
  "$WIDGET_SOURCE_DIR/WidgetStatus.swift"
  "$WIDGET_SOURCE_DIR/WidgetConfiguration.swift"
  "$WIDGET_SOURCE_DIR/AICCWidgetViews.swift"
  "$WIDGET_SOURCE_DIR/AICCWidget.swift"
  "$WIDGET_SOURCE_DIR/WidgetUsage.swift"
  "$WIDGET_SOURCE_DIR/BoardWidgetViews.swift"
  "$WIDGET_SOURCE_DIR/BoardWidget.swift"
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
mkdir -p "$WIDGET_APP_DIR/Contents/Resources"
if [ -d "$ROOT/macos/MenuBarApp/Resources" ]; then
  cp -R "$ROOT/macos/MenuBarApp/Resources/." "$WIDGET_APP_DIR/Contents/Resources/"
fi
# No App Intents metadata: the board's footer controls are Links, which need
# nothing beyond the URL scheme the host app already claims.  Xcode's generated
# metadata is what routes `Button(intent:)`, and a hand-written copy of it is
# ignored by the system — see HANDOFF.md §6.
rm -rf "$WIDGET_APP_DIR/Contents/Resources/Metadata.appintents"
xcrun swiftc \
  -parse-as-library \
  -application-extension \
  -module-name AICCWidget \
  -O \
  -sdk "$SDK_PATH" \
  -target arm64-apple-macosx14.0 \
  -framework WidgetKit \
  -framework SwiftUI \
  -framework Foundation \
  -module-cache-path /tmp/swift-module-cache \
  -Xlinker -e \
  -Xlinker _NSExtensionMain \
  "${WIDGET_FILES[@]}" \
  -o "$WIDGET_MACOS_DIR/AICCWidget"

echo "=== Building account worker (Rust helper) ==="
HELPER_NAME="aicc-account-worker"
HELPERS_DIR="$CONTENTS/Helpers"
HELPER_BIN="$HELPERS_DIR/$HELPER_NAME"
HELPER_BUILT=0
SKIP_RUST_HELPER="${SKIP_RUST_HELPER:-0}"

if [[ "$SKIP_RUST_HELPER" == "1" ]]; then
  echo "SKIP_RUST_HELPER=1 → 跳过 Rust 助手，本次产物不含账号中心助手（不可用于发布）"
else
  CARGO_BIN="${CARGO_BIN:-}"
  if [[ -z "$CARGO_BIN" ]]; then
    if command -v cargo >/dev/null 2>&1; then
      CARGO_BIN="$(command -v cargo)"
    elif [[ -x "$HOME/.cargo/bin/cargo" ]]; then
      CARGO_BIN="$HOME/.cargo/bin/cargo"
    fi
  fi
  if [[ -z "$CARGO_BIN" ]]; then
    echo "ERROR: 缺少 cargo，无法构建 ${HELPER_NAME}。" >&2
    echo "       安装 Rust（brew install rust）后重试；仅调 Swift 界面时可设 SKIP_RUST_HELPER=1。" >&2
    exit 1
  fi
  if [[ ! -f "$ROOT/account-worker/Cargo.toml" ]]; then
    echo "ERROR: 找不到 account-worker/Cargo.toml" >&2
    exit 1
  fi
  mkdir -p "$HELPERS_DIR"
  # --locked：Cargo.lock 必须被提交，避免 CI 上悄悄升级依赖。
  "$CARGO_BIN" build --release --locked \
    --manifest-path "$ROOT/account-worker/Cargo.toml"
  cp "$ROOT/account-worker/target/release/${HELPER_NAME}" "$HELPER_BIN"
  chmod 755 "$HELPER_BIN"
  # 沙箱拷贝可能留下扩展属性，会让后面的签名校验失败。
  xattr -c "$HELPER_BIN" 2>/dev/null || true
  HELPER_BUILT=1
  echo "Helper: $HELPER_BIN"
fi

echo "=== Signing ==="
# 嵌套可执行文件必须先签，再签外层 bundle，否则 --deep --strict 会失败。
if [[ "$HELPER_BUILT" == "1" ]]; then
  if [[ "$SIGNING_IDENTITY" == "-" ]]; then
    codesign --force --sign - "$HELPER_BIN"
  else
    codesign --force --options runtime --timestamp \
      --sign "$SIGNING_IDENTITY" "$HELPER_BIN"
  fi
  codesign --verify --strict "$HELPER_BIN"
fi
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

# Post-build validation
bash "$ROOT/scripts/validate-widget-bundle.sh" "$APP_DIR"
if [[ "$HELPER_BUILT" == "1" ]]; then
  bash "$ROOT/scripts/validate-account-worker-bundle.sh" "$APP_DIR"
fi
