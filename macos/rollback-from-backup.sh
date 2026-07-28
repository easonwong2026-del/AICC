#!/bin/bash
set -euo pipefail

if [[ $# -ne 1 || ! -d "$1" ]]; then
  echo "Usage: bash macos/rollback-from-backup.sh /path/to/backup" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKUP="$(cd "$1" && pwd)"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
MANIFEST_VERSION="$(cat "$ROOT/macos/MANIFEST_VERSION" 2>/dev/null || echo 0)"
BACKUP_MANIFEST_VERSION="$(cat "$BACKUP/macos/MANIFEST_VERSION" 2>/dev/null || echo 0)"
SAFETY_COPY="$(dirname "$ROOT")/backups/ai-eink-dashboard-before-rollback-$(date +%Y%m%d-%H%M%S)"

[[ "$BACKUP" != "$ROOT" && -f "$BACKUP/server.py" ]] || { echo "Invalid backup directory." >&2; exit 1; }
[[ -n "$PYTHON_BIN" ]] || { echo "Python 3 is required." >&2; exit 1; }

SCHEMA_FILE="$ROOT/data/.schema_version"
if [[ -f "$SCHEMA_FILE" ]]; then
  CURRENT_SCHEMA="$(cat "$SCHEMA_FILE" 2>/dev/null || echo 0)"
  if [[ "$BACKUP_MANIFEST_VERSION" -lt "$CURRENT_SCHEMA" ]]; then
    echo "ERROR: Backup has manifest $BACKUP_MANIFEST_VERSION vs data schema $CURRENT_SCHEMA." >&2
    echo "Reinstall from DMG instead." >&2
    exit 1
  fi
fi

mkdir -p "$(dirname "$SAFETY_COPY")"
cp -R "$ROOT" "$SAFETY_COPY"
rsync -a --delete --exclude data --exclude __pycache__ --exclude '*.pyc' \
  --exclude .gradle --exclude 'android/poke-dashboard/app/build' \
  "$BACKUP/" "$ROOT/"
PYTHON_BIN="$PYTHON_BIN" bash "$ROOT/macos/install-autostart.sh"

# Restore the schema version marker the current data was written with
if [[ -f "$SAFETY_COPY/$SCHEMA_FILE" ]]; then
  cp "$SAFETY_COPY/$SCHEMA_FILE" "$ROOT/$SCHEMA_FILE" 2>/dev/null || true
fi

echo "Rollback complete. Current data preserved. Safety copy: $SAFETY_COPY"
