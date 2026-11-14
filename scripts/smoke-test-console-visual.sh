#!/bin/bash
# 主控制台离屏渲染回归（PRD §11.1）。
#
# 用真实的 SwiftUI 视图在 960×640 / 1120×760 / 1440×900、浅色 / 深色下渲染四个页面，
# 外加关键弹层与空 / 错误状态，输出到 /private/tmp/aicc-console-qa。
#
# 这是**渲染**不是截图工具链：不需要屏幕录制权限，CI 里也能跑。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="/private/tmp/aicc-console-qa"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-console-visual.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT

rm -rf "$OUT"
mkdir -p "$OUT"

SWIFT_FILES=()
while IFS= read -r -d '' file; do SWIFT_FILES+=("$file"); done \
  < <(find "$ROOT/macos/MenuBarApp/Sources" -name '*.swift' -print0)

xcrun swiftc -parse-as-library -D AICC_VISUAL_TEST -O \
  -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  -target arm64-apple-macosx14.0 \
  -module-cache-path "${SWIFT_MODULE_CACHE:-/tmp/swift-module-cache}" \
  -framework SwiftUI -framework AppKit -framework WidgetKit \
  "${SWIFT_FILES[@]}" \
  "$ROOT/macos/Widget/WidgetConfiguration.swift" \
  "$ROOT/macos/Widget/WidgetStatus.swift" \
  "$ROOT/macos/ConsoleVisual/ConsoleVisualSmokeMain.swift" \
  -o "$TEMP_ROOT/aicc-console-visual"

"$TEMP_ROOT/aicc-console-visual"

COUNT=$(find "$OUT" -name '*.png' | wc -l | tr -d ' ')
echo "console visual QA images: $COUNT"
if [ "$COUNT" -lt 24 ]; then
  echo "FAIL: expected at least 24 images, got $COUNT" >&2
  exit 1
fi
