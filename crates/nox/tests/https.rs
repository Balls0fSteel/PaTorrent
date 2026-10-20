//! The Web UI over HTTPS, over loopback: a generated self-signed certificate (made once, kept
//! in the config directory, the same after a restart), the page, a login whose cookie is
//! `Secure`, keep-alive requests on one TLS connection, the WebSocket upgrade and its first
//! frame, a plain-HTTP request sent to the `https://` address, and a client pinned to another
//! certificate.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use patorrent_core::tls::{self, Fingerprint, Verify};
use patorrent_core::{Session, SessionParams, Settings};
use patorrent_nox::webui::https::{self, Certificate};
use patorrent_nox::webui::{Config, WebUi};

const PASS: &str = "s3cret-pass";

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("patorrent-https-test-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn settings() -> Settings {
    Settings {
        listen_interfaces: "127.0.0.1:0".into(),
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        ..Settings::default()
    }
}

fn start(session: &Session, dir: &Path, https: Certificate) -> WebUi {
    let mut c = Config::new("127.0.0.1:0".parse().unwrap(), settings(), dir.join("dl").to_string_lossy());
    c.password = Some(PASS.into());
    c.state_dir = Some(dir.join("state"));
    c.pbkdf2_iterations = 2;
    c.https = Some(https);
    WebUi::start(session.handle(), c).unwrap()
}

fn generated(dir: &Path) -> Certificate {
    Certificate::Generated {
        dir: Some(dir.join("state")),
        names: https::default_names("127.0.0.1:0".parse().unwrap()),
    }
}

fn connect(addr: SocketAddr, pin: Fingerprint) -> tls::Stream<TcpStream> {
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    tls::Stream::connect(s, "127.0.0.1", Verify::Pinned(pin)).unwrap()
}

struct Resp {
    status: u16,
    head: String,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<&str> {
        self.head
            .split("\r\n")
            .filter_map(|l| l.split_once(':'))
            .find_map(|(n, v)| n.trim().eq_ignore_ascii_case(name).then_some(v.trim()))
    }
}

/// Reads one response (head, then a `Content-Length` body) from `s`, leaving the rest in `buf`.
fn read_response(s: &mut impl Read, buf: &mut Vec<u8>) -> Resp {
    let mut chunk = [0u8; 16 * 1024];
    let end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        let n = s.read(&mut chunk).unwrap();
        assert!(n > 0, "closed before a response head: {}", String::from_utf8_lossy(buf));
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).into_owned();
    let status = head.split(' ').nth(1).unwrap().parse().unwrap();
    let mut r = Resp { status, head, body: Vec::new() };
    let len: usize = r.header("content-length").map_or(0, |v| v.parse().unwrap());
    while buf.len() < end + len {
        let n = s.read(&mut chunk).unwrap();
        assert!(n > 0, "closed in a body");
        buf.extend_from_slice(&chunk[..n]);
    }
    r.body = buf[end..end + len].to_vec();
    buf.drain(..end + len);
    r
}

fn request(host: &str, method: &str, path: &str, extra: &str, body: &str) -> Vec<u8> {
    format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n{extra}Content-Length: {}\r\n\r\n{body}", body.len())
        .into_bytes()
}

