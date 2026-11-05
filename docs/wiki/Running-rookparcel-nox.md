# Running rookparcel-nox

`rookparcel-nox` is Rookparcel without a window: the same engine, run from a terminal or as a service, and controlled through its [Web UI](Web-UI), from a desktop Rookparcel with [Remote control](Remote-control), or by Sonarr, Radarr and Prowlarr ([Sonarr, Radarr and Prowlarr](Sonarr-Radarr-and-Prowlarr)). This page covers its commands and options, running it as a Windows service, under systemd and on FreeBSD, where it keeps its files, and how to stop it.

## Where to get it

`rookparcel-nox` comes with every release ([Installing Rookparcel](Installing-Rookparcel)):

| Platform | File |
|---|---|
| Windows | `rookparcel-nox.exe`, beside `rookparcel.exe` in the zip and in the installed folder |
| Linux | in the `rookparcel-<version>-linux-<arch>.tar.gz` archive (with the systemd units), or on its own as `rookparcel-nox-<version>-<arch>.AppImage` |
| macOS | `rookparcel-nox-<version>-macos-<arch>.tar.gz` |

`rookparcel-nox --help` lists every command and option, `rookparcel-nox --version` prints the version, and `rookparcel-nox --license` prints the licence and the third-party notices.

## A first run

1. Open a terminal (on Windows a Command Prompt) in the folder with `rookparcel-nox`.
2. Start it with a download folder and the Web UI:

   ```sh
   rookparcel-nox run --save ~/Downloads --webui 8080
   ```

3. It prints a few lines starting with `*`. One of them is the Web UI's address, and, the first time, a line like this:

   ```text
   * Web UI: no password set; temporary password for this run: ... (set one with --webui-pass)
   ```

4. Open `http://127.0.0.1:8080/` in a browser and log in as `admin` with that temporary password.
5. In the page, open **Preferences > Web UI** and set your own password. From then on it is stored (as a hash), and nox asks for it at every start.

To stop it, type `q` and press <kbd>Enter</kbd>, or press <kbd>Ctrl</kbd>+<kbd>C</kbd>. It saves the state of every torrent on the way out, so the next `run` resumes them without checking the files again.

