# Changelog (English)

[简体中文完整记录](CHANGELOG.md)

This file summarizes the current and recent public releases. The Chinese changelog contains the older detailed history.

## 2.7.1 - 2026-08-29

- **macOS Widget redesign**: Redesigned the Medium Widget into a balanced horizontal split layout. Codex weekly quota serves as the primary metric with centered large typography, real progress, reset time, and 5-hour quota; WorkBuddy points and DeepSeek balance are cleanly displayed on the right.
- **Widget presentation simplification**: Removed redundant header title, footer timestamps, and extra status text from the Medium Widget; preserved stable placeholders on error without leaking diagnostic details to the desktop, and kept the unobtrusive top-right refresh control.
- **Widget compatibility**: Preserved manual refresh, 15-minute timeline, last-success cache, and stale fallback; backward-compatible with 2.7.0 widget snapshot cache format.
- **macOS runtime consolidation**: Consolidated the production runtime around the DMG App, SMAppService, ServerManager, and the bundled Python server; retired legacy LaunchAgent production paths while preserving safe one-time migration.
- **Version**: AICC 2.7.1, macOS Build 10. Android/Poke4S remains on its independent `1.2.5-pencil-home` (versionCode 11) release.

## 2.7.0 - 2026-08-28 (Release Candidate)

- Added native macOS WidgetKit Small and Medium widgets; Small shows Codex/WorkBuddy and Medium shows Codex, WorkBuddy, DeepSeek, and System.
- Added manual timeline refresh and WidgetKit reload notifications on App launch and displayed-data changes. The Widget reads only the fixed `/api/status` endpoint, keeps the last successful snapshot as stale data when the server is unavailable, and shows placeholders on a first install without a cache.
- Fixed Codex quota parsing by classifying live `account/rateLimits/read` windows by duration while preserving multi-bucket limits, sparse weekly updates, and the legacy fallback when duration is absent.
- Closed the DMG runtime gaps: the production local endpoint is fixed at `127.0.0.1:8765`, and the bundled server's `aicc-server.log` and `aicc-server-error.log` are bounded to 1 MiB at startup without blocking server launch if maintenance fails. App/server build identity validation remains enabled to prevent stale backend reuse.
- Version: AICC 2.7.0, macOS Build 9. Android/Poke4S remains on its independent `1.2.5-pencil-home` version.

## 2.6.0 - 2026-08-27

- Added OpenCodex version checks, one-click updates, and post-update version/status refresh. Running OpenCodex shows a warning before an update because the proxy may restart briefly.
- Improved macOS menu-bar and Settings stability while preserving the System, Codex, WorkBuddy, DeepSeek, and OpenCodex status surfaces.
- Simplified the architecture by removing unused Provider, Manifest, and legacy compatibility paths while preserving real collectors and the Poke4S/Android `/api/status` contract.
- Version: AICC 2.6.0, macOS Build 5. Android/Poke4S was unchanged; the public Release contained macOS artifacts only.

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
