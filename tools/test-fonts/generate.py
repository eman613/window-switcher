"""Generate original, deliberately simple MIT-licensed test outlines.

Requires fontTools 4.61.1. These fonts test collection isolation and outline
backends; they are fixtures, not application UI assets or third-party fonts.
"""
from pathlib import Path

from fontTools.fontBuilder import FontBuilder
from fontTools.pens.t2CharStringPen import T2CharStringPen
from fontTools.pens.ttGlyphPen import TTGlyphPen


def generate(output: Path, *, cff: bool, advance: int) -> None:
    chars = " ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+"
    mapping = {ord(char): f"uni{ord(char):04X}" for char in chars}
    order = [".notdef", *mapping.values()]
    builder = FontBuilder(1000, isTTF=not cff)
    builder.setupGlyphOrder(order)
    builder.setupCharacterMap(mapping)
    glyphs = {}
    for name in order:
        pen = T2CharStringPen(advance, None) if cff else TTGlyphPen(None)
        if name != "uni0020":
            pen.moveTo((50, 0))
            pen.lineTo((advance - 50, 0))
            pen.lineTo((advance - 50, 700))
            pen.lineTo((50, 700))
            pen.closePath()
        glyphs[name] = pen.getCharString() if cff else pen.glyph()
    if cff:
        builder.setupCFF("WSRoleFixture-CFF", {"FullName": "WS Role Fixture"}, glyphs, {})
    else:
        builder.setupGlyf(glyphs)
    builder.setupHorizontalMetrics({name: (advance, 50) for name in order})
    builder.setupHorizontalHeader(ascent=800, descent=-200)
    builder.setupNameTable({
        "familyName": "WS Role Fixture",
        "styleName": "Regular",
        "uniqueFontIdentifier": "WSRoleFixture-CFF" if cff else "WSRoleFixture-TTF",
        "fullName": "WS Role Fixture",
        "psName": "WSRoleFixture-CFF" if cff else "WSRoleFixture-TTF",
        "version": "Version 1.000",
        "copyright": "Original test outlines. Licensed under the repository MIT license.",
    })
    builder.setupOS2(sTypoAscender=800, sTypoDescender=-200, usWinAscent=800, usWinDescent=200)
    builder.setupPost()
    builder.font["head"].created = builder.font["head"].modified = 2082844800
    builder.font.recalcTimestamp = False
    output.parent.mkdir(parents=True, exist_ok=True)
    builder.save(output)


if __name__ == "__main__":
    directory = Path(__file__).resolve().parents[2] / "src" / "font_resources" / "fixtures"
    generate(directory / "narrow.ttf", cff=False, advance=500)
    generate(directory / "wide.otf", cff=True, advance=900)
