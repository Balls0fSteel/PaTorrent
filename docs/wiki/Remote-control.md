# Remote control

The desktop program can control a `patorrent-nox` on another computer: a NAS, a home server, a PC in another room. The window looks and works as usual, but the torrents, downloads and engine are on the server. This page covers connecting with **File > Connect to Remote...** or `--remote`, what works in a remote window, and how the connection is protected.

The alternative is the [Web UI](Web-UI) in a browser, which needs nothing installed. A remote window gives you the full desktop program instead: its dialogs, keyboard shortcuts and detail tabs.

## What you need

- A `patorrent-nox` running with its Web UI on, for example `patorrent-nox run --webui 0.0.0.0:8443 --webui-https` ([Running patorrent-nox](Running-patorrent-nox)). The remote window uses the Web UI's port and login.
- The Web UI's user name and password.
- The same PaTorrent version on both sides. A server that speaks another version refuses the connection with "the server refused the connection (HTTP 426): this server speaks remote protocol ...".

A remote window connects to `patorrent-nox`. The Windows program's own Web UI (**Preferences > Web UI**) is for browsers.

## Connecting

1. In the Windows program choose **File > Connect to Remote...**. A second PaTorrent window starts beside your own and shows the **Connect to Remote** dialog. (Or start `patorrent.exe --remote`, which opens only the remote window.)
2. Fill in:

   | Field | What to enter |
   |---|---|
   | **Host:** | The server's name or IP address. |
   | **Port:** | The Web UI's port (8080 is proposed). |
   | **User name:** | The Web UI's user name (`admin` is proposed). |
   | **Password:** | The Web UI's password. |
   | **Use HTTPS** | Ticked by default. Leave it ticked when the server runs with `--webui-https` or its own certificate. |

3. Click **Connect**.
4. The first time, a server with a self-signed certificate (the one `--webui-https` makes) is shown by its fingerprint:

   > ... presents a certificate that this computer does not trust (a self-signed one, as patorrent-nox --webui-https makes).

   Compare the long number with the `certificate SHA-256` line nox printed when it started (in its terminal, log or journal). If they match, click **Yes**. PaTorrent remembers the certificate for that server and does not ask again.
5. The window opens with the server's torrents. Its title bar shows the server's `host:port`.

The dialog remembers the host, port, user name and the HTTPS choice for next time. The password is never stored; you type it at each connection.

In the GTK program (Linux, BSD, macOS) it is the same: **File > Connect to Remote...**, or `patorrent-gtk --remote`. Each remote window is a program of its own, so you can have your local window and several servers open at once.

## Without HTTPS

If you untick **Use HTTPS** and the host is another computer, PaTorrent asks first:

> ... is another computer, and without HTTPS your user name, password and every command cross the network unencrypted: anyone on it can read or change them.

Answer **No** and turn HTTPS on at the server instead (`--webui-https`), unless the network between the two is entirely yours. A connection to the same computer (`localhost`, `127.0.0.1`) is not asked about.

## What works in a remote window

Nearly everything works as in a local window, acting on the server:

- Adding torrents: `.torrent` files from this computer are sent to the server; magnet links too.
- Start, stop, pause, force start, remove (also with the data, deleted on the server), recheck, queue order, labels, speed limits, trackers, file priorities, renaming, Torrent Properties.
- The detail tabs, with the server's peers, pieces, files and speed graph.
- **Preferences:** the engine's settings (connection, bandwidth, BitTorrent, queueing and the like) are the server's. What you change there is applied on the server and kept there.

What is different:

| | In a remote window |
|---|---|
| Folders | Folders are on the server. Folder pickers for torrents are off; **Set Download Location...** asks for a server folder as text. New downloads go to the server's default folder. |
| **Open Containing Folder** | Off: the folder is on the server. |
| Creating torrents | A torrent you create here cannot be seeded by the server, since the files are on this computer. |
| IP filter | The server's own IP filter applies; the window's is not sent. |
| **Preferences > Web UI** | A remote window cannot serve the Web UI: "A remote window cannot serve the Web UI; use the local window's Preferences." |
| Labels, RSS feeds, window settings | Kept on this computer, separately for each server (see below). |
| Scheduler, RSS Downloader, seeding goal, `.torrent` folders | The window's own run while it is open and act on the server's torrents, in addition to anything nox does itself. |

The server keeps downloading after you close the window.

## Losing the connection

When the connection drops, PaTorrent says so once:

> The connection to ... was lost: ... The list shows the torrents as they were. Close this window and use File > Connect to Remote... to connect again.

The title bar then ends in "(disconnected)". Commands go nowhere from then on. Close the window and connect again.

## When connecting fails

| Message | What to check |
|---|---|
| "Enter the server's host name or address, and its port (1 to 65535)." | The host is empty or the port is not a number in range. |
| "Cannot connect to ...: wrong user name or password" | The Web UI login. Too many wrong tries lock the address out for a while ([Web UI](Web-UI#logging-in)). |
| "... Is it a patorrent-nox with its Web UI on this port (--webui)?" | Something else answered on that port. |
| "... If the server's Web UI uses HTTPS (--webui-https), tick Use HTTPS." | The server speaks HTTPS but **Use HTTPS** was off. |
| "Cannot reach ... over HTTPS: ..." | **Use HTTPS** is on, but the server does not speak HTTPS, or cannot be reached at all. |
| "The certificate of ... is not the one you accepted before: ..." | The server's certificate changed. If you replaced it, or nox renewed its own, connect again to check the new one. Otherwise someone may be in between: do not go on. |
| "... (HTTP 400): ..." naming the Host header | The server refuses the host name you typed. Use its IP address, or add the name to the server's **Preferences > Web UI > Server domains** ([Web UI](Web-UI#security)). |
| "... (HTTP 426): this server speaks remote protocol ..." | The two programs are different versions. Update the older one ([Updating PaTorrent](Updating-PaTorrent)). |

The firewall on the server must let the Web UI's port in.

## Where the remote window keeps its files

| | Windows | GTK (Linux, BSD) |
|---|---|---|
| Connection details and accepted certificates | `remote.ini` beside `patorrent.ini` in the settings folder | `remote.ini` in `~/.config/patorrent/gtk` |
| The window's settings, labels and RSS feeds per server | `remote\<host>_<port>` in the settings folder | `~/.config/patorrent/gtk/remote/<host>_<port>` |

These never mix with your local window's settings. See [Where PaTorrent keeps its files](Where-PaTorrent-keeps-its-files).

## Security

- The remote window logs in with the Web UI's login, so everything on [Web UI security](Web-UI#security) applies: a long password, HTTPS across a network.
- Wrong passwords count towards the same lock-out as the Web UI's login form.
- Setting a new Web UI password disconnects every remote window.
- A certificate that changes after you accepted it is refused with a warning; PaTorrent asks again at the next attempt.
- The password is never written to disk on this computer.

## See also

- [Running patorrent-nox](Running-patorrent-nox): starting the server, HTTPS options and services.
- [Web UI](Web-UI): the same server in a browser.
- The technical description: [WEBUI.md, Remote mode](https://github.com/Balls0fSteel/PaTorrent/blob/main/docs/WEBUI.md#remote-mode).
