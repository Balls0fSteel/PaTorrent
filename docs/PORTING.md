# Porting libtorrent-rasterbar to Rust

PaTorrent's engine (`crates/core`) is a Rust port of **libtorrent-rasterbar 2.0.11** (git tag `v2.0.11`, the current stable release). libtorrent is the *reference*, not a dependency: nothing from it is compiled or linked into PaTorrent. We read its code, port the behaviour, and port its tests.

## Licensing and attribution

- libtorrent is BSD-3-Clause, © 2003-2020 Arvid Norberg and contributors. Ported code is a derivative work, so it **must** keep the copyright notice and license text. These go in `THIRD-PARTY-NOTICES.md` and ship with every binary (an About box link and a text file in release archives).
- Bundled third-party code inside libtorrent that we port carries its own terms:

  | Code | Origin | License |
  |---|---|---|
  | `src/sha1.cpp` | Steve Reid | public domain |
  | `src/sha256.cpp` | LibTomCrypt | public domain |
  | `src/pe_crypto.cpp` (`rc4_init`, `rc4_encrypt`) | LibTomCrypt | public domain |
  | `src/ed25519/*` | Orson Peters | zlib. Altered versions must be marked as altered. |
  | `src/puff.cpp` | Mark Adler | zlib-style |
- Every ported Rust file starts with a provenance header:

  ```rust
  // Ported from libtorrent-rasterbar 2.0.11: src/piece_picker.cpp, include/libtorrent/piece_picker.hpp
  // Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
  // Changes: <one line: what is structurally different from the original>
  ```
- PaTorrent's own code is under the PolyForm Noncommercial License 1.0.0 (LICENSE.md, PLAN.md D15). BSD-3 allows that for derived code as long as libtorrent's notice and license text stay with it.
- µTorrent is proprietary. We re-create its *look and behaviour* from observation only. We use none of its binaries, icons, bitmaps, strings or trademarks, and do not reference its name inside the program.

## Porting rules

1. **Port behaviour, not C++ structure.** Keep algorithms, constants, timeouts, limits, edge cases and wire formats exactly. Keep libtorrent's function and field names where practical, so `grep` works across both codebases.
2. **Asio callbacks become sans-IO state machines plus reactor events** (see ARCHITECTURE.md). A chain like `async_read → on_receive → async_write` becomes `feed(bytes) -> actions`.
3. **`shared_ptr`/`weak_ptr` graphs become slab ids.** `boost::multi_index`, `std::map` webs and intrusive lists become `Vec`, `HashMap`, sorted vectors, or index-linked lists, chosen per access pattern and documented.
4. **`error_code` and exceptions become `Result<T, Error>`.** One `Error` enum covers the libtorrent error categories we keep (`errors::`, bdecode, http, gzip, upnp, socks, system); i2p errors are dropped.
5. **`TORRENT_ASSERT` becomes `debug_assert!`.** `INVARIANT_CHECK` becomes a `#[cfg(debug_assertions)] fn check_invariant()`.
6. **Port the tests with the code.** A module is not done until its libtorrent tests are ported and passing. Fixtures from `test/test_torrents/` (103 files) are copied to `crates/core/tests/fixtures/` with the BSD notice.
7. **Unsafe code** is allowed only in the platform layer, SIMD hashing and OS FFI. Every block gets a `// SAFETY:` comment, and the crate is built with `#![deny(unsafe_op_in_unsafe_fn)]`.
8. **Every parser gets a fuzz target** (bdecode, torrent_info, magnet, http_parser, tracker responses, DHT messages, peer messages, uTP headers, XML).
9. **Settings.** We keep libtorrent's `settings_pack` names and defaults unless a change is deliberate. Deliberate changes are listed in `docs/SETTINGS.md`, to be written in Phase 2.
10. **Divergences** from libtorrent behaviour are recorded in this file under "Deliberate divergences".

## Module map

Line counts are libtorrent `.cpp` lines in 2.0.11; headers add roughly 50 % on top. Source total: 165 files, about 105k lines.

