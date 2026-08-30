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
