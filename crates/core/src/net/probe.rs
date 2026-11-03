//! Keep ping low's probes (D49, `docs/LOWPING.md`): a thread of its own finds the first router
//! past the home line by TTL-limited echoes and pings it about four times a second; if no hop
//! answers, it pings the fallback hosts in turn. The engine reads the round trips from a channel
//! on its tick, so it never waits for the network.
//!
//! ICMP needs no administrator rights on Windows (`IcmpSendEcho2Ex`, as `tracert`), on Linux
//! (ping sockets, `SOCK_DGRAM` / `IPPROTO_ICMP`, allowed to the groups in
//! `net.ipv4.ping_group_range`; routers' time-exceeded answers come through `IP_RECVERR`) and
//! on macOS (the same sockets, open to every user, which also receive time-exceeded answers).
//! Elsewhere (FreeBSD, OpenBSD) [`Prober::start`] reports that pings are unavailable and the
//! engine sets no caps.

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
// FreeBSD and OpenBSD have no pinger yet
#[cfg_attr(
    not(any(windows, target_os = "linux", target_os = "android", target_vendor = "apple")),
    allow(dead_code)
)]
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
    /// The reply buffer's size.
    const REPLY_BUFFER: usize = 256;
    // the documentation's minimum, for which the reads below rely on one reply at the buffer's
    // start: the reply structure, the request's data, an ICMP error's 8 bytes and an
    // IO_STATUS_BLOCK (16 bytes on 64-bit, 8 on 32-bit)
    const _: () = assert!(std::mem::size_of::<ICMP_ECHO_REPLY>() + PAYLOAD.len() + 8 + 16 <= REPLY_BUFFER);
    const _: () =
        assert!(std::mem::size_of::<ICMPV6_ECHO_REPLY_LH>() + PAYLOAD.len() + 8 + 16 <= REPLY_BUFFER);

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
            // room for one reply, its data and an ICMP error's 8 bytes (as the documentation asks);
            // a longer reply fails the call (IP_BUF_TOO_SMALL, 0 replies)
            let mut buf = [0u8; REPLY_BUFFER];
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
                    // SAFETY: the call wrote at least one reply (n > 0) at the start of the buffer,
                    // which is large enough for it (asserted above); its pointers are not followed
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

/// The Logger's text when this system gives no pings.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub(crate) const UNAVAILABLE: &str = "this system does not let PaTorrent ping (its group must be in net.ipv4.ping_group_range); only the uTP peers' delays";
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub(crate) const UNAVAILABLE: &str = "pings are not available on this system; only the uTP peers' delays";

/// An ICMP echo request: type, code 0, checksum, identifier, sequence and the payload. The
/// checksum is filled for IPv4 (Linux recomputes it, macOS does not); ICMPv6's is always the
/// kernel's.
#[cfg_attr(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")), allow(dead_code))]
fn echo_request(v6: bool, id: u16, seq: u16) -> Vec<u8> {
    let mut p = vec![if v6 { 128 } else { 8 }, 0, 0, 0];
    p.extend_from_slice(&id.to_be_bytes());
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(b"PaTorrent keep-ping-low probe...");
    if !v6 {
        let sum = internet_checksum(&p);
        p[2..4].copy_from_slice(&sum.to_be_bytes());
    }
    p
}

/// RFC 1071.
#[cfg_attr(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")), allow(dead_code))]
fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 =
        data.chunks(2).map(|c| u16::from_be_bytes([c[0], *c.get(1).unwrap_or(&0)]) as u32).sum();
    while sum >> 16 != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// The header length of the IPv4 packet `d` starts with; `None` unless it starts with one
/// (version 4, at least the 20 bytes of a header without options).
#[cfg_attr(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")), allow(dead_code))]
fn ip_header_len(d: &[u8]) -> Option<usize> {
    let first = *d.first()?;
    let len = (first & 0x0f) as usize * 4;
    (first >> 4 == 4 && len >= 20).then_some(len)
}

