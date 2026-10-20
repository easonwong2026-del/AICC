#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-widget-preview.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT
mkdir -p "$TEMP_ROOT/source" "$TEMP_ROOT/dmg" "$ROOT/dist"
# Separate identities keep the installed app, Widget and settings untouched.
tar -C "$ROOT" --exclude=.git --exclude=dist --exclude=outputs --exclude=data \
  --exclude=.macos-backups --exclude=.runtime --exclude=.gradle --exclude=.build \
  --exclude=android/poke-dashboard/app/build --exclude=__pycache__ --exclude=.DS_Store \
  -cf - . | tar -C "$TEMP_ROOT/source" -xf -
python3 - "$TEMP_ROOT/source" <<'PY'
from pathlib import Path
import plistlib
import sys

root = Path(sys.argv[1])
for directory in (root / "macos", root / "scripts"):
    for path in directory.rglob("*"):
        if path.suffix in (".swift", ".plist", ".sh"):
            text = path.read_text().replace("com.aieink.dashboard.menubar", "com.aieink.dashboard.magpie-preview")
            text = text.replace("com.aieink.dashboard.widget-settings", "com.aieink.dashboard.widget-preview-settings")
            text = text.replace("aicc-widget", "aicc-widget-preview")
            if path.name == "AICCWidget.swift":
                text = text.replace('.configurationDisplayName("AICC")', '.configurationDisplayName("AICC 候选")')
            path.write_text(text)
for path in (root / "macos/MenuBarApp/Info.plist", root / "macos/Widget/Info.plist"):
    info = plistlib.loads(path.read_bytes())
    info["CFBundleDisplayName"] = "AICC 候选"
    path.write_bytes(plistlib.dumps(info))
PY
BUNDLE_SERVER=1 AICC_SIGNING_IDENTITY=- bash "$TEMP_ROOT/source/macos/build-aicc-swiftui.sh"
cp -R "$TEMP_ROOT/source/dist/mac/AICC.app" "$TEMP_ROOT/dmg/AICC-Widget-Preview.app"
ln -s /Applications "$TEMP_ROOT/dmg/Applications"
git -C "$ROOT" rev-parse HEAD > "$TEMP_ROOT/dmg/SOURCE_COMMIT.txt"
if ! git -C "$ROOT" diff --quiet; then echo "Includes local source changes" >> "$TEMP_ROOT/dmg/SOURCE_COMMIT.txt"; fi
hdiutil create -volname "AICC Widget Preview" -srcfolder "$TEMP_ROOT/dmg" -ov -format UDZO \
  "$ROOT/dist/AICC-Magpie-Widget-Preview.dmg"
hdiutil verify "$ROOT/dist/AICC-Magpie-Widget-Preview.dmg"
rm -rf "$ROOT/dist/AICC-Widget-Preview.app"
cp -R "$TEMP_ROOT/dmg/AICC-Widget-Preview.app" "$ROOT/dist/"
shasum -a 256 "$ROOT/dist/AICC-Magpie-Widget-Preview.dmg" > "$ROOT/dist/AICC-Magpie-Widget-Preview.dmg.sha256"
