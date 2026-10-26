# Revision 6 review: print 16 px masks as text so pixel positions can be quoted exactly.
# '#' = alpha >= 192, '+' = 128..191, '.' = 1..127, ' ' = 0. Columns/rows are numbered 0-15.
# Usage: python -I ascii-masks.py <ui-icon-explorations dir> <size> <id> [<id> ...] [--rev <commit>]
import io, os, subprocess, sys
from PIL import Image

args = sys.argv[1:]
rev = None
if '--rev' in args:
    k = args.index('--rev'); rev = args[k + 1]; del args[k:k + 2]
root, size, ids = args[0], int(args[1]), args[2:]
prefix = subprocess.run(['git', 'rev-parse', '--show-prefix'], cwd=root, capture_output=True,
                        text=True, check=True).stdout.strip()

def load(ic):
    rel = f'png-mono/bold/{size}/{ic}.png'
    if rev is None:
        return Image.open(os.path.join(root, rel)).convert('RGBA')
    blob = subprocess.run(['git', 'show', f'{rev}:{prefix}{rel}'], cwd=root, capture_output=True,
                          check=True).stdout
    return Image.open(io.BytesIO(blob)).convert('RGBA')

for ic in ids:
    a = load(ic).getchannel('A').load()
    print(f'{ic} ({size} px{", " + rev if rev else ""})')
    print('    ' + ''.join(str(x % 10) for x in range(size)))
    for y in range(size):
        row = ''.join('#' if a[x, y] >= 192 else '+' if a[x, y] >= 128 else '.' if a[x, y] else ' '
                      for x in range(size))
        print(f'{y:>3} {row}')
    print()
