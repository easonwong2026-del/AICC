#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="${1:-$ROOT/dist/mac/AICC.app}"
WIDGET_APP_DIR="$APP_DIR/Contents/PlugIns/AICCWidget.appex"
WIDGET_BINARY="$WIDGET_APP_DIR/Contents/MacOS/AICCWidget"
WIDGET_PLIST="$WIDGET_APP_DIR/Contents/Info.plist"
METADATA_DIR="$WIDGET_APP_DIR/Contents/Resources/Metadata.appintents"
ACTIONSDATA="$METADATA_DIR/extract.actionsdata"
VERSION_JSON="$METADATA_DIR/version.json"

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

# 4. AppIntents metadata directory and files exist
if [ ! -d "$METADATA_DIR" ]; then
  echo "FAIL: Metadata.appintents directory does not exist at $METADATA_DIR" >&2
  exit 1
fi

if [ ! -f "$ACTIONSDATA" ]; then
  echo "FAIL: extract.actionsdata does not exist at $ACTIONSDATA" >&2
  exit 1
fi

if [ ! -f "$VERSION_JSON" ]; then
  echo "FAIL: version.json does not exist at $VERSION_JSON" >&2
  exit 1
fi
echo "PASS: Metadata.appintents files exist"

# 5. Metadata is non-empty
if [ ! -s "$ACTIONSDATA" ] || [ ! -s "$VERSION_JSON" ]; then
  echo "FAIL: Metadata files are empty" >&2
  exit 1
fi
echo "PASS: Metadata files are non-empty"

# 6. Metadata contains AICCWidgetConfigurationIntent with WidgetConfiguration protocol
python3 -c "
import json, sys

with open('$ACTIONSDATA') as f:
    data = json.load(f)

actions = data.get('actions', {})
if 'AICCWidgetConfigurationIntent' not in actions:
    print('FAIL: AICCWidgetConfigurationIntent missing from actions', file=sys.stderr)
    sys.exit(1)

intent = actions['AICCWidgetConfigurationIntent']
protocols = intent.get('systemProtocols', [])
if 'com.apple.link.systemProtocol.WidgetConfiguration' not in protocols:
    print('FAIL: WidgetConfiguration protocol missing from intent systemProtocols', file=sys.stderr)
    sys.exit(1)

params = [p.get('name') for p in intent.get('parameters', [])]
required_params = ['primaryMetric', 'secondaryMetric', 'topLeft', 'topRight', 'bottomLeft', 'bottomRight']
for rp in required_params:
    if rp not in params:
        print('FAIL: Parameter missing:', rp, file=sys.stderr)
        sys.exit(1)

enums = data.get('enums', [])
enum_ids = [e.get('identifier') for e in enums]
if 'WidgetMetricOption' not in enum_ids:
    print('FAIL: WidgetMetricOption enum missing from metadata', file=sys.stderr)
    sys.exit(1)

print('PASS: AICCWidgetConfigurationIntent and parameters verified in App Intents metadata')
"

echo "=== All App Intents validation checks PASSED ==="