| Rust module | libtorrent sources | ~Lines | Phase |
|---|---|---:|:---:|
| `core::bencode` | `bdecode.cpp` (zero-copy token array), `entry.cpp`, `bencode.hpp` | 2.4k | 1 |
| `core::util` | `hex`, `escape_string`, `string_util`, `utf8`, `parse_url`, `bitfield`, `time`, `random`, `ffs`, `bloom_filter`, `stack_allocator`, `error_code`, `sliding_average.hpp` | 2.8k | 1 |
| `core::crypto::hash` | `sha1`, `sha256`, `hasher`, `sha1_hash`, `crc32c`, `cpuid` (+ SHA-NI / ARMv8 paths, new) | 1.3k | 1 |
| `core::metainfo` | `torrent_info`, `file_storage`, `magnet_uri`, `load_torrent`, `add_torrent_params`, `resolve_links`, `info_hash.hpp` | 4.5k | 1 |
| `core::store` / `core::resume` | `read_resume_data`, `write_resume_data` (+ resume.db and metadata.pack, new) | 1.0k | 2 |
| `core::net` | `udp_socket`, `socket_type`, `instantiate_connection`, `socket_io`, `resolver`, `listen_socket_handle`, `ip_helpers` | 2.1k | 2 |
| `core::net::tls` | `ssl.cpp`, replaced by OS TLS glue | 0.2k | 2 |
| `core::http` | `http_connection`, `http_parser`, `gzip`, `puff` | 2.7k | 2 |
| `core::tracker` | `tracker_manager`, `http_tracker_connection`, `udp_tracker_connection`, `announce_entry` | 2.3k | 2 |
| `core::disk` | `mmap_disk_io` (job, fence and thread structure), `posix_storage` / `posix_disk_io` (I/O), `disk_buffer_pool`, `disk_io_thread_pool`, `disk_job_fence`, `disk_job_pool`, `storage_utils`, `file`, `file_view_pool`, `path`, `copy_file`, `truncate`, `directory`, `drive_info`, `stat_cache`, `store_buffer.hpp` | 9.2k | 2 |
| `core::peer` | `peer_connection`, `bt_peer_connection`, `receive_buffer`, `chained_buffer`, `peer_info`, `identify_client`, `fingerprint`, `generate_peer_id`, `peer_connection_handle` | 12.3k | 2 |
| `core::torrent` | `torrent`, `torrent_handle`, `torrent_status`, `file_progress` | 13.6k | 2 |
| `core::torrent::{picker, peer_list, choker}` | `piece_picker`, `peer_list`, `torrent_peer`, `torrent_peer_allocator`, `choker`, `request_blocks` | 6.4k | 2 |
| `core::session` | `session_impl`, `session`, `session_handle`, `session_params`, `settings_pack`, `session_settings`, `alert`, `alert_manager`, `performance_counters`, `session_stats`, `close_reason` | 14.9k | 2 |
| `core::bandwidth` | `bandwidth_manager`, `bandwidth_limit`, `bandwidth_queue_entry`, `peer_class`, `peer_class_set`, `stat` | 0.7k | 2 / 5 |
| `core::peer::ext` | `ut_metadata`, `ut_pex`, `smart_ban` (+ `lt_donthave` from `bt_peer_connection`) | 1.6k | 4 |
| `core::crypto::mse` | `pe_crypto` (RC4, DH-768 on a small fixed-width bigint, new) | 0.4k | 4 |
| `core::utp` | `utp_stream`, `utp_socket_manager`, `packet_buffer`, `timestamp_history` | 4.2k | 4 |
| `core::dht` | `kademlia/*`: node, routing_table, rpc_manager, traversal_algorithm, get_peers, find_data, refresh, put_data, get_item, item, dht_storage, dos_blocker, node_id, dht_state, msg, sample_infohashes, dht_tracker | 7.3k | 4 |
| `core::engine::{upnp, natpmp, portmap, lsd}`, `core::proto::{upnp, natpmp, lsd, xml}` | `upnp`, `natpmp`, `lsd`, `xml_parse`, the mapper glue of `session_impl` | 3.1k | 4 |
| `core::net::env` | `enum_net`, `ip_notifier`, `ip_voter` | 2.2k | 4 |
| `core::net::proxy` | `socks5_stream`, `http_stream.hpp`, `proxy_base`, `proxy_settings` | 1.0k | 5 |
| `core::net::ip_filter`, `core::engine::ip_filter` | `ip_filter` (+ `ipfilter.dat` and P2P block-list parser, new) | 0.3k | 5 |
| `core::peer::web` | `web_peer_connection`, `http_seed_connection`, `web_connection_base` | 2.0k | 5 |
| `core::disk::part_file` | `part_file`, `posix_part_file` | 0.9k | 5 |
| `core::metainfo::create_torrent` | `create_torrent` | 1.0k | 5 |
| `core::crypto::ed25519` | `ed25519/*`, `kademlia/ed25519` | 3.7k | 8 |
| `core::metainfo::v2` | `merkle`, `merkle_tree`, `hash_picker` | 2.0k | 8 |

