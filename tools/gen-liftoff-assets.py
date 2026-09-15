#!/usr/bin/env python3
"""Boot splash assets (the passphrase dot), stdlib only.

Usage: gen-liftoff-assets.py [output-dir]   (default: nix/liftoff/plymouth)

The dot is one bullet of the graphical style's passphrase field, a light gray disc. The script scales
it to the screen, so the png is drawn large. The mark the style shows is nix/liftoff/logo/rift-mark.png.
"""

import math
import pathlib
import struct
import sys
import zlib

GRAY = (204, 204, 204)  # #cccccc, the dot and the text


def write_png(path: pathlib.Path, width: int, height: int, pixel) -> None:
    """Write an 8-bit RGBA PNG where pixel(x, y) returns (r, g, b, a)."""
    raw = bytearray()
    for y in range(height):
        raw.append(0)  # filter type: none
        for x in range(width):
            raw.extend(pixel(x, y))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + kind
            + data
            + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)
        )

    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )
    path.write_bytes(png)


def disc(color, size: int, radius: float):
    """Anti-aliased filled circle in the middle of a size x size canvas."""
    centre = size / 2 - 0.5

    def pixel(x: int, y: int):
        d = math.hypot(x - centre, y - centre)
        alpha = max(0.0, min(1.0, radius - d + 0.5))
        return (*color, round(alpha * 255))

    return pixel


def main() -> None:
    out = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "nix/liftoff/plymouth")
    out.mkdir(parents=True, exist_ok=True)
    dot = out / "dot.png"
    write_png(dot, 64, 64, disc(GRAY, 64, 31))
    print(f"{dot}: {dot.stat().st_size} bytes")


if __name__ == "__main__":
    main()
