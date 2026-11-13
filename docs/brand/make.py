"""Stacked's README banner and social preview, with every word as outlines.

    python3 make.py REPO FONT_DIR   (FONT_DIR holds InterVariable.ttf and JetBrainsMono.ttf)
"""
import re
import sys
from pathlib import Path

from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.transformPen import TransformPen
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

REPO = Path(sys.argv[1])
FONTS = Path(sys.argv[2])
OUT = REPO / "docs/brand"
OUT.mkdir(parents=True, exist_ok=True)

BG, RAISED, LINE = "#15100a", "#2a1f0c", "#3d2f16"
TEXT, TEXT2, TEXT3, ACCENT = "#f5ecdb", "#c9b896", "#9a8865", "#e9bd6a"

_fonts = {}


def font(path, wght):
    key = (path, wght)
    if key not in _fonts:
        f = TTFont(path)
        if "fvar" in f:
            f = instantiateVariableFont(f, {"wght": wght})
        _fonts[key] = f
    return _fonts[key]


def text(s, x, y, size, fill, wght=400, path=FONTS / "InterVariable.ttf", tracking=0.0, anchor="start"):
    """`s` as one <path>, its baseline at y."""
    f = font(path, wght)
    scale = size / f["head"].unitsPerEm
    cmap, glyphs, hmtx = f.getBestCmap(), f.getGlyphSet(), f["hmtx"]
    names = [cmap[ord(c)] for c in s]
    width = sum(hmtx[n][0] for n in names) * scale + tracking * size * (len(s) - 1)
    if anchor == "middle":
        x -= width / 2
    pen = SVGPathPen(glyphs)
    cx = x
    for n in names:
        glyphs[n].draw(TransformPen(pen, (scale, 0, 0, -scale, cx, y)))
        cx += hmtx[n][0] * scale + tracking * size
    return f'<path fill="{fill}" d="{pen.getCommands()}"/>', width


def icon(x, y, size):
    src = (REPO / "packaging/icons/hicolor/scalable/apps/io.github.damnshabu.Stacked.svg").read_text()
    inner = re.search(r"<svg[^>]*>(.*)</svg>", src, re.S).group(1)
    inner = re.sub(r"<title>.*?</title>|<!--.*?-->", "", inner, flags=re.S)
    return f'<g transform="translate({x} {y}) scale({size / 128})">{inner}</g>'


def pill(label, x, y, h=44, size=19):
    p, w = text(label, 0, 0, size, TEXT2, 500)
    pad = 20
    return (
        f'<g transform="translate({x} {y})">'
        f'<rect width="{w + 2 * pad:.1f}" height="{h}" rx="{h / 2}" fill="{RAISED}" stroke="{LINE}"/>'
        f'<g transform="translate({pad} {h / 2 + size * 0.36:.1f})">{p}</g></g>'
    ), w + 2 * pad


def pills(labels, x, y, gap=12, anchor="start"):
    parts, widths = zip(*(pill(l, 0, 0) for l in labels))
    total = sum(widths) + gap * (len(labels) - 1)
    cx = x - total / 2 if anchor == "middle" else x
    out = []
    for part, w in zip(parts, widths):
        out.append(f'<g transform="translate({cx:.1f} {y})">{part}</g>')
        cx += w + gap
    return "".join(out)


def svg(w, h, body, rx=0):
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">'
        f'<rect width="{w}" height="{h}" rx="{rx}" fill="{BG}"/>{body}</svg>\n'
    )


TAGLINE = "Roblox's Android build, running natively on Linux."
FEATURES = ["No emulator", "No Wine, no VM", "x86_64 and aarch64"]

# README banner: icon left, name and line beside it.
word, _ = text("Stacked", 268, 168, 112, TEXT, 700, tracking=-0.025)
tag, _ = text(TAGLINE, 272, 222, 30, TEXT2, 400)
banner = svg(
    1280, 340,
    icon(64, 70, 172) + word + tag + pills(FEATURES, 272, 256),
    rx=28,
)
(OUT / "banner.svg").write_text(banner)

# Social preview, 1280x640: centred.
word, _ = text("Stacked", 640, 382, 136, TEXT, 700, tracking=-0.025, anchor="middle")
tag, _ = text(TAGLINE, 640, 444, 34, TEXT2, 400, anchor="middle")
url, _ = text("github.com/DamnShabu/stacked", 640, 590, 22, TEXT3, 500,
              path=FONTS / "JetBrainsMono.ttf", anchor="middle")
bar = f'<rect x="0" y="0" width="1280" height="8" fill="{ACCENT}"/>'
social = svg(1280, 640, bar + icon(560, 92, 160) + word + tag + pills(FEATURES, 640, 488, anchor="middle") + url)
(OUT / "social-preview.svg").write_text(social)
print("wrote", OUT)