### Not ported

| libtorrent piece | Reason |
|---|---|
| `mmap_disk_io` I/O path, `mmap_storage`, `mmap.cpp` | replaced by the pread backend (see ARCHITECTURE.md) |
| `posix_disk_io` threading model | synchronous on the network thread; we use the disk pool |
| `disabled_disk_io`, `session_call`, `assert.cpp`, `platform_util`, `version` | test scaffolding, asio glue or trivial |
| `i2p_stream` and i2p PEX | out of scope; maybe never |
| SSL torrents (`TORRENT_SSL_PEERS`) | rarely used; deferred indefinitely |
| deprecated ABI (`TORRENT_ABI_VERSION == 1`), `session_status`, `cache_status` | dead weight |
| Python bindings, `simulation/` (libsimulator) | replaced by our sans-IO simulator |
| `share_mode` | rarely used; revisit after parity |

## Test porting map

Status after Phase 1: the Phase 1 row is ported, except tests that need `create_torrent` (Phase 5) or a running session (Phase 2); PLAN.md "Known TODOs" lists them.

libtorrent `test/` has about 150 files and about 50k lines. Ported alongside each module:

| Phase | libtorrent tests |
|---|---|
| 1 | `test_bdecode`, `test_bencoding`, `test_bitfield`, `test_bloom_filter`, `test_crc32`, `test_hasher`, `test_sha1_hash`, `test_string`, `test_utf8`, `test_utils`, `test_sliding_average`, `test_ffs`, `test_torrent_info` (+ `test_torrents/`), `test_file_storage`, `test_magnet`, `test_info_hash`, `test_resolve_links`, `test_primitives` |
| 2 | `test_http_parser`, `test_http_connection`, `test_gzip`, `test_tracker`, `udp_tracker`, `test_receive_buffer`, `test_piece_picker`, `test_peer_list`, `test_peer_priority`, `test_fast_extension`, `test_storage`, `test_store_buffer`, `test_fence`, `test_file`, `test_file_progress`, `test_checking`, `test_recheck`, `test_read_piece`, `test_read_resume`, `test_resume`, `test_bandwidth_limiter`, `test_auto_unchoke`, `test_priority`, `test_remove_torrent`, `test_add_torrent`, `test_torrent`, `test_transfer`, `test_settings_pack`, `test_session`, `test_alert_manager`, `test_identify_client`, `test_generate_peer_id`, `test_torrent_list`, `test_time_critical`, `test_similar_torrent` |
| 4 | `test_dht`, `test_dht_storage`, `test_direct_dht`, `test_dos_blocker`, `test_utp`, `test_packet_buffer`, `test_timestamp_history`, `test_pe_crypto`, `test_lsd`, `test_natpmp`, `test_upnp`, `test_xml`, `test_enum_net`, `test_ip_voter`, `test_listen_socket`, `test_privacy` |
| 5 | `test_ip_filter`, `test_web_seed*`, `test_url_seed`, `test_part_file`, `test_create_torrent`, `test_peer_classes`, `test_copy_file`, `test_truncate`, `test_remap_files`, `test_apply_pad` |
| 8 | `test_merkle`, `test_merkle_tree`, `test_hash_picker`, `test_ed25519`, `test_hasher512` |

