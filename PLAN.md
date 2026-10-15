# LibreTorrent plan

Last updated: 2026-10-15_143444_1792067684

## Vision

A libre, lean re-creation of µTorrent 2.2.1:

- native look and feel
- small single-file binaries
- thousands of torrents loaded with negligible CPU and RAM

The BitTorrent engine is our own Rust port of **libtorrent-rasterbar 2.0.11**, used as the reference and never linked. One engine serves three frontends:

| Binary | Platform | Role |
|---|---|---|
| `libretorrent` | Windows | Win32 GUI (first) |
| `libretorrent-nox` | all | headless daemon with HTTP API and Web UI, like qbittorrent-nox |
| `libretorrent-gtk` | Linux / BSD | GTK GUI |

Where it sits: qBittorrent's modular libtorrent-plus-frontend split, with µTorrent's footprint and loadability.

Details: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (design and scale strategy) and [docs/PORTING.md](docs/PORTING.md) (libtorrent → Rust module map and porting rules).

## Current status

- **Stage:** Phase 2 (minimum viable engine) is built. It downloads and seeds real torrents from `libretorrent-nox`. All four exit criteria are met: qBittorrent 5.2.4, Transmission 4.1.0 and Deluge 2.2.0 each downloaded a test torrent from `libretorrent-nox` with identical files. Remaining Phase 2 follow-ups are in Next steps.
- Phase 2 results (release build, this dev machine):

  | What | Result |
  |---|---|
  | Debian 13.7.0 netinst ISO (792 MB) from `bttracker.debian.org` | downloaded in 7–9 s (peaks around 90–140 MB/s from 30–40 peers); SHA-512 identical to Debian's published sum |
  | Restart with `libretorrent-nox run` | restored as a seed after resume verification (file sizes), no hashing |
  | Cold start with saved torrents (`lt-swarm load`) | 1,000 in 12 ms; 10,000 in 109 ms at 45 MB RSS (store: 7.8 MB resume.db + 215 MB metadata pack, read lazily) |
  | Loopback transfer, one seed and one downloader in one process (`lt-swarm loopback`, 512 MiB) | 0.47 s, about 1.15 GB/s (9 Gbit/s), 2.5 ns of CPU per byte (both sides, hashing included) |
  | Seed check of 512 MiB | 0.30 s |

  Exe sizes: `libretorrent-nox.exe` 1.16 MB, `libretorrent.exe` 530 KB (it links the engine's dependencies now), `lt-swarm.exe` 1.16 MB.
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
| D9 (Q3) | TLS is OS-native: SChannel on Windows, system OpenSSL / LibreSSL on Unix, behind a thin trait. | Almost no added binary size; the OS patches TLS. |
| D10 (Q4) | nox exposes a qBittorrent WebAPI v2-compatible subset (incremental `sync/maindata`). | Works with Sonarr, Radarr, Prowlarr and mobile apps. |
| D11 (Q5) | The Linux/BSD GUI uses GTK 4 via gtk4-rs. | Maintained bindings. Memory can be trimmed with the Cairo renderer if needed. |
| D12 (Q6) | Windows 10+ x64 only, on stable Rust. ARM64 Windows comes later as an extra CI target. No Windows 7/8.1, no 32-bit. | No nightly/`build-std`; tier-1 target. |
| D13 (Q7) | Strict µTorrent 2.2.1 look by default: classic list view, 2.2.1 layout, menus and toolbar order. Allowed modern necessities: per-monitor DPI scaling and long paths. An optional dark mode may come later, never as the default. | Faithful clone first. |

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
- [x] `libretorrent-nox` Phase 0 CLI (`add`, `demo`)

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
- [ ] Moved:
  - coverage-guided fuzz targets: need nightly + cargo-fuzz (Q9)
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
  - [ ] HTTPS: needs the OS TLS layer (D9), see Next steps
  - [ ] batched UDP scrapes: the packet format supports 74 hashes; the engine still scrapes one torrent per request
- [x] HTTP client: parser, chunked encoding, redirects. gzip is not needed (we request identity encoding). OS TLS: open.
- [x] `store`: resume.db + metadata.pack, libtorrent fast-resume field names, metadata loaded only for running torrents
- [x] Dev CLI: `libretorrent-nox add <file> --save DIR [--seed]`, `run`, rate limits, per-user state directory, progress lines
- [x] Tests: unit tests for every module (libtorrent vectors for BEP 6 allowed-fast, BEP 40, sliding averages), plus `tests/swarm.rs`: loopback downloads through a direct connection, an HTTP tracker and a UDP tracker, and a restart without recheck
- **Exit criteria:**
  - [x] Downloads a public Linux ISO from its trackers (Debian 13.7.0 netinst, checksum verified).
  - [x] Seeds to qBittorrent, Transmission and Deluge.
    - [x] qBittorrent 5.2.4 (libtorrent 2.0.x): downloaded a 3-file, 38.7 MiB torrent from `libretorrent-nox` over loopback, all files SHA-256 identical, exactly the torrent size downloaded (2026-10-15).
    - [x] Transmission 4.1.0-beta.2 (`transmission-cli`) and Deluge 2.2.0 (`deluged` + `deluge-console`, libtorrent 2.0.x) in the Debian 13 WSL distro, across the WSL NAT to nox on Windows: the same torrent, all files SHA-256 identical; nox sent exactly one copy to each (2026-10-15).
  - [x] Restart resumes without a recheck.
  - [x] 1,000 synthetic torrents load in under 0.5 s (12 ms).

### Phase 3: Win32 GUI MVP (the µTorrent 2.2.1 main window)
- [ ] Main window parts:
  - menu and toolbar (original runtime-drawn icons)
  - category tree with counts
  - virtual torrent list: all 2.2.1 columns, in-cell progress bar, sorting, column chooser, persisted layout
  - detail tabs: General, Trackers, Peers, Pieces, Files, Speed, Logger
  - status bar: DHT nodes, D/U rate and totals, network indicator
  - splitters
- [ ] Adding torrents: Open dialog, drag and drop, command line, magnet/URL dialog. Single instance that forwards arguments.
- [ ] Context menus:
  - Start, Pause, Stop, Force Start
  - Remove, Remove and delete data
  - Force Re-check, Update Tracker
  - queue moves, bandwidth, labels
- [ ] Tray icon, minimize/close to tray, completion balloon
- [ ] Per-monitor DPI v2, ComCtl v6 manifest via linker args (no resource compiler needed)
- [ ] Scale gate: 10,000-torrent profile meets the RAM, startup and idle-CPU targets

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

### Phase 6: Headless `libretorrent-nox`
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

1. Close Phase 2:
   1. Interop, reverse direction: LibreTorrent downloading from qBittorrent, Transmission and Deluge is untested (the clients are installed in the Debian WSL distro now). Add a scripted interop run to `lt-swarm` so this can be repeated.
   2. HTTPS trackers through the OS TLS layer (D9): SChannel on Windows, OpenSSL / LibreSSL on Unix. Ubuntu's and many other public trackers are HTTPS-only.
   3. Save partially downloaded pieces in resume data (`unfinished`), so a restart does not drop them.
2. Phase 3: the µTorrent 2.2.1 main window on top of the real engine.
3. Phase 4 early pick: `ut_metadata`, so magnet links work (they are rejected with a clear message today).
4. Keep CI green on Windows, Linux and FreeBSD (CI now also reports `lt-swarm load` and `loopback`).

## Open questions

- **Q1. License** for LibreTorrent itself. libtorrent's BSD-3 allows any choice: GPL-3.0-or-later (copyleft, like qBittorrent) or MIT / BSD-3 (permissive, like libtorrent).
- **Q2. Name.** "LibreTorrent" is already a well-known GPL-3 Android torrent client (proninyaroslav/libretorrent). libtorrent already identifies peer-ID `LR` as that app, so peers and trackers would confuse the two. Keep the name, or rename before the first release?
- **Q3–Q7.** Resolved, see D9–D13.
- **Q9. Coverage-guided fuzzing.** cargo-fuzz needs a nightly Rust toolchain plus the cargo-fuzz tool, and neither is installed. Install them (`rustup toolchain install nightly` then `cargo install cargo-fuzz`) and add a fuzz CI job? The stable mutation tests cover the basics until then.
- **Q10. Interop testing setup.** Answered: qBittorrent runs as the installed app with a throwaway `--profile`; Transmission and Deluge run in the Debian WSL distro (`transmission-cli`, `deluged`, `deluge-console` installed 2026-10-15; the `deluged` system service the package enabled was stopped and disabled).
- **Q8. Peer-ID prefix (blocked on Q2).** `LR` is taken: libtorrent's `identify_client.cpp` maps it to the Android LibreTorrent. The code uses a provisional `-LB0100-` (`LB` appears in neither libtorrent's table nor the common BEP20 lists). Pick the final two-letter code together with the final name.

