#!/bin/bash
set -euo pipefail

APP="${WORKBUDDY_APP:-/Applications/WorkBuddy.app}"
DEBUG_PORT="${WORKBUDDY_DEBUG_PORT:-9223}"
ENSURE_MODE="${1:-}"

if [[ ! -d "$APP" ]]; then
  echo "WorkBuddy.app was not found. Set WORKBUDDY_APP to its full path." >&2
  [[ "$ENSURE_MODE" == "--ensure" || "$ENSURE_MODE" == "--monitor" ]] && exit 0
  exit 1
fi

EXECUTABLE_NAME="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist" 2>/dev/null || true)"
EXECUTABLE="$APP/Contents/MacOS/$EXECUTABLE_NAME"

port_ready() {
  nc -z 127.0.0.1 "$DEBUG_PORT" >/dev/null 2>&1
}

app_running() {
  pgrep -f "$APP/Contents/MacOS/$EXECUTABLE_NAME" >/dev/null 2>&1 \
    || lsof -t "$EXECUTABLE" >/dev/null 2>&1
}

if port_ready; then
  [[ "$ENSURE_MODE" == "--ensure" ]] && echo "WorkBuddy monitoring bridge is already available on 127.0.0.1:$DEBUG_PORT."
  exit 0
fi

# The periodic monitor is passive while WorkBuddy is closed. It must never
# launch the app merely to refresh the dashboard.
if [[ "$ENSURE_MODE" == "--monitor" ]] && ! app_running; then
  exit 0
fi

if app_running && [[ "$ENSURE_MODE" != "--ensure" && "$ENSURE_MODE" != "--monitor" ]]; then
  echo "Quit WorkBuddy first, then run this script again so the local monitoring port can be enabled." >&2
  exit 1
fi

if app_running; then
  osascript -e 'tell application "WorkBuddy" to quit' >/dev/null
  for _ in {1..15}; do
    app_running || break
    sleep 1
  done
fi

if ! open -na "$APP" --args --remote-debugging-address=127.0.0.1 --remote-debugging-port="$DEBUG_PORT" >/dev/null 2>&1; then
  if [[ -z "$EXECUTABLE_NAME" || ! -x "$EXECUTABLE" ]]; then
    echo "WorkBuddy executable was not found in $APP." >&2
    exit 1
  fi
  nohup "$EXECUTABLE" \
    --remote-debugging-address=127.0.0.1 \
    --remote-debugging-port="$DEBUG_PORT" \
    >/dev/null 2>&1 </dev/null &
fi
for _ in {1..20}; do
  if port_ready; then
    echo "WorkBuddy started with the localhost monitoring bridge."
    exit 0
  fi
  sleep 1
done

echo "WorkBuddy started, but monitoring port $DEBUG_PORT did not become ready." >&2
exit 1
