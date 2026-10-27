# Running patorrent-nox as a service

`patorrent-nox run` is the daemon: it resumes every saved torrent and keeps running until it is
told to stop. With `--webui [ADDR:]PORT` it serves the Web UI ([WEBUI.md](WEBUI.md)) and the
qBittorrent-compatible WebAPI, which is how a service is normally driven:

```sh
patorrent-nox run --save ~/Downloads --webui 127.0.0.1:8080
patorrent-nox run --save /srv/torrents --webui 0.0.0.0:8080 --webui-pass 'a long password'
```

Without `--webui-pass` and with no password stored yet, the first start prints a temporary
password; log in with it and set one in Preferences > Web UI. A password given on the command
line is visible to other local users in the process list, so prefer setting it in the page.
For a LAN host name, add it to Preferences > Web UI > domain list (`web_ui_domain_list`).

On a network, serve the page over HTTPS so the password and the session cookie are not sent in
the clear: `--webui-https` makes a self-signed certificate on first use and keeps it in the
config directory (`webui-cert.pem`, `webui-key.pem`), or `--webui-cert FILE --webui-key FILE`
uses yours (PEM; the key unencrypted). The browser warns about a self-signed certificate until
you accept it; compare the fingerprint it shows with the `certificate SHA-256` line nox prints.

```sh
patorrent-nox run --save /srv/torrents --webui 0.0.0.0:8443 --webui-https
```

