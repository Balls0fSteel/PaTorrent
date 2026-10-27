// Ported from libtorrent-rasterbar 2.0.11: src/web_peer_connection.cpp (constructor URL
// handling, write_request, on_receive, incoming_payload, incoming_zeroes, maybe_harvest_piece,
// handle_padfile, the restart data of disconnect, handle_error / handle_redirect up to their
// torrent calls, get_range), src/web_connection_base.cpp (add_headers),
// src/http_seed_connection.cpp (write_request and the response checks of on_receive)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: sans-IO. Requests are bytes in `out`, received bytes go in through `feed`, and what
// libtorrent does to the torrent (incoming_piece, retry_web_seed, add_web_seed, disable) comes
// out as `WebEvent`s for the engine. The Host header carries a non-default port (libtorrent
// sends the bare host name).

//! One web seed connection's HTTP side. BEP 19 (GetRight style, `url-list`): block requests
//! become `Range` requests on the file URLs. BEP 17 (Hoffman style, `httpseeds`): they become
//! `?info_hash=..&piece=..&ranges=..` requests to the seed's script. Either way the response
//! bodies become blocks again.

use std::collections::{HashMap, VecDeque};

use super::http_parser::{
    Chunk, MAX_CHUNK_HEADER, ParseError, ResponseHead, is_ok_status, is_redirect, parse_chunk_header,
    parse_head, resolve_redirect_location, split_url,
};
use crate::disk::BLOCK_SIZE;
use crate::metainfo::WebSeedKind;
use crate::metainfo::file_storage::FileStorage;
use crate::types::Sha1Hash;
use crate::util::string::{
    UrlError, base64encode, ensure_trailing_slash, escape_path, escape_string, parse_url_components,
};

/// A block-sized request (libtorrent `peer_request`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BlockRequest {
    pub piece: u32,
    pub start: u32,
    pub length: u32,
}

/// A block interrupted by a disconnect, kept on the web seed for the next connection
/// (libtorrent `web_seed_t::restart_request` / `restart_piece`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restart {
    pub request: BlockRequest,
    pub data: Vec<u8>,
}

/// libtorrent `web_peer_connection::file_request_t`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileRequest {
    file: usize,
    start: u64,
    length: u64,
}

