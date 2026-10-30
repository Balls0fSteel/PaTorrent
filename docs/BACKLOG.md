# Detailed backlog

Last updated: 2026-10-28_075806_1793170686 (Europe/Budapest).

Start with [PLAN.md](../PLAN.md) for priorities and release gates. Load only the
topic needed here. These are open checks, known limitations and conditional
improvements; their presence does not make every item a 1.0 blocker. Completed
work and the original wording remain in the [dated plan](archive/PLAN-2026-10-23.md).
Approved constraints are in [DECISIONS.md](DECISIONS.md); implementation differences
belong in [PORTING.md](PORTING.md); document deliberate setting-default changes.

## Security and TLS

- [SECURITY-REVIEW.md](SECURITY-REVIEW.md): fuzz runs are short (120 s per target
  per push). A peer's hash-work budget resets on reconnect; the reconnect delay
  bounds this. Spoofed uTP SYNs hold peer slots until the handshake timeout, as in
  libtorrent. Resent-SYN handling and `MAX_OUTSTANDING` need libutp interop.
- SSRF mitigation refuses loopback tracker paths outside `/announce`, including
  loopback scrapes. A local tracker on another path needs `ssrf_mitigation` off.
- Certificate revocation is not checked on the SChannel tracker path. Missing
  intermediates can still be fetched synchronously by SChannel; macOS trust
  evaluation may fetch intermediates or revocation data synchronously too. Move
  handshakes off the reactor if measurements show a stall.
- macOS Secure Transport is deprecated and supports TLS 1.2 at most. Replacing
  it with Network.framework requires handing over the socket; an alternative
  TLS library would change the current dependency policy. Server identities use
  the exported but undocumented `SecIdentityCreate` via `dlsym`; a temporary
  keychain is the public alternative if that symbol disappears. The backend
  requires macOS 10.14 APIs; the shipped GTK bundle currently requires macOS 15.
- Web seeds and proxy tunnels need loopback HTTPS tests with a trusted test
  certificate. SChannel certificate-chain delivery needs a real CA-chain check.
- HTTPS Web UI: open in Firefox and Chrome, compare the printed fingerprint,
  accept the certificate, log in and verify live `wss://` updates. Prior checks
  used OpenSSL/curl and loopback tests. Check the LocalService CNG key store with
  a real Windows service. HTTPS is configured on the command line; Preferences
  and the compatibility shim have no HTTPS/certificate fields, and listener
  changes need a restart.
- `schannel::tests::keys_left_by_processes_that_are_gone_are_swept` uses a fixed
  key name and can collide with overlapping tests or a running nox sweep; use a
  per-run name if fixing it. A Windows WebSocket thread can keep the persisted
  CNG key alive after server stop; closing and joining connections would release
  it at exit rather than the next start.
- When porting bundled code, update THIRD-PARTY-NOTICES.md's module list and each
  file's origin/copyright header. Implement the qBittorrent API from published
  API documentation; do not copy its source or Web UI assets. Click Help >
  Third-Party Notices in both GUIs to check the embedded text.

## Owner hand-test findings (2026-10-27_225501_1793138101)

Reported by the owner on a Win32 build; fixed 2026-10-28_075806_1793170686 (see the
changelog) and awaiting the owner's re-test on a build that shows `1.0.2-dev.<commit>`
in its title bar. Still open from that round:

- **Re-test on Win32:** scheduler grid (no flicker while dragging; dark green / light
  green / white / light red cells, black lines; greyed table when the scheduler is off),
  toolbar Add / Add from URL / Remove and the four amber checking icons at 16 px and at
  150 % DPI, alternate row shading in the torrent list and the detail tabs (UI Settings
  box), a Stopped row's empty speed cells, no blank row after an add or while hovering rows
  (found and fixed 2026-10-28_083421_1793172861), and the new keys
  (Ctrl+S / Ctrl+T / Ctrl+Shift+P start / stop / pause, Enter for Properties, Del and
  Shift+Del shown on the context menu). Ctrl+Space was left to the list view, whose own
  key it is (toggle the focused row's selection).
- **Checking progress:** several newly added torrents sat at "Checking 0.0 %" together;
  `active_checking` is 1, so confirm whether the rest should read "Queued for checking"
  (auto-managed) or whether the checks stall. Not reproduced yet.
- **GTK:** no alternate row shading (a CSS `nth-child` rule on the column view's rows would
  do; the UI Settings box is hidden there, `Features::alternate_rows`), and the new
  Start / Stop / Pause / Enter keys exist on Win32 only. The scheduler colours are shared.
- **Alternate colour:** the shade is the window colour at 15/16 (about `0xF0F0F0` on
  white), not sampled from 2.2.1; compare on the owner's machine.
- **Build string:** `cargo build` without `PATORRENT_BUILD` reads git at build-script time
  and reruns when HEAD moves, so a plain cargo build in a dirty tree shows its last
  commit without `.dirty`; `build.bat` sets the exact suffix every time.

## UI icon coverage (audit 2026-10-28_090116_1793174476)

Before the audit the GUIs embedded 35 of the 131 bold symbols (11 toolbar, 24 runtime
states). Now `patorrent_frontend::icons::UiIcon` (67 symbols) also covers the category list,
the detail tabs, the Preferences pages and the menus. File-type icons stay the OS's (Win32
`SHGetFileInfoW`, GTK `content_type_get_icon`).

- **Win32** (1.0.4–1.0.7): tree, tabs and Preferences tree via image lists; menus via
  `command_icon` (`app.rs`) and `icons::menu_icons` on the main menu, every `popup()` menu and
  the Add / RSS dialogs' menus. Greyed items get a grey copy; submenus and Disable / Enable
  Feed carry theirs in the item data (`icons::tag_item`). Without icons on purpose:
  check-marked items (the mark takes the place), Exit and Remove And.
