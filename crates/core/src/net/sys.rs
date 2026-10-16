// PaTorrent's own OS glue, not ported (libtorrent gets these through Boost.Asio and its own
// enum_net.cpp platform code).

//! Socket calls that std and mio do not offer: options set before `bind` (address reuse,
//! v6-only), the outgoing multicast interface and hop limit, the list of local interface
//! addresses and the routing table. Windows goes through `windows-sys`; Unix functions are
//! declared here by hand (D4: no libc crate), with the constants of each supported OS.

use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

use super::enum_net::{IpInterface, IpRoute, build_netmask};

/// A `sockaddr_in` / `sockaddr_in6`, built and read byte by byte so one layout serves every OS.
#[repr(C, align(8))]
struct SockaddrBuf([u8; 28]);

#[cfg(any(
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd",
    target_vendor = "apple"
))]
const SA_LEN_PREFIX: bool = true;
#[cfg(not(any(
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd",
    target_vendor = "apple"
)))]
const SA_LEN_PREFIX: bool = false;

const AF_INET: u16 = 2;

fn encode_sockaddr(addr: SocketAddr) -> (SockaddrBuf, usize) {
    let mut b = SockaddrBuf([0; 28]);
    let (af, len) = match addr {
        SocketAddr::V4(_) => (AF_INET, 16),
        SocketAddr::V6(_) => (AF_INET6, 28),
    };
    if SA_LEN_PREFIX {
        b.0[0] = len as u8;
        b.0[1] = af as u8;
    } else {
        b.0[..2].copy_from_slice(&af.to_ne_bytes());
    }
    b.0[2..4].copy_from_slice(&addr.port().to_be_bytes());
    match addr {
        SocketAddr::V4(a) => b.0[4..8].copy_from_slice(&a.ip().octets()),
        SocketAddr::V6(a) => {
            b.0[4..8].copy_from_slice(&a.flowinfo().to_be_bytes());
            b.0[8..24].copy_from_slice(&a.ip().octets());
            b.0[24..28].copy_from_slice(&a.scope_id().to_ne_bytes());
        }
    }
    (b, len)
}

/// Reads an address (and its IPv6 scope id) from a `sockaddr`. `family` overrides the one in
/// the buffer: BSD netmasks sometimes leave it unset.
///
/// # Safety
/// `p` must point to at least 16 readable bytes, 28 when the family is IPv6.
unsafe fn decode_sockaddr(p: *const u8, family: Option<u16>) -> Option<(IpAddr, u32)> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the caller guarantees the first two bytes are readable
    let head = unsafe { [*p, *p.add(1)] };
    let af = family.unwrap_or(if SA_LEN_PREFIX { head[1] as u16 } else { u16::from_ne_bytes(head) });
    if af == AF_INET {
        let mut a = [0u8; 4];
        // SAFETY: a sockaddr_in is 16 bytes
        unsafe { std::ptr::copy_nonoverlapping(p.add(4), a.as_mut_ptr(), 4) };
        Some((Ipv4Addr::from(a).into(), 0))
    } else if af == AF_INET6 {
        let mut a = [0u8; 16];
        let mut scope = [0u8; 4];
        // SAFETY: a sockaddr_in6 is 28 bytes
        unsafe {
            std::ptr::copy_nonoverlapping(p.add(8), a.as_mut_ptr(), 16);
            std::ptr::copy_nonoverlapping(p.add(24), scope.as_mut_ptr(), 4);
        }
        Some((Ipv6Addr::from(a).into(), u32::from_ne_bytes(scope)))
    } else {
        None
    }
}

#[cfg(unix)]
fn family_of(addr: &IpAddr) -> u16 {
    if addr.is_ipv4() { AF_INET } else { AF_INET6 }
}

/// Sets the interface outgoing IPv4 multicast leaves from.
pub(crate) fn set_multicast_if_v4(s: &UdpSocket, ip: Ipv4Addr) -> io::Result<()> {
    imp::setsockopt(s, IPPROTO_IP, IP_MULTICAST_IF, &ip.octets())
}

