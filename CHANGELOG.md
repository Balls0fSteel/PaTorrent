# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-10-30_195156_1793386316: The Windows GUI updates itself (1.1.24, D45)

- **Owner's choices:** D45 approved as proposed, silent install with a relaunch, the setup checked against the release's `SHA256SUMS` (no `update.txt` change).
- **core:** `fetch::http_get_to(url, opts, sink)`: the 2xx body goes to a sink as it arrives (with its length when known), under `max_size`, with the timeout between reads once the head is in; a chunked body is decoded whole first and handed over in one piece. `http_get` is unchanged. Tested against a loopback server (pieces, the length, chunked, a sink error).
- **frontend:** `update::download_setup` fetches `SHA256SUMS` and `patorrent-<v>-windows-x64-setup.exe` from the release (`PATORRENT_UPDATE_FILES` names another folder for a hand test), writes `<name>.part` while hashing, checks the SHA-256, renames; a failure or cancel leaves nothing. `expected_sha256` reads `sha256sum`'s lines. Six new strings in all 35 tables (34 translated here).
- **Win32:** new `updater` module: `installed()` reads the Uninstall key's `InstallLocation` under HKCU then HKLM (`integration::get_in`) and compares it with the exe's folder; `download` is a modal progress dialog (marquee until the size is known, Cancel) with the fetch on a worker thread; `run_setup` starts `<setup> /S /CURRENTUSER|/ALLUSERS /UPDATE=<pid>`. `dialogs::choose` is a task dialog with the application icon and custom buttons. `offer_update` shows "Download and install" / "Open download page" / Cancel on an installed build (the D44 box, the manual check and a click on the notice), the Yes/No box on a portable one; after "Close and install now?" the window exits the normal way, so the state is saved.
- **Installer:** `/UPDATE=<pid>` waits up to a minute for that process (`OpenProcess` + `WaitForSingleObject` through the bundled System plugin) before `CheckFiles`, and starts `patorrent.exe` again at the end, as the user, from `.onInstSuccess` / `.onInstFailed` for one user and from the outer copy after the elevated one for all users. Built locally with makensis 3.13 `/WX` (3.1 MB from the last release binaries).
- **CI and release jobs:** the smoke test now also runs the setup with `/S /CURRENTUSER /UPDATE=<pid of an 8 s sleep>`, checks that it waited, that `patorrent.exe` started again from the folder, and stops it before the uninstall. The two files are kept in step.
- **Docs:** D45 in DECISIONS.md (approved), README, the site's FAQ, packaging/windows/README.md (the switch), RELEASING.md's tester checklist ("Update from PaTorrent"), BACKLOG hand checks.
- **Checked:** core fetch tests (21), frontend (220, two new), Win32 (81, two new), nox; stable clippy and fmt clean; the website builds strictly and its tests pass. Not run: the end-to-end update on a PC with the previous release installed (BACKLOG); the CI `/UPDATE` step runs first on the next push.

## 2026-10-30_192440_1793384680: The update box once per version (1.1.23, D44); D45 proposed; startup/shutdown findings

