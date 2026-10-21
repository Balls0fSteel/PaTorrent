//! UI-agnostic client logic shared by every PaTorrent frontend (Win32, GTK, nox).
//!
//! Anything that decides *what* the user sees or *what* a user action means lives here, so the
//! frontends only decide *how* it is drawn:
//!
//! * [`lang`]: the string tables: every user-visible string by key, per language ([`tr!`]).
//! * [`fmt`]: µTorrent-style number formatting (sizes, speeds, ETA, ratio, percent).
//! * [`status`]: status text ("[F] Downloading", "Queued Seed", ...) and category filters.
//! * [`add`]: the Add New Torrent dialog's file tree, choices and recent save folders.
//! * [`actions`]: µTorrent's Start / Force Start / Pause / Stop on top of libtorrent's flags.
//! * [`columns`]: the torrent list's columns (titles, widths, cell text, sort order).
//! * [`detail`]: the Trackers, Peers and Files tabs' columns and cell text.
//! * [`model`]: the filtered, sorted torrent list behind a virtual list view.
//! * [`prefs`]: a small `key=value` preferences file for layout and frontend options.
//! * [`settings`]: the engine settings by libtorrent name (the Advanced preferences table).
//! * [`options`]: what the Preferences dialog edits, loaded from and stored to the preferences.
//! * [`scheduler`]: the weekly scheduler (table, modes, what to change when the hour turns).
//! * [`torrent_folders`]: the `.torrent` folders (stored copies, finished jobs, auto-load).
//! * [`rss`]: RSS / Atom feeds and the RSS Downloader (filters, episodes, history, `rss.dat`).
//! * [`properties`]: the Torrent Properties dialog (trackers, limits, seeding goal, switches).
//! * [`seeding`]: the seeding goal: when a queued seed is stopped or held to an upload rate.
//! * [`auto_shutdown`]: Options > Auto Shutdown (quit, standby, hibernate or shut down when done).
//! * [`scrape`]: the periodic scrape that gives stopped and queued torrents their swarm counts.

pub mod actions;
pub mod add;
pub mod auto_shutdown;
pub mod columns;
pub mod create;
pub mod detail;
pub mod fmt;
pub mod icons;
pub mod integration;
pub mod lang;
pub mod model;
pub mod options;
pub mod pref_pages;
pub mod prefs;
pub mod properties;
pub mod rss;
pub mod scheduler;
pub mod scrape;
pub mod seeding;
pub mod settings;
pub mod status;
pub mod torrent_folders;
