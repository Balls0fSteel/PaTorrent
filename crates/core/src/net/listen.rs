// Ported from libtorrent-rasterbar 2.0.11: src/session_impl.cpp (listen_endpoint_t,
// partition_listen_sockets, expand_unspecified_address, expand_devices, interface_to_endpoints),
// src/string_util.cpp (parse_listen_interfaces, print_listen_interfaces, split_string_quotes),
// include/libtorrent/aux_/session_impl.hpp (listen_socket_flags_t), test/test_listen_socket.cpp,
// test/test_string.cpp (parse_interface, split_string_quotes)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: routes name their interface by index, so the "internet route" test compares indices
// where libtorrent compares device names. An IPv6 address with a `%scope` suffix is taken as a
// device name (`IpAddr` has no scope id). `remove_duplicates` and `listen_plan` are
// PaTorrent's: the engine binds wildcard addresses as they are and uses libtorrent's expanded
// list for everything that depends on the interfaces (see engine::listen).

//! `listen_interfaces`: parsing it, and turning it into the endpoints to listen on.

use std::fmt;
use std::net::{IpAddr, Ipv6Addr};

use super::enum_net::{
    IpInterface, IpRoute, build_netmask, has_any_internet_route, has_internet_route, is_global,
    is_link_local, match_addr_mask,
};
use crate::flags::bit_flags;

/// One entry of `listen_interfaces` (libtorrent `listen_interface_t`): an IP address or a
/// device name, the port, and the `s` (SSL) and `l` (local network only) flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ListenInterface {
    pub device: String,
    pub port: u16,
    pub ssl: bool,
    pub local: bool,
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c')
}

/// libtorrent `strip_string`.
fn strip(s: &str) -> &str {
    s.trim_matches(is_space)
}

/// libtorrent `split_string_quotes`: splits at the first `sep`, except inside a leading quoted
/// part.
pub(crate) fn split_string_quotes(last: &str, sep: char) -> (&str, &str) {
    if last.is_empty() {
        return ("", "");
    }
    let b = last.as_bytes();
    let sep = sep as u8;
    let mut pos = 0;
    if b[0] == b'"' && sep != b'"' {
        for &c in &b[1..] {
            pos += 1;
            if c == b'"' {
                break;
            }
        }
    }
    let mut found_sep = 0;
    for &c in &b[pos..] {
        if c == sep {
            found_sep = 1;
            break;
        }
        pos += 1;
    }
    (&last[..pos], &last[pos + found_sep..])
}

/// libtorrent `parse_listen_interfaces`: a comma-separated list of `device:port` or `ip:port`
/// (`[ip]:port` for IPv6), each optionally followed by `s` (SSL) and `l` (local network).
/// Malformed entries are skipped and returned in `err`.
pub(crate) fn parse_listen_interfaces(input: &str, err: &mut Vec<String>) -> Vec<ListenInterface> {
    let mut out = Vec::new();
    let mut rest = input;
    while !rest.is_empty() {
        let (element, r) = split_string_quotes(rest, ',');
        rest = r;
        let mut element = strip(element);
        if element.len() > 1 && element.starts_with('"') && element.ends_with('"') {
            element = &element[1..element.len() - 1];
        }
        if element.is_empty() {
            continue;
        }
        let (device, port) = if let Some(after) = element.strip_prefix('[') {
            match after.find(']') {
                Some(pos) if after[pos + 1..].starts_with(':') => {
                    (strip(&after[..pos]), strip(&after[pos + 2..]))
                }
                _ => {
                    err.push(element.to_owned());
                    continue;
                }
            }
        } else {
            match element.find(':') {
                Some(pos) => (strip(&element[..pos]), strip(&element[pos + 1..])),
                None => {
                    err.push(element.to_owned());
                    continue;
                }
            }
        };
        let digits = port.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 || digits > 5 {
            err.push(element.to_owned());
            continue;
        }
        let Ok(port_num) = port[..digits].parse::<u16>() else {
            err.push(element.to_owned());
            continue;
        };
        let mut iface =
            ListenInterface { device: device.to_owned(), port: port_num, ssl: false, local: false };
        for c in strip(&port[digits..]).chars() {
            match c {
                's' => iface.ssl = true,
                'l' => iface.local = true,
                _ => {}
            }
        }
        out.push(iface);
    }
    out
}

/// libtorrent `print_listen_interfaces`: the canonical form of a parsed list.
pub(crate) fn print_listen_interfaces(list: &[ListenInterface]) -> String {
    let mut ret = String::new();
    for i in list {
        if !ret.is_empty() {
            ret.push(',');
        }
        if i.device.parse::<Ipv6Addr>().is_ok() {
            // IPv6 addresses must be wrapped in square brackets
            ret.push('[');
            ret.push_str(&i.device);
            ret.push(']');
        } else {
            ret.push_str(&i.device);
        }
        ret.push(':');
        ret.push_str(&i.port.to_string());
        if i.ssl {
            ret.push('s');
        }
        if i.local {
            ret.push('l');
        }
    }
    ret
}

