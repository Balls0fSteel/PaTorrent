//! UI-agnostic client logic shared by every LibreTorrent frontend (Win32, GTK, nox).
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

pub mod actions;
pub mod columns;
pub mod detail;
pub mod fmt;
pub mod model;
pub mod prefs;
pub mod status;
