//! Sockets: the acceptor thread and one thread per connection (keep-alive, pipelining, size
//! limits, timeouts, the TLS handshake with HTTPS), and the WebSocket loop a `GET /pa/ws` turns
//! its connection thread into.

use std::io::{self, ErrorKind, Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use patorrent_core::tls;

use super::api::{self, Request, UNAUTHENTICATED_BODY};
use super::http::{HttpError, Response, parse_head};
use super::native::Client;
use super::ws::{self, Message, Opcode};
use super::{MAX_CONNECTIONS, Shared};

/// An idle keep-alive connection is closed after this long.
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);
/// A request head must arrive within this long of its first byte.
const HEAD_TIMEOUT: Duration = Duration::from_secs(20);
/// A request body must arrive within this long of the head.
const BODY_TIMEOUT: Duration = Duration::from_secs(120);
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// The path that upgrades to the native WebSocket.
pub const WS_PATH: &str = "/pa/ws";
/// How long a WebSocket thread blocks in `read` before it looks for a new status tick, so a
/// tick reaches the client within this long of the alert batch.
pub(super) const WS_POLL: Duration = Duration::from_millis(50);
/// A ping goes out after this long without a frame from the client, and the socket is closed
/// after twice that.
const WS_PING_AFTER: Duration = Duration::from_secs(30);

pub(super) fn spawn(
    listener: TcpListener,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<JoinHandle<()>> {
    let active = Arc::new(AtomicUsize::new(0));
    std::thread::Builder::new().name("webui-accept".into()).spawn(move || {
        for conn in listener.incoming() {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            let Ok(mut stream) = conn else {
                // EMFILE and friends: back off instead of spinning
                std::thread::sleep(Duration::from_millis(50));
                continue;
            };
            if active.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
                // (an HTTPS client would not read a plain-text answer: it is just closed)
                if shared.tls.is_none() {
                    let _ = stream.set_write_timeout(Some(Duration::from_secs(1)));
                    let mut out = Vec::new();
                    Response::text(503, "too many connections").write(false, false, &mut out);
                    let _ = stream.write_all(&out);
                }
                continue;
            }
            let peer = match stream.peer_addr() {
                Ok(a) => a,
                Err(_) => continue,
            };
            active.fetch_add(1, Ordering::SeqCst);
            let guard = ActiveGuard(active.clone());
            let shared = shared.clone();
            let spawned = std::thread::Builder::new().name("webui-conn".into()).spawn(move || {
                let _guard = guard;
                connection(stream, peer, &shared);
            });
            if spawned.is_err() {
                // the guard went down with the closure
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    })
}

struct ActiveGuard(Arc<AtomicUsize>);

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Unblocks the acceptor's `accept` so it sees the stop flag.
pub(super) fn wake(addr: SocketAddr) {
    let target = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => SocketAddr::new(Ipv4Addr::LOCALHOST.into(), addr.port()),
        IpAddr::V6(ip) if ip.is_unspecified() => SocketAddr::new(Ipv6Addr::LOCALHOST.into(), addr.port()),
        _ => addr,
    };
    let _ = TcpStream::connect_timeout(&target, Duration::from_secs(1));
}

/// A client's connection: plain HTTP, or HTTPS once the TLS handshake is done.
pub(super) enum Conn {
    Plain(TcpStream),
    Tls(Box<tls::Stream<TcpStream>>),
}

impl Conn {
    fn tcp(&self) -> &TcpStream {
        match self {
            Conn::Plain(s) => s,
            Conn::Tls(s) => s.get_ref(),
        }
    }

    pub(super) fn set_read_timeout(&self, t: Duration) {
        let _ = self.tcp().set_read_timeout(Some(t));
    }

    /// Half-closes the socket. A TLS connection ends without a close_notify alert, which
    /// HTTP/1.1 tolerates (messages carry their length).
    fn shutdown_write(&self) {
        let _ = self.tcp().shutdown(Shutdown::Write);
    }
}

impl Read for Conn {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(s) => s.read(buf),
            Conn::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Conn {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Conn::Plain(s) => s.write(buf),
            Conn::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Conn::Plain(s) => s.flush(),
            Conn::Tls(s) => s.flush(),
        }
    }
}

/// Writes all of `data`; with TLS, also what the socket had no room for (`flush`).
fn write_out(stream: &mut Conn, data: &[u8]) -> bool {
    stream.write_all(data).is_ok() && stream.flush().is_ok()
}

/// The TLS handshake of an HTTPS connection, each read within [`HEAD_TIMEOUT`]. A client that
/// speaks plain HTTP to the HTTPS port is sent to the `https://` address instead.
fn start_tls(stream: TcpStream, identity: &tls::Identity, shared: &Shared) -> Option<Conn> {
    let _ = stream.set_read_timeout(Some(HEAD_TIMEOUT));
    let mut first = [0u8; 1];
    if !matches!(stream.peek(&mut first), Ok(1)) {
        return None;
    }
    // a TLS connection starts with a handshake record (22)
    if first[0] != 22 {
        redirect_to_https(Conn::Plain(stream), shared);
        return None;
    }
    let mut tls = tls::Stream::accept(stream, identity).ok()?;
    tls.handshake().ok()?;
    Some(Conn::Tls(Box::new(tls)))
}

