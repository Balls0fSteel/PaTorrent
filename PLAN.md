# PaTorrent plan

Last updated: 2026-10-15_202147_1792088507

## Vision

A lean re-creation of µTorrent 2.2.1:

- native look and feel
- small single-file binaries
- thousands of torrents loaded with negligible CPU and RAM

The BitTorrent engine is our own Rust port of **libtorrent-rasterbar 2.0.11**, used as the reference and never linked. One engine serves three frontends:

| Binary | Platform | Role |
|---|---|---|
| `patorrent` | Windows | Win32 GUI (first) |
| `patorrent-nox` | all | headless daemon with HTTP API and Web UI, like qbittorrent-nox |
| `patorrent-gtk` | Linux / BSD | GTK GUI |

Where it sits: qBittorrent's modular libtorrent-plus-frontend split, with µTorrent's footprint and loadability.

Details: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (design and scale strategy) and [docs/PORTING.md](docs/PORTING.md) (libtorrent → Rust module map and porting rules).

## Current status

- **Stage:** Phase 3 (Win32 GUI MVP) is in progress. `patorrent.exe` now shows the µTorrent 2.2.1 main window on the real engine: menu, runtime-drawn toolbar, category tree, virtual torrent list, the seven detail tabs, status bar, tray, context menus, single instance and persisted layout. Every Phase 3 item is implemented, and the scale gate passes. The phase stays open until a person has checked the window visually (computer use is blocked by policy for this app, so nothing visual is verified yet) and a download has run through the GUI. Phase 2 (minimum viable engine) is closed; its results follow.
- Phase 3 scale gate (release, 10,000 saved stopped torrents, real engine): all rows listed in 0.2–0.4 s, 60 MB working set, idle CPU 0.1 % of a core. Empty session: 19.9 MB working set, 4.5 MB private (the 12 MB target is not met as a working set; see Known TODOs). `patorrent.exe` is 1.36 MB.
- Phase 2 results (release build, this dev machine):

  | What | Result |
  |---|---|
  | Debian 13.7.0 netinst ISO (792 MB) from `bttracker.debian.org` | downloaded in 7–9 s (peaks around 90–140 MB/s from 30–40 peers); SHA-512 identical to Debian's published sum |
  | Restart with `patorrent-nox run` | restored as a seed after resume verification (file sizes), no hashing |
  | Cold start with saved torrents (`lt-swarm load`) | 1,000 in 12 ms; 10,000 in 109 ms at 45 MB RSS (store: 7.8 MB resume.db + 215 MB metadata pack, read lazily) |
  | Loopback transfer, one seed and one downloader in one process (`lt-swarm loopback`, 512 MiB) | 0.47 s, about 1.15 GB/s (9 Gbit/s), 2.5 ns of CPU per byte (both sides, hashing included) |
  | Seed check of 512 MiB | 0.30 s |

  Exe sizes: `patorrent-nox.exe` 1.16 MB, `patorrent.exe` 530 KB (it links the engine's dependencies now), `lt-swarm.exe` 1.16 MB.
- Earlier phases: the metadata layer (Phase 1: bencode, SHA-1/SHA-256 with SHA-NI, CRC32C, `TorrentInfo`, magnets) and the engine API with the simulated engine (Phase 0), which the Win32 skeleton and `lt-swarm scale` still use.
- Phase 1 measurements: SHA-1 with SHA-NI hashes 1 GiB in 0.37 s; 10,000 synthetic `.torrent` files parse in 207 ms; a fully parsed torrent holds about 25 KB, which is why stopped torrents keep no metadata (D5).
- Simulated-engine scale numbers (Phase 0): 10,000 torrents give a first full status update in 11 ms; 100,000 in 104 ms.

## Decisions

Approved 2026-10-15 (D1–D8 as proposed; D9–D13 resolve Q3–Q7 with the recommended options).

| # | Decision | Rationale |
|---|---|---|
| D1 | The engine is a Rust port of libtorrent 2.0.11 (tag `v2.0.11`, BSD-3). No FFI, nothing linked. | Monolithic, lean, fully ours; libtorrent's proven algorithms and tests. |
| D2 | One network thread on **mio**. Protocols written sans-IO. Disk thread pool. Resolver thread. | libtorrent's own scaling model. mio is the smallest portable event loop (IOCP / epoll / kqueue). |
| D3 | Disk I/O uses pread/pwrite plus our own write-back cache. No mmap. | Avoids libtorrent 2.0's mmap memory and network-drive problems. Scales. |
| D4 | Dependencies: engine = `std` + `mio` + OS TLS; Win32 GUI = `windows-sys` only; everything else ported or written in-house. | Lean and monolithic; full control over size and performance. |
| D5 | Persistence: one resume DB + one metadata pack (like µTorrent's `resume.dat`), not two files per torrent. Lazy-load info dicts. | Startup time and RAM at 10k+ torrents. |
| D6 | Targets: Windows 10+ x64 (Rust's tier-1 baseline), Linux x86_64/aarch64, FreeBSD/OpenBSD. | Rust dropped pre-Windows-10 support from its tier-1 targets in 1.78. |
| D7 | Windows: static CRT, single `.exe`, portable mode (settings beside the exe), like µTorrent. | No installer or runtime needed. |
| D8 | Port libtorrent's tests with each module. Fuzz every parser. Deterministic swarm simulator. | Correctness without libtorrent's simulator. |
| D9 (Q3) | TLS is OS-native: SChannel on Windows, system OpenSSL / LibreSSL on Unix, behind a thin trait. | Almost no added binary size; the OS patches TLS. Built in `core::net::tls` with no new crates: SChannel through more `windows-sys` features, OpenSSL through hand-written declarations of a dozen stable functions, so Unix builds link the system `libssl` / `libcrypto` (Linux needs `libssl-dev` to build; FreeBSD has it in base). |
| D10 (Q4) | nox exposes a qBittorrent WebAPI v2-compatible subset (incremental `sync/maindata`). | Works with Sonarr, Radarr, Prowlarr and mobile apps. |
| D11 (Q5) | The Linux/BSD GUI uses GTK 4 via gtk4-rs. | Maintained bindings. Memory can be trimmed with the Cairo renderer if needed. |
| D12 (Q6) | Windows 10+ x64 only, on stable Rust. ARM64 Windows comes later as an extra CI target. No Windows 7/8.1, no 32-bit. | No nightly/`build-std`; tier-1 target. |
| D13 (Q7) | Strict µTorrent 2.2.1 look by default: classic list view, 2.2.1 layout, menus and toolbar order. Allowed modern necessities: per-monitor DPI scaling and long paths. An optional dark mode may come later, never as the default. | Faithful clone first. |
| D14 (Q9) | Coverage-guided fuzzing with cargo-fuzz on nightly, in a separate `fuzz/` workspace. The internal parsers reach it through the `fuzzing` feature of `patorrent-core` (hidden module `fuzzing`). CI fuzzes each target on Linux: 2 min per push to main that touches the engine, 30 min weekly. | Nightly stays out of the product build (D12 still holds). Linux CI needs no ASan setup. |
| D15 (Q1) | License: **PolyForm Noncommercial 1.0.0**, copyright **DukeCorp1996** (`LICENSE.md` with a `Required Notice:` line, SPDX `PolyForm-Noncommercial-1.0.0` in every manifest). Ported libtorrent code also keeps its BSD-3 notice. `LICENSE.md` ships in `dist\` and the CI artifact, `patorrent-nox --license` prints it, and the About box names it. | The owner wants no commercial use and their own name as author. Accepted consequence: source-available, not OSI open source, so no Debian/Fedora/F-Droid packaging. |
| D16 (Q2, Q8) | Name: **PaTorrent** (long name **PatinaTorrent**; a patina is the good kind of rust). Peer-ID prefix `-PA0100-`, User-Agent `PaTorrent/<version>`, BEP 10 `v` = `PaTorrent <version>`. Crates `patorrent-core` / `-frontend` / `-nox`, binary `patorrent.exe`, `%APPDATA%\PaTorrent`, `patorrent.ini`. `LR` stays mapped to the Android LibreTorrent in our peer table. | "LibreTorrent" was taken (Android, peer ID `LR`) and "libre" is wrong under D15. The owner turned down Ferrite (`Fe` reads badly) and Oxbit. `PA` is free in libtorrent's table and the BEP 20 list. |
| D17 (Q8) | No peer-ID spoofing. PaTorrent always reports itself. | A borrowed ID with this engine's fingerprint gets accounts banned on private trackers and misreports the client. |

## Roadmap

Each phase ends with something runnable and measured against the targets in ARCHITECTURE.md.

### Phase 0: Foundations
- [x] Cargo workspace:
  - crates: `core`, `frontend`, `win32`, `nox`, plus `tools/swarm`
  - edition 2024
  - release profile: LTO, `codegen-units=1`, `panic=abort`, strip
  - static CRT on Windows
- [x] `third_party/libtorrent` submodule pinned to `v2.0.11`, reference only (never built)
- [x] `THIRD-PARTY-NOTICES.md`: libtorrent BSD-3, Steve Reid sha1 (PD), LibTomCrypt sha256 (PD), orlp ed25519 (zlib), puff (zlib)
- [x] CI on GitHub Actions:
  - `ci.yml`: rustfmt; clippy + tests on Windows and Linux; MSRV 1.85 check; Windows release build with exe-size, dependency and scale report
  - `freebsd.yml`: VM test run on main, weekly, and on demand
- [x] Engine public API (`Session`, `SessionHandle`, `TorrentHandle`, `Alert`, `TorrentStatus`, `Settings`, `AddTorrentParams`) with libtorrent semantics:
  - bounded alert queue with priorities and a notify callback
  - async `post_*` queries
  - `mock` engine with queueing, magnets and rate limits that scales to 100k synthetic torrents
- [x] `tools/swarm`: `scale` benchmark (load time, update sizes, RSS). The loopback swarm runner moves to Phase 2 (needs the real engine); the `.torrent` corpus generator moves to Phase 1 (needs bencode).
- [x] Win32 skeleton:
  - manifest embedded via linker args: ComCtl v6, PerMonitorV2, long paths, UTF-8 code page
  - alert notify → `PostMessageW`, DPI-aware font, status bar
- [x] `frontend` crate: µTorrent formatting, status strings and categories, Start / Force Start / Stop actions
- [x] `patorrent-nox` Phase 0 CLI (`add`, `demo`)

### Phase 1: Primitives (pure code, fully tested)
- [x] `bencode`: zero-copy bdecode (8-byte token array like `bdecode_node`), encoder, `Entry`, `print_entry`, soft-error checks
- [x] `crypto`: SHA-1 and SHA-256 (portable ports plus SHA-NI, runtime-detected), CRC32C (SSE4.2 plus table, incremental)
- [x] `util`: hex, percent escaping, base32, UTF-8 codepoints and `verify_encoding`, `parse_url_components`, path helpers, non-crypto RNG
- [x] `metainfo`:
  - `FileStorage` (pad files, BEP47 attributes, symlink sanitising, `map_block` / `map_file`, canonicalize, v2 piece math)
  - `TorrentInfo`: v1, plus v2 / hybrid parsing, piece layers, duplicate-name resolution, web seeds, DHT nodes, similar / collections
  - info-hashes; magnet URIs (BEP9, BEP53 `so=`, `x.pe`, `dht`) with `make_magnet_uri`
- [x] Ported tests:
  - libtorrent tests: test_bdecode, test_bencoding, test_hasher, test_sha1_hash, test_crc32, test_utf8, test_string (relevant parts), test_file_storage, test_torrent_info (non-session parts), test_magnet (non-session parts)
  - all 103 `test_torrents/` fixtures copied
- [x] Robustness: deterministic mutation tests on stable Rust (about 15k corrupted torrents and 5k magnets per run). They found and fixed a latent libtorrent partial-tree bug.
- [x] `lt-swarm gen` / `parse`: synthetic corpus and metadata parse benchmark (also in the CI job summary)
- [x] Coverage-guided fuzz targets (cargo-fuzz, Q9 → D14): `bdecode`, `torrent_info`, `magnet`, `peer_wire`, `tracker_response`, `http_response` in `fuzz/`; `fuzz.bat` locally, `.github/workflows/fuzz.yml` in CI
- [ ] Moved:
  - OS CSPRNG and sliding average: Phase 2, where they are first used
  - ARMv8 SHA paths: later, with ARM64 CI

### Phase 2: Minimum viable engine (TCP, v1, trackers)
- [x] Reactor:
  - one mio poll loop with a waker; command, disk and resolver channels drained per wake-up
  - resolver threads with a 20-minute cache
  - TCP listen (port retry, dual-stack aware) and connect; UDP sockets for trackers
  - [ ] hierarchical timer wheel: not needed so far. Deadlines are checked in the 500 ms tick (as libtorrent's `second_tick` does for peers); revisit if profiles show the tick
- [x] Session core:
  - settings (libtorrent names and defaults for everything the engine reads; the string-keyed table comes with the Advanced preferences, Phase 5)
  - bounded alert queue with notify callback
  - session stats (subset; full counter table later)
  - 500 ms tick over running torrents only
  - queueing and auto-management
  - global and per-torrent rate limits (token buckets, 100 ms refill)
- [x] Disk:
  - thread pool, fences (release, delete), per-piece thread affinity
  - pread/pwrite storage, incremental in-order hashing with a reorder cache, disk-queue back-pressure on peers
  - full check and resume verification
  - LRU file-handle pool, sparse files on Windows
  - [ ] block buffer pool and preallocation: later (plain `Vec` buffers so far; sparse files make preallocation optional)
- [x] Torrent: check → download → seed; start / stop / pause / force start; remove with or without data; file priorities; sequential mode; force recheck
- [x] Peer wire:
  - handshake, BEP 3 messages
  - BEP 6 fast extension (have all / none, reject, allowed fast both ways), BEP 10 extension handshake
  - request pipelining sized by rate, end-game with cancels, snubbing, timeouts, keep-alives
- [x] Piece picker (rarest-first, priorities, partial pieces first, sequential), peer list (BEP 40 ranks, fail counts, eviction), choker (fixed slots, rate-based, optimistic unchoke)
- [x] Trackers:
  - HTTP announce and scrape (BEP 3/23/24/48), redirects, chunked bodies
  - UDP (BEP 15) with connection-id cache and retransmits
  - tiers (BEP 12), backoff, `stopped` / `completed` events
  - [x] HTTPS through the OS TLS layer (D9, `core::net::tls`): SChannel (TLS 1.2 and 1.3) on Windows, OpenSSL / LibreSSL on Unix; certificate and host-name checks on, SNI. Verified against Ubuntu's IPv4 and IPv6 HTTPS trackers
  - [ ] batched UDP scrapes: the packet format supports 74 hashes; the engine still scrapes one torrent per request
- [x] HTTP client: parser, chunked encoding, redirects. gzip is not needed (we request identity encoding). OS TLS: done (`core::net::tls`).
- [x] `store`: resume.db + metadata.pack, libtorrent fast-resume field names, metadata loaded only for running torrents
- [x] Dev CLI: `patorrent-nox add <file> --save DIR [--seed]`, `run`, rate limits, per-user state directory, progress lines
- [x] Tests: unit tests for every module (libtorrent vectors for BEP 6 allowed-fast, BEP 40, sliding averages), plus `tests/swarm.rs`: loopback downloads through a direct connection, an HTTP tracker and a UDP tracker, a restart without recheck, and a restart that keeps partly downloaded pieces
- **Exit criteria:**
  - [x] Downloads a public Linux ISO from its trackers (Debian 13.7.0 netinst, checksum verified).
  - [x] Seeds to qBittorrent, Transmission and Deluge.
    - [x] qBittorrent 5.2.4 (libtorrent 2.0.x): downloaded a 3-file, 38.7 MiB torrent from `patorrent-nox` over loopback, all files SHA-256 identical, exactly the torrent size downloaded (2026-10-15).
    - [x] Transmission 4.1.0-beta.2 (`transmission-cli`) and Deluge 2.2.0 (`deluged` + `deluge-console`, libtorrent 2.0.x) in the Debian 13 WSL distro, across the WSL NAT to nox on Windows: the same torrent, all files SHA-256 identical; nox sent exactly one copy to each (2026-10-15).
  - [x] Downloads from qBittorrent, Transmission and Deluge (reverse direction): `lt-swarm interop` (2026-10-15). 40 MiB, 3 files, files byte-identical, no failed or redundant bytes from any of them. qBittorrent 5.2.4 in 0.2 s and Deluge 2.2.0 in 0.4 s; Transmission 4.1.0-beta.2 in 9.7 s, of which 9.6 s is waiting for its periodic (10 s) rechoke to unchoke us.
  - [x] Restart resumes without a recheck, including partly downloaded pieces (libtorrent `unfinished`).
  - [x] 1,000 synthetic torrents load in under 0.5 s (12 ms).

### Phase 3: Win32 GUI MVP (the µTorrent 2.2.1 main window)
Implemented 2026-10-15 (CHANGELOG 2026-10-15_172326). An item is ticked only once it has also been checked on screen; until then it is marked *built*.
- [ ] Main window parts (*built*):
  - menu and toolbar (original runtime-drawn icons)
  - category tree with counts (labels included)
  - virtual torrent list: 24 columns (the 2.2.1 set as far as known, from memory; check against a real 2.2.1), in-cell progress bar, sorting, column chooser, persisted layout
  - detail tabs: General, Trackers, Peers, Pieces, Files, Speed, Logger
  - status bar: DHT nodes, D/U rate and totals, network indicator (text only so far)
  - splitters
- [ ] Adding torrents (*built*): Open dialog, drag and drop, command line, magnet/URL dialog. Single instance that forwards arguments (verified).
- [ ] Context menus (*built*):
  - Start, Pause, Stop, Force Start
  - Remove, Remove and delete data
  - Force Re-check, Update Tracker
  - queue moves, bandwidth, labels
- [ ] Tray icon, minimize/close to tray, completion balloon (*built*)
- [x] Per-monitor DPI v2, ComCtl v6 manifest via linker args (no resource compiler needed). `WM_DPICHANGED` rescales fonts, icons and columns (not yet tried on a second monitor).
- [x] Scale gate: 10,000-torrent profile meets the startup (0.2–0.4 s), RAM (60 MB) and idle-CPU (0.1 %) targets.
- [ ] Visual check by a person, then a real download through the GUI.

### Phase 4: Discovery and connectivity
- [ ] DHT: BEP5, BEP32 (IPv6), BEP42 (node id security), BEP43 (read-only); bootstrap; state persistence
- [ ] Extensions: ut_metadata (magnets), ut_pex, lt_donthave (BEP54), smart-ban
- [ ] LSD (BEP14); UPnP IGD and NAT-PMP port mapping
- [ ] MSE/PE encryption with µTorrent-style policies (disabled / enabled / forced, allow legacy)
- [ ] uTP (BEP29) with LEDBAT, and µTorrent-2.x-like TCP/uTP preference
- [ ] IPv6 throughout; interface enumeration, network-change handling, external-IP voting

### Phase 5: µTorrent 2.2.1 feature parity
- [ ] Preferences, with all 2.2.1 pages: General, UI Settings, Directories, Connection, Bandwidth, BitTorrent, Queueing, Scheduler, Web UI, Advanced (raw settings table), Disk Cache
- [ ] Add-torrent dialog: save folder, file tree with priorities, label, skip hash check, start/stop
- [ ] Create New Torrent: piece-size auto, trackers, web seeds, private flag, progress
- [ ] Labels; RSS feeds and RSS Downloader (filters, episode tracking); weekly scheduler; per-torrent bandwidth allocation and limits; auto-shutdown; move-on-complete; auto-load folder; `.torrent` and `magnet:` association
- [ ] Speed graph, pieces map and list, availability bar
- [ ] Network features:
  - IP filter (`ipfilter.dat`)
  - proxies (SOCKS4/5, HTTP)
  - web seeds (BEP17/19)
  - super-seeding
  - part files for skipped files
  - move storage
- [ ] Localisation infrastructure (string tables)

### Phase 6: Headless `patorrent-nox`
- [ ] Daemon: Unix daemon / systemd unit, optional Windows service, signal handling, XDG paths
- [ ] Embedded HTTP/1.1 server on the core reactor; optional TLS; authentication, CSRF and host-header protections
- [ ] API (see Q4) with incremental diff updates
- [ ] Small dependency-free Web UI, embedded in the binary
- [ ] Optional: either GUI can remote-control a nox instance

### Phase 7: GTK GUI (Linux / BSD)
- [ ] Same layout and the same `frontend` model; virtualized `ColumnView`; tray via StatusNotifierItem
- [ ] Packaging: tarball and AppImage first; Flatpak, distro packages and a FreeBSD port later

### Phase 8: Modern protocol extensions
- [ ] BitTorrent v2 and hybrid torrents (BEP52): merkle trees, hash picker, v2 magnets
- [ ] BEP44 DHT items (ed25519), BEP51 sample_infohashes, BEP55 holepunch

### Continuous
- Scale and performance benchmarks in CI, with regressions failing the build once baselines exist
- Fuzzing; an interop matrix against qBittorrent, Transmission, Deluge, rTorrent and µTorrent 2.2.1
- Parser security review before each release

## Next steps

1. Phase 3: a person checks the main window on screen (icons, Done bars, General/Pieces/Speed panels, menus, dialogs, drag and drop, tray, a DPI change) and downloads a torrent through it. Fix what that turns up, then close the phase.
2. Phase 4 early pick: `ut_metadata`, so magnet links work (they are rejected with a clear message today; the GUI shows that message).
3. Keep CI green on Windows, Linux and FreeBSD.
4. Rerun `lt-swarm interop` after engine changes that touch the peer wire, the picker or the disk. It needs the local client setup in Q10, so it does not run in CI.

Phase 2 closing items, done 2026-10-15: OpenSSL TLS backend linked and tested on Linux (CI run 38738279981) and FreeBSD (run 38736854046); partial pieces in resume data (CHANGELOG 2026-10-15_160110); reverse-direction interop scripted as `lt-swarm interop` (CHANGELOG 2026-10-15_161111).

## Open questions

- **Q1, Q2, Q8, Q9.** Resolved, see D14–D17.
- **Q3–Q7.** Resolved, see D9–D13.
- **Q11. GitHub repository name.** Resolved 2026-10-15: renamed to `Balls0fSteel/PaTorrent` (GitHub redirects the old URL); Cargo.toml, the README and the `origin` remote point there. The local folder is still `libretorrent`; renaming it is optional and best done with no editor or session open in it.
- **Q10. Interop testing setup.** Answered: qBittorrent runs as the installed app with a throwaway `--profile`; Transmission and Deluge run in the Debian WSL distro (`transmission-cli`, `deluged`, `deluge-console` installed 2026-10-15; the `deluged` system service the package enabled was stopped and disabled). `lt-swarm interop` scripts all three.

## Future improvements

- Split `core` into several crates if compile times demand it.
- Native backends: io_uring disk/network on Linux; IOCP file I/O on Windows.
- Import torrents and state from µTorrent (`resume.dat`) and qBittorrent (`BT_backup`).
- Search and filter box (µTorrent had a web-search box; a local filter is more useful).
- Optional plugin hooks (Rust traits, compiled in) mirroring libtorrent's extension API.

## Known TODOs

- Getting `PA` recognized (D16, D17), at the first public release:
  - a PR to libtorrent adding `{"PA", "PaTorrent"}` to `src/identify_client.cpp`, so qBittorrent, Deluge and other libtorrent clients show the name in their peer lists
  - the same for webtorrent's `bittorrent-peerid` and Transmission's `clients.cc`, and an entry on the wiki.theory.org BEP 20 list
  - for private trackers: behave well (honest upload/download stats, announce intervals respected, no ratio tricks), keep `User-Agent`, peer ID and BEP 10 `v` consistent across versions, and publish a stable release with changelog. Tracker staff usually want a tagged release they can test. Expect a long wait, and some will never whitelist a new client.
- Peer-ID version characters: `-PA0100-` encodes 0.1.0. Bump `PEER_ID_PREFIX` together with the crate version (a test enforces it).
- Fuzzing on Windows needs Visual Studio's AddressSanitizer runtime (`clang_rt.asan_dynamic-x86_64.dll`); `fuzz.bat` finds it with vswhere. A build without a sanitizer (`-s none`) does not link on MSVC (libFuzzer's PC table uses ELF section symbols). The default toolchain here is pinned to 1.90.0, and cargo-fuzz 0.13 needs 1.91+, so it was installed with `cargo +stable install cargo-fuzz --locked`. In CI, cargo-fuzz from `taiki-e/install-action` is a musl build that defaults to the musl target, so `fuzz.yml` passes `--target x86_64-unknown-linux-gnu`.
- Fuzz targets still to add as the code lands: UDP tracker responses (no public parser yet), DHT messages, uTP headers, `ut_metadata`, resume data and the nox HTTP API (docs/PORTING.md rule 8).
- Licensing compliance (checked 2026-10-15_174600_1792079160):
  - when a new libtorrent module (or bundled code such as puff or ed25519) is ported, add it to THIRD-PARTY-NOTICES.md's "Ported so far" list and give each file the origin and copyright header.
  - the nox Web API (D10) must be written from qBittorrent's published WebAPI docs only. qBittorrent is GPL-2.0-or-later, which is incompatible with D15's noncommercial license: never copy its source or Web UI assets.
  - the Win32 GUI's About box points to THIRD-PARTY-NOTICES.md but does not show the texts. A Help > Third-Party Notices window would keep them with a GUI exe copied out on its own (nox has `--license`).
- Win32 GUI (Phase 3 follow-ups):
  - empty-session working set is 19.9 MB against the 12 MB target (private bytes 4.5 MB). Most of it is shared system DLLs; loading urlmon and parts of shell32 only when needed may help.
  - the status bar's network indicator is text; µTorrent shows a colored icon.
  - the Pieces tab draws a piece map; µTorrent 2.2.1 lists the pieces in progress (Phase 5 has "pieces map and list").
  - restored torrents each log "added" at startup (10,000 lines with a big library); µTorrent does not log restores.
  - Pause does not toggle back to running as µTorrent's does; Start resumes.
  - double-click opens the folder for finished torrents and the General tab otherwise; the Properties dialog comes with Phase 5.
  - Create New Torrent and Preferences are greyed out until Phase 5.
- Phase 2 follow-ups: batched UDP scrapes, block buffer pool, timer wheel if the tick shows up in profiles, the string-keyed settings table, the full session counter table.
- ~~Flaky on CI: `swarm::download_through_an_http_tracker`~~. Fixed 2026-10-15: the test's `wait()` dropped the alerts behind a match, so `TorrentFinished` was lost when it arrived in the same batch as `TrackerReply`. Not an engine stall (CHANGELOG 2026-10-15_153331).
- TLS (`core::net::tls`):
  - certificate revocation is not checked; SChannel would fetch CRLs synchronously on the engine thread. SChannel may still fetch a missing intermediate certificate (AIA) synchronously; move the handshake off the reactor if that ever shows up as a stall.
  - no macOS backend (Secure Transport / Network.framework) yet; non-Windows, non-Unix targets fail HTTPS announces with a clear message.
  - web seeds over HTTPS (Phase 5) can reuse `TlsStream` as is.
- `lt-swarm interop` is verified only on Windows (the qBittorrent app plus WSL). On Linux it runs `transmission-cli` and `deluged` directly and expects `qbittorrent-nox` via `--qbittorrent`; untested. rTorrent and µTorrent 2.2.1 are not in it yet (Continuous: interop matrix).
- Peers that only connect and get dropped (seed to seed) are not logged; a `peer_disconnected`-style alert category would help debugging swarms. `remove_peer` already has the close reason and throws it away; connect failures in `connect_peers_for` are not reported either.
- libtorrent tests not ported yet because they need later pieces:
  - `create_torrent` (Phase 5): mutable_torrents, make_magnet_uri v1/v2/hybrid
  - session plus resume data (Phase 2): the write_torrent_file round trips, preserve_* magnet tests
- Windows `SegmentHeap` (manifest `heapType`) might lower RAM use. Benchmark it with `lt-swarm` before enabling.
- CI minutes: the repo is private, and Windows runners bill at 2×. FreeBSD runs only on main and weekly for that reason.
- The vcpkg cache from the abandoned approach (about 2 GB under `%LOCALAPPDATA%\vcpkg`) can be deleted by the user if no other project uses it.
