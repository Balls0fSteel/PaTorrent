# Revision 4 regeneration brief

This brief is meant to be pasted into the generator (ChatGPT/Codex working in this repository). It covers the 18 symbols and the badge that failed the [revision 3 review](REPORT.md). Owner decisions (D28, 2026-10-26): **network icons use an arrow entering a port/ring; trackers use a radio tower as the shared base.**

## Rules that apply to everything

- Edit only the drawings in `assets/ui-icon-explorations/bold/` (`common.cjs`, `actions.cjs`, `states.cjs`, `misc.cjs`, `index.cjs`). Keep every ID, colour name, file path and the build pipeline unchanged.
- Draw the 16 px version natively on the 16 × 16 grid: straight edges on whole pixels, strokes 1 or 2 px, a 1 px clear margin. Give a `large` variant only where 24 px+ benefits from extra detail, and keep the same metaphor in it.
- Badges sit in the bottom-right 7 × 7 area (x 9–15, y 9–15), with a 1 px transparent gap from the base. The base keeps its complete silhouette; never cut it with the badge.
- No new identical monochrome masks. Section 4 below updates the allowlist.
- Every icon must be nameable from its 16 px black-and-white version alone.

## 1. Shared refresh badge (new primitive)

Used by reannounce, tracker-updating, reset-columns and reload-ip-filter. Today it is a 4 × 4 blob at 16 px.

- An open ring about 6 × 6 px in the badge area, with a 2 px stroke and a quarter of it missing at the top right.
- A solid 3 × 3 px triangular arrowhead at the clockwise end of the gap, so the direction is visible.
- Define it once in `common.cjs` (for example `H.refreshBadge`) and use it in all four icons.

## 2. Network base: an arrow entering a port/ring (D28)

Replaces `plug` in `states.cjs` for **network-ok**, **network-no-incoming** and **offline**.

- **Base (top-left 11 × 11):** a ring (port) with a 2 px stroke, about 8 px across, centred near (7, 6). A horizontal arrow comes from the left edge, passes through a 2–3 px gap in the ring's left side, and its head stops at the ring's centre. Shaft 2 px, head about 5 px wide. The ring's gap must be visible, otherwise it reads as a target or a "no" sign.
- **network-ok:** base + check badge, teal.
- **network-no-incoming:** base + "!" badge, amber. It means "no incoming connection seen yet", not an error, so keep the base unchanged.
- **offline:** base + × badge, neutral or red, matching the current offline colour.
- The `large` variant may thicken the ring and arrowhead; nothing else changes.

## 3. Tracker base: a radio tower (D28)

Replaces `server`/`H.announce` as the tracker base and the "(i)" beacon in `misc.cjs`.

- **Tower:** a vertical mast 2 px wide, with two splayed legs or a 5–6 px foot bar so it reads as a tower. **No dot on top**: the dot is what made the old icon read as "(i)".
- **trackers (Trackers tab):** the tower centred on the full 16 px, with **two** broadcast arcs on each side of the mast top (1.5–2 px strokes).
- **The four state icons:** the same tower in the top-left 11 × 11 with **one** arc on each side, plus a badge:
  - tracker-working: check, teal.
  - tracker-updating: refresh badge (section 1), blue.
  - tracker-error: ×, red.
  - tracker-not-contacted: an ellipsis "…" (three 2 × 2 dots with 1 px gaps along the bottom of the badge area), neutral grey. This replaces the hollow box + ring.
- **reannounce** (the "Update tracker" action) becomes the same drawing as tracker-updating: tower + refresh badge.

## 4. Allowlist change (`index.cjs`)

- Remove `['completed','tracker-working']`: tracker-working now has the tower base.
- Keep `['reannounce','tracker-updating']`: both become tower + refresh badge.

## 5. Other symbols

| ID | Redraw as |
|---|---|
| allocating | A disk or document outline with a level bar filled about halfway (solid lower part, empty upper part). |
| proxy | Three nodes in a row (small square, larger emphasised square, small square) joined by 2 px lines: client → relay → server. Not a dumbbell: the middle node must be clearly the largest. |
| dht | A ring of 5–6 solid 2 × 2 nodes joined by 1 px lines, or a small node mesh. No square frame. |
| logger | A terminal: rounded window outline with ">" and "_" inside (revision 2's version read well). |
| inactive | The `active` pulse line flattened (a straight line with one small bump), neutral grey. Alternatively a circle with a pause inside. Not a plain ring. |
| reset-columns | A table: an outline with a header bar and three or more columns, plus the refresh badge. It must not resemble the two pause bars. |
| reload-ip-filter | Keep the shield; replace the badge with the refresh badge. |
| add-url | Magnet with visible pole caps: a 1 px gap across both tips of the U, so it can't read as the letter "U". Keep the plus badge. |
| association | Page with the Verdigris P mark inside, no badge. It must differ clearly from create-torrent (page + pencil). |
| all-torrents | A 2 × 2 grid of solid squares, as in revision 2. It must not look like copy's overlapping squares. |
| download-limit | A down arrow + a small gauge (half-dial with a needle), or an arrow stopping under a cap bar with a line below. It must differ from `tray` (arrow into a tray) and `download`. Mirror the result for upload-limit. |

## 6. Check before handing back

```text
node build.cjs --sharp <absolute-path-to-sharp>
node check-geometry.cjs <absolute-path-to-sharp>
node proof-pages.cjs <absolute-path-to-sharp>
python -I review-2026-10-26/revision3/review-sheets.py . <out-dir>
python -I review-2026-10-26/revision3/shipped-sizes.py . <out-dir>/shipped.png
```

All checks must pass with the updated allowlist. In the sheets, the network trio and the tracker family should each read as one set at 16 px. The ring with an arrow must not read as a target or "no entry", and the tower must not read as "i".