/// Answers a plain-HTTP request on the HTTPS port with a redirect to the same host over HTTPS.
fn redirect_to_https(mut stream: Conn, shared: &Shared) {
    let mut buf = Vec::new();
    let head = loop {
        match parse_head(&buf, &shared.limits) {
            Ok(Some((h, _))) => break h,
            Ok(None) => {}
            Err(_) => return,
        }
        if !matches!(read_more(&mut stream, &mut buf), ReadOutcome::Data) {
            return;
        }
    };
    let host = head
        .header("host")
        .filter(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b)));
    let r = match host {
        Some(host) => {
            let url = format!("https://{host}/");
            Response::text(301, format!("This server speaks HTTPS: {url}\n")).with_header("Location", url)
        }
        None => Response::text(400, "This server speaks HTTPS.\n"),
    };
    send_and_close(&mut stream, &r);
}

enum ReadOutcome {
    Data,
    Closed,
    TimedOut,
}

fn read_more(stream: &mut Conn, buf: &mut Vec<u8>) -> ReadOutcome {
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return ReadOutcome::Closed,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                return ReadOutcome::Data;
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => continue,
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                return ReadOutcome::TimedOut;
            }
            Err(_) => return ReadOutcome::Closed,
        }
    }
}

fn send(stream: &mut Conn, r: &Response, keep_alive: bool, head_only: bool) -> bool {
    let mut out = Vec::with_capacity(r.body.len() + 512);
    r.write(keep_alive, head_only, &mut out);
    write_out(stream, &out)
}

fn connection(stream: TcpStream, peer: SocketAddr, shared: &Shared) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_write_timeout(Some(WRITE_TIMEOUT));
    let mut stream = match &shared.tls {
        None => Conn::Plain(stream),
        Some(identity) => match start_tls(stream, identity, shared) {
            Some(conn) => conn,
            None => return,
        },
    };
    let limits = shared.limits;
    let mut buf: Vec<u8> = Vec::new();
    loop {
        // the head
        stream.set_read_timeout(IDLE_TIMEOUT);
        let mut started: Option<Instant> = if buf.is_empty() { None } else { Some(Instant::now()) };
        let (head, used) = loop {
            match parse_head(&buf, &limits) {
                Ok(Some(h)) => break h,
                Ok(None) => {}
                Err(e) => {
                    send_and_close(&mut stream, &error_response(e));
                    return;
                }
            }
            if let Some(t) = started {
                let left = HEAD_TIMEOUT.saturating_sub(t.elapsed());
                if left.is_zero() {
                    send_and_close(&mut stream, &Response::text(408, "Request Timeout"));
                    return;
                }
                stream.set_read_timeout(left.min(IDLE_TIMEOUT));
            }
            match read_more(&mut stream, &mut buf) {
                ReadOutcome::Data => {
                    started.get_or_insert_with(Instant::now);
                }
                ReadOutcome::Closed => return,
                ReadOutcome::TimedOut => {
                    if !buf.is_empty() {
                        send_and_close(&mut stream, &Response::text(408, "Request Timeout"));
                    }
                    return;
                }
            }
        };
        buf.drain(..used);

        if head.path == patorrent_core::remote::PATH {
            match super::remote::handshake(shared, &head, peer.ip()) {
                Ok(()) => {
                    if write_out(&mut stream, &super::remote::switching_protocols()) {
                        super::remote::run(stream, buf, shared);
                    }
                }
                Err(r) => send_and_close(&mut stream, &api::with_security_headers(r)),
            }
            return;
        }
        if head.path == WS_PATH {
            match api::websocket_handshake(shared, &head, peer.ip()) {
                Ok((key, ws_key)) => {
                    if write_out(&mut stream, &ws::handshake_response(&ws_key)) {
                        websocket(stream, buf, shared, &key);
                    }
                }
                Err(r) => send_and_close(&mut stream, &api::with_security_headers(r)),
            }
            return;
        }

        // the body
        let max_body =
            if api::authorized(shared, &head, peer.ip()) { limits.max_body } else { UNAUTHENTICATED_BODY };
        if head.content_length > max_body {
            send_and_close(&mut stream, &Response::text(413, "request body too large"));
            return;
        }
        if head.expect_continue
            && buf.len() < head.content_length
            && !write_out(&mut stream, b"HTTP/1.1 100 Continue\r\n\r\n")
        {
            return;
        }
        let body_started = Instant::now();
        while buf.len() < head.content_length {
            let left = BODY_TIMEOUT.saturating_sub(body_started.elapsed());
            if left.is_zero() {
                send_and_close(&mut stream, &Response::text(408, "Request Timeout"));
                return;
            }
            stream.set_read_timeout(left.min(IDLE_TIMEOUT));
            match read_more(&mut stream, &mut buf) {
                ReadOutcome::Data => {}
                ReadOutcome::Closed => return,
                ReadOutcome::TimedOut => {
                    send_and_close(&mut stream, &Response::text(408, "Request Timeout"));
                    return;
                }
            }
        }
        let body: Vec<u8> = buf.drain(..head.content_length).collect();
        let head_only = head.method == "HEAD";
        let keep_alive = head.keep_alive;
        let req = Request { head, body, peer: peer.ip() };
        let resp = api::serve(shared, &req);
        if !send(&mut stream, &resp, keep_alive, head_only) || !keep_alive {
            return;
        }
        // a large buffer from an upload is not kept around for the next request
        if buf.capacity() > 64 * 1024 && buf.len() < 64 * 1024 {
            buf.shrink_to(64 * 1024);
        }
    }
}

