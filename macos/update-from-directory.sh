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
MANIFEST_VERSION="$(cat "$ROOT/macos/MANIFEST_VERSION" 2>/dev/null || echo 0)"
UPGRADE_MANIFEST_VERSION="$(cat "$SOURCE/macos/MANIFEST_VERSION" 2>/dev/null || echo 0)"

[[ "$SOURCE" != "$ROOT" ]] || { echo "Source and destination must differ." >&2; exit 1; }
for required in VERSION server.py macos/install-autostart.sh macos/MANIFEST_VERSION; do
  [[ -f "$SOURCE/$required" ]] || { echo "Invalid update source: missing $required" >&2; exit 1; }
done
[[ -n "$PYTHON_BIN" ]] || { echo "Python 3 is required." >&2; exit 1; }

SOURCE_VERSION="$(tr -d '[:space:]' < "$SOURCE/VERSION")"
if [[ "$UPGRADE_MANIFEST_VERSION" -lt "$MANIFEST_VERSION" ]]; then
  echo "ERROR: Upgrade source has older manifest ($UPGRADE_MANIFEST_VERSION vs $MANIFEST_VERSION)." >&2
  echo "Reinstall from DMG instead." >&2
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

mkdir -p "$BACKUP_ROOT" "$BACKUP"
cp "$ROOT/VERSION" "$ROOT/macos/MANIFEST_VERSION" "$BACKUP/"
rsync -a --exclude data "$ROOT/providers" "$ROOT/collectors" "$ROOT/services" "$ROOT/macos" "$ROOT/web" "$ROOT/server.py" "$BACKUP/"
echo "$MANIFEST_VERSION" > "$BACKUP/data/.schema_version"

echo "Installing..."
rsync -a --delete --exclude data --exclude __pycache__ --exclude '*.pyc' \
  --exclude .gradle --exclude 'android/poke-dashboard/app/build' \
  "$SOURCE/" "$ROOT/"
echo "$UPGRADE_MANIFEST_VERSION" > "$ROOT/data/.schema_version" 2>/dev/null || true

PYTHON_BIN="$PYTHON_BIN" bash "$ROOT/macos/install-autostart.sh"

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
  rsync -a --delete --exclude data --exclude __pycache__ --exclude '*.pyc' \
    "$BACKUP/" "$ROOT/"
  PYTHON_BIN="$PYTHON_BIN" bash "$ROOT/macos/install-autostart.sh"
  echo "Rolled back to previous version. Data preserved in $BACKUP"
  exit 1
fi

echo "Updated to $SOURCE_VERSION. Backup: $BACKUP"
