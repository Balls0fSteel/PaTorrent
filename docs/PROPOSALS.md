# Proposals under discussion

Drafted 2026-11-04_091839_1793780319 from the owner's six points. Nothing here is approved or built. Each section ends with the questions the owner must answer; an approved section gets its own design file (as DISKIO.md and PROFILES.md did) and a decision ID. The numbers D50-D55 are reserved below so the sections can be referred to while they are discussed.

| Proposal | Size | Recommendation |
|---|---|---|
| D50 Files tab filter | small (one or two change sets) | build first |
| D51 Search helper (Jackett on a click) | medium | build, as a download at the user's click, never bundled |
| D52 Native macOS GUI | large (a fourth GUI crate) | not before gate 8 shows how GTK looks on a Mac; V2.x |
| D53 Setup guide (first start) | small-medium | build after D55's help text exists, since it reuses it |
| D54 Network binding and VPNs | medium (engine work) | build as a rule, not an adapter name |
| D55 Help text | medium, mostly writing | build the English table and the three cues; translations with the wiki |

## D50: a quick filter for the Files tab

**Today:** the Files tab lists every file of the selected torrent (Win32: an owner-data list view; GTK: the count-model `GtkColumnView` in `crates/gtk/src/table.rs`; Web UI: `native.rs` `Tab::Files`). The frontend keeps the torrent's files as `detail::files` (`Arc<[FileEntry]>`). The toolbar's search box (D35) already filters the torrent list by name, so the pattern and its strings exist.

**Proposal:** a filter box at the top right of the Files tab, hint "Filter files", in all three UIs.

- Case-insensitive substring match on the file's path as the tab shows it (folders included), so `s01e03` and `subs/` both work. Several words separated by spaces must all match, as the torrent filter does.
- The visible rows are a `Vec<u32>` of file indexes in a shared `frontend::detail::FileFilter`, rebuilt when the text or the file list changes; both GUIs map row to file index through it, so selection, the priority menu, Open, Relocate and the double click act on the right file. Ctrl+A selects the visible rows only; "Prioritize by File Order" keeps acting on all files (it is a torrent-wide order).
- A count beside the box while a filter is on: "12 of 3,400 files". Escape clears the box. Ctrl+F while the Files tab has the focus goes to this box instead of the toolbar's.
- Scale: a linear scan per keystroke over tens of thousands of files, the same cost class as the torrent filter (12-16 ms at 100,000 rows on Win32), so no debounce.
- The text is kept while the selection moves to another torrent (looking for the same episode across torrents is the common case) and cleared when the window closes. Not stored.

**Cost:** frontend about 150 lines with tests, Win32 about 200, GTK about 120, Web UI about 60. No engine change, no settings, no remote protocol change.

**Questions:** Q1 keep the text across torrents, or clear it when the selection changes? Q2 is the box always visible, or only after Ctrl+F in the tab (2.2.1 had no such box, so always visible departs from the faithful layout by one small control)?

## D51: search without installing Prowlarr: the search helper

**The problem:** D41 searches through the user's own Prowlarr or Jackett. Installing either means a .NET server, a web page, API keys and indexer set-up: fine for *arr users, out of reach for everyone else. The owner wants one button: download, then search public sites.

**Licences first.** Jackett is GPL-2.0, Prowlarr GPL-3.0 (read from GitHub 2026-11-04); PaTorrent is PolyForm Noncommercial (D15), which cannot be combined with either into one program. What is allowed, and what it costs:

| Way | Licence position | Verdict |
|---|---|---|
| A. PaTorrent downloads the upstream's own release from GitHub when the user clicks, into the profile folder, and runs it as a separate process over HTTP | PaTorrent distributes nothing of theirs; two separate programs talking over a socket are separate works under the GPL FAQ. The user gets the GPL program from its authors, with its own licence file | **recommended** |
| B. Bundle Jackett in our zip, installer and bundles | "Mere aggregation" is allowed, but we would then distribute a GPL binary: its licence text, the exact corresponding source for every release we ship (a mirror or a written offer), and 50 MB more in every package against D7's one small file; a scraper of public torrent sites inside a Store package (D46) is a review risk | rejected |
| C. Our own search engine: a Cardigann (YAML indexer definition) interpreter in Rust, definitions fetched from Prowlarr's or Jackett's repositories at run time | Our code is ours; the definitions are GPL data loaded by a non-GPL program, which the FSF treats as not derivative, but it is the greyest of the three. Cost: an HTML parser with CSS selectors, the Go-template subset the definitions use, a regex engine (a new dependency or a port), cookies, login flows, rate limits, JSON paths; several thousand lines and a format that moves (schema v1 to v11) | later, only if A's footprint proves unacceptable |

