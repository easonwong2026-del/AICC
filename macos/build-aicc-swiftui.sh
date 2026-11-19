#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SERVER_ROOT="${SERVER_ROOT:-$ROOT}"
BUNDLE_SERVER="${BUNDLE_SERVER:-0}"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"

# === 签名身份 ===
#
# 为什么不能默认 ad-hoc：ad-hoc 签名的 designated requirement 是**这一次构建的
# cdhash**。钥匙串条目 ACL 记的是"哪个程序能读"，所以每重建一次就变成陌生程序，
# macOS 就要用户重新输登录密码。固定证书的 DR 是
# `identifier "..." and certificate root = H"..."`，与单次构建无关，才谈得上"授权一次"。
#
# 解析顺序：
#   1. `AICC_SIGNING_IDENTITY` 显式指定（`-` 表示明确要 ad-hoc）；
#   2. 本机固定身份（`AICC_LOCAL_IDENTITY`，默认 `AICC Local Code Signing`）；
#   3. 都没有 → 直接报错；要 ad-hoc 必须显式 `AICC_ALLOW_ADHOC=1`（CI / 临时构建）。
LOCAL_IDENTITY="${AICC_LOCAL_IDENTITY:-AICC Local Code Signing}"
REQUESTED_IDENTITY="${AICC_SIGNING_IDENTITY:-}"

# 只看证书是否在钥匙串里，不看 `find-identity -p codesigning`：
# 本机自签证书通常没有"信任设置"（CSSMERR_TP_NOT_TRUSTED），find-identity 会报
# 0 valid identities，但 codesign 照样能用——它的 DR 只需要证书根。
identity_present() {
  # 关键：`security find-certificate` 找不到证书时也可能退出 0（错误只写到 stderr），
  # 所以要判断输出，不能只看退出码。CI 上正是这里误判成"有证书"，
  # 接着 codesign 报 "The specified item could not be found in the keychain"。
  [[ -n "$(security find-certificate -a -c "$1" 2>/dev/null)" ]]
}

if [[ -n "$REQUESTED_IDENTITY" ]]; then
  SIGNING_IDENTITY="$REQUESTED_IDENTITY"
elif identity_present "$LOCAL_IDENTITY"; then
  SIGNING_IDENTITY="$LOCAL_IDENTITY"
elif [[ "${AICC_ALLOW_ADHOC:-0}" == "1" ]]; then
  SIGNING_IDENTITY="-"
  echo "WARNING: 没有可用的固定签名身份，按 AICC_ALLOW_ADHOC=1 使用 ad-hoc。" >&2
  echo "WARNING: ad-hoc 每次构建都会让钥匙串重新要求授权，只用于 CI / 临时构建。" >&2
else
  cat >&2 <<'MSG'
ERROR: 找不到可用的代码签名身份，构建中止。

  ad-hoc 签名会让钥匙串在每次重建后重新要求输入密码，所以本机安装构建
  默认要求一张固定证书，不再静默退回 ad-hoc。

  三选一：
    1) 用本机固定证书（默认名字 AICC Local Code Signing）：
         bash macos/build-aicc-swiftui.sh
       证书不存在时，用「钥匙串访问 → 证书助理 → 创建证书」建一张
       「代码签名」证书，或指定已有身份：
         AICC_SIGNING_IDENTITY="<身份名>" bash macos/build-aicc-swiftui.sh
    2) 明确接受 ad-hoc（CI、测速、临时验证）：
         AICC_ALLOW_ADHOC=1 bash macos/build-aicc-swiftui.sh
    3) 正式分发用 Developer ID：
         AICC_SIGNING_IDENTITY="Developer ID Application: ..." bash macos/build-release-mac.sh
MSG
  exit 1
fi

# 只有 Developer ID 才加时间戳与 hardened runtime：自签证书配 --timestamp
# 会让本地构建依赖 Apple 的时间戳服务（离线即失败），而它对本机使用没有意义。
SIGN_ARGS=(--force --sign "$SIGNING_IDENTITY")
case "$SIGNING_IDENTITY" in
  "Developer ID Application:"*)
    SIGN_ARGS+=(--options runtime --timestamp)
    ;;
esac

echo "Signing identity: ${SIGNING_IDENTITY}"
if [[ "$SIGNING_IDENTITY" != "-" ]] && ! security find-identity -v -p codesigning 2>/dev/null | grep -qF "\"$SIGNING_IDENTITY\""; then
  echo "NOTE: 该身份不在 find-identity 的有效身份列表里（自签证书多半没有信任设置），"
  echo "      codesign 仍然可用；这只影响需要 trust 的工具链自检。"
fi

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
  cp "$ROOT/server.py" "$ROOT/bridge_tls.py" "$ROOT/VERSION" "$BUNDLED_SERVER_DIR/"
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

# === 两步编译（PR-B2）===
# 单次 `swiftc ... -o bin` 会在结束后删掉中间 .o，dsymutil 找不到调试映射，
# 只能产出"有符号名、没有行号"的残缺 dSYM（实测 --debug-line 仅 3 行）。
# 拆成 `-wmo -c` + 链接后，.o 留在 OBJ_DIR 里，dSYM 才带完整行号表。
OBJ_DIR="$ROOT/dist/obj/$BUILD_VERSION"
rm -rf "$OBJ_DIR"
mkdir -p "$OBJ_DIR"

