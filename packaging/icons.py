#!/usr/bin/env python3
"""Write placeholder app icons for packaging into packaging/icons/.

AppImage needs a square PNG, and the Windows installers want an .ico.
cargo-packager builds the macOS .icns from the PNGs. A real app replaces
these with its own artwork; the example only needs something valid.

Standard library only, so it runs on every CI runner as is.
"""

import struct
import sys
import zlib
from pathlib import Path

SIZES = (32, 128, 256, 512)


def pixel(x, y, size):
    """A filled disc with a ring cut out, on a transparent background."""
    c = (size - 1) / 2
    r = ((x - c) ** 2 + (y - c) ** 2) ** 0.5 / (size / 2)
    if r > 0.94:
        return (0, 0, 0, 0)
    if 0.42 < r < 0.58:
        return (240, 244, 255, 255)
    return (64, 92, 220, 255)


def png(size):
    rows = b"".join(
        b"\x00" + b"".join(bytes(pixel(x, y, size)) for x in range(size))
        for y in range(size)
    )

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(rows, 9))
        + chunk(b"IEND", b"")
    )


def ico(images):
    """An .ico holding PNG images, which Windows Vista and later read."""
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for size, data in images:
        dim = 0 if size >= 256 else size
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    return out + b"".join(data for _, data in images)


def main():
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else Path(__file__).parent / "icons"
    out.mkdir(parents=True, exist_ok=True)
    images = {size: png(size) for size in SIZES}
    for size, data in images.items():
        (out / f"{size}x{size}.png").write_bytes(data)
    (out / "quark.ico").write_bytes(ico([(s, images[s]) for s in (32, 256)]))
    print(f"wrote icons to {out}")


if __name__ == "__main__":
    main()
