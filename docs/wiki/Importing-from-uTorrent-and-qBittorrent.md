# Importing from µTorrent and qBittorrent

This page explains how to bring your torrents over from µTorrent or qBittorrent on the same computer, so they carry on downloading and seeding in PaTorrent from where they are, without downloading anything again.

## What comes over

For each torrent, PaTorrent takes:

- the data, which stays where it is: nothing is copied or moved;
- the statistics, the file priorities (skipped files stay skipped) and the trackers;
- the label: qBittorrent's category, or its first tag when there is no category; µTorrent's label;
- a stopped or paused state;
- from qBittorrent, the torrent's own share limits (ratio and seeding time), which become its own [seeding goal](Torrent-details-and-files#torrent-properties).

PaTorrent does not trust the other program's record of which pieces are complete: it checks the files before it relies on them. Large torrents therefore show **Checking** for a while after the import.

The other program's files are only read, never changed. Torrents already in PaTorrent's list are left out, so you can run the import again safely.

## Before you start

1. **Close the other program**, including its tray icon. Both programs rewrite their files when they exit, so PaTorrent refuses to import while it sees µTorrent or qBittorrent running: "... is running. Close it, then import again: it rewrites its files when it exits." On macOS and BSD PaTorrent cannot tell whether it runs, so make sure yourself.
2. Make sure the drives with the downloaded data are connected, under the same letters or paths as before.

After the import, do not run both programs on the same torrents: they would both write to the same files.

## Import

1. Choose **File > Import from qBittorrent...** or **File > Import from µTorrent...**.
2. PaTorrent looks in the program's usual folder (below). If it is not there, a folder picker opens: "Select the qBittorrent folder" or "Select the µTorrent folder". Pick the folder described below.
3. PaTorrent asks: "Import 47 torrents from qBittorrent? Their data stays where it is and qBittorrent's files are not changed. Torrents already in the list are left out." The question lists the folders the data is in. Check them, then click **Yes**.
4. A message says how many were added, for example "Added 45 torrents from qBittorrent. 2 were left out; the Logger tab lists why."

The torrents appear in the list and start checking. Their labels show under **Labels** in the category list.

## Where PaTorrent looks

| Program | Usual folder | What to pick if asked |
|---|---|---|
| qBittorrent on Windows | `%LOCALAPPDATA%\qBittorrent\BT_backup` | the `BT_backup` folder, or the `qBittorrent` folder above it |
| qBittorrent on Linux and BSD | `~/.local/share/qBittorrent/BT_backup` (or under `$XDG_DATA_HOME`) | the same |
| qBittorrent on macOS | `~/Library/Application Support/qBittorrent/BT_backup` | the same |
| µTorrent on Windows | `%APPDATA%\uTorrent` | the folder that holds `resume.dat` |

qBittorrent keeps one `.fastresume` and one `.torrent` file per torrent in `BT_backup`; µTorrent keeps `resume.dat` beside the `.torrent` files. If your copy keeps them somewhere else, pick that folder. The data stays in the folders the records name, so those folders must exist on this computer.

## Torrents that were left out

Every torrent imported or left out gets a line in the [Logger tab](Torrent-details-and-files#logger): "Imported from qBittorrent: ..." or "Not imported from qBittorrent: ...: " followed by the reason.

| Reason | What to do |
|---|---|
| already in the list | nothing; it is in PaTorrent already |
| its .torrent file is missing | add the torrent yourself, from its `.torrent` file or magnet link, with **Save In** set to its data folder |
| its resume record cannot be read | the same |
| its .torrent file cannot be read: ... | the same, with a good copy of the `.torrent` file |

When PaTorrent finds no torrents at all, it says "No qBittorrent torrents were found in ..." with the folder it read.

## In the GTK version (Linux, BSD, macOS)

The same two items in the **File** menu, with the same steps. µTorrent has no usual folder outside Windows, so PaTorrent asks for the folder with `resume.dat`, for example one copied from a Windows computer.

## With patorrent-nox

The headless daemon imports at start:

```
patorrent-nox run --import qbittorrent
patorrent-nox run --import utorrent --import-dir "D:\Backup\uTorrent"
```

`--import` takes `qbittorrent` or `utorrent`; `--import-dir` names the folder when it is not the usual one (qBittorrent's `BT_backup` or its data folder, µTorrent's folder with `resume.dat`). The other program must not be running, and torrents already in nox's list are left as they are. See [Running patorrent-nox](Running-patorrent-nox).

The Web UI has no import, and in a window connected with **File > Connect to Remote...** the import items are greyed out: the server could not read this computer's folders. See [Remote control](Remote-control).

## Related pages

- [Adding torrents](Adding-torrents)
- [Managing torrents](Managing-torrents)
- [Queueing and seeding](Queueing-and-seeding)
