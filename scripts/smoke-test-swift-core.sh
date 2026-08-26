#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$ROOT/macos/MenuBarApp/Tests"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-swift-core.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT

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

xcrun swiftc \
  -parse-as-library \
  -O \
  -sdk "$SDK_PATH" \
  -target arm64-apple-macosx14.0 \
  -module-cache-path "$TEMP_ROOT/module-cache" \
  "$ROOT/macos/MenuBarApp/Sources/Models/StatusData.swift" \
  "$ROOT/macos/MenuBarApp/Sources/Models/DashboardTypography.swift" \
  "$ROOT/macos/MenuBarApp/Sources/Models/SettingsPresentationModel.swift" \
  "$ROOT/macos/MenuBarApp/Sources/Models/StatusItemMenuModel.swift" \
  "$ROOT/macos/MenuBarApp/Sources/Models/UpdateModels.swift" \
  "$ROOT/macos/MenuBarApp/Sources/Services/ProcessRunner.swift" \
  "$TEST_ROOT/CoreSmokeMain.swift" \
  -o "$TEMP_ROOT/aicc-core-smoke"

"$TEMP_ROOT/aicc-core-smoke" "$TEST_ROOT/Fixtures"
