#!/bin/bash
set -euo pipefail

if [[ $# -ne 1 || ! -d "$1" ]]; then
  echo "Usage: bash macos/rollback-from-backup.sh /path/to/backup" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BACKUP="$(cd "$1" && pwd)"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
BACKUP_CODE="$BACKUP/code"
CURRENT_DATA_SCHEMA="$(tr -d '[:space:]' < "$ROOT/macos/DATA_SCHEMA_VERSION" 2>/dev/null || echo 0)"
BACKUP_DATA_SCHEMA="$(awk -F= '$1 == "data_schema_version" {print $2}' "$BACKUP/manifest" 2>/dev/null || echo 0)"
SAFETY_COPY="$(dirname "$ROOT")/backups/aicc-before-rollback-$(date +%Y%m%d-%H%M%S)"

[[ "$BACKUP" != "$ROOT" && -d "$BACKUP_CODE" && -f "$BACKUP_CODE/server.py" ]] || { echo "Invalid or incomplete backup directory." >&2; exit 1; }
[[ -n "$PYTHON_BIN" ]] || { echo "Python 3 is required." >&2; exit 1; }

if [[ "$BACKUP_DATA_SCHEMA" -lt "$CURRENT_DATA_SCHEMA" ]]; then
  echo "ERROR: backup data schema $BACKUP_DATA_SCHEMA is older than current $CURRENT_DATA_SCHEMA." >&2
  exit 1
fi

mkdir -p "$SAFETY_COPY/code"
rsync -a --delete \
  --exclude .git --exclude dist --exclude data --exclude backups \
  --exclude __pycache__ --exclude '*.pyc' \
  --exclude .gradle --exclude 'android/poke-dashboard/app/build' \
  "$ROOT/" "$SAFETY_COPY/code/"

restore_code() {
  rsync -a --delete \
    --exclude .git --exclude dist --exclude data --exclude backups \
    --exclude __pycache__ --exclude '*.pyc' \
    --exclude .gradle --exclude 'android/poke-dashboard/app/build' \
    "$1/" "$ROOT/"
}

wait_for_health() {
  for _ in 1 2 3 4 5 6; do
    if "$PYTHON_BIN" -c "
import json, urllib.request
try:
    with urllib.request.urlopen('http://127.0.0.1:8765/api/health/ready', timeout=5) as response:
        payload = json.loads(response.read())
        raise SystemExit(0 if payload.get('status') in ('healthy', 'degraded') else 1)
except Exception:
    raise SystemExit(1)
" 2>/dev/null; then
      return 0
    fi
    sleep 5
  done
  return 1
}

restore_code "$BACKUP_CODE"
if ! PYTHON_BIN="$PYTHON_BIN" bash "$ROOT/macos/install-autostart.sh"; then
  echo "ERROR: rollback service registration failed; restoring safety copy." >&2
  restore_code "$SAFETY_COPY/code"
  PYTHON_BIN="$PYTHON_BIN" bash "$ROOT/macos/install-autostart.sh"
  exit 1
fi
if ! wait_for_health; then
  echo "ERROR: rollback health check failed; restoring safety copy." >&2
  restore_code "$SAFETY_COPY/code"
  PYTHON_BIN="$PYTHON_BIN" bash "$ROOT/macos/install-autostart.sh"
  exit 1
fi

echo "Rollback complete. User data was preserved. Safety copy: $SAFETY_COPY"
