#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
APP_DIR="${1:-$ROOT/dist/mac/AICC.app}"
python3 - "$APP_DIR" <<'PY'
import plistlib
from pathlib import Path
import subprocess
import sys

app = Path(sys.argv[1])
widget = app / "Contents/PlugIns/AICCWidget.appex"
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
    assert not (bundle / "Contents/Resources/Metadata.appintents").exists(), "Unexpected App Intent dependency"
    subprocess.run(["codesign", "--verify", "--strict", str(bundle)], check=True)
raw = subprocess.check_output(["codesign", "-d", "--entitlements", ":-", str(widget)], stderr=subprocess.DEVNULL)
entitlements = plistlib.loads(raw)
assert entitlements == {
    "com.apple.security.app-sandbox": True,
    "com.apple.security.network.client": True,
    "com.apple.security.temporary-exception.shared-preference.read-only": ["com.aieink.dashboard.widget-settings"],
}, "Widget must retain its sandbox with only network and read-only display settings"
print("PASS: signed Widget bundle, refresh URL, matching versions and narrow read-only settings permission")
PY
