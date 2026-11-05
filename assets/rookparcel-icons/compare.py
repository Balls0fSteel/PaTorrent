"""Side-by-side: real ICO frames of two icons at 16-48 px (4x) and 256 px, light and dark rows.

    python compare.py out.png "Label A=a.ico" "Label B=b.ico"
"""
import sys
from PIL import Image, ImageDraw

out, specs = sys.argv[1], sys.argv[2:]
small, K, GAP = [16, 20, 24, 32, 48], 4, 10
CELL = 48 * K
BIG = 256
LABEL = 24
W = GAP + 2 * (len(small) * (CELL + GAP) + BIG + GAP)
rowh = LABEL + max(CELL, BIG) + GAP
sheet = Image.new("RGB", (W, GAP + rowh * len(specs)), (128, 128, 128))
d = ImageDraw.Draw(sheet)
for r, spec in enumerate(specs):
    label, path = spec.split("=", 1)
    y0 = GAP + r * rowh
    d.text((GAP, y0 + 4), label, fill=(0, 0, 0))
    x = GAP
    for bg in [(250, 250, 250), (32, 32, 32)]:
        ico = Image.open(path)
        for s in small + [BIG]:
            ico.size = (s, s)
            frame = ico.copy().convert("RGBA")
            tile = Image.new("RGBA", (s, s), bg + (255,))
            tile.alpha_composite(frame)
            k = 1 if s == BIG else K
            big = tile.resize((s * k, s * k), Image.NEAREST).convert("RGB")
            c = BIG if s == BIG else CELL
            box = Image.new("RGB", (c, c), bg)
            box.paste(big, ((c - s * k) // 2, (c - s * k) // 2))
            sheet.paste(box, (x, y0 + LABEL))
            x += c + GAP
sheet.save(out)
print(out)
