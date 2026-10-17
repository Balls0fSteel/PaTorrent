//! Listen sockets follow `listen_interfaces` without a restart: a socket that is still wanted
//! stays open, new entries open beside it, dropped ones close (libtorrent
//! `reopen_listen_sockets`), and `reopen_network_sockets` keeps everything that still matches.

use std::collections::VecDeque;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use patorrent_core::{Alert, Session, SessionHandle, SessionParams, Settings};

fn settings(listen: &str) -> Settings {
    Settings {
        listen_interfaces: listen.into(),
        tick_interval: 100,
        aio_threads: 2,
        enable_dht: false,
        enable_lsd: false,
        enable_upnp: false,
        enable_natpmp: false,
        ..Settings::default()
    }
}

/// Alerts, kept across waits: a wait stops at the alert it wants and leaves the rest queued.
struct Alerts {
    h: SessionHandle,
    pending: VecDeque<Alert>,
}

impl Alerts {
    fn wait<T>(&mut self, secs: u64, what: &str, mut f: impl FnMut(&Alert) -> Option<T>) -> T {
        let deadline = Instant::now() + Duration::from_secs(secs);
        let mut seen = Vec::new();
        let mut buf = Vec::new();
        loop {
            self.h.pop_alerts(&mut buf);
            self.pending.extend(buf.drain(..));
            while let Some(a) = self.pending.pop_front() {
                if !matches!(a, Alert::StateUpdate(_)) {
                    seen.push(a.message());
                }
                if let Some(v) = f(&a) {
                    return v;
                }
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; alerts: {seen:#?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Every alert for `ms` milliseconds.
    fn drain_for(&mut self, ms: u64) -> Vec<Alert> {
        let deadline = Instant::now() + Duration::from_millis(ms);
        let mut buf = Vec::new();
        while Instant::now() < deadline {
            self.h.pop_alerts(&mut buf);
            self.pending.extend(buf.drain(..));
            std::thread::sleep(Duration::from_millis(20));
        }
        self.pending.drain(..).collect()
    }
}

fn tcp_listening(a: &Alert) -> Option<SocketAddr> {
    match a {
        Alert::ListenSucceeded { address, protocol } if format!("{protocol:?}") == "Tcp" => Some(*address),
        _ => None,
    }
}

fn closing(a: &Alert) -> Option<String> {
    match a {
        Alert::Log { message, .. } if message.contains("closing listen socket") => Some(message.clone()),
        _ => None,
    }
}

fn has_ipv6_loopback() -> bool {
    std::net::TcpListener::bind("[::1]:0").is_ok()
}

#[test]
fn listen_interfaces_changes_reopen_only_what_changed() {
    let s = Session::start(SessionParams { settings: settings("127.0.0.1:0"), state_dir: None }).unwrap();
    let mut alerts = Alerts { h: s.handle(), pending: VecDeque::new() };
    let v4 = alerts.wait(5, "IPv4 listener", tcp_listening);
    assert_eq!(v4.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    alerts.drain_for(200);

    if has_ipv6_loopback() {
        // a second entry: it opens on the same port, the first socket stays
        s.handle().apply_settings(settings("127.0.0.1:0,[::1]:0"));
        let v6 = alerts.wait(5, "IPv6 listener", |a| {
            assert!(closing(a).is_none(), "{a:?}");
            tcp_listening(a)
        });
        assert_eq!(v6, SocketAddr::new("::1".parse().unwrap(), v4.port()));
        drop(TcpStream::connect(v6).expect("the IPv6 listener accepts"));
        drop(TcpStream::connect(v4).expect("the IPv4 listener is still open"));
        alerts.drain_for(200);

        // the first entry goes: only its socket closes
        s.handle().apply_settings(settings("[::1]:0"));
        let msg = alerts.wait(5, "IPv4 socket closed", closing);
        assert!(msg.contains("127.0.0.1"), "{msg}");
        let rest = alerts.drain_for(300);
        assert!(rest.iter().all(|a| closing(a).is_none() && tcp_listening(a).is_none()), "{rest:?}");
        assert!(TcpStream::connect_timeout(&v4, Duration::from_secs(3)).is_err(), "IPv4 is closed");
        drop(TcpStream::connect(v6).expect("the IPv6 listener is still open"));
    }

    // a forced reopen with nothing changed keeps the sockets
    s.handle().reopen_network_sockets();
    let rest = alerts.drain_for(500);
    assert!(rest.iter().all(|a| closing(a).is_none() && tcp_listening(a).is_none()), "{rest:?}");

    // back to the IPv4 entry alone: the IPv6 socket closes, IPv4 reopens (on the kept port)
    s.handle().apply_settings(settings("127.0.0.1:0"));
    let again = alerts.wait(5, "listener on a new entry", tcp_listening);
    assert_eq!(again.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    drop(TcpStream::connect(again).expect("listening again"));
    s.shutdown();
}

#[test]
fn incoming_tcp_can_be_switched_off_and_on() {
    let s = Session::start(SessionParams { settings: settings("127.0.0.1:0"), state_dir: None }).unwrap();
    let mut alerts = Alerts { h: s.handle(), pending: VecDeque::new() };
    let tcp = alerts.wait(5, "listener", tcp_listening);
    alerts.drain_for(200);
    // no incoming TCP: the listener closes (the UDP socket reopens for uTP)
    s.handle().apply_settings(Settings { enable_incoming_tcp: false, ..settings("127.0.0.1:0") });
    alerts.wait(5, "listener closed", closing);
    let rest = alerts.drain_for(300);
    assert!(rest.iter().all(|a| tcp_listening(a).is_none()), "{rest:?}");
    assert!(TcpStream::connect_timeout(&tcp, Duration::from_secs(3)).is_err());
    s.handle().apply_settings(settings("127.0.0.1:0"));
    let again = alerts.wait(5, "listener again", tcp_listening);
    drop(TcpStream::connect(again).expect("listening again"));
    s.shutdown();
}