/// What a received response asks the engine to do. Everything but a block or the keep-alive
/// note ends the connection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WebEvent {
    /// A whole block (libtorrent `incoming_piece`).
    Block { piece: u32, start: u32, data: Vec<u8> },
    /// The first response said `Connection: close` (libtorrent `supports_keepalive = false`).
    NoKeepAlive,
    /// A status other than 200, 206 or 3xx (libtorrent `handle_error`). `file` is the file of
    /// the request it answered, which the server does not have.
    HttpError { status: i32, message: String, retry_after: Option<u32>, file: Option<usize> },
    /// A 3xx with a location (libtorrent `handle_redirect`). For a multi-file torrent `file`
    /// was redirected to `path` on the server `base`; for a single file, the whole web seed
    /// moved to `base` (`path` is empty).
    Redirect { base: String, path: String, file: Option<usize> },
    /// The server cannot be used (libtorrent `disable`): never try it again this session.
    Disable(&'static str),
    /// The response is broken (libtorrent `disconnect` with `peer_error` or `failure`).
    Fail(&'static str),
}

/// libtorrent `escape_file_path`: the file's path in the torrent (with the torrent name for
/// multi-file torrents), percent-encoded except for '/'.
pub fn escape_file_path(fs: &FileStorage, file: usize) -> String {
    escape_path(fs.file_path(file, "").as_bytes())
}

/// The HTTP side of one web seed connection.
pub struct WebPeer {
    host: String,
    host_header: String,
    /// The request path (already escaped): the file's URL for a single file, the directory
    /// with a trailing '/' for a multi-file torrent.
    path: String,
    /// The full URL, the base for relative redirects.
    url: String,
    basic_auth: String,
    external_auth: String,
    extra_headers: Vec<(String, String)>,
    user_agent: String,
    always_send_user_agent: bool,
    single_file: bool,
    /// BEP 17: the info-hash every request names.
    http_seed: Option<Sha1Hash>,
    first_request: bool,
    /// `http://host[:port]`: the requests go to an HTTP proxy, with absolute URLs.
    proxy_origin: Option<String>,
    /// Basic credentials for the HTTP proxy.
    proxy_auth: Option<String>,

    /// Block requests not answered yet, in order (libtorrent `m_requests`).
    requests: VecDeque<BlockRequest>,
    /// The HTTP requests they turned into (libtorrent `m_file_requests`).
    file_requests: VecDeque<FileRequest>,
    /// The front block received so far (libtorrent `m_piece`).
    piece: Vec<u8>,

    buf: Vec<u8>,
    head: Option<ResponseHead>,
    received_body: u64,
    /// Payload bytes left in the current chunk.
    chunk_pos: u64,
    /// Bytes of an incomplete chunk header already seen without a newline in them: only a new
    /// newline can complete it, so the header is parsed again only then (SEC-22).
    chunk_scanned: usize,
    /// How often a chunk header was parsed.
    #[cfg(test)]
    chunk_parses: usize,
    num_responses: u32,
    server: String,
    /// The server closes the connection after the current response (libtorrent
    /// `incoming_choke` on `Connection: close`): send no more requests.
    pub choked: bool,
    done: bool,
    out: Vec<u8>,
    events: Vec<WebEvent>,
}

impl WebPeer {
    /// libtorrent `web_connection_base` and `web_peer_connection` (or `http_seed_connection`)
    /// constructors. `info_hash` is only used by BEP 17 seeds.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        url: &str,
        kind: WebSeedKind,
        info_hash: Sha1Hash,
        external_auth: &str,
        extra_headers: &[(String, String)],
        fs: &FileStorage,
        user_agent: &str,
        always_send_user_agent: bool,
    ) -> Result<WebPeer, UrlError> {
        let u = parse_url_components(url)?;
        let https = u.protocol.eq_ignore_ascii_case("https");
        let port = u.port.unwrap_or(if https { 443 } else { 80 });
        let bracket = if u.host.contains(':') { format!("[{}]", u.host) } else { u.host.clone() };
        let default_port = port == if https { 443 } else { 80 };
        let host_header = if default_port { bracket } else { format!("{bracket}:{port}") };
        let single_file = fs.num_files() == 1;
        let mut path = u.path;
        let mut url = url.to_owned();
        let http_seed = (kind == WebSeedKind::HttpSeed).then_some(info_hash);
        if http_seed.is_some() {
            // the script's URL is used as is
        } else if !single_file {
            // handle incorrect .torrent files which are multi-file but have web seeds not
            // ending with a slash
            ensure_trailing_slash(&mut path);
            ensure_trailing_slash(&mut url);
        } else {
            // handle .torrent files that don't include the filename in the url
            if path.ends_with('/') {
                path.push_str(&escape_string(fs.name().as_bytes()));
            }
            if url.ends_with('/') {
                url.push_str(&escape_file_path(fs, 0));
            }
        }
        Ok(WebPeer {
            host: u.host,
            host_header,
            path,
            url,
            basic_auth: if u.auth.is_empty() { String::new() } else { base64encode(u.auth.as_bytes()) },
            external_auth: external_auth.to_owned(),
            extra_headers: extra_headers.to_vec(),
            user_agent: user_agent.to_owned(),
            always_send_user_agent,
            single_file,
            http_seed,
            first_request: true,
            proxy_origin: None,
            proxy_auth: None,
            requests: VecDeque::new(),
            file_requests: VecDeque::new(),
            piece: Vec::new(),
            buf: Vec::new(),
            head: None,
            received_body: 0,
            chunk_pos: 0,
            chunk_scanned: 0,
            #[cfg(test)]
            chunk_parses: 0,
            num_responses: 0,
            server: String::new(),
            choked: false,
            done: false,
            out: Vec::new(),
            events: Vec::new(),
        })
    }

    /// The URL requests go to (the file's URL for a single-file torrent).
    pub fn url(&self) -> &str {
        &self.url
    }

    /// The `Server` header of the last response, or the host name once a response arrived
    /// (libtorrent `m_server_string`, shown as the peer's client).
    pub fn server(&self) -> &str {
        &self.server
    }

    /// Block requests not answered yet.
    pub fn requests(&self) -> impl Iterator<Item = &BlockRequest> {
        self.requests.iter()
    }

    pub fn num_requests(&self) -> usize {
        self.requests.len()
    }

    /// HTTP requests not answered yet.
    #[cfg(test)]
    pub fn num_file_requests(&self) -> usize {
        self.file_requests.len()
    }

    /// Request bytes to send. The caller removes what it wrote.
    pub fn out(&mut self) -> &mut Vec<u8> {
        &mut self.out
    }

    pub fn take_events(&mut self) -> Vec<WebEvent> {
        std::mem::take(&mut self.events)
    }

    /// A terminal event was produced: nothing more is parsed.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// The requests go to an HTTP proxy (libtorrent `using_proxy`): absolute URLs, the
    /// proxy's credentials, and keep-alive asked of the proxy too. Only for `http://` seeds;
    /// `https://` ones tunnel through CONNECT.
    pub fn set_http_proxy(&mut self, creds: Option<(&str, &str)>) {
        self.proxy_origin = Some(format!("http://{}", self.host_header));
        self.proxy_auth = creds.map(|(u, p)| base64encode(format!("{u}:{p}").as_bytes()));
    }

    /// The request target: the path, or the absolute URL for a proxy.
    fn push_target(&self, request: &mut String, path: &str) {
        if let Some(origin) = &self.proxy_origin {
            request.push_str(origin);
        }
        request.push_str(path);
    }

    /// libtorrent `add_headers`.
    fn add_headers(&self, request: &mut String) {
        request.push_str("Host: ");
        request.push_str(&self.host_header);
        if self.first_request || self.always_send_user_agent {
            request.push_str("\r\nUser-Agent: ");
            request.push_str(&self.user_agent);
        }
        if !self.external_auth.is_empty() {
            request.push_str("\r\nAuthorization: ");
            request.push_str(&self.external_auth);
        } else if !self.basic_auth.is_empty() {
            request.push_str("\r\nAuthorization: Basic ");
            request.push_str(&self.basic_auth);
        }
        if let Some(auth) = &self.proxy_auth {
            request.push_str("\r\nProxy-Authorization: Basic ");
            request.push_str(auth);
        }
        for (k, v) in &self.extra_headers {
            request.push_str("\r\n");
            request.push_str(k);
            request.push_str(": ");
            request.push_str(v);
        }
        if self.proxy_origin.is_some() {
            request.push_str("\r\nProxy-Connection: keep-alive");
        }
        if self.first_request || self.proxy_origin.is_some() {
            request.push_str("\r\nConnection: keep-alive");
        }
    }

    /// libtorrent `web_peer_connection::write_request`: a contiguous run of blocks starting
    /// at `start` in `piece` (it may continue into the following pieces) becomes one `Range`
    /// request per file it covers; pad files are not requested but delivered as zeros.
    /// `redirects` maps files to the paths they were redirected to on this server. A
    /// `restart` block that is the first one requested is resumed from the bytes it holds.
    pub fn write_request(
        &mut self,
        fs: &FileStorage,
        piece: u32,
        start: u32,
        length: u32,
        redirects: &HashMap<usize, String>,
        restart: &mut Option<Restart>,
    ) {
        let piece_size = fs.piece_length() as u64;
        let (mut req_start, mut req_length) = (start as u64, length as u64);
        let mut size = length as u64;
        while size > 0 {
            let offset = start as u64 + length as u64 - size;
            let pr_start = offset % piece_size;
            let pr = BlockRequest {
                piece: piece + (offset / piece_size) as u32,
                start: pr_start as u32,
                length: size.min(BLOCK_SIZE as u64).min(piece_size - pr_start) as u32,
            };
            self.requests.push_back(pr);
            // only when this block is the only one outstanding, so nothing else is in `piece`
            let resume = self.http_seed.is_none()
                && restart.as_ref().is_some_and(|r| {
                    self.requests.len() == 1
                        && self.piece.is_empty()
                        && r.request == pr
                        && r.data.len() < pr.length as usize
                });
            if resume {
                let r = restart.take().expect("checked");
                req_start += r.data.len() as u64;
                req_length -= r.data.len() as u64;
                self.piece = r.data;
            }
            size -= pr.length as u64;
        }

        let mut request = String::with_capacity(400);
        let mut num_pad_files = 0;
        let mut new_file_requests = 0;
        if let Some(ih) = self.http_seed {
            // libtorrent `http_seed_connection::write_request`: one request per piece (the
            // caller never lets a run cross into the next piece)
            request.push_str("GET ");
            self.push_target(&mut request, &self.path);
            request.push_str("?info_hash=");
            request.push_str(&escape_string(&ih.0));
            request.push_str(&format!("&piece={piece}"));
            if start > 0 || length as u64 != fs.piece_size(piece) as u64 {
                request.push_str(&format!("&ranges={}-{}", start, start + length - 1));
            }
            request.push_str(" HTTP/1.1\r\n");
            self.add_headers(&mut request);
            request.push_str("\r\n\r\n");
            self.first_request = false;
            // the body is the requested bytes, from 0
            self.file_requests.push_back(FileRequest { file: 0, start: 0, length: length as u64 });
            new_file_requests += 1;
        } else if self.single_file {
            let file_req =
                FileRequest { file: 0, start: piece as u64 * piece_size + req_start, length: req_length };
            // do not encode single file paths, they are assumed to be encoded in the torrent
            self.push_get(&mut request, &self.path.clone(), file_req);
            self.file_requests.push_back(file_req);
            new_file_requests += 1;
        } else {
            for f in fs.map_block(piece, req_start, req_length) {
                let file_req = FileRequest { file: f.file_index, start: f.offset, length: f.size };
                self.file_requests.push_back(file_req);
                new_file_requests += 1;
                if fs.pad_file_at(f.file_index) {
                    num_pad_files += 1;
                    continue;
                }
                let path = match redirects.get(&f.file_index) {
                    Some(r) => r.clone(),
                    None => format!("{}{}", self.path, escape_file_path(fs, f.file_index)),
                };
                self.push_get(&mut request, &path, file_req);
            }
        }
        self.out.extend_from_slice(request.as_bytes());
        // a request of nothing but pad files gets no response: its zeros are due now (they
        // may wait behind the responses to earlier requests)
        if num_pad_files == new_file_requests {
            self.handle_padfile(fs);
        }
    }

    fn push_get(&mut self, request: &mut String, path: &str, f: FileRequest) {
        request.push_str("GET ");
        self.push_target(request, path);
        request.push_str(" HTTP/1.1\r\n");
        self.add_headers(request);
        request.push_str(&format!("\r\nRange: bytes={}-{}\r\n\r\n", f.start, f.start + f.length - 1));
        self.first_request = false;
    }

    /// libtorrent `handle_padfile`: pad files at the front of the request queue were never
    /// requested; their bytes are zeros.
    fn handle_padfile(&mut self, fs: &FileStorage) {
        if self.http_seed.is_some() {
            return;
        }
        while let Some(&f) = self.file_requests.front() {
            if self.requests.is_empty() || !fs.pad_file_at(f.file) {
                return;
            }
            self.incoming_zeroes(f.length);
            self.file_requests.pop_front();
        }
    }

    /// libtorrent `incoming_zeroes`.
    fn incoming_zeroes(&mut self, mut len: u64) {
        while len > 0 {
            let Some(front) = self.requests.front() else { return };
            let copy = (front.length as usize - self.piece.len()).min(len as usize);
            self.piece.resize(self.piece.len() + copy, 0);
            len -= copy as u64;
            self.maybe_harvest_piece();
        }
    }

    /// libtorrent `incoming_payload`: body bytes complete the front blocks.
    fn incoming_payload(&mut self, mut buf: &[u8]) {
        self.received_body += buf.len() as u64;
        while !buf.is_empty() {
            let Some(front) = self.requests.front() else { return };
            let copy = (front.length as usize - self.piece.len()).min(buf.len());
            self.piece.extend_from_slice(&buf[..copy]);
            buf = &buf[copy..];
            self.maybe_harvest_piece();
        }
    }

    /// libtorrent `maybe_harvest_piece`.
    fn maybe_harvest_piece(&mut self) {
        let Some(&front) = self.requests.front() else { return };
        if self.piece.len() != front.length as usize {
            return;
        }
        self.requests.pop_front();
        let data = std::mem::take(&mut self.piece);
        self.events.push(WebEvent::Block { piece: front.piece, start: front.start, data });
    }

    fn finish(&mut self, e: WebEvent) {
        self.done = true;
        self.events.push(e);
    }

    /// libtorrent `web_peer_connection::on_receive`: parses responses and hands their bodies
    /// to the outstanding blocks. Returns how many of the bytes were payload (response bodies);
    /// the rest were headers.
    pub fn feed(&mut self, fs: &FileStorage, data: &[u8]) -> u64 {
        if self.done {
            return 0;
        }
        self.buf.extend_from_slice(data);
        // in case the first file on this series of requests is a padfile we need to handle it
        // right now
        self.handle_padfile(fs);
        let mut pos = 0;
        let mut payload = 0;
        let buf = std::mem::take(&mut self.buf);
        'parse: while pos < buf.len() && !self.done {
            if self.head.is_none() {
                let (h, body) = match parse_head(&buf[pos..]) {
                    Ok(Some(v)) => v,
                    Ok(None) => break,
                    Err(ParseError::Invalid(_)) => {
                        self.finish(WebEvent::Fail("HTTP parse error"));
                        break;
                    }
                };
                pos += body;
                self.num_responses += 1;
                if h.connection_close {
                    self.choked = true;
                    if self.num_responses == 1 {
                        self.events.push(WebEvent::NoKeepAlive);
                    }
                }
                // if the status code is not one of the accepted ones, abort
                if !is_ok_status(h.status) {
                    let file =
                        self.file_requests.front().filter(|_| self.http_seed.is_none()).map(|f| f.file);
                    let retry_after = h.header_duration("retry-after");
                    self.finish(WebEvent::HttpError {
                        status: h.status,
                        message: h.message,
                        retry_after,
                        file,
                    });
                    break;
                }
                if is_redirect(h.status) {
                    self.handle_redirect(&h);
                    break;
                }
                let server = h.header("server");
                self.server = if server.is_empty() { self.host.clone() } else { server.to_owned() };
                self.received_body = 0;
                self.chunk_pos = 0;
                self.chunk_scanned = 0;
                self.head = Some(h);
            }
            // we only received the header, no data
            if pos == buf.len() {
                break;
            }
            let h = self.head.as_ref().expect("parsed");
            let (range_start, range_end) = match get_range(h) {
                Ok(r) => r,
                Err(e) => {
                    self.finish(WebEvent::Disable(e));
                    break;
                }
            };
            let chunked = h.chunked;
            let Some(&file_req) = self.file_requests.front() else {
                self.finish(WebEvent::Fail("unexpected HTTP response"));
                break;
            };
            if range_start != file_req.start as i64 || range_end != (file_req.start + file_req.length) as i64
            {
                // the byte range in the http response is different what we expected (a BEP 17
                // seed is disabled for it)
                let e = "invalid range in HTTP response";
                self.finish(if self.http_seed.is_some() { WebEvent::Disable(e) } else { WebEvent::Fail(e) });
                break;
            }
            if chunked {
                while pos < buf.len() {
                    // first deliver any payload we have in the buffer so far, ahead of the
                    // next chunk header
                    if self.chunk_pos > 0 {
                        let copy = (self.chunk_pos as usize).min(buf.len() - pos);
                        if self.received_body + copy as u64 > file_req.length {
                            self.finish(WebEvent::Fail("invalid range in HTTP response"));
                            break 'parse;
                        }
                        self.incoming_payload(&buf[pos..pos + copy]);
                        payload += copy as u64;
                        pos += copy;
                        self.chunk_pos -= copy as u64;
                        if pos == buf.len() {
                            break 'parse;
                        }
                    }
                    let rest = &buf[pos..];
                    let fresh = &rest[self.chunk_scanned.min(rest.len())..];
                    let parsed = if rest.len() <= MAX_CHUNK_HEADER && !fresh.contains(&b'\n') {
                        Ok(Chunk::Incomplete)
                    } else {
                        #[cfg(test)]
                        {
                            self.chunk_parses += 1;
                        }
                        parse_chunk_header(rest)
                    };
                    match parsed {
                        Err(_) => {
                            self.finish(WebEvent::Fail("invalid chunk header"));
                            break 'parse;
                        }
                        Ok(Chunk::Incomplete) => {
                            self.chunk_scanned = rest.len();
                            break 'parse;
                        }
                        Ok(Chunk::Header { size, header_len, .. }) => {
                            self.chunk_scanned = 0;
                            pos += header_len;
                            self.chunk_pos = size;
                            // a chunk size of zero means the request is complete. Make sure
                            // the number of payload bytes we've received matches the number we
                            // requested
                            if size == 0 {
                                if self.received_body != file_req.length {
                                    self.finish(WebEvent::Fail("invalid range in HTTP response"));
                                    break 'parse;
                                }
                                self.file_request_done(fs);
                                continue 'parse;
                            }
                        }
                    }
                }
            } else {
                // the simple case, where we don't have chunked encoding
                let copy = ((file_req.length - self.received_body) as usize).min(buf.len() - pos);
                self.incoming_payload(&buf[pos..pos + copy]);
                payload += copy as u64;
                pos += copy;
                if self.received_body == file_req.length {
                    self.file_request_done(fs);
                }
            }
        }
        if !self.done {
            self.buf = buf;
            self.buf.drain(..pos);
        }
        payload
    }

    /// We just completed an HTTP file request: pop it, and deliver the pad files after it.
    fn file_request_done(&mut self, fs: &FileStorage) {
        self.file_requests.pop_front();
        self.head = None;
        self.received_body = 0;
        self.chunk_pos = 0;
        self.chunk_scanned = 0;
        // in between each file request, there may be an implicit pad-file request
        self.handle_padfile(fs);
    }

    /// libtorrent `handle_redirect`, up to the torrent calls.
    fn handle_redirect(&mut self, h: &ResponseHead) {
        let location = h.header("location");
        if location.is_empty() {
            return self.finish(WebEvent::Disable("missing location header in redirect"));
        }
        let location = resolve_redirect_location(&self.url, location);
        if self.single_file || self.http_seed.is_some() {
            return self.finish(WebEvent::Redirect { base: location, path: String::new(), file: None });
        }
        let Some(file) = self.file_requests.front().map(|f| f.file) else {
            return self.finish(WebEvent::Fail("unexpected HTTP response"));
        };
        match split_url(&location) {
            Ok((base, path)) => self.finish(WebEvent::Redirect { base, path, file: Some(file) }),
            Err(_) => self.finish(WebEvent::Fail("invalid redirect location")),
        }
    }

    /// The restart data of libtorrent `disconnect`: a block that was partly received is kept
    /// for the next connection. The outstanding requests are dropped.
    pub fn take_restart(&mut self) -> Option<Restart> {
        let r = (!self.requests.is_empty() && !self.file_requests.is_empty() && !self.piece.is_empty())
            .then(|| Restart { request: self.requests[0], data: std::mem::take(&mut self.piece) });
        self.requests.clear();
        r
    }
}

