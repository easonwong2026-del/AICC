# macOS snapshot / DeepSeek correctness verification

Baseline: `d9296be15de6d4aaf99042785027bc64c686cf7f` — `feat(macos): display Google Antigravity quota (#39)`.
Branch: `codex/fix-macos-snapshot-deepseek`.
The original checkout was `feat/macos-google-quota`, with the same tracked tree as main. The unrelated untracked `tmp_spreadsheet_work/` directory was preserved.

## Root causes

- Host reload notifications did not transfer a display snapshot. Widget fetched independently and retained its own UserDefaults fallback.
- Widget's refresh intent only reloaded timelines. Its nominal refresh interval was 15 minutes, subject to WidgetKit scheduling.
- A stale snapshot kept provider online booleans, and the Widget did not expose stale state. Dashboard cards derived their own presentation and did not consistently honor backend failure/cache state.
- DeepSeek collapsed network/JSON failures into `Connection error` and empty balances. CollectorManager treated that dictionary as a successful replacement.
- Server initialization discarded persisted DeepSeek data. Force refresh waited 3.2 seconds despite an 8-second collector timeout, so it could return before the requested attempt completed.
- urllib's module-global opener can retain proxy settings from before a VPN/system proxy change.

## Implemented contract

`/api/status` remains the only raw business-data source. Both clients compile the same `WidgetDisplaySnapshot` normalization code. The loopback backend stores the normalized snapshot published by a successful client through local-only `POST /api/display-snapshot`, and includes it in `/api/status` responses. A SHA-256 revision over relevant raw provider values and states invalidates old presentation data. Publications for obsolete revisions are rejected with 409.

The host publishes before reloading WidgetKit. The Widget refresh intent actually calls `POST /api/refresh`, publishes the resulting normalized snapshot, then reloads its timeline. Opening the Dashboard also fetches current backend status. No App Group is introduced: this Mac has no valid signing identity, and the installed/distributed test app uses ad-hoc signing without a Team ID. Existing entitlements remain valid; the build script compiles the shared Swift source into both binaries.

Snapshots carry `fetchedAt`, `stale`, and age. Backend failures immediately mark local fallbacks stale. Five-minute expiry entries stop an unrefreshed timeline from remaining live; 24-hour expiry entries hide offline old values. Provider stale/unavailable states are shared by both presentations. WidgetKit still controls actual timeline scheduling; a reload request is not proof that pixels have already updated.

DeepSeek preserves its prior balance and success time on collector/network errors, including manager timeouts. Persisted balances are restored as stale at startup. Recovery replaces them with a fresh successful result. Fixed diagnostics distinguish timeout, DNS, TLS, connection, invalid response, and individual HTTP status codes (including 401/403/429/5xx). No exception text or API key is included in those diagnostics. Proxy settings are re-read for each request using Python's standard environment/system precedence; no TLS bypass or implicit direct-connect fallback is added.

## Actual Mac diagnosis

- Existing Keychain item was present; the running installed backend could read it. No key was changed or removed.
- The app uses `/opt/homebrew/bin/python3` (Python 3.14), not an embedded Python executable. Its server source is bundled.
- Before installation, the actual backend had neither DeepSeek-key nor proxy environment variables; it relied on Keychain and macOS proxy settings.
- macOS HTTP/HTTPS/SOCKS proxy was `127.0.0.1:10808`. DNS resolved `api.deepseek.com` to `43.242.198.77` and `61.170.82.99` during diagnosis.
- Python urllib default and direct paths, and curl default/direct/explicit proxy paths, all returned HTTP 200 with `58.36 CNY`. TLS verification remained enabled.
- The earlier `Connection error` was no longer reproducible. Its exact historical cause cannot be recovered from the old generic error. VPN/proxy changes are a plausible contributor, not an established finding.

## Verification results

