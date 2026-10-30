//! Torrent search through the user's Prowlarr or Jackett over Torznab (D41,
//! `docs/INTEGRATIONS.md` part 2), shared by the frontends: no search plugins, no scrapers.
//!
//! - [`source`]: the sources (Torznab endpoints) in the preferences, their sealed API keys and
//!   [`redact`], which keeps a key out of every message.
//! - [`torznab`]: request URLs, answers turned into [`SearchResult`]s, error documents and the
//!   caps' categories.
//! - [`worker`]: the merged search over the enabled sources ([`Search`]) and what adding a
//!   result adds ([`fetch_download`]).

pub mod source;
pub mod torznab;
pub mod worker;

pub use source::{Source, load_sources, redact, remove_source, store_sources};
pub use torznab::{Category, SearchError, SearchResult, parse_caps, parse_results};
pub use worker::{Query, Search, SearchSource, SourceStatus, fetch_caps, fetch_download, merge, proxy_for};
