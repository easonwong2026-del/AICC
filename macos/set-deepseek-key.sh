#!/bin/bash
set -euo pipefail

read -r -s -p "DeepSeek API key: " DEEPSEEK_KEY
echo
if [[ -z "$DEEPSEEK_KEY" ]]; then
  echo "No key entered." >&2
  exit 1
fi

security add-generic-password -U \
  -a "$USER" \
  -s "ai-eink-dashboard.deepseek" \
  -w "$DEEPSEEK_KEY" >/dev/null
unset DEEPSEEK_KEY
echo "DeepSeek key saved in macOS Keychain."
