// PaTorrent's own OS glue, not ported: libtorrent wraps sockets in Boost.Asio's ssl::stream on
// top of a bundled or linked OpenSSL. We use the TLS library the OS ships instead (PLAN.md D9):
// SChannel on Windows, Secure Transport on macOS, the system OpenSSL / LibreSSL on other Unixes.

//! TLS sessions: clients for HTTPS trackers, web seeds and fetches, servers for the Web UI.
//!
//! A [`TlsSession`] is sans-IO: ciphertext from the socket goes in through `feed`, plaintext to
//! send goes in through `encrypt`, and ciphertext to send accumulates in `outgoing`. That fits
//! SChannel and Secure Transport, which only work on buffers, and OpenSSL with memory BIOs alike.
//! [`TlsStream`] puts a session on top of a non-blocking socket and offers plain `Read` /
//! `Write`, so the HTTP code does not change: the handshake simply makes writes return
//! `WouldBlock` until it is done.
//!
//! A client verifies the certificate chain against the OS trust store, and the certificate must
//! match the host name (or IP address) that was asked for ([`Verify::System`]), or it must be
//! one certificate the user accepted ([`Verify::Pinned`]). SNI is sent for host names.
//!
//! A server ([`Stream::accept`]) presents an [`Identity`]: a certificate (and its chain) with
//! its private key, from PEM files or [`generate_self_signed`]. It asks for no client
//! certificate, and speaks TLS 1.2 and later only.

use std::fmt;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;

use crate::crypto::Sha256;

#[cfg(all(unix, not(target_vendor = "apple")))]
mod openssl;
mod pem;
#[cfg(windows)]
mod schannel;
#[cfg(target_vendor = "apple")]
mod secure_transport;

/// The OS backend's session type.
#[cfg(windows)]
pub(crate) type Session = schannel::Session;
#[cfg(target_vendor = "apple")]
pub(crate) type Session = secure_transport::Session;
#[cfg(all(unix, not(target_vendor = "apple")))]
pub(crate) type Session = openssl::Session;
#[cfg(not(any(windows, unix)))]
pub(crate) type Session = unsupported::Session;

#[cfg(all(unix, not(target_vendor = "apple")))]
use openssl as backend;
#[cfg(windows)]
use schannel as backend;
#[cfg(target_vendor = "apple")]
use secure_transport as backend;
#[cfg(not(any(windows, unix)))]
use unsupported as backend;

/// The SHA-256 of a certificate's DER encoding: what browsers show as its fingerprint, and what
/// [`Verify::Pinned`] compares.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub [u8; 32]);

impl Fingerprint {
    pub fn of(cert_der: &[u8]) -> Fingerprint {
        Fingerprint(Sha256::digest(cert_der).0)
    }

    /// Parses 64 hex digits, with or without `:`, `-` or spaces between the bytes, in either
    /// case (`AB:CD:…` as browsers and [`Display`](fmt::Display) show it, or `abcd…`).
    pub fn parse(s: &str) -> Option<Fingerprint> {
        let hex: String = s.chars().filter(|c| !matches!(c, ':' | '-' | ' ')).collect();
        let mut out = [0u8; 32];
        crate::util::hex::decode_into(&hex, &mut out).then_some(Fingerprint(out))
    }
}

/// Upper-case hex bytes separated by colons.
impl fmt::Display for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, b) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str(":")?;
            }
            write!(f, "{b:02X}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for Fingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Fingerprint({self})")
    }
}

/// How a client checks the server's certificate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verify {
    /// A chain to the OS trust store, issued for the host name that was asked for.
    System,
    /// Exactly this certificate, whoever issued it and for whatever name: a self-signed one the
    /// user has accepted.
    Pinned(Fingerprint),
}

/// How a client session checks the server: [`Verify`], or (for [`probe`]) not at all.
///
/// Revocation (SEC-07) is soft-fail: only a certificate known to be revoked is refused, never
/// one whose status cannot be learnt. SChannel and Secure Transport check it; the OpenSSL
/// back end does not (it has no revocation source of its own, like most Linux programs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Check {
    /// [`Verify::System`]; revocation only from what the OS has cached, so a handshake never
    /// waits for the network: the engine thread drives trackers and web seeds, and a lookup
    /// would also bypass the user's proxy.
    System,
    /// [`Verify::System`], and revocation may be looked up online: for a blocking caller on a
    /// thread of its own that uses no proxy ([`client_online`]).
    SystemOnline,
    Pinned(Fingerprint),
    Any,
}

impl From<Verify> for Check {
    fn from(v: Verify) -> Check {
        match v {
            Verify::System => Check::System,
            Verify::Pinned(f) => Check::Pinned(f),
        }
    }
}

/// Runs a TLS handshake that accepts any certificate, returns the fingerprint of the one the
/// server presented, and drops the connection without sending anything over it: for showing
/// the user a certificate to accept before a password goes out.
pub fn probe<S: Read + Write>(sock: S, host: &str) -> io::Result<Fingerprint> {
    let mut s = TlsStream::new(sock, Session::client(host, Check::Any)?);
    s.handshake()?;
    s.tls.peer_fingerprint().ok_or_else(|| io::Error::other("the server presented no certificate"))
}

