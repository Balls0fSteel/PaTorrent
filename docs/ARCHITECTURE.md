# PaTorrent architecture

Status: **in implementation**. Phases 0-2 are built as described here (one mio network thread, disk thread pool, resolver threads, slab ownership, store); PLAN.md tracks progress and open decisions, and docs/PORTING.md lists where the code departs from libtorrent.

## Goals, in priority order

1. **Loadability.** Thousands of torrents loaded at once, the UI stays instant, startup stays fast. This is the µTorrent 2.2.1 property we care about most.
2. **Low resource use.** Small binary, small idle RAM, near-zero idle CPU.
3. **Speed.** Saturate fast links from one network thread with modest CPU.
4. **Monolithic binaries.** Each frontend ships as one self-contained executable with no runtime dependencies beyond the OS (plus GTK on Linux/BSD).
5. **Portable engine.** Windows 10+, Linux, FreeBSD/OpenBSD/NetBSD. macOS falls out of the same code but is not a target.

## Shape

```
 ┌──────────────────┐  ┌──────────────────┐  ┌───────────────────────────┐
 │ patorrent     │  │ patorrent-gtk │  │ patorrent-nox          │
 │ Win32 GUI        │  │ GTK GUI          │  │ headless daemon           │
 │ (windows-sys)    │  │ (Linux / BSD)    │  │ HTTP API + Web UI         │
 └────────┬─────────┘  └────────┬─────────┘  └─────────────┬─────────────┘
          └──────────── frontend (shared, UI-agnostic) ─────┘
                 settings · torrent list model · labels · formatting · RSS · scheduler
                                    │
                SessionHandle (commands)   ▲ alerts + changed-torrent status batches
                                    ▼      │
 ┌──────────────────────────────────────────────────────────────────────────┐
 │ core: Rust port of libtorrent-rasterbar 2.0.11                           │
 │                                                                          │
 │  network thread ─ mio reactor, timers, session, torrents, peers,         │
 │                   trackers, DHT, uTP, LSD, UPnP / NAT-PMP                │
 │  disk pool      ─ positional file I/O, write cache, piece hashing        │
 │  resolver       ─ getaddrinfo on a worker, with cache                    │
 └──────────────────────────────────────────────────────────────────────────┘
```

Crates are compile-time and test boundaries only. Each executable statically links everything it uses.

## Workspace layout (target)

```
crates/
  core/       patorrent-core      engine library (the libtorrent port)
  frontend/   patorrent-frontend  UI-agnostic client logic shared by all frontends
  win32/      patorrent           Windows GUI executable
  nox/        patorrent-nox       headless daemon (starts life as the dev/test CLI)
  gtk/        patorrent-gtk       Linux/BSD GUI executable
tools/
  swarm/      loopback swarm, interop and scale benchmarks
third_party/
  libtorrent/ git submodule pinned to tag v2.0.11, reference only, never built
```

`core` starts as one crate with strict module layering. It gets split into several crates (for example `bencode`, `crypto`, `proto`, `core`) only if compile times or layering discipline call for it.

## Threads

The thread model mirrors libtorrent's, because that is what lets libtorrent scale:

| Thread | Owns | libtorrent equivalent |
|---|---|---|
| Network (exactly one) | `Session`: all torrents, peer connections, trackers, DHT, uTP, NAT-PMP/UPnP/LSD, timers, bandwidth manager | the `io_context` thread running `session_impl` |
| Disk pool (N, default 4) | file I/O, write-back cache flushes, piece hashing, file checking, move storage | `disk_io_thread_pool` + hasher threads |
| Resolver (1–2) | blocking `getaddrinfo`, results cached | asio's resolver thread + `aux::resolver` |
| Frontend (GUI / HTTP) | the UI or the API server | the client application |

- There are no locks on torrent or peer state. Only the network thread touches it.
- Disk jobs go out over a channel. Completions come back through another channel plus a `mio::Waker`.
- Frontends never call into engine internals. They post commands through `SessionHandle`, a queue plus a waker, and drain alerts from a bounded queue. A notify callback wakes the frontend, the same pattern as libtorrent's `set_alert_notify`: Win32 uses `PostMessage`, GTK uses a GLib source, nox uses its own reactor.

## Sans-IO protocol cores

Protocol logic is written as plain state machines that never touch sockets, clocks or threads. Bytes and events go in; actions come out ("send these bytes", "arm timer", "request block", "write block to disk"). This covers:

- bencode
- peer wire protocol and extensions
- the MSE/PE handshake
- uTP / LEDBAT
- DHT messages and traversals
- HTTP and UDP tracker exchanges
- the HTTP parser
- piece picker, peer list, choker

The reactor layer owns the real sockets and timers. This buys:

