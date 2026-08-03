# Changelog (English)

[简体中文完整记录](CHANGELOG.md)

This file summarizes the current and recent public releases. The Chinese changelog contains the older detailed history.

## 2.5.0 - 2026-07-31

- Added the dynamic Provider Manifest v1 layer while preserving the existing `/api/status` contract and Android/Poke4S compatibility.
- Added dynamic macOS Provider cards, ordering, visibility controls, refresh actions, diagnostics, and restore-default-order support.
- Preserved the WorkBuddy 2.4.4 bridge, daemon RPC, account-usage fields, forced refresh, automatic recovery, and failure-cache behavior.
- Fixed status-bar left/right click routing: left-click opens the dashboard and right-click opens the native menu.
- Fixed Settings window lifecycle and AppKit close-time crashes; closing Settings no longer quits AICC.
- Fixed the Codex app-server initialization race and stale-cache health reporting so quota values refresh correctly.
- Configured the public update manifest at `https://raw.githubusercontent.com/easonwong2026-del/AICC/main/updates/aicc-update.json`.
- Unified the macOS, Python, and package version to 2.5.0.

## 2.4.4 - 2026-07-31

- Improved real WorkBuddy balance reads through the local 9223 CDP bridge, including safe API field aliases and restricted DOM fallback.
- Added safe balance diagnostics, bundled reconnect support, primary renderer selection, and one-time automatic bridge healing.
- Preserved the existing WorkBuddy balance fields, 120-second refresh, forced refresh, recovery, and failure-cache behavior.

## 2.4.3 - 2026-07-31

- Fixed WorkBuddy live-balance cache refresh and connected manual `force=true` refresh through the server.
- Added safe failure fallback, cross-device status compatibility, and single-flight refresh protection.

## 2.4.2 - 2026-07-29

- Fixed OpenCodex login-shell environment handling and protected detected executable paths from shell injection.
- Simplified OpenCodex lifecycle control while preserving direct status/version checks and post-action verification.

## Older releases

See the [Chinese changelog](CHANGELOG.md) for the complete historical release notes.
