// PaTorrent's own code, not ported: libtorrent has no remote session.

//! Remote sessions: a [`Session`] whose engine is a `patorrent-nox` elsewhere, for the GUIs'
//! remote mode (PLAN.md Phase 6; docs/WEBUI.md "Remote mode").
//!
//! The GUIs are thick clients: they keep their own `frontend` models fed by the engine's
//! alerts. So the remote protocol carries the engine API itself. The client's handles queue
//! the same commands a local engine thread takes, and a pump thread sends them to the server
//! instead; the server's alerts come back into the client's alert queue. A frontend sees a
//! [`SessionHandle`] like any other.
//!
//! The connection is an HTTP/1.1 upgrade on the Web UI's port: `GET /pa/remote` with
//! `Upgrade: patorrent-remote`, the protocol version in `X-PaTorrent-Remote` and the Web UI
//! login as Basic credentials. TLS (`tls::Stream`) wraps it when the Web UI speaks HTTPS. The
//! server answers `101 Switching Protocols`, then both sides send frames: a 4-byte big-endian
//! length and one bencoded message (`wire`). The server's first frame is the [`Hello`].
//!
//! Some commands stay on the client: a session's IP and port filter tables (the server keeps
//! its own), and `Shutdown`, which closes the connection. The server answers
//! `post_torrent_updates` and `post_session_stats` itself (each client has its own "changed
//! since" state), and decides how to apply settings ([`Request`]).

mod wire;

#[cfg(test)]
mod tests;

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use crate::alert::Alert;
use crate::session::{Command, Session, SessionHandle, Shared};
use crate::settings::Settings;
use crate::status::{SessionStats, TorrentStatus};
use crate::tls::{self, Verify};
use wire::{Enc, Wire};

/// The upgrade request's path.
pub const PATH: &str = "/pa/remote";
/// The `Upgrade` token.
pub const UPGRADE: &str = "patorrent-remote";
/// The request header naming the client's protocol version.
pub const VERSION_HEADER: &str = "X-PaTorrent-Remote";
/// The protocol version: both ends must run the same one (messages are positional).
pub const PROTOCOL: u32 = 2;
/// The largest frame either side accepts (a status update of 100,000 torrents is about
/// 60 MB).
pub const MAX_FRAME: usize = 256 << 20;
/// How long the client's pump waits for the server between looks at its command queue: a
/// command waits at most this long before it is sent.
const PUMP_POLL: Duration = Duration::from_millis(20);

/// What the server says first.
#[derive(Clone, Debug, PartialEq)]
pub struct Hello {
    /// The server's PaTorrent version.
    pub server_version: String,
    /// The server's default save path: where a client's new torrents go unless it names a
    /// folder (on the server's disk).
    pub save_path: String,
    /// The engine settings the server runs with.
    pub settings: Settings,
}

// --- frames ----------------------------------------------------------------------------------

/// Appends one frame holding what `body` writes.
fn push_frame(out: &mut Vec<u8>, body: impl FnOnce(&mut Enc)) {
    let mut e = Enc::default();
    body(&mut e);
    out.extend_from_slice(&(e.0.len() as u32).to_be_bytes());
    out.extend_from_slice(&e.0);
}

/// Appends the frame of an alert.
pub fn alert_frame(alert: &Alert, out: &mut Vec<u8>) {
    push_frame(out, |e| wire::encode_alert(alert, e));
}

/// Appends a status update frame: what `post_torrent_updates` answers.
pub fn state_update_frame(v: Vec<TorrentStatus>, out: &mut Vec<u8>) {
    alert_frame(&Alert::StateUpdate(v), out);
}

/// Appends a session statistics frame: what `post_session_stats` answers.
pub fn session_stats_frame(s: SessionStats, out: &mut Vec<u8>) {
    alert_frame(&Alert::SessionStats(s), out);
}

