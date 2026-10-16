# Changelog

All notable changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (local time).

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
