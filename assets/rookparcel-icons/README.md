# Rookparcel app icon: the Postal Rook (D60)

The Postal Rook direction the owner chose from the generated concepts
(`docs/art/rookparcel-candidates-2026-11-05`, `08-postal-rook`), with a drawing of its own for
each size range instead of one picture shrunk to every size:

| Sizes | Source | What it shows |
|---|---|---|
| 16 px | `src/mark-16.txt` → `src/mark-16.svg` (pixel by pixel, `pixelmap.py`) | stamp edge, red cap with badge, the rook's head, parcel |
| 20-32 px | `src/mark-32.svg` (vector) | the head close-up: cap, pale rook face, beak on the parcel |
| 48-512 px | `src/stamp-512.png` (the generated stamp, owner's choice) | the full stamp: rook in a postal cap, courier bag, parcel in its beak |

The pale bare face at the base of the beak is what tells a rook from a crow; it also keeps the
beak visible at small sizes. Palette: stamp red `#B8352A`, navy `#14233D`, paper `#F7EEDC`, kraft
`#C4864A`, badge gold `#F2C14E`. `src/stamp-256.svg` is a vector version of the stamp, kept as the
alternative for the large sizes (`preview-512.png`).

Build (sharp, as the other icon tools; on this PC under Strata's harness):

    python pixelmap.py src/mark-16.txt src/mark-16.svg
    SHARP=<path to sharp> node build.cjs

It writes the family `04-postal-rook` into `../patina-icons/exports/app/` (PNGs at 16-512 px and an
8-frame ICO), the family `../patina-icons/app-icon.txt` names, so every program and package uses
it (`../patina-icons/INTEGRATION.md`). `comparison.png` (`compare.py`) sets the ICO's real frames
(16, 20, 24, 32, 48 px at 4x, and 256 px) beside the generated stamp's crops on light and dark.