- Baseline Python: 79 tests passed outside the sandbox (sandbox-only failures were denied loopback binds).
- Final Python: 87 tests passed. The final source-invalidation refinement also passed all 19 server integration tests.
- Existing Widget smoke: passed, extended with age/expiry, stale balance and never-successful cases.
- New shared display smoke: passed using real `APIService`, Widget loader and URLProtocol responses. Covers old 48/97/5852.96 replacement, shared field equality, actual refresh methods, last-known-good stale display, and whole-backend failure.
- Live shared display smoke against the installed backend: passed. It performs host force refresh, Widget force refresh, then the GET used when reopening Dashboard.
- flake8, Python syntax, shell syntax, removed-symbol CI reference checks, and `git diff --check`: passed.
- Actual macOS app + Widget extension build and deep/strict codesign verification: passed.
- Bundled server smoke: passed.
- DMG build, version validation, and `hdiutil verify`: passed.
- Android `assembleRelease`: passed using installed Homebrew JDK 17 and Android SDK paths.
- Full `swift test`: blocked by this Mac's missing XCTest module (Command Line Tools without full Xcode). The complete core sources, including Google/OpenCodex code, compile in the standalone Swift smoke and app build. This does not substitute for XCTest execution.
- Remote GitHub CI has not been run for this local commit. The new smoke is included in the CI workflow.

## Installation and observed values

Installed `/Applications/AICC.app`; old app backup: `/tmp/AICC-before-snapshot-fix-20260907-092825.zip`.
The installed extension is registered, and its actual container preferences contain the new snapshot schema and recent timestamps.

Live Swift data-path comparison on this Mac (not a screenshot assertion):

| Field | Dashboard force refresh | Widget force refresh |
| --- | --- | --- |
| Codex weekly | 99% | 99% |
| Codex 5-hour | 95% | 95% |
| Codex reset | 2026-09-14 13:14 | 2026-09-14 13:14 |
| WorkBuddy | 5,947 | 5,947 |
| DeepSeek | 58.36 CNY | 58.36 CNY |
| DeepSeek state | Online | Online |

The quota reset changed during this session; these values are real observations, not replacements hardcoded from the original report. Later actual Widget cache reads showed 97% / 81% / 5,947 / 58.36 CNY with the new schema, demonstrating ongoing extension writes.

Native UI automation timed out repeatedly before obtaining the Dashboard accessibility tree or screenshot. After installation, the user reported that internal testing looked good and authorized submitting the PR. This is user-reported acceptance; no automated screenshot evidence was captured. Full XCTest and remote CI remain to be verified on GitHub before merge.

## PR #40 correctness follow-up

Starting head: `0457e3e61bc8181974a180bf1e64163ef9fcf4b6` (all five CI jobs passed).

- A timed-out normal generation now starts its pending force immediately. Every force caller joins the active/follow-up run, and expired generations return without touching values or metadata.
- DeepSeek watchdog budget includes the 5s Keychain lookup, 8s HTTP timeout and 2s cleanup margin. The server force wait derives from two watchdog budgets plus 1s (31s); host and Widget share a 40s transport timeout.
- Each failure replaces all three diagnostic fields together. Only balance, usage and last-success time survive; sequential HTTP/DNS/manager failures cannot reuse old diagnostics.
- `No balance` is fresh successful data with separate account status. `Not configured` is unavailable, not cached. The new account field round-trips through the shared snapshot while legacy decoding remains supported.
- APIService callers share one task: a running normal request gets at most one pending force, and an active force absorbs subsequent clicks. The complete local response is assigned on MainActor before awaiting snapshot publication.

Local follow-up verification: 92 Python tests passed; the force test file (7 tests) and DeepSeek regression file (10 tests, including three sequential-failure subcases) also passed independently. Widget and shared display smoke passed; app/extension build and signing passed. Four new XCTest cases cover request coalescing, publication ordering and account freshness. Local `swift test` remains unavailable because XCTest is missing; the updated PR's GitHub CI must run these tests before merge review. DMG/Android results and final CI status are recorded in the PR follow-up report. No merge was requested or performed.
