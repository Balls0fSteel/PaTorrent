# Revision 2 visual QA

The first batch passed image-format checks, but those checks did not establish visual quality. Native-resolution review found overlapping page modifiers, overly heavy internal strokes, and a tile transform that reduced detail too far.

## Corrections

77 semantic symbols were directly redrawn in `refine.cjs`; the whole set also received rounded joins, lighter default strokes and a larger tile interior (90% rather than 75%). Add Torrent now uses an open page with a separate large plus; Create uses a sparkle. Bars and dots have explicit fills without inherited outlines. Dense grid/browser/settings artwork was simplified; locks, labels, completion checks and peer states have separated cues. Forced transfers use double arrows; stalled transfers retain warnings.

## Review performed

- First pass: all 131 symbols, three families and both themes, using five native-resolution 16 px sheets. Independent visual audit identified the major failures.
- Second pass: all revised 16 px sheets reviewed again; an independent reviewer also inspected all 131 at 32 px across both themes and all styles. Its remaining findings led to another correction pass (Create, New Label, selection and forced-state cues).
- Final pass: complete 16 and 24 px proof pages visually inspected, before/after comparison checked, and final forced-metadata tile aligned with the other forced states. These are visual judgments, not a guarantee that every candidate suits every use.
- Every one of the 4,716 PNGs was decoded and checked for dimensions, visible pixels and transparency. All 786 SVG variants passed padded-canvas overflow detection (no significant stroke alpha outside the 16-unit canvas).
- Stable IDs, manifest asset paths, gallery JavaScript syntax and force/stall shape distinction checked. The gallery uses actual PNG exports for small-size proof and SVG for enlarged inspection; `?v=2` asset references avoid stale cache entries after a reload.

The browser automation tool refused access to the local file URL, so live browser interaction was not reverified. The visual reviews above used rendered local proof sheets. Runtime application integration and platform/DPI checks remain pending owner selection.

## Evidence and regeneration

- [Before / after](qa-comparison.png), at exact 16 px.
- `qa/16-01.png` through `qa/16-05.png`: all 131 symbols, all families, both themes.
- Equivalent five-page sets for 24 and 32 px.
- [Geometry results](geometry-audit.json) and [export results](validation.json).

Run `node build.cjs --sharp <sharp-path>`, `node check-geometry.cjs <sharp-path>` and `node proof-pages.cjs <sharp-path>`. `compare.cjs` renders the before/after proof from original commit `42f7cab`; it requires that commit in the local Git history. These are development helpers, with no new application dependency.

Some tile symbols deliberately use a simpler primary cue (for example Add uses a plus, Create a sparkle, recheck a check, Remote a monitor). The labelled gallery supplies context; the full unboxed symbols retain more context where it remains legible.