bit_flags! {
    /// libtorrent `listen_socket_flags_t`.
    pub(crate) struct ListenFlags: u8 {
        /// The socket accepts incoming TCP connections.
        const ACCEPT_INCOMING = 0;
        /// The address cannot reach the internet: no DHT, trackers or port mapping on it.
        const LOCAL_NETWORK = 1;
        /// Expanded from a wildcard address or a device name.
        const WAS_EXPANDED = 2;
        /// Peer connections go through a proxy (unused until proxies are ported).
        const PROXY = 3;
    }
}

/// libtorrent `listen_endpoint_t`: an address to listen on, its configured port, device and
/// flags. Equality ignores the netmask, as libtorrent's does.
#[derive(Clone)]
pub(crate) struct ListenEndpoint {
    pub addr: IpAddr,
    /// If this endpoint has no gateway it cannot route outside its network; the netmask is the
    /// range of that local network.
    pub netmask: IpAddr,
    pub port: u16,
    pub device: String,
    pub ssl: bool,
    pub flags: ListenFlags,
}

impl ListenEndpoint {
    pub fn new(addr: IpAddr, port: u16, device: impl Into<String>, ssl: bool, flags: ListenFlags) -> Self {
        let netmask = build_netmask(0, addr.is_ipv4());
        ListenEndpoint { addr, netmask, port, device: device.into(), ssl, flags }
    }

    pub fn is_local_network(&self) -> bool {
        self.flags.contains(ListenFlags::LOCAL_NETWORK)
    }
}

impl PartialEq for ListenEndpoint {
    fn eq(&self, o: &Self) -> bool {
        self.addr == o.addr
            && self.port == o.port
            && self.device == o.device
            && self.ssl == o.ssl
            && self.flags == o.flags
    }
}

impl Eq for ListenEndpoint {}

impl fmt::Debug for ListenEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self}")
    }
}

impl fmt::Display for ListenEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.addr {
            IpAddr::V4(a) => write!(f, "{a}:{}", self.port)?,
            IpAddr::V6(a) => write!(f, "[{a}]:{}", self.port)?,
        }
        if !self.device.is_empty() {
            write!(f, " on \"{}\"", self.device)?;
        }
        if self.ssl {
            f.write_str(" ssl")?;
        }
        if self.is_local_network() {
            f.write_str(" local-network")?;
        }
        Ok(())
    }
}

/// libtorrent `interface_to_endpoints`: an IP address is one endpoint; a device name stands
/// for every address of that device.
pub(crate) fn interface_to_endpoints(
    iface: &ListenInterface,
    mut flags: ListenFlags,
    ifs: &[IpInterface],
    eps: &mut Vec<ListenEndpoint>,
) {
    if iface.local {
        flags |= ListenFlags::LOCAL_NETWORK;
    }
    if let Ok(addr) = iface.device.parse::<IpAddr>() {
        eps.push(ListenEndpoint::new(addr, iface.port, "", iface.ssl, flags));
        return;
    }
    flags |= ListenFlags::WAS_EXPANDED;
    // a network device: every address associated with it
    for ipface in ifs.iter().filter(|i| i.name == iface.device) {
        let local = iface.local || ipface.addr.is_loopback() || is_link_local(&ipface.addr);
        let f = if local { flags | ListenFlags::LOCAL_NETWORK } else { flags };
        eps.push(ListenEndpoint::new(ipface.addr, iface.port, iface.device.clone(), iface.ssl, f));
    }
}

/// libtorrent `expand_unspecified_address`: every wildcard endpoint is replaced by one per
/// preferred, up address of its family (of its device, if it names one), except addresses
/// another endpoint lists already. Addresses that cannot reach the internet are marked
/// `LOCAL_NETWORK`.
pub(crate) fn expand_unspecified_address(
    ifs: &[IpInterface],
    routes: &[IpRoute],
    eps: &mut Vec<ListenEndpoint>,
) {
    let (unspecified, specific): (Vec<ListenEndpoint>, Vec<ListenEndpoint>) =
        std::mem::take(eps).into_iter().partition(|e| e.addr.is_unspecified());
    *eps = specific;
    // if the routing table has no route to the internet at all, we don't use the last case of
    // `local` (macOS can notify us of network changes before the routing table is updated)
    let any_internet = has_any_internet_route(routes);
    for uep in &unspecified {
        let v4 = uep.addr.is_ipv4();
        for ipface in ifs {
            if !ipface.preferred || ipface.addr.is_ipv4() != v4 {
                continue;
            }
            if !uep.device.is_empty() && uep.device != ipface.name {
                continue;
            }
            // ignore the device name: a user-configured address without a device must not be
            // duplicated
            if eps.iter().any(|e| e.addr == ipface.addr && e.port == uep.port && e.ssl == uep.ssl) {
                continue;
            }
            // ignore interfaces that are down
            if !ipface.up {
                continue;
            }
            // local-network endpoints are not announced to internet trackers
            let local = ipface.addr.is_loopback()
                || is_link_local(&ipface.addr)
                || ipface.loopback
                || (!is_global(&ipface.addr)
                    && !ipface.pointopoint
                    && any_internet
                    && !has_internet_route(ipface.index, v4, routes));
            let mut flags = uep.flags | ListenFlags::WAS_EXPANDED;
            if local {
                flags |= ListenFlags::LOCAL_NETWORK;
            }
            eps.push(ListenEndpoint::new(ipface.addr, uep.port, uep.device.clone(), uep.ssl, flags));
        }
    }
}