## Future improvements

- Split `core` into several crates if compile times demand it.
- Native backends: io_uring disk/network on Linux; IOCP file I/O on Windows.
- Import torrents and state from µTorrent (`resume.dat`) and qBittorrent (`BT_backup`).
- Search and filter box (µTorrent had a web-search box; a local filter is more useful).
- Optional plugin hooks (Rust traits, compiled in) mirroring libtorrent's extension API.

## Known TODOs

- Q1 (license) and Q2 (name, which decides the peer-ID code for Q8) are still open. Both must be settled before the first public release.
- Phase 2 follow-ups: HTTPS trackers (OS TLS), `unfinished` pieces in resume data, batched UDP scrapes, block buffer pool, timer wheel if the tick shows up in profiles, the string-keyed settings table, the full session counter table.
- Peers that only connect and get dropped (seed to seed) are not logged; a `peer_disconnected`-style alert category would help debugging swarms.
- libtorrent tests not ported yet because they need later pieces:
  - `create_torrent` (Phase 5): mutable_torrents, make_magnet_uri v1/v2/hybrid
  - session plus resume data (Phase 2): the write_torrent_file round trips, preserve_* magnet tests
- Windows `SegmentHeap` (manifest `heapType`) might lower RAM use. Benchmark it with `lt-swarm` before enabling.
- CI minutes: the repo is private, and Windows runners bill at 2×. FreeBSD runs only on main and weekly for that reason.
- The vcpkg cache from the abandoned approach (about 2 GB under `%LOCALAPPDATA%\vcpkg`) can be deleted by the user if no other project uses it.