/// Reads an ICMP message an echo socket received from `from` after an echo to `to`: our echo's
/// reply (only from `to`: an echo is answered by the address it went to), or (IPv4) a router's
/// time-exceeded answer quoting our echo. `ip_header`: the message starts with the IPv4 header
/// (macOS); `check_id`: the identifier to check (`None` on Linux, whose kernel sets it and hands
/// a ping socket only its own replies).
#[cfg_attr(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")), allow(dead_code))]
fn parse_icmp(
    data: &[u8],
    from: IpAddr,
    to: IpAddr,
    ip_header: bool,
    check_id: Option<u16>,
    seq: u16,
    ms: f64,
) -> Option<Echo> {
    let icmp = if ip_header { data.get(ip_header_len(data)?..)? } else { data };
    let ours = |m: &[u8]| -> Option<bool> {
        let id = u16::from_be_bytes([*m.get(4)?, *m.get(5)?]);
        let s = u16::from_be_bytes([*m.get(6)?, *m.get(7)?]);
        Some(s == seq && check_id.is_none_or(|want| want == id))
    };
    match *icmp.first()? {
        // echo reply, ICMPv6 echo reply
        0 | 129 => (from == to && ours(icmp)?).then_some(Echo::Reply { from, ms }),
        // time exceeded: the router quotes our packet's IP header and first 8 bytes
        11 => {
            let quoted = icmp.get(8..)?;
            let echo = quoted.get(ip_header_len(quoted)?..)?;
            (*echo.first()? == 8 && ours(echo)?).then_some(Echo::TtlExpired { from, ms })
        }
        _ => None,
    }
}

/// One entry of Linux's error queue (`IP_RECVERR`, `MSG_ERRQUEUE`): `data` the queued packet
/// (our echo request, as the router quoted it), `control` the control messages `recvmsg`
/// returned (`struct cmsghdr` with a `size_t` length, aligned to `size_t`). The router's address
/// when it is a time-exceeded answer to echo `seq`.
#[cfg_attr(not(any(target_os = "linux", target_os = "android")), allow(dead_code))]
fn parse_error_queue(data: &[u8], control: &[u8], seq: u16) -> Option<IpAddr> {
    const SOL_IP: i32 = 0;
    const IP_RECVERR: i32 = 11;
    const SO_EE_ORIGIN_ICMP: u8 = 2;
    const ICMP_TIME_EXCEEDED: u8 = 11;
    const AF_INET: u16 = 2;
    // the queued packet is our echo request: its sequence number says which
    if data.get(6..8)? != seq.to_be_bytes() {
        return None;
    }
    let word = std::mem::size_of::<usize>();
    // CMSG_ALIGN(sizeof(struct cmsghdr))
    let head = (word + 8).next_multiple_of(word);
    let mut at = 0usize;
    while let Some(h) = control.get(at..at.checked_add(head)?) {
        let len = usize::from_ne_bytes(h[..word].try_into().ok()?);
        let level = i32::from_ne_bytes(h[word..word + 4].try_into().ok()?);
        let ty = i32::from_ne_bytes(h[word + 4..word + 8].try_into().ok()?);
        // a length shorter than the header or past the buffer ends the walk
        let body = control.get(at + head..at.checked_add(len)?)?;
        // struct sock_extended_err (16 bytes), then the offender's sockaddr_in
        if level == SOL_IP && ty == IP_RECVERR && body.len() >= 24 {
            let (origin, kind) = (body[4], body[5]);
            let family = u16::from_ne_bytes([body[16], body[17]]);
            if origin == SO_EE_ORIGIN_ICMP && kind == ICMP_TIME_EXCEEDED && family == AF_INET {
                return Some(IpAddr::V4(Ipv4Addr::new(body[20], body[21], body[22], body[23])));
            }
        }
        // `len` is within the buffer, so this cannot overflow, and it moves on (len >= head)
        at += len.next_multiple_of(word);
    }
    None
}

