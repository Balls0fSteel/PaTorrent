# Revision 6 review: the changed icons at native 16 and 20 px only, mono/light/dark, at x1 and x8 nearest,
# so the x1.25 rounding at 20 px can be judged against the 16 px drawing.
# Usage: python -I size20.py <ui-icon-explorations dir> <output png> <id> [<id> ...]
import os, sys
from PIL import Image, ImageDraw, ImageFont
root, out, ids = sys.argv[1], sys.argv[2], sys.argv[3:]
font = ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf', 12)
TH = (('png-mono', (255, 255, 255)), ('png', (246, 246, 246)), ('png-dark', (32, 32, 36)))
def tile(th, s, ic, bg, z=1):
    im = Image.open(os.path.join(root, th, 'bold', str(s), ic + '.png')).convert('RGBA')
    t = Image.new('RGBA', im.size, bg + (255,)); t.alpha_composite(im); t = t.convert('RGB')
    return t.resize((s * z, s * z), Image.NEAREST)
Z = 8
RH = 20 * Z + 10
img = Image.new('RGB', (150 + 2 * (20 * Z + 8) + 3 * 2 * 28 + 20, RH * len(ids)), (200, 200, 200))
d = ImageDraw.Draw(img)
for r, ic in enumerate(ids):
    y = r * RH
    d.text((4, y + 70), ic, fill=(0, 0, 0), font=font)
    img.paste(tile('png-mono', 16, ic, (255, 255, 255), Z), (150, y + 4 * Z))
    img.paste(tile('png-mono', 20, ic, (255, 255, 255), Z), (150 + 20 * Z + 8, y))
    x = 150 + 2 * (20 * Z + 8)
    for th, bg in TH:
        for s in (16, 20):
            img.paste(tile(th, s, ic, bg), (x, y + 70)); x += 28
img.save(out); print('done')
