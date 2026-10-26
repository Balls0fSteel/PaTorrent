# Artwork QA

## Revision 5 (2026-10-26)

- Redrew the revision 4 failures per D29: the network trio, the four tracker states and reannounce, the refresh badge (and its four users), add-url and both limits. Each was chosen from several candidates at zoom and at native 16/20/24/32 px. Rejected along the way:
  - narrowed towers whose mast top looked like a head;
  - towers that needed the outer margin or touched the badge;
  - 5 px refresh badges that read as "e".
- The build, geometry check and proof pages pass, with zero overflow, margin failures and unexpected identical masks; median partial alpha at 16 px is 22.7 %. Near matches: normal/forced metadata (0.88) and download/upload-limit (0.93, gauge plus a direction badge).
- The reviewer's scripts were rerun on the result: `review-sheets.py` and `shipped-sizes.py` (20 px clean), plus `compare-sheets.py` against `bef24b7`. Revision 4 → 5 comparisons and family sheets are in [qa/revision5](qa/revision5/).
- Independent review: [revision 5 report](review-2026-10-26/revision5/REPORT.md). It reproduced these measurements and passed 123/131. It failed the network trio (eye or key; globe fallback needed) and the tracker states plus reannounce (they read "A✓", "Ac"…).

## Revision 4 (2026-10-26)

- Redrew the 18 symbols and the badge listed in the [revision 4 brief](review-2026-10-26/revision3/REGENERATION-BRIEF.md). Pieces was also redrawn to avoid matching the new all-torrents grid. Design notes are in the README.
- Each candidate was drawn as pixel art and compared against alternatives, about a dozen rounds in all, with zoomed monochrome views and native 16/20/24/32 px renders in light and dark. Rejected versions included:
  - a ring arrow that read as "Q", and a tower that read as a stick figure;
  - a 2 px refresh badge that read as "₵", and an upright magnet that read as "Ü".
- `build.cjs`, `check-geometry.cjs` and `proof-pages.cjs` pass. There is zero overflow, zero margin failures and zero unexpected identical masks, with completed/tracker-working removed from the allowlist. The only near match is still normal/forced metadata (0.88). The median partial alpha at 16 px fell from 27 % to 23 %.
- The revision 3 review scripts (`review-sheets.py`, `shipped-sizes.py`) were run on the result. The shipped sheet showed broken 20 px rows; `H.px` now emits shared-edge paths, and the rerun was clean. The proof pages under `qa/revision3/` are regenerated with this artwork.
- Independent review: [revision 4 report](review-2026-10-26/revision4/REPORT.md). It reproduced these measurements and passed 120/131. It failed the network trio, the tracker states and reannounce, the refresh badge, add-url and both limits.

# Revision 3 artwork QA

The revision 2 QA claims are superseded by the [2026-10-26 audit](review-2026-10-26/REPORT.md). Export validity alone did not establish legibility. Revision 3 uses a complete redraw, native-size exports, mask comparisons and a separate visual review.

## Checks performed

- All 131 symbols are exported as one bold family, in light/dark/monochrome, at 16/20/24/32/48/64 px: **2,358 SVGs and 2,358 PNGs**. Every PNG is decoded for dimensions, visible ink and transparency.
- Every size is rendered from a separately fitted SVG. Small source geometry is used at 16/20; explicit large geometry is used at 24+ where supplied. The larger drawing retains the same metaphor and readable detail at 16 logical px.
- Padded-canvas raster checks cover all 786 monochrome icon/size combinations. **Zero overflow and zero outer-pixel margin failures.** The check fails on either condition.
- All pairs are compared at 16 and 32 px after thresholding alpha at 128/255. **Zero unapproved identical masks.** Eight explicit action/state or same-operation pairs are allowlisted in the manifest. Near matches at intersection-over-union ≥0.88 remain visible in the audit rather than being hidden.
- The remaining near match is normal/forced metadata at 16 px (shared page and down arrow). The added upper-right lightning is visible in monochrome and the GTK logical-size proof; this is a modifier relationship, not a colour-only distinction.
- Median partially transparent ink at native 16 px is approximately **27%**, compared with the prior report's 55% for bold. Curves and diagonals remain antialiased; straight menu/pause/stop geometry is opaque and aligned. This metric is diagnostic, not a visual-quality pass by itself.
- The complete six-page 16/24 light/dark/monochrome proof set was visually inspected. A second reviewer inspected all 131 symbols plus the shared-runtime and GTK logical-size proofs. Their final findings led to a recognisable intact plug + cross for Offline, thinner/readable resume-check clocks and a square completed-Stop base. Labels/pages/folders retain their bases; force/stall, queue/transfer, network/chart, limits/sort and tracker/Close are separated.
- The 32 px runtime exports were also previewed at 16 logical px. This is a raster proof of the GTK sizing path, not a live GTK application test.

- Gallery JavaScript passed 18 size/theme combinations, 9,432 generated asset-reference checks, search/group filtering and stable shortlist handling in a DOM harness. The local HTTP preview returned 200. This does not claim a full interactive browser/platform test.

## Evidence

- [Toolbar](preview.png)
- [All native 16 px](small-size-proof.png)
- [Shared runtime symbols](qa/revision3/runtime.png)
- [GTK logical sizing](qa/revision3/gtk-logical.png)
- Complete proof pages [1](qa/revision3/all-01.png), [2](qa/revision3/all-02.png), [3](qa/revision3/all-03.png), [4](qa/revision3/all-04.png), [5](qa/revision3/all-05.png), [6](qa/revision3/all-06.png).
- [Measured geometry and collision audit](geometry-audit.json)

## Remaining gates

Artwork review involves judgment; the sheets expose the exact pixels for owner review. Runtime integration, toolkit rendering, accessibility themes and real platform/DPI checks remain open. No application code or production artwork selection changed in this revision. The 12 app concepts remain discussion-only raster artwork.
