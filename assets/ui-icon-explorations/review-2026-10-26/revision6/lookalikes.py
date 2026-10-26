# Revision 6 review: changed icons next to possible lookalikes (globe-like and arc-like icons),
# 16 px mono at x1 (actual), x3 and x6 nearest, then native 16/20/24 px light and dark.
# Usage: python -I lookalikes.py <ui-icon-explorations dir> <output png> <group>=<id>,<id>... [...]
import json, os, sys
from PIL import Image, ImageDraw, ImageFont

root, out, groups = sys.argv[1], sys.argv[2], [g.split('=') for g in sys.argv[3:]]
ids = [i['id'] for i in json.load(open(os.path.join(root, 'manifest.json'), encoding='utf-8'))['icons']]
num = {i: k + 1 for k, i in enumerate(ids)}
font = ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf', 12)
LIGHT, DARK, WHITE = (246, 246, 246), (32, 32, 36), (255, 255, 255)

def tile(theme, size, ic, bg, scale=1):
    im = Image.open(os.path.join(root, theme, 'bold', str(size), ic + '.png')).convert('RGBA')
    t = Image.new('RGBA', im.size, bg + (255,)); t.alpha_composite(im); t = t.convert('RGB')
    return t.resize((size * scale, size * scale), Image.NEAREST) if scale != 1 else t

CW = 16 * 6 + 10
rows = []
for name, lst in groups:
    rows.append((name, lst.split(',')))
maxn = max(len(l) for _, l in rows)
RH = 18 + 96 + 6 + 52 + 30
img = Image.new('RGB', (110 + maxn * CW, RH * len(rows)), (200, 200, 200))
d = ImageDraw.Draw(img)
for r, (name, lst) in enumerate(rows):
    y = r * RH
    d.text((4, y + 40), name, fill=(0, 0, 0), font=font)
    for k, ic in enumerate(lst):
        x = 110 + k * CW
        d.text((x, y + 2), f'{num[ic]}. {ic}'[:17], fill=(0, 0, 0), font=font)
        img.paste(tile('png-mono', 16, ic, WHITE, 6), (x, y + 18))
        yy = y + 18 + 96 + 6
        img.paste(tile('png-mono', 16, ic, WHITE), (x, yy))
        img.paste(tile('png-mono', 16, ic, WHITE, 3), (x + 20, yy))
        yy += 52; xx = x
        for s in (16, 20, 24):
            img.paste(tile('png', s, ic, LIGHT), (xx, yy)); xx += s + 3
img.save(out)
print('done')
