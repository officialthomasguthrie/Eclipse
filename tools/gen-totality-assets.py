#!/usr/bin/env python3
"""Boot splash assets (sun, moon, corona), stdlib only.

Usage: gen-totality-assets.py [output-dir]   (default: nix/totality/plymouth)
"""

import math
import pathlib
import struct
import sys
import zlib

BG = (12, 10, 17)  # background, #0c0a11
GOLD = (228, 185, 110)  # #e4b96e


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


def disc(color, size: int, feather: float = 1.5):
    """Anti-aliased filled circle."""
    radius = size / 2 - 2
    centre = size / 2 - 0.5

    def pixel(x: int, y: int):
        d = math.hypot(x - centre, y - centre)
        alpha = max(0.0, min(1.0, (radius - d) / feather + 0.5))
        return (*color, round(alpha * 255))

    return pixel


def glow(color, size: int):
    """Soft radial glow for the corona."""
    centre = size / 2 - 0.5
    inner = size * 0.21
    outer = size * 0.5

    def pixel(x: int, y: int):
        d = math.hypot(x - centre, y - centre)
        if d <= inner:
            alpha = 0.55
        elif d >= outer:
            alpha = 0.0
        else:
            t = (d - inner) / (outer - inner)
            alpha = 0.55 * (1 - t) ** 2.2
        return (*color, round(alpha * 255))

    return pixel


def main() -> None:
    out = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "nix/totality/plymouth")
    out.mkdir(parents=True, exist_ok=True)
    write_png(out / "sun.png", 320, 320, disc(GOLD, 320))
    write_png(out / "moon.png", 320, 320, disc(BG, 320, feather=1.0))
    write_png(out / "corona.png", 760, 760, glow(GOLD, 760))
    for name in ("sun.png", "moon.png", "corona.png"):
        print(f"{out / name}: {(out / name).stat().st_size} bytes")


if __name__ == "__main__":
    main()