/// The `icmp` fuzz target (security review, 2026-11-02): the input as an echo socket's message
/// (with and without the IPv4 header, with and without an identifier to check, from the
/// address pinged and from another) and as an error-queue entry (packet and control messages
/// split at a byte of the input). A reply must come from the address pinged and carry our
/// sequence number; the checksum of any message with its checksum filled in must be zero.
/// Returns how many parsed.
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub fn fuzz_icmp(data: &[u8]) -> usize {
    if data.len() < 5 {
        return 0;
    }
    let seq = u16::from_be_bytes([data[0], data[1]]);
    let id = u16::from_be_bytes([data[2], data[3]]);
    let split = data[4] as usize;
    let packet = &data[5..];
    let to = IpAddr::V4(Ipv4Addr::new(84, 0, 0, 1));
    let mut parsed = 0;
    for from in [to, IpAddr::V4(Ipv4Addr::new(84, 0, 0, 2))] {
        for ip_header in [false, true] {
            for check_id in [None, Some(id)] {
                match parse_icmp(packet, from, to, ip_header, check_id, seq, 1.0) {
                    Some(Echo::Reply { from: f, .. }) => {
                        assert_eq!(f, to, "a reply from another address");
                        let icmp = if ip_header { &packet[ip_header_len(packet).unwrap()..] } else { packet };
                        assert!(matches!(icmp[0], 0 | 129));
                        assert_eq!(icmp[6..8], seq.to_be_bytes());
                        parsed += 1;
                    }
                    Some(Echo::TtlExpired { from: f, .. }) => {
                        assert_eq!(f, from);
                        parsed += 1;
                    }
                    Some(Echo::Lost) => unreachable!("never parsed"),
                    None => {}
                }
            }
        }
    }
    let (queued, control) = packet.split_at(split.min(packet.len()));
    if parse_error_queue(queued, control, seq).is_some() {
        assert_eq!(queued[6..8], seq.to_be_bytes());
        parsed += 1;
    }
    let mut message = packet.to_vec();
    if message.len() >= 4 {
        message[2..4].fill(0);
        let sum = internet_checksum(&message);
        message[2..4].copy_from_slice(&sum.to_be_bytes());
        assert_eq!(internet_checksum(&message), 0, "a filled-in checksum sums to zero");
    }
    parsed
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
mod imp {
    use std::ffi::c_int;
    use std::io;
    use std::net::{IpAddr, SocketAddr, UdpSocket};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::time::{Duration, Instant};

    use super::{Echo, Pinger, echo_request, parse_icmp};

    const AF_INET: c_int = 2;
    #[cfg(target_vendor = "apple")]
    const AF_INET6: c_int = 30;
    #[cfg(not(target_vendor = "apple"))]
    const AF_INET6: c_int = 10;
    const SOCK_DGRAM: c_int = 2;
    #[cfg(not(target_vendor = "apple"))]
    const SOCK_CLOEXEC: c_int = 0x80000;
    // macOS has no SOCK_CLOEXEC; PaTorrent starts no child processes that could inherit it
    #[cfg(target_vendor = "apple")]
    const SOCK_CLOEXEC: c_int = 0;
    const IPPROTO_ICMP: c_int = 1;
    const IPPROTO_ICMPV6: c_int = 58;
    const POLLIN: i16 = 1;
    #[cfg(not(target_vendor = "apple"))]
    const POLLERR: i16 = 8;
    #[cfg(target_vendor = "apple")]
    type NFds = u32;
    #[cfg(not(target_vendor = "apple"))]
    type NFds = std::ffi::c_ulong;

    #[repr(C)]
    struct PollFd {
        fd: c_int,
        events: i16,
        revents: i16,
    }

    unsafe extern "C" {
        fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
        fn poll(fds: *mut PollFd, n: NFds, timeout: c_int) -> c_int;
    }

    /// Echo sockets for IPv4 and (when the system allows) IPv6.
    struct IcmpPinger {
        v4: UdpSocket,
        v6: Option<UdpSocket>,
        id: u16,
        seq: u16,
    }

    fn open(v6: bool) -> io::Result<UdpSocket> {
        let (af, proto) = if v6 { (AF_INET6, IPPROTO_ICMPV6) } else { (AF_INET, IPPROTO_ICMP) };
        // SAFETY: plain socket creation
        let fd = unsafe { socket(af, SOCK_DGRAM | SOCK_CLOEXEC, proto) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh socket we own; the calls made on it (TTL, sendto, recvfrom)
        // are a datagram socket's, as UdpSocket makes them
        let s = unsafe { UdpSocket::from_raw_fd(fd) };
        #[cfg(any(target_os = "linux", target_os = "android"))]
        if !v6 {
            linux::recv_errors(&s)?;
        }
        Ok(s)
    }

    pub(super) fn pinger() -> Option<Box<dyn Pinger>> {
        let v4 = open(false).ok()?;
        let v6 = open(true).ok();
        Some(Box::new(IcmpPinger { v4, v6, id: std::process::id() as u16, seq: 0 }))
    }

    impl Pinger for IcmpPinger {
        fn echo(&mut self, to: IpAddr, ttl: u8, timeout: Duration) -> Echo {
            self.seq = self.seq.wrapping_add(1);
            let (seq, v6) = (self.seq, to.is_ipv6());
            let Some(sock) = (if v6 { self.v6.as_ref() } else { Some(&self.v4) }) else { return Echo::Lost };
            // IPv6 targets are fallback hosts, pinged with the default hop limit
            if !v6 && sock.set_ttl(ttl.into()).is_err() {
                return Echo::Lost;
            }
            let sent = Instant::now();
            if sock.send_to(&echo_request(v6, self.id, seq), SocketAddr::new(to, 0)).is_err() {
                return Echo::Lost;
            }
            let deadline = sent + timeout;
            let mut buf = [0u8; 1500];
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Echo::Lost;
                }
                let mut p = PollFd { fd: sock.as_raw_fd(), events: POLLIN, revents: 0 };
                // SAFETY: one valid pollfd
                let n = unsafe { poll(&mut p, 1, left.as_millis().clamp(1, c_int::MAX as u128) as c_int) };
                if n < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                if n <= 0 {
                    return Echo::Lost;
                }
                let ms = sent.elapsed().as_secs_f64() * 1000.0;
                // Linux queues a router's time-exceeded answer as an error of the socket
                #[cfg(any(target_os = "linux", target_os = "android"))]
                if p.revents & POLLERR != 0 {
                    match linux::read_error(sock, seq) {
                        Some(Some(from)) => return Echo::TtlExpired { from, ms },
                        Some(None) => continue,
                        None => return Echo::Lost,
                    }
                }
                if p.revents & POLLIN != 0 {
                    // a pending socket error (Linux) fails the read: POLLERR comes next
                    let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                    // macOS hands IPv4 messages over with their IP header, and every echo
                    // reply on the machine: ours carry our identifier
                    let (ip_header, check_id) =
                        if cfg!(target_vendor = "apple") { (!v6, Some(self.id)) } else { (false, None) };
                    if let Some(e) = parse_icmp(&buf[..n], from.ip(), to, ip_header, check_id, seq, ms) {
                        return e;
                    }
                }
            }
        }
    }

    /// The error queue of Linux's ping sockets (`IP_RECVERR`, `MSG_ERRQUEUE`).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    mod linux {
        use std::ffi::{c_int, c_void};
        use std::io;
        use std::net::{IpAddr, UdpSocket};
        use std::os::fd::AsRawFd;

        use super::super::parse_error_queue;

        const SOL_IP: c_int = 0;
        const IP_RECVERR: c_int = 11;
        const MSG_DONTWAIT: c_int = 0x40;
        const MSG_ERRQUEUE: c_int = 0x2000;

        #[repr(C)]
        struct IoVec {
            base: *mut c_void,
            len: usize,
        }

        /// glibc's `struct msghdr` (musl pads its two narrower fields to the same layout).
        #[repr(C)]
        struct MsgHdr {
            name: *mut c_void,
            namelen: u32,
            iov: *mut IoVec,
            iovlen: usize,
            control: *mut c_void,
            controllen: usize,
            flags: c_int,
        }

        unsafe extern "C" {
            fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, len: u32) -> c_int;
            fn recvmsg(fd: c_int, msg: *mut MsgHdr, flags: c_int) -> isize;
        }

        pub(super) fn recv_errors(s: &UdpSocket) -> io::Result<()> {
            let on: c_int = 1;
            // SAFETY: `on` is a valid int for the call
            let r = unsafe { setsockopt(s.as_raw_fd(), SOL_IP, IP_RECVERR, (&raw const on).cast(), 4) };
            if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
        }

        /// One queued error: `Some(Some(router))` for a time-exceeded answer to echo `seq`,
        /// `Some(None)` for anything else, `None` when the queue cannot be read.
        pub(super) fn read_error(s: &UdpSocket, seq: u16) -> Option<Option<IpAddr>> {
            let mut data = [0u8; 576];
            let mut control = [0u64; 64];
            let mut iov = IoVec { base: data.as_mut_ptr().cast(), len: data.len() };
            let mut msg = MsgHdr {
                name: std::ptr::null_mut(),
                namelen: 0,
                iov: &mut iov,
                iovlen: 1,
                control: control.as_mut_ptr().cast(),
                controllen: std::mem::size_of_val(&control),
                flags: 0,
            };
            // SAFETY: every buffer outlives the call and is described by its length
            let n = unsafe { recvmsg(s.as_raw_fd(), &mut msg, MSG_ERRQUEUE | MSG_DONTWAIT) };
            if n < 0 {
                return None;
            }
            // only what the call wrote: a short packet leaves the rest of `data` zero
            let data = &data[..(n as usize).min(data.len())];
            let bytes: Vec<u8> = control.iter().flat_map(|w| w.to_ne_bytes()).collect();
            let bytes = &bytes[..msg.controllen.min(bytes.len())];
            Some(parse_error_queue(data, bytes, seq))
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "android", target_vendor = "apple")))]
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
    fn echo_requests_and_their_answers() {
        let req = echo_request(false, 0x1234, 7);
        assert_eq!(&req[..2], &[8, 0]);
        assert_eq!(&req[4..8], &[0x12, 0x34, 0, 7]);
        // a correct checksum sums to zero
        assert_eq!(internet_checksum(&req), 0);
        // ICMPv6: the kernel's checksum
        assert_eq!(&echo_request(true, 1, 2)[..4], &[128, 0, 0, 0]);

        let router = ip("84.0.0.1");
        let mut reply = req.clone();
        reply[0] = 0;
        // Linux: no IP header, the kernel's identifier
        assert_eq!(
            parse_icmp(&reply, router, router, false, None, 7, 3.0),
            Some(Echo::Reply { from: router, ms: 3.0 })
        );
        assert_eq!(parse_icmp(&reply, router, router, false, None, 8, 3.0), None);
        // macOS: an IPv4 header first, and another program's echo is not ours
        let mut with_ip = vec![0x45; 1];
        with_ip.extend_from_slice(&[0; 19]);
        with_ip.extend_from_slice(&reply);
        assert!(parse_icmp(&with_ip, router, router, true, Some(0x1234), 7, 3.0).is_some());
        assert_eq!(parse_icmp(&with_ip, router, router, true, Some(0x9999), 7, 3.0), None);
        // a router's time exceeded quoting our echo behind its IP header
        let mut quoted = vec![0x45];
        quoted.extend_from_slice(&[0; 19]);
        quoted.extend_from_slice(&req[..8]);
        let mut exceeded = vec![11, 0, 0, 0, 0, 0, 0, 0];
        exceeded.extend_from_slice(&quoted);
        let mut packet = vec![0x45];
        packet.extend_from_slice(&[0; 19]);
        packet.extend_from_slice(&exceeded);
        // the router is not the address the echo went to
        assert_eq!(
            parse_icmp(&packet, router, TRACE_TO, true, Some(0x1234), 7, 4.0),
            Some(Echo::TtlExpired { from: router, ms: 4.0 })
        );
        // ICMPv6 echo reply
        let mut v6 = echo_request(true, 1, 2);
        v6[0] = 129;
        let host = ip("2001:db8::1");
        assert!(parse_icmp(&v6, host, host, false, None, 2, 1.0).is_some());
        // short or foreign messages
        assert_eq!(parse_icmp(&[0, 0], router, router, false, None, 7, 1.0), None);
        assert_eq!(parse_icmp(&[3, 1, 0, 0, 0, 0, 0, 7], router, router, false, None, 7, 1.0), None);
    }

    #[test]
    fn an_echo_reply_counts_only_from_the_address_pinged() {
        // Security review (2026-11-02): any host could answer an echo to another (macOS hands
        // every echo reply on the machine to each socket; Linux checks only the identifier),
        // so a forged reply could be the "router" the search found, or a round trip.
        let router = ip("84.0.0.1");
        let mut reply = echo_request(false, 0x1234, 7);
        reply[0] = 0;
        assert_eq!(parse_icmp(&reply, ip("203.0.113.9"), router, false, None, 7, 1.0), None);
        let mut v6 = echo_request(true, 1, 2);
        v6[0] = 129;
        assert_eq!(parse_icmp(&v6, ip("2001:db8::2"), ip("2001:db8::1"), false, None, 2, 1.0), None);
        assert!(parse_icmp(&reply, router, router, false, None, 7, 1.0).is_some());
    }

    #[test]
    fn ip_headers_are_checked_before_their_length_is_used() {
        // a header length under 20 bytes, or another version, is not an IPv4 header
        assert_eq!(ip_header_len(&[0x45]), Some(20));
        assert_eq!(ip_header_len(&[0x4f]), Some(60));
        for first in [0x40, 0x44, 0x65, 0x05, 0x08] {
            assert_eq!(ip_header_len(&[first]), None, "{first:#x}");
        }
        assert_eq!(ip_header_len(&[]), None);
        // a time-exceeded answer quoting something other than an IPv4 header is not read as
        // quoting our echo
        let router = ip("84.0.0.1");
        let req = echo_request(false, 0x1234, 7);
        for first in [0x45, 0x65] {
            let mut exceeded = vec![11, 0, 0, 0, 0, 0, 0, 0, first];
            exceeded.extend_from_slice(&[0; 19]);
            exceeded.extend_from_slice(&req[..8]);
            let parsed = parse_icmp(&exceeded, router, TRACE_TO, false, None, 7, 1.0);
            assert_eq!(parsed.is_some(), first == 0x45, "{first:#x}");
        }
    }

    /// One control message as Linux writes it: `cmsghdr` (`size_t` length), then the body,
    /// padded to `size_t`.
    fn cmsg(len: usize, level: i32, ty: i32, body: &[u8]) -> Vec<u8> {
        let word = std::mem::size_of::<usize>();
        let mut out = len.to_ne_bytes().to_vec();
        out.extend_from_slice(&level.to_ne_bytes());
        out.extend_from_slice(&ty.to_ne_bytes());
        out.resize(out.len().next_multiple_of(word), 0);
        out.extend_from_slice(body);
        out.resize(out.len().next_multiple_of(word), 0);
        out
    }

    #[test]
    fn the_error_queue_is_read_within_its_bounds() {
        // Security review (2026-11-02): the walk added the control message's length unchecked
        // (a length near usize::MAX overflowed, then sliced backwards), and the queued packet
        // was read past what recvmsg wrote (a short one matched sequence number 0).
        let word = std::mem::size_of::<usize>();
        let head = (word + 8).next_multiple_of(word);
        // sock_extended_err: errno, origin ICMP, type time exceeded; then sockaddr_in 84.0.0.1
        let mut err = vec![0u8; 16];
        err[4] = 2;
        err[5] = 11;
        err.extend_from_slice(&2u16.to_ne_bytes());
        err.extend_from_slice(&[0, 0, 84, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0]);
        let ok = cmsg(head + err.len(), 0, 11, &err);
        let req = echo_request(false, 1, 7);
        assert_eq!(parse_error_queue(&req, &ok, 7), Some(ip("84.0.0.1")));
        // another echo's error, a short packet (sequence 0), other origins
        assert_eq!(parse_error_queue(&req, &ok, 8), None);
        assert_eq!(parse_error_queue(&req[..6], &ok, 0), None);
        let mut unreachable = err.clone();
        unreachable[5] = 3;
        assert_eq!(parse_error_queue(&req, &cmsg(head + unreachable.len(), 0, 11, &unreachable), 7), None);
        // a message of another level first, then ours
        let mut two = cmsg(head + 4, 1, 29, &[1, 2, 3, 4]);
        two.extend_from_slice(&ok);
        assert_eq!(parse_error_queue(&req, &two, 7), Some(ip("84.0.0.1")));
        // lengths that do not fit: shorter than the header, past the buffer, or near the
        // address space's end
        for len in [0, head - 1, 2 * ok.len() + 1, usize::MAX, usize::MAX - head + 1] {
            let mut bad = cmsg(len, 0, 11, &err);
            bad.extend_from_slice(&ok);
            assert_eq!(parse_error_queue(&req, &bad, 7), None, "{len}");
        }
        // truncated control data
        for cut in 0..ok.len() {
            let _ = parse_error_queue(&req, &ok[..cut], 7);
        }
    }

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
        let (mut rtts, mut router) = (0, false);
        while rtts < 12 && Instant::now() < deadline {
            for e in p.events() {
                println!("{e:?}");
                rtts += matches!(e, ProbeEvent::Rtt { .. }) as usize;
                router |= matches!(e, ProbeEvent::Target { addr: Some(_), .. });
            }
            thread::sleep(Duration::from_millis(50));
        }
        if rtts == 0 && !router {
            // CI's Windows runners drop every ping
            println!("nothing answers pings on this network");
            return;
        }
        assert!(rtts > 0, "no round trips");
    }

    /// What each hop answers on this machine (`cargo test -p patorrent-core --lib
    /// probe::tests::real_trace -- --ignored --nocapture`).
    #[test]
    #[ignore = "pings the network"]
    fn real_trace() {
        let Some(mut p) = imp::pinger() else {
            println!("no pings on this platform: {UNAVAILABLE}");
            return;
        };
        for ttl in 1..=MAX_HOP {
            println!("ttl {ttl}: {:?}", p.echo(TRACE_TO, ttl, TIMEOUT));
        }
        println!("direct: {:?}", p.echo(TRACE_TO, 64, TIMEOUT));
        println!("ipv6: {:?}", p.echo("2606:4700:4700::1111".parse().unwrap(), 64, TIMEOUT));
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
