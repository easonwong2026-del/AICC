# Changelog (English)

[简体中文完整记录](CHANGELOG.md)

This file summarizes the current and recent public releases. The Chinese changelog contains the older detailed history.

## 2.12.2 - 2026-10-27

### macOS

- **Desktop widgets rendered as a solid black tile.** The bridge wrote its runtime data into
  `Contents/Resources/Server/data/` inside the App bundle. An ad-hoc signature hashes every file, so two added
  files (`host_id`, `magpie_last_success.json`) made `codesign --verify --deep --strict` report
  `a sealed resource is missing or invalid`. With the seal broken macOS stops registering the Widget extension
  (it disappears from chronod's registry) and Desktop draws the system's empty shell: a black rectangle with no
  text on it.
- **Root cause.** With `EINK_DATA_DIR` unset, `collectors/magpie.py` fell back to `data/` beside the script —
  inside an installed App, the bundle itself. Any resolved directory inside a `.app` is now redirected to
  `~/Library/Application Support/AICC-Dashboard/data` with one line on stderr. A source checkout still uses
  `./data` and an explicit `EINK_DATA_DIR` still wins.
- **Self-healing after an upgrade.** The first launch of a new build recursively clears `com.apple.quarantine`
  from its own bundle, re-registers the extension with `pluginkit -a`, and restarts `chronod`. Each step logs a
  line with its exit status to `~/Library/Logs/AICC-Dashboard/widget-health.log`.
- **The App no longer hangs after an upgrade.** The bridge secret lived in a login-keychain item whose ACL is
  bound to the signing identity; an ad-hoc rebuild changes cdhash, so `SecItemCopyMatching` waits for an
  access-authorization dialog that an `LSUIElement` menu-bar App cannot show, parking the App in launch forever.
  The secret now lives in `~/Library/Application Support/AICC-Dashboard/bridge-token` (0600), and the old item is
  read at most once during migration under a two-second deadline.
- **Ship-time guard.** `validate-widget-bundle.sh` asserts no `Server/data` exists inside the bundle and that
  `codesign --verify --deep --strict` passes; `smoke-test-bundled-server.sh` starts the server inside a `.app`
  without `EINK_DATA_DIR` and asserts it does not write into the bundle.
- **Tests.** `DataRootTests` (five cases) added to `tests/test_magpie.py`; `WidgetInstallationTests` (six cases)
  cover the App Translocation path rules.

### Docs

- Both READMEs now separate "widget is blank after an upgrade" from "widget is solid black" and give the commands
  to check each.

## 2.12.1 - 2026-10-26

### E-ink APK

- **Fixed: paired device showing no data.** Discovery probes without a credential and marks the server pairing-required; the follow-up probe with the stored token fetched the snapshot but never cleared that mark, so the app dropped the data and asked to pair again. A DHCP lease change, a router reboot or one timed-out poll was enough. A successful probe now clears it.
- **Fixed: refresh wedged for good.** The in-flight flag was only cleared on the happy path, so an unexpected error blocked every later refresh until a restart. It clears in a `finally` now.
- **Fixed: "Magpie 暂无额度" right after start.** The bridge has nothing to answer with until its first Magpie read lands; a refresh in that window drew an empty panel and then waited a whole interval. An empty answer retries after 15 s, four times.
- New test `reProbeWithStoredCredentialClearsPairingFlag`; APK is `1.2.7-pencil-home` (versionCode 13).

### Build and docs

- `release-publish.sh` takes the repository and both manifest URLs from `remote.origin.url`, so a fork publishes to itself.
- Both READMEs now state the released version.

## 2.11.0 - 2026-10-21

### Quota model

- **No longer Codex-only**: the contract is `schema_version: 3`. Any provider and any window name Magpie reports is kept; subscription windows, plans and balances are all supported, and the `provider == "codex"` filter plus the fixed `five_hour` / `weekly` mapping are gone.
- **Semantics over guesses**: the vendor's own `display` wins, then a remaining percentage, otherwise unknown. Balances stay money, missing reset times are not invented. Credit windows report what is left (`4650 credits`, `共 4727`).
- **Account anonymity**: snapshots, caches and logs keep a stable anonymous id and a first-character label only.
- **Bridge**: read-only local Magpie API with `X-AICC-Schema` negotiation; the old `codex` shape only answers `format=2`.

### macOS widget

- Generic quota cards: several windows per account, two on Small, four on Medium, `+N` overflow.
- **Pick which quotas to show**: a new sheet ticks windows per provider and account; tick order is display order, and a stale tick is skipped instead of blanking the tile.
- One hue per provider (hashed for unknown ones), amber under 15% remaining.
- Two real fixes: read the kept report before refreshing (a failed refresh no longer blanks the widget), and support macOS 26's accented rendering (cards were becoming white slabs that swallowed the text).

### E-ink APK (1.2.6)

- Rebuilt around one hero window: oversized number, superscript unit, thick bar, relative countdown, then three summary rows; accounts with three or more windows get a page listing all of them, with pooled duplicates labelled (`7 天 · Gemini`).
- Fixes the divider striking through the reset line, mismatched baselines, the dead-end "+5 windows", the missing total, the triple "重置 未知", the double footer and the inconsistent branding.
- Units localized: `credits → 积分`, `5 hours → 5 小时`, `7 days → 7 天`.

### Version

- AICC 2.11.0, macOS Build 16; APK 1.2.6 (versionCode 12, same certificate as before, upgrades in place).

## 2.10.2 - 2026-10-04

### macOS

- **Fix desktop widgets stopping synchronization**: Compile configuration and refresh intents in both the containing app and widget extension, and package App Intents metadata matching each binary's Swift types. Restoring existing configurations under the containing app identity can now resolve the intent.
- **Build regression checks**: Validate configuration parameters, metric enums, background refresh and binary types in both bundles. Negative checks reject missing host metadata and mismatched type identifiers.
- **Version**: AICC 2.10.2, macOS Build 15. Existing widget configurations are preserved; Android remains unchanged.

## 2.10.1 - 2026-10-01

### macOS

- **Desktop widget redesign**: Equal-sized cards, native rounded surfaces and consistent numeric hierarchy for Small and Medium widgets, in both appearances and languages.
- **Less information**: Removes redundant branding, healthy status dots and W prefixes, and hides full secondary 5h quota. Active accounts use heavier type; manual refresh moves to the bottom right.
- **Failures and fallback**: Keeps reduced 5h percentages and cache, unavailable and sign-in notices. Multi-account views explicitly fall back to 5h when weekly quota is missing; summaries omit raw diagnostic codes.
- **Verification**: Visual checks cover 364 combinations and detect asymmetric tile bounds; the existing visual smoke now runs in Swift CI.
- **Version**: AICC 2.10.1, macOS Build 14; Android is unchanged.

## 2.10.0 - 2026-09-30

### macOS

- **OpenCodex Account Pool**: Reads the complete Codex pool with independent Weekly / 5h quota, reset times, reauthentication and cache state per account. Pinned mode prioritizes the active account; auto mode preserves source order without inventing an active account. Supports `AICC_CODEX_SOURCE=auto / opencodex / app-server`; once auto detects OpenCodex, temporary failures do not silently switch to the legacy single-account source.
- **Forced refresh and cache recovery**: Manual refresh forces pool quota collection, concurrent requests share collection work, and failures/timeouts preserve the last successful pool with explicit stale state. Startup uses normal collection. Legacy app-server and pool caches are isolated; request-ID registration races and duplicate quota requests are fixed.
- **Dashboard information hierarchy**: Keeps Codex quota prominent with full-width single-account cards, two-column dual cards and a two-column grid for larger pools. Adds Automatic / 1 / 2 / All display preferences while showing the real pool count. Compact multi-account cards hide Reset; a single card retains full Weekly Reset. Google keeps compact Weekly / 5h progress rows; WorkBuddy and DeepSeek become auxiliary cards, with no second-by-second freshness or repeated online text in healthy states and explicit text for failures.
- **OpenCodex controls**: Removes duplicate branding and reduces spacing around status, toggle and dashboard access while preserving control behavior.
- **Consistent multi-account widgets**: Keeps independent Small two-metric and Medium four-slot configuration. Uses the complete pool and shared resolver for pinned/auto selection. Display preferences travel through the shared snapshot, survive refreshes and remain compatible with older snapshots. Reset data stays intact while presentation is simplified; other services follow the same compact hierarchy.
- **Localization and regressions**: Completes English/Chinese account-display and widget text; covers pool selection, display modes, reauthentication, zero/unknown quota, cached state and shared preferences. Fixes flake8 formatting failures and validates all five CI jobs.
- **Version and distribution**: AICC 2.10.0, macOS Build 13. Uses the same ad-hoc signing distribution as v2.9.0 and is not Apple-notarized. Requires macOS 14+, Apple Silicon and Python 3.10+; Python is not included in the DMG.

### Android / Poke4S

- Android source and behavior are unchanged. Continue using `AICC-Poke4S-1.2.6.apk` from v2.9.0 (`1.2.6-pencil-home` / versionCode 12); no duplicate APK is published here.

## 2.9.0 - 2026-09-28

### macOS

- **Per-widget metric customization**: Upgraded macOS widgets to native `AppIntentConfiguration` and `AppIntentTimelineProvider` while preserving the exact `com.aieink.dashboard.menubar.widget` kind for seamless upgrade compatibility from 2.8.0; users can right-click any desktop widget → Edit "AICC" to customize displayed metrics (2 primary metrics for Small, 4 independent slots for Medium).
- **Unified presentation layer and adaptive cards**: Introduced `WidgetMetricOption` and `MetricCardView` to route between Quota cards (Codex / Google) and Balance cards (WorkBuddy / DeepSeek) across compact and spacious layouts; includes deterministic duplicate normalization and robust fallback handling while preserving the default 2.8.0 layout.
- **Full localization and test coverage**: Added localized resources for Chinese and English configuration labels; added tests for intent persistence/decoding, duplicate normalization, offline/stale/missing data handling, and automated visual regression across 112 environmental variations.
- **Backend lifecycle hardening**: Recovers from unexpected Codex worker exits, improves connecting timeout and readiness/health handling, and fixes bundled server lifecycle issues.

### Android / Poke4S

- Poke4S repeats broadcasts, validates every discovered AICC server, migrates a changed LAN address, and offers a chooser when multiple healthy servers respond.
- Discovery replies retain legacy fields and add protocol, stable host ID, instance ID, hostname, platform, and version. Existing clients remain compatible.
- Offline cache stays visibly marked OFFLINE and Google quota remains available. BOOX firmware rejects app-initiated display wakeups, so continuously visible live quotas require always-on mode.
- **Version**: AICC 2.9.0, macOS Build 12; Android versionCode 12 and versionName `1.2.6-pencil-home`.

## 2.8.0 - 2026-09-13

- **Google quota restructuring (weekly primary, 5h secondary)**: Added `collectors/google.py` to ingest and calculate `Gem (Weekly)` and `Gem` limit buckets from OpenCodex. Weekly quota serves as the primary visual metric with large percentage typography and progress bars, paired with secondary 5-hour indicators and independent reset timestamps. A depleted 5h burst window highlights in red, and missing weekly buckets fall back explicitly to labeled 5h views.
- **Full Widget integration for Google quota**: Native macOS Small and Medium widgets now display Google quota alongside Codex with matching visual hierarchy and layout standards. Fine-tuned spacing prevents status badges (such as `Cached`) from clipping labels.
- **Snapshot and data consistency**: Unified all presentation feeds through the backend `server.py` and `services/collector_manager.py` under `display_revision` hashing; retired redundant private polling in `OpenCodexController`. Both Menu Bar and desktop Widgets consume identical `WidgetDisplaySnapshot` states with robust offline and stale fallbacks.
- **DeepSeek balance reliability**: Retains last-known-good balances during transient network drops, timeouts, TLS/DNS failures, or API glitches instead of clearing balances to zero; automatically resumes live reporting upon recovery.
- **Refresh concurrency and generation isolation**: Coalesces concurrent forced refresh requests into single worker passes; enforces generation boundaries so that new forced attempts take over after timeouts and late-arriving responses never corrupt active state.
- **Version**: AICC 2.8.0, macOS Build 11. Android/Poke4S remains on `1.2.5-pencil-home` (versionCode 11).
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
