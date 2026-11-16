#!/bin/bash
# Hard gate for the bundled Python runtime (PR-B).
#
# Fails the build when the runtime is not the version AICC decided on, was
# linked against the machine that produced it, or cannot complete a TLS 1.3 and
# a TLS 1.2 handshake.  Nothing here trusts a version string: every claim is
# re-derived by running the interpreter and reading its dynamic libraries.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
RUNTIME="${1:-$ROOT/dist/python-runtime/python}"
PYTHON="$RUNTIME/bin/python3"
EXPECT_VERSION="${AICC_RUNTIME_VERSION:-3.14.10}"
EXPECT_OPENSSL_PREFIX="OpenSSL 3.5"

fail() { echo "FAIL: $*" >&2; exit 1; }
ok() { echo "ok: $*"; }

[[ -x "$PYTHON" ]] || fail "no interpreter at $PYTHON"
[[ -f "$RUNTIME/lib/python3.14/os.py" ]] || fail "stdlib missing under $RUNTIME/lib/python3.14"

# 1. Version and TLS capability, measured by running it.
cd /tmp   # deliberately not the runtime directory: must not depend on the cwd
probe="$("$PYTHON" -c 'import ssl,sys;print(sys.version.split()[0]);print(ssl.OPENSSL_VERSION);print(ssl.HAS_TLSv1_3)')"
version="$(sed -n 1p <<<"$probe")"
openssl="$(sed -n 2p <<<"$probe")"
has13="$(sed -n 3p <<<"$probe")"
[[ "$version" == "$EXPECT_VERSION" ]] || fail "python is $version, expected $EXPECT_VERSION"
case "$openssl" in
  "$EXPECT_OPENSSL_PREFIX"*) ok "python $version, $openssl" ;;
  *) fail "openssl is $openssl, expected ${EXPECT_OPENSSL_PREFIX}* (TLS 1.3 needs 3.5 LTS)" ;;
esac
[[ "$has13" == "True" ]] || fail "ssl.HAS_TLSv1_3 is $has13"

# 2. The extension modules the bridge actually imports.
"$PYTHON" -c 'import _ssl, _hashlib, json, sqlite3, hmac, secrets, socket, threading, uuid, http.server, urllib.request' \
  || fail "an extension module the bridge needs is missing"
ok "extension modules import (_ssl, _hashlib, sqlite3, http.server)"

# 3. Nothing may point at the build machine.  A single absolute dependency
#    outside /usr/lib or /System/Library makes the App fail on any other Mac,
#    which is exactly how the python.org framework route broke.
scan_output="$("$PYTHON" - "$RUNTIME" <<'PY'
import subprocess, sys
from pathlib import Path

root = Path(sys.argv[1])
allowed_prefixes = ("@", "/usr/lib/", "/System/Library/")
macho = dylibs = 0
bad = []
deps = set()
for path in sorted(root.rglob("*")):
    if path.is_dir() or path.is_symlink():
        continue
    kind = subprocess.run(["file", "-b", str(path)], capture_output=True, text=True).stdout
    if "Mach-O" not in kind:
        continue
    macho += 1
    if path.suffix in (".so", ".dylib"):
        dylibs += 1
    listed = subprocess.run(["otool", "-L", str(path)], capture_output=True, text=True).stdout
    for line in listed.splitlines()[1:]:
        dep = line.strip().split(" (compatibility")[0].strip()
        if not dep:
            continue
        deps.add(dep)
        if not dep.startswith(allowed_prefixes):
            bad.append(f"{path.relative_to(root)} -> {dep}")
print(f"MACHO={macho}")
print(f"DYLIB={dylibs}")
print(f"DEPS={len(deps)}")
for entry in bad:
    print(f"BAD={entry}")
PY
)"
echo "$scan_output" | sed -n 's/^/    /p'
grep -q '^BAD=' <<<"$scan_output" && fail "runtime links outside /usr/lib or /System/Library (see BAD lines)"
macho_count="$(sed -n 's/^MACHO=//p' <<<"$scan_output")"
dylib_count="$(sed -n 's/^DYLIB=//p' <<<"$scan_output")"
ok "$macho_count Mach-O files ($dylib_count .so/.dylib) link only against @rpath, /usr/lib or /System/Library"

# 4. Relocatable: a copy at a different path must run with no environment help.
relo="$(mktemp -d /tmp/aicc-runtime-relo.XXXXXX)"
trap 'rm -rf "$relo"' EXIT
cp -R "$RUNTIME" "$relo/python"
relo_version="$(env -u PYTHONHOME -u PYTHONPATH "$relo/python/bin/python3" -c \
  'import ssl,sys;print(sys.version.split()[0], ssl.HAS_TLSv1_3)')"
[[ "$relo_version" == "$EXPECT_VERSION True" ]] || fail "copy at $relo does not run: $relo_version"
ok "relocatable (runs from a copied path with PYTHONHOME/PYTHONPATH unset)"

# 5. Real handshakes, both ends inside this interpreter.
"$PYTHON" "$ROOT/tests/runtime/tls_selfcheck.py" || fail "TLS self-check failed"

echo "PASS: $RUNTIME is a $EXPECT_VERSION runtime with $EXPECT_OPENSSL_PREFIX, TLS 1.3 preferred, TLS 1.2 kept"
