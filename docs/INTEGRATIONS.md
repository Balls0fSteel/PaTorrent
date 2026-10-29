# Sonarr, Radarr, Prowlarr and Torznab search

Status 2026-10-29_170000_1793289600: part 1's gaps 1, 2 and 7 are fixed (1.1.3, 1.1.5) and gap 3 is decided (strict, 1.1.4). Real Sonarr, Radarr and Prowlarr passed against nox on Linux (gap 6); the rest are open. Part 2 is a proposal (D41 pending), nothing is built.

Two ways PaTorrent meets the *arr tools:

1. **As their download client.** Sonarr, Radarr and Prowlarr drive qBittorrent over its WebAPI v2, and nox (or the Windows GUI's hosted Web UI, D40) answers that API through the compatibility shim (D10, [WEBUI.md](WEBUI.md) "Compat"). Part 1 lists what they send and where the shim falls short.
2. **As a search front end.** Instead of qBittorrent's Python search plugins or search modules of our own, PaTorrent asks the user's Prowlarr or Jackett over Torznab. Part 2 is the design.

Sources: Sonarr, Radarr and Prowlarr `develop` (`src/NzbDrone.Core/Download/Clients/QBittorrent/`, read 2026-10-29; Sonarr's last change there `4b7fc91`), Prowlarr's `NewznabController.cs` / `NewznabResults.cs` and Jackett's `ResultsController.cs` / `ResultPage.cs`. Lidarr matches Radarr except for the category name. The live check (gap 6) used the current linuxserver.io images (Sonarr 4.0.20) and [tools/arr-test](../tools/arr-test/README.md).

## Part 1: the shim as an *arr download client

### What they call

All three use the same V2 proxy. Prowlarr only adds torrents; Sonarr and Radarr also track, import and remove them.

| Call | Use | Shim |
|---|---|---|
| `GET app/webapiVersion`, unauthenticated first | 403 or 200 means v2; then parsed as a .NET `Version` | 403, then `2.11.2`: three parts and no suffix, as required |
| `POST auth/login` | only with both a user name and a password; any cookie is kept | `SID` |
| `GET app/preferences` | `save_path`, `dht`, `queueing_enabled`, `max_ratio_enabled`, `max_ratio`, `max_seeding_time_enabled`, `max_seeding_time`, `max_inactive_seeding_time*`, `max_ratio_act` | yes since 1.1.3 (gap 2) |
| `GET torrents/info?category=` | `hash`, `name`, `size`, `progress`, `eta`, `state`, `category`, `save_path`, `content_path`, `ratio`, `ratio_limit`, `seeding_time_limit`, `inactive_seeding_time_limit`, `last_activity`, Sonarr `root_path` | yes since 1.1.3 (gap 2); no `root_path` or `inactive_*` (their defaults apply) |
| `GET torrents/properties?hash=` | `save_path`, `seeding_time`; also the "is it added yet" probe after an add (10 tries, 100 ms apart) | yes |
| `GET torrents/files?hash=` | `name`, for the root folder at import | yes |
| `POST torrents/add` | `urls` (always a magnet: the *arr fetches `.torrent` files itself and uploads them as `torrents`), `category`, `stopped`/`paused` (`True`/`False`), `sequentialDownload`, `firstLastPiecePrio`, `contentLayout`, and from WebAPI 2.8.1 `ratioLimit`, `seedingTimeLimit` / Sonarr's `inactiveSeedingTimeLimit` | limits kept since 1.1.3 (gap 1); the rest below |
| `POST torrents/delete` | `hashes`, `deleteFiles=true` | yes |
| `GET torrents/categories`, `POST createCategory`, `POST setCategory` | the category must exist; keyed `savePath` | yes |
| `POST setShareLimits` | only below WebAPI 2.8.1 | yes (stored as the torrent's seeding goal) |
| `POST topPrio`, `POST setForceStart` | "First" priority, "Force Start" initial state | yes |
| `POST torrents/addTags` | Sonarr's "Add Series Tags" | 404 (gap 5) |

Booleans arrive capitalised (`True`), long values and files as multipart, the rest urlencoded; the shim already takes all three. They send no `Origin` or `Referer`, so CSRF protection passes. With an API key configured they send `Authorization: Bearer` and never log in; with a user name and password they also send `Authorization: Basic` on every request.

State mapping (Sonarr and Radarr): `pausedUP`/`stoppedUP`, `uploading`, `stalledUP`, `queuedUP`, `forcedUP` are Completed; `pausedDL`/`stoppedDL` Paused; `queuedDL`, `checking*` Queued; `metaDL` Queued with DHT on; `stalledDL`, `missingFiles`, `error` warnings; `downloading`, `forcedDL`, `moving` Downloading. The shim's `pausedUP`/`pausedDL` are accepted, so moving to 5.x's `stopped*` names is not needed for them.

**Remove Completed** (an *arr setting) removes a torrent only when its state is `pausedUP`/`stoppedUP` **and** a seed limit is reached: its own `ratio_limit` (0 or more), the global `max_ratio` when `ratio_limit` is `-2` and `max_ratio_enabled`, or the seeding-time limits likewise.

### Gaps, in order

1. **Fixed in 1.1.3: share limits on add were dropped.** We report WebAPI 2.11.2, so the *arrs send `ratioLimit` / `seedingTimeLimit` with `torrents/add` and never call `setShareLimits` afterwards. `api::add` ignores them: Prowlarr's per-indexer seed ratio and time (private trackers' minimums) are lost. Fix: carry them into `AddOptions` and store them as the torrent's seeding-goal override, exactly as `commands::share_limits` does.
2. **Fixed in 1.1.3: Remove Completed never fired.** `torrents/info` always said `ratio_limit: -2` and `seeding_time_limit: -2`, and `app/preferences` has no `max_ratio_*` keys, so the *arr sees no limit, while nox's default seeding goal (150 %, then Stop) does stop torrents as `pausedUP`. Completed downloads stay in the *arr's queue and in the client forever. Fix: report each torrent's override (ratio percent / 100, minutes; `-2` without one; `-1` for a target that is off), and report the global goal as `max_ratio_enabled` / `max_ratio` / `max_seeding_time_enabled` / `max_seeding_time` with `max_ratio_act: 0` (Pause), enabled only when the goal's action is Stop (with an upload cap the torrent keeps seeding, so no limit is "reached" for the *arr). Never report Remove (1) or Delete (3): the *arr's Test refuses those.
   - Semantics differ: µTorrent's goal needs every set target (ratio **and** time), qBittorrent's either. The *arr may consider a limit reached earlier than we stop, but it removes only a stopped torrent, so ours is the stricter and safe side. Document it; do not change the seeding goal.
3. **Decided 2026-10-29 (owner): stays strict; 1.1.4 says how to allow a name.** Host names are refused by default. Host header validation is on with an empty domain list, so only IP addresses and `localhost` pass. An *arr in Docker that names the client by service or host name (`http://patorrent:8080`) gets `400 unexpected Host header`, and the Test reports a connection failure. qBittorrent's default domain list is `*`. Owner choice: keep the safe default and make the cause obvious (the error text names `webui.domain_list`; WEBUI.md and the site's FAQ show the setting), or ship `*`. Recommended: the former.
4. **No credential-free access from other machines.** Without a user name and password the *arrs rely on qBittorrent's localhost or subnet whitelist; the shim has only the loopback bypass, and a reverse proxy loses it (SEC-03). Workaround: give the *arr the login. A `bypass_auth_subnet_whitelist` is possible later; it needs a security note.
5. **Small ones, warnings only:** `addTags` (Sonarr logs a warning per grab with series tags on; tags do not exist in µTorrent); the `Authorization: Bearer` API key (the *arrs' ApiKey field; use the login instead); `firstLastPiecePrio`, `contentLayout=Subfolder` and `inactiveSeedingTimeLimit` are ignored; a duplicate add answers `Ok.` rather than 409 (the *arr then tracks the existing torrent, which is fine).
6. **Run 2026-10-29 against real apps** (Docker on the Xubuntu VM, nox 1.1.4-1.1.5 debug build, [tools/arr-test](../tools/arr-test/README.md)). Passed:
   - Sonarr, Radarr and Prowlarr's Test: refused by host name (Sonarr's log shows the 1.1.4 hint), passing once the name is allowed. The Tests created the `tv-sonarr`, `radarr` and `prowlarr` categories.
   - A `.torrent` grab from each app, with an indexer seed time of 1 minute: the limit arrives (`seeding_time_limit 1`, gap 1).
   - Import by hardlink, nox's seeding goal stopping the torrent about a minute after it finished, and Sonarr and Radarr removing the torrent and its download file within seconds (gap 2).
   - Prowlarr's search and grab into its category.

   Not covered: a magnet grab (the synthetic swarm has no peers to send metadata); "First" priority and Force Start, which depend on the add being visible to `torrents/properties` within the app's one second; Lidarr and Readarr.

   The Windows GUI's hosted Web UI (D40) takes the same shim paths, with the window's own seeding goal stopping the torrent; `the_arr_apps_limits_and_categories_are_the_hosts_keys` (`crates/nox/tests/hosted.rs`) covers them and found `removeCategories` leaving the window's label (fixed in 1.1.6). The live run against `patorrent.exe` is still to do; [tools/arr-test](../tools/arr-test/README.md) has its setup.
7. **Fixed in 1.1.5 (found by gap 6): an idle seed's seeding time stood still.** The engine posts a status update only when a torrent transfers, has peers or is checking, as libtorrent does, so a seed with no peers kept the seeding time of its last transfer. The time-based seeding goal, which nox and both GUIs check on status updates, was then never reached, and `seeding_time` in the Web API and the GUIs' columns stayed stale. Sonarr's grab sat seeding for minutes past its 1-minute limit; after a restart the restored time (386 s) stopped it at once. Fix (`Torrent::add_running_time`): a running torrent's status is also posted each time its active or seeding time completes a whole minute. With queueing only active torrents tick, so that is a few updates a second even with 100,000 torrents; with queueing off and many running seeds it is one update per torrent a minute.

Gaps 1 and 2 were fixed in `crates/nox/src/webui/api.rs`, `commands.rs` and `torrent.rs` (1.1.3), tested by `the_arr_apps_share_limits_round_trip` in `crates/nox/tests/webui.rs`. `setShareLimits` now also marks the torrent changed, so `sync/maindata` sends the new limits. A ratio limit of 0 (qBittorrent: stop at once) is a target that is off here, as in µTorrent.

## Part 2: Torznab search (proposal, D41)

### Why Torznab

qBittorrent's search runs Python plugins that scrape sites; they break whenever a site changes and need a working Python. Writing our own scrapers would be the same treadmill. Prowlarr and Jackett already maintain hundreds of indexer definitions and serve them all through one small, stable API, Torznab (RSS 2.0 with `torznab:attr` elements). Users who run Sonarr or Radarr almost always run one of them. PaTorrent only needs an HTTP GET and the RSS parser it already has: no Python, no plugins, no new dependency.

Not in scope: qBittorrent search plugins, scraping sites directly, or acting as an indexer.

### The two servers

| | Prowlarr | Jackett |
|---|---|---|
| Endpoint | `GET {base}/{indexerId}/api` (one per indexer; no aggregate Torznab endpoint) | `GET {base}/api/v2.0/indexers/{id}/results/torznab/`; `id` may be `all` or a `tag:`/`type:` filter |
| Key | `apikey=` query, or `X-Api-Key` header | `apikey=` query |
| Results | `seeders`, `peers`, `infohash`, `size`, categories; the link is a proxied `/{id}/download?...` that answers a `.torrent`, or a 301 to a magnet; no `magneturl` attribute | the same attributes plus `magneturl` (the raw magnet); the link is a proxied `/dl/...` that answers a `.torrent` or a 302 to a magnet |
| Errors | `<error code= description=/>`, 400/410/429/500 | the same element, often with HTTP 200 (bad key: code 100) |

Both take `t=caps` (capabilities and categories) and `t=search&q=&cat=&limit=&offset=`. Categories are the Newznab numbers (2000 movies, 3000 audio, 4000 PC, 5000 TV, 7000 books, 8000 other).

### Model (`patorrent-frontend`, shared by all three frontends)

- **Sources**: a list of `{name, url, api_key, enabled}`, stored in the preferences (`search.source.N.*`). A Jackett user adds one source (`all`); a Prowlarr user adds one per indexer, or uses **Import from Prowlarr**, which reads Prowlarr's indexer list (`/api/v1/indexer`, JSON, same key) and adds a source per enabled torrent indexer. The import's exact JSON shape is to be confirmed against Prowlarr's source when it is built. Each source's Test runs `t=caps`.
- **A search**: one query (text plus an optional category) sent to every enabled source in parallel, one worker thread each, the same way the RSS worker fetches feeds (`frontend::rss::worker`), 30 s timeout, `limit=100`. Results are merged as each source answers, de-duplicated by info-hash (else by title and size), and sorted by seeders. A source that fails shows its error beside the results; the others still count.
- **Parsing**: `frontend::rss::feed` gains the item's `size` element and a generic list of `torznab:attr` / `newznab:attr` name-value pairs; `frontend::search::torznab` turns an item into a result (name, size, seeders, peers, info-hash, category, published date, source) and an `<error>` document into a message. Same caps as feeds: 16 MiB body, `MAX_ITEMS`.
- **Adding a result**: the `magneturl` attribute when present, else the link, fetched as RSS items are (a `.torrent`, or a redirect to a magnet: `rss::worker::Download`), then the frontend's normal add path, which shows the Add dialog unless the user turned it off.

### Security

- **Reach (SEC-26)**: the source URL is the user's, so the query goes to any address, Prowlarr on `localhost` included. A result's link may be followed with `Reach::User` only when it points at the source's own host (both servers proxy their downloads through themselves); any other link gets `Reach::Public` (`Reach::for_links_from` already does this for Jackett-style local feeds).
- **The API key** is stored like an RSS feed's cookies, never logged, and removed from any URL shown in an error or in the Web UI. The Web UI's native API returns sources without their keys.
- Result titles and descriptions are untrusted text: shown as text, never as HTML.
- **Proxy**: `http_get` uses the engine's proxy whenever one is set, which breaks a Prowlarr on this machine or this network. Proposed: a source on a non-public address never goes through the proxy; a public one does. Owner to confirm.

### User interface

- **Win32** (D13, D35): 2.2.1's toolbar search box had a drop-down of web search engines. Ours keeps filtering the list by name (D35) and gains that drop-down: "Filter torrents" (the default, as now), each source, "All sources", and "Manage search sources...". With a source picked, Enter opens a **Search Results** window: a list view in the 2.2.1 style (Name, Size, Seeds, Peers, Category, Age, Source), sortable, with Add and double-click to add. Sources are managed in their own dialog, so Preferences keeps 2.2.1's pages.
- **GTK**: the same model behind the `SearchEntry`'s menu and a results window.
- **Web UI**: a search panel; native commands `/pa/search/start`, `/pa/search/stop` and the sources' CRUD, results pushed over the WebSocket as a `search` message. Hosted mode (D40) uses the host's sources.
- **Later, optional**: qBittorrent's `search/*` WebAPI on the shim (`start`, `status`, `results`, `stop`, `delete`, `plugins`, each source shown as a plugin), so qBittorrent remote apps' search tab works.

### Slices

1. Frontend: sources, the Torznab parser and the merged search, with fixtures captured from Prowlarr and Jackett (results, caps, errors, a magnet redirect).
2. nox: native commands, the Web UI panel, the source settings.
3. Win32: the drop-down, Search Results and Manage Sources dialogs.
4. GTK: the same.
5. Optional: the shim's `search/*` endpoints.

About 20 new strings, in all 34 tables.

### Open choices for the owner

- Approve D41 (Torznab sources as PaTorrent's search; no Python plugins, no scrapers of our own).
- The proxy rule above.
- The Win32 drop-down and Search Results window as the place for it.
- Gap 3's default (keep the strict Host check, recommended, or `*`).
