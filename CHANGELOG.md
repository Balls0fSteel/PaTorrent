# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-10-30_122955_1793359795: Torznab search, slice 4: the GTK GUI (1.1.15, D41)

- **GTK:**
  - A menu button joined to the toolbar's `SearchEntry` offers the name filter (as before), each enabled source, "All sources" and "Manage Sources...". With a source picked, Enter opens Search Torrents. That window is not modal and has sortable columns, a status line per source, and Add by button, double-click or Enter.
  - Search Sources lets you switch, add, edit (an empty key keeps the stored one), remove and Test sources, saved in the window's ini.
  - Downloads are fetched off the main thread, then go through the app's normal add path with the Add dialog. Remote windows search from this computer and add on the server, as their RSS does.
  - No new strings.
- **Run on the Xubuntu VM:** under Xvfb against the VM's Prowlarr: 3 results, a wrong key's 401, sorting, a double-click Add through the Add dialog, Test, and the Edit dialog. A GTK size warning in the Edit dialog was fixed there. `cargo test -p patorrent-gtk` passes 28 tests, 8 of them new. The VM has no clippy; the Windows stand-in lint (`--target x86_64-unknown-linux-gnu -D warnings`) is clean on the merged tree. The key stayed out of output, and the VM was left clean.
- **Open:** no way to clear a stored key except removing the source; the search mode is not remembered between runs. Built by a subagent in a worktree; the screenshots were reviewed. Version 1.1.15.

## 2026-10-30_112341_1793355821: Torznab search, slice 2: nox and the Web UI (1.1.14, D41)

- **nox:**
  - New native commands `/pa/search`: GET, `start`, `stop`, `add`, and `source-add` / `-edit` / `-remove` / `-test`. The tick carries a `search` version.
  - Sources are stored in the options, so hosted mode uses the window's `search.source.*` keys, sealed on Windows.
  - Neither keys nor result links (Prowlarr's carry the key) ever reach the browser.
  - Adding a source, or changing a source's URL or key, needs the current password: a source makes the server fetch any address with the stored key. Statuses can therefore name the real failure (WEBUI.md "Security").
- **Web UI:** a Find button beside the filter box opens Search Torrents. It has a query, a Newznab category, a source choice, per-source status lines, a sortable results table, Add, and a Manage Sources dialog with Test. Titles are set as text only, and the phone layout works. 38 new strings, translated in all 34 tables.
- **Live run** against the Xubuntu VM's Prowlarr (`/2/api`, the fake indexer): Test answered "8 categories", a search returned 3 results, and an add put the torrent in the list. A wrong key shows "HTTP 401 Unauthorized". The key stayed out of output and commits. Captured Prowlarr results and caps (key replaced) are now fixtures; I checked that their 32-hex strings are info-hash prefixes, not keys. The browser pane check used a mock-library nox at desktop and 375 px.
- **Open:**
  - The page's static files are 208 KB with LF line endings, so the test limit went from 200 to 220 KB (owner accepted).
  - No shim `search/*` yet.
  - `/pa/search/add` adds directly, without the Add dialog.
- **Tests:** new `tests/search.rs` and a fake Prowlarr (`tests/support/torznab.rs`), plus a hosted test. nox, frontend (213 + 4) and Win32 (71) tests pass; clippy and fmt are clean. Built by a subagent in a worktree and reviewed before merging. Version 1.1.14.

## 2026-10-30_105007_1793353807: The Windows GUI's Web UI has the RSS Downloader (1.1.13)

- **Design (option a, shared model):** the hosted Web UI runs nox's own RSS endpoints (`/pa/rss`, the shim's `rss/*`) against the window's model, an `Arc<Mutex<Rss>>` in `Hosted`. There is one model and one downloader; no request waits on the GUI thread.
  - The window keeps the worker, the downloads and the saving of `rss.dat`.
  - New `Host::rss_changed` / `rss_download` calls queue work and post `WM_APP_WEBUI`, so a browser's Download goes through the window's `rss_add` and follows its label, paused, queue-top and Add dialog rules.
  - `WebUi::rss_changed` moves the tick's `rss` version after window-side changes.
  - Lock order: the RSS lock, then the Web UI State, never the reverse (WEBUI.md).
