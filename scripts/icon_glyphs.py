#!/usr/bin/env python3
"""Adds Veta's own icons, drawn in Lucide's style, to the icon subset.

Run by scripts/icons.sh after subsetting. Each icon is a list of strokes on
Lucide's 24x24 grid (stroke width 2, round caps) and gets a private-use
codepoint outside Lucide's range. Needs fonttools.
    scripts/icon_glyphs.py crates/veta-gui/assets/lucide-subset.ttf
"""

import math
import sys

from fontTools.pens.areaPen import AreaPen
from fontTools.pens.cu2quPen import Cu2QuPen
from fontTools.pens.recordingPen import RecordingPen
from fontTools.pens.reverseContourPen import ReverseContourPen
from fontTools.pens.ttGlyphPen import TTGlyphPen
from fontTools.ttLib import TTFont

# name: (codepoint, strokes as (x1, y1, x2, y2) on the 24x24 grid)
ICONS = {
    "veta-math-symbols": (
        0xF000,
        [
            (4.5, 7.5, 10.5, 7.5),  # +
            (7.5, 4.5, 7.5, 10.5),
            (13.5, 7.5, 19.5, 7.5),  # -
            (5.5, 14.5, 9.5, 18.5),  # x
            (9.5, 14.5, 5.5, 18.5),
            (13.5, 15, 19.5, 15),  # =
            (13.5, 18, 19.5, 18),
        ],
    ),
}

EM = 1000
RADIUS = 1.0
KAPPA = 0.5523


def point(x, y):
    """Grid coordinates (y down) to font units (y up)."""
    return (round(x * EM / 24, 2), round((24 - y) * EM / 24, 2))


def capsule(pen, x1, y1, x2, y2, r=RADIUS):
    """A stroke with round caps as one closed contour."""
    length = math.hypot(x2 - x1, y2 - y1)
    dx, dy = (x2 - x1) / length * r, (y2 - y1) / length * r
    nx, ny = -dy, dx
    k = KAPPA

    def arc(cx, cy, ax, ay, bx, by):
        # Quarter circle around (cx, cy) from offset a to offset b.
        pen.curveTo(
            point(cx + ax + bx * k, cy + ay + by * k),
            point(cx + bx + ax * k, cy + by + ay * k),
            point(cx + bx, cy + by),
        )

    pen.moveTo(point(x1 + nx, y1 + ny))
    pen.lineTo(point(x2 + nx, y2 + ny))
    arc(x2, y2, nx, ny, dx, dy)
    arc(x2, y2, dx, dy, -nx, -ny)
    pen.lineTo(point(x1 - nx, y1 - ny))
    arc(x1, y1, -nx, -ny, -dx, -dy)
    arc(x1, y1, -dx, -dy, nx, ny)
    pen.closePath()


def glyph(strokes, glyph_set):
    tt = TTGlyphPen(glyph_set)
    pen = Cu2QuPen(tt, max_err=0.5, reverse_direction=False)
    for stroke in strokes:
        rec = RecordingPen()
        capsule(rec, *stroke)
        area = AreaPen()
        rec.replay(area)
        # TrueType fills clockwise contours; keep every stroke clockwise so
        # overlapping strokes don't cancel out.
        rec.replay(ReverseContourPen(pen) if area.value > 0 else pen)
    return tt.glyph()


def main(path):
    font = TTFont(path)
    order = font.getGlyphOrder()
    for name, (code, strokes) in ICONS.items():
        for table in font["cmap"].tables:
            if table.isUnicode():
                assert code not in table.cmap or table.cmap[code] == name, hex(code)
        if name not in order:
            order = order + [name]
            font.setGlyphOrder(order)
        g = glyph(strokes, font.getGlyphSet())
        g.recalcBounds(font["glyf"])
        font["glyf"][name] = g
        font["hmtx"][name] = (EM, g.xMin)
        for table in font["cmap"].tables:
            if table.isUnicode():
                table.cmap[code] = name
    font.save(path)


if __name__ == "__main__":
    main(sys.argv[1])