`--webui 8080` listens on this computer only (`127.0.0.1`). To reach the page from other computers or a phone, give an address too, for example `--webui 0.0.0.0:8080`, and read [Web UI security](Web-UI#security) first.

## Commands

| Command | What it does |
|---|---|
| `rookparcel-nox run [OPTIONS]` | Resumes every torrent saved in the state folder and keeps running until it is stopped. This is the daemon. |
| `rookparcel-nox add <file.torrent>... [--save DIR] [OPTIONS]` | Adds the torrents (files or `magnet:` links), downloads them and exits when they are finished. With `--seed` it keeps seeding instead. |
| `rookparcel-nox create <file or folder> [-o OUT] [CREATE OPTIONS]` | Makes a `.torrent` file ([below](#making-a-torrent-file)). |
| `rookparcel-nox service ...` | The Windows service (Windows only, [below](#the-windows-service)). |
| `rookparcel-nox demo [--torrents N] [--active FRACTION] [--seconds N]` | Loads simulated torrents and prints totals; for testing only. |

`run` also takes `.torrent` files and magnet links after the options; they are added at the start.

## Options of run and add

### Folders and state

| Option | Meaning |
|---|---|
| `--save DIR` | The download folder. Default: the current folder. |
| `--state-dir DIR` | Where the torrent list and resume data are kept ([files](#where-it-keeps-its-files)); also the settings folder unless `--config-dir` is given. `--state DIR` is the same. |
| `--config-dir DIR` | Where the Web UI's login and settings (`webui.json`) are kept. |
| `--profile NAME` | Runs a separate nox with its own folders, login and listen port ([profiles](#profiles)). |
| `--no-state` | Loads and saves nothing. |
| `--move-on-complete DIR` | Moves each torrent that finishes downloading into DIR. |

### Seeding and stopping

| Option | Meaning |
|---|---|
| `--seed` | `add`: keep seeding after the download instead of exiting. |
| `--super-seed` | Initial seeding for torrents that are complete; implies `--seed`. |
| `--seconds N` | Stop after N seconds. |
| `--exit-when-done downloads` or `all` | Exit, with the state saved, once nothing is downloading any more (`downloads`), or nothing is downloading or seeding (`all`). The same as **Options > Auto Shutdown** in the desktop program ([Auto shutdown](Auto-shutdown)). |

### Network

| Option | Meaning |
|---|---|
| `--port N` | The listening port for peers. Default 6881. |
| `--listen LIST` | Listening interfaces instead of `--port`: `IP:PORT`, `[IPv6]:PORT` or `DEVICE:PORT`, separated by commas. |
| `--dl-limit KIB`, `--ul-limit KIB` | Download and upload limits in KiB/s; 0 is unlimited. |
| `--ip-filter FILE` | Blocks the address ranges in FILE (`ipfilter.dat` or P2P format). |
| `--proxy TYPE://[USER:PASS@]HOST:PORT` | Connects through a proxy; TYPE is `socks4`, `socks5` or `http`. Nothing incoming is accepted while peers go through it. |
| `--no-proxy-peers` | Connect to peers and web seeds directly; only trackers use the proxy. |
| `--no-proxy-trackers` | Contact trackers directly. |
| `--no-proxy-hostnames` | Resolve host names here instead of at the proxy. |
| `--socks5-udp-local-ep` | For a SOCKS5 proxy with no NAT in between; off by default. |
| `--network RULE` | Use only this network: torrent traffic is bound to the matching adapter. RULE is `vpn` (any VPN adapter), `adapter:ID` (a device name; on Windows the adapter's GUID), `range:CIDR` (such as `10.2.0.0/16`), or `off`. |
| `--no-pause-when-missing` | While the `--network` rule matches nothing, let torrents run unbound instead of waiting for the network. |

*`--network` is new in 1.1.58; not in a published release yet.* The proxy and network options are kept in the Web UI's settings like the matching boxes of its **Preferences > Connection** page. Once you change something in the Web UI, the stored value wins over the command line, so change them in the page from then on. A proxy password on the command line can be seen by other users of the computer in the process list; set it in **Preferences > Connection** instead. More on these settings: [Connection, ports and firewall](Connection-ports-and-firewall).

### Web UI

| Option | Meaning |
|---|---|
| `--webui [ADDR:]PORT` | Serves the Web UI and the qBittorrent-compatible API. Without ADDR it listens on `127.0.0.1` only. |
| `--webui-user NAME` | The login name. Default `admin`. |
| `--webui-pass PASS` | The password, stored as a hash. Without one (and none stored), a temporary password is printed at the start. |
| `--webui-save-root DIR` | Can be given several times. The Web UI may then save, move and read only in these folders and below them. Without it, anyone with the login can use any folder the nox account can reach. |
| `--webui-https` | Serves HTTPS with a self-signed certificate made on first use. |
| `--webui-cert FILE [--webui-key FILE]` | Serves HTTPS with your own certificate and key (PEM; the key not encrypted). |

A password on the command line is visible to other local users in the process list. Prefer setting it in the page's **Preferences > Web UI**. Details on HTTPS and the other security settings: [Web UI](Web-UI#https).

### Other

| Option | Meaning |
|---|---|
| `--import qbittorrent` or `utorrent` | `run` only: adds the torrents of qBittorrent or µTorrent at the start, with their data where it is (checked first), statistics, priorities, trackers and labels. The other client must not be running; its files are only read. See [Importing from uTorrent and qBittorrent](Importing-from-uTorrent-and-qBittorrent). |
| `--import-dir DIR` | The other client's folder when it is not in the usual place. |
| `--verbose` (`-v`) | Prints tracker and peer events. |
| `--no-update-check` | `run` only: does not ask the project website for newer releases. |
| `--daemon` | Unix: detaches into the background ([other init systems](#other-ways-to-run-it-in-the-background)). |
| `--pid-file FILE` | Writes the process id to FILE and locks it; a second nox with the same FILE refuses to start. |
| `--log FILE` | Appends everything nox prints to FILE. |

### The update check

`run` asks the project website which releases exist, 10 seconds after the start and every 12 hours. Nothing about your installation is sent and nothing is downloaded. A newer version is one line in the output and a link in the Web UI's status bar. Turn it off with `--no-update-check`, or in the Web UI under **Preferences > General > Privacy**, "Check for updates automatically". "Update to beta versions" there offers pre-releases too. See [Updating Rookparcel](Updating-Rookparcel).

## Making a torrent file

```sh
rookparcel-nox create ~/Videos/holiday -t udp://tracker.example:1337/announce --magnet
```

| Option | Meaning |
|---|---|
| `-o`, `--output FILE` | Where to write the torrent. Default: `<name>.torrent`. |
| `-t`, `--tracker URL` | A tracker; repeat for more tiers, or put several in one tier with commas. |
| `-w`, `--web-seed URL` | A web seed; repeatable. |
| `--node HOST:PORT` | A DHT bootstrap node; repeatable. |
| `--private` | Sets the private flag (no DHT, peer exchange or local discovery). |
| `--piece-size KIB` | Piece size in KiB. Default: automatic. |
| `--comment TEXT`, `--source TEXT`, `--creator TEXT` | The comment, the source field (used by private trackers), and "created by". |
| `--no-date` | Leaves out the creation date. |
| `--hybrid`, `--v1`, `--v2` | Hybrid (the default), v1 only, or v2 only. |
| `--pad` | Aligns files to pieces with pad files (always on for hybrid and v2). |
| `--mtime`, `--symlinks`, `--no-attributes`, `--skip-hidden` | Store modification times; store links as links (Unix); leave out hidden and executable attributes; leave out names starting with a dot. |
| `--magnet` | Also prints the magnet link. |

The desktop program's window for this is described in [Creating torrents](Creating-torrents).

## Where it keeps its files

| | Linux and BSD | Windows | macOS |
|---|---|---|---|
| Torrent list and resume data (`resume.db`, `metadata.pack`, `dht_state.dat`) | `~/.local/share/rookparcel/nox` | `%APPDATA%\Rookparcel\nox` | `~/Library/Application Support/Rookparcel/nox` |
| Settings (`webui.json`, `ipfilter.dat`, the HTTPS certificate) | `~/.config/rookparcel/nox` | the same folder | the same folder |
| The Windows service | | `%ProgramData%\Rookparcel\nox` | |

- On Linux `$XDG_DATA_HOME` and `$XDG_CONFIG_HOME` are used when they are set.
- `webui.json` is rewritten by the running nox whenever something changes in the Web UI. Edit it only while nox is stopped.
- Only one nox can use a folder at a time. A second one stops with "... is in use by another rookparcel-nox".
- **Portable mode:** with a `rookparcel.ini` beside the program, both go to a `nox` folder beside it, and download folders inside it are stored relative to it, so the whole folder can be moved. *New in 1.1.49; not in a published release yet.* See [Where Rookparcel keeps its files](Where-Rookparcel-keeps-its-files).

## Profiles

`--profile NAME` runs a separate nox with its own torrents, settings, Web UI login and service. A name has 1 to 32 characters: letters a-z, digits and hyphens, starting with a letter. Unless `--port` or `--listen` says otherwise, each profile gets a listening port of its own, worked out from its name and the same at every start.

| | default profile | `--profile work` |
|---|---|---|
| Linux and BSD | `~/.local/share/rookparcel/nox` | `~/.local/share/rookparcel/profiles/work/nox` (and the same under `~/.config`) |
| Windows | `%APPDATA%\Rookparcel\nox` | `%APPDATA%\Rookparcel\profiles\work\nox` |
| macOS | `~/Library/Application Support/Rookparcel/nox` | `.../Rookparcel/profiles/work/nox` |
| Windows service | `RookparcelNox` | `RookparcelNox_work` |
| systemd | `rookparcel-nox.service` | `rookparcel-nox@work.service` |

Each profile's Web UI needs a port of its own, for example `--webui 127.0.0.1:8081`. A port already in use stops nox with "port N is in use, perhaps by another Rookparcel profile". *Profiles are new in 1.1.44; not in a published release yet.* See [Profiles](Profiles).

## The Windows service

`rookparcel-nox service install` registers nox as a Windows service named **RookparcelNox** ("Rookparcel nox"). It starts with Windows and runs without anyone logged in.

This is not the same as **Preferences > Web UI** in `rookparcel.exe`: that serves the Web UI from the window, over the window's own torrents, only while the window runs ([Web UI](Web-UI)).

`install`, `uninstall`, `start` and `stop` need an administrator Command Prompt.

1. Put `rookparcel-nox.exe` where the service account can read it, such as `C:\Program Files\Rookparcel\`. A copy under your user folder (Downloads, Documents) cannot be opened by the service account; `install` warns about that.
2. Install it with the `run` options it should start with:

   ```bat
   "C:\Program Files\Rookparcel\rookparcel-nox.exe" service install --webui 127.0.0.1:8080
   ```

3. Start it with `rookparcel-nox service start`, or add `--now` to the install. It starts with Windows from then on.
4. Read the temporary Web UI password from `%ProgramData%\Rookparcel\nox\nox.log` (from an administrator prompt), open the page and set your own password. Or give `--webui-user` and `--webui-pass` to `install`: the password is stored as a hash in the service's `webui.json`, never in the service's command line.

Options of `install` itself:

| Option | Meaning |
|---|---|
| `--account A` | `LocalService` (the default), `NetworkService` or `LocalSystem`. |
| `--start TYPE` | `delayed` (the default: automatic, after the boot has settled), `auto` or `manual`. |
| `--replace` | Stops and removes an installed RookparcelNox first; its state stays. |
| `--now` | Starts the service once installed. |
| `--profile NAME` | Installs the profile's own service, `RookparcelNox_NAME`. |

What it sets up:

- **State and settings** in `%ProgramData%\Rookparcel\nox` (or `--state-dir`), a folder only SYSTEM, administrators and the service account can open, since it holds the Web UI login.
- **Downloads** in its `Downloads` subfolder unless you give `--save`.
- **Log** in `nox.log` there unless you give `--log`.
- **Restarts** after a failure: after 5, 30 and 60 seconds.
- At system shutdown it is given time to save the torrents' state.

Folders the service writes to that `install` did not make (a `--save` folder, a `--move-on-complete` folder) need a permission for the account, from an administrator prompt:

```bat
icacls D:\Torrents /grant "NT AUTHORITY\LocalService:(OI)(CI)M"
```

Incoming peer connections need a Windows Firewall rule for the program; a service cannot ask for one:

```bat
netsh advfirewall firewall add rule name="Rookparcel nox" dir=in action=allow program="C:\Program Files\Rookparcel\rookparcel-nox.exe"
```

The other service commands:

| Command | What it does |
|---|---|
| `rookparcel-nox service start`, `service stop` | Starts or stops it and waits until it has. `sc start RookparcelNox`, `sc stop RookparcelNox` and the Services console do the same. |
| `rookparcel-nox service status` | Prints the service's state and command line (no administrator needed). `--all` lists every profile's service. |
| `rookparcel-nox service uninstall` | Stops and removes it. The state folder and log stay. `--all` removes every profile's service that runs this program. |
| `rookparcel-nox service debug [OPTIONS]` | Runs the service's work in this console, to try the options before installing. |
| `sc control RookparcelNox paramchange` | Reloads the IP filter and reopens the log. |

To change the options, install again with `--replace`. To delete the torrents' state and the downloads too, delete `%ProgramData%\Rookparcel` after uninstalling.

`start`, `stop`, `status` and `uninstall` take `--profile NAME` for a profile's service. *Services per profile are new in 1.1.47; not in a published release yet.*

**Without the service:** `rookparcel-nox run` in a Command Prompt works too and stops cleanly on <kbd>Ctrl</kbd>+<kbd>C</kbd> or when the window is closed. The Task Scheduler is another way: a task "At startup" that runs `rookparcel-nox.exe run --save D:\Torrents --webui 8080 --log C:\Rookparcel\nox.log`. `--daemon` does not work on Windows.

## systemd (Linux)

The Linux archive has ready-made units, also in the repository's [`contrib/systemd`](https://github.com/Balls0fSteel/Rookparcel/tree/main/contrib/systemd) folder.

**As a system service**, run as a `rookparcel` account, with its state in `/var/lib/rookparcel-nox`:

```sh
sudo install -m 755 rookparcel-nox /usr/local/bin/
sudo install -D -m 644 rookparcel-nox.sysusers /etc/sysusers.d/rookparcel-nox.conf
sudo systemd-sysusers
sudo install -m 644 rookparcel-nox.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now rookparcel-nox
journalctl -u rookparcel-nox
```

The journal shows the temporary Web UI password on the first start. The unit serves the Web UI on `127.0.0.1:8080` and downloads into `/var/lib/rookparcel-nox/downloads`. To change that, and to let it write to another folder:

```ini
# sudo systemctl edit rookparcel-nox
[Service]
ExecStart=
ExecStart=/usr/local/bin/rookparcel-nox run --save /srv/torrents --webui 0.0.0.0:8080
ReadWritePaths=/srv/torrents
```

The service is locked down (it cannot write outside its own folder without `ReadWritePaths=`), so a download folder elsewhere always needs that line.

**As your own user service**, with your files in the folders listed [above](#where-it-keeps-its-files):

```sh
install -m 755 rookparcel-nox ~/.local/bin/
install -D -m 644 rookparcel-nox.service ~/.config/systemd/user/rookparcel-nox.service
systemctl --user daemon-reload
systemctl --user enable --now rookparcel-nox
loginctl enable-linger "$USER"
```

`loginctl enable-linger` keeps it running after you log out. This unit downloads into `~/Downloads`.

**Profiles** use the template units `rookparcel-nox@.service`: `systemctl enable --now rookparcel-nox@work` starts the profile `work`. They have no Web UI by default, because two cannot share a port; give each one its own in `systemctl edit rookparcel-nox@work`.

`systemctl reload rookparcel-nox` reloads the IP filter. Output goes to the journal.

## FreeBSD

The rc.d script is in the repository's [`contrib/freebsd`](https://github.com/Balls0fSteel/Rookparcel/tree/main/contrib/freebsd) folder:

```sh
pw useradd rookparcel -d /var/db/rookparcel-nox -s /usr/sbin/nologin -c "Rookparcel daemon"
install -m 755 rookparcel-nox /usr/local/bin/
install -m 555 rookparcel_nox /usr/local/etc/rc.d/
sysrc rookparcel_nox_enable=YES
sysrc rookparcel_nox_args="--webui 0.0.0.0:8080"
service rookparcel_nox start
```

It runs as `rookparcel`, with its state in `/var/db/rookparcel-nox` and the log in `/var/log/rookparcel-nox.log`, where the first start writes the temporary Web UI password. Settings in `/etc/rc.conf`: `rookparcel_nox_args` (more `run` options; default `--webui 127.0.0.1:8080`), `rookparcel_nox_user`, `rookparcel_nox_dir`, `rookparcel_nox_save`, `rookparcel_nox_log`. Use `rookparcel_nox_args`, not `rookparcel_nox_flags`. `service rookparcel_nox reload` reloads the IP filter and reopens the log.

Profiles: `sysrc rookparcel_nox_profiles="work"` and `sysrc rookparcel_nox_work_args="--webui 127.0.0.1:8081"`; `service rookparcel_nox start work` starts one, `service rookparcel_nox start` all of them.

## Other ways to run it in the background

On other Unix systems:

- `--daemon` detaches into the background and returns once nox is up (status 0) or failed to start (status 1). Without `--log`, the output after that point is lost, so read the temporary password before, or use `--log`.
- `--pid-file FILE` writes the process id; a second nox with the same file refuses to start.
- `--log FILE` appends all output to FILE, readable only by its owner since it can hold the temporary password. A `SIGHUP` reopens it, so log rotation works.

## Stopping and reloading

| What | Effect |
|---|---|
| `q` and <kbd>Enter</kbd>, <kbd>Ctrl</kbd>+<kbd>C</kbd>, `SIGTERM` | A clean stop: torrent state saved, the router's port mappings removed. |
| A second <kbd>Ctrl</kbd>+<kbd>C</kbd> or `SIGTERM` during that stop | Quits at once, without saving. |
| `SIGHUP` (Windows service: `sc control RookparcelNox paramchange`) | Reloads the IP filter's list and reopens the `--log` file. `webui.json` is not re-read. |
| Windows: closing the console window, logging off, shutting down | A clean stop. |

## See also

- [Web UI](Web-UI): the page nox serves.
- [Remote control](Remote-control): driving nox from the desktop program's window.
- [Sonarr, Radarr and Prowlarr](Sonarr-Radarr-and-Prowlarr): nox as their download client.
- The full technical description: [DAEMON.md](https://github.com/Balls0fSteel/Rookparcel/blob/main/docs/DAEMON.md).
