//! The nox Web UI's request parsing: request heads, then the body as a urlencoded form, a
//! multipart form (with the boundary from its own Content-Type), cookies and JSON
//! (`setPreferences`), as a request from the network would reach them.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_nox::webui::http::{
    Limits, cookie, multipart_boundary, parse_head, parse_multipart, parse_urlencoded, percent_decode,
};
use patorrent_nox::webui::json;

fuzz_target!(|data: &[u8]| {
    let limits = Limits { max_head: 4096, max_headers: 32, max_body: 1 << 20, max_message: 64 * 1024 };
    let Ok(Some((head, used))) = parse_head(data, &limits) else {
        // not a request head: still exercise the body parsers on the raw bytes
        let _ = parse_urlencoded(data);
        let _ = percent_decode(data, true);
        if let Ok(s) = std::str::from_utf8(data) {
            if let Ok(v) = json::parse(s) {
                assert_eq!(json::parse(&v.to_json()).ok(), Some(v));
            }
        }
        return;
    };
    assert!(used <= data.len());
    let body = &data[used..];
    let _ = parse_urlencoded(head.query.as_bytes());
    let _ = parse_urlencoded(body);
    if let Some(c) = head.header("cookie") {
        let _ = cookie(c, "SID");
    }
    if let Some(b) = head.header("content-type").and_then(multipart_boundary) {
        if let Ok(parts) = parse_multipart(body, &b) {
            for p in parts {
                assert!(p.data.len() <= body.len());
            }
        }
    }
    if let Ok(s) = std::str::from_utf8(body) {
        let _ = json::parse(s);
    }
});
