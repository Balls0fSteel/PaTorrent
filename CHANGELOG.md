# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

## 2026-11-02_223318_1793655198: Why qBittorrent "refused" our uTP connections

- **Cause:** right after a torrent's check, qBittorrent keeps it paused in libtorrent for a few seconds while its API says `stalledUP`; with no other active torrent libtorrent drops incoming connections ("no active torrents", no alert), which for uTP is a FIN acking our SYN, and we then wait 15 s before TCP, as libtorrent does. Found from libtorrent's source, a capture, a 5 s pause and an unrelated active "keeper" torrent (each makes every run connect over uTP at once), and plain libtorrent 2.1.1 accepting every time. No engine change: `tools/qbt-interop` waits 5 s after "checked" (then 0.5-1 s "down" runs over uTP on 2.0.15). BACKLOG records the optional quicker give-up.

## 2026-11-02_221313_1793653993: Version 1.1.33: the release checks for 1.1.18-1.1.32

- **Security review** of everything since v1.1.17 ([SECURITY-REVIEW.md](docs/SECURITY-REVIEW.md#release-1132-since-v1117)), 15 Low findings fixed, each with a test: the self-update runs the setup only as checked (locked and hashed again before it starts), writes no `.part` through a planted link, refuses https-to-http redirects, and its streamed fetch stops at the length and the limit; a SEC-26 bypass through a URL-parser mismatch in search downloads (older than 1.1.17); watched folders refuse oversized files before reading; duplicate file names resolve in linear work (8,000 took 29.8 s); resume `pieces` read back as saved; ICMP replies only from the host pinged, IP headers and the Linux error queue read within bounds; `hash_request_cost` counted one proof sibling too few (a piece-layer request with proofs cost 0 against the hash budget; a full piece-layer fetch now measures `P × (log2(P) - 8)`). Hardening: RSS validators, qBittorrent share limits, version length. New fuzz target `icmp` (10.8 M runs), `web_seed` takes a referrer. Open items in BACKLOG "Security and TLS".
- **Interop** against qBittorrent 5.3.0 on libtorrent 2.0.15 and 2.1.2 (new `tools/qbt-interop`): v2-only and hybrid with odd file sizes 48/48, forced RC4 8/8, slow-torrent queueing as specified. No engine change. Found: qBittorrent often refused our outgoing uTP with a FIN (explained in the entry above).
- **Windows, on the clean VM:** `setup-test.ps1` upgrading the published 1.1.17 setup: 24/24. With new `tools/windows-clean/desktop` scripts, driven in the locked console session by window messages: the self-update for one user end to end (box, progress, Cancel, install, restart not elevated) and the closing window's Quit Now during real 12 GB / 4 GB moves (copies deleted, files whole, a restart seeding). Not scriptable: the all-users update's UAC prompt.
- **Update progress text:** sizes in the usual units ("24.1 % (768 kB / 3.11 MB)"), not whole MB ("0 / 3 MB").

## 2026-11-02_211916_1793650756: Version 1.1.32: keep ping low against a bloated line (D49)

- **The test:** `tools/bloated-line/line.sh` builds a 4 / 8 Mbit/s line with half a second of buffer in network namespaces on the Xubuntu VM; `crates/core/tests/bloated_line.rs` (ignored) runs a four-session swarm and the client, a minute without keep ping low, then three with it. Ping to the first router past the line: ~480 ms without (TCP; uTP's own 108 ms), 9-10 ms median with, at 92% / 80% of the line's TCP upload / download rate and 90% / 83% with uTP. Results in `docs/LOWPING.md`.
- **Found and fixed:** the idle round trip was learned from the full line (the router is found before any rate is measured, so the half-rate learning did nothing; it now waits for the first measured second and lasts until the lowest round trip stops falling, 4-20 s); a cut could raise the cap (the rate counts what the send buffers took); the 100 ms bandwidth tick sent bursts that alone queued on a slow line (25 ms while keep ping low caps).
- **Byte counting:** under a rate limit the session's (and torrents' and peers') protocol bytes counted the part of a block a write or read ended in, then the whole block again as payload, so totals and total rates ran about half again above the line. `PeerConn::protocol_bytes` carries the difference; the totals now match the interface's bytes.

## 2026-11-02_170156_1793635316: Version 1.1.31: keep ping low, slice 3 (D49)

- **The uTP signal:** uTP sockets record when they last measured their upload (`send_delay`) and download (`recv_delay`) queueing delays; `UtpManager::shared_delays` gives the lowest per direction among those measured in the last two seconds. With three or more such peers, a direction's queue is the lower of the pings' and theirs, so the uTP peers place a ping's queue in one direction; without pings they are the only signal (FreeBSD, OpenBSD, nothing answering). Logger: "uTP peers' delays in use", and the no-pings texts now say the uTP peers are used instead of "no automatic limits".
- **Fix while testing:** a download delay of zero is a measurement too (it was taken for "none").
- **Tests:** a simulated transfer (who measures what), the combining rule, and an ignored swarm test with three uTP seeds, passing here and on the FreeBSD VM (FreeBSD 15.1, reached over SSH for the first time; its host key is now known). The router test skips on systems without pings.

## 2026-11-02_165221_1793634741: Keep ping low runs on Linux (D49)

- **Xubuntu VM (Ubuntu 26.04):** the probe and session tests pass. A new ignored test, `probe::tests::real_trace`, prints each hop's answer: the same path as from Windows (home router, the ISP's router at hop 2, 1.1.1.1 at hop 6), the time-exceeded answers arriving through `IP_RECVERR`; round trips 1.5-2 ms with sub-millisecond resolution. With `net.ipv4.ping_group_range` at `1 0` (restored afterwards) the Logger names the setting. macOS is still unrun.
- **Lab:** the VMs had no network; VMware's bridge (VMnet0, "Automatic") had to be pointed at the physical Ethernet adapter, since this PC also has VPN, VirtualBox and Hyper-V adapters. Test code and docs only, no version bump.

## 2026-11-02_162406_1793633046: Version 1.1.30: keep ping low, slice 2 (D49)

- **Linux and macOS pings:** unprivileged ICMP sockets (`SOCK_DGRAM` / `IPPROTO_ICMP`, ICMPv6 for IPv6 fallback hosts) through hand-declared `socket` and `poll` and `std::net::UdpSocket` (no libc crate, D4). Linux reads routers' time-exceeded answers from the error queue (`IP_RECVERR`, `MSG_ERRQUEUE`); macOS gets them on the socket, IPv4 with the IP header, and tells our echoes by identifier and sequence. A refused socket on Linux names `net.ipv4.ping_group_range` in the Logger. FreeBSD and OpenBSD still have none (slice 3's uTP signal).
- **Checked:** unit tests for the echo request, its checksum and the parsing of replies and time-exceeded answers; clippy clean for Windows, Linux, macOS (x86_64, aarch64) and FreeBSD. **Not checked:** a real run on Linux (the Xubuntu VM was off) or macOS (no Mac).

## 2026-11-02_130357_1793621037: Version 1.1.29: keep ping low, slice 1 (D49)

- **Bandwidth page:** 2.2.1's "Global Rate Limit Options" group in all three frontends, with "Apply rate limit to transport overhead" (libtorrent 1.x's `rate_limit_ip_overhead`, ported; off), "Apply rate limit to uTP connections" (`rate_limit_utp`, on; switching it moves connected uTP peers too) and "Keep ping low (limit speeds automatically)" (`auto_rate_limit`, off). Defaults from the owner's 2.2.1 `settings.dat` and screenshot. Advanced: `auto_limit_delay_ms` (15), `auto_limit_probe_hosts`. Remote protocol 12.
- **Engine:** `net::probe` finds the first router past the line (TTL 1-6) and pings it four times a second on its own thread (Windows `IcmpSendEcho2Ex` / `Icmp6SendEcho2`; elsewhere a Logger line says pings are not available yet), or the fallback hosts. `engine::auto_limit` caps each direction from the queue (median of five round trips above the three-minute minimum): 80% / 75% of its rate when the queue passes the allowance and the rate is near its cap, +5% a second when it clears, floors 10 kB/s or 5% of the peak. Added while building: a four-second half-rate start on a new target, and no further cuts while the queue drains. uTP's target is 25 ms while on. Status bars show "U: ... (auto 450 kB/s)".
- **Not built:** "Stop transfers on user interaction" (2.2.1's `sched_interaction`; its behaviour is undocumented, to try in the owner's copy), the per-network memory, the status bar tooltip; Linux and macOS pings are slice 2.
- **Tests:** controller simulation against a bloated line, meter, probe search and fallback with a fake route, overhead estimate, and two swarm tests for the uTP box (from the start and while connected). Live on this PC: router at hop 2, 1-2 ms. The 34 translations were written by subagents and checked by the language tests.

