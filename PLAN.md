# PaTorrent plan

Last updated: 2026-10-27_124501_1793105101

Read this file for current work. Load only the relevant sections of [detailed follow-ups](docs/BACKLOG.md) or [decisions D1-D35](docs/DECISIONS.md) when needed. Completed phase checklists and old reports are in the [history index](docs/archive/README.md); they are not the active plan.

## Vision

A lean recreation of µTorrent 2.2.1: native look, small single-file binaries, and 100,000 torrents with low idle CPU and a responsive UI on a modest PC. The engine is our Rust port of libtorrent-rasterbar 2.0.11, used as a reference and never linked.

| Binary | Platform | Role |
|---|---|---|
| `patorrent` | Windows 10+ x64 | Native Win32 GUI |
| `patorrent-nox` | All supported platforms | Daemon, native push API, Web UI, qBittorrent-compatible shim |
| `patorrent-gtk` | Linux / BSD / macOS | GTK 4 GUI on the shared frontend model |

Design: [ARCHITECTURE.md](docs/ARCHITECTURE.md), [PORTING.md](docs/PORTING.md), [WEBUI.md](docs/WEBUI.md). Release procedure: [RELEASING.md](docs/RELEASING.md).

## Current status

- **1.0.1 is published, the first stable release** (2026-10-27, tag `v1.0.1` on `afce628b`, [release](https://github.com/Balls0fSteel/PaTorrent/releases/tag/v1.0.1), Latest): Release run 40485598989 built all 14 files (124 MB; every job green) and the draft was published at 10:21 UTC; Pages runs 40487838137 and 40487838045 built `update.txt` with `stable=1.0.1`. The tag `v1.0.0` (on `b40c2e58`) has no release: its build failed on the Flatpak metainfo (D34). rc.1 (`v1.0.0-rc.1`) stays as a pre-release. The full 1.0 scope (D19) is in it; the open Road to 1.0 hand checks continue as 1.0.x validation.
- **V1.1 is implemented on `main`, unreleased** (D35, `4ee733fd`): search box and importers; see V1.1 below.
- **Benchmarks against other clients** (2026-10-27): `tools/bench`, [BENCHMARKS.md](docs/BENCHMARKS.md) and the site's Benchmarks page, from 1.0.1 on the Xubuntu VM. 10,000 torrents after a restart: 60 MB vs qBittorrent 612 MB; one-peer download 3,592 MiB/s at 1.04 CPU s/GiB. Live on the website since `b83d28f0` (Pages run 40509213942).
- Latest Win32 changes (2026-10-23): Add New Torrent uses the resizable one-column layout, separate Save In / Name fields, a remembered size and "Don't show this again"; dialogs use the system message font (Segoe UI 9 pt here). They are tested but not yet in a refreshed hand-test set. See the recent [changelog](CHANGELOG.md).
- Latest hand-test set: `dist/1.0-test-63e3f50/` (gitignored), Windows built locally and Linux / Windows artifacts from Release `39950546995`. It carries the full-row selection, routed-interface UPnP and connected-peer-list fixes, but predates the latest dialog changes. Older sets remain in `dist/1.0-test-ad714b1/` and `dist/1.0-test/`. The first set's Linux binaries/AppImages passed a command-line smoke check; later sets were not desktop-smoked, and Flatpak was not installed.
- Last recorded CI / FreeBSD / Fuzz and Release runs were green. This is recorded evidence, not a live check. macOS builds and starts on the runner; real display, association, login and tray behavior still need a Mac.
- Scale baseline on this development PC (2026-10-23): Win32 lists 100,000 torrents in 1.16 s, GUI-thread idle CPU 0.05-0.10%, working set 232 MB. Name sort 49 ms, Ctrl+A 4.7 ms, add 19 ms, remove one 14 ms. The low-end-machine gate remains open.
- **New in 1.0.1 since rc.1** (2026-10-27): the window title, tray tooltip, Web UI tab and the exes' version resources show the version; an update check that only tells (D32: a minute in the status bar, Help > Check for Updates, Preferences > General > Privacy); a Windows installer beside the zip (D31, `packaging/windows/`); the project website on GitHub Pages with the `update.txt` the check reads (D33, `site/`, [WEBSITE.md](docs/WEBSITE.md)). The app's check reads the website's `update.txt` (verified end to end with rc.1); the installer is built and smoke-tested by the release job.
- Verdigris (`01`) is provisionally integrated in the application and packaging through `assets/patina-icons/app-icon.txt`; the owner's final brand choice remains open. The candidate gallery is in `assets/patina-icons/`.

## Next steps

1. **After 1.0.1:** confirm the website serves `stable=1.0.1` in `update.txt` (the Pages deploy finished; the CDN caches 10 min) and that an rc.1 build offers the update. 1.0.1 is the installer's first release: run its wizard and the all-users UAC path once on a real PC. Register `PA` with the client-ID lists (item 14).
2. Rerun `tools/bench` before each release and on bare metal when one is free ([BACKLOG](docs/BACKLOG.md#validation-and-development-environment)).
3. Hand-check V1.1 (below, implemented) and keep the 1.0.x validation going. Refresh the hand-test set with the latest Win32 dialog changes, then check the remaining dialogs for clipped translations and layout problems in the new font.
4. Complete the manual checks in Road to 1.0 item 9, starting with Add Torrent from URL, Preferences and the changed Add dialog. Use [BACKLOG.md](docs/BACKLOG.md) for the relevant feature's exact gaps.
5. Finish the outstanding platform, scale and interop checks in items 5-8 and 11-13. Record what was actually run and what remains unverified.
6. **Security audit** (2026-10-27_131050_1793106650): [SECURITY.md](SECURITY.md) lists 18 flagged targets (SEC-01 to SEC-18), all Open. Next: a reviewer fills in each Finding, then fix the confirmed items in order. Keep the audit list out of the public repo until items are fixed; push only the policy section.
7. Keep Windows, Linux, macOS and FreeBSD CI green. MSRV is 1.85; avoid let-chains. Before pushing code, use stable clippy and rustfmt, including the separate fuzz workspace. Interop reruns are needed after peer-wire, picker or disk changes.

## Road to 1.0

Original gate numbers are retained for references in source comments and release docs. "Implemented" does not close a pending hand check. D19-D23 define the approved scope. Since D34 the open checks are 1.0.x validation, not release blockers; 1.0.1 shipped with them open (1.0.0's tagged build failed).

| # | Gate | Remaining work |
|---|---|---|
| 1 | BitTorrent v2 and hybrid (BEP 52) | Implemented, including v2 magnets, dual-hash announces and creation. Linux plain/RC4 interop passed with qBittorrent and Deluge. Keep the regression and final interop checks in item 12. |
| 2 | GTK 4 GUI | All five slices implemented, built and tested in CI; the owner approved a Linux run. Complete targeted GTK checks in the backlog, icon validation and packaging checks. |
| 3 | HTTPS Web UI and GUI remote mode | Implemented. Hand-check certificate pinning, reconnect/authentication failures and both GUIs driving nox. |
| 4 | Localisation | 34 tables compiled in, language selection and RTL implemented. Check GTK in RTL, remaining long-text layouts and machine translations with native speakers when available. |
| 5 | 100,000-torrent scale gate (D21) | Measure on low-end Windows and Linux (proposal: 2 cores, 4 GB RAM, HDD); measure GTK, the Web UI's per-client model and idle CPU, and bulk removal of 1,000 torrents (last observed 0.5-1.4 s of engine alerts). Fix measured bottlenecks. |
| 6 | Windows release zip | Done for rc.1: the tagged run 40462743583 built the zip and the draft release; it was inspected and published. Remaining: try the zip on a clean Windows machine (SmartScreen, first start, associations). The NSIS installer (D31) is built and smoke-tested by the release job (first green in dry run 40470860315); a hand test is pending. |
| 7 | Linux packages | x86_64/aarch64 tarballs, GUI/nox AppImages and Flatpaks build. Run desktop AppImage and Flatpak checks, including schemas, icons, tray, HTTPS, associations and the Background portal. Flathub still needs screenshots, submission changes and the owner's exception text; see RELEASING.md. |
| 8 | macOS (D23) | Secure Transport, CI, arm64 GTK bundle/dmg, Apple events, Login Items, associations and NSStatusItem are implemented. A real Mac must validate window content, opening files/magnets, login and tray behavior. Intel engine/nox build; the Intel GTK bundle remains absent. Current bundle requires macOS 15. |
| 9 | Manual GUI checks | Preferences (all pages, OK/Cancel/Apply, proxy and Advanced), Add (files/folders, priorities, labels, recent paths, real magnet OK/Cancel), Create, Properties (multi-select and DPI), RSS with a public feed, scheduler/hour boundary, Auto Shutdown with `PATORRENT_POWER_DRY_RUN=1`, Windows integration on the real registry, moving completed data / Set Download Location across drives, seeding goals and .torrent folders. Check Pieces progress and newly resized/font-changed dialogs. |
| 10 | Icon integration and validation | Bold UI family approved (D27); revision 6 passed independent review. Integrated (2026-10-26_150634_1793023594): both GUIs embed its 24 state icons and 11 toolbar images (GTK: dark palette on dark themes). CI run 40336848086 built and tested it on Ubuntu, macOS, Windows and MSRV (all green). Remaining: run the GTK client, check non-100% DPI, the Trackers tab with live trackers, GTK light/dark, tray/About and macOS Retina/light/dark menu bars. Final app brand remains provisional Verdigris. |
| 11 | Service integration | Run the nox Windows service under the real elevated SCM, including restart-on-failure and shutdown behavior. Install and exercise FreeBSD rc.d with root (only syntax-checked so far). |
| 12 | Final interop rerun | Use release builds: plain, RC4, TCP, uTP, uTP-only and super-seed with qBittorrent, Transmission and Deluge; v2/hybrid where supported. Transmission/Deluge uTP and super-seed switches remain unverified. Known Linux results: Transmission rejects v2-only and stalls on hybrid pad-file expectations; qBittorrent/Deluge v1/v2/hybrid plain/RC4 passed. |
| 13 | Web UI on a real phone | Android/iOS: detail/action sheets, drawer, sort, long-press multi-select, Back behavior, Properties and reconnection. Also verify desktop keyboard, selection, columns, Preferences and dark mode on the real engine. |
| 14 | Release engineering | rc.1 is out with version `1.0.0-rc.1`, peer ID `-PA1000-` and `docs/releases/<version>.md` as the release text (RELEASING.md steps 1-2). 1.0.1 (straight after rc.1, D34; 1.0.0 was tagged but its build failed) is published with version `1.0.1` and peer ID `-PA1010-`. Parser/security review and its fixes are recorded in SECURITY-REVIEW.md; keep parser review at each release. Register `PA` with libtorrent, Transmission, WebTorrent and the BEP 20 list. |

## Decisions and open questions

The approved decisions D1-D35 remain active in [DECISIONS.md](docs/DECISIONS.md). Keep the one-thread mio engine, OS TLS, lazy shared-store persistence, minimal dependencies, static portable Windows binary (an installer beside the zip, D31) and faithful 2.2.1 layout. Scale takes priority over empty-session memory. The PolyForm Noncommercial licence, PaTorrent identity and honest peer ID remain fixed. New dependencies need a written justification and changelog entry.

Q1-Q15 are resolved; their answers and the original Q10 interop setup are in the [plan snapshot](docs/archive/PLAN-2026-10-23.md). Current testing constraints and setup notes are in [BACKLOG.md](docs/BACKLOG.md#validation-and-development-environment).

- UI artwork (D27-D30): the bold family passed its revision 6 review (131/131) and is integrated (gate 10); revisions 3-6 and their reviews are in the [changelog archive](docs/archive/README.md). Optional: hand-drawn 20 px globe and pylon, dedicated 48/64 px drawings, and a web-ui globe matching the network globe.
- Compare Properties > Advanced and `rss.*` settings with the owner's real µTorrent 2.2.1 in `z_exclude_uTorrent/` (gitignored). Its Add dialog was inspected via scripted captures/control dumps; the remaining wording and behavior need comparison. The owner captured all 15 Preferences pages (General through Advanced > Run Program) as `z_exclude_uTorrent/screenshots/*.webp` on 2026-10-26: compare each page of our Preferences against them (Road to 1.0 item 9).
- Decide the Flathub permission/submission approach (item 7); the recorded exception policy requires the owner's own text. Research before designing the V2.0 goals below.


## V1.1

Scope D35 (search box, importers; plugin hooks moved to V2.0). Implemented 2026-10-27_112013_1793096413, unreleased; details in the [changelog](CHANGELOG.md) and open follow-ups in [BACKLOG.md](docs/BACKLOG.md#v11-importers-search-and-hooks).

- Done: the name search (model, Win32 toolbar box, GTK `SearchEntry`, Web UI); File > Import from qBittorrent / µTorrent in both GUIs and `nox run --import`, with labels, statistics and trusted pieces; 15 new strings in all 34 tables.
- To check by hand: the Win32 box at 100-200% DPI, its focus keys, and the import dialogs; the GTK entry and import flow on Linux; importing a stopped, a paused and a force-started µTorrent torrent to confirm the `started` mapping (0 stopped, 1 forced, 2 started, 3 paused is inferred); native review of the new translations (ar/fa/he mnemonics).
- Release as 1.1.0 once those pass; bump the version and peer ID (`-PA1100-`) together.

## V2.0 goals

Owner priorities recorded 2026-10-21 and reaffirmed 2026-10-23; not designed yet.

1. Profiles: separate settings, torrents and state, simultaneous instances, per-profile single-instance handling, ports and nox services.
2. Downloadable binaries on GitHub Releases: build on the tagged packaging workflow already started for 1.0 (items 6-8).
3. Portable mode on every platform: settings, state and downloads relative to the portable folder. Research the model first and revisit D7 together with profiles.
4. A multilingual wiki/user guide in the D24 languages, probably written and translated with Sonnet.
5. Compiled-in Rust plugin hooks (moved from V1.1, D35), once a completion action, search source or custom column needs one.

## Future improvements

- BEP 44 DHT items/ed25519, BEP 51 sample-infohash querying and BEP 55 holepunch after 1.0 unless cheap. Detailed remaining engine/test gaps are in the backlog.
- Split `core` only if compile times justify it; consider io_uring disk/network on Linux and IOCP file I/O on Windows.
- Continuous work: meaningful scale regression baselines, parser fuzzing, per-release security review and a wider interop matrix (including rTorrent and µTorrent).
- qBittorrent web search support. Worth looking into, but qBittorrent always had a rough history with the search engines. They broke, the Python installations did not work, etc. I'm interested in something more robust - if there is anything. Also, users love the Sonarr, Radarr, Prowler integration - while I never used them myself. Either we could adopt support for their search engines and save us the work of maintaining the engines, or with your help we can easily come up with our own Rust or whatever search engine modules.
- Theming support: GTK4 takes up theme looks, so I think this might be Windows only. And, maybe its not even possible. But if we could, without extra resource use, it would be really cool and nice if we could have some nice looking themes included and third party support as well.

## Known TODOs

Use the topic headings in [BACKLOG.md](docs/BACKLOG.md) for unresolved implementation gaps, behavioral differences and specific unverified checks. Update the relevant topic when work lands; summarize its outcome in the changelog. Historical completed milestones belong in the [archive](docs/archive/README.md), not in this plan.
