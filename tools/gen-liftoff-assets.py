#!/usr/bin/env python3
"""Boot splash assets (sun, moon, dot), stdlib only.

Usage: gen-liftoff-assets.py [output-dir]   (default: nix/liftoff/plymouth)

The sun is a light gray disc, the moon a black one a little smaller, so what is left is a black
disc with a thin ring. The dot is one bullet of the passphrase field. The script scales
all three to the screen, so the pngs are drawn large.
"""

import math
import pathlib
import struct
import sys
import zlib

SIZE = 512
RING = 0.03  # ring width as a fraction of the sun's diameter
GRAY = (204, 204, 204)  # #cccccc, the sun and the text
BLACK = (0, 0, 0)  # the moon


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
    sun = SIZE / 2 - 1
    write_png(out / "sun.png", SIZE, SIZE, disc(GRAY, SIZE, sun))
    write_png(out / "moon.png", SIZE, SIZE, disc(BLACK, SIZE, sun - RING * SIZE))
    write_png(out / "dot.png", 64, 64, disc(GRAY, 64, 31))
    for name in ("sun.png", "moon.png", "dot.png"):
        print(f"{out / name}: {(out / name).stat().st_size} bytes")


if __name__ == "__main__":
    main()