/// libtorrent `get_range`: the byte range a response carries, end exclusive. A 206 must have a
/// `Content-Range`; anything else is the whole entity from 0 and needs a `Content-Length`.
fn get_range(h: &ResponseHead) -> Result<(i64, i64), &'static str> {
    if h.status == 206 {
        match h.range {
            // the http range is inclusive
            Some((start, end)) if start >= 0 && end >= start => Ok((start, end + 1)),
            _ => Err("invalid range in HTTP response"),
        }
    } else if h.content_length < 0 {
        Err("missing content-length")
    } else {
        Ok((0, h.content_length))
    }
}

#[cfg(test)]
mod tests {
    //! The request and response handling of libtorrent's web seed suite (test/web_seed_suite.cpp,
    //! test_web_seed*.cpp) without sockets: pad files, misaligned files, single files, chunked
    //! bodies, redirects, errors and restart data.
    use super::*;
    use crate::metainfo::file_storage::FileFlags;

    fn url_peer(
        url: &str,
        auth: &str,
        headers: &[(String, String)],
        fs: &FileStorage,
        ua: &str,
        always_ua: bool,
    ) -> Result<WebPeer, UrlError> {
        WebPeer::new(url, WebSeedKind::UrlSeed, Sha1Hash([0; 20]), auth, headers, fs, ua, always_ua)
    }

