#!/bin/bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEMP_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/aicc-display.XXXXXX")"
trap 'rm -rf "$TEMP_ROOT"' EXIT
SDK_PATH="$(xcrun --sdk macosx --show-sdk-path)"
SOURCES=()
while IFS= read -r file; do SOURCES+=("$ROOT/macos/MenuBarApp/Sources/$file"); done < <(
  python3 - "$ROOT/macos/MenuBarApp/Package.swift" <<'PY'
import re
import sys
from pathlib import Path
source = Path(sys.argv[1]).read_text().split('sources: [', 1)[1].split(']', 1)[0]
print('\n'.join(re.findall(r'"([^\"]+\.swift)"', source)))
PY
)
xcrun swiftc -parse-as-library -sdk "$SDK_PATH" -target arm64-apple-macosx14.0 \
  -module-cache-path "$TEMP_ROOT/module-cache" \
  "${SOURCES[@]}" "$ROOT/macos/Widget/WidgetStatus.swift" \
  "$ROOT/macos/Widget/DisplaySnapshotRegressionMain.swift" -o "$TEMP_ROOT/check"
"$TEMP_ROOT/check" "$@"
