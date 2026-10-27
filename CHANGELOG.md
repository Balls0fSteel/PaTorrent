# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

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

## 2026-10-27_124501_1793105101: Benchmarks against qBittorrent, Transmission, Deluge and rTorrent

- **New:** `tools/bench/bench.py` (Python, standard library; `deluge-client` for Deluge) measures daemons on one Linux machine: `many` (N generated torrents: add time, restart until all listed, RSS/PSS, idle CPU) and `transfer` (2 GiB from a reference qBittorrent seed over TCP via an in-harness tracker; speed and CPU per GiB between 10% and 80% from the kernel's `ss` byte counters; file SHA-1 checked). `summarize.py` writes medians as Markdown, HTML tables and bar charts, and fills `site/benchmarks.html` between `<!-- bench:NAME -->` markers. Method and limits: `docs/BENCHMARKS.md`; raw results: `docs/benchmarks/2026-10-27-ubuntu-vm.jsonl`.
- **Results** (Xubuntu VM .132: 8 vCPU, 8 GB, Ubuntu 26.04.1; the published 1.0.1 tarball against Ubuntu's qBittorrent 5.1.4, Transmission 4.1.1, Deluge 2.2.1/libtorrent 2.0.12, rTorrent 0.9.8): with 10,000 torrents after a restart PaTorrent used 60 MB RSS (qBittorrent 612, Transmission 327, Deluge 701, rTorrent 2,142) and listed all in 0.19 s (0.86, 1.00, 5.76, 22.6); right after adding them 319 MB (636, 328, 905, 2,101), since a restart leaves inactive torrents' metadata out. Download from one peer: 3,592 MiB/s at 1.04 CPU s/GiB (qBittorrent 1,471 / 2.14, Deluge 1,566 / 2.11, rTorrent 1,924 / 0.85 but slowing to ~32 MB/s near the end, Transmission 62.4 / 2.66).
- **Website:** new Benchmarks page (nav on every page, Docs list, WEBSITE.md), CSS bars validated with the dataviz palette checker (accent vs neutral, light and dark); strict build and the 24 site tests pass; checked in the browser at desktop and 375 px dark. Pushed with the owner's go-ahead; Pages run 40509213942 deployed it and `benchmarks.html` answers 200 with the numbers.
- **Issues met:** VMware gives no steal time, so a busy host inflated single runs (PaTorrent once 23 CPU s/GiB, Deluge 35; alone, PaTorrent ran 3,550 MiB/s at ~1.0 six times): the transfer runs are repeated five times in rotating order and reported as medians. Ubuntu's AppArmor profile confines `transmission-daemon` to `/var/lib/transmission-daemon`; unloading it was refused (security), so the harness runs it as `debian-transmission` via `sudo runuser`, with a tmpfs on its downloads folder. Transmission does not dial 127.0.0.1 peers, so the tracker hands out the VM's LAN address. Clients trying uTP first wait ~3.5 s (Transmission ~23 s) for the TCP-only seed; that wait is excluded.

## 2026-10-27_112333_1793096613: 1.0.1 published (recorded after the fact)

- The 1.0.1 release was tagged and published from a parallel session that did not record it: tag `v1.0.1` on `afce628b`, Release run 40485598989 green (8 of 8 jobs, 14 files, 124 MB), published as Latest at 10:21 UTC. Pages runs 40487838137 and 40487838045 built `update.txt` with `stable=1.0.1` and deployed; a fetch a minute later still had the old file (CDN `max-age=600`).
- PLAN.md now shows 1.0.1 as published and V1.1 as the unreleased work on `main`; the V1.1 entry below was written while the plan still said 1.0.1 was untagged.

## 2026-10-27_112013_1793096413: V1.1: search box and importers from qBittorrent and µTorrent (D35)

- **Decision D35** (owner): the toolbar's search box filters the torrent list by name; compiled-in plugin hooks move to V2.0 (no first user yet).
- **Search:** `TorrentList::set_search` (frontend `model.rs`) applies a case-insensitive name match after the category or label filter, without allocating for ASCII text. At 100,000 rows a keystroke costs 12-16 ms in release. Win32 has an edit box at the toolbar's right end with Ctrl+F, Escape and Enter; list accelerators (Delete, Ctrl+A) are skipped while it has focus. GTK has a `SearchEntry` (subagent) whose list shortcuts are off while it has focus. The Web UI's own search now uses the model's.
- **Import** (`frontend/src/import.rs`): File > Import from qBittorrent / µTorrent in both GUIs (grayed in remote mode), and `patorrent-nox run --import CLIENT [--import-dir DIR]`. Each torrent becomes a libtorrent resume record with the `.torrent`'s info dictionary embedded, added through the existing `TorrentSource::Resume`, so the engine needed no new call. qBittorrent's `.fastresume` is taken as it is (`qBt-name`; category, else the first tag, as the label). µTorrent's `resume.dat` is mapped: `have` is LSB-first, `prio` `0x08` normal and `0x80` skip, both read from the owner's real 2.2.1 file; a renamed top folder becomes `mapped_files`. Magnets without metadata are added by hash. The import refuses while the source client runs; torrents already in the list and ones it cannot read are listed in the Logger.
- The engine trusts claimed pieces only where their files exist; otherwise it checks the files. A stopped torrent is verified when started, as in libtorrent. µTorrent's own part file (`~uTorrentPartFile_*.dat`) is not read, so pieces shared with skipped files are downloaded again.
- 15 new strings in all 34 tables. The translations are machine-made (subagent); ar/fa/he mnemonics and wording in inflected languages need native review.
- Validation: unit tests and three end-to-end tests on a real session (trusted pieces under a renamed folder, a full check when files are missing, a qBittorrent record as it is). A read-only dry run of the owner's real folders read all 47 qBittorrent torrents and the one µTorrent torrent with no skips. `nox run --import utorrent` against an isolated state folder, behind a dead proxy, added the torrent with its label. Workspace clippy (warnings denied) and fmt are clean; frontend, nox and Win32 tests pass; GTK clippy passes for Linux with the stand-in pkg-config. Not verified: the Win32 and GTK windows by hand (computer use is blocked for PaTorrent here).
- Moved the three oldest root entries verbatim to `docs/archive/CHANGELOG-2026-10-27--2026-10-27-installer.md`.
