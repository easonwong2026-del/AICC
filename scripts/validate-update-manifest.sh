#!/bin/bash
# Validate updates/aicc-update.json schema, HTTPS URLs, and version consistency.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
MANIFEST="$ROOT/updates/aicc-update.json"
VERSION_FILE="$ROOT/VERSION"
INFO_PLIST="$ROOT/macos/MenuBarApp/Info.plist"

[[ -f "$MANIFEST" ]] || { echo "Missing manifest: $MANIFEST" >&2; exit 1; }
[[ -f "$VERSION_FILE" ]] || { echo "Missing VERSION: $VERSION_FILE" >&2; exit 1; }
[[ -f "$INFO_PLIST" ]] || { echo "Missing Info.plist: $INFO_PLIST" >&2; exit 1; }

python3 - "$MANIFEST" "$VERSION_FILE" "$INFO_PLIST" <<'PY'
import datetime
import json
import pathlib
import plistlib
import re
import sys
from urllib.parse import urlparse

manifest_path = pathlib.Path(sys.argv[1])
version_path = pathlib.Path(sys.argv[2])
plist_path = pathlib.Path(sys.argv[3])

TRUSTED_HOSTS = {
    "github.com",
    "raw.githubusercontent.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "codeload.github.com",
}

SEMVER_RE = re.compile(
    r"^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
    r"(?:-((?:0|[1-9]\d*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*)"
    r"(?:\.(?:0|[1-9]\d*|[0-9A-Za-z-]*[A-Za-z-][0-9A-Za-z-]*))*))?"
    r"(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$"
)
SYSVER_RE = re.compile(r"^\d+(?:\.\d+){0,2}$")


def fail(message: str) -> None:
    print(f"FAIL: {message}", file=sys.stderr)
    sys.exit(1)


def parse_core(ver: str) -> tuple[int, int, int]:
    match = SEMVER_RE.match(ver)
    if not match:
        fail(f"invalid semantic version: {ver!r}")
    return int(match.group(1)), int(match.group(2)), int(match.group(3))


def validate_https_url(field: str, value: object) -> None:
    if value is None:
        return
    if not isinstance(value, str) or not value.strip():
        fail(f"{field} must be a non-empty string when present")
    parsed = urlparse(value.strip())
    if parsed.scheme.lower() != "https":
        fail(f"{field} must use https: {value!r}")
    if not parsed.hostname or parsed.username or parsed.password:
        fail(f"{field} has invalid authority: {value!r}")
    if parsed.hostname.lower() not in TRUSTED_HOSTS:
        fail(f"{field} host {parsed.hostname!r} is not in trusted GitHub hosts")


try:
    data = json.loads(manifest_path.read_text(encoding="utf-8"))
except Exception as exc:
    fail(f"cannot parse {manifest_path}: {exc}")

if not isinstance(data, dict):
    fail("manifest root must be a JSON object")

version = data.get("version")
if not isinstance(version, str) or not version.strip():
    fail("manifest.version must be a non-empty string")
version = version.strip()
manifest_core = parse_core(version)

repo_version = version_path.read_text(encoding="utf-8").strip()
repo_core = parse_core(repo_version)
if manifest_core > repo_core:
    fail(f"manifest version {version} is ahead of repository VERSION {repo_version}")

build = data.get("build")
if build is not None:
    if not isinstance(build, str) or not build.isdigit() or int(build) <= 0:
        fail(f"manifest.build must be a positive integer string, got {build!r}")

min_sys = data.get("minimumSystemVersion")
if min_sys is not None:
    if not isinstance(min_sys, str) or not SYSVER_RE.match(min_sys.strip()):
        fail(f"manifest.minimumSystemVersion is invalid: {min_sys!r}")
    with plist_path.open("rb") as fh:
        plist = plistlib.load(fh)
    plist_min = str(plist.get("LSMinimumSystemVersion", "")).strip()
    if plist_min and min_sys.strip() != plist_min:
        fail(
            f"manifest.minimumSystemVersion ({min_sys.strip()}) != "
            f"Info.plist LSMinimumSystemVersion ({plist_min})"
        )

validate_https_url("downloadURL", data.get("downloadURL"))
validate_https_url("releaseNotesURL", data.get("releaseNotesURL"))

published = data.get("publishedAt")
if published is not None:
    if not isinstance(published, str) or not published.strip():
        fail("manifest.publishedAt must be a non-empty string when present")
    try:
        datetime.datetime.fromisoformat(published.strip().replace("Z", "+00:00"))
    except ValueError as exc:
        fail(f"manifest.publishedAt is not valid ISO-8601: {exc}")

sha256 = data.get("sha256")
if sha256 is not None:
    if not isinstance(sha256, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", sha256.strip()):
        fail(f"manifest.sha256 must be a 64-char hex digest when present, got {sha256!r}")

print(
    f"OK: updates/aicc-update.json valid (v{version}, build {build or '—'}, "
    f"min macOS {min_sys or '—'}, repo VERSION {repo_version})."
)
PY
