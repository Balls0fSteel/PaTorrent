# Labeled review sheets: every icon, 3 families, light/dark, 64 px + 16 px (x3 nearest and actual).
# Usage: python -I review-sheets.py <ui-icon-explorations dir> <output dir>   (needs Pillow)
import json, sys, os
from PIL import Image, ImageDraw, ImageFont
root = sys.argv[1]
out = sys.argv[2]
os.makedirs(out, exist_ok=True)
m = json.load(open(os.path.join(root, 'manifest.json'), encoding='utf-8'))
icons = m['icons']
fams = ['bold', 'outline', 'tile']
font = ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf', 13)
LIGHT = (246, 246, 246)
DARK = (32, 32, 36)
PER = 12
# columns: for each family: 64 light, 64 dark, 16 light x3 nearest, 16 dark x3 nearest, 16 light actual, 16 dark actual
LBL = 210
cellw = 64 + 64 + 48 + 48 + 20 + 20 + 6 * 6 + 14
rowh = 72
for p in range(0, len(icons), PER):
    chunk = icons[p:p + PER]
    W = LBL + cellw * 3
    H = 22 + rowh * len(chunk)
    img = Image.new('RGB', (W, H), (200, 200, 200))
    d = ImageDraw.Draw(img)
    for fi, f in enumerate(fams):
        d.text((LBL + fi * cellw + 4, 3), f + ': 64L 64D 16Lx3 16Dx3 16L 16D', fill=(0, 0, 0), font=font)
    for ri, ic in enumerate(chunk):
        y = 22 + ri * rowh
        d.text((4, y + 18), f"{p + ri + 1}. {ic['id']}", fill=(0, 0, 0), font=font)
        d.text((4, y + 36), ic['label'][:30], fill=(60, 60, 60), font=font)
        for fi, f in enumerate(fams):
            x = LBL + fi * cellw
            def paste(src, size, bg, scale, x, y):
                im = Image.open(os.path.join(root, src, f, str(size), ic['id'] + '.png')).convert('RGBA')
                if scale != 1:
                    im = im.resize((size * scale, size * scale), Image.NEAREST)
                tile = Image.new('RGBA', im.size, bg + (255,))
                tile.alpha_composite(im)
                img.paste(tile.convert('RGB'), (x, y))
            paste('png', 64, LIGHT, 1, x, y + 4); x += 70
            paste('png-dark', 64, DARK, 1, x, y + 4); x += 70
            paste('png', 16, LIGHT, 3, x, y + 12); x += 54
            paste('png-dark', 16, DARK, 3, x, y + 12); x += 54
            paste('png', 16, LIGHT, 1, x, y + 28); x += 26
            paste('png-dark', 16, DARK, 1, x, y + 28); x += 26
    img.save(os.path.join(out, f'sheet-{p // PER + 1:02d}.png'))
print('done', (len(icons) + PER - 1) // PER)
