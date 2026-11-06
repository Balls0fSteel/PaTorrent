# Changelog archive: 2026-11-06, the 1.2.5 release

Moved from the root CHANGELOG.md on 2026-11-06_132853_1793968133. Paths and links keep their repository-root context.

## 2026-11-06_092025_1793953225: Website: favicon links carry the icon's hash

- The owner still saw the old icon as the site's favicon. The deployed `img/icon-*.png` were already the Postal Rook (byte for byte); browsers keep a favicon per page and do not refetch a changed file at the same URL. `site/build.py` now adds `?v=<hash of the icon PNGs>` to every page's `rel="icon"` and `apple-touch-icon` links, so a new icon gets a new URL. Test in `WholeBuild`; WEBSITE.md notes it.

## 2026-11-06_085947_1793951987: Release 1.2.5 published, the first as Rookparcel

- The Release workflow built all 14 files; the owner published the draft, and it is GitHub's Latest. The news entry is on `main`. Live site checked: `update.txt` says `stable=1.2.5`, the download page lists only `rookparcel-1.2.5` / `Rookparcel-1.2.5` files, and "PaTorrent" remains only in the news text about the rename.

## 2026-11-06_084738_1793951258: Wiki screenshots retaken; v1.2.5 tagged

- `docs/wiki/images` (18 changed, same sizes) and `docs/screenshots/search-win-dropdown*.png` retaken from the 1.2.4 demo on the hidden desktop: Rookparcel title bars and icon, `D:\Downloads\rookparcel-mock`; pushed to the wiki. The other search-guide shots never showed the name and need a real Jackett, so they stay.
- CI, FreeBSD and Fuzz green on `50362f50`; `v1.2.5` tagged there and pushed, so the Release workflow builds the draft. The news entry waits on the local branch `news-1.2.5`.

## 2026-11-06_082750_1793950070: Version 1.2.5: CI green again; release 1.2.5 prepared (1.2.1-1.2.5)

- **Fixed:** CI on `main` failed since the rename push: the runners' clippy 1.99 flags `needless_borrows_for_generic_args` on `.filter(&absolute)` in `crates/frontend/src/legacy.rs`, which stopped the workspace's clippy at the frontend crate. Local clippy (1.90) does not have the lint; workspace clippy and the `legacy` tests pass here.
- **Release 1.2.5**, the first as Rookparcel: [notes](docs/releases/1.2.5.md) (the rename, the Postal Rook, the takeover and how PaTorrent users move over, the 1.2.1 login fix), the Flatpak metainfo's `<release>`. GitHub's release titles up to 1.2.0 now read "Rookparcel X (released as PaTorrent)"; their notes and file names are unchanged.

## 2026-11-06_082258_1793949778: Website: screenshots of Rookparcel 1.2.4

- `site/img/screenshot-windows.png` (1000x680) and `add-new-torrent.png` (751x602, also `docs/screenshots/`) are new shots of the 1.2.4 Win32 GUI: "Rookparcel 1.2.4" in the title bar (built with `ROOKPARCEL_BUILD` empty), the Postal Rook icon, the toolbar search box, a downloading torrent's General tab, and the current Add New Torrent layout with the Sintel torrent. Taken on the hidden desktop from a portable demo copy (`--demo 40`), with `TMP` and `dir.download` pointed at `D:\Downloads` so no user folder shows. `index.html`: the dialog's size and alt text follow the new layout.
- Checked: `build.py --strict`, the site tests, and the home page in the browser pane.
- Still "PaTorrent" on GitHub: the release titles and bodies up to 1.2.0, and the 1.2.0 file names on the download page (the files are named so; `build.py` accepts both prefixes). Both go once a Rookparcel release is published. The wiki's other screenshots still show PaTorrent title bars.
