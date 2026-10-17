# PaTorrent Web UI design

Decided 2026-10-17 (PLAN.md D18). This is the design for the Web UI that `patorrent-nox` serves
and for the native API behind it. It replaces the "qBittorrent WebAPI subset plus a small page"
plan of Phase 6 as the *primary* design; the qBittorrent subset stays as a compatibility shim.

## The decision

Two APIs, one cache:

| | Path | For whom | Shape |
|---|---|---|---|
| Native | `/pa/...` | our Web UI, later the GUIs' remote mode | designed for the page; push |
| Compat | `/api/v2/...` | Sonarr, Radarr, Prowlarr, mobile qBittorrent remotes | qBittorrent WebAPI 2.11.x, polled |

The native API is designed from what the page needs, not the other way round. The compat shim
translates from the same cache and the same command functions; it never shapes a native feature.
Anything the shim cannot express is simply not in the shim.

What the Web UI is: the µTorrent 2.2.1 main window in a browser, driven by the same
`patorrent-frontend` model the Win32 window uses, with the things 2.2.1's WebUI lacked: push
updates, 10,000 torrents without strain, keyboard-driven, and usable on a phone.

What it is not: a copy of qBittorrent's Web UI or of µTorrent's `/gui/?list=1` token API
(no live clients worth supporting), and not multi-user.

## Goals, in priority order

1. **The desktop client, in a browser.** Toolbar, category tree with counts, the torrent list
   with the 24 columns, the detail tabs (General, Trackers, Peers, Pieces, Files, Speed, Logger),
   the status bar, the context menu, the Add New Torrent dialog, Preferences with the 2.2.1
   pages. Same words, same order, same behaviour, because the same `frontend` code decides them.
2. **Scale like the desktop.** The desktop gate is 10,000 torrents at 0.2–0.4 s startup and
   0.1 % idle CPU. The page gets the same gate (budgets below). The page never holds 10,000 rows.
3. **Footprint.** No framework, no build step, no dependencies: one HTML, one CSS, one JS file,
   plain ES2020, embedded with `include_bytes!`, under 200 KB in total uncompressed. The server
   side stays `std` only (D4).
