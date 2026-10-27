#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="${1:-$ROOT/dist/mac/AICC.app}"
python3 - "$APP_DIR" <<'PY'
import plistlib
from pathlib import Path
import json
import subprocess
import sys

app = Path(sys.argv[1])
widget = app / "Contents/PlugIns/AICCWidget.appex"
# Runtime data belongs in ~/Library/Application Support.  A `Server/data`
# directory inside the bundle means a server ran without EINK_DATA_DIR and
# wrote into the sealed bundle: codesign then fails with "a sealed resource is
# missing or invalid", macOS drops the Widget extension, and every desktop
# widget shows as an empty black tile.  That shipped once in 2.12.1.
stray = app / "Contents/Resources/Server/data"
assert not stray.exists(), f"Runtime data inside the bundle breaks its seal: {stray}"
host_info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
info = plistlib.loads((widget / "Contents/Info.plist").read_bytes())
assert host_info["CFBundleIdentifier"] == "com.aieink.dashboard.menubar", "Wrong host identity"
assert info["CFBundleIdentifier"] == "com.aieink.dashboard.menubar.widget", "Wrong Widget identity"
assert info["NSExtension"]["NSExtensionPointIdentifier"] == "com.apple.widgetkit-extension", "Wrong extension point"
for key in ("CFBundleShortVersionString", "CFBundleVersion"):
    assert host_info[key] == info[key], "Host and Widget versions differ"
assert any("aicc-widget" in entry.get("CFBundleURLSchemes", []) for entry in host_info.get("CFBundleURLTypes", [])), "Missing refresh URL scheme"
for bundle in (app, widget):
    assert (bundle / "Contents/MacOS" / ("AICC" if bundle == app else "AICCWidget")).is_file(), "Missing executable"
    subprocess.run(["codesign", "--verify", "--strict", str(bundle)], check=True)
# The board's footer controls are Links routed through the host URL scheme, so
# no App Intents metadata should ship at all.  If one appears, someone wired a
# `Button(intent:)` back in without the signed build that it needs to function.
for bundle in (app, widget):
    assert not (bundle / "Contents/Resources/Metadata.appintents").exists(), \
        f"Unexpected App Intent metadata in {bundle.name}: footer taps use Links"
raw = subprocess.check_output(["codesign", "-d", "--entitlements", ":-", str(widget)], stderr=subprocess.DEVNULL)
entitlements = plistlib.loads(raw)
assert entitlements == {
    "com.apple.security.app-sandbox": True,
    "com.apple.security.network.client": True,
    "com.apple.security.temporary-exception.shared-preference.read-only": ["com.aieink.dashboard.widget-settings"],
}, "Widget must retain its sandbox with only network and read-only display settings"
sealed = subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)],
                        capture_output=True, text=True)
assert sealed.returncode == 0, f"Bundle seal is broken: {sealed.stderr.strip()}"
print("PASS: signed Widget bundle, refresh URL, matching versions, no runtime data inside the bundle")
PY
