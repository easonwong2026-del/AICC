#!/bin/bash
set -euo pipefail

if [[ $# -ne 1 || ! -d "$1" ]]; then
  echo "Usage: bash macos/update-from-directory.sh /path/to/new-dashboard" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE="$(cd "$1" && pwd)"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
BACKUP_ROOT="${AI_EINK_BACKUP_DIR:-$(dirname "$ROOT")/backups}"
STAMP="$(date +%Y%m%d-%H%M%S)"
BACKUP="$BACKUP_ROOT/ai-eink-dashboard-before-update-$STAMP"

[[ "$SOURCE" != "$ROOT" ]] || { echo "Source and destination must differ." >&2; exit 1; }
for required in VERSION server.py macos/install-autostart.sh; do
  [[ -f "$SOURCE/$required" ]] || { echo "Invalid update source: missing $required" >&2; exit 1; }
done
[[ -n "$PYTHON_BIN" ]] || { echo "Python 3 is required." >&2; exit 1; }

(cd "$SOURCE" && PYTHONDONTWRITEBYTECODE=1 "$PYTHON_BIN" -m unittest discover -s tests -q)
mkdir -p "$BACKUP_ROOT"
cp -R "$ROOT" "$BACKUP"
rsync -a --delete --exclude data --exclude __pycache__ --exclude '*.pyc' \
  --exclude .gradle --exclude 'android/poke-dashboard/app/build' \
  "$SOURCE/" "$ROOT/"
# V2.1 and earlier kept a manual Codex snapshot that is no longer read. It is
# already present in the complete backup above, so remove it from live data.
rm -f "$ROOT/data/codex_usage.json"
PYTHON_BIN="$PYTHON_BIN" bash "$ROOT/macos/install-autostart.sh"
echo "Updated to $(tr -d '[:space:]' < "$ROOT/VERSION"). Backup: $BACKUP"
