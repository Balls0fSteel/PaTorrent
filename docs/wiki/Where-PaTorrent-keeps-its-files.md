# Where PaTorrent keeps its files

This page says where PaTorrent keeps its settings and torrent list on each platform, how portable mode keeps them beside the program instead, and how to back them up, move them to another computer or start over. Your downloads are not part of this: they are wherever you saved them.

## Finding the folder

**Help > About PaTorrent** shows the path of the settings file at the bottom. **File > Profiles > Open Profile Folder** opens the folder (*new in 1.1.48; not in a published release yet*).

## Windows

The Windows window keeps everything in one folder: `%APPDATA%\PaTorrent`, which is usually `C:\Users\<you>\AppData\Roaming\PaTorrent`. Installed and zip copies use the same folder, unless the zip copy is portable.

| File or folder | What it holds |
|---|---|
| `patorrent.ini` | the settings |
| `state\` | the torrent list and each torrent's progress |
| `rss.dat` | RSS feeds and filters |
| `remote.ini`, `remote\` | remote connections, and the settings of each remote window |
| `ipfilter.dat` | the default IP filter file, if you use one |
| `nox\` | `patorrent-nox`'s files when it ran from a console |
| `search-helper\` | the search helper, if you got it ([Searching for torrents on Windows](Searching-for-torrents-on-Windows)) |
| `profiles\` | the other [profiles](Profiles), each a folder laid out like this one |
| `patorrent.lock` | marks the folder as in use while PaTorrent runs |

The Windows service of `patorrent-nox` keeps its files in `%ProgramData%\PaTorrent\nox` instead. See [Running patorrent-nox](Running-patorrent-nox).

## Linux and the BSDs

The GTK version follows the XDG folders:

| What | Folder |
|---|---|
| settings: `patorrent.ini`, `rss.dat`, `remote.ini`, `remote/` | `$XDG_CONFIG_HOME/patorrent/gtk`, else `~/.config/patorrent/gtk` |
| torrent list: `resume.db`, `metadata.pack`, `dht_state.dat` | `$XDG_DATA_HOME/patorrent/gtk`, else `~/.local/share/patorrent/gtk` |

`patorrent-nox` uses `nox` in place of `gtk` in both paths. Under systemd its folders are the service's own; see [Running patorrent-nox](Running-patorrent-nox).

## macOS

Both the settings and the torrent list are in `~/Library/Application Support/PaTorrent/gtk`; `patorrent-nox` uses `~/Library/Application Support/PaTorrent/nox`.

## Portable mode

In portable mode PaTorrent keeps its settings and torrent list in its own folder, as µTorrent did with `settings.dat` beside `uTorrent.exe`. The folder can be on a USB stick or a synced folder.

**To turn it on**, put a file named `patorrent.ini` in the right place before the first start. It may be empty. It must be a file: a folder of that name does not count.

| Copy | Where `patorrent.ini` goes |
|---|---|
| Windows zip | beside `patorrent.exe` |
| Linux or BSD tar.gz | beside `patorrent-gtk` (or `patorrent-nox`) |
| Linux AppImage | beside the `.AppImage` file |
| macOS | beside `PaTorrent.app`, never inside it; beside `patorrent-nox` for the daemon |
| Flatpak, Windows installer | no portable mode |

*Portable mode on Windows is in every release. On Linux and the BSDs it is new in 1.1.49, on macOS in 1.1.50; neither is in a published release yet.*

The layout inside a portable folder is the same as `%APPDATA%\PaTorrent` above: the settings and `state` beside the program, `nox` for `patorrent-nox`, `profiles` for other profiles.

### What changes in portable mode

- **The first start downloads into the folder.** New downloads go to the `Downloads` folder inside it, and PaTorrent picks a random listen port, so it does not clash with another PaTorrent on 6881. *New in 1.1.45 and 1.1.49; not in a published release yet.*
- **Paths inside the folder are stored relative to it.** Save folders, the Preferences folders, the IP filter file and the RSS save folders that lie inside the portable folder are written as `./Downloads/...`. The folder can then move to another drive letter, or to another computer, and the torrents carry on without a recheck. Paths outside the folder stay as they are. *New in 1.1.49; not in a published release yet.* Paths written by an older version are converted the next time they are saved.
- **Nothing is written to the system unless you ask.** Associations, startup with the system and services are only set when you tick them in Preferences. They then name this copy's program, and break if the folder moves.
- **It does not update itself.** A portable copy's update notice opens the download page. See [Updating PaTorrent](Updating-PaTorrent).

A portable folder written by 1.1.49 or later needs 1.1.49 or later: an older version would read the relative paths wrongly.

If the folder cannot be written (a read-only stick, `C:\Program Files`), PaTorrent stops with "This portable PaTorrent cannot write to its folder" instead of putting your settings somewhere else. Move the folder, or remove `patorrent.ini` to use the normal folders.

On macOS, a `PaTorrent.app` started straight from the Downloads folder after downloading runs from a temporary copy, where `patorrent.ini` beside it is not seen. PaTorrent says so. Move `PaTorrent.app` with Finder to the folder you want and start it again.

## One folder, one PaTorrent

Two PaTorrents never use one folder at the same time. Starting PaTorrent a second time brings the running window to the front and hands it what you opened. A copy that finds the folder in use from another session, or from another computer over a network share, shows "Another PaTorrent is already using this folder" and closes. Close the other one first. (*The check across sessions and computers is new in 1.1.43; not in a published release yet.*)

To run two PaTorrents side by side on purpose, use [profiles](Profiles).

## Backing up

1. Exit PaTorrent (**File > Exit**), so everything is saved.
2. Copy the whole folder: `%APPDATA%\PaTorrent` on Windows, both `~/.config/patorrent` and `~/.local/share/patorrent` on Linux, `~/Library/Application Support/PaTorrent` on macOS, or the portable folder.

The backup holds the settings, the torrent list with each torrent's progress, the RSS feeds and every profile. It does not hold the downloaded files.

## Moving to another computer

1. Exit PaTorrent on the old computer and copy the folder, as for a backup.
2. Install PaTorrent on the new computer, but do not start it yet.
3. Put the folder in the same place there.
4. Copy or move your downloads.
5. Start PaTorrent.

Outside portable mode the torrent list stores full paths. Put the downloads at the same paths on the new computer (the same drive letter and folders on Windows), or the torrents will not find their files.

A portable folder is easier: copy the whole folder, downloads included, and start the program inside it. Its paths are relative, so another drive letter or user name does not matter.

## Starting over

To reset PaTorrent completely:

1. Exit PaTorrent.
2. Rename the folder (for example to `PaTorrent.old`), so you can go back.
3. Start PaTorrent. It starts as a new installation, with the [Setup Guide](First-start-and-the-Setup-Guide) and an empty torrent list.

Your downloaded files stay where they are; add their torrents again to seed them. The Windows uninstaller can also delete the folder: its **Settings and torrent list** option ([Installing PaTorrent](Installing-PaTorrent#uninstalling)).
