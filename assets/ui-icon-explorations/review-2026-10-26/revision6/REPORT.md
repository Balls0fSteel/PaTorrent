# UI icon review — revision 6 (2026-10-26)

Fifth review, of commit `abd81380` ("Redraw revision 6 UI icons per D30: globe network base, arcs-above pylon"): 131 bold symbols, of which 9 changed. It checks the result against the [revision 5 review](../revision5/REPORT.md) and owner decision D30. The artist's claims in `README.md` ("Revision 6") and `QA.md` were checked, not assumed. The reviewer worked independently of the artist's session.

`git diff --stat 270cb987 abd81380` confirms that exactly the 9 claimed IDs changed (36 exports each): network-ok, network-no-incoming, offline, trackers, tracker-working, tracker-updating, tracker-error, tracker-not-contacted and reannounce. The only drawing-code changes are `H.tower` and `H.towerFull` (`common.cjs`), the `globe` base (`states.cjs`) and a comment (`misc.cjs`).

Evidence in this folder:

- `sheet-01.png` … `sheet-05.png`: every icon (64/24 px, 16 px ×3 light/dark/mono, actual 16 px). Script: `review-sheets.py`.
- `shipped-sizes.png`: the 24 runtime state icons and 11 toolbar icons at native 16/20/24/32 px, light and dark, 2× nearest. Script: `shipped-sizes.py`.
- `compare-01.png`, `compare-02.png`: the 9 changed icons, revision 5 (`16b14bf`) next to revision 6 at 16 px mono ×10 with a pixel grid, plus native 16/20/24/32 px. `families.png`: the network trio, the tracker family, the refresh-badge users, the limits and likely lookalikes. Script: `compare-sheets.py` (changed-ID list, default commit and the r5/r6 column labels updated).
- `zoom-network.png`, `zoom-trackers.png`: native 16/20/24/32/48 px ×10 in light, dark and mono. Script: `zoom-sizes.py`.
- `native.png`: the 9 changed icons at native 16/20/24/32 px ×3, light/dark/mono, one row each. Script: `native-strip.py`.
- `neighbours-16.txt`, `neighbours-32.txt`: nearest mask neighbours (IoU, alpha ≥ 128) and badge clearance. Script: `neighbours.py`.
- `ascii-16.txt`, `ascii-20.txt`: the masks as text, for exact pixel positions. Script: `ascii-masks.py`.
- `lookalikes.png`: the globe and the pylon next to the icons they could be confused with (web-ui, language, remote, connection, proxy, dht, peers, bittorrent; rss, add-feed, rss-items, notifications, warning, unlimited). Script: `lookalikes.py`.
- `size20.png`: the globe and pylon at 16 and 20 px, mono ×8 and native, to judge the ×1.25 rounding. Script: `size20.py`.
- `letter-test.png`: the tracker states at 16 px next to the revision 5 base and the strings "Ac", "Âc", "Ãx" and "Λ" in Segoe UI Bold 16 px. Script: `letter-test.py`.

## Verdict

**All 131 symbols pass; none still need work. All shipped icons pass: the 24 runtime state icons (including network-ok, network-no-incoming and the four tracker states) and the 11 toolbar icons.** The 8 failures of revision 5 are fixed, and the redrawn Trackers tab (34) still passes. The set is ready for integration. The notes below are optional.

Measured checks agree with the artist. `node check-geometry.cjs` rewrote `geometry-audit.json` with byte-identical content (no git diff against `abd81380`): 786 renders, zero overflow, zero margin failures, zero unexpected identical masks. Near matches ≥ 0.88 are unchanged: normal/forced metadata (0.88, 16 px) and download/upload-limit (0.93 at 16 and 32 px). The median partial alpha at 16 px is 22.7 %. Compared with revision 5, the audit changed only the ink counts of the 9 redrawn icons. It still says `"revision": 3`.

## Verified fixed

- **Network trio (56–58)** is now a globe: 10 × 9 (x 1–10, y 1–9), with a 1 px outline, a full-width equator at y 5, and a meridian at x 4 and x 7 that closes on x 5–6 at y 2 and y 8. No arrow is left. At 16 px it reads as a wireframe globe in light, dark and mono, with the ✓ (x 10–14, y 10–14), ! (x 12–13, y 10–14) or × (x 10–14, y 10–14) badge. It does not read as a crosshair or target: it has no centre cross and no inner ring. The eye, key and "play on a stick" readings of revision 5 are gone. Its nearest mask outside the trio is recheck at 0.40–0.44.
- **Tracker states (64–67) and reannounce (15)** use a pylon in x 1–8. Two arcs sit above the apex: an outer one (y 1–3, x 1–8) and an inner one (y 4–5, x 2–7). Below them is a 2 px apex (x 4–5, y 6), then a solid Λ with 2 px legs that splay to x 1 and x 8 at y 13. There is no crossbar and no neck, and the arcs are 1 px clear of the apex. The "A✓", "Ac", "Ax" and "A…" readings of revision 5 are gone. `letter-test.png` shows the difference: the old base is a block "A" with a bar; the new one is a mast with signal arcs, and the arcs fill 5 of its 13 rows, far more than any accent would. Its nearest mask outside the family is peers at 0.39–0.42.
- **Trackers tab (34)** is the same pylon drawn wide. The outer arc spans x 1–14 (y 1–5) and the inner arc x 5–10 (y 4–5). The apex is x 7–8 at y 6, and the legs splay to x 3 and x 12 at y 14. It reads as a broadcast tower at every size and is the clearest member of the family.
- **Badge clearance.** All 9 changed icons keep at least one clear pixel between badge and base (Chebyshev distance ≥ 2): network-ok 3, network-no-incoming 3, offline 2, and 2 for all four tracker states and reannounce. The Trackers tab has no badge.
- **No new collisions.** Outside the network trio and the tracker family, no changed icon reaches IoU 0.45 with anything. Inside the families the expected values hold: 0.80 within the trio, 0.73–0.80 among the tracker states, and 1.00 for reannounce = tracker-updating, which D28 intends.
- **Re-confirmed passes.** The other 122 icons are byte-identical to revision 5 in every export (the diff touches only the 9 IDs). So add-url, both limits, the refresh-badge users and all 11 toolbar icons still pass. A full look through `sheet-01` … `sheet-05` found nothing new.

