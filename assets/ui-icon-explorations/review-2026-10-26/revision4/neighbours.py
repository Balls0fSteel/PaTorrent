# Revision 4 review: nearest monochrome-mask neighbours of the changed icons.
# Masks are png-mono/bold/<size>/<id>.png thresholded at alpha >= 128 (as check-geometry.cjs).
# Also reports, per changed icon, the smallest 8-connected gap between the badge and the base
# for icons whose badge is a separate component in the bottom-right corner.
# Usage: python -I neighbours.py <ui-icon-explorations dir> [size] [top]   (needs Pillow)
import json, sys, os
from PIL import Image

root = sys.argv[1]
size = int(sys.argv[2]) if len(sys.argv) > 2 else 16
top = int(sys.argv[3]) if len(sys.argv) > 3 else 3
CHANGED = ('network-ok network-no-incoming offline trackers tracker-working tracker-updating '
           'tracker-error tracker-not-contacted reannounce reset-columns reload-ip-filter allocating '
           'proxy dht logger inactive add-url association all-torrents pieces download-limit '
           'upload-limit').split()
m = json.load(open(os.path.join(root, 'manifest.json'), encoding='utf-8'))
ids = [i['id'] for i in m['icons']]
num = {i: k + 1 for k, i in enumerate(ids)}

def mask(i):
    im = Image.open(os.path.join(root, 'png-mono', 'bold', str(size), i + '.png')).convert('RGBA')
    a = im.getchannel('A').load()
    return {(x, y) for y in range(size) for x in range(size) if a[x, y] >= 128}

M = {i: mask(i) for i in ids}

def iou(a, b):
    u = len(a | b)
    return len(a & b) / u if u else 1.0

print(f'# {size} px: top {top} mask neighbours (IoU) of each changed icon')
for c in CHANGED:
    sc = sorted(((iou(M[c], M[o]), o) for o in ids if o != c), reverse=True)[:top]
    print(f'{num[c]:>3} {c:<24}', '  '.join(f'{o}({num[o]}) {v:.2f}' for v, o in sc))

# All pairs with IoU >= 0.70 that involve a changed icon.
print('\n# pairs >= 0.70 involving a changed icon')
seen = set()
for c in CHANGED:
    for o in ids:
        if o == c or (o, c) in seen:
            continue
        seen.add((c, o))
        v = iou(M[c], M[o])
        if v >= 0.70:
            print(f'{c}({num[c]}) ~ {o}({num[o]}) {v:.3f}')

# Components (8-connected) and the minimum Chebyshev gap between the component(s) touching the
# bottom-right 7x7 badge area and the rest. A gap of 2 means one clear pixel in between.
def comps(px):
    px = set(px); out = []
    while px:
        s = [px.pop()]; comp = set(s)
        while s:
            x, y = s.pop()
            for dx in (-1, 0, 1):
                for dy in (-1, 0, 1):
                    q = (x + dx, y + dy)
                    if q in px:
                        px.remove(q); comp.add(q); s.append(q)
        out.append(comp)
    return out

if size == 16:
    print('\n# 16 px badge/base clearance (Chebyshev distance; 2 = one clear pixel)')
    for c in CHANGED:
        cs = comps(M[c])
        badge = [k for k in cs if min(x for x, _ in k) >= 7 and min(y for _, y in k) >= 7]
        base = [k for k in cs if k not in badge]
        if not badge or not base:
            print(f'{c:<24} components={len(cs)} (no separate badge)')
            continue
        bp = set().union(*badge); ap = set().union(*base)
        d = min(max(abs(x1 - x2), abs(y1 - y2)) for x1, y1 in bp for x2, y2 in ap)
        print(f'{c:<24} components={len(cs)} badge_px={len(bp)} min_gap={d}')
