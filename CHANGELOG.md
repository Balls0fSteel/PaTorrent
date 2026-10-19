# Changelog

All notable changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (local time).

## 2026-10-19_105727_1792400247: CI fix for MSRV 1.85 and stable clippy

### Fixed
- CI run 39225424904 failed in two jobs (FreeBSD passed). `msrv` (Rust 1.85): a let-chain (`if … && let Some(t) = …`) in `engine/mod.rs`'s upload-mode handling; let-chains are stable only from 1.88. Now a nested `if let`. The three let-chains in `win32/src/app.rs` (Auto Shutdown on exit and on suspend, move-completed alerts) were rewritten too (`Option::map` / `and_then`): the MSRV job runs on Linux and never builds them, but the workspace declares `rust-version = "1.85"`.
- `test` on Windows and Linux (stable clippy 1.99): `clippy::single_element_loop` in `frontend/src/properties.rs` (the super-seeding flag loop over one pair, now a plain `match`).
- The next run (39226516579) passed everything except `nox/tests/webui.rs::native_detail_tabs_against_the_engine` on the Windows runner: the Speed tab's incremental update had two `[0, 0]` samples where the test expected exactly one. The server sends every sample since the last update by design (the page appends them all), and a slow runner ran two status ticks before the update went out. The test now accepts one or more samples, all zero.
- The run after that (39227655787) failed `external_ip::a_dht_nodes_ip_sets_the_external_address` on Windows: "failed to listen on 127.0.0.1:0 (Udp) ... os error 10013", so the DHT never started. A real engine bug: TCP drew ephemeral port 57136, which lay in a block of UDP ports Windows reserves (Hyper-V / WinNAT; this machine reserves UDP 56995–57694 too), and the retries drew the next TCP ports, which Windows hands out nearly in sequence, all inside the block. `engine/listen.rs::open_listen_socket`: once UDP refuses a port TCP drew, the remaining tries let UDP draw the port and bind TCP on it (the system never draws an excluded port for the protocol asking). libtorrent keeps the TCP socket without UDP here; noted in the doc comment. Reproduced by holding the 59 UDP ports after the next ephemeral one: 20 of 20 sessions had no UDP socket before, 0 of 20 after. New Windows-only test `udp_picks_the_port_when_tcp_keeps_drawing_ports_taken_for_udp` does the same five times.

### Verified
- Windows: `cargo +stable fmt --all --check`, `cargo fmt --all --check`, fuzz fmt check, `cargo +stable clippy --workspace --all-targets -- -D warnings` (1.97), Linux-target clippy, `cargo test --workspace`: 902 passed, 0 failed (903 with the listen test). CI results: see the next entry if any, or PLAN.

### Issues
- No 1.85 toolchain is installed here, so the MSRV check runs only in CI; local clippy on 1.90 and stable accepts let-chains (edition 2024). Grep for `&& let ` before pushing. Local stable (1.97) also lags CI's (1.99).

## 2026-10-19_104021_1792399221: uTP no longer stalls after losing a whole window; super-seed test deflaked

