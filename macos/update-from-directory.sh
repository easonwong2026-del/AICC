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
BACKUP="$BACKUP_ROOT/aicc-before-update-$STAMP"
BACKUP_CODE="$BACKUP/code"
MANIFEST_VERSION="$(tr -d '[:space:]' < "$ROOT/macos/MANIFEST_VERSION" 2>/dev/null || echo 0)"
UPGRADE_MANIFEST_VERSION="$(tr -d '[:space:]' < "$SOURCE/macos/MANIFEST_VERSION" 2>/dev/null || echo 0)"
DATA_SCHEMA_VERSION="$(tr -d '[:space:]' < "$ROOT/macos/DATA_SCHEMA_VERSION" 2>/dev/null || echo 0)"
UPGRADE_DATA_SCHEMA_VERSION="$(tr -d '[:space:]' < "$SOURCE/macos/DATA_SCHEMA_VERSION" 2>/dev/null || echo 0)"

[[ "$SOURCE" != "$ROOT" ]] || { echo "Source and destination must differ." >&2; exit 1; }
for required in VERSION server.py macos/install-autostart.sh macos/MANIFEST_VERSION macos/DATA_SCHEMA_VERSION; do
  [[ -f "$SOURCE/$required" ]] || { echo "Invalid update source: missing $required" >&2; exit 1; }
done
[[ -n "$PYTHON_BIN" ]] || { echo "Python 3 is required." >&2; exit 1; }

SOURCE_VERSION="$(tr -d '[:space:]' < "$SOURCE/VERSION")"
if [[ "$UPGRADE_MANIFEST_VERSION" -lt "$MANIFEST_VERSION" ]]; then
  echo "ERROR: Upgrade source has older manifest ($UPGRADE_MANIFEST_VERSION vs $MANIFEST_VERSION)." >&2
  echo "Reinstall from DMG instead." >&2
  exit 1
fi
if [[ "$UPGRADE_DATA_SCHEMA_VERSION" -lt "$DATA_SCHEMA_VERSION" ]]; then
  echo "ERROR: update data schema $UPGRADE_DATA_SCHEMA_VERSION is older than current $DATA_SCHEMA_VERSION." >&2
  exit 1
fi

AVAILABLE_SPACE="$(df -k "$ROOT" | awk 'NR==2{print $4}')"
NEEDED_SPACE=51200
if [[ "$AVAILABLE_SPACE" -lt "$NEEDED_SPACE" ]]; then
  echo "ERROR: Insufficient disk space." >&2
  exit 1
fi

echo "Running pre-upgrade tests..."
(cd "$SOURCE" && PYTHONDONTWRITEBYTECODE=1 "$PYTHON_BIN" -m unittest discover -s tests -q)

mkdir -p "$BACKUP_CODE"
rsync -a --delete \
  --exclude .git \
  --exclude dist \
  --exclude data \
  --exclude backups \
  --exclude __pycache__ \
  --exclude '*.pyc' \
  --exclude 'android/poke-dashboard/app/build' \
  "$ROOT/" "$BACKUP_CODE/"
{
  echo "version=$(tr -d '[:space:]' < "$ROOT/VERSION")"
  echo "manifest_version=$MANIFEST_VERSION"
  echo "data_schema_version=$DATA_SCHEMA_VERSION"
  echo "created_at=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
} > "$BACKUP/manifest"

echo "Installing..."
rsync -a --delete \
  --exclude .git --exclude dist --exclude data --exclude backups \
  --exclude __pycache__ --exclude '*.pyc' \
  --exclude .gradle --exclude 'android/poke-dashboard/app/build' \
  "$SOURCE/" "$ROOT/"

rollback_code() {
  rsync -a --delete \
    --exclude .git --exclude dist --exclude data --exclude backups \
    --exclude __pycache__ --exclude '*.pyc' \
    --exclude .gradle --exclude 'android/poke-dashboard/app/build' \
    "$BACKUP_CODE/" "$ROOT/"
  PYTHON_BIN="$PYTHON_BIN" bash "$ROOT/macos/install-autostart.sh"
}

if ! PYTHON_BIN="$PYTHON_BIN" bash "$ROOT/macos/install-autostart.sh"; then
  echo "ERROR: service registration failed; restoring previous code." >&2
  rollback_code
  exit 1
fi

echo "Performing health check..."
HEALTH_OK=false
for _ in 1 2 3 4 5 6; do
  if "$PYTHON_BIN" -c "
import json, urllib.request
try:
  with urllib.request.urlopen('http://127.0.0.1:8765/api/health/ready', timeout=5) as r:
    data = json.loads(r.read())
    exit(0 if data.get('status') in ('healthy', 'degraded') else 1)
except Exception:
  exit(1)
" 2>/dev/null; then
    HEALTH_OK=true
    break
  fi
  sleep 5
done

if [[ "$HEALTH_OK" != "true" ]]; then
  echo "ERROR: Health check failed. Rolling back..." >&2
  rollback_code
  echo "Rolled back to previous version. Data preserved in $BACKUP"
  exit 1
fi

echo "Updated to $SOURCE_VERSION. Backup: $BACKUP"