/// The first certificate of a PEM file: its fingerprint and validity (Unix seconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CertificateInfo {
    pub fingerprint: Fingerprint,
    pub not_before: i64,
    pub not_after: i64,
}

/// Reads the first certificate in a PEM file.
pub fn certificate_info(cert_pem: &[u8]) -> io::Result<CertificateInfo> {
    info_of(&pem::certificates(cert_pem)?[0])
}

fn info_of(cert_der: &[u8]) -> io::Result<CertificateInfo> {
    let (not_before, not_after) = pem::validity(cert_der)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "the certificate is corrupt"))?;
    Ok(CertificateInfo { fingerprint: Fingerprint::of(cert_der), not_before, not_after })
}

/// A server's certificate (with the chain that follows it in the file) and private key, ready
/// for [`Stream::accept`]. Cheap to clone; every connection shares it.
///
/// On Windows the private key is imported into the user's CNG key store under a name of this
/// process, because SChannel does its key operations in LSASS, which cannot use a key that
/// lives only in this process. The key is deleted when the last clone is dropped; keys left by
/// a process that died are deleted by the next process that loads an identity.
#[derive(Clone)]
pub struct Identity {
    inner: Arc<backend::Identity>,
    info: CertificateInfo,
}

impl Identity {
    /// From the PEM text of a certificate file (the server's certificate first, then any
    /// intermediates) and of a key file (PKCS #8, PKCS #1 or SEC 1, not encrypted). Both may
    /// be the same text.
    pub fn from_pem(cert_pem: &[u8], key_pem: &[u8]) -> io::Result<Identity> {
        let certs = pem::certificates(cert_pem)?;
        let key = pem::private_key(key_pem)?;
        Identity::new(&certs, &key)
    }

    /// [`from_pem`](Identity::from_pem) on two files; errors about a file name it.
    pub fn load(cert: &Path, key: &Path) -> io::Result<Identity> {
        let named = |p: &Path, e: io::Error| io::Error::new(e.kind(), format!("{}: {e}", p.display()));
        let certs = std::fs::read(cert).and_then(|t| pem::certificates(&t)).map_err(|e| named(cert, e))?;
        let block = std::fs::read(key).and_then(|t| pem::private_key(&t)).map_err(|e| named(key, e))?;
        Identity::new(&certs, &block)
    }

    fn new(certs: &[Vec<u8>], key: &pem::Block) -> io::Result<Identity> {
        let info = info_of(&certs[0])?;
        Ok(Identity { inner: Arc::new(backend::Identity::new(certs, key)?), info })
    }

    pub fn fingerprint(&self) -> Fingerprint {
        self.info.fingerprint
    }

    /// The certificate's fingerprint and validity.
    pub fn info(&self) -> CertificateInfo {
        self.info
    }
}

/// A new self-signed certificate and its private key, in PEM.
pub struct SelfSigned {
    pub cert_pem: String,
    pub key_pem: String,
}

/// Makes a self-signed certificate (ECDSA P-256 signed with SHA-256, extended key usage
/// "server authentication") for `names`, each a DNS name or an IP address, valid from a day
/// before now for `days` days. The first name is also the subject's common name. Browsers
/// still warn about it (nobody vouches for it); clients that know its fingerprint can pin it.
pub fn generate_self_signed(names: &[&str], days: u32) -> io::Result<SelfSigned> {
    if names.is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "a certificate needs at least one name"));
    }
    let mut parsed = Vec::with_capacity(names.len());
    for &n in names {
        parsed.push(match n.parse::<std::net::IpAddr>() {
            Ok(ip) => SubjectName::Ip(ip),
            Err(_) if is_dns_name(n) => SubjectName::Dns(n.to_ascii_lowercase()),
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("not a host name or IP address: {n}"),
                ));
            }
        });
    }
    let now = crate::util::time::unix_now();
    let (cert_pem, key_pem) =
        backend::generate_self_signed(names[0], &parsed, now - 86_400, now + i64::from(days) * 86_400)?;
    Ok(SelfSigned { cert_pem, key_pem })
}

/// A subject alternative name of a generated certificate.
#[cfg_attr(not(any(windows, unix)), allow(dead_code))]
pub(crate) enum SubjectName {
    Dns(String),
    Ip(std::net::IpAddr),
}

