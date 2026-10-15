// LibreTorrent's own OS glue, not ported: libtorrent wraps sockets in Boost.Asio's ssl::stream on
// top of a bundled or linked OpenSSL. We use the TLS library the OS ships instead (PLAN.md D9):
// SChannel on Windows, the system OpenSSL / LibreSSL on Unix.

//! TLS client sessions for HTTPS trackers.
//!
//! A [`TlsSession`] is sans-IO: ciphertext from the socket goes in through `feed`, plaintext to
//! send goes in through `encrypt`, and ciphertext to send accumulates in `outgoing`. That fits
//! SChannel, which only works on buffers, and OpenSSL with memory BIOs alike. [`TlsStream`] puts
//! a session on top of a non-blocking socket and offers plain `Read` / `Write`, so the HTTP code
//! does not change: the handshake simply makes writes return `WouldBlock` until it is done.
//!
//! The certificate chain is verified against the OS trust store and the certificate must match
//! the host name (or IP address) that was asked for. SNI is sent for host names.

use std::io::{self, Read, Write};

#[cfg(unix)]
mod openssl;
#[cfg(windows)]
mod schannel;

/// The OS backend's session type.
#[cfg(windows)]
pub(crate) type Session = schannel::Session;
#[cfg(unix)]
pub(crate) type Session = openssl::Session;
#[cfg(not(any(windows, unix)))]
pub(crate) type Session = unsupported::Session;

/// One TLS client connection, independent of any socket.
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
}

/// Starts a client session for `host`. The first handshake message is ready in `outgoing`.
pub(crate) fn client(host: &str) -> io::Result<Session> {
    Session::new(host)
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
}

impl<S: Read + Write, T: TlsSession> TlsStream<S, T> {
    pub fn new(sock: S, tls: T) -> TlsStream<S, T> {
        TlsStream { sock, tls, plain: Vec::new(), plain_pos: 0, eof: false }
    }

    pub fn get_mut(&mut self) -> &mut S {
        &mut self.sock
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
            n => self.tls.feed(&chunk[..n], &mut self.plain),
        }
    }
}

impl<S: Read + Write, T: TlsSession> Read for TlsStream<S, T> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
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
            // A close without close_notify is accepted as the end of the stream: HTTP bodies are
            // framed by Content-Length or chunking, which catches truncation in practice.
            if self.eof || self.tls.closed() {
                return Ok(0);
            }
            self.read_sock()?;
        }
    }
}

impl<S: Read + Write, T: TlsSession> Write for TlsStream<S, T> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
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
        self.tls.encrypt(&buf[..n])?;
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

    pub(crate) struct Session;

    impl Session {
        pub fn new(_host: &str) -> io::Result<Session> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "TLS is not available on this platform"))
        }
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
    }

    impl XorSession {
        fn new() -> XorSession {
            XorSession { handshaking: true, out: b"H".to_vec(), closed: false }
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
