#!/bin/bash
set -euo pipefail

if [[ $# -ne 1 || ! -f "$1" ]]; then
  echo "Usage: bash macos/import-migration.sh /path/to/ai-eink-dashboard-migration.zip" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TEMP_DIR"' EXIT
unzip -q "$1" -d "$TEMP_DIR"

if [[ ! -f "$TEMP_DIR/migration.json" ]] || ! grep -q '"contains_secrets"[[:space:]]*:[[:space:]]*false' "$TEMP_DIR/migration.json"; then
  echo "Invalid or unsafe migration package." >&2
  exit 1
fi

mkdir -p "$ROOT/data" "$ROOT/data/migration-backup"
for source in "$TEMP_DIR"/data/*.json; do
  [[ -e "$source" ]] || continue
  name="$(basename "$source")"
  if [[ -f "$ROOT/data/$name" ]]; then
    cp "$ROOT/data/$name" "$ROOT/data/migration-backup/$name"
  fi
  cp "$source" "$ROOT/data/$name"
done
echo "Dashboard history and fallback settings imported. Login credentials were not migrated."