/// Sends a final response, then reads (and drops) what the client is still sending for a
/// moment before closing, so the close does not reset the connection and lose the response
/// (a "lingering close").
fn send_and_close(stream: &mut Conn, r: &Response) {
    if !send(stream, r, false, false) {
        return;
    }
    stream.shutdown_write();
    stream.set_read_timeout(Duration::from_secs(2));
    let mut sink = [0u8; 16 * 1024];
    let mut left: usize = 1024 * 1024;
    while left > 0 {
        match stream.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(n) => left = left.saturating_sub(n),
        }
    }
}

fn error_response(e: HttpError) -> Response {
    Response::text(e.status(), e.message())
}

/// The WebSocket loop, on the connection's own thread: `hello` first, then client frames as
/// they come (read with a short timeout) and a `tick` whenever the cache's tick counter moved
/// past the last one sent. `buf` holds whatever arrived after the upgrade request.
fn websocket(mut stream: Conn, mut buf: Vec<u8>, shared: &Shared, key: &str) {
    let max = shared.limits.max_message;
    let mut client = Client::new();
    let mut assembler = ws::Assembler::new(max);
    let mut out = Vec::with_capacity(4096);
    let mut last_tick = {
        let st = shared.lock();
        ws::write_text(&client.hello(&st).to_json(), &mut out);
        st.tick
    };
    if !write_out(&mut stream, &out) {
        return;
    }
    stream.set_read_timeout(WS_POLL);
    let mut last_heard = Instant::now();
    let mut pinged = false;
    loop {
        out.clear();
        // frames the client sent
        let mut close: Option<(u16, String)> = None;
        loop {
            let (frame, used) = match ws::parse_frame(&buf, max) {
                Ok(Some(f)) => f,
                Ok(None) => break,
                Err(e) => {
                    close = Some((e.code(), e.message().to_owned()));
                    break;
                }
            };
            buf.drain(..used);
            last_heard = Instant::now();
            pinged = false;
            match assembler.push(frame) {
                Ok(Some(Message::Text(text))) => {
                    let mut st = shared.lock();
                    for reply in client.on_message(&text, &mut st) {
                        ws::write_text(&reply.to_json(), &mut out);
                    }
                    drop(st);
                    // the tab's engine replies, so the next tick has fresh content
                    client.request_detail(shared);
                }
                Ok(Some(Message::Ping(payload))) => ws::write_frame(Opcode::Pong, &payload, &mut out),
                Ok(Some(Message::Pong(_))) | Ok(None) => {}
                Ok(Some(Message::Close(code, _))) => {
                    // RFC 6455 section 5.5.1: echo the status code, then close
                    close = Some((if code == 1005 { 1000 } else { code }, String::new()));
                    break;
                }
                Err(e) => {
                    close = Some((e.code(), e.message().to_owned()));
                    break;
                }
            }
        }
        if let Some((code, reason)) = close {
            ws::write_close(code, &reason, &mut out);
            write_out(&mut stream, &out);
            stream.shutdown_write();
            // read what the client still sends (its close frame) for a moment, as a lingering
            // close, so it sees ours
            stream.set_read_timeout(Duration::from_secs(2));
            let mut sink = [0u8; 4096];
            while matches!(stream.read(&mut sink), Ok(n) if n > 0) {}
            return;
        }
        // a new status tick, and the session's health
        {
            let mut st = shared.lock();
            if st.tick != last_tick {
                last_tick = st.tick;
                let now = Instant::now();
                match st.sessions.get_mut(key) {
                    Some(s) => s.last_used = now,
                    None => {
                        // logged out elsewhere, or the session expired
                        drop(st);
                        ws::write_close(1008, "session ended", &mut out);
                        write_out(&mut stream, &out);
                        return;
                    }
                }
                for msg in client.on_tick(&mut st) {
                    ws::write_text(&msg.to_json(), &mut out);
                }
                drop(st);
                client.request_detail(shared);
            }
        }
        // liveness
        let silent = last_heard.elapsed();
        if silent > 2 * WS_PING_AFTER {
            ws::write_close(1001, "no reply to ping", &mut out);
            write_out(&mut stream, &out);
            return;
        }
        if silent > WS_PING_AFTER && !pinged {
            pinged = true;
            ws::write_frame(Opcode::Ping, b"pa", &mut out);
        }
        if !out.is_empty() && !write_out(&mut stream, &out) {
            return;
        }
        match read_more(&mut stream, &mut buf) {
            ReadOutcome::Data | ReadOutcome::TimedOut => {}
            ReadOutcome::Closed => return,
        }
    }
}