xcrun swiftc \
  -parse-as-library \
  -module-name AICC \
  -D AICC_HOST \
  -O -g -wmo -c \
  -sdk "$SDK_PATH" \
  -target arm64-apple-macosx14.0 \
  -module-cache-path /tmp/swift-module-cache \
  "${SWIFT_FILES[@]}" \
  -o "$OBJ_DIR/AICC.o"

xcrun swiftc \
  -sdk "$SDK_PATH" \
  -target arm64-apple-macosx14.0 \
  -framework SwiftUI \
  -framework AppKit \
  -framework Foundation \
  -framework WidgetKit \
  -Xlinker -rpath -Xlinker /usr/lib/swift \
  "$OBJ_DIR/AICC.o" \
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
  -O -g -wmo -c \
  -sdk "$SDK_PATH" \
  -target arm64-apple-macosx14.0 \
  -module-cache-path /tmp/swift-module-cache \
  "${WIDGET_FILES[@]}" \
  -o "$OBJ_DIR/AICCWidget.o"

xcrun swiftc \
  -sdk "$SDK_PATH" \
  -target arm64-apple-macosx14.0 \
  -framework WidgetKit \
  -framework SwiftUI \
  -framework Foundation \
  -Xlinker -e \
  -Xlinker _NSExtensionMain \
  "$OBJ_DIR/AICCWidget.o" \
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

# === 内置 Python 运行时（PR-B）===
# 桥接不再借用宿主 python3：系统自带的 3.9 + LibreSSL 根本做不到 TLS 1.3。
# 运行时放 Contents/Resources/Python：实测放进 Contents/Frameworks 时，
# codesign 会把 lib/python3.14 当成一个嵌套 bundle 去验，直接拒绝封签
# （"bundle format unrecognized ... In subcomponent: lib/python3.14"）。
# Resources 下不会走进嵌套代码校验，两个 Mach-O 单独签名后 App 封签正常，
# `codesign --verify --deep --strict` 通过。
# 打包前仍由 scripts/validate-python-runtime.sh 再验一次：版本、OpenSSL、
# 依赖路径、可重定位、真实握手。
PY_RUNTIME_SRC="$ROOT/dist/python-runtime/python"
PY_RUNTIME_DST="$RESOURCES_DIR/Python"
PY_RUNTIME_BUNDLED=0
if [[ -d "$PY_RUNTIME_SRC" ]]; then
  mkdir -p "$CONTENTS/Frameworks"
  rm -rf "$PY_RUNTIME_DST"
  ditto "$PY_RUNTIME_SRC" "$PY_RUNTIME_DST"
  chmod +x "$PY_RUNTIME_DST/bin/python3"
  bash "$ROOT/scripts/validate-python-runtime.sh" "$PY_RUNTIME_DST"
  PY_RUNTIME_BUNDLED=1
  echo "Python runtime: $PY_RUNTIME_DST"
elif [[ "$BUNDLE_SERVER" == "1" ]]; then
  # A packaging build without the runtime would produce an App that either
  # borrows the host's python3 (3.9 + LibreSSL, no TLS 1.3) or refuses to start.
  # Failing here is the only honest outcome: the runtime is a product
  # requirement, not an optional extra.
  echo "ERROR: BUNDLE_SERVER=1 requires the bundled Python runtime at $PY_RUNTIME_SRC" >&2
  echo "       run: bash scripts/fetch-python-runtime.sh" >&2
  exit 1
else
  echo "WARNING: no prepared runtime at $PY_RUNTIME_SRC — this is a development build." >&2
  echo "         The App will only start it with AICC_DEV=1 and a host python3." >&2
fi

echo "=== Symbols and strip (PR-B2) ==="
# 发布二进制不带调试符号，但符号必须留下来，否则线上崩溃无法符号化。
#
# 顺序是硬约束：dsymutil → strip → codesign。反过来（先签名再 strip）会让
# 签名失效，内核直接拒绝执行——PR-A 里实测过：strip 后未重签的 python3.14
# 静默退出、退出码 1 且没有任何输出。
AICC_STRIP="${AICC_STRIP:-1}"
SYMBOLS_DIR="$ROOT/dist/symbols/$VERSION-$BUILD_VERSION"

