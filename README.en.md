# AICC

[简体中文](README.md)

AICC is a local AI status center for macOS. It displays Codex, WorkBuddy, DeepSeek, and system status, with support for Poke4S e-ink devices. The server uses only the Python standard library.

The current macOS version is AICC 2.7.0 (Build 9); Android/Poke4S use an independent version line.

The macOS menu-bar app uses fixed Codex, WorkBuddy, DeepSeek, System, and OpenCodex status cards. It reads `/api/status` and the fixed operation endpoints only. The Python server uses four fixed collectors for Codex, WorkBuddy, DeepSeek, and system status.

## Access

- Mac dashboard: `http://localhost:8765`
- Settings: native SwiftUI Settings window from the menu bar
- Poke4S: `http://<Mac Wi-Fi address>:8765/?kiosk=1`

The Mac and Poke4S device must be on the same Wi-Fi network. The native Android client can discover the Mac over UDP.

## macOS setup and maintenance

See [MAC-MIGRATION.md](MAC-MIGRATION.md) for the first deployment. Common commands:

For end users, download the DMG from [GitHub Releases](https://github.com/easonwong2026-del/AICC/releases) and drag AICC
into `/Applications`. The AICC App starts the bundled Python server through `ServerManager`, and its login-at-launch
setting uses `SMAppService`. This is the only supported production macOS runtime path.

When upgrading from an older installation, the first AICC launch performs a one-time cleanup scoped to the three exact
LaunchAgent identities owned by the retired installer. It unloads the old jobs, removes their matching plists when safe,
and continues normal startup; failures do not crash the App or scan/kill unrelated services or processes.

Source checkouts support development debugging only; they are not a production installation path.

```bash
bash macos/start-dashboard.sh              # DEV ONLY: foreground server, no background registration
bash macos/set-deepseek-key.sh
BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh
bash macos/build-dmg.sh
```

Formal users check the public manifest through Settings → General → About & Updates, then download and replace the
published DMG. Source directories do not provide a second production updater or rollback system.

The DMG App stores logs in `~/Library/Logs/AICC-Dashboard/`. Before the bundled server starts,
`aicc-server.log` and `aicc-server-error.log` are capped at 1 MiB by retaining their latest 256 KiB;
log maintenance failures never block server startup.

The AICC server owns WorkBuddy bridge auto-heal. The bundled App owns the server lifecycle; no external LaunchAgent is
registered by the production path.

The self-contained App is generated at `dist/mac/AICC.app`; the DMG is written to `dist/` by default. Set `RELEASE_DIR` to place release artifacts elsewhere. The DMG bundles the server code and can be dragged into `/Applications`. Runtime data is stored in `~/Library/Application Support/AICC-Dashboard/data/`, not inside the App bundle.

The App requires macOS 14 or later, Apple Silicon, and an executable Python 3.10+. The DMG does not include Python. Without a Developer ID signature, the artifact is ad hoc signed; on another Mac, use Finder's right-click → Open flow if Gatekeeper blocks the first launch.

The menu-bar App displays status, starts and stops the internal data service, controls OpenCodex, opens Settings, shows logs, and configures launch at login. Fixed collectors remain in the existing Python service. The supervisor only checks `/api/health/live`; the dashboard reads cached `/api/status` data and does not trigger an extra collector refresh.

## Manual update checks

Settings → General → About & Updates checks once only when you click the button. It does not poll in the background, download files, or install updates automatically.

Published builds use this public manifest:

`https://raw.githubusercontent.com/easonwong2026-del/AICC/main/updates/aicc-update.json`

Set `AICC_UPDATE_MANIFEST_URL` in the App environment to override it. The manifest URL and its download/release-note URLs must use HTTPS. If the source is missing or not configured, Settings shows “Update source not configured” and provides the GitHub Releases page.

Manifest format:

```json
{
  "version": "2.5.1",
  "build": "5",
  "minimumSystemVersion": "14.0",
  "downloadURL": "https://example.com/AICC-2.5.1.dmg",
  "releaseNotesURL": "https://example.com/aicc/releases/2.5.1",
  "publishedAt": "2026-08-03T00:00:00Z"
}
```

### Release version rules

- `VERSION` is the only manual source of the AICC product version.
- A new user-feature release increments the minor version; a bugfix release increments the patch version.
- Increment `CFBundleVersion` for every formal macOS release; ordinary development commits do not bump versions automatically.
- Update `updates/aicc-update.json` only after the corresponding GitHub Release and DMG both exist.
- The 2.7.0 release is published and is present in the public update manifest.

### macOS desktop Widget

- Requirements: macOS 14+, Apple Silicon, with the AICC App installed and running.
- Add it from the desktop: right-click → Edit Widgets → search for “AICC”, then choose Small or Medium.
- Small shows Codex and WorkBuddy; Medium shows Codex, WorkBuddy, DeepSeek, and System.
- The Widget reads `http://127.0.0.1:8765/api/status`. The production port is fixed at `8765`; the Widget does not call the refresh endpoint or start the server.
- Use the refresh button in the Widget to reload its timeline. The AICC App also notifies WidgetKit on launch and when displayed data changes.
- If the server is temporarily unavailable, the Widget keeps the last successful snapshot as stale data; a first install without a cache shows `—` placeholders.
- The AICC App starts and supervises the backend. After an App restart, it reloads Widget timelines and resumes live data.

## Data collection and privacy

- Codex: starts `codex app-server` on demand, reads account limits, and stops the child process after 30 seconds without panel access while retaining the last successful cache.
- WorkBuddy: reads account balance through the local `127.0.0.1:9223` debugging bridge. AICC does not start WorkBuddy when the App is closed and does not save or transmit tokens or cookies.
- DeepSeek: reads the key from the environment or macOS Keychain and never writes it to the project directory.
- System: uses built-in macOS tools for memory and CPU information.

Collectors run independently. A timeout in one service does not block the whole dashboard, concurrent requests share an in-flight collection, and caches are written only when data changes or the save interval expires.

LAN devices can read the dashboard. Refresh, write, and WorkBuddy reconnect endpoints are restricted to the Mac itself.

## Poke4S and Android

Open the kiosk page and tap “Enter e-ink mode”. The page refreshes every five minutes and keeps the last successful data.

The current Android source version is `1.2.5-pencil-home` (versionCode 11) and is independent of macOS 2.7.0. Do not use an old or nonexistent APK path from the repository; download the latest APK from [GitHub Releases](https://github.com/easonwong2026-del/AICC/releases). The published 1.2.5 APK is currently attached to the [v2.5.0 release](https://github.com/easonwong2026-del/AICC/releases/tag/v2.5.0), with a direct [APK download](https://github.com/easonwong2026-del/AICC/releases/download/v2.5.0/Poke4S-AI-Dashboard-v1.2.5-pencil-home.apk).

The Android source is in `android/poke-dashboard/` and is independently buildable with the Gradle wrapper. It is not a runtime dependency of the Mac server. Release builds enable code and resource shrinking.

## Validation

```bash
python3 -B -m unittest discover -s tests -v
swift test --package-path macos/MenuBarApp
bash scripts/smoke-test-swift-core.sh
bash scripts/smoke-test-widget.sh
BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh
bash scripts/smoke-test-bundled-server.sh
bash macos/build-dmg.sh
bash scripts/validate-version.sh
( cd android/poke-dashboard && ./gradlew test assembleRelease )
```

See [English changelog](CHANGELOG.en.md), [中文完整 changelog](CHANGELOG.md), and [LICENSE](LICENSE) for release history and licensing information.