/// Sets the interface (by index) outgoing IPv6 multicast leaves from.
pub(crate) fn set_multicast_if_v6(s: &UdpSocket, index: u32) -> io::Result<()> {
    imp::setsockopt(s, IPPROTO_IPV6, IPV6_MULTICAST_IF, &index.to_ne_bytes())
}

/// The hop limit of outgoing IPv6 multicast (std only has the IPv4 TTL).
pub(crate) fn set_multicast_hops_v6(s: &UdpSocket, hops: u32) -> io::Result<()> {
    imp::setsockopt(s, IPPROTO_IPV6, IPV6_MULTICAST_HOPS, &hops.to_ne_bytes())
}

/// A UDP socket bound to `addr`. With `reuse` other sockets (other clients on this machine)
/// may bind the same port, as multicast listeners must. IPv6 sockets are v6-only.
pub(crate) fn udp_socket(addr: SocketAddr, reuse: bool) -> io::Result<UdpSocket> {
    let s = imp::socket(addr.is_ipv4())?;
    if reuse {
        imp::setsockopt(&s, SOL_SOCKET, SO_REUSEADDR, &1i32.to_ne_bytes())?;
        if let Some(opt) = SO_REUSEPORT {
            // BSDs share a wildcard-bound port between multicast listeners only with this
            imp::setsockopt(&s, SOL_SOCKET, opt, &1i32.to_ne_bytes())?;
        }
    }
    if addr.is_ipv6() {
        imp::setsockopt(&s, IPPROTO_IPV6, IPV6_V6ONLY, &1i32.to_ne_bytes())?;
    }
    let (sa, len) = encode_sockaddr(addr);
    imp::bind(&s, &sa, len)?;
    Ok(s)
}

/// Every IP address of every local interface (libtorrent `enum_net_interfaces`).
pub(crate) fn interfaces() -> io::Result<Vec<IpInterface>> {
    imp::interfaces()
}

/// The routing table (libtorrent `enum_routes`): `GetIpForwardTable2` on Windows, `/proc/net`
/// on Linux, a `PF_ROUTE` sysctl dump on FreeBSD and OpenBSD. Elsewhere `Unsupported`, and
/// NAT-PMP (which needs the gateway) stays off.
pub(crate) fn routes() -> io::Result<Vec<IpRoute>> {
    imp::routes()
}

const IPPROTO_IP: i32 = 0;
const IPPROTO_IPV6: i32 = 41;