/// Letters, digits and `-` in dot-separated labels (and a leading `*.`), at most 253 bytes.
fn is_dns_name(s: &str) -> bool {
    let s = s.strip_prefix("*.").unwrap_or(s);
    !s.is_empty()
        && s.len() <= 253
        && s.split('.').all(|l| {
            !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

/// A TLS connection over a socket `S`, client or server. Blocking sockets (with timeouts)
/// and non-blocking ones both work; see [`TlsStream`].
pub struct Stream<S> {
    inner: TlsStream<S, Session>,
}

impl<S: Read + Write> Stream<S> {
    /// The server side of a connection a client opened. Nothing is sent or read yet: the
    /// handshake runs in [`handshake`](Stream::handshake), or within the first read or write.
    pub fn accept(sock: S, identity: &Identity) -> io::Result<Stream<S>> {
        Ok(Stream { inner: TlsStream::new(sock, Session::server(&identity.inner)?) })
    }

    /// The client side, for `host` (a DNS name or an IP address literal, used for SNI and for
    /// [`Verify::System`]). The ClientHello is queued, not sent.
    pub fn connect(sock: S, host: &str, verify: Verify) -> io::Result<Stream<S>> {
        Ok(Stream { inner: TlsStream::new(sock, Session::client(host, verify.into())?) })
    }

    /// Runs the handshake to its end. With a non-blocking socket (or a timeout) `WouldBlock`
    /// (`TimedOut` on Windows) comes back until it is done; call again when the socket is ready.
    pub fn handshake(&mut self) -> io::Result<()> {
        self.inner.handshake()
    }

    pub fn is_handshaking(&self) -> bool {
        self.inner.tls.handshaking()
    }

    /// The fingerprint of the certificate the server presented, once the handshake is done
    /// (clients only).
    pub fn peer_fingerprint(&self) -> Option<Fingerprint> {
        self.inner.tls.peer_fingerprint()
    }

    pub fn get_ref(&self) -> &S {
        &self.inner.sock
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.inner.sock
    }
}

impl<S: Read + Write> Read for Stream<S> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

/// `write` encrypts and sends what the socket takes; with a non-blocking socket or a timeout
/// the rest stays queued, so finish with `flush`, which fails while anything is left.
impl<S: Read + Write> Write for Stream<S> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// One TLS connection, client or server, independent of any socket.
pub(crate) trait TlsSession {
    /// True until the handshake is done, and again while a post-handshake exchange (TLS 1.3
    /// session tickets reach SChannel as a renegotiation) waits for more data. Application data
    /// can only be encrypted when this is false.
    fn handshaking(&self) -> bool;
    /// Takes ciphertext received from the peer. Decrypted application data is appended to `plain`.
    fn feed(&mut self, ciphertext: &[u8], plain: &mut Vec<u8>) -> io::Result<()>;
    /// Encrypts application data into `outgoing`. Only valid once the handshake is done.
    fn encrypt(&mut self, plain: &[u8]) -> io::Result<()>;
    /// Ciphertext waiting to be sent. The caller removes what it has written.
    fn outgoing(&mut self) -> &mut Vec<u8>;
    /// The peer ended the session (close_notify).
    fn closed(&self) -> bool;
    /// A client's view of the server's certificate, once the handshake is done.
    fn peer_fingerprint(&self) -> Option<Fingerprint> {
        None
    }
}

/// Starts a client session for `host` that verifies the server against the OS trust store.
/// The first handshake message is ready in `outgoing`.
pub(crate) fn client(host: &str) -> io::Result<Session> {
    Session::client(host, Check::System)
}

/// [`client`], for a blocking caller off the engine thread that connects without a proxy:
/// revocation may be looked up online ([`Check::SystemOnline`]).
pub(crate) fn client_online(host: &str) -> io::Result<Session> {
    Session::client(host, Check::SystemOnline)
}

/// A TLS session over a socket. Works with blocking and non-blocking sockets; with a
/// non-blocking one, `WouldBlock` from the socket is passed through, and the caller retries on
/// the next readiness event (either direction may be needed during the handshake).
pub(crate) struct TlsStream<S, T = Session> {
    sock: S,
    tls: T,
    /// Decrypted data not read yet, from `plain_pos` on.
    plain: Vec<u8>,
    plain_pos: usize,
    eof: bool,
    /// The session failed (a bad record, a certificate that did not pass): every later call
    /// returns this again, so nothing is ever sent to a server that failed its check, whatever
    /// the session's own state says (SEC-07).
    failed: Option<(io::ErrorKind, String)>,
}

impl<S: Read + Write, T: TlsSession> TlsStream<S, T> {
    pub fn new(sock: S, tls: T) -> TlsStream<S, T> {
        TlsStream { sock, tls, plain: Vec::new(), plain_pos: 0, eof: false, failed: None }
    }

    /// Whether the peer ended the session with close_notify. A connection that just closed did
    /// not: an attacker on the path can cut it, so a body that only the end of the connection
    /// frames is complete only when this is true (SEC-07).
    pub fn closed_cleanly(&self) -> bool {
        self.tls.closed()
    }

    fn check_failed(&self) -> io::Result<()> {
        match &self.failed {
            Some((kind, message)) => Err(io::Error::new(*kind, message.clone())),
            None => Ok(()),
        }
    }

    /// Notes a session error for good and returns it.
    fn fail(&mut self, e: io::Error) -> io::Error {
        self.failed = Some((e.kind(), e.to_string()));
        e
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.sock
    }

    /// Runs the handshake to its end: see [`Stream::handshake`].
    pub fn handshake(&mut self) -> io::Result<()> {
        self.check_failed()?;
        loop {
            self.flush_tls()?;
            if !self.tls.outgoing().is_empty() {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            if !self.tls.handshaking() {
                return Ok(());
            }
            if self.eof || self.tls.closed() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed during the TLS handshake",
                ));
            }
            self.read_sock()?;
        }
    }

    /// Writes pending ciphertext until it is gone or the socket would block.
    fn flush_tls(&mut self) -> io::Result<()> {
        let out = self.tls.outgoing();
        while !out.is_empty() {
            match self.sock.write(out) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    out.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    /// One socket read into the session.
    fn read_sock(&mut self) -> io::Result<()> {
        let mut chunk = [0u8; 16 * 1024];
        match self.sock.read(&mut chunk)? {
            0 if self.tls.handshaking() => Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "connection closed during the TLS handshake",
            )),
            0 => {
                self.eof = true;
                Ok(())
            }
            n => match self.tls.feed(&chunk[..n], &mut self.plain) {
                Err(e) => Err(self.fail(e)),
                ok => ok,
            },
        }
    }
}

impl<S: Read + Write, T: TlsSession> Read for TlsStream<S, T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.check_failed()?;
        loop {
            self.flush_tls()?;
            if self.plain_pos < self.plain.len() {
                let n = buf.len().min(self.plain.len() - self.plain_pos);
                buf[..n].copy_from_slice(&self.plain[self.plain_pos..self.plain_pos + n]);
                self.plain_pos += n;
                if self.plain_pos == self.plain.len() {
                    self.plain.clear();
                    self.plain_pos = 0;
                }
                return Ok(n);
            }
            // A close without close_notify ends the stream too: HTTP bodies are framed by
            // Content-Length or chunking, which catches truncation. A body framed by the close
            // alone needs `closed_cleanly` (SEC-07).
            if self.eof || self.tls.closed() {
                return Ok(0);
            }
            self.read_sock()?;
        }
    }
}

