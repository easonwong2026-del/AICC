#!/bin/bash
set -euo pipefail

LABEL="com.aieink.dashboard"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
WORKBUDDY_LABEL="com.aieink.workbuddy-monitor"
WORKBUDDY_PLIST="$HOME/Library/LaunchAgents/$WORKBUDDY_LABEL.plist"
LOG_TRIM_LABEL="com.aieink.log-maintenance"
LOG_TRIM_PLIST="$HOME/Library/LaunchAgents/$LOG_TRIM_LABEL.plist"
launchctl bootout "gui/$UID/$LABEL" >/dev/null 2>&1 || true
launchctl bootout "gui/$UID/$WORKBUDDY_LABEL" >/dev/null 2>&1 || true
launchctl bootout "gui/$UID/$LOG_TRIM_LABEL" >/dev/null 2>&1 || true
rm -f "$PLIST" "$WORKBUDDY_PLIST" "$LOG_TRIM_PLIST"
echo "Dashboard and WorkBuddy monitored auto-start removed."
