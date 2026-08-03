# AICC

[简体中文](README.md)

AICC is a local AI status center for macOS. It displays Codex, WorkBuddy, DeepSeek, and system status, with support for Poke4S e-ink devices. The server uses only the Python standard library.

Since 2.5.0, AICC uses a dynamic Provider architecture. Collectors expose raw status, a manifest adapter produces display data, and the macOS menu-bar app drives cards, ordering, visibility, and actions from the manifest. Adding a built-in Provider does not require changes to the Dashboard core. See [Provider architecture](docs/provider-architecture.md), [Provider schema v1](docs/provider-schema-v1.md), [Provider UI guidelines](docs/provider-ui-guidelines.md), and the [built-in Provider guide](docs/adding-a-built-in-provider.md).

## Access

- Mac dashboard: `http://localhost:8765`
- Settings: native SwiftUI Settings window from the menu bar
- Poke4S: `http://<Mac Wi-Fi address>:8765/?kiosk=1`

The Mac and Poke4S device must be on the same Wi-Fi network. The native Android client can discover the Mac over UDP.

## macOS setup and maintenance

See [MAC-MIGRATION.md](MAC-MIGRATION.md) for the first deployment. Common commands:

```bash
bash macos/start-dashboard.sh
bash macos/install-autostart.sh
bash macos/uninstall-autostart.sh
bash macos/set-deepseek-key.sh
BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh
bash macos/build-dmg.sh
```

Safe updates run tests and create a complete backup while preserving quota history and caches:

```bash
bash macos/update-from-directory.sh /path/to/new-dashboard
bash macos/rollback-from-backup.sh /path/to/backup
```

Logs are stored in `~/Library/Logs/AICC-Dashboard/` with daily size limits.

The self-contained App is generated at `dist/mac/AICC.app`; the DMG is written to `dist/` by default. Set `RELEASE_DIR` to place release artifacts elsewhere. The DMG bundles the server code and can be dragged into `/Applications`. Runtime data is stored in `~/Library/Application Support/AICC-Dashboard/data/`, not inside the App bundle.

The App requires macOS 14 or later, Apple Silicon, and an executable Python 3 (Python 3.10+ recommended). The DMG does not include Python. Without a Developer ID signature, the artifact is ad hoc signed; on another Mac, use Finder's right-click → Open flow if Gatekeeper blocks the first launch.

The menu-bar App displays status, starts and stops the internal data service, controls OpenCodex, opens Settings, shows logs, and configures launch at login. Provider collection remains in the existing Python service. The supervisor only checks `/api/health/live`; the dashboard reads cached `/api/status` data and does not trigger an extra Provider refresh.

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

## Data collection and privacy

- Codex: starts `codex app-server` on demand, reads account limits, and stops the child process after 30 seconds without panel access while retaining the last successful cache.
- WorkBuddy: reads account balance through the local `127.0.0.1:9223` debugging bridge. AICC does not start WorkBuddy when the App is closed and does not save or transmit tokens or cookies.
- DeepSeek: reads the key from the environment or macOS Keychain and never writes it to the project directory.
- System: uses built-in macOS tools for memory and CPU information.

Collectors run independently. A timeout in one service does not block the whole dashboard, concurrent requests share an in-flight collection, and caches are written only when data changes or the save interval expires.

LAN devices can read the dashboard. Refresh, write, and WorkBuddy reconnect endpoints are restricted to the Mac itself.

## Poke4S and Android

Open the kiosk page and tap “Enter e-ink mode”. The page refreshes every five minutes and keeps the last successful data.

The Android source is in `android/poke-dashboard/` and is independently buildable with the Gradle wrapper. It is not a runtime dependency of the Mac server. Release builds enable code and resource shrinking.

## Validation

```bash
python3 -B -m unittest discover -s tests -v
swift test --package-path macos/MenuBarApp
bash scripts/smoke-test-swift-core.sh
BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh
bash scripts/smoke-test-bundled-server.sh
bash macos/build-dmg.sh
bash scripts/validate-version.sh
```

See [English changelog](CHANGELOG.en.md), [中文完整 changelog](CHANGELOG.md), and [LICENSE](LICENSE) for release history and licensing information.
