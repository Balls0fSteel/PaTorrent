# Updating Rookparcel

Rookparcel checks for new versions by itself and tells you when one is out. This page covers the check, how an installed Windows copy updates itself, updating the zip, Linux and macOS copies by hand, beta versions, turning the check off, and `rookparcel-nox`.

## The update check

About ten seconds after Rookparcel starts, and then every 12 hours while it runs, it asks the project website which version is the newest. This is one small HTTPS request for a short list of version numbers. Nothing about your computer, your torrents or your settings is sent, and nothing is downloaded or installed without your yes. If Rookparcel uses a proxy (**Options > Preferences > Connection**), the check goes through it too.

When a newer version is out:

- A question box appears, once for each new version.
- After that, a notice stays in the status bar until you click it: "Rookparcel 1.1.63 is available - click here to download it".
- The Logger tab has a line with the link too.

To check at once, choose **Help > Check for Updates**. It answers either way, with the newer version or with "You have the newest version".

## An installed copy on Windows updates itself

A Rookparcel installed by the Windows setup can install the new version itself. *New in 1.1.40.*

1. The question reads "Rookparcel 1.1.63 is available. You have 1.1.40. Download and install it now?". Click **Download and install**. (**Open download page** opens the release page in your browser instead; **Cancel** does nothing.)
2. A small window shows the download's progress. Rookparcel checks the downloaded setup against the release's `SHA256SUMS` before it uses it.
3. Rookparcel asks: "To install it, Rookparcel closes now and starts again when Setup is done. Close and install now?". Click **Yes**.
4. Rookparcel saves everything and closes. The setup installs the new version silently into the same folder and starts Rookparcel again.

Your settings, torrents and associations stay as they were. If the installation fails, the old version stays and starts again.

For an installation for everyone (in `C:\Program Files`), Windows asks for administrator rights while the setup runs.

**Updating from 1.1.17 or older:** those versions cannot install updates; their question opens the download page. Download the new setup and run it once by hand, as in [Installing Rookparcel](Installing-Rookparcel#installing-over-an-older-version). From then on, Rookparcel updates itself.

## Updating by hand

### Windows installer

Download the new `rookparcel-<version>-windows-x64-setup.exe` and run it. It finds the installed Rookparcel and installs over it, keeping the folder, the mode (for you or for everyone), the settings and the torrents. Exit Rookparcel first, or the setup asks you to. See [Installing Rookparcel](Installing-Rookparcel#installing-over-an-older-version).

### Windows zip

A zip copy does not update itself. Its question offers to open the download page.

1. Download the new zip.
2. Exit Rookparcel (**File > Exit**).
3. Unzip the new `rookparcel.exe` and `rookparcel-nox.exe` over the old ones.
4. Start Rookparcel.

Keep `rookparcel.ini` in the folder if the copy is portable. The settings and torrents are not in the zip, so nothing of yours is overwritten.

### Linux and macOS

The GTK version's question opens the release page.

- **AppImage:** download the new `.AppImage`, make it executable (`chmod +x`), and start it in place of the old one. Delete the old file. Settings and torrents are not inside the AppImage, so they carry over. For a portable AppImage, put the new file beside `rookparcel.ini`.
- **tar.gz:** exit Rookparcel and unpack the new binaries over the old ones.
- **Flatpak:** the Flatpak has no update check. Download the newer bundle from the [Releases page](https://github.com/Balls0fSteel/Rookparcel/releases) and install it as in [Installing Rookparcel](Installing-Rookparcel#flatpak).
- **macOS:** quit Rookparcel, then drag the new `Rookparcel.app` to Applications and replace the old one. If Gatekeeper refuses the new copy, open it as in [Installing Rookparcel](Installing-Rookparcel#macos).

## Beta versions

Rookparcel sometimes publishes a pre-release (a release candidate) before a version. To be offered those too, tick **Update to beta versions** in **Options > Preferences > General**, under **Privacy**. It is off by default.

A pre-release always hears of the next pre-release, whatever the box says.

## Turning the check off

Untick **Check for updates automatically** in **Options > Preferences > General**, under **Privacy**. Rookparcel then never asks the website by itself. **Help > Check for Updates** still works when you choose it.

Some builds have no update check at all: the Flatpak, and packages built for a package manager that does the updating.

## rookparcel-nox

`rookparcel-nox` checks the same way, ten seconds after `run` starts and every 12 hours. *New in 1.1.17.* It never downloads or installs anything. When a newer version is out:

- it prints a line, for example `* Rookparcel 1.2.0 is available: https://github.com/Balls0fSteel/Rookparcel/releases/tag/v1.2.0`;
- its Web UI shows the line in its Logger and a link in the page's status bar.

To turn it off, start it with `--no-update-check`, or untick **Check for updates automatically** in the Web UI's **General > Privacy**. **Update to beta versions** is there too.

To update the daemon, stop it, replace `rookparcel-nox` with the new one and start it again. For the Windows service, stop it first from an administrator prompt (`sc stop RookparcelNox`) and start it again afterwards; the Windows installer asks for this when it finds the service running. See [Running rookparcel-nox](Running-rookparcel-nox).

## After updating

- **Remote control:** when a release changes the remote protocol, a window and the `rookparcel-nox` it drives must both be on that release. Update both. The release notes say when this applies ([Remote control](Remote-control)).
- **Going back to an older version:** settings and torrents carry over between 1.1.x versions. 1.1.40 stores resume data in a smaller form, so going back from it to an older version rechecks each torrent once. A [portable folder](Where-Rookparcel-keeps-its-files#portable-mode) used by 1.1.49 or later needs 1.1.49 or later.
- What changed in each release: the release notes on the [Releases page](https://github.com/Balls0fSteel/Rookparcel/releases), or the website's [News](https://balls0fsteel.github.io/Rookparcel/news.html) page.
