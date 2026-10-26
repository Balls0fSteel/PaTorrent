# UI icon review — revision 4 (2026-10-26)

Third review, of commit `bef24b7` ("Redraw revision 4 UI icons: network port, radio tower and refresh badge"): 131 bold symbols, of which 22 changed. It checks the result against the [revision 3 review](../revision3/REPORT.md) and its [brief](../revision3/REGENERATION-BRIEF.md) (owner decision D28). The artist's claims in `README.md` and `QA.md` were checked, not assumed. The reviewer worked independently of the artist's session.

Evidence in this folder:

- `sheet-01.png` … `sheet-05.png`: every icon, as in revision 3 (64/24 px, 16 px ×3 light/dark/mono, actual 16 px). Script: `review-sheets.py`.
- `shipped-sizes.png`: the 24 runtime state icons and 11 toolbar icons at native 16/20/24/32 px, light and dark, 2× nearest. Script: `shipped-sizes.py`.
- `compare-01.png` … `compare-03.png`: the 22 changed icons, revision 3 (`6e113a3`) next to revision 4 at 16 px mono ×10 with a pixel grid, plus native 16/20/24/32 px. `families.png`: the network trio, the tracker family, the refresh-badge users, the limits and the likely lookalikes side by side. Script: `compare-sheets.py`.
- `zoom-network.png`, `zoom-trackers.png`, `zoom-add-url-limits.png`: native 16/20/24/32/48 px ×3 in light, dark and mono. Script: `zoom-sizes.py`.
- `neighbours.py`: nearest monochrome-mask neighbours (IoU, alpha ≥ 128) and badge clearance of the changed icons.
- `proposals.png`: pixel sketches for the "Redraw as" column, including the ones that failed. Script: `proposals.py`.

## Verdict

**Half of the redraw works. 120 of 131 symbols pass; 11 still need work, plus the shared refresh badge. 7 of the 11 ship in the app, the same 7 as last time: network-ok, network-no-incoming, the four tracker states and the Add URL toolbar icon.** The other 4 are offline, reannounce, download-limit and upload-limit.

Measured checks agree with the artist. `node check-geometry.cjs` rewrote `geometry-audit.json` with identical content: 786 renders, zero overflow, zero margin failures, zero unexpected identical masks. The only near match is still normal/forced metadata (IoU 0.88). The median partial alpha at 16 px is **22.7 %** (audit and PNGs both), down from 26.7 % in the revision 3 audit. That matches the "27 % → 23 %" claim. The 20 % in the revision 3 report came from a different measurement. All 22 changed icons are fully opaque at 16–48 px.

## Verified fixed

- **9 of the 18 failed symbols now pass:**
  - trackers (34): a tower with two arcs a side. No "(i)" reading.
  - allocating (89): a box filled to half its height.
  - proxy (52): node – hollow relay – node. The middle node is clearly the largest.
  - dht (59): a ring of six nodes, with no frame.
  - logger (38): a ">_" terminal again.
  - inactive (26): the flattened pulse. It works as the partner of `active`.
  - reset-columns (128): a three-column table with a header bar. It no longer looks like pause.
  - association (130): a page with the P mark. It is clearly different from create-torrent.
  - all-torrents (24): a 2 × 2 tile grid, no longer like copy.
- reload-ip-filter (116) now has a clean, readable shield.
- **Allowlist.** completed/tracker-working was removed. The audit lists exactly 7 allowed pairs × 2 sizes, and reannounce/tracker-updating is identical as the brief asked.
- **Badge clearance.** Every badged icon that changed keeps at least one clear pixel between badge and base (Chebyshev distance ≥ 2, `neighbours.py`). No base is cut.
- **20 px.** The row gaps of the first attempt are gone. Edges are crisp at 20 and 24 px. Strokes alternate between 2 and 3 px, which is expected at ×1.25.
- **No new collisions.** No changed icon reaches IoU 0.70 with an unrelated icon. The closest are logger ~ file-video 0.74 and logger ~ disk 0.72, but the ">_", "▶" and slot contents tell them apart. all-torrents ~ moving is 0.65, and pieces is ≤ 0.52 to anything.

## Brief compliance and deviations

| Deviation | Decision | Why |
|---|---|---|
| Refresh badge ring 1 px instead of 2 px | **Accept the 1 px stroke, reject the gap** | At a 6 × 7 px size, a 2 px stroke leaves a 2 px hole, so the "C/₵" claim is plausible. But the brief asked for a quarter of the ring missing. The drawn opening is a single pixel (x 14, y 11) right under the arrowhead. At 16 px the badge reads as a closed "e" or "G", with no direction. It also starts at y 8, outside the brief's badge area. That is harmless, because the 1 px gap holds. |
| Arrowhead in the ring's mouth, not its centre | **Reject (the result)** | It is true that a head inside the 4 px hole would merge with the ring. To fit the head in the mouth, though, the ring lost its whole left side (a 4 px opening on an 8 px ring), so it became a "⊃" hook. With a badge under its right stroke, the base reads as **"→?"**. That is clearest with "!" (57) and still clear with ✓ (56). |
| Tilted horseshoe magnet | **Reject** | Upright split caps probably did read as "Ü". The tilt has not fixed it. The outer arm is 3 px thick, but the inner arm is a 2–3 px stub. Both caps float away as separate dashes. At 16 px it reads as "Ć", "C" with accents, or "6". It does not read as a magnet. |
| Allocating as a bottom-half fill | **Accept** | This is what the brief asked for ("level bar filled about halfway"). It reads as a half-full drive or container. It does not read as a disk in particular, which is fine. |
| Pieces redrawn though not in the brief | **Accept** | The old 3 + 1 tile drawing would have looked like the new all-torrents grid. The new 3 × 3 piece map has IoU ≤ 0.52 with everything. The hollow cells are only 1 px holes, but the grid reads by itself. |

