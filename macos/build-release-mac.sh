#!/bin/bash
set -euo pipefail

# Build, sign, notarize, and staple the AICC DMG for distribution.
# Requires a valid Developer ID certificate and notary credentials.

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"
DMG="$ROOT/dist/AICC-$VERSION.dmg"
SYSDIR="$ROOT/dist/AICC-$VERSION-signed.dmg"
TEAM_ID="${AICC_TEAM_ID:-}"
APPLE_ID="${AICC_APPLE_ID:-}"
PASSWORD="${AICC_NOTARY_PASSWORD:-}"

if [[ -n "${CI:-}" || -z "$TEAM_ID" ]]; then
  echo "=== Signing ad-hoc ==="
  codesign --force --deep --sign - "$ROOT/dist/mac/AICC.app"
  codesign --verify --deep --strict "$ROOT/dist/mac/AICC.app"
  exit 0
fi

DEVELOPER_ID="Developer ID Application: $TEAM_ID"
echo "=== Hardened signing ==="
codesign --force --deep --options runtime \
  --entitlements "$ROOT/macos/entitlements.plist" \
  --sign "$DEVELOPER_ID" \
  "$ROOT/dist/mac/AICC.app"
codesign --verify --deep --strict "$ROOT/dist/mac/AICC.app"
spctl --assess --verbose "$ROOT/dist/mac/AICC.app"

echo "=== Building signed DMG ==="
cp "$DMG" "$SYSDIR"
codesign --force --sign "$DEVELOPER_ID" "$SYSDIR"

echo "=== Notarization ==="
xcrun notarytool submit "$SYSDIR" \
  --apple-id "$APPLE_ID" \
  --team-id "$TEAM_ID" \
  --password "$PASSWORD" \
  --wait

echo "=== Stapling ==="
xcrun stapler staple "$ROOT/dist/mac/AICC.app"
xcrun stapler staple "$SYSDIR"

echo "=== Assess ==="
spctl --assess --verbose --type install "$ROOT/dist/mac/AICC.app"
echo "=== Complete: $SYSDIR ==="
