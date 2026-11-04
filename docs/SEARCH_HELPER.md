# The search helper (D51)

For users without a Prowlarr or Jackett of their own: Options > Search Sources offers "Get the Search Helper (Jackett)...". PaTorrent downloads Jackett's own release from GitHub at that click, unpacks it into the profile folder and runs it as a separate program on `127.0.0.1`, only while searching. Its `all` Torznab endpoint becomes an ordinary search source, "Search helper (all)", so the D41 search (`docs/INTEGRATIONS.md` part 2) does everything else. The design is the D51 section of `docs/PROPOSALS.md`, way A. This file describes what was built.

## Licence position (from PROPOSALS.md)

Jackett is GPL-2.0, Prowlarr GPL-3.0; PaTorrent is PolyForm Noncommercial (D15), which cannot be combined with either into one program. Way A, the one built: PaTorrent downloads the upstream's own release from GitHub when the user clicks, into the profile folder, and runs it as a separate process over HTTP. PaTorrent distributes nothing of theirs; two separate programs talking over a socket are separate works under the GPL FAQ. The user gets the GPL program from its authors, with its own licence file. Bundling (way B) was rejected: we would then distribute a GPL binary with its licence text and the corresponding source for every release, and 50 MB more in every package against D7's one small file. Our own Cardigann interpreter (way C) stays a later option.

## Where it is off

`patorrent_frontend::search::helper::available()`:

