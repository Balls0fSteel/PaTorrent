# UI icon review — revision 3 (2026-10-26)

Second review, of commit `2d22173` ("pixel-fitted bold UI artwork"): one bold family, 131 symbols, separately fitted SVGs per size, light/dark/mono PNGs at 16–64 px. The first review is [../REPORT.md](../REPORT.md).

Evidence in this folder:

- `sheet-01.png` … `sheet-05.png`: every icon (numbered as in the manifest). Columns: 64 px light/dark, 24 px light/dark, 16 px ×3 nearest-neighbour in light, dark and mono, then actual 16 px light/dark. Script: `review-sheets.py`.
- `shipped-sizes.png`: the 24 runtime state icons and 11 toolbar icons at native 16/20/24/32 px (the Win32 sizes for 100/125/150/200 % DPI), light and dark, 2× nearest. Script: `shipped-sizes.py`.

## Verdict

**A large improvement. Most of the first report is resolved, but 18 symbols still need another pass, plus the refresh badge that reannounce and reload-ip-filter also use. 7 of them are icons the app ships: the network pair, the four tracker states and the Add URL toolbar icon.**

Verified fixed:

- **Duplicate shapes.** Only the 8 allowlisted action/state pairs have identical masks, at both 16 and 32 px (mono, alpha ≥ 128). Queue vs transfer, error vs warning, network vs statistics, tracker-error vs close, limits vs sort, folder vs open-folder and the list icons are all separated now.
- **Pixel fitting.** The median share of semi-transparent ink at 16 px fell from 55 % to **20 %**. Straight-edged icons are crisp, and the worst remaining icons are curve-heavy (rss, search, rss-history) where antialiasing is expected.
- **Redrawn as recommended:**
  - wanted-complete, new-label and remove-label now keep an intact base shape.
  - language is a speech bubble with "A"; file-image and file-archive are recognisable.
  - The forced states use a lightning bolt, metadata uses page + arrow, and error is an octagon with ×.
  - about uses the Verdigris P mark, and create-torrent is page + pencil.
  - rss-rule, add-feed and rss-history use a separate badge.
  - preferences is a gear that survives 16 px.
- **Colour.** One consistent rule for state colours, and completion is shown by the check shape only.
- **Sizes.** The shipped icons stay consistent across 16/20/24/32 px. 24 px and up switch to the more detailed drawing without changing the metaphor.

Correction to the first report: the app embeds **24** state icons in total (18 torrent + 4 tracker + 2 network), as `StateIcon::ALL` in `crates/frontend/src/icons.rs` shows. I wrote "24 state + 4 tracker + 2 network". The revision 3 README already corrects this.

## Still wrong

### Shipped icons (priority)

| # | Icon | Problem | Regenerate as |
|---|---|---|---|
| 56, 57 (+58) | network-ok, network-no-incoming (and offline) | The base is a two-prong plug seen head-on. At 16–20 px it reads as a fork, Ψ or tuning fork, not as a connection. These are the status-bar network icons, and offline shares the same base. | A shape that says "incoming connection": an arrow entering a port/ring, or a globe with an inward arrow. Keep the ✓ / ! / × badges. If you keep the plug, draw it in profile with a cable so the silhouette can't be a fork. |
| 64–67 | tracker-working, -updating, -error, -not-contacted | The family is inconsistent. working is a bare check with no base; updating and error use a solid "list page"; not-contacted is a hollow rectangle + ring, which reads as nothing. None of the bases says "tracker". | One shared tracker base on all four (the same one as the Trackers tab icon, see below), with the badges ✓, ↻, × and a neutral "–" or empty ring. |
| — | the ↻ refresh badge | At 16 px the circular-arrow badge is a 4 × 4 blob without a visible arrowhead (tracker-updating, reannounce, reset-columns, reload-ip-filter). | Draw the badge pixel-by-pixel at 16 px: an open ring with a 2 × 2 arrowhead and a visible gap. |

### Other icons

| # | Icon | Problem | Regenerate as |
|---|---|---|---|
| 34 | trackers | The "(i)" beacon on a stand reads as an info symbol, close to general/help. | Radio tower: a vertical mast with two arcs on each side and no dot on top, so it can't read as "i". Reuse it as the tracker base above. |
| 89 | allocating | A box with blocks inside; unreadable at every size. | Disk or document with a partly filled level bar, or a hatched/dotted fill. |
| 52 | proxy | Still a dumbbell / "|‑□‑|" with no meaning. | Three nodes in a row (client → relay → server) with the middle one emphasised, or a globe behind a small server. |
| 59 | dht | A square frame with corner nodes reads as a crop or selection tool. | A ring of 5–6 nodes, or a mesh of nodes with no frame. |
| 38 | logger | A box with blocks reads as a form or keyboard; revision 2's terminal (">_") was clearer. | Restore the terminal prompt, or a page with log lines and a timestamp dot. |
| 26 | inactive | A plain ring: looks like an empty radio button or the letter O. | A sleep/"z" mark, a pause-in-circle, or the dimmed pulse line of `active` crossed out. |
| 128 | reset-columns | The two-column base looks like a pause sign. | Three or more columns with a header bar (table), plus a fixed ↻ badge. |
| 2 | add-url | The magnet reads as the letter "U". | Magnet with visible pole caps (a 1 px gap across both tips), or a single link. |
| 130 | association | The link badge is a blob at 16 px, and the icon is 85 % mask-identical to create-torrent. | A larger, simpler badge (app P-mark or a single link ring), or a page with the P mark inside. |
| 24 | all-torrents | Stacked squares now look like `copy` (19). | Back to the 2 × 2 grid, which no longer collides since pieces is distinct. Or a stacked list. |
| 117 | download-limit | An arrow onto a bar is the classic download/save glyph and almost the same as the `tray` mark. | Arrow + a small gauge, or an arrow that stops under a cap bar *above* a line, distinct from the tray. |

Minor, optional: connection (an Ethernet plug in a box is a blob at 16 px), properties (page with ± reads as a calculator), disk (78 % mask-similar to file-video), and queued-download vs queueing (84 % similar, though they appear in different UI areas).

## What I need for the next pass

1. **The network metaphor.** Plug in profile, inward arrow, or globe? This decides network-ok, network-no-incoming and offline together.
2. **One tracker base** for the Trackers tab and all four tracker states. Radio tower recommended.
3. The rest can follow the "Regenerate as" column; no decision needed.

The same spec applies as before: native 16 px drawings, the shared badge system, and no new identical masks outside the allowlist. After regenerating, run `python -I review-2026-10-26/revision3/review-sheets.py . <out>` and `shipped-sizes.py`. I can re-run this review on the result.
