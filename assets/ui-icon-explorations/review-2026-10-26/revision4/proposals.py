# Revision 4 review: reviewer's pixel sketches for the "Redraw as" column. Not artwork. Some show that a
# suggestion fits the 16 px grid and the badge rules (refresh badge with a 2 px gap, speed-limit gauge);
# others are recorded because they FAILED (towers 1-3, ports A and mirrored), to show which obvious
# fixes do not work. See REPORT.md. Rows are 16 px strings, '#' is ink.
# Each sketch is shown at x10 with a grid, then at native 16 px on white, light grey and dark.
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
# Badges as drawn in revision 4 (states.cjs / common.cjs), for context.
OK = (9, 9, ['.....#', '....##', '#..##.', '####..', '.##...'])
BANG = (13, 9, ['##', '##', '##', '##', '..', '##'])
CROSS = (9, 9, ['##..##', '.####.', '..##..', '..##..', '.####.', '##..##'])
SMALLX = (10, 10, ['##.##', '.###.', '..#..', '.###.', '##.##'])
PLUS = (9, 9, ['..##..', '..##..', '######', '######', '..##..', '..##..'])
# Refresh badge, revision 4, and a sketch with a 2 px gap under the arrowhead (like #21 refresh).
R4 = (9, 8, ['..##.#', '.#..##', '#..###', '#.....', '#....#', '.#..#.', '..##..'])
RB = (9, 8, ['..####', '.#..##', '#....#', '#.....', '#.....', '.#..#.', '..##..'])

def icon(base_rows, *badges):
    g = put(EMPTY, 0, 0, grid(base_rows))
    for x, y, rows in badges:
        g = put(g, x, y, rows)
    return g

# Tracker base sketch 1 (fails: still an insect/figure): shorter waves, lattice legs with a cross-brace.
TOWER = ['',
         '..#.##.#',
         '.#..##..#',
         '.#..##..#',
         '..#.##.#',
         '....##',
         '...#..#',
         '...####',
         '..#....#',
         '..#....#',
         '..#....#']
# Revision 4 compact tower for comparison.
TOWER_R4 = ['', '..#.##.#', '.##.##.##', '.#..##..#', '.#..##..#', '.##.##.##', '..#.##.#', '...####',
            '...#..#', '..##..##', '..#....#', '..#....#']
# Network sketch A (fails: still "-?"): a nearly closed 2 px ring with a 2 px mouth and a headless shaft.
PORT = ['',
        '........####',
        '.......######',
        '......##....##',
        '.####.......##',
        '.####.......##',
        '......##....##',
        '.......######',
        '........####']
PORT_R4 = ['', '........####', '....#..######', '....##......##', '.######.....##', '.######.....##',
           '....##......##', '....#..######', '........####']
# Network sketch B (least "?"-like, but reads as a key or plug): 1 px ring, arrowhead inside, 3 px mouth.
PORT_B = ['',
          '.......######',
          '......#......#',
          '.........#...#',
          '.##########..#',
          '.........#...#',
          '......#......#',
          '.......######']
# Speed-limit: half dial with a flat base and a needle, as the base; small arrow as the badge.
GAUGE = ['',
         '....######',
         '..##......##',
         '.#..........#',
         '.#.......##.#',
         '#.......##...#',
         '#......##....#',
         '#.....##.....#',
         '##############']
DOWN_BADGE = (10, 9, ['.##.', '.##.', '####', '.##.'])
# Add URL as a single chain link (two rounded loops, 45 degrees) with the plus badge.
LINK = ['',
        '.......####',
        '......#....#',
        '.....#..##.#',
        '....#..#..#',
        '...#..#..#',
        '..#..#..#',
        '.#.##..#',
        '.#....#',
        '..####']

# Tracker base sketch 2 (fails: reads as "P)" or "R"): one-sided waves, so it has no "arms".
TOWER2 = ['',
          '..##...#',
          '..##.#..#',
          '..##..#.#',
          '..##..#.#',
          '..##.#..#',
          '..##...#',
          '..##',
          '.#..#',
          '.####',
          '.#..#',
          '.#..#']
# Network sketch, mirrored (fails: reads as "C+"): ring top-left, arrow entering from the right.
PORT_M = [r[::-1].rjust(15, '.')[-15:] if r else r for r in PORT_R4]
# Tracker base sketch 3 (fails: reads as a trident "Ψ" on a stand): tall mast, stepped foot, no legs.
TOWER3 = ['',
          '..#....#',
          '.#..##..#',
          '.#..##..#',
          '.#..##..#',
          '..#.##.#',
          '....##',
          '....##',
          '....##',
          '...####',
          '..######']

SK = [
    ('tracker-working r4', icon(TOWER_R4, OK)),
    ('tower sketch + check', icon(TOWER, OK)),
    ('tower sketch + refresh', icon(TOWER, RB)),
    ('tower sketch + x', icon(TOWER, CROSS)),
    ('network-ok r4', icon(PORT_R4, OK)),
    ('port sketch A (no head) + check', icon(PORT, OK)),
    ('port sketch A + !', icon(PORT, BANG)),
    ('port sketch B + !', icon(PORT_B, BANG)),
    ('mirrored port + check', icon(PORT_M, OK)),
    ('mirrored port + !', icon(PORT_M, BANG)),
    ('mirrored port + x', icon(PORT_M, SMALLX)),
    ('tower sketch 2 + check', icon(TOWER2, OK)),
    ('tower sketch 2 + refresh', icon(TOWER2, RB)),
    ('tower sketch 2 + x', icon(TOWER2, CROSS)),
    ('tower sketch 3 + check', icon(TOWER3, OK)),
    ('tower sketch 3 + refresh', icon(TOWER3, RB)),
    ('tower sketch 3 + x', icon(TOWER3, CROSS)),
    ('refresh badge r4', icon(['', '', '', '', '.######', '.#....#', '.#....#', '.######'], R4)),
    ('refresh sketch 2px gap', icon(['', '', '', '', '.######', '.#....#', '.#....#', '.######'], RB)),
    ('speed-limit sketch', icon(GAUGE, DOWN_BADGE)),
    ('add-url link sketch', icon(LINK, PLUS)),
]

Z = 10
font = ImageFont.truetype('C:/Windows/Fonts/segoeui.ttf', 13)
COLS = 4
cw, ch = 16 * Z + 90, 16 * Z + 30
img = Image.new('RGB', (cw * COLS + 10, ch * ((len(SK) + COLS - 1) // COLS) + 10), (200, 200, 200))
d = ImageDraw.Draw(img)
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
    for j, (bg, fg) in enumerate((((255, 255, 255), (0, 0, 0)), ((246, 246, 246), (30, 90, 200)),
                                   ((32, 32, 36), (120, 170, 255)))):
        sx, sy = x0 + 16 * Z + 12, y0 + j * 26
        d.rectangle([sx, sy, sx + 15, sy + 15], fill=bg)
        for yy, row in enumerate(g):
            for xx, c in enumerate(row):
                if c == '#':
                    d.point((sx + xx, sy + yy), fill=fg)
img.save(sys.argv[1])
print(sys.argv[1])