#[cfg(windows)]
use windows_consts::*;
#[cfg(windows)]
mod windows_consts {
    use windows_sys::Win32::Networking::WinSock as ws;
    pub const AF_INET6: u16 = ws::AF_INET6;
    pub const SOL_SOCKET: i32 = ws::SOL_SOCKET;
    pub const SO_REUSEADDR: i32 = ws::SO_REUSEADDR;
    pub const SO_REUSEPORT: Option<i32> = None;
    pub const IP_MULTICAST_IF: i32 = ws::IP_MULTICAST_IF;
    pub const IPV6_MULTICAST_IF: i32 = ws::IPV6_MULTICAST_IF;
    pub const IPV6_MULTICAST_HOPS: i32 = ws::IPV6_MULTICAST_HOPS;
    pub const IPV6_V6ONLY: i32 = ws::IPV6_V6ONLY;
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod unix_consts {
    pub const AF_INET6: u16 = 10;
    pub const SOL_SOCKET: i32 = 1;
    pub const SO_REUSEADDR: i32 = 2;
    pub const SO_REUSEPORT: Option<i32> = None;
    pub const IP_MULTICAST_IF: i32 = 32;
    pub const IPV6_MULTICAST_IF: i32 = 17;
    pub const IPV6_MULTICAST_HOPS: i32 = 18;
    pub const IPV6_V6ONLY: i32 = 26;
    pub const SOCK_CLOEXEC: i32 = 0x80000;
    pub const IFF_MULTICAST: u32 = 0x1000;
}

#[cfg(any(
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "openbsd",
    target_os = "netbsd",
    target_vendor = "apple"
))]
mod unix_consts {
    #[cfg(any(target_os = "freebsd", target_os = "dragonfly"))]
    pub const AF_INET6: u16 = 28;
    #[cfg(any(target_os = "openbsd", target_os = "netbsd"))]
    pub const AF_INET6: u16 = 24;
    #[cfg(target_vendor = "apple")]
    pub const AF_INET6: u16 = 30;
    pub const SOL_SOCKET: i32 = 0xffff;
    pub const SO_REUSEADDR: i32 = 4;
    pub const SO_REUSEPORT: Option<i32> = Some(0x200);
    pub const IP_MULTICAST_IF: i32 = 9;
    pub const IPV6_MULTICAST_IF: i32 = 9;
    pub const IPV6_MULTICAST_HOPS: i32 = 10;
    pub const IPV6_V6ONLY: i32 = 27;
    #[cfg(any(target_os = "freebsd", target_os = "dragonfly", target_os = "netbsd"))]
    pub const SOCK_CLOEXEC: i32 = 0x1000_0000;
    #[cfg(target_os = "openbsd")]
    pub const SOCK_CLOEXEC: i32 = 0x8000;
    // macOS has no SOCK_CLOEXEC; PaTorrent starts no child processes that could inherit it
    #[cfg(target_vendor = "apple")]
    pub const SOCK_CLOEXEC: i32 = 0;
    pub const IFF_MULTICAST: u32 = 0x8000;
}

#[cfg(unix)]
use unix_consts::*;

#[cfg(windows)]
mod imp {
    use std::io;
    use std::net::UdpSocket;
    use std::os::windows::io::{AsRawSocket, FromRawSocket, RawSocket};
    use std::sync::Once;

