# Porting libtorrent-rasterbar to Rust

LibreTorrent's engine (`crates/core`) is a Rust port of **libtorrent-rasterbar 2.0.11** (git tag `v2.0.11`, the current stable release). libtorrent is the *reference*, not a dependency: nothing from it is compiled or linked into LibreTorrent. We read its code, port the behaviour, and port its tests.

## Licensing and attribution

- libtorrent is BSD-3-Clause, © 2003-2020 Arvid Norberg and contributors. Ported code is a derivative work, so it **must** keep the copyright notice and license text. These go in `THIRD-PARTY-NOTICES.md` and ship with every binary (an About box link and a text file in release archives).
- Bundled third-party code inside libtorrent that we port carries its own terms:

  | Code | Origin | License |
  |---|---|---|
  | `src/sha1.cpp` | Steve Reid | public domain |
  | `src/sha256.cpp` | LibTomCrypt | public domain |
  | `src/ed25519/*` | Orson Peters | zlib. Altered versions must be marked as altered. |
  | `src/puff.cpp` | Mark Adler | zlib-style |
- Every ported Rust file starts with a provenance header:

  ```rust
  // Ported from libtorrent-rasterbar 2.0.11: src/piece_picker.cpp, include/libtorrent/piece_picker.hpp
  // Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
  // Changes: <one line: what is structurally different from the original>
  ```
- LibreTorrent's own license is still undecided (PLAN.md Q1). BSD-3 is compatible with any choice.
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
| `core::nat` | `upnp`, `natpmp`, `lsd`, `xml_parse` | 3.1k | 4 |
| `core::net::env` | `enum_net`, `ip_notifier`, `ip_voter` | 2.2k | 4 |
| `core::net::proxy` | `socks5_stream`, `http_stream.hpp`, `proxy_base`, `proxy_settings` | 1.0k | 5 |
| `core::ip_filter` | `ip_filter` (+ µTorrent `ipfilter.dat` parser, new) | 0.3k | 5 |
| `core::peer::web` | `web_peer_connection`, `http_seed_connection`, `web_connection_base` | 2.0k | 5 |
| `core::disk::part_file` | `part_file`, `posix_part_file` | 0.9k | 5 |
| `core::metainfo::create` | `create_torrent` | 1.0k | 5 |
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

| Area | libtorrent 2.0.11 | LibreTorrent | Why |
|---|---|---|---|
| Disk backend | mmap by default; `posix_disk_io` runs I/O on the network thread | pread/pwrite on the disk thread pool with our own write-back cache | memory accounting and performance on Windows and network drives; scale |
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
