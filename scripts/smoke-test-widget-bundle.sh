#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="${1:-$ROOT/dist/mac/AICC.app}"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-widget-bundle.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT
bash "$ROOT/scripts/validate-widget-bundle.sh" "$APP_DIR"
cp -R "$APP_DIR" "$TEMP_ROOT/AICC.app"
plutil -remove CFBundleURLTypes "$TEMP_ROOT/AICC.app/Contents/Info.plist"
if bash "$ROOT/scripts/validate-widget-bundle.sh" "$TEMP_ROOT/AICC.app" > "$TEMP_ROOT/result" 2>&1; then
  echo "FAIL: Widget without a refresh URL was accepted" >&2
  exit 1
fi
grep -q 'Missing refresh URL scheme' "$TEMP_ROOT/result"
echo "PASS: Broken installed refresh routing is rejected"