impl<S: Read + Write, T: TlsSession> Write for TlsStream<S, T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.check_failed()?;
        while self.tls.handshaking() {
            self.flush_tls()?;
            if self.eof || self.tls.closed() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed during the TLS handshake",
                ));
            }
            self.read_sock()?;
        }
        let n = buf.len().min(16 * 1024);
        if let Err(e) = self.tls.encrypt(&buf[..n]) {
            return Err(self.fail(e));
        }
        self.flush_tls()?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.flush_tls()?;
        if !self.tls.outgoing().is_empty() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        self.sock.flush()
    }
}

#[cfg(not(any(windows, unix)))]
mod unsupported {
    use std::io;

    use super::{Check, SubjectName};

    pub(crate) struct Session;

    fn unsupported() -> io::Error {
        io::Error::new(io::ErrorKind::Unsupported, "TLS is not available on this platform")
    }

    impl Session {
        pub fn client(_host: &str, _check: Check) -> io::Result<Session> {
            Err(unsupported())
        }
        pub fn server(_identity: &Identity) -> io::Result<Session> {
            Err(unsupported())
        }
    }

    pub(crate) struct Identity;

    impl Identity {
        pub fn new(_certs: &[Vec<u8>], _key: &super::pem::Block) -> io::Result<Identity> {
            Err(unsupported())
        }
    }

    pub(crate) fn generate_self_signed(
        _cn: &str,
        _names: &[SubjectName],
        _not_before: i64,
        _not_after: i64,
    ) -> io::Result<(String, String)> {
        Err(unsupported())
    }

    impl super::TlsSession for Session {
        fn handshaking(&self) -> bool {
            true
        }
        fn feed(&mut self, _: &[u8], _: &mut Vec<u8>) -> io::Result<()> {
            Err(io::ErrorKind::Unsupported.into())
        }
        fn encrypt(&mut self, _: &[u8]) -> io::Result<()> {
            Err(io::ErrorKind::Unsupported.into())
        }
        fn outgoing(&mut self) -> &mut Vec<u8> {
            unreachable!("never constructed")
        }
        fn closed(&self) -> bool {
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// A stand-in session: a two-message "handshake" (client sends `H`, server answers `S`), then
    /// application data XORed with 0x55.
    struct XorSession {
        handshaking: bool,
        out: Vec<u8>,
        closed: bool,
        /// Fails the certificate check after marking the handshake done, the order SEC-07
        /// found in the back ends.
        reject: bool,
    }

    impl XorSession {
        fn new() -> XorSession {
            XorSession { handshaking: true, out: b"H".to_vec(), closed: false, reject: false }
        }
    }

    impl TlsSession for XorSession {
        fn handshaking(&self) -> bool {
            self.handshaking
        }
        fn feed(&mut self, mut data: &[u8], plain: &mut Vec<u8>) -> io::Result<()> {
            if self.handshaking {
                match data.split_first() {
                    Some((b'S', rest)) => {
                        self.handshaking = false;
                        if self.reject {
                            return Err(io::Error::other("the server's certificate is not the accepted one"));
                        }
                        data = rest;
                    }
                    _ => return Err(io::Error::other("bad handshake")),
                }
            }
            for &b in data {
                if b == 0 {
                    self.closed = true;
                    return Ok(());
                }
                plain.push(b ^ 0x55);
            }
            Ok(())
        }
        fn encrypt(&mut self, plain: &[u8]) -> io::Result<()> {
            assert!(!self.handshaking);
            self.out.extend(plain.iter().map(|b| b ^ 0x55));
            Ok(())
        }
        fn outgoing(&mut self) -> &mut Vec<u8> {
            &mut self.out
        }
        fn closed(&self) -> bool {
            self.closed
        }
    }

    /// A non-blocking socket double: reads come from queued segments (`None` = would block),
    /// writes accept at most `write_room` bytes before blocking.
    #[derive(Default)]
    struct FakeSock {
        incoming: VecDeque<Option<Vec<u8>>>,
        written: Vec<u8>,
        write_room: usize,
    }

    impl Read for FakeSock {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.incoming.pop_front() {
                Some(Some(seg)) => {
                    let n = seg.len().min(buf.len());
                    buf[..n].copy_from_slice(&seg[..n]);
                    if n < seg.len() {
                        self.incoming.push_front(Some(seg[n..].to_vec()));
                    }
                    Ok(n)
                }
                Some(None) => Err(io::ErrorKind::WouldBlock.into()),
                None => Ok(0),
            }
        }
    }