Also off-brief:

- The "…" badge of tracker-not-contacted runs from x 7 to x 14, under the tower's right leg instead of inside the badge area. It reads as a dotted ground line. It is also the `more` (127) glyph. Minor.
- Several icons lost their `large` variant. 24/32/48/64 px now upscale the 16 px pixel art. That is acceptable at 24/32 px. At 48/64 px they are blocky (add-url and the tracker family look crude). This is acceptable only while 48/64 px do not ship, as the README says.

## Still wrong

### Shipped icons (priority)

| # | Icon | Problem | Redraw as |
|---|---|---|---|
| 56, 57 (+58) | network-ok, network-no-incoming (and offline) | The ring lost its whole left side to make room for the arrowhead, so it is a "⊃" hook. A badge under its right stroke completes a question mark, so the base reads "→?" (57 reads "→?!"). The trio is consistent, but as a set of "?" marks. | Keep the ring a ring. Its only opening should be the mouth the shaft enters (≤ 3 px). Put the arrowhead inside a 1 px ring with ≥ 1 px clearance (`proposals.png`, "port sketch B"), or decide on another base (see below). In the sketches, a 2 px ring with a 2 px mouth still read "-?", and mirroring the layout read "C+". |
| 64–67 (+15) | tracker-working, -updating, -error, -not-contacted (and reannounce) | The family is consistent now: one base, four badges. But the compact tower reads as a figure or an insect at every size (`zoom-trackers.png`). The two arcs sit 1 px from the mast for 6 rows, so they read as arms. The hip bar and two legs read as a body standing. This is the "stick figure" the QA says it rejected. | The arcs must not run alongside the mast body. At most 3–4 rows next to the mast tip, ≥ 2 px clear of it. The mast continues below them for at least as long. Use a narrow Λ lattice without a hip bar. Three sketches still read as an insect, "P)" and "Ψ", so this probably needs an owner decision (below). The Trackers-tab tower (34) does read correctly. |
| — | refresh badge (65, 15, 116, 128) | It has a visible arrowhead now, which is better than revision 3's blob. But the gap is 1 px, so at 16 px it reads as a closed "e" or "G". At 20/24 px it is fine. | Same 1 px ring, opened like the full `refresh` (21): drop the bottom-right pixels so that two rows (y 11–12) are clear under the arrowhead. Rows from x 9, y 8: `..####`, `.#..##`, `#....#`, `#.....`, `#.....`, `.#..#.`, `..##..` ("refresh sketch 2px gap"). It reads as "C↻" at 16 px. |
| 2 | add-url | A tilted "C" with floating dashes: "Ć" or "6". The arms are unequal and the caps are detached (see deviations). | A single chain link at 45° (two 1 px loops interlocked), or an upright magnet with equal 2 px arms and caps of the same width. Owner choice below. The link sketch in `proposals.png` is readable but thin. |

### Other icons

| # | Icon | Problem | Redraw as |
|---|---|---|---|
| 117, 118 | download-limit, upload-limit | The 7 × 6 "gauge" has vertical sides and no base, so it reads as an arch "∩" or a letter "a", not a dial. The arrow dominates, so the icons read as download/upload plus a mark. | Turn it round. Make the gauge the base: a flat-bottomed half-dial about 12 × 8, with a 1 px arc, a solid base line and a needle from the base centre to the upper right. Put a small 4 × 4 ↓ or ↑ as the badge ("speed-limit sketch"). This reads as a speedometer at 16 px. |

Minor, optional:

- network-ok at 20 px: the check's right arm becomes a 1 px spur two rows tall.
- inactive is weak on its own (a tilde or hat) but fine next to active.
- pieces: the hollow cells are 1 px, so "missing pieces" is barely visible. The grid still reads.
- `geometry-audit.json` still says `"revision": 3`.

## What I need for the next pass

1. **Network base (affects 56–58).** The D28 port, drawn this way, turns into a "?" with the corner badge. Should the artist try a closed 1 px port with the arrowhead inside first, and fall back to a globe with the status badge (no arrow) if that still fails? I recommend yes to both.
2. **Compact tracker tower (affects 64–67, 15).** A symmetric tower in 9 px reads as a figure. Which is allowed? (a) The four states use the full Trackers-tab tower, pulled narrower, with a smaller 5 × 5 badge (x 10–14, y 10–14). (b) A different, non-tower tracker base. I recommend (a).
3. **Add URL.** Chain link or a third magnet attempt? I recommend the link. It is the usual "URL" glyph, and magnet links are URLs.
4. The refresh badge and the limits can follow the table; no decision needed.

The rules stay the same: native 16 px drawings, the shared badge system, a 1 px clear gap, and no new identical masks outside the allowlist. After regenerating, run `review-sheets.py`, `shipped-sizes.py` and `compare-sheets.py` from this folder. Judge the tracker and network families from `families.png` at actual size.
