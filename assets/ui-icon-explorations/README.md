# PaTorrent bold UI artwork — revision 8

Open [review.html](review.html) to inspect the **139-symbol bold set** at native 16/20/24/32/48/64 px. The gallery supports light/dark artwork, monochrome proofs, UI-area/search filters and shortlist export. The larger inspection image uses the separately fitted 48 px SVG. SVG downloads match the selected size.

## Design

- One bold family; tiles and outline are no longer active generation targets.
- Native 16 px drawings and explicit larger variants for dense compound symbols. Each output size has its own fitted SVG; PNGs are rendered directly from that SVG, never downscaled from another PNG.
- Integer horizontal/vertical edges and 1/2 px small-size strokes; antialiasing is retained for diagonals and curves. A clear outer pixel remains at every export size.
- Compound symbols keep a complete base beside their modifier, with transparent space between them. Forced transfers use lightning, stalled transfers use an exclamation mark. Metadata states share the same page/down-arrow base.
- Colour describes operational state: downloads/metadata teal, seeding blue, checking/queueing/paused amber, stopped/inactive grey, errors red. Completion is a check shape; it never recolours paused or stopped variants.
- About is a flat symbolic rendition of the currently selected Verdigris medallion and P/down-arrow. The existing app/window/tray/packaging brand remains selected; the twelve earlier app concepts are a historical discussion sheet.
- `network-no-incoming` means no incoming connection has been observed; it is not a claim that the client is disconnected. The network trio shares one base, a globe (D30), with check, ! and × badges.

## Revision 8 (2026-10-28)

