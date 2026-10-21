import sys
from fontTools.ttLib import TTFont
from fontTools.pens.pointInsidePen import PointInsidePen

f = TTFont(sys.argv[1])
cm = f.getBestCmap()
gs = f.getGlyphSet()
PX = 100


def wanted(cp):
    return 0x3400 <= cp <= 0x4DBF or 0x4E00 <= cp <= 0x9FFF or 0xF900 <= cp <= 0xFAFF


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
    for row in range(11):
        y = 900 - row * PX - PX // 2
        bits = 0
        for col in range(11):
            x = PX + col * PX + PX // 2
            pen = PointInsidePen(gs, (x, y))
            gl.draw(pen)
            bits = (bits << 1) | (1 if pen.getResult() else 0)
        rows.append(bits << 5)
    if not any(rows):
        continue
    out.append(f"STARTCHAR u{cp:04X}\nENCODING {cp}\nBBX 11 11 0 -1\nBITMAP")
    out.extend(f"{r:04X}" for r in rows)
    out.append("ENDCHAR")
    n += 1
open(sys.argv[2], "w", newline="\n").write("\n".join(out) + "\n")
print(n, "glyphs")