## 2026-11-02_123805_1793619485: Keep ping low approved (D49)

- **Owner:** "Approve as proposed": Q1 the slices in the proposed order (the Global Rate Limit Options group, the controller and Windows probes; then Linux and macOS probes; then the uTP signal), Q2 off by default, Q6 `utp_target_delay` 25 ms while on. D49 moved to the active decisions; LOWPING.md marked approved. D49 builds the shared controller that D48 part 4 would reuse.

## 2026-11-02_112509_1793615109: Keep ping low: the owner's answers (D49)

- **Q3:** the checkbox goes on the Bandwidth page; the Advanced table is only for what people should not touch. **Q4:** when no router past the line answers, probe an editable Advanced list, default `1.1.1.1,8.8.8.8,9.9.9.9`. **Q5:** the download is limited too, under the same checkbox. **Q7:** 2.2.1's "Global Rate Limit Options" group (transport overhead, uTP connections, stop transfers on user interaction) comes in the same change.
- LOWPING.md updated: the Bandwidth page layout, the fallback hosts, the direction chosen from which of our rates is near its cap, the settings. Found while writing: the overhead option needs porting (libtorrent 1.x `rate_limit_ip_overhead`), the uTP option already exists underneath, and "stop on user interaction" is GUI-side and may be Windows only. Open: Q1, Q2, Q6.

