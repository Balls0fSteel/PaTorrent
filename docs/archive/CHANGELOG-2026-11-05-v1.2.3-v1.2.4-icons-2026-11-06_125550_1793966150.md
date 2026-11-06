# Changelog archive: 2026-11-05, v1.2.3-v1.2.4 and the icon work

Moved from the root CHANGELOG.md on 2026-11-06_125550_1793966150. Paths and links keep their repository-root context.

## 2026-11-05_230706_1793916426: Published: the rename on GitHub `main` and the wiki

- The owner renamed the repository to `Balls0fSteel/Rookparcel`; Pages serves `https://balls0fsteel.github.io/Rookparcel/`. `origin` now names the new URL. `main` was fast-forwarded to `58e5b1fa` (1.2.2-1.2.4, the icons and the site colours) and pushed.
- Wiki `b97aafa`: `docs/wiki` and the search guide copied over; `Installing-`, `Updating-`, `Where-...-keeps-its-files` and `Running-...-nox` have new names, and the old pages are one-line pointers to them (they also show in GitHub's automatic page list). Checked first that the wiki had no edits outside `docs/wiki`. The screenshots still show the old name in title bars.

## 2026-11-05_230319_1793916199: Version 1.2.4: the small app icon is the courier rook (09, redrawn)

- The owner chose GPT's `09 Compact Courier` for 16-32 px. Redrawn in `assets/rookparcel-icons`: `src/courier-32.svg` (20-32 px, vector) and `src/courier-16.txt`/`.svg` (16 px, pixel by pixel), with a heavier, long two-toned rook beak (pale bare base), a smaller lidded eye, a peaked cap with band and gold badge, and the flat cream square replaced by a paper tile with a stamp-red rim that holds its edge on dark taskbars. The stamp stays from 48 px; `build.cjs` rebuilt `04-postal-rook`. D60 updated.
- Checked: the Win32 and nox builds, `the_app_icon_at_every_size`, `comparison.png` (before, GPT's 09, now; light and dark), and the icons Windows extracts from the built `rookparcel.exe`.

## 2026-11-05_225249_1793915569: Postal Rook candidates designed for small sizes

- Saved 11 built-in image-generation masters, exact prompts, PNG frames and ICOs in [the small-icon study](docs/art/postal-rook-small-2026-11-05/README.md). Decoded all 110 ICO frames; reviewed actual 16/20/24/32/48 px on light and dark backgrounds against the original stamp.
- Independent visual review favors 10 Night Courier, 11 Closed Envelope and 09 Compact Courier. These are new candidate studies; no production assets, D60 decision or program version changed. Next: owner selection and dedicated small-size refinement.

## 2026-11-05_225036_1793915436: Website in the Postal Rook's colours

- `site/css/style.css`: the teal and copper palette (from the Verdigris icon) becomes the stamp's: a navy hero and page heads with a stamp-red rule, red buttons, a paper-cream button with a kraft rim on the hero, gold hero links and focus ring, a pale red list hover, our benchmark bars in red; light and dark. Links stay a navy blue (red reads as an error). `--copper` is now `--accent`; every page's `theme-color` is `#14233d`.
- Checked: every text pair at WCAG AA or better in both schemes (lowest 5.7:1, the buttons); `build.py --strict` with the test releases, the site tests, and the pages in the browser pane in light and dark (computed colours, the benchmark bars, no console errors).

## 2026-11-05_224301_1793914981: Version 1.2.3: the Postal Rook is the app icon (D60)

- Family `04-postal-rook` (`assets/patina-icons/exports/app/`, built by `assets/rookparcel-icons/build.cjs`): the drawn 16 px and 20-32 px marks, and from 48 px the generated stamp the owner chose. `app-icon.txt` and the Flatpak `ICON_DIR` name it, so the Win32 and nox executables, GTK, the Web UI favicon, AppImages, macOS bundles, the Flatpak, the installer and the site all follow; D57's Verdigris is superseded.
- Checked: the Win32 and nox builds, `the_app_icon_at_every_size`, the site tests; the icons Windows extracts from `rookparcel.exe` (16-32 px drawn, 48 and 256 px the stamp) and a hidden demo window's title bar. The site's teal and copper colours still follow the old icon.

## 2026-11-05_223827_1793914707: Postal Rook app icon redrawn as vectors, per size

- `assets/rookparcel-icons/`: the owner's chosen Postal Rook as hand-written SVG in three drawings: a pixel-by-pixel 16 px mark, a 20-32 px head close-up (cap, pale rook face, beak on a kraft parcel) and the 48-512 px stamp (walking rook, courier bag). `build.cjs` makes the PNGs and a 10-frame ICO (25 KB against the cropped candidate's 145 KB); `comparison.png` sets its real frames beside the generated candidate's. At 16-32 px the head, cap and parcel stay readable where the crops blur. Candidate only: not yet wired into the programs.

## 2026-11-05_222944_1793914184: Cropped Rookparcel ICO candidates

- Exported 25 ICO files: light/dark crops of all 12 concepts plus a transparent Postal Rook stamp; matching PNGs, preview, crop coordinates, exporter and ZIP in the artwork folder. Postal Rook is the owner's favorite.
- Decoded all 250 embedded frames (10 sizes, 16-256 px) and visually checked the crop preview. Small sizes are resampled studies; dedicated pixel refinement and production integration remain open. No program change or version bump.

## 2026-11-05_221629_1793913389: Rookparcel UI icon studies

- Saved four concept sheets (96 labeled studies), exact prompts and visual review in [the UI artwork index](docs/art/rookparcel-ui-2026-11-05/README.md). Reviewed existing toolbar and small-size proofs with an independent audit.
- Verified all four PNGs decode; documented generated semantic inconsistencies. Exploration only: no production assets or program version changed; native-size vector refinement remains open.
