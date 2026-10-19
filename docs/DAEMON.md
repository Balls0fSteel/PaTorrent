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

Ready-made service files are in [`contrib/`](../contrib): systemd units (system and user) and a
FreeBSD rc.d script. The command-line options are the configuration; the rest (limits, folders,
the Web UI's settings) is set in the Web UI and kept in `webui.json`.

## Files

| What | Linux / BSD | Windows |
|---|---|---|
| state: `resume.db`, `metadata.pack`, `dht_state.dat` | `$XDG_DATA_HOME/patorrent/nox`, else `~/.local/share/patorrent/nox` | `%APPDATA%\PaTorrent\nox` |
| config: `webui.json`, the default `ipfilter.dat` | `$XDG_CONFIG_HOME/patorrent/nox`, else `~/.config/patorrent/nox` | the same folder as the state |

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
  once the daemon is up, with status 0, or 1 when it failed to start; until then its output
  reaches the terminal, so start-up errors and the temporary Web UI password are seen. Then the
  output goes to the `--log` file, or nowhere.
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

There is no Windows service yet (a follow-up). `patorrent-nox run` in a console stops cleanly on
Ctrl+C or when the window closes. To start it with Windows, use the Task Scheduler: a task "At
startup" that runs `patorrent-nox.exe run --save D:\Torrents --webui 8080 --log
C:\PaTorrent\nox.log` as a chosen account, "whether the user is logged on or not". `--log`
works on Windows too; `--daemon` does not.