## 2026-11-02_111726_1793614646: Keep ping low proposal (D49)

- **Owner's request:** make the game-ping question a proposal. Written as [LOWPING.md](docs/LOWPING.md), proposed D49, questions Q1-Q7; no code.
- **Design:** one setting, off by default. ICMP probes to the first router past the home line (found by TTL; no admin rights needed on Windows, Linux and macOS, FreeBSD has none) plus the uTP peers' one-way delays (`send_delay` / `recv_delay`, already measured); the upload is capped at 80% of its rate when the queue stays over 15 ms and grows back 5% a second, with floors. It reuses D48 part 4's controller. No game list: the queue covers every game and call.
- **Prior art checked:** Tixati's auto throttle (caps sinking far too low, fixed by longer averages), Azureus AutoSpeed, cake-autorate on routers.

## 2026-11-02_111330_1793614410: Smart disk I/O: the owner's first answers (D48)

- **Q3:** disk I/O only; CPU load is fine. **Q8:** "smart recheck" means gentle, not fewer. DISKIO.md part 1 now uses every platform tool: background I/O and memory priority (Windows), `ioprio_set` idle per thread (what `ionice -c 3` does; Linux), `IOPOL_THROTTLE` (macOS); keeping other programs' file cache (`posix_fadvise`, `F_NOCACHE`); giving way when the check's own reads slow down, which works on NVMe and a NAS where the priorities do nothing; and the cap.
- **Ping for games:** answered in BACKLOG (networking): a delay-measuring upload limit works without a game list; uTP's LEDBAT already backs off but at 100 ms. Also recorded: our Bandwidth page lacks 2.2.1's "Global Rate Limit Options" group.

## 2026-11-02_110639_1793613999: Smart disk I/O proposal (D48), search engines and themes reviewed

- **Owner's idea:** smart recheck, throttling reads and writes on high disk I/O, SSD / HDD / NAS detection. Written up as [DISKIO.md](docs/DISKIO.md), proposed D48, questions Q1-Q8; no code.
- **Findings that shaped it:** all checks run on the one `lt-hash` thread, one after another for every drive, at normal priority and uncapped; `active_checking` is in `Settings` but unused; the `aio_threads` take jobs by `piece % threads`, so all ten can seek on one HDD; the write back-pressure (`disk_backlogged`) slows downloads to the disk's pace but leaves no room for other programs; nothing detects drive types.
- **Search engines:** 2.2.1's "Search Engines [name|URL]" list (Advanced > UI Extras) is not built; worth it with an empty default list (BACKLOG "V1.1"). The BACKLOG's search entry still said the Torznab UI was not built; corrected.
- **Themes:** options recorded in BACKLOG "Themes": icon skins first, a colour file, Win32 dark mode as a separate decision (undocumented `uxtheme` calls). 2.2.1's skin file names could not be read from its UPX-packed exe.
- Docs only, no version bump.
