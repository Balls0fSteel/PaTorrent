# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-10-26_143727_1793021847: Revision 6 UI icons per D30: globe network base, arcs-above pylon

- The owner chose option (a) for trackers and confirmed the globe fallback (D30):
  - **Network trio:** a globe with ✓, ! and × badges.
  - **Tracker states and reannounce:** a pylon in x 1–8, with two curved broadcast arcs above its apex and a solid splayed Λ (no neck, no crossbar), keeping the 5 × 5 badges.
  - **Trackers tab:** redrawn as the same pylon at full width.
- Validation: build, geometry check (no overflow, margin failures or unexpected identical masks; near matches unchanged) and proof pages pass. The revision 5 review scripts were rerun; the evidence is in `assets/ui-icon-explorations/qa/revision6/`. An independent review is still pending.

## 2026-10-26_124308_1793014988: Independent review of revision 5 UI icons

- An independent reviewer checked commit `16b14bf` with the revision 4 method. It also produced native-size strips, ASCII masks, mask neighbours and sketches. The geometry audit reproduced byte for byte.
- 123 of 131 pass. Add URL (chain link), both limits (speedometers), the refresh badge and all 11 toolbar icons now pass.
- Still failing (6 shipped icons):
  - Network trio: the closed ring with its detached arrowhead reads as an eye or a key, so D29's globe fallback applies.
  - Tracker states and reannounce: the 4 px A-frame reads as "A✓", "Ac", "Ax", "A…".
- The review also corrected the artist's notes: the globe was never drawn in the repository; the tower is not the tab tower narrowed, and its arcs are asymmetric. The revision 5 commit is `16b14bf`; it was misquoted as `6b14bf0`. The README is corrected.
- Owner decision needed on the tracker base, because D29's narrowed tower cannot have splayed legs beside a 5 × 5 badge. Options:
  - (a) the reviewer's recommendation: a pylon in x 1–8 with arcs above its apex, and the Trackers tab redrawn to match;
  - (b) keep side arcs and give tracker states a smaller badge;
  - (c) a non-tower base.
- Report and evidence: `assets/ui-icon-explorations/review-2026-10-26/revision5/`.

## 2026-10-26_120258_1793012578: Revision 5 UI icons per D29: closed port, narrowed tower, chain link

- The owner accepted the review's recommendations (D29). The redraws:
  - **Network trio:** an arrow piercing a closed ring, with the head inside; the globe fallback was not needed.
  - **Tracker states and reannounce:** the Trackers-tab tower narrowed (two arcs a side clear of the mast, A-frame lattice), with 5 × 5 badges.
  - **Refresh badge:** a small open-C copy of the refresh action, with a two-row gap.
  - **Add URL:** a chain link.
  - **Download/upload limits:** a speedometer with a small arrow badge.
  - The "…" badge stays inside the badge area.
- Validation: build, geometry check (no overflow, margin failures or unexpected identical masks) and proof pages pass. The reviewer's sheet scripts were rerun; 20 px is clean. The only new near match is download/upload-limit (0.93), a direction-modifier pair. Comparisons are in `assets/ui-icon-explorations/qa/revision5/`. An independent review of revision 5 is still pending.

## 2026-10-26_110747_1793009267: Independent review of revision 4 UI icons

- A reviewer working independently of the artist inspected all 131 symbols. It used the revision 3 sheets, new revision 3/4 side-by-side zooms, family sheets, mask neighbours and badge clearance. The geometry audit reproduced exactly: no overflow, margin or unexpected identical masks; median partial alpha 22.7 %.
- 120 of 131 pass. 9 of the 18 revision 3 failures are fixed (trackers tab, allocating, proxy, dht, logger, inactive, reset-columns, association, all-torrents), plus the reload-ip-filter shield.
- Still failing:
  - The network trio: the open ring plus the corner badge reads "→?".
  - The four tracker states and reannounce: the compact tower reads as a figure.
  - The refresh badge: its 1 px gap closes it into an "e".
  - add-url: the tilted magnet reads as "Ć".
  - download/upload-limit: the gauge reads as "∩".
- Of those, the shipped network pair, the four tracker states and Add URL are the same 7 shipped icons as last time. Accepted deviations: the 1 px refresh stroke, allocating as a bottom fill and the pieces redraw. Rejected: the arrowhead in the ring's mouth and the tilted magnet.
- Report, sheets and scripts are in `assets/ui-icon-explorations/review-2026-10-26/revision4/`. Three owner decisions are open: the network base, the compact tracker base and chain link vs magnet for Add URL.

