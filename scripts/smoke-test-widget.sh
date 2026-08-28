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

if grep -RniE 'Text\(".*(Updated|ago)' "$ROOT/macos/Widget"; then
  echo "Error: Found user-visible Updated text in Widget views" >&2
  exit 1
fi

if ! grep -q 'WidgetCenter.shared.reloadTimelines' "$ROOT/macos/Widget/RefreshWidgetIntent.swift"; then
  echo "Error: RefreshWidgetIntent does not call WidgetCenter.shared.reloadTimelines" >&2
  exit 1
fi

if ! grep -q 'com.apple.security.app-sandbox' "$ROOT/macos/Widget/entitlements.plist"; then
  echo "Error: Widget entitlements missing app-sandbox" >&2
  exit 1
fi

if ! grep -Fq 'supportedFamilies([.systemSmall, .systemMedium])' "$ROOT/macos/Widget/AICCWidget.swift"; then
  echo "Error: Widget missing supportedFamilies declaration" >&2
  exit 1
fi

if ! grep -q -- '-framework WidgetKit' "$ROOT/macos/build-aicc-swiftui.sh"; then
  echo "Error: Host build script does not link WidgetKit" >&2
  exit 1
fi

if ! grep -q 'WidgetCenter.shared.reloadAllTimelines()' "$ROOT/macos/MenuBarApp/Sources/Services/APIService.swift"; then
  echo "Error: APIService does not reload widget timelines" >&2
  exit 1
fi

if ! grep -q 'WidgetCenter.shared.reloadAllTimelines()' "$ROOT/macos/MenuBarApp/Sources/AICCApp.swift"; then
  echo "Error: AICCApp does not reload widget timelines on launch" >&2
  exit 1
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