- unit tests without a network
- deterministic whole-swarm simulation with a virtual clock and virtual network, replacing libtorrent's `simulation/` + libsimulator
- fuzzing of every parser
- an easy swap of the I/O backend later (io_uring, native IOCP) without touching protocol code

## Ownership model

libtorrent's web of `shared_ptr`/`weak_ptr` becomes slabs with generational indices:

- `TorrentId`, `PeerId` and `ConnId` are `Copy` handles. A stale handle is detected, never dereferenced.
- The session owns `Slab<Torrent>` and `Slab<PeerConnection>`. Torrents refer to peers by id and vice versa.
- libtorrent's per-state torrent lists (`torrent_want_tick`, `torrent_want_peers_download`, `torrent_want_peers_finished`, `torrent_want_scrape`, `torrent_downloading_auto_managed`, `torrent_seeding_auto_managed`, `torrent_checking_auto_managed`) become intrusive index lists. Per-tick work is then proportional to *active* torrents, not *loaded* torrents. This is the single most important property for loadability.

## Scale design: where cost comes from and how we avoid it

| Cost driver at 10k+ torrents | Design |
|---|---|
| Per-torrent memory while idle | Target ≈1–2 KB per stopped torrent, plus its name. Loaded state for a stopped or idle-seeding torrent is name, save path, sizes, stats, flags and queue position. The info dict and piece hashes are **not** kept for inactive torrents; they load lazily from the metadata pack on start. The piece picker exists only while downloading (as in libtorrent). Peer lists are dropped or capped when stopped. No per-torrent timers: one session tick walks only the want-tick list. |
| Startup time | One resume database (bencoded records, like µTorrent's `resume.dat`) plus one append-only metadata pack. That's 2 files to read instead of 2 files per torrent (qBittorrent's `BT_backup` is 20,000 files for 10,000 torrents). No rechecks when resume data is consistent. Torrents are restored without touching their data files; files open on demand. |
| Status traffic to the UI | Only changed torrents cross the thread boundary, like libtorrent's `post_torrent_updates`. Frontends keep a row cache and re-render only visible rows. nox serves diffs (`rid`/`cid`-style incremental updates, as both the qBittorrent and µTorrent Web APIs do). |
| GUI list | Virtual list views. Win32 uses `LVS_OWNERDATA`; GTK uses `GtkColumnView` over a `GListModel`. No per-row objects; sort keys cached per row; 1 Hz refresh of visible rows only. |
| Tracker load | Per-tracker-host UDP connection-id cache. Announce jitter to avoid thundering herds. Batched UDP scrapes (up to ~74 info-hashes per packet). Announces only for started torrents. |
| DHT load | Announce pacing spread across torrents (libtorrent's `dht_announce_interval` logic). The read-only/announce budget scales with active torrents, not loaded ones. |
| File handles | A bounded LRU handle pool (libtorrent `file_view_pool`). A few hundred OS handles regardless of torrent count. |
| Disk I/O | Positional `pread`/`pwrite` (Windows: `ReadFile`/`WriteFile` with explicit offsets), our own write-back cache and read-ahead, **no mmap**. libtorrent 2.0's default mmap backend is behind the well-known "high RAM use / slow on network drives" reports in libtorrent-2.0-based clients, and libtorrent 2.1 added a pread backend for that reason. We keep 2.0's job, fence and thread-pool structure (`mmap_disk_io.cpp`, `disk_job_fence.cpp`, `disk_io_thread_pool.cpp`) with `posix_storage`'s I/O moved onto the pool; 2.0's `posix_disk_io` runs I/O synchronously on the network thread. Pieces hash incrementally as in-order blocks arrive, so completed pieces are not re-read. |
| Hot-path allocation | Pooled 16 KiB block buffers (`disk_buffer_pool`), per-connection receive buffers sized to need, no heap allocation per protocol message, `Bytes`-like shared send buffers (`chained_buffer`). |

### Performance targets

To be validated by `tools/swarm` on a reference machine and tracked per release. Numbers will be revised once we have measurements.

| Metric | Target |
|---|---|
| Win32 GUI executable, release | ≤ 4 MB |
| RSS, empty session, GUI open | ≤ 12 MB |
| RSS, 10,000 torrents loaded (stopped / idle seeding) | ≤ 150 MB |
| Cold start to interactive UI with 10,000 torrents | ≤ 2 s |
| Idle CPU with 10,000 torrents loaded | < 1 % of one core |
| Single-torrent download over loopback swarm | saturates 1 Gbit/s using < 25 % of one core |

## Platform layer

| Concern | Windows | Linux / BSD |
|---|---|---|
| Event loop | mio (IOCP / AFD) | mio (epoll / kqueue) |
| File I/O | `ReadFile`/`WriteFile` with offsets; sparse files (`FSCTL_SET_SPARSE`); optional preallocation | `pread`/`pwrite`; `fallocate`/`posix_fallocate`; sparse by default |
| Interfaces & routes (`enum_net.cpp`) | `GetAdaptersAddresses`, `GetIpForwardTable2` | `getifaddrs`; netlink (Linux), routing sysctl (BSD) |
| Network change notification (`ip_notifier.cpp`) | `NotifyIpInterfaceChange` | netlink / `PF_ROUTE` socket |
| TLS (HTTPS trackers and web seeds) | SChannel (OS) | system OpenSSL / LibreSSL (both in `core::net::tls`, PLAN.md D9) |
| Randomness | `ProcessPrng` / `BCryptGenRandom` | `getrandom` / `arc4random` |

## Dependency policy

- **Engine (`core`).** `std` + `mio` + OS bindings (`windows-sys` / `libc`, which mio already pulls in), plus OS-provided TLS. Everything else is ported from libtorrent or written here:
  - bencode
  - SHA-1 and SHA-256: portable versions plus SHA-NI / ARMv8 SHA via `std::arch`, detected at runtime
  - SHA-512 and ed25519 (the ref10 code libtorrent bundles)
  - RC4 and 768-bit Diffie-Hellman for MSE
  - CRC32C, using SSE4.2 when available
  - inflate (`puff`)
  - the UPnP XML scanner
  - the HTTP/1.1 client and parser
  - URL parsing
  - the DNS cache

  TLS is the one thing we never implement ourselves.
- **Win32 GUI.** `windows-sys` only. Raw Win32 and common controls, the same thing µTorrent was built on.
- **GTK GUI.** gtk-rs (see open question Q5).
- **nox.** Nothing extra: own minimal HTTP/1.1 server on the core reactor; Web UI static files embedded with `include_bytes!`.
- Any new dependency needs a written justification in PLAN.md and a CHANGELOG entry.

## Engine API sketch

This shape mirrors libtorrent's `session_handle` / `torrent_handle` / alerts, so ported code and docs map one to one. It drops the deprecated surface.

```rust
let session = Session::start(SessionParams { settings, dht_state, store: StoreConfig { dir } })?;
let h = session.handle();                       // Clone + Send, usable from any thread

h.set_alert_notify(move || wake_frontend());    // called on the network thread; must be cheap
h.add_torrent(AddTorrentParams { source, save_path, flags, ..Default::default() });
//   -> Alert::TorrentAdded { id, .. } | Alert::AddTorrentFailed { .. }
h.torrent(id).pause();  h.torrent(id).resume();  h.torrent(id).queue_up();
h.torrent(id).set_file_priorities(&prios);  h.torrent(id).force_recheck();
h.post_torrent_updates();                       // -> Alert::StateUpdate(Vec<TorrentStatus>) (changed only)
h.post_session_stats();                         // -> Alert::SessionStats(Counters)
h.pop_alerts(&mut alerts);
```

## Persistence

- **Store** (`core::store`, optional but used by every frontend):
  - `resume.db`: one bencoded record per torrent. Records carry the libtorrent fastresume fields plus frontend metadata such as label and "paused vs stopped". It is written incrementally (dirty records appended, compacted when the garbage ratio exceeds a threshold), with atomic replace on compaction.
  - `metadata.pack`: append-only, indexed by info-hash, holding raw info dictionaries. Read lazily.
  - `dht_state.dat`: DHT node ids and the routing table's nodes (libtorrent `dht_state` format), written by the engine on shutdown and every 30 minutes, read at startup to bootstrap.
- **Frontend settings:** `settings.ini`, human-editable.
- **Locations:**
  - Windows: portable mode when `settings.ini` sits next to the executable (like µTorrent), otherwise `%APPDATA%\PaTorrent`.
  - Unix: `$XDG_CONFIG_HOME/patorrent` and `$XDG_DATA_HOME/patorrent`.

## Frontends

- **Win32** reproduces the µTorrent 2.2.1 main window:
  - menu and toolbar
  - category tree with counts
  - torrent list with an in-cell progress bar
  - detail tabs: General, Trackers, Peers, Pieces, Files, Speed, Logger
  - status bar showing DHT, D/U rates and totals
  - tray icon, single instance with command-line forwarding, per-monitor DPI v2, static CRT, portable mode

  All icons and artwork are original, drawn at runtime or embedded. No µTorrent assets, names or trademarks.
- **GTK** uses the same layout and the same `frontend` model.
- **nox** is a daemon with an HTTP API and a small dependency-free Web UI. It can also be the target of a "remote" connection from either GUI.
