# PaTorrent plan

Last updated: 2026-11-05_183712_1793900232

Read this file for current work. Load only the relevant sections of [detailed follow-ups](docs/BACKLOG.md) or [decisions D1-D58](docs/DECISIONS.md) when needed. Completed phase checklists and old reports are in the [history index](docs/archive/README.md); they are not the active plan.

## Vision

A lean recreation of µTorrent 2.2.1: native look, small single-file binaries, and 100,000 torrents with low idle CPU and a responsive UI on a modest PC. The engine is our Rust port of libtorrent-rasterbar 2.0.11, used as a reference and never linked.

| Binary | Platform | Role |
|---|---|---|
| `patorrent` | Windows 10+ x64 | Native Win32 GUI |
| `patorrent-nox` | All supported platforms | Daemon, native push API, Web UI, qBittorrent-compatible shim |
| `patorrent-gtk` | Linux / BSD / macOS | GTK 4 GUI on the shared frontend model |

Design: [ARCHITECTURE.md](docs/ARCHITECTURE.md), [PORTING.md](docs/PORTING.md), [WEBUI.md](docs/WEBUI.md), [INTEGRATIONS.md](docs/INTEGRATIONS.md). Release procedure: [RELEASING.md](docs/RELEASING.md).

## Current status

- **Releases:**
  - 1.0.1 (2026-10-27) was the first stable release, with the full 1.0 scope (D19). `v1.0.0` has no release because its build failed (D34). The open Road to 1.0 hand checks continue as 1.0.x validation.
  - 1.1.0 (2026-10-28) added V1.1 (D35), the security fixes and the icons. Its peer ID is `-PA1011-`.
- **1.1.17** (2026-10-30, [notes](docs/releases/1.1.17.md)): Torznab search, RSS in the hosted Web UI, nox's update check; the website's News page.
- **1.1.40 is published, Latest** (2026-11-03, tag `v1.1.40`, [notes](docs/releases/1.1.40.md); peer ID `-PA1012-`, remote protocol 13): keep ping low (D49), smart disk I/O (D48), the closing window (D47), the Windows self-update (D44-D45) and the libtorrent 2.0.12-2.1.2 fixes; `update.txt` serves `stable=1.1.40`.
- **1.2.0** (1.1.41-1.1.78, tagged 2026-11-05; [notes](docs/releases/1.2.0.md)): selected-files progress for stopped torrents; the closing window's timing and size; profiles and portable mode (D42); D50-D55; the bug hunt and backlog fixes. CHANGELOG and its archive.
- **Benchmarks** (2026-10-27, [BENCHMARKS.md](docs/BENCHMARKS.md), `tools/bench`, on the site): 10,000 torrents after a restart use 60 MB against qBittorrent's 612 MB; a one-peer download runs at 3,592 MiB/s and 1.04 CPU s/GiB.
- Last recorded CI / FreeBSD / Fuzz and Release runs were green (recorded, not a live check). macOS builds and starts on the runner; real display, association, login and tray behavior still need a Mac.
- Scale baseline on this development PC (2026-10-23): Win32 lists 100,000 torrents in 1.16 s, GUI-thread idle CPU 0.05-0.10%, working set 232 MB. Name sort 49 ms, Ctrl+A 4.7 ms, add 19 ms, remove one 14 ms. The low-end-machine gate remains open.
- Verdigris (`01`) is the final brand (D57, 2026-11-04).

## Next steps

**Feature order chosen by the owner (2026-10-30_085652_1793347012):**

