#!/bin/bash
set -euo pipefail

LABEL="com.aieink.dashboard"
PLIST="$HOME/Library/LaunchAgents/$LABEL.plist"
LEGACY_WORKBUDDY_LABEL="com.aieink.workbuddy-monitor"
LEGACY_WORKBUDDY_PLIST="$HOME/Library/LaunchAgents/$LEGACY_WORKBUDDY_LABEL.plist"
LOG_TRIM_LABEL="com.aieink.log-maintenance"
LOG_TRIM_PLIST="$HOME/Library/LaunchAgents/$LOG_TRIM_LABEL.plist"
launchctl bootout "gui/$UID/$LABEL" >/dev/null 2>&1 || true
# Only clean the WorkBuddy monitor left by older AICC installations.
launchctl bootout "gui/$UID/$LEGACY_WORKBUDDY_LABEL" >/dev/null 2>&1 || true
launchctl bootout "gui/$UID/$LOG_TRIM_LABEL" >/dev/null 2>&1 || true
rm -f "$PLIST" "$LEGACY_WORKBUDDY_PLIST" "$LOG_TRIM_PLIST"
echo "Dashboard and log maintenance auto-start removed; legacy WorkBuddy monitor cleaned if present."