    /// A file storage like libtorrent's `torrent_args().file(..)`: sizes, `pad` marks pad files.
    fn storage(files: &[(u64, bool)], piece_len: u32) -> FileStorage {
        let mut fs = FileStorage::new();
        for (i, &(size, pad)) in files.iter().enumerate() {
            let name = if pad { format!("torrent_dir/.pad/{i}") } else { format!("torrent_dir/{i}") };
            let flags = if pad { FileFlags::PAD_FILE } else { FileFlags::empty() };
            fs.add_file(&name, size, flags, 0, "", None).unwrap();
        }
        fs.set_piece_length(piece_len);
        let n = fs.total_size().div_ceil(piece_len as u64) as u32;
        fs.set_num_pieces(n);
        fs
    }

    fn single(size: u64, piece_len: u32) -> FileStorage {
        let mut fs = FileStorage::new();
        fs.add_file("test-single-file", size, FileFlags::empty(), 0, "", None).unwrap();
        fs.set_piece_length(piece_len);
        fs.set_num_pieces(size.div_ceil(piece_len as u64) as u32);
        fs
    }

    /// The torrent's bytes: byte i of the torrent is `i % 251`, pad files are zeros.
    fn content(fs: &FileStorage) -> Vec<u8> {
        let mut out = Vec::new();
        for f in 0..fs.num_files() {
            let off = fs.file_offset(f);
            for i in 0..fs.file_size(f) {
                out.push(if fs.pad_file_at(f) { 0 } else { ((off + i) % 251) as u8 });
            }
        }
        out
    }

