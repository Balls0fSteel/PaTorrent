# UI icon review — revision 5 (2026-10-26)

Fourth review, of commit `16b14bf` ("Redraw revision 5 UI icons per D29: closed port, narrowed tower, chain link"): 131 bold symbols, of which 13 changed. It checks the result against the [revision 4 review](../revision4/REPORT.md) and owner decision D29. The artist's claims in `README.md` ("Revision 5") and `QA.md` were checked, not assumed. The reviewer worked independently of the artist's session.

`git diff 3fe8899 16b14bf` confirms that exactly the 13 claimed IDs changed (36 exports each): network-ok, network-no-incoming, offline, tracker-working, tracker-updating, tracker-error, tracker-not-contacted, reannounce, reset-columns, reload-ip-filter, add-url, download-limit and upload-limit.

Evidence in this folder:

- `sheet-01.png` … `sheet-05.png`: every icon (64/24 px, 16 px ×3 light/dark/mono, actual 16 px). Script: `review-sheets.py`.
- `shipped-sizes.png`: the 24 runtime state icons and 11 toolbar icons at native 16/20/24/32 px, light and dark, 2× nearest. Script: `shipped-sizes.py`.
- `compare-01.png`, `compare-02.png`: the 13 changed icons, revision 4 (`3fe8899`) next to revision 5 at 16 px mono ×10 with a pixel grid, plus native 16/20/24/32 px. `families.png`: the network trio, the tracker family, the refresh-badge users, the limits and likely lookalikes. Script: `compare-sheets.py` (changed-ID list and default commit updated).
- `zoom-network.png`, `zoom-trackers.png`, `zoom-add-url-limits.png`: native 16/20/24/32/48 px ×10 in light, dark and mono. Script: `zoom-sizes.py`.
- `native-a.png`, `native-b.png`: the changed icons (and authentication) at native 16/20/24/32 px ×3, light/dark/mono, one row each. Script: `native-strip.py`.
- `neighbours-16.txt`, `neighbours-32.txt`: nearest mask neighbours (IoU, alpha ≥ 128) and badge clearance. Script: `neighbours.py`.
- `ascii-16.txt`, `ascii-20.txt`: the masks as text, for exact pixel positions. Script: `ascii-masks.py`.
- `proposals.png`: pixel sketches for the "Redraw as" column, including the ones that failed. Script: `proposals.py`.

## Verdict

**123 of 131 symbols pass; 8 still need work. 6 of the 8 ship in the app: network-ok, network-no-incoming and the four tracker states.** The other 2 are offline and reannounce, which share those bases. Add URL, the refresh badge and both limits now pass. All 11 toolbar icons pass.

Measured checks agree with the artist. `node check-geometry.cjs` rewrote `geometry-audit.json` with byte-identical content (no git diff): 786 renders, zero overflow, zero margin failures, zero unexpected identical masks. Near matches ≥ 0.88: normal/forced metadata (0.88, 16 px) and download/upload-limit (0.93 at 16 and 32 px). The median partial alpha at 16 px is 22.7 %. The audit still says `"revision": 3`.

## Verified fixed

- **Add URL (2)** is a horizontal chain link: two open loops (x 1–14, y 1–6) with a 2 px break top and bottom, and a 6 × 2 bar through the middle, plus the add badge. It reads as the usual "insert link" glyph at every size and in every mode. It sits well next to add-torrent in the toolbar. Its nearest mask is new-label at 0.52.
- **Limits (117, 118)** are now speedometers: a flat-based half dial (14 × 8), a needle from the base centre to the upper right, and a 6 × 5 ↓ or ↑ badge. They read as gauges at 16 px, and the badge direction is clear in mono at 16 px (shaft on top vs head on top). Colour differs as well (teal vs blue). Their 0.93 mask similarity is a modifier pair, like normal/forced metadata. Accept it.
- **Refresh badge (65, 15, 116, 128)** is open now. Rows 12–13 are clear under the head, so it no longer reads as a closed "e" or "G". It is a 5 × 6 copy of refresh (21) and gives the same impression: a "C" with a heavy top-right end. It works as a refresh mark in reset-columns and reload-ip-filter.
- **Network ring** is closed. The "?" hook and the "→?" reading of revision 4 are gone.
- **Tracker tower** no longer reads as a figure or insect: the arcs are separate strokes and the "arms" are gone. The "…" badge is now inside the badge area (x 10–14, y 13–14).
- **Badge clearance.** All 13 changed icons keep at least one clear pixel between badge and base (Chebyshev distance ≥ 2; network-ok 3, reset-columns 3, reload-ip-filter 4, add-url 3).
- **No new collisions.** Outside the network trio, the tracker family and the limits pair, no changed icon reaches IoU 0.70 with anything. The highest are add-url ~ new-label 0.52 and reload-ip-filter ~ new-label 0.48.
- network-ok at 20 px: the 1 px check spur of revision 4 is gone.

## Owner decisions applied (D29)

