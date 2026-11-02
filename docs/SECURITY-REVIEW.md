# Parser security review

Date: 2026-10-22. Scope: every parser of network or file input in `patorrent-core` and
`patorrent-nox` (PLAN.md "Road to 1.0", item 14). Method: a grep for `unwrap()`, `expect(`,
slicing, `as usize`, shifts, `panic!` and `unreachable!` in the parser files to seed the hunt,
then a read of each parser and of the engine code that acts on its output (bounds of piece
indices and offsets, allocation driven by length fields, repeated work per read, what a remote
side can make us store or send). Compared with libtorrent 2.0.11 where the code is a port.

Every parser below has a cargo-fuzz target in `fuzz/fuzz_targets` (26 targets since the ICMP
replies' was added on 2026-11-02, all in `.github/workflows/fuzz.yml`); the CI fuzzes each for 120 s per push and 30
minutes weekly. Panics on malformed input are the fuzzers' job, so the read concentrated on
limits and on logic the fuzzers cannot judge.

## Found and fixed

| Severity | Where | Problem | Fix | Test |
|---|---|---|---|---|
| Medium | `engine/tracker.rs` `http_resolved` | libtorrent's `ssrf_mitigation` (on by default, `Settings::ssrf_mitigation`) was ported for web seeds only. An HTTP tracker URL in a torrent or magnet, or a tracker's redirect, could make us send GET requests to any service on loopback (`http://127.0.0.1:8080/admin/...`). | As libtorrent `http_tracker_connection::on_filter`: when the path does not start with `/announce` (case-insensitive), loopback addresses are dropped; with none left the request fails with "banned by IP filter". Scrapes of loopback trackers (`/scrape`) are refused too, as in libtorrent. | `engine::tracker::tests::ssrf_mitigation_keeps_http_trackers_off_other_loopback_paths`; `engine::listen::session_tests::a_lost_internet_address_resets_the_votes_and_reannounces` now expects the refused scrape |
| Medium | `engine/v2.rs` `on_hash_request` | A v2 peer's 49-byte `hash request` can ask for 8192 hashes (256 KiB), and every valid request was answered. Reading is not throttled by the send buffer, so a peer that sends requests and never reads grows our send buffer without bound (memory exhaustion). libtorrent has the same behaviour. | `HASH_REPLY_SEND_BUFFER_LIMIT` (1 MiB): with that much queued for the peer, requests get a `hash reject` (no larger than the request) and the hashes are not computed. ut_metadata already had this kind of guard (`ut_metadata::SEND_BUFFER_LIMIT`, libtorrent's). Recorded in docs/PORTING.md "Deliberate divergences". | `engine::v2::tests::hash_requests_are_rejected_while_the_send_buffer_is_full` |
| Low | `proto/http.rs` `parse_response_limited` (HTTP trackers, UPnP) | The buffered parser runs again after every read; a server that never ends its header made each run scan the whole buffer (up to `MAX_RESPONSE`, 4 MiB): quadratic CPU on the engine thread for the request's lifetime. | A header longer than `http_parser::MAX_HEADER` (16 KiB, the incremental parser's cap that web seeds and `fetch` already use) fails the request. | `proto::http::tests::an_endless_header_is_refused` |
| Low (hardening) | `store.rs` `log_entry` (`resume.log`) | Offsets of a journal entry were added unchecked: a length near `u32::MAX` would wrap on a 32-bit target. Not reachable on 64-bit. | `checked_add` throughout; the replay stops at the entry. `resume.db` parsing and the journal replay are now the functions `parse_db` / `replay_log`, so the fuzzer reaches them without files. | `store::tests::crafted_journal_entries_end_the_replay` |
| Low | `proto/http.rs` `parse_response_limited` (HTTP trackers, UPnP; `net/fetch.rs`) | Closed from "Open" (2026-10-22). The buffered parser decoded the whole chunked body again after every read: a tracker trickling a chunked body cost O(n²) copying, up to 4 MiB a run, until `tracker_completion_timeout` (30 s). | `ResponseParser` keeps its place across reads as libtorrent's `http_parser` does: the head's end (searched from where the last search stopped), the decode offset, the current chunk's remaining bytes, the body decoded so far. Each read costs the bytes it added. The tracker and UPnP connections and `fetch` hold one per request; `parse_response` stays as the one-shot form. Header (16 KiB) and body limits unchanged. | `proto::http::tests::a_body_trickled_one_byte_at_a_time_decodes`, `proto::http::tests::the_work_per_read_is_bounded` (1 MiB in 1 KiB chunks read 100 bytes at a time: at most the response's size plus 4 bytes a read scanned, where re-decoding scanned about 5 GiB) |
| Low | `engine/v2.rs` `on_hash_request` | Closed from "Open" (2026-10-22). Our trees hold piece layers only, so a hash request near a file's root, or one with proof layers, costs about one SHA-256 per piece of the file, and a peer could repeat it without limit (libtorrent too). | A per-peer token bucket of hash-tree work (`HashWorkBudget` on `PeerConn`): each valid request's cost is estimated before computing (`hash_request_cost`, `2^h - 1` hashes per node `h` layers above the piece layer, answers and proofs); beyond the budget the peer gets a `hash reject`. Burst `P × log2(P)` hashes (`P` the padded piece count, at least 65,536), refill 65,536 a second. A libtorrent client fetching every piece layer from us costs about `P × (log2(P) - 9)`, inside the burst; caching interior layers was not chosen because it doubles the trees' memory. Recorded in docs/PORTING.md. | `engine::v2::tests::a_client_fetching_piece_layers_stays_within_the_hash_budget` (the hash picker against our tree, no time passing), `engine::v2::tests::repeated_root_level_hash_requests_are_rejected_beyond_the_budget` |
| Low | `nox/src/webui/auth.rs` `BanList` | Closed from "Open" (2026-10-22). Failed logins were counted and banned per address: an IPv6 client could step through its /64 for a fresh count with each address (qBittorrent behaves the same). | IPv6 clients are counted and banned by their /64, IPv4 clients (IPv4-mapped IPv6 included) by address; the list stays bounded at 10,000 entries. | `webui::auth::tests::an_ipv6_client_is_counted_and_banned_by_its_64` |

