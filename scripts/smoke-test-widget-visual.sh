#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-widget-visual.XXXXXX")"
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
  -framework SwiftUI \
  -framework AppIntents \
  -framework WidgetKit \
  -framework AppKit \
  -module-cache-path "${SWIFT_MODULE_CACHE:-/tmp/swift-module-cache}" \
  "$ROOT/macos/MenuBarApp/Sources/Models/WidgetDisplaySnapshot.swift" \
  "$ROOT/macos/Widget/WidgetStatus.swift" \
  "$ROOT/macos/Widget/WidgetConfiguration.swift" \
  "$ROOT/macos/Widget/RefreshWidgetIntent.swift"   "$ROOT/macos/Widget/AICCWidgetViews.swift" \
  "$ROOT/macos/Widget/WidgetVisualSmokeMain.swift" \
  -o "$TEMP_ROOT/aicc-widget-visual-smoke"

"$TEMP_ROOT/aicc-widget-visual-smoke"