    /// Parses the GET requests in `out`: (path, first byte, last byte).
    fn gets(out: &[u8]) -> Vec<(String, u64, u64)> {
        let s = String::from_utf8(out.to_vec()).unwrap();
        s.split("\r\n\r\n")
            .filter(|r| !r.is_empty())
            .map(|r| {
                let path = r.strip_prefix("GET ").unwrap().split(' ').next().unwrap().to_owned();
                let range = r.split("Range: bytes=").nth(1).unwrap();
                let (a, b) = range.split_once('-').unwrap();
                (path, a.parse().unwrap(), b.trim().parse().unwrap())
            })
            .collect()
    }

    /// A server: answers each GET with a 206 of the file's bytes, optionally chunked.
    fn serve(fs: &FileStorage, all: &[u8], reqs: &[(String, u64, u64)], chunked: bool) -> Vec<u8> {
        let mut out = Vec::new();
        for (path, a, b) in reqs {
            let file = (0..fs.num_files())
                .find(|&f| fs.num_files() == 1 || path.ends_with(&format!("/{}", escape_file_path(fs, f))))
                .unwrap_or_else(|| panic!("unknown path {path}"));
            let off = fs.file_offset(file);
            let body = &all[(off + a) as usize..=(off + b) as usize];
            let mut r = format!(
                "HTTP/1.1 206 Partial Content\r\nServer: test/1.0\r\nContent-Range: bytes {a}-{b}/*\r\n"
            );
            if chunked {
                r.push_str("Transfer-Encoding: chunked\r\n\r\n");
                let mut v = r.into_bytes();
                for c in body.chunks(1000) {
                    v.extend_from_slice(format!("{:x};ext\r\n", c.len()).as_bytes());
                    v.extend_from_slice(c);
                    v.extend_from_slice(b"\r\n");
                }
                v.extend_from_slice(b"0\r\nX-Trailer: 1\r\n\r\n");
                out.extend(v);
            } else {
                r.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
                out.extend_from_slice(r.as_bytes());
                out.extend_from_slice(body);
            }
        }
        out
    }

    /// Downloads the whole torrent through one WebPeer, `run` blocks per request, feeding the
    /// responses in slices of `step` bytes. Returns the reassembled torrent.
    fn download(fs: &FileStorage, run: u32, step: usize, chunked: bool) -> Vec<u8> {
        let all = content(fs);
        let mut p = url_peer("http://127.0.0.1:8080/web_seed", "", &[], fs, "UA/1", false).unwrap();
        let mut got = vec![0xeeu8; all.len()];
        let mut blocks = Vec::new();
        for piece in 0..fs.num_pieces() {
            for b in 0..fs.piece_size(piece).div_ceil(BLOCK_SIZE) {
                blocks.push((piece, b));
            }
        }
        for group in blocks.chunks(run as usize) {
            let (piece, b) = group[0];
            let end = group
                .last()
                .map(|&(p, b)| p as u64 * fs.piece_length() as u64 + (b as u64 + 1) * BLOCK_SIZE as u64)
                .unwrap();
            let start = piece as u64 * fs.piece_length() as u64 + b as u64 * BLOCK_SIZE as u64;
            let len = end.min(fs.total_size()) - start;
            p.write_request(fs, piece, b * BLOCK_SIZE, len as u32, &HashMap::new(), &mut None);
        }
        let reqs = gets(&std::mem::take(p.out()));
        for (path, ..) in &reqs {
            assert!(!path.contains(".pad"), "pad files are never requested: {path}");
        }
        let resp = serve(fs, &all, &reqs, chunked);
        for c in resp.chunks(step) {
            p.feed(fs, c);
            for e in p.take_events() {
                match e {
                    WebEvent::Block { piece, start, data } => {
                        let off = piece as usize * fs.piece_length() as usize + start as usize;
                        got[off..off + data.len()].copy_from_slice(&data);
                    }
                    e => panic!("unexpected {e:?}"),
                }
            }
        }
        // zeros of trailing pad files arrive with the last response
        for e in p.take_events() {
            if let WebEvent::Block { piece, start, data } = e {
                let off = piece as usize * fs.piece_length() as usize + start as usize;
                got[off..off + data.len()].copy_from_slice(&data);
            }
        }
        assert_eq!(p.num_requests(), 0, "every block answered");
        assert_eq!(p.num_file_requests(), 0);
        assert_eq!(p.server(), "test/1.0");
        got
    }

    /// The torrents of libtorrent's `run_http_suite` (url seed test cases 1 to 6).
    fn suite() -> Vec<FileStorage> {
        let case1: Vec<(u64, bool)> = [
            (0, false),
            (5, true),
            (11, false),
            (16000, false),
            (368, true),
            (16384, true),
            (16384, true),
            (17, false),
            (10, false),
            (8000, false),
            (8000, false),
            (1, false),
            (1, false),
            (1, false),
            (1, false),
            (1, false),
            (100, false),
            (0, false),
            (1, false),
            (1, false),
            (1, false),
            (100, false),
            (1, false),
            (1, false),
            (1, false),
            (1, false),
            (1, true),
            (1, true),
            (1, true),
            (1, false),
            (0, false),
            (0, false),
            (0, false),
            (1, false),
            (13, false),
            (65000, false),
            (34, false),
            (75, false),
            (2, false),
            (30, false),
            (400, false),
            (500, false),
            (23000, false),
            (900, false),
            (43000, false),
            (400, false),
            (4300, false),
            (6, false),
            (4, true),
        ]
        .into();
        vec![
            storage(&case1, 32 * 1024),
            // the end of the torrent are pad files
            storage(
                &[(0, true), (11, false), (5, false), (16000, false), (368, true), (16384, true)],
                16 * 1024,
            ),
            // misaligned
            storage(&[(16383, false), (11, false), (5, false), (16000, false)], 16 * 1024),
            // a full piece pad file
            storage(&[(32768, true), (16000, false), (11, false), (5, false)], 16 * 1024),
            // properly aligned pad files
            storage(
                &[
                    (32760, false),
                    (8, true),
                    (32760, false),
                    (8, false),
                    (32700, false),
                    (68, true),
                    (32000, false),
                ],
                32 * 1024,
            ),
            single(199_092, 32 * 1024),
        ]
    }

