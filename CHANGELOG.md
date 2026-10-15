# Changelog

All notable changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (local time).

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
- Not yet verified: CI on GitHub (first run happens on this push), the Linux-only `rss_bytes` path, the MSRV 1.85 build, and FreeBSD.

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