## Owner decisions applied (D30)

| Decision | Implemented as decided? | Works? |
|---|---|---|
| Network ok / no-incoming / offline: a globe base with ✓, ! and × badges, no arrow | Yes. The 10 × 9 globe with outline, equator and lens meridian, the same three badges, and no arrow. | **Yes.** It reads as a globe or network at 16, 24 and 32 px in all three modes, not as a crosshair, target or eye. The three read as one set; only the badge differs. |
| Tracker states and reannounce: a pylon in x 1–8 with broadcast arcs above its apex and splayed legs | Yes. The base is in x 1–8, with two arcs above the apex and legs that splay from 4 px wide at y 7–8 to 8 px wide at y 11–13. | **Yes.** It reads as a radio or broadcast mast, not as a figure, an umbrella or the letter "A". The closest other reading is "a Wi-Fi sign over a mast", which still means "signal" (see notes). |
| Trackers tab (34) redrawn to match | Yes. The same construction at x 1–14: two arcs above a 2 px apex and a solid Λ. | **Yes.** In `families.png` the tab and the four states read as one family. The tab's outer arc is wider than its legs, so it reads a little more clearly as a tower than the narrow states. |

## Still wrong

Nothing fails. No shipped or non-shipped icon needs a redraw, and there are no regressions.

Minor, optional:

- **20 px.** The ×1.25 rounding hits these symmetric bases harder than the artist's "uneven by 1 px" suggests (`size20.png`, `ascii-20.txt`). The globe gets solid 5 × 2 caps (x 6–10, y 1–2 and y 11–12), a 2-row equator (y 6–7), and a 2 px left outline against a 1 px right one. So it looks denser, like a ball with a grid. The pylon's top arc becomes a 5 × 2 slab, and its legs are 3 px on the left and 2 px on the right. Both still read as the same symbols, so 20 px is acceptable under the rule accepted since revision 4. Hand-drawn 20 px versions of the globe and pylon would fix this if 20 px is used.
- **Globe at large sizes.** At 48/64 px (upscaled 16 px art), the amber network-no-incoming globe with its 2 × 2 holes can look like a waffle. The shipped sizes (16–32 px) are fine. This is the known limitation of upscaled pixel art.
- **Tracker states.** The narrow pylon's arcs are as wide as its legs, so at 16 px the states are a compact column. A viewer who expects text might still see an accented letter ("Âc"). `letter-test.png` shows a clear difference from real text, so I do not count it as a failure. For more margin later, the arcs could be widened past x 8: they sit at y 1–5, above the badge area, so the badge rule allows it. This is not required.
- **web-ui (47)** has its own smaller globe badge, drawn differently. It does not collide with the network trio (IoU ≤ 0.32), but the two globes are not the same drawing. Making them match is an optional consistency fix.
- **Refresh badge.** The revision 5 note still applies: dropping (13, 11) would taper the head. The difference is small.
- `geometry-audit.json` still says `"revision": 3`.

## Artist claims checked

- README "Revision 6" and the drawing code agree with the pixels: the 10 × 9 globe, the pylon in x 1–8, two curved arcs above the apex, a solid 2 px Λ with no neck and no crossbar, and the Trackers tab at x 1–14. All true.
- QA: "zero overflow, margin failures and unexpected identical masks; near matches unchanged (0.88, 0.93); median partial alpha 22.7 %". True: the rerun reproduced the committed audit byte for byte.
- QA: "`compare-sheets.py` against `16b14bf`" was rerun. True, but the copy in `qa/revision6/` was made with the revision 5 changed-ID list unchanged. Its columns are labelled "r4 (16b14bf)" and "r5". It shows 5 icons that did not change (reset-columns, reload-ip-filter, add-url, download-limit, upload-limit) and leaves out the Trackers tab (34), which did change. `families.png` and `native.png` in `qa/revision6/` are byte-identical to the ones in this folder.
- QA says `review-sheets.py` and `shipped-sizes.py` were rerun, but their output is not in `qa/revision6/`. The rejected candidates (bold ⊕ globe, flat-arc hat, neck figure, tall "I" mast, lattice "A" or house) have no images in the commit, so they cannot be checked. Neither point affects the verdict.
- QA: "At 20 px the globe and pylon edges are uneven by 1 px (×1.25)". This understates the effect a little; see the 20 px note above.

## Ready for integration

The set is ready for integration. All 24 runtime state icons and 11 toolbar icons pass at 16, 20, 24 and 32 px in light, dark and mono. This pass needs no owner decision. The minor notes above are optional, and none of them blocks shipping.

For any later change, the rules stay the same: native 16 px drawings, the shared badge system, a 1 px clear gap, and no new identical masks outside the allowlist. After regenerating, update the changed-ID list, then run `review-sheets.py`, `shipped-sizes.py`, `compare-sheets.py <dir> <out> abd81380` and `native-strip.py` from this folder.