    #[test]
    fn web_seed_suite_transfers() {
        for (i, fs) in suite().iter().enumerate() {
            let want = content(fs);
            for (run, step, chunked) in
                [(1, 7, false), (4, 1000, false), (64, 100_000, false), (3, 333, true), (64, 5, true)]
            {
                let got = download(fs, run, step, chunked);
                assert!(got == want, "case {} run {run} step {step} chunked {chunked}", i + 1);
            }
        }
    }

    #[test]
    fn request_format() {
        let fs = storage(&[(20_000, false), (5, true), (30_000, false)], 16 * 1024);
        let mut p =
            url_peer("http://u:p@[::1]:81/dir with", "", &[("X-A".into(), "b".into())], &fs, "UA/1", false)
                .unwrap();
        assert_eq!(p.url(), "http://u:p@[::1]:81/dir with/");
        let mut redirects = HashMap::new();
        p.write_request(&fs, 0, 0, 16384, &redirects, &mut None);
        let first = String::from_utf8(std::mem::take(p.out())).unwrap();
        assert_eq!(
            first,
            "GET /dir with/torrent_dir/0 HTTP/1.1\r\nHost: [::1]:81\r\nUser-Agent: UA/1\r\n\
             Authorization: Basic dTpw\r\nX-A: b\r\nConnection: keep-alive\r\nRange: bytes=0-16383\r\n\r\n"
        );
        // piece 1 holds the end of file 0, the pad file and the start of file 2; file 2 is
        // redirected. No User-Agent or keep-alive after the first request.
        redirects.insert(2, "/elsewhere/f%202".to_owned());
        p.write_request(&fs, 1, 0, 16384, &redirects, &mut None);
        let next = String::from_utf8(std::mem::take(p.out())).unwrap();
        assert_eq!(
            next,
            "GET /dir with/torrent_dir/0 HTTP/1.1\r\nHost: [::1]:81\r\nAuthorization: Basic dTpw\r\nX-A: b\r\n\
             Range: bytes=16384-19999\r\n\r\n\
             GET /elsewhere/f%202 HTTP/1.1\r\nHost: [::1]:81\r\nAuthorization: Basic dTpw\r\nX-A: b\r\n\
             Range: bytes=0-12762\r\n\r\n"
        );
        assert_eq!(p.num_requests(), 2);
        assert_eq!(p.num_file_requests(), 4);

        // a single file: the URL is the file's when it ends with '/'; the default port is
        // left out of Host
        let fs = single(100_000, 32 * 1024);
        let mut p = url_peer("https://h.example/files/", "Bearer x", &[], &fs, "UA", true).unwrap();
        assert_eq!(p.url(), "https://h.example/files/test-single-file");
        p.write_request(&fs, 1, 16384, 16384 + 32768, &HashMap::new(), &mut None);
        let r = String::from_utf8(std::mem::take(p.out())).unwrap();
        assert_eq!(
            r,
            "GET /files/test-single-file HTTP/1.1\r\nHost: h.example\r\nUser-Agent: UA\r\nAuthorization: Bearer x\r\n\
             Connection: keep-alive\r\nRange: bytes=49152-98303\r\n\r\n"
        );
        let blocks: Vec<_> = p.requests().copied().collect();
        assert_eq!(
            blocks,
            [
                BlockRequest { piece: 1, start: 16384, length: 16384 },
                BlockRequest { piece: 2, start: 0, length: 16384 },
                BlockRequest { piece: 2, start: 16384, length: 16384 }
            ]
        );

        // through an HTTP proxy (libtorrent `using_proxy`): absolute URLs, the proxy's
        // credentials, keep-alive on every request
        let mut p = url_peer("http://h.example:8000/f", "", &[], &fs, "UA", false).unwrap();
        p.set_http_proxy(Some(("Aladdin", "open sesame")));
        p.write_request(&fs, 0, 0, 16384, &HashMap::new(), &mut None);
        p.write_request(&fs, 0, 16384, 16384, &HashMap::new(), &mut None);
        let r = String::from_utf8(std::mem::take(p.out())).unwrap();
        assert_eq!(
            r,
            "GET http://h.example:8000/f HTTP/1.1\r\nHost: h.example:8000\r\nUser-Agent: UA\r\n\
             Proxy-Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==\r\nProxy-Connection: keep-alive\r\n\
             Connection: keep-alive\r\nRange: bytes=0-16383\r\n\r\n\
             GET http://h.example:8000/f HTTP/1.1\r\nHost: h.example:8000\r\n\
             Proxy-Authorization: Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==\r\nProxy-Connection: keep-alive\r\n\
             Connection: keep-alive\r\nRange: bytes=16384-32767\r\n\r\n"
        );
    }

    fn feed_str(p: &mut WebPeer, fs: &FileStorage, s: &str) -> Vec<WebEvent> {
        p.feed(fs, s.as_bytes());
        p.take_events()
    }