## New fuzz targets

- `udp_tracker`: BEP 15 replies (connect, announce with IPv4 and IPv6 peers, scrape, error)
  through `proto::tracker::udp_decode`, checking the decoded sizes against the input.
- `resume_data`: a fast-resume record through `ResumeData::decode` with an encode / decode round
  trip of whatever decodes, and the store's start-up read (`resume.db` parsed, `resume.log`
  replayed over it, every record decoded).
- `remote_wire` (2026-10-22): the remote protocol (`core::remote`): the input as one frame
  through every decoder (commands, alerts, the hello, and the typed values on their own:
  settings, add-torrent parameters, statuses, peers, trackers, files, errors, bitfields), then
  split into length-prefixed frames by `FrameReader`, each through them again. Whatever decodes
  must re-encode to bytes that decode and encode to the same bytes (bytes, not values: floats
  may be NaN). Reached through `fuzzing::remote_wire` (`remote::fuzz_wire`, behind the
  `fuzzing` feature).

## Reviewed and judged fine

| Parser | File | Limits and checks | Notes |
|---|---|---|---|
| bencode | `bencode/bdecode.rs` | depth 100, 2,000,000 tokens (libtorrent's defaults), buffers up to 512 MiB (token offsets are 29 bits), string headers at most 17 bytes, integers over 20 digits rejected, exact overflow checked on read | Faithful port of libtorrent's token array; no recursion while decoding. |
| .torrent / info dict, v1 and v2 file trees | `metainfo/torrent_info.rs`, `file_storage.rs`, `sanitize.rs` | `LoadLimits`: 10 MB buffer, 2^21 pieces, depth 100, 3,000,000 tokens; piece size ≤ 512 MiB (power of two ≥ 16 KiB for v2); file sizes bounded; v1 and v2 file lists of a hybrid must agree | Every path element goes through `sanitize_append_path_element`: separators, `.`/`..`, control and bidi characters dropped or replaced, Windows-invalid characters replaced, long names cut. Absolute paths cannot arise. Symlink targets are sanitized and the disk layer never creates symlinks. Windows device names (`CON`) are left as is: the disk layer uses `\\?\` paths, as libtorrent. |
| BEP 52 piece layers, merkle proofs, hash requests | `metainfo/merkle_tree.rs`, `engine/hash_picker.rs`, `engine/v2.rs` | `validate_hash_request`: file index, layer, index + count within the layer, count ≤ 8192, proof layers below the root; a per-peer budget of hash-tree work (new); `hashes` replies must match the request size exactly and prove out, or the peer is dropped | Block-hash requests are rejected (the trees keep piece layers only). |
| Magnet links | `metainfo/magnet.rs` | `so=` indices ≤ 10,000 (libtorrent's cap), `x.pe` / `dht` endpoints validated | |
| HTTP responses (trackers, UPnP) | `proto/http.rs` | header 16 KiB (new), body 4 MiB (`MAX_RESPONSE`), chunk sizes checked against the limit before adding, decoded incrementally (new), 5 redirects | |
| HTTP responses (web seeds, `fetch`) | `proto/http_parser.rs`, `net/fetch.rs` | header 16 KiB, chunk headers, `fetch` body 16 MiB by default and one deadline for the whole exchange, redirects included | |
| gzip / deflate | `util/gzip.rs`, `util/puff.rs` | output limit chosen by the caller (512 MiB for block lists); running out of input is an error | |
| HTTP tracker responses | `proto/tracker.rs` | integers clamped to `u32`, compact peers in whole entries only, dictionary peers need a valid IP and port 1-65535 | Peers then go through `add_peer` (IP filter, `max_peerlist_size`). |
| UDP tracker responses | `proto/tracker.rs` `udp_decode`, `engine/tracker.rs` | replies must match a transaction id we sent, come from the address we sent to, and be in the stage that expects them | |
| DHT | `dht/` | libtorrent's: 2000 torrents and 500 peers per torrent stored, 100 peers per reply, 20 samples, the DoS blocker (`dht_block_ratelimit`), token checks on `announce_peer`, names cut to 100 bytes | |
| Peer wire, extension handshake | `proto/peer_wire.rs`, `engine/wire.rs` | messages ≤ 512 KiB; HAVE / BITFIELD / DONT_HAVE checked against the piece count (a bitfield of another size disconnects); without metadata HAVE indices < 2^21; requests: piece < count, 0 < length ≤ 128 KiB, `start + length` within the piece, we have the piece, at most `max_allowed_in_request_queue` queued; PIECE only for a block we requested with the exact length; ALLOWED_FAST kept to 64; `reqq` caps our queue only downwards | |
| ut_metadata | `proto/ut_metadata.rs`, `engine/metadata.rs` | `max_metadata_size`, block index and size against the announced total, data only for pieces we asked that peer for, answers held back above 160 KiB queued (libtorrent's 10 blocks), at most 1024 held-back requests, then rejects | |
| ut_pex | `proto/ut_pex.rs`, `engine/pex.rs` | 500 KiB per message, 6 messages per minute, `max_pex_peers` (50) known per peer, local addresses only from local peers | |
| MSE / PE handshake | `proto/pe_handshake.rs`, `crypto/mse.rs` | pads ≤ 512 bytes, sync point searched in at most 512 + 20 bytes | |
| uTP | `utp/` | a port of libtorrent's packet handling; fuzzed with datagrams into every socket state | Read line by line on 2026-10-22: see "uTP" below. |
| LSD | `proto/lsd.rs`, `engine/lsd.rs` | our own cookie ignored, info-hashes we do not have ignored, none for private torrents | |
| UPnP | `proto/upnp.rs`, `proto/xml.rs`, `engine/upnp.rs` | replies from outside the local network ignored (`ignore_nonrouters`), 50 devices, 50 mappings, plain HTTP locations and control URLs only, responses 4 MiB | The XML scanner expands no entities and builds no tree. |
| NAT-PMP / PCP | `proto/natpmp.rs`, `engine/natpmp.rs` | replies only from the gateway's endpoint, exact sizes | |
| Proxies (SOCKS4/5, HTTP CONNECT) | `proto/proxy.rs` | HTTP CONNECT header 8 KiB, SOCKS5 address types and lengths checked, host names cut to 255 bytes | |
| Web seeds | `proto/web_peer.rs`, `engine/web_seed.rs` | the response range must equal the request; requests ≤ `urlseed_max_request_bytes`; SSRF mitigation (no query strings to local addresses, no redirects from global to local addresses) | |
| ipfilter.dat / P2P | `net/ip_filter.rs` | 512 MiB per list (after gunzip), invalid lines counted and skipped | |
| Resume data and the state store | `resume.rs`, `store.rs` | records bdecoded with the default limits; `resume.db` with a token limit of half its size; journal entries bounds-checked (now overflow-checked); pack records checked against the file length before reading | Local files: unfinished pieces are bounds-checked by the picker; `mapped_files` may hold absolute paths, as libtorrent allows (renames are the user's). |
| Part files | `disk/part_file.rs` | slot indices bounded by the piece count; a slot named twice is ignored the second time | |
| nox HTTP server | `nox/src/webui/http.rs`, `server.rs` | head 16 KiB, 100 headers, body 100 MiB (64 KiB before login), 1000 multipart parts, 64 connections, 30 s idle and 120 s body timeouts | |
| nox WebSocket | `nox/src/webui/ws.rs` | frames and messages 64 KiB (checked before the payload is buffered), client frames must be masked, control frames unfragmented and ≤ 125 bytes, reserved bits and unknown opcodes refused | |
| nox login | `nox/src/webui/api.rs`, `auth.rs` | constant-time comparisons, PBKDF2 password hash, failure count and ban per IPv4 address or IPv6 /64 (new; 5 failures, 1 h by default), ban list bounded at 10,000 entries | |
| Remote protocol | `core/src/remote/` | frames ≤ 256 MiB (nox: its body limit + 1 MiB), decoded with a token limit, exhaustive decoders that fail on anything malformed; fuzzed (`remote_wire`, new) | Reachable only after login. |

## Open

As of 2026-10-22 (the chunked decode, the hash-request CPU, the login ban by prefix and the
remote protocol's fuzz target are closed above; uTP was read line by line, see "uTP" below,
whose own "Open" list holds what is left there):

- Fuzzing runs for short periods (120 s per target per push). The new targets have had one
  local 60 s run each on Windows (`fuzz.bat`): `udp_tracker` 6,609,867 runs, `resume_data`
  961,403 runs, `remote_wire` 413,530 runs (coverage 1,522 edges, from an empty corpus), no
  findings.
- The hash-work budget is per connection: a peer that reconnects starts with a full burst
  (`P × log2(P)` hashes: tens of milliseconds of one core for 10,000 pieces, several seconds
  at the 2^21-piece maximum, about what serving that torrent's piece layers once costs).
  Connection limits bound how often that can happen.

## uTP

Date: 2026-10-22. Scope: `crates/core/src/utp/` (`mod.rs`, `socket.rs`, `packet_buffer.rs`,
`timestamp_history.rs`, about 3,900 lines with the tests), read line by line against
libtorrent 2.0.11's `src/utp_stream.cpp`, `src/utp_socket_manager.cpp`,
`include/libtorrent/aux_/utp_stream.hpp`, `src/packet_buffer.cpp` and
`src/timestamp_history.cpp`, and the engine's use of the manager (`engine/mod.rs`
`utp_dispatch`, read only). Questions asked of each path: what a short or malformed datagram
does, what one unsolicited datagram makes us store or send, what a peer that is connected can
make us allocate or compute, and where 16- and 32-bit wrapping arithmetic can turn around.

### Found and fixed

| Severity | Where | Problem | Fix | Test |
|---|---|---|---|---|
| Medium | `utp/socket.rs` receive buffer | Every in-order packet was kept as its own heap packet until the engine read it, and the receive window (1 MiB) counted payload bytes only. A peer sending 1-byte packets made a full window cost about 100 bytes of heap per byte, about 100 MiB per connection, whenever the engine was not reading (download rate limit, disk backlog). libtorrent pools its packets but has the same per-packet cost class. | Received bytes go into one byte ring (`VecDeque<u8>`), shrunk to 16 KiB once drained: the window bounds the memory. | `utp::tests::one_byte_packets_take_one_byte_each_in_the_receive_buffer` |
| Medium | `utp/socket.rs` detached sockets | Once nothing of ours is in flight every incoming packet restarts the retransmission timer. A peer that kept sending data without acknowledging our FIN kept a socket the engine had let go alive for ever, storing up to 1 MiB that nobody read. Such sockets count against the SYN-flood bound (`2 × connections_limit` sockets), so enough of them shut out incoming uTP, and they are not engine peers, so the engine's own limit does not see them. libtorrent behaves the same. | A detached socket acknowledges data and drops it (its receive buffer is freed at detach, out-of-order data is not held), and it is deleted 60 s after the upper layer let go (`DETACHED_LINGER`) whatever arrives. | `utp::tests::a_detached_socket_stores_nothing_and_goes_away_although_the_peer_keeps_sending` |
| Low | `utp/socket.rs` `incoming` | An empty data packet received out of order was queued for the reader once the gap closed, uncounted: unbounded while the engine was not reading. libtorrent drops empty payloads (`size <= 0`). | Empty payloads are dropped (with the byte ring there is nothing to queue). | as the first row |
| Low | `utp/socket.rs` `parse_sack` | A SYN carrying a selective ACK made the new socket (sequence number 0, acked 0) step `acked_seq_nr` through all 65,536 values: about 0.1-0.3 ms of CPU per spoofed SYN. libtorrent does the same. | The selective ACK of a packet that reaches a socket in state `None` (only the SYN that opens it) is not walked. | `utp::tests::a_selective_ack_on_a_syn_does_not_walk_the_sequence_space` (counts the steps) |
| Low | `utp/mod.rs` `incoming_packet` | A SYN whose extension chain was malformed returned early from the socket's parser before the socket connected, and the manager still handed that socket to the engine as an incoming connection in state `None` (it lived until the engine's handshake timeout). libtorrent does the same. | A SYN that does not leave its socket `Connected` leaves no socket. | `utp::tests::a_syn_with_a_broken_extension_chain_leaves_no_socket` |
| Low | `utp/mod.rs` `incoming_packet` | A SYN sent again (its SYN-ACK was lost; libutp clients resend SYNs) made a second socket and a second incoming connection: the first stayed half-open until the handshake timeout. libtorrent does the same. | A SYN from the address and with the connection id of a socket we accepted is answered by that socket: acknowledged again while nothing else came from the other end, ignored after. | `utp::tests::a_syn_sent_again_is_acknowledged_again_without_a_second_socket` |
| Low | `utp/mod.rs` demultiplexing | A packet not for the last socket was matched by scanning every socket: O(sockets) work per datagram for anyone who can send datagrams (up to `2 × connections_limit` sockets from SYNs, more with outgoing ones). libtorrent uses a multimap by connection id. | An index by receive id (`by_recv_id`), kept on insert and remove. | `utp::tests::sockets_are_found_by_id_after_their_slots_are_reused` |
| Low | `utp/socket.rs` `send_pkt` | Nothing bounded the sequence numbers in flight. A receiver advertising a huge window, with a congestion window grown by fast ACKs, could have us send past half the 16-bit space without an ACK: the wrapping comparisons (ACK validity, `packet_buffer`, loss detection) then turn around and the send buffer's slots collide. libtorrent has no bound either; real receivers advertise about 1 MiB. | At most 16,384 sequence numbers unacknowledged (`MAX_OUTSTANDING`; 9 MiB at the smallest MTU, 23 MiB at 1452 bytes). | `utp::tests::no_more_than_a_quarter_of_the_sequence_space_is_in_flight` |
| Low | `utp/mod.rs` SYN-flood bound | With a negative `connections_limit` (unlimited to the engine) the bound `2 × limit` was negative and every incoming SYN was refused. | A negative limit counts as 10,000 connections for the bound (20,000 sockets). | `utp::tests::an_unlimited_connections_limit_still_accepts_and_bounds_syns` (also checks the bound with a limit of 2) |
| Low | `utp/mod.rs` `incoming_packet`, `engine/mod.rs` `utp_dispatch` | Closed from "Open" (2026-10-23). The manager acknowledged a SYN (the deferred `ST_STATE` went out when the UDP socket was drained) before the engine decided; a connection the engine then refused (paused session, `enable_incoming_utp` off, IP filter, connection limit) got a FIN as well: two 20-byte datagrams for one SYN, where libtorrent's session refuses inside the manager's callback and sends one FIN carrying the ACK. | The manager asks an accept filter, with the source address, about each SYN that makes a new connection, before anything is sent (`incoming_packet_filtered`). The engine's filter (`Engine::utp_refusal`) runs libtorrent's `incoming_connection` checks in its order and counts connections accepted earlier in the same wakeup against the limit; a refused socket is closed at once, so its FIN (acknowledging the SYN, cancelling the deferred ACK) is the only answer, it is never handed to the engine, and it lingers only until the FIN is acknowledged or its resends run out. The `PeerBlocked` alerts (`UtpDisabled`, `IpFilter`) are raised after the read loop. `utp_dispatch` keeps the same checks as a second line. | `utp::tests::a_refused_syn_gets_one_fin_carrying_the_ack_and_no_connection`, `utp::tests::a_refused_syn_whose_fin_is_never_acknowledged_goes_away`, `utp::tests::an_accepted_syn_through_the_filter_is_answered_and_connects`; the `utp` fuzz target's flood stage refuses one of its two addresses |
| Low (hardening) | `utp/socket.rs` `do_ledbat`, `experienced_loss` | The LEDBAT gain was computed in 64 bits; with a target delay near 1 µs and a large gain factor (both settings) a delay of a few hundred milliseconds overflowed (a panic in a debug build, a wrapped window in release). The loss cut multiplied by `utp_loss_multiplier` likewise. Not reachable with libtorrent's defaults. | Both in 128 bits, the window bounded at 2 GiB (`MAX_CWND`). | `utp::tests::an_extreme_target_delay_and_gain_do_not_overflow_the_window_arithmetic` (panics on the old code) |

### Fuzzing

The `utp` target (`fuzz/fuzz_targets/utp.rs`) gained a stage: the input's datagrams go
unpatched, one by one and from two addresses, into a manager with `connections_limit` 2, which
must never hold more than 5 sockets and never send more datagrams than it received (the
amplification bound: its sockets have nothing of their own to send). The accepted connection of
the connected stage is detached halfway, so the rest of the datagrams reach a closing socket
nobody reads, and after the final detach and 100 s of ticks no socket may be left. One local
run on Windows (`fuzz.bat utp 120`, from an empty corpus): 488,360 runs, coverage 1,353 edges
(5,742 features, a 556-entry corpus), no findings.

### Reviewed and judged fine

| Area | Checks | Notes |
|---|---|---|
| Header | at least 20 bytes, version 1 (checked by the manager before any lookup), type below 5 | A DHT message never parses as version 1 (`d` is 0x64). |
| Extension chain | each extension's two-byte header and its length checked against the datagram before reading; unknown extensions skipped; `close_reason` read and dropped | A selective ACK is at most 255 bytes (2,040 bits); the walk stops at our next sequence number. |
| 16-bit arithmetic | `wrapping_*` throughout; `compare_less_wrap`; an ACK must lie between `acked_seq_nr − 3` and the last sequence number sent (the FIN's in `syn_sent` / `fin_sent`); data more than `max(16, capacity / 1100)` = 953 packets ahead of `ack_nr` is dropped; data after a received FIN is ignored | Out-of-order bytes count against the same 1 MiB window as in-order ones. |
| `packet_buffer` | indices masked by a power-of-two capacity; `at`, `remove`, `insert` take distances modulo 0x10000 (the earlier wrap fix); growth bounded by the reorder window (receive side) and now `MAX_OUTSTANDING` (send side) | A distance past 0xffff cannot reach `reserve` from either side any more. |
| Unexpected packets per state | a SYN on an open socket is ignored (counted invalid); in `syn_sent` only a packet acknowledging the SYN connects; a RESET needs the socket's address, its receive id and an ACK within what we sent; a RESET or other packet for an unknown connection is ignored, and no RESET is sent for one (libtorrent's `#error send reset` is not done there either) | Off-path injection needs the 16-bit connection id, the address and an in-window ACK: BEP 29's weakness, as in libtorrent. |
| Amplification | an unknown non-SYN gets nothing; a SYN gets one 20-byte `ST_STATE`; a FIN one `ST_STATE`; a socket in an error state answers nothing (the RESET branch is unreachable, as in libtorrent) | A SYN the engine refuses (limit, IP filter) gets one 20-byte FIN instead of the `ST_STATE` (since 2026-10-23, see Found and fixed), resent at most `utp_fin_resends` times if never acknowledged. |
| Timestamps and delay | `u32` wrapping differences; the other side's base delay shift compensates only for drops of 0 to 10 ms; the delay used is clamped to the RTT; `target_delay` at least 1 (no division by zero); LEDBAT runs only with bytes in flight; `packet_timeout` capped at 60 s, the shift bounded by `num_timeouts < 7` | `timestamp_history` is a faithful port; its counter saturates at 0xfffe. |
| MTU discovery | `u16` subtractions bounded below (the floor starts at 548 and every ceiling the manager remembers is a socket's MTU, at least its floor); a probe that hits `EMSGSIZE` is resent unprobed | A peer that drops probes on three connections lowers the starting ceiling for new ones (the largest of the last three failures; libtorrent's design). |
| SYN flood | more than `2 × connections_limit` sockets: SYNs are dropped (libtorrent's rule); a socket whose address was never confirmed fails on its first timeout; accepted sockets go to the engine, whose connection limit, IP filter and handshake timeout apply | |
| Integer ranges | window and in-flight counters are `i32` / `i64` with the window now bounded; `adv_wnd` is the peer's `u32` and only ever a minimum; RTT samples come from `Instant` differences | |
| Shutdown | detach sends the FIN once everything queued is out; the socket goes once the FIN is acknowledged, its resends or timeouts run out, or now the linger expires; the session's shutdown drops the manager (libtorrent drops the FINs too) | |

Deliberate deviations from libtorrent, beside those in docs/PORTING.md: the retransmission
timer restarts only on ACK progress (2026-10-19); the timers run from the session tick (up to
one tick late); and the fixes above (byte receive buffer, empty payloads dropped, detached
sockets drop data and linger 60 s, at most 16,384 sequence numbers in flight, no selective ACK
walk on a SYN, a SYN sent again answered by its socket, unconnected SYN sockets removed, the
socket index, the unlimited-connections bound, wide LEDBAT arithmetic).

### Open

- Spoofed SYNs make half-open connections that take engine peer slots until the handshake
  timeout (uTP has no SYN cookies; libtorrent is the same). An engine-side mitigation would
  count unconfirmed incoming uTP connections apart, or give them a shorter handshake timeout.
- `send_pkt` scans every unacknowledged sequence number for packets to resend on each send:
  O(packets in flight) per packet, now at most 16,384 (libtorrent the same). The ready list's
  duplicate check is O(sockets) per mark.
- The accepting side cannot send before the initiator's first packet after the SYN (the SYN
  advertises a zero window; libtorrent the same). BitTorrent's initiator speaks first.
- Not run against other implementations since these changes: the SYN-resend handling with
  libutp clients (µTorrent, Transmission), and `MAX_OUTSTANDING` is never reached against real
  receivers (about 1 MiB windows). The fuzz target has had one 120 s local run.

## Release 1.1.32 (since v1.1.17)

Date: 2026-11-02_220138_1793653298. Scope: every parser of network or file input, and every
handler of untrusted downloaded data, added or changed in `git diff v1.1.17..HEAD`: keep ping
low's probes (`net/probe.rs`, D49) and its use of the uTP delays (`engine/auto_limit.rs`,
`UtpManager::shared_delays`); the Windows self-update (`net/fetch.rs` streaming,
`frontend/update.rs`, `win32/updater.rs`, D45) and `update.txt` (D43, nox and GTK); RSS
conditional GET and gzip, Torznab and the shim's `search/*`, the qBittorrent import,
watched folders; the libtorrent 2.0.12-2.1.2 ports (NAT-PMP gateway and lease, merkle proofs
and the hash picker, torrent metadata, HTTP parsing, web seeds, UPnP, proxies, LSD, the DHT's
IP filter, resume data). Method as above; ports compared with libtorrent.

