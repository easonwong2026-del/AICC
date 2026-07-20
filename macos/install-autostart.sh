#!/bin/bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PYTHON_BIN="${PYTHON_BIN:-$(command -v python3 || true)}"
LABEL="com.aieink.dashboard"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
WORKBUDDY_LABEL="com.aieink.workbuddy-monitor"
WORKBUDDY_PLIST="$HOME/Library/LaunchAgents/$WORKBUDDY_LABEL.plist"
LOG_TRIM_LABEL="com.aieink.log-maintenance"
LOG_TRIM_PLIST="$HOME/Library/LaunchAgents/$LOG_TRIM_LABEL.plist"
LOG_DIR="$HOME/Library/Logs/AI-EInk-Dashboard"

if [[ -z "$PYTHON_BIN" ]]; then
  echo "Python 3 was not found. Install Python 3.10 or newer first." >&2
  exit 1
fi

xml_escape() {
  printf '%s' "$1" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' -e 's/"/\&quot;/g'
}

reload_agent() {
  local label="$1"
  local plist="$2"
  local target="gui/$UID/$label"
  if launchctl print "$target" >/dev/null 2>&1; then
    launchctl bootout "gui/$UID" "$plist" >/dev/null 2>&1 \
      || launchctl bootout "$target" >/dev/null 2>&1 \
      || true
    for _ in 1 2 3 4 5; do
      launchctl print "$target" >/dev/null 2>&1 || break
      sleep 1
    done
  fi
  for _ in 1 2 3 4 5; do
    if launchctl bootstrap "gui/$UID" "$plist"; then
      # Scheduled one-shot agents may finish before kickstart returns. Their
      # successful registration is the durable condition we need here.
      launchctl kickstart -k "$target" >/dev/null 2>&1 || true
      return 0
    fi
    # A still-registered service means the plist is valid and usable; restart
    # it instead of treating launchd's duplicate-registration EIO as fatal.
    if launchctl print "$target" >/dev/null 2>&1; then
      launchctl kickstart -k "$target" >/dev/null 2>&1 || true
      return 0
    fi
    sleep 1
  done
  return 1
}

mkdir -p "$HOME/Library/LaunchAgents" "$LOG_DIR"
ROOT_XML="$(xml_escape "$ROOT")"
PYTHON_XML="$(xml_escape "$PYTHON_BIN")"
PATH_XML="$(xml_escape "$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin")"
LOG_DIR_XML="$(xml_escape "$LOG_DIR")"
CODEX_BIN="$(command -v codex || true)"
CODEX_ENV_XML=""
if [[ -n "$CODEX_BIN" ]]; then
  CODEX_ENV_XML="<key>CODEX_CLI_PATH</key><string>$(xml_escape "$CODEX_BIN")</string>"
fi

cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key>
  <array><string>$PYTHON_XML</string><string>-B</string><string>$ROOT_XML/server.py</string></array>
  <key>WorkingDirectory</key><string>$ROOT_XML</string>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>$PATH_XML</string><key>PYTHONDONTWRITEBYTECODE</key><string>1</string>$CODEX_ENV_XML</dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>$LOG_DIR_XML/dashboard.log</string>
  <key>StandardErrorPath</key><string>$LOG_DIR_XML/dashboard-error.log</string>
</dict>
</plist>
EOF

cat > "$WORKBUDDY_PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$WORKBUDDY_LABEL</string>
  <key>ProgramArguments</key>
  <array><string>/bin/bash</string><string>$ROOT_XML/macos/start-workbuddy-monitored.sh</string><string>--monitor</string></array>
  <key>WorkingDirectory</key><string>$ROOT_XML</string>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>$PATH_XML</string></dict>
  <key>RunAtLoad</key><true/>
  <key>StartInterval</key><integer>60</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>$LOG_DIR_XML/workbuddy-monitor.log</string>
  <key>StandardErrorPath</key><string>$LOG_DIR_XML/workbuddy-monitor-error.log</string>
</dict>
</plist>
EOF

cat > "$LOG_TRIM_PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LOG_TRIM_LABEL</string>
  <key>ProgramArguments</key>
  <array><string>/bin/bash</string><string>$ROOT_XML/macos/trim-logs.sh</string><string>$LOG_DIR_XML</string></array>
  <key>RunAtLoad</key><true/>
  <key>StartInterval</key><integer>86400</integer>
  <key>ProcessType</key><string>Background</string>
</dict>
</plist>
EOF

plutil -lint "$PLIST"
plutil -lint "$WORKBUDDY_PLIST"
plutil -lint "$LOG_TRIM_PLIST"
reload_agent "$LABEL" "$PLIST"
reload_agent "$WORKBUDDY_LABEL" "$WORKBUDDY_PLIST"
reload_agent "$LOG_TRIM_LABEL" "$LOG_TRIM_PLIST"
echo "Dashboard auto-start installed: $PLIST"
echo "WorkBuddy monitored auto-start installed: $WORKBUDDY_PLIST"
echo "Log maintenance installed: $LOG_TRIM_PLIST"