- Removed `HOSTED_NO_RSS`, its 404s and `app.js`'s RSS hiding. The alternative limits stay out (owner). `PrefWebUiNote` now ends "The page's RSS Downloader edits this window's feeds and filters.", machine-translated in all 34 tables. D40 has an amendment note.
- **Tests:** new hosted test `the_rss_endpoints_edit_the_hosts_own_downloader` (a fake host owning the model) and Win32 `a_browsers_rss_downloads_wait_for_the_window_in_order`. Win32 71, nox (lib 71, hosted 9, webui 43, others) and frontend tests pass; clippy and fmt are clean. Reviewed: the window releases the lock before adds, redraws and notifications.
- **Not yet run:** a real window with a browser. The redraws, `rss.dat` saving after a browser change and an open RSS dialog refreshing are a hand check (BACKLOG). Built by a subagent in a worktree. Version 1.1.13.

## 2026-10-30_104643_1793353603: Torznab search, slice 1: the shared model (1.1.12, D41)

- **D41 approved (owner):** search through the user's Prowlarr or Jackett over Torznab, with no plugins and no scrapers. A source on a non-public address never uses the proxy.
- **Slice 1, `patorrent-frontend` only, no UI yet:**
  - `rss::feed` reads an item's `<size>` and its `torznab:attr` / `newznab:attr` values (at most 64 per item).
  - New `search` module:
    - `torznab`: results, the error document, caps, and URL building for both servers.
    - `source`: sources stored as `search.source.N.*`, with the API key sealed like the proxy password (`prefs::is_sealed_key`) and redacted from every message.
    - `worker`: a merged multi-source search in the RSS worker's model, de-duplicated by info-hash, else title and size, and sorted by seeders. It also has the source's Test and the download of a result.
  - The fuzz target `rss` also feeds the Torznab parsers.
- **Reviewed before merging:** a result's link may reach non-public addresses only on the source's own scheme, host **and port** (`source::url_origin`; the agent's version compared hosts only), so a poisoned link cannot reach another local service.
- **Open:** the fixtures are written from the servers' documentation, not captured (slice 2 runs against the VM's Prowlarr). An HTTP 4xx answer's `<error>` text is lost because `http_get` drops error bodies, and a stopped search's requests run to their 30 s timeout.
- **Tests:** frontend 211 + 4 pass, with clippy and fmt clean. Built by a subagent in a worktree. Version 1.1.12.

## 2026-10-30_103224_1793352744: No alternative speed limits in the Win32 GUI (owner)

- **Decision (owner):** keep 2.2.1's controls; the Win32 GUI gets no alternative speed limits toggle. The hosted Web UI keeps refusing them (`speedLimitsMode` 0, `HOSTED_NO_ALT`), now by design. PLAN Phase C is RSS in hosted mode and optionally a GTK hosting page.

## 2026-10-30_101143_1793351503: nox checks for updates like the GUIs (1.1.11, D43)

- **Owner choice ("on, like the GUIs"):** `patorrent-nox run` fetches the site's `update.txt` (frontend `update::Checker`) 10 s after start, then every 12 h, through the session's proxy.
  - A newer version is one line on stdout and in the Web UI Logger, and a status-bar link (`update` in `hello` / `tick`).
  - A failure logs once per check.
  - It is off with `--no-update-check` (kept by `service install`) or the Web UI's new General > Privacy `update.check` / `update.beta`.
  - Hosted mode never checks; the window does.
  - Without `--webui`, only the flag turns it off. The check is recorded as D43; DAEMON.md has a section on it.
- **Tests:** a fake-fetcher schedule, failure and off cases; `hello` / `tick` carry the update, hosted carries none; the preferences round trip; `--no-update-check` parsing; the daemon tests pass the flag and never reach the network. All nox tests pass; clippy and fmt are clean.
- **Hand check:** a debug nox against a local `update.txt` (`stable=9.9.9`) printed `PaTorrent 9.9.9 is available: …/releases/tag/v9.9.9` after 10 s; an unreachable URL printed one failure line. That run used default settings and mapped port 6881 on the LAN router through UPnP/NAT-PMP for 14 s; future hand runs should disable the mappers. The status-bar link has not been looked at in a browser.
- Built by a subagent in a worktree and reviewed before merging. Version 1.1.11.

