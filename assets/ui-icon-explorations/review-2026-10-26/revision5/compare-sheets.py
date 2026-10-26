# Revision 5 review: the 13 changed icons, revision 4 (commit 3fe8899) next to revision 5 (working tree).
# Per row: r4 16 px mono x10 | r5 16 px mono x10 (1 px grid) | r5 native 16 L/D/mono | r5 native 20 L/D |
#          r5 native 24 L/D | r5 native 32 L/D.
# Also writes families.png: each family at native 16 px mono/light/dark side by side and at x8.
# Usage: python -I compare-sheets.py <ui-icon-explorations dir> <output dir> [old commit]   (needs Pillow, git)
import io, json, os, subprocess, sys
from PIL import Image, ImageDraw, ImageFont

root, out = sys.argv[1], sys.argv[2]
old = sys.argv[3] if len(sys.argv) > 3 else '3fe8899'
CHANGED = ('network-ok network-no-incoming offline tracker-working tracker-updating tracker-error '
           'tracker-not-contacted reannounce reset-columns reload-ip-filter add-url download-limit '
           'upload-limit').split()
ids = [i['id'] for i in json.load(open(os.path.join(root, 'manifest.json'), encoding='utf-8'))['icons']]
num = {i: k + 1 for k, i in enumerate(ids)}
font = ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf', 13)
LIGHT, DARK, WHITE = (246, 246, 246), (32, 32, 36), (255, 255, 255)
prefix = subprocess.run(['git', 'rev-parse', '--show-prefix'], cwd=root, capture_output=True,
                        text=True, check=True).stdout.strip()

def load(theme, size, ic, rev=None):
    rel = f'{theme}/bold/{size}/{ic}.png'
    if rev is None:
        return Image.open(os.path.join(root, rel)).convert('RGBA')
    blob = subprocess.run(['git', 'show', f'{rev}:{prefix}{rel}'], cwd=root, capture_output=True,
                          check=True).stdout
    return Image.open(io.BytesIO(blob)).convert('RGBA')

def tile(im, bg, scale=1, grid=False):
    t = Image.new('RGBA', im.size, bg + (255,))
    t.alpha_composite(im)
    t = t.convert('RGB')
    if scale != 1:
        t = t.resize((im.size[0] * scale, im.size[1] * scale), Image.NEAREST)
    if grid:
        d = ImageDraw.Draw(t)
        for k in range(0, t.size[0] + 1, scale):
            d.line([(k, 0), (k, t.size[1])], fill=(205, 205, 215))
            d.line([(0, k), (t.size[0], k)], fill=(205, 205, 215))
        for k in range(0, t.size[0] + 1, scale * 4):
            d.line([(k, 0), (k, t.size[1])], fill=(150, 150, 190))
            d.line([(0, k), (t.size[0], k)], fill=(150, 150, 190))
    return t

Z = 10
LBL = 170
rowh = 16 * Z + 14

def compare(chunk, name):
    W = LBL + 2 * (16 * Z + 14) + (16 + 8) * 3 + (20 + 8) * 2 + (24 + 8) * 2 + (32 + 8) * 2 + 20
    img = Image.new('RGB', (W, 24 + rowh * len(chunk)), (200, 200, 200))
    d = ImageDraw.Draw(img)
    d.text((LBL, 4), f'r4 ({old}) 16 mono x{Z}', fill=(0, 0, 0), font=font)
    d.text((LBL + 16 * Z + 14, 4), f'r5 16 mono x{Z}', fill=(0, 0, 0), font=font)
    d.text((LBL + 2 * (16 * Z + 14), 4), 'r5 native 16 L/D/M, 20 L/D, 24 L/D, 32 L/D', fill=(0, 0, 0), font=font)
    for r, ic in enumerate(chunk):
        y = 24 + r * rowh
        d.text((6, y + 60), f'{num[ic]}. {ic}', fill=(0, 0, 0), font=font)
        x = LBL
        try:
            img.paste(tile(load('png-mono', 16, ic, old), WHITE, Z, True), (x, y))
        except subprocess.CalledProcessError:
            d.text((x + 10, y + 60), 'not in r4', fill=(0, 0, 0), font=font)
        x += 16 * Z + 14
        img.paste(tile(load('png-mono', 16, ic), WHITE, Z, True), (x, y))
        x += 16 * Z + 14
        for th, bg in (('png', LIGHT), ('png-dark', DARK), ('png-mono', WHITE)):
            img.paste(tile(load(th, 16, ic), bg), (x, y + 60)); x += 24
        for s in (20, 24, 32):
            for th, bg in (('png', LIGHT), ('png-dark', DARK)):
                img.paste(tile(load(th, s, ic), bg), (x, y + 60)); x += s + 8
    img.save(os.path.join(out, name))

os.makedirs(out, exist_ok=True)
for k in range(0, len(CHANGED), 8):
    compare(CHANGED[k:k + 8], f'compare-{k // 8 + 1:02d}.png')

# Families: how each set reads together, native size and x8.
FAM = [
    ('network trio', ['network-ok', 'network-no-incoming', 'offline', 'connection', 'random-port']),
    ('tracker family', ['trackers', 'tracker-working', 'tracker-updating', 'tracker-error',
                        'tracker-not-contacted', 'reannounce']),
    ('refresh badge users', ['refresh', 'reannounce', 'tracker-updating', 'reset-columns',
                             'reload-ip-filter']),
    ('limits', ['download-limit', 'upload-limit', 'download', 'upload', 'tray', 'speed', 'unlimited']),
    ('grids/boxes', ['all-torrents', 'pieces', 'copy', 'moving', 'allocating', 'disk', 'logger',
                     'file-video', 'random-port']),
    ('misc', ['proxy', 'dht', 'inactive', 'active', 'add-url', 'add-torrent', 'association',
              'create-torrent', 'properties']),
]
S = 8
maxn = max(len(f) for _, f in FAM)
W = 150 + maxn * (16 * S + 10)
rowf = 16 * S + 3 * 22 + 40
img = Image.new('RGB', (W, rowf * len(FAM)), (200, 200, 200))
d = ImageDraw.Draw(img)
for r, (title, fam) in enumerate(FAM):
    y = r * rowf
    d.text((6, y + 6), title, fill=(0, 0, 0), font=font)
    for k, ic in enumerate(fam):
        x = 150 + k * (16 * S + 10)
        d.text((x, y + 4), f'{num[ic]}. {ic}'[:20], fill=(0, 0, 0), font=font)
        img.paste(tile(load('png-mono', 16, ic), WHITE, S), (x, y + 22))
        yy = y + 22 + 16 * S + 6
        for th, bg in (('png-mono', WHITE), ('png', LIGHT), ('png-dark', DARK)):
            xx = x
            for s in (16, 20, 24):
                img.paste(tile(load(th, s, ic), bg), (xx, yy)); xx += s + 6
            yy += 26
img.save(os.path.join(out, 'families.png'))
print('done')