- **GTK** (1.0.6–1.0.7), not yet seen running: sidebar, tabs and the Preferences page list
  (a `ListBox` in place of the `StackSidebar`, which shows no icons). Menus stay text only:
  GTK 4 popover menus show icons only in button rows.
- **Revision 8 artwork** (1.0.7, 2026-10-28_111544_1793182544): website, check-updates,
  notices, import, window, edit, feed-disabled and no-label, drawn here as pixel art (nothing
  copied, so no third-party notice). Check / Uncheck reuse select-all / clear-selection.
  Geometry audit clean; the [independent review](../assets/ui-icon-explorations/review-2026-10-28/revision8/REPORT.md)
  passed all eight, and three of its optional refinements went into 1.0.8.

Still to check: GTK on Linux (sidebar and Preferences row spacing, Alt+mnemonics on the tabs,
the dark palette, an error jumping to a Preferences page selecting its row); Win32 at
125–200 % (the Preferences list is made once at the dialog's DPI), RTL (mirrored menus may
mirror the bitmaps) and the tray menu on screen; the Web UI (`nox`) was not audited.

## Win32 layout and dialogs

- The system-message-font change needs visual checks on Properties, RSS,
  Remote, Auto Shutdown, notices and the remaining Preferences pages, especially
  long translations. Older changelog pixel sizes used Tahoma and are historical.
- Add New Torrent: `Advanced...` needs properties on `AddTorrentParams` before
  the torrent joins the session. The current Win32 dialog already has the
  resizable one-column layout and `Don't show this again`; do not re-add these.
  Check a multi-file torrent, folder deselection, priorities, labels, recent
  Save In folders, the renamed root, resizing and persisted size; confirm skipped
  files stay off disk. A failed add leaves its chosen label in `patorrent.ini`
  (harmless). Adding an existing torrent refuses it rather than offering to merge
  trackers as 2.2.1 did.
- Preferences: click every page, OK/Cancel/Apply, Ctrl+P, toolbar, Browse, Reload
  with a real IP filter and Advanced at 150% DPI. The proxy controls and password
  sealing are implemented; verify DPAPI with a second Windows account (a copied
  sealed value becomes empty there). Unix proxy passwords remain plain text;
  libsecret is open. A new engine setting needs a `settings::TABLE` row.
- Preferences defaults for peer connections/upload slots apply to future adds,
  not existing torrents; setters now exist if live propagation is wanted. Both
  defaults are unlimited, unlike 2.2.1's remembered 50 and 4. `aio_threads` and
  `hashing_threads` are read only at session start. Local-GUI Web UI Preferences
  remains a placeholder; Advanced lacks per-setting help. Win32 and shared
  `pref_pages` still have separate field tables and must be kept in sync.
- Dialog procedures hold `&mut State` across calls that can notify the same
  procedure; a `RefCell` pattern like `app.rs::with_app` would be stricter.
- Create New Torrent: click Add file/directory, Save As, progress, Cancel and
  Start seeding. Source tag, pad-file and DHT-node controls are possible extras,
  not part of the remembered 2.2.1 dialog. The Unix symlink/attribute creation
  tests changed to hybrids and need a Unix run; `resolve_duplicates` can now
  construct its fixture using `CreateTorrent`. Deprecated mutable-torrent cases
  remain unported.
- Torrent Properties: test one/several torrents, mixed boxes, invalid trackers,
  greying and 150% DPI. Compare 2.2.1's General tab, target-zero semantics, both
  targets needing to be reached, default 150%, and unchecked upload-limit action.
  Properties > Advanced and frontend-only `rss.*` Advanced rows still need a
  design based on the reference client in `z_exclude_uTorrent/`.
- Seeding goals lack an end-to-end app-loop check and a real Logger check of
  `seeding goal reached`. A goal's upload cap persists as the torrent's own limit
  after raising the goal; the upload-slot override has no separate stored flag.
  Properties waits up to 2 s for trackers on the GUI thread and leaves a blank
  unedited tracker box alone on timeout. Private torrents do not grey DHT/PEX
  controls (the engine ignores them). Shim `addTrackers` gives each URL a new tier.
- Windows Integration: check actual HKCU registration, Explorer/browser opens,
  autostart and Start minimized after login, every box and Apply; tests touched
  scratch registry keys only. Check Default Apps on Windows 10. Windows'
  hash-protected `UserChoice` can override registration. Removing an association
  removes only entries naming this executable and does not restore the previous
  app. Startup checking defaults off and is skipped for `--minimized`.
- Main window: second-monitor `WM_DPICHANGED` is untried. Compare torrent-list
  columns and Pieces columns with 2.2.1; Pieces' first column cannot be right
  aligned in a Win32 list view. Pause does not toggle to running; Start resumes.
  The accepted empty-session working set is about 20 MB; measure before memory
  work such as lazy shell32 loading or the manifest's `SegmentHeap`.
- One old full Windows test run omitted the win32 result without a captured
  error; isolated and later full runs passed. If it recurs, capture exit code and
  full output to identify a crashing hidden-dialog test.
- µTorrent's UI Settings "Show current speed in the title bar" (`[D: x U: y]`
  before the name and version) is not implemented; the title is
  "PaTorrent <version>" only.

## GTK desktop checks and limitations

CI builds/tests the GTK client, and the owner found the GUI fine on 2026-10-23.
That broad check does not establish that every detailed case below was tried.

