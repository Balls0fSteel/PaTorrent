# UI icon review — revision 2 (2026-10-26)

Review of `assets/ui-icon-explorations/` after the "Revision 2" redraw (commit `deb8fb9`): 131 symbols × 3 families (bold, outline, tile) × light/dark. Every icon was inspected at 64 px and at real 16 px on both backgrounds, plus a pixel-level 16 px zoom. Exact shape duplicates were confirmed by comparing the 32 px masks of every pair within each family, not by eye.

Evidence in this folder:

- `sheet-01.png` … `sheet-11.png`: every icon, numbered as below. Each family column shows 64 px light, 64 px dark, 16 px ×3 (nearest-neighbour) light and dark, then actual 16 px light and dark.
- `zoom-16px.png`: 16 px at 12× for pause, add-torrent, language, force-downloading, tray, wanted-complete, new-label, offline (rows: bold, outline, tile).
- `review-sheets.py`: regenerates the sheets: `python -I review-sheets.py .. <out-dir>` (Pillow).

## Verdict

**47 of the 131 symbols need regeneration in at least one family**. The README/QA claim that "shapes remain the primary cue" does not hold: 19 pairs are pixel-identical shapes that differ only by colour, 18 symbols are malformed or unreadable at 16 px, and the tile family replaces many metaphors with a generic glyph (plain plus, plain X, plain check, plain arrow) that collides with other icons.

What is good and can stay as is: start, stop, queue-up/down/top, copy, save, search, filter, all-torrents, completed, labels, rss, statistics, speed, general, files, logger, ratings, connection, scheduler, appearance, privacy, notifications, power, dht, lock, unlock, stalled-download, stalled-seed, no-peers, priority high/normal/low, skip, file-audio, file-video, help, close, menu, back, expand, collapse, select-all, authentication, random-port, unlimited, more, logout, tray. (Several of these still take part in a collision below; the fix is usually on the other icon of the pair.)

## 1. Systemic problems (affect the whole set)

1. **No pixel-grid fitting at 16 px.** The 16 px PNGs are plain downscales of the vectors. In the outline family a median **71 %** of visible pixels are semi-transparent (bold 55 %); `menu`, `priority-normal`, `remove` and `copy` are 94–100 % partial pixels — every line straddles two pixel rows and renders as a two-pixel grey smear (see `zoom-16px.png`). 16 px is the Win32 size at 100 % DPI, so this is the most important size.
2. **Tile family is the weakest.** The tile's rounded square has a semi-transparent outer ring at 16 px (fuzzy edge), and the white marks are antialiased into pale tints (the add-torrent plus renders mint, not white). Only ~10–11 px are left for the glyph, so compound symbols were replaced by generic ones that lose meaning (see section 3).
3. **Colour-only distinctions.** Pairs below that differ only by colour fail for colour-blind users, monochrome/high-contrast themes and the outline family (which is single-colour by design, so they become fully identical there).
4. **Inconsistent metaphors between families.** Properties (document vs bullet list), preferences (gear vs sliders), downloading-metadata (hook vs list), allocating (box+plus vs drawer+minus), reload-ip-filter (shield+arrow vs shield+minus), remove-label (different tag orientation). A family switch should change the style, never the meaning.
5. **Inconsistent state colours.** paused-incomplete is amber but paused-complete is blue; stopped-incomplete is grey but stopped-complete is green. Decide whether colour encodes *activity* (paused = amber, stopped = grey) or *completeness*, and apply it to all states.
6. **Outline family is not "quieter".** Its stroke weight is close to the bold family; its description promises square joins but it uses round joins. Either make it a real 1.5 px-line family or drop it.

## 2. Malformed or unreadable drawings — must be redrawn