    #[test]
    fn errors_redirects_and_ranges() {
        let fs = storage(&[(20_000, false), (30_000, false)], 16 * 1024);
        let new = || url_peer("http://a.b/base", "", &[], &fs, "UA", false).unwrap();
        let mut p = new();
        p.write_request(&fs, 0, 0, 16384, &HashMap::new(), &mut None);
        assert_eq!(
            feed_str(
                &mut p,
                &fs,
                "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 5\r\nConnection: close\r\n\r\n"
            ),
            [
                WebEvent::NoKeepAlive,
                WebEvent::HttpError {
                    status: 503,
                    message: "Service Unavailable".into(),
                    retry_after: Some(5),
                    file: Some(0)
                }
            ]
        );
        assert!(p.is_done() && p.choked);
        assert!(feed_str(&mut p, &fs, "HTTP/1.1 200 OK\r\n\r\n").is_empty(), "nothing after the end");

        // a redirect of file 1 to another server
        let mut p = new();
        p.write_request(&fs, 1, 0, 16384, &HashMap::new(), &mut None);
        let ev = feed_str(&mut p, &fs, "HTTP/1.1 302 Found\r\nLocation: http://c.d:99/x/f1?k\r\n\r\n");
        assert_eq!(
            ev,
            [WebEvent::Redirect { base: "http://c.d:99".into(), path: "/x/f1?k".into(), file: Some(0) }]
        );
        // relative locations resolve against the seed's URL
        let mut p = new();
        p.write_request(&fs, 2, 0, 16384, &HashMap::new(), &mut None);
        let ev = feed_str(&mut p, &fs, "HTTP/1.1 301 Moved\r\nLocation: /y/f\r\n\r\n");
        assert_eq!(
            ev,
            [WebEvent::Redirect { base: "http://a.b".into(), path: "/y/f".into(), file: Some(1) }]
        );
        let mut p = new();
        p.write_request(&fs, 0, 0, 16384, &HashMap::new(), &mut None);
        assert_eq!(
            feed_str(&mut p, &fs, "HTTP/1.1 302 Found\r\n\r\n"),
            [WebEvent::Disable("missing location header in redirect")]
        );
        // a single file moves as a whole
        let one = single(50_000, 16 * 1024);
        let mut p = url_peer("http://a.b/f", "", &[], &one, "UA", false).unwrap();
        p.write_request(&one, 0, 0, 16384, &HashMap::new(), &mut None);
        let ev = feed_str(&mut p, &one, "HTTP/1.1 307 Temp\r\nLocation: g\r\n\r\n");
        assert_eq!(ev, [WebEvent::Redirect { base: "http://a.b/g".into(), path: String::new(), file: None }]);

        // the wrong range, a 200 without a length, a 206 without a range, garbage
        for (resp, want) in [
            (
                "HTTP/1.1 206 OK\r\nContent-Range: bytes 1-16384/*\r\n\r\nx",
                WebEvent::Fail("invalid range in HTTP response"),
            ),
            ("HTTP/1.1 200 OK\r\n\r\nxyz", WebEvent::Disable("missing content-length")),
            (
                "HTTP/1.1 206 OK\r\nContent-Length: 16384\r\n\r\nxyz",
                WebEvent::Disable("invalid range in HTTP response"),
            ),
            (
                "HTTP/1.1 200 OK\r\nContent-Length: 50000\r\n\r\nxyz",
                WebEvent::Fail("invalid range in HTTP response"),
            ),
            ("garbage\r\n\r\n", WebEvent::Fail("HTTP parse error")),
        ] {
            let mut p = new();
            p.write_request(&fs, 0, 0, 16384, &HashMap::new(), &mut None);
            assert_eq!(feed_str(&mut p, &fs, resp), [want], "{resp:?}");
        }
        // a 200 for the whole file is fine when that is what was asked for
        let tiny = single(10, 16 * 1024);
        let mut p = url_peer("http://a.b/f", "", &[], &tiny, "UA", false).unwrap();
        p.write_request(&tiny, 0, 0, 10, &HashMap::new(), &mut None);
        let ev = feed_str(&mut p, &tiny, "HTTP/1.0 200 OK\r\nContent-Length: 10\r\n\r\n0123456789");
        assert_eq!(
            ev,
            [WebEvent::NoKeepAlive, WebEvent::Block { piece: 0, start: 0, data: b"0123456789".to_vec() }]
        );
        // a chunked body longer than the range
        let mut p = url_peer("http://a.b/f", "", &[], &tiny, "UA", false).unwrap();
        p.write_request(&tiny, 0, 0, 10, &HashMap::new(), &mut None);
        let ev = feed_str(
            &mut p,
            &tiny,
            "HTTP/1.1 206 OK\r\nContent-Range: bytes 0-9/10\r\nTransfer-Encoding: chunked\r\n\r\n10\r\n0123456789abcdef",
        );
        assert_eq!(ev, [WebEvent::Fail("invalid range in HTTP response")]);
    }

    #[test]
    fn a_dripping_chunk_header_is_cut_off() {
        // SEC-22: a 206 chunked answer whose chunk header never ends, a byte per read
        let tiny = single(10, 16 * 1024);
        let mut p = url_peer("http://a.b/f", "", &[], &tiny, "UA", false).unwrap();
        p.write_request(&tiny, 0, 0, 10, &HashMap::new(), &mut None);
        let head = "HTTP/1.1 206 OK\r\nContent-Range: bytes 0-9/10\r\nTransfer-Encoding: chunked\r\n\r\n";
        assert!(feed_str(&mut p, &tiny, &format!("{head}1;")).is_empty());
        let mut reads = 0;
        while p.take_events().is_empty() {
            reads += 1;
            assert!(reads <= MAX_CHUNK_HEADER, "the header must fail before it passes the limit");
            p.feed(&tiny, b"x");
        }
        assert!(p.done);
        assert!(p.chunk_parses <= 2, "parsed {} times: the scan restarts on each read", p.chunk_parses);
        // a header split over many reads still works, with one parse per newline
        let mut p = url_peer("http://a.b/f", "", &[], &tiny, "UA", false).unwrap();
        p.write_request(&tiny, 0, 0, 10, &HashMap::new(), &mut None);
        assert!(feed_str(&mut p, &tiny, head).is_empty());
        for c in b"a;ext\r\n0123456789\r\n0\r\nx: y\r\n\r\n" {
            p.feed(&tiny, &[*c]);
        }
        let ev = p.take_events();
        assert_eq!(ev, [WebEvent::Block { piece: 0, start: 0, data: b"0123456789".to_vec() }]);
        assert!(!p.done);
        assert!(p.chunk_parses <= 6, "parsed {} times", p.chunk_parses);
    }

