"""Rasterizes a 12px pixel TTF (100 units per pixel, 1200 units per em) into a 12x12 BDF.

Usage: python tools/ttf_to_bdf.py <font.ttf> <out.bdf> [ascent_px]
"""

import sys

from fontTools.ttLib import TTFont
from PIL import Image, ImageDraw, ImageFont

CELL = 12
ASCENT_ROW = 9


def wanted(cp):
    return 0x3400 <= cp <= 0x4DBF or 0x4E00 <= cp <= 0x9FFF or 0xF900 <= cp <= 0xFAFF


def main():
    path, out_path = sys.argv[1], sys.argv[2]
    ascent = int(sys.argv[3]) if len(sys.argv) > 3 else 11
    cmap = TTFont(path).getBestCmap()
    font = ImageFont.truetype(path, CELL)
    lines = []
    count = 0
    for cp in sorted(cmap):
        if not wanted(cp):
            continue
        img = Image.new("L", (CELL, CELL), 0)
        ImageDraw.Draw(img).text((0, ascent), chr(cp), font=font, fill=255, anchor="ls")
        px = img.load()
        rows = []
        for y in range(CELL):
            bits = 0
            for x in range(CELL):
                bits = (bits << 1) | (1 if px[x, y] >= 128 else 0)
            rows.append(bits << 4)
        if not any(rows):
            continue
        lines.append(f"STARTCHAR u{cp:04X}\nENCODING {cp}\nBBX {CELL} {CELL} 0 {ASCENT_ROW + 1 - CELL}\nBITMAP")
        lines.extend(f"{r:04X}" for r in rows)
        lines.append("ENDCHAR")
        count += 1
    with open(out_path, "w", newline="\n") as f:
        f.write("\n".join(lines) + "\n")
    print(count, "glyphs")


main()
