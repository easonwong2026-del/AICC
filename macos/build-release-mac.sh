#!/bin/bash
set -euo pipefail

# Build, sign, notarize, and staple the AICC DMG for distribution.
# Without credentials this produces an explicitly ad-hoc test artifact.

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"
DMG="$ROOT/dist/AICC-$VERSION.dmg"
SIGNING_IDENTITY="${AICC_SIGNING_IDENTITY:-}"
NOTARY_PROFILE="${AICC_NOTARY_PROFILE:-}"

if [[ -z "$SIGNING_IDENTITY" || -z "$NOTARY_PROFILE" ]]; then
  echo "=== Building ad-hoc test artifact (not notarized) ==="
  echo "WARNING: ad-hoc 签名会让钥匙串在每次重建后重新要求授权，且不能用于分发。"
  AICC_ALLOW_ADHOC=1 AICC_SIGNING_IDENTITY=- bash "$ROOT/macos/build-dmg.sh"
  hdiutil verify "$DMG"
  echo "NOT_AVAILABLE: Developer ID signing or notarization credentials were not provided."
  exit 0
fi

echo "=== Building Developer ID signed DMG ==="
AICC_SIGNING_IDENTITY="$SIGNING_IDENTITY" bash "$ROOT/macos/build-dmg.sh"
codesign --verify --deep --strict "$ROOT/dist/mac/AICC.app"
spctl --assess --verbose "$ROOT/dist/mac/AICC.app"

echo "=== Notarization ==="
xcrun notarytool submit "$DMG" \
  --keychain-profile "$NOTARY_PROFILE" \
  --wait

echo "=== Stapling ==="
xcrun stapler staple "$DMG"

echo "=== Assess ==="
spctl --assess --verbose --type install "$ROOT/dist/mac/AICC.app"
xcrun stapler validate "$DMG"

# 打包时已经用 manifest 校验过「DMG 里的二进制 ↔ dSYM」。公证 + staple 会再改
# 一次 DMG 字节（写入票据），所以这里只复核 UUID 链，并把最终 sha256 记进清单，
# 让符号归档与“真正被下载的那个文件”仍然一一对应。
BUILD_VERSION="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$ROOT/dist/mac/AICC.app/Contents/Info.plist")"
SYMBOLS_DIR="$ROOT/dist/symbols/$VERSION-$BUILD_VERSION"
if [[ -f "$SYMBOLS_DIR/manifest.txt" ]]; then
  bash "$ROOT/scripts/verify-symbols.sh" \
    --app "$ROOT/dist/mac/AICC.app" \
    --symbols "$SYMBOLS_DIR"
  echo "dmg_sha256_final $(shasum -a 256 "$DMG" | awk '{ print $1 }')" >> "$SYMBOLS_DIR/manifest.txt"
  echo "=== Symbols: ${SYMBOLS_DIR}（私有归档，不上传 Release）==="
else
  echo "ERROR: 没有符号清单 $SYMBOLS_DIR/manifest.txt，拒绝收尾（发布会失去符号化能力）" >&2
  exit 1
fi

echo "=== Complete: $DMG ==="
