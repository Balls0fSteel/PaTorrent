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
   plain ES2020, embedded with `include_bytes!`, under 220 KB in total uncompressed (counted
   with LF line ends; 200 KB until the torrent search's panel took it to about 208 KB,
   2026-10-30: the owner to confirm the new budget). The server
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
is a second, binary stream from the same cache, not a variation of the page protocol: see
[Remote mode](#remote-mode) below.

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
 "labels":[["linux",3],["tv",0]],"no_label":9997,"status":{"dht":312,"down":"3.2 MB/s","up":"940 kB/s","down_total":"...","up_total":"..."}}
{"t":"detail","id":"<hash>","tab":"peers","rows":[...]}
{"t":"log","lines":[...]}
{"t":"error","msg":"..."}
```

`rows` answers a `view` change with the whole window. `tick` carries, once per status tick
(the engine's status interval, 1 s): the window's id order when it changed (the sort by speed
moves rows; 60 hashes is nothing), the cells of rows whose `version` changed (the per-torrent
`Entry::version` the compat `sync/maindata` already keeps), the category counts and the status
bar, plus `labels` (every label the daemon knows, from its category map and the torrents, with
its torrent count, sorted by name) and `no_label` whenever either changed since this client's
last tick; a fresh socket gets them with its first tick. A row in the window that did not change is not sent. Cells are strings in the client's
column order; a column change re-sends the window.

A `tick` also carries `"rss": n` when the RSS Downloader changed, `"search": n` when the
torrent search's results, status lines or sources changed (the page asks `GET /pa/rss` or
`GET /pa/search` again) and `"adds": n` when a magnet's Add dialog can open; each is sent
when it moved since the client's last tick.

When nox's update check (D43, [DAEMON.md](DAEMON.md#the-update-check)) found a newer PaTorrent,
`hello` and every `tick` carry `"update":{"version":"1.2.0","url":"<its release page>"}`; the
page shows it as a link in the status bar (the `UpdateNotice` text) and hides the link when a
`tick` comes without it (the newest version again, or the check turned off in Preferences >
General > Privacy). Hosted (D40), the desktop client checks for itself and `update` is never
sent.

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
| Pieces | the piece bitfield run-length encoded plus the downloading pieces with their blocks | `post_pieces` |
| Files | the file tree with priority and progress | `post_files` + `post_progress` |
| Speed | a ring of the last 10 minutes of rates, kept server-side so a reconnect still has history | status ticks |
| Logger | the alert log tail, as the Win32 Logger tab shows it | the alert loop |

As built in slice 4: every message is `{"t":"detail","id":"<hash>","tab":"<tab>", ...}`; the
WebSocket thread posts the tab's `post_*` requests after the subscription and on every tick,
the alert loop fills the cache, and the next tick sends the tab when a hash of its content
changed (nothing when it did not, nothing before the first reply lands). `hello` carries the
four tables' column definitions (`detail.trackers` / `peers` / `pieces` / `files`: title, width,
right).
General: `bars` (Downloaded, Availability: label, text, fill in thousandths) and `groups`
(Transfer, General: `[label, value]` pairs). Trackers / Peers / Files: `rows` of
`{"c":[cells],"k":[sort keys]}` (a number per numeric column, null to sort by text), Files
rows also with `i` (the engine's file index), `p` (progress in thousandths) and `prio`.
Pieces: `n`, `have`, `rle` (alternating run lengths, a run of missing pieces first),
`downloading` (the partial pieces' indices, for the bar) and `rows` for the list under the bar,
one per partial piece in the table format above plus `b`, its blocks as one letter each (`f`
on disk, `w` being written, `r` requested, `-` free), drawn in the Blocks column. Speed: `samples` of `[down, up]` (the whole ring with `all` and `keep` on
subscription, the new ones after), `scale` (`top` and four axis labels, when it changed) and
`legend`. Logger: `lines` of `[unix time, text]` (`all` the first time). A torrent removed
while subscribed gets one `{"gone":true}`; `{"t":"detail","id":null}` unsubscribes.

### Commands

All `POST`, JSON body, `Content-Type: application/json`, CSRF-checked (Origin and the custom
header), answered `204`, or `4xx` with `{"error":"..."}`.

As built in slice 5 (`webui/commands.rs`): every `POST /pa/...` must carry
`X-Requested-With: PaTorrent` (403 `{"error":"missing X-Requested-With: PaTorrent header"}`
without it; a browser cannot send a custom header cross-site without a CORS preflight, which
the server never grants) and pass the shim's `Origin` / `Referer` rule (403
`{"error":"origin not allowed"}`); a request without an `Origin` (curl) passes on the header
alone. A body naming torrents carries `"ids": [hashes]` or instead `"all": true` with the
client's view filter (`"filter": {"category": ..., "label": ..., "search": ...}`, the same
shape as the `view` subscription), applied server-side, so Ctrl+A under a filter acts on
every matching torrent, not the loaded window. Unknown ids are skipped; a body whose ids name
nothing is 404. The shim's `torrents/*` handlers call the same functions.

| Endpoint | Body | Does |
|---|---|---|
| `/pa/torrents/start`, `force-start`, `pause`, `stop` | `{"ids":[...]}` | `frontend::actions` |
| `/pa/torrents/recheck`, `reannounce` | `{"ids":[...]}` | |
| `/pa/torrents/remove` | `{"ids":[...],"data":false}` | Remove / Remove and delete data |
| `/pa/torrents/queue` | `{"ids":[...],"move":"up"\|"down"\|"top"\|"bottom"}` | queue positions (several torrents keep their relative order) |
| `/pa/torrents/limits` | `{"ids":[...],"down":0,"up":0}` (either optional) | bytes per second, 0 unlimited |
| `/pa/torrents/label` | `{"ids":[...],"label":"..."}` | a new label is created; empty removes it |
| `/pa/torrents/location` | `{"ids":[...],"path":"..."}` | `move_storage` (Set Download Location) |
| `/pa/torrents/super-seed` | `{"ids":[...],"on":true}` | Initial Seeding |
| `/pa/torrents/files` | `{"id":"...","priorities":{"<index>":0..7}}` | file priorities |
| `/pa/torrents/properties` (`GET ?ids=<hash>,<hash>`, or `GET ?all=1&category=..&label=..&search=..` for Ctrl+A with the view's filter; `POST`) | `GET`: `count`, `can_initial_seed`, `finished` (every torrent is: the page's double-click opens the dialog only when not), `name` (one torrent only, for the title), `fields` (`trackers` one URL per line with a blank line between tiers, `upload_kib`, `download_kib`, `upload_slots`, `goal_ratio`, `goal_minutes` as text, blank where the torrents differ; `override_slots`, `override_goal`, `initial_seeding`, `dht`, `pex` as `"on"` / `"off"` / `"mixed"`); `POST`: `{"ids":[...],"fields":{...}}` (or `"all":true` with the `filter`) with any of those fields, 400 `{"error":"<field>: <why>"}` | Torrent Properties (`frontend::properties`; the page's dialog from the context menu and a double-click): fields left out or unchanged stay as they are on every torrent; tracker lists are asked of the engine 32 at a time within one 5 s deadline; the seeding goal override is stored as `torrent.<hash>.seed_ratio` / `.seed_time` in `options` |
| `/pa/torrents/rename` | `{"id":"...","old":"...","new":"...","folder":bool}` | files and folders (`folder` optional: a file when `old` names one, else a folder) |
| `/pa/add` | `multipart/form-data`: `.torrent` files as `torrent` parts plus an `options` part holding the JSON below, or JSON `{"url":"magnet:...","save":"...","name":"...","label":"...","start":true,"skip_check":false,"top":false,"priorities":{"<index>":0..7}}` (`url` also takes a bare info-hash or an `http://` link to a torrent); with `"dialog":true` a magnet joins the session in upload mode to wait for its metadata (200 `{"hash"}`, 409 when it is in the list already); then JSON `{"hash":"...", ...the choices}` finishes it, or `{"hash":"...","cancel":true}` removes it with its part file | the Add dialog's choices (`frontend::add::AddChoice`, for a waiting magnet `apply_to_torrent`); `GET /pa/add` gives the dialog its defaults: `save_in` (the default folder, then the recent ones), `labels`, `start`, `show_dialog`, and `ready`, the waiting magnets whose metadata is known, as `{"hash", "name"}` |
| `/pa/torrent-info` | a `.torrent` upload (a `torrent` part); or `GET ?hash=` for a waiting magnet (404 until its metadata is known) | parses it and returns the Add dialog's Torrent Contents without adding (two-step add): `name`, `comment`, `created_by`, `size` / `size_text`, `date` / `date_text`, `private`, `hash`, `files` of `{"i": engine index, "path": below the torrent's folder, "size", "text"}` (pad files left out) |
| `/pa/session/limits` | `{"down":0,"up":0,"alt":false,"alt_down":0,"alt_up":0}`, each optional: `alt` switches the alternative limits, `down` / `up` set the mode then in force; `GET` reads them back | global and alternative limits |
| `/pa/prefs` (`GET`, `POST`) | `GET`: `pages` (the 2.2.1 tree, each page its groups of items: `{"f": id}` with optional `kb`, `min`, `max`, `enabled_by` (one check box id, or several space-separated that must all be on), `secret`; `{"row": [...]}`; `{"stub": label}`; `{"note": text}`; `{"button": label, "action": name}`), `fields` (id to `label`, `type` of `bool` / `int` with `min` and `max` / `text` / `folder` / `enum` with `options` / `password`, `value`, `default`, `readonly`) and `advanced` (every engine setting's id); `POST`: a partial object of ids to values, 204 or 400 `{"error": "<field>: <why>"}` naming the first bad field, nothing applied then | Preferences (slice 6, `webui/prefs.rs`): every engine setting by libtorrent name, the `frontend::options::GuiOptions` nox has an equivalent for under their preferences keys, the Win32 pages' composites (`listen_port`, `proxy_kind` / `proxy_auth`, `enc_out` / `enc_legacy`, `utp`), the alternative limits and the Web UI's own (`webui.*`; `webui.password` write-only); applied live, stored as the frontend's keys in the state file's `options` and re-applied at start; the shim's `setPreferences` maps onto the same ids |
| `/pa/session/shutdown` | | |

The compat shim's `torrents/*` handlers call the same internal functions, so one implementation
serves both. `torrents/setLocation` is Set Download Location (it creates the directory first:
400 for an empty location, 403 / 409 when the directory cannot be created) and `topPrio`,
`bottomPrio`, `increasePrio`, `decreasePrio` are `/pa/torrents/queue`'s top, bottom, up and down. `editTracker` (409 when `origUrl` is missing or `newUrl` already listed, 400 for a URL that
is not `http`, `https` or `udp`), `addTrackers` (each URL in a new tier after the last) and
`removeTrackers` (409 when none was listed) edit the tracker list through `replace_trackers` /
`add_tracker`; `setShareLimits` sets the torrent's own seeding goal (`ratioLimit` x 100 % and
`seedingTimeLimit` minutes; -1 is no target, -2 the default, both -2 drop the override;
`inactiveSeedingTimeLimit` is ignored).

`torrents/properties` (`isPrivate`, `comment`, `created_by`, `creation_date`), the trackers
list's "This torrent is private" rows and the native General tab's Created On / Created By /
Comment ask the engine once per torrent (`TorrentHandle::metadata`, a call answered on a
channel; a stopped torrent's info dictionary is read from the session store for it) and keep
the answer in `State::meta`, so a torrent restored from the state directory shows them too
(the engine keeps the `.torrent`'s comment, creator and date in its resume data). The
`isPrivate` field of `torrents/info` is the cached value: right for torrents added in this run
and for those whose properties were asked for. `torrents/pieceStates` is 0 missing, 1 with
blocks in flight (the picker's partial pieces, as the Pieces tab lists them), 2 had.
`torrents/addPeers` (`hashes`, `peers` as `ip:port|[ip6]:port`; 400 when none is an address)
connects to the peers. A `torrents/add` URL (and `/pa/add`'s `url`) that is an `http://` or
`https://` link is fetched through the session's proxy whenever one is set
(`FetchProxy::from_settings`): `proxy_peer_connections` and `proxy_tracker_connections` scope
the engine's peer and tracker connections only, as libtorrent's `.torrent` URL download took
the session proxy unconditionally.

#### The RSS Downloader

`webui/rss.rs` runs µTorrent 2.2.1's downloader (`frontend::rss`) on the status tick (so only
with `--webui`): due feeds and the `.torrent` files the filters pick are fetched on the shared
worker's threads (`frontend::rss::worker`, through the engine's proxy whenever one is set), never on
the alert or HTTP threads; the tick collects the results and adds through the same `add` as
`/pa/add` (the filter's Save in, Label, "highest priority" as Add to top of queue, "paused
state" as the page's Pause). `rss.dat` lives in the config directory beside `webui.json`. The
downloader has its own lock, always taken before the State lock. Every change bumps a version
that the `tick` message carries as `"rss": n` (sent when it changed since the client's last
tick), so an open dialog and the tree's Feeds branch reload `GET /pa/rss`. Hosted by the
Windows GUI the same endpoints edit the window's own downloader (see "Hosted by a desktop
client" below).

| Endpoint | Body | Does |
|---|---|---|
| `GET /pa/rss` | | `feeds` (`id`, `name`, `url`, `alias`, `enabled`, `subscribe`, `smart`, `items`, `updating`, `status`), `filters` (`id`, `name`, `enabled`, `pattern`, `not`, `save_in`, `feed` (0 all), `quality` (bit mask) and `quality_text`, `episodes`, `min_interval` (seconds), `label`, `smart`, `top_priority`, `paused`), `history` (`[name, date, feed]`, newest first), `qualities` (`[bit, name]`), `intervals` (`[seconds, name]`), `labels` |
| `GET /pa/rss/items?feed=ID` | | the item list (`frontend::rss::view`): `columns` and `rows` of `{feed, id, downloaded, can, cells}`; no `feed` (or 0) for every feed's |
| `GET /pa/rss/preview?filter=ID` | | the items of the feeds' last fetches the filter matches, `[title, feed]` |
| `/pa/rss/feed-add` | `{"url":"...","alias":"","subscribe":false,"smart":false}` | answers `{"id"}` and fetches the feed |
| `/pa/rss/feed-edit` | `{"id":n, "url", "alias", "enabled", "subscribe", "smart"}` (each optional) | a new URL drops the old items and fetches again |
| `/pa/rss/feed-remove`, `/pa/rss/feed-update` | `{"id":n}` (`feed-update` without one: every enabled feed) | |
| `/pa/rss/filter-add`, `/pa/rss/filter-edit` | the filter's fields as in `GET /pa/rss` (`filter-edit` with `id`; each optional) | `filter-add` answers `{"id"}` |
| `/pa/rss/filter-remove`, `filter-up`, `filter-down` | `{"id":n}` | order is priority: an item goes to the first filter that takes it |
| `/pa/rss/download` | `{"feed":n,"item":"<item id>"}` | the user's own download of an item (labelled with the feed's name) |
| `/pa/rss/history-clear`, `/pa/rss/process` | `{}` | `process` runs the filters over the feeds' current items (the page sends it when the dialog closes) |

The shim's `rss/*` map onto the same downloader: `addFeed` (`url`, `path` as the alias),
`removeItem`, `moveItem` (renames), `refreshItem`, `markAsRead` (kept in memory only),
`items` (`withData` for the articles; `isRead` is an added or marked article), `setRule` (a
filter by name; `mustContain` / `mustNotContain` in our wildcard syntax, `episodeFilter`,
`smartFilter`, `affectedFeeds` (one feed, else all), `ignoreDays` as the Minimum interval,
`addPaused`, `assignedCategory`, `savePath`; `useRegex` is ignored), `renameRule`,
`removeRule`, `rules`, `matchingArticles`. The RSS tree is flat (a feed's path is its name):
`addFolder` and paths with folders are 409.

#### The torrent search

`webui/search.rs` (D41, [INTEGRATIONS.md](INTEGRATIONS.md) part 2) searches the user's
Prowlarr or Jackett over Torznab with the shared model (`frontend::search`): one search at a
time for the whole server (every page sees the same results), one worker thread per source,
the answers merged by info-hash (else title and size) and sorted by seeders. The worker's
threads only answer into a channel; every alert batch (and every `GET /pa/search`) collects
them and bumps a version that the `tick` carries as `"search": n`, so an open search window
asks `GET /pa/search` again. The sources are `search.source.N.name|url|key|enabled` in the
stored preferences (`options`; the key sealed like the proxy password: DPAPI on Windows);
hosted they are the window's own `patorrent.ini` keys, reached through the preferences
journal, and a change the window makes moves the version too. A source on this machine or
network never goes through the engine's proxy, a public one does; a result's link may reach a
non-public address only on the source's own scheme, host and port.

| Endpoint | Body | Does |
|---|---|---|
| `GET /pa/search` | | `sources` (`index`, `name` (the host without one), `url_redacted` (any `apikey`-like query value masked), `enabled`, `has_key`; never the key), `running`, `id` (0 before the first search), `query` (`q`, `categories`), `statuses` (one per source searched: `source`, `status` `searching` / `done` / `failed` / `stopped`, `count`, `error` (the key removed), `text` (the translated line)), `total`, `results` (the first 500 by seeders: `id`, `name`, `size` and `size_text`, `seeders`, `peers` (null when unknown), `info_hash`, `categories`, `category_text`, `published` (Unix time) and `age_text`, `source_names`; never the link, which carries the indexer's key), `main_categories` (`[number, name]` for the category list) |
| `/pa/search/start` | `{"q":"...","categories":[5000],"sources":[index]}` (`categories`, `sources` optional) | 204; a new search replaces the last (its late answers are dropped). Without `sources` every enabled source is searched; named sources are searched even when switched off. 400 without `q`, 404 for an unknown source, 409 when there is nothing to search |
| `/pa/search/stop` | `{}` | the sources still searching show `stopped`; the results so far stay |
| `/pa/search/add` | `{"id":"<result id>"}` plus `/pa/add`'s options (`save`, `label`, `start`, ...) | fetches the result on the request's thread (its magnet, else its link: a `.torrent` or a redirect to a magnet, else a magnet made from the info-hash), then adds it as `/pa/add` does (hosted: into the window's engine); 200 `{"hash","text"}`, 404 for an id not in the current results, 409 when the torrent is in the list, 400 with the reason (the key removed) |
| `/pa/search/source-add` | `{"name","url","key","enabled","password"}` | 200 `{"index"}`; `url` must be `http://` or `https://`; needs the current password (below) |
| `/pa/search/source-edit` | `{"index", "name", "url", "key", "enabled", "clear_key", "password"}` (each but `index` optional) | 204; an omitted or empty `key` keeps the stored one, `clear_key` drops it; a new URL or a new key needs the current password, a name or the switch alone does not |
| `/pa/search/source-remove` | `{"index"}` | the rest are numbered again |
| `/pa/search/source-test` | `{"index"}` | asks the source for its caps (`t=caps`) on the request's thread (30 s at most) and answers when it is done: 200 `{"ok":true,"categories":n,"text"}` or `{"ok":false,"error","text"}` |

The shim's `search/*` (D41 slice 5, written from qBittorrent's `searchcontroller.cpp`) run
their own searches over the same sources, so a qBittorrent remote app's search tab works in
both modes (hosted: the window's sources and engine). Each source is a **plugin**: `name` is
`torznab-` and 8 hex digits of a SHA-1 of its URL with the key masked (stable while sources
are renumbered or re-keyed; a twin URL gets `-2`, ...), `fullName` its name (the host without
one), `url` its scheme, host and port only, `version` `1.0`, `enabled`, and
`supportedCategories` `[{id,name}]`: `all`, `anime` (Newznab 5070), `books` (7000), `games`
(1000, 4050), `movies` (2000), `music` (3000), `software` (4000), `tv` (5000); any other
category searches everything.

- `start` (`pattern`, `category`, `plugins`; 400 without one) answers `{"id":n}` (from 1,
  counting up). `plugins=enabled` (or `multi`) searches the enabled sources; `all` and plugins
  named with `|` are searched even when switched off, as in `/pa/search/start`; 409 when that
  leaves nothing, and 409 `Unable to create more than 5 concurrent searches.` with five still
  running (qBittorrent's limit). Searches are kept until `delete` (in memory: a restart drops
  them); past 32 a new one drops the oldest finished one.
- `status` (`id`, or none or 0 for all) answers `[{id, status: "Running"|"Stopped", pattern,
  category, plugins, total}]`; `stop` and `delete` (`id`) 200, 404 for an unknown id.
- `results` (`id`, `limit`, `offset`) answers `{status, results, total}`, the results in the
  order they first came (merging re-sorts the native list; clients page by offset), with
  qBittorrent's paging (a negative `offset` counts from the end, 409 past the end). Each has
  `fileName`, `fileUrl`, `fileSize`, `nbSeeders`, `nbLeechers` (peers less seeders), `engineName`
  (the plugin), `engineFullName`, `siteUrl` (the plugin's `url`), `descrLink` (empty: the
  model keeps no details page) and `pubDate` (Unix time); -1 for a number the indexer did not
  give.
- **`fileUrl` never holds the indexer's link** (it carries the API key): it is the result's
  magnet, or one made from its info-hash when it has no link, else
  `patorrent-search:<id>/<result id>`. `torrents/add` (`urls`) and `downloadTorrent`
  (`torrentUrl`; `pluginName` is ignored) take that URL back and fetch the result on the
  request's thread as `/pa/search/add` does (the link on the source's own origin may reach a
  non-public address, the proxy rule included); it works while its search is kept. Other
  `torrentUrl`s are added as `torrents/add` adds a URL.
- `enablePlugin` (`names` with `|`, `enable`) and `uninstallPlugin` (`names`) switch off or
  remove sources as the native switch and `source-remove` do (no password: neither points a
  source or its key anywhere new); unknown names are ignored. `installPlugin` is 409 (sources
  are added in the Web UI's search window, behind the password); `updatePlugins` is 200 and
  does nothing.
- `start`, `stop`, `delete`, `downloadTorrent` and the plugin changes take POST only; `status`,
  `results` and `plugins` GET too. Answers are collected on every alert batch and request.

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
bottom sheet opened by a tap, the list collapses to Name over a progress bar with status, speeds
and ETA, the context menu becomes a sheet behind a per-row actions button (a long press starts
the multi-selection), the toolbar keeps Add, Start, Pause, Remove and a More menu with the
rest and the sort. The drawer, the sheets and the dialogs each hold a `history` entry while
open, so the Android Back button closes the topmost one.

Drag-and-drop of `.torrent` files and magnet text onto the window opens the Add dialog. The Add
dialog is the 2.2.1 one: Save In, Name, the file tree with check boxes and priorities, Skip hash
check, Start torrent, Label. A magnet (Add URL, a drop, a paste) is added with `"dialog":true`
and waits in the session in upload mode, outside the queue, fetching its metadata and writing
nothing (`frontend::add::AwaitingMetadata`, the Win32 GUI's model too). When the engine hands out
its `.torrent`, the `tick` carries `"adds": n`; the page asks `GET /pa/add`, queues the `ready`
magnets it has not shown and opens the same dialog for each (`GET /pa/torrent-info?hash=`, Skip
hash check greyed: the files were checked when the metadata arrived). OK sends the choices with
the hash, Cancel (Esc, Back) removes the torrent. A magnet finished in another window closes its
dialog here; a page opened later shows the dialogs of magnets still waiting. Other URLs add
directly.

The toolbar's Find (beside the name filter; on the phone under More) opens Search Torrents:
the query (the filter box's text to start with), a category (All, Movies 2000, Audio 3000,
PC 4000, TV 5000, Books 7000, Other 8000), a source (all enabled ones, or one), Search and
Stop; each source's status line; the results (Name, Size, Seeds, Peers, Category, Age,
Source; a click on a header sorts, the choice kept in `localStorage`; the phone keeps Name,
Size and Seeds), added with a double-click or Add. Manage Sources lists the sources with a
check box to switch each on or off, Add / Edit (name, Torznab URL, API key (empty keeps it),
on or off, the current Web UI password), Remove and Test.

### Security

As today: SID cookie, PBKDF2 password hash, failed-login ban (per IPv4 address, per /64 for
IPv6), Host validation, bypass for local clients, CSRF checks on every `POST` and on the
WebSocket handshake: the `Origin` (or, without one, the `Referer`) must name the request's own
`Host`. The domain list is not consulted for this. A request with neither header passes,
since only browsers send them and the SID cookie is `SameSite=Strict`. Host validation is on
by default with an empty domain list (qBittorrent's default is `*`; owner's choice 2026-10-29:
keep it strict), so only IP addresses and `localhost` pass; the 400 answer names Preferences >
Web UI > Server domains (`webui.domain_list`), where a Docker or LAN host name is allowed
([INTEGRATIONS.md](INTEGRATIONS.md) gap 3). Added:
`Content-Security-Policy: default-src 'self'` (so no inline script or style in the HTML),
`X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `ETag` on the static files.
A new password ends every other login: their sessions, WebSockets and `/pa/remote`
connections; the session that changed it stays.

The login, the password, the local-auth bypass, Host validation, the domain list, CSRF
protection, the ban settings and the session timeout change only with the current password:
`webui.current_password` in `/pa/prefs`, or `web_ui_current_password` in the shim's
`setPreferences` (our key: qBittorrent's API has none, so its clients cannot change these). A
wrong current password ends the session that sent it. A failed login is counted before the
password is hashed, so parallel guesses stop at the ban limit.

Search sources (D41; owner may revisit): adding a source, or changing a source's URL or API
key, needs the current password too (`password` in `/pa/search/source-add` and
`source-edit`; a wrong one ends the session, as above). A source makes the server fetch its
URL wherever it points (the user's own Prowlarr is usually on this machine or network) and
send the stored key there, so a stolen session must not be able to point an existing source,
and its key, at a server of its choosing, nor make the server probe the network with
answers it can read. Add-by-URL and RSS feeds also fetch any address for a session, but they
answer one fixed text whatever failed (SEC-05, SEC-26); with sources behind the password, a
source's status line and Test can show the real reason (`HTTP 401`, a Torznab error, the
key removed), which is what a user setting up Prowlarr needs. Renaming, switching a source
on or off, removing it, testing it and searching need only the session. The key is never
sent to the page (`has_key` only; the URL with any `apikey` value masked), result links
(which carry the indexer's key) are never sent either, and a result's title is shown as
text only.

Behind a reverse proxy on the same machine every client connects from loopback. A request
with `X-Forwarded-For`, `Forwarded` or `X-Real-IP` never gets the local-auth bypass, and
from a loopback peer the last `X-Forwarded-For` address (the one the proxy added) is the
client for bans, so one client's failed logins do not ban everyone. Configure the proxy to
set or append that header.

Two more ways in, both off by default (owner decision 2026-10-30), both changed only with the
current password, and reported read-only by the shim's `app/preferences`:

- **API key** (`webui.api_key_enabled`, `webui.api_key`): `Authorization: Bearer <key>` stands
  in for a login (the *arrs' ApiKey field). The key is at least 16 characters, write-only, and
  stored as a PBKDF2 hash like the password; the SHA-256 of the last key that verified is kept
  in memory so the hash runs once per key, not per request. A wrong key counts as a failed
  login towards the client's ban, and a request that names a key is refused when it is wrong
  even from a whitelisted address. It works through a reverse proxy.
- **Subnet whitelist** (`webui.bypass_auth_subnets`, qBittorrent's
  `bypass_auth_subnet_whitelist`): `;`-separated CIDRs or bare addresses, IPv4 and IPv6 (an
  IPv4-mapped client counts as its IPv4 address); an entry that does not parse is refused.
  Like the local-auth bypass it never matches a proxied request (`X-Forwarded-For`,
  `Forwarded`, `X-Real-IP`), since behind a proxy the peer address says nothing about the
  client. Anyone on a whitelisted network can do everything a logged-in user can, including
  adding torrents that write to the save roots: whitelist only networks you trust as much as
  the login.

HTTPS (`--webui-https`, or `--webui-cert FILE [--webui-key FILE]`): the port serves TLS only,
through the OS library (SChannel on Windows, OpenSSL on Unix: `patorrent_core::tls`), TLS 1.2
and 1.3. `--webui-https` alone makes a self-signed certificate on first use (ECDSA P-256, 825
days, the loopback names, the host name and a specific listen address) and keeps it in the
config directory as `webui-cert.pem` / `webui-key.pem`, renewed a month before it expires; nox
prints its SHA-256 fingerprint at every start, which is what a browser's certificate warning
should be checked against. With HTTPS the session cookie is `Secure`, and a plain-HTTP request
to the port gets a `301` to the `https://` address. On Windows the key is imported into the
running account's CNG key store under a name of the process (SChannel signs in LSASS, which
cannot use a key held only in our process). It is deleted once the Web UI and its last
connection are gone; when that did not happen before the process ended (killed, or a WebSocket
still open at exit), the next start deletes it. It is a copy of a key that is on disk anyway.

### Language

The page's texts come from the string table (D20, `frontend::lang`) in the server's language:
`lang.js` (served beside `app.js`, without a login, made at the first request) sets
`window.PA_LANG` to the language code, its direction (`dir`: `ltr`, or `rtl` for Arabic,
Hebrew and Persian) and every text by key, the `&` mnemonics taken out. A right-to-left
language sets `dir="rtl"` on the page: the stylesheet uses logical properties
(`inset-inline-start`, `text-align: end`, ...) so the layout mirrors, and the script turns
column drags, the tree splitter, menus and their arrow keys the other way. The static HTML marks its texts with `data-t` (and `data-t-title`, `data-t-aria`, `data-t-ph`)
and keeps the English in place for a browser without scripts; `app.js` replaces them at load
and takes its own texts with `T(key, args)`. What the server builds (cells, column titles,
the detail tabs, Preferences, the Logger) arrives translated. Tests check that every key the
page and the script name exists and that the HTML's English is the table's.

The language is Preferences > General's "User interface" (`ui.language` in `webui.json`, a
choice of `[value, text]` with "System default" first; `PATORRENT_LANG` over it). Applying a new
one switches the server's texts at once and rebuilds `lang.js`; the page that applied it
reloads, other open pages keep their texts until they reload. The compat API's `locale` names
the language in force.

## Remote mode

A GUI can drive a nox's engine instead of its own (`patorrent_core::remote`; the server side is
`webui/remote.rs`). The client is a `Session` like a local one: its handles queue the engine's
own commands, and instead of an engine thread a pump thread sends them to the nox and posts the
nox's alerts into the client's alert queue. The GUI's models, dialogs and automations run
unchanged on top.

- **Transport.** An HTTP/1.1 upgrade on the Web UI's port (HTTPS too):
  `GET /pa/remote` with `Connection: Upgrade`, `Upgrade: patorrent-remote`,
  `X-PaTorrent-Remote: 7` (the protocol version) and the Web UI login as
  `Authorization: Basic`. The checks are the WebSocket's: ban list, `Host`, `Origin` when there
  is one (only a browser sends it), then the login, whose failures count towards the ban like
  the login form's (a session cookie or the local-auth bypass also do). `101` switches; a wrong
  version gets `426`, a wrong login `401`.
- **Frames.** A 4-byte big-endian length and one bencoded list: the message name, then the
  fields in declaration order (positional, so both ends must speak the same version; structs
  are lists, options are lists of zero or one, flags integers, addresses text). The server's
  first frame is `hello`: version, the server's PaTorrent version, its default save path and
  its engine settings.
- **Client to server**: the engine commands (add with the `.torrent` bytes or the magnet, remove,
  pause, flags, queue, limits, trackers, priorities, move storage, rename, the `post_*` detail
  requests, settings, session pause). A session's IP and port filter tables stay on the client
  (the server's own filter applies; the client logs that it was not sent).
- **Server to client**: every alert of the nox's engine, queued per client (a client more than
  100,000 alerts behind is disconnected and resynchronises on reconnect), except the replies to
  the nox's own status requests. A client's `post_torrent_updates` is answered from the cache:
  the torrents whose version passed what this client was last sent (all of them the first
  time), so each client has its own "changed since" as with a local engine.
  `post_session_stats` gets the cache's statistics.
- **Settings** sent by a client take the Preferences dialog's path (`prefs::commit`): checked,
  applied with the global limits of the mode in force, stored in `webui.json`.
- **Certificates.** Over HTTPS a client either trusts the system's roots or pins one
  certificate's SHA-256 fingerprint. `remote::probe_certificate` reads the fingerprint of a
  self-signed server without sending it anything, so the GUI can show it to the user (to
  compare with the line nox printed) before the password goes out.
- **Lost connections** end in an `Alert::RemoteDisconnected`; the client's commands go nowhere
  after it.

The Win32 GUI (`remote_ui.rs`): `patorrent.exe --remote`, or File > Connect to Remote... (which
starts such a window beside the local one), asks for the host, port, login and HTTPS (on by
default). Plain HTTP to a host other than this computer is asked about first, since the login
and every command would cross the network unencrypted; `remote::connect` refuses it without
that answer (`Target::allow_plaintext`). The GTK GUI asks the same. A
certificate Windows does not trust is shown by its fingerprint and pinned once accepted; a
pinned one that changes is refused with a warning and asked about again at the next attempt.
The window then runs on the remote session with the server's engine settings and default
folder, and keeps its own files (settings, labels, RSS feeds) in `remote\<host>_<port>` in the
settings folder; the connection details and pins are in `remote.ini` there, the password is
never stored. Folder pickers for torrents, Open Containing Folder, seeding a torrent created
here and the local IP filter are off: those folders would be this computer's. The window's
own automations (scheduler, RSS, seeding goal, `.torrent` folders, scrapes) act on the server's
engine too, beside nox's own. A lost connection is reported once and marks the title
"(disconnected)"; reconnecting means a new window.

The GTK GUI (`crates/gtk/src/remote.rs`) does the same: `patorrent-gtk --remote`, or File >
Connect to Remote... A remote window is a process of its own outside GTK's single instance
(`G_APPLICATION_NON_UNIQUE`), so a second local start still goes to the local window and
several servers can be open at once. The certificate check and the connection run on GIO's
thread pool while the dialog waits greyed with a spinner. Its files are in
`remote/<host>_<port>` in `~/.config/patorrent/gtk`, `remote.ini` beside the local
`patorrent.ini`. Set Download Location asks for a folder on the server as text (the Web UI's
question), as the Win32 window now does too, and completion notifications go to
`org.freedesktop.Notifications` directly (a non-unique application has no bus name for
`GNotification`). The connect step itself (the form, the pins, the failure texts) is shared in
`patorrent_frontend::remote`.

### Hosted by a desktop client

The Windows GUI serves the Web UI over its own engine (D40), as µTorrent and qBittorrent do:
`patorrent_nox::webui::WebUi` started with `Config::hosted` (`Hosted { host, options }`), fed
every alert batch the GUI pops, exactly as nox's main loop does. The host implements
`webui::Host`: `options_changed(changes)` (called on a request thread with the Web UI's lock
held: queue them) and `shutdown()`.

- **The mirror.** The GUI is the application: it owns `patorrent.ini`, the labels
  (`torrent.<key>.label`), the pause marks (`torrent.<key>.paused`), the seeding-goal
  overrides, the engine settings (`lt.*`), its options and every automation. The Web UI keeps
  a copy of those preferences (`State::options`, started from `Hosted::options`) and the two
  sides exchange changes through `patorrent_frontend::prefs::Prefs`' journal: whatever a browser
  changes (a label, a pause mark, a Preferences page, the session limits, a seeding goal, the
  login as `webui.username` / `webui.password`, the hash) is written into the mirror as the
  host's keys and handed to `Host::options_changed`; whatever the host changes comes back
  through `WebUi::options_changed`, which updates the lists (clients see it at the next tick),
  replaces the login (a new hash ends every session, SEC-24) and re-reads the settings and
  options the Preferences pages show. The key is `InfoHashes::key_hex` (the v1 hash, else the
  full v2 one); the Web UI keys torrents by their v1-compatible hash, so it keeps a second
  index. A torrent's first sight takes the host's label and pause mark.
- **Nothing on disk, nothing to the engine but commands.** No `webui.json`, no `rss.dat`: the
  login comes from `webui.username` / `webui.password` in the options (a temporary password
  when there is none, as nox makes one), the label list from the `torrent.*.label` keys. A
  Preferences `POST` is parsed, checked and stored as always, but the host applies it: no
  `apply_settings`, IP filter, peer-exchange flags or language switch here (the host's language
  is the process's; the page's `lang.js` follows it). `State::settings` is the host's
  configured settings (`settings::load` of the `lt.*` keys).
- **The RSS Downloader is the host's own, shared, not proxied**. Two designs were weighed:
  (a) the host's model behind an `Arc<Mutex<..>>` that the request threads lock, or
  (b) `Host` methods marshalling each RSS request to the host's thread and waiting for its
  JSON. (a) was chosen: `frontend::rss` is UI-independent (the model and its worker know no
  window), so the endpoint code of `rss.rs` runs unchanged against the host's model, nothing
  waits on the host's thread (no round trip, no timeout, no deadlock with a modal dialog's
  loop), and there is one model, never two downloaders. `Hosted::rss` is the window's
  `Arc<Mutex<Rss>>` (`Rss`: the `RssDownloader`, its `Worker`, `dirty`); `Shared::rss` is that
  same `Arc`. The window keeps the worker (its fetches post to the window, which collects
  them; nox's tick does not run), the downloads and `rss.dat` (saved by its timer when
  `Rss::dirty`). A browser's change calls `Host::rss_changed` and its downloads (an item's
  Download, the filters run by `process`, a shim rule) go to `Host::rss_download`: the model
  has marked them added and logged them in the History, and the window adds them through its
  own RSS add path (label, top of queue, paused, the Add dialog's defaults, as for its own
  RSS adds). The window's own changes (a fetch, its dialog, the tree's Feeds menu) call
  `WebUi::rss_changed`, which moves the `tick`'s `rss`. **Lock order, every thread: the RSS
  lock, then the Web UI's State lock**; the window never holds the RSS lock across a dialog,
  a message loop or a call that locks it again, and the `Host` methods only queue and post
  (`WM_APP_WEBUI`), so a request thread never waits on the window. A feed's error text is
  nox's one line (`could not fetch <url>`) in the page, not the window worker's detail
  (SEC-05, SEC-26).
- **Off.** The scheduler, seeding goal, scrapes, `.torrent` folders, auto-load and
  move-completed are the host's; `hello` carries `"hosted": true` (false under nox). The
  alternative speed limits do not exist (the host has none): the Bandwidth page has no such
  row, `alt_enabled` is refused (400 from `POST /pa/prefs`), `POST /pa/session/limits` with
  `alt` / `alt_down` / `alt_up` and the shim's `toggleSpeedLimitsMode` / `setSpeedLimitsMode`
  answer 409, `speedLimitsMode` is 0. The normal limits work and reach the host as
  `lt.download_rate_limit` / `lt.upload_rate_limit`.
- **Shutdown.** `app/shutdown` and `session/shutdown` call `Host::shutdown` (without the lock)
  besides marking the Web UI shut down.

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

Measured with slice 3 (CHANGELOG 2026-10-17_220311_1792267391): all within budget in Chrome and
Firefox. The page measures itself with `#gate` in its URL (a panel and the window title); the
daemon loads the profile with `run --no-state --mock-library 10000 --active 0`.

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
   the shim while there). Landed: CHANGELOG 2026-10-17_224011_1792269611.
5. **Commands and the Add dialog**: everything in the Win32 context menu, upload, magnet, the
   two-step add with the file tree, Remove and delete data. The shim's handlers moved onto the
   shared functions. Landed: CHANGELOG 2026-10-18 (slice 5).
6. **Preferences** generated from `frontend::options`, the 2.2.1 pages. Landed: CHANGELOG
   2026-10-18_101527_1792311327.
7. **Phone layout** pass, then the long-poll fallback if a deployment needs it. The phone
   layout landed: CHANGELOG 2026-10-18_230237_1792357357.
8. Later, with Phase 5: labels in the tree and the Label column (the tree's Labels branch with
   "No Label" and live counts landed: CHANGELOG 2026-10-18_232431_1792358671), RSS (landed: the
   RSS Downloader dialog with Items, Filters and History, the tree's Feeds branch, `/pa/rss` and
   the shim's `rss/*`), the scheduler
   (landed: the Preferences > Scheduler page with its 7 x 24 table, CHANGELOG 2026-10-18_235124_1792360284), per-torrent
   limits; and the GUIs' remote mode on its own binary stream.

## Open points

- ~~Whether the WebSocket thread reads and writes on one thread with a short read timeout or uses
  a second thread per socket.~~ Decided in slice 1 (CHANGELOG 2026-10-17_210452_1792263892): one
  thread per socket, a 50 ms read timeout, and the cache's tick counter checked after every read
  or timeout; no second thread, no Condvar wait.
- ~~A phone's long-press context menu versus a visible per-row menu button.~~ Decided in slice 7
  (CHANGELOG 2026-10-18_230237_1792357357): the button; a long press starts the multi-selection.
- Compression: none for now (no dependencies, and the window-bounded messages are small). If a
  real remote link wants it, `permessage-deflate` needs an inflater and deflater of our own.
