//! Keep ping low's probes (D49, `docs/LOWPING.md`): a thread of its own finds the first router
//! past the home line by TTL-limited echoes and pings it about four times a second; if no hop
//! answers, it pings the fallback hosts in turn. The engine reads the round trips from a channel
//! on its tick, so it never waits for the network.
//!
//! ICMP needs no administrator rights on Windows (`IcmpSendEcho2Ex`, as `tracert`). Linux and
//! macOS (unprivileged ICMP sockets) are the next slice; elsewhere [`Prober::start`] reports
//! that pings are unavailable and the engine sets no caps.

use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

/// What the probe thread reports.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum ProbeEvent {
    /// The pings go to `addr` from now on (`fallback`: one of the fallback hosts, numbered from
    /// 0 in [`ProbeEvent::Rtt`]); `None`: nothing answers, no samples until the next search.
    Target { addr: Option<IpAddr>, hop: Option<u8>, fallback: bool },
    /// A round trip of target `target` (always 0 for a router), in milliseconds.
    Rtt { target: usize, ms: f64 },
}

/// One echo's outcome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Echo {
    /// `from` answered the echo.
    Reply { from: IpAddr, ms: f64 },
    /// The TTL ran out at router `from`.
    TtlExpired { from: IpAddr, ms: f64 },
    /// No answer within the timeout, or an error.
    Lost,
}

/// Sends one echo request and waits for its answer.
pub(crate) trait Pinger: Send {
    fn echo(&mut self, to: IpAddr, ttl: u8, timeout: Duration) -> Echo;
}

/// Pings per second to a router past the line.
const PROBE_INTERVAL: Duration = Duration::from_millis(250);
/// The router is searched for again this often, and after a network change.
const REDISCOVER: Duration = Duration::from_secs(30 * 60);
/// Echoes in a row without an answer before the router is searched for again.
const LOST_LIMIT: u32 = 20;
const TIMEOUT: Duration = Duration::from_millis(1000);
/// The farthest hop searched: the line is within the first few.
const MAX_HOP: u8 = 6;
/// Traced towards when no fallback host is IPv4.
const TRACE_TO: IpAddr = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));

/// Whether `ip` is past the home line: not a private, link-local, loopback or unspecified
/// address. Carrier-grade NAT (100.64.0.0/10) is the provider's, so it counts as past the line.
pub(crate) fn past_the_line(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(a) => {
            !(a.is_private()
                || a.is_link_local()
                || a.is_loopback()
                || a.is_unspecified()
                || a.is_broadcast())
        }
        IpAddr::V6(a) => {
            let first = a.segments()[0];
            // fc00::/7 unique local, fe80::/10 link-local
            !(a.is_loopback() || a.is_unspecified() || first & 0xfe00 == 0xfc00 || first & 0xffc0 == 0xfe80)
        }
    }
}

/// The fallback hosts: the addresses in the comma-separated list (names are not looked up).
pub(crate) fn parse_hosts(list: &str) -> Vec<IpAddr> {
    list.split(',').filter_map(|h| h.trim().trim_matches(['[', ']']).parse().ok()).collect()
}

/// How the thread pings once it searched.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Mode {
    /// Echoes to the router itself.
    Direct(IpAddr),
    /// Echoes towards `to` that expire at hop `ttl`, for a router that does not answer its own
    /// echoes but reports expired ones.
    Expiring {
        to: IpAddr,
        ttl: u8,
        router: IpAddr,
    },
    Fallback,
    Nothing,
}

/// Finds the first router past the line (`tracert` to `to` with TTL 1 to [`MAX_HOP`]), and how
/// to ping it.
fn discover(p: &mut dyn Pinger, to: IpAddr, stop: &AtomicBool) -> Option<(Mode, u8)> {
    for ttl in 1..=MAX_HOP {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        let from = match p.echo(to, ttl, TIMEOUT) {
            Echo::TtlExpired { from, .. } | Echo::Reply { from, .. } => from,
            // a hop that stays silent: try the next one
            Echo::Lost => continue,
        };
        if !past_the_line(from) {
            continue;
        }
        let direct = (0..3).any(|_| matches!(p.echo(from, 64, TIMEOUT), Echo::Reply { .. }));
        let mode = if direct { Mode::Direct(from) } else { Mode::Expiring { to, ttl, router: from } };
        return Some((mode, ttl));
    }
    None
}