| # | Icon | Families | Problem | Regenerate as |
|---|---|---|---|---|
| 90 | wanted-complete | all | Folder outline is distorted: the check mark replaces the bottom-right corner and the folder's right edge bends diagonally into it. Reads as a broken shape. | Intact folder silhouette + separate check badge in the bottom-right corner, with a 1 px gap (knockout) between them. |
| 121 | new-label | all | Tag outline is truncated: the plus replaces the tag's tip, leaving an unrecognisable bent line (clear in `zoom-16px.png`). | Complete small tag (rotated 45°, with hole) top-left + separate plus badge bottom-right, knocked out. |
| 122 | remove-label | all | Same truncated tag in bold/outline; tile uses a differently oriented tag. | Same tag as new-label + minus badge. Same orientation in all families. |
| 48 | language | all | "A文": the 文 is illegible at 16 and 20 px in every family; at 16 px both glyphs are noise. | Simple globe-free mark that survives 16 px: speech bubble with "A", or two overlapping speech bubbles; or a single bold "A" with a small "あ/文" only at ≥32 px. |
| 58 | offline | all | The slash cuts the Wi-Fi arcs into fragments; at 16 px it is an unreadable scribble. | Two arcs + dot (not three) with one diagonal slash that has a knockout gap; or a plug with slash; or a cloud with slash. |
| 2 | add-url | all | Chain link fills to a blob at 16 px and the plus collides with it. | Single simplified link (two rounded rectangles, 45°) with a large gap, plus badge knocked out; or a magnet (U shape) which also covers "magnet". |
| 30 | rss-rule | all | Curved arrow is drawn on top of the RSS arcs; mush at 16 px. | RSS dot+arcs in the top-left two-thirds, with a small funnel or check badge bottom-right, separated. |
| 123 | add-feed | bold, outline | Plus overlaps the arcs; mush at 16 px. | RSS mark + separated plus badge (same badge system as new-label/add-torrent). |
| 70, 71, 73 | force-downloading, force-seeding, force-downloading-metadata | all | The two arrowheads touch: at 16 px they merge into a comb ("W" / "M") and no longer read as arrows. QA.md claims this was fixed. | One arrow + a lightning bolt / "!" badge, or a single arrow with a double head stacked vertically (⇊ stacked, not side-by-side). Must stay distinct from stalled (arrow + "!"). |
| 72 | downloading-metadata | bold, outline | The hook glyph ("⊐" over an arrow) has no meaning; tile uses list + arrow instead. Inconsistent with 73, which uses bar + arrows. | Small document/tag/"i" + down arrow, the same base used by 73 with the force cue added. |
| 81 | paused-complete | all | The check is crammed against the second pause bar and is ~3 px at 16 px; colour (blue) differs from paused-incomplete (amber). | Pause bars + check badge with knockout gap; colour per the state-colour decision (section 1.5). |
| 57 | network-no-incoming | tile | The bars degenerate to "‑ . ‑" dashes: reads as an emoticon "‑.‑!". | Same signal bars as network-ok with the tallest bar replaced by "!", drawn so each bar is at least 2 px wide. |
| 98 | file-image | tile | Picture reduced to a chevron in a frame: reads as eject/"up". | Frame + mountain + sun, simplified to one triangle and one dot. |
| 99 | file-archive | all | Zipper bar reads as a thermometer, door or USB stick; tile (two bars in a frame) reads as pause. | Box/crate with lid line, or document with zipper teeth along one side. |
| 53 | disk | all | OK at 64 px; at 16 px it resembles a credit card. Low priority. | Drive with an LED dot and two-tone front, or stacked-cylinder "database" cache. |

## 3. Wrong or weak metaphor