### Fixed
- `super_seed::two_leeches_finish_while_the_super_seed_uploads_about_one_copy` failed under load ("the seed uploaded 1.82 copies"; FreeBSD and Windows). Not a super-seeding bug (offers, HAVE-driven re-offers and refused unoffered requests all checked out; libtorrent's own test asserts no upload bound): the uTP link between the two leeches stalled. A window lost whole to a full loopback UDP receive buffer (49 packets, about 70 KB in one trace) was never resent, because every incoming packet restarted the retransmission timer (libtorrent 2.0.11, unchanged in master) and the other leech's HAVEs kept arriving every ~100 ms; with nothing received there were no duplicate ACKs either. The link stayed dead and the super seed served both leeches.
- `utp/socket.rs`: an incoming packet restarts the timer and clears `num_timeouts` only when it acknowledges something or nothing is in flight (RFC 6298 §5.3); documented as a deviation in the file header. New simulated test `a_window_lost_whole_is_resent_while_the_peer_keeps_sending` (fails 20 of 20 without the fix, passes 40 of 40 with it).
- `tests/super_seed.rs`: TCP only (each lost uTP tail still costs the 500 ms minimum timeout, which is not what the 1.5-copies bound measures), and the leeches connect to each other before the seed. The bound is unchanged. Commit `915424b`, merge `4491b4f`.

### Verified
- Failure rate of the two-leech test: FreeBSD under 16 parallel copies plus a workspace test loop 68/400 (17 %) → 14/400 with the uTP fix alone → 0/1200 with both changes; Windows under 8 parallel copies 8/1760 → 0/1760 with the uTP fix alone. Never reproduced on Xubuntu.
- Merged main `4491b4f`: `cargo test --workspace --no-fail-fast` on Xubuntu 852 passed, 0 failed; FreeBSD 852 passed, 0 failed; Windows 902 passed, 0 failed in two runs. One earlier Windows run printed no result line for the win32 crate's 39 tests and no FAILED line; the cause was not captured (see PLAN.md Known TODOs).

## 2026-10-19_101030_1792397430: RSS Downloader GUIs, Auto Shutdown, the Web UI's Properties dialog, nox as a Windows service

The second parallel batch, merged into main: `57e8f71` (merge `af08c41`), `5560b04` (`8c1054d`), `97b76ac` (`f31bd0c`), `3422742` / `4afa736` / `89873e1` (`c0b1a83`).

### Added
- **RSS Downloader GUIs** on the `frontend::rss` model.
  - `frontend::rss::worker`: feed refreshes each on their own thread and `.torrent` downloads in one batch thread (`notify` on the worker thread), `collect()` on the frontend's thread (`feed_fetched`, downloads, `forget` on a failed download); magnets come back at once; downloads send the feed's cookies and the proxy, follow a magnet redirect and refuse a non-bencoded body; `AddRequest::apply_to`; a `Fetcher` trait for tests. `rss::view`: the item and History rows and texts. `request_item`, `edit_feed` (a new URL drops the old items), `move_filter`.
  - Win32 `rss.rs`: the RSS Downloader (Feeds list with Add / Edit / Remove / Update and a menu; the Filters tab with a check-box list, Add / Remove / Up / Down, Name, Filter, Not, Save in, Feed, Quality popup, Episode number, Minimum interval, Label, Smart ep. filter, highest priority, paused, and a live preview; the History tab with Clear History) and Add / Edit RSS Feed (URL from the clipboard, alias, the subscription radios, smart episode filter). Edits apply live; filters run over the feeds' items when the dialog closes. `app.rs`: Options > RSS Downloader..., a toolbar button (`Icon::Rss`), the Feeds branch in the tree with µTorrent's feed menu, a feed's items in the main list (own columns, double-click to download, torrent commands off), the timer starting due refreshes and saving `rss.dat` beside `patorrent.ini`; adds go through `add_to_session` (label, paused, highest priority = top of queue; a torrent already listed is skipped and logged). Messages `WM_APP + 7` and `WM_APP + 40`, commands 134–140.
  - nox `webui/rss.rs`: its own lock in `Shared`, always taken before `State`, fetches never under a lock; `rss.dat` in the config directory; run on the status tick (so with `--webui` only); adds through `commands::add`; `rss` in the WebSocket `tick` when it changed. Native `GET /pa/rss`, `/pa/rss/items`, `/pa/rss/preview`, `POST /pa/rss/feed-*`, `filter-*`, `download`, `history-clear`, `process`. The shim's `rss/*` (addFeed, removeItem, moveItem, refreshItem, markAsRead in memory, items, setRule, renameRule, removeRule, rules, matchingArticles; addFolder answers 409), written from qBittorrent's published WebAPI docs only (D15).
  - The page: the RSS Downloader dialog (Items, Filters, History) with the nested Add RSS Feed dialog, an RSS toolbar button, the tree's Feeds branch opening a feed's items, the phone's More menu entry and a full-screen sheet; `menuHost()` now means the topmost open dialog. Static files 187.8 KB of the 200 KB budget.
- **Auto Shutdown** (Options > Auto Shutdown). `frontend::auto_shutdown`: Disabled plus Quit / Standby / Hibernate / Shut Down when downloads or everything complete; arms only after it has seen work while set, then a 30 s countdown that new work aborts; resets to Disabled after acting or on Cancel, never saved. Busy: moving torrents; for "downloads", started torrents not finished, checking or fetching metadata (scheduler-held ones too); for "everything", any started torrent. Stopped, paused and errored torrents are not work. Core: `SessionHandle::save_resume_data()` writes every torrent's resume data on the engine thread and answers `Alert::ResumeDataSaved` (mock too). Win32 `shutdown.rs`: the topmost countdown with Cancel; `Power::available()` greys what the machine cannot do (`IsPwrSuspendAllowed` / `IsPwrHibernateAllowed`); `perform()` enables SE_SHUTDOWN_NAME and calls `SetSuspendState` or `ExitWindowsEx(EWX_SHUTDOWN | EWX_POWEROFF, planned)` without EWX_FORCE, stubbed in tests, reported only with `PATORRENT_POWER_DRY_RUN`. Quit is File > Exit's clean path; Shut Down runs after it; Standby / Hibernate save first and act on `ResumeDataSaved`. nox: `run --exit-when-done downloads|all` (quit only, 5 s grace).
- **Torrent Properties on the Web UI page**: its own `<dialog id="props">`, `// ---- Torrent Properties` and `#props` sections mirroring the Win32 General tab; from the context menu (and so the phone's action sheet) and a double-click on an unfinished torrent; several torrents with blank / indeterminate fields and three-state boxes, only changed fields sent; greying as in Win32; a server 400 shown under its field (a bad tracker's line selected); Enter / Ctrl+Enter / Escape, the Back-button entry and a full-screen sheet under 768 px. The GET also takes the view's filter (`all=1&category=&label=&search=`, for Ctrl+A) and returns `name` and `finished`; `Shared::query_many` collects tracker lists 32 at a time within one 5 s deadline instead of up to 5 s per torrent.
- **nox as a Windows service** (`service.rs`): `service install | uninstall | start | stop | run | debug`. `PaTorrentNox` ("PaTorrent nox"), LocalService by default (`--account NetworkService|LocalSystem`; built-in accounts only, no password), delayed auto-start (`--start`), the image path quoted per Windows's argument rules with the `run` options made absolute (install asks `run`'s parser which options take values; refuses torrent files, `--daemon`, `--no-state`; warns about `--webui-pass`, a missing `--webui` and an exe inside the user profile). State in `%ProgramData%\PaTorrent\nox` (not LocalService's hidden `%APPDATA%`), created with an ACL for SYSTEM, Administrators and the service account only; an existing folder gets the `icacls` command printed instead. Downloads in `<state>\Downloads`, `--log` defaulting to `nox.log` there. Restart on failure (5 s, 30 s, 60 s; reset after a day), 60 s preshutdown. START_PENDING → RUNNING at the point systemd gets READY, STOP_PENDING → STOPPED with exit codes 0 / 1 failed / 2 command line / 3 log; heartbeat checkpoints every second for up to 2 min. Stop and shutdown go through `signals.rs`'s clean path, PARAMCHANGE reloads like SIGHUP, logoff is ignored. `service debug` runs it in a console printing each status. docs/DAEMON.md has the Windows service section (the Task Scheduler stays as an alternative). windows-sys features `Win32_Security`, `Win32_Security_Authorization`, `Win32_System_Services` (+ `Win32_UI_Shell` for tests), `Win32_System_Power`, `Win32_System_Shutdown`.

### Decisions
- Feed items show in the Win32 main list but inside the dialog on the page. The nox RSS lock is ordered before `State`.
- Auto Shutdown: not persisted, armed only after seeing work, no EWX_FORCE, "everything" waits for seeds to stop; nox only quits, no page menu item.
- The service uses built-in accounts only; the state folder's ACL is set only when install creates the folder; on Windows the reload line reads "PARAMCHANGE: reloading".

### Verified
- Each branch on Windows (build, `cargo test --workspace` on 1.90.0, stable clippy `-D warnings`, fmt, Linux-target clippy). Merged main (`c0b1a83`) on Windows: 901 passed, 0 failed; stable clippy `-D warnings`, both fmt checks and Linux-target clippy clean.
- Torrent Properties page in the built-in browser and headless Edge (one and two torrents, double-click, bad tracker and rate, Escape, Ctrl+A, mobile preset, dark mode, a short window).
- RSS: the page in the built-in browser against a loopback feed (feed, filter with preview, Items, a failed download forgotten, filters on Close adding two labelled torrents, the tree's Feeds branch, the phone viewport); Win32 `--demo 3` with a seeded `rss.dat` (fetch, filter, History, `rss.dat` saved; the window not seen). `crates/nox/tests/rss.rs` runs the endpoints and shim against a loopback feed server.
- Auto Shutdown by hand: Quit When Downloads Complete (countdown, clean exit at 32 s), Shut Down + Cancel, and Hibernate / Shut Down with a build whose OS call failed on purpose (reverted); nox `--exit-when-done downloads`.
- Windows service, unelevated: install refused with "Access is denied", `service run` outside the SCM fails with 1063 and a hint, `service debug` prints START_PENDING / RUNNING / STOP_PENDING / STOPPED. The elevated SCM run is pending.

### Issues
- `super_seed::two_leeches_finish_while_the_super_seed_uploads_about_one_copy` failed once in a full Windows run too (not FreeBSD-only); fixed in the next entry (2026-10-19_104021_1792399221).

## 2026-10-19_092115_1792394475: nox daemon verified on Linux and FreeBSD

### Fixed
- The `webui.json` migration message named the new path as its source; it now reads "moved webui.json from OLD to the config directory NEW" (`fd0c725`, merge `93087a8`).
- The system unit's header installs the sysusers file with `install -D`: Ubuntu has no `/etc/sysusers.d` until something creates it.
- docs/DAEMON.md says where `--daemon` output goes with and without `--log` (with `--log`, normal output including the temporary password goes to the log from the start; errors reach the terminal until the daemon is up).

### Verified
- Xubuntu 26.04.1 and FreeBSD 15.1: `cargo test --workspace --no-fail-fast` on the merged tree, 837 passed, 0 failed on each, including both Unix tests in `crates/nox/tests/daemon.rs`. Windows: 865 passed.
- By hand on both VMs: `run --daemon --pid-file --log --webui` returns 0 once up (PPID 1, no terminal, not a session leader, cwd `/` on Linux), the Web UI answers, a second instance is refused with "already running (pid N)", SIGHUP reopens a moved log, SIGTERM stops cleanly and removes the pid file. XDG migration on Xubuntu (old `webui.json` moved to `~/.config`, state left in `~/.local/share`).
- systemd 259 on Xubuntu: the user unit and the system unit with the sysusers file (`systemd-analyze verify` clean; active only after READY with the status line; reload round trip; stop with exit 0; `/var/lib/patorrent-nox` mode 0750; `systemd-analyze security` 1.1 "OK"; interface / route listing and NAT-PMP / UPnP mapping work inside the sandbox). Everything installed was removed again.
- FreeBSD: `sh -n` on the rc.d script (not installed: no root).

### Issues
- `super_seed::two_leeches_finish_while_the_super_seed_uploads_about_one_copy` failed once in FreeBSD's first full parallel run ("the seed uploaded 1.82 copies"); 5 of 5 passes alone and a clean next full run. Recorded as a timing flake in PLAN.md.

## 2026-10-19_091815_1792394295: Torrent Properties and seeding goal, .torrent folders, nox daemon mode, RSS core (four parallel branches)

Built in parallel by four sub-agents in their own worktrees and merged into main: `0a1daad` (merge `c8a53af`), `f5ace82` (`83b934e`), `9c94a42` (`7d5864b`), `d00ac83` (`5dfe903`). Two of the worktrees started from the stale `origin/main` (8c9ad1e, 27 commits behind); they were brought onto local main before committing.

### Added
- **Torrent Properties (Phase 5).**
  - Engine: `TorrentHandle::set_max_connections` / `set_max_uploads` for running torrents (0 = no limit). Lowering the connection limit disconnects the excess peers in libtorrent's `compare_disconnect_peer` order, incoming peers over the limit are refused ("too many connections"), and the choker applies `max_uploads` per torrent (optimistic unchoke exempt; `max_uploads` was saved before but never used). `TorrentStatus::connections_limit` / `uploads_limit`. `replace_trackers` / `add_tracker` with `AnnounceEntry::new(url, tier)`: sorted by tier, blank and repeated URLs dropped, kept URLs keep their state (no second `started`), replies after a change matched by URL, the list saved in resume data. `TorrentFlags::DISABLE_DHT` now stops DHT announces (it was ignored), and `disable_dht` / `disable_lsd` / `disable_pex` persist in resume data. The mock engine handles the new commands. New loopback tests in `crates/core/tests/torrent_properties.rs`.
  - `frontend::properties`: the dialog model for one or several torrents (differing values load blank or `Tri::Mixed` and stay untouched), validation with `FieldError`, tracker text ↔ tiers (blank line between tiers; http / https / udp only), `Changes::apply` and `Changes::store`.
  - `frontend::seeding`: µTorrent's seeding goal, decided client-side because libtorrent's `share_ratio_limit` / `seed_time_limit` only rank seeds in the queue. Applies to auto-managed, running, finished torrents (never force-started ones): stop, or cap the upload rate when "Limit the upload rate to X kB/s" is set with X > 0. Defaults 150 %, no minimum time, stop. Keys `queue.seed_ratio`, `queue.seed_time`, `queue.seed_limit`, `queue.seed_limit_kib`; per-torrent overrides `torrent.<info-hash>.seed_ratio` / `.seed_time`, removed with the torrent.
  - Win32: `properties.rs`, the Torrent Properties dialog (General tab: Trackers; Bandwidth Settings with upload / download rates, upload slots and "Override default number of upload slots"; Seeding Goal with its override; Initial Seeding, Enable DHT, Peer Exchange), several torrents at once with three-state boxes, from the context menu's Properties and by double-clicking an unfinished torrent. Preferences > Queueing's Seeding Goal box is live; the goal is enforced on status updates.
  - nox: `GET` / `POST /pa/torrents/properties` on the same model; the shim's `editTracker`, `addTrackers`, `removeTrackers`, `setShareLimits`; the seeding goal on the Queueing page and enforced in the alert loop. docs/WEBUI.md and docs/PORTING.md (three divergence rows) updated.
- **.torrent folders (Preferences > Directories "Location of .torrents").** `frontend::torrent_folders`: "Store .torrents in", "Move .torrents for finished jobs to", "Automatically load .torrents from" and "Delete loaded .torrents", stored as `dir.store_torrents`, `dir.torrents`, `dir.move_torrents`, `dir.finished_torrents`, `dir.autoload`, `dir.autoload_dir`, `dir.autoload_delete` in `GuiOptions`. `Poller` scans the auto-load folder every 2 s and hands out a `*.torrent` once its size and mtime agree on two scans; `TorrentFiles` follows the alerts: the copy is stored only after the engine reports the add, the auto-loaded file is renamed to `.torrent.loaded` (or deleted) only then, a failed add leaves it until it changes, and a finished torrent's copy is moved (rename, or copy and delete across drives). Copies are named `<torrent name>.torrent`, sanitised for Windows, `<name>.N.torrent` for another torrent of the same name. Win32: the placeholders became live controls greyed as µTorrent's, applied without a restart; the timer runs the poller (auto-loaded torrents skip the Add dialog); every add path goes through `add_to_session`. nox: the same fields on the Web UI's Directories page (`enabled_by` takes several ids), the poller on the status tick (so only with `--webui`), copies stored through `commands::add`.
- **nox daemon mode (Phase 6).**
  - `paths.rs`: on Unix the library stays in `$XDG_DATA_HOME/patorrent/nox` (default `~/.local/share`; data you would miss, so DATA rather than STATE), the config (`webui.json`, default `ipfilter.dat`) moves to `$XDG_CONFIG_HOME/patorrent/nox`; systemd's `$STATE_DIRECTORY` / `$CONFIGURATION_DIRECTORY` win; empty or relative XDG values are ignored. `--state-dir` (and the old `--state`) and `--config-dir`; the config follows an explicit state directory. With the default directories `webui.json` is moved and `ipfilter.dat` copied to the new config directory. Windows: `%APPDATA%\PaTorrent\nox`, or `nox` beside the exe in portable mode (`patorrent.ini` there).
  - `signals.rs`: SIGINT / SIGTERM stop cleanly (resume data saved, port mappings deleted), a second one exits at once; SIGHUP reloads the IP filter (`WebUi::reload_ip_filter`) and reopens the `--log` file; SIGPIPE ignored. Hand-declared `sigaction` on Linux and FreeBSD (`signal()` elsewhere), handler → atomic + socket pair. Windows: `SetConsoleCtrlHandler`, waiting for the save on close / logoff / shutdown.
  - `daemon.rs`: `--daemon` (double fork, `setsid`, `chdir /`, returns 0 once the session and Web UI are up), `--pid-file` (locked: `flock` / exclusive share mode; a second instance is refused), `--log FILE` (mode 600), hand-written sd_notify (`READY=1` with `STATUS=`, `RELOADING=1`, `STOPPING=1`; path and abstract sockets).
  - `contrib/`: hardened systemd system unit (`Type=notify`, `User=patorrent`, `StateDirectory=`, ProtectSystem=strict and friends, syscall filter) with a sysusers snippet, a user unit, and a FreeBSD rc.d script; `.gitattributes` keeps `contrib/**` LF. `docs/DAEMON.md`; README links to it. nox uses `windows-sys` on Windows (already a workspace dependency).
- **RSS core (Phase 5, no GUI yet).**
  - `patorrent_core::fetch` (`net/fetch.rs`): a public blocking `http_get` for http and https (OS TLS), 30 s deadline, 16 MiB body cap, 5 redirects (a redirect to `magnet:` is returned, not followed), cookies, User-Agent `PaTorrent/<version>`, the engine's proxy code (`FetchProxy::from_settings` follows `proxy_tracker_connections`). Reuses the HTTP parser (`proto::http::parse_response_limited`). `patorrent_core::xml` exported. 14 loopback tests; live HTTPS checked against cloudflare.com, releases.ubuntu.com and distrowatch.com.
  - nox's `webui/fetch.rs` wraps it, so `torrents/add` and the native add take https:// URLs.
  - `frontend::rss`: RSS 2.0 / RSS 1.0 / Atom parser on the XML scanner (enclosures, torrent-namespace `magnetURI` / `infoHash` / `contentLength` / `fileName` under any prefix, entities, CDATA, BOM, Latin-1, RFC 822 / RFC 3339 dates, size and depth caps); episodes from titles (`S01E02`, `1x02`, air dates, `Episode 12`, anime numbering), episode ranges and a quality mask; the µTorrent 2.2.1 RSS Downloader model (feeds with alias, interval, `:COOKIE:` URLs and the subscribe flag; filters with Filter / Not wildcards, Save in, Feed, Quality, Episode number, Label, Minimum interval, Smart ep. filter, highest priority, add paused; bounded per-feed history; `due_feeds` / `next_due`; `AddRequest`s out); `rss.dat` (bencode, atomic save). Fuzz target `rss` in CI (CI also fuzzes on `crates/frontend/**` changes); deterministic mutation tests.

### Fixed
- nox: Ctrl+C killed the process outright (no resume save, no port-mapping deletion); only `q` + Enter stopped it cleanly. Now every stop signal does.
- The fetch helper sets timeouts on the socket it reads from: on Windows a `try_clone`d socket is a separate socket with its own timeouts.
- `fuzz_targets/nox_http.rs` did not build since `Limits` gained `max_message`.
- A deadlock in the new `setShareLimits` handler (a lock guard held into the command), found and fixed before the merge.

### Decisions
- The seeding goal is enforced by the frontends, not the engine (libtorrent's limits never stop a torrent). With the 150 % default, existing queued seeds now stop at a 1.5 ratio in both the GUI and nox, as in µTorrent.
- nox's `.torrent` auto-load and copies run with `--webui` only (no `run --watch DIR`).
- nox service files live in `contrib/` because `/dist` is ignored build output. SIGHUP does not re-read `webui.json`, which the running daemon owns.
- Several µTorrent 2.2.1 details were written from memory and are listed in PLAN.md to check against a real 2.2.1 (RSS default refresh 15 min, quality list, episode syntax; the seeding goal's 0 / both-targets semantics; the auto-load polling interval and duplicate naming).

### Verified
- Each branch on Windows: `cargo build --workspace`, `cargo test --workspace` (1.90.0), `cargo +stable clippy --workspace --all-targets -- -D warnings`, `cargo +stable fmt --all --check`, and Linux-target clippy. `fuzz.bat rss 60`: 571,257 runs, no crash. The .torrent folders were also tried in `patorrent.exe --demo 0` (auto-load, `.loaded` rename, stored copy moved on finish).
- Merged main (`5dfe903`) on Windows: `cargo build --workspace`, `cargo test --workspace` (865 passed, 0 failed), `cargo +stable clippy --workspace --all-targets -- -D warnings`, `cargo +stable fmt --all --check` and the fuzz workspace's fmt check all pass. Linux / FreeBSD runs on the VMs: pending at the time of this entry (see the next entry if any).
- Not clicked through by hand: the Win32 Properties dialog and the Directories / Queueing pages, the Web UI's new preferences.

## 2026-10-18_235124_1792360284: the weekly scheduler (Phase 5)

### Added
- µTorrent 2.2.1's weekly scheduler. `frontend::scheduler` holds the model, shared by both clients: a 7 × 24 table (Monday 00:00 first), each hour Full speed, Limited, Turn off or Seeding only; the Limited upload / download rates (kB/s, 0 = unlimited); and "Disable DHT when turning off" (on by default). These are stored as `sched.enable`, `sched.table` (168 mode codes: 0 full, 1 off, 2 limited, 3 seeding only), `sched.ul_rate`, `sched.dl_rate` and `sched.dis_dht`, and are part of `GuiOptions` (`GuiOptions::schedule`). `Scheduler::update` follows the clock and returns what changed (settings, session pause / resume, upload mode on or off) only when the hour's mode changes, or on every Apply.
  - Full speed uses the configured settings. Limited replaces the global rate limits. Turn off pauses the session (`Session::pause`, which keeps each torrent's own state) and switches the DHT off when asked. Seeding only puts every torrent into upload mode.
- Engine: `TorrentFlags::UPLOAD_MODE` is now honoured as libtorrent does it. A torrent in upload mode is never interested and requests no blocks; switching the flag sends INTERESTED / NOT_INTERESTED to every active peer at once. Before this, only the web seeds checked it. Resume data does not save it, so a restart starts every torrent in normal mode and the scheduler applies it again.
- `core::util::time`: `unix_now` and `local_utc_offset` (`localtime_r`'s `tm_gmtoff` on Unix, `GetLocalTime` / `GetSystemTime` on Windows). The offset is asked of the OS each time, so a DST change is followed. nox needs this; it had no local time before.
- Win32: the Preferences > Scheduler page is live (it was a placeholder). It has the Enable Scheduler box; the table as its own control (`sched_grid.rs`: a click moves a cell to the next mode, a drag paints that mode over the cells it crosses, including cells skipped by a fast drag; below the table, a legend and the hovered hour); the two Limited rates; and Disable DHT. The table and rates grey out while the scheduler is off. The app runs the scheduler on its one-second timer and on Apply, logs "Scheduler: <mode>" in the Logger tab, and shows "| Scheduler: ..." in the status bar's network part while the scheduler holds the session back. Torrents added or restored during Seeding only hours get upload mode too.
- nox: the scheduler runs on the Web UI's status tick (so only with `--webui`, where the preferences live) and on every preferences commit. `apply_limits` now goes through it, so the alternative limits and the scheduler's Limited rates combine as expected. `/pa/prefs` has the five `sched.*` fields; the table is a new field type `schedule`, validated as 168 codes. The Scheduler page lays them out as the Win32 dialog does. The page (`app.js`) draws the table as a pointer-driven grid that works with mouse and touch (click or tap to cycle, drag to paint, with gaps filled), plus a legend and the hovered hour.

### Verified
- New tests: the engine swarm test `upload_mode_downloads_nothing_until_it_is_switched_off` (it fails without the engine change); `frontend::scheduler` unit tests (local day and hour across zones, table round trip and bad tables, the mode cycle, the settings per mode, transitions only at mode changes and on Apply); the Win32 hidden-dialog test `the_scheduler_page_edits_the_schedule` (a simulated click and drag, the rates, an untouched grid keeping its table) plus grid hit-testing and gap filling; the nox prefs test `the_scheduler_fields_edit_the_schedule`; and `util::time`.
- `cargo test --workspace` passed on Windows; `cargo +stable clippy --workspace --all-targets -D warnings`, the same clippy for `x86_64-unknown-linux-gnu`, and `cargo +stable fmt --all --check` are clean.
- In the desktop app's browser pane against `run --no-state --mock-library 50 --webui 8090`, at desktop width: the Scheduler page showed the greyed table; Enable, a click and a drag across Tue 01:00–04:00 painted Limited (the first try skipped Tue 03:00 on a fast drag, which led to the gap filling), and Apply stored the table (`GET /pa/prefs` read back `022220` for Tue 00:00–05:00).
- Not checked: the Win32 page by hand; the effect of an actual hour change on a live session (the mock library does not show a session pause). The per-mode effects are covered by the unit and swarm tests.

### Decided
- Seeding only uses libtorrent's per-torrent upload mode on every torrent. A frontend-side list of torrents paused by the scheduler would have to survive restarts and clash with the user's own Pause; upload mode is not persisted and leaves finished torrents alone.
- The qBittorrent shim's scheduler preferences (`scheduler_enabled`, `schedule_from_hour`, ...) are not mapped: qBittorrent's scheduler switches the alternative limits on a daily window, which does not fit the 2.2.1 table.
## 2026-10-18_233807_1792359487: the shim's setLocation and queue moves; Back closes the phone's sheets

### Added
- The qBittorrent shim (`webui/api.rs`) gets `torrents/setLocation` (`hashes`, `location`) and `torrents/topPrio` / `bottomPrio` / `increasePrio` / `decreasePrio` (`hashes`, `all` allowed), POST only, as adapters onto `commands.rs`: the queue moves are `/pa/torrents/queue`'s top / bottom / up / down (several torrents keep their relative order); `setLocation` is the native Set Download Location. Following the WebAPI documentation, `setLocation` creates the directory first: 400 for an empty location, 403 when creating it is refused (`PermissionDenied`), 409 when it fails otherwise (a file in the way). An unknown hash is no error, as in the other shim commands. docs/WEBUI.md names them.
- `app.js`: on the phone the drawer, the detail sheet, the menu sheets (actions and More) and the Add / Preferences dialogs each hold a `history` entry while open (`layerOpened` / `layerClosed`, a stack of close functions), and a `popstate` closes the topmost layers down to the entry's depth (`{paLayer: n}`). A layer the page closes (its close button, the scrim, Escape, a menu item, Cancel) gives its entry back with `history.go`, a turn later, so a close followed by an open (More > Preferences, the next file's Add dialog) costs no entry. The desktop pushes nothing, and switching to the desktop layout drops the entries; entries left over from a reload are dropped at start.

### Issues
- The first `topPrio` handler deadlocked: `commands::queue(shared, &selected(&shared.lock(), &p), m)` keeps the lock guard alive for the whole statement, and `queue` locks again. The ids are now taken in their own statement, as the other shim handlers already note.

### Verified
- `cargo test -p patorrent-nox`: 73 passed, including the new `shim_queue_moves_and_set_location` (each move against the `priority` field, `all`, 405 on GET, the directory created and two of three torrents moved, 400 for an empty or blank location, 409 below a file). `cargo +stable clippy --workspace --all-targets -- -D warnings` and `cargo +stable fmt --all --check` clean.
- In the desktop app's browser pane against `run --no-state --mock-library 200 --webui 8080` at 375 x 812, with `history.back()` standing in for the Back button: a tap opens the detail sheet (depth 1) and back closes it; five open / close-button cycles leave the history at depth 0; the actions sheet, the drawer (back and scrim), the sheet with the More menu over it (two backs close them in order), More > Preferences (one entry) with back and with Cancel all end at depth 0. At desktop width a context menu and Preferences push nothing, and a phone-to-desktop switch with the sheet open drops its entry.
- Not tried on a real Android phone (Chrome there routes Back to a modal dialog's close watcher first; the dialog's `close` event then gives the entry back the same way).

## 2026-10-18_232431_1792358671: Web UI slice 8, labels in the tree

### Added
- The page's category tree shows the Labels branch as the Win32 tree does: "No Label" first, then every label, each with its torrent count, live. `webui/native.rs` (`Client::labels_update`) adds `labels` (`[[name, count], ...]`, sorted by name) and `no_label` to a `tick` whenever either changed since that client's last one; a fresh socket gets them with its first tick. The names are the daemon's category map plus any label in use, so a label created with no torrent yet stays listed with 0 (the map is persistent, qBittorrent's category semantics, unlike 2.2.1, which drops a label with its last torrent).
- `app.js`: "No Label" filters on the empty label (`{"label": ""}`, which `parse_view` already took); the filter code now tells `null` (no label filter) from `""` (No Label). When the names change (another client, or the qBittorrent shim's `createCategory` / `removeCategories`) the tree is rebuilt; if the selected label went away the view falls back to all torrents, as the Win32 tree does.

### Verified
- `cargo test -p patorrent-nox`: all passed; the native protocol test now checks the tick's `labels` / `no_label`, that they are sent once, a label with no torrent, and the No Label filter. `cargo +stable fmt --all --check` and `clippy -p patorrent-nox --all-targets -D warnings` clean.
- In the desktop app's browser pane against `run --no-state --mock-library 300 --active 0.1 --webui 18080`: the tree showed No Label 300; Label > New Label... on three rows made it No Label 297, linux 3 within a tick; No Label and linux filtered to 297 and 3 rows; `torrents/removeCategories` through the shim removed linux from the tree and moved the view back to Torrents (300).
- Not checked at phone width (the drawer is the same tree element).

## 2026-10-18_230237_1792357357: Web UI slice 7, the phone layout

### Added
- The page's phone layout (docs/WEBUI.md slice 7), under 768 px. `app.js` switches on the same media query as `style.css`, live when a phone rotates or a window is resized: the row height, the subscribed columns and the place of the detail all change, and the view is sent again.
  - The list: no header, one 60 px row per torrent: Name, then the progress bar with its percentage, then status, down / up speed and ETA. `PHONE_COLS` is a fixed 6-column subscription in place of the header's columns; with the 10,000-torrent profile the first `rows` is 6.6 KB for 34 rows. No horizontal scroll.
  - The category tree is a drawer behind the toolbar's menu button; picking a category closes it.
  - A tap opens the torrent's detail tabs as a bottom sheet with the name and a close button on top. The phone subscribes to the detail only while the sheet is open.
  - Each row has a visible actions button that opens the context menu as a bottom sheet. A submenu replaces the sheet's items, with a back item first. On the phone the menu drops "Open Containing Folder" and starts with "Details".
  - A long press (450 ms; a 10 px move or a scroll cancels it) starts the selection mode: round check marks on the rows, a tap toggles a row, the toolbar's Start / Pause / Remove act on the selection, and Done ends it (Done takes Add's place while it shows).
  - The toolbar keeps the menu button, Add, Remove, Start, Pause and a More menu: Add Torrent from URL, Stop, Move Up / Down Queue, Select All, Sort By, Sort Descending, Preferences, Log Out. More also holds the sort that the header gives the desktop. The search box gets its own line.
  - The Add New Torrent and Preferences dialogs fill the screen. Toolbar buttons, tree items and menu items are finger-sized. A scrim behind the drawer and the sheets closes them on a tap, and Escape closes them too.

### Decided
- docs/WEBUI.md's open point, a long-press context menu or a visible per-row menu button: the button. A long press is the multi-selection, as in phone apps generally. A visible button is easy to find and does not race the browser's own long-press handling. On the phone a touch's `contextmenu` is ignored; a mouse right-click still opens the menu sheet.

### Issues
- At first the phone row showed no speeds: the native protocol's column ids are `down_speed` / `up_speed` (`webui/native.rs` `column_id`), not the `frontend::columns` layout keys `dlspeed` / `ulspeed`. Fixed before the commit.
- The first desktop check after the change showed every toolbar button at full width: the new `.dt` visibility class collided with the detail tables' existing `.dt` (`min-width: 100%`). Renamed to `only-desk` / `only-phone`.

### Verified
- In the desktop app's browser pane against `patorrent-nox run --no-state --mock-library 300 --active 0.1 --webui 18080`, at 375 x 812 and 360 x 740:
  - the list, the drawer and a category
  - a tap to the detail sheet
  - the actions sheet, into Bandwidth Allocation > Set Download Limit (check mark on Unlimited) and back
  - a long press with touch pointer events into the selection mode, toggles, Done
  - the More menu, and Preferences full screen
- At desktop width the 2.2.1 layout is unchanged, and a live switch desktop > phone > desktop keeps the selection and brings the detail pane back.
- The 10,000-torrent profile at phone size: first `rows` 6,661 B (34 rows, 6 columns); ticks with changes 440 B to 2 KB.
- Not yet tried on a real phone.
- `cargo test -p patorrent-nox`: 72 passed. This includes the static-file test (no inline script or style; the three files are 138 KB of the 200 KB budget). No Rust changed.

## 2026-10-18_182203_1792340523: merge of the scrape-backoff fix and the two deflaked tests

Branch `claude/eager-proskuriakova-7ccee3` (commit `4c9fa62`) fast-forwarded onto main; no conflicts.

### Verified
- On main after the merge, Windows: `cargo test --workspace --no-fail-fast`: 775 passed, 0 failed, 3 ignored. The merged commit is the one the branch ran four times clean on the Xubuntu VM (entry below), so the VM run was not repeated.

## 2026-10-18_161548_1792332948: a failed scrape no longer puts a tracker into its retry backoff; the two tests flaky under the 24-CPU run

### Fixed
- `crates/core/src/engine/mod.rs` (`apply_tracker_results`): a failed *scrape* was handled like a failed announce: `TrackerEntry::failed` raised the failure count, showed the tracker as `Error` with the scrape's message and set its next announce 17.5 s away (then 36 s, ...; libtorrent's backoff formula). libtorrent's `torrent::tracker_request_error` only posts `scrape_failed_alert` for a scrape request and leaves the announce entry alone. The engine now does the same: a failed scrape raises the new `Alert::ScrapeFailed { id, url, message }` (`"<url>: scrape failed: <message>"`, Critical priority like `ScrapeReply`, carries the torrent id) and changes nothing on the tracker entry; failed announces are handled as before. Besides the test below this is visible in the tracker list: a tracker whose announces work but whose `/scrape` does not (no scrape support, a redirect, a body that is not a scrape) stays `Working` with no failure count and no retry countdown.
- Why `engine::listen::session_tests::a_lost_internet_address_resets_the_votes_and_reannounces` failed now and then under the full parallel run on the 24-CPU VM: the test's tracker reply carries neither `complete` nor `incomplete`, so the engine scrapes right after it, and the tracker answers the scrape with the same announce body, which does not parse (`invalid scrape response`). Measured with temporary timestamps on Windows: the scrape fails about 1 ms after the first announce; the test's network change, whose `reannounce_all` clears `fails` and `next_announce`, follows about 8 ms later; the second announce then goes out on the next 100 ms tick. Under load that order flips: the reset comes first, the scrape failure then sets `fails = 1` and `next_announce = now + 17.5 s`, `trackers_to_announce` holds the tracker until then, and the test's 10 s receive expires. With the fix a scrape failure cannot touch the schedule in either order. The test keeps its 10 s receive (the announce now waits on one tick and a loopback round trip) and additionally captures the torrent id, waits for the `ScrapeFailed` alert and asks for the tracker list after the second announce: one tracker, `fails == 0`, status `Working`. With the old engine code that check fails every time, since the second reply's scrape failure marked the tracker `Error` too.
- `crates/nox/tests/webui.rs`: once a view is subscribed the Web UI's status ticks arrive on their own schedule, so the reply a test waits for can have a `tick` frame in front of it (`websocket_handshake_and_frames` read `tick` where it expected the `error` for the fragmented `{"t":"nothing"}` frame). The `Ws` helper's `recv_json` now reads past `tick` text frames (`recv_frame_skipping_ticks`; `is_tick` parses the payload), the pong check after the ping uses the same, and the helpers that read ticks on purpose (`tick_until`, `cells_until`, `expect_only_ticks`) use the new raw `recv_message`. The other websocket tests were checked for the same race: the close-frame loops and `recv_sized`'s tick loop already tolerated ticks, and `detail_until` passes them by either way.

### Verified
- Windows: `cargo +stable fmt --all --check`, `cargo +stable clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast`: 775 passed, 0 failed, 3 ignored.
- Xubuntu VM (24 CPUs, 192.168.32.132), the working tree shipped as `git archive --prefix=patorrent/ $(git write-tree)` over SSH: `cargo test --workspace --no-fail-fast` four times in a row (16:15 to 16:17, about 30 s each with the build cached): 755 passed, 0 failed, 3 ignored every time; both previously flaky tests passed in all four.

### Notes
- `git archive HEAD` only ships committed content; for an uncommitted tree, `git add -A && git archive --prefix=patorrent/ $(git write-tree)` archives the index. The remote `tar` warns that every time stamp is under a second "in the future" (the archive is stamped with the current time and the VM clock is a little behind); harmless.

## 2026-10-18_155612_1792331772: new Xubuntu test VM at 192.168.32.132 replaces the Ubuntu VM

### Changed
- The Linux test VM is now Xubuntu 26.04.1 at 192.168.32.132 (user `x`, passwordless sudo, 24 CPUs, 7.7 GiB, 492 GB disk); the Ubuntu VM at 192.168.32.128 is retired. The owner installed `openssh-server` and the `~/.ssh/patorrent_vm` key on the console; over SSH the VM then got `build-essential`, `curl`, `git`, `pkg-config`, `libssl-dev` through apt and rustup with the pinned 1.90.0 toolchain (minimal profile) as user `x`. The tree goes over as before: `git archive --prefix=patorrent/ HEAD | ssh -i ~/.ssh/patorrent_vm x@192.168.32.132 'tar -xf - -C ~'`.
- PLAN.md's VM note points at the new address.

### Verified
- `cargo test --workspace --no-fail-fast` on the new VM, four runs of main at `ce0e3ce`: 755 tests, 3 ignored; one run all passed, the others had one or two timing failures (see below). Each failing test passed three times in a row when run alone.

### Issues
- Two tests are flaky under the full parallel run on a 24-CPU box: `engine::listen::session_tests::a_lost_internet_address_resets_the_votes_and_reannounces` (the `rx.recv_timeout(10 s)` for the second announce after the fake network change expired) and `websocket_handshake_and_frames` in `crates/nox/tests/webui.rs` (`recv_json` returned a periodic `tick` message where the test expected the `error` reply to the fragmented `{"t":"nothing"}` frame; the test does not skip ticks). Neither failed on the old 2- to 4-CPU VMs. Not fixed here; noted in PLAN.md.
- The Transmission / Deluge interop clients (Q10) are still not installed on the Linux VM.

## 2026-10-18_102227_1792311747: merge of Web UI slice 6, verified on Windows and both VMs

Branch `claude/cranky-antonelli-522710` merged into main; the only conflict was both sides adding a CHANGELOG entry at the top (both kept, the slice 6 entry first).

### Verified
- On main after the merge, Windows: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast`: 775 passed, 0 failed, 3 ignored.
- `cargo test --workspace --no-fail-fast` on the VMs (the tree shipped with `git archive | ssh ... tar -x`): Ubuntu 26.04.1, 755 passed, 0 failed; FreeBSD 15.1, 755 passed, 0 failed. This is the first VM run of slices 5 and 6 (the commands, the Add dialog, Preferences).
- By hand in the Chromium pane on the branch's release build with a scratch state directory: Ctrl+P opened Preferences with the 2.2.1 page tree, Bandwidth took a 50 kB/s download limit on Apply with the dialog staying open and the status bar showing `[50.0 kB/s]`, the state file then held `lt.download_rate_limit = 51200` and no plain password, the Advanced filter narrowed the table to the `dht` rows, Escape cancelled (a first Escape inside the filter box only clears it, the browser's search-field behaviour).

## 2026-10-18_101527_1792311327: Web UI slice 6: Preferences generated from `frontend::options`

### Added
- `crates/nox/src/webui/prefs.rs`: the Preferences model behind `GET` and `POST /pa/prefs` (docs/WEBUI.md slice 6). `GET` returns `pages` (µTorrent 2.2.1's tree in order: General, UI Settings, Directories, Connection, Bandwidth, BitTorrent, Queueing, Scheduler, Web UI, Advanced, Disk Cache under Advanced; each page its group boxes of items laid out as the Win32 dialog lays them out, with the same labels and order: `{"f": id}` with optional `kb` (shown in kB/s, bytes on the wire), `min` / `max` (the page control's range), `enabled_by` (the Directories folder and "Only move from" follow "Move completed downloads to", the proxy user and password follow Authentication, the IP filter path follows Enable), `secret`; `{"row": [...]}` for the proxy Type / Proxy / Port and Username / Password rows; `{"stub": label}` for the Win32 dialog's disabled placeholders; `{"note": text}`; `{"button": "Random port"}`), `fields` (every field once: `label`, `type` of `bool`, `int` with `min` / `max`, `text`, `folder`, `enum` with `options`, `password`; `value`, `default`, `readonly`) and `advanced` (every engine setting's id in the table's order). Three kinds of field share the table: every `frontend::settings::TABLE` row by its libtorrent name (the Advanced page), the `frontend::options::GuiOptions` nox has an equivalent for under their preferences keys (`dir.download`, `dir.move_completed`, `dir.completed`, `dir.move_only_default`, `ui.confirm_delete`, `ui.add_stopped`, `ui.show_add_dialog`, `net.random_port`, `bt.pex`, `bt.max_peers`, `bt.upload_slots`, `ipfilter.enable`, `ipfilter.path`; the tray and balloon options are desktop-only and left out), the composites the Win32 pages use (`listen_port` over `listen_interfaces`, `proxy_kind` / `proxy_auth` over `proxy_type` with the 2.2.1 Type combo, `enc_out` / `enc_legacy` through `settings::set_encryption`, `utp` for both uTP directions), the alternative limits (`alt_dl_limit`, `alt_up_limit`, `alt_enabled`, an extra Bandwidth group) and the Web UI's own (`webui.listen` read-only, `webui.username`, `webui.password` write-only, `webui.bypass_local_auth`, `webui.host_header_validation`, `webui.domain_list`, `webui.csrf`, `webui.max_auth_fail_count`, `webui.ban_duration`, `webui.session_timeout`). `POST` takes a partial object of ids to values: everything is parsed (`Entry::parse` for the engine settings, the same rules for the rest; JSON `true`, `"1"` and `1` all read as the texts do) and set on a working copy, checked with the Win32 dialog's cross-field rules (a proxy type needs a host and a port, a download folder, a completed folder when moving, a user name), then committed together: engine settings through `apply_settings` (the `download_rate_limit` / `upload_rate_limit` rows edit the normal global limits, `Shared::apply_limits` sends the mode in force), a block list loaded at once (a list that does not load refuses the change naming `ipfilter.path`), peer exchange toggled on every torrent, a new password hashed, the folders handed to the daemon's `MoveCompleted`; 204, or 400 `{"error": "<field>: <why>"}` naming the first bad field with nothing applied.
- Persistence: everything the Web UI's preferences set is stored in `State::options`, a `frontend::prefs::Prefs` holding the same keys the Win32 client keeps in `patorrent.ini` (`lt.<name>` for the engine settings, the `GuiOptions` keys without the tray ones, `webui.<name>` for the Web UI's own, `add.recent_dirs`), saved as the `options` object of `webui.json` and re-applied by `prefs::restore` at the next start onto the settings the daemon was started with (a stored value wins over the command line, a value never stored keeps the command line's; `net.random_port` picks a new port; the IP filter is loaded unless `--ip-filter` loaded that same file). Decision: the keys go into `webui.json` rather than a second `.ini` file next to it, so nox keeps one atomically written state file; the engine settings are stored when they differ from the defaults, as the Win32 client stores them, plus every row the Web UI changed on purpose (a setting set back to its default must still win over the command line) and every row stored before. A `preferences` object written by earlier builds (the shim's keys) is applied once through the shim's mapping and then dropped. `--move-on-complete` and `--ip-filter` seed the Directories and BitTorrent pages (`Config::move_completed_dir`, `Config::ip_filter`), and `patorrent-nox` follows the Directories page live (`WebUi::sync_move_completed` in the main loop).
- The shim: `app/setPreferences` maps each qBittorrent key onto the native ids (`dl_limit` in kB/s onto `download_rate_limit`, `upnp` onto both mappers, `encryption` onto both policies, `proxy_type` numbers onto libtorrent's, `web_ui_*` onto `webui.*`) and commits them through the same code, so there is one state and one store; a key that does not parse is skipped, and a set that fails the cross-field checks is retried one change at a time, which keeps the old lenient behaviour. `app/preferences` is unchanged for its clients (`pex` now reports the preference).
- `GET /pa/add` answers `show_dialog` from `ui.show_add_dialog` (the page then adds a dropped file with the defaults when it is off); `/pa/add` applies the per-torrent defaults (`bt.pex`, `bt.max_peers`, `bt.upload_slots`, `ui.add_stopped`) through `GuiOptions::apply_to_new` before the dialog's choices, as the Win32 add does; `hello` carries `confirm_delete`, which the page's Remove honours; the status bar's `tick` carries `down_limit` / `up_limit` (empty when unlimited) and the page shows them in brackets.
- The page (`page.html`, `style.css`, `app.js`, 123 KB together): the Preferences dialog (a `<dialog>` like the Add one) from the toolbar button (enabled now) and Ctrl+P: the page tree on the left with Disk Cache under Advanced (arrow keys move between pages), the page on the right generated from the JSON (group boxes, check boxes, number fields with the page's range, text and folder fields, selects, read-only and password fields, the disabled placeholders and notes, the "Random port" button), OK / Cancel / Apply with a message line. Edits are kept per field; Apply sends only the changed ones and keeps the dialog open (re-reading the model so the composites read back exactly), OK sends and closes, Cancel and Escape discard, Enter in a field does nothing (as 2.2.1), Ctrl+Enter is OK, a 400 switches to the field's page, marks the control and shows the message, and the page checks a number field's range before sending with the Win32 dialog's wording. Advanced is the full table with a filter box, sorted by name, changed values in bold, a value editor (True / False for switches, a text field otherwise, Enter sets), Set and Reset; a setting changed there shows on its page and the other way round. The dialog is never taller than the viewport: the pages scroll inside it, the Advanced table takes the room; under 700 px the tree becomes a strip above the page. Scheduler stays a placeholder page saying it is not available yet, as in the Win32 dialog.
- `frontend::prefs::Prefs::entries()` (every key and value) for the state file.
- Tests: `prefs.rs` unit tests (every engine setting and every own field once, the page ids and order, Disk Cache under Advanced, every placed field exists and is placed once, every own field is on a page, defaults read back on a fresh state; the parsing and the refusals: unknown id, a bad int, a range, read-only, a bad bool, null, an enum by number or by label, the first bad field in body order, the Win32 cross-field checks, the composites, the write-only password, the rate limit rows; the stored keys); `tests/webui.rs` `native_prefs_model_and_partial_post` (the model's shape and values against the engine, 403 without a session, the bad fields and nothing applied, a valid partial change seen natively and in the shim's `app/preferences`, `transfer/downloadLimit` and `GET /pa/add`, persisted in the state file without the password or the tray keys, the domain list taking effect for the next request, the old password failing and the new one logging in, the shim's `setPreferences` landing on the same state) and `native_prefs_survive_a_restart` (a pre-slice-6 state file migrated, the listen port and the download folder set through `/pa/prefs`, seen in the shim, nox restarted on the state directory and both re-applied along with a setting set back to its default while the command line's other settings are kept). 48 nox unit tests, 22 in `tests/webui.rs`.

### Notes
- Checked by hand in the Chromium pane against the real engine (`patorrent-nox run --webui 8089 --webui-pass ... --state <scratch> --save <scratch>`, release build): every page opens with the 2.2.1 groups and labels, Apply of a 50 kB/s download limit showed `[50.0 kB/s]` in the status bar with the dialog still open, Advanced filtered to `enable_dht`, Set False showed it in bold and Reset took it back, Cancel discarded a pending Set and a proxy type (Ctrl+P reopened the dialog with the stored values), a Socks5 type without a host came back as a 400 marking the Proxy field with "Please enter the proxy server's address.", Escape cancelled; the state file held `lt.download_rate_limit` afterwards.
- `nox`'s `Prefs` keeps its own fields (`save_path`, `start_paused`, the limits, the Web UI's) and gained the `GuiOptions` ones it had no home for (the folders, `confirm_delete`, `show_add_dialog`, `random_port`, `pex`, the per-torrent limits, the IP filter); `Prefs::gui()` / `set_gui()` convert to and from `GuiOptions` for its helpers and its preferences keys.
- The shim's error messages for `pa/prefs` 400s also flash in the status bar under the dialog's backdrop (the shared `pa()` helper); harmless, left as is.
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast` on Windows: 775 passed, 0 failed, 3 ignored (nox unit 48, webui 22). Not run on the Linux / FreeBSD VMs (nox, frontend and docs only).

## 2026-10-18_094628_1792309588: merge of Web UI slice 5

Branch `claude/festive-thompson-12b587` merged into main; the only conflict was both sides adding a CHANGELOG entry at the top (both kept, the slice 5 entry first).

### Verified
- On main after the merge: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast` on Windows: 769 passed, 0 failed, 3 ignored.
- By hand in the Chromium pane on the branch's release build against the real engine: a `.torrent` dropped on the page opened the Add New Torrent dialog with its file tree; a folder unchecked (the size showing "4.30 MB of 6.39 MB selected"), High Priority from the file context menu, a label typed, OK; the torrent appeared with that size and label, the Files tab showing Don't Download / High / Normal; Pause, Stop and Start from the toolbar showed Paused, Stopped and Downloading; Low Priority from the Files tab's menu.
- Not run on the Ubuntu and FreeBSD VMs (nox only; the next VM run covers it).

## 2026-10-18_093747_1792309067: Web UI slice 5: the native commands and the Add dialog

### Added
- `crates/nox/src/webui/commands.rs`: one implementation of every command behind both APIs (docs/WEBUI.md "Commands"). `POST /pa/torrents/start`, `force-start`, `pause`, `stop` (through `frontend::actions`; the user's Pause is recorded per torrent in `State::user_paused`, kept in the state file, and fed into each client's `TorrentList`, so the cells and the status class say "Paused" as the Win32 list does; Start, Force Start, Stop and the shim's stop clear it), `recheck`, `reannounce`, `remove` (`"data": true` deletes the files), `queue` (`"move": "up" | "down" | "top" | "bottom"`; several torrents keep their relative order: Up and Bottom are applied from the lowest queue position, Down and Top from the highest), `limits` (`down` / `up` in bytes per second, 0 unlimited, either optional), `label` (a new label is created, empty removes it), `location` (`move_storage`, torrents already there skipped), `super-seed`, `files` (`priorities` of file index to 0..7, so Low works), `rename` (`old`, `new`, optional `folder`: a file when `old` names one, else a folder); `/pa/add` (multipart `torrent` parts plus an `options` part, or a JSON body with `url`: a magnet, a bare info-hash or an `http://` link) applied through `frontend::add::AddChoice` (`save`, `name` renaming the root on disk, `label`, `start`, `skip_check`, `top`, `priorities`; the save folder remembered in the Save In list, "Add to top of queue" applied by the alert loop when the engine reports the torrent added); `/pa/torrent-info` (parses an upload and returns the Add dialog's Torrent Contents and file list without adding, for the two-step add); `GET /pa/add` (the dialog's defaults: `save_in`, `labels`, `start`, `show_dialog`); `/pa/session/limits` (`GET` and `POST`: `down`, `up`, `alt`, `alt_down`, `alt_up`); `/pa/session/shutdown`. JSON bodies with `Content-Type: application/json` (415 otherwise), 204 on success, `4xx` with `{"error": "..."}`.
- A body names torrents with `"ids": [...]` or `"all": true` plus the client's view filter (`category`, `label`, `search`, the `view` subscription's shape), applied server-side with the same rules as the list, so Ctrl+A under a filter acts on every matching torrent and not only the loaded window.
- CSRF for the native commands: every `POST /pa/...` must carry `X-Requested-With: PaTorrent` (403 `{"error": "missing X-Requested-With: PaTorrent header"}`), besides the shim's `Origin` / `Referer` rule (403 `{"error": "origin not allowed"}` here, the shim keeps its 401). A request without an `Origin` (curl) passes on the header alone. The header name is the design's "custom header"; a browser cannot send it cross-site without a CORS preflight, which the server never answers.
- The shim's `torrents/stop`, `pause`, `start`, `resume`, `setForceStart`, `setSuperSeeding`, `recheck`, `reannounce`, `delete`, `filePrio`, `renameFile`, `renameFolder`, `setDownloadLimit`, `setUploadLimit`, `setCategory` and `add` now call the same functions; their request shapes, status codes and tests are unchanged (`setCategory` still refuses an unknown category with 409 before calling; `add` maps `savepath`, `rename`, `category`, `stopped` / `paused`, `skip_checking`, `sequentialDownload`, `dlLimit`, `upLimit` onto the dialog's options, so a torrent file added through the shim is added exactly as the Win32 dialog adds one).
- The page: every command in the toolbar, the context menu, the keyboard and the Files priority menu goes to the native endpoints (one `pa()` helper: JSON, the CSRF header, the `{"error"}` shown in the status bar); the four queue moves and the toolbar's Up / Down are enabled, Advanced > Set Download Location... prompts for a folder on the server (the shim has no folder browser), the Files menu offers Low Priority, Shift+Delete removes with data, and a selection made with Ctrl+A sends `"all"` with the view's filter. The Add New Torrent dialog (`<dialog>`, the 2.2.1 layout of the Win32 one: Save In with the server's recent folders in a datalist, Name, Torrent Options with Skip hash check, Start torrent, Add to top of queue and Label; Torrent Contents with Name, Comment, Size as "x of y selected", Date, the file tree with check boxes, folder rows that expand and collapse, tri-state folder checks, a Priority column showing Mixed, a context menu with the four priorities plus Check / Uncheck, Select All / None; OK and Cancel) opens for the Add Torrent button, a dropped `.torrent` and, when several are chosen, one after the other; it asks the server for the file tree first (`pa/torrent-info`) and uploads the file with the choices on OK (`pa/add`). A dropped or pasted magnet and Add URL add directly for now. Menus open inside the dialog while it is modal (a modal dialog's top layer would otherwise hide them). The three files are 100 KB together (app.js 80 KB).
- `State` keeps `user_paused`, `pending_top` and the Add dialog's recent folders (`frontend::add::remember_dir` on a `frontend::prefs::Prefs`); the state file gains `paused` and `recent_dirs`.
- docs/WEBUI.md "Commands": the shapes as built (the header, `"all"` with the filter, `GET /pa/add`, the torrent-info reply, the session limits).
- Tests: `commands.rs` unit tests for the selection parsing and the "all with filter" resolution, the queue order, the add options, the torrent-info shape and the CSRF header; `tests/webui.rs` `native_commands_against_the_engine` (three torrents on the real engine: Pause / Stop / Start / Force Start in the cells and the status class, the shim's stop clearing a Pause, queue moves to top, up and bottom read off the `#` column, limits in the Down / Up Limit cells and through the shim, a label in the cell and the shim's categories, "all" under a label filter and a search, super seeding / recheck / reannounce / location accepted and the move seen in `save_path`, the error codes, remove with data deleting the files on disk), `native_add_dialog_flow_and_magnet` (torrent-info then add with a renamed root, priorities, a label and a new folder, seen in the shim's files and the Save In list; a magnet through the JSON body; the refusals; the session limits read back natively and through the shim; shutdown) and `native_commands_are_csrf_checked`; the shim's tests pass unchanged.

### Notes
- Left open, as in the Win32 dialog: a magnet or URL adds without the dialog (the dialog would need the metadata first, PLAN.md Phase 5). The page could show the dialog once `has_metadata` turns true; not done here.
- The server has no "show the add dialog" preference yet (nox's `Prefs` has none; the Win32 one is `ui.show_add_dialog` in the frontend options), so `GET /pa/add` answers `show_dialog: true` and the page always shows it for files; the page honours a `false` by adding with the defaults, for slice 6.
- The context menu's check marks (limits, label, super seeding) still come from one `torrents/info` read of the shim when the menu opens; the native API has no per-torrent read for them yet, and the cells do not carry them.
- A torrent file added stopped sits in `checkingResumeData` (the engine checks on start), which the status text shows as "Checking 0.0 %"; the Win32 list shows the same. Not changed here.
- The shim's `torrents/stop` and `torrents/start` handlers must bind the selection before calling the command: a lock guard held in the argument list deadlocked the first run (the commands take the lock themselves to record the Pause).
- Checked by hand in the Chromium pane against the real engine (`patorrent-nox run --no-state --webui 8089 --webui-pass ... --save ...`, release build; the mock engine was not needed): a dropped multi-file `.torrent` opening the dialog with its tree, a folder unchecked, High from the context menu, a new name and label, Start torrent off, OK; the torrent listed as "Checking", started, paused ("Paused" with the paused class), a magnet through Add URL, Move Down and Move to Top of Queue read off the `#` column, Set Download Location seen in Save As, the Files tab with the renamed root and the dialog's priorities, Low Priority from the Files menu, Ctrl+A under a search with Stop acting on the match only, Shift+Delete. Found and fixed on the way: menus hidden behind the modal dialog, and the Labels group not learning a label the dialog created.
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast` on Windows: 769 passed, 0 failed, 3 ignored (webui 20, nox unit 44). Not run on the Linux / FreeBSD VMs.

## 2026-10-18_090418_1792307058: Web UI slices 1 to 4 verified on the Ubuntu and FreeBSD VMs

### Verified
- `cargo test --workspace --no-fail-fast` on main at f54ca78 (the tree shipped with `git archive | ssh ... tar -x`): Ubuntu 26.04.1, 741 passed, 0 failed across 19 suites; FreeBSD 15.1, 741 passed, 0 failed across 19 suites. This covers the WebSocket, the native protocol, the page's static-file test and the detail tabs, and the engine's `Alert::PieceInfo::downloading`.

## 2026-10-17_224511_1792269911: merge of Web UI slice 4

Branch `claude/intelligent-blackwell-06ca46` merged into main without conflicts (its entry 2026-10-17_224011_1792269611 follows).

### Verified
- On main after the merge: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast` on Windows: 761 passed, 0 failed, 3 ignored.
- The tabs by hand in the Chromium pane against `patorrent-nox run --no-state --mock-library 200 --active 0.1 --webui 8089` (release build of the branch): General, Trackers, Peers, Pieces, Files, Speed and Logger on a downloading torrent, the pane empty with nothing focused.
- Not rerun on the Ubuntu and FreeBSD VMs. This slice touched `crates/core` (`Alert::PieceInfo::downloading`, `PiecePicker::downloading_pieces`, the mock) besides nox, so the next VM run is due before the next engine work.

## 2026-10-17_224011_1792269611: Web UI slice 4: the detail tabs

### Added
- The `detail` subscription answered (`crates/nox/src/webui/native.rs`, docs/WEBUI.md "Detail tabs"): the focused torrent's tab is built from the cache the way the Win32 tabs build theirs (`frontend::detail`'s tracker, peer and file columns and cells, `peer_flags`, `fmt`), sent whole once, then on a status tick only when a hash of its content changed; nothing before the tab's engine reply has landed. The WebSocket thread posts the tab's `post_*` requests (`post_trackers` for General and Trackers, `post_peer_info`, `post_piece_info`, `post_file_list` plus `post_file_progress`) after the subscription and on every tick while subscribed; a switch to another torrent or tab sends the new content at once and stops the old; `id: null` unsubscribes; a torrent removed while subscribed gets one `gone` message. Shapes: General as the Downloaded and Availability bars plus the 2.2.1 Transfer group (Time Elapsed, Remaining, Wasted, Downloaded, Uploaded, Seeds and Peers "x of y connected", Download / Upload Speed, Down / Up Limit, Share Ratio, Status) and General group (Save As, Remaining, Total Size, Added On, Pieces, Completed On, Created On, Created By, Hash, Trackers count, Comment) as label-value pairs; Trackers, Peers and Files as rows of cells with a sort key per numeric column (so the page sorts "1.2 MB/s" by the number, not the text), Files rows with the engine's file index, progress in thousandths and the priority; Pieces as the bitfield run-length encoded plus the pieces being downloaded; Speed as a per-torrent ring of 600 (down, up) samples, one per status tick, started on the first subscription and dropped 600 ticks after the last client looked, sent whole on subscription with the axis (1, 2 or 5 x 10^n kB/s like the Win32 graph, labels formatted by `fmt::speed_always`) and as the appended samples after; Logger as the tail of a 500-line ring of the alert lines `patorrent-nox` prints (the Win32 Logger's rule: everything but the `post_*` replies and the port mappers' chatter), the session-wide ones and the torrent's own, as `[unix time, text]` so the browser shows its local time. `hello` now carries the three tables' column definitions.
- `Alert::PieceInfo` carries `downloading`: the pieces with blocks requested, being written or finished (`PiecePicker::downloading_pieces`), ascending; the simulated engine marks the first four missing pieces of a running download. The cache keeps it in `Reply::Pieces`.
- `Reply::Peers` from `Alert::PeerInfo` in the cache, and `sync/torrentPeers` in the shim: `hash` and `rid`, a full update at rid 0 (or an unknown rid, or another torrent than the client's last call), else only the changed fields per `ip:port` and `peers_removed`; `show_flags` true. Fields: ip, port, client, connection (BT / uTP / Web), flags (the µTorrent letters of the Peers tab), flags_desc, country and country_code (empty), progress, dl_speed, up_speed, downloaded, uploaded, relevance (0), files (empty).
- The page (`app.js` 64 KB, `style.css` 12.5 KB; 80 KB with the HTML, under the 200 KB budget): General as the two bars and two columns of label-value pairs; Trackers, Peers and Files as tables with the frontend's titles and widths, sticky headers, click-to-sort (client-side, by the server's keys), row selection with click, Ctrl and Shift; Files with the priority context menu (High, Normal, Don't Download through the shim's `torrents/filePrio`; Low disabled until the native command of slice 5, since the qBittorrent API has no low priority); Pieces as a canvas bar, each pixel column shaded by the share of its pieces we have, the downloading pieces marked underneath; Speed as a canvas line graph in the Win32 tab's look (download green, upload red, the quarter grid with the server's labels, a 10-minute time axis, newest at the right, the legend with the current rates); Logger as a monospace log that follows the tail unless scrolled up. Every tab updates in place: tables reconcile rows by key (tracker URL, peer address, file index), General sets changed texts only, so the scroll position, the sort and the selection stay. The pane is empty with no row focused (a click on empty space clears it, as 2.2.1 does); the active tab persists in `localStorage`.
- Tests: `native.rs` unit tests on a hand-built cache for the General pairs, the three tables' rows and keys, the pieces run-length encoding, the speed ring (whole, appended, rescaled, dropped when idle) and the logger tail, plus the no-change suppression of each; `tests/webui.rs` `native_detail_tabs_against_the_engine` (the small torrent with a tracker nothing listens on: General, Trackers, Files with a priority change through the shim, Pieces, Speed, Logger, the switch back, the unsubscribe with only ticks after, an unknown id, the `gone` after a delete) and `peers_tab_and_sync_torrent_peers_on_the_mock` (the Peers tab and the shim's full update and diff on the simulated engine, which answers the peer and tracker queries).

