# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-10-27_090124_1793088084: The website is live; the update check works end to end

- Pushed `2717982d..9692f69f` (the owner's go-ahead). With the owner's approval, Pages was switched from the legacy "deploy from `main`" source to GitHub Actions and the `github-pages` environment got a `v*` tag rule (WEBSITE.md).
- The push's Pages run 40470075159 deployed, but the legacy `pages-build-deployment` run, started by the same push before the switch, finished 23 s later and put the README back. A manual run, 40470559108, redeployed the site: every page, the screenshot and the stylesheet answer 200 and a missing page 404; `update.txt` is `prerelease=1.0.0-rc.1`. The CDN caches files for up to 10 minutes.
- Help > Check for Updates in a portable debug build fetched the live file over HTTPS and answered "You have the newest version, PaTorrent 1.0.0-rc.1."
- FreeBSD 40470075203 passed on `9692f69f`; CI 40470075192 and Fuzz 40470075130 were still running when this was written. The Release workflow (the installer job) has not run.

## 2026-10-27_084916_1793087356: Windows installer (D31) and the project website (D33)

- **Installer** (commit `37b042de`, built by a subagent in its own worktree, reviewed and merged here): `packaging/windows/patorrent.nsi` (NSIS 3.13, Modern UI 2, the app's 34 languages; its own 13 texts in `strings.nsh`) builds `patorrent-<version>-windows-x64-setup.exe` beside the unchanged zip. Per user into `%LOCALAPPDATA%\Programs\PaTorrent` without elevation by default, or all users into Program Files by relaunching itself through UAC (MultiUser.nsh elevates at start). Start menu entry, optional desktop shortcut and associations, upgrades over the folder its Uninstall key names, Retry / Cancel instead of killing a running PaTorrent or a busy `patorrent-nox.exe` (the service), and an uninstaller that keeps `%APPDATA%\PaTorrent` unless asked.
- `patorrent.exe --associate torrent,magnet` and `--unregister` (hidden, no window, exit 0 / 1 / 2) reuse `integration.rs`; `unregister` removes only what names this executable, including ProgIDs another client has since replaced. Four new tests on a scratch registry tree.
- The Release workflow's windows job installs NSIS 3.13 (`windows-latest` has none; pinned download with winget-pkgs' SHA-256), builds with `/WX`, then installs silently on the runner with the shortcut and both associations, checks files, shortcuts, the Uninstall key and associations, runs the installed nox, uninstalls and checks nothing is left. RELEASING.md lists what a tester checks.
- **Website** (commit `0d8439d5`, likewise a subagent's): `site/` holds hand-written pages (home, Download, FAQ, Documentation, 404) in the verdigris colours with µTorrent-style toolbar, list views and status-bar footer; light and dark, down to 360 px, nothing loaded from elsewhere. `site/build.py` (standard library) fills in the newest release from the API (links from the release's real files, sorted by name) and writes `update.txt` and `.nojekyll`; `.github/workflows/pages.yml` deploys on site changes, release events, weekly and by hand. The main screenshot (`site/img/screenshot-windows.png`) is a Win32 `--demo` capture with the new title. README links the site and describes updates; RELEASING.md step 6 covers the Pages run after publishing.
- Validation: Win32 tests (63) after the merge, workspace clippy and rustfmt clean; makensis `/WX` on the merged tree; the agent's silent per-user install, upgrade, busy-file abort and uninstall left nothing behind (HKCU snapshot identical). Site: 24 unit tests, a live-API build (`prerelease=1.0.0-rc.1`, 13 files) and a preview here (no console errors, no overflow); the agent checked 5 pages at 1280/375/360 px, light and dark. The Web UI tab title read "PaTorrent 1.0.0-rc.1" against a mock nox.
- Not verified: anything on GitHub (the installer job, the NSIS checksum, the Pages deploy); the installer's wizard, UAC path and real associations; Pages is still in legacy mode (owner setting).

## 2026-10-27_083359_1793086439: The version where µTorrent showed it, and an update check (D32)

- Commit `283357e4`. **Version:** the main window's title is "PaTorrent 1.0.0-rc.1" (Win32 and GTK), as are the remote titles and the tray tooltip's first line; the three strings gained `{version}` in all 35 tables. The Web UI's tab title carries it too. `patorrent.exe` and `patorrent-nox.exe` now have a `VERSIONINFO` resource (1.0.0.0, flagged pre-release, the text version, product and company): the `.res` writer moved to `crates/win32/res/resource.rs`, shared with a new `crates/nox/build.rs`. Read back with PowerShell's `VersionInfo` from both debug builds.
- **Update check** (`crates/frontend/src/update.rs`): reads the website's `update.txt` (`stable=`, `prerelease=`; D33) 10 s after the start and every 12 h on a worker thread through the RSS proxy. A newer release shows for a minute at the start of the status bar (Win32: its own part with the app icon, a hand cursor and a tooltip; GTK: a link button) and in the Logger; a click opens the release page. No pop-up, nothing downloaded. Help > Check for Updates answers in a message box; Help > PaTorrent Website opens the site. Preferences > General > Privacy, as in µTorrent: "Check for updates automatically" (on), "Update to beta versions" (off); a pre-release build is offered the next pre-release anyway. Off in a Flatpak and with `PATORRENT_NO_UPDATE_CHECK` at build time; `PATORRENT_UPDATE_URL` points a hand test elsewhere.
- The 11 new strings were translated into all 34 tables by a subagent; the Danish, Norwegian, Russian, Spanish, Portuguese, Hebrew and Greek Help-menu mnemonics differ from English to avoid clashes. Found in passing: Serbian's MenuAbout and MenuNotices share the О mnemonic (backlog).
- Validation: frontend 180, Win32 59 and nox tests pass; workspace clippy and rustfmt are clean. Hand test of a portable debug build in `--demo` mode with a local `update.txt` announcing 9.9.9 and 10.0.0-rc.1: the title, the notice after ~10 s (10.0.0-rc.1 offered, as 1.0.0-rc.1 is a pre-release; at the default 900 px the network part shrinks to its icon and the clipped text is in the tooltip), Help > Check for Updates' Yes/No box and the Privacy group, in English and Hungarian.
- Not verified: clicking the notice (it opens the browser); the GTK side, which compiles only on Unix (CI will build it); macOS. Until GitHub Pages is switched on, every automatic check logs "Update check failed: HTTP 404".

## 2026-10-27_065010_1793083810: 1.0.0-rc.1 published: the first tagged release

- Tag `v1.0.0-rc.1` on `6ed8f6c2` (CI 40408647551, FreeBSD 40408647459 and Fuzz 40408647609 green). Release run 40462743583 passed every job on its first try: version, Windows, Linux x86_64 and aarch64, both Flatpaks, macOS and the draft. The tagged path (Road to 1.0 item 6) works.
- The draft had the 13 files RELEASING.md lists (121 MB in all: the Windows zip 4.5 MB, the Linux tarballs 4.6 / 4.2 MB, the GUI AppImages 37.1 / 36.3 MB, the nox AppImages 5.2 / 4.9 MB, the Flatpaks 2.2 / 2.0 MB, the dmg 16.0 MB, the macOS nox tarballs 2.0 / 2.2 MB, SHA256SUMS), was marked a pre-release and carried `docs/releases/1.0.0-rc.1.md` as its text.
- Checked here before publishing: the zip's SHA-256 matches SHA256SUMS; its `patorrent-nox.exe --version` prints `patorrent-nox 1.0.0-rc.1`; its `patorrent.exe` started in portable mode (an empty `patorrent.ini` beside it), showed the PaTorrent window and was still running after 8 s at a 21 MB working set. The macOS job's screenshot shows the window with the runner's local-network prompt in front of it (the known runner artifact) and the GUI running after 20 s; the `.app` needs macOS 15.0, nox 11.0 (arm64) and 10.12 (x86_64).
- Published 2026-10-27T06:49:46Z as a pre-release: <https://github.com/Balls0fSteel/PaTorrent/releases/tag/v1.0.0-rc.1>.
- Not verified: the Linux tarballs, AppImages and Flatpaks and the macOS files were only built and (for nox) started on the runners; nothing from this release has run on a Linux desktop or a real Mac. The zip was not tried on a clean Windows machine (SmartScreen).

## 2026-10-26_215725_1793051845: RSS date parser: signed fields overflowed (fuzz crash)

- Fuzz run 40404037267 (and the scheduled run 40296131012 that morning) crashed the `rss` target: `parse_rfc822` parsed the day, year and time fields as `i64`, so `0:-002074284530519326355:9` passed the `mi > 59` check and `mi * 60` overflowed (`crates/frontend/src/rss/feed.rs`). The fields are now unsigned (`u32` day and year, `u8` time), so a signed field is no date. A release build wrapped instead of panicking, so the shipped binaries were never crashable by a feed; the fuzz build has overflow checks.
- Four regression cases in `feed::tests::dates`; the frontend RSS tests, clippy and rustfmt pass. `fuzz/Cargo.lock` now names the workspace crates at 1.0.0-rc.1.
- CI run 40404037237 and FreeBSD 40404037325 on the release commit `9581087b` were green; the other 24 fuzz targets passed.

## 2026-10-26_212009_1793049609: Release candidate 1.0.0-rc.1: version, notes, README and the tagged release path

- Version `1.0.0-rc.1` (Road to 1.0 item 14): `Cargo.toml`, `Cargo.lock`, peer ID `-PA1000-` (a pre-release carries the version it leads to; the test now strips the suffix), the Windows manifest `1.0.0.0` and the Flatpak metainfo's first `<release>`. `cargo metadata --locked` and the core version tests pass.
- `docs/releases/1.0.0-rc.1.md`: the release text (downloads per platform, first-run steps, what is in it, known limitations). The Release workflow now checks out the repository and uses `docs/releases/<version>.md` as the draft's notes when it exists, and marks a suffixed version as a pre-release. `docs/RELEASING.md` steps 1, 2, 4 and 5 record the manifest, the metainfo, the notes file and the pre-release tag.
- README: the warning now describes a release candidate, the status paragraph matches the plan (GTK, v2, languages, remote mode are in), and a Download table lists the release files per platform.
- Backlog: the Windows executables have no `VERSIONINFO` resource (Explorer shows no version); noted under Win32.
- Not in this commit: `tools/swarm/src/interop.rs` carries an uncommitted qBittorrent-log wait for `lt-swarm interop` (uTP-only runs); it is unrelated to the release and left for its own change.
- The tag `v1.0.0-rc.1` and the run's outcome are recorded in the next entry.

## 2026-10-26_150634_1793023594: Integrate the bold artwork: 24 state icons and the toolbar (gate 10)

- Both GUIs now use the revision 6 artwork. `crates/frontend/src/icons.rs` embeds the bold PNG exports from `assets/ui-icon-explorations/png{,-dark}/bold/`:
  - the 24 runtime state icons at 16/20/24/32 px;
  - the 11 toolbar images at 16/20/24/32/48 px.
  The runtime-drawn toolbar vectors are removed.
- **Win32:**
  - The toolbar slot is 24 px scaled to the nearest export: 24, 24, 32 and 48 px at 100, 125, 150 and 200 %. Before it was 22 px drawn at run time.
  - `toolbar_bgra` decodes the PNG for the normal and greyed image lists. A small in-tree reader handles 8-bit RGBA PNGs with core's existing inflater; no new dependency.
- **GTK:**
  - The toolbar uses the artwork (32 px drawn at 16 logical px) instead of desktop theme icons.
  - State and toolbar icons switch to the dark palette when the GTK theme is dark (prefer-dark setting or a theme name containing "dark") and follow theme changes.
- Tests check every embedded size and palette, PNG decoding, toolbar sizes, and that the icon lists match the artwork manifest's `runtime` and `toolbar` lists.
- Validation:
  - Win32 build, clippy and tests pass (frontend 174, Win32 59).
  - A portable debug run was captured: bold toolbar, greyed disabled buttons and the globe network icon at 100 % DPI.
- Not verified:
  - The GTK client compiles only on Unix and WSL is unavailable here. After the push, CI run 40336848086 built and tested it on Ubuntu, macOS, Windows and MSRV, all green; it has not been run on a desktop.
  - Non-100 % DPI, the GTK dark palette, the Trackers tab with real trackers and macOS are still hand checks.
- Moved the six oldest root entries, verbatim, to `docs/archive/CHANGELOG-2026-10-23--2026-10-26.md`.
