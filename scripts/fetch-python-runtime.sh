#!/bin/bash
# Fetch the bundled Python runtime for AICC (PR-B).
#
# Source: astral-sh/python-build-standalone, `install_only` flavour — built for
# embedding (relative install names, no /Library/Frameworks, no Tk, no test
# suite).  The pinned asset is verified against the release's published
# SHA256SUMS before anything is unpacked, then handed to
# scripts/validate-python-runtime.sh, which runs it for real.
#
# Usage: bash scripts/fetch-python-runtime.sh [--dest DIR] [--cache DIR] [--no-validate]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"

PY_VERSION="3.14.10"
PBS_RELEASE="20261110"
PLATFORM="aarch64-apple-darwin"
FLAVOUR="install_only"
ASSET="cpython-${PY_VERSION}+${PBS_RELEASE}-${PLATFORM}-${FLAVOUR}.tar.gz"
ASSET_SHA256="42031ed5f0d38777335252c8c77d64708c8010dd67c128246bdec25000672666"
ASSET_BYTES=27109898
RELEASE_URL="https://github.com/astral-sh/python-build-standalone/releases/download/${PBS_RELEASE}"

DEST="$ROOT/dist/python-runtime/python"
CACHE="${AICC_RUNTIME_CACHE:-$ROOT/dist/python-runtime-cache}"
VALIDATE=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --dest) DEST="$2"; shift 2 ;;
    --cache) CACHE="$2"; shift 2 ;;
    --no-validate) VALIDATE=0; shift ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

mkdir -p "$CACHE"
PART="$CACHE/$ASSET.part"
ASSET_PATH="$CACHE/$ASSET"

size_of() { stat -f%z "$1" 2>/dev/null || echo 0; }
digest_of() { shasum -a 256 "$1" | awk '{print $1}'; }

# A cached file that does not match the pin must never be reused: it would make
# every later run fail the checksum instead of healing itself.
if [[ -f "$ASSET_PATH" ]]; then
  if [[ "$(digest_of "$ASSET_PATH")" != "$ASSET_SHA256" ]]; then
    echo "==> cached archive does not match the pinned sha256; discarding it"
    rm -f "$ASSET_PATH"
  fi
fi

if [[ ! -f "$ASSET_PATH" ]]; then
  echo "==> downloading $ASSET"
  # This network drops large GitHub release downloads mid-transfer, so resume
  # instead of restarting: each attempt keeps what it already wrote.
  for attempt in $(seq 1 40); do
    if [[ "$(size_of "$PART")" -ge "$ASSET_BYTES" ]]; then break; fi
    curl -sSL -C - --connect-timeout 15 --max-time 600 --speed-time 60 --speed-limit 2048 \
      -o "$PART" "$RELEASE_URL/$ASSET" || true
    echo "    attempt $attempt: $(size_of "$PART") / $ASSET_BYTES bytes"
  done
  # Only a complete, correctly checksummed download may be promoted.  A partial
  # `.part` stays put so the next run resumes it; promoting it would poison the
  # cache with a file that fails the checksum forever.
  [[ "$(size_of "$PART")" -eq "$ASSET_BYTES" ]] \
    || { echo "FAIL: download incomplete ($(size_of "$PART") of $ASSET_BYTES bytes); rerun to resume" >&2; exit 1; }
  if [[ "$(digest_of "$PART")" != "$ASSET_SHA256" ]]; then
    rm -f "$PART"
    echo "FAIL: downloaded archive failed sha256 and was removed; rerun to download it again" >&2
    exit 1
  fi
  mv "$PART" "$ASSET_PATH"
fi

echo "==> verifying sha256"
actual=$(digest_of "$ASSET_PATH")
[[ "$actual" == "$ASSET_SHA256" ]] || { echo "FAIL: sha256 $actual != $ASSET_SHA256" >&2; exit 1; }
echo "    $actual"

echo "==> unpacking"
STAGE="$(mktemp -d "$CACHE/unpack.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
tar xzf "$CACHE/$ASSET" -C "$STAGE"
[[ -d "$STAGE/python" ]] || { echo "FAIL: archive has no python/ directory" >&2; exit 1; }
unpacked=$(du -sk "$STAGE/python" | awk '{print $1}')

