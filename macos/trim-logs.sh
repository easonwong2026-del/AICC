#!/bin/bash
set -euo pipefail

LOG_DIR="${1:-$HOME/Library/Logs/AI-EInk-Dashboard}"
MAX_BYTES="${EINK_LOG_MAX_BYTES:-1048576}"
KEEP_BYTES="${EINK_LOG_KEEP_BYTES:-262144}"

[[ -d "$LOG_DIR" ]] || exit 0
for file in "$LOG_DIR"/*.log; do
  [[ -f "$file" ]] || continue
  size="$(stat -f %z "$file" 2>/dev/null || echo 0)"
  [[ "$size" -gt "$MAX_BYTES" ]] || continue
  temporary="$(mktemp "$LOG_DIR/.trim.XXXXXX")"
  tail -c "$KEEP_BYTES" "$file" > "$temporary"
  cp "$temporary" "$file"
  rm -f "$temporary"
done
