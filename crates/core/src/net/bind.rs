// Rookparcel's own code, not ported: libtorrent binds to the `outgoing_interfaces` and
// `listen_interfaces` it is given and has no rule that follows a VPN adapter (D54).

//! "Use only this network" (D54): which local addresses a [`NetworkRule`] picks from the
//! interfaces the OS lists, and which adapters look like VPN tunnels.

use std::io;
use std::net::{IpAddr, SocketAddr};

use super::enum_net::{IpInterface, build_netmask, is_global, is_link_local, match_addr_mask};
use crate::settings::NetworkRule;
use crate::status::NetAdapter;

/// Windows `IfType` values (IANA ifType) of tunnels: PPP (the built-in VPN client), a
/// proprietary virtual interface (Wintun, TAP-Windows) and a tunnel.
const IF_TYPE_PPP: u32 = 23;
const IF_TYPE_PROP_VIRTUAL: u32 = 53;
const IF_TYPE_TUNNEL: u32 = 131;

/// Words in a Windows adapter description that name a VPN driver.
const VPN_DRIVERS: [&str; 4] = ["wintun", "tap-windows", "wireguard", "tap adapter"];
/// Unix device-name prefixes of tunnels: Linux `wg*`, `tun*`, `tap*`; macOS / BSD `utun*`.
const VPN_PREFIXES: [&str; 4] = ["wg", "tun", "tap", "utun"];

/// Whether an interface looks like a VPN tunnel ("Any VPN adapter").
pub(crate) fn is_vpn(i: &IpInterface) -> bool {
    if i.loopback {
        return false;
    }
    let desc = i.description.to_ascii_lowercase();
    let name = i.name.to_ascii_lowercase();
    i.pointopoint
        || matches!(i.if_type, IF_TYPE_PPP | IF_TYPE_PROP_VIRTUAL | IF_TYPE_TUNNEL)
        || VPN_DRIVERS.iter().any(|d| desc.contains(d))
        || VPN_PREFIXES.iter().any(|p| {
            name.strip_prefix(p).is_some_and(|rest| {
                rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_digit() || c == '-' || c == '_')
            })
        })
}

/// An address that can carry torrent traffic: up, usable, not link-local (a link-local source
/// reaches nothing past the link).
fn usable(i: &IpInterface) -> bool {
    i.up && i.preferred && !is_link_local(&i.addr) && !i.addr.is_unspecified()
}

/// The adapters of these interfaces, one per stable id, in the order the OS lists them.
pub(crate) fn adapters(ifs: &[IpInterface]) -> Vec<NetAdapter> {
    let mut out: Vec<NetAdapter> = Vec::new();
    for i in ifs {
        if i.name.is_empty() {
            continue;
        }
        let k = match out.iter().position(|a| a.id == i.name) {
            Some(k) => k,
            None => {
                let name = if i.friendly.is_empty() { i.name.clone() } else { i.friendly.clone() };
                out.push(NetAdapter { id: i.name.clone(), name, vpn: false, addresses: Vec::new() });
                out.len() - 1
            }
        };
        let a = &mut out[k];
        a.vpn |= is_vpn(i);
        if usable(i) && !a.addresses.contains(&i.addr) {
            a.addresses.push(i.addr);
        }
    }
    out
}

/// What a rule matched: one address per family (`None`: that family carries no torrent
/// traffic), and the adapters they are on, for the log.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Binding {
    pub v4: Option<IpAddr>,
    pub v6: Option<IpAddr>,
    /// The friendly names of the adapters the addresses are on.
    pub adapters: Vec<String>,
}

impl Binding {
    /// The bound address of `remote`'s family.
    pub fn for_family(&self, v4: bool) -> Option<IpAddr> {
        if v4 { self.v4 } else { self.v6 }
    }

    pub fn is_empty(&self) -> bool {
        self.v4.is_none() && self.v6.is_none()
    }
}

