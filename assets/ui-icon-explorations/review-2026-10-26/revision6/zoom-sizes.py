# Revision 6 review: chosen icons at native 16/20/24/32/48 px in light, dark and mono,
# each enlarged with nearest-neighbour so the rounding at 20/24 px is visible.
# Usage: python -I zoom-sizes.py <ui-icon-explorations dir> <output png> <scale> <id> [<id> ...]
import json, os, sys
from PIL import Image, ImageDraw, ImageFont

root, out, Z, ids = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4:]
order = [i['id'] for i in json.load(open(os.path.join(root, 'manifest.json'), encoding='utf-8'))['icons']]
font = ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf', 13)
THEMES = (('png', (246, 246, 246)), ('png-dark', (32, 32, 36)), ('png-mono', (255, 255, 255)))
SIZES = (16, 20, 24, 32, 48)
W = 190 + sum(s * Z + 8 for s in SIZES)
rowh = 48 * Z + 8
img = Image.new('RGB', (W, len(ids) * len(THEMES) * rowh + 4), (200, 200, 200))
d = ImageDraw.Draw(img)
y = 4
for ic in ids:
    for th, bg in THEMES:
        d.text((6, y + 4), f'{order.index(ic) + 1}. {ic}', fill=(0, 0, 0), font=font)
        d.text((6, y + 22), th, fill=(60, 60, 60), font=font)
        x = 190
        for s in SIZES:
            im = Image.open(os.path.join(root, th, 'bold', str(s), ic + '.png')).convert('RGBA')
            t = Image.new('RGBA', im.size, bg + (255,))
            t.alpha_composite(im)
            img.paste(t.convert('RGB').resize((s * Z, s * Z), Image.NEAREST), (x, y))
            x += s * Z + 8
        y += rowh
img.save(out)
print(out)
