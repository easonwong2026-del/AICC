# macOS migration checklist

The Mac-ready backend uses native Python and `launchd`. Docker is intentionally
not used because Codex authentication and WorkBuddy desktop collection belong
to the signed-in macOS user session.

## Package contents

The portable package contains dashboard source, safe cached readings, fallback
settings, and history. It does not contain DeepSeek keys, Codex/ChatGPT
credentials, WorkBuddy tokens, Windows executables, Android SDK, or Gradle
downloads.

## First run on the Mac

1. Install and sign in to the current ChatGPT desktop app.
2. Verify that `codex --version` works in Terminal. If the command is installed
   somewhere unusual, set `CODEX_CLI_PATH` to its full path before starting.
3. Unzip the package into a stable location such as
   `~/AICC`. Avoid moving it after installing auto-start.
4. In Terminal, enter that directory and prepare the scripts:

```bash
chmod +x macos/*.sh
bash macos/start-dashboard.sh
```

5. Open `http://localhost:8765/api/codex/status`. A connected response confirms
   that macOS Codex authentication and `app-server` are working.
6. Store the DeepSeek key in macOS Keychain:

```bash
bash macos/set-deepseek-key.sh
```

7. After installing WorkBuddy, quit it completely and test the local collector:

```bash
bash macos/start-workbuddy-monitored.sh
```

The WorkBuddy macOS client must be validated on the actual machine. If its app
bundle or local database differs from Windows, the dashboard retains the manual
fallback instead of failing.

## Auto-start

After the manual checks pass:

```bash
bash macos/install-autostart.sh
```

The LaunchAgents start the dashboard and WorkBuddy's localhost-only monitoring
bridge at sign-in. The dashboard restarts after a crash.
Logs are stored in `~/Library/Logs/AICC-Dashboard/`.

To remove it:

```bash
bash macos/uninstall-autostart.sh
```

## Poke4S

The existing Poke4S APK does not need to be rebuilt. Its UDP discovery request
will find the Mac and save the new address. When macOS asks whether Python may
accept incoming connections, allow it on the trusted private network.

If discovery is blocked, long-press the Poke4S dashboard and enter the Mac's
Wi-Fi address manually, for example `http://192.168.0.20:8765`.

## Arrival-day verification

- `http://localhost:8765/api/health` returns `{"ok": true}`.
- `/api/codex/status` shows `Connected`, live percentages, and reset credits.
- `/api/status` shows DeepSeek after Keychain configuration.
- WorkBuddy balance updates or clearly falls back to manual data.
- Poke4S discovers the Mac after both devices join the same Wi-Fi.
- Re-login test confirms the LaunchAgent starts automatically.
