# Troubleshooting and FAQ

This page helps when something does not work: slow or stalled downloads, a port that cannot be reached, warnings from Windows or macOS, private trackers. It also says where Rookparcel's messages are, how to reset its settings, and how to report a problem.

## Downloads are slow or do not start

Look at the torrent's **Status** column and at the status bar at the bottom of the window. They usually say why.

| What you see | What it means and what to do |
|---|---|
| **Queued** | The queue is full: the number of active torrents or downloads set in **Preferences > Queueing** is reached. The torrent starts when another one finishes or stops. **Force Start** in the right-click menu runs it outside the queue. See [Queueing and seeding](Queueing-and-seeding). |
| **Stopped** or **Paused** | Start it with **Start** (<kbd>Ctrl</kbd>+<kbd>S</kbd>). With **Don't start the download automatically** on in Preferences > UI Settings, new torrents are added stopped. |
| **Queued for checking**, **Checking** | Rookparcel is checking the files already on disk. Torrents on the same drive are checked one after another; with **Check torrents gently** on, they give way to other programs and take longer while the disk is busy. See [Disk and storage](Disk-and-storage). |
| **Waiting for the network** | **Use Only This Network** is on in Preferences > Connection and its network (usually your VPN) is not there. The torrent carries on by itself when it comes back. See [Connection, ports and firewall](Connection-ports-and-firewall). |
| **Error: ...** | The message after "Error:" says what went wrong. |
| "Scheduler: limited", "Scheduler: turned off" or "Scheduler: seeding only" in the status bar | The scheduler is holding speeds back this hour. See [Speed limits and scheduler](Speed-limits-and-scheduler). |
| "(auto ...)" beside the speeds in the status bar | **Keep ping low** is capping the speed because the delay on your line grew. See [Keep ping low](Keep-ping-low). |

Other things to check:

- **Seeds and peers.** A torrent with no seeds cannot finish. The **Trackers** tab shows whether the trackers answer; the **Peers** tab shows who you are connected to. See [Torrent details and files](Torrent-details-and-files).
- **Magnet links** need the torrent's file list from other peers first, and most need the DHT: keep **Enable DHT Network** on in Preferences > BitTorrent.
- **Speed limits.** Check **Preferences > Bandwidth** and the torrent's own limits (**Bandwidth Allocation** in the right-click menu, or **Properties**). 0 means unlimited.
- **A busy disk.** With **Slow downloads down (disk writes)** on (Preferences > Advanced > Disk I/O), Rookparcel lowers the download speed while the disk answers slowly.
- **A proxy for peer connections.** With **Use proxy for peer-to-peer connections** on, nobody can connect to you, which leaves fewer peers.
- **No incoming connections** (below) also costs speed: you can reach only peers that accept connections themselves.

## "No incoming connections" or "Port ... in use"

The left part of the status bar shows the state of the network:

| Text | Meaning |
|---|---|
| **Network OK** | Other peers have connected to you, so your listening port is reachable. |
| **No incoming connections** | No peer has connected to you yet. Right after the start this is normal; if it stays, your port is probably blocked by the router or the firewall. |
| **Port 6881 in use** (with your port number) | Another program, often another Rookparcel profile, holds the port, so Rookparcel listens on another one where nothing is forwarded. Click the text to open **Preferences > Connection** and choose another port. *New in 1.1.45; not in a published release yet.* |

To make the port reachable:

1. Open **Options > Preferences > Connection** and note the **Port used for incoming connections**.
2. Tick **Enable UPnP port mapping** and **Enable NAT-PMP port mapping**, so the router is asked to forward the port. Or forward the port by hand in the router (TCP and UDP) to this computer, and leave **Randomize port each start** off.
3. Allow `rookparcel.exe` through the firewall (on Linux, `rookparcel-gtk`), or open the port in it.
4. Behind a VPN, incoming connections work only if the VPN forwards a port to you.

