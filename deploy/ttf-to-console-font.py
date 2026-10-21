#!/usr/bin/env python3
"""Rasterize a TTF/OTF into a Linux console font.

The console draws from PSF bitmaps, not outlines, so a downloaded .ttf cannot
be used directly however good it looks in a terminal emulator. This renders one
at a fixed pixel size into a PSF2, which `setfont` can load.

Two things make the result usable rather than a blurry mess:

  * Rendering targets monochrome, not antialiased grey. A framebuffer console
    has no alpha channel, so a grey edge pixel becomes an arbitrary on/off
    decision; asking FreeType to make that decision with hinting produces far
    cleaner stems than thresholding a grey bitmap afterwards.

  * The block and bar characters are *not* taken from the font. Even fonts
    that include them rarely tile seamlessly at an arbitrary cell size -- a
    half-block that lands a pixel short leaves a seam across a filled bar. They
    are synthesized to the exact cell instead, by the same code that patches
    stock console fonts (make-console-font.py), so a meter is solid.

Usage:
    ttf-to-console-font.py <font.ttf> <size-px> <out.psf> [--cell WxH]

The cell defaults to the font's own advance and line height at that size.
"""

import struct
import sys
import runpy
from pathlib import Path

geometry = runpy.run_path(str(Path(__file__).with_name("make-console-font.py")))

try:
    import freetype
except ImportError:
    raise SystemExit(
        "freetype-py is required:  python3 -m pip install freetype-py"
    )

# Everything the dashboard can emit, within the console's 512-glyph ceiling.
DASHBOARD_GLYPHS = (
    list(range(0x20, 0x7F))  # printable ASCII
    + [0x00B7, 0x00B0]  # middle dot, degree
    + list(range(0x2500, 0x2504))  # box: horizontal, vertical
    + [0x250C, 0x2510, 0x2514, 0x2518, 0x251C, 0x2524, 0x252C, 0x2534, 0x253C]
    + list(range(0x2580, 0x2591))  # blocks, all eighths
    + [0x2591, 0x2592, 0x2593]  # shades
    + [0x25B0, 0x25B1, 0x25B2, 0x25BC, 0x25CB, 0x25CC, 0x25CD, 0x25CF, 0x25D0]
    + [0x2022, 0x2026]  # bullet, ellipsis
    # Rounded corners, last: they are drawn to meet the ─ and │ above.
    + [0x256D, 0x256E, 0x256F, 0x2570]
)

# Synthesized rather than rasterized — see the module docstring.
SYNTHETIC = set(geometry["NEEDED"])


