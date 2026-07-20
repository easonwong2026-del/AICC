#!/bin/bash
set -euo pipefail

if [[ $# -ne 1 || ! -d "$1" ]]; then
  echo "Usage: bash macos/rollback-from-backup.sh /path/to/backup" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKUP="$(cd "$1" && pwd)"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
SAFETY_COPY="$(dirname "$ROOT")/backups/ai-eink-dashboard-before-rollback-$(date +%Y%m%d-%H%M%S)"

[[ "$BACKUP" != "$ROOT" && -f "$BACKUP/server.py" ]] || { echo "Invalid backup directory." >&2; exit 1; }
[[ -n "$PYTHON_BIN" ]] || { echo "Python 3 is required." >&2; exit 1; }

mkdir -p "$(dirname "$SAFETY_COPY")"
cp -R "$ROOT" "$SAFETY_COPY"
rsync -a --delete --exclude data --exclude __pycache__ --exclude '*.pyc' \
  --exclude .gradle --exclude 'android/poke-dashboard/app/build' \
  "$BACKUP/" "$ROOT/"
PYTHON_BIN="$PYTHON_BIN" bash "$ROOT/macos/install-autostart.sh"
echo "Rollback complete. Current data was preserved. Safety copy: $SAFETY_COPY"
