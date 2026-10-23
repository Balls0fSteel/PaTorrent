//! Downloads a `.torrent` named by an `http://` or `https://` URL for `torrents/add` and the
//! native add, through the engine's blocking fetch helper (`patorrent_core::fetch`: redirects,
//! chunked bodies, OS TLS) and the session's proxy when one is set.

use patorrent_core::fetch::{FetchOptions, FetchProxy, http_get};

use super::http::Limits;

pub enum Fetched {
    Torrent(Vec<u8>),
    /// The URL redirected to a magnet link (indexers do this).
    Magnet(String),
}

pub fn fetch(url: &str, proxy: Option<FetchProxy>) -> Result<Fetched, String> {
    if url.starts_with("magnet:") {
        return Ok(Fetched::Magnet(url.to_owned()));
    }
    let opts = FetchOptions { max_size: Limits::default().max_body, proxy, ..FetchOptions::default() };
    let resp = http_get(url, &opts).map_err(|e| e.to_string())?;
    match resp.redirect() {
        Some(m) if m.starts_with("magnet:") => Ok(Fetched::Magnet(m.to_owned())),
        Some(other) => Err(format!("{other}: unsupported redirect")),
        None => Ok(Fetched::Torrent(resp.body)),
    }
}
