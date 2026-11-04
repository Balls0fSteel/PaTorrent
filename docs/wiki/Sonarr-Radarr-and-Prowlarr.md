# Sonarr, Radarr and Prowlarr

Sonarr, Radarr and Prowlarr can use PaTorrent as their download client. PaTorrent's Web UI answers the same API as qBittorrent's, so in these apps you add PaTorrent as a **qBittorrent** client. This page covers setting that up with `patorrent-nox` or with the Windows program's own Web UI, categories, download folders, and removing finished downloads.

To search your Prowlarr or Jackett from inside PaTorrent (the other direction), see [Searching for torrents](Searching-for-torrents).

## What works

These were checked against the real apps (Sonarr 4, Radarr and Prowlarr in Docker), both with `patorrent-nox` on Linux and with the Windows program's Web UI:

- The download client **Test** in each app.
- Grabs from Sonarr, Radarr and Prowlarr, each into its own category.
- An indexer's seed time arriving with the torrent and stopping it on time.
- Sonarr and Radarr importing the finished file (by hardlink, or through a remote path mapping) and then removing the torrent with **Remove Completed**.
- Prowlarr's search and grab.

Not checked: Lidarr and Readarr (Lidarr uses the same client code as Radarr), and grabs of magnet links.

## 1. Turn on PaTorrent's Web UI

**With patorrent-nox:**

```sh
patorrent-nox run --save /srv/media/downloads --webui 0.0.0.0:8080
```