The **General** tab shows "Your address as trackers see it:", which tells you whether your traffic goes through a VPN. *New in 1.1.58; not in a published release yet.* More in [Connection, ports and firewall](Connection-ports-and-firewall).

## Windows SmartScreen or macOS Gatekeeper warns about Rookparcel

The programs are not code-signed, so the systems warn the first time:

- **Windows:** "Windows protected your PC". Click **More info**, then **Run anyway**.
- **macOS:** the app is refused until you open it with a right-click > **Open**.

To make sure a file is the one that was published, compare its checksum with the `SHA256SUMS` file of the release. Details for each platform are in [Installing Rookparcel](Installing-Rookparcel).

## Can I use Rookparcel on private trackers?

Not yet. Private trackers accept only the clients they have whitelisted, and Rookparcel's client ID (its peer ID starts with `-PA`, and it sends `Rookparcel/<version>` to trackers) is not whitelisted anywhere yet. Rookparcel always reports itself honestly as Rookparcel, so using it on a private tracker could get your account into trouble. Check your tracker's list of allowed clients first.

Torrents marked private never use the DHT, Local Peer Discovery or Peer Exchange, whatever Preferences say.

## Double-clicking a .torrent file or a magnet link does not open Rookparcel

Tick **Associate with .torrent files** and **Associate with magnet: links** in **Preferences > General**. If Windows still opens them with another program, a program was chosen in Windows itself, and only you can change that: click **Default apps...** and choose Rookparcel there. See [Tray, notifications and startup](Tray-notifications-and-startup#open-torrent-files-and-magnet-links-with-rookparcel).

## The Web UI cannot be reached from another device

- Tick **Allow connections from other computers** in **Preferences > Web UI**; otherwise it listens on this computer only.
- Allow `rookparcel.exe` through Windows Firewall for the Web UI's port.
- "port ... is in use, perhaps by another Rookparcel profile": choose another **Listening port**.
- An app that reaches Rookparcel by a host name (as in Docker) fails with "unexpected Host header". Add the name in **Server domains**, or use the IP address.

See [Web UI](Web-UI) and [Sonarr, Radarr and Prowlarr](Sonarr-Radarr-and-Prowlarr).

## Closing Rookparcel takes a long time

Rookparcel saves the torrent list and progress, tells the trackers it stopped and waits for files that are still moving. When that takes more than a moment, the "Rookparcel is closing" window shows what it is waiting for. **Quit Now** stops waiting; it asks first and lists what it gives up. The torrent list and progress are saved first in any case.

## A portable copy cannot write to its folder

"This portable Rookparcel cannot write to its folder" means the folder with `rookparcel.exe` and `rookparcel.ini` is read-only for you, for example under `C:\Program Files`. Move the folder where you can write, or remove `rookparcel.ini` beside the program to keep the settings in your user folder. See [Where Rookparcel keeps its files](Where-Rookparcel-keeps-its-files).

## The search helper does not work

See the table at the end of [Searching for torrents on Windows](Searching-for-torrents-on-Windows) and [Searching for torrents](Searching-for-torrents).

## Linux and macOS notes

- **No tray icon on GNOME.** Plain GNOME has no tray; install the AppIndicator extension, or use Rookparcel without one (closing the window then quits it). See [Tray, notifications and startup](Tray-notifications-and-startup#gtk-version-linux-bsd-macos).
- **Minimize to tray does nothing** on some Wayland desktops, which do not report that a window was minimized.
- **Logging out on Xfce** does not wait for Rookparcel to save; quit it first.

## Where the logs are

**The Windows and GTK versions** write no log file. Their messages (errors, the IP filter, the update check, the scheduler, completed downloads) go to the **Logger** tab at the bottom of the main window, which keeps the last 1000 lines while Rookparcel runs. Look at it right after the problem happens, before you exit, and note the lines about it.

**rookparcel-nox** prints its messages to the terminal. For more detail start it with `--verbose`, which adds tracker and peer events (each disconnect with its reason). `--log FILE` appends everything to a file. When it runs as a service:

| Where | Log |
|---|---|
| Windows service | `nox.log` in the state folder, `%ProgramData%\Rookparcel\nox` unless `--state-dir` says otherwise (read it from an elevated prompt) |
| systemd | the journal: `journalctl -u rookparcel-nox` |
| FreeBSD rc.d | `/var/log/rookparcel-nox.log` |

See [Running rookparcel-nox](Running-rookparcel-nox).

**The search helper** (Jackett) keeps its own settings and log in the `data` folder inside `search-helper` in your Rookparcel settings folder.

## Resetting settings

- **One setting:** in **Preferences > Advanced**, select it and click **Reset**, then **OK**. Rows marked `*` differ from the default.
- **The first-start questions:** **Options > Setup Guide** (<kbd>Ctrl</kbd>+<kbd>G</kbd>) asks them again. *New in 1.1.62; not in a published release yet.*
- **Everything:** exit Rookparcel (**File > Exit**), then rename the folder where it keeps its settings and torrents (on Linux and BSD, the settings folder and the data folder); Rookparcel then starts as new, with an empty torrent list. On Windows **Help > About Rookparcel** shows the folder. Renaming it back restores everything. See [Where Rookparcel keeps its files](Where-Rookparcel-keeps-its-files).

## Reporting a bug

Open an issue on the tracker: <https://github.com/Balls0fSteel/Rookparcel/issues>. Please include:

1. Your system (Windows 11, Ubuntu 24.04, macOS 15...) and the file you downloaded.
2. Rookparcel's version, from **Help > About Rookparcel**.
3. What you did, what you expected and what happened instead, step by step.
4. For a crash or a stuck torrent, the lines from the **Logger** tab, or the output of `rookparcel-nox run --verbose`.

Leave out passwords, API keys and anything else private; the issue tracker is public.

## Reporting a security problem

Do not open a public issue. Use GitHub's private vulnerability reporting: <https://github.com/Balls0fSteel/Rookparcel/security/advisories/new>. See [SECURITY.md](https://github.com/Balls0fSteel/Rookparcel/blob/main/SECURITY.md).

## FAQ

**Is Rookparcel free?** Free for noncommercial use under the PolyForm Noncommercial License 1.0.0: personal use, study, hobby projects, charities, schools and public institutions, at no cost. Commercial use needs separate permission. See [LICENSE.md](https://github.com/Balls0fSteel/Rookparcel/blob/main/LICENSE.md).

**Is it related to µTorrent or qBittorrent?** No. Rookparcel recreates the look and behaviour of µTorrent 2.2.1 but uses none of its code. Its engine is an independent Rust port of libtorrent-rasterbar 2.0.11. Its Web API follows qBittorrent's published documentation so that tools such as Sonarr can talk to it.

**Which languages does it speak?** English and 34 more, all built in. It follows your system's language; choose another in **Preferences > General**. The translations are machine-made; corrections are welcome as issues.

**What does Rookparcel send, and to whom?** The update check downloads one small file from the project website, with nothing about you or your torrents; turn it off in **Preferences > General > Privacy**. Searches go only to the search sources you added. Like every BitTorrent client, Rookparcel shows your IP address to the trackers, the DHT and the peers of your torrents. See [Updating Rookparcel](Updating-Rookparcel).

**Where are my settings and torrents?** See [Where Rookparcel keeps its files](Where-Rookparcel-keeps-its-files).

**Can I run several copies?** Yes, as profiles. See [Profiles](Profiles).

**Where do I get help?** Read the [FAQ on the website](https://balls0fsteel.github.io/Rookparcel/faq.html) and the pages of this wiki, or ask on the [issue tracker](https://github.com/Balls0fSteel/Rookparcel/issues).
