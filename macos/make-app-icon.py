#!/usr/bin/env python3
"""Generate a tiny monochrome macOS icon without third-party dependencies."""

from __future__ import annotations

import os
import struct
import sys
import zlib
from pathlib import Path


def rgba_png_bytes(size: int) -> bytes:
    pixels = bytearray()
    radius = max(4, size // 8)
    border = max(2, size // 18)
    pad = max(8, size // 6)
    line = max(2, size // 14)

    def rounded_rect_alpha(x: int, y: int, inset: int) -> bool:
        left = inset
        top = inset
        right = size - inset - 1
        bottom = size - inset - 1
        if left + radius <= x <= right - radius and top <= y <= bottom:
            return True
        if left <= x <= right and top + radius <= y <= bottom - radius:
            return True
        for cx, cy in ((left + radius, top + radius), (right - radius, top + radius), (left + radius, bottom - radius), (right - radius, bottom - radius)):
            if (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2:
                return True
        return False

    for y in range(size):
        pixels.append(0)
        for x in range(size):
            outer = rounded_rect_alpha(x, y, pad)
            inner = rounded_rect_alpha(x, y, pad + border)
            draw = outer and not inner
            for idx in range(3):
                yy = size // 2 - line * 3 + idx * line * 3
                if pad * 2 <= x <= size - pad * 2 and yy <= y <= yy + line:
                    draw = True
            pixels.extend((20, 20, 20, 255 if draw else 0))

    def chunk(kind: bytes, data: bytes) -> bytes:
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)

    raw = bytes(pixels)
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    return png


def rgba_png(path: Path, size: int) -> None:
    path.write_bytes(rgba_png_bytes(size))


def icns(path: Path) -> None:
    entries = [
        (b"icp4", 16),
        (b"icp5", 32),
        (b"icp6", 64),
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
    ]
    chunks = []
    total = 8
    for icon_type, size in entries:
        data = rgba_png_bytes(size)
        chunk = icon_type + struct.pack(">I", len(data) + 8) + data
        chunks.append(chunk)
        total += len(chunk)
    path.write_bytes(b"icns" + struct.pack(">I", total) + b"".join(chunks))


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: make-app-icon.py OUTPUT_ICONSET_OR_ICNS", file=sys.stderr)
        return 2
    output = Path(sys.argv[1])
    if output.suffix == ".icns":
        output.parent.mkdir(parents=True, exist_ok=True)
        icns(output)
        return 0
    iconset = output
    iconset.mkdir(parents=True, exist_ok=True)
    outputs = {
        "icon_16x16.png": 16,
        "icon_16x16@2x.png": 32,
        "icon_32x32.png": 32,
        "icon_32x32@2x.png": 64,
        "icon_128x128.png": 128,
        "icon_128x128@2x.png": 256,
        "icon_256x256.png": 256,
        "icon_256x256@2x.png": 512,
        "icon_512x512.png": 512,
        "icon_512x512@2x.png": 1024,
    }
    for name, size in outputs.items():
        rgba_png(iconset / name, size)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