- **Owner's notes:** the update should pop up once per new version and then only show the status-bar text; could Windows update itself after a yes; is a progress indicator needed for a slow start or exit (NAS drives). The owner chose the Yes/No box of the manual check as the pop-up and a notice that stays until clicked.
- **1.1.23 (D44):** `update::Checker` keeps the newest version a box was shown for (`update.seen` in `patorrent.ini` / the GTK ini) and `finish` now returns `Finished { kind, answer, announce }`. An automatic check announces a version newer than the seen one in the Yes/No box (Yes opens the release page), always refreshes the standing status-bar notice, and takes the notice down when nothing is newer; the manual box marks its version seen too. `NOTICE_TIME` and `expire` are gone from both GUIs' timers. nox is unchanged (it passes no seen version). No new strings.
- **Checked:** `update::tests` (7, two rewritten and one new) pass; Win32 and nox type-check with all targets. GTK was edited the same way and only linted (no GTK here). Not hand-checked: see BACKLOG "Updates, installer and website".
- **Proposed D45, the seamless Windows update:** installed builds only, the setup and `SHA256SUMS` downloaded from the release behind a progress dialog, a "close PaTorrent now?" question, then the setup; needs a streaming `http_get_to_file` (today's `fetch` is in memory under a 30 s whole-exchange timeout), a `/WAITPID` setup switch and a relaunch after a silent install. Written up with its open choices; no code.
- **Startup and shutdown, investigated** (new BACKLOG section): startup needs no progress bar; the auto-load poll on the GUI thread is the one NAS risk at start. Shutdown hides the window and then waits without bound (moves, disk-thread join, a check's stat pass that ignores `abort`), a relaunch during it exits silently after 5 s, and the Win32 GUI handles no `WM_QUERYENDSESSION`, so a Windows logoff can kill it before the resume data is saved. The code-ready fixes are listed there; bounding the waits is the owner's call.

## 2026-10-30_151918_1793369958: The code-ready rest of PLAN.md (1.1.19-1.1.22, remote protocol 11)

- **Owner's pick:** the four chunks of PLAN.md that need no hand check or decision, built in five parallel worktree branches and merged one by one, each with its own version.
- **1.1.19, the qBittorrent shim's `search/*` (D41 slice 5):**
  - Each Torznab source is a plugin named `torznab-<hash of its URL without the key>`. Up to five searches run at once, with append-only results for offset paging.
  - `fileUrl` is a magnet or an opaque `patorrent-search:` URL that `torrents/add` and `downloadTorrent` resolve on the server, so the indexer key never leaves it. `installPlugin` is refused.
  - Tested in nox and in hosted mode against the fake Prowlarr.
- **1.1.20, libtorrent triage:** every fix-like commit without a ChangeLog line in v2.0.11..v2.1.2 (about 55) was read against our code; the verdicts are in PORTING.md.
  - Ported: checked decimal parsing (magnet `so=`/`dht=`, UPnP error codes), redirects with a query or fragment, early failure on non-HTTP bytes, legacy Transmission peer IDs, and smart-ban blame with pad blocks.
  - v2 hashes: stricter merkle proof checks, unsolicited HASHES ignored, received piece-layer spans no longer asked for again, and `hashes_rejected` matching.
  - Hardening: uTP packet-buffer indices, a saturating `sliding_average`, and the merkle uncle-count bound.
  - The v2 hash changes alter what we send. The peer ID stays `-PA1012-` because it has not been released yet; its comment now covers these changes.
- **1.1.21, engine options:**
  - 160-bit MSE DH exponent, so the handshake is cheaper; interop with full-width peers is unchanged.
  - The download queue reports v2-only tail pieces at their v2 size.
  - `dont_count_slow_torrents` now works, with 2.0.14's debounce and the new settings `inactive_down_rate`, `inactive_up_rate` and `auto_manage_startup`.
  - New settings `natpmp_gateway` and `natpmp_lease_duration`. The remote protocol is now 11, so a GUI and nox must both be current.
- **1.1.22, small gaps:**
  - The Win32 Preferences > Web UI page sets the API key (hashed, write-only) and the subnet whitelist. The Preferences dialog is 30 units taller to fit them.
  - "Show current speed in the title bar" (`ui.title_speed`, off) in both GUIs, with two new strings in all 35 tables.
  - RSS uses conditional GET (ETag and Last-Modified in `rss.dat`) and gzip. A 304 counts only when a validator was sent, and the shared HTTP parser no longer waits for a body on a 204 or 304.
  - The qBittorrent importer maps `qBt-ratioLimit` and `qBt-seedingTimeLimit` to seeding goals, taking only the ratio when both are set unless `MatchAll`.
- **Fixed:** the fuzz workspace failed stable clippy (let-chain suggestions), because it had no MSRV. It now sets `rust-version = "1.85"`, and its stale `Cargo.lock` is synced.
- **Checked:** on merged `main`, `cargo fmt --check`, clippy `-D warnings` (workspace without GTK, and the fuzz workspace) and the workspace tests (without GTK) all pass. GTK was linted from Windows with the stand-in pkg-config; its tests and UI were not run. No interop run against libtorrent 2.0.15 / 2.1.x; that is in BACKLOG.

## 2026-10-30_140531_1793365531: Fixes from libtorrent 2.0.12-2.1.2 (1.1.18, peer ID `-PA1012-`)

- **Triage (owner asked):** every ChangeLog entry from libtorrent 2.0.12 to 2.1.2 (about 100) was checked against the upstream diff and our code. About 45 do not apply (i2p, SSL torrents, mmap, WebTorrent, C++ API) and about 25 were already right here. The reference stays 2.0.11 with the fixes cherry-picked: v2.0.11 to v2.1.0 is 1,161 commits, mostly reorganisation. The list is in PORTING.md "Upstream fixes after 2.0.11"; deferred and untriaged items are in BACKLOG.md.
- **Ported, in six parallel worktree branches:**
  - **v2:** requests sized by `piece_size2` for v2-only torrents (libtorrent 2.0.13+ peers reject padded tail requests), and merkle `get_hashes` with one hash (a file with 513 pieces could not have its piece layer fetched from us).
  - **Disk:** renaming never overwrites an existing file. On Windows `MoveFileExW` is called without replace, and a change of case only goes through. Resume data is written as `file-version` 2, with packed `pieces`: a 2,000-piece seed went from 2,623 to 872 bytes. Versions 1 and 2 are read, and older builds recheck on a v2 record.
  - **Web seeds:** the original file names are requested (`orig_files`, our bug). The single-file-in-multi-file URL is fixed. Basic auth is dropped on cross-origin redirects. HTTP status codes and SOCKS5 versions are parsed strictly.
  - **DHT:** `apply_filter_to_dht` (default on) applies the IP filter to the DHT. It is in the Advanced table, and the remote protocol is now 10.
  - **Metainfo:** invalid path elements become `_`, tracker URLs are validated, there is a `max_directory_depth` of 100, BEP 47 symlinks are made relative in created torrents, and `remap_files` is removed.
  - **Engine:**
    - LSD first announces are staggered at most 4 s apart and keep their resends.
    - A forced or network-change reannounce of a hybrid announces both hashes (our bug).
    - Point-to-point interfaces without a route are local.
    - A UPnP send that would block no longer disables UPnP.
    - Idle torrents post their last zero rate.
    - File-rename alerts ignore the mask.
- **Checked:** on the merged tree, the core, frontend and nox test suites all pass; `cargo fmt --check` and clippy `-D warnings` (core, frontend, nox, Win32) are clean. The fuzz workspace was only `cargo check`ed for the DHT and web seed targets. The new unix-only symlink tests compiled but were not run (Windows). There was no interop run against a real libtorrent 2.0.15 / 2.1.x peer; that is in BACKLOG.
