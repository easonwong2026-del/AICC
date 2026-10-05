#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="${1:-$ROOT/dist/mac/AICC.app}"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-appintents.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT
bash "$ROOT/scripts/validate-widget-appintents.sh" "$APP_DIR"
cp -R "$APP_DIR" "$TEMP_ROOT/AICC.app"
HOST_METADATA="$TEMP_ROOT/AICC.app/Contents/Resources/Metadata.appintents"
expect_rejection() {
  if bash "$ROOT/scripts/validate-widget-appintents.sh" "$TEMP_ROOT/AICC.app" > "$TEMP_ROOT/validation.log" 2>&1; then
    echo "FAIL: Invalid host metadata was accepted" >&2
    exit 1
  fi
  grep -q 'FAIL: AICC App Intents metadata:' "$TEMP_ROOT/validation.log"
}
mv "$HOST_METADATA" "$TEMP_ROOT/host-metadata"
expect_rejection
mv "$TEMP_ROOT/host-metadata" "$HOST_METADATA"
python3 - "$HOST_METADATA/extract.actionsdata" <<'PYTHON'
import json
from pathlib import Path
import sys
path = Path(sys.argv[1])
data = json.loads(path.read_text())
data["actions"]["AICCWidgetConfigurationIntent"]["mangledTypeName"] = "10AICCWidget0A19ConfigurationIntentV"
path.write_text(json.dumps(data))
PYTHON
expect_rejection
echo "PASS: Missing host registration and mismatched compiled intent types are rejected"
