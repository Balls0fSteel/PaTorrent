# PaTorrent plan

Last updated: 2026-10-27_065010_1793083810

Read this file for current work. Load only the relevant sections of [detailed follow-ups](docs/BACKLOG.md) or [decisions D1-D30](docs/DECISIONS.md) when needed. Completed phase checklists and old reports are in the [history index](docs/archive/README.md); they are not the active plan.

## Vision

A lean recreation of µTorrent 2.2.1: native look, small single-file binaries, and 100,000 torrents with low idle CPU and a responsive UI on a modest PC. The engine is our Rust port of libtorrent-rasterbar 2.0.11, used as a reference and never linked.

| Binary | Platform | Role |
|---|---|---|
| `patorrent` | Windows 10+ x64 | Native Win32 GUI |
| `patorrent-nox` | All supported platforms | Daemon, native push API, Web UI, qBittorrent-compatible shim |
| `patorrent-gtk` | Linux / BSD / macOS | GTK 4 GUI on the shared frontend model |

Design: [ARCHITECTURE.md](docs/ARCHITECTURE.md), [PORTING.md](docs/PORTING.md), [WEBUI.md](docs/WEBUI.md). Release procedure: [RELEASING.md](docs/RELEASING.md).

## Current status

- **1.0.0-rc.1 is published** (2026-10-27, tag `v1.0.0-rc.1` on `6ed8f6c2`, [release](https://github.com/Balls0fSteel/PaTorrent/releases/tag/v1.0.0-rc.1), pre-release): Release run 40462743583 built all 13 files (121 MB) and the draft was checked and published. The release text is `docs/releases/1.0.0-rc.1.md`; the Windows zip was started here, the other files only on the runners. Bug reports from it feed the gates below.
- Release candidate, working toward the full 1.0 scope (D19): Win32, GTK and nox; v2 / hybrid torrents; localisation; HTTPS and remote mode. These features are implemented; the remaining work is validation, release gates and specific follow-ups below. The owner ran the GTK GUI and found it fine on 2026-10-23; wider testers come with the release candidate.
- Latest Win32 changes (2026-10-23): Add New Torrent uses the resizable one-column layout, separate Save In / Name fields, a remembered size and "Don't show this again"; dialogs use the system message font (Segoe UI 9 pt here). They are tested but not yet in a refreshed hand-test set. See the recent [changelog](CHANGELOG.md).
- Latest hand-test set: `dist/1.0-test-63e3f50/` (gitignored), Windows built locally and Linux / Windows artifacts from Release `39950546995`. It carries the full-row selection, routed-interface UPnP and connected-peer-list fixes, but predates the latest dialog changes. Older sets remain in `dist/1.0-test-ad714b1/` and `dist/1.0-test/`. The first set's Linux binaries/AppImages passed a command-line smoke check; later sets were not desktop-smoked, and Flatpak was not installed.
- Last recorded CI / FreeBSD / Fuzz and Release runs were green. This is recorded evidence, not a live check. macOS builds and starts on the runner; real display, association, login and tray behavior still need a Mac.
- Scale baseline on this development PC (2026-10-23): Win32 lists 100,000 torrents in 1.16 s, GUI-thread idle CPU 0.05-0.10%, working set 232 MB. Name sort 49 ms, Ctrl+A 4.7 ms, add 19 ms, remove one 14 ms. The low-end-machine gate remains open.
- Verdigris (`01`) is provisionally integrated in the application and packaging through `assets/patina-icons/app-icon.txt`; the owner's final brand choice remains open. The candidate gallery is in `assets/patina-icons/`.

## Next steps

1. Refresh the hand-test set with the latest Win32 dialog changes, then check the remaining dialogs for clipped translations and layout problems in the new font.
2. Complete the manual checks in Road to 1.0 item 9, starting with Add Torrent from URL, Preferences and the changed Add dialog. Use [BACKLOG.md](docs/BACKLOG.md) for the relevant feature's exact gaps.
3. Finish the outstanding platform, scale and interop checks in items 5-8 and 11-13. Record what was actually run and what remains unverified.
4. Keep Windows, Linux, macOS and FreeBSD CI green. MSRV is 1.85; avoid let-chains. Before pushing code, use stable clippy and rustfmt, including the separate fuzz workspace. Interop reruns are needed after peer-wire, picker or disk changes.

## Road to 1.0

Original gate numbers are retained for references in source comments and release docs. "Implemented" does not close a pending hand check. D19-D23 define the approved scope.

| # | Gate | Remaining work |
|---|---|---|
| 1 | BitTorrent v2 and hybrid (BEP 52) | Implemented, including v2 magnets, dual-hash announces and creation. Linux plain/RC4 interop passed with qBittorrent and Deluge. Keep the regression and final interop checks in item 12. |
| 2 | GTK 4 GUI | All five slices implemented, built and tested in CI; the owner approved a Linux run. Complete targeted GTK checks in the backlog, icon validation and packaging checks. |
| 3 | HTTPS Web UI and GUI remote mode | Implemented. Hand-check certificate pinning, reconnect/authentication failures and both GUIs driving nox. |
| 4 | Localisation | 34 tables compiled in, language selection and RTL implemented. Check GTK in RTL, remaining long-text layouts and machine translations with native speakers when available. |
| 5 | 100,000-torrent scale gate (D21) | Measure on low-end Windows and Linux (proposal: 2 cores, 4 GB RAM, HDD); measure GTK, the Web UI's per-client model and idle CPU, and bulk removal of 1,000 torrents (last observed 0.5-1.4 s of engine alerts). Fix measured bottlenecks. |
| 6 | Windows release zip | Done for rc.1: the tagged run 40462743583 built the zip and the draft release; it was inspected and published. Remaining: try the zip on a clean Windows machine (SmartScreen, first start, associations). |
| 7 | Linux packages | x86_64/aarch64 tarballs, GUI/nox AppImages and Flatpaks build. Run desktop AppImage and Flatpak checks, including schemas, icons, tray, HTTPS, associations and the Background portal. Flathub still needs screenshots, submission changes and the owner's exception text; see RELEASING.md. |
| 8 | macOS (D23) | Secure Transport, CI, arm64 GTK bundle/dmg, Apple events, Login Items, associations and NSStatusItem are implemented. A real Mac must validate window content, opening files/magnets, login and tray behavior. Intel engine/nox build; the Intel GTK bundle remains absent. Current bundle requires macOS 15. |
| 9 | Manual GUI checks | Preferences (all pages, OK/Cancel/Apply, proxy and Advanced), Add (files/folders, priorities, labels, recent paths, real magnet OK/Cancel), Create, Properties (multi-select and DPI), RSS with a public feed, scheduler/hour boundary, Auto Shutdown with `PATORRENT_POWER_DRY_RUN=1`, Windows integration on the real registry, moving completed data / Set Download Location across drives, seeding goals and .torrent folders. Check Pieces progress and newly resized/font-changed dialogs. |
| 10 | Icon integration and validation | Bold UI family approved (D27); revision 6 passed independent review. Integrated (2026-10-26_150634_1793023594): both GUIs embed its 24 state icons and 11 toolbar images (GTK: dark palette on dark themes). CI run 40336848086 built and tested it on Ubuntu, macOS, Windows and MSRV (all green). Remaining: run the GTK client, check non-100% DPI, the Trackers tab with live trackers, GTK light/dark, tray/About and macOS Retina/light/dark menu bars. Final app brand remains provisional Verdigris. |
| 11 | Service integration | Run the nox Windows service under the real elevated SCM, including restart-on-failure and shutdown behavior. Install and exercise FreeBSD rc.d with root (only syntax-checked so far). |
| 12 | Final interop rerun | Use release builds: plain, RC4, TCP, uTP, uTP-only and super-seed with qBittorrent, Transmission and Deluge; v2/hybrid where supported. Transmission/Deluge uTP and super-seed switches remain unverified. Known Linux results: Transmission rejects v2-only and stalls on hybrid pad-file expectations; qBittorrent/Deluge v1/v2/hybrid plain/RC4 passed. |
| 13 | Web UI on a real phone | Android/iOS: detail/action sheets, drawer, sort, long-press multi-select, Back behavior, Properties and reconnection. Also verify desktop keyboard, selection, columns, Preferences and dark mode on the real engine. |
| 14 | Release engineering | rc.1 is out with version `1.0.0-rc.1`, peer ID `-PA1000-` and `docs/releases/<version>.md` as the release text (RELEASING.md steps 1-2). For 1.0: set version `1.0.0` and peer ID `-PA1000-`, write release notes and inspect draft artifacts/checksums. Parser/security review and its fixes are recorded in SECURITY-REVIEW.md; keep parser review at each release. Register `PA` with libtorrent, Transmission, WebTorrent and the BEP 20 list. |

## Decisions and open questions

The approved decisions D1-D30 remain active in [DECISIONS.md](docs/DECISIONS.md). Keep the one-thread mio engine, OS TLS, lazy shared-store persistence, minimal dependencies, static portable Windows binary and faithful 2.2.1 layout. Scale takes priority over empty-session memory. The PolyForm Noncommercial licence, PaTorrent identity and honest peer ID remain fixed. New dependencies need a written justification and changelog entry.

Q1-Q15 are resolved; their answers and the original Q10 interop setup are in the [plan snapshot](docs/archive/PLAN-2026-10-23.md). Current testing constraints and setup notes are in [BACKLOG.md](docs/BACKLOG.md#validation-and-development-environment).

- UI artwork (2026-10-26_100209_1793005329, D27): `assets/ui-icon-explorations/review.html` now presents the full 131-symbol bold revision 3, native fitted SVG/PNG exports at six sizes, light/dark/monochrome proofs and a Verdigris About mark. Clipping/margin and duplicate-mask audits pass; full visual review and GTK logical-size proofs are recorded in `QA.md`. Artwork only: integrate the 24 current runtime symbols (18 torrent + four tracker + two network), then the 11 toolbar actions; production/platform checks and final app-brand selection remain open. Second review (2026-10-26_101510_1793006110, [report](assets/ui-icon-explorations/review-2026-10-26/revision3/REPORT.md)): mostly fixed, but 18 symbols need another pass before integration, including shipped network-ok/no-incoming, all four tracker states and Add URL; network/tracker bases decided (D28); the generator brief is [REGENERATION-BRIEF.md](assets/ui-icon-explorations/review-2026-10-26/revision3/REGENERATION-BRIEF.md). Revision 4 (2026-10-26_105105_1793008265) redrew all 18 plus the refresh badge and pieces; checks pass and the deviations are in the icon README. Independent review (2026-10-26_110747_1793009267, [report](assets/ui-icon-explorations/review-2026-10-26/revision4/REPORT.md)): 120/131 pass. Still failing: network trio ("→?"), compact tracker tower (reads as a figure), refresh badge gap, add-url and both limits. Open owner decisions:
  - Network: a closed port with the arrowhead inside, falling back to a globe?
  - Tracker states: the narrowed full tower with a 5 × 5 badge, or another base?
  - Add URL: chain link or magnet?
  The owner accepted all three (D29). Revision 5 (2026-10-26_120258_1793012578, commit `16b14bf`) redrew those failures. Its review (2026-10-26_124308_1793014988, [report](assets/ui-icon-explorations/review-2026-10-26/revision5/REPORT.md)) passed 123/131. Add URL, the limits, the refresh badge and all toolbar icons are ready.
  - Network trio: reads as an eye or a key, so the D29 globe fallback applies.
  - Tracker states and reannounce: read "A✓". Owner decision open: (a) a pylon in x 1–8 with arcs above it and the Trackers tab redrawn to match (recommended), (b) smaller tracker badges, (c) a non-tower base.
  The owner chose (a) and the globe (D30). Revision 6 (2026-10-26_143727_1793021847) redrew both bases and the Trackers tab; checks pass. Revision 6 review (2026-10-26_145432_1793022872, [report](assets/ui-icon-explorations/review-2026-10-26/revision6/REPORT.md)): all 131 pass, ready for integration. Integrated 2026-10-26_150634_1793023594 (gate 10); the GTK build and the platform checks remain. Optional: hand-drawn 20 px globe and pylon, dedicated 48/64 px drawings, and a web-ui globe matching the network globe. Optional: dedicated 48/64 px drawings for the pixel-art symbols.
- Compare Properties > Advanced and `rss.*` settings with the owner's real µTorrent 2.2.1 in `z_exclude_uTorrent/` (gitignored). Its Add dialog was inspected via scripted captures/control dumps; the remaining wording and behavior need comparison. The owner captured all 15 Preferences pages (General through Advanced > Run Program) as `z_exclude_uTorrent/screenshots/*.webp` on 2026-10-26: compare each page of our Preferences against them (Road to 1.0 item 9).
- Decide the Flathub permission/submission approach (item 7); the recorded exception policy requires the owner's own text. Research before designing the V2.0 goals below.


## V1.1 goals
- Import µTorrent `resume.dat` and qBittorrent `BT_backup`; add local search/filter; consider compiled-in Rust plugin hooks.

## V2.0 goals

Owner priorities recorded 2026-10-21 and reaffirmed 2026-10-23; not designed yet.

1. Profiles: separate settings, torrents and state, simultaneous instances, per-profile single-instance handling, ports and nox services.
2. Downloadable binaries on GitHub Releases: build on the tagged packaging workflow already started for 1.0 (items 6-8).
3. Portable mode on every platform: settings, state and downloads relative to the portable folder. Research the model first and revisit D7 together with profiles.
4. A multilingual wiki/user guide in the D24 languages, probably written and translated with Sonnet.

## Future improvements

- BEP 44 DHT items/ed25519, BEP 51 sample-infohash querying and BEP 55 holepunch after 1.0 unless cheap. Detailed remaining engine/test gaps are in the backlog.
- Split `core` only if compile times justify it; consider io_uring disk/network on Linux and IOCP file I/O on Windows.
- Continuous work: meaningful scale regression baselines, parser fuzzing, per-release security review and a wider interop matrix (including rTorrent and µTorrent).
- qBittorrent web search support. Worth looking into, but qBittorrent always had a rough history with the search engines. They broke, the Python installations did not work, etc. I'm interested in something more robust - if there is anything. Also, users love the Sonarr, Radarr, Prowler integration - while I never used them myself. Either we could adopt support for their search engines and save us the work of maintaining the engines, or with your help we can easily come up with our own Rust or whatever search engine modules.
- Theming support: GTK4 takes up theme looks, so I think this might be Windows only. And, maybe its not even possible. But if we could, without extra resource use, it would be really cool and nice if we could have some nice looking themes included and third party support as well.

## Known TODOs

Use the topic headings in [BACKLOG.md](docs/BACKLOG.md) for unresolved implementation gaps, behavioral differences and specific unverified checks. Update the relevant topic when work lands; summarize its outcome in the changelog. Historical completed milestones belong in the [archive](docs/archive/README.md), not in this plan.