Ready-made service files are in [`contrib/`](../contrib): systemd units (system and user) and a
FreeBSD rc.d script. On Windows `patorrent-nox service install` registers a Windows service
([below](#the-windows-service)). The command-line options are the configuration; the rest
(limits, folders, the Web UI's settings) is set in the Web UI and kept in `webui.json`.

## Through a proxy

`--proxy TYPE://[USER:PASS@]HOST:PORT` (TYPE `socks4`, `socks5` or `http`; credentials pick the
authenticating types, SOCKS4 takes a user id only) sends peers, web seeds and trackers through
the proxy, and with SOCKS5 uTP and the DHT too (UDP ASSOCIATE). Nothing incoming is accepted
while peers go through it. Four switches set the rest of the Web UI's Connection page proxy
box and its Advanced rows, each with a `--no-` form; the last one given wins:

| Switch | Setting | Default |
|---|---|---|
| `--[no-]proxy-peers` | `proxy_peer_connections`: peer and web seed connections through the proxy (off: direct, incoming connections, LSD and port mapping work again) | on |
| `--[no-]proxy-trackers` | `proxy_tracker_connections`: tracker announces and scrapes through the proxy | on |
| `--[no-]proxy-hostnames` | `proxy_hostnames`: the proxy resolves host names (SOCKS5 sends the name, SOCKS4 uses SOCKS4a) | on |
| `--[no-]socks5-udp-local-ep` | `socks5_udp_send_local_ep`: UDP ASSOCIATE names our UDP address and port (wrong behind a NAT) | off |

```sh
patorrent-nox run --webui 8080 --proxy socks5://user:pass@proxy.lan:1080 --no-proxy-trackers
```

They are applied through the same settings table as the Web UI's preferences, so they are
stored in `webui.json` with the rest at the next change made in the Web UI, and from then on
the stored value wins over the command line (change it in the Web UI). A password on the
command line is visible to other local users in the process list; set it in Preferences >
Connection instead. In `webui.json` (and the GUI's `patorrent.ini`) the proxy password is
sealed with DPAPI on Windows (`dpapi:...`, readable only by the same Windows account) and plain
elsewhere.

## Files

| What | Linux / BSD | Windows | macOS |
|---|---|---|---|
| state: `resume.db`, `metadata.pack`, `dht_state.dat` | `$XDG_DATA_HOME/patorrent/nox`, else `~/.local/share/patorrent/nox` | `%APPDATA%\PaTorrent\nox` | `~/Library/Application Support/PaTorrent/nox` |
| config: `webui.json`, the default `ipfilter.dat`, the generated HTTPS certificate | `$XDG_CONFIG_HOME/patorrent/nox`, else `~/.config/patorrent/nox` | the same folder as the state | the same folder as the state |
| the Windows service | | `%ProgramData%\PaTorrent\nox` for both, unless `service install` gets `--state-dir` | |

- The torrent library is data you would miss, so it lives under `XDG_DATA_HOME`, not
  `XDG_STATE_HOME` (which the XDG spec keeps for history and layout that may be lost). Nothing
  is cached. Empty or relative XDG variables are ignored, as the spec says.
- Under systemd, `StateDirectory=` and `ConfigurationDirectory=` (`$STATE_DIRECTORY`,
  `$CONFIGURATION_DIRECTORY`) come first; the system unit uses `/var/lib/patorrent-nox`.
- `--state-dir DIR` (or the older `--state DIR`) puts the state in DIR, and the config too unless
  `--config-dir DIR` is also given; `--no-state` keeps nothing.
- Windows portable mode: with a `patorrent.ini` beside the exe (the GUI's rule, D7), both go to
  `nox` beside the exe.
- Until 2026-10-19 Unix kept `webui.json` in the state directory. On the first start with the
  default directories it is moved to the config directory (and `ipfilter.dat` copied), with a
  `* moved ...` line in the output.
- `webui.json` belongs to the running daemon, which rewrites it on every change made in the Web
  UI: edit it only while the daemon is stopped.

## Stopping and reloading

| Signal / event | Effect |
|---|---|
| SIGTERM, SIGINT (Ctrl+C), `q` + Enter on the console | clean shutdown: resume data saved, the router's port mappings deleted, the pid file removed; the process exits 0 |
| a second SIGTERM / SIGINT / Ctrl+C during that shutdown | exits at once, without saving |
| SIGHUP | reloads the IP filter's block list (the Web UI's choice when it runs, else `--ip-filter`; a list that fails to load leaves the old rules in force) and reopens the `--log` file |
| SIGPIPE | ignored |
| Windows: Ctrl+C, Ctrl+Break, closing the console window, logoff, shutdown | clean shutdown; for the last three Windows ends the process when the handler returns, so it waits for the state to be saved |
| Windows service: stop, shutdown / PARAMCHANGE | clean shutdown / reload, as above ([the Windows service](#the-windows-service)); logoff is ignored |

`webui.json` is not re-read on SIGHUP (see above).

## systemd

`contrib/systemd/system/patorrent-nox.service` runs the daemon as the `patorrent` account
(`contrib/systemd/system/patorrent-nox.sysusers` creates it) with its state in
`/var/lib/patorrent-nox` and the usual sandboxing (`ProtectSystem=strict`, `ProtectHome=yes`,
`NoNewPrivileges=yes`, a system-call filter, no capabilities). Installation steps are at the top
of the file. Downloads outside `/var/lib/patorrent-nox` need `ReadWritePaths=` in a drop-in:

```ini
# sudo systemctl edit patorrent-nox
[Service]
ExecStart=
ExecStart=/usr/local/bin/patorrent-nox run --save /srv/torrents --webui 0.0.0.0:8080
ReadWritePaths=/srv/torrents
```

`contrib/systemd/user/patorrent-nox.service` is the same for one user (`systemctl --user`),
with the XDG directories above; `loginctl enable-linger $USER` keeps it running after logout.

Both are `Type=notify`: the daemon sends `READY=1` (with a `STATUS=` line naming the Web UI
address) once the session runs and the Web UI listens, `RELOADING=1` / `READY=1` around a
SIGHUP, and `STOPPING=1` when it shuts down, over `$NOTIFY_SOCKET` (the protocol of
`sd_notify(3)`, implemented without libsystemd). `systemctl reload` sends SIGHUP; with systemd
253 or newer `Type=notify-reload` does the same without `ExecReload=`. Output goes to the
journal; no `--log` is needed.

## FreeBSD

`contrib/freebsd/patorrent_nox` is an rc.d script: it starts `patorrent-nox run --daemon` as the
`patorrent` account with its state in `/var/db/patorrent-nox`, a pid file in
`/var/run/patorrent-nox/` and the log in `/var/log/patorrent-nox.log`. `service patorrent_nox
reload` sends SIGHUP. Settings go in `/etc/rc.conf` (`sysrc`): `patorrent_nox_enable`,
`patorrent_nox_args` (more `run` options, default `--webui 127.0.0.1:8080`),
`patorrent_nox_user`, `patorrent_nox_dir`, `patorrent_nox_save`, `patorrent_nox_log`.

## Other init systems: `--daemon`, `--pid-file`, `--log`

- `--daemon` (Unix) detaches the classic way: fork, `setsid`, fork again, `chdir /`, stdin from
  `/dev/null` (relative paths on the command line are made absolute first). The command returns
  once the daemon is up, with status 0, or 1 when it failed to start. Start-up errors reach the
  terminal either way. Without `--log` all output reaches the terminal until the daemon is up
  (so the temporary Web UI password is seen) and goes nowhere afterwards; with `--log` normal
  output goes to the log from the start (the temporary password is there) and errors from the
  moment the daemon is up.
- `--pid-file FILE` writes the process id and keeps FILE locked (`flock`; on Windows a share
  mode that refuses other writers) while the daemon runs; a second instance with the same FILE
  stops with "already running (pid N)". The file is removed at a clean exit.
- `--log FILE` appends everything the daemon prints to FILE (mode 600: it can hold the temporary
  password). SIGHUP reopens it, so logrotate works with a `postrotate` that sends SIGHUP:

  ```
  /var/log/patorrent-nox.log {
      weekly
      rotate 4
      compress
      delaycompress
      missingok
      postrotate
          kill -HUP "$(cat /run/patorrent-nox/patorrent-nox.pid)" 2>/dev/null || true
      endscript
  }
  ```

## Windows

### The Windows service

`patorrent-nox service install [OPTIONS]` registers the service **`PaTorrentNox`** (display
name "PaTorrent nox") with the Service Control Manager. It runs `patorrent-nox run` with the
given options; `install`, `uninstall`, `start` and `stop` need an administrator prompt.

1. Put the exe where the service account can read it, e.g. `C:\Program Files\PaTorrent\`. A copy
   under your user profile (Downloads, Documents, a build folder) cannot be opened by
   LocalService; `install` warns about that.
2. Install it from an elevated prompt, with the `run` options it should start with:

   ```bat
   "C:\Program Files\PaTorrent\patorrent-nox.exe" service install --webui 127.0.0.1:8080
   ```

   Relative paths are made absolute (a service starts in `C:\Windows\System32`). Torrent files on
   the command line, `--daemon` and `--no-state` are refused. Install options of its own:
   `--account LocalService|NetworkService|LocalSystem` (default LocalService) and
   `--start delayed|auto|manual` (default delayed: automatic, after the boot has settled).
3. `patorrent-nox service start` (or `sc start PaTorrentNox`, or the Services console) starts it
   and waits until it runs; it starts with Windows from then on.

What `install` sets up:

| | |
|---|---|
| image path | `"C:\...\patorrent-nox.exe" service run OPTIONS`, the exe quoted; `sc qc PaTorrentNox` shows it, so install again to change the options |
| account | `NT AUTHORITY\LocalService`: few rights, no password, anonymous on the network |
| state and config | `%ProgramData%\PaTorrent\nox` unless `--state-dir` is given. Not the account's `%APPDATA%`, which is hidden under `C:\Windows\ServiceProfiles\LocalService`. A folder that does not exist yet is created with a protected ACL: full control for SYSTEM, Administrators and the service account, nothing inherited from `%ProgramData%` (where every user may read), because `webui.json` and the log hold the Web UI login. A folder that exists already gets the same ACL, but only if it, its parents and everything in it belong to SYSTEM, Administrators, a service account or you, with no junctions or links inside; otherwise the install stops, since another user may have made it first |
| downloads | `Downloads` in the state folder unless `--save` is given |
| log | `--log`, else `nox.log` in the state folder (a service has no console). It shows the temporary Web UI password when none is stored; read it from an elevated prompt |
| recovery | restart after 5 s, 30 s and 60 s; the count starts again after a day without failures. Also when it stops by itself with an error code, not only on a crash |
| shutdown | the service asks for 60 s at system shutdown (PRESHUTDOWN) to save resume data and delete port mappings |

Folders the account has to write that `install` did not create (an existing state folder, a
`--save` folder, `--move-on-complete`) need a grant, from an elevated prompt:

```bat
icacls D:\Torrents /grant "NT AUTHORITY\LocalService:(OI)(CI)M"
```

Incoming peer connections need a Windows Firewall rule for the exe (the first `run` in a
console normally asks; a service cannot):

```bat
netsh advfirewall firewall add rule name="PaTorrent nox" dir=in action=allow program="C:\Program Files\PaTorrent\patorrent-nox.exe"
```

`--webui-pass` would become part of the image path, which every local user can read with `sc
qc`: leave it out, log in with the temporary password from the log and set one in Preferences >
Web UI.

Controls:

| Control | Effect |
|---|---|
| `sc stop PaTorrentNox`, `service stop`, the Services console | STOP_PENDING at once, then the same clean shutdown as Ctrl+C; STOPPED with exit code 0 |
| system shutdown (PRESHUTDOWN, SHUTDOWN) | the same |
| `sc control PaTorrentNox paramchange` | the reload SIGHUP does (IP filter, log reopened) |
| a user logging off | ignored |

The service reports START_PENDING until the session runs and the Web UI listens (the point where
systemd gets `READY=1`), then RUNNING. The pending states count a checkpoint every second for at
most two minutes. When it stops by itself, `sc query PaTorrentNox` shows the reason as
SERVICE_EXIT_CODE (with WIN32_EXIT_CODE 1066): 1 it failed (see the log), 2 its command line is
invalid, 3 the log cannot be opened.

`patorrent-nox service uninstall` stops and removes it; the state folder and log stay.
`patorrent-nox service debug [OPTIONS]` runs the service's body in a console, without the SCM,
printing the states it would report (and keeping output on the console unless `--log` is given):
handy to try the options before installing. `service run` is the entry the SCM starts; in a
console it fails with "could not connect to the service controller" (os error 1063).

Step by step, from an elevated prompt (the ones marked "user" work unelevated too):

```bat
mkdir "C:\Program Files\PaTorrent"
copy target\release\patorrent-nox.exe "C:\Program Files\PaTorrent\"
cd /d "C:\Program Files\PaTorrent"
patorrent-nox service install --webui 127.0.0.1:8080
sc qc PaTorrentNox                         & rem user: image path, account, start type
sc qfailure PaTorrentNox                   & rem user: the restart actions
patorrent-nox service start
sc query PaTorrentNox                      & rem user: STATE 4 RUNNING
type "%ProgramData%\PaTorrent\nox\nox.log" & rem the temporary Web UI password
start http://127.0.0.1:8080/
sc control PaTorrentNox paramchange        & rem "* PARAMCHANGE: reloading" in the log
patorrent-nox service stop                 & rem STOPPED (exit code 0)
patorrent-nox service uninstall
rmdir /s "%ProgramData%\PaTorrent"         & rem only to delete the state and downloads too
```

### Without the service

`patorrent-nox run` in a console stops cleanly on Ctrl+C or when the window closes. The Task
Scheduler is an alternative to the service: a task "At startup" that runs `patorrent-nox.exe run
--save D:\Torrents --webui 8080 --log C:\PaTorrent\nox.log` as a chosen account, "whether the
user is logged on or not". `--log` and `--pid-file` work on Windows too; `--daemon` does not.
