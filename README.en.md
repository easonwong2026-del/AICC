# AICC · Magpie display companion

[简体中文](README.md)

AICC provides a native macOS Widget and an Android e-ink Codex quota display. Magpie owns authentication,
accounts, routing and model calls. AICC has no status item, popover dashboard or web dashboard, and no
independent vendor collectors. This branch is an unpublished candidate; the existing product version
2.10.2 / Build 15 and bundle identities remain unchanged.

The Widget reads local Magpie directly, defaults to Codex, and works with AICC closed. Edit the Widget to
choose its port and account count. Small displays up to two accounts; medium up to four. Existing metric
configurations still decode, but retired providers show unknown values. No legacy source cache is reused.
Automatic Codex refresh attempts are limited to five-minute intervals per port in the Widget process;
WidgetKit controls actual scheduling. Manual refresh only refreshes Codex. Failures retain same-port,
anonymized cache with explicit stale state; local snapshots expire after 24 hours.

The minimal settings App optionally runs a Python bridge for e-ink devices. Without the bridge, closing
its window exits AICC. With the bridge enabled, it remains in the background; login launch stays silent.
Android discovers HTTP 8765 over UDP 8766 and pairs using a single-use eight-digit code (two-minute
lifetime, five attempts). Its independent read-only credential is stored in the Mac Keychain, passed to
Python on stdin, and encrypted with Android Keystore. No Magpie gateway key reaches Android. Revoking
devices in settings rotates the bridge credential. Host identity prevents sending it to another host
after an IP change. Backup is disabled.

Android defaults to five-minute foreground updates, refreshes on resume, pauses when asleep/backgrounded,
retains meaningful-change redraw and existing BOOX flash calibration, and pages at most four accounts.
The LAN bridge uses unencrypted HTTP: use a trusted LAN and never expose it publicly.

See the [Chinese README](README.md) for setup/build commands and [migration evidence](docs/MAGPIE-MIGRATION.md)
for verified API contracts, version limits and outstanding physical-device acceptance. No release or
installation is implied by CI artifacts.