    use windows_sys::Win32::Foundation::{ERROR_BUFFER_OVERFLOW, ERROR_NO_DATA, NO_ERROR};
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        FreeMibTable, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_DNS_SERVER, GAA_FLAG_SKIP_MULTICAST,
        GetAdaptersAddresses, GetIpForwardTable2, IF_TYPE_SOFTWARE_LOOPBACK, IP_ADAPTER_ADDRESSES_LH,
        IP_ADAPTER_NO_MULTICAST, MIB_IPFORWARD_TABLE2,
    };
    use windows_sys::Win32::NetworkManagement::Ndis::IfOperStatusUp;
    use windows_sys::Win32::Networking::WinSock as ws;

    use super::{IpInterface, IpRoute, SockaddrBuf, build_netmask, decode_sockaddr};

    pub fn routes() -> io::Result<Vec<IpRoute>> {
        let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
        // SAFETY: `table` is a valid out pointer; the table is freed below
        let r = unsafe { GetIpForwardTable2(ws::AF_UNSPEC, &mut table) };
        if r != NO_ERROR || table.is_null() {
            return Err(io::Error::from_raw_os_error(r as i32));
        }
        let mut out = Vec::new();
        // SAFETY: `NumEntries` rows follow the header in the one allocation Windows returned;
        // `SOCKADDR_INET` has the `sockaddr_in` / `sockaddr_in6` layout `decode_sockaddr` reads
        unsafe {
            let n = (*table).NumEntries as usize;
            let rows = (*table).Table.as_ptr();
            for i in 0..n {
                let row = &*rows.add(i);
                let gateway = decode_sockaddr((&raw const row.NextHop).cast::<u8>(), None);
                let dest = decode_sockaddr((&raw const row.DestinationPrefix.Prefix).cast::<u8>(), None);
                if let (Some((gateway, _)), Some((destination, _))) = (gateway, dest) {
                    out.push(IpRoute {
                        destination,
                        netmask: build_netmask(
                            row.DestinationPrefix.PrefixLength as u32,
                            destination.is_ipv4(),
                        ),
                        gateway,
                        source_hint: None,
                        index: row.InterfaceIndex,
                    });
                }
            }
            FreeMibTable(table.cast());
        }
        Ok(out)
    }

    fn last_error() -> io::Error {
        // SAFETY: no preconditions
        io::Error::from_raw_os_error(unsafe { ws::WSAGetLastError() })
    }

    /// std starts Winsock lazily; a socket made here may be the process's first.
    fn startup() {
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let mut data: ws::WSADATA = unsafe { std::mem::zeroed() };
            // SAFETY: `data` is a valid out pointer; Winsock stays loaded for the process
            unsafe { ws::WSAStartup(0x0202, &mut data) };
        });
    }

    pub fn socket(v4: bool) -> io::Result<UdpSocket> {
        startup();
        let af = if v4 { ws::AF_INET } else { ws::AF_INET6 } as i32;
        // SAFETY: plain socket creation with the flags std uses
        let s = unsafe {
            ws::WSASocketW(
                af,
                ws::SOCK_DGRAM,
                ws::IPPROTO_UDP,
                std::ptr::null(),
                0,
                ws::WSA_FLAG_OVERLAPPED | ws::WSA_FLAG_NO_HANDLE_INHERIT,
            )
        };
        if s == ws::INVALID_SOCKET {
            return Err(last_error());
        }
        // SAFETY: `s` is a fresh socket we own
        Ok(unsafe { UdpSocket::from_raw_socket(s as RawSocket) })
    }

    pub fn setsockopt(s: &UdpSocket, level: i32, name: i32, value: &[u8]) -> io::Result<()> {
        // SAFETY: `value` is valid for its length
        let r = unsafe {
            ws::setsockopt(s.as_raw_socket() as ws::SOCKET, level, name, value.as_ptr(), value.len() as i32)
        };
        if r == 0 { Ok(()) } else { Err(last_error()) }
    }

    pub(super) fn bind(s: &UdpSocket, sa: &SockaddrBuf, len: usize) -> io::Result<()> {
        // SAFETY: `sa` holds a sockaddr of `len` bytes
        let r = unsafe { ws::bind(s.as_raw_socket() as ws::SOCKET, sa.0.as_ptr().cast(), len as i32) };
        if r == 0 { Ok(()) } else { Err(last_error()) }
    }

    pub fn interfaces() -> io::Result<Vec<IpInterface>> {
        let flags = GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST | GAA_FLAG_SKIP_DNS_SERVER;
        let mut size: u32 = 16 * 1024;
        let mut buf: Vec<u64>;
        loop {
            buf = vec![0; (size as usize).div_ceil(8)];
            // SAFETY: `buf` holds `size` writable bytes, 8-byte aligned
            let r = unsafe {
                GetAdaptersAddresses(
                    ws::AF_UNSPEC as u32,
                    flags,
                    std::ptr::null(),
                    buf.as_mut_ptr().cast(),
                    &mut size,
                )
            };
            match r {
                NO_ERROR => break,
                ERROR_BUFFER_OVERFLOW => continue,
                ERROR_NO_DATA => return Ok(Vec::new()),
                e => return Err(io::Error::from_raw_os_error(e as i32)),
            }
        }
        let mut out = Vec::new();
        let mut a = buf.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while !a.is_null() {
            // SAFETY: a linked list inside `buf`, written by GetAdaptersAddresses
            let ad = unsafe { &*a };
            let name = if ad.AdapterName.is_null() {
                String::new()
            } else {
                // SAFETY: a NUL-terminated string inside `buf`
                unsafe { std::ffi::CStr::from_ptr(ad.AdapterName.cast()) }.to_string_lossy().into_owned()
            };
            let up = ad.OperStatus == IfOperStatusUp;
            let loopback = ad.IfType == IF_TYPE_SOFTWARE_LOOPBACK;
            // SAFETY: plain integers in unions
            let (multicast, index4) = unsafe {
                (ad.Anonymous2.Flags & IP_ADAPTER_NO_MULTICAST == 0, ad.Anonymous1.Anonymous.IfIndex)
            };
            let mut u = ad.FirstUnicastAddress;
            while !u.is_null() {
                // SAFETY: as above
                let ua = unsafe { &*u };
                // SAFETY: Windows fills in a sockaddr of `iSockaddrLength` bytes
                if let Some((addr, scope_id)) =
                    unsafe { decode_sockaddr(ua.Address.lpSockaddr as *const u8, None) }
                {
                    out.push(IpInterface {
                        addr,
                        netmask: build_netmask(ua.OnLinkPrefixLength as u32, addr.is_ipv4()),
                        scope_id,
                        index: if addr.is_ipv4() { index4 } else { ad.Ipv6IfIndex },
                        name: name.clone(),
                        up,
                        loopback,
                        multicast,
                        preferred: ua.DadState == ws::IpDadStatePreferred,
                    });
                }
                u = ua.Next;
            }
            a = ad.Next;
        }
        Ok(out)
    }
}