## 2026-10-30_100921_1793351361: Web UI API key and subnet whitelist, both off by default (1.1.10)

- **API key (owner, "both, off by default"):** `Authorization: Bearer <key>` is accepted on the shim, `/pa/*`, the WebSocket and the remote handshake when `webui.api_key_enabled` is on. The key is at least 16 characters and is stored hashed (`webui.api_key`). It is verified outside the State lock once, and later requests compare a cached SHA-256 in constant time. A wrong key counts toward the ban. With keys on, a wrong key is refused even from loopback or a whitelisted subnet.
- **Subnet whitelist:** `webui.bypass_auth_subnets` (CIDRs or bare addresses, IPv4 and IPv6) skips the login for those clients, never for a proxied request (SEC-03). The shim reports `bypass_auth_subnet_whitelist(_enabled)` read-only.
- All three settings are SEC-03 security fields: changing them needs the current password. They are in Web UI Preferences > Authentication, with 5 new strings, machine-translated in all 34 tables. Docs: WEBUI.md "Security", INTEGRATIONS.md gaps 4-5.
- **Tests:** subnet and Bearer unit tests, two webui tests and a hosted test; all nox (70 lib, 42 webui, 8 hosted, rest) and frontend tests pass. Built by a subagent in a worktree and reviewed before merging. Version 1.1.10.
- **CI:** the merged `main` (run 41028782209) built the installer and passed its install and uninstall steps on the first GitHub run.

## 2026-10-30_095330_1793350410: Win32 Preferences > Web UI has a Server domains field (1.1.9)

- **Win32:** the Connectivity group gains "Server domains (separate multiple entries with a semicolon):" (the Web UI's existing `WebDomains` string, so no new translation), stored as `webui.domain_list`; an empty field removes the key. An *arr in Docker that names this PC by a host name no longer needs the browser's Preferences to be allowed. The status line, Open Web UI and the note moved down; the demo screenshot shows the note's three lines within the page.
- **Tests:** the page test applies a domain list, and `the_setup_round_trips_through_the_preferences` stores and clears it. Win32 tests (70) pass; clippy and fmt are clean. Version 1.1.9.

## 2026-10-30_095028_1793350228: Shim and hosted Web UI fixes: categories, seed limits, tags (1.1.8)

- **Category folder on add (both modes):** a `torrents/add` without `savepath` saves to its category's `savePath`, as qBittorrent does under automatic management. `autoTMM=false` turns this off, and an explicit `savepath` always wins.
- **Hosted categories survive a restart:** a category is also the host's `webui.category.<name>` key (`%` and `=` escaped), with the folder as its value. A category that no torrent uses yet (an *arr's Test creates one) is therefore listed again at the next start. Removing the category removes the key.
- **Hosted seed limits resent:** a seeding-goal change made in the window (`torrent.<key>.seed_ratio` / `.seed_time`) bumps the torrent's version, so `sync/maindata` resends its `ratio_limit` / `seeding_time_limit`.
- **Tags:** `addTags`, `removeTags`, `createTags` and `deleteTags` answer 200 and do nothing, so Sonarr stops logging a warning per grab (INTEGRATIONS.md gap 5).
- **Tests:** new `an_add_without_a_folder_takes_its_categorys` (webui) and `an_unused_category_survives_a_restart` (hosted); the hosted *arr test checks the resend (it fails without the fix), and the *arr shim test covers the tag endpoints. All nox tests pass; clippy and fmt are clean. Version 1.1.8; the peer ID is unchanged.

## 2026-10-30_093153_1793349113: 1.1.7 published; the 1.1.2 draft deleted

