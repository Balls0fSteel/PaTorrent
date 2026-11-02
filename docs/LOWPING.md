# Keep ping low (D49)

Written 2026-11-02_111726_1793614646; the owner answered Q3, Q4, Q5 and Q7 on 2026-11-02_112509_1793615109 and approved the rest as proposed on 2026-11-02_123805_1793619485 (Q1 the slices in this order, Q2 off by default, Q6 the 25 ms uTP target while on). Approved. Slice 1 is in 1.1.29 (2026-11-02_130357_1793621037); what it left out is under "Slice 1 status" below. Slice 2 is in 1.1.30 (2026-11-02_162406_1793633046), under "Slice 2 status"; slice 3 in 1.1.31 (2026-11-02_170156_1793635316), under "Slice 3 status"; the bloated-line test and its fixes in 1.1.32 (2026-11-02_211916_1793650756). The owner asked whether PaTorrent can find a safe upload speed by itself so games keep a low ping, and set aside a "gamer mode" driven by a list of game executables. It goes beyond 2.2.1 and is separate from the disk work ([DISKIO.md](DISKIO.md), proposed D48), though both end in the same kind of cap.

## Why ping rises

The home line's upload is the narrowest link. When PaTorrent sends faster than the line, the packets wait in the modem's or router's queue, and every other packet waits behind them: a game's, a video call's, a DNS query's. A few hundred milliseconds of queue is common (bufferbloat). Sending a little slower than the line keeps the queue empty and the ping at its idle value, whatever program is playing. So the feature does not need to know that a game runs; it needs to notice the queue.

The download direction has the same problem at the provider's end of the line. We can only steer it indirectly (by reading more slowly, so the senders' TCP backs off), with a delay.

## What we have

- **The cap's place:** the global peer class holds the session's `upload_rate_limit` / `download_rate_limit` (`crates/core/src/engine/peer_class.rs`), and `update_mixed_mode` already sets the tcp class's limits every tick. An automatic cap fits in the same tick.
- **uTP already measures the queue.** Every uTP socket keeps `send_delay` and `recv_delay`: the one-way delay above that connection's lowest seen delay, per direction (LEDBAT, `crates/core/src/utp/socket.rs`). uTP slows itself when it reaches `utp_target_delay` (100 ms), which is too late for games, and TCP peers never slow down for delay.
- **Network changes** are already noticed (the IP notifier), which is when a learned speed stops being valid.
- Nothing pings, and no limit changes by itself.

## Prior art

- **Tixati** has an "auto throttle" that pings or traceroutes and aims at a target ping. Its forum shows the risks: limits that sink far below the line (one report: 150 / 300 KB/s on a 1000 / 3500 KB/s line), fixed later by averaging longer before cutting.
- **Azureus/Vuze** had an AutoSpeed plugin of the same kind.
- **cake-autorate** (OpenWrt) does this on the router for all traffic: several ping targets, a continuous probe stream, and one-way delay where ICMP timestamp requests (type 13) are answered, which many networks block.
- A 2009 write-up of the method: treat the lowest latency of the last three minutes as the baseline and everything above it as queue.
- A router with SQM (CAKE) or a working gaming QoS does all of this better and for every device; the Help text says so.

## Proposal

One setting, **"Keep ping low (limit speeds automatically)"**, on the Bandwidth page. When on, the engine learns an automatic cap for the upload and for the download (owner, Q5: both, for gaming) and applies the lower of it and the user's own limit. The user's limit is never changed.

### Signals

