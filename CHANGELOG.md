# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-10-27_112013_1793096413: V1.1: search box and importers from qBittorrent and µTorrent (D35)

- **Decision D35** (owner): the toolbar's search box filters the torrent list by name; compiled-in plugin hooks move to V2.0 (no first user yet).
- **Search:** `TorrentList::set_search` (frontend `model.rs`) applies a case-insensitive name match after the category or label filter, without allocating for ASCII text. At 100,000 rows a keystroke costs 12-16 ms in release. Win32 has an edit box at the toolbar's right end with Ctrl+F, Escape and Enter; list accelerators (Delete, Ctrl+A) are skipped while it has focus. GTK has a `SearchEntry` (subagent) whose list shortcuts are off while it has focus. The Web UI's own search now uses the model's.
- **Import** (`frontend/src/import.rs`): File > Import from qBittorrent / µTorrent in both GUIs (grayed in remote mode), and `patorrent-nox run --import CLIENT [--import-dir DIR]`. Each torrent becomes a libtorrent resume record with the `.torrent`'s info dictionary embedded, added through the existing `TorrentSource::Resume`, so the engine needed no new call. qBittorrent's `.fastresume` is taken as it is (`qBt-name`; category, else the first tag, as the label). µTorrent's `resume.dat` is mapped: `have` is LSB-first, `prio` `0x08` normal and `0x80` skip, both read from the owner's real 2.2.1 file; a renamed top folder becomes `mapped_files`. Magnets without metadata are added by hash. The import refuses while the source client runs; torrents already in the list and ones it cannot read are listed in the Logger.
- The engine trusts claimed pieces only where their files exist; otherwise it checks the files. A stopped torrent is verified when started, as in libtorrent. µTorrent's own part file (`~uTorrentPartFile_*.dat`) is not read, so pieces shared with skipped files are downloaded again.
- 15 new strings in all 34 tables. The translations are machine-made (subagent); ar/fa/he mnemonics and wording in inflected languages need native review.
- Validation: unit tests and three end-to-end tests on a real session (trusted pieces under a renamed folder, a full check when files are missing, a qBittorrent record as it is). A read-only dry run of the owner's real folders read all 47 qBittorrent torrents and the one µTorrent torrent with no skips. `nox run --import utorrent` against an isolated state folder, behind a dead proxy, added the torrent with its label. Workspace clippy (warnings denied) and fmt are clean; frontend, nox and Win32 tests pass; GTK clippy passes for Linux with the stand-in pkg-config. Not verified: the Win32 and GTK windows by hand (computer use is blocked for PaTorrent here).
- Moved the three oldest root entries verbatim to `docs/archive/CHANGELOG-2026-10-27--2026-10-27-installer.md`.

## 2026-10-27_104216_1793094136: Version 1.0.1: the first stable release instead of 1.0.0 (not tagged)

- The metainfo fix `b2ce7c06` passed CI 40476434941, FreeBSD 40476435274 and Release dry run 40476448542 (every job, both Flatpaks included). The owner chose a new version over moving the pushed `v1.0.0` tag, which stays on `b40c2e58` without a release.
- Version `1.0.1`: `Cargo.toml`, both lockfiles (workspace crates only), peer ID `-PA1010-`, Windows manifest `1.0.1.0`, the metainfo's first `<release>` (the unreleased 1.0.0 entry became 1.0.1). `docs/releases/1.0.0.md` became `1.0.1.md` and says why there is no 1.0.0; README, the site's Download and FAQ pages, WEBSITE.md and RELEASING.md's peer-ID example follow. D34 notes the change.
- Validation: core version tests, frontend 180, nox and Win32 63 tests pass; debug `patorrent-nox --version` prints 1.0.1 and `patorrent.exe`'s file version is 1.0.1, not pre-release; rustfmt clean; site tests pass.

## 2026-10-27_095101_1793091061: v1.0.0's Release run failed on the Flatpak metainfo; fixed

- CI 40472951003, FreeBSD, Fuzz and Pages passed on `b40c2e58`; with the owner's go-ahead it was tagged `v1.0.0` and pushed. Release run 40475482914 built Windows, both Linux legs and macOS, but both Flatpak legs failed `appstreamcli validate`: `releases-not-in-order 1.0.0 << 1.0.0-rc.1`. AppStream sorts a `-rc.1` suffix after the plain version, so the draft release was skipped (nothing published).
- Fix: the metainfo's rc.1 entry is now `1.0.0~rc.1` (a tilde sorts before), and the workflow's first-release check maps the crate version's `-` to `~`, so future pre-releases are written the same way (RELEASING.md, Flatpak). The `v1.0.0` tag still names `b40c2e58` and has to move to the fixed commit (owner's decision).
- Moved the three oldest root entries verbatim to `docs/archive/CHANGELOG-2026-10-26--2026-10-26-rc1.md`.

