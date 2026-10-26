# PaTorrent bold UI artwork — revision 3

Open [review.html](review.html) to inspect the **131-symbol bold set** at native 16/20/24/32/48/64 px. The gallery supports light/dark artwork, monochrome proofs, UI-area/search filters and shortlist export. The larger inspection image uses the separately fitted 48 px SVG. SVG downloads match the selected size.

## Design

- One bold family; tiles and outline are no longer active generation targets.
- Native 16 px drawings and explicit larger variants for dense compound symbols. Each output size has its own fitted SVG; PNGs are rendered directly from that SVG, never downscaled from another PNG.
- Integer horizontal/vertical edges and 1/2 px small-size strokes; antialiasing is retained for diagonals and curves. A clear outer pixel remains at every export size.
- Compound symbols keep a complete base beside their modifier, with transparent space between them. Forced transfers use lightning, stalled transfers use an exclamation mark. Metadata states share the same page/down-arrow base.
- Colour describes operational state: downloads/metadata teal, seeding blue, checking/queueing/paused amber, stopped/inactive grey, errors red. Completion is a check shape; it never recolours paused or stopped variants.
- About is a flat symbolic rendition of the currently selected Verdigris medallion and P/down-arrow. The existing app/window/tray/packaging brand remains selected; the twelve earlier app concepts are a historical discussion sheet.
- `network-no-incoming` means no incoming connection has been observed; it is not a claim that the client is disconnected. Offline uses an intact plug plus a cross.

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

Only these pairs may have identical monochrome masks: upload/seeding, download/downloading, pause/paused-incomplete, stop/stopped-incomplete, recheck/checking-files, completed/tracker-working, filter/RSS-filters, and reannounce/tracker-updating. Queue controls, limits, sort, network/statistics, tracker-error/Close, RSS-items/queueing/properties, and Folder/Open Folder have distinct drawings.

## Integration handoff

This change supplies artwork and review tools. The application still embeds the previous `assets/patina-icons/exports/states` images, draws the Win32 toolbar at runtime and uses GTK theme icons for toolbar actions. `manifest.json` identifies the **24 currently embedded symbols (18 torrent + four tracker + two network)** and the 11 toolbar symbols to integrate first. The report's earlier “24 state + four tracker + two network” count was incorrect.

After integration, hand-check Windows DPI, GTK light/dark and 32 px textures displayed at 16 logical px, and macOS Retina/menu-bar behaviour. Repository licensing applies; the handoff ZIP includes `LICENSE.md`.
