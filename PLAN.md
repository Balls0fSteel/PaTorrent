# PaTorrent plan

Last updated: 2026-10-27_104216_1793094136

Read this file for current work. Load only the relevant sections of [detailed follow-ups](docs/BACKLOG.md) or [decisions D1-D34](docs/DECISIONS.md) when needed. Completed phase checklists and old reports are in the [history index](docs/archive/README.md); they are not the active plan.

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
- **The next release is 1.0.1, not another candidate** (owner, D34, 2026-10-27; 1.0.0's tagged build failed, see next steps): rc.1 runs fine. The full 1.0 scope (D19: Win32, GTK and nox; v2 / hybrid torrents; localisation; HTTPS and remote mode) is implemented; the open hand checks in Road to 1.0 continue as 1.0.x validation and no longer block the release. The owner ran the GTK GUI and found it fine on 2026-10-23; wider testers come with the release candidate.
- Latest Win32 changes (2026-10-23): Add New Torrent uses the resizable one-column layout, separate Save In / Name fields, a remembered size and "Don't show this again"; dialogs use the system message font (Segoe UI 9 pt here). They are tested but not yet in a refreshed hand-test set. See the recent [changelog](CHANGELOG.md).
- Latest hand-test set: `dist/1.0-test-63e3f50/` (gitignored), Windows built locally and Linux / Windows artifacts from Release `39950546995`. It carries the full-row selection, routed-interface UPnP and connected-peer-list fixes, but predates the latest dialog changes. Older sets remain in `dist/1.0-test-ad714b1/` and `dist/1.0-test/`. The first set's Linux binaries/AppImages passed a command-line smoke check; later sets were not desktop-smoked, and Flatpak was not installed.
- Last recorded CI / FreeBSD / Fuzz and Release runs were green. This is recorded evidence, not a live check. macOS builds and starts on the runner; real display, association, login and tray behavior still need a Mac.
- Scale baseline on this development PC (2026-10-23): Win32 lists 100,000 torrents in 1.16 s, GUI-thread idle CPU 0.05-0.10%, working set 232 MB. Name sort 49 ms, Ctrl+A 4.7 ms, add 19 ms, remove one 14 ms. The low-end-machine gate remains open.
- **Since rc.1, unreleased** (2026-10-27): the window title, tray tooltip, Web UI tab and the exes' version resources show the version; an update check that only tells (D32: a minute in the status bar, Help > Check for Updates, Preferences > General > Privacy); a Windows installer beside the zip (D31, `packaging/windows/`); the project website on GitHub Pages with the `update.txt` the check reads (D33, `site/`, [WEBSITE.md](docs/WEBSITE.md)). Pushed as `9692f69f`; the website is live from the Pages workflow and the app's check reads its `update.txt` (verified end to end). The Release dry run 40470860315 built and smoke-tested the installer on the runner (green) and compiled the GTK client on Linux; CI on `73736841` was still running.
- Verdigris (`01`) is provisionally integrated in the application and packaging through `assets/patina-icons/app-icon.txt`; the owner's final brand choice remains open. The candidate gallery is in `assets/patina-icons/`.

## Next steps

1. **Release 1.0.1** (item 14, RELEASING.md), the first stable release: the tag `v1.0.0` (on `b40c2e58`) failed in Release run 40475482914 on the Flatpak metainfo and has no release; it stays, and the version moved on to `1.0.1` (owner, 2026-10-27_104216_1793094136) with `docs/releases/1.0.1.md`. Wait for CI on the version commit, then the owner decides on the tag `v1.0.1`; inspect the draft and publish; then the Pages run puts `stable=1.0.1` in `update.txt`. It is the installer's first release: run its wizard and the all-users UAC path once on a real PC if possible.
2. After 1.0.1: plan V1.1 (below) and keep the 1.0.x validation going. Refresh the hand-test set with the latest Win32 dialog changes, then check the remaining dialogs for clipped translations and layout problems in the new font.
3. Complete the manual checks in Road to 1.0 item 9, starting with Add Torrent from URL, Preferences and the changed Add dialog. Use [BACKLOG.md](docs/BACKLOG.md) for the relevant feature's exact gaps.
4. Finish the outstanding platform, scale and interop checks in items 5-8 and 11-13. Record what was actually run and what remains unverified.
5. Keep Windows, Linux, macOS and FreeBSD CI green. MSRV is 1.85; avoid let-chains. Before pushing code, use stable clippy and rustfmt, including the separate fuzz workspace. Interop reruns are needed after peer-wire, picker or disk changes.

## Road to 1.0

Original gate numbers are retained for references in source comments and release docs. "Implemented" does not close a pending hand check. D19-D23 define the approved scope. Since D34 the open checks are 1.0.x validation, not release blockers; only item 14 stands between rc.1 and 1.0.1 (1.0.0's tagged build failed).

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
| 14 | Release engineering | rc.1 is out with version `1.0.0-rc.1`, peer ID `-PA1000-` and `docs/releases/<version>.md` as the release text (RELEASING.md steps 1-2). For 1.0.1 (straight after rc.1, D34; 1.0.0 was tagged but its build failed): set version `1.0.1` and peer ID `-PA1010-` (done), write release notes and inspect draft artifacts/checksums. Parser/security review and its fixes are recorded in SECURITY-REVIEW.md; keep parser review at each release. Register `PA` with libtorrent, Transmission, WebTorrent and the BEP 20 list. |

## Decisions and open questions

The approved decisions D1-D34 remain active in [DECISIONS.md](docs/DECISIONS.md). Keep the one-thread mio engine, OS TLS, lazy shared-store persistence, minimal dependencies, static portable Windows binary (an installer beside the zip, D31) and faithful 2.2.1 layout. Scale takes priority over empty-session memory. The PolyForm Noncommercial licence, PaTorrent identity and honest peer ID remain fixed. New dependencies need a written justification and changelog entry.

Q1-Q15 are resolved; their answers and the original Q10 interop setup are in the [plan snapshot](docs/archive/PLAN-2026-10-23.md). Current testing constraints and setup notes are in [BACKLOG.md](docs/BACKLOG.md#validation-and-development-environment).

- UI artwork (D27-D30): the bold family passed its revision 6 review (131/131) and is integrated (gate 10); revisions 3-6 and their reviews are in the [changelog archive](docs/archive/README.md). Optional: hand-drawn 20 px globe and pylon, dedicated 48/64 px drawings, and a web-ui globe matching the network globe.
- Compare Properties > Advanced and `rss.*` settings with the owner's real µTorrent 2.2.1 in `z_exclude_uTorrent/` (gitignored). Its Add dialog was inspected via scripted captures/control dumps; the remaining wording and behavior need comparison. The owner captured all 15 Preferences pages (General through Advanced > Run Program) as `z_exclude_uTorrent/screenshots/*.webp` on 2026-10-26: compare each page of our Preferences against them (Road to 1.0 item 9).
- Decide the Flathub permission/submission approach (item 7); the recorded exception policy requires the owner's own text. Research before designing the V2.0 goals below.


## V1.1 goals

Owner goals, not designed yet. Explored 2026-10-27_091619_1793088979; the details and open questions are in [BACKLOG.md](docs/BACKLOG.md#v11-importers-search-and-hooks).

1. **Import from qBittorrent `BT_backup`.** Its `.fastresume` files are libtorrent resume records, the format `crates/core/src/resume.rs` already reads under libtorrent's key names, so pieces, statistics, priorities and trackers carry over without a recheck; the `qBt-*` keys give save path, name, category (as label), limits and the stopped state. The cheapest of the four.
2. **Import from µTorrent `resume.dat`.** One bencoded dictionary keyed by `.torrent` file name (path, caption, label, `have` bitfield, priorities, totals, dates, trackers). It is the audience's own client. The owner's copy in `z_exclude_uTorrent/` lists no torrents, so a populated test file has to be made first.
3. **Local search/filter.** A name filter in `TorrentList` (`crates/frontend/src/model.rs`) serves Win32, GTK and the Web UI at once; it must stay fast at 100,000 rows. Open: where it lives, since 2.2.1's toolbar box searched the web.
4. **Compiled-in Rust plugin hooks.** No concrete first user yet; define one (for example a completion action or a search provider) before an API, or move this to V2.0.

Both importers share one frontend module, a GUI menu entry and a nox option, need a public engine call to add a torrent from a resume record (`ResumeData` is crate-private), leave the source client's files untouched and refuse while that client runs.

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