- **C, D.** RSS in hosted mode and Torznab search (D41, 1.1.12-1.1.19): done; hand checks in BACKLOG. No alternative speed limits in the Win32 GUI (owner, 2026-10-30).
- **E.** Profiles and portable mode (D42, [PROFILES.md](docs/PROFILES.md), [PROFILES_PLAN.md](docs/PROFILES_PLAN.md)): all slices built (1.1.43-1.1.54) and checked on the VMs, GNOME, KDE, Flatpak, AppImages and a real Mac; what is left is in [BACKLOG](docs/BACKLOG.md#profiles-and-portable-mode-d42). Shipped in **1.2.0** (D58); its website text goes out with the release's news entry.
- **F.** Microsoft Store release: D46 approved 2026-11-04 (free MSIX, individual account, settings shared with the setup copy, all markets and 34 languages, Verdigris icon); build per [STORE.md](docs/STORE.md).
- **G.** Smart disk I/O (owner's idea, 2026-11-02_110639_1793613999): approved and built (D48, [DISKIO.md](docs/DISKIO.md), 1.1.36-1.1.40); real-disk measurements of the busy-disk throttle in BACKLOG "Disk and storage". **Keep ping low** (D49, [LOWPING.md](docs/LOWPING.md)): built in 1.1.29-1.1.31; 1.1.32 passed a bloated-line test (ping ~480 ms to 9 ms at 80-92% of the line). Next: the owner's line with a game, a Mac run; the rest in BACKLOG, including 2.2.1's "Stop transfers on user interaction".
- **H.** Approved 2026-11-04 (D56): 2.2.1's web search engines (empty default list), icon skins, and dark mode; research in [BACKLOG](docs/BACKLOG.md#themes-researched-2026-11-02_110639_1793613999), each designed before code.
- **I.** The owner's six points (2026-11-04, [PROPOSALS.md](docs/PROPOSALS.md)), each merged with its own version: D50 Files filter (1.1.56, 1.1.64), D55 help text (1.1.57, 1.1.65), D54 "Use only this network" (1.1.58), D51 the search helper (1.1.59, 1.1.63, 1.1.67; real Jackett passed on Windows, Xubuntu and macOS, [SEARCH_HELPER.md](docs/SEARCH_HELPER.md); the guide [SEARCH_ON_WINDOWS.md](docs/SEARCH_ON_WINDOWS.md) is also on the [wiki](https://github.com/Balls0fSteel/PaTorrent/wiki)), D53 the Setup Guide (1.1.62). D52 the native macOS GUI: approved, not started; each slice needs a Mac. 1.1.66 fixes a remote engine crash (`reqq` 1-3). The 2026-11-05 bug hunt ([BUGHUNT-2026-11-05.md](docs/BUGHUNT-2026-11-05.md), 38 findings) is closed in 1.1.69-1.1.74 (a Web UI auth bypass among the highs). Backlog code items followed in 1.1.75-1.1.78 (GTK keys and shading, engine, updater and Win32, frontend and nox). 1.1.66, 1.1.70, 1.1.74 and 1.1.76 change wire behaviour, so 1.2.0 takes peer ID `-PA1013-` (owner, 2026-11-05). **Open:** a real VPN run, a real first start through the guide, the GUI looks; the Win32 test binary's rare heap fault under parallel load (BACKLOG "Validation"; run its tests on one thread meanwhile).

**Owner notes (2026-10-30_192440_1793384680):** all done (D44, D45, D47); hand checks in [BACKLOG](docs/BACKLOG.md#updates-installer-and-website).

0. **Owner hand-test findings (2026-10-27):** fixed in 1.0.2-1.0.8; re-test on a fresh Win32 build ([BACKLOG](docs/BACKLOG.md#owner-hand-test-findings-2026-10-27_225501_1793138101)).
1. Run the installer's wizard and the all-users UAC path once on a real PC, and register `PA` with the client-ID lists (item 14). `update.txt` serves `stable=1.1.40`.
2. Rerun `tools/bench` before each release and on bare metal when one is free ([BACKLOG](docs/BACKLOG.md#validation-and-development-environment)).
3. Hand-check V1.1 (below, implemented) and keep the 1.0.x validation going. Refresh the hand-test set with the latest Win32 dialog changes, then check the remaining dialogs for clipped translations and layout problems in the new font.
4. Complete the manual checks in Road to 1.0 item 9, starting with Add Torrent from URL, Preferences and the changed Add dialog. Use [BACKLOG.md](docs/BACKLOG.md) for the relevant feature's exact gaps.
5. Finish the outstanding platform, scale and interop checks in items 5-8 and 11-13. Record what was actually run and what remains unverified.
6. **Security fixes** (done 2026-10-27): every audit finding is Fixed or Ruled out ([SECURITY.md](SECURITY.md); the full audit is the untracked `SECURITY-AUDIT.md`). Native review still wanted for `RemotePlaintext`, `ForwardRefused`, `PrefCurrentPassword`, `PrefNeedCurrentPassword`.
7. **Sonarr/Radarr/Prowlarr** ([INTEGRATIONS.md](docs/INTEGRATIONS.md) part 1): gaps 1-3 are fixed or decided (1.1.3-1.1.4); real apps passed against nox and the hosted Web UI (2026-10-29, `tools/arr-test`). Still open: a magnet grab with real peers, and "First" priority / Force Start. Torznab search is item D above.
8. Keep Windows, Linux, macOS and FreeBSD CI green. MSRV is 1.85; avoid let-chains. Before pushing code, use stable clippy and rustfmt, including the separate fuzz workspace. Interop reruns are needed after peer-wire, picker or disk changes.

## Road to 1.0

Original gate numbers are retained for references in source comments and release docs. "Implemented" does not close a pending hand check. D19-D23 define the approved scope. Since D34 the open checks are 1.0.x validation, not release blockers; 1.0.1 shipped with them open (1.0.0's tagged build failed).

| # | Gate | Remaining work |
|---|---|---|
| 1 | BitTorrent v2 and hybrid (BEP 52) | Implemented, including v2 magnets, dual-hash announces and creation. Linux plain/RC4 interop passed with qBittorrent and Deluge. Keep the regression and final interop checks in item 12. |
| 2 | GTK 4 GUI | All five slices implemented, built and tested in CI; the owner approved a Linux run. Complete targeted GTK checks in the backlog, icon validation and packaging checks. |
| 3 | HTTPS Web UI and GUI remote mode | Implemented. Hand-check certificate pinning, reconnect/authentication failures and both GUIs driving nox. |
| 4 | Localisation | 34 tables compiled in, language selection and RTL implemented. Check GTK in RTL, remaining long-text layouts and machine translations with native speakers when available. |
| 5 | 100,000-torrent scale gate (D21) | Measure on low-end Windows and Linux (proposal: 2 cores, 4 GB RAM, HDD); measure GTK, the Web UI's per-client model and idle CPU, and bulk removal of 1,000 torrents (last observed 0.5-1.4 s of engine alerts). Fix measured bottlenecks. |
| 6 | Windows release zip | Done: the release job builds and smoke-tests the zip and the NSIS installer (D31). 1.1.2's zip and setup passed on a clean Windows 11 VM (2026-10-29, `tools/windows-clean`: start, static CRT, upgrade, uninstall, all users). Remaining, needing a desktop: SmartScreen on a real download, the wizard and UAC pages, "Run PaTorrent" not elevated, drag and drop, double-click associations. |
| 7 | Linux packages | x86_64/aarch64 tarballs, GUI/nox AppImages and Flatpaks build. Run desktop AppImage and Flatpak checks, including schemas, icons, tray, HTTPS, associations and the Background portal. Flathub still needs screenshots, submission changes and the owner's exception text; see RELEASING.md. |
| 8 | macOS (D23) | Secure Transport, CI, arm64 GTK bundle/dmg, Apple events, Login Items, associations and NSStatusItem are implemented. A real Mac must validate window content, opening files/magnets, login and tray behavior. Intel engine/nox build; the Intel GTK bundle remains absent. Current bundle requires macOS 15. |
| 9 | Manual GUI checks | Preferences (all pages, OK/Cancel/Apply, proxy and Advanced), Add (files/folders, priorities, labels, recent paths, real magnet OK/Cancel), Create, Properties (multi-select and DPI), RSS with a public feed, scheduler/hour boundary, Auto Shutdown with `PATORRENT_POWER_DRY_RUN=1`, Windows integration on the real registry, moving completed data / Set Download Location across drives, seeding goals and .torrent folders. Check Pieces progress and newly resized/font-changed dialogs. |
| 10 | Icon integration and validation | Bold UI family (D27) integrated in both GUIs. Remaining: non-100% DPI, the Trackers tab with live trackers, GTK light/dark, tray/About and macOS Retina/light/dark menu bars. |
| 11 | Service integration | Run the nox Windows service under the real elevated SCM, including restart-on-failure and shutdown behavior. Install and exercise FreeBSD rc.d with root (only syntax-checked so far). |
| 12 | Final interop rerun | Use release builds: plain, RC4, TCP, uTP, uTP-only and super-seed with qBittorrent, Transmission and Deluge; v2/hybrid where supported. Transmission/Deluge uTP and super-seed switches remain unverified. Known Linux results: Transmission rejects v2-only and stalls on hybrid pad-file expectations; qBittorrent/Deluge v1/v2/hybrid plain/RC4 passed. |
| 13 | Web UI on a real phone | Android/iOS: detail/action sheets, drawer, sort, long-press multi-select, Back behavior, Properties and reconnection. Also verify desktop keyboard, selection, columns, Preferences and dark mode on the real engine. |
| 14 | Release engineering | rc.1 and 1.0.1 (D34) are out; `docs/releases/<version>.md` is the release text. Keep the parser/security review (SECURITY-REVIEW.md) at each release. Register `PA` with libtorrent, Transmission, WebTorrent and the BEP 20 list. |

## Decisions and open questions

The approved decisions D1-D40 remain active in [DECISIONS.md](docs/DECISIONS.md). Keep the one-thread mio engine, OS TLS, lazy shared-store persistence, minimal dependencies, static portable Windows binary (an installer beside the zip, D31) and faithful 2.2.1 layout. Scale takes priority over empty-session memory. The PolyForm Noncommercial licence, PaTorrent identity and honest peer ID remain fixed. New dependencies need a written justification and changelog entry.

Q1-Q15 are resolved; their answers and the original Q10 interop setup are in the [plan snapshot](docs/archive/PLAN-2026-10-23.md). Current testing constraints and setup notes are in [BACKLOG.md](docs/BACKLOG.md#validation-and-development-environment).

- UI artwork (D27-D30): the bold family passed its revision 6 review (131/131) and is integrated (gate 10); revisions 3-6 and their reviews are in the [changelog archive](docs/archive/README.md). Optional: hand-drawn 20 px globe and pylon, dedicated 48/64 px drawings, and a web-ui globe matching the network globe.
- Compare Properties > Advanced and `rss.*` settings with the owner's real µTorrent 2.2.1 in `z_exclude_uTorrent/` (gitignored). Its Add dialog was inspected via scripted captures/control dumps; the remaining wording and behavior need comparison. The owner captured all 15 Preferences pages (General through Advanced > Run Program) as `z_exclude_uTorrent/screenshots/*.webp` on 2026-10-26: compare each page of our Preferences against them (Road to 1.0 item 9).
- D46 and D50-D58 were settled 2026-11-04_224953_1793828993; no decision is proposed.
- Decide the Flathub permission/submission approach (item 7); the recorded exception policy requires the owner's own text.


## V1.1

Released in 1.1.0 (D35: the name search, and importers from qBittorrent / µTorrent with D36). Hand checks are still open: the Win32 box at 100-200% DPI and its keys, the import dialogs in both GUIs, the µTorrent `started` mapping, and native review of the new strings. See [BACKLOG.md](docs/BACKLOG.md#v11-importers-search-and-hooks).

## V2.0 goals

Owner priorities recorded 2026-10-21 and reaffirmed 2026-10-23. Goals 1 and 3 are designed in [PROFILES.md](docs/PROFILES.md) and planned in [PROFILES_PLAN.md](docs/PROFILES_PLAN.md) (D42, approved 2026-11-03); step E above.

1. Profiles: separate settings, torrents and state, simultaneous instances, per-profile single-instance handling, ports and nox services.
2. Downloadable binaries on GitHub Releases: build on the tagged packaging workflow already started for 1.0 (items 6-8).
3. Portable mode on every platform: settings, state and downloads relative to the portable folder. Research the model first and revisit D7 together with profiles.
4. A multilingual wiki/user guide in the D24 languages. The English guide is done (2026-11-04_211029_1793823029): 28 pages on the [GitHub wiki](https://github.com/Balls0fSteel/PaTorrent/wiki), source in `docs/wiki` (copy it to `PaTorrent.wiki.git` after edits; `docs/wiki/BRIEF.md` has the conventions). Open: translations; F1 could open the wiki (`help::WIKI_URL` is `None`); keep pages current as features change.
5. Compiled-in Rust plugin hooks (moved from V1.1, D35), once a completion action, search source or custom column needs one.

## Future improvements

- BEP 44 DHT items/ed25519, BEP 51 sample-infohash querying and BEP 55 holepunch after 1.0 unless cheap. Detailed remaining engine/test gaps are in the backlog.
- Split `core` only if compile times justify it; consider io_uring disk/network on Linux and IOCP file I/O on Windows.
- Continuous work: meaningful scale regression baselines, parser fuzzing, per-release security review and a wider interop matrix (including rTorrent and µTorrent).
- Theming (owner's wish: included and third-party themes, no extra resource use): researched, options in [BACKLOG](docs/BACKLOG.md#themes-researched-2026-11-02_110639_1793613999); next step H.

## Known TODOs

Use the topic headings in [BACKLOG.md](docs/BACKLOG.md) for unresolved implementation gaps, behavioral differences and specific unverified checks. Update the relevant topic when work lands; summarize its outcome in the changelog. Historical completed milestones belong in the [archive](docs/archive/README.md), not in this plan.