/// Appends the hello frame.
pub fn hello_frame(hello: &Hello, out: &mut Vec<u8>) {
    push_frame(out, |e| {
        e.list();
        e.bytes(b"hello");
        PROTOCOL.put(e);
        hello.server_version.put(e);
        hello.save_path.put(e);
        hello.settings.put(e);
        e.end();
    });
}

fn decode_hello(frame: &[u8]) -> Option<(u32, Hello)> {
    wire::decode(frame, |name, f| {
        (name == "hello").then_some(())?;
        let protocol = f.next()?;
        Some((protocol, Hello { server_version: f.next()?, save_path: f.next()?, settings: f.next()? }))
    })
}

/// Splits a byte stream into frames.
pub struct FrameReader {
    buf: Vec<u8>,
    max: usize,
}

impl FrameReader {
    /// `initial` is what arrived with the upgrade, after its head. Frames longer than `max`
    /// are an error.
    pub fn new(initial: Vec<u8>, max: usize) -> FrameReader {
        FrameReader { buf: initial, max }
    }

    /// One read from `r` into the buffer: `Ok(false)` at the end of the stream. A timeout
    /// comes back as the error it is (`WouldBlock` or `TimedOut`).
    pub fn read_from(&mut self, r: &mut impl Read) -> io::Result<bool> {
        let mut chunk = [0u8; 64 * 1024];
        loop {
            match r.read(&mut chunk) {
                Ok(0) => return Ok(false),
                Ok(n) => {
                    self.buf.extend_from_slice(&chunk[..n]);
                    return Ok(true);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }

    /// The next whole frame's message, if one has arrived.
    pub fn next_frame(&mut self) -> io::Result<Option<Vec<u8>>> {
        let Some(head) = self.buf.get(..4) else { return Ok(None) };
        let len = u32::from_be_bytes(head.try_into().expect("4 bytes")) as usize;
        if len > self.max {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {len} bytes")));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let frame = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(frame))
    }
}

/// Whether an I/O error is a read timeout.
pub fn is_timeout(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut)
}

// --- the server's side -----------------------------------------------------------------------

/// What a client's command needs from the server beyond the engine.
#[derive(Debug)]
pub enum Request {
    /// Passed to the engine already; its alerts are the answer.
    Forwarded,
    /// Answer with a [`state_update_frame`] of what changed since this client's last one.
    PostTorrentUpdates,
    /// Answer with a [`session_stats_frame`].
    PostSessionStats,
    /// New engine settings: the server applies them its own way (and keeps them).
    ApplySettings(Box<Settings>),
}

/// Decodes a client's frame and passes the command to `handle`, or returns what the server
/// must do itself. `None` for a frame that is not a command of this protocol version.
pub fn dispatch(handle: &SessionHandle, frame: &[u8]) -> Option<Request> {
    Some(match wire::decode_command(frame)? {
        Command::PostTorrentUpdates => Request::PostTorrentUpdates,
        Command::PostSessionStats => Request::PostSessionStats,
        Command::ApplySettings(s) => Request::ApplySettings(s),
        cmd => {
            handle.send(cmd);
            Request::Forwarded
        }
    })
}

// --- the client's side -----------------------------------------------------------------------

/// Where a remote session connects to.
#[derive(Clone, Debug)]
pub struct Target {
    pub host: String,
    pub port: u16,
    /// `None` for plain HTTP.
    pub tls: Option<Verify>,
    pub username: String,
    pub password: String,
}

/// Why [`connect`] failed.
#[derive(Debug)]
pub enum ConnectError {
    /// The network, or TLS (a certificate that is not trusted or not the pinned one).
    Io(io::Error),
    /// The server answered the upgrade with this status: 401 for a wrong login, 403 for a
    /// banned address or a host name it does not know, 404 for a server without remote
    /// sessions, 426 for another protocol version.
    Refused { status: u16, message: String },
    /// The server's answer made no sense.
    Protocol(String),
}

impl std::fmt::Display for ConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectError::Io(e) => write!(f, "{e}"),
            ConnectError::Refused { status: 401, .. } => f.write_str("wrong user name or password"),
            ConnectError::Refused { status, message } if message.is_empty() => {
                write!(f, "the server refused the connection (HTTP {status})")
            }
            ConnectError::Refused { status, message } => {
                write!(f, "the server refused the connection (HTTP {status}): {message}")
            }
            ConnectError::Protocol(m) => write!(f, "not a PaTorrent remote session: {m}"),
        }
    }
}

