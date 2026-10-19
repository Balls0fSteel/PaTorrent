//! Downloads a `.torrent` named by an `http://` or `https://` URL for `torrents/add`, through
//! the engine's blocking fetch helper (`patorrent_core::fetch`: redirects, chunked bodies, OS
//! TLS).

use patorrent_core::fetch::{FetchOptions, http_get};

use super::http::Limits;

pub enum Fetched {
    Torrent(Vec<u8>),
    /// The URL redirected to a magnet link (indexers do this).
    Magnet(String),
}

pub fn fetch(url: &str) -> Result<Fetched, String> {
    if url.starts_with("magnet:") {
        return Ok(Fetched::Magnet(url.to_owned()));
    }
    let opts = FetchOptions { max_size: Limits::default().max_body, ..FetchOptions::default() };
    let resp = http_get(url, &opts).map_err(|e| e.to_string())?;
    match resp.redirect() {
        Some(m) if m.starts_with("magnet:") => Ok(Fetched::Magnet(m.to_owned())),
        Some(other) => Err(format!("{other}: unsupported redirect")),
        None => Ok(Fetched::Torrent(resp.body)),
    }
}
