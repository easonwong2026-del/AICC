# AICC · Magpie multi-provider quota companion

[简体中文](README.md)

AICC shows the quota Magpie already knows about: a native macOS Widget and an Android e-ink panel covering every
provider and account in Magpie. Magpie owns authentication, accounts, routing and quota reads. AICC has no status
item, popover or web dashboard, no vendor collectors of its own, and never reads vendor credential files.
This branch is an unpublished candidate; the shipped version 2.10.2 / Build 15 and the bundle identities stay unchanged.

## What it shows

Magpie reports three kinds of quota, and AICC renders all three without hard-coding providers or window names:

| Kind | Example | Rendering |
| --- | --- | --- |
| `subscription` | Codex `5 hours` / `7 days`, WorkBuddy `Credits`, Antigravity `Gemini · 7 days` | one line per window, original window name kept |
| `plan` | GLM Coding Plan, Command Code plans | as above, plus plan name and term |
| `balance` | API-key or account balance with splits | the amount itself, never converted to a percentage |

Window names are dynamic and may repeat — Antigravity returns `Gemini · 7 days` before a refresh and
`Gemini 3.8 Flash (High)` plus seven windows afterwards. Nothing in the model assumes five-hour or weekly windows.
Account identities are masked: the snapshot keeps a stable anonymous id and a first-character label, while emails,
raw account names, provider error text and credentials never reach disk.

Verified against real quota on this machine: **Codex, WorkBuddy, Antigravity only**. See the
[provider compatibility matrix](docs/PROVIDER-COMPATIBILITY.md) for per-provider status.

## macOS Widget

Small shows up to two quota cards, medium up to four; each card is one provider account with its first two windows,
`+N` when it has more, and a remainder badge when the widget cannot fit everything. Empty, offline, cached, failed
and unknown-provider states each render explicitly — a missing Codex account never disables the widget.
Clicking the widget asks the host for **one refresh across every provider**, then reloads the timeline.

Auto-refresh aims at five minutes per port in the Widget process; WidgetKit decides the real schedule, so this is a
reload cadence, not a live timer. Failures fall back to a per-port anonymized cache labelled as cached; readings
older than five minutes are marked cached, and snapshots older than 24 hours are not shown as quota.

Configuration (port, card count, optional provider filter) lives in the Widget's own display-settings domain and is
edited in the AICC settings window. The widget kind identifier and upgrade compatibility are unchanged; the static
configuration is kept because dynamic App Intents configuration cannot be registered reliably with this build path.

## E-ink APK

The APK reads the same generic snapshot through the paired, read-only bridge: multiple providers, multiple accounts,
subscription windows, credits, plans and balances, with real cached/offline/failed state. Cards are four per page,
1/2/all counts are configurable, all-accounts mode pages on tap, and the panel only redraws when the content actually
changes. BOOX flash calibration, sleep pause and wake refresh are preserved.

Devices declare `X-AICC-Schema: 3`; the bridge serves the old `codex` shape only when explicitly asked (`?format=2`).
Bridge credentials are separate from Magpie's gateway key, stored in the Mac Keychain, handed to Python on stdin and
encrypted with Android Keystore; backup stays disabled. The LAN bridge is unencrypted HTTP — trusted networks only.

## Build and acceptance

See the [Chinese README](README.md) for the full command list, [migration notes](docs/MAGPIE-MIGRATION.md) for the API
contract, refresh policy and security boundary, and the [acceptance record](docs/ACCEPTANCE.md) for what was actually
run in this round. CI artifacts are candidates; they do not imply a release or an installation.
