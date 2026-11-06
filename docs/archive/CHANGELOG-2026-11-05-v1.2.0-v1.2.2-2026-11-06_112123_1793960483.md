# Changelog archive: 2026-11-05, v1.2.0-v1.2.2

Moved from the root CHANGELOG.md on 2026-11-06_112123_1793960483. Paths and links keep their repository-root context.

## 2026-11-05_220518_1793912718: Rookparcel artwork exploration

- Saved 12 PNG concept sheets covering rook, parcel, bird, monogram and mascot directions, with exact prompts in [the artwork index](docs/art/rookparcel-candidates-2026-11-05/README.md).
- Exploration only: no identity selected or production icon changed. Verified all 12 PNG files and the prompt index are present; no build or tests needed for these assets. Next: select and refine a candidate.

## 2026-11-05_220414_1793912654: Version 1.2.2: PaTorrent is renamed Rookparcel (D59)

Entries below this one keep the old name PaTorrent.

- **Renamed** everywhere outside history (`docs/archive`, `docs/releases`, older entries here, the icon art): crates `rookparcel-*`, binaries `rookparcel(.exe)`, `rookparcel-nox`, `rookparcel-gtk`, app id `io.github.balls0fsteel.Rookparcel`, packaging and contrib files, wiki page names, `ROOKPARCEL_*` variables, the 35 language files (Korean particles adjusted for a consonant-final name; the About text loses the PatinaTorrent long name). Peer ID `-RP1014-` (`PA` stays in the client table as PaTorrent), User-Agent and BEP 10 `v` `Rookparcel/…`, remote protocol 16, the ping probe still 32 bytes.
- **Takeover of a PaTorrent install** (`crates/frontend/src/legacy.rs`): the old default roots (`%APPDATA%\PaTorrent`, XDG `patorrent`, Application Support) are renamed once at start, with `patorrent.ini` renamed in every settings folder and paths into the old root rewritten; a portable `patorrent.ini` is taken over in place. A running PaTorrent blocks it: `lockfile` holds `patorrent.lock` too, and the start says the folder is in use. The registry's ProgIDs, Default apps entry and per-profile `Run` values (`integration::adopt_patorrent`), GTK's desktop and autostart entries and macOS's LaunchAgent move to the new names; the setup removes an installed PaTorrent; `service install` replaces a `PaTorrentNox` service and moves `%ProgramData%\PaTorrent`.
- **Still read:** the `patorrent resume db` tag, `PATORRENT_LANG`/`_BUILD`/`_NO_UPDATE_CHECK`/`_STORE`, `X-Requested-With: PaTorrent`, `patorrent-search:` URLs, a PaTorrent GUI's remote upgrade (answered with the version); the DPAPI entropy keeps its bytes. The site lists old- and new-named release assets. No update bridge (owner): 1.2.x installs are not told.
- Docs: D59, README, DAEMON.md "Coming from PaTorrent", RELEASING.md's first-release steps (repository rename, wiki push, registering `RP`).
- **Upgrade checked on Windows** (scratch `APPDATA`, real 1.2.1 `patorrent-nox` from `cdb0578d`, then 1.2.2's): 16/16 checks. A running PaTorrent makes Rookparcel exit 1 with nothing moved; afterwards the folder moves, `patorrent.ini` is renamed with its paths following, no old lock is left, both the default and a `work` profile's torrents come back, a second start moves nothing, and a portable folder is taken over in place without touching `APPDATA`. Its library was in the journal only, so the old `resume.db` tag rests on the unit test. Not run: the GUIs on a real install (the registry takeover has a scratch-key test), the setup over an installed PaTorrent, Linux, macOS.

## 2026-11-05_205340_1793908420: Version 1.2.1: a login banned meanwhile is answered "banned"

- **Fixed:** `check_login` (`crates/nox/src/webui/api.rs`) returns `Login::{Ok, Failed, Banned}`. A login refused because another guess got the address banned meanwhile is answered 403 with the ban text instead of "Fails."; a remote-session login in that case gets 403 instead of 401 (the client already shows 403's text). `login`'s separate ban check before it is gone, so the check and the count happen under one lock.
- Found by macOS CI (`parallel_guesses_stop_at_the_ban_limit` counted 6 "Fails."); the ban itself always held. Clippy (nox, Win32), all nox tests on one thread at Idle priority, and that test 10 times in a row pass. BACKLOG item removed.

## 2026-11-05_183712_1793900232: Release 1.2.0 (1.1.41-1.1.78)

- **1.2.0** ships 1.1.41-1.1.78 (D58): profiles and portable mode everywhere (D42), the Files filter (D50), the search helper (D51), the Setup Guide (D53), "Use only this network" (D54), help texts (D55), the 2026-11-05 bug hunt (37 of 38 fixed, one ruled out) and the backlog fixes. Release notes: [docs/releases/1.2.0.md](docs/releases/1.2.0.md); Flatpak metainfo `<release>` added.
- Version bumped to 1.2.0. Peer ID `-PA1013-` (owner, 2026-11-05_183712_1793900232) for the wire changes of 1.1.66, 1.1.70, 1.1.74 and 1.1.76. The remote protocol is 15 (14 since 1.1.58, 15 since 1.1.70).
- **Published 2026-11-05_204405_1793907845:** CI, FreeBSD and Fuzz green on 8e613e30 (one fuzz job hung in the runner's `apt-get` and was rerun); the draft's 14 files built, checksums and `patorrent-nox --version` checked, the text matches the notes. The website (FAQ and download text from `site-profiles`, the `site/news.html` entry) went out right after; `update.txt` says `stable=1.2.0`.
- **Issue:** macOS CI on the website push failed `parallel_guesses_stop_at_the_ban_limit`: a guess banned between `login`'s ban check and `check_login`'s is refused but answered "Fails.". The ban holds; logged in BACKLOG "Validation" with the fix.