Adds the eight menu and category symbols the [icon coverage audit](../../docs/BACKLOG.md#ui-icon-coverage-audit-2026-10-28_090116_1793174476) found missing. All are drawn here (permissive sets were looked at for ideas only; nothing is copied): website, check-updates, notices and window as pixel art with 2 px strokes; import, edit and the two compact-base symbols as vector paths on the grid, with anti-aliased diagonals. No existing drawing changed: the cross badge moved to `H.crossBadge`, with identical output. The [revision 8 review](review-2026-10-28/revision8/REPORT.md) passed all eight; three of its optional refinements are applied (marked below).

- **website** (Help > Website): an arrow leaving an open frame through its top-right corner.
- **check-updates**: a download onto a line, with the shared refresh badge.
- **notices** (Third-Party Notices): a copyright mark, a C in a 14 px ring, beside Help's and About's circles. Refined: the C spans y 4–11, so its counter is 4 rows; with 2 it read as a heavy square at 20–32 px.
- **import** (File > Import): an arrow entering a bracket. Refined: the bracket is 14 rows like log out's, centred on the arrow.
- **window** (tray Show / Hide): a frame under a 4 px title bar with a cut-out button. A 3 px bar read as a plain frame at 16 px.
- **edit** (Edit Feed): a pencil, its eraser split off by one clear diagonal pixel (refined: the first gap let body and eraser touch at its ends).
- **feed-disabled**, **no-label**: the compact feed and the compact label tag with the cross badge.
- Check / Uncheck in the Add dialog reuse select-all / clear-selection.

## Revision 7 (2026-10-28)

Redraws what the owner's Win32 hand test (2026-10-27) flagged: mixed stroke widths on three toolbar symbols and an unreadable checking state. Every changed drawing is pixel art (`H.px`) with one 2 px stroke, so the 24 px fit is a uniform 3 px; the 1.5 px `large` page variant is gone.

- **Add torrent, Create torrent:** a 7 × 12 page outline in x 1–8 with a dog-ear, the top-right corner cut three pixels deep along a 2 px diagonal (a two-pixel cut read as the letter D). The old page had a solid 4 px fold and its 1.5 px large variant gave a 3 px bottom edge beside 2 px sides at 24 px. Create torrent shares the page, so it changed too.
- **Add URL:** an upright horseshoe magnet, the revision 4 review's alternative: a 7 px arch, equal 2 px arms and pole tips split off by a 1 px gap. The revision 5 chain link (D29) mixed 1 px loops with the 2 px bar and plus; the owner's finding named the symbol a magnet, so the magnet is back (D38).
- **Remove:** handle, lid, walls, three ribs and two slots are all 2 px, each centred on x 8 with an odd half-width so the 24 px fit stays centred. The old solid can had 1 px slots that fitted to 2 px and 1 px at 24 px, and its lid sat 1 px off centre.
- **Checking states:** one checking lens (`H.lens`): an 11 px ring with a 2 px rim at x 1–12, a 2 px handle to (14, 14) and a 7 px hole for a mark with a 1 px gap to the rim. Checking files (and Recheck, the allowed reuse pair) carry a tick, checking resume data carries "…" (a quick look, not a verified result). The queued pair is the list with a 6 × 6 lens badge or the "…" badge. The floppy and the 1 px clocks, which the owner saw as "a black floppy with an orange mark", are gone. Checking files and checking resume data are 0.88 similar, a modifier pair like metadata/forced metadata.

## Revision 6 (2026-10-26)

Redraws the two bases the [revision 5 review](review-2026-10-26/revision5/REPORT.md) failed, following D30.

- **Network** (ok, no-incoming, offline): D29's globe fallback, a 10 × 9 globe with an outline, an equator and a lens meridian. Both port versions failed: open it read as "?", closed it read as an eye or a key. A bolder globe with a straight meridian read as a ⊕ crosshair, so it was rejected.
- **Tracker states and reannounce:** a radio pylon in x 1–8. Two curved broadcast arcs sit above the apex, then a solid 2 px Λ with no neck and no crossbar. A neck under the arcs made a figure in a hat, and a crossbar made the letter "A".
- **Trackers tab:** the same pylon drawn wide (x 1–14), so the family shares one base.

## Revision 5 (2026-10-26)

Redraws what the [revision 4 review](review-2026-10-26/revision4/REPORT.md) failed, following the owner's choices in D29. The pixel-art method is unchanged.

- **Badges:** ✓, × and … are 5 × 5 at x 10–14, y 10–14. The refresh badge is 5 × 6 (y 9–14), ! is 2 × 5, the add badge 6 × 6 and the limit arrows 6 × 5. Below y 8, bases stay at x ≤ 8, so every badge keeps a 1 px gap.
- **Network:** a closed ring. The arrow's shaft pierces its left side and the head sits inside it, clear of the ring, so the open "?" hook is gone. A globe was only sketched outside the repository; it is not in the drawings. The [revision 5 review](review-2026-10-26/revision5/REPORT.md) found this base reads as an eye or a key, so the globe fallback is next.
- **Tracker states and reannounce:** a narrowed tower with two arcs a side and a tall mast on a 4 px A-frame. Correction: the arcs are 1 px clear of the mast on the left and 2 px on the right, and the legs are vertical, so this is not the Trackers-tab tower narrowed. The review found the A-frame reads as the letter "A".
- **Refresh badge:** a small copy of the full refresh action: an open C, a solid corner arrowhead and a two-row gap under it.
- **Add URL:** a chain link (two loops joined by a bar) with the plus badge.
- **Download/upload limits:** a speedometer base with a small ↓/↑ badge. Their masks are 0.93 similar because only the arrow differs; like normal/forced metadata, that is a modifier pair.
- The "…" badge now stays inside the badge area.

## Revision 4 (2026-10-26)

The 18 symbols failed by the [revision 3 review](review-2026-10-26/revision3/REPORT.md) were redrawn from its [brief](review-2026-10-26/revision3/REGENERATION-BRIEF.md). They are pixel art on the 16 px grid (`H.px` in `common.cjs`), emitted as absolute-coordinate paths so 20/24 px rounding never opens gaps between rows.

- **Network** (ok, no-incoming, offline; replaced in revision 5): an arrow whose head sits in the mouth of a ring open on its left. The head stops in the mouth rather than at the centre: a head inside the 4 px hole merged with the ring.
- **Trackers:** one radio tower with no top dot. The Trackers tab has two arcs a side; the four states use a compact tower with one arc a side and ✓, ↻, × or … badges. Reannounce = tracker-updating.
- **Refresh badge** (`H.refreshBadge`, used by four icons): an open ring with a solid ◥ arrowhead in the 6 × 7 corner. A 2 px stroke filled the ring into a "C" or "₵" at this size, so the ring is 1 px with doubled corners, like the full-size refresh action.
- Allocating is a drive filled to half its height; proxy is node–hollow relay–node; DHT is a ring of six nodes; logger is a ">_" terminal; inactive is the pulse flattened to one bump; reset-columns is a three-column table; reload-ip-filter keeps a shield.
- **Add URL** is a horseshoe magnet tilted 45° with split pole caps. Upright, the split caps read as "Ü" or a face.
- Association is a page with the P mark; all-torrents is a 2 × 2 grid of tiles. Pieces became a piece map (solid and hollow cells), because its old 3 + 1 tile drawing would have matched the new all-torrents.
- The download and upload limits are an arrow plus a speed gauge.
- Known limitation: the pixel-art symbols are blocky at 48/64 px. They keep the 16 px metaphor; dedicated large drawings can follow if those sizes ship.

## Files

| Path | Contents |
|---|---|
| `inventory.json` | Stable 139 IDs, labels and groups |
| `bold/{actions,states,misc}.cjs` | Editable small/large vector drawings |
| `bold/common.cjs`, `bold/index.cjs` | Shared primitives, About mark and explicit reuse allowlist |
| `bold/render.cjs` | Native-size SVG fitting and light/dark palette |
| `svg/bold/{id}.svg` | Native 16 px light SVG; stable original paths |
| `svg-size/bold/{size}/{id}.svg` | Separate fitted 20/24/32/48/64 px light drawings |
| `svg-dark`, `svg-dark-size` | Matching dark SVGs |
| `svg-mono`, `svg-mono-size` | Black monochrome SVGs |
| `png{,-dark,-mono}/bold/{size}/{id}.png` | Transparent native-size PNG exports |
| `manifest.json`, `manifest.js` | Active family, semantics, colours, reuse and runtime/toolbar lists |
| `preview.png` | Toolbar proof: native 16/24 px and nearest-neighbour zoom |
| `small-size-proof.png` | Complete actual-size 16 px contact sheet |
| `qa/revision3/all-01.png` … `all-06.png` | All symbols: native 16/24, light/dark/mono and 3× nearest zoom |
| `qa/revision3/runtime.png`, `gtk-logical.png` | The 24 shared runtime states and GTK 32-source-at-16-logical proof |
| `geometry-audit.json`, `validation.json`, `QA.md` | Measured checks, visual review and remaining platform gates |

**Active output: 2,502 SVGs + 2,502 PNGs** (139 × six sizes × three colour modes). Older outline/tile files, revision 2 proof pages and `refine.cjs`/`compare.cjs` remain for historical comparison and are not advertised by the active manifest. The dated [revision 2 report](review-2026-10-26/REPORT.md) explains the defects that prompted this redraw.

## Reproduce

The existing development-only Sharp library is the only Node dependency. No application dependency was added.

```text
node build.cjs --sharp <absolute-path-to-sharp>
node check-geometry.cjs <absolute-path-to-sharp>
node proof-pages.cjs <absolute-path-to-sharp>
```

To create the handoff ZIP, use the shared home virtual environment's Python:

```text
python package.py
```

The ZIP is written to `dist/artwork/patorrent-bold-revision3.zip` from the repository root.

## Intentional reuse

Only these pairs may have identical monochrome masks: upload/seeding, download/downloading, pause/paused-incomplete, stop/stopped-incomplete, recheck/checking-files, filter/RSS-filters, and reannounce/tracker-updating (both tower + refresh). Completed/tracker-working left the list in revision 4: tracker-working now has the tower base. Queue controls, limits, sort, network/statistics, tracker-error/Close, RSS-items/queueing/properties, and Folder/Open Folder have distinct drawings.

## Integration handoff

Integrated 2026-10-26_150634_1793023594: `crates/frontend/src/icons.rs` embeds `png{,-dark}/bold/` for the **24 runtime symbols (18 torrent + four tracker + two network)** at 16/20/24/32 px and the 11 toolbar symbols at 16/20/24/32/48 px, the lists `manifest.json` names (a test keeps them equal). Renaming, removing or resizing these exports breaks the application build. See `assets/patina-icons/INTEGRATION.md` for how each GUI uses them.

Still to hand-check: Windows DPI above 100 %, GTK light/dark and 32 px textures displayed at 16 logical px, and macOS Retina/menu-bar behaviour. Repository licensing applies; the handoff ZIP includes `LICENSE.md`.
