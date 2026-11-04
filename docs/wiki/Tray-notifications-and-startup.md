# Tray, notifications and startup

This page covers PaTorrent's tray icon, closing and minimizing to the tray, the notification when a download completes, starting PaTorrent when you log in, and making PaTorrent the program that opens `.torrent` files and `magnet:` links.

## The tray icon (Windows)

While PaTorrent runs, its icon is in the notification area of the taskbar. Pointing at it shows the version and the current download and upload speeds.

- A left click (or a double-click) hides the main window, or brings it back.
- A right click opens the tray menu:

| Item | What it does |
|---|---|
| **Hide PaTorrent** / **Show PaTorrent** | Hides the main window, or shows it again |
| **Add Torrent...** | Opens a `.torrent` file, as **File > Add Torrent...** does |
| **Add Torrent from URL...** | Adds a magnet link or a web address |
| **Start All Torrents** | Starts every torrent in the list |
| **Stop All Torrents** | Stops every torrent in the list |
| **Exit** | Quits PaTorrent |

## Close and minimize to the tray

Two settings decide what happens to the window:

| Setting | Default | What it does |
|---|---|---|
| **Close to tray** | On | The window's close button (and <kbd>Alt</kbd>+<kbd>F4</kbd>) hides PaTorrent in the tray instead of exiting, so transfers go on. |
| **Minimize to tray** | Off | Minimizing hides the window in the tray instead of the taskbar. |

Set them in **Options > Preferences > UI Settings** under **System Tray**, or tick them straight in the **Options** menu (**Close to Tray** and **Minimize to Tray**).

To quit PaTorrent while **Close to tray** is on, use **File > Exit** or **Exit** in the tray menu. PaTorrent then stops its torrents and saves their progress. If that takes more than a moment (for example while files are still moving to another drive), a window shows what it is waiting for, with a **Quit Now** button.

The Web UI that the Windows window serves runs only while PaTorrent runs, so closing to the tray keeps it available. See [Web UI](Web-UI).

## Notification when a download completes

With **Show balloon notifications in tray** on (the default), PaTorrent shows a "Download Complete" notification with the torrent's name each time a download finishes. Turn it off in **Options > Preferences > UI Settings**.

When an update is available, PaTorrent says so in the status bar instead; see [Updating PaTorrent](Updating-PaTorrent).

## Show the speed in the title bar

**Show current speed in the title bar** (Preferences > UI Settings, off by default) puts the download and upload speeds at the start of the window's title, so you can read them on the taskbar button.

## Start PaTorrent when you log in

1. Open **Options > Preferences > General**.
2. Tick **Start PaTorrent on system startup**.
3. To have it start in the tray without showing its window, also tick **Start minimized**.
4. Click **OK**.

On Windows this is an entry in your user account's startup programs. No administrator rights are needed, and a portable copy can use it too. If you move a portable copy to another folder, tick the box again: it is ticked only while the entry starts this very copy.

With [profiles](Profiles), each profile has its own startup entry, so you can start several at login. *Per-profile entries are new in 1.1.46; not in a published release yet.*

The [Setup Guide](First-start-and-the-Setup-Guide) offers the same choice on its last page.

## Open .torrent files and magnet links with PaTorrent

1. Open **Options > Preferences > General**.
2. Tick **Associate with .torrent files** and **Associate with magnet: links**.
3. Click **OK**.

Double-clicking a `.torrent` file, or clicking a magnet link in your browser, then hands it to PaTorrent. If PaTorrent is already running, the running copy takes it. With **Show a window that displays the files inside the torrent** on, the Add New Torrent window opens; with **Activate the program window** on, the main window also comes to the front. See [Adding torrents](Adding-torrents).

Good to know on Windows:

- The settings are for your user account only and need no administrator rights. The installer can set them too.
- If you chose another program in Windows itself ("Always use this app", or Settings > Default apps), Windows keeps that choice, and only you can change it there. PaTorrent then says which program Windows still uses, for example "Windows still opens .torrent files with ...". Click **Default apps...** to open the right place in Windows Settings and choose PaTorrent.
- With profiles, only one profile can open `.torrent` files and magnet links. Ticking the boxes in one profile takes them over from the other; the page names the profile that has them now.
- **Check association on startup** (off by default) makes PaTorrent ask at each start whether to take the associations back when another program has them. It does not ask when PaTorrent starts minimized at login.

## GTK version (Linux, BSD, macOS)

**The tray.** On Linux and BSD, PaTorrent shows a tray icon on desktops that keep one: KDE Plasma, Xfce, LXQt, Cinnamon, MATE, waybar, and GNOME with the AppIndicator extension. Its menu has the same items as on Windows. On plain GNOME without that extension there is no tray: the tray boxes do not appear in Preferences, and closing the window quits PaTorrent. On macOS the icon is in the menu bar. <kbd>Ctrl</kbd>+<kbd>Q</kbd> quits from the window.

**Minimize to tray** needs the desktop to report that the window was minimized; where it does not (some Wayland desktops), minimizing stays a normal minimize.

**Notifications** go through the desktop's own notification service, with the same "Download Complete" text.

**Start at login.** **Start PaTorrent on system startup** writes an autostart entry in your user's `~/.config/autostart` folder. In a Flatpak it asks the desktop through its background permission instead, and the desktop may ask you to allow it; there only the default profile can start at login. On macOS it is a login item (System Settings > General > Login Items lists it).

**Associations.** The boxes make PaTorrent the default program for `.torrent` files and `magnet:` links in your user's settings. Unticking a box gives the type back to the system's default; it does not restore a program you used before. If you move an AppImage, tick the boxes again. In a Flatpak the boxes are greyed: the desktop already offers PaTorrent for both types, and you choose the default program in your desktop's settings. On macOS the boxes work only when PaTorrent runs as `PaTorrent.app`, and turning one off is done in Finder's Get Info > Open with.

## Web UI and patorrent-nox

The Web UI has no tray, notifications or startup settings: its Preferences show the association and autostart boxes greyed, with the note that they belong to the desktop client. To start `patorrent-nox` with the computer, run it as a service; see [Running patorrent-nox](Running-patorrent-nox).
