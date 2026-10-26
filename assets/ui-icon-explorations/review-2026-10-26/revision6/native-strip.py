# Revision 6 review: chosen icons at native 16/20/24/32 px in light, dark and mono, each x3 nearest,
# one icon per row, so the shipped sizes can be judged without the large zoom sheets.
# Usage: python -I native-strip.py <ui-icon-explorations dir> <output png> <id> [<id> ...]
import json, os, sys
from PIL import Image, ImageDraw, ImageFont

root, out, ids = sys.argv[1], sys.argv[2], sys.argv[3:]
order = [i['id'] for i in json.load(open(os.path.join(root, 'manifest.json'), encoding='utf-8'))['icons']]
font = ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf', 13)
THEMES = (('png', (246, 246, 246)), ('png-dark', (32, 32, 36)), ('png-mono', (255, 255, 255)))
SIZES, Z, LBL = (16, 20, 24, 32), 3, 180
W = LBL + len(THEMES) * (sum(s * Z + 6 for s in SIZES) + 14)
rowh = 32 * Z + 8
img = Image.new('RGB', (W, len(ids) * rowh + 4), (200, 200, 200))
d = ImageDraw.Draw(img)
for r, ic in enumerate(ids):
    y = 4 + r * rowh
    d.text((6, y + 40), f'{order.index(ic) + 1}. {ic}', fill=(0, 0, 0), font=font)
    x = LBL
    for th, bg in THEMES:
        for s in SIZES:
            im = Image.open(os.path.join(root, th, 'bold', str(s), ic + '.png')).convert('RGBA')
            t = Image.new('RGBA', im.size, bg + (255,))
            t.alpha_composite(im)
            img.paste(t.convert('RGB').resize((s * Z, s * Z), Image.NEAREST), (x, y))
            x += s * Z + 6
        x += 14
img.save(out)
print(out)