## 2026-10-26_105105_1793008265: Revision 4 UI icons: network port, radio tower and 16 more redrawn

- Redrew the 18 symbols the revision 3 review failed, plus the refresh badge, from the revision 4 brief (D28), all as pixel art on the 16 px grid:
  - **Network trio:** an arrow into an open port with ✓, ! and × badges.
  - **Trackers:** one radio-tower base for the Trackers tab (two arcs a side) and the four states (✓, ↻, ×, …); reannounce = tracker-updating.
  - **Other 11:** each redrawn as its brief row asks, except where noted below.
- Also redrew pieces as a piece map, because its old drawing matched the new all-torrents grid.
- Deviations from the brief, all for legibility at 16 px:
  - The refresh badge uses a 1 px ring and a solid ◥ arrowhead; at 2 px it read as "C" or "₵".
  - The network arrowhead sits in the ring's mouth, not at its centre.
  - The Add URL magnet is tilted 45°; upright with split caps it read as "Ü".
  - Allocating is a drive filled to half its height; the half-width bar looked like a toggle.
- Shared primitives are `H.px`, `H.refreshBadge`, `H.tower`/`H.towerFull` and `H.reannounce`; `H.px` emits shared-edge paths so 20 px has no gaps. Removed `completed`/`tracker-working` from the reuse allowlist.
- Validation: the build, geometry check (no overflow, margin failures or unexpected identical masks), proof pages and both revision 3 review scripts pass. The shipped 16/20/24/32 px sheet was inspected; an independent re-review is still pending. Moved the two oldest root changelog entries out: they were already archived verbatim.

## 2026-10-26_101801_1793006281: Network/tracker icon decision and revision 4 brief

- Owner decided (D28): network icons use an arrow entering a port/ring; the Trackers tab and all tracker states use a radio tower. Added `assets/ui-icon-explorations/review-2026-10-26/revision3/REGENERATION-BRIEF.md` for the generator: a shared 16 px refresh badge, both new bases with per-state badges, the allowlist change (completed/tracker-working removed), the other 11 redraws and the checks to run. No artwork changed yet.

## 2026-10-26_101510_1793006110: Review of revision 3 bold UI artwork

- Re-inspected all 131 symbols at 64/24/16 px in light, dark and mono, the 24 shipped state icons and 11 toolbar icons at native 16/20/24/32 px, and all-pair mono masks. Confirmed: only the 8 allowlisted pairs are identical; median partial alpha at 16 px is 20 %; most first-review defects are fixed.
- Still to redo (18 symbols): network-ok/no-incoming/offline (plug base reads as a fork), the four tracker states (no shared base; not-contacted unreadable), the 16 px refresh badge, trackers ("(i)" reads as info), allocating, proxy, dht, logger, inactive, reset-columns, add-url, association, all-torrents and download-limit. The first report's runtime-icon count was wrong: 24 in total. Report, sheets and scripts are in `assets/ui-icon-explorations/review-2026-10-26/revision3/`.

## 2026-10-26_100209_1793005329: Generate the full bold UI artwork with native-size proofs

- Owner approved the proposed single-bold, full-set direction (D27). Added editable drawings for all 131 stable IDs, separate larger compound variants, per-size fitted SVGs and 2,358 PNGs across six sizes and light/dark/monochrome. About follows the current Verdigris identity; paused/stopped colours follow activity, and completion remains a check modifier.
- Replaced broken/ambiguous compounds and unrelated shared silhouettes; explicit reuse is limited to eight documented pairs. Updated the gallery with native-size SVG downloads and monochrome review, complete proof sheets, the 24-current-symbol runtime map and a reproducible handoff ZIP. Historical outline/tile explorations remain outside the active manifest.
- Validation: all PNGs decoded; 786 padded monochrome size checks have no overflow/margin failures; all-pair 16/32 px masks have no unexpected duplicates. Full visual review plus independent review prompted final Offline, clock and completed-Stop fixes. Median partial alpha at 16 px is 27% (earlier bold audit: 55%). GTK sizing proof inspected; gallery size/theme/path/filter checks and ZIP integrity passed. Application integration and live platform/DPI checks remain open. No runtime code or existing app selection changed.