/// The addresses `rule` picks from `ifs`; `None` for [`NetworkRule::Off`] (nothing is bound).
/// An empty binding means the rule matched nothing. Per family, a global address wins over a
/// local one, then the OS's order.
pub(crate) fn match_rule(rule: &NetworkRule, ifs: &[IpInterface]) -> Option<Binding> {
    let wanted = |i: &IpInterface| match rule {
        NetworkRule::Off => false,
        NetworkRule::Adapter(id) => i.name == *id,
        NetworkRule::Vpn => is_vpn(i),
        NetworkRule::Range(net, bits) => {
            net.is_ipv4() == i.addr.is_ipv4()
                && match_addr_mask(
                    i.addr.to_canonical(),
                    *net,
                    build_netmask(u32::from(*bits), net.is_ipv4()),
                )
        }
    };
    if *rule == NetworkRule::Off {
        return None;
    }
    let mut b = Binding::default();
    for v4 in [true, false] {
        let pick = ifs
            .iter()
            .filter(|i| i.addr.is_ipv4() == v4 && usable(i) && wanted(i))
            .min_by_key(|i| !is_global(&i.addr));
        if let Some(i) = pick {
            if v4 {
                b.v4 = Some(i.addr);
            } else {
                b.v6 = Some(i.addr);
            }
            let name = if i.friendly.is_empty() { i.name.clone() } else { i.friendly.clone() };
            if !b.adapters.contains(&name) {
                b.adapters.push(name);
            }
        }
    }
    Some(b)
}

/// `listen_interfaces` with its addresses replaced by the bound ones, on the port of its first
/// entry (0 when it has none): what the listen sockets, the uTP and DHT socket and LSD open.
/// Empty when the binding is empty: nothing listens.
pub(crate) fn bound_listen_interfaces(configured: &str, b: &Binding) -> String {
    let port = configured
        .split(',')
        .map(str::trim)
        .find(|e| !e.is_empty())
        .and_then(|e| e.rsplit_once(':'))
        .and_then(|(_, p)| p.trim_end_matches(|c: char| c.is_ascii_alphabetic()).parse::<u16>().ok())
        .unwrap_or(0);
    let mut out = Vec::new();
    if let Some(a) = b.v4 {
        out.push(format!("{a}:{port}"));
    }
    if let Some(a) = b.v6 {
        out.push(format!("[{a}]:{port}"));
    }
    out.join(",")
}

/// The local address an outgoing connection to `remote` binds to (libtorrent
/// `bind_outgoing_socket`): `None` without a rule, the bound address of its family with one.
/// A family the binding has no address of carries no torrent traffic: the connection fails
/// as a refused bind would.
pub(crate) fn outgoing_from(bind: Option<&Binding>, remote: SocketAddr) -> io::Result<Option<IpAddr>> {
    match bind {
        None => Ok(None),
        Some(b) => b.for_family(remote.is_ipv4()).map(Some).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "the network torrent traffic is bound to has no address of this family",
            )
        }),
    }
}

