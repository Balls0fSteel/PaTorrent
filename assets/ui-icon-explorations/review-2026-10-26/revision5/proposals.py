# Revision 5 review: reviewer's pixel sketches for the "Redraw as" column. Not artwork.
# Rows are 16 px strings ('#' = ink), placed with the revision 5 badges (states.cjs / common.cjs).
# Each sketch is shown at x10 with a grid, then at native 16 px on white, light grey and dark, and
# at x3 nearest (the review-sheet scale). Sketches that FAILED are kept and labelled, as in revision 4.
# Usage: python -I proposals.py <output png>   (needs Pillow)
import sys
from PIL import Image, ImageDraw, ImageFont

def grid(rows):
    rows = (rows + [''] * 16)[:16]
    return [r.ljust(16, '.')[:16] for r in rows]

def put(base, x0, y0, rows):
    out = [list(r) for r in base]
    for dy, r in enumerate(rows):
        for dx, ch in enumerate(r):
            if ch == '#':
                out[y0 + dy][x0 + dx] = '#'
    return [''.join(r) for r in out]

EMPTY = ['.' * 16] * 16
# Revision 5 badges.
OK = (10, 10, ['....#', '...##', '#.##.', '###..', '.#...'])
BANG = (12, 10, ['##', '##', '##', '..', '##'])
CROSS = (10, 10, ['##.##', '.###.', '..#..', '.###.', '##.##'])
MORE = (10, 13, ['#.#.#', '#.#.#'])
R5 = (10, 9, ['..###', '.#.##', '#..##', '#....', '#....', '.###.'])
# Refresh badge with a true corner arrowhead: (13,11) dropped, so the head tapers to one pixel.
RB = (10, 9, ['..###', '.#.##', '#...#', '#....', '#....', '.###.'])

def icon(base_rows, *badges):
    g = put(EMPTY, 0, 0, grid(base_rows))
    for x, y, rows in badges:
        g = put(g, x, y, rows)
    return g

# --- Network bases -------------------------------------------------------------------------
PORT5 = ['', '......######', '.....##....##', '....##..#...##', '.######.##...#', '.######.##...#',
         '....##..#...##', '.....##....##', '......######']
# Shaft joined to the head (fills x 7): one continuous arrow, head inside the ring.
PORT_J = ['', '......######', '.....##....##', '....##..#...##', '.#########...#', '.#########...#',
          '....##..#...##', '.....##....##', '......######']
# Globe (D29 fallback): 10 x 9 at x 1-10, y 1-9, equator and a lens meridian; x >= 9 only above y 9.
GLOBE = ['', '....####', '..##.##.##', '.#..#..#..#', '.#..#..#..#', '.##########', '.#..#..#..#',
         '.#..#..#..#', '..##.##.##', '....####']
# --- Tracker bases (badge rule: x <= 8 from y 9 down) -----------------------------------------
ARCS = ['', '..#.........#', '.#..#.....#..#', '.#.#.......#.#', '.#.#.......#.#', '.#..#.....#..#',
        '..#.........#']
def tower(mast_and_base):
    out = []
    for k in range(16):
        a = ARCS[k] if k < len(ARCS) else ''
        b = mast_and_base[k] if k < len(mast_and_base) else ''
        out.append(''.join('#' if (i < len(a) and a[i] == '#') or (i < len(b) and b[i] == '#') else '.'
                           for i in range(16)))
    return out
T5 = tower(['', '', '', '......##', '......##', '......##', '......##', '......##', '.....#..#', '.....####',
            '.....#..#', '.....#..#', '.....#..#', '.....#..#'])
# No crossbar: the mast forks into two legs (fails? see report).
T_NOBAR = tower(['', '', '', '......##', '......##', '......##', '......##', '......##', '.....#..#',
                 '.....#..#', '.....#..#', '.....#..#', '.....#..#', '.....#..#'])
# Lattice with X bracing, 4 px wide.
T_X = tower(['', '', '', '......##', '......##', '......##', '......##', '......##', '.....#..#',
             '......##', '.....#..#', '.....#..#', '......##', '.....#..#', '.....#..#'])
# Tall mast down to a short foot, no A-frame.
T_FOOT = tower(['', '', '', '......##', '......##', '......##', '......##', '......##', '......##',
                '......##', '......##', '......##', '.....####', '....######'])
# Asymmetric splay: the left leg leans out, the right leg stays at x 8.
T_LEAN = tower(['', '', '', '......##', '......##', '......##', '......##', '......##', '.....#..#',
                '.....####', '....#...#', '....#...#', '...#....#', '...#....#'])