# Modules the bridge never imports.  Everything removed here is optional for
# `server.py`; the validator below proves the runtime still starts and speaks
# TLS, so a wrong guess fails the build instead of shipping.
for drop in \
  lib/python3.14/test \
  lib/python3.14/idlelib \
  lib/python3.14/ensurepip \
  lib/python3.14/pydoc_data \
  lib/python3.14/tkinter \
  lib/python3.14/turtledemo \
  lib/python3.14/turtle.py \
  lib/python3.14/lib2to3 \
  lib/python3.14/site-packages/pip \
  lib/python3.14/site-packages/setuptools \
  lib/python3.14/site-packages/_distutils_hack \
  lib/python3.14/site-packages/pkg_resources \
  lib/python3.14/lib-dynload/_tkinter.cpython-314-darwin.so \
  lib/python3.14/config-3.14-darwin \
  lib/tcl9.0 \
  lib/tk9.0 \
  lib/tcl9 \
  lib/tk9 \
  lib/itcl4.3.8 \
  lib/thread3.0.6 \
  lib/libtcl9.0.dylib \
  lib/libtcl9tk9.0.dylib \
  lib/pkgconfig \
  share \
  include ; do
  rm -rf "${STAGE:?}/python/$drop"
done
# `bin/python3.14` is statically linked (otool -L lists only libSystem, libz and
# libedit), so nothing loads lib/libpython3.14.dylib at runtime — it exists for
# third-party code that links libpython.  Dropping it removes 19 MB; the
# validator proves the interpreter still runs, imports its extensions and
# completes a TLS handshake without it.
rm -f "$STAGE/python/lib/libpython3.14.dylib"
# Console scripts are dev conveniences, and every extra executable inside the
# bundle is one more thing codesign has to walk (`pydoc3.14` is a shell script,
# which `codesign --deep` refuses to seal). The bridge runs `python3 -B`, so none
# of these are needed at runtime.
rm -f "$STAGE/python/bin/pip" "$STAGE/python/bin/pip3" "$STAGE/python/bin/pip3.14" \
      "$STAGE/python/bin/idle3" "$STAGE/python/bin/idle3.14" \
      "$STAGE/python/bin/pydoc3" "$STAGE/python/bin/pydoc3.14" \
      "$STAGE/python/bin/python3-config" "$STAGE/python/bin/python3.14-config" 2>/dev/null || true
pruned=$(du -sk "$STAGE/python" | awk '{print $1}')

mkdir -p "$(dirname "$DEST")"
rm -rf "$DEST"
mv "$STAGE/python" "$DEST"
trap - EXIT
rm -rf "$STAGE"

echo "==> measuring"
runtime_kb=$(du -sk "$DEST" | awk '{print $1}')
deb=$(find "$DEST" -type f \( -name '*.so' -o -name '*.dylib' \) | wc -l | tr -d ' ')
deb_kb=$(find "$DEST" -type f \( -name '*.so' -o -name '*.dylib' \) -exec stat -f%z {} + | awk '{total+=$1} END {print int(total/1024)}')
zip_file="$CACHE/$ASSET.zip"
rm -f "$zip_file"
( cd "$(dirname "$DEST")" && zip -qr -9 "$zip_file" "$(basename "$DEST")" )
zip_kb=$(( ($(stat -f%z "$zip_file") + 1023) / 1024 ))
asset_kb=$(( $(stat -f%z "$CACHE/$ASSET") / 1024 ))

printf '\n| 项目 | 体积 |\n| --- | --- |\n'
printf '| 下载包 (%s) | %s MB |\n' "$ASSET" "$((asset_kb / 1024))"
printf '| 解包后 | %s MB |\n' "$((unpacked / 1024))"
printf '| 裁剪后 | %s MB |\n' "$((pruned / 1024))"
printf '| 压缩后 (zip -9) | %s MB |\n' "$((zip_kb / 1024))"
printf '| 运行时文件数 / 动态库 | %s 个文件，%s 个 .so/.dylib（%s MB） |\n' \
  "$(find "$DEST" -type f | wc -l | tr -d ' ')" "$deb" "$((deb_kb / 1024))"
printf '\n'

if [[ "$VALIDATE" == "1" ]]; then
  echo "==> validating"
  bash "$ROOT/scripts/validate-python-runtime.sh" "$DEST"
fi

echo "runtime ready at $DEST"
