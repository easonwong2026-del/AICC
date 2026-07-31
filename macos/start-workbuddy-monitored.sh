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
  pgrep -f "^$EXECUTABLE" >/dev/null 2>&1
}

renderer_ready() {
  if ! command -v curl >/dev/null 2>&1; then
    return 0
  fi
  local body
  body="$(curl -s --max-time 2 "http://127.0.0.1:$DEBUG_PORT/json/list" || true)"
  [[ -n "$body" ]] && echo "$body" | grep -q 'webSocketDebuggerUrl'
}

stop_app() {
  # Direct process kill instead of `osascript quit`: quitting via Apple Events
  # requires Automation permission and silently fails without it, leaving the
  # app running without the debugging bridge.
  local pids
  pids="$(pgrep -f "^$EXECUTABLE" || true)"
  if [[ -n "$pids" ]]; then
    echo "$pids" | xargs kill 2>/dev/null || true
  fi
  for _ in {1..15}; do
    app_running || return 0
    sleep 1
  done
  if app_running; then
    echo "$pids" | xargs kill -9 2>/dev/null || true
    for _ in {1..10}; do
      app_running || return 0
      sleep 1
    done
  fi
  return 1
}

launch_with_bridge() {
  if ! open -na "$APP" --args --remote-debugging-address=127.0.0.1 --remote-debugging-port="$DEBUG_PORT" >/dev/null 2>&1; then
    if [[ -z "$EXECUTABLE_NAME" || ! -x "$EXECUTABLE" ]]; then
      echo "WorkBuddy executable was not found in $APP." >&2
      return 1
    fi
    nohup "$EXECUTABLE" \
      --remote-debugging-address=127.0.0.1 \
      --remote-debugging-port="$DEBUG_PORT" \
      >/dev/null 2>&1 </dev/null &
  fi
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

if [[ "$ENSURE_MODE" != "--ensure" && "$ENSURE_MODE" != "--monitor" ]] && app_running; then
  echo "WorkBuddy is already running without the local monitoring bridge." >&2
  echo "Run this script with --ensure (or use AICC settings → 重连 WorkBuddy) to restart it with the bridge." >&2
  exit 1
fi

if [[ "$ENSURE_MODE" == "--monitor" ]]; then
  # WorkBuddy is running but the bridge is not available. Never restart the
  # app from the passive monitor; only the explicit user action may do that.
  exit 0
fi

if app_running; then
  stop_app || {
    echo "FAIL:stop:WorkBuddy could not be stopped to enable the local bridge" >&2
    exit 1
  }
fi

launch_with_bridge || {
  echo "FAIL:launch:WorkBuddy could not be launched with the local bridge" >&2
  exit 1
}

port_seen=0
for _ in {1..30}; do
  if port_ready; then
    port_seen=1
    if renderer_ready; then
      echo "WorkBuddy started with the localhost monitoring bridge."
      echo "AICC_WORKBUDDY_READY"
      exit 0
    fi
  fi
  sleep 1
done

if [[ "$port_seen" == "1" ]]; then
  echo "WorkBuddy bridge is up, but the main renderer page was not found." >&2
  echo "AICC_WORKBUDDY_FAIL:renderer" >&2
else
  echo "WorkBuddy started, but monitoring port $DEBUG_PORT did not become ready." >&2
  echo "AICC_WORKBUDDY_FAIL:timeout" >&2
fi
exit 1