# Tower moved left so its legs can splay symmetrically inside x 1-8 (the badge rule):
# broadcast arcs above the apex instead of beside the mast, legs splayed, no hip bar.
T_WIFI = ['', '..######', '.#......#', '...####', '..#....#', '....##', '....##', '...#..#', '...#..#',
          '..#....#', '..#....#', '.#......#', '.#......#', '.#......#']
SK = [
    ('tower wifi-arcs Lambda + check', icon(T_WIFI, OK)),
    ('tower wifi-arcs Lambda + refresh', icon(T_WIFI, RB)),
    ('tower wifi-arcs Lambda + x', icon(T_WIFI, CROSS)),
    ('tower wifi-arcs Lambda + ...', icon(T_WIFI, MORE)),
    ('network-ok r5', icon(PORT5, OK)),
    ('no-incoming r5', icon(PORT5, BANG)),
    ('port, shaft joined + check', icon(PORT_J, OK)),
    ('port, shaft joined + !', icon(PORT_J, BANG)),
    ('globe + check', icon(GLOBE, OK)),
    ('globe + !', icon(GLOBE, BANG)),
    ('globe + x', icon(GLOBE, CROSS)),
    ('tracker-updating r5', icon(T5, R5)),
    ('tower no crossbar + check', icon(T_NOBAR, OK)),
    ('tower no crossbar + refresh', icon(T_NOBAR, RB)),
    ('tower X lattice + check', icon(T_X, OK)),
    ('tower X lattice + refresh', icon(T_X, RB)),
    ('tower mast+foot + check', icon(T_FOOT, OK)),
    ('tower mast+foot + refresh', icon(T_FOOT, RB)),
    ('tower mast+foot + x', icon(T_FOOT, CROSS)),
    ('tower mast+foot + ...', icon(T_FOOT, MORE)),
    ('tower leaning leg + check', icon(T_LEAN, OK)),
    ('tower leaning leg + refresh', icon(T_LEAN, RB)),
    ('refresh badge r5 (table)', icon(['', '.#############', '.#############', '.#...#...#...#', '.#...#...#...#',
                                        '.#...#...#...#', '.#############'], R5)),
    ('refresh badge, tapered head', icon(['', '.#############', '.#############', '.#...#...#...#',
                                          '.#...#...#...#', '.#...#...#...#', '.#############'], RB)),
]

Z = 10
font = ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf', 13)
COLS = 4
cw, ch = 16 * Z + 80, 16 * Z + 30 + 54
img = Image.new('RGB', (cw * COLS + 10, ch * ((len(SK) + COLS - 1) // COLS) + 10), (200, 200, 200))
d = ImageDraw.Draw(img)
COL = (((255, 255, 255), (0, 0, 0)), ((246, 246, 246), (30, 90, 200)), ((32, 32, 36), (120, 170, 255)))
for k, (name, g) in enumerate(SK):
    x0, y0 = 10 + (k % COLS) * cw, 10 + (k // COLS) * ch
    d.text((x0, y0), name, fill=(0, 0, 0), font=font)
    y0 += 18
    d.rectangle([x0, y0, x0 + 16 * Z, y0 + 16 * Z], fill=(255, 255, 255))
    for yy, row in enumerate(g):
        for xx, c in enumerate(row):
            if c == '#':
                d.rectangle([x0 + xx * Z, y0 + yy * Z, x0 + xx * Z + Z - 1, y0 + yy * Z + Z - 1], fill=(0, 0, 0))
    for t in range(17):
        d.line([(x0 + t * Z, y0), (x0 + t * Z, y0 + 16 * Z)], fill=(210, 210, 220))
        d.line([(x0, y0 + t * Z), (x0 + 16 * Z, y0 + t * Z)], fill=(210, 210, 220))
    for j, (bg, fg) in enumerate(COL):
        sx, sy = x0 + 16 * Z + 12, y0 + j * 26
        d.rectangle([sx, sy, sx + 15, sy + 15], fill=bg)
        for yy, row in enumerate(g):
            for xx, c in enumerate(row):
                if c == '#':
                    d.point((sx + xx, sy + yy), fill=fg)
    # x3 strip under the grid, in the three colour modes.
    for j, (bg, fg) in enumerate(COL):
        sx, sy = x0 + j * 54, y0 + 16 * Z + 6
        d.rectangle([sx, sy, sx + 47, sy + 47], fill=bg)
        for yy, row in enumerate(g):
            for xx, c in enumerate(row):
                if c == '#':
                    d.rectangle([sx + xx * 3, sy + yy * 3, sx + xx * 3 + 2, sy + yy * 3 + 2], fill=fg)
img.save(sys.argv[1])
print(sys.argv[1])
