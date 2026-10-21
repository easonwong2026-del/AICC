#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-board-visual.XXXXXX")"
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

OUT_DIR="${AICC_BOARD_QA_DIR:-/private/tmp/board_qa}"
rm -rf "$OUT_DIR"

xcrun swiftc \
  -parse-as-library \
  -O \
  -sdk "$SDK_PATH" \
  -target arm64-apple-macosx14.0 \
  -framework SwiftUI \
  -framework WidgetKit \
  -framework AppKit \
  -module-cache-path "${SWIFT_MODULE_CACHE:-/tmp/swift-module-cache}" \
  "$ROOT/macos/MenuBarApp/Sources/Models/QuotaModel.swift" \
  "$ROOT/macos/MenuBarApp/Sources/Models/UsageModel.swift" \
  "$ROOT/macos/MenuBarApp/Sources/Models/WidgetPreferences.swift" \
  "$ROOT/macos/Widget/WidgetStatus.swift" \
  "$ROOT/macos/Widget/WidgetConfiguration.swift" \
  "$ROOT/macos/Widget/WidgetUsage.swift" \
  "$ROOT/macos/Widget/AICCWidgetViews.swift" \
  "$ROOT/macos/Widget/BoardWidgetViews.swift" \
  "$ROOT/macos/Widget/BoardWidget.swift" \
  "$ROOT/macos/Widget/BoardVisualSmokeMain.swift" \
  -o "$TEMP_ROOT/aicc-board-visual"

"$TEMP_ROOT/aicc-board-visual"

rendered="$(ls "$OUT_DIR"/*.png 2>/dev/null | wc -l | tr -d ' ')"
if [[ "$rendered" -lt 28 ]]; then
  echo "Error: expected at least 28 board renders, found $rendered" >&2
  exit 1
fi
echo "Board visuals: $rendered PNGs in $OUT_DIR"
