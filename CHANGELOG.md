# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

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

## 2026-10-30_132816_1793363296: 1.1.17 published; the website's News page is live

- **Release:** tag `v1.1.17` on `381432a5`. Release run 41054110247 built 14 files; all 13 artifacts matched `SHA256SUMS`, and the zip's `patorrent-nox --version` printed 1.1.17. CI had passed on `2ed9f708`, which has the same code. The release was published as Latest with the owner's go-ahead.
- **Website:** the site branch was merged after publishing (`17c933cb`) and Pages deployed it. `update.txt` reads `stable=1.1.17`; `news.html` and `news.atom` answer 200, and the live News page was looked at in the browser pane.

## 2026-10-30_131301_1793362381: 1.1.17 release prepared; README and a News page for the website

- **Release (owner: release before describing the new features):**
  - `docs/releases/1.1.17.md` covers everything since 1.1.7: Torznab search in all three interfaces, RSS in the Windows GUI's Web UI, the API key and subnet whitelist, the *arr fixes and nox's update check.
  - Flatpak metainfo has a 1.1.17 `<release>`. The peer ID stays `-PA1011-`.
- **README:** 1.1.17 is the latest release, with the search, the *arr support, nox's update check and a link to the News page. The outdated "Phase 3, in progress" and "not yet run on Linux" labels are gone, and the Windows GUI's own Web UI is now dated 1.1.7, since 1.1.2 was never published.
- **Website (owner: a short changelog like qBittorrent's News page):**
  - New `site/news.html`, with a dated, tagged entry per published release, and `news.atom`, which `build.py` generates and checks.
  - A News link on every page, and a build-filled "Latest news" line on the home page.
  - Home and FAQ content for search and the *arr apps, and fixes to stale FAQ answers.
  - 30 site tests pass and the strict build is clean. Built by a subagent and checked in the browser pane, light and dark, desktop and 375 px.
  - The site branch is merged only after 1.1.17 is published, because a `site/` push deploys at once. RELEASING.md gained the news step.

## 2026-10-30_125841_1793361521: A merged search result names its sources in source order (1.1.17)

- **CI failure (run 41050620708, Windows):** `search::tests::results_fill_sort_and_add` expected "Prowlarr, jackett.lan" and got "jackett.lan, Prowlarr". The merged result listed its sources in the order their parallel answers arrived, so the Source column (Win32, GTK and the Web UI's `source_names`) and two frontend tests depended on timing.
- **Fix (`frontend::search::worker::merge`):** a merged result's `sources` are now the union in source order. Its `source` (the one to download from, the entry with most seeders) is unchanged. The frontend tests' expectations were updated; frontend 213 + 4 and Win32 79 pass, and the failing test passed 5 runs in a row. Version 1.1.17.

## 2026-10-30_123456_1793360096: Torznab search, slice 3: the Win32 GUI (1.1.16, D41)

- **Win32:**
  - A drop-down arrow beside the toolbar's search box (`TBN_DROPDOWN`, Alt+Down) offers "Filter torrents" (as before), each enabled source, "All sources" and "Manage Sources...". The choice is remembered as `ui.search_source`, with any key masked, and the box's hint shows the mode.
  - With a source picked, Enter opens Search Torrents: a resizable, modeless window with a status line per source, a sortable owner-data list, and Add by button, double-click or Enter. A download is fetched on a thread, then goes through the normal add path and Add dialog.
  - Search Sources lets you switch, add, edit (an empty key keeps the stored one), remove and Test sources, saved sealed. Changes from the hosted Web UI refresh the drop-down.
  - Remote windows search from this computer and add on the server, as GTK does. 4 new strings, in all 34 tables.
- **Checked:** a portable `--demo` copy run live against the VM's Prowlarr: 3 results, Test "OK, 8 categories", and the Edit dialog. The results list showed a horizontal scrollbar, so Name was narrowed afterwards; that change was not captured again.
- **Not checked:**
  - Whether the key is actually sealed in the saved ini: the check was refused as credential access, and the slice-1 unit test covers it.
  - A live Add (demo mode has no engine).
  - Real 150/200% DPI (BACKLOG).
- **Tests:** Win32 79 (8 new), frontend 213 + 4 and nox pass; clippy and fmt are clean. Built by a subagent in a worktree; the screenshots were reviewed. Version 1.1.16. With this, D41's slices 1-4 are done.

## 2026-10-30_122955_1793359795: Torznab search, slice 4: the GTK GUI (1.1.15, D41)

- **GTK:**
  - A menu button joined to the toolbar's `SearchEntry` offers the name filter (as before), each enabled source, "All sources" and "Manage Sources...". With a source picked, Enter opens Search Torrents. That window is not modal and has sortable columns, a status line per source, and Add by button, double-click or Enter.
  - Search Sources lets you switch, add, edit (an empty key keeps the stored one), remove and Test sources, saved in the window's ini.
  - Downloads are fetched off the main thread, then go through the app's normal add path with the Add dialog. Remote windows search from this computer and add on the server, as their RSS does.
  - No new strings.
- **Run on the Xubuntu VM:** under Xvfb against the VM's Prowlarr: 3 results, a wrong key's 401, sorting, a double-click Add through the Add dialog, Test, and the Edit dialog. A GTK size warning in the Edit dialog was fixed there. `cargo test -p patorrent-gtk` passes 28 tests, 8 of them new. The VM has no clippy; the Windows stand-in lint (`--target x86_64-unknown-linux-gnu -D warnings`) is clean on the merged tree. The key stayed out of output, and the VM was left clean.
- **Open:** no way to clear a stored key except removing the source; the search mode is not remembered between runs. Built by a subagent in a worktree; the screenshots were reviewed. Version 1.1.15.