    impl Write for FakeSock {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.write_room == 0 {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let n = buf.len().min(self.write_room);
            self.write_room -= n;
            self.written.extend_from_slice(&buf[..n]);
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn xor(b: &[u8]) -> Vec<u8> {
        b.iter().map(|b| b ^ 0x55).collect()
    }

    #[test]
    fn handshake_then_data_over_nonblocking_socket() {
        let sock = FakeSock { incoming: [None].into(), ..Default::default() };
        let mut s = TlsStream::new(sock, XorSession::new());

        // The socket cannot take the client hello yet, and nothing has arrived.
        let e = s.write(b"GET").unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::WouldBlock);
        assert!(s.sock.written.is_empty());

        // Room to write, the server's answer arrives split from the first data byte.
        s.sock.write_room = 1024;
        s.sock.incoming.extend([Some(b"S".to_vec()), Some(xor(b"HT")), None]);
        assert_eq!(s.write(b"GET").unwrap(), 3);
        let mut want = b"H".to_vec();
        want.extend(xor(b"GET"));
        assert_eq!(s.sock.written, want);

        let mut buf = [0u8; 64];
        assert_eq!(s.read(&mut buf).unwrap(), 2);
        assert_eq!(&buf[..2], b"HT");
        assert_eq!(s.read(&mut buf).unwrap_err().kind(), io::ErrorKind::WouldBlock);

        // Data, then close_notify (a 0 byte here) while more bytes follow.
        let mut seg = xor(b"TP/1.1");
        seg.extend([0, 1, 2]);
        s.sock.incoming.push_back(Some(seg));
        let mut got = Vec::new();
        let mut small = [0u8; 4];
        loop {
            match s.read(&mut small).unwrap() {
                0 => break,
                n => got.extend_from_slice(&small[..n]),
            }
        }
        assert_eq!(got, b"TP/1.1");
    }

    #[test]
    fn partial_writes_are_retried_from_the_buffer() {
        let sock = FakeSock { incoming: [Some(b"S".to_vec())].into(), write_room: 1, ..Default::default() };
        let mut s = TlsStream::new(sock, XorSession::new());
        assert_eq!(s.write(b"abcdef").unwrap(), 6);
        assert_eq!(s.sock.written, b"H");
        assert_eq!(s.flush().unwrap_err().kind(), io::ErrorKind::WouldBlock);
        s.sock.write_room = 100;
        s.flush().unwrap();
        let mut want = b"H".to_vec();
        want.extend(xor(b"abcdef"));
        assert_eq!(s.sock.written, want);
    }

    #[test]
    fn eof_during_handshake_is_an_error() {
        let sock = FakeSock { write_room: 100, ..Default::default() };
        let mut s = TlsStream::new(sock, XorSession::new());
        assert_eq!(s.write(b"x").unwrap_err().kind(), io::ErrorKind::UnexpectedEof);
        let sock = FakeSock { write_room: 100, incoming: [Some(b"X".to_vec())].into(), ..Default::default() };
        let mut s = TlsStream::new(sock, XorSession::new());
        assert_eq!(s.write(b"x").unwrap_err().to_string(), "bad handshake");
    }

    /// A session that failed its check stays failed: a caller that tries again gets the error
    /// again and nothing is encrypted, though the session itself thinks the handshake is done
    /// (SEC-07).
    #[test]
    fn a_failed_session_stays_failed() {
        let sock = FakeSock { write_room: 100, incoming: [Some(b"S".to_vec())].into(), ..Default::default() };
        let mut s = TlsStream::new(sock, XorSession { reject: true, ..XorSession::new() });
        let e = s.handshake().unwrap_err();
        assert!(e.to_string().contains("not the accepted one"), "{e}");
        assert!(!s.tls.handshaking(), "the session thinks it is done");
        assert!(s.handshake().is_err());
        assert!(s.write(b"password").is_err());
        assert!(s.read(&mut [0; 8]).is_err());
        assert_eq!(s.sock.written, b"H", "nothing but the hello went out");
    }

    /// The end of the connection is a clean close only after close_notify (SEC-07).
    #[test]
    fn a_close_without_close_notify_is_not_clean() {
        for (tail, clean) in [(vec![], false), (vec![0], true)] {
            let mut seg = b"S".to_vec();
            seg.extend(xor(b"body"));
            seg.extend(tail);
            let sock = FakeSock { write_room: 100, incoming: [Some(seg)].into(), ..Default::default() };
            let mut s = TlsStream::new(sock, XorSession::new());
            let mut got = Vec::new();
            s.read_to_end(&mut got).unwrap();
            assert_eq!(got, b"body");
            assert_eq!(s.closed_cleanly(), clean);
        }
    }