archive_and_strip() {
  local binary="$1" name="$2"
  [[ -f "$binary" ]] || { echo "ERROR: 找不到 $binary" >&2; exit 1; }

  if [[ "$AICC_STRIP" != "1" ]]; then
    echo "AICC_STRIP=${AICC_STRIP}：跳过 strip，$name 保留完整符号（仅用于调试构建）"
    return 0
  fi

  mkdir -p "$SYMBOLS_DIR"
  # 1) 未裁剪的原件：即使 dSYM 丢了，也能用 `atos -o` 还原函数名。
  cp "$binary" "$SYMBOLS_DIR/$name.unstripped"
  # 2) dSYM：必须与二进制同 UUID，否则符号化对不上。
  xcrun dsymutil "$binary" -o "$SYMBOLS_DIR/$name.dSYM" >/dev/null

  local bin_uuid dsym_uuid line_rows
  bin_uuid="$(dwarfdump --uuid "$binary" 2>/dev/null | awk '{print $2}')"
  dsym_uuid="$(dwarfdump --uuid "$SYMBOLS_DIR/$name.dSYM" 2>/dev/null | awk '{print $2}')"
  if [[ -z "$bin_uuid" || "$bin_uuid" != "$dsym_uuid" ]]; then
    echo "ERROR: $name 的 dSYM UUID ($dsym_uuid) 与二进制 ($bin_uuid) 不一致" >&2
    exit 1
  fi
  # 3) 只产出一个没有行号的 dSYM 等于没留：实测单次 swiftc 调用只能得到
  #    3 行 --debug-line，这里要求它真的有行号表。
  line_rows="$(dwarfdump --debug-line "$SYMBOLS_DIR/$name.dSYM" 2>/dev/null | wc -l | tr -d ' ')"
  if [[ "${line_rows:-0}" -lt 100 ]]; then
    echo "ERROR: $name 的 dSYM 只有 ${line_rows} 行 .debug_line，行号不可用" >&2
    exit 1
  fi

  local before after
  before="$(stat -f%z "$binary")"
  # -x 删本地符号，-S 删调试段：两者都不动动态符号，Swift 运行时反射照常。
  xcrun strip -x -S "$binary"
  after="$(stat -f%z "$binary")"
  if [[ "$after" -ge "$before" ]]; then
    echo "ERROR: strip 没有缩小 ${name}（$before → $after 字节）" >&2
    exit 1
  fi
  echo "  $name: $before → $after 字节（省 $(( (before - after) / 1024 )) KiB），dSYM ${line_rows} 行行号，UUID $bin_uuid"
}

archive_and_strip "$MACOS_DIR/$APP_NAME" "AICC"
archive_and_strip "$WIDGET_MACOS_DIR/AICCWidget" "AICCWidget"
if [[ "$AICC_STRIP" == "1" ]]; then
  echo "  符号归档：${SYMBOLS_DIR}（不进 App，不进 DMG）"
fi

echo "=== Signing ==="
# 嵌套可执行文件必须先签，再签外层 bundle，否则 --deep --strict 会失败。
if [[ "$HELPER_BUILT" == "1" ]]; then
  codesign "${SIGN_ARGS[@]}" "$HELPER_BIN"
  codesign --verify --strict "$HELPER_BIN"
fi
# 运行时里只剩两个 Mach-O，但顺序仍然是先内层后外层：
# 每个 .so/.dylib → 解释器 → 目录，最后才由 App 把整棵树封进去。
if [[ "$PY_RUNTIME_BUNDLED" == "1" ]]; then
  while IFS= read -r -d '' binary; do
    codesign "${SIGN_ARGS[@]}" "$binary"
  done < <(find "$PY_RUNTIME_DST" -type f \( -name "*.so" -o -name "*.dylib" \) -print0)
  for executable in "$PY_RUNTIME_DST"/bin/python3.14 "$PY_RUNTIME_DST"/bin/python3; do
    [[ -f "$executable" ]] && codesign "${SIGN_ARGS[@]}" "$executable"
  done
  codesign --verify --strict "$PY_RUNTIME_DST/bin/python3"
fi
codesign "${SIGN_ARGS[@]}" --entitlements "$WIDGET_ENTITLEMENTS" "$WIDGET_APP_DIR"
codesign --verify --strict "$WIDGET_APP_DIR"

codesign "${SIGN_ARGS[@]}" --entitlements "$ROOT/macos/entitlements.plist" "$APP_DIR"
test -x "$WIDGET_MACOS_DIR/AICCWidget"
test "$(plutil -extract CFBundleShortVersionString raw -o - "$WIDGET_INFO")" = "$VERSION"
test "$(plutil -extract CFBundleVersion raw -o - "$WIDGET_INFO")" = "$BUILD_VERSION"
codesign --verify --strict "$WIDGET_APP_DIR"
codesign --verify --deep --strict "$APP_DIR"

echo "=== Signature ==="
codesign -dvv "$APP_DIR" 2>&1 | grep -E "^(Identifier|Authority|TeamIdentifier)=" || true
echo "helper:"
codesign -dvv "$HELPER_BIN" 2>&1 | grep -E "^(Identifier|Authority)=" || true
echo "requirement:"
codesign -d --requirements - "$APP_DIR" 2>&1 | tail -1 || true

echo "=== Done ==="
echo "$APP_DIR"
echo "$WIDGET_APP_DIR"

# Post-build validation
bash "$ROOT/scripts/validate-widget-bundle.sh" "$APP_DIR"
if [[ "$HELPER_BUILT" == "1" ]]; then
  bash "$ROOT/scripts/validate-account-worker-bundle.sh" "$APP_DIR"
fi