#[test]
fn the_web_ui_over_https() {
    let dir = temp_dir("serve");
    let session = Session::start(SessionParams { settings: settings(), state_dir: None }).unwrap();
    let web = start(&session, &dir, generated(&dir));
    let info = web.https().expect("serving HTTPS").clone();
    assert!(info.generated);
    assert_eq!(info.cert_path.as_deref(), Some(dir.join("state").join(https::CERT_FILE).as_path()));
    let addr = web.local_addr();
    let host = addr.to_string();
    assert_eq!(web.url(), format!("https://{host}/"));

    // the page and a login on one connection (keep-alive), the cookie only for HTTPS
    let mut c = connect(addr, info.fingerprint);
    let mut buf = Vec::new();
    c.write_all(&request(&host, "GET", "/", "", "")).unwrap();
    c.flush().unwrap();
    let page = read_response(&mut c, &mut buf);
    assert_eq!(page.status, 200);
    assert!(String::from_utf8_lossy(&page.body).contains("<title>PaTorrent</title>"));
    assert_eq!(c.peer_fingerprint(), Some(info.fingerprint));
    let form = format!("username=admin&password={PASS}");
    let extra = "Content-Type: application/x-www-form-urlencoded\r\n";
    c.write_all(&request(&host, "POST", "/api/v2/auth/login", extra, &form)).unwrap();
    c.flush().unwrap();
    let login = read_response(&mut c, &mut buf);
    assert_eq!((login.status, login.body.as_slice()), (200, &b"Ok."[..]));
    let cookie = login.header("set-cookie").unwrap().to_owned();
    assert!(cookie.contains("; Secure;") && cookie.contains("HttpOnly"), "{cookie}");
    let sid = cookie.split(';').next().unwrap().to_owned();
    // a large static file, then an API call, pipelined
    let mut two = request(&host, "GET", "/app.js", "", "");
    two.extend(request(&host, "GET", "/api/v2/app/version", &format!("Cookie: {sid}\r\n"), ""));
    c.write_all(&two).unwrap();
    c.flush().unwrap();
    let js = read_response(&mut c, &mut buf);
    assert_eq!(js.status, 200);
    assert!(js.body.len() > 50_000, "{}", js.body.len());
    let version = read_response(&mut c, &mut buf);
    assert_eq!((version.status, version.body.as_slice()), (200, &b"v5.0.0"[..]));

    // the WebSocket: the upgrade and the hello frame
    let mut ws = connect(addr, info.fingerprint);
    let upgrade = format!(
        "GET /pa/ws HTTP/1.1\r\nHost: {host}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
         Origin: https://{host}\r\nCookie: {sid}\r\n\r\n"
    );
    ws.write_all(upgrade.as_bytes()).unwrap();
    ws.flush().unwrap();
    let mut wbuf = Vec::new();
    let r = read_response(&mut ws, &mut wbuf);
    assert_eq!(r.status, 101, "{}", r.head);
    let mut chunk = [0u8; 4096];
    let payload = loop {
        if wbuf.len() >= 4 {
            assert_eq!(wbuf[0], 0x81, "a final text frame");
            let (len, start) = match wbuf[1] {
                126 => (u16::from_be_bytes([wbuf[2], wbuf[3]]) as usize, 4),
                127 => panic!("a hello this large"),
                n => (n as usize, 2),
            };
            if wbuf.len() >= start + len {
                break String::from_utf8(wbuf[start..start + len].to_vec()).unwrap();
            }
        }
        let n = ws.read(&mut chunk).unwrap();
        assert!(n > 0);
        wbuf.extend_from_slice(&chunk[..n]);
    };
    assert!(payload.contains("\"hello\""), "{payload}");

    // plain HTTP to the HTTPS port: sent to the https:// address
    let mut plain = TcpStream::connect(addr).unwrap();
    plain.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    plain.write_all(&request(&host, "GET", "/", "", "")).unwrap();
    let r = read_response(&mut plain, &mut Vec::new());
    assert_eq!(r.status, 301);
    assert_eq!(r.header("location"), Some(format!("https://{host}/").as_str()));

    // a client that pinned another certificate does not get through
    let other = tls::generate_self_signed(&["localhost"], 1).unwrap();
    let other = tls::certificate_info(other.cert_pem.as_bytes()).unwrap().fingerprint;
    let s = TcpStream::connect(addr).unwrap();
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let mut wrong = tls::Stream::connect(s, "127.0.0.1", Verify::Pinned(other)).unwrap();
    let e = wrong.handshake().unwrap_err();
    assert!(e.to_string().contains(&info.fingerprint.to_string()), "{e}");

    // the next start keeps the certificate
    drop((c, ws, wrong));
    drop(web);
    let web = start(&session, &dir, generated(&dir));
    let again = web.https().unwrap();
    assert!(!again.generated);
    assert_eq!(again.fingerprint, info.fingerprint);
    drop(web);
    drop(session);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_certificate_that_does_not_load_stops_the_start() {
    let dir = temp_dir("bad");
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../core/tests/fixtures/tls");
    let session = Session::start(SessionParams { settings: settings(), state_dir: None }).unwrap();
    let mut c = Config::new("127.0.0.1:0".parse().unwrap(), settings(), dir.join("dl").to_string_lossy());
    c.password = Some(PASS.into());
    c.pbkdf2_iterations = 2;
    c.https =
        Some(Certificate::Files { cert: fixtures.join("ec-cert.pem"), key: fixtures.join("rsa-pkcs1.pem") });
    let e = WebUi::start(session.handle(), c).err().expect("a key of another certificate");
    assert!(e.to_string().contains("does not belong"), "{e}");
    let _ = std::fs::remove_dir_all(&dir);
}
