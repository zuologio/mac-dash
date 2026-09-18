#!/usr/bin/env python3
"""Build a Linux console font that can actually draw the dashboard.

Every console font shipped with Raspberry Pi OS is missing the partial-block
characters the sparklines and meters are drawn from. This is not a matter of
picking a better one: the unicode tables of all ~700 fonts under
/usr/share/consolefonts were decoded and not one is complete. U+2581-2587
(sparkline heights), U+2589-258F (meter eighths) and U+25B0/25B1 (process
bars) are absent from every last one, and the console is capped at 512 glyphs
per font, so "install a fuller Unicode font" cannot work either.

The console substitutes a fallback glyph for an unmapped codepoint, which is
why the panel shows a row of `#` where the bars should be. Nothing is wrong
with what the program writes; the target simply cannot draw it.

So draw them. These glyphs are pure geometry — fill N/8 of the cell — which is
the same reason they are the right characters for a bar chart in the first
place. They are synthesized at whatever cell size the source font uses and
spliced in, replacing glyphs the dashboard will never emit.

Usage:
    make-console-font.py <source.psf[.gz]> <out.psf>

Handles PSF1 and PSF2. Any 512-glyph console font works as a base;
Terminus24x12 is a good choice for a 1080p panel.
"""

import gzip
import struct
import sys

PSF1_MAGIC = b"\x36\x04"
PSF2_MAGIC = b"\x72\xb5\x4a\x86"

# What the dashboard needs and no font has.
NEEDED = list(range(0x2580, 0x2591)) + [0x25B0, 0x25B1]


class Font:
    def __init__(self, psf2, mode, charsize, width, height, glyphs, mapping):
        self.psf2 = psf2
        self.mode = mode
        self.charsize = charsize
        self.width = width
        self.height = height
        self.glyphs = glyphs
        self.mapping = mapping

    @property
    def stride(self):
        """Bytes per glyph row — a 12-pixel-wide cell needs two."""
        return (self.width + 7) // 8