| # | Icon | Families | Problem | Regenerate as |
|---|---|---|---|---|
| 101 | about | all | A bare "P". The tile version is a parking sign. | The app mark (once chosen), or an "i" in the app-mark shape. |
| 52 | proxy | all | Pill/resistor shape; carries no meaning. | Three nodes in a row (client → relay → server), relay emphasised; or a shield-less "mask"; or a globe with a small server in front. |
| 43 | bittorrent | all | Four-dot X reads as close/cancel/molecule. | Swarm: central node linked to 3 peer nodes (triangle), clearly not an X. |
| 125 | rss-history | all | Pie chart: reads as statistics. | Clock with counter-clockwise arrow (same family as checking-resume-data) or RSS mark + small clock badge. |
| 116 | reload-ip-filter | all | Shield + right arrow (bold/outline) or shield + minus (tile): no "reload" cue. | Shield + circular arrow badge. |
| 117, 118 | download-limit, upload-limit | all | Arrow + three lines is the standard *sort* icon and is near-identical to sort-ascending/descending. No limit cue. | Arrow with a horizontal cap bar it stops at, or arrow + small gauge/speedometer. |
| 128 | reset-columns | all | Column grid without any reset cue. | Column grid + counter-clockwise arrow badge. |
| 89 | allocating | all | Box with plus reads as "add to inbox"; tile is a drawer with minus. | Disk/document with a partially filled level bar or dotted fill. |
| 46 | remote | all | Monitor with a play triangle reads as video/media player; tile is a plain monitor = desktop-integration. | Phone/tablet + signal arcs, or monitor + small globe/arcs. |
| 47 | web-ui | all | Generic page layout; nothing says "web". At 16 px the tile version blurs. | Browser window (title bar with 2–3 dots) + small globe. |
| 34 | trackers | all | Org chart: the bottom nodes become noise at 16 px and it reads as hierarchy. | Radio tower / beacon with arcs (announce), or a server with a peer list. |
| 3 | create-torrent | all | Page + sparkle is indistinguishable from add-torrent at 16 px; tile is a bare 4-point star (AI/"magic"/favourite). | Page + pencil, or page + hammer/wrench; tile must keep the page. |
| 5 | delete-data | tile | A plain X: identical to close and tracker-error. | Trash can + X badge, same as bold/outline. Bold/outline are 88 % similar to `remove` at 32 px; make the X badge larger/outside the can. |
| 14, 74 | recheck, checking-files | tile | A plain check: identical to completed and tracker-working. | Magnifier + check, as in bold/outline. |
| 130 | association | tile | A plain down arrow: identical to download. | Page with the app mark or a link/chain badge. |
| 123 | add-feed | tile | A plain plus: same shape as the add-torrent tile, colour only. | RSS mark + plus badge. |
| 18 | properties | tile | Bullet list (= queueing, rss-items) instead of the document used by bold/outline. | Same document/sliders metaphor in all families. |
| 40 | preferences | all | Bold/outline gear teeth smear to grey at 16 px; tile uses sliders, which is also the `advanced` icon. | Gear with 6 teeth and a large hole drawn for 16 px; keep sliders only for `advanced`. |
| 16 | open-folder | all | A plain closed folder, pixel-identical to `folder`; `directories` is almost the same. | Open folder (tilted front flap) or folder + out-arrow. |
| 13 | queue-bottom | all | Arrow down onto a bar is the standard download/save-to-disk glyph. | Keep the bar *under* the arrow but add list lines, or use chevron + bar (pairs with queue-top). |
| 85 | error | all | Same triangle as `warning` (colour only). | Error = circle or octagon with "!" or "×"; keep the triangle for warning. |
| 56 | network-ok | all | Same rising bars as `statistics` (4 % pixel difference). | Globe/plug with check, or keep bars for network and give statistics a line or pie chart. |

## 4. Shape collisions (identical silhouettes)

Pixel-identical within a family (confirmed by mask comparison). Mark each pair **OK (intended reuse)** or **fix**:

| Pair | Where both appear | Recommendation |
|---|---|---|
| queue-up = upload = seeding | toolbar action vs list state vs status bar | **Fix**: give queue actions a list/bar cue (queue-up = arrow + list lines, or chevron); keep plain arrows for transfer. |
| queue-down = download = downloading | same | **Fix**, as above. |
| reannounce = refresh = tracker-updating | context menu, toolbar, trackers tab | **Fix** reannounce (tower/megaphone + arrow); tracker-updating may reuse reannounce. |
| error = warning | list states, dialogs | **Fix** (see section 3). |
| tracker-error = close (+ delete-data tile) | trackers tab vs window chrome | Fix tracker-error: circle with ×, or tower + × badge. |
| inactive = tracker-not-contacted; ≈ skip | sidebar, trackers tab, file priority | Fix at least one; circle-minus vs circle-slash are near-identical at 16 px. |
| statistics ≈ network-ok | sidebar vs status bar | **Fix** (see section 3). |
| queueing = rss-items (= properties tile) | preferences vs RSS | Fix: queueing = numbered list (1/2/3) or stacked items with arrow. |
| desktop-integration = remote (tile) | preferences | Fix remote. |
| sort-ascending ≈ upload-limit, sort-descending ≈ download-limit | column header vs context menu | Fix the limits (section 3). |
| completed = tracker-working | sidebar vs trackers tab | OK (same meaning). |
| download = downloading, pause = paused-incomplete, stop = stopped-incomplete | action vs state | OK if intended (README says so). |
| recheck = checking-files | action vs state | OK. |
| folder = open-folder | file list vs context menu | Fix open-folder. |
| filter = rss-filters | colour only | OK. |

