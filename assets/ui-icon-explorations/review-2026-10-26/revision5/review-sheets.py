# Revision 3/4/5 review sheets: bold family, 64/24 px light+dark, 16 px x3 light/dark/mono, actual 16 px.
# Usage: python -I review-sheets.py <ui-icon-explorations dir> <output dir>   (needs Pillow)
import json, sys, os
from PIL import Image, ImageDraw, ImageFont

root, out = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)
icons = json.load(open(os.path.join(root, 'manifest.json'), encoding='utf-8'))['icons']
font = ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf', 13)
LIGHT, DARK, WHITE = (246, 246, 246), (32, 32, 36), (255, 255, 255)
PER, COLS = 14, 2
LBL = 180
# per icon: 64L 64D 24L 24D 16Lx3 16Dx3 16Mx3 16L 16D
cellw = LBL + 64 + 64 + 24 + 24 + 48 * 3 + 16 + 16 + 11 * 6
rowh = 72

def paste(img, src, size, bg, scale, x, y, ic):
    im = Image.open(os.path.join(root, src, 'bold', str(size), ic + '.png')).convert('RGBA')
    if scale != 1:
        im = im.resize((size * scale, size * scale), Image.NEAREST)
    t = Image.new('RGBA', im.size, bg + (255,))
    t.alpha_composite(im)
    img.paste(t.convert('RGB'), (x, y))
    return x + im.size[0] + 6

pages = 0
for p in range(0, len(icons), PER * COLS):
    chunk = icons[p:p + PER * COLS]
    img = Image.new('RGB', (cellw * COLS, 22 + rowh * PER), (200, 200, 200))
    d = ImageDraw.Draw(img)
    for c in range(COLS):
        d.text((c * cellw + LBL, 3), '64L 64D 24L 24D 16Lx3 16Dx3 16Monox3 16L 16D', fill=(0, 0, 0), font=font)
    for k, ic in enumerate(chunk):
        c, r = divmod(k, PER)
        x, y = c * cellw, 22 + r * rowh
        d.text((x + 4, y + 18), f"{p + k + 1}. {ic['id']}", fill=(0, 0, 0), font=font)
        d.text((x + 4, y + 36), ic['label'][:26], fill=(60, 60, 60), font=font)
        x += LBL
        x = paste(img, 'png', 64, LIGHT, 1, x, y + 4, ic['id'])
        x = paste(img, 'png-dark', 64, DARK, 1, x, y + 4, ic['id'])
        x = paste(img, 'png', 24, LIGHT, 1, x, y + 24, ic['id'])
        x = paste(img, 'png-dark', 24, DARK, 1, x, y + 24, ic['id'])
        x = paste(img, 'png', 16, LIGHT, 3, x, y + 12, ic['id'])
        x = paste(img, 'png-dark', 16, DARK, 3, x, y + 12, ic['id'])
        x = paste(img, 'png-mono', 16, WHITE, 3, x, y + 12, ic['id'])
        x = paste(img, 'png', 16, LIGHT, 1, x, y + 28, ic['id'])
        x = paste(img, 'png-dark', 16, DARK, 1, x, y + 28, ic['id'])
    pages += 1
    img.save(os.path.join(out, f'sheet-{pages:02d}.png'))
print('pages', pages)