def draw(cp, width, height):
    """Bitmap rows for one codepoint, as ints MSB-first, or None."""
    rows = [0] * height
    full = (1 << width) - 1

    # U+2581..U+2588 — lower k/8 blocks: the sparkline's vertical resolution.
    if 0x2581 <= cp <= 0x2588:
        eighths = cp - 0x2580
        filled = round(height * eighths / 8)
        for y in range(height - filled, height):
            rows[y] = full
        return rows

    # U+2589..U+258F — left k/8 blocks: the meter's sub-character resolution.
    # Note the reversed order — U+258F is thinnest, U+2589 widest.
    if 0x2589 <= cp <= 0x258F:
        eighths = 0x2590 - cp
        cols = round(width * eighths / 8)
        mask = (full << (width - cols)) & full if cols else 0
        for y in range(height):
            rows[y] = mask
        return rows

    if cp == 0x2590:  # right half block
        cols = width // 2
        for y in range(height):
            rows[y] = (1 << cols) - 1
        return rows

    if cp == 0x2580:  # upper half block
        for y in range(height // 2):
            rows[y] = full
        return rows

    # U+25B0 / U+25B1 — the process-list bars, filled and hollow. A slant only
    # reads as a parallelogram in a cell wider than 8px; below that it is an
    # unreadable smear, so a narrow cell gets a plain block instead.
    if cp in (0x25B0, 0x25B1):
        top, bottom = height // 4, height - height // 4
        inset = 1 if width < 10 else 2
        slant = width >= 10
        for y in range(top, bottom):
            if slant:
                # Shift the body one pixel right as it descends.
                shift = (bottom - 1 - y) * 2 // (bottom - top) if bottom > top else 0
                body = ((full >> (inset * 2)) << inset) >> shift
            else:
                body = (full >> (inset * 2)) << inset
            if cp == 0x25B0 or y in (top, bottom - 1):
                rows[y] = body & full
            else:
                # Hollow: keep only the outermost set bit on each side.
                if body:
                    hi = body.bit_length() - 1
                    lo = (body & -body).bit_length() - 1
                    rows[y] = ((1 << hi) | (1 << lo)) & full
        return rows

    return None


def parse_table(blob, count):
    """Decode a PSF unicode table into {codepoint: glyph index}."""
    mapping, index, i, pending = {}, 0, 0, []
    if not blob:
        return mapping
    # PSF2 tables are UTF-8 with 0xFF terminators; PSF1 tables are LE u16.
    psf2 = b"\xff" in blob and blob.count(b"\xff\xff") < blob.count(b"\xff") / 2
    if psf2:
        for chunk in blob.split(b"\xff"):
            if index >= count:
                break
            for part in chunk.split(b"\xfe"):
                try:
                    for ch in part.decode("utf-8"):
                        mapping.setdefault(ord(ch), index)
                except UnicodeDecodeError:
                    pass
            index += 1
        return mapping
    while i + 1 < len(blob) and index < count:
        value = struct.unpack("<H", blob[i:i + 2])[0]
        if value == 0xFFFF:
            for cp in pending:
                mapping.setdefault(cp, index)
            pending = []
            index += 1
        elif value != 0xFFFE:
            pending.append(value)
        i += 2
    return mapping


def read(path):
    data = gzip.open(path, "rb").read() if path.endswith(".gz") else open(path, "rb").read()

    if data[:4] == PSF2_MAGIC:
        _, hdrsize, flags, count, charsize, height, width = struct.unpack("<7I", data[4:32])
        body = data[hdrsize:hdrsize + count * charsize]
        glyphs = [bytearray(body[i * charsize:(i + 1) * charsize]) for i in range(count)]
        table = data[hdrsize + count * charsize:] if flags & 1 else b""
        return Font(True, flags, charsize, width, height, glyphs, parse_table(table, count))

    if data[:2] == PSF1_MAGIC:
        mode, charsize = data[2], data[3]
        count = 512 if mode & 1 else 256
        glyphs = [bytearray(data[4 + i * charsize:4 + (i + 1) * charsize]) for i in range(count)]
        table = data[4 + count * charsize:] if mode & 0x06 else b""
        return Font(False, mode, charsize, 8, charsize, glyphs, parse_table(table, count))

    raise SystemExit(f"{path}: not a PSF font")


def by_glyph(mapping):
    out = {}
    for cp, index in mapping.items():
        out.setdefault(index, []).append(cp)
    return out


def reclaimable(cp):
    """Whether a codepoint's slot can be repurposed.

    Every slot in these fonts is already occupied, so room has to be made. The
    dashboard renders ASCII, box-drawing and a few geometric symbols; the Latin
    accents and non-Latin scripts filling most of a Uni2 font are dead weight
    on a panel that will only ever show English hostnames and numbers.
    """
    if cp < 0x0100:  # ASCII and Latin-1
        return False
    if 0x2500 <= cp <= 0x25FF:  # box drawing and geometric shapes
        return False
    if cp in (0x2022, 0x00B7):  # bullets used as separators
        return False
    return True


def write(font, path):
    blob = bytearray()
    count = len(font.glyphs)
    if font.psf2:
        blob += PSF2_MAGIC
        blob += struct.pack(
            "<7I", 0, 32, font.mode | 1, count, font.charsize, font.height, font.width
        )
    else:
        blob += PSF1_MAGIC
        blob.append(font.mode | 0x02)
        blob.append(font.charsize)
    for glyph in font.glyphs:
        blob += bytes(glyph).ljust(font.charsize, b"\x00")

    reverse = by_glyph(font.mapping)
    for index in range(count):
        for cp in sorted(reverse.get(index, [])):
            blob += chr(cp).encode("utf-8") if font.psf2 else struct.pack("<H", cp)
        blob += b"\xff" if font.psf2 else b"\xff\xff"

    with open(path, "wb") as handle:
        handle.write(blob)


def build(src, out):
    font = read(src)

    reverse = by_glyph(font.mapping)
    free = [
        index
        for index in sorted(reverse, reverse=True)
        if reverse[index] and all(reclaimable(cp) for cp in reverse[index])
    ]
    freed = set(free)
    font.mapping = {cp: i for cp, i in font.mapping.items() if i not in freed}

    added = []
    for cp in NEEDED:
        bits = draw(cp, font.width, font.height)
        if bits is None:
            continue
        if cp in font.mapping:
            index = font.mapping[cp]
        elif free:
            index = free.pop(0)
            font.mapping[cp] = index
        else:
            print(f"  no slot left for U+{cp:04X}", file=sys.stderr)
            continue
        glyph = bytearray(font.charsize)
        rows_avail = font.charsize // max(1, font.stride)
        for y in range(min(font.height, rows_avail)):
            # `bits` is width-wide and MSB-first; a row occupies stride bytes,
            # so it must be left-aligned into stride*8 bits before slicing.
            # Packing it right-aligned shifts every glyph toward the cell's
            # right edge and drops the leftmost columns.
            row = bits[y] << (font.stride * 8 - font.width)
            for b in range(font.stride):
                shift = (font.stride - 1 - b) * 8
                glyph[y * font.stride + b] = (row >> shift) & 0xFF
        font.glyphs[index] = glyph
        added.append(cp)

    write(font, out)
    kind = "PSF2" if font.psf2 else "PSF1"
    print(
        f"  {out}: {kind} {font.width}x{font.height}, {len(font.glyphs)} glyphs, "
        f"{len(font.mapping)} codepoints, {len(added)} drawn"
    )
    return added


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit(__doc__)
    build(sys.argv[1], sys.argv[2])