- Add New Torrent: test default/narrow sizes, Save In/Label menus, picker/free
  space, folder expansion and keyboard navigation, mixed checkboxes, multi-row
  Space, priority readback, thousands of files and scroll position, renamed root,
  Escape/Cancel, a real magnet and a restart while awaiting metadata, and RTL
  arrows. `add::same_dir` compares without case and can merge `/Data` and `/data`.
  GTK's `Don't show this again` and pre-add Advanced options remain absent.
- Preferences: check long captions, greying, pickers, Random Port, IP filter
  reload/status, Advanced filter/changed rows/Set/Reset/Enter/True/False, invalid
  values and focus/page changes, OK/Cancel/Apply/Escape, the language notice, and
  live rate/move/auto-load/seeding-goal effects. The flat `StackSidebar` indents
  Disk Cache with spaces. Web UI remains a missing page; Scheduler is present.
- Dialogs: test scheduler click/drag/fast drag, hover/legend, disabled state,
  long translations, Arabic layout and a live hour boundary. Neither GUI's grid
  has keyboard control. Test Properties' mixed-value cycle, invalid trackers and
  unfinished-torrent double-click; Create's big-folder progress, cancel/close
  while hashing, save name/folder and Start seeding; labels/limits, New Label,
  Set Download Location and Initial Seeding greying in context menus.
- Auto Shutdown: start with `PATORRENT_POWER_DRY_RUN=1`, checking countdown,
  Cancel, new work cancelling it and Quit; then test real logind suspend. Power
  availability is read once at startup, so policy/battery changes need a restart.
  FreeBSD/OpenBSD/macOS can only Quit through this UI; logind is unavailable.
  Power entries are separate check actions so each can be disabled independently.
- RSS: test a public feed end to end, clipboard URL, status, filter preview,
  Quality, ordering, History/Clear, tree feeds/items, download double-click,
  menus/deleting the displayed feed and `rss.dat` persistence. While feed items
  show, torrent toolbar buttons still act on the hidden torrent selection.
- Torrent search (D41 slice 4, 2026-10-30_122748_1793359668): tried under Xvfb
  on Xubuntu against its Prowlarr (menu, All sources, results, a wrong key's
  401, header sort, double-click to the Add dialog, Test, Edit, a check box).
  Still check on a real desktop: GNOME/KDE themes, the menu button linked to
  the entry, keyboard-only use (Enter in the box, the menu by keyboard, Enter
  on a row, Escape), Stop while a slow source answers, closing the window
  mid-search, the Add dialog turned off (magnet and `.torrent`), a result
  already in the list, a Jackett `all` source, a source behind the proxy,
  RTL, long source names, a remote window (sources are its own, searched from
  this computer) and Add/Remove keeping the picked mode. No source can drop its
  stored key (the Web UI's `clear_key`): remove and add it again.
- Detail tabs: check narrow General layout, tab sizes, map height and scrolling,
  Blocks cells, dark-theme graph, Files priority menu/readback, Logger tailing
  only at the end, restored tab/split positions. The piece map now renders visible
  rows only; check it with many pieces on a real desktop. Details follow the
  first selected torrent, with no focused-row preference or View hide control.
  Bars/maps retain Win32 white/blue/green colors even in a dark theme.
- Desktop integration: check GNOME/KDE, X11/Wayland, every box and Apply,
  `xdg-mime` defaults, file-manager/browser opens, restart/moved executable,
  login autostart/minimized, startup question and a missing XDG applications
  directory. Turning an association off resets all user choices for the type,
  not only ours. Moving an AppImage invalidates its entries until set again.
  `GenericName` is always English; `Comment` uses the current language.
- Tray: check KDE Plasma, Xfce, GNOME AppIndicator and plain GNOME without a
  tray; panel sizes, tooltip rates, left-click, Show/Hide menu, every entry,
  close/minimize boxes, hidden Exit/Ctrl+Q, minimized login, watcher restart and
  completion notifications. Check X11/Wayland; compositors may not report
  minimized state, in which case minimize-to-tray stays a normal minimize.
  Tray menus lack Auto Shutdown and limits in both GUIs.
- Icons (2026-10-26_100209_1793005329, D27): bold revision 3 artwork is ready in
  `assets/ui-icon-explorations/`; integrate its 24 shared symbols and 11 toolbar
  actions. Win32 still draws its toolbar and GTK still uses theme icons; both
  GUIs still embed the earlier Patina state exports. Preserve state precedence
  and the no-incoming-observed meaning. Then check the actual toolkit rendering.
- Icons: check installed themed icons, tray `IconName`/`IconThemePath` across
  hosts, list/Trackers textures, About and DPIs beyond 100%. The chosen family is
  provisionally Verdigris via `app-icon.txt`; non-MSVC Windows builds retain a
  generic icon and Explorer may cache an old icon after a swap.

## Localisation

- Native speakers should review the 34 machine-translated tables; automated
  checks establish keys/placeholders/mnemonics, not wording. Edit
  `crates/frontend/lang/<code>.lng`; `PATORRENT_LANG=<file>.lng` tries a loose table.
  Keep the English table as source/fallback and do not copy 2.2.1 translations.
- GTK RTL and the Web UI's RTL behavior still need desktop/phone checks. Bidi
  Latin-unit values can appear as `MB 91.6`; translated units could read better.
  Number/date formats are shared across languages. Plural rules are intentionally
  absent unless a translation needs them. Engine/OS errors, protocol values,
  CLI/console, API errors, gate diagnostics and Advanced setting names are not
  translated; API errors can reach the page's status bar.
- GUI language changes apply after restart; nox switches immediately and the
  applying page reloads, while other pages keep their texts until reloaded.
  Win32 remote windows share the base ini's language; GTK has its own ini and
  nox uses Web UI state. `PATORRENT_LANG` overrides preferences. System selection
  uses preferred Windows UI languages, Unix locale variables and macOS CFLocale;
  daemons inherit their service account/environment. Registry descriptions use
  the language when written.
- Win32 fitting can still clip text or misalign fields after long labels grow.
  Recheck after the new font/layout: full translated screenshots did not cover
  every dialog/language. Notes on Advanced/Disk Cache wrap mid-word in some
  Russian/French/Greek texts even with fitting disabled; cause is unresolved.

## macOS and packaged Linux builds

- macOS `.app` has started on the runner with Homebrew moved away, bundled
  libraries and strict codesign; a real Mac must check painted window contents,
  toolbar icons, file dialogs and interactions. The runner's local-network prompt
  obscures a black window body and names the runner host; a real display must
  distinguish a runner artifact from a paint bug.
- Check Finder `odoc`, browser `GURL` (including a link launching the app),
  AppKit event dispatch from GDK, `CFBundleGetMainBundle` after launcher exec,
  unregistered-bundle LaunchServices errors and login LaunchAgent behavior.
  Associations are available from the bundle, not a bare executable; disabling
  one points to Finder Get Info rather than resetting defaults.
- Check `NSStatusItem` clicks/menu/tooltip, Retina, light/dark menu bars,
  removal on Quit, Dock reopening and updates pausing while its menu is open.
  The item is visible in runner screenshots; its interactions are unverified.
  The GTK GUI bundle is arm64 only; Intel engine/nox build coverage does not
  establish an Intel GUI. GTK translations and fontconfig are not bundled.
  Network changes are detected by the 30 s reread rather than a PF_ROUTE socket.
- macOS-only flake: `a_tracker_reply_sets_the_external_address` sometimes misses
  a fake tracker reply in full suites, despite many targeted passes. The tracker
  accepted/read/answered at 0.12 s; no engine error/cause was found. Timeout
  diagnostics now request tracker state/session stats; inspect those on recurrence.
  A real affected announce times out after 30 s and retries.
- AppImages: open GUI/file dialog, schemas/loaders/icons/tray, `.torrent`
  association via `$APPIMAGE`, and HTTPS trackers using bundled OpenSSL on a
  desktop, including aarch64. Existing smoke checks of binaries and nox do not
  cover the GUI. The GTK plugin forces X11/XWayland; remove this only after a
  Wayland check. Built on Ubuntu 24.04, they need glibc 2.39; Ubuntu 22.04's GTK
  4.6 is older than the GUI's 4.12 requirement.
- Flatpak: install/run both architectures on real desktops; check tray,
  notifications, associations and Background-portal autostart. Portal Apply is
  asynchronous, stores grants in `background.ini` and has no parent-window
  handle. The manifest's broad home access and logind permission need Flathub
  review; use download-folder access or document portals if required.
- Flathub submission still needs a git source, GTK screenshots/metainfo,
  verification of the `io.github.balls0fsteel` id, license acceptance and exception
  requests. The recorded Flathub policy requires the owner to write exception
  reasons; preserve that requirement from [RELEASING.md](RELEASING.md). Do not
  generate the owner's submission prose. Aarch64 bundles build but are untried
  on arm64 desktops. Plain Linux tarballs need the build baseline's glibc,
  OpenSSL 3 and GTK 4 libraries. FreeBSD packaging is later.

## Remote mode and Web UI

- Both GUIs need remote hand checks against HTTPS nox: connect dialog focus,
  Enter/Escape, spinner/disabled controls and translated wrapping; bad port,
  password and absent listener; first certificate Cancel/accept, saved pin and
  changed certificate. Compare the printed fingerprint. Check title/list/add/
  pause/Properties/Preferences/Apply, typed server paths, Create with seeding
  disabled, completion notice and one disconnect message after stopping nox.
- Check local/remote process separation: a plain file-open targets the local
  instance; `--remote file.torrent` adds after connecting. GTK uses `NON_UNIQUE`;
  remote notifications use `org.freedesktop.Notifications` rather than
  `GNotification`, so Flatpak needs the corresponding permission instead of the
  notification portal. Each tray has its own bus connection/name.
- Remote windows run their own scheduler/RSS/seeding goals/folder automation/
  scrapes beside nox's, so two schedulers can conflict. Decide whether to disable
  or move these automations server-side. Labels belong to the window rather
  than nox categories. No automatic reconnect or stored passwords; DPAPI/libsecret
  could provide storage. Preferences folder pickers still browse the local
  computer; server paths in Add/Set Download Location are typed. GUI-only
  preference pages stay local; language is local and engine settings are remote.
- Web UI on Android Chrome/iOS Safari: real-phone long press, multiselect,
  scrolling, detail/action sheets, drawer, sort and Back closing the top layer.
  Desktop check: sorts, column drag/resize/chooser, context menu, Shift/Ctrl
  selection, Delete/Space, dark mode, preferences validation/apply and magnet
  dialogs. Try native Properties on the real engine and in Firefox.
- Native Properties names individually selected torrents in GET URLs (about
  350 within the 16 KB head limit); Ctrl+A uses unlimited `all=1`. Tracker lists
  are fetched 32 at a time within one 5 s deadline, leaving blanks on timeout.
  Finished-torrent double-click cannot open the server's folder from a browser.
- A magnet's ready Add dialog appears in every page; first OK/Cancel wins and
  others close at the next tick. Other URL adds still bypass the page's Add
  dialog. Long-poll fallback is conditional on an actual deployment needing it.
- **Sonarr/Radarr/Prowlarr (audit 2026-10-29_161749_1793287069, from their source):**
  the ordered gap list is in [INTEGRATIONS.md](INTEGRATIONS.md) part 1. Share limits on
  add and the reported seed limits (Remove Completed) are fixed in 1.1.3. Decided: the strict
  Host check stays; its 400 names the setting (1.1.4). Real Sonarr, Radarr and Prowlarr
  passed against nox on Linux (2026-10-29, `tools/arr-test`), which found and fixed idle
  seeds' frozen seeding time (1.1.5). The hosted Web UI's paths are tested (1.1.6; its
  `removeCategories` now clears the window's label), and the live run against the Windows GUI
  passed (2026-10-29, `tools/arr-test` README). Still to run: a magnet
  grab with real peers, "First" priority / Force Start, and a mobile qBittorrent remote. Other shim
  gaps: search, logs, automatic management and cookies (tags are accepted as no-ops since 1.1.8).
  RSS exists but has the limitations described in the RSS section. The Bearer API key and the
  subnet whitelist exist since 1.1.10 and are off by default. A wrong-key failure is counted
  before the slow hash and undone on success, so a client's first burst of five or more parallel
  requests with a correct key could be banned for a moment, until one of them verifies. Not seen
  with the *arrs, which send their requests one at a time. Neither setting has controls in the
  Win32 dialog; hosted, they are set from the browser's Preferences.
  States use documented `pausedDL`/`pausedUP`; change to 5.x `stoppedDL`/`stoppedUP`
  if a client needs it. `sync/maindata` stores roughly 50 field values per torrent
  per logged-in client; consider change counters at large scale/many clients.
- Restored `torrents/info.isPrivate` stays false until metadata/properties are
  fetched. Saved Web UI preferences override CLI rate limits and port next start;
  revisit if surprising. The native page's 100k per-client model still needs
  measurement independently from the older whole-page gate.

## Automation, RSS and daemon operations

- Scheduler: click the Win32 grid and verify a real hour boundary for Turn off,
  Seeding only and Limited. Clock polling can apply a mode up to 1 s late; Turn
  off shows per-torrent queued/stopped state rather than a scheduled status.
  Outstanding download requests survive upload mode (libtorrent clears them).
  nox runs scheduler only with `--webui`; shim scheduler preferences are unmapped.
- RSS: click Win32 layout at 100/150% DPI, Quality popup, feed tree menus/item
  list, filters/preview/History and a public feed through each GUI. Compare 2.2.1's
  Close-only dialog, tabs/toolbar/menu/item columns, 15 min refresh, smart repack,
  default label, interval/quality lists, episode syntax and per-episode filtering.
  A no-wildcard pattern matches anywhere, empty filters match nothing, and titles
  without episodes require an empty Episode filter. These choices need comparison.
- RSS limitations: page feed items live in its dialog rather than the torrent
  list; failed downloads remain in History and engine-refused adds stay marked
  added. Shim read marks are not persisted, `useRegex` is ignored,
  `mustContain` uses our wildcard syntax and RSS folders return 409. nox runs RSS
  only with `--webui`, with no persistence under `--no-state`. Feed fetches ignore
  the IP filter, lack conditional GET/ETag and gzip, and Atom HTML titles are not
  decoded twice. Check URL add and HTTPS CONNECT fetching with the session proxy.
- Auto Shutdown: test real power calls/privileges on a test machine after dry
  runs; hibernate availability, tray/modal-dialog countdown, standby wake and
  cancellation. Compare 2.2.1 wording/separators/countdown/Quit/arming rules,
  persistence and menu position. No tray Auto Shutdown entry. `Everything
  completes` waits for seeds to stop, so a forced seed or one without a goal
  holds it indefinitely. nox `all` without Web UI waits for all torrents stopped
  and does not itself enforce seeding goals.
- .torrent folders: click Store/Move/Auto-load on Win32/Web UI, restart and
  cross-drive Store/Move. A restored metadata-less magnet gets no stored copy;
  cancelling after metadata leaves the copy, and nox command-line adds get none.
  Renaming breaks lookup by current torrent name; sharing Store and Auto-load
  produces `.loaded` duplicates. Win32 auto-load failures still show error boxes.
  Compare the 2 s poll/clash naming/duplicate handling with 2.2.1. nox watches
  only with `--webui`, with no standalone `run --watch DIR`.
- Windows nox service: run the real elevated SCM sequence from
  [DAEMON.md](DAEMON.md), including configuration/status, start/Web UI/reload/
  stop/uninstall, crash restart, preshutdown, LocalService state/log access and
  surviving user logoff. Start failures lack Event Log reporting; no per-service
  SID/reduced privilege list; progress heartbeat is capped at 2 min. Changing
  options requires reinstalling (`install --replace --now` does it in one go;
  the password is stored hashed since 1.1.1, no longer in the ImagePath).
  Console close/logoff/shutdown waiting is also untested.
- The Win32 GUI's hosted Web UI (1.1.2, D40) is tested through nox's hosted
  integration tests, the dialog's fake hook and a demo screenshot: hand-check a
  real session once (enable with a password, Open Web UI, log in, label and
  pause a torrent from each side, change a rate limit from the browser, HTTPS,
  the port in use, the box turned off, a browser `app/shutdown`). The page's RSS
  Downloader edits the window's own model (shared `Arc<Mutex<Rss>>`, WEBUI.md
  "Hosted by a desktop client"); hand-check it too: add, edit and remove a feed
  and a filter from the browser with the window's RSS dialog open and closed, an
  item's Download from the browser (label, paused, top of queue as the window's
  own RSS adds), a window-side fetch reloading the page's dialog, `rss.dat`
  saved after a browser change. Not in hosted mode: alternative speed limits
  (owner, 2026-10-30: the Win32 GUI gets none; the shim's `speedLimitsMode`
  answers 0), and a magnet
  added from the browser without metadata survives a window restart only in the
  window's Add dialog. The `--webui` daemon-only paths (`webui.json`) are
  unchanged. Fixed in 1.1.8:
  - a created category that no torrent uses yet is kept as the host's `webui.category.<name>` key;
  - an add without a folder takes its category's `savePath` (both modes);
  - a window-side `seed_*` change resends the torrent in `sync/maindata`.

  Since 1.1.9 the Win32 page has a Server domains field (`webui.domain_list`). GTK has no
  hosting page yet; the host side is in
  `crates/win32/src/webui_host.rs` and could move to `frontend`. The
  `patorrent-nox service` improvements of 1.1.1 (`--replace`, `--now`, the
  hashed login) stay for the daemon; its UAC path is still a hand check.
- FreeBSD rc.d is syntax-checked but not installed/run with root. Daemon state
  directories have no shared lock, so different pid files can open one database.
  `--daemon` startup stderr goes to the terminal rather than the log. systemd and
  Unix signal/daemon behavior were exercised previously; do not treat that as
  proof of the unperformed service/rc.d cases.

## Disk and storage

- Block-buffer pooling and preallocation are deferred; sparse files reduce the
  immediate need. The timer wheel is conditional on profiles showing tick cost.
  A full session-counter table remains open. D3 records the approved write-back
  cache design, but the implementation notes report a read cache and no write
  cache; do not imply the latter has been implemented.
- Read cache: test Linux HDD seeding under competing load and evaluate
  `posix_fadvise(RANDOM)` against kernel/Btrfs/ZFS readahead. Whole-piece lines
  won on the Windows test disk; an adaptive bound could combine that with the
  32-block default. Newly written verified pieces are not retained explicitly
  (the OS cache already has them). Smart-ban reads use this cache and writes
  invalidate affected lines. Measure before choosing either enhancement.
- Move storage: hand-check same/cross-drive Windows moves and move-on-complete,
  Logger and errors; Unix copy paths were tested. A single-file Save As variant
  could rename while moving (current UI always selects a folder). Adding already
  complete data triggers move-on-complete; restored seeds do not. Its destination
  is not a recent Save In option. Large cross-volume copies hold one disk thread
  and that torrent's jobs; shutdown waits for an in-flight move.
- Rename: no Win32 Files-menu rename (2.2.1 also lacked it). Before metadata,
  direct rename fails but `renamed_files` is deferred correctly. Engine rename
  replaces an existing destination; shim checks only names used within a torrent.
  Use `/` separators because mapped names are stored verbatim. API rename returns
  before disk completion; errors appear only in nox console.
- Part files: already-existing skipped data stays in its file rather than
  moving into the part file. After restarting a completed torrent and unskipping
  a file, a redundant seed disconnect can impose a 60 s reconnect delay; an
  unskip could reset that timer. Resume tests still missing include torrent-file
  round trips and `preserve_*` magnet cases.
- Magnets: stopping drops partial metadata; `so=` default-don't-download and a
  custom name are lost after a pre-metadata restart. `TorrentSource::Resume`
  without an embedded info dictionary is rejected. Unknown size displays 0 B
  rather than an empty cell. Verify real-magnet Add OK/Cancel and restart while
  waiting: changed folder, unchecked files, removed row/part data on Cancel.
  Waiting uses forced upload mode, so it reads `[F] Downloading` with 0 B/s;
  queue-respecting metadata would wait behind active slots. Duplicate magnets
  are refused. Skip hash check cannot apply after metadata's check, but changing
  Save In rechecks there. `STOP_WHEN_READY` differs from libtorrent by stopping
  after metadata/check rather than when metadata fetching starts; document it.

## Engine networking and protocol follow-ups

- Proxies: real Dante/Squid/Tor/ssh-D/3proxy runs; HTTPS tracker/web-seed TLS
  over tunnels, IPv6 proxy/relay and incoming uTP over relay remain untested.
  `proxy_send_host_in_connect` (not in 2.0.11) now defaults to true (SEC-15);
  check it against the libtorrent version that has it. I2P and `anonymous_mode` remain unported. New nox proxy
  scope flags need real-proxy checks; saved Web UI settings still win next start.
- Network changes: live FreeBSD PF_ROUTE and Windows adapter-change tests need
  privileged setup; OpenBSD offsets lack coverage. macOS/NetBSD/DragonFly poll
  every 30 s. Only one UDP socket per family is used by uTP/DHT when several
  device addresses bind; wildcard sockets are not split per address/BEP45 node.
  When all devices disappear, trackers keep the old port and old-socket uTP
  connections time out. SSL listen entries are explicitly unsupported.
- Port mapping: OpenBSD is not in CI; FreeBSD route layout assumes 64-bit long.
  NetBSD/DragonFly have no route backend, so NAT-PMP is off and UPnP tries all
  multicast interfaces. UPnP devices with hostname URLs are rejected rather
  than resolved. nox does not print mapping-deletion alerts after shutdown;
  router queries verified deletion. Keep both mappers disabled for loopback
  tests/interop to avoid forwarding their ports on a real router.
- DHT: BEP44 get/put/mutable/immutable/signing, direct-DHT tests and related
  storage tests are later. BEP51 answering exists; querying and matching tests
  are not ported. BEP55 holepunch remains later. Consider more bootstrap hosts
  if the single libtorrent default becomes a problem, documenting a deliberate
  default change in the porting notes. Check DHT disable/re-enable on a
  live session.
- LSD: same-host clients are found on many interfaces; duplicate IDs prevent
  multiple connections but retries continue. A shared-ID/address hint could
  reduce them. Hybrids announce one hash rather than both. Test another machine
  on a real Linux/FreeBSD LAN; `lo` may lack multicast flags. Auto-managed seeds
  take announce slots in list order rather than libtorrent `seed_rank`.
- PEX: three-client interop should confirm our messages (interop disables PEX
  today); the Debian swarm did not drop us. Outgoing flags cover seed/encryption/
  uTP; received holepunch flags are discarded. Encryption support hints come
  from PEX only and are not persisted, matching current libtorrent behavior.
- uTP: `close_reason` is parsed/dropped and never sent; engine disconnect text
  has no libtorrent close-reason codes. `peer_connect_alert` and
  `incoming_connection_alert` are also unported. Send/receive delay statistics
  are tracked internally but absent from `PeerInfo`.
- uTP MTU assumes Ethernet/Teredo rather than interface MTU. OpenBSD/NetBSD/
  DragonFly probes fragment instead of using DF. A silent black hole can kill a
  connection; only after three failures is the next connection's ceiling reduced.
  Timer deadlines can be one 500 ms session tick late and do not drive reactor
  wakeups; congestion-window-blocked writes can keep the 100 ms bandwidth tick
  running, so profile this if many idle peers cost CPU. Shutdown does not wait
  for FIN acknowledgments, as in libtorrent.
- Mixed TCP/uTP mode and peer classes are implemented but need real-network
  checks. UDP loopback can lose tails under load (500 ms stalls); use TCP for
  timing-sensitive tests and measure socket-buffer defaults before increasing
  them. The retransmission-timer-on-ACK-progress rule deliberately differs from
  libtorrent and belongs in PORTING.md/interop notes.
- Super-seeding: compare `[S] Seeding` and Advanced-menu contents with 2.2.1.
  Reset Bans/Clear Peer List/Set Destination Name are absent; deprecated
  `strict_super_seeding` is unported. qBittorrent transport/super-seed checks pass;
  Transmission/Deluge remain. Check HAVE_NONE-to-HAVE_ALL acceptance when turning
  super-seeding off.
- IP filters: `PeerBlocked` Logger display is open; nox takes filters per run.
  Add session export/get APIs only if a UI needs them. Privileged-port and invalid
  outgoing-interface block reasons need their unported settings. Gzip lists are
  supported, zip lists are not. Router external-IP votes use the unspecified
  source; a `SessionVoter` property/fuzz test could cover engine-side voting.
- Periodic scrape: compare stopped/queued behavior and interval with 2.2.1.
  Fixed config has no switch/frontend Advanced row. Tracker announce intervals
  are not persisted, so restored private torrents use 30 min until announcing.
  At 200 per 30 s, more than about 12k paused torrents exceed a 30 min cycle;
  startup's first pass is slow and HTTP may issue 200 requests at once. Visual
  Win32 checks remain; nox scrapes only with Web UI, none while scheduled off,
  and scrape replies/failures are omitted from Logger.
- At low connection limits, observe real peer turnover (e.g. 20 slots, 5 min).
  A readable peer still consumes remaining quota until the tick; libtorrent
  queues it when the channel runs short. Keep current even-split behavior unless
  measurements show starvation.
- Web seeds: check displayed URLs/client names in a running GUI; credentials in
  stored URLs are currently shown verbatim. Partial-file availability/per-file
  redirects can leave an unaligned boundary piece without a usable seed, as in
  libtorrent.

## Updates, installer and website

- The update check (D32) works end to end on Win32: Help > Check for Updates
  read the live `update.txt` over HTTPS ("You have the newest version"), and the
  notice was hand-checked with `PATORRENT_UPDATE_URL`. Not checked:
  clicking the notice (opens the browser), the GTK link button and Help items on
  a desktop (CI compiles them), macOS, a proxy, and a check across sleep/resume.
- nox's update check (D43, 2026-10-30): unit- and Web UI-tested with fake
  fetchers and a refused local URL only. Not checked: a live `update.txt`, the
  page's status-bar link in a browser, a proxy. Without `--webui` only
  `--no-update-check` turns it off (a stored `update.check` in `webui.json` is
  read only when the Web UI runs); nox has no manual "check now".
- Serbian's Help menu: MenuAbout and MenuNotices both use the О mnemonic.
- The new strings (update check, Help items) are machine translations like the
  rest; a native speaker should review them with the tables (Localisation), and
  the installer's own texts in `packaging/windows/strings.nsh`.
- Installer (D31): a silent per-user install, upgrade, busy-file abort and
  uninstall were run locally, and the release job's build, NSIS checksum and
  smoke test passed in dry run 40470860315. Hand-check as RELEASING.md "Windows
  installer" lists: the wizard and language dialog (RTL too), all users through
  UAC (declining it, over-the-shoulder elevation, `/ALLUSERS`), the Finish
  page's launch, `--associate` on a real registry, a running `PaTorrentNox`
  service, Windows on Arm, SmartScreen on the unsigned setup.
- Installer follow-ups (`ci.yml`'s `release-windows` builds, installs and
  uninstalls it since 2026-10-30_085840_1793347120 with a copy of
  `release.yml`'s steps: keep the two in step): associations of an all-users
  install are per user (HKLM or first-run handling?); `--unregister` leaves `.torrent`'s `Content Type` and an empty
  `OpenWithProgids`, as Preferences does; Authenticode signing of the exes and
  the setup; a winget manifest.
- Website (D33): live since 2026-10-27_090124_1793088084 (Pages source: GitHub Actions; the
  `github-pages` environment takes `main` and `v*` tags). The first release
  published after this will be the first release-event run. Its screenshots
  are from the Win32 demo; GTK and Web UI screenshots would help.

## V1.1: importers, search and hooks

Implemented 2026-10-27_112013_1793096413 (D35); see the changelog. Open:

- **µTorrent `started`:** mapped as 0 Stopped, 1 Force Started, 2 Started, 3 Paused,
  inferred; the owner's file has only a 2. Import one torrent in each state from the
  installed 2.2.1 to confirm. `targets` (renamed files) is read as `[index, name]` with the
  name relative to the torrent's folder; not yet seen in a real file. Per-torrent seeding
  goals (`wanted_ratio`, `override_seedsettings`) and `settings.dat` (port, limits,
  folders) are not imported.
- **µTorrent part file:** pieces shared with skipped files live in
  `~uTorrentPartFile_*.dat`, which is not read, so they are downloaded again (seen with the
  owner's torrent: its two wanted pieces were rechecked to 0). Reading that format would
  avoid it; low priority.
- **qBittorrent:** 4.x records (libtorrent 1.2) checked on 47 real files; 5.x keys
  (`qBt-stopped`?) and the SQLite store (`torrents.db`) are not handled. Seeding limits
  (`qBt-ratioLimit`, `qBt-seedingTimeLimit`) and the incomplete folder (`qBt-downloadPath`)
  are not carried over: data stays at `save_path`. v2-only torrents come without piece
  layers and fetch them from peers.
- **Search:** name only; the 2.2.1 drop-down of web search engines is not there. Proposed:
  Torznab sources (Prowlarr, Jackett) in that drop-down, [INTEGRATIONS.md](INTEGRATIONS.md)
  part 2 (D41 approved; the shared model is built, the UI is not). Win32 does not debounce (12-16 ms a keystroke at
  100,000 rows); measure on the low-end machine (gate 5).
- **Remote mode:** Import is disabled in both GUIs; importing into a remote nox goes through
  `nox run --import` on that machine.
- **Plugin hooks:** moved to V2.0 (D35) until a completion action, search source or column
  needs one.
- **Other V1.1 candidates** (owner to choose): the installer built in
  `ci.yml`; a winget manifest; the qBittorrent shim gaps Sonarr and Radarr need (now listed
  in INTEGRATIONS.md); remote-mode automation conflicts; RSS conditional GET and gzip; BEP 44 / 51
  / 55.

## Validation and development environment

- The remaining 100k measurements are a low-end 2-core/4 GB/HDD machine on
  Windows/light Linux, GTK using the compact model, Web UI per-client models,
  idle CPU and responsiveness there, and deleting 1,000 torrents at once
  (currently 0.5–1.4 s of engine alerts). Turn measured baselines into CI
  regression gates; do not revive the dropped 12 MB empty-session target.
- Rerun release-mode interop after peer-wire/picker/disk changes and before
  tagging: plain/encrypted, v1/v2/hybrid, TCP/uTP/uTP-only, and super-seeding.
  Linux v1/v2/hybrid plain/RC4 already passed against qBittorrent/Deluge;
  Transmission refuses v2-only and its hybrid stalls on pad files. New transport/
  super-seed switches still need Transmission/Deluge/Linux paths, including
  their settings/file verification and WSL wildcard binding. Debug uTP+RC4 has
  unexplained 0.4–0.8 s stalls (11–80 s vs 0.27 s release); benchmark release.
  Add rTorrent/2.2.1 and three-client PEX cases to the ongoing matrix.
- Benchmarks against other clients (`tools/bench`, docs/BENCHMARKS.md): the 2026-10-27 run is
  on the Xubuntu VM, where host load inflates CPU time; rerun on bare metal when possible, and
  before releases. Not covered yet: 100,000 torrents (the corpus is ~3 GB of metadata, too big
  for the 8 GB VM's libtorrent clients), multi-peer swarms, uTP, seeding CPU, a current rTorrent
  (Ubuntu ships 0.9.8) and the GUIs (Win32 vs µTorrent 2.2.1 and qBittorrent on Windows). The
  VM keeps `~/bench` (harness, PaTorrent 1.0.1, venv with deluge-client); the two tmpfs mounts
  (`/mnt/benchram`, Transmission's downloads) do not survive a reboot.
- Preserve parser review before each release and ongoing fuzzing. UDP tracker,
  resume-data and remote-wire targets already exist; do not list them as missing.
  ARMv8 SHA acceleration remains later with ARM64 CI.
- MSRV is 1.85; avoid let-chains (`&& let`, 1.88+). The local pinned toolchain is
  1.90.0; stable CI/clippy can be newer. Before pushing run stable workspace
  clippy with all targets and warnings denied, stable workspace fmt and fuzz
  fmt; rely on CI's MSRV check when no local 1.85 exists. GTK uses gtk4-rs 0.10;
  0.11 needs Rust 1.92. Intentionally update the Ubuntu 24.04 CI pin.
- Windows can type-check Unix targets but cannot link without target libraries.
  Linux/FreeBSD targets are installed for 1.90; macOS arm64/x64 for 1.90 and
  arm64 for stable. Stable macOS clippy excludes GTK here (no SDK). A stand-in
  pkg-config with `PKG_CONFIG_ALLOW_CROSS=1` permits GTK cross checks only, not
  linking/runtime proof. Build and test GTK with real GTK libraries in CI/Linux.
- Windows fuzzing needs Visual Studio's ASan DLL (`fuzz.bat` finds it via
  vswhere); `-s none` does not link on MSVC. cargo-fuzz 0.13 needs 1.91+, so use
  stable for its install. CI's installed musl cargo-fuzz needs an explicit
  `--target x86_64-unknown-linux-gnu` for these jobs.
- VM addresses/tooling in the archived plan are historical: Xubuntu `.132`
  replaced `.128`, FreeBSD was `.131`, WSL2 later stopped working and the Linux
  build VM disappeared. Confirm availability before SSH/interop; the Xubuntu VM
  is also used for the owner's Android work. FreeBSD lacked sudo/root access.
  A git-archive copy permits testing without git on the target; old screenshots
  used Xvfb, throwaway XDG folders and `~/gtk-shot/gtk-shot.sh`.
- FreeBSD CI runs main/weekly; with a public repo it could run every push.
  macOS CI runs each push/PR, but Intel is build-only for non-GTK targets and
  OpenBSD lacks CI. Do not infer desktop coverage from these jobs.
- The version is bumped with every change set and the peer ID is a separate
  counter (D37, `-PA1010-` with 1.0.1, `-PA1011-` since 1.1.0; bump it only for a wire-behaviour
  change). Make release notes/tagged draft artifacts and request `PA`
  recognition from libtorrent, Transmission, webtorrent's peer-id parser and
  the BEP20 list. Keep stats/intervals/identity honest for private trackers;
  staff need a stable tagged release and may never whitelist a new client.
- The abandoned vcpkg cache (about 2 GB under `%LOCALAPPDATA%/vcpkg`) is an
  optional user cleanup only if no other project needs it.
