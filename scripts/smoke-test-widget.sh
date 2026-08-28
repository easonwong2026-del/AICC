#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-widget.XXXXXX")"
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
  "$ROOT/macos/Widget/WidgetStatus.swift" \
  "$ROOT/macos/Widget/WidgetStatusSmokeMain.swift" \
  -o "$TEMP_ROOT/aicc-widget-status-smoke"

"$TEMP_ROOT/aicc-widget-status-smoke"