    // --- The OS backend ---------------------------------------------------------------------

    /// The first flight is a TLS handshake record carrying a ClientHello with SNI.
    #[test]
    fn client_hello_carries_sni() {
        let mut s = client("tracker.example.org").expect("TLS backend available");
        assert!(s.handshaking());
        let hello = s.outgoing().clone();
        assert!(hello.len() > 50, "{hello:?}");
        assert_eq!(hello[0], 0x16, "handshake record");
        assert_eq!(hello[1], 0x03);
        assert_eq!(hello[5], 0x01, "ClientHello");
        assert!(hello.windows(19).any(|w| w == b"tracker.example.org"), "SNI host name present");
    }

    /// IP literals get no SNI (RFC 6066) but still start a handshake.
    #[test]
    fn client_hello_for_ip_literal() {
        let mut s = client("127.0.0.1").expect("TLS backend available");
        let hello = s.outgoing().clone();
        assert_eq!((hello[0], hello[5]), (0x16, 0x01));
        assert!(!hello.windows(9).any(|w| w == b"127.0.0.1"));
    }

    /// A plain HTTP server answering a ClientHello is a handshake failure, not a hang.
    #[test]
    fn plaintext_server_fails_the_handshake() {
        let mut s = client("tracker.example.org").expect("TLS backend available");
        let mut plain = Vec::new();
        let r = s.feed(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n", &mut plain);
        assert!(r.is_err(), "garbage accepted");
        assert!(plain.is_empty());
    }

    // --- Servers and pinned clients, over loopback -------------------------------------------

    const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/tls/");

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{FIXTURES}{name}")).unwrap()
    }

    fn reply() -> Vec<u8> {
        (0..100_000u32).map(|i| (i * 7 + i / 251) as u8).collect()
    }

    /// Serves one connection: reads 4 bytes, answers with [`reply`], returns what it read.
    fn serve_once(id: Identity) -> (std::net::SocketAddr, std::thread::JoinHandle<io::Result<Vec<u8>>>) {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            let (sock, _) = l.accept()?;
            sock.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
            let mut s = Stream::accept(sock, &id)?;
            s.handshake()?;
            assert_eq!(s.peer_fingerprint(), None);
            let mut req = [0u8; 4];
            s.read_exact(&mut req)?;
            s.write_all(&reply())?;
            s.flush()?;
            Ok(req.to_vec())
        });
        (addr, h)
    }

    fn fetch(addr: std::net::SocketAddr, host: &str, verify: Verify) -> io::Result<Option<Fingerprint>> {
        let sock = std::net::TcpStream::connect(addr)?;
        sock.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
        let mut c = Stream::connect(sock, host, verify)?;
        c.write_all(b"ping")?;
        c.flush()?;
        let mut got = vec![0u8; 100_000];
        c.read_exact(&mut got)?;
        assert!(got == reply(), "payload differs");
        Ok(c.peer_fingerprint())
    }

    fn round_trip(id: &Identity, host: &str) {
        let (addr, server) = serve_once(id.clone());
        let seen = fetch(addr, host, Verify::Pinned(id.fingerprint())).unwrap();
        assert_eq!(seen, Some(id.fingerprint()));
        assert_eq!(server.join().unwrap().unwrap(), b"ping");
    }

    #[test]
    fn a_generated_certificate_serves_a_pinned_client() {
        let pem = generate_self_signed(&["localhost", "127.0.0.1", "::1"], 30).unwrap();
        assert!(pem.cert_pem.starts_with("-----BEGIN CERTIFICATE-----\n"), "{}", pem.cert_pem);
        assert!(pem.key_pem.starts_with("-----BEGIN PRIVATE KEY-----\n"), "{}", pem.key_pem);
        let info = certificate_info(pem.cert_pem.as_bytes()).unwrap();
        let now = crate::util::time::unix_now();
        assert!((info.not_before - (now - 86_400)).abs() < 60, "{info:?}");
        assert!((info.not_after - (now + 30 * 86_400)).abs() < 60, "{info:?}");
        let id = Identity::from_pem(pem.cert_pem.as_bytes(), pem.key_pem.as_bytes()).unwrap();
        assert_eq!(id.info(), info);
        // the name is not checked when pinned
        round_trip(&id, "127.0.0.1");
        round_trip(&id, "patorrent.example");
    }

    #[test]
    fn keys_in_every_supported_form_serve() {
        for (cert, key) in [
            ("ec-cert.pem", "ec-pkcs8.pem"),
            ("ec-cert.pem", "ec-sec1.pem"),
            ("rsa-cert.pem", "rsa-pkcs1.pem"),
        ] {
            let id =
                Identity::from_pem(&fixture(cert), &fixture(key)).unwrap_or_else(|e| panic!("{key}: {e}"));
            round_trip(&id, "localhost");
        }
        // one file holding both, as some tools write them
        let both = [fixture("rsa-pkcs1.pem"), fixture("rsa-cert.pem")].concat();
        round_trip(&Identity::from_pem(&both, &both).unwrap(), "localhost");
        let id = Identity::load(
            &Path::new(FIXTURES).join("ec-cert.pem"),
            &Path::new(FIXTURES).join("ec-pkcs8.pem"),
        )
        .unwrap();
        assert_eq!(
            id.fingerprint().to_string(),
            "78:B1:27:3E:3E:EA:74:50:73:AD:12:CA:BC:20:04:88:A7:81:5B:08:FE:8C:95:CF:8F:0E:A2:4F:89:E0:7D:D5"
        );
    }

