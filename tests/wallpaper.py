#!/usr/bin/env python3
"""The test desktops' wallpaper: a rose, violet and blue blend, as PNG.

    python3 tests/wallpaper.py <out.png> <width> <height>

Standard library only, so it runs on every CI runner.
"""
import struct
import sys
import zlib

out, w, h = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
# Corner colours, blended across the screen (rose, deep blue, violet,
# sky blue): enough colour for glass and shadows to show.
tl, tr, bl, br = (196, 132, 150), (52, 66, 140), (98, 64, 150), (116, 160, 222)
raw = bytearray()
for y in range(h):
    v = y / max(1, h - 1)
    left = [a + (b - a) * v for a, b in zip(tl, bl)]
    right = [a + (b - a) * v for a, b in zip(tr, br)]
    raw += b"\0"
    for x in range(w):
        u = x / max(1, w - 1)
        raw += bytes(round(a + (b - a) * u) for a, b in zip(left, right))

def chunk(kind, data):
    body = kind + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))


with open(out, "wb") as f:
    f.write(b"\x89PNG\r\n\x1a\n")
    f.write(chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)))
    f.write(chunk(b"IDAT", zlib.compress(bytes(raw), 9)))
    f.write(chunk(b"IEND", b""))
