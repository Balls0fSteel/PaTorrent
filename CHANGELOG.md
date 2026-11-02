# Changelog

Recent completed changes, issues and decisions. Timestamps: `YYYY-MM-DD_HHMMSS_<unix epoch>` (Europe/Budapest).

Earlier history: [archive index](docs/archive/README.md), including all 148 original entries from 2026-10-15 through 2026-10-23. Open archived entries only for a specific historical question. Current work: [PLAN.md](PLAN.md).

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