    #[test]
    fn identities_that_cannot_work_are_refused() {
        let e = Identity::from_pem(&fixture("ec-cert.pem"), &fixture("rsa-pkcs1.pem")).err().unwrap();
        assert!(e.to_string().contains("does not belong"), "{e}");
        let e = Identity::from_pem(&fixture("rsa-cert.pem"), &fixture("ec-sec1.pem")).err().unwrap();
        assert!(e.to_string().contains("does not belong"), "{e}");
        let e = Identity::from_pem(&fixture("ec-cert.pem"), &fixture("ec-encrypted.pem")).err().unwrap();
        assert!(e.to_string().contains("encrypted"), "{e}");
        let missing = Path::new(FIXTURES).join("missing.pem");
        let e = Identity::load(&missing, &Path::new(FIXTURES).join("ec-pkcs8.pem")).err().unwrap();
        assert!(e.to_string().contains("missing.pem"), "{e}");
        let e = Identity::load(
            &Path::new(FIXTURES).join("ec-cert.pem"),
            &Path::new(FIXTURES).join("ec-cert.pem"),
        )
        .err()
        .unwrap();
        assert!(e.to_string().contains("ec-cert.pem: no private key"), "{e}");
        assert!(generate_self_signed(&[], 1).is_err());
        assert!(generate_self_signed(&["bad name"], 1).is_err());
        assert!(generate_self_signed(&["x,DNS:evil"], 1).is_err());
    }

    /// The certificate and key encodings the Security.framework backend builds by hand
    /// (`pem::self_signed_tbs`, `pem::p256_pkcs8`), signed through CNG here: CryptoAPI verifies
    /// the self-signature and SChannel serves them.
    #[cfg(windows)]
    #[test]
    fn a_certificate_built_by_hand_serves() {
        use super::pem;
        let key = pem::private_key(&fixture("ec-pkcs8.pem")).unwrap();
        let (_, sec1) = pem::pkcs8_key(&key.der).unwrap();
        let (d, point) = pem::ec_private_key(sec1).unwrap();
        let point = point.unwrap();
        let now = crate::util::time::unix_now();
        let names = [SubjectName::Dns("localhost".into()), SubjectName::Ip("127.0.0.1".parse().unwrap())];
        let serial = [0x5a, 1, 2, 3, 4, 5, 6, 7];
        let tbs = pem::self_signed_tbs("localhost", &names, &serial, now - 86_400, now + 30 * 86_400, point);
        let cert = pem::self_signed_certificate(&tbs, &schannel::ecdsa_sign_for_test(&key.der, &tbs));
        assert!(schannel::self_signature_verifies(&cert));
        let mut bad = cert.clone();
        let n = bad.len();
        bad[n - 5] ^= 1;
        assert!(!schannel::self_signature_verifies(&bad));
        let cert_pem = pem::encode("CERTIFICATE", &cert);
        let key_pem = pem::encode("PRIVATE KEY", &pem::p256_pkcs8(d, point));
        let id = Identity::from_pem(cert_pem.as_bytes(), key_pem.as_bytes()).unwrap();
        assert_eq!(id.info().not_after - id.info().not_before, 31 * 86_400);
        round_trip(&id, "localhost");
    }

    #[test]
    fn a_client_refuses_a_certificate_it_did_not_accept() {
        let id = Identity::from_pem(&fixture("ec-cert.pem"), &fixture("ec-pkcs8.pem")).unwrap();
        // pinned to another certificate: the error names the one presented
        let other = Identity::from_pem(&fixture("rsa-cert.pem"), &fixture("rsa-pkcs1.pem")).unwrap();
        let (addr, server) = serve_once(id.clone());
        let e = fetch(addr, "localhost", Verify::Pinned(other.fingerprint())).unwrap_err();
        assert!(e.to_string().contains(&id.fingerprint().to_string()), "{e}");
        assert!(server.join().unwrap().is_err());
        // verified against the system's roots: a self-signed certificate is not trusted
        let (addr, server) = serve_once(id.clone());
        let e = fetch(addr, "localhost", Verify::System).unwrap_err();
        eprintln!("system verification: {e}");
        assert!(server.join().unwrap().is_err());
        // the back end does not mark the handshake done for a certificate that failed, and a
        // second try fails too: nothing is encrypted for that server (SEC-07)
        let (addr, server) = serve_once(id);
        let sock = std::net::TcpStream::connect(addr).unwrap();
        sock.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
        let mut c = Stream::connect(sock, "localhost", Verify::Pinned(other.fingerprint())).unwrap();
        assert!(c.handshake().is_err());
        assert!(c.is_handshaking(), "the session must not count as established");
        assert!(c.handshake().is_err());
        assert!(c.write_all(b"ping").is_err());
        drop(c);
        assert!(server.join().unwrap().is_err());
    }