## 2026-10-26_093102_1793003462: Review of revision 2 UI icon candidates

- Inspected all 131 symbols × 3 families at 64 px and real 16 px on light/dark, and compared every pair's 32 px mask. 47 symbols need regeneration: malformed drawings (wanted-complete, new/remove-label, language, offline, add-url, rss-rule, forced-state arrows), weak metaphors (about, proxy, bittorrent, rss-history, limits, reset-columns), 19 pixel-identical shape pairs that differ only by colour (queue-up = seeding = upload, error = warning, reannounce = refresh, …), generic tile substitutes, and no pixel-grid fitting (71 % of outline 16 px pixels are semi-transparent). QA.md's earlier pass had missed these.
- Added `assets/ui-icon-explorations/review-2026-10-26/`: report with per-icon fixes, owner decisions and a regeneration spec (pixel-fitted 16 px SVGs, shared badge system, no shared silhouettes), labelled review sheets and their Pillow script. App concept 08 resembles the Edge logo and 03 Ubuntu's mark. No artwork or runtime code changed.

## 2026-10-26_092453_1793003093: Redraw unclear small-size UI icon candidates after full visual audit

- The owner found bad symbols, notably Add Torrent. Format-only validation had missed visual collisions: heavy shared strokes and 75% tile scaling collapsed interior details. Redrew 77 semantic symbols, separated page/action badges, simplified dense geometry, corrected filled bars/dots and made forced/stalled states distinct. Regenerated all 786 SVGs and 4,716 PNGs; tile interior now uses 90% scale.
- Added exact PNG proof to the gallery, a revised-only filter and revisioned asset links; retained stable IDs and shortlist keys. Added paginated 16/24/32 px proof sheets, a before/after comparison, geometry checker and QA notes. No runtime application artwork selection changed.
- Validation: full 16/24 px visual review, independent full 16/32 px audit and follow-up corrections; every PNG decoded for size/ink/alpha; all 786 variants pass padded-canvas clipping checks. Manifest paths/IDs, gallery syntax and force/stall distinction passed. Live browser automation refused the file URL; rendered proof sheets supplied visual evidence. Platform/DPI checks remain pending selection.

## 2026-10-26_084833_1793000913: Broad small-size UI icon exploration batch

- Added `assets/ui-icon-explorations/`: 131 UI symbols in three styles (bold color, strong outline, contrast tile), with light/dark variants: 786 original SVGs and 4,716 transparent PNGs at 16/20/24/32/48/64 px. Covers toolbar, torrent/tracker/network states, actions, navigation, detail tabs, settings, RSS and files.
- Added an offline gallery with actual-size comparison, theme/size controls, search, UI-area filters, favorites and JSON shortlist export; full and excerpted 16 px proof sheets; reproducible export source and manifests. Added a built-in image_gen sheet of 12 new application-mark concepts and its exact prompt. The current application artwork remains selected pending owner choice (gate 10).
- Validation: decoded every PNG and verified dimensions, visible pixels and transparency; checked every manifest path and unique ID; gallery JavaScript syntax passed; visually inspected the 16 px excerpt on both backgrounds. Production integration, full platform/DPI checks and owner selection remain pending. No runtime code changed; application tests were unnecessary.

## 2026-10-23_161421_1792764861: Compact the active plan and changelog; preserve dated Markdown history

### Changed
- Preserved both original files verbatim under `docs/archive/`, with an index and their date/timestamp coverage. The two newest development entries remain below for the current handoff.
- Replaced the accumulated phase history in `PLAN.md` with current status, next steps, all 14 original 1.0 gate numbers, open choices and V2.0/future goals. Moved active decision rationale (D1-D26) to `docs/DECISIONS.md` and unresolved detailed follow-ups to topic sections in `docs/BACKLOG.md`; reconciled completed work that older notes still described as missing.
- Added repository instructions to keep root documents around 16 KiB / 200 lines, rotate complete older entries into uniquely named Markdown archives, and read history only when needed. Markdown keeps history searchable and diffable without decompression.

### Verified
- Compared the snapshots with the pre-cleanup files, checked decision IDs and retained entry content, and reviewed the active plan/backlog for unresolved work. Checked active-document local links and Git whitespace. Documentation only; application tests were not needed.
