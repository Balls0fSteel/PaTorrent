# PaTorrent bold UI artwork — revision 6

Open [review.html](review.html) to inspect the **131-symbol bold set** at native 16/20/24/32/48/64 px. The gallery supports light/dark artwork, monochrome proofs, UI-area/search filters and shortlist export. The larger inspection image uses the separately fitted 48 px SVG. SVG downloads match the selected size.

## Design

- One bold family; tiles and outline are no longer active generation targets.
- Native 16 px drawings and explicit larger variants for dense compound symbols. Each output size has its own fitted SVG; PNGs are rendered directly from that SVG, never downscaled from another PNG.
- Integer horizontal/vertical edges and 1/2 px small-size strokes; antialiasing is retained for diagonals and curves. A clear outer pixel remains at every export size.
- Compound symbols keep a complete base beside their modifier, with transparent space between them. Forced transfers use lightning, stalled transfers use an exclamation mark. Metadata states share the same page/down-arrow base.
- Colour describes operational state: downloads/metadata teal, seeding blue, checking/queueing/paused amber, stopped/inactive grey, errors red. Completion is a check shape; it never recolours paused or stopped variants.
- About is a flat symbolic rendition of the currently selected Verdigris medallion and P/down-arrow. The existing app/window/tray/packaging brand remains selected; the twelve earlier app concepts are a historical discussion sheet.
- `network-no-incoming` means no incoming connection has been observed; it is not a claim that the client is disconnected. The network trio shares one base, a globe (D30), with check, ! and × badges.

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
| `inventory.json` | Stable 131 IDs, labels and groups |
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

**Active output: 2,358 SVGs + 2,358 PNGs** (131 × six sizes × three colour modes). Older outline/tile files, revision 2 proof pages and `refine.cjs`/`compare.cjs` remain for historical comparison and are not advertised by the active manifest. The dated [revision 2 report](review-2026-10-26/REPORT.md) explains the defects that prompted this redraw.

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

This change supplies artwork and review tools. The application still embeds the previous `assets/patina-icons/exports/states` images, draws the Win32 toolbar at runtime and uses GTK theme icons for toolbar actions. `manifest.json` identifies the **24 currently embedded symbols (18 torrent + four tracker + two network)** and the 11 toolbar symbols to integrate first. The report's earlier “24 state + four tracker + two network” count was incorrect.

After integration, hand-check Windows DPI, GTK light/dark and 32 px textures displayed at 16 logical px, and macOS Retina/menu-bar behaviour. Repository licensing applies; the handoff ZIP includes `LICENSE.md`.
