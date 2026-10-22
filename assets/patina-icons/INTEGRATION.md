# How the PatinaTorrent icons are used

The application icon is **01 Verdigris, provisionally**: the owner has not picked the brand
direction yet. Everything that shows it reads the family name from one file,
[`app-icon.txt`](app-icon.txt) (one line, such as `01-verdigris`). The release workflow's
AppImage and macOS jobs read it in a step; the Flatpak manifest cannot read a file, so it names
the folder itself (`ICON_DIR`) and the workflow's `flatpak` job stops with an error when that
differs from `app-icon.txt`.

## Where each file goes

| Asset | Used by | How |
|---|---|---|
| `exports/app/<family>.ico` | Win32 `patorrent.exe` | `crates/win32/build.rs` writes its frames as an RT_GROUP_ICON (ID 1) plus RT_ICONs into a `.res` that `link.exe` embeds (MSVC builds). Explorer, the taskbar and Alt-Tab show it; `crates/win32/src/icons.rs` loads it with `LoadImageW` for the window (big and small), the tray and the About box. |
| `exports/app/<family>/{16,20,24,32,48,64,128,256}.png` | GTK `patorrent-gtk`, nox's Web UI | Embedded by `crates/frontend/src/icons.rs` (`app_png`). GTK writes them as an icon theme (`hicolor/<n>x<n>/apps/io.github.balls0fsteel.PaTorrent.png`) to `$XDG_CACHE_HOME/patorrent/icons` at startup for the window icon, the About box and the tray's `IconThemePath`, and to `$XDG_DATA_HOME/icons/hicolor` when desktop integration writes the `.desktop` entries; 16 to 64 px are also the tray's `IconPixmap`. The Web UI serves the 32 px one as `/favicon.png`. |
| `exports/app/<family>/256.png` | AppImages | `$APP_ICON_DIR/256.png` in `.github/workflows/release.yml`'s Linux job, `APP_ICON_DIR` set from `app-icon.txt` in a step. |
| `exports/app/<family>/` | the macOS `.app` bundles, the Flatpak | `APP_ICON_DIR` in the workflow's macOS job (set from `app-icon.txt`); `ICON_DIR` in `packaging/flatpak/io.github.balls0fsteel.PaTorrent.yml`, checked against `app-icon.txt` by the `flatpak` job. |
| `exports/states/{16,20,24,32}/<state>.png` | both GUIs | Embedded by `crates/frontend/src/icons.rs` (`StateIcon`). Win32: the torrent list's Name column, the Trackers tab and the status bar's network indicator, at the size for the window's DPI (16 px at 100 %, 20 at 125 %, 24 at 150 %, 32 at 200 %). GTK: the Name column, the Trackers tab and the status bar, the 32 px export drawn at 16 logical pixels. |

Which state icon a torrent shows is `status::status_icon` (`crates/frontend/src/status.rs`),
which follows the branches of the status text and `states/manifest.json`; trackers and the
network indicator are `icons::tracker_icon` and `icons::network_icon`. The four optional states
(stalled download, idle seed, no peers, allocating), `wanted-complete` and `overlay-force` are not
used. The toolbar images stay the runtime-drawn ones (`icons::toolbar_bgra`); the GTK toolbar
uses the desktop's theme icons.

No image decoder is involved: Windows reads the PNGs as icon images
(`CreateIconFromResourceEx`), GTK as textures (`gdk::Texture::from_bytes`).

## Switching to another candidate

1. Put the folder name of the candidate in `app-icon.txt`: `02-exchange` or `03-foundry`.
2. Change `ICON_DIR` in `packaging/flatpak/io.github.balls0fsteel.PaTorrent.yml` to the same
   family's folder (the release workflow's `flatpak` job fails until it matches). The workflow's
   AppImage and macOS jobs need no change: they read `app-icon.txt`.
3. Build. `crates/frontend/build.rs` and `crates/win32/build.rs` rebuild when `app-icon.txt`
   changes and stop with an error when it names no folder of `exports/app/`; the frontend's
   tests check every embedded size of the family named there.

Windows caches executable icons: Explorer may show the old one until its icon cache is cleared
(`ie4uinit.exe -show`) or the file is renamed. GTK users who turned desktop integration on get
the new icon in `$XDG_DATA_HOME/icons/hicolor` the next time an entry is written (a box changed
in Preferences); the cache folder is refreshed at every start.
