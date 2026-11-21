#!/bin/bash
# 设置中心离屏渲染回归（Phase 2B）。
#
# 用真实的 SwiftUI 设置窗口在 560×520 / 760×620 / 960×720、浅色 / 深色下渲染
# 四个分类，外加关键状态、错误与弹层，输出到 /private/tmp/aicc-settings-qa。
#
# 样本固定且脱敏；构建里 `SettingsModel.loadOnce()` 直接返回，
# 不会启动助手、不会调用 tls.reset / 撤销设备 / 迁移 / LAN 修改。
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="/private/tmp/aicc-settings-qa"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-settings-visual.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT

rm -rf "$OUT"
mkdir -p "$OUT"

SWIFT_FILES=()
while IFS= read -r -d '' file; do SWIFT_FILES+=("$file"); done \
  < <(find "$ROOT/macos/MenuBarApp/Sources" -name '*.swift' -print0)

xcrun swiftc -parse-as-library -D AICC_VISUAL_TEST -D AICC_SETTINGS_VISUAL -O \
  -sdk "$(xcrun --sdk macosx --show-sdk-path)" \
  -target arm64-apple-macosx14.0 \
  -module-cache-path "${SWIFT_MODULE_CACHE:-/tmp/swift-module-cache}" \
  -framework SwiftUI -framework AppKit -framework WidgetKit \
  "${SWIFT_FILES[@]}" \
  "$ROOT/macos/Widget/WidgetConfiguration.swift" \
  "$ROOT/macos/Widget/WidgetStatus.swift" \
  "$ROOT/macos/SettingsVisual/SettingsVisualState.swift" \
  "$ROOT/macos/SettingsVisual/SettingsVisualSmokeMain.swift" \
  -o "$TEMP_ROOT/aicc-settings-visual"

"$TEMP_ROOT/aicc-settings-visual"

COUNT=$(find "$OUT" -name '*.png' | wc -l | tr -d ' ')
echo "settings visual QA images: $COUNT"
if [ "$COUNT" -lt 24 ]; then
  echo "FAIL: expected at least 24 images, got $COUNT" >&2
  exit 1
fi
