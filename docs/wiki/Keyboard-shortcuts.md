# Keyboard shortcuts

This page lists the keyboard shortcuts of the Windows version of PaTorrent, then what differs in the GTK version and in the Web UI. Most of them are also written beside their menu items.

## Main window (Windows)

| Keys | What they do |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>O</kbd> | **File > Add Torrent...** |
| <kbd>Ctrl</kbd>+<kbd>U</kbd> | **File > Add Torrent from URL...** (a magnet link or web address on the clipboard is filled in) |
| <kbd>Ctrl</kbd>+<kbd>N</kbd> | **File > Create New Torrent...** |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> | **Options > Preferences** |
| <kbd>Ctrl</kbd>+<kbd>G</kbd> | **Options > Setup Guide** |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | Moves to the search box on the toolbar (when the toolbar is shown). In the Files tab, moves to the Files tab's filter box instead. |
| <kbd>Ctrl</kbd>+<kbd>A</kbd> | Selects every torrent in the list. In the Files tab, selects the files the filter shows. |
| <kbd>Ctrl</kbd>+<kbd>S</kbd> | **Start** the selected torrents |
| <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>P</kbd> | **Pause** the selected torrents |
| <kbd>Ctrl</kbd>+<kbd>T</kbd> | **Stop** the selected torrents |
| <kbd>Del</kbd> | **Remove** the selected torrents |
| <kbd>Shift</kbd>+<kbd>Del</kbd> | **Remove And > Delete Data**: removes the torrents and deletes their files |
| <kbd>Enter</kbd> | **Properties** of the selected torrents (in the torrent list) |
| <kbd>Shift</kbd>+<kbd>F10</kbd> or the menu key | Opens the right-click menu under the focused row |
| <kbd>Alt</kbd>+<kbd>F4</kbd> | Closes the window. With **Close to tray** on (the default), PaTorrent keeps running in the tray. |

With **Confirm when deleting torrents** on in Preferences, <kbd>Del</kbd> and <kbd>Shift</kbd>+<kbd>Del</kbd> ask first. **File > Exit** has no shortcut; see [Tray, notifications and startup](Tray-notifications-and-startup).

*<kbd>Ctrl</kbd>+<kbd>G</kbd> and the Setup Guide are new in 1.1.62, and the Files tab's filter box in 1.1.56; neither is in a published release yet.*

Mouse shortcuts that go with them:

- Double-clicking a torrent that has finished opens its folder; double-clicking one that has not opens **Properties**.
- In the Files tab, double-clicking a file or pressing <kbd>Enter</kbd> on it opens it.

## The search box and the Files tab filter

| Keys | In the toolbar's search box | In the Files tab's filter box |
|---|---|---|
| <kbd>Escape</kbd> | Empties the box and moves to the torrent list | Empties the box and moves to the file list |
| <kbd>Enter</kbd> | With **Filter torrents** chosen, moves to the list. With a search source chosen, searches it. | Moves to the file list |
| <kbd>Ctrl</kbd>+<kbd>A</kbd> | Selects the box's text | Selects the box's text (so does <kbd>Ctrl</kbd>+<kbd>F</kbd>) |
| <kbd>Alt</kbd>+<kbd>Down</kbd> | Opens the box's drop-down menu (sources, **Manage Sources...**) | |

While you type in either box, <kbd>Del</kbd> and <kbd>Ctrl</kbd>+<kbd>A</kbd> edit the text, not the list. See [Searching for torrents](Searching-for-torrents) and [Torrent details and files](Torrent-details-and-files).

## Add New Torrent window

In the file list of the Add New Torrent window:

| Keys | What they do |
|---|---|
| <kbd>Space</kbd> | Ticks or unticks the selected files and folders |
| <kbd>Right</kbd> or <kbd>+</kbd> | Opens the focused folder |
| <kbd>Left</kbd> or <kbd>-</kbd> | Closes the focused folder; on a file or a closed folder, <kbd>Left</kbd> goes to its parent folder |
| <kbd>Enter</kbd> | Opens or closes the focused folder |

See [Adding torrents](Adding-torrents).

## Preferences and other windows

| Keys | What they do |
|---|---|
| <kbd>F1</kbd> in Preferences | Shows the help of the focused setting beside it, and in the help pane under the pages |
| <kbd>Escape</kbd> | Closes a dialog like **Cancel**. In the Setup Guide it is **Skip**. |
| <kbd>Tab</kbd>, <kbd>Enter</kbd>, <kbd>Escape</kbd> in the Search Torrents window | Move between its controls, act on the focused one, close it |