/// libtorrent `expand_devices`: each endpoint takes the netmask and device name of the
/// interface whose subnet holds it (its named device's, if it has one). Without one it can't
/// reach anything: the netmask is empty.
pub(crate) fn expand_devices(ifs: &[IpInterface], eps: &mut [ListenEndpoint]) {
    for ep in eps.iter_mut() {
        let iface = ifs.iter().find(|i| {
            (ep.device.is_empty() || i.name == ep.device) && match_addr_mask(i.addr, ep.addr, i.netmask)
        });
        match iface {
            Some(i) => {
                ep.netmask = i.netmask;
                ep.device = i.name.clone();
            }
            None => ep.netmask = build_netmask(0, ep.addr.is_ipv4()),
        }
    }
}

/// libtorrent `partition_listen_sockets`: moves the sockets that match one of `eps` (same
/// address, configured port, device, SSL and flags) to the front, in their order, and removes
/// each matched endpoint so no other socket can match it. Returns the number of matched
/// sockets; the rest are to be closed, and what is left of `eps` is to be opened.
pub(crate) fn partition_listen_sockets<S>(
    eps: &mut Vec<ListenEndpoint>,
    sockets: &mut Vec<S>,
    endpoint_of: impl Fn(&S) -> &ListenEndpoint,
) -> usize {
    let (keep, remove): (Vec<S>, Vec<S>) = std::mem::take(sockets).into_iter().partition(|s| {
        let sock = endpoint_of(s);
        match eps.iter().position(|ep| ep == sock) {
            Some(i) => {
                eps.remove(i);
                true
            }
            None => false,
        }
    });
    let n = keep.len();
    sockets.extend(keep);
    sockets.extend(remove);
    n
}

/// Drops endpoints that would bind an address twice, and specific addresses that a wildcard
/// endpoint of the same family covers. All listen sockets share one port, so a second bind of
/// an address fails, and a specific address beside the wildcard fails on Linux (elsewhere it
/// would take that address's connections from the wildcard socket, to the same effect).
pub(crate) fn remove_duplicates(eps: &mut Vec<ListenEndpoint>) {
    let wildcards: Vec<(bool, bool)> =
        eps.iter().filter(|e| e.addr.is_unspecified()).map(|e| (e.addr.is_ipv4(), e.ssl)).collect();
    let mut seen: Vec<(IpAddr, bool)> = Vec::new();
    eps.retain(|e| {
        if seen.contains(&(e.addr, e.ssl)) {
            return false;
        }
        seen.push((e.addr, e.ssl));
        e.addr.is_unspecified() || !wildcards.contains(&(e.addr.is_ipv4(), e.ssl))
    });
}

/// The endpoints for one set of interfaces and routes.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ListenPlan {
    /// The sockets to bind: the configured endpoints, device names expanded into the device's
    /// addresses, wildcard addresses kept as they are (one socket serves every address of the
    /// family, present and future), duplicates removed.
    pub bind: Vec<ListenEndpoint>,
    /// libtorrent's list: wildcards expanded into every interface address as well. LSD, port
    /// mapping, the local addresses and the reaction to network changes follow it.
    pub expanded: Vec<ListenEndpoint>,
}

