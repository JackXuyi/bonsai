#!/usr/bin/env python3
"""Generate placeholder bonsai icons (icon.png 128x128, tray.png 32x32).

Pure stdlib PNG writer (zlib + struct), no Pillow dependency.
Regenerate: python3 scripts/gen_icons.py
"""

import struct
import zlib
from pathlib import Path

TRANSPARENT = (0, 0, 0, 0)
POT = (139, 90, 43, 255)       # terracotta
POT_RIM = (166, 111, 56, 255)
TRUNK = (92, 58, 32, 255)
LEAF = (76, 140, 43, 255)
LEAF_DARK = (58, 110, 33, 255)
LEAF_LIGHT = (106, 168, 70, 255)


def circle(cx: float, cy: float, r: float, x: int, y: int) -> bool:
    return (x - cx) ** 2 + (y - cy) ** 2 <= r * r


def rect(x0: int, y0: int, x1: int, y1: int, x: int, y: int) -> bool:
    return x0 <= x <= x1 and y0 <= y <= y1


def make_icon(size: int) -> bytes:
    s = size
    k = s / 128.0  # design in a 128px space, scale for other sizes
    px = bytearray(TRANSPARENT * s * s)

    def put(x: int, y: int, color):
        i = (y * s + x) * 4
        px[i:i + 4] = bytes(color)

    def blend(x: int, y: int, color):
        # simple paint-over
        put(x, y, color)

    for y in range(s):
        for x in range(s):
            fx, fy = x / k, y / k
            # pot (bottom)
            if rect(int(44 * k), int(92 * k), int(84 * k), int(118 * k), x, y):
                blend(x, y, POT)
                if fy <= 92 * k + 6:
                    blend(x, y, POT_RIM)
                continue
            # trunk
            if rect(int(60 * k), int(64 * k), int(68 * k), int(96 * k), x, y):
                blend(x, y, TRUNK)
                continue
            # branch to the left
            if rect(int(46 * k), int(66 * k), int(62 * k), int(72 * k), x, y):
                blend(x, y, TRUNK)
                continue
            # foliage: cluster of circles (bonsai cloud-pruning look)
            leaves = [
                (64, 44, 26, LEAF),
                (44, 56, 15, LEAF_DARK),
                (84, 56, 15, LEAF_LIGHT),
                (64, 66, 16, LEAF_DARK),
            ]
            for cx, cy, r, color in leaves:
                if circle(cx * k, cy * k, r * k, x, y):
                    blend(x, y, color)
                    break

    # encode PNG
    raw = b"".join(b"\x00" + bytes(px[y * s * 4:(y + 1) * s * 4]) for y in range(s))

    def chunk(tag: bytes, data: bytes) -> bytes:
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    ihdr = struct.pack(">IIBBBBB", s, s, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def main():
    out = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
    out.mkdir(parents=True, exist_ok=True)
    (out / "icon.png").write_bytes(make_icon(128))
    (out / "tray.png").write_bytes(make_icon(32))
    print(f"wrote {out/'icon.png'} (128x128) and {out/'tray.png'} (32x32)")


if __name__ == "__main__":
    main()
