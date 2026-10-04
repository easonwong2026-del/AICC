#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="${1:-$ROOT/dist/mac/AICC.app}"
WIDGET_APP_DIR="$APP_DIR/Contents/PlugIns/AICCWidget.appex"
WIDGET_BINARY="$WIDGET_APP_DIR/Contents/MacOS/AICCWidget"
WIDGET_PLIST="$WIDGET_APP_DIR/Contents/Info.plist"

echo "=== Validating Widget Extension & App Intents Metadata ==="
echo "Target: $WIDGET_APP_DIR"

# 0. Source architecture check: AppIntentConfiguration & AppIntentTimelineProvider
if grep -q "StaticConfiguration" "$ROOT/macos/Widget/AICCWidget.swift"; then
  echo "FAIL: StaticConfiguration detected in AICCWidget.swift (must use AppIntentConfiguration)" >&2
  exit 1
fi
if ! grep -q "AppIntentConfiguration" "$ROOT/macos/Widget/AICCWidget.swift"; then
  echo "FAIL: AppIntentConfiguration missing from AICCWidget.swift" >&2
  exit 1
fi
if ! grep -q "AppIntentTimelineProvider" "$ROOT/macos/Widget/AICCWidget.swift"; then
  echo "FAIL: AppIntentTimelineProvider missing from AICCWidget.swift" >&2
  exit 1
fi
echo "PASS: Source code uses AppIntentConfiguration and AppIntentTimelineProvider"

# 1. Widget Extension bundle exists
if [ ! -d "$WIDGET_APP_DIR" ]; then
  echo "FAIL: Widget extension bundle does not exist at $WIDGET_APP_DIR" >&2
  exit 1
fi
echo "PASS: Widget extension bundle exists"

# 2. Widget binary exists and is executable
if [ ! -f "$WIDGET_BINARY" ] || [ ! -x "$WIDGET_BINARY" ]; then
  echo "FAIL: Widget binary does not exist or is not executable at $WIDGET_BINARY" >&2
  exit 1
fi
echo "PASS: Widget binary exists and is executable"

# 3. Info.plist exists and is valid
if [ ! -f "$WIDGET_PLIST" ]; then
  echo "FAIL: Info.plist not found at $WIDGET_PLIST" >&2
  exit 1
fi

BUNDLE_ID="$(plutil -extract CFBundleIdentifier raw -o - "$WIDGET_PLIST" 2>/dev/null || true)"
EXT_POINT="$(plutil -extract NSExtension.NSExtensionPointIdentifier raw -o - "$WIDGET_PLIST" 2>/dev/null || true)"
BUILD_VER="$(plutil -extract CFBundleVersion raw -o - "$WIDGET_PLIST" 2>/dev/null || true)"

if [ "$BUNDLE_ID" != "com.aieink.dashboard.menubar.widget" ]; then
  echo "FAIL: CFBundleIdentifier is '$BUNDLE_ID', expected 'com.aieink.dashboard.menubar.widget'" >&2
  exit 1
fi

if [ "$EXT_POINT" != "com.apple.widgetkit-extension" ]; then
  echo "FAIL: NSExtensionPointIdentifier is '$EXT_POINT', expected 'com.apple.widgetkit-extension'" >&2
  exit 1
fi
echo "PASS: Info.plist is valid (id: $BUNDLE_ID, point: $EXT_POINT, build: $BUILD_VER)"

# Both identities must resolve saved configurations and interactive refreshes.
python3 - "$APP_DIR" <<'PYTHON'
import json
from pathlib import Path
import subprocess
import sys

app = Path(sys.argv[1])
for bundle, module in [(app, "AICC"), (app / "Contents/PlugIns/AICCWidget.appex", "AICCWidget")]:
    metadata = bundle / "Contents/Resources/Metadata.appintents"
    try:
        data = json.loads((metadata / "extract.actionsdata").read_text())
        version = json.loads((metadata / "version.json").read_text())
        assert version, "Empty metadata version"
        symbols = subprocess.check_output(["nm", str(bundle / "Contents/MacOS" / module)], text=True)
        actions = data.get("actions", {})
        intent = actions["AICCWidgetConfigurationIntent"]
        assert "com.apple.link.systemProtocol.WidgetConfiguration" in intent.get("systemProtocols", []), "Missing WidgetConfiguration protocol"
        parameters = {p.get("name") for p in intent.get("parameters", [])}
        required = {"primaryMetric", "secondaryMetric", "topLeft", "topRight", "bottomLeft", "bottomRight"}
        assert required <= parameters, "Missing configuration parameters"
        refresh = actions["RefreshWidgetIntent"]
        assert refresh.get("openAppWhenRun") is False, "Refresh must run in the background"
        enum = next(e for e in data.get("enums", []) if e.get("identifier") == "WidgetMetricOption")
        assert {c["identifier"] for c in enum["cases"]} == {"codex", "google", "workbuddy", "deepseek"}, "Missing metrics"
        for item, name in [(intent, "AICCWidgetConfigurationIntent"), (refresh, "RefreshWidgetIntent"), (enum, "WidgetMetricOption")]:
            assert item["fullyQualifiedTypeName"] == f"{module}.{name}", "Wrong metadata module"
            assert "_$s" + item["mangledTypeName"] + "Mn" in symbols, f"Metadata type absent from binary: {name}"
    except (OSError, ValueError, KeyError, StopIteration, AssertionError, subprocess.CalledProcessError) as error:
        print(f"FAIL: {module} App Intents metadata: {error}", file=sys.stderr)
        sys.exit(1)
    print(f"PASS: {module} configuration, refresh and enum metadata match compiled types")
PYTHON

echo "=== All App Intents validation checks PASSED ==="