impl std::error::Error for ConnectError {}

impl From<io::Error> for ConnectError {
    fn from(e: io::Error) -> ConnectError {
        ConnectError::Io(e)
    }
}

/// A plain or TLS connection.
enum Conn {
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

/// Opens a TCP connection to `host:port` within `timeout`, trying each address in turn.
fn tcp_connect(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let mut last = io::Error::new(io::ErrorKind::NotFound, format!("{host}: no address"));
    for addr in (host, port).to_socket_addrs()? {
        match TcpStream::connect_timeout(&addr, timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// The fingerprint of the certificate a TLS server at `host:port` presents, without sending
/// it anything: to ask the user before connecting to a self-signed server ([`Verify::Pinned`]).
pub fn probe_certificate(host: &str, port: u16, timeout: Duration) -> io::Result<tls::Fingerprint> {
    let sock = tcp_connect(host, port, timeout)?;
    sock.set_read_timeout(Some(timeout))?;
    sock.set_write_timeout(Some(timeout))?;
    tls::probe(sock, host)
}

/// The `Host` header for `host:port` (IPv6 literals in brackets).
fn host_header(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Connects to a nox and returns a [`Session`] driving its engine, and the server's [`Hello`].
/// `timeout` bounds the connection, the handshake and the login. Dropping the session (or
/// [`Session::shutdown`]) closes the connection; the server's torrents keep running.
pub fn connect(target: &Target, timeout: Duration) -> Result<(Session, Hello), ConnectError> {
    let sock = tcp_connect(&target.host, target.port, timeout)?;
    sock.set_nodelay(true)?;
    sock.set_read_timeout(Some(timeout))?;
    sock.set_write_timeout(Some(timeout))?;
    let mut conn = match target.tls {
        None => Conn::Plain(sock),
        Some(verify) => {
            let mut s = tls::Stream::connect(sock, &target.host, verify)?;
            s.handshake()?;
            Conn::Tls(Box::new(s))
        }
    };
    let auth =
        crate::util::string::base64encode(format!("{}:{}", target.username, target.password).as_bytes());
    let request = format!(
        "GET {PATH} HTTP/1.1\r\nHost: {}\r\nConnection: Upgrade\r\nUpgrade: {UPGRADE}\r\n\
         {VERSION_HEADER}: {PROTOCOL}\r\nAuthorization: Basic {auth}\r\n\r\n",
        host_header(&target.host, target.port)
    );
    conn.write_all(request.as_bytes())?;
    conn.flush()?;

    // the response head
    let deadline = Instant::now() + timeout;
    let mut reader = FrameReader::new(Vec::new(), MAX_FRAME);
    let end = loop {
        if let Some(p) = reader.buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        if reader.buf.len() > 16 * 1024 {
            return Err(ConnectError::Protocol("an answer head over 16 KB".into()));
        }
        if !reader.read_from(&mut conn)? {
            return Err(ConnectError::Protocol("the server closed the connection".into()));
        }
    };
    let head = String::from_utf8_lossy(&reader.buf[..end]).into_owned();
    reader.buf.drain(..end);
    let status: u16 = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .filter(|_| head.starts_with("HTTP/1."))
        .ok_or_else(|| ConnectError::Protocol("not an HTTP answer".into()))?;
    if status != 101 {
        // the body says why (a few hundred bytes at most): what has arrived, then a little more
        let len: usize = head
            .lines()
            .find_map(|l| l.split_once(':').filter(|(n, _)| n.trim().eq_ignore_ascii_case("content-length")))
            .and_then(|(_, v)| v.trim().parse().ok())
            .unwrap_or(0)
            .min(4096);
        while reader.buf.len() < len && Instant::now() < deadline {
            if !matches!(reader.read_from(&mut conn), Ok(true)) {
                break;
            }
        }
        let message = String::from_utf8_lossy(&reader.buf[..len.min(reader.buf.len())]).trim().to_owned();
        return Err(ConnectError::Refused { status, message });
    }

    // the hello
    let hello = loop {
        if let Some(frame) = reader.next_frame()? {
            match decode_hello(&frame) {
                Some((PROTOCOL, hello)) => break hello,
                Some((v, _)) => {
                    return Err(ConnectError::Protocol(format!(
                        "protocol {v}, this client speaks {PROTOCOL}"
                    )));
                }
                None => return Err(ConnectError::Protocol("no hello".into())),
            }
        }
        if Instant::now() >= deadline {
            return Err(ConnectError::Io(io::ErrorKind::TimedOut.into()));
        }
        if !reader.read_from(&mut conn)? {
            return Err(ConnectError::Protocol("the server closed the connection".into()));
        }
    };

    conn.tcp().set_read_timeout(Some(PUMP_POLL))?;
    let settings = Settings { alert_queue_size: hello.settings.alert_queue_size, ..Settings::default() };
    let session =
        Session::spawn(settings, Box::new(|| {}), move |rx, shared, _| pump(rx, shared, conn, reader));
    Ok((session, hello))
}

/// The client's network thread: queued commands out, the server's alerts in.
fn pump(rx: mpsc::Receiver<Command>, shared: Arc<Shared>, mut conn: Conn, mut reader: FrameReader) {
    let lost = 'connected: loop {
        let mut out = Vec::new();
        loop {
            match rx.try_recv() {
                Ok(Command::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => {
                    // what was queued before the shutdown still goes out
                    if !out.is_empty() {
                        let _ = conn.write_all(&out).and_then(|()| conn.flush());
                    }
                    let _ = conn.tcp().shutdown(std::net::Shutdown::Both);
                    return;
                }
                Ok(cmd) => {
                    let mut e = Enc::default();
                    if wire::encode_command(&cmd, &mut e) {
                        out.extend_from_slice(&(e.0.len() as u32).to_be_bytes());
                        out.extend_from_slice(&e.0);
                    } else {
                        shared.alerts.post(Alert::Log {
                            id: None,
                            message: "the IP filter is the server's: not set on a remote session".into(),
                        });
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        if !out.is_empty() {
            if let Err(e) = conn.write_all(&out).and_then(|()| conn.flush()) {
                break 'connected e.to_string();
            }
        }
        match reader.read_from(&mut conn) {
            Ok(true) => {}
            Ok(false) => break 'connected "the server closed the connection".to_owned(),
            Err(e) if is_timeout(&e) => {}
            Err(e) => break 'connected e.to_string(),
        }
        loop {
            match reader.next_frame() {
                Ok(Some(frame)) => match wire::decode_alert(&frame) {
                    Some(a) => {
                        shared.alerts.post(a);
                    }
                    None => {
                        shared.alerts.post(Alert::Log {
                            id: None,
                            message: "remote session: a message this version does not know".into(),
                        });
                    }
                },
                Ok(None) => break,
                Err(e) => break 'connected e.to_string(),
            }
        }
    };
    let _ = conn.tcp().shutdown(std::net::Shutdown::Both);
    shared.alerts.post(Alert::RemoteDisconnected { message: lost });
    // commands go nowhere now; wait for the frontend to shut the session down
    while !matches!(rx.recv(), Ok(Command::Shutdown) | Err(_)) {}
}
