# macOS Release Acceptance Checklist

Use this checklist for each candidate macOS release. Record the candidate version and build in the release PR or release report.

Status: **NOT EXECUTED by automation**

- [ ] Install the candidate DMG into `/Applications` and confirm About shows the recorded version/build.
- [ ] Confirm the bundled server is live at `http://127.0.0.1:8765/api/health/live` and `/api/status` shows current data.
- [ ] Confirm exactly one AICC process and one bundled Python server are running.
- [ ] Add the AICC Widget from the desktop widget editor; verify Small and Medium layouts render.
- [ ] With the server online, verify current Widget data and manual refresh.
- [ ] Stop the server and verify last-successful stale cache data remains available.
- [ ] Verify a first-install/no-cache Widget shows `—` placeholders rather than failing.
- [ ] Restart AICC and confirm port `8765`, live data, and Widget recovery.
- [ ] In AICC Settings, enable login at launch and verify recovery through `SMAppService` after logout/login.
- [ ] Confirm `~/Library/Logs/AICC-Dashboard/aicc-server.log` and `aicc-server-error.log` remain bounded after restart.
- [ ] Run version validation and record signing/notarization status.

## macOS Application Regression

Use a matching macOS GUI session for the candidate under test. Record the
candidate version/build, macOS version, date, and evidence. The automated gate
is [.github/workflows/ci.yml](.github/workflows/ci.yml); this section covers
manual GUI and lifecycle behavior without duplicating CI results.

### Settings window — 20 cycles

For each cycle, open Settings from the Dashboard gear, close it with the red
close button, and reopen it. Repeat the same cycle from the status-item
Settings menu. Verify:

- both entry points reuse one Settings window and focus or restore an existing
  window instead of creating a duplicate;
- closing Settings leaves AICC, the status item, the bundled server/API
  refresh, and the Dashboard available;
- reopening Settings does not crash or hang, and closing the last regular
  window does not terminate AICC.

The current `SettingsWindowCoordinator` intentionally keeps a recently closed
`NSWindow` alive briefly while AppKit finishes its close transaction. Immediate
hosting-tree release is not an acceptance condition.

### Status item / Dashboard — 30 cycles

Repeat left-click open/close and right-click menu interactions. Verify:

- left click toggles the Dashboard popover; right click opens a transient menu
  without opening the Dashboard;
- the menu is rebuilt for each invocation and contains four actions—open
  Dashboard, refresh all, Settings, and Quit—with one divider;
- the Dashboard popover can be closed and reopened without a crash, hang, or
  duplicate status item;
- Dashboard appearance/disappearance remains paired, displayed values and
  settings changes continue to update, and system appearance is respected;
- the Dashboard gear and status-item Settings action reach the same Settings
  window lifecycle.

### Update scenarios

| Scenario | Expected result |
| --- | --- |
| Update source not configured | Show the not-configured state and keep the Releases link usable. |
| Current/equal or older remote version | Show up to date; do not download or install. |
| Newer valid remote version | Show update available and open only the HTTPS release/download link. |
| Checking with repeated clicks | Keep one in-flight check; repeated clicks do not start another request. |
| Malformed manifest, invalid version, invalid JSON, or invalid URL | Show a failure state and keep Settings usable. |
| Non-HTTPS source or redirect | Reject the check and show a failure state. |
| HTTP error, network failure, or timeout | Show a failure state without crashing or blocking the app. |

Confirm About shows the current short version and build, with `—` for missing
bundle fields. No update path may automatically download or install software.

### Memory / stability

Record a baseline and a second measurement after the Settings and Dashboard
cycles above. In a real GUI session, the following built-in macOS tools can
provide evidence:

```bash
APP_PID="$(pgrep -x AICC | head -1)"
ps -o pid,ppid,rss,%cpu,command -p "$APP_PID"
footprint "$APP_PID"
leaks --noContent "$APP_PID"
```

Verify one intended AICC process and one intended bundled Python server, no
crash or hang, no unbounded RSS/footprint growth, and no accumulating Settings
windows or Dashboard popovers. Leave the app running through a normal
long-running interval and confirm scheduled refresh remains responsive. Do
not attribute system-framework leak reports directly to AICC without further
evidence.

### Result

- Candidate version/build:
- macOS version:
- Date:
- [ ] PASS
- [ ] FAIL
- [ ] BLOCKED
- Evidence and notes:
