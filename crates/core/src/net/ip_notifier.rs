// Ported from libtorrent-rasterbar 2.0.11: src/ip_notifier.cpp, include/libtorrent/aux_/ip_notifier.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: no asio. On Linux and FreeBSD / OpenBSD the notifier is a socket the reactor polls:
// netlink with the link, address and route groups (libtorrent: addresses only), and a PF_ROUTE
// socket on the BSDs (libtorrent has no BSD notifier). On Windows the IP Helper callbacks
// (interfaces, unicast addresses and, beyond libtorrent, routes) set a flag and wake the
// reactor. Elsewhere `IpNotifier::new` fails with `Unsupported` and the engine re-reads the
// interfaces periodically. A notification only says "something changed": the engine reads the
// interfaces again, as libtorrent does.

//! Network change notifications: an interface or address appeared or went away, or a route
//! changed.

use std::io;
use std::sync::Arc;

use mio::{Registry, Token};

/// Wakes the reactor from another thread.
pub(crate) type Wake = Arc<dyn Fn() + Send + Sync>;

/// libtorrent `ip_change_notifier`.
pub(crate) struct IpNotifier {
    imp: imp::Notifier,
}

impl IpNotifier {
    /// Starts watching. `wake` is called (from another thread) where the OS reports changes
    /// through callbacks rather than a socket. `Unsupported` where there is no notifier.
    pub fn new(wake: Wake) -> io::Result<IpNotifier> {
        Ok(IpNotifier { imp: imp::Notifier::new(wake)? })
    }

    /// Registers the notifier's socket (where it has one) with the reactor.
    pub fn register(&mut self, registry: &Registry, token: Token) -> io::Result<()> {
        self.imp.register(registry, token)
    }

    pub fn deregister(&mut self, registry: &Registry) {
        self.imp.deregister(registry);
    }

    /// The notifier's socket is readable: drains it. Whether a pertinent change was among the
    /// messages (an error counts as one: the engine re-reads the interfaces anyway).
    pub fn on_readable(&mut self) -> bool {
        self.imp.on_readable()
    }

    /// Whether a callback reported a change since the last call (Windows).
    pub fn take_signal(&self) -> bool {
        self.imp.take_signal()
    }
}

/// Whether a netlink message type reports a change of links, addresses or routes
/// (`RTM_NEWLINK` .. `RTM_DELLINK`, `RTM_NEWADDR` .. `RTM_DELADDR`, `RTM_NEWROUTE` ..
/// `RTM_DELROUTE`).
#[cfg_attr(not(any(target_os = "linux", target_os = "android", test)), allow(dead_code))]
fn netlink_pertinent(buf: &[u8]) -> bool {
    const RTM_NEWLINK: u16 = 16;
    const RTM_DELLINK: u16 = 17;
    const RTM_NEWADDR: u16 = 20;
    const RTM_DELADDR: u16 = 21;
    const RTM_NEWROUTE: u16 = 24;
    const RTM_DELROUTE: u16 = 25;
    const NLMSG_HDRLEN: usize = 16;
    let mut off = 0;
    let mut pertinent = false;
    // nlmsg_ok / nlmsg_next
    while off + NLMSG_HDRLEN <= buf.len() {
        let len = u32::from_ne_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]) as usize;
        if len < NLMSG_HDRLEN || off + len > buf.len() {
            break;
        }
        let ty = u16::from_ne_bytes([buf[off + 4], buf[off + 5]]);
        pertinent |=
            matches!(ty, RTM_NEWLINK | RTM_DELLINK | RTM_NEWADDR | RTM_DELADDR | RTM_NEWROUTE | RTM_DELROUTE);
        off += (len + 3) & !3;
    }
    pertinent
}

