# UI icon review — revision 8 (2026-10-28)

Sixth review, of commit `0001f3b1` ("Artwork revision 8: the last missing menu icons (1.0.7)"): 139 bold symbols, 8 of them new. It checks the new symbols against the rules in `README.md` ("Design" and "Revision 8"). The artist's claims were checked against the drawings and the pixels, not assumed. The reviewer worked independently of the artist's session.

The 8 new IDs are website, check-updates, notices, import, window, edit, feed-disabled and no-label. They are drawn in `bold/misc.cjs` and use `H.crossBadge` (`common.cjs`). The `px` row strings in the code match the 16 px masks in `ascii-16.txt` exactly, so the committed PNGs are up to date. The review also looked at where the icons are used: `crates/win32/src/app.rs` `command_icon`, the category tree and `rss.rs`.

Evidence in this folder (all made by `sheets.cjs`: `node sheets.cjs <artwork dir> <out dir> <sharp path>`):

- `overview.png`: the 8 new symbols at native 16/20/24/32 px in light, dark and mono, nearest ×4.
- `group-<id>.png` (8 sheets): each new symbol above its closest neighbours. Light and dark at 16/20/24/32 px (nearest ×8/×6/×5/×4), mono 16 px ×8 with a pixel grid, and 1× light/dark.
- `iou-1.png` … `iou-3.png`: the same layout for the highest-IoU neighbours the groups did not cover (error, appearance, copy, file-video, disk, logger, random-port, folder, scheduler, select-all, file-image, rss-history, wanted-complete, no-peers, start, unlock, privacy).
- `menu-1x.png`, `menu-2x.png`: a menu mock-up with each icon beside its real menu text. It shows Win32 at 100/125/150/200 % (native 16/20/24/32 px), and GTK with the 32 px export downscaled to 16 logical px, on light (#f9f9f9) and dark (#2b2b2b) menus. The 2× file is a nearest-neighbour copy of the 1× one.
- `ascii-16.txt`, `ascii-20.txt`, `ascii-24.txt`, `ascii-32.txt`: the mono masks as text (`#` alpha ≥ 128, `+` 32–127), for exact pixel positions.
- `neighbours.txt`: the top 6 mask neighbours (IoU) at 16 and 32 px, edge ink at all six sizes, and the badge gap (Chebyshev distance; 2 means one clear pixel).

## Verdict

**All 8 new symbols pass: website, check-updates, notices, import, window, edit, feed-disabled and no-label. None needs a redraw.** Each reads as its meaning at 16 px in light, dark and mono, and none collides with a neighbour. The notes below are optional.

The measured checks agree with the artist. `node check-geometry.cjs` rewrote `geometry-audit.json` byte for byte (no git diff): 834 renders (139 × 6 sizes), zero overflow, zero margin failures and zero unexpected identical masks. The near matches (≥ 0.88) are the seven known modifier pairs; none involves a new symbol. No new symbol has ink on its outer row or column at any size, from 16 to 64 px. The highest IoU from a new symbol to an older one is notices/error at 0.77, and their shared ring is the only overlap (see below).

## Per symbol

| Symbol | Verdict | Reasons |
|---|---|---|
| website (Help > Website) | **PASS** | It is the familiar external-link sign: a frame open at its top-right corner, with a ↗ whose head is a 2 px L corner (x 8–14, y 1–7). The frame and head keep a 1 px gap (Chebyshev 2) on both sides. Frame and head arms are 2 px; the shaft is a 2 px staircase. It does not resemble web-ui (window and globe), set-location or open-folder. Its highest IoU, random-port at 0.58, is just a shared frame. At 20 px the strokes mix 2 and 3 px (the known ×1.25 effect), but the symbol still reads. |
| check-updates | **PASS** | An arrow onto a baseline (x 1–8) with the shared refresh badge (x 10–14, y 9–14), keeping a 1 px gap. It reads as "download + refresh", a fair "check for updates", and it is distinct from download (no line or badge) and tray (U-tray, no badge). Its highest IoU is 0.40 (reannounce). Note: at 100 % the refresh badge reads like a small "c" (`menu-2x.png`). This badge was accepted in revisions 5 and 6 and is used on four other icons, so it is not a failure. |
| notices (© mark) | **PASS** | A pixel-art 14 px ring with a 2 px C reads as © at every size and in both palettes. It is not confused with help (?), about (P), general (i), skip (⊘) or error: their IoU of 0.77 comes only from the ring, and error has a red × inside it. Notes: (1) the C's counter is only 2 rows (y 7–8 at 16 px; y 9–10 at 20 px, with 3 px arms), so at 20–32 px the C looks like a heavy square "⊏". (2) The C's arm tips are 1 px from the ring's right side (x 12) but 3 px from its left. (3) The ring is centred on 7.5, half a pixel off Help's and About's anti-aliased circles at 8. Optional fix: redraw the C over y 4–11, x 5–10 — rows 4 and 11 at x 6–10, rows 5 and 10 at x 5–10, rows 6–9 at x 5–6 — so the counter is 4 rows (y 6–9, x 7–10) and the ring gaps are balanced. |
| import (File > Import) | **PASS** | "→]" reads as "into", and it is not tray's ↓-into-U, which would have been the clash. Logout ([→) is not confused with it: it is red, its IoU is under 0.35, and Exit has no menu icon, so the two never share a menu. Notes: (1) the bracket is `M8 2h5v11H8`, rows 1–13, so it is centred on y 7 while the arrow is centred on 7.5. The arrowhead therefore sits 1 row clear of the top arm but only diagonally clear of the bottom arm ((6,11) to (8,12)). (2) It is not the exact mirror the README claims: logout's bracket is 14 rows and its head 10 rows tall. Optional fix: `M8 2h5v12H8` (rows 1–14). That centres the bracket on the arrow and matches logout's height. |
| window (tray Show / Hide) | **PASS** | A 14 × 13 frame with a 4 px title bar and a 2 × 2 cut-out button reads as an application window at 16 px, light and dark. The solid bar is what separates it from clear-selection (plain frame, 0.65), file-video, disk and logger (0.63–0.66). The 20 px fit (a 5-row bar, a 3 px frame) is clean. |
| edit (Edit Feed) | **PASS** | A 45° pencil with a split-off eraser: the standard edit sign, clear at 1×. It is distinct from create-torrent (page + small pencil), properties and rss-rule. Note: the eraser gap is a single empty pixel diagonal. At 16 and 20 px the body and eraser touch diagonally at the slit's ends ((10,6)/(11,5) at 16 px; one component at alpha ≥ 128), so the split shows as a light line, not a clear gap. Optional fix: shorten the body to `M1 11 7.5 4.5l3 3L4 14H1Z`, which gives one fully clear pixel (Chebyshev 2). |
| feed-disabled (RSS > Disable Feed) | **PASS** | The compact feed (shared with add-feed, rss-rule and rss-history) with the cross badge. The badge gap is 3; the base stays in x 1–8, y 1–8. It reads as "feed ×", which pairs with the plain feed shown for Enable Feed. It is distinct from Remove Feed (trash) in the same menu and from tracker-error (pylon, red). Notes: (1) the compact feed's 1 px radial gaps fill with partial alpha at 16 and 20 px, a muddy feed already accepted for its three siblings. (2) The × keeps the base's orange. The set's other × badges mean an error (red) or offline (grey); a grey × would separate "disabled" from "feed error". |
| no-label (category list) | **PASS** | The compact tag of new-label and remove-label, with the cross badge (gap 4). It is grey, which suits "none" and is distinct from purple new-label (+) and red remove-label (−); the IoU of 0.73–0.75 is the shared base, as intended. In the tree it sits under the purple Labels heading and reads as "no label". |

## Artist claims checked

- README "all are drawn … in revision 7's pixel art with 2 px strokes": only partly true. website, check-updates, notices and window are `H.px` pixel art. import (2 px `s()` stroke plus an arrow path) and edit (filled paths) are vector, and so are feed-disabled and no-label, which reuse the vector compact feed and compact tag. None of this breaks a rule: their horizontal and vertical edges are on the grid, and the diagonals and curves are anti-aliased, which the design section allows.
- "import … log out's mirror image": not exact; see import above.
- "the cross badge moved to `H.crossBadge`, with identical output": true. tracker-error and offline did not change in the commit, and the audit is unchanged.
- "No existing drawing changed": true for the masks; no near-match or overflow entry changed.

## Summary

8 reviewed: **8 PASS, 0 FAIL.** Four optional refinements, none of which blocks shipping: centre the import bracket (`v12`), give the notices C a 4-row counter, widen the edit eraser gap, and consider a grey × for feed-disabled. The 20 px rounding artefacts (mixed 2/3 px strokes, the heavier × badge) are the kind accepted since revision 4. The dark palette shows no problems.