## 5. Application concept sheet (`app-concepts.png`)

It is a raster contact sheet only: there are no vector masters, no transparent per-size exports and no ICO, so none of the 12 can ship as is. Notes before choosing:

- **08 (wave)** closely resembles the Microsoft Edge logo; **03 (three dots on a broken ring)** resembles Ubuntu's "circle of friends". Avoid both (trademark confusion).
- **02 (two transfer arrows)** and **10 (circular exchange)** are the generic repeat/sync symbols; weak as a brand.
- **01** and **11** (P-based) work at small size and match the current About "P"; **05** (bolt-arrow) and **06** (sprout) are distinctive.

## 6. What I need from you to regenerate

### Decisions

1. **One family or three?** Regenerating three full families triples the work and the tile family has the most problems. Recommended: pick **bold** or **outline** as the UI family, and use tiles only for the list's state icons if you want them at all.
2. **Scope.** Either (a) only the 47 icons flagged here, or (b) a fresh full set to the spec below. Priority if partial: the 24 state + 4 tracker + 2 network icons (used by the app today via `exports/states`), then the 11 toolbar actions (currently runtime-drawn), then everything else.
3. **Intended reuse.** Confirm the "OK" rows in section 4, and decide the queue-vs-transfer arrow fix.
4. **State colour rule** (section 1.5): does colour encode activity or completeness?
5. **About/brand mark:** which app concept (or the provisional Verdigris) the About icon should use.

### Technical spec to give the generator

Use a vector/code tool (ChatGPT writing SVG, as `refine.cjs` does), **not** image generation: raster image models cannot produce pixel-exact 16 px icons. Ask for:

- SVG, `viewBox="0 0 16 16"`, **separate drawings for 16 px and for 24/32 px** (the 32 px export is also what GTK draws at 16 logical px, so it must not carry more detail than reads at 16).
- **Pixel-fitted 16 px:** horizontal/vertical strokes exactly 1 or 2 px wide, positioned on whole pixels (a 1 px stroke centred at x.5, a 2 px stroke at x.0); no diagonals thinner than 1.5 px; 1 px clear margin around the glyph (14 × 14 live area).
- **Badge system** for compound icons: base glyph in the top-left 11 × 11 area, badge (plus, minus, check, ×, clock, arrow) in a 7 × 7 bottom-right area, with a 1 px knockout gap cut from the base. Same badge geometry everywhere.
- **No two icons with the same silhouette** unless listed as intended reuse; every pair must stay distinguishable in pure black-and-white.
- Tiles (if kept): fully opaque square snapped to the pixel grid (no semi-transparent edge ring), glyph in pure white, inner area ≥ 12 × 12 px, same metaphor as the line family.
- Palette per meaning, used identically in every family; both light and dark variants; keep the existing IDs and file layout (`svg/{family}/{id}.svg`, `svg-dark/…`) so `build.cjs`, the gallery and shortlists keep working.

### Acceptance check after generation

Run `node build.cjs --sharp <sharp-path>`, then `python -I review-2026-10-26/review-sheets.py . <out>`, and review the 16 px columns. Each icon passes if it can be named correctly from the 16 px black-and-white version alone, and no two icons in the same UI area share a silhouette. I can run this check and re-audit the new set when you have it.
