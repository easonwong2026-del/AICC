# macOS installation and migration checklist

AICC's supported macOS production runtime is the self-contained DMG App. The App
uses `SMAppService` for login-at-launch and `ServerManager` for the bundled Python
server; Docker and external LaunchAgents are not part of the production setup.

## Recommended production deployment

1. Download the current AICC DMG from [GitHub Releases](https://github.com/easonwong2026-del/AICC/releases).
2. Open the DMG and drag `AICC.app` into `/Applications`.
3. Launch AICC and confirm the dashboard is available at
   `http://127.0.0.1:8765`.
4. In Settings, use “About & Updates” to check the public update manifest. AICC
   does not install updates or run a background updater automatically; install a
   newer published DMG when one is available.
5. Enable login at launch in Settings if the App should start after login. This
   setting is managed by `SMAppService`.

The production App writes server logs to
`~/Library/Logs/AICC-Dashboard/` and runtime data to
`~/Library/Application Support/AICC-Dashboard/data/`. The bundled server owns
`127.0.0.1:8765`; the Widget and Poke4S read that same endpoint.

## Upgrade from an older source/LaunchAgent installation

On its first launch, the App performs an idempotent cleanup for only these exact
legacy identities:

- `com.aieink.dashboard`
- `com.aieink.workbuddy-monitor`
- `com.aieink.log-maintenance`

It unloads each matching job from the current user's GUI domain and removes the
matching plist from `~/Library/LaunchAgents` only after the old job is no longer
loaded. A failure is logged and ignored so the App can continue; AICC never
scans or kills unrelated LaunchAgents or Python processes. The old
`~/Library/Logs/AI-EInk-Dashboard/` directory is not used or written by the
current App; existing files there are left untouched.

## Source-checkout development only

For local backend debugging or smoke testing, a source checkout can still run
the server in the foreground:

```bash
chmod +x macos/start-dashboard.sh
bash macos/start-dashboard.sh
```

`start-dashboard.sh` is DEV ONLY. It does not install a login item, register a
LaunchAgent, or represent the DMG user's production startup path. Stop it with
`Ctrl-C` before launching the App on the same Mac so only one development or
production server owns port `8765`.

Developer-only helpers for local validation include:

```bash
bash macos/set-deepseek-key.sh
bash macos/start-workbuddy-monitored.sh
```

The WorkBuddy bridge is normally owned by the bundled server. The helper above
is useful for explicit source-checkout diagnostics and does not start the AICC
server or register a background service.

## Poke4S

The existing Poke4S client does not need to be rebuilt. Its UDP discovery request
finds the Mac and saves the current address. If discovery is blocked, long-press
the Poke4S dashboard and enter the Mac's Wi-Fi address manually, for example
`http://192.168.0.20:8765`.

## Arrival-day verification

- `http://127.0.0.1:8765/api/health/live` returns a healthy live response.
- `/api/codex/status` shows the expected Codex connection and quota data.
- `/api/status` shows DeepSeek after Keychain configuration.
- WorkBuddy balance updates or clearly falls back to its last successful cache.
- Poke4S discovers the Mac when both devices are on the same Wi-Fi network.
- After enabling login at launch, re-login restores AICC through `SMAppService`.
- `lsof -nP -iTCP:8765 -sTCP:LISTEN` shows only the expected AICC backend.