/// A non-blocking TCP connection to `to`, from `from` when given.
pub(crate) fn connect_tcp(from: Option<IpAddr>, to: SocketAddr) -> io::Result<mio::net::TcpStream> {
    match from {
        None => mio::net::TcpStream::connect(to),
        Some(ip) => {
            super::sys::tcp_connect_from(SocketAddr::new(ip, 0), to).map(mio::net::TcpStream::from_std)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// An address of an adapter: Windows-style when `if_type` is set (a GUID name, a friendly
    /// name and a description), else Unix-style (the device name only).
    fn ifc(addr: &str, name: &str, friendly: &str, description: &str, if_type: u32) -> IpInterface {
        let a = ip(addr);
        IpInterface {
            addr: a,
            netmask: build_netmask(if a.is_ipv4() { 24 } else { 64 }, a.is_ipv4()),
            scope_id: 0,
            index: 1,
            name: name.into(),
            friendly: friendly.into(),
            description: description.into(),
            if_type,
            up: true,
            loopback: a.is_loopback(),
            multicast: true,
            pointopoint: false,
            preferred: true,
        }
    }

    const ETH: &str = "{11111111-1111-1111-1111-111111111111}";
    const WG: &str = "{22222222-2222-2222-2222-222222222222}";
    const TAP: &str = "{33333333-3333-3333-3333-333333333333}";

    fn windows() -> Vec<IpInterface> {
        vec![
            ifc("127.0.0.1", "{loop}", "Loopback Pseudo-Interface 1", "Software Loopback Interface 1", 24),
            ifc("192.168.1.20", ETH, "Ethernet", "Intel(R) Ethernet Connection", 6),
            ifc("fe80::1", ETH, "Ethernet", "Intel(R) Ethernet Connection", 6),
            ifc("2001:db8::20", ETH, "Ethernet", "Intel(R) Ethernet Connection", 6),
            ifc("10.64.0.5", WG, "Mullvad", "Mullvad Tunnel", IF_TYPE_PROP_VIRTUAL),
            ifc("fc00:bbbb::5", WG, "Mullvad", "Mullvad Tunnel", IF_TYPE_PROP_VIRTUAL),
        ]
    }

    #[test]
    fn rules_parse_and_print() {
        for t in ["", "vpn", "adapter:wg0", "range:10.2.0.0/16", "range:fd00::/8"] {
            assert_eq!(NetworkRule::parse(t).unwrap().to_string(), t);
        }
        assert_eq!(NetworkRule::parse(" off ").unwrap(), NetworkRule::Off);
        assert_eq!(NetworkRule::parse("range:10.2.0.7").unwrap(), NetworkRule::Range(ip("10.2.0.7"), 32));
        for bad in ["adapter:", "range:10.2.0.0/33", "range:fd00::/129", "range:nope", "wifi"] {
            assert!(NetworkRule::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn vpn_adapters_are_recognised() {
        let w = |desc: &str, t: u32| is_vpn(&ifc("10.0.0.2", "{x}", "X", desc, t));
        assert!(w("Wintun Userspace Tunnel", 6), "Wintun by description");
        assert!(w("TAP-Windows Adapter V9", 6), "OpenVPN's TAP");
        assert!(w("WireGuard Tunnel", 6));
        assert!(w("WAN Miniport (IKEv2)", IF_TYPE_PPP), "the built-in VPN client");
        assert!(w("Some VPN", IF_TYPE_TUNNEL));
        assert!(w("NordLynx", IF_TYPE_PROP_VIRTUAL));
        assert!(!w("Intel(R) Wi-Fi 6 AX201", 71));
        assert!(!w("Realtek PCIe GbE", 6));
        let u = |name: &str| is_vpn(&ifc("10.0.0.2", name, name, "", 0));
        for name in ["wg0", "wg-mullvad", "tun0", "tap1", "utun3"] {
            assert!(u(name), "{name}");
        }
        for name in ["eth0", "wlan0", "enp3s0", "en0", "docker0", "tunnelbroker", "wgx"] {
            assert!(!u(name), "{name}");
        }
        // point-to-point devices (WireGuard on Linux under any name, PPP) are tunnels
        assert!(is_vpn(&IpInterface { pointopoint: true, ..ifc("10.0.0.2", "nordlynx", "nordlynx", "", 0) }));
        assert!(!is_vpn(&ifc("127.0.0.1", "lo", "lo", "", 0)), "never loopback");
    }

    #[test]
    fn adapters_are_grouped_by_id() {
        let a = adapters(&windows());
        let names: Vec<&str> = a.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Loopback Pseudo-Interface 1", "Ethernet", "Mullvad"]);
        assert_eq!(a[1].id, ETH);
        assert_eq!(a[1].addresses, [ip("192.168.1.20"), ip("2001:db8::20")], "no link-local address");
        assert!(!a[1].vpn);
        assert!(a[2].vpn);
    }

    #[test]
    fn the_vpn_rule_picks_the_tunnel() {
        let b = match_rule(&NetworkRule::Vpn, &windows()).unwrap();
        assert_eq!(b.v4, Some(ip("10.64.0.5")));
        assert_eq!(b.v6, Some(ip("fc00:bbbb::5")));
        assert_eq!(b.adapters, ["Mullvad"]);
        // the tunnel is down: nothing matches
        let down: Vec<IpInterface> = windows().into_iter().filter(|i| i.name != WG).collect();
        assert!(match_rule(&NetworkRule::Vpn, &down).unwrap().is_empty());
        assert_eq!(match_rule(&NetworkRule::Off, &windows()), None);
    }

    #[test]
    fn a_v4_only_tunnel_turns_ipv6_off() {
        let mut ifs = windows();
        ifs.retain(|i| !(i.name == WG && i.addr.is_ipv6()));
        let b = match_rule(&NetworkRule::Vpn, &ifs).unwrap();
        assert_eq!((b.v4, b.v6), (Some(ip("10.64.0.5")), None));
        assert_eq!(b.for_family(false), None);
        assert_eq!(bound_listen_interfaces("0.0.0.0:6881,[::]:6881", &b), "10.64.0.5:6881");
    }

    #[test]
    fn the_adapter_rule_follows_the_id_not_the_name() {
        let rule = NetworkRule::Adapter(ETH.into());
        let b = match_rule(&rule, &windows()).unwrap();
        assert_eq!((b.v4, b.v6), (Some(ip("192.168.1.20")), Some(ip("2001:db8::20"))));
        // renamed, re-addressed: still the same adapter
        let mut ifs = windows();
        for i in ifs.iter_mut().filter(|i| i.name == ETH) {
            i.friendly = "LAN".into();
            if i.addr == ip("192.168.1.20") {
                i.addr = ip("192.168.7.9");
            }
        }
        assert_eq!(match_rule(&rule, &ifs).unwrap().v4, Some(ip("192.168.7.9")));
        // a TAP adapter that is gone
        assert!(match_rule(&NetworkRule::Adapter(TAP.into()), &ifs).unwrap().is_empty());
        // Unix: the device name
        let unix = [ifc("10.8.0.2", "tun0", "tun0", "", 0), ifc("192.168.1.2", "eth0", "eth0", "", 0)];
        assert_eq!(match_rule(&NetworkRule::Adapter("tun0".into()), &unix).unwrap().v4, Some(ip("10.8.0.2")));
    }

    #[test]
    fn the_range_rule_matches_addresses_on_any_adapter() {
        let rule = NetworkRule::parse("range:10.64.0.0/10").unwrap();
        let b = match_rule(&rule, &windows()).unwrap();
        assert_eq!((b.v4, b.v6), (Some(ip("10.64.0.5")), None), "only addresses in the range");
        let rule6 = NetworkRule::parse("range:fc00::/7").unwrap();
        assert_eq!(match_rule(&rule6, &windows()).unwrap().v6, Some(ip("fc00:bbbb::5")));
        // loopback can be named by range (the tests bind to it)
        let lo = NetworkRule::parse("range:127.0.0.1/32").unwrap();
        assert_eq!(match_rule(&lo, &windows()).unwrap().v4, Some(ip("127.0.0.1")));
        assert!(
            match_rule(&NetworkRule::parse("range:172.16.0.0/12").unwrap(), &windows()).unwrap().is_empty()
        );
    }

    #[test]
    fn global_addresses_win_and_unusable_ones_never_bind() {
        let ifs = [
            ifc("fd00::7", "wg0", "wg0", "", 0),
            ifc("2001:db8::7", "wg0", "wg0", "", 0),
            IpInterface { up: false, ..ifc("10.9.0.1", "wg0", "wg0", "", 0) },
            IpInterface { preferred: false, ..ifc("10.9.0.2", "wg0", "wg0", "", 0) },
            ifc("10.9.0.3", "wg0", "wg0", "", 0),
        ];
        let b = match_rule(&NetworkRule::Vpn, &ifs).unwrap();
        assert_eq!((b.v4, b.v6), (Some(ip("10.9.0.3")), Some(ip("2001:db8::7"))));
        assert_eq!(bound_listen_interfaces("0.0.0.0:7000", &b), "10.9.0.3:7000,[2001:db8::7]:7000");
        assert_eq!(bound_listen_interfaces("", &Binding::default()), "");
        assert_eq!(bound_listen_interfaces("", &b), "10.9.0.3:0,[2001:db8::7]:0");
    }
}