| Decision | Implemented as decided? | Works? |
|---|---|---|
| Network: closed port ring with the arrowhead inside | Yes. A 1 px ring (x 4–13, y 1–8), shaft x 1–6 through its left side, head inside. | **No.** The head is a 2-column triangle (x 8–9, y 3–6), and a 1 px gap (x 7) separates it from the shaft. So no arrow is visible. At 16 px the base reads as an eye, a "play" button on a stick, or a key. It is close to a mirror image of authentication (114); the mirrored, shifted mask IoU is 0.42. It cannot be named "network" from the 16 px mono version. **The globe fallback is needed.** |
| Network fallback: globe with the status badge, no arrow | Not drawn. The README says it was "drawn but not needed", but no globe is in `bold/*.cjs` or in the commit. | A 10 × 9 globe sketch fits the badge rules and reads as a globe at 16 px (`proposals.png`). |
| Tracker states and reannounce: the Trackers-tab tower narrowed, 5 × 5 badges | Partly. The badges are 5 × 5 (refresh 5 × 6). The base keeps two arcs a side, but it is not the Trackers-tab tower narrowed. The tab tower (34) has splayed legs. The state tower has a 4 px A-frame with vertical legs and a crossbar at y 9. | **No.** The lower half is a block letter "A". With the badges, the four states read **"A✓", "Ac", "Ax", "A…"** at every size from 16 to 64 px (`zoom-trackers.png`, `sheet-01/03`). The "Ac" of tracker-updating and reannounce is the clearest. |
| Add URL: single chain link with the plus badge | Yes (a two-loop link, the common glyph). | **Yes.** |

The badge rules cause the tower failure. A 5 × 5 badge at x 10–14, y 10–14 with a 1 px gap keeps the base at x ≤ 8 from y 9 down. Two arcs a side keep the mast centred near x 7. That leaves the legs 4 px wide (x 5–8), so they cannot splay. I tried four more bases on the same centre; all read as letters: no crossbar "n", X-lattice "8", mast and foot "1" or "L", one leaning leg still "A".

## Still wrong

### Shipped icons (priority)

| # | Icon | Problem | Redraw as |
|---|---|---|---|
| 56, 57 (+58) | network-ok, network-no-incoming (and offline) | The arrowhead is detached from the shaft and only 2 px wide. Inside the ring it reads as a pupil or a play triangle. The ring and shaft read as an eye, a key (a mirror image of 114) or a magnifier, not as an arrow entering a port. The trio is consistent, but its base is not identifiable. | Apply the D29 fallback: a globe with the same ✓, ! and × badges, no arrow. Sketch: a 10 × 9 globe at x 1–10, y 1–9 with a 1 px outline, an equator at y 5 and a lens meridian (x 4 and x 7, closing at x 5–6 on y 2 and y 8). It keeps x ≥ 9 only above y 9, so the 1 px badge gap holds. If the owner wants to keep the arrow instead, at least fill x 7 so the shaft and head form one arrow ("port, shaft joined" sketch). That reads as an arrow at ×10 but is still key-like at 16 px. |
| 64–67 (+15) | tracker-working, -updating, -error, -not-contacted (and reannounce) | The 4 px A-frame with vertical legs and a crossbar is a letter "A". With the badges it reads "A✓", "Ac", "Ax", "A…". It is also not the Trackers-tab (34) base, which has splayed legs. | Needs an owner decision, because D29's constraints cannot be met together (see above). Best sketch ("tower wifi-arcs Λ"): move the tower left into x 1–8. Put the broadcast arcs above the apex, not beside the mast: arcs at y 1–2 (x 2–7, ends x 1 and x 8) and y 3–4 (x 3–6, ends x 2 and x 7). Then a 2 px apex at x 4–5 (y 5–6) and splayed Λ legs to x 1 and x 8 at y 11–13, with no hip bar. Redraw the Trackers tab (34) the same way, wider, so the family keeps one base. It reads as a pylon, not a letter. The risk is a "Wi-Fi tower" reading. |

### Other icons

None. All non-shipped changed icons pass.

Minor, optional:

- Refresh badge: (13, 11) makes the head a 2 × 3 block. Dropping it tapers the head to a point ("refresh badge, tapered head"). The difference is small.
- Tower arcs: the README says they are "2 px clear of the mast tip". That holds on the right; on the left the inner arc at (4, 5) is only 1 px from the mast (x 6). Moot if the base is redrawn.
- README: "badges are 5 × 5 at x 10–14, y 10–14". That is true of ✓, × and …. The add badge is 6 × 6 (x 9–14, y 9–14), the limit arrows are 6 × 5 (x 9–14) and ! is 2 × 5. All keep the 1 px gap, so this is a documentation detail.
- 20 px: the limit dials and the tower legs have uneven 1/2 px sides from ×1.25 scaling. This was accepted in revision 4. 48/64 px are still upscaled 16 px pixel art (known limitation).
- `geometry-audit.json` still says `"revision": 3`.

## What I need for the next pass

1. **Network (56–58).** None. D29 already covers this case: the closed port fails at 16 px, so draw the globe fallback. Confirm only if you would rather keep the arrow with the shaft joined. I recommend the globe.
2. **Tracker base (64–67, 15, and 34).** A narrowed tower with two side arcs cannot have splayed legs next to a 5 × 5 bottom-right badge. Which is allowed? (a) The arcs-above Λ pylon in x 1–8 for the four states and reannounce, with the Trackers tab (34) redrawn to match. (b) Side arcs kept, with a smaller badge for the tracker states only. (c) A different, non-tower tracker base. I recommend (a).
3. Everything else is ready for integration: add-url, both limits, the refresh-badge users, and the 11 toolbar icons.

The rules stay the same: native 16 px drawings, the shared badge system, a 1 px clear gap, and no new identical masks outside the allowlist. After regenerating, run `review-sheets.py`, `shipped-sizes.py`, `compare-sheets.py <dir> <out> 16b14bf` and `native-strip.py` from this folder. Judge the network and tracker families at actual size in mono.