1. **Probes (the main signal).** Four ICMP echo requests a second (about 250 bytes a second) to the first router past the home line:
   - **Finding it:** at start, at every network change and every 30 minutes, TTL-limited echoes (TTL 1-6). The target is the first hop that answers from an address that is not private (not RFC 1918, link-local or ULA; carrier-grade NAT 100.64/10 counts as past the line). Its delay includes the line's queue and little else. Double NAT (a provider router behind ours) is skipped the same way.
   - **Windows:** `IcmpSendEcho2Ex` / `Icmp6SendEcho2` with the TTL in `IP_OPTION_INFORMATION`; no administrator rights (as `tracert`). On a small thread of its own, so the engine thread never waits.
   - **Linux:** unprivileged ICMP sockets (`SOCK_DGRAM`, `IPPROTO_ICMP`), allowed when the process's group is inside `net.ipv4.ping_group_range` (open by default on current systemd distributions; to verify on Debian, Alpine and for nox's system user). Time-exceeded answers through `IP_RECVERR` (to verify on ping sockets; `tracepath`'s UDP method is the fallback). Registered with the mio reactor like the IP notifier.
   - **macOS:** `SOCK_DGRAM` ICMP sockets are open to every user; the time-exceeded path is to verify.
   - **FreeBSD:** no unprivileged ICMP; the uTP signal only.
   - **Fallback hosts** (owner, Q4): if no hop past the line answers, the probes go to a list in the Advanced table, `auto_limit_probe_hosts`, default `1.1.1.1,8.8.8.8,9.9.9.9`, which the user can edit if those stop answering (IPv6 addresses are accepted too). The probes are spread over the list (about one a second per host) and the median taken, since such hosts answer pings late now and then. Their longer path does not matter: its fixed delay is in the baseline, and only rises count. Those operators see our pings; the Help text says so. If none answers either, the uTP signal is used alone.
2. **uTP (free, no extra traffic).** The lowest `send_delay` among at least three busy uTP peers: a peer's own congestion only raises its own sample, so the minimum is the queue they all share, which is ours. The same with `recv_delay` for the download. It needs uTP peers to be busy; seeding to TCP-only peers gives nothing.

The probe's delay, minus its lowest value of the last three minutes, is the queue. The median of the last five probes is used, so one late answer (routers answer pings when they have time) cuts nothing.

### The controller

- **Cut:** when the queue stays above the allowance (15 ms by default; an Advanced setting) for one second *and* our upload is at least half the current cap (so the queue is probably ours), the cap becomes 80% of our upload rate over the last second.
- **Grow:** while the queue stays under half the allowance and our upload uses at least 75% of the cap, the cap grows 5% a second.
- **Floors:** never below 10 kB/s or 5% of the highest rate seen on this network, so a noisy Wi-Fi or a busy sibling's upload cannot starve PaTorrent (the Tixati lesson). If the queue is high while we send little, someone else fills the line and the cap does not move.
- **Memory:** the cap learned on a network (keyed by the gateway's hardware address where the system reports it, since many homes share `192.168.1.1`) is saved with the session state and is the starting point next time, so a restart does not begin with a burst.
- **Shared with D48 part 4:** the same baseline-and-cap controller, fed with disk delays there and network delays here; the engine applies the lowest cap of all. D49 is approved first, so it builds the controller and D48 part 4, if approved, reuses it.

### Download (Q5, answered: yes, under the same checkbox)

The same controller on the download channel. A round trip cannot say which direction is queued, so the direction is chosen from what we do: the one near its cap is cut (both, if both are); the uTP peers' `send_delay` and `recv_delay` tell the directions apart where available. The download reacts more slowly, because the senders' TCP needs a few round trips to notice that we read more slowly, so its cut keeps a little more margin (75% instead of 80%).

### uTP's own target (Q6, answered: yes)

While the setting is on, `utp_target_delay` drops from 100 ms to 25 ms (the user's own value returns when it is turned off), so uTP peers give way before the controller has to act. No message changes; uTP connections simply yield sooner. `PEER_ID_PREFIX` stays.

### What the user sees

- The Bandwidth page (owner, Q3: Advanced is only for what people should not touch) gets 2.2.1's missing "Global Rate Limit Options" group (owner, Q7) with the new box at its end:

```
Global Rate Limit Options
  [ ] Apply rate limit to transport overhead
  [ ] Apply rate limit to uTP connections
  [ ] Stop transfers on user interaction
  [ ] Keep ping low (limit speeds automatically)
```

  - **Transport overhead:** counts the TCP/IP and uTP headers against the limits (libtorrent 1.x `rate_limit_ip_overhead`, dropped in 2.0); to port. The automatic cap always counts them, since the line carries them.
  - **uTP connections:** the engine already can (the global class allowed or disallowed for uTP sockets through the peer class type filter, PORTING.md); this box sets it.
  - **Stop transfers on user interaction:** compare its exact behaviour with the owner's 2.2.1 before building it. It is a GUI feature (input idle time: Win32 `GetLastInputInfo`; GTK has no portable equivalent, so it may be Windows only); nox and the Web UI do not show it.
- While the automatic cap is below the user's limit, the status bar's upload reads "U: 412 kB/s (auto 450)" and its tooltip says why; the Web UI shows the same. The Logger notes the target hop found, and each change of the cap by more than 20% (at most one line a minute).
- Advanced table: the allowance in ms and the fallback probe hosts.

### Not proposed

- **A game list** (owner's idea, set aside): it would need a list kept up to date, a download from GitHub and scanning the running processes; the queue measurement covers every game, video calls and anything else without knowing them.
- **Full-screen detection** (owner, D48 Q3: not wanted).
- Shaping other programs' traffic: only a router can.

## Settings, wire and scale

- New `Settings` fields (PaTorrent extensions, not libtorrent names): `auto_rate_limit` (bool, both directions), `auto_limit_delay_ms` (15), `auto_limit_probe_hosts` (`1.1.1.1,8.8.8.8,9.9.9.9`), the overhead box, and the uTP box as a setting over the existing type filter; "stop on user interaction" is a GUI setting. Remote protocol 12 (shared with D48 if they land together); the Web UI gets the checkbox; the qBittorrent shim has no equivalent and does not expose it.
- No new dependencies (`windows-sys` features `Win32_NetworkManagement_IpHelper` and `Win32_Networking_WinSock` are already in core; Unix through `extern "C"` as `net/sys.rs`).
- Per session, not per torrent: nothing is added to a torrent (D21).

## Testing

- **Controller:** unit tests against a simulated line (a queue drained at a fixed rate, with a game's ping through it): the cap must settle within 10-20% under the line's rate without oscillating, recover when another program's upload stops, and respect the floors.
- **Real:** on the Xubuntu VM, `tc` shaping the uplink to a few Mbit/s with a deep `tbf` queue (bufferbloat on purpose), PaTorrent seeding to the lab's clients, a steady ping through the same link: idle ping, ping with the setting off, ping with it on, and the upload rate reached. Then the owner's own line with a game.
- **Probes:** hop discovery on this PC (Windows), the VM (Linux), behind double NAT and with an ISP that drops TTL-expired answers.

## Order and size

Medium to large, in slices, each a patch version (D37):

1. The "Global Rate Limit Options" group (overhead, uTP, user interaction), then the cap plumbing, the controller for both directions, the status bar and Logger texts, and the probes with the fallback hosts on Windows (most gamers).
2. Probes on Linux and macOS.
3. The uTP signal, as a second signal and the FreeBSD one.

## Slice 1 status (1.1.29, 2026-11-02_130357_1793621037)

- **Done:** the settings (`auto_rate_limit`, `auto_limit_delay_ms`, `auto_limit_probe_hosts`, `rate_limit_ip_overhead`, `rate_limit_utp`; remote protocol 12), the Bandwidth page's group in all three frontends, the probes on Windows (`net::probe`: TTL search, direct echoes or expired ones from a router that ignores its own, the fallback hosts each against its own idle round trip), the controller for both directions (`engine::auto_limit`), the 25 ms uTP target, the status bar's "(auto ...)" in both GUIs and the Web UI, and Logger lines. Checked on this PC: the first router past the line found at hop 2, 1-2 ms round trips.
- **Changed while building:** a new target starts with four seconds at half the current rate, so the idle round trip shows even when the line is already full; and a cut waits while the queue from the last cut is still draining, so a deep modem buffer does not drive the caps to the floor (the Tixati lesson). The automatic caps count what the global limits count: the measured feedback makes up for the headers, and the transport overhead box decides whether they are counted.
- **Left out:** "Stop transfers on user interaction". 2.2.1 stores it as `sched_interaction` among its scheduler keys (found in the unpacked exe), off by default, and nothing online says what it does; the owner's copy should show it before it is built. Also not yet: the per-network memory of the caps (keyed by the gateway's hardware address), the status bar tooltip saying why, and the lab VM test with a shaped, bloated uplink.

## Slice 2 status (1.1.30, 2026-11-02_162406_1793633046)

- **Done:** pings on Linux and macOS through unprivileged ICMP sockets (`SOCK_DGRAM` / `IPPROTO_ICMP`, and `IPPROTO_ICMPV6` for IPv6 fallback hosts), made by hand-declared `socket` and `poll` calls and used through `std::net::UdpSocket` (D4: no libc crate). Linux: the kernel sets the identifier and hands a socket only its own replies; routers' time-exceeded answers are read from the error queue (`IP_RECVERR`, `recvmsg` with `MSG_ERRQUEUE`, the offender's address after `sock_extended_err`). macOS: the IPv4 messages arrive with their IP header and every echo reply on the machine does, so ours are told by identifier and sequence; time-exceeded answers arrive on the socket itself, quoting our echo. When Linux refuses the socket, the Logger names `net.ipv4.ping_group_range`. Unit tests for the packets and their parsing; type-checked and linted for Linux, macOS (both architectures) and FreeBSD.
- **Run on Linux (2026-11-02_165221_1793634741):** on the Xubuntu VM (Ubuntu 26.04, bridged, `ping_group_range` `0 2147483647`) the trace saw the same hops as Windows, the routers' time-exceeded answers coming through `IP_RECVERR`; the first router past the line was found at hop 2 and answered in 1.5-2 ms, and a session logged it. With `ping_group_range` set to `1 0` the Logger named the setting.
- **Not yet run:** macOS (no Mac in the lab), so its time-exceeded delivery is unverified; and whether Debian, Alpine and nox's system user allow ping sockets by default.

## Slice 3 status (1.1.31, 2026-11-02_170156_1793635316)

- **Done:** each uTP socket now records when it last measured `send_delay` (from the peer's acknowledgements of our data) and `recv_delay` (from the timestamps of any packet the peer sends, so an upload's acknowledgements count for the download too). `UtpManager::shared_delays` gives, per direction, the lowest delay among the sockets that measured one in the last two seconds and how many did; with at least three, that direction's queue is the lower of the pings' and the uTP peers', so the uTP peers tell which direction a ping's queue is in, and without pings (FreeBSD, OpenBSD, or nothing answering) they are the only signal. The Logger says "uTP peers' delays in use" once.
- **Checked:** a simulated uTP transfer (the sender measures both directions, the receiver the download only); the combining rule; three uTP seeds on loopback reach the Logger line here (with pings) and on the FreeBSD VM (15.1, uTP only).
- **Still open:** the queue-and-cut behaviour against a real bloated line (the `tc` test), for both signals. Done in 1.1.32, below.

## The bloated-line test (1.1.32, 2026-11-02_211916_1793650756)

`tools/bloated-line/line.sh` builds a line in Linux network namespaces on the Xubuntu VM: home (192.168.77.2) → a modem whose upload is a 4 Mbit/s token bucket with half a second of buffer → the ISP router 84.1.1.1 (hop 2, past the line) whose 8 Mbit/s download to the modem has the same → four swarm sessions at 203.0.113.10. Idle round trip 8.5 ms. `crates/core/tests/bloated_line.rs` (ignored) runs the swarm and the client; the client spends a minute without keep ping low, then three with it, and prints each second's rates, caps, its own `ping` to the router and the bytes on the home interface.

| Run | Without: rate, ping median / 90% | With: rate, ping median / 90% |
|---|---|---|
| Upload, TCP | 479 kB/s, 478 / 508 ms | 440 kB/s (92%), 9 / 69 ms |
| Download, TCP | 948 kB/s, 473 / 512 ms | 756 kB/s (80%), 9 / 23 ms |
| Upload, uTP | 475 kB/s, 108 / 109 ms | 429 kB/s (90%), 9 / 30 ms |
| Download, uTP | 952 kB/s, 108 / 108 ms | 788 kB/s (83%), 10 / 46 ms |

(Each phase's last two thirds. uTP alone holds the queue at its 100 ms target; keep ping low brings it to idle.) The first runs failed, and found four faults, fixed in 1.1.32:

- **The idle round trip was learned from a full line.** The router is found before the first one-second step measured a rate, so the half-rate learning saw a rate of 0 and did nothing; the idle value then came from a 400 ms line, and the queue looked like noise. Learning now waits for the first step's rate.
- **Four seconds of half rate did not drain the line.** The system's send buffers (Linux sizes them to the bloated round trip) hold seconds of data that still leave at the line's rate. Learning now lasts until the lowest round trip stops falling (at least 4 s, at most 20 s).
- **The session counted upload and download bytes half again too high under a rate limit.** Payload counts per whole 16 KiB block, and a write or read that ended inside one counted its part as protocol, then the whole block as payload once it finished. `PeerConn::protocol_bytes` now carries the difference, so the totals match the bytes on the interface (they affected the session and torrent byte counts and rates everywhere, not only this feature). Separately, a cut is never above the current cap: the rate counts what the send buffers took, which can exceed the cap for a while.
- **The 100 ms bandwidth tick sent each tenth of a second's quota at once**, a queue on a slow line by itself: the cap could not pass ~80% of the line. While keep ping low holds a cap the tick is 25 ms.

## Questions for the owner

All answered.

- **Q1.** Answered: approved, with the slices in this order.
- **Q2.** Answered: off by default.
- **Q3.** Answered: the Bandwidth page; the Advanced table only holds what people should not touch.
- **Q4.** Answered: fallback hosts in the Advanced table, default `1.1.1.1,8.8.8.8,9.9.9.9`, editable.
- **Q5.** Answered: the download too, for gaming (under the same checkbox).
- **Q6.** Answered: yes, uTP's target delay is 25 ms while the setting is on.
- **Q7.** Answered: yes, 2.2.1's "Global Rate Limit Options" group comes in the same change.

## Sources

- [Tixati forum: auto bandwidth limiter reports](https://forum.tixati.com/support/7651)
- [Tribler: upload speed estimation and earlier work (Azureus AutoSpeed, eMule USS)](https://tribler.org/UploadSpeed)
- [cake-autorate](https://github.com/lynxthecat/cake-autorate)
- [apenwarr, "Stuffing the stuff" (2009): baseline latency over three minutes](https://apenwarr.ca/log/20091029)
- [IcmpSendEcho2Ex](https://learn.microsoft.com/windows/win32/api/icmpapi/nf-icmpapi-icmpsendecho2ex)