/// Whether a routing-socket message (`rt_msghdr`, `ifa_msghdr`, `if_msghdr`, ...) reports a
/// change of interfaces, addresses or routes. Link-layer entries (ARP / NDP, `RTF_LLINFO`) come
/// as route messages too and are ignored. `flags_at` is where `rtm_flags` sits.
#[cfg_attr(not(any(target_os = "freebsd", target_os = "openbsd", test)), allow(dead_code))]
fn route_msg_pertinent(m: &[u8], flags_at: usize, ifannounce: u8) -> bool {
    const RTM_VERSION: u8 = 5;
    const RTM_ADD: u8 = 1;
    const RTM_DELETE: u8 = 2;
    const RTM_CHANGE: u8 = 3;
    const RTM_NEWADDR: u8 = 0xc;
    const RTM_DELADDR: u8 = 0xd;
    const RTM_IFINFO: u8 = 0xe;
    const RTF_LLINFO: i32 = 0x400;
    if m.len() < 4 || m[2] != RTM_VERSION {
        return false;
    }
    match m[3] {
        RTM_ADD | RTM_DELETE | RTM_CHANGE => {
            if m.len() < flags_at + 4 {
                return false;
            }
            let flags = i32::from_ne_bytes([m[flags_at], m[flags_at + 1], m[flags_at + 2], m[flags_at + 3]]);
            flags & RTF_LLINFO == 0
        }
        RTM_NEWADDR | RTM_DELADDR | RTM_IFINFO => true,
        t => t == ifannounce,
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    use std::ffi::{c_int, c_void};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    use mio::unix::SourceFd;
    use mio::{Interest, Registry, Token};

    use super::{Wake, netlink_pertinent};

    const AF_NETLINK: c_int = 16;
    const SOCK_RAW: c_int = 3;
    const SOCK_NONBLOCK: c_int = 0o4000;
    const SOCK_CLOEXEC: c_int = 0o2000000;
    const NETLINK_ROUTE: c_int = 0;
    const RTMGRP_LINK: u32 = 0x1;
    const RTMGRP_IPV4_IFADDR: u32 = 0x10;
    const RTMGRP_IPV4_ROUTE: u32 = 0x40;
    const RTMGRP_IPV6_IFADDR: u32 = 0x100;
    const RTMGRP_IPV6_ROUTE: u32 = 0x400;
    const SOL_NETLINK: c_int = 270;
    const NETLINK_NO_ENOBUFS: c_int = 5;
    const ENOBUFS: i32 = 105;

    #[repr(C)]
    struct SockaddrNl {
        family: u16,
        pad: u16,
        pid: u32,
        groups: u32,
    }

    unsafe extern "C" {
        fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
        fn bind(fd: c_int, addr: *const c_void, len: u32) -> c_int;
        fn setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, len: u32) -> c_int;
        fn recv(fd: c_int, buf: *mut c_void, len: usize, flags: c_int) -> isize;
    }

    pub struct Notifier {
        fd: OwnedFd,
        buf: Vec<u8>,
    }

    impl Notifier {
        pub fn new(_wake: Wake) -> io::Result<Notifier> {
            // SAFETY: plain socket creation
            let fd = unsafe { socket(AF_NETLINK, SOCK_RAW | SOCK_NONBLOCK | SOCK_CLOEXEC, NETLINK_ROUTE) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: a fresh socket we own
            let fd = unsafe { OwnedFd::from_raw_fd(fd) };
            // Linux can report ENOBUFS when the socket's buffer overflows; we'd rather lose
            // messages (we re-read everything anyway)
            let on: c_int = 1;
            // SAFETY: `on` is a valid int
            unsafe {
                setsockopt(fd.as_raw_fd(), SOL_NETLINK, NETLINK_NO_ENOBUFS, (&raw const on).cast(), 4);
            }
            let addr = SockaddrNl {
                family: AF_NETLINK as u16,
                pad: 0,
                pid: 0,
                groups: RTMGRP_LINK
                    | RTMGRP_IPV4_IFADDR
                    | RTMGRP_IPV6_IFADDR
                    | RTMGRP_IPV4_ROUTE
                    | RTMGRP_IPV6_ROUTE,
            };
            // SAFETY: `addr` is a sockaddr_nl of the given size
            let r = unsafe {
                bind(fd.as_raw_fd(), (&raw const addr).cast(), std::mem::size_of::<SockaddrNl>() as u32)
            };
            if r != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(Notifier { fd, buf: vec![0; 16 * 1024] })
        }

        pub fn register(&mut self, registry: &Registry, token: Token) -> io::Result<()> {
            registry.register(&mut SourceFd(&self.fd.as_raw_fd()), token, Interest::READABLE)
        }

        pub fn deregister(&mut self, registry: &Registry) {
            let _ = registry.deregister(&mut SourceFd(&self.fd.as_raw_fd()));
        }

        pub fn on_readable(&mut self) -> bool {
            let mut pertinent = false;
            loop {
                // SAFETY: `buf` holds `len` writable bytes
                let n = unsafe { recv(self.fd.as_raw_fd(), self.buf.as_mut_ptr().cast(), self.buf.len(), 0) };
                if n < 0 {
                    let e = io::Error::last_os_error();
                    match e.kind() {
                        io::ErrorKind::WouldBlock => break,
                        io::ErrorKind::Interrupted => continue,
                        // messages were lost: assume they mattered
                        _ if e.raw_os_error() == Some(ENOBUFS) => pertinent = true,
                        _ => return true,
                    }
                    continue;
                }
                if n == 0 {
                    break;
                }
                pertinent |= netlink_pertinent(&self.buf[..n as usize]);
            }
            pertinent
        }

        pub fn take_signal(&self) -> bool {
            false
        }
    }
}

#[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
mod imp {
    use std::ffi::{c_int, c_void};
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

    use mio::unix::SourceFd;
    use mio::{Interest, Registry, Token};

    use super::{Wake, route_msg_pertinent};

    const PF_ROUTE: c_int = 17;
    const SOCK_RAW: c_int = 3;
    #[cfg(target_os = "freebsd")]
    const SOCK_FLAGS: c_int = 0x1000_0000 | 0x2000_0000; // SOCK_CLOEXEC | SOCK_NONBLOCK
    #[cfg(target_os = "openbsd")]
    const SOCK_FLAGS: c_int = 0x8000 | 0x4000;
    /// Where `rtm_flags` sits in `rt_msghdr`, and the `RTM_IFANNOUNCE` type.
    #[cfg(target_os = "freebsd")]
    const FLAGS_AT: usize = 8;
    #[cfg(target_os = "freebsd")]
    const RTM_IFANNOUNCE: u8 = 0x11;
    #[cfg(target_os = "openbsd")]
    const FLAGS_AT: usize = 16;
    #[cfg(target_os = "openbsd")]
    const RTM_IFANNOUNCE: u8 = 0xf;

    unsafe extern "C" {
        fn socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
        fn recv(fd: c_int, buf: *mut c_void, len: usize, flags: c_int) -> isize;
    }

    pub struct Notifier {
        fd: OwnedFd,
        buf: Vec<u8>,
    }

    impl Notifier {
        pub fn new(_wake: Wake) -> io::Result<Notifier> {
            // SAFETY: plain socket creation; AF_UNSPEC (0): messages of every family
            let fd = unsafe { socket(PF_ROUTE, SOCK_RAW | SOCK_FLAGS, 0) };
            if fd < 0 {
                return Err(io::Error::last_os_error());
            }
            // SAFETY: a fresh socket we own
            Ok(Notifier { fd: unsafe { OwnedFd::from_raw_fd(fd) }, buf: vec![0; 8 * 1024] })
        }

        pub fn register(&mut self, registry: &Registry, token: Token) -> io::Result<()> {
            registry.register(&mut SourceFd(&self.fd.as_raw_fd()), token, Interest::READABLE)
        }

        pub fn deregister(&mut self, registry: &Registry) {
            let _ = registry.deregister(&mut SourceFd(&self.fd.as_raw_fd()));
        }

        pub fn on_readable(&mut self) -> bool {
            let mut pertinent = false;
            loop {
                // SAFETY: `buf` holds `len` writable bytes
                let n = unsafe { recv(self.fd.as_raw_fd(), self.buf.as_mut_ptr().cast(), self.buf.len(), 0) };
                if n < 0 {
                    let e = io::Error::last_os_error();
                    match e.kind() {
                        io::ErrorKind::WouldBlock => break,
                        io::ErrorKind::Interrupted => continue,
                        // ENOBUFS: messages were lost, assume they mattered
                        _ => return true,
                    }
                }
                if n == 0 {
                    break;
                }
                // one message per read; walk them anyway, by `rtm_msglen`
                let buf = &self.buf[..n as usize];
                let mut off = 0;
                while off + 4 <= buf.len() {
                    let len = u16::from_ne_bytes([buf[off], buf[off + 1]]) as usize;
                    if len < 4 || off + len > buf.len() {
                        break;
                    }
                    pertinent |= route_msg_pertinent(&buf[off..off + len], FLAGS_AT, RTM_IFANNOUNCE);
                    off += len;
                }
            }
            pertinent
        }

        pub fn take_signal(&self) -> bool {
            false
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::io;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use mio::{Registry, Token};
    use windows_sys::Win32::Foundation::{HANDLE, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        CancelMibChangeNotify2, MIB_NOTIFICATION_TYPE, NotifyIpInterfaceChange, NotifyRouteChange2,
        NotifyUnicastIpAddressChange,
    };
    use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;

    use super::Wake;

    struct Shared {
        changed: AtomicBool,
        wake: Wake,
    }

    /// Runs on a system worker thread.
    unsafe extern "system" fn on_change<Row>(
        ctx: *const c_void,
        _row: *const Row,
        _ty: MIB_NOTIFICATION_TYPE,
    ) {
        // SAFETY: `ctx` is the `Shared` the registration was given; it outlives the
        // registrations (`Drop` cancels them first, which waits for running callbacks)
        let s = unsafe { &*ctx.cast::<Shared>() };
        s.changed.store(true, Ordering::Release);
        (s.wake)();
    }

    pub struct Notifier {
        shared: *const Shared,
        handles: Vec<HANDLE>,
    }

    impl Notifier {
        pub fn new(wake: Wake) -> io::Result<Notifier> {
            let shared = Arc::into_raw(Arc::new(Shared { changed: AtomicBool::new(false), wake }));
            let mut n = Notifier { shared, handles: Vec::new() };
            let ctx = shared.cast::<c_void>();
            let mut h: HANDLE = std::ptr::null_mut();
            // SAFETY: valid callbacks, context and out pointer; InitialNotification off
            let r = unsafe { NotifyIpInterfaceChange(AF_UNSPEC, Some(on_change), ctx, false, &mut h) };
            n.push(r, h)?;
            // SAFETY: as above
            let r = unsafe { NotifyUnicastIpAddressChange(AF_UNSPEC, Some(on_change), ctx, false, &mut h) };
            n.push(r, h)?;
            // SAFETY: as above
            let r = unsafe { NotifyRouteChange2(AF_UNSPEC, Some(on_change), ctx, false, &mut h) };
            n.push(r, h)?;
            Ok(n)
        }

        fn push(&mut self, r: u32, h: HANDLE) -> io::Result<()> {
            if r != NO_ERROR {
                // `self` is dropped by the caller's `?`, cancelling the earlier ones
                return Err(io::Error::from_raw_os_error(r as i32));
            }
            self.handles.push(h);
            Ok(())
        }

        pub fn register(&mut self, _registry: &Registry, _token: Token) -> io::Result<()> {
            Ok(())
        }

        pub fn deregister(&mut self, _registry: &Registry) {}

        pub fn on_readable(&mut self) -> bool {
            false
        }

        pub fn take_signal(&self) -> bool {
            // SAFETY: `shared` lives until `Drop`
            unsafe { &*self.shared }.changed.swap(false, Ordering::Acquire)
        }
    }

    impl Drop for Notifier {
        fn drop(&mut self) {
            for h in self.handles.drain(..) {
                // SAFETY: a handle from a successful registration; this waits for callbacks in
                // progress
                unsafe { CancelMibChangeNotify2(h) };
            }
            // SAFETY: from `Arc::into_raw` in `new`; no callback can run any more
            drop(unsafe { Arc::from_raw(self.shared) });
        }
    }
}

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_os = "freebsd",
    target_os = "openbsd"
)))]
mod imp {
    use std::io;

