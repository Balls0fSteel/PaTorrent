# Rookparcel app icon (candidate, D59)

The Postal Rook direction the owner chose from the generated concepts
(`docs/art/rookparcel-candidates-2026-11-05`), redrawn as vectors, with a drawing of its own
for each size range instead of one picture shrunk to every size:

| Sizes | Drawing | What it shows |
|---|---|---|
| 16 px | `src/mark-16.txt` → `src/mark-16.svg` (pixel by pixel, `pixelmap.py`) | stamp edge, red cap with badge, the rook's head, parcel |
| 20-32 px | `src/mark-32.svg` | the head close-up: cap, pale rook face, beak on the parcel |
| 48-512 px | `src/stamp-256.svg` | the full stamp: a walking rook in a postal cap, courier bag, parcel in its beak |

The pale bare face at the base of the beak is what tells a rook from a crow; it also keeps the
beak visible at small sizes. Palette: stamp red `#B8352A`, navy `#14233D`, paper `#F7EEDC`, kraft
`#C4864A`, badge gold `#F2C14E`.

Build (sharp, as the other icon tools; on this PC under Strata's harness):

    python pixelmap.py src/mark-16.txt src/mark-16.svg
    SHARP=<path to sharp> node build.cjs      # exports/16-512.png, exports/rookparcel.ico

`comparison.png` sets the ICO's real frames (16, 20, 24, 32, 48 px at 4x, and 256 px) beside the
generated concept's on light and dark backgrounds; `preview-512.png` is the large stamp. Not yet
wired into the programs: that replaces the Verdigris P in the Win32 resources, the GTK/macOS/
Flatpak icons and the site.