## 2026-10-27_092114_1793089274: Version 1.0.0: notes, README and website wording (not tagged)

- Version `1.0.0` in `Cargo.toml`, `Cargo.lock` and `fuzz/Cargo.lock` (only the workspace crates' versions changed); the peer ID stays `-PA1000-` and the Windows manifest `1.0.0.0`. The Flatpak metainfo's first `<release>` is 1.0.0, type stable.
- **In 1.0.0 since rc.1:** the Windows installer (D31), the update check (D32), the version in the title, tray tooltip, Web UI tab and the exes' file properties, Help > PaTorrent Website, and the website (D33). No state format changed, so rc.1's settings and torrents carry over.
- `docs/releases/1.0.0.md` is the release text: what is new since rc.1, the installer row and first-run step, and the known limitations without the pre-release warnings. README's banner is a note about the first stable release, its status names 1.0 and the V1.1 importers; README, the site's Download and FAQ pages and WEBSITE.md say "from 1.0.0 on" instead of "the release after 1.0.0-rc.1".
- Validation: core version tests (peer ID against the version) and the frontend update tests pass; a debug `patorrent-nox --version` prints `patorrent-nox 1.0.0`, and `patorrent.exe`'s file properties read 1.0.0 with the pre-release flag off; rustfmt clean; site tests pass; the metainfo parses. CI on this commit decides the tag.

## 2026-10-27_091619_1793088979: 1.0.0 follows rc.1 directly (D34); V1.1 goals explored

- **Decision D34** (owner): rc.1 runs fine, so the next release is 1.0.0, not rc.2. The open Road to 1.0 hand checks become 1.0.x validation; item 14 (version, notes, draft inspection) remains. PLAN.md's next step 1 is now the 1.0.0 release; no CI run has finished green since rc.1 (two were cancelled by newer pushes), so one must before tagging.
- **V1.1 exploration** (PLAN.md "V1.1 goals", details in BACKLOG.md): qBittorrent's `.fastresume` files are libtorrent resume records, which `resume.rs` already reads, so that importer is the cheapest; µTorrent's `resume.dat` maps onto the same records but the owner's copy lists no torrents, so a test fixture has to be made; a name filter belongs in the frontend's `TorrentList` for all three UIs; plugin hooks need a first concrete use. Both importers need a public engine call to add from a resume record.
- Fixed a doubled "installer beside it, D31" in PLAN.md.

## 2026-10-27_090712_1793088432: First CI run of the installer: green (Release dry run 40470860315)

- Release started by hand on `73736841` (no release made). The windows job passed every step: NSIS 3.13's download matched the pinned SHA-256; makensis `/WX` built `patorrent-1.0.0-rc.1-dev.7373684-windows-x64-setup.exe` (2.7 MB, 35 language tables) from the 2 min 28 s release build; the silent per-user install with the desktop shortcut and both associations put the 5 files, both shortcuts, a correct Uninstall key (version, publisher, folder, 8.2 MB) and the `.torrent` / `magnet:` registrations in place, the installed nox printed `patorrent-nox 1.0.0-rc.1`, and the silent uninstall left no file, shortcut, key or association.
- Both Linux release jobs built `patorrent-gtk` with the update-check code, its first compile anywhere; macOS and the Flatpaks were still building. CI on `9692f69f` was cancelled by the newer push; CI on `73736841` was running. Still unverified: the installer's wizard, the all-users/UAC path and a hand test on a clean machine.

## 2026-10-27_090124_1793088084: The website is live; the update check works end to end

- Pushed `2717982d..9692f69f` (the owner's go-ahead). With the owner's approval, Pages was switched from the legacy "deploy from `main`" source to GitHub Actions and the `github-pages` environment got a `v*` tag rule (WEBSITE.md).
- The push's Pages run 40470075159 deployed, but the legacy `pages-build-deployment` run, started by the same push before the switch, finished 23 s later and put the README back. A manual run, 40470559108, redeployed the site: every page, the screenshot and the stylesheet answer 200 and a missing page 404; `update.txt` is `prerelease=1.0.0-rc.1`. The CDN caches files for up to 10 minutes.
- Help > Check for Updates in a portable debug build fetched the live file over HTTPS and answered "You have the newest version, PaTorrent 1.0.0-rc.1."
- FreeBSD 40470075203 passed on `9692f69f`; CI 40470075192 and Fuzz 40470075130 were still running when this was written. The Release workflow (the installer job) has not run.

