//! A fake Torznab indexer for the search tests (`search.rs`, `hosted.rs`), shaped like
//! Prowlarr: its answers are the ones captured from Prowlarr (`frontend/src/search/fixtures`),
//! with the download links pointing back at this server.
//!
//! - `/1/api`: `t=caps` the caps, else the three results; 401 without `apikey=KEY`.
//! - `/9/api`: a Torznab error document with HTTP 200 (Jackett's way).
//! - `/slow/api`: the results after 3 s.
//! - `/2/download?...&file=<title>`: Fawlty Towers is a `.torrent`, Night of the Living Dead a
//!   302 to a magnet, anything else 404; 401 without the key.

#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use patorrent_core::bencode::Entry;
use patorrent_core::crypto::Sha1;

/// The indexer's API key.
pub const KEY: &str = "S3cretK3y0123456789abcdef";
pub const FAWLTY: &str = "Fawlty.Towers.S01E01.720p.WEB-DL.x264-PATEST";
pub const NIGHT: &str = "Night.of.the.Living.Dead.1968.720p.WEB-DL.x264-PATEST";
pub const GRAB: &str = "PaTorrent.Prowlarr.Grab.Test.x264-PATEST";

const RESULTS: &str = include_str!("../../../frontend/src/search/fixtures/prowlarr-results.xml");
const CAPS: &str = include_str!("../../../frontend/src/search/fixtures/prowlarr-caps.xml");

/// A one-file torrent named `name` and its info-hash.
pub fn torrent(name: &str) -> (Vec<u8>, String) {
    let data = name.as_bytes().repeat(100);
    let mut info = Entry::new_dict();
    *info.key("length") = Entry::Int(data.len() as i64);
    *info.key("name") = Entry::Bytes(name.as_bytes().to_vec());
    *info.key("piece length") = Entry::Int(16384);
    *info.key("pieces") = Entry::Bytes(Sha1::digest(&data).0.to_vec());
    let hash = Sha1::digest(&info.bencode()).to_hex();
    let mut t = Entry::new_dict();
    *t.key("info") = info;
    (t.bencode(), hash)
}

/// The magnet Night of the Living Dead's link redirects to, and its hash.
pub fn night_magnet() -> (String, String) {
    let hash = torrent("night").1;
    (format!("magnet:?xt=urn:btih:{hash}&dn=Night"), hash)
}

pub struct Fake {
    pub addr: SocketAddr,
    /// Every request line's target, as received.
    pub requests: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    pub fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }
}

fn reply(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut r = format!("HTTP/1.0 {status}\r\n{headers}Content-Length: {}\r\n\r\n", body.len()).into_bytes();
    r.extend_from_slice(body);
    r
}

pub fn start() -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let results = RESULTS.replace("http://localhost:9696", &format!("http://{addr}")).replace("APIKEY", KEY);
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let results = results.clone();
            let seen = seen.clone();
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).into_owned();
                let target = head.split_whitespace().nth(1).unwrap_or("").to_owned();
                seen.lock().unwrap().push(target.clone());
                let (path, query) = target.split_once('?').unwrap_or((&target, ""));
                let keyed = query.split('&').any(|kv| kv == format!("apikey={KEY}"));
                let out = match path {
                    "/1/api" | "/slow/api" if !keyed => reply("401 Unauthorized", "", b""),
                    "/1/api" if query.contains("t=caps") => reply("200 OK", "", CAPS.as_bytes()),
                    "/1/api" => reply("200 OK", "", results.as_bytes()),
                    "/slow/api" => {
                        std::thread::sleep(Duration::from_secs(3));
                        reply("200 OK", "", results.as_bytes())
                    }
                    "/9/api" => reply(
                        "200 OK",
                        "",
                        br#"<?xml version="1.0" encoding="UTF-8"?><error code="100" description="Invalid API Key"/>"#,
                    ),
                    "/2/download" if !keyed => reply("401 Unauthorized", "", b""),
                    "/2/download" if query.contains(&format!("file={FAWLTY}")) => {
                        reply("200 OK", "", &torrent("fawlty").0)
                    }
                    "/2/download" if query.contains(&format!("file={NIGHT}")) => {
                        reply("302 Found", &format!("Location: {}\r\n", night_magnet().0), b"")
                    }
                    _ => reply("404 Not Found", "", b""),
                };
                let _ = s.write_all(&out);
            });
        }
    });
    Fake { addr, requests }
}