/// The probe thread's loop: search, ping until a search is due, repeat. Ends when `stop` is set
/// or the engine dropped the receiver.
fn run(
    mut p: Box<dyn Pinger>,
    hosts: Vec<IpAddr>,
    stop: Arc<AtomicBool>,
    rediscover: Arc<AtomicBool>,
    tx: mpsc::Sender<ProbeEvent>,
) {
    let trace_to = hosts.iter().copied().find(IpAddr::is_ipv4).unwrap_or(TRACE_TO);
    while !stop.load(Ordering::Relaxed) {
        rediscover.store(false, Ordering::Relaxed);
        let (mode, event) = match discover(p.as_mut(), trace_to, &stop) {
            Some((mode @ (Mode::Direct(a) | Mode::Expiring { router: a, .. }), ttl)) => {
                (mode, ProbeEvent::Target { addr: Some(a), hop: Some(ttl), fallback: false })
            }
            _ if !hosts.is_empty() => {
                (Mode::Fallback, ProbeEvent::Target { addr: None, hop: None, fallback: true })
            }
            _ => (Mode::Nothing, ProbeEvent::Target { addr: None, hop: None, fallback: false }),
        };
        if tx.send(event).is_err() {
            return;
        }
        // the fallback hosts share four pings a second, at most one each per 250 ms
        let interval = match mode {
            Mode::Fallback => PROBE_INTERVAL.max(Duration::from_millis(1000 / hosts.len().max(1) as u64)),
            _ => PROBE_INTERVAL,
        };
        let started = Instant::now();
        let mut lost = 0;
        let mut turn = 0;
        while !stop.load(Ordering::Relaxed)
            && !rediscover.load(Ordering::Relaxed)
            && started.elapsed() < REDISCOVER
            && lost < LOST_LIMIT
        {
            let sent = Instant::now();
            let sample = match mode {
                Mode::Direct(a) => match p.echo(a, 64, TIMEOUT) {
                    Echo::Reply { ms, .. } => Some((0, ms)),
                    _ => None,
                },
                Mode::Expiring { to, ttl, router } => match p.echo(to, ttl, TIMEOUT) {
                    Echo::TtlExpired { from, ms } if from == router => Some((0, ms)),
                    _ => None,
                },
                Mode::Fallback => {
                    let i = turn % hosts.len();
                    turn += 1;
                    match p.echo(hosts[i], 64, TIMEOUT) {
                        Echo::Reply { ms, .. } => Some((i, ms)),
                        _ => None,
                    }
                }
                // nothing to ping: wait for a network change or the next search
                Mode::Nothing => {
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }
            };
            match sample {
                Some((target, ms)) => {
                    lost = 0;
                    if tx.send(ProbeEvent::Rtt { target, ms }).is_err() {
                        return;
                    }
                }
                // a fallback host may answer late now and then; only a router going quiet
                // calls for a new search
                None if mode != Mode::Fallback => lost += 1,
                None => {}
            }
            if let Some(rest) = interval.checked_sub(sent.elapsed()) {
                thread::sleep(rest);
            }
        }
    }
}

/// The running probe thread. Dropping it stops the thread (within one echo's timeout).
pub(crate) struct Prober {
    stop: Arc<AtomicBool>,
    rediscover: Arc<AtomicBool>,
    rx: mpsc::Receiver<ProbeEvent>,
}

impl Prober {
    /// Starts pinging; `hosts` are the fallback hosts. `None` where this platform has no
    /// unprivileged pings yet.
    pub fn start(hosts: Vec<IpAddr>) -> Option<Prober> {
        Some(Prober::with_pinger(imp::pinger()?, hosts))
    }

    pub fn with_pinger(pinger: Box<dyn Pinger>, hosts: Vec<IpAddr>) -> Prober {
        let stop = Arc::new(AtomicBool::new(false));
        let rediscover = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let (s, r) = (stop.clone(), rediscover.clone());
        let spawned =
            thread::Builder::new().name("lt-ping".into()).spawn(move || run(pinger, hosts, s, r, tx));
        if spawned.is_err() {
            stop.store(true, Ordering::Relaxed);
        }
        Prober { stop, rediscover, rx }
    }

    /// Searches for the router again (the network changed).
    pub fn rediscover(&self) {
        self.rediscover.store(true, Ordering::Relaxed);
    }

    /// The events reported since the last call.
    pub fn events(&self) -> impl Iterator<Item = ProbeEvent> + '_ {
        self.rx.try_iter()
    }
}