### Notes
- The Availability bar on General fills by `distributed_copies` capped at one copy (the share of the torrent available at all); 2.2.1 shades it per piece, which would need the piece reply on a tab that otherwise needs none.
- The Logger line's time is the alert's Unix time; the page formats it in the browser's time zone (nox has no time-zone database, so the server cannot give a local time).
- The engine's `sync/torrentPeers` field subset follows qBittorrent's published WebAPI documentation; the flag letters are µTorrent's (`frontend::detail::peer_flags`), not qBittorrent's.
- The test client no longer drains frames with a short read timeout between steps: on Windows a timed-out `recv` left the socket aborted (10053) for the read after it; the unsubscribe check counts messages instead.
- Verified by hand in the Chromium pane against `patorrent-nox run --no-state --mock-library 200 --active 0.1 --webui 8089`: all seven tabs on a downloading torrent, the Peers sort by Down Speed, the Files priority menu, the General tab keeping its scroll position across ticks, the pane empty with nothing focused.
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast` on Windows: 761 passed, 0 failed, 3 ignored (webui 17).

## 2026-10-17_220857_1792267737: merge of Web UI slice 3

Branch `claude/awesome-clarke-6d8110` merged into main without conflicts (its entry 2026-10-17_220311_1792267391 follows).

### Verified
- On main after the merge: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast` on Windows: 753 passed, 0 failed, 3 ignored.
- The page by hand in the Chromium pane against `patorrent-nox run --no-state --mock-library 10000 --active 0.01 --webui 8089` (release build of the branch): login, the list with the 10,000 rows sorted by Down Speed and updating every second, a deep scroll through the virtual list, the Completed category filter (8,391 rows), the context menu on a row in the Win32 order with the queue moves and Set Download Location disabled as documented, the selection count in the status bar.
- Not rerun on the Ubuntu and FreeBSD VMs: slices 1 to 3 changed `crates/nox` and `crates/frontend` only on Windows so far; the next VM run should cover them.

## 2026-10-17_220311_1792267391: Web UI slice 3: the page

