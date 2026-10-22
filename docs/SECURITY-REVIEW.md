# Parser security review

Date: 2026-10-22. Scope: every parser of network or file input in `patorrent-core` and
`patorrent-nox` (PLAN.md "Road to 1.0", item 14). Method: a grep for `unwrap()`, `expect(`,
slicing, `as usize`, shifts, `panic!` and `unreachable!` in the parser files to seed the hunt,
then a read of each parser and of the engine code that acts on its output (bounds of piece
indices and offsets, allocation driven by length fields, repeated work per read, what a remote
side can make us store or send). Compared with libtorrent 2.0.11 where the code is a port.

Every parser below has a cargo-fuzz target in `fuzz/fuzz_targets` (25 targets since the
remote protocol's was added on 2026-10-22, all in `.github/workflows/fuzz.yml`); the CI fuzzes each for 120 s per push and 30
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
| uTP | `utp/` | a port of libtorrent's packet handling; fuzzed with datagrams into every socket state | Not read line by line in this review. |
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
remote protocol's fuzz target are closed above):

- uTP (`utp/`) has not been read line by line; its fuzz target stands in.
- Fuzzing runs for short periods (120 s per target per push). The new targets have had one
  local 60 s run each on Windows (`fuzz.bat`): `udp_tracker` 6,609,867 runs, `resume_data`
  961,403 runs, `remote_wire` 413,530 runs (coverage 1,522 edges, from an empty corpus), no
  findings.
- The hash-work budget is per connection: a peer that reconnects starts with a full burst
  (`P × log2(P)` hashes: tens of milliseconds of one core for 10,000 pieces, several seconds
  at the 2^21-piece maximum, about what serving that torrent's piece layers once costs).
  Connection limits bound how often that can happen.
