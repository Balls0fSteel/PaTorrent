# Changelog

All notable changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (local time).

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
