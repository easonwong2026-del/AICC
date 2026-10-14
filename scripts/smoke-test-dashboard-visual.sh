#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-dashboard-visual.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT
cp -R "$ROOT/macos/MenuBarApp/Resources/." "$TEMP_ROOT/"
SWIFT_FILES=()
while IFS= read -r -d '' file; do SWIFT_FILES+=("$file"); done < <(find "$ROOT/macos/MenuBarApp/Sources/Models" "$ROOT/macos/MenuBarApp/Sources/Views" "$ROOT/macos/MenuBarApp/Sources/Helpers" -name '*.swift' -print0)
for name in APIService CacheManager OpenCodexController ProcessRunner; do
  SWIFT_FILES+=("$ROOT/macos/MenuBarApp/Sources/Services/$name.swift")
done
xcrun swiftc -parse-as-library -O \
  -sdk /Library/Developer/CommandLineTools/SDKs/MacOSX.sdk \
  -target arm64-apple-macosx14.0 -module-cache-path "${SWIFT_MODULE_CACHE:-/tmp/swift-module-cache}" \
  -framework SwiftUI -framework AppKit -framework WidgetKit -framework AppIntents \
  "${SWIFT_FILES[@]}" \
  "$ROOT/macos/Widget/WidgetConfiguration.swift" "$ROOT/macos/Widget/WidgetStatus.swift" \
  "$ROOT/macos/Widget/RefreshWidgetIntent.swift" "$ROOT/macos/Widget/DashboardVisualSmokeMain.swift" \
  -o "$TEMP_ROOT/aicc-dashboard-visual"
"$TEMP_ROOT/aicc-dashboard-visual" "$@"
