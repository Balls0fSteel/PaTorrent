# PaTorrent plan

Last updated: 2026-10-17_203617_1792262177

## Vision

A lean re-creation of µTorrent 2.2.1:

- native look and feel
- small single-file binaries
- thousands of torrents loaded with negligible CPU and RAM

The BitTorrent engine is our own Rust port of **libtorrent-rasterbar 2.0.11**, used as the reference and never linked. One engine serves three frontends:

| Binary | Platform | Role |
|---|---|---|
| `patorrent` | Windows | Win32 GUI (first) |
| `patorrent-nox` | all | headless daemon with a native push API and Web UI ([docs/WEBUI.md](docs/WEBUI.md)), plus a qBittorrent WebAPI shim for Sonarr / Radarr and the mobile remotes |
| `patorrent-gtk` | Linux / BSD | GTK GUI |

Where it sits: qBittorrent's modular libtorrent-plus-frontend split, with µTorrent's footprint and loadability.

Details: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) (design and scale strategy), [docs/PORTING.md](docs/PORTING.md) (libtorrent → Rust module map and porting rules) and [docs/WEBUI.md](docs/WEBUI.md) (the Web UI and nox's native API).

## Current status

- **PatinaTorrent icon candidates (2026-10-15_203223_1792089143):** three original teal/copper application directions (Verdigris, Exchange, Foundry), transparent PNGs and multi-size Windows ICOs, plus 30 editable state SVGs with PNG exports, exact state mapping and an interactive review gallery in `assets/patina-icons/`. Completed transparency/rendering, ICO frame, state-coverage and desktop/mobile gallery checks. Next: choose a brand direction, then integrate its app/tray icon and the state family into the runtime-drawn Win32 UI. The candidates are not yet connected to the application.

- **Stage:** Phase 3 (Win32 GUI MVP) is closed: the owner ran `patorrent.exe`, found the window fine, added a torrent and it downloaded (2026-10-15). Phase 4 has started: magnet links work (`ut_metadata`, BEP 9; CHANGELOG 2026-10-15_204502_1792089902), and the mainline DHT (BEP 5/32/42/43, `core::dht`; CHANGELOG 2026-10-15_211826_1792091906) finds peers for magnets without trackers: the Debian 13.7.0 netinst magnet with only `xt` bootstrapped from `dht.libtorrent.org` in 2 s, had DHT peers at 4 s, its metadata at 5 s and the whole 792 MB at 13 s. Peer exchange (`ut_pex`, BEP 11; CHANGELOG 2026-10-15_221714_1792095434) brought about 70 candidate peers within 10 s of the Debian magnet's first connections. Local service discovery (BEP 14; CHANGELOG 2026-10-15_234806_1792100886) finds peers on the local network: two `patorrent-nox` processes on one machine found each other on all 14 of its interfaces and kept a single connection. Port mapping (UPnP IGD and NAT-PMP / PCP; CHANGELOG 2026-10-16_155608_1792158968) forwards the listen port through the router: on this network (a Technicolor gateway, WANIPConnection:2) NAT-PMP had TCP and UDP 6881 mapped and the public address within a second of start, UPnP within two; the router listed both mappings as `PaTorrent/0.1.0` while `patorrent-nox` ran and no longer after it exited. MSE/PE protocol encryption (CHANGELOG 2026-10-16_163419_1792161259) makes and accepts encrypted connections with libtorrent's policies: the loopback policy matrix passes (forced / enabled / disabled on either side, RC4 and plaintext streams, the plaintext-first-then-encrypted retry), and `lt-swarm interop --encryption forced` downloaded from qBittorrent 5.2.4 over RC4. uTP (BEP 29, CHANGELOG 2026-10-16_220720_1792181240) carries peer connections over the UDP listen socket with LEDBAT congestion control: outgoing connections try uTP first and fall back to TCP (libtorrent's policy), incoming SYNs are accepted, and the loopback suite downloads over uTP, uTP-only, encrypted over uTP, and through the fallback. lt_donthave (BEP 54) and smart-ban (CHANGELOG 2026-10-16_225433_1792184073) close the Phase 4 extensions: peers can take announced pieces back, and a peer that slipped two bad blocks into a piece the seed otherwise supplied is banned once the piece passes, with the seed kept. Session-wide external-IP voting, Create New Torrent (v1, engine and `patorrent-nox create`) and the IP / port filter followed (CHANGELOG 2026-10-17_114537_1792230337), then web seeds (BEP 19 / 17), network-change handling with `listen_interfaces` expansion (closing Phase 4) and the Win32 Create New Torrent dialog (CHANGELOG 2026-10-17_140012_1792238412). Phase 5 has started: the Win32 Preferences dialog, proxies (SOCKS4/5, HTTP), super-seeding and part files were built in parallel and merged (CHANGELOG 2026-10-17_161604_1792246564), then move storage and the Win32 Add New Torrent dialog (CHANGELOG 2026-10-17_165701_1792249021). Phase 6 has started: `patorrent-nox run --webui [ADDR:]PORT` serves a qBittorrent WebAPI v2 subset (login with a salted PBKDF2 hash, ban, CSRF and Host checks, torrents/info, add, stop / start, delete, files, trackers, categories, preferences, incremental `sync/maindata`) and a small Web UI page (CHANGELOG 2026-10-17_164324_1792248204); the native API (D18) has its WebSocket and session protocol (CHANGELOG 2026-10-17_210452_1792263892). Phase 2 (minimum viable engine) is closed too; its results follow.
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
| D10 (Q4) | nox exposes a qBittorrent WebAPI v2-compatible subset (incremental `sync/maindata`). Since D18 it is a compatibility shim beside the native API, translating from the same cache; it never shapes a native feature. | Works with Sonarr, Radarr, Prowlarr and mobile apps. |
| D11 (Q5) | The Linux/BSD GUI uses GTK 4 via gtk4-rs. | Maintained bindings. Memory can be trimmed with the Cairo renderer if needed. |
| D12 (Q6) | Windows 10+ x64 only, on stable Rust. ARM64 Windows comes later as an extra CI target. No Windows 7/8.1, no 32-bit. | No nightly/`build-std`; tier-1 target. |
| D13 (Q7) | Strict µTorrent 2.2.1 look by default: classic list view, 2.2.1 layout, menus and toolbar order. Allowed modern necessities: per-monitor DPI scaling and long paths. An optional dark mode may come later, never as the default. | Faithful clone first. |
| D14 (Q9) | Coverage-guided fuzzing with cargo-fuzz on nightly, in a separate `fuzz/` workspace. The internal parsers reach it through the `fuzzing` feature of `patorrent-core` (hidden module `fuzzing`). CI fuzzes each target on Linux: 2 min per push to main that touches the engine, 30 min weekly. | Nightly stays out of the product build (D12 still holds). Linux CI needs no ASan setup. |
| D15 (Q1) | License: **PolyForm Noncommercial 1.0.0**, copyright **DukeCorp1996** (`LICENSE.md` with a `Required Notice:` line, SPDX `PolyForm-Noncommercial-1.0.0` in every manifest). Ported libtorrent code also keeps its BSD-3 notice. `LICENSE.md` ships in `dist\` and the CI artifact, `patorrent-nox --license` prints it, and the About box names it. | The owner wants no commercial use and their own name as author. Accepted consequence: source-available, not OSI open source, so no Debian/Fedora/F-Droid packaging. |
| D16 (Q2, Q8) | Name: **PaTorrent** (long name **PatinaTorrent**; a patina is the good kind of rust). Peer-ID prefix `-PA0100-`, User-Agent `PaTorrent/<version>`, BEP 10 `v` = `PaTorrent <version>`. Crates `patorrent-core` / `-frontend` / `-nox`, binary `patorrent.exe`, `%APPDATA%\PaTorrent`, `patorrent.ini`. `LR` stays mapped to the Android LibreTorrent in our peer table. | "LibreTorrent" was taken (Android, peer ID `LR`) and "libre" is wrong under D15. The owner turned down Ferrite (`Fe` reads badly) and Oxbit. `PA` is free in libtorrent's table and the BEP 20 list. |
| D17 (Q8) | No peer-ID spoofing. PaTorrent always reports itself. | A borrowed ID with this engine's fingerprint gets accounts banned on private trackers and misreports the client. |
| D18 | The Web UI is our own, not qBittorrent's: the µTorrent 2.2.1 main window in a browser, driven by a native push API (`/pa/ws` WebSocket down, JSON `POST`s up) designed from what the page needs. The page is a thin terminal of `patorrent-frontend`: the server runs the filtered, sorted `TorrentList` per client and sends formatted cells for the visible window only. No framework, no build step, embedded. The qBittorrent subset (D10) stays as a shim. Design: [docs/WEBUI.md](docs/WEBUI.md). | Parity with the desktop for free (one `frontend` decides what is shown), 10,000 torrents without the page holding them, works on a phone, keeps the Sonarr / Radarr ecosystem at no design cost. Decided with the owner 2026-10-17. |

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
- [x] Coverage-guided fuzz targets (cargo-fuzz, Q9 → D14): `bdecode`, `torrent_info`, `magnet`, `peer_wire`, `tracker_response`, `http_response`, `ut_metadata` in `fuzz/`; `fuzz.bat` locally, `.github/workflows/fuzz.yml` in CI
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
Implemented 2026-10-15 (CHANGELOG 2026-10-15_172326). Checked on screen by the owner 2026-10-15 (window, adding a torrent, a download through the GUI).
- [x] Main window parts:
  - menu and toolbar (original runtime-drawn icons)
  - category tree with counts (labels included)
  - virtual torrent list: 24 columns (the 2.2.1 set as far as known, from memory; check against a real 2.2.1), in-cell progress bar, sorting, column chooser, persisted layout
  - detail tabs: General, Trackers, Peers, Pieces, Files, Speed, Logger
  - status bar: DHT nodes, D/U rate and totals, network indicator (text only so far)
  - splitters
- [x] Adding torrents: Open dialog, drag and drop, command line, magnet/URL dialog. Single instance that forwards arguments (verified).
- [x] Context menus:
  - Start, Pause, Stop, Force Start
  - Remove, Remove and delete data
  - Force Re-check, Update Tracker
  - queue moves, bandwidth, labels
- [x] Tray icon, minimize/close to tray, completion balloon
- [x] Per-monitor DPI v2, ComCtl v6 manifest via linker args (no resource compiler needed). `WM_DPICHANGED` rescales fonts, icons and columns (not yet tried on a second monitor).
- [x] Scale gate: 10,000-torrent profile meets the startup (0.2–0.4 s), RAM (60 MB) and idle-CPU (0.1 %) targets.
- [x] Visual check by a person, then a real download through the GUI (2026-10-15).

### Phase 4: Discovery and connectivity
- [x] DHT: BEP5, BEP32 (IPv6), BEP42 (node id security), BEP43 (read-only); bootstrap; state persistence (2026-10-15). `core::dht` ports libtorrent's routing table, rpc manager, get_peers / announce / bootstrap lookups (plus the obfuscated variant), peer storage with scrape filters, DoS blocker, node ids and `dht_state`; the engine runs it on UDP sockets on the listen port, feeds peers to running torrents (`PeerSource::DHT`), announces running torrents and shows the node count in the status bar. Ported tests: `test_dht` (all but the BEP 44 / 51 / signing ones), `test_dos_blocker`, `test_bloom_filter`, `test_ip_voter`, most of `test_dht_storage`. Fuzz target `dht_message`. Verified with Debian's swarm and no trackers
- [ ] Extensions:
  - [x] ut_metadata (BEP 9): magnets and bare info-hashes fetch the info dictionary from peers, then check and download as usual with their peers kept; we serve metadata to others (not for private torrents). A magnet waiting for metadata is saved and restored. Verified with Debian's swarm (2026-10-15)
  - [x] ut_pex (BEP 11): we send each peer the full list, then a diff once a minute (up to 100 added, seeds flagged) and take peers from theirs (50 per connection and family, flood limit, local addresses only from local peers); not for private torrents or with `disable_pex`. Verified with Debian's swarm (2026-10-15_221714_1792095434)
  - [x] lt_donthave (BEP 54) and smart-ban (2026-10-16_225433_1792184073): every extension handshake names `lt_donthave` (private torrents too); an incoming DONT_HAVE clears the piece in the peer's set, the picker's availability and our interest (a seed becomes a plain peer); we send it when an upload read fails and when a piece in our set fails a check. Hash failures now blame the senders as libtorrent does: `get_downloaders` per block, trust points (-2 per failure, floor -7, +1 per pass, cap 8), `hashfails`, parole (`use_parole_mode`: a suspect only downloads whole pieces alone), the only sender of a bad piece banned at once, and the smart_ban plugin hashing every block of a failed piece per peer and banning, when the piece later passes, the peers whose blocks differed. `Alert::PeerBanned`; `PeerFlags::ON_PAROLE`. Tests: codec round trip, picker downloaders / parole / `we_dont_have`, the plugin's rules, and two swarm tests (a DONT_HAVE peer, after `test_fast_extension`'s `dont_have`; a corrupting peer banned while the seed is kept)
- [x] LSD (BEP 14): multicast announces of running torrents (3 sends at 0, 2 and 6 s, then a round-robin over `local_service_announce_interval`), one socket per local interface and family that the listen addresses cover, subnet filter, own-cookie filter; peers as `PeerSource::LSD`, never for private torrents or with `TorrentFlags::DISABLE_LSD` (2026-10-15_234806_1792100886)
- [x] UPnP IGD and NAT-PMP / PCP port mapping (2026-10-16_155608_1792158968): `core::engine::{natpmp, upnp, portmap}` and `core::proto::{natpmp, upnp, xml}` port libtorrent's `natpmp.cpp`, `upnp.cpp`, `xml_parse.cpp` and the mapper glue of `session_impl`. One mapper pair per IPv4 interface with a default route (the routing table comes from `net::sys::routes`: `GetIpForwardTable2`, `/proc/net`, or a `PF_ROUTE` sysctl on FreeBSD and OpenBSD); each forwards the TCP listen port and the DHT's UDP port, refreshes the leases and deletes the mappings at shutdown. PCP first, NAT-PMP on "unsupported version". `Settings::enable_upnp`, `enable_natpmp`, `upnp_lease_duration`, `upnp_ignore_nonrouters`. Alerts `PortmapSucceeded`, `PortmapFailed`, `PortmapLog`, `ExternalIp`. Ported tests: `test_xml` (all), `test_upnp`'s loopback IGD run and the 50-mapping cap, plus NAT-PMP / PCP tests of our own against a fake gateway on loopback. Fuzz targets `upnp` and `natpmp`. Verified on the real router
- [x] MSE/PE encryption (2026-10-16_163419_1792161259): `core::crypto::mse` ports `pe_crypto` (RC4 with the 1024-byte discard, the 768-bit Diffie-Hellman exchange on a Montgomery multiplier of our own, the `req1` / `req2` / `req3` and `keyA` / `keyB` SHA-1 keys); `core::proto::pe_handshake` is the sans-IO handshake state machine (libtorrent's `read_pe_*` states, both sides); `core::engine::pe` applies `out_enc_policy` / `in_enc_policy` / `allowed_enc_level` / `prefer_rc4` with libtorrent's defaults (enabled / enabled / both, no RC4 preference) and libtorrent's `pe_support` alternation plus fast reconnect under `Enabled`. Incoming encrypted connections find their torrent through `SHA-1("req2", info-hash)`; ut_pex sends and reads the `e` flag; peers report `RC4_ENCRYPTED` / `PLAINTEXT_ENCRYPTED` (`E` / `e` in the GUI). Ported tests: `test_pe_crypto` (both), plus known answers for RC4 and the exponentiation and a loopback handshake test over the level matrix. Fuzz target `pe_handshake`. Swarm tests: forced on both sides (RC4, and plaintext stream with both levels), enabled vs forced (plaintext refused, encrypted retry), enabled vs disabled both ways, forced vs disabled never connects. `lt-swarm interop --encryption forced` verified against qBittorrent
  - µTorrent-style Preferences (Phase 5) map onto these: "Outgoing: Disabled / Enabled / Forced" is `out_enc_policy`; "Allow incoming legacy connections" checked is `in_enc_policy = Enabled`, unchecked `Forced` (µTorrent never refuses encrypted incoming connections, so `in_enc_policy = Disabled` has no UI); `allowed_enc_level` stays `Both` except `Forced`, which qBittorrent / Transmission pair with RC4 only
- [x] uTP (BEP 29) with LEDBAT (2026-10-16_220720_1792181240): `core::utp` ports `utp_stream` / `utp_socket_impl` (sequence numbers, send and reorder buffers, selective ACKs, Nagle, path MTU discovery with DF probes, the delay histories with clock-drift compensation, LEDBAT and slow start, timeouts and resends), `utp_socket_manager` (demultiplexing by connection id, incoming SYNs, deferred ACKs, stalls on a full UDP buffer), `packet_buffer` and `timestamp_history`, as a sans-IO layer the engine drives through a `UtpIo` trait. Each TCP listener now has a UDP socket on the same port beside it (libtorrent's listen socket pair; the DHT moved onto it, IPv6 v6-only so Linux gets both families; the DHT announces with `implied_port`). `PeerConn` carries a `Transport` (TCP or uTP); the connect policy is libtorrent's: uTP first for every peer (`supports_utp` starts true, a PEX `utp` flag sets it), a failed uTP connect marks the peer TCP-only and reconnects over TCP at once, a successful one pins uTP. `enable_incoming_utp` / `enable_outgoing_utp` plus the `utp_*` tuning settings with libtorrent's defaults; the µTorrent `bt.transp_disposition` bits map onto the four `enable_*` flags. `SessionStats` carries the uTP socket count and packet counters (nox prints them). Ported tests: `test_packet_buffer` (all), `test_timestamp_history` (plus its two TODO cases), `compare_less_wrap`; a virtual-network simulation (loss, both directions, Nagle, stalls, the close handshake, timeouts, resets, MTU probing, PMTU black holes) replaces `simulation/test_utp.cpp`. Fuzz target `utp`. Swarm tests: default preference, uTP-only, fallback to TCP, TCP-only leech, RC4 over uTP, uTP-only vs TCP-only never connect
- [x] IPv6 throughout; interface enumeration, network-change handling
  - [x] external-IP voting (2026-10-17_114537_1792230337): `net::ip_voter::SessionVoter`, one libtorrent `ip_voter` per address family, fed by trackers, peers (`yourip`), DHT replies and the port mappers; a change posts `ExternalIp`, re-ranks peers (BEP 40) and moves the DHT node id (BEP 42)
  - [x] `listen_interfaces` expansion (device and address sockets), network-change handling (`ip_notifier`: listeners, LSD, port mappers, re-announces follow interface changes), v6-only TCP listeners, NAT-PMP / PCP on IPv6 (2026-10-17_140012_1792238412). Wildcards stay one socket pair per family (see the follow-ups)

### Phase 5: µTorrent 2.2.1 feature parity
- [ ] Preferences, with all 2.2.1 pages: General, UI Settings, Directories, Connection, Bandwidth, BitTorrent, Queueing, Scheduler, Web UI, Advanced (raw settings table), Disk Cache
  - [x] the Win32 dialog (2026-10-17_155028_1792245028): Options > Preferences / Ctrl+P / toolbar, every page in the tree, OK / Cancel / Apply without a restart, patorrent.ini persistence, IP filter (enable, path, Reload); `patorrent_frontend::settings` is the string-keyed table (every `Settings` field by libtorrent name, plus `bt.transp_disposition`) and `patorrent_frontend::options` the toolkit-independent model for GTK
  - [x] the Proxy Server box on the Connection page (type, host, port, authentication, resolve names through the proxy, peer and tracker connections) (2026-10-17_161604_1792246564)
  - [x] Directories > "Move completed downloads to" (folder, Browse) and "Only move from the default download directory" (2026-10-17_193944_1792258784): `dir.move_completed` / `dir.completed` / `dir.move_only_default` in patorrent.ini, applied live through `patorrent_core::MoveCompleted` in the alert loop
  - [ ] placeholders still disabled: Scheduler, Web UI (Phase 6), language, file association / autostart, .torrent folders (store / auto-load), seeding goal
- [x] Add-torrent dialog: save folder, file tree with priorities, label, skip hash check, start/stop (2026-10-17_163655_1792247815): µTorrent 2.2.1's Add New Torrent for `.torrent` files from File > Add Torrent, drag and drop, the command line, a second instance's arguments and Add Torrent from URL (http/https); Save In with the recent folders (`add.recent_dirs`), Name, Torrent Contents (name, comment, size with free disk space, date), a virtual file tree with check boxes and priorities, Select All / None, Skip hash check, Start torrent, Add to top of queue, Label; Preferences > UI Settings "Show a window that displays the files inside the torrent" (`ui.show_add_dialog`). Model in `patorrent_frontend::add`
  - [ ] magnets: still added directly (no file list before the metadata arrives). Showing the dialog once metadata is known needs either an engine "fetch metadata only" add (libtorrent: add paused with `stop_when_ready` / qBittorrent's metadata-download session) plus a way to apply the save path afterwards (move storage), or a GUI-side wait on `MetadataReceived` with `prioritize_files` and `move_storage`
  - [x] Name renames the torrent's folder (or its only file) on disk through `AddTorrentParams::renamed_files` (2026-10-17_200844_1792260524); `add::renamed_root` builds the renames, so GTK gets the same
- [ ] Create New Torrent: piece-size auto, trackers, web seeds, private flag, progress
  - [x] engine and nox, v1 (2026-10-17_114537_1792230337): `core::metainfo::create_torrent` (libtorrent `create_torrent`, `add_files`, `set_piece_hashes`), `make_magnet_uri_from_info`, `patorrent-nox create`
  - [x] the Win32 dialog (2026-10-17_140012_1792238412): File > Create New Torrent / Ctrl+N, µTorrent 2.2.1 layout, hashing on a worker thread, Start seeding
  - [ ] v2 / hybrid (Phase 8)
- [ ] Labels; RSS feeds and RSS Downloader (filters, episode tracking); weekly scheduler; per-torrent bandwidth allocation and limits; auto-shutdown; move-on-complete; auto-load folder; `.torrent` and `magnet:` association
- [ ] Speed graph, pieces map and list, availability bar
- [ ] Network features:
  - [x] IP filter (`ipfilter.dat`, PeerGuardian P2P) and port filter in the engine and nox (2026-10-17_114537_1792230337); in the GUI Preferences (2026-10-17_155028_1792245028)
  - [x] proxies (SOCKS4/4a, SOCKS5 with password and UDP associate, HTTP CONNECT / plain HTTP proxy requests) in the engine and nox (2026-10-17_160554_1792245954); the Preferences Proxy Server box (2026-10-17_161604_1792246564)
  - [x] web seeds (BEP 19 `url-list` and magnet `ws=`, BEP 17 `httpseeds`) in the engine (2026-10-17_140012_1792238412)
  - [x] part files for skipped files (2026-10-17_160022_1792245622): libtorrent's `part_file` in `core::disk::part_file`; blocks of skipped files go to `.<info-hash>.parts` in the save path, unskipping moves them into the real file, `RemoveFlags::DELETE_PARTFILE` works
  - [x] super-seeding (BEP 16, µTorrent "initial seeding") in the engine, nox (`--super-seed`) and the Win32 context menu (Advanced > Initial Seeding) (2026-10-17_155425_1792245265)
  - [x] move storage (2026-10-17_163533_1792247733): `TorrentHandle::move_storage(path, MoveFlags)` with libtorrent's five modes, running or stopped, part file included, `Alert::StorageMoved` / `StorageMovedFailed`; `MoveCompleted` for "Move completed downloads"; `patorrent-nox ... --move-on-complete DIR`. In the Win32 GUI (2026-10-17_193944_1792258784): Preferences > Directories "Move completed downloads", the context menu's Advanced > Set Download Location..., "Moving" status, Logger lines and an error box on `StorageMovedFailed`
- [ ] Localisation infrastructure (string tables)

### Phase 6: Headless `patorrent-nox`
Design: [docs/WEBUI.md](docs/WEBUI.md) (D18, decided 2026-10-17_203617_1792262177).
- [ ] Daemon: Unix daemon / systemd unit, optional Windows service, signal handling, XDG paths
- [x] Embedded HTTP/1.1 server (std threads beside the engine, not on its reactor; CHANGELOG 2026-10-17_164324_1792248204); authentication (SID cookie, PBKDF2 password hash, failed-login ban), CSRF and host-header protections
  - [ ] optional TLS
- [x] Compat API (D10, a shim since D18) with incremental `sync/maindata` diffs: the qBittorrent WebAPI 2.11.2 subset in CHANGELOG 2026-10-17_164324_1792248204
  - [ ] still missing: tags, RSS, search, logs, peers (`sync/torrentPeers`), move / setLocation (`TorrentHandle::move_storage` exists now); `renameFile` / `renameFolder` are in (2026-10-17_200844_1792260524), trackers editing, queue positions, share limits, automatic torrent management, cookies
- [x] Small dependency-free Web UI, embedded in the binary: a first page (login, list, add by link, start / stop / remove), polling the compat API; to be replaced by the native page below
- [ ] Native API and Web UI (D18, docs/WEBUI.md), in slices:
  - [x] 1. WebSocket in the HTTP server (RFC 6455 handshake, frames, ping / pong, close, `Origin` check), per-client wake-up from the alert loop: `webui/ws.rs`, `GET /pa/ws` (CHANGELOG 2026-10-17_210452_1792263892)
  - [x] 2. session protocol: `view` subscriptions on a per-client `frontend::model::TorrentList`, `hello` / `rows` / `tick` with counts and the status bar, cells from `frontend::columns`: `webui/native.rs` (CHANGELOG 2026-10-17_210452_1792263892); the 10,000-torrent profile gives a 22 KB `rows` (60 rows, 24 columns) and a 202-byte idle `tick`
  - [ ] 3. the page: 2.2.1 layout, virtual list, category tree, status bar, context menu, keyboard, selection; the 10,000-torrent gate measured
  - [ ] 4. detail tabs (General, Trackers, Peers, Pieces, Files, Speed, Logger); `post_peers` added to the cache and `sync/torrentPeers` to the shim
  - [ ] 5. commands and the Add dialog (upload, magnet, two-step add with the file tree, Remove and delete data); the shim's handlers moved onto the shared functions
  - [ ] 6. Preferences generated from `frontend::options`
  - [ ] 7. phone layout; long-poll fallback only if a deployment needs it
  - [ ] 8. with Phase 5: labels, RSS, scheduler, per-torrent limits; the GUIs' remote mode on its own binary stream
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

1. Try a magnet link in the GUI (Add Torrent from URL, or a `magnet:` link on the command line): the row should show "Downloading metadata" with a growing percentage, then turn into a normal download.
2. Phase 5 continues (2026-10-17_161604_1792246564: Preferences, proxies, super-seeding and part files are in). Click through Preferences by hand once (every page, OK / Cancel / Apply, the Proxy box, the Advanced list) and Advanced > Initial Seeding. The Add New Torrent dialog is in (2026-10-17_163655_1792247815): open a multi-file `.torrent`, uncheck a folder, set a priority from the right-click menu, pick a label and a recent Save In folder, OK, then check the Files tab and that the skipped files stay off disk. Move storage is wired into the GUI (2026-10-17_193944_1792258784): set Preferences > Directories "Move completed downloads to", let a small torrent finish and watch it go "Moving" then land there (Logger tab); right-click > Advanced > Set Download Location... on a seed, once to another folder on the same drive and once to another drive letter. Next candidates: labels and the scheduler. `patorrent-nox --proxy socks5://host:1080` and `add ... --super-seed` exercise the new engine features. `patorrent-nox run --no-state --seconds 20 --verbose` shows the mappers at work (`NatPmp:` / `Upnp:` lines); `--listen LIST` takes a `listen_interfaces` string; `add ... --verbose` prints where connected peers came from every 10 s.
3. Keep CI green on Windows, Linux and FreeBSD. The three flaky tests that made the 2026-10-15/16 runs red (UPnP fake router outliving its test, random DHT bucket count, the stingy peer's last-piece arithmetic) are fixed (CHANGELOG 2026-10-16_203336_1792175616); if a run goes red now, it is a real failure.
4. Web UI: docs/WEBUI.md slices 1 (WebSocket) and 2 (the session protocol) are in (2026-10-17_210452_1792263892); next is slice 3 (the page on `/pa/ws`), then measure the 10,000-torrent gate in the browser. The server-side half of the gate is measured: 22 KB for 60 rows by 24 columns, 202 bytes per idle tick. The current page (`patorrent-nox run --webui 8080 --webui-pass <password>`, http://127.0.0.1:8080/) keeps working on the compat API until slice 3 replaces it. Sonarr / Radarr (qBittorrent download client, host 127.0.0.1, port 8080) and a qBittorrent mobile app still show which shim endpoints matter. Use `--webui 0.0.0.0:8080` plus `web_ui_domain_list` for a LAN host name.
5. Rerun `lt-swarm interop` (and `lt-swarm interop --encryption forced`) after engine changes that touch the peer wire, the picker or the disk. It needs the local client setup in Q10, so it does not run in CI. On 2026-10-16 WSL2 refused to start on this machine ("Virtual Machine Platform" / virtualization off), so only the qBittorrent case ran; rerun the Transmission and Deluge cases, plain and encrypted, once WSL is back.

Phase 2 closing items, done 2026-10-15: OpenSSL TLS backend linked and tested on Linux (CI run 38738279981) and FreeBSD (run 38736854046); partial pieces in resume data (CHANGELOG 2026-10-15_160110); reverse-direction interop scripted as `lt-swarm interop` (CHANGELOG 2026-10-15_161111).

## Open questions

- **Q1, Q2, Q8, Q9.** Resolved, see D14–D17.
- **Q3–Q7.** Resolved, see D9–D13.
- **Q11. GitHub repository name.** Resolved 2026-10-15: renamed to `Balls0fSteel/PaTorrent` (GitHub redirects the old URL); Cargo.toml, the README and the `origin` remote point there. The local folder is still `libretorrent`; renaming it is optional and best done with no editor or session open in it.
- **Q10. Interop testing setup.** Answered: qBittorrent runs as the installed app with a throwaway `--profile`; Transmission and Deluge run in the Debian WSL distro (`transmission-cli`, `deluged`, `deluge-console` installed 2026-10-15; the `deluged` system service the package enabled was stopped and disabled). `lt-swarm interop` scripts all three; `--encryption forced|enabled|disabled` sets the clients' encryption settings too (qBittorrent `Session\Encryption`, Transmission `encryption`, Deluge `enc_in_policy` / `enc_out_policy` / `enc_level`).

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
- Add New Torrent dialog (2026-10-17_163655_1792247815):
  - a label chosen in the dialog is stored as `torrent.<info-hash>.label` before the engine adds the torrent; if the add then fails, the line stays in `patorrent.ini` (harmless, reused if the torrent is added again).
  - µTorrent 2.2.1 also had "Don't show this again" and per-torrent "Advanced" (sequential download, initial seeding) in the dialog; not built.
  - adding a `.torrent` already in the list shows "already in the torrent list" instead of merging its trackers (µTorrent asks whether to load the new trackers).
- Part files (CHANGELOG 2026-10-17_160022_1792245622):
  - the Win32 "Remove" (without data) passes `RemoveFlags::empty()`, so a removed torrent with skipped files leaves its `.<info-hash>.parts` behind; qBittorrent passes `delete_partfile` there. Decide when the remove dialog is next touched.
  - ~~moving storage should call `PartFile::move_partfile` and rekey the `PartFiles` registry~~: done (2026-10-17_163533_1792247733).
  - skipping a file that is already on disk keeps its blocks in the file, as libtorrent does ("move stuff into the part file: not implemented yet" in `posix_storage.cpp`).
  - after a restart, a finished torrent closes its connection to a seed as redundant and waits `min_reconnect_time` (60 s) before reconnecting once a file is unskipped; libtorrent behaves the same, but an unskip could reset the peer list's reconnect timers.
- Peer-ID version characters: `-PA0100-` encodes 0.1.0. Bump `PEER_ID_PREFIX` together with the crate version (a test enforces it).
- CI lints with stable clippy (1.97.1 on 2026-10-17), newer than the pinned 1.90.0: run `cargo +stable clippy --workspace --all-targets -- -D warnings` and `cargo +stable fmt --all --check` plus `cargo +stable fmt --manifest-path fuzz/Cargo.toml --check` before pushing (CHANGELOG 2026-10-17_153322_1792244002).
- Unix code can be type-checked and linted here: `cargo clippy --target x86_64-unknown-linux-gnu` (or `x86_64-unknown-freebsd`); both targets are installed for the 1.90.0 toolchain. Running the tests still needs CI.
- Fuzzing on Windows needs Visual Studio's AddressSanitizer runtime (`clang_rt.asan_dynamic-x86_64.dll`); `fuzz.bat` finds it with vswhere. A build without a sanitizer (`-s none`) does not link on MSVC (libFuzzer's PC table uses ELF section symbols). The default toolchain here is pinned to 1.90.0, and cargo-fuzz 0.13 needs 1.91+, so it was installed with `cargo +stable install cargo-fuzz --locked`. In CI, cargo-fuzz from `taiki-e/install-action` is a musl build that defaults to the musl target, so `fuzz.yml` passes `--target x86_64-unknown-linux-gnu`.
- Fuzz targets still to add as the code lands: UDP tracker responses (no public parser yet), resume data (docs/PORTING.md rule 8). The nox HTTP API: done (`nox_http`). DHT messages: done (`dht_message`); ut_pex: done (`ut_pex`); LSD: done (`lsd`); UPnP (SSDP, the XML scanner, device descriptions and SOAP replies): done (`upnp`); NAT-PMP / PCP replies: done (`natpmp`); the MSE/PE handshake: done (`pe_handshake`); uTP packets into every socket state: done (`utp`).
- Linux / FreeBSD VMs (2026-10-17): log in with `ssh -i ~/.ssh/patorrent_vm x@192.168.32.128` (Ubuntu) or `x@192.168.32.131` (FreeBSD); Rust 1.90.0 is in `~/.cargo` on both. Copy the tree with `git archive --prefix=patorrent/ HEAD | ssh ... 'tar -xf - -C ~'` (no git on FreeBSD), then `. ~/.cargo/env && cargo test --workspace`. FreeBSD has no sudo yet (needs root's password). The Transmission / Deluge interop clients (Q10) are not installed on the Ubuntu VM yet.
- Web UI follow-ups (CHANGELOG 2026-10-17_164324_1792248204):
  - `torrents/add` takes `http://` links only: HTTPS needs the engine's TLS stream exported from `patorrent-core` (a small public fetch helper would do). HTTPS for the Web UI itself (Phase 6 "optional TLS") needs the same.
  - state strings are the documentation's `pausedDL` / `pausedUP`; qBittorrent 5.x sends `stoppedDL` / `stoppedUP`. Switch if a client turns out to care (one constant per state in `webui/torrent.rs`).
  - `sync/maindata` keeps the last sent field values per session to diff against: about 50 values per torrent per logged-in client. Fine for hundreds of torrents; at 10,000 torrents and several clients consider diffing on per-field change counters instead.
  - `isPrivate`, `comment`, `created_by`, `creation_date` are known only for torrents added through the API in the running process: a `TorrentStatus` field (or a metadata query) in the engine would fill them for restored torrents too.
  - stored `setPreferences` values override the command line's rate limits and port at the next start; revisit if that surprises anyone.
  - `pieceStates` reports no "downloading" (1) pieces: the engine's `PieceInfo` has no partial-piece set.
- Web seed follow-ups (CHANGELOG 2026-10-17_140012_1792238412):
  - a loopback HTTPS web seed test.
  - the Win32 Peers tab should show web seed URLs (`PeerInfo.client` is the server's `Server` header).
  - as in libtorrent, a server known to have only some files claims no piece overlapping the others; with unaligned files and per-file redirects a boundary piece may have no web seed.
- Proxy follow-ups (CHANGELOG 2026-10-17_160554_1792245954):
  - the Win32 Preferences Connection page has a disabled "Proxy Server" box: wire it to the `proxy_*` settings (µTorrent 2.2.1 types None / SOCKS4 / SOCKS5 / HTTPS / HTTP: both HTTP kinds are `ProxyType::Http`, "Authentication" picks the `_pw` types; "Use proxy for hostname lookups" is `proxy_hostnames`, "Use proxy server for peer-to-peer connections" `proxy_peer_connections`). The frontend settings table needs a hand-written `Entry` for `ProxyType`
  - not tried against real proxies (Dante, Squid, Tor, `ssh -D`, 3proxy); HTTPS trackers and web seeds through a tunnel (TLS over the tunnel) have no loopback test; IPv6 proxy addresses and an IPv6 relay are untested; incoming uTP through the relay is untested
  - `proxy_send_host_in_connect` is not a libtorrent 2.0.11 setting; its meaning here (CONNECT names the host, plus a `Host` header) should be checked against the libtorrent version that has it
  - nox `--proxy` sets the proxy only: `proxy_hostnames`, `proxy_peer_connections`, `proxy_tracker_connections` and `socks5_udp_send_local_ep` keep their defaults until the settings table is in nox
  - not ported: I2P (`i2p_proxy`), `anonymous_mode`
- Network-change follow-ups (CHANGELOG 2026-10-17_140012_1792238412):
  - no live test of the FreeBSD PF_ROUTE notifier (needs root on the VM) or of Windows notifications (needs an adapter added as admin); OpenBSD offsets untested; macOS / NetBSD / DragonFly use the 30 s re-read.
  - one UDP socket per family: with several device-bound addresses of a family, uTP and the DHT use only one; wildcards are never split into per-address sockets / DHT nodes (BEP 45 multi-homing).
  - with nothing bound (device-only setup while the device is gone) trackers still get the old port; uTP connections on a closed UDP socket are not moved and time out.
  - SSL listen entries (`...s`) are refused with `ListenFailed` (no SSL torrents).
- Rename follow-ups (CHANGELOG 2026-10-17_200844_1792260524):
  - the Win32 Files tab has no Rename item (µTorrent 2.2.1 had none either; 3.x added one); `TorrentHandle::rename_file` is ready for it.
  - renaming before the metadata is known fails (`FileRenameFailed`); `AddTorrentParams::renamed_files` does work for magnets, applied when the metadata arrives.
  - a rename onto an existing file replaces it (`std::fs::rename`, as libtorrent's `rename`); nox's `renameFile` refuses names already used by the torrent, the engine does not check.
  - `mapped_files` keeps names verbatim; a name with `\` on Windows is stored as given (FileStorage splits on `/` only), so frontends should pass `/` separators.
  - `torrents/renameFile` / `renameFolder` answer 200 before the disk rename finishes (qBittorrent does too); a failure shows only in nox's console.
- Move storage follow-ups (CHANGELOG 2026-10-17_163533_1792247733):
  - ~~Win32: Preferences > Directories "Move completed downloads" through `MoveCompleted`, Advanced > Set Download Location, `StorageMoved` / `StorageMovedFailed` in the Logger tab~~: done (2026-10-17_193944_1792258784), hidden-dialog tests only; not clicked through by hand. nox's Web API (Phase 6) still lacks `setLocation` for the same call.
  - Win32 Set Download Location: µTorrent 2.2.1 showed a Save As box (file name included) for single-file torrents; ours is a folder picker for every torrent, so it cannot rename the file (`rename_file` exists now; a Save As box for single-file torrents could move and rename together). It is offered for running torrents too (the engine moves running torrents); greyed only while a selected torrent is already moving.
  - "Move completed downloads" also moves a torrent that is added with its data already complete (the check posts `TorrentFinished`, as libtorrent's does); restored seeds are not moved. The move-on-complete folder is not offered as a Save In choice in the Add New Torrent dialog.
  - a move across real volumes on Windows (another drive letter) has not been tried; the copy path is tested on the Linux and FreeBSD VMs only (tmpfs / ZFS dataset to the target directory).
  - big cross-volume copies run on one disk thread and hold the torrent's other disk jobs back until they finish (libtorrent the same); shutdown waits for a move in flight.
  - ~~`rename_file` is not ported~~: done (2026-10-17_200844_1792260524), see the rename follow-ups.
- Super-seeding follow-ups (CHANGELOG 2026-10-17_155425_1792245265):
  - the "[S] Seeding" status text follows µTorrent's "[F]" style; the exact µTorrent 2.2.1 string was not checked against the real client.
  - µTorrent's Advanced submenu has more items (Set Destination Name, Reset Bans, Clear Peer List...); Set Download Location... (2026-10-17_193944_1792258784) and Initial Seeding are there so far.
  - `lt-swarm interop` with a super seed against qBittorrent / Transmission / Deluge leeches has not run: they get HAVE_NONE then HAVE_ALL when it is switched off (libtorrent accepts that; others may not).
  - libtorrent's deprecated `strict_super_seeding` (re-offer when any peer reports the offered piece) is not ported.
- Preferences follow-ups (CHANGELOG 2026-10-17_155028_1792245028):
  - click through the dialog by hand once (only the hidden-dialog tests have run): every page, OK / Cancel / Apply, Ctrl+P, the toolbar button, Browse, Reload with a real `ipfilter.dat`, the Advanced list at 150 % DPI.
  - `settings::tests::every_field_has_a_row` fails when a `Settings` field has no row in `patorrent_frontend::settings::TABLE`: every new field needs a row (the proxy rows were added at the merge, 2026-10-17_161604_1792246564).
  - the proxy password is stored in patorrent.ini as plain text (`lt.proxy_password`), as µTorrent stored it in settings.dat; DPAPI (`CryptProtectData`) would keep it from other users of a copied ini. The proxy box does not grey out host / port / user / password while the type is "(none)", as µTorrent's did.
  - "Maximum connected peers per torrent" and "upload slots per torrent" apply to torrents added afterwards (`AddTorrentParams::max_connections` / `max_uploads`); there is no `TorrentHandle` setter for running torrents yet, and both default to no limit (µTorrent: 50 and 4).
  - `aio_threads` and `hashing_threads` are read at session start only (the Disk Cache page says so); `apply_settings` does not resize the pools.
  - placeholders listed under Phase 5; the Advanced page has no per-setting help text (µTorrent had none either).
  - the dialog procedures hold a `&mut State` across calls that send notifications back into the same procedure (tree and list view), as `create.rs` does; a `RefCell` like `app.rs`'s `with_app` would be stricter.
- Create dialog follow-ups (CHANGELOG 2026-10-17_140012_1792238412): click through it by hand once; possible extras libtorrent supports but µTorrent lacked (source tag, pad files, DHT nodes).
- Create torrent follow-ups (CHANGELOG 2026-10-17_114537_1792230337):
  - v2 and hybrid creation (BEP 52): `set_hash2`, piece layers, file tree; then the rest of `test_create_torrent.cpp` and `make_magnet_uri` v2 / hybrid.
  - `test_torrent_info.rs`'s `resolve_duplicates` comment says it waits for create_torrent; it could now build its torrents with `CreateTorrent`.
- IP filter follow-ups (CHANGELOG 2026-10-17_114537_1792230337):
  - ~~GUI: Preferences "IP filter" (enable, file path, reload), loading `ipfilter.dat` from the settings directory like µTorrent~~ and ~~persist the path and reload on start~~: done in the Preferences dialog (2026-10-17_155028_1792245028). Still open: `Alert::PeerBlocked` in the Logger tab; nox takes the filter per run only.
  - `get_ip_filter` / `export_filter` on the session API if the GUI needs to show the rules.
  - other `peer_blocked_alert` reasons (`privileged_ports`, `utp_disabled`, `tcp_disabled`, `invalid_local_interface`, `ssrf_mitigation`) are not ported; the peer-class filter waits for peer classes.
  - gzip-compressed block lists (`.gz`, common for P2P lists) are not read.
- External-IP voting follow-ups (CHANGELOG 2026-10-17_114537_1792230337):
  - router votes are attributed to the unspecified address (libtorrent has a TODO to use the router's address).
  - the `dht_message` fuzz target no longer exercises voting (the voter is engine-side); `SessionVoter` could get its own fuzz / property test.
- uTP follow-ups (CHANGELOG 2026-10-16_220720_1792181240):
  - `mixed_mode_algorithm` (libtorrent `peer_proportional`: throttle TCP peers in proportion while uTP peers are connected, so uTP's yielding does not just hand the link to our own TCP connections) is not ported; it needs per-protocol rate limits (peer classes). Until then the behaviour is `prefer_tcp` (no throttling).
  - the `close_reason` extension is parsed and dropped; nothing is sent (the engine has no close reasons yet). Holepunching (BEP 55, which libtorrent runs over uTP) is Phase 8.
  - `mtu_for_dest` assumes Ethernet (1500) or Teredo (1280) like libtorrent 2.0; the interface MTU (jumbo frames, VPN tunnels) is not read. The DF bit is set per probe through `setsockopt` around each send (Windows, Linux, FreeBSD, macOS); on OpenBSD, NetBSD and DragonFly probes are fragmented instead of dropped, so the search ends at the interface MTU.
  - a path that silently drops big packets without fragmenting (no ICMP, DF ignored) kills the connection whose probe it ate, as in libtorrent; the manager lowers the ceiling for new connections only after three such failures (`restrict_mtu` keeps the largest of the last three).
  - uTP sockets are ticked from the session tick (500 ms by default), so a timeout fires up to one tick late; libtorrent is the same. The reactor's wake-up deadline does not include uTP timeouts.
  - a peer blocked by its congestion window sets `write_blocked`, which makes the 100 ms bandwidth tick run while any such peer has data queued (the uTP ready list wakes it anyway when the window opens). Fine so far; worth a look if idle CPU with many uTP peers shows it.
  - `PeerInfo` has no send / receive delay fields (libtorrent `utp_stream::send_delay` / `recv_delay`); the socket tracks them.
  - the shutdown does not wait for uTP FINs to be acknowledged (libtorrent drops them too, "uTP connections cannot be closed gracefully" once the UDP socket closes).
  - ~~the Phase 5 Preferences page maps µTorrent's `bt.transp_disposition` bits~~: done (2026-10-17_155028_1792245028): the BitTorrent page's uTP box sets both uTP directions, Advanced has `bt.transp_disposition` and the `utp_*` tuning.
  - `lt-swarm interop` has not run with uTP against qBittorrent / Transmission / Deluge yet (WSL2 is down on this machine, see Next steps 4); qBittorrent alone can be tried with TCP disabled on our side.
- MSE/PE follow-ups (CHANGELOG 2026-10-16_163419_1792161259):
  - ~~the Phase 5 Preferences page maps µTorrent's encryption settings~~: done (2026-10-17_155028_1792245028) as noted under Phase 4; `allowed_enc_level` and `prefer_rc4` are on the Advanced page.
  - the DHT and the trackers do not yet learn `pe_support` for peers (libtorrent takes it from ut_pex only, as we do); nothing is persisted about it.
  - `peer_connection::fast_reconnect` exists only for this alternation; a general reconnect policy (libtorrent's `min_reconnect_time * failcount` after any close) is still the stricter `min_reconnect_time * (failcount + 1)` here.
  - interop with Transmission and Deluge under `--encryption forced` is untested (WSL2 was down on 2026-10-16); rTorrent and µTorrent 2.2.1 as before.
- DHT follow-ups (Phase 4 DHT is in, CHANGELOG 2026-10-15_211826_1792091906):
  - BEP 44 items (`get` / `put`, ed25519) and BEP 51 `sample_infohashes` are Phase 8; until then such queries get a `find_node`-style answer. Not ported yet: the matching `test_dht` tests (put/get/mutable/immutable, signing_test1-3, sample_infohashes), `test_direct_dht` and the item parts of `test_dht_storage`.
  - ~~Linux: `[::]` is dual-stack, so the IPv6 DHT socket fails to bind after `0.0.0.0` and the DHT runs IPv4 only.~~ The DHT now runs on the engine's UDP listen sockets, opened with `IPV6_V6ONLY` through `net::sys` (2026-10-16_220720_1792181240), so both families run on Linux too. The TCP listener still has the dual-stack limitation (IPv6 is served by the v4 socket there).
  - the default bootstrap list is libtorrent's (`dht.libtorrent.org:25401` only). qBittorrent adds `router.bittorrent.com:6881`, `router.utorrent.com:6881`, `dht.transmissionbt.com:6881`; worth it if that one host is ever down. A deliberate settings change, so it goes in docs/SETTINGS.md.
  - ~~the GUI has no DHT preferences~~: "Enable DHT Network" on the BitTorrent page, the rest on Advanced (2026-10-17_155028_1792245028). Turning DHT off or on through `apply_settings` at run time was not checked against a live DHT.
  - DHT nodes in `.torrent` files (`nodes`) are not added yet; magnet `dht=` nodes are (IP literals only, host names are skipped).
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
  - Preferences is in (2026-10-17_155028_1792245028) but has not been clicked through by hand yet (see the Preferences follow-ups). Create New Torrent is in (2026-10-17_140012_1792238412) but has not been clicked through by hand yet: Add directory, Save as, progress, Cancel, Start seeding.
- PEX follow-ups (CHANGELOG 2026-10-15_221714_1792095434):
  - flags we send: `seed`, `e` (encryption) and `utp` (the peer is connected over uTP). The holepunch bit follows BEP 55. Received `e` sets the peer's `pe_support`, `utp` its `supports_utp`; `holepunch` is dropped.
  - ~~the GUI has no PEX switch~~: "Enable Peer Exchange" sets / clears `TorrentFlags::DISABLE_PEX` on every torrent and on new ones (2026-10-17_155028_1792245028).
  - `lt-swarm interop` runs the other clients with PeX off; a PEX interop case (three clients in one swarm) would check that they accept our messages. In the real Debian swarm no peer dropped us after our PEX messages.
- Port-mapping follow-ups (CHANGELOG 2026-10-16_155608_1792158968):
  - the routing table is read on Windows, Linux, FreeBSD and OpenBSD. Elsewhere (macOS, NetBSD, DragonFly) `net::sys::routes` is `Unsupported`: NAT-PMP stays off and UPnP runs on every multicast-capable interface. The FreeBSD `rt_msghdr` layout assumes 64-bit `long`; the `routes_are_listed` unit test checks it in CI (FreeBSD runs on main and weekly). OpenBSD is not in CI.
  - UPnP device URLs with a host name instead of an IP literal disable that device (libtorrent resolves them). Not seen on any router so far.
  - ~~the GUI has no UPnP / NAT-PMP preferences~~: on the Connection page (2026-10-17_155028_1792245028). The status bar's network indicator is still text; µTorrent's turns green once a port is mapped, a natural use of the state icons once they are integrated.
  - `patorrent-nox` does not print the alerts of the shutdown (the mapping deletions): `Session::shutdown` joins the engine thread before the last alerts are popped. The deletions were verified by asking the router instead.
  - with the mappers on, the swarm tests, `lt-swarm` and `interop` would forward loopback sessions' ports on the real router; they switch both off (`enable_upnp: false`, `enable_natpmp: false`).
- LSD follow-ups (CHANGELOG 2026-10-15_234806_1792100886):
  - a client on the same machine is announced and found on every local interface (14 here: LAN, Hyper-V, WSL, VirtualBox, loopback, link-local IPv6), so its peer list holds 14 addresses for one peer. The duplicate peer-id rule keeps one connection, but the others are retried at the normal pacing and closed again at the handshake. libtorrent does the same; a "same peer-id seen at another address" mark in the peer list would stop the retries.
  - only the v1 (or truncated v2) info-hash is announced; libtorrent also announces the truncated v2 hash of hybrid torrents (Phase 8).
  - `active_lsd_limit` (and the DHT / tracker limits) is not enforced: every running torrent announces.
  - Linux and FreeBSD are covered by CI only (the swarm test multicasts on loopback); not tried on a real LAN with another machine. Linux `lo` has no `IFF_MULTICAST`; if the CI swarm test fails there, the loopback join needs a look.
  - ~~the GUI has no LSD switch~~: "Enable Local Peer Discovery" on the BitTorrent page (2026-10-17_155028_1792245028).
- ~~Connection slots: DHT peers never connected in the rate-limited Debian run~~. Fixed 2026-10-15 (CHANGELOG 2026-10-15_230633_1792098393): the bandwidth tick served read-blocked peers in slot order with all the quota, so new peers never read their handshake. DHT decoding was fine, and counting pending connects against `connections_limit` matches libtorrent. Left over:
  - peer turnover (libtorrent `peer_turnover` / `peer_turnover_cutoff`: at 90 % of the limit, drop the slowest 4 % every unchoke interval) is not ported. With 150 of 200 slots connected it isn't needed yet, but a full limit then stays full of whoever got in first.
  - a readable peer still reads all the quota left between ticks (libtorrent queues it once the channel runs short, `need_queueing`). Fine as long as the tick's even split keeps every waiting peer moving.
- Magnets (ut_metadata) follow-ups:
  - partly fetched metadata is dropped when the torrent stops; it starts over on the next start (metadata is small, so this costs little).
  - a magnet's `so=` "don't download the rest" (DEFAULT_DONT_DOWNLOAD) is not saved; after a restart before the metadata arrives, unlisted files default to normal priority.
  - `TorrentSource::Resume` without an embedded info dictionary is still rejected; it could take the magnet path.
  - a name the user gave a magnet is kept only until a restart before the metadata arrives; then the torrent's real name is used.
  - the GUI shows a magnet's size as 0 B until the metadata arrives; µTorrent leaves the cell empty.
- Phase 2 follow-ups: batched UDP scrapes, block buffer pool, timer wheel if the tick shows up in profiles, the full session counter table. (The string-keyed settings table is `patorrent_frontend::settings`, 2026-10-17_155028_1792245028.)
- ~~Flaky on CI: `engine::upnp::tests::maps_and_unmaps_on_a_loopback_igd`, `dht::routing_table::tests::routing_table_v4`, `swarm::partial_pieces_survive_a_restart`~~. Fixed 2026-10-16_203336_1792175616: the fake IGD's threads are joined when the guard drops; the bucket count accepts 13 (random ids); the stingy peer skips the short last piece and counts real bytes (CHANGELOG 2026-10-16_203336_1792175616). Linux CI jobs are pinned to `ubuntu-24.04`; bump the pin on purpose, not by `ubuntu-latest` moving.
- ~~Flaky: `dht::node::tests::scrape_v6` (seen on FreeBSD, about 1 run in 300 anywhere)~~. Fixed 2026-10-17_165701_1792249021: the Bloom filter size estimate of 100 random addresses missed the +-3 tolerance (libtorrent's test has the same flake); the announcing addresses are now a fixed scattered sequence.
- ~~Flaky on CI: `swarm::download_through_an_http_tracker`~~. Fixed 2026-10-15: the test's `wait()` dropped the alerts behind a match, so `TorrentFinished` was lost when it arrived in the same batch as `TrackerReply`. Not an engine stall (CHANGELOG 2026-10-15_153331).
- TLS (`core::net::tls`):
  - certificate revocation is not checked; SChannel would fetch CRLs synchronously on the engine thread. SChannel may still fetch a missing intermediate certificate (AIA) synchronously; move the handshake off the reactor if that ever shows up as a stall.
  - no macOS backend (Secure Transport / Network.framework) yet; non-Windows, non-Unix targets fail HTTPS announces with a clear message.
  - web seeds over HTTPS go through `TlsStream`; no loopback HTTPS test yet (needs a trusted test certificate).
- `lt-swarm interop` is verified only on Windows (the qBittorrent app plus WSL). On Linux it runs `transmission-cli` and `deluged` directly and expects `qbittorrent-nox` via `--qbittorrent`; untested. rTorrent and µTorrent 2.2.1 are not in it yet (Continuous: interop matrix).
- Peers that only connect and get dropped (seed to seed) are not logged; a `peer_disconnected`-style alert category would help debugging swarms. `remove_peer` already has the close reason and throws it away; connect failures in `connect_peers_for` are not reported either.
- libtorrent tests not ported yet because they need later pieces:
  - `create_torrent`: mutable_torrents and the v2 / hybrid cases (v2_only, hybrid*, piece_layer, pieces_root_empty_file, v2_path_conflict, make_magnet_uri v2 / hybrid) wait for v2 creation (Phase 8); the v1 parts are ported
  - session plus resume data (Phase 2): the write_torrent_file round trips, preserve_* magnet tests
- Windows `SegmentHeap` (manifest `heapType`) might lower RAM use. Benchmark it with `lt-swarm` before enabling.
- CI minutes: the repo is private, and Windows runners bill at 2×. FreeBSD runs only on main and weekly for that reason.
- The vcpkg cache from the abandoned approach (about 2 GB under `%LOCALAPPDATA%\vcpkg`) can be deleted by the user if no other project uses it.