The help in Preferences also appears as a tooltip when you point at a setting. In the main window, a menu item or toolbar button you point at is described in the status bar. *These help texts are new in 1.1.57; not in a published release yet.*

See [Preferences reference](Preferences-reference).

## Menu keys

Every menu can be reached with <kbd>Alt</kbd> and the underlined letter, as in other Windows programs: <kbd>Alt</kbd>+<kbd>F</kbd> for **File**, <kbd>Alt</kbd>+<kbd>O</kbd> for **Options**, <kbd>Alt</kbd>+<kbd>H</kbd> for **Help**, then the underlined letter of an item.

## GTK version (Linux, BSD, macOS)

The GTK version has these shortcuts:

| Keys | What they do |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>O</kbd> | **Add Torrent...** |
| <kbd>Ctrl</kbd>+<kbd>U</kbd> | **Add Torrent from URL...** |
| <kbd>Ctrl</kbd>+<kbd>N</kbd> | **Create New Torrent...** |
| <kbd>Ctrl</kbd>+<kbd>F</kbd> | Moves to the search box |
| <kbd>Ctrl</kbd>+<kbd>P</kbd> | **Preferences** |
| <kbd>Ctrl</kbd>+<kbd>G</kbd> | **Setup Guide** |
| <kbd>Ctrl</kbd>+<kbd>Q</kbd> | **Exit**: quits PaTorrent, also when the window would close to the tray |
| <kbd>Delete</kbd> | **Remove** |
| <kbd>Shift</kbd>+<kbd>Delete</kbd> | Remove and delete the data |
| <kbd>Ctrl</kbd>+<kbd>A</kbd> | Selects every torrent |
| <kbd>F1</kbd> in Preferences | Shows the focused setting's help |
| <kbd>Escape</kbd> | Empties the search box or the Files tab's filter; closes dialogs |

What differs from Windows:

- There is no shortcut for Start, Pause, Stop or Properties. Use the toolbar or the right-click menu.
- <kbd>Ctrl</kbd>+<kbd>A</kbd> does not select all files in the Files tab.
- <kbd>Ctrl</kbd>+<kbd>Q</kbd> quits; on Windows there is no quit shortcut.
- On macOS the shortcuts use the <kbd>Control</kbd> key, not <kbd>Command</kbd>.
- The Add New Torrent window's keys are the same as on Windows.

## Web UI

In the Web UI's torrent list (click the list first so it has the keyboard):

| Keys | What they do |
|---|---|
| <kbd>Up</kbd>, <kbd>Down</kbd>, <kbd>Page Up</kbd>, <kbd>Page Down</kbd>, <kbd>Home</kbd>, <kbd>End</kbd> | Move the selection |
| <kbd>Shift</kbd> with those keys | Extends the selection |
| <kbd>Ctrl</kbd> with those keys | Moves the focus without changing the selection |
| <kbd>Ctrl</kbd>+<kbd>Space</kbd> | Adds the focused row to the selection, or takes it out |
| <kbd>Space</kbd> | **Pause** the selected torrents |
| <kbd>Delete</kbd> | **Remove** |
| <kbd>Shift</kbd>+<kbd>Delete</kbd> | Remove and delete the data |
| <kbd>Ctrl</kbd>+<kbd>A</kbd> | Selects every torrent |
| <kbd>Shift</kbd>+<kbd>F10</kbd> or the menu key | Opens the right-click menu |
| <kbd>Escape</kbd> | Closes an open menu |

Elsewhere in the Web UI:

| Keys | What they do |
|---|---|
| <kbd>Ctrl</kbd>+<kbd>P</kbd> | Opens **Preferences** |
| <kbd>Escape</kbd> in the filter box | Empties it and moves to the list |
| <kbd>Enter</kbd> in the filter box | Moves to the list |
| <kbd>F1</kbd> in Preferences | Shows the focused setting's help |
| <kbd>Ctrl</kbd>+<kbd>Enter</kbd> in Preferences | **OK** |
| <kbd>Enter</kbd> in the Advanced page's value field | Sets the value |
| <kbd>Escape</kbd> in Preferences | Closes it like **Cancel** |

On a Mac, <kbd>Command</kbd> works wherever the Web UI says <kbd>Ctrl</kbd>. Your browser's own shortcuts may take some keys first. See [Web UI](Web-UI).
