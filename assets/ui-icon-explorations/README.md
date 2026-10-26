# PaTorrent UI icon explorations

Open [review.html](review.html) directly in a browser to compare complete alternative icon families. No server, network connection, or installation is needed. Keep `manifest.js` beside the gallery.

## Choosing icons

- Compare **Bold color**, **Strong outline**, and **Contrast tile** side by side.
- Start at **16 px**, then check 20, 24 and 32 px. Each cell also offers a 48 px inspection view. These are real displayed sizes, not enlarged 16 px screenshots.
- Switch between light and dark backgrounds to see the matching artwork.
- Search by name, filter by UI area, and star individual family/icon combinations. The shortlist is kept in browser storage when available; export its JSON to retain or share choices reliably.
- Use the SVG and PNG links beneath a candidate to retrieve that exact theme and size.

## Files

- `svg/{family}/{id}.svg`: original editable light-interface vectors.
- `svg-dark/{family}/{id}.svg`: corresponding dark-interface vectors.
- `png/{family}/{size}/{id}.png` and `png-dark/{family}/{size}/{id}.png`: transparent raster exports at the sizes listed in the manifest.
- `manifest.json`: semantic icon inventory, families and export sizes.
- `manifest.js`: the same inventory exposed to the offline gallery as `window.ICON_EXPLORATIONS`.
- `app-concepts.png`, when present: additional generated application-mark concepts, presented as a selection sheet rather than individual ready-to-ship UI assets.

The UI symbols are original vector artwork designed around substantial strokes, simple silhouettes and clear negative space for small sizes. Color reinforces meaning; shapes remain the primary cue. Each family is a selection candidate, and individual symbols can be mixed or refined after review.

These files do not change the application's existing icon selection or runtime UI. Choose the family and individual variants first; integration and platform/DPI checks follow that decision. Repository licensing applies.

## Batch and regeneration

This batch contains **131 semantic symbols × 3 families = 393 candidates**, each with light/dark artwork: **786 SVGs** and **4,716 PNGs** at **16, 20, 24, 32, 48, 64 px**. Coverage includes the current 11 toolbar actions and 24 shared state icons, context actions, sidebar/navigation, details, preferences, RSS, file priorities/types, utility controls and brand support marks. Some controls intentionally reuse familiar glyphs (for example download and downloading).

- [preview.png](preview.png): first 22 symbols at actual 16 px on both backgrounds.
- [small-size-proof.png](small-size-proof.png): all symbols at actual 16 px on both backgrounds.
- [validation.json](validation.json): export counts and validation results.
- [prompts.json](prompts.json): exact built-in image_gen prompt for the 12 application concepts and provenance of the directly drawn UI vectors.
- `build.cjs`: deterministic vector and PNG export source. Uses the existing development-only `sharp` library, with no application dependency added.

Regenerate with `node build.cjs --sharp /absolute/path/to/sharp`. This regenerates vectors, manifests, PNGs and proof sheets; it preserves the separately generated app concept sheet and gallery. The application concepts remain a contact sheet, not transparent individual app masters or Windows ICOs. Choose a concept before preparing production app exports.

All raster exports were decoded and checked for expected dimensions, visible pixels and transparency. The proof sheet was inspected visually. These are review candidates: composite states and tile interiors need your judgment at actual size, followed by platform/DPI checks after selection.

## Revision 2 — visual corrections

77 symbols have been redrawn after a complete small-size audit, including Add Torrent, URL/Create, Pieces, Web UI, language, labels, locks and composite torrent states. The tile glyphs have more room, and fine details no longer inherit the same thick stroke as the outer frame. `refine.cjs` contains the revised geometry and is loaded by `build.cjs`.

The gallery now displays exact exported PNGs at the chosen proof size, while the 48 px inspection image remains SVG. Reload the page to pick up revision 2, then use **Show revised only** to focus on direct redraws. Existing IDs and saved shortlist keys are preserved.

See [QA.md](QA.md) for the review scope, limitations, reproducible checks and paginated 16/24/32 px proof sheets; [qa-comparison.png](qa-comparison.png) compares representative fixes with the first batch. The padded-canvas audit checks all 786 SVGs for clipped strokes, beyond simple image-format validation.

## Review 2026-10-26

[review-2026-10-26/REPORT.md](review-2026-10-26/REPORT.md) lists the 47 symbols that still fail after revision 2, the decisions needed and the specification for regenerating them.