### Added
- The page (`crates/nox/src/webui/page.html`, `style.css`, `app.js`: 3.5, 10 and 46 KB, 59 KB together, embedded with `include_str!`): the µTorrent 2.2.1 main window on the native session protocol (D18, docs/WEBUI.md). The login form as before (the shim's `auth/login` still issues the SID cookie), then the toolbar in the Win32 order (Add Torrent with a file input, Add URL, Remove, Start, Pause, Stop; Move Up / Down Queue and Preferences disabled until slices 5 and 6), the category tree with the counts from `tick` and a Labels group from `hello`, the virtualized torrent list, the detail pane with the seven tab headers (content is slice 4; the focused row and tab are already sent as `detail`), the status bar (DHT nodes, D / U rates and totals, the selection count, the connection state), draggable splitters, and the column layout (order, widths, hidden set, sort) persisted in `localStorage`. Light theme by default, dark by `prefers-color-scheme`. No framework, no build step, plain ES2020.
- The virtual list as docs/WEBUI.md "Page layout" says: a scroll container sized to `total × 22 px`, a pool of row elements for the visible window plus 10 rows of overscan, the `view` re-sent once per animation frame when the visible rows leave the window last sent (not on every scroll event); the header and the rows share one `grid-template-columns` custom property, so a column resize is one CSSOM write. Header click sorts (the column's default direction first, as the Win32 list), a drag past 5 px reorders, the handle at a header's right edge resizes, the header's context menu is the column chooser (with Reset Columns); Done draws an in-cell bar from the row's `done`; rows carry the state class (`st-downloading`, `st-seeding`, `st-paused`, `st-error`, ...) for the text and bar colours. A tick with nothing in the window changed costs a JSON parse and a few string compares; rows are touched only when the order or a cell moved.
- Selection with click, Shift and Ctrl, arrows, PageUp / PageDown, Home / End (a focus moved outside the window is applied when its row arrives), Ctrl+A; Delete removes (confirm), Shift+Delete removes with data, Space pauses, the Menu key and Shift+F10 open the context menu at the focused row. The context menu is the Win32 window's (`app.rs::torrent_menu`), in its order: Open Containing Folder (disabled: a browser has no folders), Force Start, Start, Pause, Stop, the four queue moves (disabled: the shim has none until slice 5), Remove, Remove And > Delete Data, Force Re-Check, Update Tracker, Advanced > Set Download Location... (disabled, slice 5) and Initial Seeding, Bandwidth Allocation > Set Download / Upload Limit with the 2.2.1 presets, Label > New Label..., Remove Label and the labels. Every enabled entry goes to the shim for now (`torrents/start`, `setForceStart`, `pause`, `stop`, `delete`, `recheck`, `reannounce`, `setSuperSeeding`, `setDownloadLimit`, `setUploadLimit`, `createCategory` plus `setCategory` as the label); the check marks (limits, label, super-seeding) come from one `torrents/info` call for the selection when the menu opens, since the cells do not carry them. Menus take the keyboard (arrows, Enter, Escape, Left / Right for submenus).
- `idle` on `visibilitychange` hidden and the `view` again when visible; the WebSocket reconnects with backoff (1 s doubling to 30 s), asks `app/version` first so an ended session shows the login form instead of looping, and resubscribes the view, the detail and the column list on `hello`.
- Static files (`api.rs`): `/`, `/index.html`, `/style.css` and `/app.js` with an `ETag` (a SHA-1 of the body, computed once in a `LazyLock`), `Cache-Control: no-cache`, `304 Not Modified` on `If-None-Match`, `405` on POST. The page's CSP is `default-src 'self'; frame-ancestors 'self'; form-action 'self'` with no `'unsafe-inline'` any more: the HTML has no inline script or style; progress bars, column widths and splitter positions are CSSOM writes, which a CSP allows. `with_security_headers` sets `Referrer-Policy: no-referrer` now (the design's choice; the CSRF check uses `Origin`, which browsers send on every POST and WebSocket upgrade) and adds a header only when the response did not set it itself.
- `patorrent-nox run --mock-library N` (undocumented): the simulated engine with N synthetic torrents instead of the real one, `--active FRACTION` of them force-started and moving data, the auto-managed limits at zero as in the nox tests, so the page can be measured against the 10,000-torrent profile: `run --no-state --mock-library 10000 --active 0 --webui 8089 --webui-pass ...`.
- `#gate` in the page URL shows a panel (mirrored into the window title, so a script can read it off a browser it cannot see into) with what the page itself measures: login (or load) to the first painted list with its stages (login, socket open, hello, rows, paint), the first `rows` size, tick sizes, and messages received while hidden.
- Test `static_files_have_their_headers_and_no_inline_code` in `crates/nox/tests/webui.rs`: the three files, their content types, ETag, 304, HEAD, 405, the security headers, the CSP without `unsafe`, the 200 KB budget, and the HTML free of `<style>`, `style=`, `on*=` handlers, `javascript:` and inline `<script>` content.

### Removed
- The polling page: `page.html`'s inline stylesheet and `app.js`'s `sync/maindata` loop. The shim (`/api/v2/`) and its tests are untouched.

### The 10,000-torrent gate (docs/WEBUI.md "Budgets")
Release `patorrent-nox run --no-state --mock-library 10000` on this machine, read off the page's `#gate` panel. Browsers: Google Chrome 155.0.8059.80 and Firefox 158.0 started from a shell with fresh profiles (`gate.ps1`, kept outside the repo), and the Chromium 152 pane of the Claude desktop app for the typed login.

| What | Target | Measured |
|---|---|---|
| Login to first painted list | under 0.5 s | 48 ms in Chromium 152 (login reply at 16 ms, socket open 18, `rows` 41, painted 48; 45 rows). Page load to first painted list with the loopback auth bypass: Chrome 46 ms (100-row window), Firefox 109 ms (54 rows). The login `POST` alone is 14 ms by curl (PBKDF2, 100,000 iterations) |
| First `rows`, 60-row window, all 24 columns | under 30 KB | 22 KB by the server test (unchanged). The page's default view: 10.8 KB for 45 rows × 14 columns, 24.2 KB for 100 rows × 14 columns |
| `tick`, idle swarm | under 1 KB | 204 B, every tick, all three browsers |
| `tick`, 100 active torrents, 60 in view | under 20 KB | 16.0–16.6 KB with 86 active rows in view, 15 columns, every row changing every tick (`--active 0.01`, sorted by Down Speed); about 11.5 KB at 60 rows |
| Page idle CPU, window visible, idle swarm | not measurable above the browser's own idle | whole-browser CPU over 30 s on a profile's second run: Chrome 0.83 % of a core on the page against 1.04 % on `about:blank`; Firefox 1.04 % against 1.15 %. A profile's first run is noisy (Chrome 4.1 %, Firefox 19 % with first-run work, and the `#gate` title rewrite every second), so the numbers are without `#gate` |
| Hidden tab | no messages | 0 messages over 20 s with the window minimized, Chrome and Firefox; 0 over 8 s with another tab in front, Chromium |
| `patorrent-nox` size growth for the whole Web UI | under 300 KB | release exe 2,682,368 B (a dev `cargo build --release`, not `build.bat`'s dist build) against 2,517,504 B for main before slices 1 to 3: 165 KB so far |

How: the browser started on the page with a fresh profile, `TotalProcessorTime` summed over all its processes before and after 30 s, hidden by minimizing the window (`document.hidden`) for 20 s, the title read with `Get-Process`. Typing the login into a real browser could not be scripted (Windows' foreground lock refuses `SendKeys` from a background shell, and the script refuses to type anywhere else), so the real-browser runs used `bypass_local_auth` set through `app/setPreferences`, and the Chromium pane did the login path.

### Notes
- Ctrl+A under a filter or search selects what the window holds (up to 200 rows); only in an unfiltered view do commands go out as `hashes=all`. The native commands of slice 5 take the filter server-side.
- The shim's `torrents/pause` and `stop` are the same engine call, so Pause from the page shows as Stopped until slice 5 moves the commands onto `frontend::actions` with the user-paused record.
- `requestAnimationFrame` does not fire in a background tab, so the gate's paint stage is only meaningful with the tab in front (the first attempts in the pane read 466 and 1040 ms for that reason, with the rows at 41 ms).
- The `hidden` seconds the gate counts come from `visibilitychange`, which the Chromium pane raised late once (8 s shown for a 17 s switch); the message count is what matters and was 0.
- Verified: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast` on Windows: 753 passed, 0 failed, 3 ignored (webui 15).

## 2026-10-17_212558_1792265158: merge of Web UI slices 1 and 2

Branch `claude/vigilant-fermi-649e6d` merged into main without conflicts (its entry 2026-10-17_210452_1792263892 follows the design entry below).

### Verified
- On main after the merge: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast` on Windows: 752 passed, 0 failed, 3 ignored (webui 14).
- Not rerun on the Ubuntu and FreeBSD VMs yet: `crates/nox` and `crates/frontend` changed (`webui/ws.rs`, `webui/native.rs`, `status::status_class`), so the next VM run should cover them.

## 2026-10-17_210452_1792263892: Web UI slices 1 and 2: WebSocket and the native session protocol

### Added
- `webui/ws.rs` (sans-IO, RFC 6455): the opening handshake (`Sec-WebSocket-Accept` from `core::crypto::Sha1` plus a base64 encoder of our own, RFC 4648 vectors tested), frame parsing with the client masking the RFC makes mandatory (an unmasked client frame is a 1002 close), 7-, 16- and 64-bit lengths, control frames (no fragmentation, at most 125 bytes), message assembly from text fragments with control frames in between, binary refused with 1003, bad UTF-8 with 1007, a frame or message over `http::Limits::max_message` (new, 64 KiB) with 1009, close frames with their status code, and the server's unmasked frame writer. `webui/http.rs`: `Limits::max_message`.
- `GET /pa/ws` in `webui/server.rs`: the connection thread checks the ban list, the `Host` header, `Origin` with the CSRF rule of `api::csrf_ok` (a bad or `null` `Origin` is 403, no `Origin` passes as for requests, so scripts can connect), a valid session exactly like a request (`authenticate`: SID cookie, or the loopback bypass; none is 403), then the upgrade headers (400 without `Upgrade` / `Connection` / a 24-character key, 426 with `Sec-WebSocket-Version: 13` for another version), writes the 101 and becomes the socket's thread. The loop reads with a 50 ms timeout and after every read or timeout compares `State::tick` (bumped once per `on_alerts` batch that carried a `StateUpdate` or `SessionStats`) with the tick it last sent; so one thread per socket, no second reader thread and no Condvar wait: the read timeout already bounds tick latency to 50 ms on a 1 s status interval, and the tests needed nothing more (docs/WEBUI.md open point resolved). Pings are answered with pongs, a client close is echoed with its code then the socket is shut down with a lingering read, a ping goes out after 30 s of silence and the socket is closed after 60 s, a session logged out or expired closes with 1008. The connection counts against `MAX_CONNECTIONS` while open.
- `webui/native.rs`, the session protocol of docs/WEBUI.md "Messages", per client (`native::Client`): `hello` (version, the 24 columns with id, title, default width, alignment, default visibility and sort direction from `frontend::columns::Column`, the five categories with their 2.2.1 titles, the labels, which are the shim's category names), `view` (filter by category or label plus a case-insensitive name search, sort `[column, descending]`, the columns, the window `[first, count]` capped at 200) answered with `rows` (total, first, the window's ids, and per id its `Entry::version`, the cell strings in the client's column order, `done` 0..1000 and the `state` class), `detail` (stored, no content until slice 4), `idle`, `error` for bad JSON or unknown messages. Once per status tick, `tick` with the total, the window's ids and first only when the order moved, the cells of rows whose version changed since the client was last sent them (new to the window counts), the per-category counts and the status bar (`dht`, `down`, `up`, `down_total`, `up_total` formatted with `frontend::fmt`); an idle client or one without a view gets nothing. Each client keeps its own `frontend::model::TorrentList` fed from the shared cache by `Entry::version` watermark (`State::next_version`) and a removal counter (`State::removals`), its labels from the shim's category map, never from the engine. Column ids are snake_case of the `Column` names (`name`, `queue_position`, ..., `wasted`); ids are the info-hash hex `State::by_hash` uses.
- `frontend::status::status_class`: the short class (`error`, `moving`, `checking`, `queued`, `paused`, `finished`, `stopped`, `metadata`, `seeding`, `downloading`) with the same branches as `status_text`, for the page's CSS.

### Tests
- `ws::tests`: RFC 4648 base64 vectors, the RFC 6455 section 1.3 accept key, the section 5.7 masked "Hello" frame and its unmasked twin, lengths, reserved bits, unknown opcodes, fragmented and oversized control frames, the 64-bit length sign bit, message assembly and every close code, the writer.
- `native::tests`: column ids round-trip and are unique, `view` parsing and its refusals, errors / idle / detail without a cache, and the diff rules on a hand-built cache: fastest first, cells exactly as `Column::text` gives them, a tick with nothing changed carries neither ids nor cells, a row that changes and leaves the window re-sends the order and the cells of the row new to the window only, a change to one row in a stable order sends that row once, a label set through the category map shows in the cell, a search narrows the total, a removal leaves the list.
- `tests/webui.rs` with a raw `TcpStream` WebSocket client: the handshake accept value, `hello`'s columns and categories, a masked text round trip (`view` to `rows`), a fragmented message, bad JSON, ping answered with pong (payload echoed), a 16-bit-length frame, the close handshake (1000 echoed, then EOF), unmasked (1002), binary (1003) and oversized (1009) closes; a bad or `null` `Origin` 403, no or a forged cookie 403, version 8 is 426, a plain `GET /pa/ws` is 400, no `Origin` is accepted, logout closes the socket with 1008. Against the real engine: a two-torrent library gives `rows` with the right total, order and cell strings (`48.8 kB`, `0.0 %`, blank idle speed, `Stopped`), ticks with counts and the status bar, a stop yields a tick carrying that row and never the unchanged one, a column change re-sends the window, category / search / label filters and an unknown category error, idle silences the ticks and a `view` resumes them, a delete drops the total. Against the mock: sorted by speed with 40 force-started torrents the ids are re-sent only when the order moved and rows new to the window come with cells, sorted by name they never are; an idle client, and one without a view, gets no frames across several ticks.
- The 10,000-torrent profile (`Session::start_mock`, 10,000 synthetic torrents, nothing running): the 60-row, 24-column `rows` message is 22,145 bytes (budget 30 KB) and an idle `tick` is 202 bytes (budget 1 KB); the window clamps at the end of the list.

### Changed
- `webui/mod.rs`: `State::tick` and `State::removals`; `on_alerts` counts a tick per batch with a state update or session stats. The existing page, `app.js` and the `/api/v2/` shim are untouched and their tests pass unchanged.
- docs/WEBUI.md: the one-thread-or-two open point is struck through with the decision above; PLAN.md Phase 6 slices 1 and 2 ticked, next step 4 points at slice 3.

### Verified
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` on Windows (rustc 1.90.0).

### Issues
- The first `rows` after connecting to a 10,000-torrent library may report a smaller total while the first `StateUpdate` is still being applied; the next `tick` brings the rest (the test re-asks). The page (slice 3) should treat `total` as live.
- Two test waits were wrong on the first full-workspace run (the machine was busy): the real-engine test read the Status cell while the torrent was still checking (now it waits for `downloading` / `stalledDL` through the shim first), and the mock tests asserted `rows` on a frame that was a `tick` between two re-asks (now they skip ticks while waiting). Both are test fixes; the server was right.

## 2026-10-17_203617_1792262177: Web UI design decided (D18, docs/WEBUI.md)

### Decided
- The Web UI is our own, not qBittorrent's: the µTorrent 2.2.1 main window in a browser (toolbar, category tree, the 24-column list, the seven detail tabs, status bar, context menu, Add dialog, Preferences pages), with push updates, a 10,000-torrent gate, keyboard operation and a phone layout. Design in `docs/WEBUI.md`, decision D18 in PLAN.md.
- Two APIs over one cache: a native `/pa/...` API designed from what the page needs (one WebSocket down for state, JSON `POST`s up for commands), and the existing qBittorrent WebAPI subset (D10) kept as a compatibility shim for Sonarr, Radarr, Prowlarr and the mobile remotes. The shim translates from the same cache and calls the same command functions; it never shapes a native feature.
- The page is a thin terminal of `patorrent-frontend`: the server runs the filtered, sorted `frontend::model::TorrentList` per client and sends formatted cells (`frontend::columns`, `fmt`, `status`, `detail`) for the visible window only. Parity with the Win32 window comes from sharing that code; the page never holds the whole list, so traffic and memory are bounded by the window, which is what makes a phone work too.
- No framework, no build step, no dependencies: one HTML, one CSS, one JS file embedded in the binary, under 200 KB; the server side stays `std` only. Dark mode by `prefers-color-scheme` on the web page (D13's strict look binds the desktop only).
- The GUIs' remote mode (Phase 6 optional item) will be a separate binary stream of `TorrentStatus` records from the same cache, not a variation of the page protocol.
- Not chased: µTorrent's `/gui/?list=1` token API (no live clients), qBittorrent's Web UI look, multi-user.

### Changed
- PLAN.md: D18 added, D10 reworded as the shim, the binary table and the Phase 6 list carry the eight slices (WebSocket, session protocol, page, detail tabs, commands and Add, Preferences, phone, Phase 5 features), next step 4 points at slice 1.
- docs/ARCHITECTURE.md: the nox line points at WEBUI.md.

### Issues
- None; documentation only, no code changed.

## 2026-10-17_200844_1792260524: rename_file, renamed files at add time, Web API renameFile / renameFolder

### Added
- `TorrentHandle::rename_file(file, new_name)` (libtorrent `torrent_handle::rename_file`): the name is relative to the save path, torrent folder included, or absolute. A disk barrier (`BarrierAction::Rename`, libtorrent's fence job) runs `posix_storage::rename_file`: if the file is on disk it is renamed (directories made, open handles released first for Windows); a file not downloaded yet only changes its name. The storage keeps the renamed layout beside the shared `TorrentInfo` (`Storage::mapped`, libtorrent `m_mapped_files`), so later reads, writes, checks, moves and deletes use the new paths; `move_storage` moves renamed relative files with the rest and leaves absolute ones.
- A stopped torrent gets a storage just for the rename, shared with moves (`Torrent::moving`) and released when no move or rename is pending (`release_disk_op_storage`); `start_torrent` waits for pending renames as it does for moves, and shutdown waits for them too.
- `Alert::FileRenamed { id, index, old_name, new_name }` and `Alert::FileRenameFailed { id, index, error }` (libtorrent `file_renamed_alert` / `file_rename_failed_alert`); errors for no metadata yet, an empty name, an index past the last file, and disk failures (also logged with the path).
- `AddTorrentParams::renamed_files` (libtorrent `renamed_files`): applied whenever the metadata is loaded (`Torrent::apply_renames`), so it works for `.torrent` files, resume data and magnets; indices past the last file are ignored.
- Resume data `mapped_files` in libtorrent's layout (one string per file up to the last renamed one, empty for the others), read back into `renamed_files`; renaming a file back to its metadata name drops the entry.
- nox Web API `torrents/renameFile` and `torrents/renameFolder` (POST, `hash`, `oldPath`, `newPath` as `torrents/files` names them): 400 without the paths, 409 for an absolute or invalid new name (empty, `.`, `..`, `:` or Windows-forbidden characters), no such file or folder, no metadata, or a name another file of the torrent keeps. nox prints `FileRenamed` / `FileRenameFailed`.
- Win32 Add New Torrent: the Name field now renames the torrent's folder, or a single-file torrent's file, on disk (`patorrent_frontend::add::renamed_root`, used by `AddChoice::apply_to`, which now takes the `TorrentInfo`); a name with a separator, `.` or `..` renames only the list entry as before. The Logger tab shows `FileRenamed` / `FileRenameFailed` and the Files tab refreshes.
- `mock` engine answers `rename_file` with `FileRenamed`.

### Tests
- `disk::tests::rename_file_on_disk_then_read_and_move`: relative, absolute and out-of-folder renames, a read through the new name, a full check, a move taking renamed files along, a missing file renamed by name only, a directory that cannot be made, an index past the end.
- `tests/rename_file.rs` (after libtorrent `test_torrent.cpp` `test_rename_file` / `test_rename_file_fastresume`): a seed renames while seeding and while stopped, the names survive a restart, resume verification finds the files under them and a leech downloads the torrent's own layout from them, then a rename back; a download into `renamed_files` set at add time (the folder renamed, one file moved, an index past the end ignored); a magnet without metadata refuses.
- `resume::tests::round_trip` covers `mapped_files`; `add::tests` cover `renamed_root` and the choice; the nox `torrent_lifecycle_and_maindata_diffs` test covers both endpoints and their refusals.

### Verified
- `cargo fmt --all --check` (1.90 and stable), `cargo clippy --workspace --all-targets -- -D warnings` on 1.90.0 and stable, `cargo clippy -p patorrent-core -p patorrent-frontend -p patorrent-nox --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`.
- `cargo test --workspace --no-fail-fast` on Windows: all green (core 555 with 3 ignored, rename_file 3, move_storage 8, swarm 34, webui 8, frontend 47, win32 21, the rest).

- `cargo test --workspace --no-fail-fast` on the Ubuntu and FreeBSD VMs (commit 895f7d9, copied after the owner approved it): all green on both (core 556 with 3 ignored, rename_file 3, move_storage 8, swarm 34, webui 8, frontend 47, the rest).

### Issues
- The first attempt to copy the tree to the VMs was refused by the auto-mode classifier (it removes `~/patorrent` there first); it ran once the owner approved it.
- Not verified by hand: the Add dialog's Name on a real torrent.

## 2026-10-17_194329_1792259009: merge of the Win32 move storage wiring

Branch `claude/gallant-archimedes-20a750` merged into main without conflicts (its entry follows).

### Verified
- `cargo fmt --all --check`, `cargo +stable fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings` on 1.90.0 and stable; `cargo clippy -p patorrent-frontend --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`.
- `cargo test --workspace --no-fail-fast` on Windows: 729 passed, 0 failed. Only `crates/win32` and `crates/frontend` changed, so the VMs were not rerun.
- Not verified: the Preferences option and Set Download Location by hand.

## 2026-10-17_193944_1792258784: Win32: Move completed downloads and Set Download Location

### Added
- Preferences > Directories: "Move completed downloads to:" with its folder field and Browse button, and "Only move from the default download directory" (on by default, as in µTorrent), no longer disabled placeholders. The folder field, Browse and the second check box grey out while the first is off; switching it on with no folder is refused on the Directories page. Stored in patorrent.ini as `dir.move_completed`, `dir.completed`, `dir.move_only_default` (`patorrent_frontend::options::GuiOptions`, so GTK gets the same model).
- `GuiOptions::move_completed()` builds the `patorrent_core::MoveCompleted` for the options (`only_from` = the default download folder when the second box is on); `update_move_completed` re-points a running one on Apply and keeps the save paths it has learned.
- The Win32 alert loop feeds every alert to that `MoveCompleted`: a finished download is moved, logged ("moving the completed download to ..."), and shows "Moving" until `StorageMoved`.
- Torrent context menu > Advanced > "Set Download Location...": a folder picker opened on the first selected torrent's save path, then `move_storage(dir, AlwaysReplaceFiles)` for every selected torrent not already there (µTorrent 2.2.1 moves the files). Greyed while a selected torrent is moving.
- `StorageMoved` / `StorageMovedFailed` get their own Logger lines with the torrent's name and refresh the list (new save path, end of "Moving"); a failure also shows an error box with the error and the file.
- The folder pickers (`create::pick_folder_at`) can open on a start folder; Preferences' two Browse buttons open on the folder in their field.
- Tests: `options::tests::move_completed_follows_the_options` (the mover from the options, re-pointing, the ini keys), the round-trip test covers the three new options; hidden-dialog test `preferences::tests::move_completed_downloads_on_directories` (controls on the Directories page, greying, the empty-folder refusal, Apply, persistence, shown back).

### Decisions
- One folder picker for single- and multi-file torrents; µTorrent's Save As box for single-file torrents (rename while moving) needs `rename_file`, which is not ported.
- Set Download Location is offered for running torrents as well, since the engine moves them; µTorrent's own greying rules were not checked against the real client.

### Verified
- `cargo fmt --all --check` (1.90 and stable), `cargo clippy --workspace --all-targets -- -D warnings` on 1.90.0 and stable, `cargo test --workspace --no-fail-fast` on Windows: all green (win32 21, frontend 46, core 554 with 3 ignored, move_storage 8, swarm 34, the rest).
- Not verified: the dialog, the menu item, a real move and the error box by hand.

## 2026-10-17_165701_1792249021: merge of move storage, the Add New Torrent dialog and the nox Web UI server

Three features built in parallel on their own branches and merged into main: `claude/move-storage-2c300f`, `claude/add-dialog-d82bc0`, `claude/nox-webapi-73059f` (their entries follow).

### Fixed
- `dht::node::tests::scrape_v6` failed once on FreeBSD after the merge (`downloaders 46.00749`, tolerance +-3). Not caused by the merge: the test announces 100 random addresses, and the Bloom filter estimate misses the tolerance about once in 300 runs (1 failure in 300 local runs of both scrape tests); libtorrent's `test_scrape` draws random addresses too. The announcing addresses are now a fixed scattered sequence (`scattered_addr`), so the estimate is the same every run.

### Issues
- Merge conflicts in PLAN.md and CHANGELOG.md only (each branch edited the same Phase 5 / Next steps lines; combined). `crates/nox/src/main.rs` (move storage's `--move-on-complete` and the Web UI options) merged cleanly.
- The Add New Torrent branch's PLAN note "Next candidates: move storage" was rewritten to "wiring move storage into the GUI", since move storage is in.

### Verified
- `cargo fmt --all --check`, `cargo +stable fmt --all --check`, `cargo +stable fmt --manifest-path fuzz/Cargo.toml --check`; `cargo clippy --workspace --all-targets -- -D warnings` on 1.90.0 and stable.
- `cargo test --workspace --no-fail-fast` on the merged main: Windows 554 core (3 ignored), win32 20, frontend 45, move_storage 8, swarm 34, web seed 12, proxy 9, nox suites and all others green; Ubuntu 26.04.1 all green (555 core); FreeBSD 15.1 all green except the `scrape_v6` flake above, and the DHT tests pass there with the fix.
- Not verified: the new GUI dialog and the Web UI page by hand, real WebAPI clients (Sonarr, Radarr), a move across Windows drive letters.

## 2026-10-17_164324_1792248204: Phase 6 starts: the nox Web UI server and a qBittorrent WebAPI v2 subset

Branch `claude/nox-webapi-73059f`. Written from qBittorrent's published WebAPI documentation ("WebUI API (qBittorrent 5.0)" on its wiki) only; no qBittorrent source or Web UI asset was read or copied (PLAN.md Known TODOs, licensing note; D10).

### Added
- `patorrent-nox run --webui [ADDR:]PORT` (ADDR defaults to 127.0.0.1: `--webui 8080`, `--webui 0.0.0.0:8080`, `--webui [::1]:8080`), `--webui-user NAME` (default `admin`), `--webui-pass PASS`; `add` takes them too. The crate is now a library plus the binary (`src/lib.rs`, `patorrent_nox::webui`), so the integration tests and the fuzz target reach the server.
- `crates/nox/src/webui/`:
  - `http.rs`: sans-IO HTTP/1.1 request heads (RFC 9112: origin- and absolute-form targets, leading blank lines skipped, bare-LF line ends tolerated; obsolete line folding, bad header names, control characters, a second `Host`, conflicting or malformed `Content-Length` refused with 400; `Transfer-Encoding` answered 501; HTTP/1.0 keep-alive; `Expect: 100-continue`), limits (16 KiB head, 100 headers, 100 MiB body), `application/x-www-form-urlencoded`, `multipart/form-data` (RFC 7578, quoted parameters, at most 1000 parts), cookies, and the response writer (CR / LF stripped from header values).
  - `json.rs`: a JSON value with a writer (escapes `<`, U+2028 / U+2029) and a parser (RFC 8259, depth limit 64, surrogate pairs, numbers out of double range refused).
  - `auth.rs`: HMAC-SHA256 and PBKDF2-HMAC-SHA256 on the engine's SHA-256; salted password hashes `pbkdf2-sha256:<iterations>:<salt>:<hash>` (100,000 iterations, a 16-byte salt from the OS RNG); constant-time compare; the failed-login ban list.
  - `server.rs`: std threads: one acceptor, one thread per connection (at most 64; more get 503), keep-alive with pipelining, 30 s idle / 20 s head / 120 s body timeouts, a 64 KiB body limit until the client has logged in, a lingering close after an error response so a reset does not swallow it.
  - `api.rs`: the request checks and the endpoints; `torrent.rs`: engine status as WebAPI torrent objects and state strings; `fetch.rs`: `http://` downloads for `torrents/add` (redirects, including to `magnet:`); `page.html` + `app.js`: a small original Web UI (login, the torrent list kept current with `sync/maindata`, add by link, start / stop / remove).
- Security: an `SID` cookie (`HttpOnly; SameSite=Strict`, 128 random bits, idle timeout `web_ui_session_timeout` 3600 s, at most 256 sessions); 5 failed logins ban the address for 3600 s (`web_ui_max_auth_fail_count`, `web_ui_ban_duration`: 403 on everything while banned, the right password included); CSRF protection: an `Origin`, or without one a `Referer`, must name the request's `Host` (401 otherwise; requests with neither pass, as scripts send none); Host header validation: IP literals, `localhost` and names in `web_ui_domain_list` (`*` / `?` wildcards) only, 400 otherwise (stops DNS rebinding); `bypass_local_auth` off by default; `X-Frame-Options: SAMEORIGIN`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: same-origin`, `Cache-Control: no-store`, and a CSP on the page (scripts from 'self' only). Endpoints that change something take POST only (405).
- Endpoints: `auth/login`, `auth/logout`; `app/version`, `webapiVersion`, `buildInfo`, `preferences`, `setPreferences`, `defaultSavePath`, `shutdown`; `transfer/info`, `speedLimitsMode`, `toggleSpeedLimitsMode`, `setSpeedLimitsMode`, `downloadLimit`, `uploadLimit`, `setDownloadLimit`, `setUploadLimit`; `torrents/info` (filter, category, sort, reverse, limit, offset, hashes), `count`, `properties`, `trackers` (with the DHT / PeX / LSD rows), `files` (indexes), `pieceStates`, `add` (urls: magnets, bare info-hashes, `http://`; multipart torrents; savepath, category, paused / stopped, skip_checking, rename, upLimit / dlLimit, sequentialDownload), `stop` / `pause`, `start` / `resume`, `setForceStart`, `setSuperSeeding`, `delete` (deleteFiles), `recheck`, `reannounce`, `filePrio`, `setDownloadLimit`, `setUploadLimit`, `downloadLimit`, `uploadLimit`, `categories`, `createCategory`, `editCategory`, `removeCategories`, `setCategory`, `tags` (always empty); `sync/maindata` with rid-based diffs (`full_update`, `torrents` with only the changed fields, `torrents_removed`, `categories`, `categories_removed`, `tags`, `server_state`).
- `<state dir>/webui.json`: the user name, the password hash, preferences set through `setPreferences` (applied again at the next start), categories and torrent categories; written to a temporary file and renamed. With no password given or stored, a random temporary password is printed at start (`--no-state` runs too).
- Tests: 22 unit tests (HTTP heads, forms, multipart, cookies, responses, JSON, HMAC (RFC 4231 cases 2 and 6), PBKDF2 (RFC 7914 vector), password hashes, the ban list, the Host / CSRF / wildcard checks, state strings, filters, torrent objects). `crates/nox/tests/webui.rs`, 8 tests on a real session and server over loopback: login / logout / 403 / 405 / versions / the page; the ban after 5 failures; CSRF (`Origin`, `Referer`, `null`, a cross-site login) and Host rejection, then acceptance through `web_ui_domain_list`; a torrent's whole life (multipart add with a category, `torrents/info` parameters, maindata full update then diff then a stale rid, properties, files, trackers, pieceStates, filePrio with its 400 / 409 answers, stop / start with the state in the diff, category changes and `categories_removed`, limits, recheck, reannounce, delete and `torrents_removed`); magnets and bad torrents (415, `Fails.`); connection handling (pipelining, keep-alive, `Expect: 100-continue`, 413 before login, 501 for chunked, 431 for a huge head, 400 for garbage); preferences and the stored hash across a restart; the temporary password.
- Fuzz target `nox_http`: request heads, then the body as urlencoded, multipart (boundary from its own Content-Type), cookies and JSON, with a JSON write / read round trip asserted. In the `fuzz.yml` matrix; `fuzz.yml` now also runs on `crates/nox/**` changes; `fuzz/Cargo.toml` depends on `patorrent-nox`.

### Decisions
- `app/version` answers `v5.0.0` and `app/webapiVersion` `2.11.2`, the qBittorrent release whose API this follows: Sonarr, Radarr and the mobile apps choose features and endpoint names by them. `app/buildInfo` names PaTorrent (`app`, `app_version`) beside the documented fields (`libtorrent` is `2.0.11.0`, the version we port).
- State strings follow the documentation's list: stopped torrents are `pausedDL` / `pausedUP` (qBittorrent 5.x itself says `stoppedDL` / `stoppedUP`; clients accept both). `stop` / `start` and the older `pause` / `resume` both exist; the `stopped` and `paused` filters are the same.
- Torrents added through the API are auto-managed (queued under the engine's active limits, like qBittorrent with queueing on); `stop` clears auto-management and pauses, `start` sets it and resumes, `setForceStart` clears it. Torrents added with `patorrent-nox add` stay outside the queue and show as `forcedDL` / `forcedUP`.
- `hash` is the v1 info-hash, or the truncated v2 hash of a v2-only torrent (`best_v1_compatible`); `infohash_v1` / `infohash_v2` carry both.
- Values the engine status does not have: `auto_tmm` and `f_l_piece_prio` are false; `max_ratio` and `max_seeding_time` -1, `ratio_limit` and `seeding_time_limit` -2 ("global"); `num_complete` / `num_incomplete` -1 until scraped; a tracker's `num_peers` -1; `isPrivate`, `comment`, `created_by` and `creation_date` are known only for torrents added through the API in this run; a file's `availability` is the mean availability of its pieces; `pieceStates` has no "downloading" (1) yet. The "infinite" ETA is 8,640,000 s; ratios are capped at 9999.
- Preference units follow the documentation: `dl_limit`, `up_limit`, `alt_dl_limit`, `alt_up_limit` in KiB/s with -1 for unlimited (0 or less sets unlimited); `transfer/*Limit` in bytes/s. `transfer/setDownloadLimit` sets the alternative limit while alternative mode is on.
- The `setPreferences` keys we support are applied at once and stored; at the next start, stored preferences override the command line's rate limits and port. `web_ui_password` is stored only as its hash; `proxy_password` is not stored; the user name is stored on its own. The Web UI's own address and port come from the command line only.
- Host header validation accepts only IP addresses and `localhost` by default (qBittorrent's default domain list is `*`): a host name must be added to `web_ui_domain_list`.
- `torrents/add` with an unknown category creates it (without a save path). The `savepath` parameter or the default save path is used; a category's save path is only recorded (automatic torrent management is not implemented).
- The server runs on std threads, not the engine's mio reactor: requests never touch engine state, and a thread per connection keeps the blocking URL download in `torrents/add` simple. Requests that need the engine (`files`, `trackers`, `pieceStates`, `filePrio`) post a `post_*` call and wait (5 s at most) for the alert that nox's main loop hands to `WebUi::on_alerts`.

### Fixed (found by the new fuzz target before commit)
- `json::parse` turned `1e999` into an infinite float, which writes back as `null`; out-of-range numbers are refused now.
- The JSON writer printed large integral floats (`7.766666666766667e18`) without a fraction, so they read back as integers; floats always carry a `.` or an exponent now.

### Issues
- `crates/core` is unchanged: no engine merge hot spots. `crates/nox/src/main.rs` changed in a few small places (three options and their usage text, `start_webui` before the loop, `on_alerts` after `pop_alerts`, `app/shutdown` ending the loop, the Web UI dropped before shutdown). The move-storage branch adds a `move` option to the same file; if both touch `parse` / `Opts`, merge by hand.
- HTTPS links in `torrents/add` are refused (the engine's TLS is internal to `patorrent-core`); the Web UI itself has no HTTPS yet.

### Verified
- `cargo fmt --all --check`, `cargo +stable fmt --all --check`, `cargo +stable fmt --manifest-path fuzz/Cargo.toml --check`; `cargo clippy --workspace --all-targets -- -D warnings` with 1.90.0 and stable on Windows, and `-p patorrent-nox` for `x86_64-unknown-linux-gnu` and `x86_64-unknown-freebsd`.
- `cargo test --workspace --no-fail-fast`: Windows 545 core (3 ignored), Ubuntu 26.04.1 and FreeBSD 15.1 546 core (3 ignored); on all three nox 22 unit + 1 main + 8 Web UI integration tests, swarm 34, frontend 37, web seed 12, proxy 9 and every other suite green. After the two JSON fixes, `cargo test -p patorrent-nox` again on all three.
- `nox_http` fuzzing on Windows: 3,224,671 runs in 181 s after the fixes, no findings.
- Manual check with curl 8.17.0 against `patorrent-nox run --state <dir> --listen 127.0.0.1:0 --webui 18080 --webui-pass ...` on Windows: unauthenticated `app/version` 403; `auth/login` 200 `Ok.` with `Set-Cookie: SID=<32 hex>; HttpOnly; SameSite=Strict; path=/`; `app/version` `v5.0.0`, `webapiVersion` `2.11.2`; `sync/maindata?rid=0` `{"rid":1,"full_update":true,"torrents":{},"categories":{},"tags":[],"server_state":{...}}`; `torrents/add` with a magnet and `stopped=true` `Ok.`; `rid=1` gave `{"rid":2,"torrents":{"0123…4567":{...full object, "state":"pausedDL"...}}}`; `rid=2` gave `{"rid":3}` (nothing changed); `Origin: http://evil.example` 401; `Host: rebind.example:18080` 400; `torrents/delete` then `rid=3` gave `{"rid":4,"torrents_removed":["0123…4567"]}`; `webui.json` held `pbkdf2-sha256:100000:<salt>:<hash>` and not the password.
- Not verified: Sonarr / Radarr / qBittorrent mobile apps against it; the embedded page in a browser by hand; many simultaneous clients; IPv6 listening.

## 2026-10-17_163655_1792247815: Win32 Add New Torrent dialog (Phase 5)

Branch `claude/add-dialog-d82bc0`.

### Added
- `crates/win32/src/add.rs`: µTorrent 2.2.1's Add New Torrent dialog, built at runtime from a dialog template like Create and Preferences (dialog units, so it scales with the monitor's DPI). Save In (editable combo with the default download folder and the last 10 folders used, "..." browses), Name, Torrent Options (Skip hash check, Start torrent, Add to top of queue, Label: an editable combo with the labels in use), Torrent Contents (name, comment, "X of Y selected" with the volume's free space, creation date), the file tree, Select All / Select None, OK / Cancel. OK creates the save folder if it is missing.
- The file tree is an owner-data list view over `FileTree`: only visible rows are materialized, so a 5,000-file torrent opens at once (tested). Folders are indented rows with the shell's folder icons (open / closed) and file icons by extension; double-click, Enter, Right / Left (or + / -) and a click on the icon open and close them, Left on a file jumps to its folder. Check boxes are themed state images drawn at the list's DPI with a third, mixed state for folders whose files differ; a click on the box or Space toggles the rows (a folder toggles every file below it, open or closed). Right-click: High / Normal / Low / Don't Download and Check / Uncheck for the selection; Priority shows "Mixed" for folders with different priorities.
- `patorrent_frontend::add` (toolkit-independent, for GTK too): `FileTree` (tree from `TorrentInfo`, pad files left out and never downloaded, check / priority / expand state, folder summaries, `file_priorities()` with 0 for unchecked files, so part files keep them off disk), `AddChoice::apply_to` (save path, name, priorities, Start torrent = queued, off = µTorrent's Stop, Skip hash check = `SEED_MODE`), `recent_dirs` / `remember_dir` (`add.recent_dirs` in patorrent.ini), `save_in_choices`, `size_text`.
- Preferences > UI Settings > When Adding Torrents: "Show a window that displays the files inside the torrent" (`GuiOptions::show_add_dialog`, `ui.show_add_dialog`, on by default). Off adds `.torrent` files directly as before.
- The dialog opens for `.torrent` files from File > Add Torrent, drag and drop, the command line, arguments forwarded by a second instance, and Add Torrent from URL (http/https downloads). "Start torrent" starts checked unless "Don't start the download automatically" is on; the Preferences' per-torrent defaults (peer exchange, connection and upload slot limits) still apply.
- Add to top of queue: the info-hash is remembered and the torrent is moved to the top when `TorrentAdded` arrives. Label: stored as the torrent's label line before the add, so the list shows it on first sight.
- Tests: frontend `add::tests` (tree order, folder toggling and mixed state, priorities kept across uncheck / check, collapsing, 20,000 files, single- and multi-file torrents, `AddChoice` flags, recent folders); hidden-dialog tests in `win32::add::tests` (controls and contents, Space on files and folders through the list view's own `LVN_KEYDOWN`, Select All / None, Left / Right, OK's `AddChoice` with the created folder, 5,000 files) and `preferences::tests::the_add_dialog_switch_is_on_ui_settings`.
- Screenshot: [docs/screenshots/add-new-torrent.png](docs/screenshots/add-new-torrent.png) (debug build, portable mode, a 10-file test torrent made with `patorrent-nox create`, captured with `PrintWindow`).

### Decisions
- Magnet links (and `.torrent` files the parser rejects) are still added directly: the dialog needs the file list. See PLAN.md Phase 5 for what showing it after the metadata arrives would take.
- `WM_COPYDATA` from a second instance now queues the arguments and posts `WM_APP_ARGS` to itself: the sender waits for `WM_COPYDATA` to return, so a modal dialog must not run inside it.
- A `.torrent` already in the list gets a message instead of the dialog.
- Name sets `AddTorrentParams::name`, which the engine only uses as the list name: the folder on disk keeps the torrent's name (no rename API in the engine; noted in PLAN.md, `crates/core` untouched).
- `windows-sys` feature `Win32_Storage_FileSystem` added (`SHGetFileInfoW`'s attributes, `GetDiskFreeSpaceExW`).

### Verified
- `cargo fmt --all --check`, `cargo +stable fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings` on 1.90.0 and stable (1.97.1); `cargo clippy -p patorrent-frontend --all-targets --target x86_64-unknown-linux-gnu -- -D warnings`.
- `cargo test --workspace --no-fail-fast` on Windows: win32 20 (6 new dialog tests, 1 new Preferences test), frontend 45 (8 new), core 545 (3 ignored) and every integration suite green.
- By hand: `patorrent.exe <test.torrent>` in portable mode shows the dialog (screenshot above) with the tree, icons, check boxes and free space at 100 % scaling. Not verified by hand: OK through to a download, other DPI scales, magnets, the drag-and-drop and second-instance paths (covered only by code review: they share `add_sources`).

## 2026-10-17_163533_1792247733: move storage (Phase 5)

Branch `claude/move-storage-2c300f`.

### Added
- `TorrentHandle::move_storage(path, MoveFlags)`, ported from libtorrent 2.0.11 `torrent::move_storage` / `on_storage_moved`, `posix_storage::move_storage` and `aux::move_storage` (`storage_utils.cpp`, with `move_file`): every file is renamed, or copied and deleted when the rename fails across volumes (EXDEV / `ERROR_NOT_SAME_DEVICE`); a missing file is skipped; on an error the renamed files go back. Then the part file follows (`PartFile::move_partfile`, now used) and the `PartFiles` registry is rekeyed, the sources of copied files are deleted and the directories left empty are removed (never the old save path itself, never a directory with other files in it).
- `MoveFlags` (libtorrent `move_flags_t`): `AlwaysReplaceFiles` (default), `FailIfExist`, `DontReplace` (keeps destination files, then rechecks), `ResetSavePath` (moves nothing, rechecks), `ResetSavePathUnchecked`.
- `Alert::StorageMoved { id, old, new }` and `Alert::StorageMovedFailed { id, error, file, operation }` (libtorrent `storage_moved_alert` / `storage_moved_failed_alert`, critical priority), `FileOperation` (libtorrent `operation_t`: file_stat, mkdir, file_rename, file_copy, partfile_move). `TorrentStatus::moving_storage` is set while a move is pending (the frontend already shows "Moving").
- Disk: `DiskIo::async_move_storage`, a barrier like the file priority change (libtorrent's fence job): the storage's later jobs wait on every disk thread, so a running torrent keeps downloading and seeding around the move and its queued writes land at the new place. `Storage`'s save path became mutable (`RwLock`), changed only by the move; open files are released first (Windows cannot rename open files).
- Engine: a running torrent moves through its storage. A stopped torrent gets a storage just for the move (libtorrent keeps a paused torrent's storage, so its files move too), released afterwards. A magnet without metadata just takes the new path. The new save path goes into the status and the resume data. A start waits for a pending move; removing with data deletes through the moving storage, at the destination; `NeedFullCheck` forces a recheck; shutdown waits for moves in flight so the saved path matches the files.
- `patorrent_core::MoveCompleted` (and `same_dir`): µTorrent's "Move completed downloads", client side as in qBittorrent: fed every alert, it calls `move_storage` on `TorrentFinished`, optionally only for torrents in one folder ("Only move from the default download directory"). Settings-free; frontends own the folder.
- `patorrent-nox add|run ... --move-on-complete DIR`: finished torrents move to DIR; `add` exits once the moves are done. Verified by hand: a complete torrent added with `--move-on-complete` moved and exited 0, its emptied folder removed.
- Tests: `test_storage.cpp`'s move cases (`move_storage_to_self`, `move_storage_into_self`, `move_storage_reset` for both reset flags, `dont_move_intermingled_files`), plus `FailIfExist` / `DontReplace` / overwrite, a rolled-back failure (a directory where a file must go), the part file and registry following a running move with a write queued behind it, a stopped storage's part file; `tests/move_storage.rs`: both ends moving mid-transfer (`test_transfer.cpp`'s `move_storage`), a seed moved while seeding and while stopped whose new path survives a restart and then seeds, a skipped file's part file following the move then unskipped, `FailIfExist` / `DontReplace` against a file at the destination, a magnet without metadata (`test_torrent.cpp`'s `test_move_storage_no_metadata`), stop + move + start and move + remove-with-data issued back to back, `MoveCompleted`, and a move from the temp directory into the target directory (another file system on both VMs: tmpfs on Ubuntu, a separate ZFS dataset on FreeBSD, so the copy path runs there); nox `--move-on-complete` parsing.

### Decisions (docs/PORTING.md "Deliberate divergences")
- The move is a per-storage barrier, and stopped torrents get a storage for the move (they keep none here, D5).
- A rollback moves back only the files this move renamed (libtorrent also "moves back" a destination file kept by `dont_replace`); errors name the path in the way (the destination file for `FailIfExist`, the new save path for mkdir, the part file's path).
- No `Settings` field: "Move completed downloads" stays with the frontend (`MoveCompleted`), so the settings table needs no new row.

### Merge notes (for the coordinator)
- `engine/mod.rs`: imports (`FileOperation`, `MoveError`, `MoveStatus`, `MoveFlags`), `shutdown` (wait for moves), `handle` (`Command::MoveStorage` arm), new `move_storage` / `on_storage_moved` before "Running, pausing, checking", `remove_torrent` (moving storage taken before `stop_torrent`), `start_torrent` (early return while moving), `on_disk` (`Completion::Moved` arm). `engine/torrent.rs`: `moving`, `moves_pending`. `disk/mod.rs`: `Barrier` now carries a `BarrierAction`.
- `Command::MoveStorage` and `TorrentHandle::move_storage` gained a `MoveFlags` argument; nothing else called them (the mock now posts `StorageMoved`).
- `crates/nox/src/main.rs`: one option (`--move-on-complete`), its usage lines, and a few lines in `run`'s alert loop.

### Verified
- `cargo fmt --all --check`, `cargo +stable fmt --all --check`, `cargo +stable fmt --manifest-path fuzz/Cargo.toml --check`; `cargo clippy --workspace --all-targets -- -D warnings` (1.90.0 and stable) on Windows, and for core / nox / frontend with `--target x86_64-unknown-linux-gnu` and `x86_64-unknown-freebsd`.
- `cargo test --workspace --no-fail-fast` on Windows (554 core unit tests, 3 ignored; `move_storage` 8, swarm 34, web seed 12, proxy 9, super seed 4, frontend 37, nox 2), Ubuntu 26.04.1 and FreeBSD 15.1 (555 core each; `move_storage` 8 and every other suite passing). On both VMs the temporary directory is another file system than the target directory (tmpfs; a separate ZFS dataset), so the copy-and-delete path ran.
- Not verified: a move across real volumes on Windows (another drive letter); the Win32 GUI (untouched).

## 2026-10-17_161604_1792246564: Phase 5 merge (Preferences, proxies, super seeding, part files)

Four features built in parallel on their own branches and merged into main: `claude/part-files-b92a97`, `claude/super-seeding-362b23`, `claude/proxies-c663c9`, `claude/preferences-99a764` (their entries follow).

### Added
- The Preferences Connection page's Proxy Server box is live (it was a disabled placeholder on the Preferences branch): Type ((none), Socks4, Socks5, HTTP), Proxy and Port, Authentication (picks `socks5_pw` / `http_pw`; SOCKS4 sends the user name as its user id), Username, Password (masked), "Resolve hostnames through proxy" (`proxy_hostnames`), "Use proxy for peer-to-peer connections" and "Use proxy for tracker connections". A proxy type without an address or port is refused on its page.
- `patorrent_frontend::settings` rows for the ten proxy settings; `proxy_type` uses libtorrent's `proxy_type_t` numbers (0 none .. 5 http_pw) through `proxy_to_int` / `proxy_from_int`.
- Tests: `the_proxy_box_sets_the_proxy_settings` (apply, ini round trip, shown back) and `a_proxy_needs_an_address`.

### Issues
- Merge conflicts: PLAN.md, CHANGELOG.md, docs/PORTING.md, THIRD-PARTY-NOTICES.md (each branch extended the same lines; combined), `fuzz.yml` and `fuzz/Cargo.toml` (`part_file` and `proxy` targets side by side), `engine/wire.rs` (super seeding moved the piece set into `write_bitfield`, proxies replaced `listen_port` with `announce_port()`; both kept).
- `settings::tests::every_field_has_a_row` failed after the proxies merge, as the Preferences branch predicted: the proxy fields had no rows. Fixed with the rows above.
- The unrelated old branch `claude/beautiful-curran-74d92d` (fixed DHT test addresses) is still unmerged; main fixed the same flake by widening the bucket-count range.

### Verified
- `cargo fmt --all --check`, `cargo +stable fmt --all --check`, `cargo +stable fmt --manifest-path fuzz/Cargo.toml --check`; `cargo clippy --workspace --all-targets -- -D warnings` (1.90.0 and stable) on Windows, and for core / frontend / nox with `--target x86_64-unknown-linux-gnu` and `x86_64-unknown-freebsd`.
- `cargo test --workspace --no-fail-fast` on the merged main: Windows 545 core (3 ignored), Ubuntu 26.04.1 and FreeBSD 15.1 546 core; swarm 34, web seed 12, proxy 9, frontend 37 and all other suites green on all three.
- Not verified: the Preferences dialog and the Initial Seeding menu item by hand; real proxy servers.

## 2026-10-17_160554_1792245954: proxies (SOCKS4/4a, SOCKS5, HTTP) in the engine and nox

Branch `claude/proxies-c663c9` (Phase 5).

### Added
- Ported libtorrent 2.0.11's proxy support: `socks5_stream`, `http_stream`, `proxy_base`, `proxy_settings`, `instantiate_connection`, the SOCKS5 UDP associate of `udp_socket`, the proxy listen socket and incoming refusal of `session_impl`, the web seed proxy branches of `torrent.cpp`, `utp_socket_manager::mtu_for_dest` with SOCKS5.
- Settings with libtorrent's names and defaults: `proxy_type` (`ProxyType::None`, `Socks4`, `Socks5`, `Socks5Pw`, `Http`, `HttpPw`), `proxy_hostname`, `proxy_port`, `proxy_username`, `proxy_password`, `proxy_hostnames` (true), `proxy_peer_connections` (true), `proxy_tracker_connections` (true), `socks5_udp_send_local_ep` (false), plus `proxy_send_host_in_connect` (false; not a 2.0.11 setting).
- `proto::proxy` (sans-IO): SOCKS4 / SOCKS4a CONNECT, SOCKS5 (RFC 1928) with RFC 1929 username / password, CONNECT and UDP ASSOCIATE, bind addresses of every type; HTTP CONNECT with basic authentication and an optional `Host` header; the SOCKS5 UDP header (`udp_wrap` / `udp_unwrap`). Each handshake says how many bytes it needs next, so nothing of the tunnelled stream is read. libtorrent's error codes and messages (`socks_error`, and the system errors SOCKS replies map to).
- `engine::proxy`: the session's proxy (its name resolved once per settings change, retried 30 s after a failure), the tunnel driver on non-blocking sockets, UDP routing, and the SOCKS5 UDP associate (10 s connect timeout, libtorrent's `retry_connection` backoff of 5..125 s, no retry after an authentication or protocol refusal).
- Through the proxy: outgoing TCP peer connections; HTTP and HTTPS trackers and web seeds (an HTTP proxy is asked for plain `http://` URLs directly, with `Proxy-Authorization`; everything else tunnels, TLS inside the tunnel); UDP trackers, the DHT and uTP through the SOCKS5 associate (uTP's MTU leaves room for the SOCKS5 header). With `proxy_hostnames`, tracker and web seed host names go to SOCKS5 / SOCKS4a proxies unresolved.
- With peers proxied (libtorrent's "proxy" listen socket): no TCP listener, incoming TCP refused, one IPv4 UDP socket, no LSD and no port mapping, port 1 announced to trackers, no listen port in the extension handshake, DHT lookups without announces. Datagrams from anyone but the relay are dropped while everything is proxied.
- `Alert::Proxy` (libtorrent `socks5_alert`): the proxy's name failing to resolve, the UDP associate failing or being lost, and (once a minute per error) peer and web seed connections the proxy refused for its own reasons; `ProxyOperation`. Tracker errors carry the proxy's error ("SOCKS authentication error").
- `patorrent-nox --proxy TYPE://[USER:PASS@]HOST:PORT` (socks4 / socks5 / http; credentials pick the `_pw` types).
- Tests: codec unit tests (SOCKS4 / 4a, SOCKS5 with and without a password and host names, every reply code and address type, HTTP CONNECT, UDP framing), tunnel and UDP routing tests, the HTTP proxy request format for trackers and web seeds, nox URL parsing; `tests/proxy.rs` with in-test SOCKS4, SOCKS5 (password, UDP associate relay) and HTTP proxies: downloads through each with no TCP listener, an HTTP proxy asked for a tracker and a web seed by absolute URL (port 1 announced), a tracker host name resolved by the SOCKS5 proxy, a wrong password (tracker error and `Alert::Proxy`), a UDP tracker through the associate (every datagram from the relay), the DHT's bootstrap query through the relay (`socks5_udp_send_local_ep`), and a uTP-only download through the relay.
- Fuzz target `proxy` (replies of every handshake kind in arbitrary pieces, SOCKS5 datagrams): 1,384,692 runs in 121 s on Windows, no findings; in the `fuzz.yml` matrix.

### Decisions (docs/PORTING.md "Deliberate divergences")
- The proxy's host name is resolved once by the session; connections and HTTP tracker requests wait for it.
- SOCKS4 gets host names as SOCKS4a with `proxy_hostnames` (libtorrent resolves locally for SOCKS4).
- HTTP CONNECT names an address resolved locally, as libtorrent does, unless `proxy_send_host_in_connect`; an `https://` web seed through an HTTP proxy tunnels to the seed.
- One UDP associate per session, on the UDP socket of the proxy's family; DHT and UDP tracker datagrams sent before it is up are held (64 at most), uTP datagrams are lost; a relay address of `0.0.0.0` means the proxy's address.
- The proxy listen socket binds the first configured IPv4 address when there is one (libtorrent: `0.0.0.0`).
- A tracker reached through a proxy casts no external-IP vote; a web seed reached by name gets a placeholder address in `100::/64` for blame.

### Fixed (during the work)
- A UDP tracker transaction started through a proxy that is then turned off no longer hits an `expect` in `udp_send`: it goes direct if it knows the tracker's address, and fails otherwise.

### Issues
- `proxy_send_host_in_connect` was asked for but is not in libtorrent 2.0.11; its meaning here is our reading (see PLAN.md Known TODOs).
- Merge notes for the parallel branches are in the shared side-session notes (`proxies-branch.md`): the Preferences branch's `every_field_has_a_row` test needs rows for the ten new `Settings` fields (`ProxyType` needs a hand-written `Entry`); `engine/mod.rs` has small hunks in `run`, `start`, `on_event`, `on_accept`, `on_udp_readable`, `on_peer_event`, `apply_settings`, `drain_completions`, `announce`, `stop_torrent`, `connect_to_peer`; `fuzz.yml` / `fuzz/Cargo.toml` add `proxy` next to other branches' targets.

### Verified
- `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings` for Windows, `x86_64-unknown-linux-gnu` and `x86_64-unknown-freebsd`; `cargo +stable clippy --workspace --all-targets -- -D warnings`, `cargo +stable fmt --all --check`, `cargo +stable fmt --manifest-path fuzz/Cargo.toml --check`.
- `cargo test --workspace --no-fail-fast` on Windows (532 core unit tests, 3 ignored), Ubuntu 26.04.1 and FreeBSD 15.1 (533 core), all integration tests passing, `tests/proxy.rs` 9 of 9 on each.
- Not verified: real proxy servers, HTTPS through a tunnel, IPv6 proxies, incoming uTP through the relay, the GUI (untouched: the Preferences branch owns it).

## 2026-10-17_160022_1792245622: part files for skipped files (Phase 5)

Branch `claude/part-files-b92a97`.

### Added
- `core::disk::part_file`, ported from libtorrent 2.0.11 `part_file.cpp` / `posix_part_file.cpp`: libtorrent's on-disk format (big-endian piece count, piece size and one `i32` slot per piece in a header rounded up to 1 KiB; slot `s` at `header + s * piece_size`). Created on the first write, read / write / `export_file` / `free_piece` / `flush_metadata` (deletes the file once it holds no piece) / `move_partfile`. `SlotMap` is the I/O-free slot map, fuzzed on its own.
- Storage (libtorrent `posix_storage`): files with priority 0 that are not on disk keep their blocks in `.<info-hash>.parts` in the save path and are never created (no zero-length or sparse placeholder, empty skipped files included). Reads, hashing, the full check and resume verification read through the part file. A file whose priority leaves 0 gets its blocks exported into it; one set to 0 that already exists keeps using the file (libtorrent's behaviour). Pieces no skipped file still overlaps are freed, so the part file disappears once nothing is skipped. A file unskipped while the torrent was stopped (no storage) is exported when the storage starts.
- `DiskIo::async_set_file_priority` (libtorrent's fence job): a barrier sent to every disk thread; each parks the storage's later jobs until the last thread has applied the change, then `Job::Unblock` replays them. Other torrents' jobs are not held up. `Completion::FilePriority`; an export error stops the torrent with `FileError` / `TorrentError` (libtorrent `on_file_priority`). The engine sends it from `set_file_priorities`; `new_storage` takes the file priorities.
- `RemoveFlags::DELETE_PARTFILE` (libtorrent `delete_partfile`) now deletes the part file (`FenceAction::DeletePartFile`, answered by `TorrentDeleted`); `DELETE_FILES` deletes it with the files.
- Tests: `test_part_file.cpp` (`part_file`; `posix_part_file` is the same code here), header format and parsing, slot reuse and trimming, export of boundary pieces, the registry; storage: a skipped middle file stays absent, the boundary pieces pass, a recheck and resume data find them, a restart keeps the part file, unskipping exports the blocks with the file's pieces queued right behind the change, re-skipping an existing file, delete with data / part file only, resume data that claims a piece the part file lacks, unskip while stopped, and the barrier's parking order; loopback swarm `a_skipped_file_stays_in_the_part_file_until_it_is_unskipped` (download with a file skipped, restart, recheck, unskip from a second seed, `DELETE_PARTFILE`).
- Fuzz target `part_file` (header parse with the torrent's and the header's own counts, slot map invariants, round trip, allocations and frees), in the `fuzz.yml` matrix.

### Decisions
- No storage flag to turn part files off: libtorrent 2.0.11 has none either (only `storage_mode_t` allocate / sparse); part files are always used for priority-0 files.
- Divergences in docs/PORTING.md: lowest-free-slot reuse and trailing slots cut off, duplicate header slots ignored; boundary pieces freed after an unskip; resume data must find claimed pieces in the part file (libtorrent exempts skipped files); the priority barrier; the open handle kept until release and live part files shared by path between the old and new storage of a restarted torrent (an old storage's late release must not write back a stale slot map).

### Issues
- The fuzzer's first run found a wrong invariant in the new target: a slot map with free slots at the end reads back trimmed (the header cannot record them; `flush_metadata` trims before writing). The check now compares with the trimmed map.
- After a restart, a finished leech closes its connection to a seed as redundant and then waits `min_reconnect_time` (60 s) to reconnect after an unskip, as libtorrent does; the swarm test unskips against a second seed. Noted in PLAN.md Known TODOs.
- Not done: the Win32 "Remove" (without data) still passes no flags, so it leaves the `.parts` file (PLAN.md Known TODOs); `move_partfile` is ported but unused until move storage. Not verified: interop with a real client while a file is skipped (`lt-swarm interop` was not rerun).

### Verified
- `cargo fmt --all --check`, `cargo +stable fmt --all --check`, `cargo +stable fmt --manifest-path fuzz/Cargo.toml --check`; `cargo clippy --workspace --all-targets -- -D warnings` for Windows, `x86_64-unknown-linux-gnu` and `x86_64-unknown-freebsd`; `cargo +stable clippy --workspace --all-targets -- -D warnings` (1.97.1).
- `cargo test --workspace --no-fail-fast` on Windows (530 core, 3 ignored; swarm 34), Ubuntu 26.04.1 and FreeBSD 15.1 (531 core each; swarm 34, web seed 12, all other suites passing).
- `fuzz.bat part_file 120`: 458,586 runs, no findings (after the fix under Issues).

## 2026-10-17_155425_1792245265: super seeding (BEP 16, µTorrent "initial seeding")

Built on branch `claude/super-seeding-362b23` (Phase 5).

### Added
- `engine::super_seed`, ported from libtorrent 2.0.11 `torrent.cpp` (`set_super_seeding`, `get_piece_to_super_seed`, the check in `files_checked`), `peer_connection.cpp` (`superseed_piece`, the super-seeding branches of `incoming_have`, `incoming_request`, `second_tick`) and `bt_peer_connection::write_bitfield`.
- Behaviour while `TorrentFlags::SUPER_SEEDING` is set on a checked seed: no bitfield / HAVE_ALL but HAVE_NONE (fast-extension peers) and two offered pieces per peer, picked by libtorrent's rule (rarest among connected peers, pieces offered to anyone avoided, random tie-break); a HAVE for an offered piece (also a redundant one) brings the next offer in its place; a peer not interested for 10 s gets another offer once a second; requests outside the offers are rejected (REJECT for fast peers, silently dropped otherwise); no allowed-fast set; `upload_only` is left out of the extension handshake. Switching it off sends HAVE_ALL / the bitfield to every super-seeded peer.
- `write_bitfield` is its own method now (handshake and the end of super seeding).
- The flag works through `set_flags` / `unset_flags` and `AddTorrentParams::flags`, and is saved in resume data as libtorrent's `super_seeding`. It is dropped when the check finds the torrent incomplete, ignored on a checked non-seed, and a seed that loses a piece ends the offers.
- `TorrentStatus::super_seeding` (libtorrent `torrent_status::super_seeding`): in effect, not just flagged. The frontend status text shows "[S] Seeding" ("[F] [S] Seeding" when forced).
- `patorrent-nox add ... --super-seed` (implies `--seed`).
- Win32: torrent context menu Advanced > Initial Seeding, checked when every selected torrent super seeds, greyed unless all are seeds; it toggles the flag.
- Tests: `super_seed::tests` (the piece choice: rarest, offered pieces avoided, random ties, seeds counted, nothing for a complete peer, unsized bitfields); `tests/super_seed.rs` (a super seed and two leeches that also trade with each other: both finish, the seed uploads 1.02-1.08 copies over 30 runs, the flag is dropped on a leech; a scripted peer: HAVE_NONE and two HAVEs instead of a bitfield, no allowed-fast, no `upload_only`, an un-offered request rejected, a HAVE bringing a new offer, the replaced offer rejected; switching off sends HAVE_ALL and un-offered pieces are served, and a new peer gets HAVE_ALL; a rate-limited leech keeps its connection through the switch and finishes); the resume round trip and the frontend status strings.

### Changed
- A BITFIELD / HAVE_ALL / HAVE_NONE after an earlier one replaces the peer's piece set (libtorrent `peer_lost` first) instead of closing the connection, so a super seed that stops can send HAVE_ALL after HAVE_NONE.

### Decisions
- Only peers that got the super-seeding handshake are limited to their offers: switching it on mid-run leaves connected peers with the full set (libtorrent would reject their requests). Recorded with the other divergences in docs/PORTING.md.

### Issues
- A loopback test flaked 3 times in 12 runs while being written: when the short last piece (6,713 bytes) was offered, the scripted peer asked for 16 KiB of it and got a correct REJECT. The test now requests the real block length.

## 2026-10-17_155028_1792245028: Win32 Preferences dialog and the string-keyed settings table

### Added
- Options > Preferences (Ctrl+P, and the toolbar button) is enabled: µTorrent 2.2.1's layout, a tree of pages (General, UI Settings, Directories, Connection, Bandwidth, BitTorrent, Queueing, Scheduler, Web UI, Advanced with Disk Cache under it) beside the page, OK / Cancel / Apply. OK and Apply store everything in patorrent.ini and apply it at once through `SessionHandle::apply_settings`; the settings are loaded at startup.
- Pages: UI Settings (confirm deletes, tray balloons, close / minimize to tray, add torrents stopped); Directories (download folder with Browse); Connection (listen port, Random port, randomize each start, UPnP, NAT-PMP, a disabled Proxy Server placeholder); Bandwidth (global rates in kB/s, global connections, peers and upload slots per new torrent, global upload slots); BitTorrent (DHT, LSD, PEX, uTP, encryption "Outgoing" + "Allow incoming legacy connections", IP filter enable / path / Browse / Reload); Queueing (active torrents, downloads, seeds, slow torrents); Disk Cache (disk write queue, open files, I/O and hashing threads); Advanced (filter, name / value list with `*` on changed values, a value field or True / False, Set, Reset). General, Scheduler and Web UI are disabled placeholders.
- `patorrent_frontend::settings`: every `Settings` field by its libtorrent name (bool / int with range / string; the encryption enums as libtorrent's numbers), plus µTorrent's `bt.transp_disposition`; `load` / `store` keep the non-default ones as `lt.<name>` lines (the old `net.port` key is read once and replaced by `lt.listen_interfaces`); `listen_port` / `set_listen_port`, `set_encryption` / `allow_legacy_incoming`, `transp_disposition` helpers.
- `patorrent_frontend::options`: `Options` (engine settings plus `GuiOptions`: folders, tray, per-torrent defaults, IP filter path), loaded from and stored to the preferences, `apply_to_new` for torrents being added, `random_port`.
- IP filter in the GUI: loaded at startup when enabled (from `ipfilter.path`, or `ipfilter.dat` in the settings folder like µTorrent), reloaded on Apply when the switch or path changed and by "Reload"; results go to the Logger tab.
- Tests: frontend `settings::tests` (every field has a row, every row round-trips, parsing and ranges, transport bits, listen ports, encryption, ini round trip) and `options::tests`; win32 `preferences::tests` (the dialog built hidden: every page and control, page switching, values in and out, Apply hands over the right `Settings`, invalid values refused without half-applying, untouched rounded values survive, the Advanced table and the pages follow each other, Reload, Random port).

### Decisions
- Page controls write back only what the user changed, so a value set on Advanced that a page shows rounded (a rate that is not a whole kB/s) is not clobbered by Apply.
- "Allow incoming legacy connections" off sets `in_enc_policy = Forced`; "Outgoing: Forced" pairs with RC4 only (`allowed_enc_level`), anything else with both, as noted under Phase 4.
- The uTP box sets both uTP directions; single transports are `bt.transp_disposition` on Advanced.
- Per-torrent peer and upload-slot limits default to no limit (the engine's behaviour so far) and apply to torrents added afterwards.
- A `Settings` field without a row in the table fails `every_field_has_a_row`, so branches adding settings (proxies, super-seeding, part files) must add rows.

### Not verified
- The dialog has not been clicked through by hand; only the hidden-dialog tests ran. Turning DHT, LSD or the port mappers off and on at run time relies on the engine's `apply_settings` and was not watched on a live network.

## 2026-10-17_153322_1792244002: CI fix for stable clippy and rustfmt

### Fixed
- CI run 39032057727 failed on stable Rust 1.97.1 (local checks use the pinned 1.90.0): `clippy::unnecessary_sort_by` twice in `engine/listen.rs` (now `sort_by_key`), `clippy::assertions_on_constants` in the `ip_notifier` start-up test (now a `cfg!` branch that panics), and rustfmt in `fuzz/fuzz_targets/web_seed.rs`. FreeBSD and Fuzz workflows had passed.

### Issues
- Local checks with the pinned toolchain miss lints that CI's stable clippy has. The stable toolchain here now has clippy and rustfmt: before pushing, also run `cargo +stable clippy --workspace --all-targets -- -D warnings`, `cargo +stable fmt --all --check` and `cargo +stable fmt --manifest-path fuzz/Cargo.toml --check`. Stable has no Linux / FreeBSD targets installed, so Unix-only code is checked against stable by CI only.

## 2026-10-17_140012_1792238412: web seeds, network-change handling, Win32 Create New Torrent dialog

Three features built in parallel on their own branches (`claude/web-seeds-1fbca0`, `claude/network-change-221b8e`, `claude/create-dialog-2e5433`) and merged into main, plus `claude/relaxed-kirch-0df13a` (unused uTP test accessors removed).

### Added: web seeds (BEP 19 `url-list`, BEP 17 `httpseeds`, Phase 5)
- Ported libtorrent's `web_peer_connection`, `http_seed_connection`, `web_connection_base`, the web seed handling of `torrent.cpp` and the response side of `http_parser`. New modules: `proto::http_parser` (incremental response headers, `Content-Range`, chunk headers, `100 Continue`, `Connection: close`, `resolve_redirect_location`, `split_url`), `proto::web_peer` (sans-IO request and response handling), `engine::web_seed` (lifecycle, blame, peer info). New picker method `pick_contiguous` (libtorrent `expand_piece`).
- Behaviour: range requests per file (pad files never requested, returned as zeros); a single-file URL may leave out the file name; per-file redirects create ephemeral seeds that have only the redirected files; keep-alive and pipelining (`urlseed_pipeline_size`), one request at a time to servers that close after each response; restart data for a block cut off mid-transfer; non-OK status retries after `Retry-After` / `urlseed_wait_retry`, DNS failures after `web_seed_name_lookup_retry`; `urlseed_timeout`, `max_web_seed_connections`, HTTPS through `net::tls`, IP and port filters, SSRF mitigation, IDNA ban; trust points and banning (a single-file seed that is the only sender of a bad piece is banned; a multi-file seed stops claiming the bad file's pieces); web seeds appear in the peer info and in the torrent's peer and seed counts.
- API: `TorrentHandle::add_url_seed`, `remove_url_seed`, `add_http_seed`, `remove_http_seed`, `post_web_seeds`; `Alert::UrlSeed` and `Alert::WebSeeds`; `AddTorrentParams::url_seeds` / `http_seeds`; resume data `url-list` / `httpseeds` (the store keeps only the info dictionary, so seeds are saved there).
- Settings, with libtorrent's defaults: `urlseed_pipeline_size`, `urlseed_timeout`, `urlseed_wait_retry`, `urlseed_max_request_bytes`, `web_seed_name_lookup_retry`, `max_web_seed_connections`, `ban_web_seeds`, `ssrf_mitigation`, `allow_idna`, `always_send_user_agent`. `util::string::base64encode` (basic auth in seed URLs).
- Tests: unit tests after `test_http_parser` and the web seed suite's six torrent layouts (plain and chunked), BEP 17, request format, restart data and the picker; 12 loopback tests in `tests/web_seed.rs` (multi-file, single file, chunked, HTTP/1.0 close, per-file redirect, single-file redirect, 503 then success, BEP 17, peer list, corrupt seed banned while a good one completes, IP filter, saved / added / removed seeds). Fuzz target `web_seed` (665k runs, no findings; in the `fuzz.yml` matrix).
- Deliberate divergences (docs/PORTING.md): web seed connections live in their own slab; requests stop at the rate-based queue instead of claiming 16 MiB windows; 503 / 429 don't mark the file missing on that server; a filtered seed waits `urlseed_wait_retry` before the next try; empty files don't clear pieces; the Host header carries a non-default port.

### Added: network-change handling, `listen_interfaces` expansion, IPv6 port mapping (Phase 4)
- `net::listen`, ported from libtorrent 2.0.11 `session_impl.cpp` / `string_util.cpp`: `parse_listen_interfaces` (device names, `[ipv6]:port`, `s` and `l` flags, malformed entries reported), `print_listen_interfaces`, `ListenEndpoint` / `ListenFlags`, `interface_to_endpoints`, `expand_unspecified_address`, `expand_devices`, `partition_listen_sockets`; plus our `remove_duplicates` and `listen_plan`. `enum_net` gained `is_global`, `is_link_local`, `has_internet_route`, `has_any_internet_route` and the `pointopoint` flag.
- `engine::listen` (libtorrent `reopen_listen_sockets`, `setup_listener`, `update_listen_interfaces`, `on_ip_change`): listen sockets are reopened against the endpoint list; matching sockets stay, the rest close, new ones post `ListenSucceeded` / `ListenFailed` per TCP and UDP socket. Device names (`tun0:6881`) and specific addresses get their own sockets and follow their device; wildcards stay one socket pair per family. `listen_interfaces` and turning incoming TCP / UDP on or off apply without a restart; a device that comes back gets its old port.
- `net::ip_notifier` (libtorrent `ip_notifier`): Linux netlink (link, address and route groups), FreeBSD / OpenBSD `PF_ROUTE`, Windows `NotifyIpInterfaceChange` / `NotifyUnicastIpAddressChange` / `NotifyRouteChange2`, elsewhere a 30 s re-read. A change is acted on after 1 s and only if interfaces or default routes changed: listen sockets reopen; LSD reopens if its interfaces changed; port mappers on vanished interfaces / gateways close, new ones start, unchanged ones keep their mappings; a family whose internet-facing address went away forgets its external-IP votes; running torrents re-announce to trackers and the DHT if internet-facing addresses or the port changed; TCP peers whose local address is gone are closed. The DHT starts late (or gains a node) when a family's first UDP socket appears after startup.
- NAT-PMP / PCP on global IPv6 listen addresses (PCP to the link-local default gateway with the interface's scope id); mappers follow the expanded endpoints.
- `SessionHandle::reopen_network_sockets()`; `patorrent-nox --listen LIST`.
- Tests: all of `test_listen_socket.cpp`, the `parse_interface` / `split_string_quotes` cases of `test_string.cpp`, `has_internet_route` of `test_enum_net.cpp`, plan / dedup, notifier message filters and start-up, the dual-stack TCP listener, voter reset; session tests with simulated interfaces; `tests/network_change.rs`.
- Decisions: wildcards are not split into per-address sockets (libtorrent does): one wildcard socket serves every address of its family; trackers, the DHT (one node per family), uTP and the voters keep their per-family design, and the expanded list drives everything interface-dependent. Voters reset when an internet-facing address is lost.

### Added: Win32 Create New Torrent dialog
- File > Create New Torrent (Ctrl+N) and the toolbar button are enabled. Layout after µTorrent 2.2.1: Select Source (path, "Add file", "Add directory"), Skip Files (`*` / `?` wildcards separated by `|` or `;`, case-insensitive per file and folder name), Torrent Properties (Trackers with blank lines between tiers, Web Seeds, Comment, Piece size "(auto detect)" and 16 kB..16 MB), Other ("Start seeding", "Private torrent", "Preserve file order"), "Create and save as..." and Close.
- Hashing runs on a worker thread (`set_piece_hashes`) with a progress bar; Cancel / Esc / close stop it; errors go to a message box. "Start seeding" adds the torrent with the source's parent folder as save path (the engine checks, then seeds). Trackers, piece size and the checkboxes are remembered in patorrent.ini (`create.*`).
- `patorrent_frontend::create` holds the toolkit-independent part for a later GTK dialog; `dialogs::Template` generalises the in-memory dialog template builder.
- Tests: frontend `create::tests` (tiers, skip patterns, piece sizes, a folder with every field, single file, file order, cancel and errors, a loopback session seeding a created torrent); win32 `create::tests` (the dialog built hidden: controls, combo entries, remembered values, fields to a parsed .torrent, Cancel mid-hash).
- Decisions: "Preserve file order" keeps each directory's listing order, unchecked sorts; the dialog is modal; no pad files (µTorrent 2.2.1 made none).

### Changed
- IPv6 TCP listeners are v6-only, so `0.0.0.0` and `[::]` share a port on Linux.
- `NatPmp::start` takes the NAT endpoint; `IpInterface` has `pointopoint`.

### Fixed
- `dht::routing_table::tests::routing_table_v6` / `_v4`: the bucket count of 10,000 random nodes ranged 10..=13 over 600 local runs and hit 14 on the Ubuntu VM; the check now accepts 10..=14.
- `utp/socket.rs`: three unused `#[cfg(test)]` accessors broke `clippy -p patorrent-core --all-targets` on Unix.

### Issues
- Merge conflicts: `THIRD-PARTY-NOTICES.md` (both branches extended the engine / proto line; combined), `engine/mod.rs` (new `network` and `web` fields side by side), `mock.rs` (both added ignored commands).
- Not verified: the Create dialog by hand in the GUI (computer use is blocked for patorrent.exe; checked through a hidden-dialog test and a PrintWindow render); HTTPS web seeds against a real server; live network-change notifications on FreeBSD (no root on the VM) and Windows (no admin rights to add an adapter); OpenBSD paths.

### Verified
- On merged main: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings` for Windows, `x86_64-unknown-linux-gnu`, `x86_64-unknown-freebsd`; `cargo test --workspace --no-fail-fast` on Windows (520 core, 3 ignored), Ubuntu 26.04.1 and FreeBSD 15.1 (521 core) with all integration, swarm, web seed and network-change tests passing. The network-change session also checked live change detection on Ubuntu with a dummy interface (`ip link add pt0 type dummy`: sockets opened, moved and closed, LSD 2 -> 3 interfaces).

## 2026-10-17_114537_1792230337: Create New Torrent (v1), IP filter, session-wide external-IP voting

Three features built in parallel on their own branches (`claude/create-torrent-8c3d6d`, `claude/ip-filter-8673bb`, `claude/external-ip-voting-26748b`) and merged into main.

### Added: Create New Torrent (engine and nox, v1)
- `core::metainfo::create_torrent`, ported from libtorrent 2.0.11 `create_torrent.cpp`: `CreateTorrent` (trackers with tiers, BEP 19 / BEP 17 seeds, DHT nodes, comment, creator, creation date, private flag, `source`, similar / collections / ssl-cert), libtorrent's automatic piece size and piece-size rules (v1: multiple of 16 KiB or a power of two, at most 128 MiB), the canonical layout with pad files (`CANONICAL_FILES`, `CANONICAL_FILES_NO_TAIL_PADDING`), BEP 47 attributes (hidden on Windows, executable and symlinks on Unix), and a verbatim copy of an existing v1 torrent. `add_files` walks a file or folder with a predicate; `set_piece_hashes` reads the files in order on the calling thread, hashes on up to four threads, reports progress per piece and can be cancelled from the callback.
- `make_magnet_uri_from_info` (libtorrent `make_magnet_uri(torrent_info)`).
- `patorrent-nox create <path> [-o OUT] [-t URL[,URL]]... [-w URL]... [--private] [--piece-size KIB] [--comment] [--source] ...` with a progress line and optional `--magnet`.
- Tests: the v1 parts of `test_create_torrent.cpp` (auto piece size, piece size limits, round trips, every metadata field, the v1 padding / sorting byte-exact cases), the `make_magnet_uri` cases of `test_magnet.cpp`, hashing from real directories (cancel, shrunk / missing files, pad files as zeros, hidden attribute on Windows), and a loopback swarm test that seeds and downloads a created torrent with pad files.
- Decisions: v2 / hybrid creation returns `V2Unsupported` until the merkle code (Phase 8); directory entries are added sorted by name (libtorrent uses `readdir` order); the nox default creator is `PaTorrent/<version>`.

### Added: IP filter and port filter (Phase 5)
- Ported libtorrent's `ip_filter` / `port_filter` (`core::net::ip_filter`) and how the session, torrents, the peer list and tracker connections apply them (`core::engine::ip_filter`).
- `SessionHandle::set_ip_filter`, `load_ip_filter(path)` and `set_port_filter`. Peers from trackers, the DHT, PEX, LSD and `connect_peer` are rejected on add; every outgoing connect is checked again (this also covers resume-data peers); incoming connections from blocked addresses are refused at accept (TCP and uTP) unless some torrent ignores the filter, and again when the handshake names a filtered torrent. A new filter closes connections to newly blocked peers and forgets them. Each case posts the new `Alert::PeerBlocked { id, addr, reason }` (libtorrent `peer_blocked_alert`, reasons `IpFilter` / `PortFilter`).
- Per-torrent `TorrentFlags::APPLY_IP_FILTER` semantics as in libtorrent; turning the flag on re-applies the filter.
- New setting `apply_ip_filter_to_trackers` (default true): tracker addresses the filter blocks are dropped after DNS; if none are left the announce fails with "banned by IP filter".
- Block-list parser for eMule / µTorrent `ipfilter.dat` (`first - last , level , description`, level <= 127 blocks, zero-padded octets, IPv6, lone addresses) and PeerGuardian P2P (`description:first-last`), skipping comments and counting invalid lines. New fuzz target `ip_filter` (1.5M executions locally with ASan, no findings; added to the fuzz workflow matrix).
- nox: `--ip-filter FILE` on `add` / `run`. Torrents added by nox now carry `APPLY_IP_FILTER` (they were added with empty flags).
- Tests: libtorrent's `test_ip_filter.cpp` ported (8 tests), plus parser, edge-of-range and a randomized model test; six loopback swarm tests (blocked seed never connected, blocked port, refused incoming, refused on attach, existing connection closed by a new filter, blocked and exempt trackers).
- Decisions (docs/PORTING.md "Deliberate divergences"): IPv4-mapped IPv6 addresses are looked up in the IPv4 table; resume-data peers are filtered when picked for a connection rather than when loaded; the port filter does not touch incoming peers (their source port is ephemeral). The `non_filter_torrents` counter is replaced by a scan of the torrents done only when an incoming address is actually blocked.

### Added: session-wide external-IP voting (Phase 4)
- `net::ip_voter::SessionVoter` keeps one libtorrent `ip_voter` per address family (libtorrent has one per listen socket; we have one listener / UDP socket / DHT node per family). Votes come from trackers (`external ip`), peers (`yourip` in the BEP 10 handshake; we already sent ours), DHT nodes (`ip`) and the NAT-PMP / PCP / UPnP external address (all routers as one voter, attributed to the unspecified address as libtorrent does).
- `engine::external_ip` ports `session_impl::set_external_address`: a change posts `Alert::ExternalIp`, re-ranks every torrent's peers (BEP 40, against the address each peer sees us at, so LAN peers rank against our LAN listen address) and gives the DHT node of that family a new BEP 42 id (`DhtTracker::update_node_id`). The DHT's private voter is gone; a node started after the address is known takes an id for it at once. The port-mapping rule "taken only while no address is known; trackers overwrite it" is replaced by the vote.
- Tests: all five `test_ip_voter` cases were already in; new unit tests for the session voter and `external_ip`, the DHT handing votes up, and loopback engine tests (`tests/external_ip.rs`) where a tracker reply, a peer's `yourip` and a DHT ping set the address. Divergences in docs/PORTING.md: per-family voters; router votes post the alert and move the DHT id (libtorrent casts them directly on the voter); the alert carries the newly voted address.

### Changed
- `PeerList::add` takes an `ExternalIp` instead of `Option<IpAddr>`; `DhtTracker::new_socket(is_v4, external, now)` and `take_events()` changed signature. `apply_tracker_results` hands tracker peers to the filtering `add_peers`.

### Fixed
- The disk layer did not zero pad-file ranges when reading pieces, and the recheck reuses one buffer, so a torrent with pad files never checked as complete (found by the create-torrent swarm test).

### Issues
- Merge: both test branches appended to `crates/core/tests/swarm.rs` (resolved by keeping both, header comment combined); `engine/mod.rs` conflicted only on the `mod` list. The tracker-reply block auto-merged: peers go through `add_peers`, then the tracker's `external ip` is voted.
- The Linux and FreeBSD VMs accepted only password logins; the owner installed the key `~/.ssh/patorrent_vm` on both. Both VMs had no Rust: rustup with 1.90.0 was installed as user `x` on each (Ubuntu also got `build-essential`, `curl`, `pkg-config`, `libssl-dev` through apt). FreeBSD has no sudo or git (installing them needs root's password), so the source goes over as `git archive | ssh ... tar -x`.

### Verified
- On the merged main: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings` on Windows, and `--target x86_64-unknown-linux-gnu` / `x86_64-unknown-freebsd` clean; `cargo test --workspace` on Windows: 468 core unit tests (3 ignored) and all integration / swarm tests pass. Same on Ubuntu 26.04.1 (Linux 7.0) and FreeBSD 15.1-RELEASE VMs: 469 core unit tests (the Unix-only `create_torrent_symlink` and `v1_attributes` included) and all integration / swarm tests pass (2026-10-17_130950_1792235390).

## 2026-10-16_225433_1792184073: lt_donthave (BEP 54) and smart-ban

### Added
- `proto::peer_wire`: the `lt_donthave` extension (libtorrent `bt_peer_connection::dont_have_msg`, id 7), `encode_dont_have` / `decode_dont_have` (a 6-byte extended packet; any other size is ignored, as libtorrent does). The extension handshake names it for every peer, private torrents included (libtorrent's built-in extensions). `engine::wire::on_dont_have` ports `peer_connection::incoming_dont_have`: an invalid or redundant index is ignored, otherwise the bit leaves the peer's set, the picker's availability drops by one (a seed is recounted as a peer with all pieces but that one, `peer_list::set_seed(false)`) and our interest is rechecked. `write_dont_have` goes out when an upload read fails (`on_disk_read_complete`: DONT_HAVE plus a REJECT) and when a piece in our set fails a hash check (`torrent::piece_failed` on a predictive piece: DONT_HAVE and rejects of its queued requests; the picker's `we_dont_have`).
- `engine::smart_ban`, a port of `smart_ban.cpp` and the blame half of `torrent.cpp` (`piece_failed`, `penalize_peers`, the trust points of `piece_passed`, `ban_peer`): the picker now remembers the peer each block was requested from or written by (`block_info::peer`, by address; `get_downloaders`). On a hash failure the senders are penalised (`Trust`: -2 trust points per failure with a floor of -7, `hashfails` capped at 255, parole; the only sender of the piece, or a peer at -7, is banned) and the plugin reads every block of the failed piece back and stores its SHA-1 per peer (`m_block_hashes`; a peer that sends two versions of a block is banned on the spot; a piece found bad on disk with no known sender costs nobody). When the piece passes, the recorded blocks are read back and compared: a peer whose block differs is banned, one whose block matches is cleared; the senders of a passed piece gain a trust point and leave parole; a seed drops the map. `use_parole_mode` (default true): a peer on parole only picks pieces nobody else has touched and never joins the end-game, so its next failure is its own. `torrent::ban_peer` closes the connection ("banned: ...") and posts `Alert::PeerBanned` (endpoint and peer id, as `peer_ban_alert`) plus a log line with the reason. `PeerInfo::num_hashfails` now comes from the peer-list entry (as `torrent_peer::hashfails`) and `PeerFlags::ON_PAROLE` is set.
- Tests: `dont_have_round_trip` (the framing of test_fast_extension's `dont_have` and the `lt_donthave` entry of `extension_handshake`); picker `we_dont_have` (after test_piece_picker's), `downloaders_and_parole`, `dec_refcount`; `smart_ban::tests` (trust points; the differing-block rule; two versions of a block; a banned or vanished peer; read errors; `clear` keeping reads in flight). Swarm tests: `a_peer_takes_a_piece_back_with_dont_have` (a scripted HAVE_ALL peer reads our handshake for the id and takes piece 3 back: no longer a seed, 40 of 41 pieces, availability of piece 3 at 0) and `smart_ban_bans_the_sender_of_a_bad_block_and_keeps_the_seed` (256 KiB pieces; a scripted peer answers its first two requests with inverted data and chokes; the seed supplies the other fourteen blocks, the piece fails, both are suspects; the seed downloads it again alone, it passes, the bad peer's blocks differ: one `PeerBanned` for it with its peer id, `total_failed_bytes` of one piece, the files correct). The `peer_wire` fuzz target decodes `lt_donthave` payloads of extended messages.

### Changed
- `PiecePicker::pick` and `mark_as_writing` take the peer's address; `Downloading` carries a downloader per block. Connection ids keep their top bit clear (the smart_ban read cookies use it).
- `PeerConn::hashfails` is gone (counted on the peer list).
- docs/PORTING.md: divergence rows for the smart_ban read-back (no piece lock), parole picking, the `PeerBanned` alert from smart_ban, and no predictive pieces. THIRD-PARTY-NOTICES.md lists smart-ban and lt_donthave.

### Issues
- The first run of the smart-ban swarm test in the full suite failed on `total_failed_bytes`: the scripted peer claimed every piece, and one time in six the picker's random tie-break starts with the fixture's short last piece (one block), so the bad peer was the only sender of a whole piece, banned by `penalize_peers`' sole-sender rule, and a second piece failed too. The scripted peer no longer claims the last piece.

### Verified
- `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`, also the core crate for `x86_64-unknown-linux-gnu`, `x86_64-unknown-freebsd` and with `--features fuzzing`; `cargo test --workspace` on Windows: 413 core (5 new: the codec, two picker and two smart_ban tests, plus `trust_points`), 3 robustness, 26 swarm (2 new), frontend and binaries, all green.

## 2026-10-16_220720_1792181240: uTP (BEP 29) with LEDBAT

### Added
- `core::utp`, a port of `utp_stream.cpp` / `utp_stream.hpp` (`utp_socket_impl`), `utp_socket_manager.cpp`, `packet_buffer.cpp` and `timestamp_history.cpp`: the uTP header and extensions (selective ACK; the close reason is parsed and dropped), sequence-number arithmetic with `compare_less_wrap`, the send buffer and the reorder buffer as circular packet buffers, duplicate-ACK and SACK-driven fast resends, timeouts with `utp_num_resends` / `utp_syn_resends` / `utp_fin_resends` and exponential back-off, Nagle with at most one undersized packet in flight, path MTU discovery (DF-bit probes between the 548-byte floor and the interface ceiling, `EMSGSIZE` handling, the three-entry `restrict_mtu` memory), the one-way delay histories (20-minute base, clock-drift compensation from the other end's base), LEDBAT with slow start and `ssthres`, the window cut on loss at most once per `utp_cwnd_reduce_timer`, the advertised receive window (1 MiB), the SYN / SYN-ACK / FIN / RESET state machine (`none`, `syn_sent`, `connected`, `fin_sent`, `error_wait`, `deleting`) and the deferred ACK that coalesces the ACKs of one wakeup. The manager demultiplexes datagrams by connection id (last-socket fast path), accepts SYNs (not past twice `connections_limit`), handles `WouldBlock` stalls of the UDP socket and ticks the timers. It is sans-IO: the engine passes a `UtpIo` sender and the clock in, and sockets with news for the upper layer go on a ready list instead of asio handlers.
- The listen socket pair: every TCP listener gets a UDP socket on the same port (`net::sys::udp_socket`, v6-only on IPv6), retried together over `max_retry_port_bind` ports when either bind fails. uTP and the DHT share it; the engine's `on_udp_readable` gives uTP first dibs on each datagram and hands bencoded ones to the DHT (libtorrent `session_impl::on_udp_packet`). The DHT no longer opens sockets of its own and announces with `implied_port` when incoming uTP is on. `net::sys::set_dont_fragment` (`IP_DONTFRAGMENT`, `IP_MTU_DISCOVER`, `IP_DONTFRAG`) is set around MTU probes.
- `PeerConn` carries a `Transport` (a mio TCP stream or a uTP socket id); reads copy out of the uTP socket's receive buffer, writes hand the send buffer to the congestion window and keep what did not fit, RC4 applies to either transport. The connect policy is libtorrent's (`torrent::connect_to_peer`, `on_connection_failed`, `on_connection_complete`): with `enable_outgoing_utp` every peer is tried over uTP first (`torrent_peer::supports_utp` starts true; a PEX `utp` flag sets it), a failed uTP connect clears it and reconnects over TCP at once, a successful one pins uTP (`confirmed_supports_utp`); outgoing TCP off means uTP only. Incoming uTP connections join through the same handshake path as TCP ones (`enable_incoming_utp`, `connections_limit`, session pause). ut_pex sends the `utp` flag for peers connected over uTP.
- Settings: `utp_target_delay` (100 ms), `utp_gain_factor` (3000), `utp_min_timeout` (500 ms), `utp_syn_resends` (2), `utp_fin_resends` (2), `utp_num_resends` (3), `utp_connect_timeout` (3000 ms), `utp_loss_multiplier` (50 %), `utp_cwnd_reduce_timer` (100 ms), with libtorrent's defaults; `enable_incoming_utp` / `enable_outgoing_utp` now do something. `SessionStats` gained `utp_sockets`, `utp_packets_in`, `utp_packets_out`, `utp_packet_loss`, `utp_timeouts`, `utp_resends`; `patorrent-nox` prints the socket count (and losses / timeouts) in its totals line and "N over uTP" in the `--verbose` peer report. `PeerInfo` sets `UTP_SOCKET` (the GUI's `P` flag).
- Tests: `test_packet_buffer` (insert, wrap, wrap2, reverse_wrap), `test_timestamp_history` (plus the two cases it leaves as TODOs: the minute step and a sample between the base and the minute entry), `compare_less_wrap`, header round trip, `mtu_for_dest`; `utp::tests`, two managers on a virtual network with a virtual clock in place of `simulation/test_utp.cpp`: connect, 1 MiB transfer, both directions at once, 4 % and 8 % loss both ways, Nagle, a stalled UDP socket, the close handshake deleting both sockets, a remote FIN as EOF, a connect nobody answers (times out after `utp_connect_timeout` and is not resent: an unconfirmed socket fails on its first timeout), a peer vanishing mid-transfer, RESET, non-uTP packets declined, incoming off, an oversized probe (`EMSGSIZE`) lowering the ceiling, a PMTU black hole killing the connection and restricting the MTU after three failures, the ready list on window opening. Swarm tests: the defaults connect over uTP, uTP-only sessions, the fallback to TCP when the seed has no UDP socket, a TCP-only leech on a uTP seed, RC4 over uTP, uTP-only vs TCP-only never connect. Fuzz target `utp` (datagrams into a manager without sockets, a connecting socket and an accepted one, with the connection id patched in, then the timers), in `fuzz/Cargo.toml` and the `fuzz.yml` matrix.

### Changed
- `Settings::enable_incoming_tcp = false` no longer skips the listen sockets: the UDP side still opens for uTP and the DHT, and `listen_port` is its port.
- The Linux IPv6 DHT limitation is gone: the IPv6 UDP listen socket is opened v6-only, so it binds after `0.0.0.0` and the DHT runs both families there too.
- docs/PORTING.md: divergence rows for the uTP I/O model, packet buffers, the close reason, mixed-mode throttling (not ported) and the listen-port retry; the DHT socket and announce-port rows updated. THIRD-PARTY-NOTICES.md lists `utp`.

### Verified
- `cargo fmt --all --check`; `cargo clippy --workspace --all-targets -- -D warnings`, also the core crate for `x86_64-unknown-linux-gnu`, `x86_64-unknown-freebsd` and with `--features fuzzing`; `cargo +nightly check` of the fuzz workspace; `fuzz.bat utp 90`: 434,780 runs, 1,133 coverage points, no finding. `cargo test --workspace`: 407 core (25 of them uTP), 3 robustness, 24 swarm (6 uTP), 18 frontend tests, all green on Windows.
- Real swarm, release `patorrent-nox add` of the Debian 13.7.0 netinst magnet with its tracker, `--dl-limit 6000`, 50 s: TCP and UDP listening on 6881 for both families; metadata at 2 s; the download at the 6 MB/s limit from 3 s on (278 MB at 49 s). Peers over uTP, from the 10-second reports: 55 of 75 connected at 10 s (tracker 34, DHT 7, PEX 36, incoming 3), 60 of 88 at 20 s, 110 of 161 at 30 s, 124 of 181 at 40 s with 92 incoming connections, all of them connected. 563 uTP timeouts over the run are the SYNs to peers without uTP or behind a NAT (each costs one `utp_connect_timeout` before the TCP retry); 23 packets lost. No error in the log.

### Notes
- libtorrent's `peer_proportional` mixed mode (throttling TCP while uTP peers are connected) is not ported; see the uTP follow-ups in PLAN.md.
- `lt-swarm interop` has not run with uTP yet (WSL2 is still down); the swarm run above was the real-world check.

## 2026-10-16_203336_1792175616: CI: fix the three flaky tests behind the red runs

### Fixed
- `engine::upnp::tests`: the fake router's `Stop` guard now joins both of its threads. Before, the SSDP responder had up to 100 ms left after the guard dropped and the HTTP server thread never stopped, so the next IGD test (serialized by `IGD_LOCK`, but not waiting for the threads) found two routers and talked to both: duplicate `PortmapEvent`s on FreeBSD (run 38913226650) and Linux, and a mapping made on one router but deleted on the other on Windows (run 38913226275). The guard now sets the flag, connects once to the HTTP listener to wake `accept()`, and joins; the IGD tests take 0.2 s instead of 0.01 s because of the responder's 100 ms read timeout.
- `dht::routing_table::tests::routing_table_v4`: the active-bucket assertion accepts 11..=13. libtorrent's test accepts 11 or 12, but the addresses and ids are random (OS RNG): 60 local runs gave 11 ×29, 12 ×27, 13 ×4, and FreeBSD run 38813771618 saw 13.
- `swarm::partial_pieces_survive_a_restart`: the stingy peer never serves the fixture's last piece as a partial (it is a single 1,625-byte block, so serving it completes the piece) and counts the partial bytes it actually sent instead of 16 KiB each. Run 38784078071 served piece 40 as a partial and the quota check never came true, so the test sat on the 30 s deadline.

### Changed
- All three workflows run Linux jobs on `ubuntu-24.04` instead of `ubuntu-latest`, which GitHub moves to Ubuntu 26 on 2026-10-19 (annotation on every recent run). A pinned image cannot change the system OpenSSL or toolchain under the build between two pushes.

### Verified
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` (also the core crate for `x86_64-unknown-linux-gnu` and `x86_64-unknown-freebsd`), `cargo test --workspace` (382 core, 3 robustness, 18 swarm, 18 frontend tests) on Windows. The UPnP module ran 15 times, `routing_table_v4` 40 times and the partial-pieces test 5 times in a row without a failure.

### Notes
- CI history: the Phase 0–3 runs were green except for the three tests above and the clippy / Unix build fixes already recorded. The MSE/PE commit (4561716) had not been pushed yet, so CI had not seen it; it goes up with this fix.

## 2026-10-16_163419_1792161259: MSE/PE protocol encryption

### Added
- `core::crypto::mse`, a port of `pe_crypto.cpp` / `pe_crypto.hpp`: RC4 (libTomCrypt's `rc4_init` / `rc4_encrypt`) and `rc4_handler` with the first 1024 keystream bytes of each direction discarded; `dh_key_exchange` (the 768-bit MSE prime, generator 2) on a fixed-width `U768` with a Montgomery multiplier (CIOS, `R = 2^768`) instead of boost::multiprecision; the derived keys `HASH("req1", S)`, `HASH("req2", SKEY)`, `HASH("req3", S)` and `keyA` / `keyB` (`init_pe_rc4_handler`). The secret exponent comes from `crypto_random_bytes`.
- `core::proto::pe_handshake`: the encrypted handshake as a sans-IO state machine (`PeHandshake::feed`), both sides, with libtorrent's states and limits: 96-byte keys with 0..=512 bytes of padding, the sync-hash / verification-constant search over at most 512 bytes, `crypto_provide` / `crypto_select` with `allowed_enc_level` and `prefer_rc4` (most or least significant bit), pads up to 512, `len(IA)` up to 68, the IA always RC4 and decrypted in place. The responder reports the torrent it found as a step, so the engine can accept or refuse it.
- `core::engine::pe`: `out_enc_policy` decides how an outgoing connection starts (`Forced`: encrypted; `Disabled`: plain; `Enabled`: plain first, then encrypted, alternating per peer through `KnownPeer::pe_support`, with `fast_reconnect` so the other kind is tried at once); an incoming connection whose first bytes are not the protocol string becomes an encrypted handshake unless `in_enc_policy` is `Disabled`, and a plain one is refused under `Forced`. Incoming encrypted connections find their torrent through `Engine::by_obfuscated` (`SHA-1("req2", info-hash)`, libtorrent `find_obfuscated`). `PeerConn` decrypts the receive buffer as bytes arrive and encrypts the send buffer as bytes leave once RC4 is on; the handshake timeout and the duplicate peer-id rule apply unchanged.
- ut_pex: we send the `e` flag for peers that completed an encrypted handshake and set `pe_support` for peers received with it (libtorrent `send_peer` / `update_peer`).
- Peer info: `RC4_ENCRYPTED` / `PLAINTEXT_ENCRYPTED` are set (the GUI already showed `E` / `e`).
- `lt-swarm interop --encryption forced|enabled|disabled`: sets our policy and the clients' (qBittorrent `Session\Encryption`, Transmission `encryption`, Deluge `enc_*`), the readiness probe handshakes encrypted when forced, and the result lines carry `<client>_encryption=rc4|plaintext|none`. `lt-swarm` enables the core's `fuzzing` feature for the handshake machine.
- Fuzz target `pe_handshake` (raw bytes into either side, plus a real exchange with the input choosing chunk sizes and corrupting bytes), in `fuzz/Cargo.toml` and the `fuzz.yml` matrix.
- Tests: `test_pe_crypto` (`diffie_hellman`, `rc4` with `test_enc_handler`), RC4 known answers (Wikipedia's vectors), exponentiation known answers (`2^767`, `2^768 mod p = 2^768 - p`, Fermat, a key at or above the prime), fixed-width key export; handshake tests over the level matrix with 1-, 7-, 100-byte and whole deliveries, refusals (incompatible levels, unknown torrent, 512 bytes without a sync hash). Swarm tests: forced on both sides over RC4; forced with both levels (encrypted handshake, plaintext stream); an `Enabled` leech refused in plaintext by a `Forced` seed retries encrypted; `Enabled` and `Disabled` talk plaintext either way; `Forced` and `Disabled` never connect, either way.

### Changed
- `PeerList::disconnected` takes `fast_reconnect`; `KnownPeer::pe_support` (default false, as libtorrent 2.0.11's `torrent_peer`).
- `HandshakeError::BadProtocol` now means "garbage or the start of an encrypted handshake"; the "Phase 4" close reason is gone.

### Verified
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` (also `--target x86_64-unknown-linux-gnu` and `x86_64-unknown-freebsd` for the core crate) and `cargo test --workspace` on Windows: 382 core unit tests, 18 swarm tests (5 new), all green. `fuzz.bat pe_handshake 30`: 8303 runs in 31 s (each run does two Diffie-Hellman exchanges under ASan), 163 corpus entries, no crashes.
- `lt-swarm interop --clients qbittorrent --encryption forced` (qBittorrent 5.2.4 set to "Require encryption"): the readiness probe handshook encrypted, and PaTorrent downloaded the 40 MiB (160 pieces) over RC4 (`qbittorrent_encryption=rc4`) in 1.61 s, first data after 0.2 s, 0 failed and 0 redundant bytes, files byte-identical. Without the option and with `--encryption enabled` (both sides "prefer"), the connection is plaintext (`pe_support` starts false, so the first attempt is plain and qBittorrent accepts it): 0.51 s, 0 failed bytes.

### Notes
- Divergences recorded in docs/PORTING.md: the xor mask is hashed over the padded 96-byte secret (libtorrent's responder uses an unpadded export and fails one incoming encrypted handshake in 256); `fast_reconnect` for both attempts of the `Enabled` alternation (our peer list waits `min_reconnect_time` after a clean close, libtorrent's does not); the OS RNG for the secret exponent.
- WSL2 would not start on this machine today ("Virtual Machine Platform" / virtualization off), so the Transmission and Deluge interop cases did not run; PLAN.md keeps them as a TODO.

## 2026-10-16_155608_1792158968: Port mapping (UPnP IGD, NAT-PMP / PCP)

### Added
- `core::proto::xml`: libtorrent's `xml_parse` scanner (tags, strings, CDATA, comments, attributes, parse errors as tokens), with all of `test_xml`'s cases. Two of libtorrent's unsigned underflows are guarded: a comment holding whitespace and `<?>`.
- `core::proto::natpmp`: NAT-PMP (RFC 6886) and PCP (RFC 6887) packets as `natpmp.cpp` builds and reads them: the public-address request, MAP requests of both versions (PCP with v4-mapped addresses and the nonce), and a reply parser with libtorrent's size checks, the "unsupported version" short-cut and the NAT-PMP → PCP result-code mapping with its message table.
- `core::proto::upnp`: the SSDP `M-SEARCH`, a parser for search responses and `NOTIFY`s (only newline-terminated lines count; a colon-less line ends the header, as in libtorrent's `http_parser`), `find_control_url` / `find_error_code` / `find_ip_address` over the XML scanner (`test_xml`'s `upnp_parser1-5`), the SOAP bodies of `AddPortMapping`, `DeletePortMapping` and `GetExternalIPAddress` and the POST around them (byte for byte as `upnp.cpp`), and the UPnP error table.
- `core::net::sys::routes` and `enum_net::{IpRoute, get_gateway}`: the routing table from `GetIpForwardTable2` (Windows), `/proc/net/route` plus `/proc/net/ipv6_route` (Linux) or a `NET_RT_DUMP` sysctl parsed as libtorrent's `parse_route` does (FreeBSD, OpenBSD); `Unsupported` elsewhere. `get_gateway` is libtorrent's: the default route through the interface, with the source hint telling networks on one device apart.
- `core::engine::natpmp`, a port of `natpmp.cpp`: one UDP socket per interface towards the gateway on port 5351; PCP first, NAT-PMP after an "unsupported version"; map requests resent 250 ms, 500 ms, ... apart, nine times, then a two-hour wait; leases renewed at three quarters of their lifetime; the public address from NAT-PMP's own request or PCP's MAP reply; every mapping deleted (TTL 0) on close without waiting for replies.
- `core::engine::upnp`, a port of `upnp.cpp`: SSDP searches on a multicast socket bound to the interface address on port 1900 and a unicast one, 2 s, 4 s, ... apart (twelve, four once a device answered, then "no router found"); up to 50 root devices, each fetched over a small non-blocking HTTP client (30 s for the description, 10 s per SOAP call, one request per device at a time); the first WANIPConnection:1 / :2 or WANPPPConnection:1 control URL; `GetExternalIPAddress`, then the mappings one by one; error 725 drops the lease duration, 718 and 501 move to a random port above 40000 (four tries), 727 and the rest are reported; leases refreshed at three quarters; `DeletePortMapping` for every mapping on close, and the engine's shutdown waits for those replies (up to `stop_tracker_timeout`).
- `core::engine::portmap`, the `session_impl` glue: a mapper pair per IPv4 interface the listen sockets cover that has a default route (every multicast-capable one where the routing table is unsupported), never loopback; each maps the TCP listen port and the DHT's UDP port with the same external numbers; `enable_natpmp` / `enable_upnp` start and stop them at runtime (a stopped UPnP mapper lives on until its deletes are answered); the router's external address is taken when none is known yet.
- Settings `upnp_ignore_nonrouters` (false) and `upnp_lease_duration` (3600), libtorrent's names and defaults. Alert `PortmapLog` (libtorrent `portmap_log_alert`); `PortmapSucceeded`, `PortmapFailed` and `ExternalIp` are now posted.
- nox prints mapped ports, failures and the external address; `--verbose` adds the mappers' log. The Win32 logger shows the results but not the step-by-step log.
- Fuzz targets `upnp` (SSDP, XML, device descriptions, SOAP replies) and `natpmp` (replies), also in `fuzz.yml`.

### Changed
- The DHT's socket list is visible to the mapper glue (the UDP port to forward).
- The loopback swarm tests, `lt-swarm` and `interop` switch both mappers off, so they never forward ports on the real router.
- PORTING.md: divergence rows for the per-interface mapper set, the UPnP HTTP client (IP literals, no redirects, no TLS), the unmap result (libtorrent reports a clean `DeletePortMapping` reply as "unknown UPnP error (-1)"), the log volume and the routing-table sources. THIRD-PARTY-NOTICES.md lists the new modules as ported.

### Fixed
- Swarm test `a_leech_finds_another_through_the_seeds_peer_exchange` failed 2 runs in 3 on the previous commit as well: since the duplicate-connection rule (LSD commit) A and B race to connect each other once both have the other's address from the seed, and the rule keeps the connection of the larger peer id, which B may see as incoming. A now runs with `DISABLE_PEX`, so only B learns an address and the direction is fixed (5 of 5 runs pass).

### Verified
- Unit tests: `test_xml` ported in full; SSDP response / NOTIFY parsing and every rejection; the exact SOAP bodies and POST header; NAT-PMP and PCP request layouts and reply decoding; the gateway lookup; the routing table reads on this machine (every route's interface is listed). `engine::natpmp`: maps through PCP and deletes on close, falls back to NAT-PMP (with the public-address request), gives up after nine resends. `engine::upnp` against a fake IGD on loopback (the port of `test_upnp`): the router model is found, two mappings succeed with the external address, close sends two `DeletePortMapping`s; a port conflict (718) moves to a random port and deletes that one; 50 mappings fit and the 51st is refused; a silent network disables the mapper after twelve searches with "no router found".
- `patorrent-nox run --no-state --seconds 25 --verbose` on this network (Technicolor Internet Gateway Device, `WANIPConnection:2` at 192.168.1.1:5000): NAT-PMP found the gateway from the routing table, got "unsupported version" for PCP, mapped TCP and UDP 6881 and learned the public address within a second; UPnP found the root device on the first search, read the control URL, the external address and mapped both ports within two seconds. A UPnP `GetSpecificPortMappingEntry` query of the router listed both as `PaTorrent/0.1.0` to 192.168.1.101 (lease 3592 s) while nox ran and answered 714 (no such entry) after it exited.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` (369 core unit tests, the swarm and robustness suites); clippy for `x86_64-unknown-linux-gnu` and `x86_64-unknown-freebsd` clean. `fuzz.bat upnp 40`: 700,898 runs; `fuzz.bat natpmp 30`: 6,594,023 runs; no crash.
- Not verified: Linux, FreeBSD and OpenBSD route parsing on a real machine (CI runs the unit test on Linux and FreeBSD), a router that only speaks NAT-PMP on another network, WANPPPConnection routers, the GUI on screen.

## 2026-10-15_235803_1792101483: Unix build fix for net::sys

### Fixed
- Linux and FreeBSD builds failed since the LSD commit (6770eac): the hand-declared C `socket` in `net::sys` had the same name as the module's own `socket` wrapper. The declaration is now `c_socket` (`link_name = "socket"`).

### Verified
- `cargo clippy -p patorrent-core -p patorrent-nox -p lt-swarm --all-targets --target x86_64-unknown-linux-gnu` and `--target x86_64-unknown-freebsd` with `-D warnings`: clean (the targets are now installed for the 1.90.0 toolchain, so Unix code can be checked on this machine). The previous commit gives 5 errors there, as in CI. Tests on Linux and FreeBSD run in CI only.

## 2026-10-15_235250_1792101170: CI clippy fix (sort_by_key)

### Fixed
- CI has been red since commit 24af0c6: the CI runner's newer clippy (stable) flags `sort_by` with a key comparison as `unnecessary_sort_by` in `dht/routing_table.rs` (replacement ordering and a test). Both now use `sort_by_key`; the order is the same (both sorts are stable). The local toolchain is pinned to 1.90.0, whose clippy does not have the lint, and the `stable` toolchain here has no clippy component, so this was checked by CI only.

## 2026-10-15_234806_1792100886: Local service discovery (LSD, BEP 14)

### Added
- `core::proto::lsd`: the BT-SEARCH packet as libtorrent's `render_lsd_packet` writes it (one info-hash, our cookie in hex), and a parser with the request-header rules of libtorrent's `http_parser` that `lsd::on_announce` relies on (method `bt-search` in any case, a blank or colon-less line ends the header, `strtol` for `Port` and `cookie`, port 1–65534, several `Infohash` headers, 40 hex digits and non-zero).
- `core::engine::lsd`, a port of `lsd.cpp` with the session and torrent parts: the wildcard listen addresses expand into every up, preferred local address (libtorrent `expand_unspecified_address`); one socket per interface and family, bound to port 6771 with address reuse (shared with other clients), in the group `239.192.152.143` / `ff15::efc0:988f` on that interface, hop limit 32, loopback on. A torrent announces when it starts (a magnet) or finishes checking, three sends at 0, 2 and 6 s, then in a round-robin over the running torrents within `local_service_announce_interval` (libtorrent default 5 min). Datagrams from outside the interface's subnets and our own (cookie) are ignored; peers go into the peer list as `PeerSource::LSD` and are connected at once. Never for private torrents, nor with `TorrentFlags::DISABLE_LSD`. `Settings::enable_lsd` (on by default, as in libtorrent) starts and stops it at runtime.
- `core::net::enum_net` (`IpInterface`, `match_addr_mask`, `build_netmask`, with libtorrent's `test_enum_net` cases) and `core::net::sys`: UDP sockets with options set before `bind` (address reuse, plus `SO_REUSEPORT` on the BSDs, v6-only), the outgoing multicast interface and IPv6 hop limit, and the local interface list (`GetAdaptersAddresses` on Windows; `getifaddrs` on Unix, declared by hand under D4 with each OS's constants and `sockaddr` layout).
- Alerts `LsdPeer { id, address }` and `LsdError { address, error }`; setting `local_service_announce_interval`.
- nox: `--verbose` prints LSD peers, the 10 s peer summary counts `lsd`, LSD errors go to stderr.
- Fuzz target `lsd` (parse, then our own announce for each hash must parse back to it), also in `fuzz.yml`.

### Fixed
- A peer reached at two of its addresses stayed connected twice. Ported libtorrent's duplicate peer-id rule (`bt_peer_connection`, `read_peer_id`): when a handshake carries the peer-id of another connection of the torrent, the connection initiated by the side with the smaller peer-id is closed, so both ends close the same one. LSD made this visible: two clients on one machine found each other on all 14 local interfaces and held 28 connections.

### Changed
- The loopback swarm tests, `lt-swarm` and its interop runs switch LSD off, so tests running in parallel on one fixture cannot find each other.
- PORTING.md: deliberate divergences for LSD sockets (one per interface and family, IPv6 by interface index), resends and pacing (one session-wide resend timer, running torrents only) and interface changes (read once). THIRD-PARTY-NOTICES.md lists LSD and the `enum_net` helpers as ported.

### Verified
- Unit tests: packet rendering and round trip, several and malformed `Infohash` headers, every rejected packet kind, C `strtol` behaviour, sockaddr encoding round trip, two reusable sockets on one port, the interface list holds loopback, endpoint expansion and grouping from listen addresses, the subnet filter with IPv6 scopes, netmasks and `match_addr_mask`.
- Swarm test `a_leech_finds_the_seed_through_local_service_discovery`: no tracker, DHT or `connect_peer`; the leech finishes the download from peers found through LSD on loopback (Windows: 0.2 s).
- Swarm test `a_second_connection_to_the_same_peer_is_closed`: the leech connects to a seed listening on `0.0.0.0` at 127.0.0.1 and 127.0.0.2; one connection stays on each side. Without the fix the leech kept two.
- Two `patorrent-nox` processes (ports 46881 and 46991, wildcard listen addresses) on this machine: both opened LSD on 14 interfaces (IPv4 and IPv6, including loopback and link-local), shared port 6771, found each other on every interface within a second, no LSD errors, and ended with one connection (source LSD on one side, incoming on the other).
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace`. `fuzz.bat lsd 60`: 941,718 runs, no crash.
- `lt-swarm interop` after the handshake change: qBittorrent 5.2.4 0.12 s, Transmission 4.1.0-beta.2 9.7 s (its 10 s rechoke), Deluge 2.2.0 0.38 s; all ok, no failed or redundant bytes.
- Not verified: Linux and FreeBSD (CI will run the swarm test there), a second machine on a real LAN, LSD in the GUI on screen.

### Issues seen
- `dht::routing_table::tests::routing_table_v6` failed in some full test runs (13 active buckets, the test accepts 11 or 12). It fails on the previous commit too (1 in 15 runs there, 3 in 15 here: random draws). Recorded in PLAN.md (Known TODOs).

## 2026-10-15_230633_1792098393: Rate limits no longer starve new peers of their handshake

### Fixed
- With a download limit set, peers that connected after the first ones never got past the handshake. The 100 ms bandwidth tick handed the refilled quota to read-blocked peers in slot order, up to 256 KiB each, so the first, busy peers took all of it every tick. A new peer's handshake sat in its socket until the 10 s handshake timeout. This was the "none of the DHT peers connect" seen in the PEX run: DHT and PEX peers arrive after the tracker peers have the low slots. Now each tick splits every channel's quota (the session's and each torrent's) evenly over the peers waiting for it, as libtorrent's `bandwidth_manager::update_quotas` / `bw_request::assign_bandwidth` do (`quota_shares`). The same applies to writes and to peers resumed after the disk queue drains.

### Diagnosis
- DHT peers were never the problem. `get_peers` `values` decoding (the mainline single-string format and the list format), `read_endpoint` and the engine's `normalize` (IPv4-mapped senders) match libtorrent, and peers go into the list of their own family. A dump of the peer states in the old build showed most DHT attempts stuck in `CONNECTING` (unreachable peers, normal for DHT). About 50 DHT and 49 PEX peers had connected over TCP but stayed in `HANDSHAKE`, and so did 15 tracker peers.
- Pending connects count against `connections_limit` as in libtorrent: `session_impl::num_connections()` is `m_connections.size()` and `torrent::want_peers` checks `num_peers()`, both including half-open connections. That was left as it is.

### Verified
- New swarm test `a_new_peer_gets_download_quota_while_another_saturates_the_limit`: a leech limited to 20 kB/s downloads from seed A, then seed B must get past the handshake within 8 s. It failed before the fix (B stayed in `HANDSHAKE`) and passes now.
- Real swarm, the same command as the PEX run (Debian 13.7.0 magnet with its tracker, `--dl-limit 6000`, 75 s): at 70 s, 150 of 198 peers connected (tracker 34/34, DHT 90/56, PEX 81/67), against 19 connected (DHT 0, PEX 0) with the old build. Download held at the 5.8 MB/s limit. Only 2 peers were still in the handshake; the rest pending were TCP connects to unreachable peers.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace`.

### Changed
- PORTING.md: a deliberate-divergence row for how bandwidth is handed out (token buckets plus an even split, instead of libtorrent's request queue and peer classes).

## 2026-10-15_221714_1792095434: Peer exchange (ut_pex, BEP 11)

### Added
- `core::proto::ut_pex`, a port of libtorrent's `ut_pex.cpp`: `TorrentPex` (the torrent plugin: once a minute, a diff of the peers added and dropped since the last one, at most 100 added, IPv4 and IPv6, seeds flagged), `PeerPex` (the peer plugin: the full list first, then the diff, once a minute while the torrent has other peers; incoming messages limited to 500 KiB and six per minute, peers remembered per connection up to `max_pex_peers` per family, dropped entries forgotten, local addresses taken only from local peers, the internal `lt_v2` flag cleared) and `encode_full`.
- `core::engine::pex`: we advertise `ut_pex` (id 1) in the extension handshake and run the ticks with the other per-torrent work. The list we send holds connected peers whose listen port we know: the address we connected to, or an incoming peer's handshake `p`. Peers we receive go into the peer list as `PeerSource::PEX` (a seed flag marks an unconnected entry as a seed) and are connected through the usual pacing. Off for private torrents (also for a magnet whose metadata turns out private) and with `TorrentFlags::DISABLE_PEX`.
- Setting `max_pex_peers` (libtorrent default 50).
- nox `--verbose` prints, every 10 s, the connected peers per source (all / past the handshake).
- Fuzz target `ut_pex` (peer state fed with several messages, then our diff and full list must decode), also in `fuzz.yml`.

### Changed
- PORTING.md: deliberate divergences for the diff's addresses and cap, and for connect boost, the DHT ping and private magnets. THIRD-PARTY-NOTICES.md lists ut_pex as ported.

### Verified
- Unit tests: the exact bencoded full list and its round trip, the 100-entry cap, filters and caps on receive, flooding, diffs over several rounds (added, dropped, unchanged, overflow into the next diff), the peer tick (full, then diffs, never alone).
- Swarm test `a_leech_finds_another_through_the_seeds_peer_exchange`: two sessions that want nothing connect to a seed; one connects to the other at its listen port, with source PEX.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` (322 core unit tests plus integration suites). `fuzz.bat ut_pex 60`: 204,793 runs, no crash.
- `lt-swarm interop` (qBittorrent 5.2.4, Transmission 4.1.0-beta.2, Deluge 2.2.0, PeX off on their side): all ok, files identical, no failed or redundant bytes. So advertising ut_pex did not upset their handshakes.
- Real swarm: the Debian 13.7.0 netinst magnet with its tracker, limited to 6 MB/s for 75 s: about 70 PEX candidates by 10 s (1–10 of them among the 21–29 connected peers) and another 22 in the next diffs about a minute later. No peer dropped us after our PEX messages (the connected count held steady).
- Not verified: PEX in the GUI on screen; that other clients act on the peers we send them.

### Issues seen
- In the same run, about 180 of the 200 connection slots were pending outgoing connects, and none of the ~170 DHT-sourced attempts completed. That predates PEX; it is recorded in PLAN.md (Known TODOs) for a look.

## 2026-10-15_211826_1792091906: Mainline DHT (BEP 5, 32, 42, 43)

### Added
- `core::dht`, a port of libtorrent's `kademlia/*` as sans-IO state machines:
  - `routing_table`: k-buckets with replacement caches, extended top buckets, prefix-spread replacement (`classify_prefix`, `replace_node_impl`), one entry per IP and per /24 (/64) per bucket, BEP 42 preference for splitting, `next_refresh`, `find_node`, `update_node_id`.
  - `node`: the rpc manager (2-byte transaction ids, 1 s short timeout that widens the branch factor, 15 s timeout), the traversal algorithm (sorted results, 100-result cap, aggressive lookups, search-IP restriction), `get_peers` / `announce` (announce_peer to the 8 closest nodes with write tokens), obfuscated lookups (`dht_privacy_lookups`), bootstrap, the 5 s refresh tick, and query handling (`ping`, `find_node`, `get_peers` with scrape bloom filters, `announce_peer` with rotating 4-byte write tokens, `want` lists across families, BEP 42 `ip` key, BEP 43 read-only).
  - `storage` (announced peers, 45 min expiry, limits), `dos_blocker`, `node_id` (BEP 42 ids, secret ids, prefix masks), `msg` (`verify_message`, compact nodes), `state` (`dht_state`), and `DhtTracker` (one node per address family, upload quota, write-key rotation, external-IP votes that move the node to a BEP 42 id).
  - Observers and lookups live in slabs owned by the node instead of libtorrent's shared_ptr graph; an observer is freed once it is neither a lookup result nor an outstanding request.
- `net::ip_voter` (libtorrent `ip_voter`, `is_local`, `hash_address`), `util::bloom_filter`, `util::random::crypto_random_bytes` (BCryptGenRandom / `/dev/urandom`, for write-token secrets and random ids).
- Engine (`engine/dht.rs`): UDP sockets for IPv4 and IPv6 on the TCP listen port (`ListenSucceeded` with `Protocol::Udp`), bootstrap routers from `Settings::dht_bootstrap_nodes` resolved on the resolver threads (the DHT starts once they are resolved, as libtorrent), `dht_state.dat` beside `resume.db` (read at startup, written on shutdown and every 30 min), announces paced over running torrents with new torrents first (libtorrent `prioritize_dht` / `on_dht_announce`), peers added with `PeerSource::DHT` (not for private torrents), magnet `dht=` nodes pinged, `SessionStats::dht_nodes`.
- Alerts `DhtReply { id, num_peers }` and `DhtBootstrap`. `Settings` gains libtorrent's `dht_*` settings with their defaults.
- Outgoing connections keep the source the peer list knows (DHT peers no longer show as tracker peers).
- nox: the totals line shows "DHT: N nodes"; `--verbose` prints DHT replies.
- Fuzz target `dht_message` (whole tracker: blocker, queries, replies to our own bootstrap, lookups), also in `fuzz.yml`.

### Changed
- Loopback swarm tests and `lt-swarm` run with the DHT off.
- PORTING.md: deliberate divergences for DHT sockets, announce pacing, the announce port, BEP 44/51 queries, ICMP errors and the external-IP vote. ARCHITECTURE.md: `dht_state.dat`.

### Verified
- Ported tests: `test_dht` (ping, invalid messages, get_peers/announce, scrape v4/v6, id enforcement v4/v6, routing table v4/v6 incl. the BEP 42 vectors, bootstrap, `want`, short nodes, get_peers v4/v6, traversal_done, dual stack, node_set_id, read-only node, error messages, unsorted results, routing table uniform/balance/extended/set_id/for_each, verify_message, node-id distribution, distance and prefix vectors, compare_ip_cidr, dht_state, node_entry ordering, mostly_verified, classify_prefix, replace_node_impl, all_in_same_bucket, rate limits), `test_dos_blocker`, `test_bloom_filter`, `test_ip_voter`, and `test_dht_storage`'s peer tests; plus timeouts, refresh and obfuscated-lookup tests. The BEP 44 / 51 / signing tests wait for Phase 8.
- Swarm test `trackerless_magnet_finds_its_seed_through_the_dht`: three loopback sessions (a bootstrap node, a seed announcing to it, a magnet with only `xt`) — the magnet gets DHT peers, the metadata and the files.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` (317 core unit tests plus integration suites). `fuzz.bat dht_message 120`: 393,856 runs, no crash.
- Real swarm, twice with a fresh state directory: `patorrent-nox add "magnet:?xt=urn:btih:7acf8fb590b2060dd9c3146ef770169d593433b0"` (Debian 13.7.0 netinst, no `tr=`): 11–14 DHT nodes after 1–3 s, DHT peers at 4 s, metadata at 4–5 s, the 792 MB ISO finished at 13 s (every piece hash-checked). `dht_state.dat` was written; a restart with it had 41 nodes after 1 s and 106 after 39 s.
- Not verified: the GUI's "DHT: N nodes" on screen; IPv6 DHT traffic on a network with IPv6 (this machine's runs used IPv4 nodes); Linux and FreeBSD (CI will run the unit and swarm tests).

## 2026-10-15_204502_1792089902: Magnet links (ut_metadata, BEP 9)

### Added
- `core::proto::ut_metadata`: the BEP 9 message codec (request / data / reject; 17 KiB message limit), `MetadataFetch` (libtorrent `ut_metadata_plugin`: block selection by fewest requests, at most one request per block every 3 s to a live peer, size limits, SHA-1 / SHA-256 verification incl. a v2 hash truncated to 20 bytes) and `PeerMetadata` (libtorrent `ut_metadata_peer_plugin`: two requests in flight, a 60 s pause after a reject, 20-70 s after bad data, 5 min more when a single-block download failed).
- `core::engine::metadata`: the engine side. Magnets and `TorrentSource::InfoHash` are added without metadata (name from `dn=`, else the info-hash; trackers from `tr=`, peers from `x.pe`, `so=` priorities applied once the file list is known) and run in the "Downloading metadata" state: they announce (`left` = 16 KiB, as libtorrent), connect to peers and fetch the info dictionary. Progress in that state is the metadata progress. When it is complete and matches the info-hash, it is stored in the metadata pack, `MetadataReceived` is posted, peers' piece maps (collected at wire length) get their real size, and the torrent checks and downloads with the same peers.
- We advertise `ut_metadata` (id 2) and `metadata_size` in the extension handshake and serve metadata blocks (libtorrent's 160 KiB send-buffer limit, queued requests beyond it). Private torrents neither advertise nor serve it.
- `Alert::MetadataFailed` and `Error::MismatchingInfoHash`. Metadata that matches the hash but does not parse stops the torrent with an error, as libtorrent does.
- Setting `max_metadata_size` (libtorrent default 30 MiB). `Bitfield::resize`. `encode_ext_handshake` takes `metadata_size`.
- Fuzz target `ut_metadata` (decoder plus a fetch fed with whatever the input claims), also in `fuzz.yml`.
- nox prints metadata received / failed.

### Changed
- A magnet waiting for metadata is saved and restored as such (no "metadata missing" error); removing one with "delete files" reports success.
- No unchoking for torrents without a piece picker (metadata or file check pending).
- PORTING.md: a deliberate divergence in how the metadata size is learned (see the table).

### Verified
- Unit tests for the codec, the fetcher and the peer back-off; swarm tests: a magnet with 40 KB of metadata (three blocks) fetches it from a seed, downloads, and is a seed after a restart without the seed; a magnet with no peers survives a restart in "Downloading metadata", then completes after a peer is added.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo test --workspace` (245 core unit tests plus integration suites). `fuzz.bat ut_metadata 60`: 886,718 runs, no crash.
- Real swarm: `patorrent-nox add "magnet:?xt=urn:btih:7acf8fb5...&tr=http://bttracker.debian.org:6969/announce"` (Debian 13.7.0 netinst, 60 KB of metadata in four blocks): metadata within a second of the first peer, the 792 MB ISO in 9 s from 40 peers. Adding Debian's own `.torrent` with the same save folder then checked every piece and went straight to Seeding with nothing downloaded.
- Release sizes: `patorrent.exe` 1.42 MB, `patorrent-nox.exe` 1.22 MB.
- Not verified: a magnet in the GUI on screen.

## 2026-10-15_203223_1792089143: PatinaTorrent icon candidates and state icon family

### Added
- `assets/patina-icons/`: three original teal-enamel/copper application icon directions, **Verdigris** (P/download monogram, recommended), **Exchange** (peer-to-peer arrows), and **Foundry** (copper gear/download badge). Transparent 1254 px PNG masters, PNG exports at 16/20/24/32/48/64/128/256/512 px and Windows ICOs with eight frames from 16 to 256 px. Masters were generated with the built-in image_gen tool; the exact prompts are saved.
- Thirty original editable 16-grid state SVGs: 18 current torrent variants, four tracker states, two network states, a wanted-files-complete detail, four optional future states and a force-start overlay. PNG exports at 16/20/24/32/48/64 px. The manifest records state conditions and precedence, including the distinction between core Finished and frontend Finished.
- Local interactive gallery with light/dark previews, actual-size proofs, category/size controls and download links; application and state comparison sheets; optional Node/sharp export helper and validation summary. No application code or default icon changed.

### Verified
- All 207 exported PNGs contain visible content with transparent corners. All 24 embedded ICO frames decode with the expected dimensions. Thirty SVG sources have accessible metadata and no external resources; every manifest state matches its source and exported PNGs.
- Inspected both comparison sheets at small and large sizes. The complete state family was checked against the actual frontend precedence. Refined the optional allocation badge for 16 px readability.
- Gallery: all images load, no script errors, candidate selection, tracker filtering, theme toggle and size-dependent PNG links work. No horizontal overflow at 1440 px or 390 px widths.
- Asset-only addition; application tests/build were not rerun. Raster app candidates use resampled small sizes rather than separately hand-hinted pixel artwork.

## 2026-10-15_202933_1792088973: Phase 3 closed

### Verified
- The owner ran `patorrent.exe` (via `run.bat`): the main window works, a torrent was added and downloaded through the GUI. All Phase 3 items are ticked in PLAN.md. The known GUI follow-ups (working set, network icon, Pause toggle, restore log lines) stay in Known TODOs.
- Still untried: a DPI change across two monitors.

## 2026-10-15_202147_1792088507: Fix the Fuzz CI workflow (musl target)

### Fixed
- Every Fuzz job failed at compile time with "sanitizer is incompatible with statically linked libc" and "can't find crate for `core`" for `x86_64-unknown-linux-musl`. `taiki-e/install-action` installs a prebuilt cargo-fuzz built for musl, and cargo-fuzz defaults `--target` to the triple it was built for. ASan needs glibc, so `fuzz.yml` now passes `--target x86_64-unknown-linux-gnu` to `cargo fuzz run` and `cargo fuzz cmin`.
- CI and FreeBSD were already green on the latest commit; the earlier CI run was cancelled by the newer push, not failed.

## 2026-10-15_201640_1792088200: GitHub repository renamed to PaTorrent (Q11)

### Changed
- `gh repo rename PaTorrent`: the repository is now `Balls0fSteel/PaTorrent` (private). GitHub redirects the old `LibreTorrent` URL, and gh switched the `origin` remote to the new URL.
- The `repository` field in Cargo.toml and the README clone URL use the new name.

## 2026-10-15_201238_1792087958: Renamed to PaTorrent; PolyForm Noncommercial license (D15–D17)

### Decisions
- D15: license PolyForm Noncommercial 1.0.0, copyright DukeCorp1996.
- D16: name PaTorrent (long name PatinaTorrent), peer-ID prefix `-PA0100-`.
- D17: no peer-ID spoofing.
- Entries below this one keep the old name LibreTorrent, as it was at the time.

### Added
- `LICENSE.md`: `Required Notice: Copyright 2026 DukeCorp1996 (https://dukecorp1996.x35.in/)`, then the PolyForm Noncommercial 1.0.0 text verbatim from github.com/polyformproject/polyform-licenses at tag 1.0.0 (sha256 of the text `c0ea4a89…fabbe5`).
- `license = "PolyForm-Noncommercial-1.0.0"` and `authors = ["DukeCorp1996"]` in the workspace and every crate (the fuzz crate included).
- README "License" section. The About box names PatinaTorrent, the copyright and the license. `patorrent-nox --license` prints LICENSE.md, then THIRD-PARTY-NOTICES.md. `build.bat` and the CI release artifact ship LICENSE.md.

### Changed (rename)
- Crates `patorrent-core`, `patorrent-frontend`, `patorrent-nox` and `patorrent` (Windows GUI, `patorrent.exe`). The fuzz crate is `patorrent-fuzz`. The manifest file is `crates/win32/res/patorrent.exe.manifest`.
- Run time: `%APPDATA%\PaTorrent`, portable `patorrent.ini`, `$XDG_*/patorrent`, single-instance mutex `Local\PaTorrent-…`, window classes, tray tip, dialog titles, resume DB `file-format` = `patorrent resume db`. No old `%APPDATA%\LibreTorrent` existed on this machine, so no migration code was added.
- Wire identity: peer ID `-PA0100-`, `User-Agent: PaTorrent/0.1.0`, BEP 10 `v` = `PaTorrent 0.1.0`. In our peer table `PA` = PaTorrent, and `LR` now maps to the Android LibreTorrent (as in libtorrent) instead of the old provisional `LB`.
- "libre" wording removed from README, PLAN and the About box (not accurate under a noncommercial license). PORTING.md and THIRD-PARTY-NOTICES.md updated.
- Not changed: the GitHub URL `Balls0fSteel/LibreTorrent` (PLAN.md Q11: the owner renames the repo first), the local folder name and the `lt-swarm` tool name (it means libtorrent swarm).

### Verified
- `cargo fmt --check` (workspace and fuzz), `cargo clippy --workspace --all-targets -D warnings` and `cargo test --workspace` (236 core unit tests plus the integration suites) pass. `cargo +nightly fuzz build` builds. `build.bat` fills `dist\` with patorrent.exe (1.39 MB), patorrent-nox.exe (1.19 MB), lt-swarm.exe, LICENSE.md and THIRD-PARTY-NOTICES.md. `patorrent-nox --version` and `--license` give the expected output.
- Not verified: the renamed GUI window on screen (computer use stays blocked for this app).

## 2026-10-15_180656_1792080416: Coverage-guided fuzzing (Q9); license and name answers recorded

### Added
- `fuzz/`: cargo-fuzz crate (its own workspace) with six targets: `bdecode` (plus partial decode and the printer), `torrent_info`, `magnet` (parse, write, re-parse), `peer_wire` (handshake, BEP 10 extension handshake, message stream), `tracker_response` (announce, scrape, compact v4/v6 peers) and `http_response` (responses, URL parsing, redirects).
- `libretorrent-core` feature `fuzzing`: a hidden `fuzzing` module re-exports the crate-private wire, tracker and HTTP parsers. Off in every normal build.
- `fuzz.bat <target> [seconds]`: runs a target on Windows. It finds Visual Studio's ASan runtime DLL with vswhere and seeds `bdecode` / `torrent_info` with the 103 test torrents.
- `.github/workflows/fuzz.yml`: one Linux job per target on nightly. Runs 120 s on pushes to main that touch `crates/core` or `fuzz`, 1,800 s weekly, or a chosen time by hand. The corpus is kept in the Actions cache (minimized after each run); crash inputs are uploaded as artifacts.
- CI `fmt` job also checks the fuzz crate. `.gitignore`: `/fuzz/target`.

### Tooling
- Installed the nightly toolchain (`rustup toolchain install nightly --profile minimal`) and cargo-fuzz 0.13.2. `cargo install cargo-fuzz` failed on the pinned default toolchain 1.90.0 (a dependency needs rustc 1.91), so it was built with `cargo +stable install cargo-fuzz --locked`. rustup's self-update failed with "Access is denied" on `rustup-init.exe` (harmless; rustup stays at 1.29.0).

### Issues
- `cargo fuzz build -s none` does not link with MSVC (unresolved `__start___sancov_pcs`). The default ASan build links, but the exe needs `clang_rt.asan_dynamic-x86_64.dll` on PATH (exit 0xc0000135 otherwise); `fuzz.bat` handles this. A first `for /f` in `fuzz.bat` lost the quotes around the vswhere path and found nothing; fixed by running it through `call`.

### Verified
- Unseeded 30 s runs, no findings: bdecode 231k execs, torrent_info 2.43M, magnet 1.51M, peer_wire 3.13M, tracker_response 1.22M, http_response 727k. Seeded 60 s runs through `fuzz.bat`: torrent_info 837k execs (2,852 coverage edges), bdecode 176k, no findings.
- `cargo fmt --check` (workspace and fuzz crate) and `cargo clippy -D warnings` (workspace, core with `fuzzing`, fuzz crate) pass.

### Decisions recorded (PLAN.md)
- D14: fuzzing setup as above.
- Q1: the owner wants their own name as author and a non-commercial license; PolyForm Noncommercial 1.0.0 is proposed; the copyright holder name and the consequences of not being open source are still to confirm.
- Q2: rename before the first release. Shortlist with free peer-ID codes: Ferrite (`FE`), Patina (`PA`), Oxbit (`OX`), Rivet (`RV`).
- Q8: peer-ID spoofing was asked about. It is not built, and the concerns are written down.

## 2026-10-15_174803_1792079283: Build and run scripts

### Added
- `build.bat` (release build of the workspace, copied into `dist\` with THIRD-PARTY-NOTICES.md; `build.bat debug` for a debug build), `run.bat` (starts `dist\libretorrent.exe`, building first if needed, passing arguments on) and `run-demo.bat` (the simulated engine with 2,000 torrents, or a given count). README describes them.
- `.gitignore`: `/dist`, `*.tmp`, a portable `libretorrent.ini` and `state\` in the repository root, Visual Studio and JetBrains folders, Windows thumbnail files and `.claude/settings.local.json`. `.gitattributes`: `.bat` files keep CRLF line endings, which `cmd` needs.

### Verified
- `build.bat` builds and fills `dist\` (libretorrent.exe 1.36 MB, libretorrent-nox.exe 1.16 MB, lt-swarm.exe 1.30 MB). `run-demo.bat 500` started `dist\libretorrent.exe` with 500 rows; it exited cleanly through File → Exit. `git status` stays clean with `dist\` filled.

## 2026-10-15_174600_1792079160: Affiliation disclaimer and license compliance check

### Changed
- README: a "Third-party code and affiliation" section. LibreTorrent is not affiliated with or endorsed by libtorrent, qBittorrent, µTorrent or BitTorrent, Inc. It says how the libtorrent port is licensed and that qBittorrent's name is used only for compatibility, interop tests and peer identification.
- THIRD-PARTY-NOTICES.md: the same disclaimer for libtorrent.
- `libretorrent-nox --license` prints THIRD-PARTY-NOTICES.md, which is built into the binary, so an executable copied out of the release folder still carries the BSD-3-Clause notice.
- GUI About box: libtorrent's copyright line, where to find the license texts, and the disclaimer.

### Compliance review
- libtorrent (BSD-3-Clause): source files keep the copyright notice (every ported file has an origin and copyright header). Binary releases reproduce it (the CI release artifact includes THIRD-PARTY-NOTICES.md, nox now embeds it). No endorsement is implied (new disclaimer). Code bundled with libtorrent and listed in its LICENSE: puff and ed25519 (zlib-style; not ported yet, notices already present), SHA-1 and SHA-256 (public domain, ported, credited). The Python bindings (Boost license) and `route.h` (APSL/BSD-4) are not ported and must not be without adding their terms.
- qBittorrent (GPL-2.0-or-later): no code, artwork or Web UI is used. The repo mentions it only in docs, the interop tool (which runs the user's installed copy) and the peer-ID table. Rule for D10 recorded in PLAN.md.

### Verified
- `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass; `libretorrent-nox --license` prints the notices.

## 2026-10-15_174445_1792079085: CI green on the Phase 3 commits

### Verified
- CI run 38762048688 on 4171eaa: all jobs pass (fmt, MSRV, tests on Linux and Windows, release-windows). FreeBSD run 38762048759 passes, including the swarm test that hit the write-quota overflow before.

## 2026-10-15_173755_1792078675: CI failures on the Phase 3 push: an engine overflow, a tight test, a new lint

CI run 38761101805 and FreeBSD run 38761101723 (on 1c626c1) failed for three unrelated reasons.

### Fixed
- Engine crash (FreeBSD: `attempt to add with overflow` in `PeerConn::write_some`). Without an upload limit the write quota is `usize::MAX`, and `spos + (max - total)` overflowed whenever an earlier write had left bytes unsent. Debug builds panicked. Release builds wrapped, sliced out of range and aborted the process (`panic = "abort"`). The bug is older than Phase 3. It needs a partial write followed by an unlimited one, which loopback rarely produces on Windows or Linux. The sum now saturates. Test: `engine::peer::tests::unlimited_write_after_a_partial_one` (fails without the fix).
- `swarm::partial_pieces_survive_a_restart` timed out on the Linux runner ("the leech did not ask for enough blocks"). The scripted peer ignores most requests, and unanswered requests fill the leech's pipeline: 4 deep at first, 1 once the peer counts as snubbed. With four partial pieces to collect, a slow runner could sit on them past the 30 s deadline. The test now collects two partial pieces; everything after that is measured relative to what was sent. On a timeout it prints every (piece, offset) the leech asked for. Passed 12 of 12 locally before the change and 6 of 6 after, so this is a reasoned fix, not a reproduced one.
- clippy `collapsible_match` (new in CI's Rust 1.99, not in the local 1.90) in the GUI's Open Containing Folder action.

### Verified
- `cargo test --workspace` (236 core unit tests), `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass locally.

## 2026-10-15_172326_1792077806: Phase 3 started: the µTorrent 2.2.1 main window on the real engine

### Added
- `libretorrent.exe` now runs the real engine (state in `state\`, preferences in `libretorrent.ini`). It is portable when `libretorrent.ini` sits beside the exe, like µTorrent's `settings.dat`; otherwise it uses `%APPDATA%\LibreTorrent`. `--demo N` still runs the simulated engine.
- Main window (`crates/win32`):
  - menu (File, Options, Help) and accelerators (Ctrl+O, Ctrl+U, Ctrl+A, Del, Shift+Del)
  - toolbar with original icons drawn at run time (`icons.rs`: vector shapes, 4×4 supersampling, premultiplied alpha, separate disabled set; no image files, sharp at any DPI)
  - category tree with counts (Torrents: Downloading, Completed, Active, Inactive; Labels: No Label plus each label)
  - virtual torrent list (`LVS_OWNERDATA`) with 24 columns, a progress bar in the Done cell, header-click sorting with arrows, a column chooser on the header's context menu, drag-to-reorder, and the selection kept on the same torrents across re-sorts
  - detail tabs: General (download and availability bars, transfer and general fields), Trackers, Peers (µTorrent flags), Pieces (piece map shaded by availability), Files (priority menu), Speed (600 s graph), Logger
  - status bar (network state, DHT nodes, D/U rate and totals) and draggable splitters
  - layout persisted: window placement, splitters, visible panes, column order, widths and visibility, sort, current tab
- Adding torrents: Open dialog (multi-select), drag and drop, command line, Add from URL dialog (prefilled from the clipboard; `http(s)` fetched with the OS downloader on a worker thread, `magnet:` passed to the engine). Single instance per settings folder: a second start forwards its arguments with `WM_COPYDATA` and exits.
- Torrent context menu: Open Containing Folder, Force Start, Start, Pause, Stop, queue moves (up, down, top, bottom), Remove, Remove And Delete Data (both confirmed), Force Re-Check, Update Tracker, per-torrent download and upload limits, labels (new, remove, pick). Labels and Paused-versus-Stopped are kept in `libretorrent.ini` per info-hash.
- Tray icon: show or hide on click, a menu (add, start all, stop all, exit), rates in the tooltip, a balloon when a download completes, re-added when Explorer restarts. Close to tray (on by default) and Minimize to tray (off) are under Options.
- `frontend` crate: `columns` (titles, widths, cell text, sort order, natural name sort), `model::TorrentList` (row cache, category and label filters, sorting, change counter), `detail` (Trackers/Peers/Files tables), `prefs` (key=value file, values verbatim), `fmt::date_time`.
- `lt-swarm load --keep DIR` leaves the generated state in place (used to open 10,000 torrents in the GUI).

### Fixed
- Engine: a `StateUpdate` dropped because the alert queue was full lost its torrents for good. `post_torrent_updates` reports only changed torrents and had already cleared their flags, so nothing brought them back. Dropped updates now mark their torrents changed again. Found with 10,000 saved torrents in the GUI: the restore alerts filled the queue and the list stayed empty. Test: `swarm::dropped_state_updates_are_reported_again` (fails without the fix).
- GUI: alerts queued before the notify callback was registered never triggered it (it fires on empty → non-empty only). The window now drains the queue once at startup.
- Preferences: the first file format escaped backslashes, so hand-written Windows paths lost them (`C:\Users` read back as `C:Users`). Values are now stored verbatim.

### Verified (release build, this machine)
- Scale gate, 10,000 saved (stopped) torrents on the real engine, three runs: window in 35–242 ms, all 10,000 rows listed in 209–404 ms (target ≤ 2 s), 60–61 MB working set (target ≤ 150 MB), idle CPU 0.08–0.10 % of a core over 60 s (target < 1 %). The first measurement showed 0.6–1.3 %: every second re-sorted all rows and repainted the list and detail panel even when nothing changed. Empty updates no longer mark the list dirty, and the tree, list, panel and status bar repaint only on change.
- Empty session: 19.9 MB working set, 4.5 MB private. The ≤ 12 MB RSS target is not met as a working set, mostly shared system DLLs (shell32, comctl32, urlmon); see PLAN.md.
- Simulated engine with 10,000 torrents (2 % active): 38 MB.
- Single instance: a second start with a `.torrent` path handed it over in 25 ms and exited; the running window listed it. A torrent added in one run was restored in the next.
- Selecting a torrent filled the Files tab from the real engine. File → Exit saves `libretorrent.ini` and the engine state and exits in about 65 ms with 10,000 torrents.
- `libretorrent.exe` release: 1.36 MB (target ≤ 4 MB).
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass.

### Not verified
- Nothing visual. Computer use is blocked by policy for this app, so the checks above went through process statistics and Win32 queries (child windows, row counts, selection), not screenshots. Icons, the Done bars, the painted tabs, menus, dialogs, drag and drop, the tray and DPI changes still need a look by a person.
- No download through the GUI yet. The engine path is the same as nox's, which is tested.

## 2026-10-15_164712_1792075632: CI green on the Phase 2 closing commits

### Verified
- CI run 38749949289 on abfc698: all jobs pass on the first try (fmt, MSRV, tests on Linux and Windows, release-windows). That covers the new partial-pieces swarm test and the swarm-test fix from efb4944, whose first CI run this was. FreeBSD run 38749949308 passes.

## 2026-10-15_161111_1792073471: `lt-swarm interop`: LibreTorrent downloads from qBittorrent, Transmission and Deluge

This closes Phase 2: the reverse direction of the interop exit criterion is now verified and repeatable.

### Added
- `lt-swarm interop [--clients qbittorrent,transmission,deluge] [--mb N] [--qbittorrent PATH] [--wsl-distro NAME] [--keep]` (`tools/swarm/src/interop.rs`). It generates a 3-file torrent (default 40 MiB, 256 KiB pieces, odd file sizes so pieces straddle files) and runs each client as a seed with a throwaway configuration: DHT, LSD, PeX and port mapping off, everything else (encryption, uTP) at its defaults. Then an in-process LibreTorrent session downloads from it and the files are compared byte for byte. Output is `key=value` lines plus a summary, like the other subcommands.
  - qBittorrent: the installed app with a temporary `--profile` (legal notice accepted, peer-country lookup off in its ini), `--save-path`, `--skip-dialog`. The user's own profile is never touched.
  - Transmission (`transmission-cli -M -U -v`, `settings.json` in a temp config dir) and Deluge (`deluged -d` with a temp `core.conf`, the torrent added through `deluge-console`): in the WSL distro on Windows (`wsl.exe -e sh -c`, not through the login shell), directly with `sh` elsewhere. Their data is copied into `/tmp/lt-interop-<pid>/<client>` first.
  - Readiness needs no client API: a probe connects, handshakes and counts the pieces in the client's `bitfield` and `have` messages, and the download starts once the client has them all. The probe also reports the client's peer-ID prefix.
  - `LT_INTEROP_TRACE=1` prints download progress every 200 ms.

### Verified (this machine, Windows 11 plus the Debian 13 WSL distro)
- 40 MiB, 160 pieces, from each client: files byte-identical, payload exactly the torrent size, no failed or redundant bytes.

  | Client (peer-ID) | Time | First data after |
  |---|---|---|
  | qBittorrent 5.2.4 (`-qB5240-`) | 0.20 s | under 0.2 s |
  | Transmission 4.1.0-beta.2 (`-TR410B-`) | 9.72 s | 9.6 s |
  | Deluge 2.2.0 (`-DE220s-`) | 0.41 s | 0.2 s |
- No client process and no temp directory is left behind on Windows or in WSL.
- `cargo clippy -p lt-swarm --all-targets -- -D warnings` and `cargo fmt --check` pass.

### Issues encountered
- Transmission took 9.7 s on every run, with no data for the first 9.6 s and then the whole torrent in one burst. Delaying our connect by 4 s made data start 4 s sooner after connecting, so the wait runs on a timer from Transmission's start, not from our connection. That fits Transmission's periodic (10 s) rechoke, which unchokes new interested peers only on its next pass. It is not an engine stall. `-U` (no upload limit) made no difference and stays in the command.
- The first cleanup command never removed the WSL temp directory. `pkill -f <dir>` also matched the cleanup shell's own command line, which contained the same path, so the shell was killed before `rm` ran. The pattern is now written as `[/]tmp/...` and the `rm` path is built from a variable.
- `wsl.exe` prints a warning about an unknown key `wsl2.pageReporting` in the user's `.wslconfig`. It is harmless and was left alone.

## 2026-10-15_160110_1792072870: Partly downloaded pieces survive a restart

### Added
- Resume data now carries libtorrent's `unfinished` list: for each partial piece, `piece` and a `bitmask` of finished blocks (MSB first, the block bitfield's bytes, as `write_resume_data.cpp` writes it). Malformed entries are skipped, as `read_resume_data.cpp` does.
- Saving: blocks in the `Finished` state (on disk) are recorded when the torrent stops and in every resume save while it runs. Blocks still being written are left out. While a torrent is stopped its partial pieces stay on `Torrent::unfinished` and count towards `total_done`.
- Restoring follows `torrent::on_resume_data_checked`: the blocks go back into the picker only when the resume data matched the files. A full check (no resume data, `force_recheck`, or resume data that failed) drops them.
- Disk: the new `async_restore_partial` job tells the piece's hash state which blocks are already on disk. They are read back when the in-order hash cursor reaches them. A piece that was complete but not yet hashed at shutdown is hashed at once. A restored block the files no longer hold fails the piece, which is downloaded again, rather than failing the torrent.

### Verified
- Unit tests: resume round trip including `unfinished` and malformed entries, picker save/restore, and the disk restore paths (blocks around restored ones, a piece complete on restore, a truncated file).
- `swarm::partial_pieces_survive_a_restart`: a scripted peer sends one whole piece plus block 0 of four other pieces. After a restart the leech reports the same `total_done` (96 KiB) without a recheck, finishes from a real seed with no failed hash, and downloads only the missing bytes. With the restore disabled the test fails (32 KiB done after the restart).
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass.

### Notes
- As in libtorrent, partial blocks are saved only with the periodic or final resume save (`resume_dirty` is set when a piece passes and when the torrent stops), not after each block. After a crash, blocks written since the last save are downloaded again.

## 2026-10-15_153331_1792071211: Flaky HTTP tracker swarm test: root cause and fix

### Fixed
- `swarm::download_through_an_http_tracker` (one failure on the Windows runner, CI run 38740694714) was a bug in the test harness, not in the engine. `wait()` popped every queued alert, returned at the first match and dropped the rest of that batch. When the test thread was slow (a loaded runner), the leech had already downloaded the torrent by the time the thread popped `TrackerReply`, so `TorrentFinished` was in the same batch and was discarded. The "finished" wait then saw only what came after: the "completed" announce (`on_finished` makes it due at once) failing against the one-shot tracker, which had exited, and its retry 17.5 s later. Those are exactly the two "failed to connect to tracker" errors in the CI log. Because of the tracker backoff, a stalled download that never finished could not have produced that pair in this order; a finished one does.
- The swarm tests now wait through a `Watch` (session handle plus a backlog): alerts behind the matching one stay queued for the next wait.

### Verified
- Reproduced the CI signature exactly by sleeping 5 s before the "tracker reply" wait (a simulated slow test thread): timeout with only the tracker errors. With the fix, the same delay passes.
- 12 concurrent runs of the swarm test binary (CPU load): all pass. `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass.

### Notes
- Peer path, reviewed while looking for the stall: a failed outgoing connect waits `min_reconnect_time × (failcount + 1)` (120 s after the first failure) before a retry, as in libtorrent. This did not cause this failure. A loopback test that depends on a single seed would stall on such a failure, though, so it is worth keeping in mind. Peer disconnect reasons are still dropped in `remove_peer` (see PLAN.md Known TODOs).

## 2026-10-15_152239_1792070559: CI fully green

### Verified
- CI run 38740694714 on 16572af: all jobs pass (Linux and Windows tests, MSRV, fmt, release-windows with the slow tests and the size report) after a rerun of the failed Windows job. FreeBSD (run 38740694536) passes.

### Issues encountered
- Flaky test: `swarm::download_through_an_http_tracker` failed once on the Windows runner. The tracker reply arrived (the later "failed to connect to tracker" errors are re-announces after the test's one-shot tracker had exited), but the download did not finish within 60 s. It passed on the rerun, on the previous CI run, and 15 of 15 times locally. It is in the peer path, not the tracker code; PLAN.md Known TODOs tracks it.

## 2026-10-15_144716_1792068436: CI after the first push of Phase 2 and HTTPS

### Verified
- FreeBSD CI (run 38736854046): passed. The OpenSSL TLS backend links against the base system's libssl and its unit tests pass (ClientHello with SNI, IP literal, plaintext-server failure).

### Fixed
- clippy `drain_collect` (new in CI's Rust 1.99, not in the local 1.90): `engine/wire.rs` snub now uses `std::mem::take`. This failed the Linux and Windows CI jobs (run 38736854243) before the tests ran.
- Release-only test build: the picker test calls `check_invariant`, which existed only with `debug_assertions`, so `cargo test --release -- --ignored` (the release-windows CI job, run 38738279981) did not compile. It now also exists under `cfg(test)`.

### Verified (run 38738279981)
- Linux CI passed: clippy, and `cargo test --workspace` with the OpenSSL backend linked against the runner's system OpenSSL. Windows tests, MSRV, fmt and FreeBSD passed too.

### Issues encountered
- Local clippy (Rust 1.90) cannot see lints that are new in CI's stable toolchain; `rustup update` would close the gap (left to the user).

## 2026-10-15_144002_1792068002: HTTPS trackers through the OS TLS library

### Added
- **`core::net::tls`** (LibreTorrent's own OS glue; libtorrent uses OpenSSL through Boost.Asio, PLAN.md D9):
  - `TlsSession`, a sans-IO client session trait (ciphertext in, plaintext out, outgoing ciphertext buffer), and `TlsStream`, which runs a session over a non-blocking socket behind plain `Read` / `Write`. The handshake is driven by mio readiness: writes return `WouldBlock` until it is done.
  - SChannel backend (Windows): `SCH_CREDENTIALS` for TLS 1.3 with a fallback to `SCHANNEL_CRED` (TLS 1.2) on older Windows; automatic chain and host-name validation against the Windows trust store; SNI through the target name; TLS 1.3 post-handshake messages (session tickets arrive as `SEC_I_RENEGOTIATE`) go back through `InitializeSecurityContextW`. One process-wide credentials handle, so SChannel can resume sessions.
  - OpenSSL / LibreSSL backend (Unix): memory BIOs, `SSL_VERIFY_PEER` with the default trust store, `SSL_set1_host` (or the IP address for literals), SNI for host names. Declared by hand against the API common to OpenSSL 1.1.1, 3.x and LibreSSL.
- HTTP trackers: `https://` announces, scrapes and redirects now work; the connection is a plain or TLS stream between the socket and the unchanged HTTP code (`engine/tracker.rs`).
- Tests: `TlsStream` over a fake non-blocking socket with a stand-in session (handshake, partial writes, close_notify, EOF during the handshake); the OS backend's ClientHello (SNI present, absent for IP literals) and failure against a plaintext server. Live tests behind `--ignored` and `LT_NETWORK_TESTS=1`: HTTPS GETs to Ubuntu (TLS 1.2) and Cloudflare (TLS 1.3), and rejection of a wrong host name, an expired, a self-signed and an untrusted-root certificate (badssl.com).

### Changed
- No new crates. `windows-sys` gains the `Win32_Security_Authentication_Identity`, `Win32_Security_Credentials` and `Win32_Security_Cryptography` features in the engine. Unix builds link the system `libssl` and `libcrypto`; Linux CI installs `libssl-dev`. THIRD-PARTY-NOTICES.md lists the system TLS libraries.

### Verified
- `libretorrent-nox add ubuntu-24.04.5-live-server-amd64.iso.torrent` (and the 26.04 desktop torrent): both `https://torrent.ubuntu.com/announce` and `https://ipv6.torrent.ubuntu.com/announce` answered, and the peer they returned sent data. Their answer holds one peer even with `numwant=200`; curl gets the same 6-byte peer list, so that is the tracker, not our parser.
- Live tests on Windows 11: Ubuntu negotiated TLS 1.2, Cloudflare TLS 1.3 including the session-ticket renegotiation path; the four bad certificates failed with readable messages (wrong host name, expired, not trusted).
- `cargo test --workspace` (233 core unit tests, 6 of them new; 3 robustness, 4 swarm, 7 frontend) and `cargo clippy --workspace --all-targets -D warnings` pass on Windows; `cargo fmt --check` is clean.

### Not verified / open
- The OpenSSL backend type-checks and passes clippy on Windows (compiled with a temporary cfg, not linked), but it has not been linked or run on Linux or FreeBSD: no Rust toolchain in WSL, and CI runs on push. See PLAN.md Next steps.
- Revocation is not checked (see PLAN.md Known TODOs).

### Issues encountered
- A shell command with a stray `cat >` waited on stdin until the tool timeout; it was stopped before running anything.

## 2026-10-15_143444_1792067684: Interop: Transmission and Deluge download from libretorrent-nox

With this, all four Phase 2 exit criteria are met.

### Setup
- Installed in the Debian 13 WSL distro, with the user's approval: `transmission-cli` 4.1.0~beta2 and `deluged` / `deluge-console` 2.2.0.
- The Debian package enabled and started a system `deluged` service. It was stopped and disabled after the test (`sudo systemctl enable --now deluged` brings it back). The packages stay installed.
- Neither CLI can be told about a peer directly, so a tiny HTTP tracker in WSL handed out the nox seed's address. The clients got a copy of the torrent with that announce URL (same info-hash).
- nox (from commit f3fbefa) seeded on Windows port 17882; the WSL clients reached it through the WSL NAT at 172.18.112.1. Both clients ran with temporary config directories, default encryption settings, and DHT / LSD / port mapping off.

### Verified
- **Transmission 4.1.0-beta.2** downloaded the 3-file, 38.7 MiB test torrent in about 3 s; all files SHA-256 identical to the originals.
- **Deluge 2.2.0** (libtorrent 2.0.x) downloaded the same torrent in about 5 s from adding it; all files SHA-256 identical.
- nox's upload counter shows exactly one copy per client (38.7 MB, then 77.4 MB in total), each to one incoming peer.

### Issues encountered
- The `transmission-cli` log looked like a failure (it ends at "Progress: 0.0%"). It is cut off mid-line: stdout is block-buffered when redirected, so its final status lines were never written. The files and nox's counters show the transfer completed.
- `deluge-console` 2.2.0 prints an `AttributeError: 'ConsoleUI' object has no attribute 'started_deferred'` traceback on this Python version; the commands still run. Its `info` showed "Downloaded: 0 B" after finishing although the files were written; a console display quirk.
- An inline `wsl.exe bash -c` command lost its variables between Git Bash and WSL; nothing was damaged (an empty `rm -rf`), and WSL commands moved to script files.

### Cleanup
- The tracker, the private deluged and the nox seed were stopped; `/tmp/lt-interop` in WSL and the Windows test data were deleted.

## 2026-10-15_143016_1792067416: Interop: qBittorrent downloads from libretorrent-nox

### Verified
- qBittorrent 5.2.4 (the installed app, started with a throwaway `--profile` in a temp directory, so the user's own profile was never touched) downloaded a 3-file, 38.7 MiB test torrent (256 KiB pieces) from `libretorrent-nox add ... --seed` over loopback.
  - qBittorrent's settings were mostly defaults: DHT, PeX, LSD and port forwarding off so it only saw our seed; encryption left at its default ("allow").
  - qBittorrent connected to nox (nox reported an incoming connection) and finished about 7 s after the peer was added through its Web API. The finished download is `stalledUP` at 100 %; downloaded bytes equal the torrent size (no hash failures or waste).
  - All three files are SHA-256 identical to the originals.
- Exe tested: `libretorrent-nox` built from commit f3fbefa.

### Issues encountered
- qBittorrent 5.x refuses to start its Web UI without credentials, even with localhost authentication bypassed. The temp profile got a random throwaway login (PBKDF2 hash only, the password was never stored).
- qBittorrent downloaded its IP geolocation database (db-ip.com) on first start of the fresh profile; harmless, but noted.
- Not measured: why the transfer took about 7 s to start. A likely cause is qBittorrent trying an encrypted or uTP connection first, which nox does not support until Phase 4, before falling back to plain TCP; not confirmed.

### Cleanup
- The test qBittorrent instance (the PID we started) and the nox seed were stopped; the temp profile and test data were deleted. No other qBittorrent was running.

## 2026-10-15_124018_1792060818: Phase 2, the real engine

### Added (ported from libtorrent-rasterbar 2.0.11 unless noted)
- **`core::disk`**: pread/pwrite storage on a thread pool plus a checking thread. Jobs for a piece always run on the same thread, so incremental SHA-1 hashing needs no locks; blocks are hashed in order as they are written (early blocks wait in a reorder cache), so finished pieces are never read back. Fences (release, delete files and empty directories), full checks, resume verification by file size, an LRU file-handle pool, sparse files on Windows, and back-pressure on peers when writes queue up.
- **`core::proto`** (sans-IO codecs):
  - peer wire: handshake, BEP 3 messages, BEP 6 (with the allowed-fast set generator), BEP 10 extension handshake, peer-id generation and client identification
  - HTTP/1.1 response parsing with chunked bodies and redirects
  - tracker announce/scrape URLs and responses (BEP 3/23/24/48) and BEP 15 UDP packets
- **`core::engine`**:
  - the session loop on mio: listen sockets, outgoing connections with pacing, a 500 ms tick over running torrents only, bandwidth buckets, queueing and auto-management, session pause
  - torrents: check → download → seed, pause/resume/force start, remove with or without data, file priorities, sequential download, force recheck, rate limits
  - peers: request pipelining sized by download rate, end-game with cancels, snubbing, timeouts, keep-alives, upload serving with disk reads capped by the send watermark
  - piece picker (rarest first, priorities, partial pieces first, sequential), peer list with BEP 40 ranks, a fixed-slot rate-based choker with an optimistic slot
  - trackers: HTTP and UDP state machines on the reactor, tiers, backoff, `started`/`completed`/`stopped` events, connection-id cache, scrapes when announces carry no counts
- **`core::net`**: resolver threads with a cache; listen-interface parsing; token buckets.
- **`core::resume`** and **`core::store`** (new): libtorrent fast-resume records in one `resume.db` plus an append-only `metadata.pack`; metadata is loaded only when a torrent starts.
- **`util::stat`**: `sliding_average` (with libtorrent's tests) and transfer-rate channels.
- **API**: `Session::start(SessionParams { settings, state_dir })`, `TorrentHandle::connect_peer`, new settings (connection pacing, tracker timeouts and policy, peer timeouts, request queue, choker intervals, send watermark).
- **`libretorrent-nox`** on the real engine: `add <file> --save DIR [--seed]`, `run` (resume saved torrents), `--state`, `--port`, rate limits, `q` to quit cleanly.
- **`lt-swarm loopback`** and **`lt-swarm load`** benchmarks, also in the CI job summary.
- **Tests**: unit tests for every new module (BEP 6 and BEP 40 reference vectors, libtorrent's sliding-average tests) and `tests/swarm.rs` (loopback downloads through a direct connection, an HTTP tracker and a UDP tracker, then a restart that resumes without rechecking).

### Changed
- `mio` (and through it `log`, plus `libc` on Unix) is now an engine dependency, as planned in D2/D4; `windows-sys` is used by the engine for sparse files. THIRD-PARTY-NOTICES.md lists them.
- The `mock` feature is off by default; the Win32 GUI, `lt-swarm` and nox's `demo` command enable it explicitly.
- Magnet links and bare info-hashes are rejected with a clear message until `ut_metadata` lands; v2-only torrents until Phase 8.

### Verified
- **Real download**: Debian 13.7.0 netinst (792 MB) from `bttracker.debian.org` with `libretorrent-nox add`: 7–9 s, 30–40 peers, SHA-512 identical to Debian's `SHA512SUMS`. Done twice; the second run on the final code.
- **Restart**: `libretorrent-nox run` restored the torrent as a seed after resume verification (no hashing).
- **Benchmarks** (release, this machine): 1,000 saved torrents restore in 12 ms and 10,000 in 109 ms (45 MB RSS); 512 MiB over loopback in 0.47 s (about 1.15 GB/s, 2.5 ns CPU per byte).
- `cargo test --workspace`: 241 tests pass (227 core unit, 3 robustness, 4 swarm, 7 frontend) plus 1 release-only test. `cargo clippy --workspace --all-targets -D warnings` and `cargo fmt --check` are clean. The core crate alone (without `mock`) also builds and passes.

### Not verified / open
- Seeding to qBittorrent, Transmission and Deluge (exit criterion). A 60-second public seeding run uploaded nothing: Debian's swarm is almost all seeds (seed-to-seed connections are closed by design) and the inbound port is probably firewalled on this machine. qBittorrent is installed here but was not launched, to avoid touching the user's desktop profile; PLAN.md Q10 asks how to set up interop testing.
- HTTPS trackers (needs the OS TLS layer), saving partial pieces in resume data, batched UDP scrapes.
- MSRV 1.85 is not checked locally (only Rust 1.90 is installed); CI checks it.

### Fixed during development
- `file_piece_range_inclusive` returns an exclusive end (it includes partly covered pieces); a unit test caught that file priorities spilled into the next piece.
- The allowed-fast list a peer sent us was first used to decide what *we* serve while choking; there are now separate lists, and we send our own BEP 6 set.
- Bulk adds rewrote `resume.db` once per torrent (quadratic; 10,000 adds timed out). Store writes are now debounced.
- A first BEP 40 version used a guessed mask scheme; it was replaced by a straight port and libtorrent's test vectors.

### Issues encountered
- Shell heredocs mangled some multi-line edit scripts again; edits now go through script files.
- Rust 2024 reserves `gen`; renamed a local.

## 2026-10-15_115117_1792057877: CI fixes for Phase 1

### Fixed
- MSRV 1.85: SSE4.2 CRC32C intrinsics need an explicit `unsafe` block before Rust 1.87. It is now wrapped and marked `#[allow(unused_unsafe)]`, so newer compilers stay warning-free.
- clippy `byte_char_slices` (new in CI's stable clippy, Rust 1.99): two test loops now use byte strings.

### Issues encountered
- This machine runs Rust 1.90, while CI uses the latest stable (1.99). Newer clippy lints show up in CI first. Running `rustup update` locally would close the gap; that is left to the user.
- First Phase 1 CI run: FreeBSD passed, including the non-Windows filename rules; Linux and Windows clippy and the MSRV check failed on the items above.

## 2026-10-15_114351_1792057431: Phase 1, primitives

### Added (ported from libtorrent-rasterbar 2.0.11, with its tests)
- **`core::bencode`**
  - zero-copy decoder: 8-byte tokens, libtorrent error codes and positions, partial trees, soft-error checks, `print_entry`
  - `Entry` owned value with canonical encoder and `to_string`
  - tests: all of test_bdecode, test_bencoding
- **`core::crypto`**
  - SHA-1 (Steve Reid) and SHA-256 (LibTomCrypt): portable ports, plus x86-64 SHA-NI paths selected at runtime
  - CRC32C: SSE4.2 plus table, one-shot and incremental
  - tests: RFC and NIST vectors through both SHA paths, random cross-checks, and the 1 GiB vector (release-only)
- **Digest arithmetic on `Sha1Hash` / `Sha256Hash`** (xor/and/or/not, shifts, leading zeros), with test_sha1_hash.
- **`core::util`**
  - UTF-8 codepoints and `verify_encoding` (test_utf8)
  - string helpers, escaping, base32, `maybe_url_encode`, `parse_url_components` (test_string parts)
  - path helpers; a non-crypto RNG
- **`core::metainfo`**
  - `FileStorage`: compact entries, names borrowed from the info section, path index, `map_block`/`map_file`, symlink sanitising, canonicalize, v2 piece math (test_file_storage)
  - `TorrentInfo`: v1 plus v2/hybrid parsing, piece layers, name sanitising, duplicate-name resolution, trackers, web seeds, DHT nodes (test_torrent_info, all 103 fixtures)
  - magnet parse/build (test_magnet)
  - `MetainfoError` with libtorrent's messages
- **`tests/robustness.rs`**: deterministic mutation testing of bdecode, TorrentInfo and magnets on stable Rust (about 15k corrupted torrents and 5k magnets per run).
- **`lt-swarm gen` / `lt-swarm parse`**: synthetic `.torrent` corpus and metadata parse/memory benchmark. CI runs it and reports it in the job summary, plus the release-only slow tests.

### Changed
- The simulated engine parses `.torrent` files and magnets with the real parsers and shows real file trees.
- `Error::InvalidTorrentFile` / `InvalidMagnet` were replaced by `Error::Metainfo(MetainfoError)`.
- `.gitattributes`: `*.torrent binary`, so fixtures are never line-ending converted.

### Fixed
- **Latent libtorrent bug.** After a parse error, the containers that bdecode closes while unwinding linked to their own end token. `has_soft_error` then read keys out of bounds; the mutation test caught it on its first run. Unwound containers now link past their end token, which also keeps the partial tree structurally valid. A regression test is added.
- **Hybrid torrents without v1 piece hashes.** Our mutation tests also showed that a `meta version 2` torrent with a v1 file list but no `pieces` was accepted. It claimed a v1 info-hash it could never verify. It is now rejected with `torrent_missing_pieces` (libtorrent accepts it); the divergence is recorded in PORTING.md and covered by a unit test.

### Performance (release build, this machine)
- SHA-1 with SHA-NI: 1 GiB in 0.37 s (about 2.9 GB/s).
- Metadata parsing, 10,000 synthetic torrents (211 MB, 134k files): 207 ms, about 21 µs each. Skipping libtorrent's always-computed SHA-256 for v1 torrents brought this down from 308 ms.
- A fully parsed torrent holds about 25 KB (243 MB for 10,000 held, 7 MB once dropped). This confirms the lazy-metadata design (D5).

### Decisions and divergences
- Recorded in docs/PORTING.md: `/` path separator everywhere, iterator-based bdecode lookups, valid partial trees, no SHA-256 for v1, hashed directory index, strict btih hex.
- libtorrent's tests feed C strings, so their embedded NULs end the input early. Ours see every byte; NUL bytes are dropped, as libtorrent's own filter does.
- `TorrentInfo::piece_layer()` returns the root hash for single-piece v2 files, as libtorrent does.

### Issues encountered
- `gen` is a reserved keyword in edition 2024; the corpus generator function is named `generate` (the CLI command is still `gen`).
- Shell heredocs mangled inline Python escapes a few times; edit scripts now go through files.
- Coverage-guided fuzzing (cargo-fuzz) needs a nightly toolchain that is not installed. Asked the user (PLAN.md Q9) rather than working around it.

### Verified
- Windows: `cargo clippy --workspace --all-targets -D warnings` clean, `cargo fmt --check` clean.
- `cargo test --workspace`: 204 tests pass (194 core unit, 3 robustness, 7 frontend), plus 1 release-only test.
- Long local mutation run: 3 seeds x 4000 rounds x 103 fixtures (about 1.2 million corrupted torrents) in release mode. No panics, no invariant violations. Longer runs use `LT_FUZZ_ROUNDS` / `LT_FUZZ_SEED`.

## 2026-10-15_104714_1792054034: Phase 0, foundations

### Added
- **Cargo workspace** (edition 2024, MSRV 1.85):
  - crates: `core`, `frontend`, `win32`, `nox`, `tools/swarm`
  - release profile: fat LTO, 1 codegen unit, `panic=abort`, stripped
  - `profiling` profile
  - static CRT via `.cargo/config.toml`
  - shared lints: `unsafe_op_in_unsafe_fn = deny`, clippy `all`
- **`third_party/libtorrent`**: a shallow git submodule pinned to tag `v2.0.11`, reference only and never built.
- **`THIRD-PARTY-NOTICES.md`**: libtorrent BSD-3, puff (zlib), ed25519 (zlib), SHA-1/SHA-256 (public domain), and the table of linked Rust crates.
- **`libretorrent-core` engine API**, with libtorrent's names and semantics:
  - `Session` / `SessionHandle` / `TorrentHandle` (command channel plus waker)
  - `Alert` with libtorrent priorities, and an `AlertQueue` (bounded; high priority gets 2× the limit, critical 3×; drop counting; empty→non-empty notify)
  - `TorrentStatus`, `PeerInfo`, `AnnounceEntry`, `FileEntry`, `SessionStats`
  - `Settings` (libtorrent 2.0.11 defaults)
  - `AddTorrentParams`, `TorrentFlags` / `RemoveFlags` / `DownloadPriority`
  - `InfoHashes` / `Sha1Hash` / `Sha256Hash`, generational `TorrentId`
  - minimal `Bitfield` and hex helpers
  - a dependency-free `bit_flags!` macro
- **Simulated engine** (`mock` feature, on by default until Phase 2):
  - queueing and auto-management limits
  - magnets that fetch metadata
  - out-of-order piece completion
  - global and per-torrent rate limits
  - forced recheck
  - dirty-only status updates
  - details generated lazily per torrent
  - synthetic libraries up to 100k+ torrents
- **`libretorrent-frontend`**: µTorrent-style formatting (size, speed, ETA, ratio, percent, availability), 2.2.1 status strings ("[F] Downloading", "Queued Seed", "Finished", …), category filters, and Start / Force Start / Stop on top of libtorrent flags.
- **`libretorrent` (Win32) skeleton**:
  - manifest embedded through MSVC linker args (ComCtl v6, PerMonitorV2, long paths, UTF-8 code page) with no resource compiler
  - engine alerts delivered to the GUI thread via `PostMessageW`
  - DPI-aware message font and a status bar showing DHT / D / U
  - `--demo N` loads synthetic torrents
- **`libretorrent-nox` Phase 0 CLI**: `add <magnet|file>` and `demo --torrents N`, with alert notify over a channel. Arguments are parsed by hand (no dependencies).
- **`lt-swarm scale`**: load time, first full update, steady update size and latency, RSS (Windows and Linux), shutdown time; `key=value` output for CI.
- **CI**:
  - `ci.yml`: rustfmt; clippy (`-D warnings`) and tests on Windows and Linux; MSRV check; Windows release build with exe-size, linked-crate and scale report in the job summary; Windows artifacts
  - `freebsd.yml`: FreeBSD VM tests on main, weekly, and on demand

### Decisions
- Q3–Q7 resolved as recommended (PLAN.md D9–D13): OS-native TLS, a qBittorrent WebAPI v2 subset for nox, GTK 4, Windows 10+ x64 on stable Rust, strict 2.2.1 look (dark mode optional later, never the default).
- Peer-ID prefix is provisionally `-LB0100-`. `LR` already belongs to the Android LibreTorrent in libtorrent's `identify_client.cpp`, which ties Q8 to the naming question (Q2).
- `core::mock` re-runs the queue logic once per command batch, not once per command, so bulk adds stay O(n).

### Verified
- `cargo clippy --workspace --all-targets -D warnings`: clean. `cargo test --workspace`: 25 tests pass (18 core, 7 frontend).
- Release build:
  - `libretorrent.exe` is 392 KB and `libretorrent-nox.exe` 455 KB
  - imports are OS DLLs only (no VC++ runtime)
  - the GUI starts and stays up with 2,000 demo torrents (17 MB working set)
- `lt-swarm scale` on the simulated engine:
  - 10k torrents: first full update in 11 ms, ~220 changed per second afterwards, RSS 16 MB loaded / 12 MB steady
  - 100k torrents: 104 ms, 122 / 73 MB

### Issues encountered
- A first draft of the queue test asserted on the very first status update, which comes before auto-management has run for that batch. The test now waits for the queue to settle; updates are eventually consistent, as in libtorrent.
- A visual check of the Win32 window was not possible: a fullscreen application was in the foreground, and the screen capture got that instead. The capture was deleted immediately and not retried.
- First GitHub run of a8bde92 is green on all jobs:
  - CI: fmt, test (Windows), test (Linux), MSRV 1.85, release-windows with its artifact
  - FreeBSD VM tests: 1m42s
- Follow-up: bumped `actions/checkout` and `actions/upload-artifact` from v4 to v7. v4 runs on the deprecated Node 20 runtime, which GitHub flagged.

## 2026-10-15_100350_1792051430: Planning baseline

### Added
- `PLAN.md`: vision, proposed decisions D1–D8, phased roadmap (Phases 0–8), open questions Q1–Q8.
- `docs/ARCHITECTURE.md`:
  - engine/frontend split and thread model
  - sans-IO protocol cores; slab-id ownership
  - scale design for 10k+ torrents, performance targets
  - platform layer, dependency policy, persistence format
- `docs/PORTING.md`:
  - porting rules and attribution requirements
  - libtorrent 2.0.11 → Rust module map with line counts and phases
  - list of what is not ported
  - test-porting map; proposed deliberate divergences
- `README.md`, `.gitignore`, `.gitattributes`.

### Decisions (proposed, awaiting confirmation)
- The engine is a **Rust port** of libtorrent-rasterbar 2.0.11, used as reference code only. It is not linked and not called over FFI.
- One mio-based network thread with sans-IO protocol state machines, a disk thread pool and a resolver thread, mirroring libtorrent's own model.
- Disk I/O uses pread/pwrite plus our own write cache instead of libtorrent 2.0's mmap backend.
- Persistence is a single resume DB plus a metadata pack instead of per-torrent files; info dicts are loaded lazily, for loadability.
- Frontends: Win32 GUI first, then headless `nox` (qbittorrent-nox equivalent), then a GTK GUI for Linux/BSD.

### Findings (libtorrent 2.0.11 survey)
- Size: 165 `.cpp` files, ~105k lines of source, ~55k lines of headers, ~50k lines of tests, 103 test torrents. The largest units are `torrent.cpp` (12.3k), `session_impl.cpp` (7.4k), `peer_connection.cpp` (6.9k), `piece_picker.cpp` (4.0k) and `bt_peer_connection.cpp` (3.8k).
- The default disk backend is mmap. The alternative `posix_disk_io` performs file I/O synchronously on the network thread. Neither fits our scale goals, so we port the job/fence/thread-pool structure with pread I/O.
- `session_impl` keeps per-state torrent lists (`torrent_want_tick`, …), so per-tick work scales with active torrents. We keep this design.
- License: BSD-3-Clause. Bundled sha1 (public domain), sha256 (public domain, LibTomCrypt), ed25519 (zlib, Orson Peters) and puff (zlib-style, Mark Adler) need attribution once ported.

### Issues encountered
- **Wrong first approach (reverted).** I initially misread the goal and began linking libtorrent:
  - built libtorrent 2.0.11 + Boost 1.91 + OpenSSL with Visual Studio 2026's bundled vcpkg (static, release-only overlay triplet, about 4.4 minutes)
  - started a C ABI bridge crate

  The user clarified that libtorrent is to be *ported*, not linked. All of that scaffolding was deleted before the first commit, including `vcpkg_installed/` (4.2 GB). The shared vcpkg cache (about 2 GB under `%LOCALAPPDATA%\vcpkg`) was left in place for the user to delete if unused.
- Tooling note: VS 2026's `Launch-VsDevShell.ps1` prints a harmless "vswhere.exe is not recognized" warning unless the VS Installer directory is on `PATH`. A pure-Rust build does not need the dev shell, since rustc locates the MSVC linker itself.
