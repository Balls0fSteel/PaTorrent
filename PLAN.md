# LibreTorrent plan

Last updated: 2026-10-15_104714_1792054034

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

- **Stage:** Phase 0 (foundations) done; Phase 1 (primitives) is next.
- Working today, on the simulated engine:
  - the engine API (`Session`, handles, bounded alert queue, dirty-only status updates)
  - the shared `frontend` crate
  - a Win32 skeleton window
  - the `libretorrent-nox` CLI driver
  - the `lt-swarm scale` benchmark
- Measured on the simulated engine (release build, this dev machine). This covers the API and update path only; the real engine will add its own cost:

  | Torrents loaded | First full status update | RSS loaded / steady | Updates after that |
  |---|---|---|---|
  | 10,000 | 11 ms | 16 / 12 MB | ~220 changed torrents per second |
  | 100,000 | 104 ms | 122 / 73 MB | ~500 changed torrents per second |

  Exe sizes: `libretorrent.exe` 392 KB, `libretorrent-nox.exe` 455 KB. Static CRT; imports only OS DLLs.
- libtorrent 2.0.11 has been surveyed: 165 `.cpp` files, about 105k lines of source, about 55k lines of headers, about 50k lines of tests. Subsystem sizes are in PORTING.md.
- The earlier scaffolding that linked libtorrent via vcpkg and a C ABI bridge was removed after the direction was clarified (see CHANGELOG).

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
- [ ] `bencode`: zero-copy bdecode (token array like `bdecode_node`), encoder, `Entry`
- [ ] `crypto::hash`: SHA-1 and SHA-256 (portable plus SHA-NI, runtime-detected), CRC32C (SSE4.2), OS CSPRNG
- [ ] `util`: hex, percent escaping, UTF-8 and path sanitising, `parse_url`, bitfield, sliding average
- [ ] `metainfo` v1:
  - `file_storage`, `torrent_info` (pad files, BEP47 attributes, symlinks)
  - `info_hash`
  - magnet URIs (BEP9, BEP53 `so=`)
- [ ] Port Phase 1 tests and `test_torrents/` fixtures. Add fuzz targets for bdecode, torrent_info and magnet.

### Phase 2: Minimum viable engine (TCP, v1, trackers)
- [ ] Reactor: mio poll loop, hierarchical timer wheel, waker, resolver thread with cache, TCP listen/connect, UDP sockets
- [ ] Session core:
  - settings (libtorrent names and defaults)
  - bounded alert queue with notify callback
  - stats counters
  - 500 ms session tick
  - per-state torrent lists
  - queueing and auto-management
  - global rate limits
- [ ] Disk:
  - thread pool, job fences, block buffer pool
  - pread storage, write-back cache, incremental piece hashing
  - full check and resume verification
  - LRU file-handle pool, sparse files and preallocation
- [ ] Torrent: state machine (check → download → seed), start/stop/pause/force-start, remove (with or without data), file priorities, sequential mode
- [ ] Peer wire:
  - handshake, BEP3 messages
  - BEP6 fast extension, BEP10 extension protocol
  - request pipelining, end-game, snubbing, timeouts, keep-alives
- [ ] Piece picker (rarest-first, partial pieces, priorities), peer list (BEP40 priorities), choker (rate-based download, round-robin seeding)
- [ ] Trackers:
  - HTTP(S) announce and scrape (BEP3/23/24/48)
  - UDP (BEP15) with connection-id cache and batched scrape
  - tiers (BEP12)
- [ ] HTTP client: parser, chunked encoding, gzip (puff), redirects, OS TLS
- [ ] `store`: resume DB and metadata pack, libtorrent-compatible fastresume fields, lazy info-dict loading
- [ ] Dev CLI (the seed of `nox`): `libretorrent-nox add <file|magnet> --save <dir>` with a progress line
- **Exit criteria:**
  - Downloads a public Linux ISO from its trackers.
  - Seeds to qBittorrent, Transmission and Deluge.
  - Restart resumes without a recheck.
  - 1,000 synthetic torrents load in under 0.5 s.

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

## Next steps (after approval)

1. Phase 1: `bencode` first (everything else depends on it), then SHA-1/SHA-256 (+ SHA-NI), then `metainfo` v1 and magnets. Port the matching libtorrent tests alongside.
2. In parallel with Phase 1/2: build the µTorrent 2.2.1 main window (Phase 3 UI work) against the mock engine.
3. Keep CI green on Windows, Linux and FreeBSD. The first run passed on all three.

## Open questions

- **Q1. License** for LibreTorrent itself. libtorrent's BSD-3 allows any choice: GPL-3.0-or-later (copyleft, like qBittorrent) or MIT / BSD-3 (permissive, like libtorrent).
- **Q2. Name.** "LibreTorrent" is already a well-known GPL-3 Android torrent client (proninyaroslav/libretorrent). libtorrent already identifies peer-ID `LR` as that app, so peers and trackers would confuse the two. Keep the name, or rename before the first release?
- **Q3–Q7.** Resolved, see D9–D13.
- **Q8. Peer-ID prefix (blocked on Q2).** `LR` is taken: libtorrent's `identify_client.cpp` maps it to the Android LibreTorrent. The code uses a provisional `-LB0100-` (`LB` appears in neither libtorrent's table nor the common BEP20 lists). Pick the final two-letter code together with the final name.

## Future improvements

- Split `core` into several crates if compile times demand it.
- Native backends: io_uring disk/network on Linux; IOCP file I/O on Windows.
- Import torrents and state from µTorrent (`resume.dat`) and qBittorrent (`BT_backup`).
- Search and filter box (µTorrent had a web-search box; a local filter is more useful).
- Optional plugin hooks (Rust traits, compiled in) mirroring libtorrent's extension API.

## Known TODOs

- Q1 (license) and Q2 (name, which decides the peer-ID code for Q8) are still open. Both must be settled before the first public release.
- `core`'s `mock` feature is on by default; turn it off by default when the real engine lands (Phase 2).
- Windows `SegmentHeap` (manifest `heapType`) might lower RAM use. Benchmark it with `lt-swarm` before enabling.
- CI minutes: the repo is private, and Windows runners bill at 2×. FreeBSD runs only on main and weekly for that reason.
- The vcpkg cache from the abandoned approach (about 2 GB under `%LOCALAPPDATA%\vcpkg`) can be deleted by the user if no other project uses it.
