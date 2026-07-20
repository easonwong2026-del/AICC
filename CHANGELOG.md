# Changelog

## 2.3.0 - 2026-07-20

- Add a native 128 KB macOS menu bar app for starting, stopping, restarting, and monitoring the local dashboard service.
- Show cached Codex quota directly in the menu bar without triggering a quota refresh or collector run.
- Add a generated monochrome macOS app icon and a reproducible `macos/build-menubar-app.sh` build path.
- Ship Android 1.2.5 Pencil Home UI with the final Poke4S home layout, larger quota typography, corrected reset-credit expiry fallback, and fixed charging status text.
- Document that the menu bar app is built into `dist/mac/AI E-Ink Dashboard.app` first, with `/Applications` installation kept as an explicit user action.

## 2.2.2 - 2026-07-18

- Stop the scheduled WorkBuddy monitor from launching WorkBuddy while the app is closed.
- Probe the visible WorkBuddy balance only once per newly opened renderer session, then serve the last successful cached balance until the next session.
- Continue reading local daily usage without waking WorkBuddy or repeatedly opening its account menu.

## Android 1.2.2 Optimized C - 2026-07-18

- Apply the selected compact two-column layout while retaining every V1.2.1 field.
- Pair each Codex quota with its own reset time, and pair reset-credit count with reset-credit expiry in a separate footer row.
- Place WorkBuddy and DeepSeek side by side, keep balance and daily use distinct, and trim meaningless trailing zeroes from DeepSeek daily usage.
- Preserve the V1.2 long-press settings dialog and the allocation-light single-Activity renderer.

## Android 1.2.1 Optimized - 2026-07-18

- Restore the original V1.2 AI COMMAND dashboard, four equal rectangular cards, dual Codex quota rows, split DeepSeek balance/usage view, and long-press settings dialog.
- Retain the current server contract, collector freshness states, reset-credit fields, Mac system details, and Wi-Fi auto-discovery.
- Keep reused Canvas geometry, coalesced networking, guarded callbacks, unchanged-cache write suppression, R8/resource shrinking, single-task launch, and zero WebViews.

## Android 2.1.0 - 2026-07-18

- Rework the Poke4S home screen around the selected compact-card layout.
- Use the device's Noto Sans CJK system face (the Source Han Sans family) with heavier synthetic strokes, without bundling a font asset.
- Enlarge primary figures, labels, status text, and system details while tightening margins and card gaps.
- Replace fragile hairlines with high-contrast 3 px borders, dividers, and an outlined 13 px quota bar.
- Keep the allocation-light native Canvas renderer, single Activity, and zero WebViews.

## 2.2.1 - 2026-07-18

- Remove the unused legacy manual Codex snapshot and stale Windows-only documentation.
- Replace the collector dataclass with a compact slotted record and avoid loading runtime-only `typing` and Windows `ctypes` modules on macOS.
- Release the on-demand Codex child after 30 idle seconds instead of 90 seconds.
- Guard Android callbacks after Activity destruction and avoid rewriting unchanged cached responses.
- Make updates and rollbacks remove stale code while continuing to preserve live data.
- Make LaunchAgent reloads tolerate macOS duplicate-registration timing instead of reporting a false deployment failure.
- Add the Poke4S native V2.0 client with a redesigned monochrome premium UI, full-page settings navigation, current backend fields, and an 18 KB shrunk release APK.
- Keep the Poke4S launcher in a single Android task to prevent duplicate activities and excess memory on BOOX firmware.
- Increase dashboard labels, states, detail rows, and provider values for real-device readability on the 758×1024 Poke4S panel.
- Refresh the package metadata and Mac-focused operating guide.

## 2.2.0 - 2026-07-18

- Run Codex, DeepSeek, WorkBuddy, and system collection independently with bounded wait time.
- Expose collector freshness, errors, and last-success timestamps to the dashboard.
- Restrict write and recovery endpoints to localhost and add browser security headers.
- Add WorkBuddy bridge monitoring, a local reconnect action, and a 60-second health check.
- Deduplicate balance history and reduce unchanged cache writes.
- Cap service logs with a daily maintenance LaunchAgent.
- Add integration tests plus backup-based update and rollback scripts.
- Preserve the on-demand Codex child-process shutdown introduced during the macOS migration.

## 2.1.0 - 2026-07-14

- Initial macOS migration package with Codex, DeepSeek, WorkBuddy, and Poke4S discovery support.
