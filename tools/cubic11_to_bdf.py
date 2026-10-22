"""Rasterizes Cubic 11 into a BDF.

Usage: python tools/cubic11_to_bdf.py <Cubic_11.ttf> <out.bdf> [latin]

By default the BDF holds CJK ideographs in a 12x12 cell. With `latin` it holds the digits,
letters and +-!?& in a 16x16 cell laid out like the 12px glyphs (2 px right, 3 px down), so
descenders that fall below the 11px cell are kept; pack it with `glyphpack-gen ... 16 latin`.
"""

import sys
from fontTools.ttLib import TTFont
from fontTools.pens.pointInsidePen import PointInsidePen

latin = "latin" in sys.argv[3:]
f = TTFont(sys.argv[1])
cm = f.getBestCmap()
gs = f.getGlyphSet()
PX = 100


def wanted(cp):
    if latin:
        return cp < 0x80 and (chr(cp).isalnum() or chr(cp) in "+-!?&")
    return 0x3400 <= cp <= 0x4DBF or 0x4E00 <= cp <= 0x9FFF or 0xF900 <= cp <= 0xFAFF


ROWS, COLS, X0 = (13, 12, 0) if latin else (11, 11, PX)
out = []
n = 0
for cp in sorted(cm):
    if not wanted(cp):
        continue
    name = cm[cp]
    g = f["glyf"][name]
    if g.numberOfContours == 0:
        continue
    gl = gs[name]
    rows = []
    for row in range(ROWS):
        y = 900 - row * PX - PX // 2
        bits = 0
        for col in range(COLS):
            x = X0 + col * PX + PX // 2
            pen = PointInsidePen(gs, (x, y))
            gl.draw(pen)
            bits = (bits << 1) | (1 if pen.getResult() else 0)
        rows.append(bits << (16 - COLS))
    if not any(rows):
        continue
    bbx = "12 13 2 -2" if latin else "11 11 0 -1"
    out.append(f"STARTCHAR u{cp:04X}\nENCODING {cp}\nBBX {bbx}\nBITMAP")
    out.extend(f"{r:04X}" for r in rows)
    out.append("ENDCHAR")
    n += 1
open(sys.argv[2], "w", newline="\n").write("\n".join(out) + "\n")
print(n, "glyphs")
