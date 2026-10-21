#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-settings-visual.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT
SWIFT_FILES=()
while IFS= read -r -d '' file; do SWIFT_FILES+=("$file"); done < <(find "$ROOT/macos/MenuBarApp/Sources" -name '*.swift' -print0)
xcrun swiftc -parse-as-library -D AICC_VISUAL_TEST -D AICC_HOST -O \
  -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  -target arm64-apple-macosx14.0 -module-cache-path "${SWIFT_MODULE_CACHE:-/tmp/swift-module-cache}" \
  -framework SwiftUI -framework AppKit -framework WidgetKit \
  "${SWIFT_FILES[@]}" \
  "$ROOT/macos/Widget/WidgetConfiguration.swift" "$ROOT/macos/Widget/WidgetStatus.swift" \
  "$ROOT/macos/Widget/DashboardVisualSmokeMain.swift" \
  -o "$TEMP_ROOT/aicc-settings-visual"
"$TEMP_ROOT/aicc-settings-visual"