Log in once in a browser with the temporary password nox prints and set your own in **Preferences > Web UI**. Details: [Web UI](Web-UI#turning-it-on-in-patorrent-nox) and [Running patorrent-nox](Running-patorrent-nox).

`0.0.0.0` lets other computers and containers reach it. When the apps run on the same computer, not in containers, `--webui 8080` (this computer only) is enough.

**With the Windows program:** **Options > Preferences > Web UI**, tick **Enable Web UI**, set a **Username:** and **Password:**, and tick **Allow connections from other computers** if the apps run on another computer. Windows Firewall needs a rule for `patorrent.exe` then. Details: [Web UI](Web-UI#turning-it-on-in-the-windows-program). PaTorrent must be running for the apps to reach it.

## 2. Add PaTorrent in the app

In Sonarr, Radarr or Prowlarr, add a new download client of the type **qBittorrent** and fill in:

| Field | Value |
|---|---|
| Host | The IP address of the computer running PaTorrent (or a host name you allowed, see [below](#host-names-and-docker)). |
| Port | The Web UI's port, such as `8080`. |
| SSL | Off, unless the Web UI uses HTTPS with a certificate the app trusts. The checks above were made over plain HTTP. |
| Username, Password | The Web UI's login. |
| Category | A category of your choice, such as `tv-sonarr`, `radarr` or `prowlarr`. |

Click **Test**, then save. The Test creates the category in PaTorrent if it does not exist yet.

Instead of a user name and password, the apps can send an API key. Turn on **Accept an API key (Authorization: Bearer)** in PaTorrent's **Preferences > Web UI**, enter a key of at least 16 characters there, and give the same key to the app. A third way is **Bypass authentication for clients in these subnets (...)** with the app's network, which lets every client on it in without a login: list only networks you trust as much as the password ([Web UI security](Web-UI#security)).

## Host names and Docker

PaTorrent answers only to IP addresses and `localhost` unless you allow more names. An app that reaches it by a name, such as `http://patorrent:8080` from a Docker container or a PC's network name, gets "Bad Request: unexpected Host header", and its Test reports a connection failure.

Either use the IP address in the app, or add the name to **Server domains (separate multiple entries with a semicolon):**:

- In nox's Web UI: **Preferences > Web UI**, under **Connectivity**. Changing it there asks for the **Current password:**.
- In the Windows program: **Options > Preferences > Web UI**.

Several names are separated with `;`; `*` matches anything. Prefer naming the hosts over `*`: the check protects against web pages that reach the Web UI under a name of their own.

## Categories and labels

A qBittorrent category is a **label** in PaTorrent. The torrents an app adds get its category as their label, so you see them under that label in the categories list ([Managing torrents](Managing-torrents)). Changing a torrent's label in PaTorrent changes its category for the app too; an app only tracks torrents in its own category.

When the app gives no download folder of its own, a torrent goes to its category's save folder if the category has one, and otherwise to PaTorrent's default download folder.

PaTorrent has no tags. Sonarr's series tags are accepted and dropped without an error.

## Download folders and paths

The app imports finished files by reading them from the path PaTorrent reports. That path must mean the same file to both:

- **Same computer, no containers:** nothing to do.
- **Containers:** mount the download folder at the same path in the app's container as PaTorrent uses, for example `/srv/media` in both. Then hardlinks work too.
- **Different computers** (for example PaTorrent on Windows saving to a share on the server where Sonarr runs): add a **Remote Path Mapping** in Sonarr and Radarr from PaTorrent's path to the app's path, such as `\\server\media\downloads\` to `/srv/media/downloads/`. This was the checked setup with the Windows program.

With nox, `--webui-save-root DIR` limits the folders the Web UI, and so the apps, may save into ([Running patorrent-nox](Running-patorrent-nox#web-ui)). Include the folders the apps use.

## Seeding limits and Remove Completed

**Remove Completed** in Sonarr and Radarr removes a torrent once it is stopped and its seeding limit is reached. PaTorrent reports both:

- **A seed ratio or seed time set on the indexer** (in Sonarr, Radarr or Prowlarr) arrives with each torrent and becomes that torrent's own seeding goal. A value the indexer leaves empty is taken from PaTorrent's settings.
- **PaTorrent's own seeding goal** is in **Preferences > Queueing**, under **Seeding Goal**: "Minimum ratio (%): [0: no ratio goal]" (150 % by default) and "Minimum seeding time (minutes): [0: no time goal]". When a torrent reaches it, PaTorrent stops it, and the app can remove it. See [Queueing and seeding](Queueing-and-seeding).

Two things to know:

- With **Limit the upload rate instead of stopping the torrent** on, a torrent keeps seeding after its goal, so the app never sees it as finished and does not remove it.
- PaTorrent's goal is reached when every target that is set is reached (ratio and time), as in µTorrent; qBittorrent stops at the first. So a torrent may seed a little longer than the app expects, but it is never removed before both are met.

A torrent you start with **Force Start** never reaches its seeding goal.

## Prowlarr

Prowlarr only adds torrents; it does not track or import them. Add PaTorrent in Prowlarr the same way (a qBittorrent client with its own category, such as `prowlarr`) to send grabs from Prowlarr's own search to PaTorrent.

## Known limits

- PaTorrent reports itself as qBittorrent's API version 2.11.2.
- Ignored when an app sends them: "first and last piece first", the content layout choice, and Sonarr's inactive seeding time limit.
- Adding a torrent that is already in the list is answered as a success; the app then tracks the existing one.

## If something goes wrong

| What you see | What it means |
|---|---|
| The Test fails, and PaTorrent's address is a name | The Host check refused the name. Use the IP address, or add the name to **Server domains** ([above](#host-names-and-docker)). |
| The Test fails with an IP address | The Web UI is not on, listens on `127.0.0.1` only (nox's `--webui 8080`, or **Allow connections from other computers** off), a firewall blocks the port, or the login is wrong. Too many wrong logins lock the app's address out for an hour. |
| The import fails: the file is not found | The app sees the download under another path. Use the same mount path, or a Remote Path Mapping ([above](#download-folders-and-paths)). |
| Finished torrents stay in the app's queue | PaTorrent keeps seeding them: no seeding goal is set, the goal is not reached yet, or **Limit the upload rate instead of stopping the torrent** is on. |

## See also

- [Web UI](Web-UI): turning it on, HTTPS and security.
- [Running patorrent-nox](Running-patorrent-nox): nox as a service that starts with the computer.
- The technical notes on what the apps call: [INTEGRATIONS.md, part 1](https://github.com/Balls0fSteel/PaTorrent/blob/main/docs/INTEGRATIONS.md#part-1-the-shim-as-an-arr-download-client).
