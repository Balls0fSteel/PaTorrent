//! UI-agnostic client logic shared by every PaTorrent frontend (Win32, GTK, nox).
//!
//! Anything that decides *what* the user sees or *what* a user action means lives here, so the
//! frontends only decide *how* it is drawn:
//!
//! * [`fmt`]: µTorrent-style number formatting (sizes, speeds, ETA, ratio, percent).
//! * [`status`]: status text ("[F] Downloading", "Queued Seed", ...) and category filters.
//! * [`actions`]: µTorrent's Start / Force Start / Pause / Stop on top of libtorrent's flags.
//! * [`columns`]: the torrent list's columns (titles, widths, cell text, sort order).
//! * [`detail`]: the Trackers, Peers and Files tabs' columns and cell text.
//! * [`model`]: the filtered, sorted torrent list behind a virtual list view.
//! * [`prefs`]: a small `key=value` preferences file for layout and frontend options.
//! * [`settings`]: the engine settings by libtorrent name (the Advanced preferences table).
//! * [`options`]: what the Preferences dialog edits, loaded from and stored to the preferences.

pub mod actions;
pub mod columns;
pub mod create;
pub mod detail;
pub mod fmt;
pub mod model;
pub mod options;
pub mod prefs;
pub mod settings;
pub mod status;