**Why Jackett and not Prowlarr for A:** one console process with a JSON config and command-line port, data folder and `--NoUpdates`; an `all` aggregate Torznab endpoint that our `Source` model already names; public indexers need no configuration; an admin API to tick indexers. Prowlarr is an application server (its own database, scheduler, app sync) meant to run all the time. Sizes from the latest release (v0.24.3009): Windows zip 57 MB, Linux and macOS tarballs 48-51 MB, self-contained (no .NET to install); unpacked well over 100 MB; a .NET process idles at 100-200 MB of RAM. That is the price, and why it is opt-in and started only for a search.

**Proposal (A):**

1. **The button.** Options > Search Sources gets "Get the search helper (Jackett)...". A box says what it is (a separate program by its own authors under the GPL, from github.com/Jackett/Jackett), the download size, where it goes (`<profile>/search-helper/`), that it runs only while searching, and asks for the click. The download reuses the updater's progress dialog and `fetch` (OS TLS); the release's asset `digest` from the GitHub API is checked against the file, as the updater checks `SHA256SUMS`. The zip and tar.gz readers are small (the inflater exists).
2. **Running it.** Started on the first search and stopped after ten idle minutes (or at exit): `127.0.0.1`, a free port, `--NoUpdates --NoRestart --DataFolder <...>`, an API key and an admin password we generate and keep sealed like a source's key. The source "Search helper (all)" is added to the Torznab sources, enabled, with that key: from there D41 does the rest.
3. **Picking sites.** After the install, a list of the helper's public indexers (name, language, type, from its indexer listing), nothing ticked. Ticked ones are configured through the helper's API. Indexers that need FlareSolverr (sites behind Cloudflare) are shown greyed with the reason; the helper never includes FlareSolverr. Private sites and credentials: "Open the helper's own page" (its web UI on localhost), not ours.
4. **Updates.** "Update the search helper" on the same page checks GitHub releases; also offered when the helper fails to start. Never automatic (Jackett releases almost daily).
5. **Where it is off:** the Flatpak (as the update check is: a sandbox that downloads and runs programs would not pass Flathub review), the Store flavour (D46) and FreeBSD (no Jackett build); those users keep "Add a source" for a Prowlarr or Jackett of their own. nox and the Web UI get the button in a second slice (a headless install for the daemon's profile).
6. **Privacy and safety:** the helper binds to loopback only; its admin UI has the generated password; our fetch of the release goes through the proxy when one is set (it is a public address, D41's rule); nothing is sent anywhere but github.com. The helper's own fetches of the indexers follow its own proxy setting, which we set from ours.

**Cost:** frontend (download, archive readers, the helper's process and API, the indexer list) about 1,200 lines with tests against a fake helper; Win32 and GTK pages about 300 each; Web UI later. No engine change. One new website FAQ entry.

**Questions:** Q1 agree to A, with B rejected? Q2 nothing ticked by default (recommended), or a small default set of public indexers (makes it two clicks, but puts named piracy sites in our code and screenshots, which the Store and Flathub will read)? Q3 Jackett, or Prowlarr despite its weight? Q4 should nox get it in the same release?

## D52: a native macOS interface

**Today:** the GTK GUI is the macOS GUI (D23): an arm64 `.app` bundling GTK and its libraries, ad-hoc signed, with Apple events, Login Items, associations and the `NSStatusItem` tray written natively in `crates/gtk/src/macos.rs` and `tray_macos.rs`. It has started on the CI runner; a real Mac has not yet checked how it looks and feels (gate 8).

**What a native GUI would be:** a fourth binary, `patorrent-mac`, in `crates/cocoa`, on the shared frontend model like the other two. The view layer is the work: the Win32 one is 21,000 lines, the GTK one 14,000 (the frontend they share is 22,500). AppKit pieces: `NSWindow` with an `NSToolbar` and `NSSearchField`; `NSSplitView` with the sidebar `NSOutlineView`, the torrent `NSTableView` (a data source is virtual, so 100,000 rows cost nothing) and the detail `NSTabView`; `NSMenu` menus; Preferences generated from `frontend::pref_pages` (the declaration both GUIs already build their pages from, so the 15 pages are not drawn by hand); the dialogs (Add, Create, Properties, RSS, Search, Import, Profiles, closing window); the pieces bar and speed graph in a layer-backed view; `NSOpenPanel`, drag and drop, `UNUserNotificationCenter`, the Dock badge. The tray, Apple events, LaunchAgent and association code move from the GTK crate into a shared `macos` module. Dark mode, Retina and right-to-left come from AppKit. Bindings: `objc2` and `objc2-app-kit` (MIT, generated from the SDK) in the role `windows-sys` plays for Win32, which needs a D4 justification entry; hand-written `objc_msgSend` calls are possible but slower to write and easier to get wrong.

**What it buys:** a universal (arm64 and x86_64) single binary of about 10 MB in place of a 100 MB arm64-only bundle; native controls, text fields, scrolling and menus; no GTK theme and font bundling problems. **What it costs:** 10-15 slices of GUI work, each needing eyes on a Mac, which this project does not have on hand; the GTK bundle stays the only macOS GUI until the native one covers every dialog; notarization (an Apple developer account) is a separate question that the GTK bundle shares.

**Recommendation:** finish gate 8 on the GTK bundle first and decide from what the Mac shows. If GTK on macOS is acceptable, D52 is a V2.x luxury; if it is not, D52 is the macOS plan and the GTK bundle is dropped for macOS. Either way nothing is built before a Mac is available for every slice.

**Questions:** Q1 is there a Mac available for slice-by-slice checks (gate 8 needs one anyway)? Q2 accept `objc2-app-kit` as the binding, or require hand-written bindings? Q3 if built, does it replace the GTK bundle for macOS, or do both ship?

## D53: a first-start setup guide

**Today:** the first start of a named profile picks a random port (`profile::first_run_settings`); the default profile asks nothing. The Windows setup wizard asks about the Start menu, associations and autostart; the portable zip, the Linux packages and macOS ask nothing at all. 2.2.1 had Options > Setup Guide (Ctrl+G), with a speed test and a network test.

**Proposal:** a "Setup Guide" dialog at the first start of a profile (settings file without keys) and under Options afterwards, so it keeps 2.2.1's menu entry. Three or four pages, each a question with a short explanation drawn from D55's help text, Back, Next, Finish, and "Skip" keeping the defaults; nothing is written until Finish. The pages' model (questions, answers, the settings they set) lives in `frontend::setup_guide` and is tested there; the GUIs draw it. No speed test and no network test: both need servers the project does not run.

1. **Where downloads go.** The download folder and whether finished downloads move elsewhere (Preferences > Directories).
2. **Your router.** "Modern (bought after about 2015), or not sure" versus "Older, or the ISP's basic box". The explanation: every connection to a peer is one entry in the router's connection table, DHT and uTP add many short UDP ones, and a small table fills up, after which the router drops connections, slows the whole house or restarts. The older choice sets global connections 200 to 100, per torrent 50 to 40, new connections per second 30 to 8 (`connection_speed`), upload slots unchanged; the modern choice keeps the defaults. The numbers are a starting point for the owner to confirm. It also points at keep ping low (D49) for the gaming case.
3. **Your disks.** "Use smart disk I/O" (D48's `check_gently`, `tune_for_drive`, the busy-disk throttles) with the explanation of what it does and that checks take longer while something else uses the disk.
4. **This computer (not in the Windows setup's install):** start with the system, associate `.torrent` and `magnet:`, where the setup wizard did not already ask (portable, Linux, macOS).

**Cost:** frontend about 300 lines, Win32 and GTK about 400 each (a wizard frame each; the pages are generated from the model the way Preferences pages are). nox has no guide; its config is the file.

**Questions:** Q1 the pages above, fewer, or others (language is already automatic, D24)? Q2 the older-router numbers? Q3 shown at the default profile's first start too, or only under Options there (the Windows setup already asked some of this)?

## D54: binding to a network, and VPNs without a stable adapter

**Today:** `listen_interfaces` binds the listening sockets only (as libtorrent's); outgoing connections, uTP, the DHT socket and tracker requests take whatever route the OS picks. There is no `net.bind_ip` or `net.outgoing_ip` yet (2.2.1 has both under Advanced). So nothing today keeps traffic inside a VPN.

**The premise, corrected:** WireGuard on Windows does create an adapter (Wintun, named after the tunnel: "WireGuard Tunnel", "Mullvad", "NordLynx", "ProtonVPN"); so do OpenVPN (TAP) and macOS (`utun`) and Linux (`wg0`, `tun0`). What is missing is stability: the adapter exists only while the tunnel is up, its name is the app's choice and changes with the app's language or version, and its address is handed out per session. A binding to a name therefore breaks at the next update, and a binding to an address at the next connect. VPNs with no adapter at all are SOCKS5 proxies (Preferences > Connection covers those already) and split-tunnel apps that route by process, which need no binding on our side.

**Proposal:** a rule, evaluated whenever the network changes, instead of a fixed name.

- **Preferences > Connection, "Use only this network":** off; an adapter picked from the list the engine already enumerates (shown by friendly name, stored by the stable id: the GUID on Windows, the name on Unix); an address range such as `10.2.0.0/16` or `fd00::/8`, which survives renamed and re-created adapters; or "Any VPN adapter" (Windows: tunnel and virtual interface types and Wintun, TAP and WireGuard in the description; Linux: `wg*`, `tun*`, `tap*`; macOS: `utun*`), which is the setting most people want and needs no knowledge.
- **The engine** binds every socket that carries torrent traffic to the matched adapter's addresses: listening (as now), outgoing TCP, the uTP and DHT UDP sockets, trackers, web seeds and the local-peer discovery socket. The Web UI and the remote protocol stay unbound (the owner must still reach them). A v4-only tunnel turns IPv6 off for torrent traffic, which is the usual leak. UPnP and NAT-PMP are not sent to a tunnel adapter's gateway, except where the VPN supports NAT-PMP itself (ProtonVPN, PIA), a follow-up worth its own slice because it gives an open port behind the VPN.
- **When no adapter matches** (the tunnel is down): "Pause while the network is missing" (on by default once a rule is set) pauses all torrents, the status bar says "Waiting for the network", the Logger says which rule found nothing, and everything resumes when it comes back. This is the kill switch users ask for; without it the engine would simply fail to bind and sit idle, which is also safe but silent.
- **Re-evaluation** rides on the existing change notifier (`NotifyIpInterfaceChange`, netlink, PF_ROUTE; the 30 s reread on macOS, BACKLOG) and `on_ip_change`.
- **Seeing that it works:** the General tab and the status bar's tooltip show "Your address as trackers see it", which the engine already learns from tracker replies (`external_address`), so the user can check for a leak without an outside service.

**Cost:** engine (the outgoing bind in TCP, uTP, DHT and HTTP, following libtorrent's `outgoing_interfaces`; the matcher; the pause state) about 800 lines with tests on loopback aliases and fake adapter lists; settings and the page in `pref_pages` once for both GUIs and the Web UI; nox keys; remote protocol bump; `PEER_ID_PREFIX` unchanged (no wire change).

**Questions:** Q1 the three rule kinds, or only the adapter and "any VPN adapter"? Q2 pause-all as the default when the network is missing? Q3 the NAT-PMP-through-VPN follow-up in scope?

## D55: help text without a question mark on everything

**Today:** 21 tooltip uses in Win32 and 9 in GTK, mostly on toolbar buttons. µTorrent 2.2.1 had none per control; its Help menu went to the web.

**Proposal: one table, three cues, no marker.**

- **One source of text.** A `help` table in the language tables keyed like the strings (`H::PrefMaxConnections` beside `S::PrefMaxConnections`): a sentence or two in plain words about what the setting does *in this engine* and when to change it, written from the code, not from µTorrent's manual. The same key later becomes the wiki page's anchor (`.../wiki/<lang>/preferences#max-connections`), so wiring the wiki up is a URL template and a "More..." link, with nothing rewritten. English first; the other 33 languages when the wiki is translated (D24's flow), falling back to English until then.
- **Cue 1, hover:** a tooltip on every setting, menu item and column header. Hovering is free and known, so it needs no marker. Win32: one tooltip control per dialog with `TTF_SUBCLASS` tools, multi-line (`TTM_SETMAXTIPWIDTH`), a 0.7 s delay and a 20 s show time; GTK: `set_tooltip_text`; Web UI: `title`, and on a phone the long press. Menus on Win32 also write the item's text into the status bar while it is highlighted (`WM_MENUSELECT`), the native convention of the era.
- **Cue 2, the pane:** Preferences gets a description pane along the bottom of the page (as Visual Studio's property grid has) showing the text of the focused or hovered control. One pane, always there, no clutter; it answers "what is an IP filter" to someone who does not hover precisely, and to touch users. The Setup Guide (D53) uses the same texts.
- **Cue 3, F1:** on a focused control shows its tooltip now and opens the wiki at its anchor once the wiki exists (`WM_HELP` on Win32, a `GtkShortcut` on GTK). One line at the bottom of Preferences, "Hover a setting for help, F1 for the guide", is the only hint the user ever sees.
- **Not proposed:** a `?` per control (the owner's point), `WS_EX_CONTEXTHELP` title-bar help (one icon per dialog; harmless, but the pane covers it), dotted underlines (a web habit, foreign on Win32).

**Size and cost:** about 200 texts of 30-50 words, 8,000 words of English, about 50 KB that deflates to 15 KB in every binary (D26). All 34 languages would be about 1.6 MB of text, 400 KB deflated, which is too much for a lean binary: ship English compiled in, and the other languages as `help-<lang>.lng` fetched from the website on first use into the profile folder (or beside the executable in the zip), the owner's choice. Writing the English is the bulk of the work (two or three change sets); the cues are about 300 lines per GUI; the pane and the F1 route are small.

**Questions:** Q1 the three cues, or tooltips only for now? Q2 English compiled in and the rest fetched or shipped as files? Q3 who reviews the English texts before translation (they state engine behaviour, so they should be read against the code)?
