#!/bin/bash
set -euo pipefail

# Render fixed SwiftUI card components to PNGs for PR before/after screenshots.
# Requires a macOS GUI session and CommandLineTools.

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/docs/screenshots}"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-screenshots.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT

mkdir -p "$OUT"

SDK_PATH=""
for candidate in \
  /Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk \
  /Library/Developer/CommandLineTools/SDKs/MacOSX26.sdk \
  /Library/Developer/CommandLineTools/SDKs/MacOSX15.4.sdk \
  /Library/Developer/CommandLineTools/SDKs/MacOSX15.sdk \
  /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk; do
  if [[ -d "$candidate" ]]; then
    SDK_PATH="$candidate"
    break
  fi
done

if [[ -z "$SDK_PATH" ]]; then
  SDK_PATH="$(xcrun --sdk macosx --show-sdk-path)"
fi

SRC="$ROOT/macos/MenuBarApp/Sources"
xcrun swiftc \
  -parse-as-library \
  -O \
  -sdk "$SDK_PATH" \
  -target arm64-apple-macosx14.0 \
  -framework SwiftUI \
  -framework AppKit \
  -framework Foundation \
  -module-cache-path "$TEMP_ROOT/module-cache" \
  "$SRC/Models/DashboardTypography.swift" \
  "$SRC/Models/StatusData.swift" \
  "$SRC/Models/SettingsPresentationModel.swift" \
  "$SRC/Models/SettingsData.swift" \
  "$SRC/Views/CodexCard.swift" \
  "$SRC/Views/WorkBuddyCard.swift" \
  "$SRC/Views/DeepSeekCard.swift" \
  "$SRC/Views/ServiceRow.swift" \
  "$SRC/Views/SystemCard.swift" \
  "$ROOT/scripts/render-provider-screenshots.swift" \
  -o "$TEMP_ROOT/aicc-screenshots"

"$TEMP_ROOT/aicc-screenshots" "$OUT"