    #[test]
    fn restart_data() {
        let fs = single(100_000, 32 * 1024);
        let all = content(&fs);
        let mut p = url_peer("http://a.b/f", "", &[], &fs, "UA", false).unwrap();
        p.write_request(&fs, 0, 16384, 32768, &HashMap::new(), &mut None);
        std::mem::take(p.out());
        let mut resp = b"HTTP/1.1 206 OK\r\nContent-Range: bytes 16384-49151/*\r\n\r\n".to_vec();
        resp.extend_from_slice(&all[16384..16384 + 20_000]);
        p.feed(&fs, &resp);
        let ev = p.take_events();
        assert_eq!(ev.len(), 1, "one block, and 3616 bytes of the next");
        let mut restart = p.take_restart();
        assert_eq!(
            restart.as_ref().map(|r| (r.request, r.data.len())),
            Some((BlockRequest { piece: 1, start: 0, length: 16384 }, 3616))
        );
        assert_eq!(p.num_requests(), 0);

        // the next connection asks for the rest of that block only
        let mut p = url_peer("http://a.b/f", "", &[], &fs, "UA", false).unwrap();
        p.write_request(&fs, 1, 0, 16384, &HashMap::new(), &mut restart);
        assert!(restart.is_none());
        let reqs = gets(&std::mem::take(p.out()));
        assert_eq!(reqs, [("/f".to_owned(), 32768 + 3616, 49151)]);
        let mut resp = b"HTTP/1.1 206 OK\r\nContent-Range: bytes 36384-49151/*\r\n\r\n".to_vec();
        resp.extend_from_slice(&all[36384..49152]);
        p.feed(&fs, &resp);
        assert_eq!(
            p.take_events(),
            [WebEvent::Block { piece: 1, start: 0, data: all[32768..49152].to_vec() }]
        );
        // restart data for another block is left alone
        let mut other =
            Some(Restart { request: BlockRequest { piece: 2, start: 0, length: 16384 }, data: vec![1] });
        p.write_request(&fs, 1, 16384, 16384, &HashMap::new(), &mut other);
        assert!(other.is_some());
    }

    #[test]
    fn only_pad_files() {
        // libtorrent test case 4: piece 0 and 1 are one pad file; their zeros come without a
        // request, but only once the responses queued before them are in
        let fs = storage(&[(16000, false), (16768, true), (16384, true), (11, false)], 16 * 1024);
        let all = content(&fs);
        let mut p = url_peer("http://a.b/", "", &[], &fs, "UA", false).unwrap();
        p.write_request(&fs, 0, 0, 16384, &HashMap::new(), &mut None);
        p.write_request(&fs, 2, 0, 16384, &HashMap::new(), &mut None);
        assert!(p.take_events().is_empty(), "behind the first response");
        let reqs = gets(&std::mem::take(p.out()));
        assert_eq!(reqs.len(), 1);
        let resp = serve(&fs, &all, &reqs, false);
        p.feed(&fs, &resp);
        let ev = p.take_events();
        assert_eq!(ev.len(), 2);
        assert_eq!(ev[1], WebEvent::Block { piece: 2, start: 0, data: vec![0; 16384] });
        // nothing outstanding: zeros at once
        p.write_request(&fs, 1, 0, 16384, &HashMap::new(), &mut None);
        assert_eq!(p.take_events(), [WebEvent::Block { piece: 1, start: 0, data: vec![0; 16384] }]);
        assert!(p.out().is_empty());
    }

    #[test]
    fn bep17_http_seed() {
        let fs = single(100_000, 32 * 1024);
        let ih = Sha1Hash([0xab; 20]);
        let mut p =
            WebPeer::new("http://h.example/seed", WebSeedKind::HttpSeed, ih, "", &[], &fs, "UA", false)
                .unwrap();
        assert_eq!(p.url(), "http://h.example/seed");
        // a whole piece, then a range of the short last piece
        p.write_request(&fs, 0, 0, 32768, &HashMap::new(), &mut None);
        p.write_request(&fs, 3, 0, 1696, &HashMap::new(), &mut None);
        p.write_request(&fs, 1, 16384, 16384, &HashMap::new(), &mut None);
        let out = String::from_utf8(std::mem::take(p.out())).unwrap();
        let q = "%ab".repeat(20);
        assert_eq!(
            out,
            format!(
                "GET /seed?info_hash={q}&piece=0 HTTP/1.1\r\nHost: h.example\r\nUser-Agent: UA\r\nConnection: keep-alive\r\n\r\n\
                 GET /seed?info_hash={q}&piece=3 HTTP/1.1\r\nHost: h.example\r\n\r\n\
                 GET /seed?info_hash={q}&piece=1&ranges=16384-32767 HTTP/1.1\r\nHost: h.example\r\n\r\n"
            )
        );
        let all = content(&fs);
        let mut resp = Vec::new();
        for (a, b) in [(0usize, 32768usize), (98304, 100_000), (49152, 65536)] {
            resp.extend_from_slice(
                format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", b - a).as_bytes(),
            );
            resp.extend_from_slice(&all[a..b]);
        }
        for c in resp.chunks(999) {
            p.feed(&fs, c);
        }
        let blocks: Vec<(u32, u32, usize)> = p
            .take_events()
            .into_iter()
            .map(|e| match e {
                WebEvent::Block { piece, start, data } => {
                    let off = piece as usize * 32768 + start as usize;
                    assert!(data[..] == all[off..off + data.len()]);
                    (piece, start, data.len())
                }
                e => panic!("{e:?}"),
            })
            .collect();
        assert_eq!(blocks, [(0, 0, 16384), (0, 16384, 16384), (3, 0, 1696), (1, 16384, 16384)]);
        // a body of the wrong size disables the seed; errors carry no file
        p.write_request(&fs, 2, 0, 32768, &HashMap::new(), &mut None);
        p.feed(&fs, b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n12345");
        assert_eq!(p.take_events(), [WebEvent::Disable("invalid range in HTTP response")]);
        let mut p =
            WebPeer::new("http://h.example/seed", WebSeedKind::HttpSeed, ih, "", &[], &fs, "UA", false)
                .unwrap();
        p.write_request(&fs, 2, 0, 32768, &HashMap::new(), &mut None);
        p.feed(&fs, b"HTTP/1.1 503 Busy\r\nRetry-After: 9\r\n\r\n");
        assert_eq!(
            p.take_events(),
            [WebEvent::HttpError { status: 503, message: "Busy".into(), retry_after: Some(9), file: None }]
        );
        let mut p =
            WebPeer::new("http://h.example/seed", WebSeedKind::HttpSeed, ih, "", &[], &fs, "UA", false)
                .unwrap();
        p.write_request(&fs, 2, 0, 32768, &HashMap::new(), &mut None);
        p.feed(&fs, b"HTTP/1.1 302 Found\r\nLocation: /other\r\n\r\n");
        assert_eq!(
            p.take_events(),
            [WebEvent::Redirect { base: "http://h.example/other".into(), path: String::new(), file: None }]
        );
    }
}
