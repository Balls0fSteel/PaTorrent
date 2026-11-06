# Where Rookparcel keeps its files

This page says where Rookparcel keeps its settings and torrent list on each platform, how portable mode keeps them beside the program instead, and how to back them up, move them to another computer or start over. Your downloads are not part of this: they are wherever you saved them.

## Finding the folder

**Help > About Rookparcel** shows the path of the settings file at the bottom. **File > Profiles > Open Profile Folder** opens the folder (*new in 1.1.48; not in a published release yet*).

## Windows

The Windows window keeps everything in one folder: `%APPDATA%\Rookparcel`, which is usually `C:\Users\<you>\AppData\Roaming\Rookparcel`. Installed and zip copies use the same folder, unless the zip copy is portable.

| File or folder | What it holds |
|---|---|
| `rookparcel.ini` | the settings |
| `state\` | the torrent list and each torrent's progress |
| `rss.dat` | RSS feeds and filters |
| `remote.ini`, `remote\` | remote connections, and the settings of each remote window |
| `ipfilter.dat` | the default IP filter file, if you use one |
| `nox\` | `rookparcel-nox`'s files when it ran from a console |
| `search-helper\` | the search helper, if you got it ([Searching for torrents on Windows](Searching-for-torrents-on-Windows)) |
| `profiles\` | the other [profiles](Profiles), each a folder laid out like this one |
| `rookparcel.lock` | marks the folder as in use while Rookparcel runs |

The Windows service of `rookparcel-nox` keeps its files in `%ProgramData%\Rookparcel\nox` instead. See [Running rookparcel-nox](Running-rookparcel-nox).

## Linux and the BSDs

The GTK version follows the XDG folders:

| What | Folder |
|---|---|
| settings: `rookparcel.ini`, `rss.dat`, `remote.ini`, `remote/` | `$XDG_CONFIG_HOME/rookparcel/gtk`, else `~/.config/rookparcel/gtk` |
| torrent list: `resume.db`, `metadata.pack`, `dht_state.dat` | `$XDG_DATA_HOME/rookparcel/gtk`, else `~/.local/share/rookparcel/gtk` |

`rookparcel-nox` uses `nox` in place of `gtk` in both paths. Under systemd its folders are the service's own; see [Running rookparcel-nox](Running-rookparcel-nox).

## macOS

Both the settings and the torrent list are in `~/Library/Application Support/Rookparcel/mac`, laid out like the Windows folder above; `rookparcel-nox` uses `~/Library/Application Support/Rookparcel/nox`. A [profile](Profiles) `work` is in `~/Library/Application Support/Rookparcel/profiles/work/mac`.

The GTK version of `Rookparcel.app` in earlier releases used a `gtk` folder in the same places. The first start of the native app renames each `gtk` folder to `mac` (the default profile's and every profile's) and corrects the paths into it in its settings, so everything carries over. It does this once: only where there is no `mac` folder yet, and not while the old app is using the folder. Portable folders are left as they are.

*The native macOS app and its `mac` folder are not in a published release yet; earlier releases use `gtk`.*

## Portable mode

In portable mode Rookparcel keeps its settings and torrent list in its own folder, as µTorrent did with `settings.dat` beside `uTorrent.exe`. The folder can be on a USB stick or a synced folder.

**To turn it on**, put a file named `rookparcel.ini` in the right place before the first start. It may be empty. It must be a file: a folder of that name does not count.

| Copy | Where `rookparcel.ini` goes |
|---|---|
| Windows zip | beside `rookparcel.exe` |
| Linux or BSD tar.gz | beside `rookparcel-gtk` (or `rookparcel-nox`) |
| Linux AppImage | beside the `.AppImage` file |
| macOS | beside `Rookparcel.app`, never inside it; beside `rookparcel-nox` for the daemon |
| Flatpak, Windows installer | no portable mode |

*Portable mode on Windows is in every release. On Linux and the BSDs it is new in 1.1.49, on macOS in 1.1.50; neither is in a published release yet.*

The layout inside a portable folder is the same as `%APPDATA%\Rookparcel` above: the settings and `state` beside the program, `nox` for `rookparcel-nox`, `profiles` for other profiles.

### What changes in portable mode

- **The first start downloads into the folder.** New downloads go to the `Downloads` folder inside it, and Rookparcel picks a random listen port, so it does not clash with another Rookparcel on 6881. *New in 1.1.45 and 1.1.49; not in a published release yet.*
- **Paths inside the folder are stored relative to it.** Save folders, the Preferences folders, the IP filter file and the RSS save folders that lie inside the portable folder are written as `./Downloads/...`. The folder can then move to another drive letter, or to another computer, and the torrents carry on without a recheck. Paths outside the folder stay as they are. *New in 1.1.49; not in a published release yet.* Paths written by an older version are converted the next time they are saved.
- **Nothing is written to the system unless you ask.** Associations, startup with the system and services are only set when you tick them in Preferences. They then name this copy's program, and break if the folder moves.
- **It does not update itself.** A portable copy's update notice opens the download page. See [Updating Rookparcel](Updating-Rookparcel).

A portable folder written by 1.1.49 or later needs 1.1.49 or later: an older version would read the relative paths wrongly.

If the folder cannot be written (a read-only stick, `C:\Program Files`), Rookparcel stops with "This portable Rookparcel cannot write to its folder" instead of putting your settings somewhere else. Move the folder, or remove `rookparcel.ini` to use the normal folders.

On macOS, a `Rookparcel.app` started straight from the Downloads folder after downloading runs from a temporary copy, where `rookparcel.ini` beside it is not seen. Rookparcel says so. Move `Rookparcel.app` with Finder to the folder you want and start it again.

## One folder, one Rookparcel

Two Rookparcels never use one folder at the same time. Starting Rookparcel a second time brings the running window to the front and hands it what you opened. A copy that finds the folder in use from another session, or from another computer over a network share, shows "Another Rookparcel is already using this folder" and closes. Close the other one first. (*The check across sessions and computers is new in 1.1.43; not in a published release yet.*)

To run two Rookparcels side by side on purpose, use [profiles](Profiles).

## Backing up

1. Exit Rookparcel (**File > Exit**; on macOS **Rookparcel > Quit Rookparcel**), so everything is saved.
2. Copy the whole folder: `%APPDATA%\Rookparcel` on Windows, both `~/.config/rookparcel` and `~/.local/share/rookparcel` on Linux, `~/Library/Application Support/Rookparcel` on macOS, or the portable folder.

The backup holds the settings, the torrent list with each torrent's progress, the RSS feeds and every profile. It does not hold the downloaded files.

## Moving to another computer

1. Exit Rookparcel on the old computer and copy the folder, as for a backup.
2. Install Rookparcel on the new computer, but do not start it yet.
3. Put the folder in the same place there.
4. Copy or move your downloads.
5. Start Rookparcel.

Outside portable mode the torrent list stores full paths. Put the downloads at the same paths on the new computer (the same drive letter and folders on Windows), or the torrents will not find their files.

A portable folder is easier: copy the whole folder, downloads included, and start the program inside it. Its paths are relative, so another drive letter or user name does not matter.

## Starting over

To reset Rookparcel completely:

1. Exit Rookparcel.
2. Rename the folder (for example to `Rookparcel.old`), so you can go back.
3. Start Rookparcel. It starts as a new installation, with the [Setup Guide](First-start-and-the-Setup-Guide) and an empty torrent list.

Your downloaded files stay where they are; add their torrents again to seed them. The Windows uninstaller can also delete the folder: its **Settings and torrent list** option ([Installing Rookparcel](Installing-Rookparcel#uninstalling)).
