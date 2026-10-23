"""Rasterizes a pixel TTF into a BDF.

Usage: python tools/ttf_to_bdf.py <font.ttf> <out.bdf> [ascent_px] [latin] [size=N] [xshift=N]

`size` is the pixel size of the em (default 12, which suits fonts with 100 units per pixel
and 1200 units per em). `xshift` moves the glyphs right (default 0).

By default the BDF holds CJK ideographs in a 12x12 cell. With `latin` it holds the digits,
letters and +-!?& in a 16x16 cell laid out like the 12px glyphs (2 px right, 3 px down), so
descenders that fall below the 12px cell are kept; pack it with `glyphpack-gen ... 16 latin`.
"""

import sys

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont


def is_cjk(cp):
    return 0x3400 <= cp <= 0x4DBF or 0x4E00 <= cp <= 0x9FFF or 0xF900 <= cp <= 0xFAFF


def is_latin(cp):
    return cp < 0x80 and (chr(cp).isalnum() or chr(cp) in "+-!?&")


def option(args, name, default):
    for a in args:
        if a.startswith(name + "="):
            return int(a.split("=", 1)[1])
    return default


def main():
    path, out_path = sys.argv[1], sys.argv[2]
    args = sys.argv[3:]
    latin = "latin" in args
    numbers = [a for a in args if a.isdigit()]
    ascent = int(numbers[0]) if numbers else 11
    size = option(args, "size", 12)
    xshift = option(args, "xshift", 0)
    cell, ascent_row = (16, 13) if latin else (12, 9)
    origin = (2, 3 + ascent) if latin else (xshift, ascent)
    wanted = is_latin if latin else is_cjk
    cmap = TTFont(path).getBestCmap()
    font = ImageFont.truetype(path, size)
    lines = []
    count = 0
    for cp in sorted(cmap):
        if not wanted(cp):
            continue
        img = Image.new("L", (cell, cell), 0)
        ImageDraw.Draw(img).text(origin, chr(cp), font=font, fill=255, anchor="ls")
        px = img.load()
        rows = []
        for y in range(cell):
            bits = 0
            for x in range(cell):
                bits = (bits << 1) | (1 if px[x, y] >= 128 else 0)
            rows.append(bits << (16 - cell))
        if not any(rows):
            continue
        lines.append(f"STARTCHAR u{cp:04X}\nENCODING {cp}\nBBX {cell} {cell} 0 {ascent_row + 1 - cell}\nBITMAP")
        lines.extend(f"{r:04X}" for r in rows)
        lines.append("ENDCHAR")
        count += 1
    with open(out_path, "w", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    print(count, "glyphs")


main()