    use mio::{Registry, Token};

    use super::Wake;

    pub struct Notifier;

    impl Notifier {
        pub fn new(_wake: Wake) -> io::Result<Notifier> {
            Err(io::Error::new(io::ErrorKind::Unsupported, "no network change notifications on this OS"))
        }

        pub fn register(&mut self, _registry: &Registry, _token: Token) -> io::Result<()> {
            Ok(())
        }

        pub fn deregister(&mut self, _registry: &Registry) {}

        pub fn on_readable(&mut self) -> bool {
            false
        }

        pub fn take_signal(&self) -> bool {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nlmsg(ty: u16, payload: usize) -> Vec<u8> {
        let len = 16 + payload;
        let mut m = Vec::new();
        m.extend_from_slice(&(len as u32).to_ne_bytes());
        m.extend_from_slice(&ty.to_ne_bytes());
        m.extend_from_slice(&[0; 10]);
        m.resize(len, 0);
        m.resize((len + 3) & !3, 0);
        m
    }

    #[test]
    fn netlink_messages() {
        // RTM_NEWNEIGH (28) and RTM_GETLINK (18) don't matter; RTM_NEWADDR does
        assert!(!netlink_pertinent(&nlmsg(28, 12)));
        let mut buf = nlmsg(28, 13);
        buf.extend(nlmsg(18, 0));
        assert!(!netlink_pertinent(&buf));
        buf.extend(nlmsg(20, 24));
        assert!(netlink_pertinent(&buf));
        for ty in [16, 17, 21, 24, 25] {
            assert!(netlink_pertinent(&nlmsg(ty, 4)), "{ty}");
        }
        // a truncated header or a bogus length stops the walk
        assert!(!netlink_pertinent(&nlmsg(20, 0)[..15]));
        let mut bad = nlmsg(20, 0);
        bad[0] = 200;
        assert!(!netlink_pertinent(&bad));
    }

    #[test]
    fn route_messages() {
        let msg = |ty: u8, flags: i32| {
            let mut m = vec![0u8; 40];
            m[..2].copy_from_slice(&40u16.to_ne_bytes());
            m[2] = 5;
            m[3] = ty;
            m[8..12].copy_from_slice(&flags.to_ne_bytes());
            m
        };
        assert!(route_msg_pertinent(&msg(1, 0x1), 8, 0x11), "RTM_ADD");
        assert!(route_msg_pertinent(&msg(2, 0x3), 8, 0x11), "RTM_DELETE");
        assert!(!route_msg_pertinent(&msg(1, 0x400 | 0x4), 8, 0x11), "an ARP entry");
        assert!(route_msg_pertinent(&msg(0xc, 0), 8, 0x11), "RTM_NEWADDR");
        assert!(route_msg_pertinent(&msg(0xe, 0), 8, 0x11), "RTM_IFINFO");
        assert!(route_msg_pertinent(&msg(0x11, 0), 8, 0x11), "RTM_IFANNOUNCE");
        assert!(!route_msg_pertinent(&msg(0x4, 0), 8, 0x11), "RTM_GET");
        assert!(!route_msg_pertinent(&msg(0x7, 0), 8, 0x11), "RTM_MISS");
        let mut old = msg(0xc, 0);
        old[2] = 4;
        assert!(!route_msg_pertinent(&old, 8, 0x11), "another version");
    }

    /// The notifier starts where the OS has one, and a fresh one has nothing to report.
    #[test]
    fn notifier_starts() {
        let poll = mio::Poll::new().unwrap();
        match IpNotifier::new(Arc::new(|| {})) {
            Ok(mut n) => {
                n.register(poll.registry(), Token(1)).unwrap();
                assert!(!n.take_signal());
                n.deregister(poll.registry());
            }
            Err(e) => {
                assert_eq!(e.kind(), io::ErrorKind::Unsupported, "{e}");
                if cfg!(any(windows, target_os = "linux", target_os = "freebsd")) {
                    panic!("no notifier on a platform that has one: {e}");
                }
            }
        }
    }
}
