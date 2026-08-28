# AICC 2.7.0 / Build 9 — manual macOS acceptance

Status: **NOT EXECUTED by automation**. Complete this checklist on the target Mac before publishing the tag, GitHub Release, or final artifact.

1. Install the built DMG into `/Applications`, launch AICC, and confirm About shows `2.7.0` / Build `9`.
2. Confirm the backend is live at `http://127.0.0.1:8765/api/health/live` and that the dashboard shows current data.
3. Add the AICC Widget from the desktop widget editor; verify Small and Medium layouts render.
4. With the server online, verify the Widget displays current data, then use its refresh button and confirm it updates.
5. Stop the AICC server, wait for a Widget refresh, and confirm the last successful values remain available as stale cache data.
6. Verify a first-install/no-cache Widget shows `—` placeholders rather than failing.
7. Restart AICC and confirm the server returns on port `8765` and the Widget resumes live data.
8. In AICC Settings, enable login at launch; log out and back in, then confirm `SMAppService` restores AICC, the server, and Widget data.
9. Check `~/Library/Logs/AICC-Dashboard/aicc-server.log` and `aicc-server-error.log`; after restarting AICC with oversized files, confirm each is bounded and startup still succeeds.

Do not publish until every item passes.