#[cfg(unix)]
mod imp {
    use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
    use std::io;
    use std::net::UdpSocket;
    use std::os::fd::{AsRawFd, FromRawFd};

    use super::{
        IFF_MULTICAST, IpInterface, IpRoute, SOCK_CLOEXEC, SockaddrBuf, build_netmask, decode_sockaddr,
        family_of,
    };

    const SOCK_DGRAM: c_int = 2;

    /// Linux `/proc/net/route` and `/proc/net/ipv6_route`: no netlink needed (libtorrent uses
    /// `RTM_GETROUTE`; the files carry the same table).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    pub fn routes() -> io::Result<Vec<IpRoute>> {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        const RTF_UP: u32 = 1;
        const RTF_GATEWAY: u32 = 2;
        let index_of = |name: &str| -> u32 {
            let Ok(c) = std::ffi::CString::new(name) else { return 0 };
            // SAFETY: a NUL-terminated name
            unsafe { if_nametoindex(c.as_ptr()) }
        };
        let mut out = Vec::new();
        let v4 = std::fs::read_to_string("/proc/net/route")?;
        for line in v4.lines().skip(1) {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 8 {
                continue;
            }
            let hex = |s: &str| u32::from_str_radix(s, 16);
            let (Ok(dst), Ok(gw), Ok(flags), Ok(mask)) = (hex(f[1]), hex(f[2]), hex(f[3]), hex(f[7])) else {
                continue;
            };
            if flags & RTF_UP == 0 {
                continue;
            }
            // the kernel prints each address as a host-order integer of its network-order bytes
            let ip = |v: u32| IpAddr::V4(Ipv4Addr::from(v.to_ne_bytes()));
            out.push(IpRoute {
                destination: ip(dst),
                netmask: ip(mask),
                gateway: ip(if flags & RTF_GATEWAY != 0 { gw } else { 0 }),
                source_hint: None,
                index: index_of(f[0]),
            });
        }
        if let Ok(v6) = std::fs::read_to_string("/proc/net/ipv6_route") {
            for line in v6.lines() {
                let f: Vec<&str> = line.split_whitespace().collect();
                if f.len() < 10 {
                    continue;
                }
                let hex128 = |s: &str| u128::from_str_radix(s, 16);
                let (Ok(dst), Ok(plen), Ok(gw), Ok(flags)) = (
                    hex128(f[0]),
                    u32::from_str_radix(f[1], 16),
                    hex128(f[4]),
                    u32::from_str_radix(f[8], 16),
                ) else {
                    continue;
                };
                if flags & RTF_UP == 0 {
                    continue;
                }
                out.push(IpRoute {
                    destination: Ipv6Addr::from(dst).into(),
                    netmask: build_netmask(plen, false),
                    gateway: Ipv6Addr::from(if flags & RTF_GATEWAY != 0 { gw } else { 0 }).into(),
                    source_hint: None,
                    index: index_of(f[9]),
                });
            }
        }
        Ok(out)
    }

    /// A `NET_RT_DUMP` sysctl, parsed as libtorrent's `parse_route` does: each `rt_msghdr` is
    /// followed by the sockaddrs its `rtm_addrs` bits name, each rounded up to a `long`.
    #[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
    pub fn routes() -> io::Result<Vec<IpRoute>> {
        const CTL_NET: c_int = 4;
        const PF_ROUTE: c_int = 17;
        const NET_RT_DUMP: c_int = 1;
        #[cfg(target_os = "freebsd")]
        let mib = [CTL_NET, PF_ROUTE, 0, 0, NET_RT_DUMP, 0];
        // OpenBSD takes the routing table id as a seventh element
        #[cfg(target_os = "openbsd")]
        let mib = [CTL_NET, PF_ROUTE, 0, 0, NET_RT_DUMP, 0, 0];
        let mut needed: usize = 0;
        // SAFETY: a size query; `needed` is a valid out pointer
        if unsafe {
            sysctl(mib.as_ptr(), mib.len() as c_uint, std::ptr::null_mut(), &mut needed, std::ptr::null(), 0)
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        if needed == 0 {
            return Ok(Vec::new());
        }
        let mut buf = vec![0u8; needed];
        // SAFETY: `buf` holds `needed` writable bytes
        if unsafe {
            sysctl(
                mib.as_ptr(),
                mib.len() as c_uint,
                buf.as_mut_ptr().cast(),
                &mut needed,
                std::ptr::null(),
                0,
            )
        } < 0
        {
            return Err(io::Error::last_os_error());
        }
        buf.truncate(needed);
        Ok(parse_rt_dump(&buf))
    }

    #[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
    fn parse_rt_dump(buf: &[u8]) -> Vec<IpRoute> {
        const RTM_VERSION: u8 = 5;
        const RTM_ADD: u8 = 1;
        const RTM_GET: u8 = 4;
        const RTAX_DST: usize = 0;
        const RTAX_GATEWAY: usize = 1;
        const RTAX_NETMASK: usize = 2;
        const RTAX_IFA: usize = 5;
        const RTAX_MAX: usize = 8;
        let long = std::mem::size_of::<usize>();
        let roundup = |a: usize| if a > 0 { 1 + ((a - 1) | (long - 1)) } else { long };
        let u16_at = |m: &[u8], o: usize| u16::from_ne_bytes([m[o], m[o + 1]]);
        let i32_at = |m: &[u8], o: usize| i32::from_ne_bytes([m[o], m[o + 1], m[o + 2], m[o + 3]]);
        let mut out = Vec::new();
        let mut off = 0;
        while off + 20 <= buf.len() {
            let msglen = u16_at(buf, off) as usize;
            if msglen < 20 || off + msglen > buf.len() {
                break;
            }
            let m = &buf[off..off + msglen];
            off += msglen;
            if m[2] != RTM_VERSION || (m[3] != RTM_ADD && m[3] != RTM_GET) {
                continue;
            }
            // FreeBSD: rtm_index at 4, rtm_addrs at 12, then rt_metrics (14 longs) after
            // rtm_inits (a long at 32). OpenBSD: rtm_hdrlen at 4 says where the sockaddrs start.
            #[cfg(target_os = "freebsd")]
            let (hdrlen, index, addrs) = (32 + 15 * long, u16_at(m, 4) as u32, i32_at(m, 12));
            #[cfg(target_os = "openbsd")]
            let (hdrlen, index, addrs) = (u16_at(m, 4) as usize, u16_at(m, 6) as u32, i32_at(m, 12));
            if hdrlen > m.len() {
                continue;
            }
            let mut sas: [Option<SockaddrBuf>; RTAX_MAX] = Default::default();
            let mut sa_off = hdrlen;
            for (i, slot) in sas.iter_mut().enumerate() {
                if addrs & (1 << i) == 0 {
                    continue;
                }
                if sa_off >= m.len() {
                    break;
                }
                let sa_len = m[sa_off] as usize;
                let mut sa = SockaddrBuf([0; 28]);
                let take = sa_len.min(28).min(m.len() - sa_off);
                sa.0[..take].copy_from_slice(&m[sa_off..sa_off + take]);
                *slot = Some(sa);
                sa_off += roundup(sa_len);
            }
            // SAFETY: 28-byte buffers
            let decode = |sa: &Option<SockaddrBuf>, fam: Option<u16>| {
                sa.as_ref().and_then(|s| unsafe { decode_sockaddr(s.0.as_ptr(), fam) }).map(|(a, _)| a)
            };
            // a route needs all three; a link-layer gateway (a directly connected network)
            // has no IP family and is skipped, as in libtorrent
            let (Some(gateway), Some(destination)) =
                (decode(&sas[RTAX_GATEWAY], None), decode(&sas[RTAX_DST], None))
            else {
                continue;
            };
            let Some(netmask) = decode(&sas[RTAX_NETMASK], Some(family_of(&destination))) else { continue };
            out.push(IpRoute {
                destination,
                netmask,
                gateway,
                source_hint: decode(&sas[RTAX_IFA], None),
                index,
            });
        }
        out
    }

    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "openbsd"
    )))]
    pub fn routes() -> io::Result<Vec<IpRoute>> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "route enumeration is not implemented on this OS"))
    }
    const IFF_UP: c_uint = 0x1;
    const IFF_LOOPBACK: c_uint = 0x8;

    /// The leading fields of `struct ifaddrs`, the same on Linux, the BSDs and macOS.
    #[repr(C)]
    struct IfAddrs {
        next: *mut IfAddrs,
        name: *const c_char,
        flags: c_uint,
        addr: *const u8,
        netmask: *const u8,
    }

    unsafe extern "C" {
        #[link_name = "socket"]
        fn c_socket(domain: c_int, ty: c_int, protocol: c_int) -> c_int;
        #[link_name = "bind"]
        fn c_bind(fd: c_int, addr: *const c_void, len: u32) -> c_int;
        #[link_name = "setsockopt"]
        fn c_setsockopt(fd: c_int, level: c_int, name: c_int, value: *const c_void, len: u32) -> c_int;
        fn getifaddrs(out: *mut *mut IfAddrs) -> c_int;
        fn freeifaddrs(ifa: *mut IfAddrs);
        fn if_nametoindex(name: *const c_char) -> c_uint;
        #[cfg(any(target_os = "freebsd", target_os = "openbsd"))]
        fn sysctl(
            name: *const c_int,
            namelen: c_uint,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *const c_void,
            newlen: usize,
        ) -> c_int;
    }

    pub fn socket(v4: bool) -> io::Result<UdpSocket> {
        let af = if v4 { super::AF_INET } else { super::AF_INET6 } as c_int;
        // SAFETY: plain socket creation
        let fd = unsafe { c_socket(af, SOCK_DGRAM | SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh socket we own
        Ok(unsafe { UdpSocket::from_raw_fd(fd) })
    }

    pub fn setsockopt(s: &UdpSocket, level: i32, name: i32, value: &[u8]) -> io::Result<()> {
        // SAFETY: `value` is valid for its length
        let r =
            unsafe { c_setsockopt(s.as_raw_fd(), level, name, value.as_ptr().cast(), value.len() as u32) };
        if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    pub(super) fn bind(s: &UdpSocket, sa: &SockaddrBuf, len: usize) -> io::Result<()> {
        // SAFETY: `sa` holds a sockaddr of `len` bytes
        let r = unsafe { c_bind(s.as_raw_fd(), sa.0.as_ptr().cast(), len as u32) };
        if r == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
    }

    pub fn interfaces() -> io::Result<Vec<IpInterface>> {
        let mut head: *mut IfAddrs = std::ptr::null_mut();
        // SAFETY: `head` is a valid out pointer
        if unsafe { getifaddrs(&mut head) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut out = Vec::new();
        let mut p = head;
        while !p.is_null() {
            // SAFETY: a list allocated by getifaddrs, freed below
            let ifa = unsafe { &*p };
            p = ifa.next;
            // SAFETY: getifaddrs fills in complete sockaddrs (or null)
            let Some((addr, scope_id)) = (unsafe { decode_sockaddr(ifa.addr, None) }) else { continue };
            // SAFETY: as above; the netmask has the address's family
            let netmask = unsafe { decode_sockaddr(ifa.netmask, Some(family_of(&addr))) }.map_or_else(
                || build_netmask(if addr.is_ipv4() { 32 } else { 128 }, addr.is_ipv4()),
                |m| m.0,
            );
            let (name, index) = if ifa.name.is_null() {
                (String::new(), 0)
            } else {
                // SAFETY: a NUL-terminated interface name
                let name = unsafe { CStr::from_ptr(ifa.name) }.to_string_lossy().into_owned();
                // SAFETY: as above
                (name, unsafe { if_nametoindex(ifa.name) })
            };
            out.push(IpInterface {
                addr,
                netmask,
                scope_id,
                index,
                name,
                up: ifa.flags & IFF_UP != 0,
                loopback: ifa.flags & IFF_LOOPBACK != 0,
                multicast: ifa.flags & IFF_MULTICAST != 0,
                // getifaddrs does not say whether an IPv6 address is deprecated or tentative
                preferred: true,
            });
        }
        // SAFETY: allocated by getifaddrs above
        unsafe { freeifaddrs(head) };
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sockaddr_round_trip() {
        for a in ["192.168.1.7:6771", "[fe80::1%7]:6771", "[2001:db8::5]:1"] {
            let a: SocketAddr = a.parse().unwrap();
            let (buf, _) = encode_sockaddr(a);
            // SAFETY: a 28-byte buffer
            let (ip, scope) = unsafe { decode_sockaddr(buf.0.as_ptr(), None) }.unwrap();
            assert_eq!(ip, a.ip());
            if let SocketAddr::V6(v6) = a {
                assert_eq!(scope, v6.scope_id());
            }
        }
    }

    #[test]
    fn reusable_ports_can_be_shared() {
        let a = udp_socket("127.0.0.1:0".parse().unwrap(), true).unwrap();
        let port = a.local_addr().unwrap().port();
        let b = udp_socket(SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), port), true).unwrap();
        assert_eq!(b.local_addr().unwrap().port(), port);
        set_multicast_if_v4(&b, Ipv4Addr::LOCALHOST).unwrap();
    }

    #[test]
    fn local_interfaces_are_listed() {
        let ifs = interfaces().unwrap();
        assert!(ifs.iter().any(|i| i.addr == IpAddr::V4(Ipv4Addr::LOCALHOST) && i.loopback), "{ifs:?}");
    }

    #[test]
    fn routes_are_listed() {
        match routes() {
            Ok(routes) => {
                // every route's interface is one we list, and the masks fit their families
                let ifs = interfaces().unwrap();
                for r in &routes {
                    assert_eq!(r.destination.is_ipv4(), r.netmask.is_ipv4(), "{r:?}");
                    assert_eq!(r.destination.is_ipv4(), r.gateway.is_ipv4(), "{r:?}");
                    assert!(ifs.iter().any(|i| i.index == r.index) || r.index == 0, "{r:?} not in {ifs:?}");
                }
                if cfg!(any(windows, target_os = "linux")) {
                    assert!(!routes.is_empty());
                }
            }
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::Unsupported, "{e}"),
        }
    }
}