### Found and fixed

| Severity | Where | Problem | Fix | Test |
|---|---|---|---|---|
| Low | `win32/src/updater.rs` `run_setup`, `frontend/src/update.rs` `download_setup` | The setup's SHA-256 was taken while it was written to `%TEMP%\PaTorrent-update`, and the file was run by path after the "install now?" box, which can stay open for any time. Another process of the user (or of another user with a shared `TEMP`) could swap it between the check and the start; for an all-users install the user would then approve the UAC prompt of whatever was swapped in. | `download_setup` returns `Setup { path, sha256 }`; `spawn_checked` opens the file with `FILE_SHARE_READ` only (`open_locked`: no one can write, rename or delete it), hashes it again and starts it while holding that handle; a changed file is refused. | `updater::tests::the_setup_runs_only_as_checked`, `update::tests::the_setup_downloads_and_is_checked` |
| Low | `frontend/src/update.rs` `download_setup` | The part file was opened with `File::create`, which writes through a hard link or symlink planted at `<name>.part` and truncates its target. | The old `.part` is removed, then the file is opened with `create_new`. | `update::tests::the_setup_downloads_and_is_checked` (a planted hard link keeps its target's bytes) |
| Low | `net/fetch.rs` `exchange` (`http_get_to`, streaming to a sink) | Bytes after `Content-Length` in the same read were written into the setup file; a `Content-Length` over `max_size` was refused only after the first read reached the sink; a close-framed body could pass the limit by one read (16 KiB). | A `Content-Length` over the limit is refused before anything reaches the sink; each read hands over at most what is left of the named length; the limit is checked before the sink is called. | `net::fetch::tests::a_streamed_body_stops_at_its_length_and_the_limit` |
| Low (hardening) | `net/fetch.rs` `http_get_inner`, `frontend/update.rs` | A redirect from https to plain http was refused only when cookies were sent: a redirect in the `update.txt` / `SHA256SUMS` / setup chain could drop to clear text, where both the checksum list and the setup can be substituted. Needs GitHub itself to redirect so. | `FetchOptions::no_downgrade` (off by default: RSS and add-by-URL unchanged), set by the update check and the setup download. | `net::fetch::tests::https_to_http_redirects` (extended) |
| Low (hardening) | `frontend/update.rs` `Version::parse` | A version of any length up to the 64 KiB body was accepted; it goes into message boxes, URLs and the setup's file name. | At most 64 bytes (`VERSION_MAX`). | `update::tests::versions_parse_and_print_as_written` |
| Low | `frontend/search/source.rs` `url_origin` (`worker.rs` `download_reach`: `/pa/search/add`, the shim's `search/downloadTorrent`, `torrents/add` with `patorrent-search:` URLs) | SEC-26 bypass by a parser mismatch, older than v1.1.17 (the shim adds ways to reach it). `download_reach` split a result link with the frontend's own parser (host after the last `@`); the fetch connects by `parse_url_components` (libtorrent's: an `@` ends userinfo only after a `:`, and only the first). `http://[127.0.0.1]@idx.example.org/x` looked like the source's origin and got `Reach::User`, but the fetch went to 127.0.0.1:80: a blind GET to loopback or the LAN from an indexer's link once the user adds that result. | `url_origin` reads URLs with `parse_url_components` (host lowercased, trailing dot dropped, the scheme's default port), the parser the fetch connects by; `clean_url` (the fetch's escaping) touches none of `@ : [ ] / ?` and cuts at `#`, which also ends the authority there. | `search::worker::tests::the_proxy_rule` (extended; fails on the old code) |
| Low | `frontend/torrent_folders.rs` `read`, `find`, `place`, `move_copy` (watched folders in all three frontends) | `fs::read` loaded the whole file before `TorrentInfo::from_bytes` refused anything over 10 MB: any `*.torrent` landing in a watched folder (a downloads folder, a share) was read whole, again after every restart since a failed file stays (on Unix `/dev/zero` for ever). | `read_torrent_file`: refused when the open file is over `LoadLimits::max_buffer_size`, and at most the limit + 1 bytes read. | `torrent_folders::tests::an_oversized_torrent_file_is_refused_without_reading_it` |
| Low (hardening) | `frontend/import.rs` `QbtLimit::target` | A huge qBittorrent `qBt-ratioLimit` / `qBt-seedingTimeLimit` saturated to `u32::MAX`, which `SeedGoal::load_override` reads back with `get_i32`: the ratio read as missing (default goal), the time as 0 (off). Local files. | Clamped to `1..=i32::MAX`. | `import::tests::qbittorrent_share_limits` (NaN, inf, 1e300, `i64::MAX`, a round trip at `i32::MAX`) |
| Low (hardening) | `frontend/rss/mod.rs` `fetch_feed` | A feed's `ETag` / `Last-Modified` were stored in `rss.dat` and sent back on every refresh at any length up to the 16 KiB head, control characters included (CR/LF were already stripped by `fetch::request`). | `validator()`: trimmed; dropped (no conditional request) over 1 KiB (`MAX_VALIDATOR`) or with control characters. | `rss::tests::oversized_or_odd_validators_are_not_kept` |
| Low | `metainfo/torrent_info.rs` `resolve_duplicate_filenames_slow` | Older than v1.1.17, libtorrent the same: `n` files of one name tried about `n²/2` names (8,000 in a 192 KB info section: 29.8 s in a debug build), and names forged to one CRC32C were all compared with each other; on the engine thread (`got_metadata`, adding a torrent). | Entries keyed by (CRC32C, keyed SipHash of the lowercase name); each base and extension remembers its last counter. Results equal libtorrent's. | `metainfo::torrent_info::tests::duplicate_names_resolve_in_linear_work` (3,000 duplicates: N-1 tries; 3,000 names forged to one CRC32C: none) |
| Low (fuzz CI) | `resume.rs` `from_node` (`pieces`) | Since records are saved as file-version 2, version-1 `pieces` whose length is not `lt-num-pieces`, or an empty one, did not read back the same once saved: the `resume_data` target's round trip would fail within seconds. | Both versions decode to the shape a version-2 save gives back (whole bytes, cut to `lt-num-pieces`; no bits is `None`). | `resume::tests::every_pieces_field_reads_back_the_same_once_saved` |
| Low (hardening) | `net/probe.rs` `parse_icmp` (Linux, macOS) | An echo reply was taken from whichever host sent it. macOS hands every echo reply on the machine to each socket (our identifier is the process id), Linux checks only its own identifier, and our sequence numbers count up: a forged reply could become the "router" the search found (then pinged four times a second) or a round trip. | A reply counts only from the address pinged (RFC 1122 3.2.2.6). A forged time-exceeded answer cannot be told apart (any router may send one). Windows matches replies itself, unchanged. | `net::probe::tests::an_echo_reply_counts_only_from_the_address_pinged` |
| Low (hardening) | `net/probe.rs` `ip_header_len` | The IHL was used without checking the version or that it covers a header (0 to 16 bytes): a router's time-exceeded answer quoting something else (version 6, IHL 5) was read as quoting our echo. | Version 4 and at least 20 bytes, or `None`. | `net::probe::tests::ip_headers_are_checked_before_their_length_is_used` (fails on the old code) |
| Low (hardening) | `net/probe.rs` Linux `read_error` (`recvmsg` on the error queue) | The control-message walk added each `cmsg_len` unchecked (a length near `usize::MAX` overflowed: a panic in debug, a backwards slice in release), and the queued packet's sequence number was read from the 576-byte buffer whatever `recvmsg` returned (a short packet matched sequence 0). Kernel data, not remote. | The walk is now `parse_error_queue`, a safe function on byte slices (fuzzed, tested on every platform) with checked offsets; the packet is cut to the bytes received. The `unsafe` is left to `recvmsg` with its buffers. | `net::probe::tests::the_error_queue_is_read_within_its_bounds` |
| Low | `engine/v2.rs` `hash_request_cost` | The estimate summed proof siblings for layers `k+1..=P`, but `MerkleTree::get_hashes` sends `k..=P` (`k` the layers the `count` hashes imply): the first sibling was never counted, so a piece-layer request with `count = 2^k` and `proof_layers = k` was estimated at 0 but cost `2^k - 1` SHA-256 (8,191 for 8,192 hashes) and never drained the `HashWorkBudget`; bounded only by the 1 MiB send-buffer guard and the upload rate. Fixed after the review (the file was in another change set). | The loop runs over `k..=proof_layers` at height `height + i`, as `get_hashes` sends. A libtorrent client fetching every piece layer now measures `P × (log2(P) - 8)` (81,858 hashes for 16,384 pieces), still inside the `P × log2(P)` burst. | `engine::v2::tests::repeated_root_level_hash_requests_are_rejected_beyond_the_budget` (count 512, `proof_layers` 9 at the piece layer: 511; one node with `proof_layers` 1 now counts its own sibling), `a_client_fetching_piece_layers_stays_within_the_hash_budget` |

### New fuzz targets

- `icmp` (`fuzz/fuzz_targets/icmp.rs`, through `fuzzing::icmp` = `net::probe::fuzz_icmp`): the
  input as an echo socket's message through `parse_icmp` (with and without the IPv4 header,
  with and without an identifier, from the address pinged and another), as an error-queue entry
  through `parse_error_queue`, and through the checksum. A reply must come from the address
  pinged and carry our sequence number; a filled-in checksum must sum to zero. One local run on
  Windows (`fuzz.bat icmp 120`, empty corpus): 10,787,825 runs, coverage 219 edges, no findings.
- `web_seed` also resolves a `Location` against a referrer from the input (the referrer's query
  and fragment handling of 2.0.12+ was not reached with a fixed referrer): 248,509 runs in 60 s,
  no findings. `resume_data` ran 60 s after the `pieces` fix: no findings.

### Reviewed and judged fine

| Parser or area | File | Limits and checks | Notes |
|---|---|---|---|
| ICMP replies, Windows | `net/probe.rs` `IcmpPinger` | `IcmpSendEcho2Ex` / `Icmp6SendEcho2` synchronous; replies read only when the call returns a count above 0, from the buffer's start; the 256-byte buffer is asserted at compile time to hold the documented minimum (structure, 32-byte payload, 8 bytes, `IO_STATUS_BLOCK`); a longer reply fails the call; the structures' pointers are not followed | The system matches replies to its requests. |
| ICMP replies, Linux and macOS | `net/probe.rs` | 1,500-byte receive buffer; `parse_icmp` and the quoted header bounds-checked with `get`; the sequence number (and on macOS the identifier) must match; time-exceeded answers count in the expiring mode only from the router found; `poll` with the remaining time, so a flood of foreign messages ends at the 1 s timeout | Forging needs the sequence number (sequential) and, off the path, the identifier: a fake round trip only misleads the cap (below). |
| uTP delay signal | `utp/socket.rs`, `utp/mod.rs` `shared_delays`, `engine/auto_limit.rs` | delays from connected sockets only, clamped to the RTT where one is known; negative values count as 0; the lowest of at least three sockets that measured in the last 2 s; combined with the pings by the lower | See "Keep ping low and fake delays" below. |
| `update.txt` | `frontend/update.rs` `Manifest::parse`, `fetch` | 64 KiB; only `stable=` / `prerelease=`; SemVer core plus `[0-9A-Za-z-]` identifiers, now at most 64 bytes; only versions above the running one | The version can only name a release tag of the fixed repository. |
| `SHA256SUMS`, setup name | `update.rs` `expected_sha256`, `setup_name` | 64 KiB over HTTPS from `github.com/<repo>/releases/download/v<ver>/`; exact file-name match; a full 32-byte digest; the file name is a fixed pattern | No path or quoting characters can reach the name. |
| Setup download and start | `fetch.rs` `http_get_to`, `win32/updater.rs` | 64 MiB; 30 s to the head, then 30 s between reads (Cancel ends it at once); SEC-26 reach rules; 5 redirects; checked before the rename; `CreateProcess` with an absolute path and fixed arguments (`/S`, `/CURRENTUSER` or `/ALLUSERS`, `/UPDATE=<pid>`); no elevation by PaTorrent | Nothing from `update.txt` or the server reaches the command line. |
| nox and GTK update checks | `nox/src/update_check.rs`, `gtk/src/app.rs` | check only, no download; the Web UI writes the version as text and links `https://` URLs only | |
| RSS conditional GET, gzip | `frontend/rss/` | validators sent only while the items are in memory, cleared when the URL changes; a 304 counts only for a conditional request; gzip inflated under `max_size` (16 MiB) | |
| Torznab, shim `search/*` | `frontend/search/torznab.rs`, `nox/webui/search.rs` | through the RSS parser (5,000 items, 8 KiB fields); caps 2,000 categories; pattern 512 characters, 5 running and 32 kept jobs; `fileUrl` only a checked `magnet:` or the opaque `patorrent-search:` URL; `.torrent` 16 MiB, then `LoadLimits`; query values percent-encoded | Authenticated. Fuzzed by `rss`. |
| qBittorrent import | `frontend/import.rs` | default bdecode limits; `pieces`, `verified`, `unfinished` dropped; `mapped_files` sanitized | Local files. |
| NAT-PMP / PCP | `proto/natpmp.rs`, `engine/natpmp.rs`, `engine/portmap.rs` | lease a local setting (0 is 3,600); the router's lifetime × 3/4 in `u64`; the gateway setting an IP literal of the interface's family | |
| Merkle proofs, hash picker | `metainfo/merkle.rs`, `merkle_tree.rs`, `engine/hash_picker.rs` | zero nodes refused; uncle count checked before `leaf_count << n` (the old spin fixed by the port); reply length `count + num_proof_hashes()`; `add_hashes` validates first | |
| Metadata, magnets | `metainfo/torrent_info.rs`, `sanitize.rs`, `magnet.rs` | depth 100 on v1 paths, symlink targets and v2 trees; `is_valid_tracker_url`; `parse_decimal` overflow-checked; `dht=` ports 1-65,534 | |
| HTTP, web seeds, proxies, UPnP | `proto/http_parser.rs`, `http.rs`, `web_peer.rs`, `engine/web_seed.rs`, `proxy.rs`, `upnp.rs` | strict status codes; redirect query and fragment sliced on ASCII; credentials dropped across origins (`same_origin`); SOCKS version exactly 5 | |
| DHT IP filter, LSD | `dht/`, `engine/lsd.rs` | the filter before the DoS blocker and bdecode, on nodes from replies, bootstrap and `add_node`; LSD resends bounded | Routers deliberately unfiltered (libtorrent). |
| Peer wire, v2 blocks | `engine/wire.rs`, `smart_ban.rs`, `engine/peer.rs` | `piece_size_for_req - start` reached only for blocks we requested; v2 blocks kept below the piece's size | Read only (another change set was open there). |

### Keep ping low and fake delays

A peer controls both of its delays: `recv_delay` comes from the timestamps it sends, and
`send_delay` from the timestamp differences it echoes. What it can do with them:

- **Claim no delay.** With pings, the lower of the pings' queue and the uTP peers' is used, and
  the uTP figure is the lowest among the sockets: one peer claiming zero, among three that
  measured, hides a direction's queue, and the caps do not cut. The cost is the user's: the line
  bloats as without the feature. libtorrent's LEDBAT has the same exposure per socket.
- **Claim a large delay.** It only raises that peer's own figure; the minimum ignores it while
  any other uTP socket measured, and with pings the lower of the two ignores it too. It moves
  the caps only when there are no pings (FreeBSD, OpenBSD, Linux outside `ping_group_range`,
  nothing answering) and the attacker holds every uTP socket that measured in the last two
  seconds, at least three. Then a direction we use at half its cap or more is cut every three
  seconds to its floor: 10 kB/s or 5% of the highest rate seen on this network, and it grows
  back 5% a second once they go. This is wider than libtorrent's exposure (a fake delay there
  slows only that socket) but bounded by the floor, needs the pings to be missing, and is off by
  default. Accepted; see Open.

A forged ICMP reply (now only from the address pinged) or time-exceeded answer can likewise
only show a lower round trip, with the same effect as a zero delay.

### Open

- **v2 file-tree path memory (libtorrent the same).** `FileStorage::paths` stores each directory
  path in full while the tree pays for each element once: a 180 KB info section can make 37.8 MB
  of paths (about 210×), about 2 GB at the 3 M token limit, more from a magnet's metadata
  (`max_metadata_size` 30 MiB; `from_info_section` does not check `max_buffer_size`).
  Proposed: a `LoadLimits` cap on the total unique directory-path bytes, a policy decision.
- **Self-update: the checksum and the setup share one origin.** A compromised GitHub account
  or release workflow defeats the check. Proposed: an offline Ed25519 key signing `SHA256SUMS`,
  its public key in the binary; or Authenticode with a pinned publisher (`WinVerifyTrust`).
- **Self-update: the setup's own elevated re-launch.** For all users the NSIS setup re-launches
  `$EXEPATH` with `runas`; the file in `%TEMP%` can be swapped in that gap, after PaTorrent's
  lock ended. With a shared `TEMP` (`TMP=C:\Temp`) another user can also pre-create
  `PaTorrent-update` and plant DLLs beside the setup. Proposed: download into
  `%LOCALAPPDATA%\PaTorrent\update` (or a fresh random folder whose owner is checked), or have
  the setup copy itself to an admin-only folder before elevating.
- **`PATORRENT_UPDATE_URL` / `PATORRENT_UPDATE_FILES` are live in release builds** and accept
  plain http. Whoever sets the user's environment already runs code as the user, but for an
  all-users install this chooses the next update's UAC prompt. Proposed: debug builds only, or
  https only.
- **`http_get_to` has no overall deadline**: a server trickling a byte every 29 s is ended only
  by Cancel (fine for the dialog; a future caller without one would need a minimum rate).
- **Search fetch threads outlive their jobs.** A stopped or deleted shim job, or a replaced
  native search, leaves its per-source fetches running until their 30 s timeout: an
  authenticated client looping `search/start` + `search/delete` gets unbounded concurrent
  threads (each up to 16 MiB from the user's own indexers). Proposed: a shared in-flight
  counter in `frontend/search/worker.rs`.
- **A merged search result** keeps the link of the source with most seeders, and
  `fetch_download` does not check the fetched torrent's info-hash against the result's: one
  indexer can answer for another's hash (older than v1.1.17).
- **Fake uTP delays without pings** (above): counting distinct hosts (IPv4 address, IPv6 /64)
  instead of sockets for the three would stop a single host; not done, as the bloated-line and
  loopback tests run their swarms from one address and would need distinct ones.
- **Fuzzing gaps:** no target reaches the merkle tree and hash picker (`MerkleTree::add_hashes`
  / `get_hashes` and `merkle_validate_and_insert_proofs` on a small tree built from the input,
  honest replies must verify, altered ones must not); `import.rs`'s `qbittorrent_goal` /
  `qbittorrent_flags` are private behind a file-based entry. The new runs are one short local
  run each.