/// The listen endpoints for `ifaces` (SSL entries are skipped: no SSL torrents) on these
/// interfaces and routes.
pub(crate) fn listen_plan(ifaces: &[ListenInterface], ifs: &[IpInterface], routes: &[IpRoute]) -> ListenPlan {
    let mut eps = Vec::new();
    for iface in ifaces.iter().filter(|i| !i.ssl) {
        interface_to_endpoints(iface, ListenFlags::ACCEPT_INCOMING, ifs, &mut eps);
    }
    let mut expanded = eps.clone();
    expand_unspecified_address(ifs, routes, &mut expanded);
    expand_devices(ifs, &mut expanded);
    let mut bind = eps;
    expand_devices(ifs, &mut bind);
    for e in bind.iter_mut().filter(|e| e.addr.is_unspecified()) {
        // a wildcard belongs to no device (an interface whose netmask "holds" 0.0.0.0 does not
        // make it one)
        e.device.clear();
        e.netmask = build_netmask(0, e.addr.is_ipv4());
    }
    remove_duplicates(&mut bind);
    ListenPlan { bind, expanded }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACCEPT: ListenFlags = ListenFlags::ACCEPT_INCOMING;
    const EXPANDED: ListenFlags = ListenFlags::WAS_EXPANDED;
    const LOCAL: ListenFlags = ListenFlags::LOCAL_NETWORK;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    /// Interface indices for libtorrent's device names (routes name devices by index here).
    fn index_of(device: &str) -> u32 {
        match device {
            "lo" => 1,
            "eth0" => 2,
            "eth1" => 3,
            "enp3s0" => 4,
            "eth99" => 99,
            _ => 50,
        }
    }

    fn ifc(addr: &str, device: &str, netmask: Option<&str>) -> IpInterface {
        let a = ip(addr);
        IpInterface {
            addr: a,
            netmask: netmask.map_or_else(|| build_netmask(0, a.is_ipv4()), ip),
            scope_id: 0,
            index: index_of(device),
            name: device.into(),
            up: true,
            loopback: false,
            multicast: true,
            pointopoint: false,
            preferred: true,
        }
    }

    fn rt(dst: &str, device: &str, gateway: &str) -> IpRoute {
        let d = ip(dst);
        IpRoute {
            destination: d,
            netmask: build_netmask(0, d.is_ipv4()),
            gateway: ip(gateway),
            source_hint: None,
            index: index_of(device),
        }
    }

    fn ep(addr: &str, port: u16, device: &str, ssl: bool, flags: ListenFlags) -> ListenEndpoint {
        ListenEndpoint::new(ip(addr), port, device, ssl, flags)
    }

    fn sock(addr: &str, original_port: u16, device: &str) -> ListenEndpoint {
        ep(addr, original_port, device, false, ACCEPT)
    }

    fn li(device: &str, port: u16, ssl: bool, local: bool) -> ListenInterface {
        ListenInterface { device: device.into(), port, ssl, local }
    }

    fn check_parse(input: &str, expected: &[ListenInterface], expected_err: &[&str], output: &str) {
        let mut err = Vec::new();
        let list = parse_listen_interfaces(input, &mut err);
        assert_eq!(list, expected, "{input:?}");
        assert_eq!(err, expected_err, "{input:?}");
        assert_eq!(print_listen_interfaces(&list), output, "{input:?}");
    }

    #[test]
    fn parse_interface() {
        check_parse(
            "  a:4,b:35, c : 1000s, d: 351 ,e \t:42,foobar:1337s\n\r,[2001::1]:6881",
            &[
                li("a", 4, false, false),
                li("b", 35, false, false),
                li("c", 1000, true, false),
                li("d", 351, false, false),
                li("e", 42, false, false),
                li("foobar", 1337, true, false),
                li("2001::1", 6881, false, false),
            ],
            &[],
            "a:4,b:35,c:1000s,d:351,e:42,foobar:1337s,[2001::1]:6881",
        );
        // IPv6 address
        check_parse(
            "[2001:ffff::1]:6882s",
            &[li("2001:ffff::1", 6882, true, false)],
            &[],
            "[2001:ffff::1]:6882s",
        );
        // IPv4 address
        check_parse("127.0.0.1:6882", &[li("127.0.0.1", 6882, false, false)], &[], "127.0.0.1:6882");
        // maximum padding
        check_parse("  nic\r\n:\t 12\r s ", &[li("nic", 12, true, false)], &[], "nic:12s");
        // negative tests
        check_parse("nic:99999999999999999999999", &[], &["nic:99999999999999999999999"], "");
        check_parse("nic:  -3", &[], &["nic:  -3"], "");
        check_parse("nic:  ", &[], &["nic:"], "");
        check_parse("nic :", &[], &["nic :"], "");
        check_parse("nic ", &[], &["nic"], "");
        check_parse("nic s", &[], &["nic s"], "");
        // a port above 65535 has at most five digits but is still out of range
        check_parse("nic:65536", &[], &["nic:65536"], "");
        // port 0
        check_parse("127.0.0.1:0", &[li("127.0.0.1", 0, false, false)], &[], "127.0.0.1:0");
        // SSL flag
        check_parse("127.0.0.1:1234s", &[li("127.0.0.1", 1234, true, false)], &[], "127.0.0.1:1234s");
        // local flag
        check_parse("127.0.0.1:1234l", &[li("127.0.0.1", 1234, false, true)], &[], "127.0.0.1:1234l");
        // both
        check_parse("127.0.0.1:1234ls", &[li("127.0.0.1", 1234, true, true)], &[], "127.0.0.1:1234sl");
        // IPv6 errors
        check_parse("[aaaa::1", &[], &["[aaaa::1"], "");
        check_parse("[aaaa::1]", &[], &["[aaaa::1]"], "");
        check_parse("[aaaa::1]:", &[], &["[aaaa::1]:"], "");
        check_parse("[aaaa::1]:s", &[], &["[aaaa::1]:s"], "");
        check_parse("[aaaa::1] :6881", &[], &["[aaaa::1] :6881"], "");
        check_parse("[aaaa::1]:6881", &[li("aaaa::1", 6881, false, false)], &[], "[aaaa::1]:6881");
        // unterminated [
        check_parse("[aaaa::1,foobar:0", &[li("foobar", 0, false, false)], &["[aaaa::1"], "foobar:0");
        // multiple errors
        check_parse("foo:,bar", &[], &["foo:", "bar"], "");
        // quoted elements
        check_parse("\"abc,.\",bar", &[], &["abc,.", "bar"], "");
        // silent error
        check_parse("\"", &[], &["\""], "");
        // multiple errors and one correct
        check_parse(
            "foo,bar,0.0.0.0:6881",
            &[li("0.0.0.0", 6881, false, false)],
            &["foo", "bar"],
            "0.0.0.0:6881",
        );
        // the default setting
        check_parse(
            "0.0.0.0:6881,[::]:6881",
            &[li("0.0.0.0", 6881, false, false), li("::", 6881, false, false)],
            &[],
            "0.0.0.0:6881,[::]:6881",
        );
    }

    #[test]
    fn split_string_quotes_cases() {
        assert_eq!(split_string_quotes("a b", ' '), ("a", "b"));
        assert_eq!(split_string_quotes("\"a b\" c", ' '), ("\"a b\"", "c"));
        assert_eq!(split_string_quotes("\"a b\"foobar c", ' '), ("\"a b\"foobar", "c"));
        assert_eq!(split_string_quotes("a\nb foobar", ' '), ("a\nb", "foobar"));
        assert_eq!(split_string_quotes("a b\"foo\"bar", '"'), ("a b", "foo\"bar"));
        assert_eq!(split_string_quotes("a", ' '), ("a", ""));
        assert_eq!(split_string_quotes("\"a b", ' '), ("\"a b", ""));
        assert_eq!(split_string_quotes("", ' '), ("", ""));
    }

    #[test]
    fn partition_listen_sockets_wildcard2specific() {
        let mut sockets = vec![sock("0.0.0.0", 6881, ""), sock("4.4.4.4", 6881, "")];
        // remove the wildcard socket and replace it with a specific IP
        let mut eps = vec![sock("4.4.4.4", 6881, ""), sock("4.4.4.5", 6881, "")];
        let keep = partition_listen_sockets(&mut eps, &mut sockets, |s| s);
        assert_eq!(eps.len(), 1);
        assert_eq!(keep, 1);
        assert_eq!(sockets.len() - keep, 1);
        assert_eq!(sockets[0].addr, ip("4.4.4.4"));
        assert_eq!(sockets[1].addr, ip("0.0.0.0"));
        assert_eq!(eps[0], sock("4.4.4.5", 6881, ""));
    }

    #[test]
    fn partition_listen_sockets_port_change() {
        let mut sockets = vec![sock("4.4.4.4", 6881, ""), sock("4.4.4.5", 6881, "")];
        // change the ports
        let mut eps = vec![sock("4.4.4.4", 6882, ""), sock("4.4.4.5", 6882, "")];
        assert_eq!(partition_listen_sockets(&mut eps, &mut sockets, |s| s), 0);
        assert_eq!(eps.len(), 2);
    }

    #[test]
    fn partition_listen_sockets_device_bound() {
        let mut sockets = vec![sock("4.4.4.5", 6881, ""), sock("0.0.0.0", 6881, "")];
        // replace the wildcard socket with a pair of device bound sockets
        let mut eps =
            vec![sock("4.4.4.5", 6881, ""), sock("4.4.4.6", 6881, "eth1"), sock("4.4.4.7", 6881, "eth1")];
        let keep = partition_listen_sockets(&mut eps, &mut sockets, |s| s);
        assert_eq!(keep, 1);
        assert_eq!(sockets.len() - keep, 1);
        assert_eq!(sockets[0].addr, ip("4.4.4.5"));
        assert_eq!(sockets[1].addr, ip("0.0.0.0"));
        assert_eq!(eps.len(), 2);
    }

    #[test]
    fn partition_listen_sockets_device_ip_change() {
        let mut sockets = vec![sock("10.10.10.10", 6881, "enp3s0"), sock("4.4.4.4", 6881, "enp3s0")];
        // change the IP of a device bound socket
        let mut eps = vec![sock("10.10.10.10", 6881, "enp3s0"), sock("4.4.4.5", 6881, "enp3s0")];
        let keep = partition_listen_sockets(&mut eps, &mut sockets, |s| s);
        assert_eq!(keep, 1);
        assert_eq!(sockets.len() - keep, 1);
        assert_eq!(sockets[0], sock("10.10.10.10", 6881, "enp3s0"));
        assert_eq!(sockets[1], sock("4.4.4.4", 6881, "enp3s0"));
        assert_eq!(eps, [sock("4.4.4.5", 6881, "enp3s0")]);
    }

    #[test]
    fn partition_listen_sockets_original_port() {
        // the sockets remember the configured port (6881) although they bound 6883: all are kept
        let mut sockets = vec![sock("10.10.10.10", 6881, ""), sock("4.4.4.4", 6881, "")];
        let mut eps = vec![sock("10.10.10.10", 6881, ""), sock("4.4.4.4", 6881, "")];
        assert_eq!(partition_listen_sockets(&mut eps, &mut sockets, |s| s), 2);
        assert!(eps.is_empty());
    }

    #[test]
    fn partition_listen_sockets_ssl() {
        let mut sockets = vec![sock("10.10.10.10", 6881, ""), sock("4.4.4.4", 6881, "")];
        // add ssl sockets
        let mut eps = vec![
            sock("10.10.10.10", 6881, ""),
            sock("4.4.4.4", 6881, ""),
            ep("10.10.10.10", 6881, "", true, ACCEPT),
            ep("4.4.4.4", 6881, "", true, ACCEPT),
        ];
        assert_eq!(partition_listen_sockets(&mut eps, &mut sockets, |s| s), 2);
        assert_eq!(eps.len(), 2);
    }

    #[test]
    fn partition_listen_sockets_op_ports() {
        let mut sockets = vec![sock("10.10.10.10", 0, ""), sock("4.4.4.4", 6881, "")];
        // replace OS assigned ports with explicit ports
        let mut eps = vec![sock("10.10.10.10", 6882, ""), sock("4.4.4.4", 6882, "")];
        assert_eq!(partition_listen_sockets(&mut eps, &mut sockets, |s| s), 0);
        assert_eq!(eps.len(), 2);
    }

    #[test]
    fn expand_devices_cases() {
        let ifs = [
            ifc("127.0.0.1", "lo", Some("255.0.0.0")),
            ifc("192.168.1.2", "eth0", Some("255.255.255.0")),
            ifc("24.172.48.90", "eth1", Some("255.255.255.0")),
            ifc("::1", "lo", Some("ffff:ffff:ffff:ffff::")),
            ifc("fe80::d250:99ff:fe0c:9b74", "eth0", Some("ffff:ffff:ffff:ffff::")),
            ifc("2601:646:c600:a3:d250:99ff:fe0c:9b74", "eth0", Some("ffff:ffff:ffff:ffff::")),
        ];
        let mut eps = vec![
            ep("127.0.0.1", 6881, "", false, ListenFlags::empty()),
            ep("192.168.1.2", 6881, "", false, ListenFlags::empty()),
        ];
        expand_devices(&ifs, &mut eps);
        assert_eq!(
            eps,
            [
                ep("127.0.0.1", 6881, "lo", false, ListenFlags::empty()),
                ep("192.168.1.2", 6881, "eth0", false, ListenFlags::empty()),
            ]
        );
        assert_eq!(eps[0].netmask, ip("255.0.0.0"));
        assert_eq!(eps[1].netmask, ip("255.255.255.0"));
        // an address no interface holds can reach nothing
        let mut eps = vec![ep("10.9.9.9", 6881, "", false, ListenFlags::empty())];
        expand_devices(&ifs, &mut eps);
        assert_eq!(eps[0].device, "");
        assert_eq!(eps[0].netmask, ip("0.0.0.0"));
    }

    #[test]
    fn expand_unspecified() {
        // this causes us to only expand IPv6 addresses on eth0
        let routes = [rt("0.0.0.0", "eth0", "1.2.3.4"), rt("::", "eth0", "1234:5678::1")];
        let ifs = [
            ifc("127.0.0.1", "lo", None),
            ifc("192.168.1.2", "eth0", None),
            ifc("24.172.48.90", "eth1", None),
            ifc("::1", "lo", None),
            ifc("fe80::d250:99ff:fe0c:9b74", "eth0", None),
            ifc("2601:646:c600:a3:d250:99ff:fe0c:9b74", "eth0", None),
        ];
        let global = ACCEPT | EXPANDED;
        let local = ACCEPT | EXPANDED | LOCAL;
        let v4_nossl = ep("0.0.0.0", 6881, "", false, ACCEPT);
        let v4_ssl = ep("0.0.0.0", 6882, "", true, ACCEPT);
        let v4_loopb_nossl = ep("127.0.0.1", 6881, "", false, local);
        let v4_loopb_ssl = ep("127.0.0.1", 6882, "", true, local);
        let v4_g1_nossl = ep("192.168.1.2", 6881, "", false, global);
        let v4_g1_ssl = ep("192.168.1.2", 6882, "", true, global);
        let v4_g2_nossl = ep("24.172.48.90", 6881, "", false, global);
        let v4_g2_ssl = ep("24.172.48.90", 6882, "", true, global);
        let v6_unsp_nossl = ep("::", 6883, "", false, global);
        let v6_unsp_ssl = ep("::", 6884, "", true, global);
        let v6_ll_nossl = ep("fe80::d250:99ff:fe0c:9b74", 6883, "", false, local);
        let v6_ll_ssl = ep("fe80::d250:99ff:fe0c:9b74", 6884, "", true, local);
        let v6_g_nossl = ep("2601:646:c600:a3:d250:99ff:fe0c:9b74", 6883, "", false, global);
        let v6_g_ssl = ep("2601:646:c600:a3:d250:99ff:fe0c:9b74", 6884, "", true, global);
        let v6_loopb_ssl = ep("::1", 6884, "", true, local);
        let v6_loopb_nossl = ep("::1", 6883, "", false, local);

        let mut eps = vec![v4_nossl.clone(), v4_ssl.clone(), v6_unsp_nossl.clone(), v6_unsp_ssl.clone()];
        expand_unspecified_address(&ifs, &routes, &mut eps);
        let count = |e: &ListenEndpoint, eps: &[ListenEndpoint]| eps.iter().filter(|x| *x == e).count();
        assert_eq!(eps.len(), 12);
        for e in [
            &v4_g1_nossl,
            &v4_g1_ssl,
            &v4_g2_nossl,
            &v4_g2_ssl,
            &v6_ll_nossl,
            &v6_ll_ssl,
            &v6_g_nossl,
            &v6_g_ssl,
            &v6_loopb_ssl,
            &v6_loopb_nossl,
            &v4_loopb_ssl,
            &v4_loopb_nossl,
        ] {
            assert_eq!(count(e, &eps), 1, "{e} in {eps:?}");
        }
        for e in [&v6_unsp_nossl, &v6_unsp_ssl, &v4_nossl, &v4_ssl] {
            assert_eq!(count(e, &eps), 0, "{e}");
        }

        // a user configured endpoint is not duplicated
        let v6_g_nossl_dev = ep("2601:646:c600:a3:d250:99ff:fe0c:9b74", 6883, "eth0", false, ACCEPT);
        let mut eps = vec![v6_unsp_nossl.clone(), v6_g_nossl_dev.clone()];
        expand_unspecified_address(&ifs, &routes, &mut eps);
        assert_eq!(eps.len(), 3);
        assert_eq!(count(&v6_ll_nossl, &eps), 1);
        assert_eq!(count(&v6_g_nossl, &eps), 0);
        assert_eq!(count(&v6_loopb_nossl, &eps), 1);
        assert_eq!(count(&v6_g_nossl_dev, &eps), 1);
    }

    #[test]
    fn expand_unspecified_no_default() {
        // even though this route isn't a default route, it's a route for a global address
        let routes = [rt("128.0.0.0", "eth0", "128.0.0.0")];
        let ifs = [ifc("192.168.1.2", "eth0", Some("255.255.0.0"))];
        let mut eps = vec![ep("0.0.0.0", 6881, "", false, ACCEPT)];
        expand_unspecified_address(&ifs, &routes, &mut eps);
        assert_eq!(eps, [ep("192.168.1.2", 6881, "", false, ACCEPT | EXPANDED)]);
    }

    fn expand_with_flags(up: bool, loopback: bool, pointopoint: bool) -> Vec<ListenEndpoint> {
        let routes = [rt("0.0.0.0", "eth99", "0.0.0.0")];
        let ifs = [IpInterface { up, loopback, pointopoint, ..ifc("192.168.1.2", "eth0", None) }];
        let mut eps = vec![ep("0.0.0.0", 6881, "", false, ACCEPT)];
        expand_unspecified_address(&ifs, &routes, &mut eps);
        eps
    }

    fn expand_address(addr: &str) -> Vec<ListenEndpoint> {
        let ifs = [ifc(addr, "eth0", Some("255.255.0.0"))];
        let mut eps = vec![ep("0.0.0.0", 6881, "", false, ACCEPT)];
        expand_unspecified_address(&ifs, &[], &mut eps);
        eps
    }

    #[test]
    fn expand_unspecified_ppp() {
        let global = ACCEPT | EXPANDED;
        let local = global | LOCAL;
        assert_eq!(expand_with_flags(true, false, true), [ep("192.168.1.2", 6881, "", false, global)]);
        assert_eq!(expand_with_flags(true, false, false), [ep("192.168.1.2", 6881, "", false, local)]);
    }

    #[test]
    fn expand_unspecified_down_if() {
        let local = ACCEPT | EXPANDED | LOCAL;
        assert_eq!(expand_with_flags(false, false, false), []);
        assert_eq!(expand_with_flags(true, false, false), [ep("192.168.1.2", 6881, "", false, local)]);
    }

    #[test]
    fn expand_unspecified_if_loopback() {
        let local = ACCEPT | EXPANDED | LOCAL;
        assert_eq!(expand_with_flags(true, true, false), [ep("192.168.1.2", 6881, "", false, local)]);
    }

    #[test]
    fn expand_unspecified_addresses() {
        let global = ACCEPT | EXPANDED;
        let local = global | LOCAL;
        assert_eq!(expand_address("1.2.3.4"), [ep("1.2.3.4", 6881, "", false, global)]);
        assert_eq!(expand_address("169.254.1.2"), [ep("169.254.1.2", 6881, "", false, local)]);
        assert_eq!(expand_address("127.1.1.1"), [ep("127.1.1.1", 6881, "", false, local)]);
    }

    #[test]
    fn expand_unspecified_skips_tentative_addresses() {
        let ifs = [
            IpInterface { preferred: false, ..ifc("2001:db8::1", "eth0", None) },
            ifc("2001:db8::2", "eth0", None),
        ];
        let mut eps = vec![ep("::", 6881, "", false, ACCEPT)];
        expand_unspecified_address(&ifs, &[], &mut eps);
        assert_eq!(eps, [ep("2001:db8::2", 6881, "", false, ACCEPT | EXPANDED)]);
    }

    fn to_endpoint(iface: &ListenInterface, ifs: &[IpInterface]) -> Vec<ListenEndpoint> {
        let mut ret = Vec::new();
        interface_to_endpoints(iface, ACCEPT, ifs, &mut ret);
        ret
    }

    #[test]
    fn interface_to_endpoint() {
        assert_eq!(
            to_endpoint(&li("10.0.1.1", 6881, false, false), &[]),
            [ep("10.0.1.1", 6881, "", false, ACCEPT)]
        );
        let ifs = [
            // a global IPv4 address, not a private network
            ifc("185.0.1.2", "eth0", None),
            ifc("192.168.2.2", "eth1", None),
            ifc("fe80::d250:99ff:fe0c:9b74", "eth0", None),
            // a global IPv6 address, not a private network
            ifc("2601:646:c600:a3:d250:99ff:fe0c:9b74", "eth1", None),
        ];
        assert_eq!(
            to_endpoint(&li("eth0", 1234, false, false), &ifs),
            [
                ep("185.0.1.2", 1234, "eth0", false, EXPANDED | ACCEPT),
                ep("fe80::d250:99ff:fe0c:9b74", 1234, "eth0", false, EXPANDED | ACCEPT | LOCAL),
            ]
        );
        assert_eq!(
            to_endpoint(&li("eth1", 1234, false, false), &ifs),
            [
                ep("192.168.2.2", 1234, "eth1", false, EXPANDED | ACCEPT),
                ep("2601:646:c600:a3:d250:99ff:fe0c:9b74", 1234, "eth1", false, EXPANDED | ACCEPT),
            ]
        );
        let ifs2 = [ifc("10.0.1.1", "eth0", None)];
        assert_eq!(
            to_endpoint(&li("eth0", 1234, false, false), &ifs2),
            [ep("10.0.1.1", 1234, "eth0", false, EXPANDED | ACCEPT)]
        );
        // the `l` flag marks every endpoint local
        assert_eq!(
            to_endpoint(&li("10.0.1.1", 1, false, true), &[]),
            [ep("10.0.1.1", 1, "", false, ACCEPT | LOCAL)]
        );
        // a device that is not there has no endpoints
        assert_eq!(to_endpoint(&li("tun0", 1, false, false), &ifs), []);
    }

    #[test]
    fn duplicates_are_removed() {
        let mut eps = vec![
            ep("0.0.0.0", 6881, "", false, ACCEPT),
            ep("192.168.1.2", 6881, "eth0", false, ACCEPT | EXPANDED),
            ep("2001:db8::1", 6881, "eth0", false, ACCEPT | EXPANDED),
            ep("2001:db8::1", 6882, "", false, ACCEPT),
            ep("0.0.0.0", 6881, "", true, ACCEPT),
        ];
        remove_duplicates(&mut eps);
        assert_eq!(
            eps,
            [
                ep("0.0.0.0", 6881, "", false, ACCEPT),
                ep("2001:db8::1", 6881, "eth0", false, ACCEPT | EXPANDED),
                ep("0.0.0.0", 6881, "", true, ACCEPT),
            ]
        );
    }

    #[test]
    fn plan_keeps_wildcards_and_expands_devices() {
        let routes = [rt("0.0.0.0", "eth0", "192.168.1.1")];
        let ifs = [
            ifc("127.0.0.1", "lo", Some("255.0.0.0")),
            ifc("192.168.1.2", "eth0", Some("255.255.255.0")),
            ifc("10.8.0.2", "tun0", Some("255.255.255.0")),
            ifc("fe80::1", "tun0", Some("ffff:ffff:ffff:ffff::")),
        ];
        let mut err = Vec::new();
        let list = parse_listen_interfaces("0.0.0.0:6881,tun0:6881,127.0.0.1:6881,[::1]:6881s", &mut err);
        let plan = listen_plan(&list, &ifs, &routes);
        // the v4 wildcard covers tun0's and loopback's IPv4 addresses; the SSL entry is skipped
        assert_eq!(
            plan.bind,
            [
                ep("0.0.0.0", 6881, "", false, ACCEPT),
                ep("fe80::1", 6881, "tun0", false, ACCEPT | EXPANDED | LOCAL),
            ]
        );
        assert_eq!(plan.bind[1].netmask, ip("ffff:ffff:ffff:ffff::"));
        // libtorrent's list: the configured addresses, then the wildcard's expansion
        assert_eq!(
            plan.expanded,
            [
                ep("10.8.0.2", 6881, "tun0", false, ACCEPT | EXPANDED),
                ep("fe80::1", 6881, "tun0", false, ACCEPT | EXPANDED | LOCAL),
                ep("127.0.0.1", 6881, "lo", false, ACCEPT),
                ep("192.168.1.2", 6881, "eth0", false, ACCEPT | EXPANDED),
            ]
        );
        // a device-only setup binds nothing while the device is gone
        let list = parse_listen_interfaces("tun0:6881", &mut err);
        assert_eq!(listen_plan(&list, &ifs[..2], &routes), ListenPlan::default());
        assert!(err.is_empty());
    }
}
