# AICC 2.4.1 Performance Baseline

## Measurement Environment

- macOS: Sequoia 26.x (Apple Silicon)
- AICC Swift: Release build via `BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh`
- Warm-up: 2 minutes after launch, menu bar panel closed
- Samples: 3, median reported, 5 second interval between samples
- Tool: `scripts/measure-resources.sh`

## Baseline (2.4.0, pre-optimisation)

**Important**: This baseline was recorded on a mixed-installation environment. The Swift
PID came from the 2.4.0 Release build (`dist/mac/AICC.app`). The Python PID came from
the older `/Applications/AI E-Ink Dashboard.app` installation, **not** the bundled
2.4.0 server. Resource differences between bundled and external server instances are
therefore **not directly attributable** to this release.

### Panel Closed

| Metric | Swift (AICC.app) | Python (external /Applications) |
|--------|-------------------|----------------------------------|
| RSS (KB) | 104448 / 93264 / 93232 → **median 93264** | 12752 / 13440 / 13440 → **median 13440** |
| CPU (%) | 0.1 / 0.1 / 0.2 → **median 0.1** | 0 / 0 / 0 → **median 0** |
| Threads | 4 / 4 / 4 → **median 4** | 6 / 4 / 4 → **median 4** |
| FDs | 78 / 78 / 78 → **median 78** | 40 / 38 / 38 → **median 38** |

Thread and FD counts above exclude the header line emitted by `ps -M` and
`lsof`; the originally captured raw line counts included that header.

### Panel Open

To be recorded separately — requires manually opening the menu bar panel,
re-running with `--label open`.

### Steady-State HTTP Requests

Not available — the external `/Applications/AI E-Ink Dashboard.app` log path
was not captured. Use `AICC_ACCESS_LOG=~/Library/Logs/AICC-Dashboard/aicc-server.log`
with `scripts/measure-resources.sh` in future runs.

## Post-Optimisation (2.4.1)

The release build was copied to a temporary app bundle, given a temporary
bundle identifier to avoid replacing or stopping the installed 2.4.0 app, and
ad-hoc signed. It reused the already-running service on port 8765, so it did
not start a second Python process.

### Panel Closed

| Metric | Swift (2.4.1 temporary release app) | Python (same external legacy service) |
|--------|-------------------------------------|---------------------------------------|
| RSS (KB) | 65968 / 72320 / 71920 → **median 71920** | 12944 / 13088 / 13088 → **median 13088** |
| CPU (%) | 0.0 / 0.3 / 0.0 → **median 0.0** | 0.0 / 0.0 / 0.0 → **median 0.0** |
| Threads | 3 / 9 / 4 → **median 4** | 4 / 4 / 4 → **median 4** |
| FDs | 50 / 49 / 49 → **median 49** | 38 / 38 / 38 → **median 38** |

Compared with the mixed-environment 2.4.0 Swift baseline, median Swift RSS
was 21,344 KB lower (22.9%) and the median FD count was 29 lower (37.2%).
This is a directional local result, not a controlled benchmark: the test app
used a temporary bundle identifier and both versions shared an external legacy
Python service. The small Python difference is not attributable to 2.4.1.

### Panel Open

Not measured automatically. Opening a menu-bar extra reliably requires UI
automation permissions, which were not changed for this release.

### Steady-State Requests

Request counts remain unavailable because the shared legacy server was not
running with an access log. Code-level verification confirms that the removed
Swift diagnostic task no longer issues a 60-second request and that OpenCodex
polling is owned by the visible panel lifecycle.

For a controlled follow-up run:

```bash
# Panel closed
bash scripts/measure-resources.sh --label closed --warmup 120 --samples 3

# Open the menu bar panel manually, then:
bash scripts/measure-resources.sh --label open --warmup 0 --samples 3
```

## Key Changes Affecting Resource Usage

| Change | Expected Effect |
|--------|----------------|
| Removed the Codex/ChatGPT workspace launch observer | No persistent app-launch callback observer |
| Removed `APIService.startHealthRefresh` (60s health polling) | −1 running task, −1 periodic HTTP request every 60 s |
| OpenCodex status: panel-only polling (8-10 s) | Zero background requests when panel closed |
| Removed the menu-bar system-health row | No separate Swift diagnostic health fetch |
| Removed the three desktop-coupling preferences | Three fewer `@AppStorage` properties |
| Deleted dead files (main.m.bak, build-menubar-app.sh) | No runtime effect |
| ProcessRunner replaces inline `Process` + `Pipe` usage | Safer file-handle lifecycle, but identical steady-state FD count |

## Verification

```bash
# Build
BUNDLE_SERVER=1 bash macos/build-aicc-swiftui.sh

# Run baseline measurement
bash scripts/measure-resources.sh --label closed

# Verify no background OpenCodex polling
# (panel closed: lsof -iTCP -sTCP:ESTABLISHED -p <Swift-PID> should show 0 ocx connections)

# Verify no Codex/ChatGPT workspace launch observer
# (search logs or use instruments)
```
