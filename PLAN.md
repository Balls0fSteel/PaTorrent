# PaTorrent plan

Last updated: 2026-10-30_140531_1793365531

Read this file for current work. Load only the relevant sections of [detailed follow-ups](docs/BACKLOG.md) or [decisions D1-D40 and proposed D42](docs/DECISIONS.md) when needed. Completed phase checklists and old reports are in the [history index](docs/archive/README.md); they are not the active plan.

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
- **1.1.17 is published, Latest** (2026-10-30, tag `v1.1.17`, [notes](docs/releases/1.1.17.md)): Torznab search in all three GUIs, RSS in the Windows GUI's Web UI, the API key and subnet whitelist, and nox's update check. The website has a News page (`site/news.html`, `news.atom`).
- **1.1.7 was published** (2026-10-30, tag `v1.1.7`, [notes](docs/releases/1.1.7.md)), replacing the never-published 1.1.2 draft: the Windows GUI's own Web UI (D40), nox's hashed service login and `--replace --now`.
- **libtorrent 2.0.12-2.1.2 fixes** (1.1.18, unreleased, peer ID `-PA1012-`): the relevant upstream fixes are ported, and the reference stays 2.0.11 ([PORTING.md](docs/PORTING.md#upstream-fixes-after-2011)). Open: an interop rerun with v2-only odd-size torrents against libtorrent 2.0.15 / 2.1.x, and the untriaged commits ([BACKLOG](docs/BACKLOG.md#libtorrent-2012-212-follow-ups-triaged-2026-10-30_140531_1793365531))
- **Benchmarks** (2026-10-27, [BENCHMARKS.md](docs/BENCHMARKS.md), `tools/bench`, on the site): 10,000 torrents after a restart use 60 MB against qBittorrent's 612 MB; a one-peer download runs at 3,592 MiB/s and 1.04 CPU s/GiB.
- Latest hand-test set: `dist/1.0-test-63e3f50/` (gitignored). It predates the 2026-10-23 dialog changes (the resizable Add dialog and the system message font), which are tested but not yet hand-tested from a refreshed set. Linux binaries were smoke-checked on the command line only.
- Last recorded CI / FreeBSD / Fuzz and Release runs were green. This is recorded evidence, not a live check. macOS builds and starts on the runner; real display, association, login and tray behavior still need a Mac.
- Scale baseline on this development PC (2026-10-23): Win32 lists 100,000 torrents in 1.16 s, GUI-thread idle CPU 0.05-0.10%, working set 232 MB. Name sort 49 ms, Ctrl+A 4.7 ms, add 19 ms, remove one 14 ms. The low-end-machine gate remains open.
- 1.0.1 added the version in titles and resources, the update check (D32), the installer (D31) and the website with `update.txt` (D33, [WEBSITE.md](docs/WEBSITE.md)); the check was verified end to end with rc.1.
- Verdigris (`01`) is provisionally integrated in the application and packaging through `assets/patina-icons/app-icon.txt`; the owner's final brand choice remains open. The candidate gallery is in `assets/patina-icons/`.

## Next steps

**Feature order chosen by the owner (2026-10-30_085652_1793347012):**

- **A.** Done: 1.1.7 is published (2026-10-30).
- **B.** Small fixes. Done in 1.1.8: the hosted window's `seed_*` changes reach `sync/maindata`, a category's `savePath` is used on add, hosted unused categories are kept, and tags are accepted as no-ops. The installer is built and smoke-tested in `ci.yml`. Win32 Preferences > Web UI has a Server domains field (1.1.9). The Bearer API key and the subnet whitelist are in 1.1.10. nox checks for updates (1.1.11, D43). Phase B is done.
- **C.** RSS in hosted mode is done (1.1.13: the browser edits the window's own RSS model; hand check pending). Optional: a GTK hosting page. The owner decided against alternative speed limits in the Win32 GUI (2026-10-30_103224_1793352744): keep 2.2.1's controls. The hosted Web UI keeps refusing them, by design.
- **D.** Torznab search (D41, approved; a source on a non-public address bypasses the proxy), in the slices of [INTEGRATIONS.md](docs/INTEGRATIONS.md) part 2. Slices 1-2 are done: the frontend model (1.1.12), and nox with the Web UI, run live against the VM's Prowlarr (1.1.14). Slice 4 (GTK, 1.1.15) and slice 3 (Win32, 1.1.16) are done, so all four slices are in. Left: hand checks (BACKLOG) and the optional shim `search/*`. Optional: the shim's `search/*`.
- **E.** A V2.0 profiles and portable-mode design: written ([PROFILES.md](docs/PROFILES.md), proposed D42); awaiting the owner's review of Q1-Q9. No code before approval.

0. **Owner hand-test findings (2026-10-27):** fixed in 1.0.2-1.0.8. The owner re-tests them on a fresh Win32 build; what stays open is in [BACKLOG](docs/BACKLOG.md#owner-hand-test-findings-2026-10-27_225501_1793138101).
1. Run the installer's wizard and the all-users UAC path once on a real PC, and register `PA` with the client-ID lists (item 14). `update.txt` serves `stable=1.1.7` (2026-10-30).
2. Rerun `tools/bench` before each release and on bare metal when one is free ([BACKLOG](docs/BACKLOG.md#validation-and-development-environment)).
3. Hand-check V1.1 (below, implemented) and keep the 1.0.x validation going. Refresh the hand-test set with the latest Win32 dialog changes, then check the remaining dialogs for clipped translations and layout problems in the new font.
4. Complete the manual checks in Road to 1.0 item 9, starting with Add Torrent from URL, Preferences and the changed Add dialog. Use [BACKLOG.md](docs/BACKLOG.md) for the relevant feature's exact gaps.
5. Finish the outstanding platform, scale and interop checks in items 5-8 and 11-13. Record what was actually run and what remains unverified.
6. **Security fixes** (reviewed 2026-10-27_132153_1793107313; all done 2026-10-27_212434_1793132674): every audit finding is Fixed or Ruled out; what stays is accepted and named in its Resolution notes. The owner accepted the named remainders (2026-10-27_214505_1793133905). The audit is kept out of the public repo (owner, 2026-10-27): the tracked [SECURITY.md](SECURITY.md) is the policy only, the full audit is the untracked `SECURITY-AUDIT.md` (in `.git/info/exclude`). Linux tests, a portable `--shell` run and Open Folder's shell path passed; macOS CI is the check for the Secure Transport revocation path. Machine translations needing native review: `RemotePlaintext`, `ForwardRefused`, `PrefCurrentPassword`, `PrefNeedCurrentPassword`. The remote protocol is now 10 (SEC-12, then `apply_filter_to_dht`): a GUI and nox must both be current.
7. **Sonarr/Radarr/Prowlarr** ([INTEGRATIONS.md](docs/INTEGRATIONS.md) part 1): gaps 1-3 are fixed or decided (1.1.3-1.1.4). Real apps passed against nox and against `patorrent.exe`'s hosted Web UI (2026-10-29, `tools/arr-test`); those runs led to the 1.1.5 and 1.1.6 fixes. Still open: a magnet grab with real peers, and "First" priority / Force Start. Torznab search is item D above.
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
| 6 | Windows release zip | Done for rc.1: the tagged run 40462743583 built the zip and the draft release; it was inspected and published. The NSIS installer (D31) is built and smoke-tested by the release job (first green in dry run 40470860315). 1.1.2's zip and setup passed on a clean Windows 11 VM over SSH (2026-10-29, `tools/windows-clean`: zip start, static CRT, upgrade from 1.1.0, uninstall, all users). Remaining, needing a desktop: SmartScreen on a real download, the wizard and UAC pages, "Run PaTorrent" not elevated, drag and drop, double-click associations. |
| 7 | Linux packages | x86_64/aarch64 tarballs, GUI/nox AppImages and Flatpaks build. Run desktop AppImage and Flatpak checks, including schemas, icons, tray, HTTPS, associations and the Background portal. Flathub still needs screenshots, submission changes and the owner's exception text; see RELEASING.md. |
| 8 | macOS (D23) | Secure Transport, CI, arm64 GTK bundle/dmg, Apple events, Login Items, associations and NSStatusItem are implemented. A real Mac must validate window content, opening files/magnets, login and tray behavior. Intel engine/nox build; the Intel GTK bundle remains absent. Current bundle requires macOS 15. |
| 9 | Manual GUI checks | Preferences (all pages, OK/Cancel/Apply, proxy and Advanced), Add (files/folders, priorities, labels, recent paths, real magnet OK/Cancel), Create, Properties (multi-select and DPI), RSS with a public feed, scheduler/hour boundary, Auto Shutdown with `PATORRENT_POWER_DRY_RUN=1`, Windows integration on the real registry, moving completed data / Set Download Location across drives, seeding goals and .torrent folders. Check Pieces progress and newly resized/font-changed dialogs. |
| 10 | Icon integration and validation | Bold UI family approved (D27); revision 6 passed independent review. Integrated (2026-10-26_150634_1793023594): both GUIs embed its 24 state icons and 11 toolbar images (GTK: dark palette on dark themes). CI run 40336848086 built and tested it on Ubuntu, macOS, Windows and MSRV (all green). Remaining: run the GTK client, check non-100% DPI, the Trackers tab with live trackers, GTK light/dark, tray/About and macOS Retina/light/dark menu bars. Final app brand remains provisional Verdigris. |
| 11 | Service integration | Run the nox Windows service under the real elevated SCM, including restart-on-failure and shutdown behavior. Install and exercise FreeBSD rc.d with root (only syntax-checked so far). |
| 12 | Final interop rerun | Use release builds: plain, RC4, TCP, uTP, uTP-only and super-seed with qBittorrent, Transmission and Deluge; v2/hybrid where supported. Transmission/Deluge uTP and super-seed switches remain unverified. Known Linux results: Transmission rejects v2-only and stalls on hybrid pad-file expectations; qBittorrent/Deluge v1/v2/hybrid plain/RC4 passed. |
| 13 | Web UI on a real phone | Android/iOS: detail/action sheets, drawer, sort, long-press multi-select, Back behavior, Properties and reconnection. Also verify desktop keyboard, selection, columns, Preferences and dark mode on the real engine. |
| 14 | Release engineering | rc.1 is out with version `1.0.0-rc.1`, peer ID `-PA1000-` and `docs/releases/<version>.md` as the release text (RELEASING.md steps 1-2). 1.0.1 (straight after rc.1, D34; 1.0.0 was tagged but its build failed) is published with version `1.0.1` and peer ID `-PA1010-`. Parser/security review and its fixes are recorded in SECURITY-REVIEW.md; keep parser review at each release. Register `PA` with libtorrent, Transmission, WebTorrent and the BEP 20 list. |

## Decisions and open questions

The approved decisions D1-D40 remain active in [DECISIONS.md](docs/DECISIONS.md). Keep the one-thread mio engine, OS TLS, lazy shared-store persistence, minimal dependencies, static portable Windows binary (an installer beside the zip, D31) and faithful 2.2.1 layout. Scale takes priority over empty-session memory. The PolyForm Noncommercial licence, PaTorrent identity and honest peer ID remain fixed. New dependencies need a written justification and changelog entry.

Q1-Q15 are resolved; their answers and the original Q10 interop setup are in the [plan snapshot](docs/archive/PLAN-2026-10-23.md). Current testing constraints and setup notes are in [BACKLOG.md](docs/BACKLOG.md#validation-and-development-environment).

- UI artwork (D27-D30): the bold family passed its revision 6 review (131/131) and is integrated (gate 10); revisions 3-6 and their reviews are in the [changelog archive](docs/archive/README.md). Optional: hand-drawn 20 px globe and pylon, dedicated 48/64 px drawings, and a web-ui globe matching the network globe.
- Compare Properties > Advanced and `rss.*` settings with the owner's real µTorrent 2.2.1 in `z_exclude_uTorrent/` (gitignored). Its Add dialog was inspected via scripted captures/control dumps; the remaining wording and behavior need comparison. The owner captured all 15 Preferences pages (General through Advanced > Run Program) as `z_exclude_uTorrent/screenshots/*.webp` on 2026-10-26: compare each page of our Preferences against them (Road to 1.0 item 9).
- Proposed D41: search through the user's Prowlarr/Jackett over Torznab, in the toolbar search box's drop-down, with no Python plugins or scrapers of our own; also the proxy rule for local sources ([INTEGRATIONS.md](docs/INTEGRATIONS.md) part 2).
- Proposed D42: profiles and portable mode ([PROFILES.md](docs/PROFILES.md)).
- Decide the Flathub permission/submission approach (item 7); the recorded exception policy requires the owner's own text.


## V1.1

Released in 1.1.0 (D35: the name search, and importers from qBittorrent / µTorrent with D36). Hand checks are still open: the Win32 box at 100-200% DPI and its keys, the import dialogs in both GUIs, the µTorrent `started` mapping, and native review of the new strings. See [BACKLOG.md](docs/BACKLOG.md#v11-importers-search-and-hooks).

## V2.0 goals

Owner priorities recorded 2026-10-21 and reaffirmed 2026-10-23. Goals 1 and 3 are designed in [PROFILES.md](docs/PROFILES.md) (2026-10-30_090053_1793347253, proposed D42, open questions Q1-Q9); no code before the owner approves.

1. Profiles: separate settings, torrents and state, simultaneous instances, per-profile single-instance handling, ports and nox services.
2. Downloadable binaries on GitHub Releases: build on the tagged packaging workflow already started for 1.0 (items 6-8).
3. Portable mode on every platform: settings, state and downloads relative to the portable folder. Research the model first and revisit D7 together with profiles.
4. A multilingual wiki/user guide in the D24 languages, probably written and translated with Sonnet.
5. Compiled-in Rust plugin hooks (moved from V1.1, D35), once a completion action, search source or custom column needs one.

## Future improvements

- BEP 44 DHT items/ed25519, BEP 51 sample-infohash querying and BEP 55 holepunch after 1.0 unless cheap. Detailed remaining engine/test gaps are in the backlog.
- Split `core` only if compile times justify it; consider io_uring disk/network on Linux and IOCP file I/O on Windows.
- Continuous work: meaningful scale regression baselines, parser fuzzing, per-release security review and a wider interop matrix (including rTorrent and µTorrent).
- Web search without qBittorrent's fragile Python plugins, and Sonarr/Radarr/Prowlarr support: now Next steps item 7 and proposed D41.
- Theming support: GTK4 takes up theme looks, so I think this might be Windows only. And, maybe its not even possible. But if we could, without extra resource use, it would be really cool and nice if we could have some nice looking themes included and third party support as well.

## Known TODOs

Use the topic headings in [BACKLOG.md](docs/BACKLOG.md) for unresolved implementation gaps, behavioral differences and specific unverified checks. Update the relevant topic when work lands; summarize its outcome in the changelog. Historical completed milestones belong in the [archive](docs/archive/README.md), not in this plan.
