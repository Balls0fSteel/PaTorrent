# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-10-27_152058_1793114458: Security fix SEC-04: Web UI slots before login

- **Lingering close:** both (after an error answer and after a WebSocket close) now stop after 2 s in all, not 2 s per read; one client trickling a byte every 1.9 s used to hold a slot for up to 1 MiB of reads, and 64 of them locked everyone out.
- **Slots:** the acceptor admits 64 connections in all and 8 per address (an IPv6 /64 counting as one, like bans). Loopback has no per-address limit, so a reverse proxy on the same machine keeps working.
- **Bans at accept:** a banned address gets its 403 before anything is read and holds its slot only for the linger. Answering straight from the acceptor was tried first: Windows reset the connection over the unread request and the client lost the 403, and lingering in the acceptor would stall every accept. The banned text is now one constant (`api::BANNED`).
- Validation: unit tests for the slot accounting and the trickling client (2 s instead of about 60 s), the accept-time ban in the Web UI ban test; core, frontend, nox and Win32 suites pass; workspace clippy and rustfmt clean. Still open: an overall TLS handshake deadline, the JSON duplicate-key cost (after login).

## 2026-10-27_150953_1793113793: Security fix SEC-16: imports are checked, feeds cannot nest forever

- **Decision D36** (owner; supersedes D35's "trusting their pieces"): imported records lose their claimed and partial pieces, so the engine checks the files, and every `mapped_files` path is cleaned element by element into the save folder. The confirm question now lists the save folders (up to 12, then a count). A crafted backup can no longer get a private file of the right length seeded or map a download into Startup.
- **RSS:** start tags past the parser's depth limit are only counted, and an end tag closes one of them first; 200,000 `<a>` then 200,000 `</b>` used to mean about 4e10 name comparisons.
- **Behaviour change:** every import runs a full check (time once per import); files µTorrent had moved outside the save folder are downloaded again inside it. `nox run --import` help and the V1.1 plan line updated.
- Validation: unit tests for hostile `targets`/`mapped_files`, dropped pieces and the folder list; end-to-end tests on a real session (all pieces claimed over other data: none taken; a partial claim over complete data: all five found by the check); the RSS depth test. Three existing tests that asserted trusted pieces now assert the check. Core, frontend, nox and Win32 suites pass; workspace clippy and rustfmt clean. Moved three older entries to `docs/archive/CHANGELOG-2026-10-27--2026-10-27-v1.1-benchmarks.md`.

## 2026-10-27_144050_1793112050: Security fix SEC-14: remote login over HTTPS

- **Default:** Connect to Remote has Use HTTPS ticked unless the user turned it off before.
- **Plain HTTP:** `remote::connect` refuses it to any host but `localhost` or a loopback address unless `Target::allow_plaintext` is set (`ConnectError::Plaintext`, nothing sent). The Win32 and GTK dialogs set it only after the new `RemotePlaintext` question; nox's own default is still HTTP, so a LAN setup keeps working after one Yes.
- **Deadline:** reading the server's answer head now stops at the connect deadline; a server dripping one byte at a time held it for up to 16 KB of read timeouts.
- **Strings:** `RemotePlaintext` in en and the other 34 tables; the translations are machine-made and need native review (as with V1.1's strings).
- Validation: core tests for the refusal (nothing reaches a listener) and the dripping server (gives up at 1 s), and a frontend test for the default and the question; core, frontend, nox and Win32 suites and the language-table checks pass; workspace clippy and rustfmt clean; GTK clippy clean for Linux with a stand-in pkg-config. Not verified: either dialog by hand.

## 2026-10-27_142631_1793111191: Security fix SEC-06: cookies stay on their site

- **Fetch:** `http_get` sends the `Cookie` header only to the first URL's origin (scheme, host, port). A redirect to another origin is followed without cookies, and with cookies set an https→http redirect is refused (`FetchError::Http`).
- **RSS:** item downloads get the feed's `:COOKIE:` value only through `RssFeed::cookies_for`: an item on the feed's host or a parent/child subdomain of it, and not from an https feed to an http item. A feed item can no longer carry a private tracker's login to another host.
- **Behaviour change:** downloads on a sibling subdomain of the feed (`rss.x.org` → `dl.x.org`) now go without cookies; telling siblings from unrelated sites under `co.uk` would need the public suffix list. The subscription test's feed moved to its items' host, since it used to expect cookies on a foreign host.
- Validation: regression tests with two real servers (cookies on a same-origin redirect, none after a cross-origin one), the https→http decision, and the RSS rule end to end; core, frontend, nox and Win32 suites pass; workspace clippy and rustfmt clean. Moved three older entries to `docs/archive/CHANGELOG-2026-10-27--2026-10-27-release-1.0.md`.

## 2026-10-27_134446_1793108686: Security fix SEC-12: UPnP device URLs and the tracker loopback rule

- **UPnP:** a search reply or NOTIFY is used only when its `LOCATION` host is the responder's own IP, and a control URL from the description must stay on that host; otherwise the device is ignored or disabled. A LAN host can no longer point us at 127.0.0.1 or another machine. Line breaks in either URL were already refused by the SEC-19 parser.
- **Trackers:** the SSRF rule (only `/announce` on this machine) now also catches `0.0.0.0`, `::` and IPv4-mapped loopback, and runs for targets handed to a proxy (an address, or `localhost` and its subdomains), which skipped local resolution before.
- **Dropped during the work:** a check on the UPnP service type: the parser only ever takes one of the known WAN service names, so it could not carry a header; and a test case with `&#13;`, which the XML scanner does not decode.
- Validation: three regression tests (each case connected or was accepted before the fix); core, frontend, nox and Win32 suites pass; workspace clippy and rustfmt clean; core clippy clean for Linux and macOS targets. Not covered: peer lists aimed at LAN services (SEC-12 item 3).

## 2026-10-27_133749_1793108269: Security fixes SEC-02, SEC-01: cryptographic randomness

- **SEC-02 (fixed):** `util::random` is now a thread-local ChaCha20 generator (new `crypto/chacha20.rs`, RFC 8439 vector) keyed from the OS, with fast key erasure on every refill. The xorshift64* generator is gone, so MSE pads, peer IDs, tracker keys, UDP tracker/DHT/uTP ids, ISNs, the LSD cookie and node ids no longer follow from one leaked output; no call site changed. New `random_bytes`.
- **SEC-01 (fixed):** `crypto_random_bytes` uses `getentropy` (no file descriptor) then `/dev/urandom` on Unix, `BCryptGenRandom` on Windows, and panics instead of falling back to a weak generator.
- Tests: the review's state-recovery attack on generator output and on a real cleartext PadA, refills and threads, long OS reads, and the no-source panic. Core, frontend, nox and Win32 suites pass; workspace clippy and rustfmt clean; core clippy also clean for Linux, macOS and FreeBSD targets (not run on them).
- Cost: each `random_u64` is now a ChaCha20 output (four blocks per 28 values) instead of three shifts; random numbers are not on a per-byte path, and the suites ran no slower.

## 2026-10-27_133305_1793107985: Security fixes SEC-19, SEC-09, SEC-05

- **SEC-19 (fixed):** `parse_url_components` trims whitespace and refuses URLs with control characters, so a magnet `tr=`/`ws=`, an announce URL, a UPnP control URL or a redirect can no longer put CR/LF into a request line. `maybe_url_encode` keeps a lenient split. HTTP responses with a bare CR/LF in a header line are refused.
- **SEC-09 (partly):** `MerkleTree::add_hashes` checks the proof read-only before expanding to the full tree, so a peer's junk `hashes` message no longer allocates gigabytes. A torrent author's own giant v2 file still expands on valid hashes or the first block, as in libtorrent; a memory cap remains open.
- **SEC-05 (partly):** new `webui/path_policy.rs`: request paths must be rooted, without `..`, and on Windows plain drive paths (no UNC, `\\?\`, device or stream paths, which also stops NTLM leaks to `\\host`). New `nox run --webui-save-root DIR` (repeatable, command line only) limits adds, moves, category paths and the folder/IP-filter preferences to those folders. Only changed preference paths are checked, so existing relative CLI folders keep working. Refusal texts are English (like the other command errors), not translated.
- **Issue met:** the first policy used `is_absolute`, which on Windows refused `/data/new` (rooted on the current drive) and broke two Web UI tests; it now requires `has_root` and still refuses `C:dir`.
- Validation: a regression test for each (fails without the fix); core 739, nox (32 Web UI), frontend and Win32 suites pass; workspace clippy with warnings denied and rustfmt clean. The separate fuzz workspace has pre-existing `collapsible_if` clippy errors in untouched targets (`nox_http.rs`, `dht_message.rs`, `utp.rs`).

## 2026-10-27_132153_1793107313: Security review of SEC-01 to SEC-18

- Seven parallel Opus reviewers read the code for the 18 targets (read-only, no runtime tests); the key claim of every High and Medium item was rechecked by hand. Findings filled in `SECURITY.md`, with eight new items (SEC-19 to SEC-26) and a fix-order table.
- **High:** CRLF from magnet `tr=`/`ws=` and announce URLs reaches the HTTP request line, so a magnet can send commands to local services such as Redis (SEC-19). `MerkleTree::add_hashes` allocates the full block tree before checking the proof, so one `hashes` message can abort the client (SEC-09). A Web UI login can write anywhere as the nox user (SEC-05).
- **Notable Medium:** the cleartext MSE pad is raw xorshift output, which reveals the generator state behind peer IDs, tracker keys and DHT/UDP/uTP IDs (SEC-02); UPnP control-URL SSRF; cookies across redirects; plaintext remote auth; trusted imports; unbounded REJECT send buffers; seed mode without verification; metadata stall; web seed chunk-header growth; adopted ProgramData folder.
- Ruled out: decompression is not reachable from the network (SEC-13); TLS pinning is checked before data on all back ends (SEC-07 keeps hardening items). Nothing fixed yet.

## 2026-10-27_131050_1793106650: SECURITY.md with flagged audit targets

- **New:** `SECURITY.md`: reporting policy, threat model, and 18 audit targets (SEC-01 to SEC-18) with locations, what was seen, what to check, and an empty Finding for the reviewer. Nothing is verified yet.
- **Noted while flagging:** `crypto_random_bytes` silently falls back to xorshift if the OS RNG fails (SEC-01). Peer IDs, UDP tracker transaction IDs and uTP initial sequence numbers use the non-cryptographic generator (SEC-02). `csrf_ok` accepts requests with neither `Origin` nor `Referer` (SEC-03). `fetch` sends cookies to every redirect host (SEC-06).
- Committed locally, not pushed: the list should stay private until the items are fixed.