    #[test]
    fn a_probe_reports_the_certificate_and_sends_nothing() {
        let id = Identity::from_pem(&fixture("ec-cert.pem"), &fixture("ec-pkcs8.pem")).unwrap();
        let (addr, server) = serve_once(id.clone());
        let sock = std::net::TcpStream::connect(addr).unwrap();
        sock.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
        assert_eq!(probe(sock, "patorrent.example").unwrap(), id.fingerprint());
        // the server got through its handshake, then the connection ended without a byte
        assert!(server.join().unwrap().is_err());
    }

    #[test]
    fn a_plaintext_client_fails_the_server_handshake() {
        let id = Identity::from_pem(&fixture("ec-cert.pem"), &fixture("ec-pkcs8.pem")).unwrap();
        let (addr, server) = serve_once(id);
        let mut sock = std::net::TcpStream::connect(addr).unwrap();
        let start = std::time::Instant::now();
        sock.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").unwrap();
        let e = server.join().unwrap().unwrap_err();
        eprintln!("plain request: {e}");
        // at once, not after the read timeout
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{e}");
    }

    #[test]
    fn fingerprints_print_and_parse() {
        let f = Fingerprint([0xab; 32]);
        let text = f.to_string();
        assert_eq!(text.len(), 32 * 3 - 1);
        assert!(text.starts_with("AB:AB:"));
        assert_eq!(Fingerprint::parse(&text), Some(f));
        assert_eq!(Fingerprint::parse(&text.replace(':', "").to_lowercase()), Some(f));
        assert_eq!(Fingerprint::parse(&text.replace(':', " ")), Some(f));
        assert_eq!(Fingerprint::parse("AB:CD"), None);
        assert_eq!(Fingerprint::parse(&format!("{text}:00")), None);
        assert!(is_dns_name("my-box.lan") && is_dns_name("*.example.org") && is_dns_name("localhost"));
        assert!(!is_dns_name("") && !is_dns_name("a..b") && !is_dns_name("a b") && !is_dns_name("-x,y"));
    }

    // --- Live checks (network): set LT_NETWORK_TESTS=1 and run with --ignored --------------

    fn network() -> bool {
        let on = std::env::var_os("LT_NETWORK_TESTS").is_some();
        if !on {
            eprintln!("skipped: set LT_NETWORK_TESTS=1 to run network tests");
        }
        on
    }

    fn https_get(connect: &str, host: &str, path: &str) -> io::Result<Vec<u8>> {
        let sock = std::net::TcpStream::connect(connect)?;
        sock.set_read_timeout(Some(std::time::Duration::from_secs(20)))?;
        let mut s = TlsStream::new(sock, client(host)?);
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n");
        s.write_all(req.as_bytes())?;
        let mut resp = Vec::new();
        s.read_to_end(&mut resp)?;
        Ok(resp)
    }

    #[test]
    #[ignore = "network"]
    fn live_https_get() {
        if !network() {
            return;
        }
        // Ubuntu's tracker speaks TLS 1.2; Cloudflare offers TLS 1.3, whose session tickets
        // arrive after the handshake (SChannel reports them as a renegotiation).
        for host in ["torrent.ubuntu.com", "www.cloudflare.com"] {
            let resp = https_get(&format!("{host}:443"), host, "/").unwrap();
            assert!(resp.starts_with(b"HTTP/1."), "{host}: {}", String::from_utf8_lossy(&resp));
        }
    }

    /// Connects and completes the handshake with `check`.
    fn live_handshake(host: &str, check: Check) -> io::Result<()> {
        let sock = std::net::TcpStream::connect(format!("{host}:443"))?;
        sock.set_read_timeout(Some(std::time::Duration::from_secs(20)))?;
        TlsStream::new(sock, Session::client(host, check)?).handshake()
    }

    #[test]
    #[ignore = "network"]
    fn live_revoked_certificates_are_refused() {
        if !network() {
            return;
        }
        live_handshake("www.cloudflare.com", Check::SystemOnline).unwrap();
        // test sites whose certificates are revoked (SEC-07); OpenSSL checks no revocation
        for host in ["revoked-rsa-dv.ssl.com", "revoked.badssl.com"] {
            let online = live_handshake(host, Check::SystemOnline);
            // the engine's check, without the network: what the lookup above left in the cache
            let cached = live_handshake(host, Check::System);
            eprintln!("{host}: online {online:?}, cached {cached:?}");
            if cfg!(windows) {
                let e = online.unwrap_err().to_string();
                assert!(e.contains("revoked"), "{host}: {e}");
                assert!(cached.unwrap_err().to_string().contains("revoked"), "{host}");
            }
        }
    }

    #[test]
    #[ignore = "network"]
    fn live_certificate_checks() {
        if !network() {
            return;
        }
        // Valid chain, wrong name: verification must fail.
        let e = https_get("torrent.ubuntu.com:443", "wrong.example.org", "/").unwrap_err();
        eprintln!("wrong host: {e}");
        for bad in ["expired.badssl.com", "self-signed.badssl.com", "untrusted-root.badssl.com"] {
            let e = https_get(&format!("{bad}:443"), bad, "/").unwrap_err();
            eprintln!("{bad}: {e}");
        }
    }
}