- **Release:** tag `v1.1.7` on `f7ed2f40`. Release run 41023963735 passed in 11 min with 14 files; every file matched `SHA256SUMS`, and the zip's `patorrent-nox --version` printed 1.1.7. CI, Fuzz and FreeBSD passed on the same commit. The release was published as Latest with the owner's go-ahead.
- **1.1.2:** the never-published draft and its tag were deleted (owner).

## 2026-10-30_090053_1793347253: V2.0 profiles and portable-mode design (proposed D42)

- **Design (plan item E):** new [docs/PROFILES.md](docs/PROFILES.md): `--profile NAME` on all three binaries, named profiles in `profiles/<name>` inside today's root (the default profile is the 1.x folder, so nothing migrates), per-profile Win32 window class and GTK application id, a `patorrent.lock` per profile, random listen ports for new profiles, per-profile nox services (SCM, a systemd template, rc.d profiles), associations owned by one profile, portable mode by `patorrent.ini` on every platform but Flatpak with relative paths inside the folder, and the installer's (D31) checks. Nine open questions for the owner; a brief comparison with µTorrent and qBittorrent from memory.
- **Found while reading the code:** the Win32 second start's `FindWindowW("PaTorrentMainWnd")` is not keyed by data dir like its mutex, so with a portable and an installed copy both running a torrent can go to the wrong one; the GUI tests the portable marker with `exists()` and nox with `is_file()`. Both are step 1 of the design. Docs only, no version bump.

## 2026-10-30_085840_1793347120: CI builds and smoke-tests the Windows installer

- **CI (Phase 1 item 1):** `ci.yml`'s `release-windows` job now installs the pinned NSIS 3.13, builds the setup (`/WX`) and installs and uninstalls it silently, with the same steps and checks as `release.yml`'s `windows` job, so installer breakage shows on every push and PR instead of only at a tag. Its version is `<workspace version>-dev.<commit>` from `cargo metadata`, as `release.yml` names an untagged build. The setup joins the existing `patorrent-windows-x64` artifact. Checked locally: the YAML parses, every PowerShell step parses, the version step gives `1.1.7-dev.<sha>`; actionlint was not available, and the job has not run on GitHub yet. CI only, so no version bump.

## 2026-10-30_085652_1793347012: 1.1.7 release prepared; next features planned

- **Release (owner, 2026-10-30):** release `main` as 1.1.7 in place of the unpublished 1.1.2 draft. New `docs/releases/1.1.7.md` covers everything since 1.1.0: the hosted Web UI, the service login, the *arr fixes (1.1.3-1.1.6), activate on add and Stopped on add. There is a 1.1.7 `<release>` in the Flatpak metainfo. No program change, so no version bump.
- **Plan (owner chose all four areas):** release first; then the small shim, ops and hosted fixes; then the hosted Web UI's alternative limits and RSS; then Torznab search (D41) with local sources bypassing the proxy; and a V2.0 profiles and portable design. Details are in [PLAN.md](PLAN.md) Next steps.

## 2026-10-30_082342_1793345022: Adding a torrent activates the window; a torrent added stopped reads Stopped (1.1.7)

- **Bug (owner):** opening a `.torrent` or magnet while PaTorrent ran left its window behind. The running instance did call `SetForegroundWindow`, but Windows' foreground lock refuses a background process; the second instance (started by the user's click) now hands its right over with `AllowSetForegroundWindow` before forwarding. µTorrent 2.2.1 has this as UI Settings > When Adding Torrents > "Activate the program window" (on by default); PaTorrent gains it as `ui.activate_on_add` (Win32 and GTK Preferences, shared `pref_pages`). Off, only the Add New Torrent dialog comes up, which now raises itself. The Web UI keeps a host's value and does not show it.
- **Bug (owner):** a torrent added stopped read "Checking 0.0 %": the engine creates torrents in `CheckingResumeData` and leaves it only when started. `status_text`, `status_class` and `status_icon` now treat a stopped or paused (not auto-managed) torrent in a checking state as Stopped / Paused, in all three frontends. The engine and the qBittorrent shim's `checkingResumeData` (as qBittorrent reports it) are unchanged.
- **Tests:** status text, class and icon cases for stopped and paused checking states; the new option's default and round-trip in the options tests.