## Deliberate divergences

Record each one here with the reason.

| Area | libtorrent 2.0.11 | PaTorrent | Why |
|---|---|---|---|
| Disk backend | mmap by default; `posix_disk_io` runs I/O on the network thread | pread/pwrite on the disk thread pool. Blocks are written through and hashed in order as they arrive (a small reorder cache holds early blocks), so finished pieces are never read back | memory accounting and performance on Windows and network drives; scale |
| Persistence | left to the client (qBittorrent: two files per torrent) | `core::store`: one resume DB + one metadata pack | startup time and file count at 10k+ torrents |
| Inactive torrents | `torrent_info` (incl. piece hashes) stays in memory | info dict loaded lazily on start, dropped when stopped | per-torrent RAM at scale |
| Ownership | `shared_ptr` / `weak_ptr` graph | slab + generational ids | no refcount traffic, no lifetime cycles, Rust-friendly |
| Path separator (Phase 1) | native separator (`\` on Windows) inside `file_storage` | always `/` internally; converted at the disk layer | identical behaviour, tests and resume data on every OS. Windows-only filename rules still apply only on Windows. |
| bdecode lookups (Phase 1) | `bdecode_node` caches the last `list_at` / `dict_at` position in mutable fields | `Node` is a plain `Copy` handle; loops use `list_iter` / `dict_iter` | no interior mutability; O(1) per step either way |
| bdecode partial trees (Phase 1) | after a parse error, unwound containers' `next_item` points at their own end token | it points past the end token, as for normally closed containers | libtorrent's `has_soft_error` reads out of bounds on such trees (found by our mutation tests) |
| v1 info-hash (Phase 1) | always computes SHA-256 of the info section, then discards it for v1 torrents | SHA-256 only when `meta version` is 2 | about a third faster metadata parsing; same results |
| `file_storage` paths (Phase 1) | linear search of the directory table per file | most-recent check plus hash index | avoids quadratic loading of hostile torrents with many directories |
| Hybrid without v1 pieces (Phase 1) | a `meta version 2` torrent with a v1 file list but no `pieces` is accepted, claims a v1 hash, and every v1 hash check later fails | rejected with `torrent_missing_pieces` | a torrent that can never verify v1 data should not load (found by our mutation tests) |
| Magnet btih (Phase 1) | a 40-character btih with non-hex characters is silently accepted (garbage hash) | rejected with `invalid_info_hash` | correctness |
| Disk job routing (Phase 2) | jobs go to any thread of the pool; partial-piece hash state is shared under locks | every job for a piece runs on thread `piece % threads`, so the hash state needs no lock; fences are broadcast jobs whose last finisher runs the action | simpler, lock-free hashing |
| Checking queue (Phase 2) | `active_checking` torrents check at once, the rest queue in the session | checks run one at a time, FIFO, on a dedicated hashing thread | same effect with `active_checking = 1` (the default) |
| Resume verification (Phase 2) | compares file sizes and modification times | file sizes only (every file holding a claimed piece must reach that far) | mtimes differ across copy tools and file systems; sizes catch truncation |
| ut_metadata size (Phase 4) | every extension handshake with `metadata_size` resizes the download buffer, even after blocks arrived; a failed hash check keeps the size | a handshake size is taken only before the first block arrives, and the first data message's `total_size` replaces a wrong one; a failed hash check forgets the size | one peer announcing a wrong size can't wreck a download that is under way, and a bad size can't stick after a failure |
| ut_pex diff (Phase 4) | the diff remembers each peer's connection endpoint but advertises an incoming peer's listen port, so it is later "dropped" with its ephemeral port; past the 100-entry cap, the first skipped peer is remembered as sent | the diff tracks the advertised address; peers past the cap are not remembered and go out in a later diff | others drop the address they were given, and no peer is left out for good |
| ut_pex peers (Phase 4) | new PEX peers trigger `do_connect_boost` and a DHT ping; the plugin is decided once at torrent creation (a magnet that turns out private keeps it) | PEX peers wait for the regular connect pacing (`connection_speed`); no DHT ping; ut_pex stops as soon as the torrent is known to be private | keeps `connection_speed` meaningful; never leaks a private torrent's peers |
| Bandwidth limits (Phase 2) | each peer queues a `bw_request` with the `bandwidth_manager`; every tick each channel's quota is split over the queued requests by peer-class priority, with a `ttl` | token buckets (session and torrent) refilled every 100 ms; a peer reads or writes at once while quota lasts; peers that ran out wait, and each tick each gets an equal share of the session's and its torrent's quota (`quota_shares`, every priority 1, no peer classes or `ttl`) | far less machinery; the same even split, so a busy peer can't starve a new one of the bytes for its handshake |
| Unfinished pieces (Phase 2) | saved in resume data (`unfinished`) and re-hashed on load | not saved yet: partial pieces are downloaded again after a restart | small cost (at most one piece per peer); follow-up in PLAN.md |
| Piece picker (Phase 2) | pieces sit in a vector bucketed by priority and availability, moved on every HAVE | per-piece counters plus a pick order rebuilt lazily, at most every 500 ms while availability changes | far less code; ordering can lag by one rebuild, which only affects which of two equally rare pieces goes first |
| Announce policy (Phase 2) | `announce_to_all_tiers` = false | true (the libtorrent setting, with qBittorrent's and µTorrent's behaviour as default) | more peers on multi-tier public torrents |
| Seed choking (Phase 2) | `seed_choking_algorithm` = round robin | fastest upload (one of libtorrent's options) | simple and effective; round robin can follow with the full choker port |
| UDP trackers (Phase 2) | share the listen socket | one ephemeral UDP socket per address family | the listen socket's datagrams are demultiplexed for uTP and the DHT; trackers don't need it |
| DHT sockets (Phase 4) | one DHT node per listen socket, sharing the uTP/UDP listen socket | at most one node per address family, on the UDP listen socket beside the TCP listener (shared with uTP; v6-only on IPv6, so Linux runs both families) | one node per family is what a wildcard listen setup gives libtorrent too |
| DHT announce pacing (Phase 4) | `dht_announce_interval` is spread over every loaded torrent (stopped ones are skipped when their turn comes) | spread over running torrents only | with thousands of stopped torrents, libtorrent's running ones announce far less often than the interval |
| Listen port retry (Phase 4) | `max_retry_port_bind` tries the next port when the TCP or UDP bind fails; with port 0 the UDP bind failing fails the listen socket | the same pair retry; with port 0 a fresh ephemeral port is drawn up to three times, then the TCP listener is kept without its UDP socket | a session should not fail to start over a UDP port clash |
| uTP I/O model (Phase 4) | asio handlers: the application hands read and write buffers to `utp_stream` and is called back; `m_read_buffer` is filled directly | the socket keeps its own receive buffer and the application copies out of it; a write hands bytes in and keeps what the window did not take; sockets with news go on a ready list the reactor drains after each wakeup | no asio; the same packets on the wire |
| uTP packet buffers (Phase 4) | `packet_pool`: three slabs of recycled packet buffers | plain heap buffers | simpler; revisit if the allocator shows up in profiles |
| uTP close reason (Phase 4) | the `close_reason` extension (BEP 29 extension 3) carries libtorrent's `close_reason_t` | parsed and dropped, never sent | no close reasons in the engine yet |
| Mixed-mode throttling (Phase 4) | `mixed_mode_algorithm = peer_proportional`: TCP peers are rate-limited in proportion when uTP peers are connected | not ported (`prefer_tcp` behaviour: no throttling) | no peer classes yet; follow-up in PLAN.md |
| DHT items (Phase 4) | `get` / `put` (BEP 44) and `sample_infohashes` (BEP 51) are served from storage | answered like `find_node` (a `get` also gets a write token); nothing is stored | Phase 8 ports BEP 44 / 51 with ed25519 |
| DHT ICMP errors (Phase 4) | a "port unreachable" fails the outstanding request to that endpoint at once | ignored; the request times out | Windows reports these on `recv_from` without the address, Unix not at all on unconnected sockets |
| DHT external IP (Phase 4) | the listen socket's `ip_voter` gets votes from DHT replies, trackers and peers | the DHT's own voter per family, fed by DHT replies only; a change gives the node a new BEP 42 id | `core::net::env` (Phase 4) will share one voter |
| LSD sockets (Phase 4) | one LSD socket per listen socket, after a wildcard listen address is expanded into every local address; IPv6 joins the group on the address's scope id (0, "any interface", for global addresses) | the same expansion, then one socket per interface and family; the interface's other addresses only widen the subnet filter. IPv6 joins and sends on the interface's index | fewer sockets and duplicate packets on hosts with many addresses per interface; IPv6 announces leave on the right link |
| LSD resends and pacing (Phase 4) | one resend timer per LSD socket; `local_service_announce_interval` spread over every loaded torrent | one session-wide resend timer (the per-socket ones fired together anyway; a newer announce still replaces the older one's resends); the interval is spread over running torrents only, as for the DHT | same packets; running torrents keep announcing at a useful rate in big libraries |
| LSD interfaces (Phase 4) | the listen sockets (and so LSD) follow interface changes (`ip_notifier`) | interfaces are read once, when LSD starts | network-change handling is a separate Phase 4 item |
| Port mapping per interface (Phase 4) | one `natpmp` and one `upnp` per listen socket (a wildcard listen address is expanded into every interface address; NAT-PMP also on IPv6) | a wildcard IPv4 listen address is expanded into the up, non-loopback IPv4 interfaces that have a default route (every multicast-capable one where the routing table cannot be read: macOS, NetBSD, DragonFly); no IPv6 mappers yet | the same set in practice, without a mapper per Hyper-V / WSL / VirtualBox adapter; IPv6 belongs to the "IPv6 throughout" item |
| UPnP HTTP (Phase 4) | `http_connection` with the resolver (host names in device URLs), 5 redirects, optional TLS | a small non-blocking client of its own: IP-literal hosts only (a host name disables the device), no redirects, plain HTTP | routers announce IP literals; one less resolver client on the reactor |
| UPnP unmap result (Phase 4) | a `DeletePortMapping` reply with status 200 and no `<errorCode>` is reported with `error_code(-1, upnp_category())`, which is an error ("unknown UPnP error (-1)") | reported as a success (port 0) | `-1` is the parser's "no error code" marker, not an error |
| UPnP log (Phase 4) | `portmap_log_alert` for every step, including the full SOAP bodies | `Alert::PortmapLog` for the steps and the SOAP replies; the request bodies are logged only for SOAP calls | the same debugging value with less noise |
| smart_ban read-back (Phase 4) | `piece_failed` locks the piece in the picker and clears it on the disk thread (`async_clear_piece`) so the plugin's `force_copy` reads of the failed blocks beat the re-download; blocks are tied to a `torrent_peer*` | the reads are queued before the picker restores the piece, and every job for a piece runs in order on one disk thread, so they precede any new write; no lock, no clear job; blocks are tied to the peer's address | the same ordering guarantee from the disk design; addresses are what the peer list is keyed by |
| Parole picking (Phase 4) | the `on_parole` picker option (with `prioritize_partials`) restricts a peer to pieces it downloads exclusively, through the bucketed picker's `exclusive` flags | a peer on parole only takes partial pieces whose blocks are all its own or free, otherwise starts a new piece, and never requests busy blocks (no end-game) | the same effect on our lazily ordered picker |
| Peer ban alert (Phase 4) | `peer_ban_alert` is posted by `penalize_peers` only; a smart_ban ban just disconnects | `Alert::PeerBanned` for both, plus a log line with the reason | the GUI's logger should show why a peer vanished |
| Predictive pieces (Phase 4) | `predictive_piece_announce` announces pieces before their hash check and takes them back with DONT_HAVE on failure | not ported: pieces are announced after they pass; DONT_HAVE goes out on upload read errors and if a piece in our set ever fails | an optimisation for high-latency swarms; revisit with the choker port |
| Routing table (Phase 4) | netlink on Linux, `sysctl(NET_RT_DUMP)` on the BSDs and macOS, `GetIpForwardTable2` on Windows | `/proc/net/route` and `/proc/net/ipv6_route` on Linux, `sysctl` on FreeBSD and OpenBSD, `GetIpForwardTable2` on Windows; `Unsupported` elsewhere (NAT-PMP stays off there) | no netlink without libc; the files carry the same table |
| MSE xor mask (Phase 4) | `dh_key_exchange::compute_secret` hashes `HASH("req3", S)` over an `export_bits` buffer that is not right-aligned, so when the shared secret has a leading zero byte (one exchange in 256) the responder's mask is wrong and the incoming handshake fails with `invalid_info_hash`; the initiator side (`write_pe3_sync`) uses the padded `export_key` | both sides hash the fixed-width 96-byte `S` | the MSE specification's `S` is 96 bytes; libtorrent's own initiator sends it that way |
| MSE fast reconnect (Phase 4) | under `pe_enabled`, only the encrypted attempt sets `fast_reconnect`; a cleanly closed plaintext attempt is retried at once anyway, because `peer_list` waits `min_reconnect_time * failcount` and the failcount is 0 | both attempts set `fast_reconnect`, which clears the peer's connect time | our peer list waits `min_reconnect_time` even after a clean close (less hammering), so the alternation needs the flag both ways |
| MSE secret exponent (Phase 4) | `aux::random_bytes` (mt19937) | `crypto_random_bytes` (the OS RNG) | the key material should not come from a predictable generator |
| IP filter, mapped addresses (Phase 5) | an IPv4-mapped IPv6 address (`::ffff:a.b.c.d`) is looked up in the IPv6 table | looked up in the IPv4 table | an IPv4 block list also covers a peer that reached us through a dual-stack socket |
| IP filter, resume peers (Phase 5) | `add_peer` filters peers from resume data as they are loaded | they are loaded unfiltered and filtered (and forgotten, with `Alert::PeerBlocked`) when picked for a connection, as is any other peer | resume data is applied before the torrent joins the session; the check before every connect also covers anything that slipped in |
| Port filter and incoming peers (Phase 5) | `apply_port_filter` also disconnects incoming peers whose (ephemeral) remote port is in a blocked range | the port filter only applies to peers we would connect to; `PeerBlocked` for a closed connection carries the peer's port (libtorrent: 0) | the port filter is about listen ports; an incoming peer's source port says nothing |
| Persistence writes (Phase 2) | n/a (client side) | `resume.db` is rewritten atomically, debounced (2 s after adds, every 30 s for progress); the metadata pack is appended at once | bulk adds rewrite once instead of once per torrent. An append-only record log (ARCHITECTURE.md) follows if the rewrite shows up in profiles |
| Torrent creation (Phase 5) | hybrid v1 + v2 by default (`v1_only` / `v2_only` flags); hashing goes through the disk subsystem (`async_hash`, `hashing_threads`); `add_files` walks directories in `readdir` order | v1 only (`V2_ONLY` and `set_hash2` fail with a clear error; BEP 52 creation is a follow-up); the caller's thread reads the files in order and up to four threads hash; directory entries are added sorted by name; the progress callback can cancel; an optional info `source` key | v2 needs the merkle tree port (Phase 8); sequential reads suit spinning disks and network shares; sorted walks make the same folder give the same torrent on every OS |
