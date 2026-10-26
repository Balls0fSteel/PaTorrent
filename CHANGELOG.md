# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-10-26_212009_1793049609: Release candidate 1.0.0-rc.1: version, notes, README and the tagged release path

- Version `1.0.0-rc.1` (Road to 1.0 item 14): `Cargo.toml`, `Cargo.lock`, peer ID `-PA1000-` (a pre-release carries the version it leads to; the test now strips the suffix), the Windows manifest `1.0.0.0` and the Flatpak metainfo's first `<release>`. `cargo metadata --locked` and the core version tests pass.
- `docs/releases/1.0.0-rc.1.md`: the release text (downloads per platform, first-run steps, what is in it, known limitations). The Release workflow now checks out the repository and uses `docs/releases/<version>.md` as the draft's notes when it exists, and marks a suffixed version as a pre-release. `docs/RELEASING.md` steps 1, 2, 4 and 5 record the manifest, the metainfo, the notes file and the pre-release tag.
- README: the warning now describes a release candidate, the status paragraph matches the plan (GTK, v2, languages, remote mode are in), and a Download table lists the release files per platform.
- Backlog: the Windows executables have no `VERSIONINFO` resource (Explorer shows no version); noted under Win32.
- Not in this commit: `tools/swarm/src/interop.rs` carries an uncommitted qBittorrent-log wait for `lt-swarm interop` (uTP-only runs); it is unrelated to the release and left for its own change.
- The tag `v1.0.0-rc.1` and the run's outcome are recorded in the next entry.

## 2026-10-26_150634_1793023594: Integrate the bold artwork: 24 state icons and the toolbar (gate 10)

- Both GUIs now use the revision 6 artwork. `crates/frontend/src/icons.rs` embeds the bold PNG exports from `assets/ui-icon-explorations/png{,-dark}/bold/`:
  - the 24 runtime state icons at 16/20/24/32 px;
  - the 11 toolbar images at 16/20/24/32/48 px.
  The runtime-drawn toolbar vectors are removed.
- **Win32:**
  - The toolbar slot is 24 px scaled to the nearest export: 24, 24, 32 and 48 px at 100, 125, 150 and 200 %. Before it was 22 px drawn at run time.
  - `toolbar_bgra` decodes the PNG for the normal and greyed image lists. A small in-tree reader handles 8-bit RGBA PNGs with core's existing inflater; no new dependency.
- **GTK:**
  - The toolbar uses the artwork (32 px drawn at 16 logical px) instead of desktop theme icons.
  - State and toolbar icons switch to the dark palette when the GTK theme is dark (prefer-dark setting or a theme name containing "dark") and follow theme changes.
- Tests check every embedded size and palette, PNG decoding, toolbar sizes, and that the icon lists match the artwork manifest's `runtime` and `toolbar` lists.
- Validation:
  - Win32 build, clippy and tests pass (frontend 174, Win32 59).
  - A portable debug run was captured: bold toolbar, greyed disabled buttons and the globe network icon at 100 % DPI.
- Not verified:
  - The GTK client compiles only on Unix and WSL is unavailable here. After the push, CI run 40336848086 built and tested it on Ubuntu, macOS, Windows and MSRV, all green; it has not been run on a desktop.
  - Non-100 % DPI, the GTK dark palette, the Trackers tab with real trackers and macOS are still hand checks.
- Moved the six oldest root entries, verbatim, to `docs/archive/CHANGELOG-2026-10-23--2026-10-26.md`.

## 2026-10-26_145432_1793022872: Independent review of revision 6 UI icons: all 131 pass

- An independent reviewer checked commit `abd81380` with the revision 5 method. It added lookalike, 20 px and letter-test sheets. The geometry audit reproduced byte for byte, and the other 122 icons are byte-identical to revision 5.
- **All 131 symbols pass**, including the 24 shipped runtime icons and 11 toolbar icons. The set is ready for integration.
  - The globe reads as a globe, not a crosshair or eye.
  - The pylon reads as a broadcast mast, not a figure or an "A".
  - The Trackers tab and the states read as one family.
  - Every badge keeps its gap, and nothing comes closer than IoU 0.45 outside its family.
- Optional notes:
  - 20 px rounding thickens the globe and pylon; hand-drawn 20 px versions would help if 20 px ships.
  - At 48/64 px the upscaled globe can look like a waffle.
  - The tracker arcs could widen past x 8.
  - web-ui's globe is a different drawing.
- The review corrected the artist's evidence: the `qa/revision6` comparison sheets used revision 5's ID list. They are replaced with the review's sheets, and QA.md is corrected. Report: `assets/ui-icon-explorations/review-2026-10-26/revision6/`.

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