- builds with `PATORRENT_NO_UPDATE_CHECK` (a package manager updates them), and the Store flavour's `PATORRENT_STORE` (D46, `docs/STORE.md`; that flavour is not built yet, the switch is honoured already);
- a Flatpak (`FLATPAK_ID` set: a sandbox that downloads and runs programs would not pass Flathub's review), as the update check is;
- systems with no Jackett build: FreeBSD, and any OS or CPU without an asset below.

There, the Search Sources window shows no helper buttons; "Add" for a Prowlarr or Jackett of the user's own stays. nox and the Web UI get the buttons in a later slice.

## As built

Frontend, `crates/frontend/src/search/helper/`:

| File | What |
|---|---|
| `release.rs` | `GET https://api.github.com/repos/Jackett/Jackett/releases/latest`; the asset for this platform; the download into memory with the asset's `digest` (`sha256:<hex>`) checked. An asset without a digest is refused. `PATORRENT_HELPER_RELEASES` names another listing for a hand test. |
| `archive.rs` | A zip reader (the central directory, then each local header; stored and deflated entries, Zip64 sizes) and a tar.gz reader (ustar, GNU long names, pax `path`/`size`), both through the engine's inflater (`util::puff`, `util::gzip`). Any name that is absolute, has a drive, a `..` or a NUL refuses the whole archive; links are skipped, never made; 1 GiB and 100,000 entries at most. The executable bit is kept on Unix. |
| `config.rs` | The folder (`Layout`), `helper.ini` (version, port, API key and admin password; the last two sealed with DPAPI on Windows like a source's key, `prefs::SEALED`), `ServerConfig.json`, the proxy mapping. |
| `sha512.rs` | SHA-512 for Jackett's password hash. |
| `json.rs` | A small JSON reader and writer (the frontend had none; no dependency, D4). |
| `api.rs` | A tiny HTTP/1.1 client to `127.0.0.1:<port>` only (the shared fetch only `GET`s) and the admin API calls below. |
| `process.rs` | `Runner`: the child process, the health check, the idle stop and the kill. |
| `mod.rs` | The process-wide helper (`configure`, `route`, `install`, `remove`, `list_sites`, `apply_sites`, `page`, `shutdown`) and the "Search helper (all)" source. |

Folder: `<profile folder>/search-helper/` (Windows: `%APPDATA%\PaTorrent[\profiles\<name>]\search-helper`, or the portable folder; GTK: the profile's data folder, `~/.local/share/patorrent[/profiles/<name>]/gtk/search-helper`):

- `app/Jackett/...`: the unpacked release (`JackettConsole.exe` on Windows, `jackett` elsewhere). An update unpacks into `app.new` and swaps it in; a failed unpack leaves the old one.
- `data/`: Jackett's `--DataFolder` (its `ServerConfig.json`, `Indexers/`, `log.txt`).
- `helper.ini`: ours.

Install: the newest release is asked for first (a marquee), the confirm box names what it is (a separate program by its own authors under the GPL-2.0, from github.com/Jackett/Jackett), the version, the size, the folder, and that it runs only while searching; then the download behind the updater's progress dialog (Win32: `updater::run`, the D45 dialog made generic; GTK: a small progress window), the unpack, a generated API key (32 characters `[a-z0-9]`, as Jackett's own) and admin password (24), the source added and enabled, and the site picker. Update is the same path again (the key and password stay, so the source keeps working); "up to date" when the tag is the installed one. Remove stops it, deletes the folder and takes its source out. Never automatic.

Running: the first search of the helper's source starts it (`worker::fetch_results`, `fetch_caps` and `fetch_download` go through `helper::route`, which recognises the source by its key on `127.0.0.1` and rewrites its URL to the running helper's port). A start picks a free loopback port (the last one when it is still free), writes `ServerConfig.json`, starts the program with no console window (`CREATE_NO_WINDOW`), no inherited output, its own folder as the working directory, then polls `/health` every 250 ms for up to 90 s; a child that exits first or does not answer is killed. A thread stops it after ten minutes without a search (the API's shutdown call, five seconds, then a kill). The GUIs call `helper::shutdown()` at exit (a kill; a start under way gives up). Searches of other sources never wait on a start.

Proxy: our fetch of the release goes through the session's proxy (github.com is public, D41's rule). The helper's own proxy (`ProxyType`, `ProxyUrl`, `ProxyPort`, `ProxyUsername`, `ProxyPassword`) is written from ours at every start: HTTP 0, SOCKS4 1, SOCKS5 2, none -1. The helper's Torznab answers come over loopback, never through the proxy.

Sites: after the install, and on "Pick Sites...", the helper is started, signed in to, and its public indexers listed (name, language, type), sorted by name. Nothing is ticked for the user; a site ticked before stays ticked. A site whose Cardigann definition (`Definitions/<id>.yml` in the release) has `type: info_flaresolverr` is greyed with "Needs FlareSolverr, which the helper does not include" and cannot be ticked. On OK, newly ticked sites are configured with their defaults, unticked configured ones removed; failures are listed per site. "Open the Helper's Page" starts it, copies the generated password to the clipboard, shows it, and opens `http://127.0.0.1:<port>/UI/Dashboard` in the browser, for private sites and their logins.

## Jackett's options, settings and API, with their sources

Read from `github.com/Jackett/Jackett`, master, 2026-11-04 (source text only; nothing was run).

Command line (`src/Jackett.Common/Models/Config/ConsoleOptions.cs`, parsed case-insensitively in `src/Jackett.Server/Program.cs`): we pass `--NoUpdates` ("Disable automatic updates"), `--NoRestart` ("Don't restart after update"), `--ListenPrivate` (`-z`, "Only allow local access") and `--DataFolder <data>`. There is no bind-address flag: the address comes from `ServerConfig.json`'s `AllowExternal` (`ServerConfig.GetListenAddresses`: `http://*:{Port}/` when true, else `http://{LocalBindAddress}:{Port}/`), which `--ListenPrivate` sets to false (`src/Jackett.Server/Initialisation.cs`, `ProcessConsoleOverrides`). We do not pass `--Port`: on Windows, changing the port or the listening mode from the command line needs administrator rights and otherwise exits (`Initialisation.cs`); the port goes into `ServerConfig.json` instead, and since that file already says `AllowExternal: false`, `--ListenPrivate` changes nothing and asks for no rights.

`ServerConfig.json` (`src/Jackett.Common/Models/Config/ServerConfig.cs`; `<DataFolder>/ServerConfig.json` per `src/Jackett.Common/Services/ConfigurationService.cs`; Newtonsoft, PascalCase names, enums as numbers): we set `Port`, `LocalBindAddress` (`127.0.0.1`), `AllowExternal` (false; Jackett's default is true on Linux and macOS), `AllowCORS` (false), `APIKey`, `AdminPassword`, `InstanceId` (when missing), `UpdateDisabled` (true), `UpdatePrerelease` (false) and the five proxy fields (`ProxyType` per `ProxyType.cs`: `Disabled = -1, Http = 0, Socks4 = 1, Socks5 = 2`). Every other field is kept as found. `ConfigurationService.BuildServerConfig` keeps a file that parses and has an `APIKey`. `AdminPassword` is the lowercase hex SHA-512 of `password + APIKey` in UTF-16LE (`src/Jackett.Server/Services/SecurityService.cs`, `HashPassword`).

HTTP (`src/Jackett.Server/Startup.cs`: cookie authentication named `Jackett`, a global authorize filter; the admin API takes no API key):

| Call | Source | Use |
|---|---|---|
| `GET /health` → `{"status":"OK"}` (anonymous) | `Controllers/HealthcheckController.cs` | health check |
| `POST /UI/Dashboard`, form field `password` → `Set-Cookie: Jackett=...` | `Controllers/UIController.cs` | sign-in |
| `GET /api/v2.0/indexers?configured=false` | `Controllers/IndexerApiController.cs`, DTO `src/Jackett.Common/Models/DTO/Indexer.cs` (`id`, `name`, `type`, `language`, `configured`, `last_error`) | the site list |
| `GET`, then `POST /api/v2.0/indexers/{id}/Config` (the items posted back; 204) | `IndexerApiController.cs`, `src/Jackett.Common/Models/DTO/Config.cs` | configure a site with its defaults |
| `DELETE /api/v2.0/indexers/{id}` | `IndexerApiController.cs` | unconfigure |
| `POST /api/v2.0/server/Shutdown?apikey=` (anonymous with the key) | `Controllers/ServerConfigurationController.cs` | clean stop |
| `GET /api/v2.0/indexers/all/results/torznab/api?apikey=&t=search&q=` (and `t=caps`) | `Controllers/ResultsController.cs` | the search source (D41) |

FlareSolverr: the indexer listing has no flag for it; Cardigann definitions carry `type: info_flaresolverr` (for example `src/Jackett.Common/Definitions/1337x.yml`), which `src/Jackett.Common/Indexers/Definitions/CardigannIndexer.cs` shows as a "FlareSolverr" notice.

Release assets (`azure-pipelines.yml`; every build self-contained, archived with a top `Jackett/` folder):

| System | Asset |
|---|---|
| Windows x64 and ARM64 | `Jackett.Binaries.Windows.zip` (a `win-x86` build; there is no x64 asset) |
| Linux x86-64 / ARM64 (glibc) | `Jackett.Binaries.LinuxAMDx64.tar.gz` / `LinuxARM64` |
| Linux musl builds | `LinuxMuslAMDx64` / `LinuxMuslARM64` |
| macOS Intel / Apple silicon | `Jackett.Binaries.macOS.tar.gz` / `macOSARM64` |

Program names (`src/Jackett.Server/Jackett.Server.csproj`): `JackettConsole.exe` on Windows, `jackett` elsewhere.

## Tests

All against fakes; no third-party code runs. `crates/frontend/src/search/helper/`: the asset choice; the zip and tar.gz readers on archives built in the test with the engine's deflater (hostile names, links, long names, pax, executable bits, truncation); `ServerConfig.json` kept and overridden; `helper.ini` sealed; SHA-512 known answers; a fake server (`tests.rs`) answering as GitHub's release listing and download and as a running Jackett (sign-in with the form-encoded password, the chunked indexer listing, the configure and delete calls, a Torznab search of `all` with and without the key); the process runner against a stand-in child (this test binary, re-run with one test selected, serving `/health` and the shutdown call): start, reuse, idle stop, the clean shutdown, the exit kill, a child that never answers, one that exits. Win32: the site picker (nothing ticked, a FlareSolverr row refusing its tick) and the buttons following the install. GTK: what the picker starts ticked.

## To verify with a real Jackett (the owner)

**Run 2026-11-04 on this Windows PC** (owner's go-ahead) through the code path the GUI uses, by the ignored test `crates/frontend/tests/real_jackett.rs` (`PATORRENT_REAL_JACKETT=<empty folder> cargo test -p patorrent-frontend --test real_jackett -- --ignored --nocapture`, a scratch folder standing in for the profile): Jackett v0.24.3009, `Jackett.Binaries.Windows.zip` (57,100,482 bytes), downloaded and checked in 8-9 s, unpacked to `search-helper\app\Jackett\JackettConsole.exe`; the first start answered `/health` after 2.4-2.9 s, no console window, no UAC prompt; `ServerConfig.json` kept our port, `AllowExternal: false` and our key; the sign-in with the generated password worked; 95 public sites listed, 25 marked as needing FlareSolverr (1337x among them); ticking The Pirate Bay and LimeTorrents configured them (unticking removed them, ticking again restored them); a search for "ubuntu" through the `all` endpoint returned 100 results in 0.77 s (names, sizes, seeders, info-hashes, magnets and `/dl/` links); the stop left nothing answering on the port; a second start reused the port and the configured sites; `shutdown()` killed it. A first try with YTS and Nyaa (a movie and an anime site) returned nothing for "ubuntu", which is those sites, not the helper. Items 1, 2, 4, 5, 6 and 7's search are done, and 8's exit; still to do: 3, 7's add through a `/dl/` link after an idle stop, 8's ten idle minutes, 9, 10, 11, and the GUI's own buttons and dialogs (the run drove the frontend, not the windows).

A helper left running when PaTorrent dies without a normal exit was seen in that run (a test panic orphaned `JackettConsole.exe`, which also held an inherited pipe open): since 1.1.63 every helper on Windows is put into a job object with kill-on-job-close (`process::job`), so it dies with the process however the process ends; elsewhere the normal exit still kills it and a crash leaves it until the next start.

1. Windows, a non-admin account: Get the Search Helper; the confirm box's version and size match the GitHub release page; the download's progress and the SHA-256 check pass; `search-helper\app\Jackett\JackettConsole.exe` exists.
2. The first search: no console window appears, no UAC prompt (watch for `--ListenPrivate`/`--DataFolder` asking for rights, and for Jackett's `%APPDATA%\Jackett` migration relaunching itself elevated when an old Jackett folder exists), the answer comes within the start timeout; `netstat -ano` shows it listening on `127.0.0.1:<port>` only.
3. Linux and macOS: the same, and `ss -ltnp` shows `127.0.0.1` only (Jackett's own default there is every interface); macOS Gatekeeper's reaction to an unsigned `jackett` started by us.
4. `data/ServerConfig.json` after the first start still has our port, `AllowExternal: false`, our `APIKey`; Jackett did not regenerate the key.
5. The sign-in works with the generated password (the hash format), and the browser page accepts it.
6. The site picker lists the public sites with language and type; known Cloudflare sites (1337x) are greyed; ticking two and OK configures them (they show configured on the helper's page); unticking removes them.
7. A search returns results from the ticked sites; adding a result works through the helper's `/dl/` link, also after the helper was stopped for being idle.
8. Ten idle minutes later the process is gone; exiting PaTorrent while it runs leaves no `JackettConsole`/`jackett` process.
9. With a proxy set in Preferences, the helper's settings page shows it, and its searches go through it.
10. Update when a newer tag exists keeps the source working (same key); Remove deletes the folder and the source.
11. A second profile gets a helper of its own on another port.
