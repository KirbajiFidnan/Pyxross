"""Generate the default Pyxross dark-theme 9-slice atlas PNG (no Pillow).

Atlas layout: rows = components, columns = 4 states (normal/hover/pressed/disabled).
Each tile is 32x32 px with corner_size = 4. A 2px bevel ring (lighter top/left,
darker bottom/right) suggests the 9-slice edge; the center is a flat fill.

Rows:
  0: panel_bg
  1: panel_header
  2: button
  3: scrollbar_track
  4: scrollbar_handle
  5: tab
"""
import struct
import zlib

TILE = 32
COLS = 4  # normal, hover, pressed, disabled

# (r, g, b) per component per state
ROWS = [
    # panel_bg
    [(0x2A, 0x2A, 0x2A), (0x32, 0x32, 0x32), (0x22, 0x22, 0x22), (0x2A, 0x2A, 0x2A)],
    # panel_header
    [(0x3A, 0x3A, 0x3A), (0x40, 0x40, 0x40), (0x30, 0x30, 0x30), (0x3A, 0x3A, 0x3A)],
    # button
    [(0x3D, 0x3D, 0x3D), (0x4A, 0x4A, 0x4A), (0x2F, 0x2F, 0x2F), (0x33, 0x33, 0x33)],
    # scrollbar_track (no hover/pressed differentiation)
    [(0x20, 0x20, 0x20), (0x20, 0x20, 0x20), (0x20, 0x20, 0x20), (0x20, 0x20, 0x20)],
    # scrollbar_handle
    [(0x40, 0x40, 0x40), (0x4D, 0x4D, 0x4D), (0x2F, 0x2F, 0x2F), (0x35, 0x35, 0x35)],
    # tab
    [(0x2E, 0x2E, 0x2E), (0x3A, 0x3A, 0x3A), (0x26, 0x26, 0x26), (0x30, 0x30, 0x30)],
]

W = TILE * COLS
H = TILE * len(ROWS)


def tile(row, col, x, y):
    """Fill a 32x32 tile at pixel offset (x, y) for the given row/col."""
    base = ROWS[row][col]
    for dy in range(TILE):
        for dx in range(TILE):
            # 2px bevel ring
            if dx < 2 or dy < 2:
                # top/left edge: lighter
                c = tuple(min(255, v + 18) for v in base)
            elif dx >= TILE - 2 or dy >= TILE - 2:
                # bottom/right edge: darker
                c = tuple(max(0, v - 18) for v in base)
            else:
                c = base
            if row == 3:  # scrollbar_track: subtle inner groove
                if 4 <= dx < 28 and 4 <= dy < 28:
                    c = tuple(max(0, v - 8) for v in base)
            elif row == 5 and col == 0:  # tab normal: brighter top edge
                if dy == 0 and 2 <= dx < 30:
                    c = tuple(min(255, v + 26) for v in base)
            yield (x + dx, y + dy, c)


def main():
    pixels = [[(0, 0, 0, 0)] * W for _ in range(H)]
    for r, row in enumerate(ROWS):
        for c in range(COLS):
            for (px, py, color) in tile(r, c, c * TILE, r * TILE):
                pixels[py][px] = (*color, 255)

    # Build PNG.
    raw = b""
    for y in range(H):
        raw += b"\x00"  # filter: None
        for x in range(W):
            raw += bytes(pixels[y][x])

    def chunk(typ, data):
        return (struct.pack(">I", len(data)) + typ + data
                + struct.pack(">I", zlib.crc32(typ + data) & 0xFFFFFFFF))

    ihdr = struct.pack(">IIBBBBB", W, H, 8, 6, 0, 0, 0)  # 8-bit RGBA
    png = (b"\x89PNG\r\n\x1a\n"
           + chunk(b"IHDR", ihdr)
           + chunk(b"IDAT", zlib.compress(raw, 9))
           + chunk(b"IEND", b""))
    with open("/app/pyx/assets/themes/default/atlas.png", "wb") as f:
        f.write(png)
    print(f"wrote atlas.png ({W}x{H})")


if __name__ == "__main__":
    main()