impl Drop for Prober {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(windows)]
mod imp {
    use std::net::{IpAddr, Ipv4Addr, SocketAddrV6};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        ICMP_ECHO_REPLY, ICMPV6_ECHO_REPLY_LH, IP_OPTION_INFORMATION, IP_SUCCESS, IP_TTL_EXPIRED_TRANSIT,
        Icmp6CreateFile, Icmp6SendEcho2, IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho2Ex,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_INET6, SOCKADDR_IN6};

    use super::{Echo, Pinger};

    /// `IcmpSendEcho2Ex` / `Icmp6SendEcho2`, synchronous (no event, no APC routine).
    struct IcmpPinger {
        v4: HANDLE,
        v6: HANDLE,
    }

    // the handles are only used from the probe thread
    unsafe impl Send for IcmpPinger {}

    pub(super) fn pinger() -> Option<Box<dyn Pinger>> {
        // SAFETY: plain calls without arguments; failures return INVALID_HANDLE_VALUE
        let (v4, v6) = unsafe { (IcmpCreateFile(), Icmp6CreateFile()) };
        if v4 == INVALID_HANDLE_VALUE {
            return None;
        }
        Some(Box::new(IcmpPinger { v4, v6 }))
    }

    const PAYLOAD: [u8; 32] = *b"PaTorrent keep-ping-low probe...";

    fn sockaddr6(a: std::net::Ipv6Addr) -> SOCKADDR_IN6 {
        let s = SocketAddrV6::new(a, 0, 0, 0);
        // SAFETY: SOCKADDR_IN6 is plain data
        let mut out: SOCKADDR_IN6 = unsafe { std::mem::zeroed() };
        out.sin6_family = AF_INET6;
        out.sin6_addr.u.Byte = s.ip().octets();
        out
    }

    impl Pinger for IcmpPinger {
        fn echo(&mut self, to: IpAddr, ttl: u8, timeout: Duration) -> Echo {
            let timeout = timeout.as_millis().min(u32::MAX as u128) as u32;
            let options = IP_OPTION_INFORMATION { Ttl: ttl, ..Default::default() };
            // room for one reply, its data and an ICMP error's 8 bytes (as the documentation asks)
            let mut buf = [0u8; 256];
            match to {
                IpAddr::V4(a) => {
                    // SAFETY: the handle is open, the buffers outlive the synchronous call
                    let n = unsafe {
                        IcmpSendEcho2Ex(
                            self.v4,
                            std::ptr::null_mut(),
                            None,
                            std::ptr::null(),
                            0,
                            u32::from_ne_bytes(a.octets()),
                            PAYLOAD.as_ptr().cast(),
                            PAYLOAD.len() as u16,
                            &options,
                            buf.as_mut_ptr().cast(),
                            buf.len() as u32,
                            timeout,
                        )
                    };
                    if n == 0 {
                        return Echo::Lost;
                    }
                    // SAFETY: the call wrote at least one reply at the start of the buffer
                    let r: ICMP_ECHO_REPLY = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
                    let from = IpAddr::V4(Ipv4Addr::from(r.Address.to_ne_bytes()));
                    let ms = r.RoundTripTime as f64;
                    match r.Status {
                        IP_SUCCESS => Echo::Reply { from, ms },
                        IP_TTL_EXPIRED_TRANSIT => Echo::TtlExpired { from, ms },
                        _ => Echo::Lost,
                    }
                }
                IpAddr::V6(a) => {
                    if self.v6 == INVALID_HANDLE_VALUE {
                        return Echo::Lost;
                    }
                    let src = sockaddr6(std::net::Ipv6Addr::UNSPECIFIED);
                    let dst = sockaddr6(a);
                    // SAFETY: as above
                    let n = unsafe {
                        Icmp6SendEcho2(
                            self.v6,
                            std::ptr::null_mut(),
                            None,
                            std::ptr::null(),
                            &src,
                            &dst,
                            PAYLOAD.as_ptr().cast(),
                            PAYLOAD.len() as u16,
                            &options,
                            buf.as_mut_ptr().cast(),
                            buf.len() as u32,
                            timeout,
                        )
                    };
                    if n == 0 {
                        return Echo::Lost;
                    }
                    // SAFETY: as above
                    let r: ICMPV6_ECHO_REPLY_LH = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast()) };
                    let words = r.Address.sin6_addr;
                    let from = IpAddr::V6(std::net::Ipv6Addr::from(words.map(u16::from_be)));
                    let ms = r.RoundTripTime as f64;
                    match r.Status {
                        IP_SUCCESS => Echo::Reply { from, ms },
                        IP_TTL_EXPIRED_TRANSIT => Echo::TtlExpired { from, ms },
                        _ => Echo::Lost,
                    }
                }
            }
        }
    }

    impl Drop for IcmpPinger {
        fn drop(&mut self) {
            // SAFETY: the handles came from IcmpCreateFile / Icmp6CreateFile
            unsafe {
                IcmpCloseHandle(self.v4);
                if self.v6 != INVALID_HANDLE_VALUE {
                    IcmpCloseHandle(self.v6);
                }
            }
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn pinger() -> Option<Box<dyn super::Pinger>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::net::Ipv6Addr;

    #[test]
    fn the_line_is_where_private_addresses_end() {
        for a in
            ["192.168.1.1", "10.0.0.1", "172.16.5.1", "169.254.1.1", "127.0.0.1", "fe80::1", "fd00::1", "::1"]
        {
            assert!(!past_the_line(a.parse().unwrap()), "{a}");
        }
        for a in ["100.64.0.1", "84.1.2.3", "2a00:1450::1", "1.1.1.1"] {
            assert!(past_the_line(a.parse().unwrap()), "{a}");
        }
    }

    #[test]
    fn fallback_hosts_are_addresses() {
        assert_eq!(
            parse_hosts(" 1.1.1.1, example.org,[2606:4700::1111],, 9.9.9.9 "),
            vec![
                "1.1.1.1".parse::<IpAddr>().unwrap(),
                IpAddr::V6("2606:4700::1111".parse::<Ipv6Addr>().unwrap()),
                "9.9.9.9".parse().unwrap()
            ]
        );
    }

    /// A route: hop n (1-based) is `hops[n - 1]`; routers in `silent` ignore echoes to
    /// themselves.
    struct FakeRoute {
        hops: Vec<IpAddr>,
        silent: Vec<IpAddr>,
        answers: HashMap<IpAddr, f64>,
    }

    impl Pinger for FakeRoute {
        fn echo(&mut self, to: IpAddr, ttl: u8, _: Duration) -> Echo {
            if self.hops.contains(&to) {
                return if self.silent.contains(&to) {
                    Echo::Lost
                } else {
                    Echo::Reply { from: to, ms: 5.0 }
                };
            }
            if let Some(&from) = self.hops.get(ttl as usize - 1) {
                return Echo::TtlExpired { from, ms: 5.0 };
            }
            match self.answers.get(&to) {
                Some(&ms) => Echo::Reply { from: to, ms },
                None => Echo::Lost,
            }
        }
    }

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn discovery_skips_the_home_routers() {
        let stop = AtomicBool::new(false);
        let mut r = FakeRoute {
            hops: vec![ip("192.168.0.1"), ip("192.168.1.1"), ip("84.0.0.1"), ip("84.0.1.1")],
            silent: vec![],
            answers: HashMap::new(),
        };
        assert_eq!(discover(&mut r, TRACE_TO, &stop), Some((Mode::Direct(ip("84.0.0.1")), 3)));
        // a router that only reports expired echoes
        r.silent = vec![ip("84.0.0.1")];
        assert_eq!(
            discover(&mut r, TRACE_TO, &stop),
            Some((Mode::Expiring { to: TRACE_TO, ttl: 3, router: ip("84.0.0.1") }, 3))
        );
        // nothing past the line within six hops
        r.hops = vec![ip("10.0.0.1"); 8];
        assert_eq!(discover(&mut r, TRACE_TO, &stop), None);
    }

    #[test]
    fn the_thread_reports_the_router_and_its_round_trips() {
        let r = FakeRoute {
            hops: vec![ip("192.168.0.1"), ip("84.0.0.1")],
            silent: vec![],
            answers: HashMap::new(),
        };
        let p = Prober::with_pinger(Box::new(r), vec![]);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while events.len() < 2 && Instant::now() < deadline {
            events.extend(p.events());
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            events[0],
            ProbeEvent::Target { addr: Some(ip("84.0.0.1")), hop: Some(2), fallback: false }
        );
        assert_eq!(events[1], ProbeEvent::Rtt { target: 0, ms: 5.0 });
    }

    /// This machine's real pings (`cargo test -p patorrent-core --lib probe::tests::real --
    /// --ignored --nocapture`): the router found and a few round trips.
    #[test]
    #[ignore = "pings the network"]
    fn real_pings() {
        let Some(p) = Prober::start(parse_hosts("1.1.1.1,8.8.8.8,9.9.9.9")) else {
            println!("no pings on this platform");
            return;
        };
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut rtts = 0;
        while rtts < 12 && Instant::now() < deadline {
            for e in p.events() {
                println!("{e:?}");
                rtts += matches!(e, ProbeEvent::Rtt { .. }) as usize;
            }
            thread::sleep(Duration::from_millis(50));
        }
        assert!(rtts > 0, "no round trips");
    }

    #[test]
    fn without_a_router_the_fallback_hosts_take_turns() {
        let answers = HashMap::from([(ip("1.1.1.1"), 12.0), (ip("9.9.9.9"), 20.0)]);
        // more private hops than the search goes: no router past the line is found
        let r = FakeRoute { hops: vec![ip("10.0.0.1"); 8], silent: vec![], answers };
        let p = Prober::with_pinger(Box::new(r), vec![ip("1.1.1.1"), ip("9.9.9.9")]);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut events = Vec::new();
        while events.len() < 3 && Instant::now() < deadline {
            events.extend(p.events());
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(events[0], ProbeEvent::Target { addr: None, hop: None, fallback: true });
        assert_eq!(events[1], ProbeEvent::Rtt { target: 0, ms: 12.0 });
        assert_eq!(events[2], ProbeEvent::Rtt { target: 1, ms: 20.0 });
    }
}
