# Revision 6 review: the tracker states next to the letter strings they were read as in revision 5
# ("A✓", "Ac", "Ax", "A…") and the closest accented forms ("Âc", "Ȃx"), rendered in Segoe UI Bold
# at 16 px, plus the r5 (16b14bf) tracker base. Mono 16 px on white, x1 and x5 nearest.
# Usage: python -I letter-test.py <ui-icon-explorations dir> <output png>
import io, os, subprocess, sys
from PIL import Image, ImageDraw, ImageFont
root, out = sys.argv[1], sys.argv[2]
prefix = subprocess.run(['git', 'rev-parse', '--show-prefix'], cwd=root, capture_output=True, text=True,
                        check=True).stdout.strip()
def icon(ic, rev=None):
    rel = f'png-mono/bold/16/{ic}.png'
    if rev:
        im = Image.open(io.BytesIO(subprocess.run(['git', 'show', f'{rev}:{prefix}{rel}'], cwd=root,
                                                  capture_output=True, check=True).stdout))
    else:
        im = Image.open(os.path.join(root, rel))
    im = im.convert('RGBA'); t = Image.new('RGBA', im.size, (255, 255, 255, 255)); t.alpha_composite(im)
    return t.convert('RGB')
bold = ImageFont.truetype('C:/Windows/Fonts/segoeuib.ttf', 16)
lbl = ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf', 12)
def text(s):
    t = Image.new('RGB', (24, 16), (255, 255, 255)); ImageDraw.Draw(t).text((0, -3), s, fill=(0, 0, 0), font=bold)
    return t
cells = [('r6 working', icon('tracker-working')), ('r6 updating', icon('tracker-updating')),
         ('r6 error', icon('tracker-error')), ('r6 not-cont.', icon('tracker-not-contacted')),
         ('r5 updating', icon('tracker-updating', '16b14bf')), ('r6 trackers', icon('trackers')),
         ('"Ac"', text('Ac')), ('"Âc"', text('Âc')), ('"Ãx"', text('Ãx')), ('"Λ"', text('Λ'))]
Z = 5; CW = 24 * Z + 12
img = Image.new('RGB', (CW * len(cells) + 8, 16 * Z + 50), (200, 200, 200)); d = ImageDraw.Draw(img)
for k, (n, im) in enumerate(cells):
    x = 6 + k * CW
    d.text((x, 2), n, fill=(0, 0, 0), font=lbl)
    img.paste(im.resize((im.size[0] * Z, 16 * Z), Image.NEAREST), (x, 18))
    img.paste(im, (x, 18 + 16 * Z + 6))
img.save(out); print('done')
