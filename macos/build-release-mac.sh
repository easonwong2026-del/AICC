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
  AICC_SIGNING_IDENTITY=- bash "$ROOT/macos/build-dmg.sh"
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
echo "=== Complete: $DMG ==="