def synth(cp, width, height, lines=None):
    """Geometric glyphs, drawn to tile the cell exactly."""
    rows = [0] * height
    full = (1 << width) - 1

    if 0x2581 <= cp <= 0x2588:  # lower k/8 blocks
        filled = round(height * (cp - 0x2580) / 8)
        for y in range(height - filled, height):
            rows[y] = full
        return rows

    if 0x2589 <= cp <= 0x258F:  # left k/8 blocks, U+258F thinnest
        cols = round(width * (0x2590 - cp) / 8)
        mask = (full << (width - cols)) & full if cols else 0
        for y in range(height):
            rows[y] = mask
        return rows

    if cp == 0x2580:  # upper half
        for y in range(height // 2):
            rows[y] = full
        return rows

    if cp == 0x2590:  # right half
        cols = width // 2
        for y in range(height):
            rows[y] = (1 << cols) - 1
        return rows

    if cp in (0x25B0, 0x25B1):  # process bars, filled and hollow
        top, bottom = height // 4, height - height // 4
        inset = max(1, width // 8)
        body = (full >> (inset * 2)) << inset
        for y in range(top, bottom):
            if cp == 0x25B0 or y in (top, bottom - 1):
                rows[y] = body & full
            elif body:
                hi = body.bit_length() - 1
                lo = (body & -body).bit_length() - 1
                rows[y] = ((1 << hi) | (1 << lo)) & full
        return rows

    return geometry["draw"](cp, width, height, lines)


def rasterize(face, cp, width, height, baseline):
    """One glyph from the font, as bitmap rows, or None if absent."""
    if face.get_char_index(cp) == 0:
        return None
    face.load_char(cp, freetype.FT_LOAD_RENDER | freetype.FT_LOAD_TARGET_MONO)
    bitmap = face.glyph.bitmap
    rows = [0] * height
    left = face.glyph.bitmap_left
    top = baseline - face.glyph.bitmap_top
    for y in range(bitmap.rows):
        dst = top + y
        if not 0 <= dst < height:
            continue
        acc = 0
        for x in range(bitmap.width):
            byte = bitmap.buffer[y * bitmap.pitch + (x >> 3)]
            if byte & (0x80 >> (x & 7)):
                col = left + x
                if 0 <= col < width:
                    acc |= 1 << (width - 1 - col)
        rows[dst] = acc
    return rows


def build(path, size_px, out, cell=None):
    face = freetype.Face(path)
    face.set_pixel_sizes(0, size_px)

    if cell:
        width, height = cell
    else:
        # The font's own metrics: advance width and line height at this size.
        face.load_char("M", freetype.FT_LOAD_RENDER | freetype.FT_LOAD_TARGET_MONO)
        width = face.glyph.advance.x >> 6
        height = face.size.height >> 6

    # fbcon pads every glyph row to a whole number of bytes, so a cell that is
    # not a multiple of 8 wide is stored 16 bits wide and read back as though
    # it were 16 wide: the glyphs come out sheared and the console silently
    # keeps whatever font it had. Round up to the byte, which widens the cell
    # by a few pixels of blank column rather than corrupting every glyph.
    if width % 8:
        width += 8 - width % 8

    # The kernel refuses a glyph taller than 32 rows on most builds.
    if height > 32:
        raise SystemExit(
            f"cell would be {height} rows tall; fbcon accepts at most 32. "
            "Use a smaller --ttf-size."
        )
    baseline = face.size.ascender >> 6
    if baseline <= 0 or baseline >= height:
        baseline = height - max(1, height // 6)

    glyphs, mapping, missing = [], {}, []
    for cp in DASHBOARD_GLYPHS:
        if cp in mapping:
            continue
        lines = geometry["line_position"](
            tuple(glyphs[mapping[c]] if c in mapping else None for c in (0x2500, 0x2502)),
            width,
            height,
        )
        rows = synth(cp, width, height, lines) if cp in SYNTHETIC else None
        if rows is None:
            rows = rasterize(face, cp, width, height, baseline)
        if rows is None:
            missing.append(cp)
            continue
        mapping[cp] = len(glyphs)
        glyphs.append(rows)
        if len(glyphs) >= 512:
            break

    stride = (width + 7) // 8
    charsize = stride * height
    blob = bytearray(b"\x72\xb5\x4a\x86")
    blob += struct.pack("<7I", 0, 32, 1, len(glyphs), charsize, height, width)
    for rows in glyphs:
        for row in rows:
            aligned = row << (stride * 8 - width)
            for b in range(stride):
                blob.append((aligned >> ((stride - 1 - b) * 8)) & 0xFF)
    reverse = {}
    for cp, index in mapping.items():
        reverse.setdefault(index, []).append(cp)
    for index in range(len(glyphs)):
        for cp in sorted(reverse.get(index, [])):
            blob += chr(cp).encode("utf-8")
        blob += b"\xff"

    with open(out, "wb") as handle:
        handle.write(blob)
    print(
        f"  {out}: {width}x{height} cell, {len(glyphs)} glyphs"
        f"  (grid {1920 // width}x{1080 // height} at 1080p)"
    )
    if missing:
        print(f"  not in this font, skipped: {len(missing)} codepoints")
    return width, height


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(args) != 3:
        raise SystemExit(__doc__)
    cell = None
    for a in sys.argv[1:]:
        if a.startswith("--cell="):
            w, h = a.split("=", 1)[1].lower().split("x")
            cell = (int(w), int(h))
    build(args[0], int(args[1]), args[2], cell)