4. **Works on a phone.** The same page, responsive. The remote app is the URL.
5. **Beyond 2.2.1.** Push instead of polling, keyboard selection and shortcuts, dark mode by
   `prefers-color-scheme` (the web page is not bound by D13's strict desktop look), labels, RSS,
   the scheduler and per-torrent limits in the remote as Phase 5 lands them in the engine.

## Architecture

### The page is a thin terminal of `patorrent-frontend`

`crates/frontend` already holds everything that decides *what* the user sees: `columns` (titles,
widths, cell text, sort order), `detail` (the Trackers, Peers, Files columns), `fmt` (sizes,
speeds, ETA, ratio), `status` (status text and category filters), `model::TorrentList` (the
filtered, sorted list behind a virtual list view), `actions` (Start / Force Start / Pause / Stop
over libtorrent's flags), `add` (the Add dialog's file tree and choices), `options` (what
Preferences edits). The Win32 window only draws.

The Web UI does the same, across the wire: the server runs `TorrentList` per client with that
client's filter, sort and visible window, formats the cells with `columns` and `fmt`, and sends
**cell strings** for the visible rows plus the few raw numbers the page draws from (progress,
state class). The page draws. Consequences:

- Parity with the desktop for free, no JavaScript port of `fmt` or `status` to drift.
- Bandwidth is bounded by the visible window (at most about 100 rows by 24 columns), not by the
  torrent count.
- Sorting and filtering are server-side on the slab, which the desktop already does at 10,000.

The GUIs' remote mode (Phase 6 "either GUI can remote-control a nox instance") is a *thick*
client: it wants `TorrentStatus` records, not cells, so its own `TorrentList` keeps working. That
is a second, binary stream from the same cache, not a variation of the page protocol. Out of
scope here; noted so the native API is not bent for it.

### Transport: one WebSocket down, HTTP POST up

- `GET /pa/ws` upgrades to a WebSocket (RFC 6455). The server sends state; the client sends only
  **subscriptions** (what it is looking at). Nothing on the WebSocket mutates anything.
- **Commands are HTTP `POST`s** with JSON bodies and get a status code and an error body each.
  That keeps them testable with `curl`, lets the compat shim call the same functions, and avoids
  request ids and reply matching on the socket.
- Fallback for proxies that break WebSockets: `GET /pa/sync?rid=N` long-poll with the same
  messages. Second slice, only if a real deployment needs it.

Server side: the HTTP server is std threads, one per connection (`webui/server.rs`). A WebSocket
is such a thread kept open: it blocks on a `Condvar` the alert loop (`WebUi::on_alerts`) notifies
once per status tick, sends what changed for that client, and reads client frames in between
(a short read timeout, or a second thread per socket if that proves simpler). Needed and
dependency-free: the handshake (`Sec-WebSocket-Accept` is SHA-1, which `core::crypto` has, plus
base64), frame parsing with client masking, ping / pong, close; about 300 lines plus tests.

### Messages

JSON (our `webui/json.rs`), one object per frame, with a `t` field naming the type.

Client to server (subscriptions):

```json
{"t":"view","filter":{"category":"downloading","label":null,"search":""},
 "sort":["down_speed",false],"columns":["name","size","done","status","down_speed","up_speed","eta"],
 "window":[0,60]}
{"t":"detail","id":"<info-hash hex>","tab":"peers"}
{"t":"detail","id":null}
{"t":"idle"}
```

`window` is `[first, count]` in the sorted, filtered view. Scrolling changes it, debounced to one
animation frame. The page sends `idle` on `visibilitychange` (hidden) and `view` again when
visible; an idle client costs the server nothing per tick.

Server to client:

```json
{"t":"hello","version":"0.1.0","columns":[{"id":"name","title":"Name","width":260,"align":"left"}, ...],
 "categories":[...],"labels":[...]}
{"t":"rows","total":10000,"first":0,"ids":["<hash>", ...],
 "cells":{"<hash>":{"v":17,"c":["debian-13.7.0-amd64-netinst.iso","792 MB","100.0 %", ...],
                    "done":1000,"state":"seeding"}, ...}}
{"t":"tick","total":10000,"ids":["<hash>", ...],"cells":{"<hash>":{...}},"counts":{"all":10000,"downloading":12,...},
 "status":{"dht":312,"down":"3.2 MB/s","up":"940 kB/s","down_total":"...","up_total":"..."}}
{"t":"detail","id":"<hash>","tab":"peers","rows":[...]}
{"t":"log","lines":[...]}
{"t":"error","msg":"..."}
```

`rows` answers a `view` change with the whole window. `tick` carries, once per status tick
(the engine's status interval, 1 s): the window's id order when it changed (the sort by speed
moves rows; 60 hashes is nothing), the cells of rows whose `version` changed (the per-torrent
`Entry::version` the compat `sync/maindata` already keeps), the category counts and the status
bar. A row in the window that did not change is not sent. Cells are strings in the client's
column order; a column change re-sends the window.

Ids are the info-hash in hex, as in the shim (`State::by_hash`); the slab `TorrentId` is not
stable across restarts. Hybrid torrents (Phase 8) use the v1 hash, else the truncated v2 hash,
as qBittorrent does, so both APIs agree.

### Detail tabs

One subscription at a time (`detail`): the server sends the tab's full content, then updates it
every tick while subscribed:

| Tab | Content | Source |
|---|---|---|
| General | the 2.2.1 Transfer / General fields as label–value pairs, formatted | `frontend::detail`, `fmt` |
| Trackers | rows with `detail`'s tracker columns | `post_trackers` reply cache |
| Peers | rows with `detail`'s peer columns, flags included | a `post_peers` reply (new, `sync/torrentPeers` for the shim too) |
| Pieces | the piece bitfield run-length encoded plus the downloading pieces | `post_pieces` |
| Files | the file tree with priority and progress | `post_files` + `post_progress` |
| Speed | a ring of the last 10 minutes of rates, kept server-side so a reconnect still has history | status ticks |
| Logger | the alert log tail, as the Win32 Logger tab shows it | the alert loop |

### Commands

All `POST`, JSON body, `Content-Type: application/json`, CSRF-checked (Origin and the custom
header), answered `204`, or `4xx` with `{"error":"..."}`.

| Endpoint | Body | Does |
|---|---|---|
| `/pa/torrents/start`, `force-start`, `pause`, `stop` | `{"ids":[...]}` | `frontend::actions` |
| `/pa/torrents/recheck`, `reannounce` | `{"ids":[...]}` | |
| `/pa/torrents/remove` | `{"ids":[...],"data":false}` | Remove / Remove and delete data |
| `/pa/torrents/queue` | `{"ids":[...],"move":"up"\|"down"\|"top"\|"bottom"}` | queue positions |
| `/pa/torrents/limits` | `{"ids":[...],"down":0,"up":0}` | bytes per second, 0 unlimited |
| `/pa/torrents/label` | `{"ids":[...],"label":"..."}` | |
| `/pa/torrents/location` | `{"ids":[...],"path":"..."}` | `move_storage` (Set Download Location) |
| `/pa/torrents/super-seed` | `{"ids":[...],"on":true}` | Initial Seeding |
| `/pa/torrents/files` | `{"id":"...","priorities":{"<index>":0..7}}` | file priorities |
| `/pa/torrents/rename` | `{"id":"...","old":"...","new":"..."}` | files and folders |
| `/pa/add` | `multipart/form-data`: `.torrent` files, or JSON `{"url":"magnet:...","save":"...","label":"...","start":true,"skip_check":false,"priorities":{...}}` | the Add dialog's choices (`frontend::add::AddChoice`) |
| `/pa/torrent-info` | a `.torrent` upload | parses it and returns the Add dialog's file tree without adding (two-step add) |
| `/pa/session/limits` | `{"down":0,"up":0,"alt":false}` | global and alternative limits |
| `/pa/prefs` (`GET`, `POST`) | the `frontend::options` model as JSON, grouped by 2.2.1 page | Preferences |
| `/pa/session/shutdown` | | |

The compat shim's `torrents/*` handlers call the same internal functions, so one implementation
serves both.

### Page layout

Desktop (the 2.2.1 window): toolbar; category tree on the left with counts; the list; the detail
tabs below; the status bar. Splitters by drag, layout persisted in `localStorage`. Light theme by
default, dark by `prefers-color-scheme`.

The list is virtualized: a scroll container sized to `total * rowHeight`, a pool of row
elements for the visible window plus overscan, column headers with click-to-sort, drag to
reorder and resize, a column chooser from the header's context menu. Selection with click,
Shift and Ctrl, arrows, Home / End, Ctrl+A; Delete removes, Space pauses, the context menu on
right-click and the Menu key lists what the Win32 context menu lists, in its order.

Phone (under 768 px): the category tree becomes a drawer behind a button, the detail pane a
bottom sheet, the list collapses to Name over a progress bar with speeds and ETA, the context
menu becomes a long-press sheet, the toolbar keeps Add, Start, Pause, Remove.

Drag-and-drop of `.torrent` files and magnet text onto the window opens the Add dialog. The Add
dialog is the 2.2.1 one: Save In, Name, the file tree with check boxes and priorities, Skip hash
check, Start torrent, Label. A magnet shows the dialog once metadata arrives (the same open
question as the Win32 dialog, PLAN.md Phase 5).

### Security

As today: SID cookie, PBKDF2 password hash, failed-login ban, Host validation, bypass for local
clients, CSRF checks on every `POST` and on the WebSocket handshake (`Origin` must match the
request's own origin or the domain list). Added: `Content-Security-Policy: default-src 'self'`
(so no inline script or style in the HTML), `X-Content-Type-Options: nosniff`,
`Referrer-Policy: no-referrer`, `ETag` on the three static files. TLS is the open Phase 6 item.

## Budgets (gates, like the desktop's)

10,000 torrents loaded, measured with the `lt-swarm`-style synthetic profile against the real
engine, Firefox and Chromium:

| What | Target |
|---|---|
| Login to first painted list | under 0.5 s |
| First `rows` message, 60-row window, all 24 columns | under 30 KB |
| `tick` with an idle swarm | under 1 KB |
| `tick` with 100 active torrents, 60 in view | under 20 KB |
| Page idle CPU, window visible, idle swarm | not measurable above the browser's own idle |
| Hidden tab | no messages |
| Server CPU per client per tick | proportional to the window, after one sort per client per tick when the sort key changed; the sort of 10,000 by name or speed is under 2 ms in `frontend::model` |
| `patorrent-nox` size growth for the whole Web UI | under 300 KB |

## Slices

In order; each is a CHANGELOG entry and lands green on the three CI platforms.

1. **WebSocket** in `webui/http.rs` and `server.rs`: handshake, frames, masking, ping / pong,
   close, `Origin` check, the per-client wake-up from the alert loop. Tests with a raw socket
   client in `crates/nox/tests/webui.rs`.
2. **Native session protocol**: `view` subscriptions on a per-client `TorrentList`, `hello`,
   `rows`, `tick` with counts and status bar; cells from `frontend::columns`. Tests for the diff
   rules (unchanged rows not sent, order sent only when it moved, idle sends nothing).
3. **The page**: layout, virtual list, category tree, status bar, context menu, keyboard,
   selection. Replaces `page.html` / `app.js`. The 10,000-torrent gate measured and recorded.
4. **Detail tabs**, with the `post_peers` reply added to the cache (and `sync/torrentPeers` to
   the shim while there).
5. **Commands and the Add dialog**: everything in the Win32 context menu, upload, magnet, the
   two-step add with the file tree, Remove and delete data. The shim's handlers moved onto the
   shared functions.
6. **Preferences** generated from `frontend::options`, the 2.2.1 pages.
7. **Phone layout** pass, then the long-poll fallback if a deployment needs it.
8. Later, with Phase 5: labels in the tree and the Label column, RSS, the scheduler, per-torrent
   limits; and the GUIs' remote mode on its own binary stream.

## Open points

- Whether the WebSocket thread reads and writes on one thread with a short read timeout or uses
  a second thread per socket. Decide in slice 1 by what the tests make simplest.
- A phone's long-press context menu versus a visible per-row menu button. Decide in slice 7
  with the page in hand.
- Compression: none for now (no dependencies, and the window-bounded messages are small). If a
  real remote link wants it, `permessage-deflate` needs an inflater and deflater of our own.
