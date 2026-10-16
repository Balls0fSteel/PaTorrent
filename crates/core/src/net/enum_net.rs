// Ported from libtorrent-rasterbar 2.0.11: src/enum_net.cpp (ip_interface, match_addr_mask,
// build_netmask), test/test_enum_net.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the platform enumeration lives in `net::sys` (GetAdaptersAddresses, getifaddrs; no
// netlink). An interface entry keeps its IPv6 scope id beside the address, since `IpAddr` has
// none, and the interface index as well as its name.

//! Local interface addresses and subnet matching.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// One address of a local network interface (libtorrent `ip_interface`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IpInterface {
    pub addr: IpAddr,
    pub netmask: IpAddr,
    /// IPv6 scope id of the address (the interface index for link-local addresses, else 0).
    pub scope_id: u32,
    /// The interface index, for multicast membership and the outgoing interface.
    pub index: u32,
    pub name: String,
    pub up: bool,
    pub loopback: bool,
    pub multicast: bool,
    /// Not deprecated, tentative or a failed duplicate (always true where the OS doesn't say).
    pub preferred: bool,
}

/// One entry of the routing table (libtorrent `ip_route`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IpRoute {
    pub destination: IpAddr,
    pub netmask: IpAddr,
    /// Unspecified for routes to directly connected networks.
    pub gateway: IpAddr,
    /// The preferred source address, where the OS says (libtorrent `source_hint`).
    pub source_hint: Option<IpAddr>,
    /// The interface the route leaves through, as `IpInterface::index` numbers it.
    pub index: u32,
}

/// libtorrent `get_gateway`: the default route's gateway for an interface address, if the
/// interface has one. Local IPv6 addresses never reach the internet.
pub(crate) fn get_gateway(iface: &IpInterface, routes: &[IpRoute]) -> Option<IpAddr> {
    if iface.addr.is_ipv6() && crate::net::ip_voter::is_local(&iface.addr) {
        return None;
    }
    routes
        .iter()
        .find(|r| {
            r.destination.is_unspecified()
                && r.destination.is_ipv4() == iface.addr.is_ipv4()
                && !r.gateway.is_unspecified()
                // in case there are multiple networks on the same networking device, the
                // source hint may be the only thing telling them apart
                && r.source_hint.is_none_or(|s| s == iface.addr)
                && r.index == iface.index
        })
        .map(|r| r.gateway)
}

/// libtorrent `build_netmask`: a mask with the top `bits` set.
pub(crate) fn build_netmask(bits: u32, v4: bool) -> IpAddr {
    if v4 {
        let m = if bits == 0 { 0 } else { u32::MAX << (32 - bits.min(32)) };
        IpAddr::V4(Ipv4Addr::from(m))
    } else {
        let m = if bits == 0 { 0 } else { u128::MAX << (128 - bits.min(128)) };
        IpAddr::V6(Ipv6Addr::from(m))
    }
}

/// libtorrent `match_addr_mask`: whether `a1` and `a2` are in the same subnet under `mask`. All
/// three must be of one family; IPv6 scope ids are compared by the caller.
pub(crate) fn match_addr_mask(a1: IpAddr, a2: IpAddr, mask: IpAddr) -> bool {
    match (a1, a2, mask) {
        (IpAddr::V4(a), IpAddr::V4(b), IpAddr::V4(m)) => {
            u32::from(a) & u32::from(m) == u32::from(b) & u32::from(m)
        }
        (IpAddr::V6(a), IpAddr::V6(b), IpAddr::V6(m)) => {
            u128::from(a) & u128::from(m) == u128::from(b) & u128::from(m)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn match_addr_mask_cases() {
        assert!(match_addr_mask(ip("10.0.1.176"), ip("10.0.1.176"), ip("255.255.255.0")));
        assert!(match_addr_mask(ip("10.0.1.3"), ip("10.0.3.3"), ip("255.255.0.0")));
        assert!(!match_addr_mask(ip("10.0.1.3"), ip("10.1.3.3"), ip("255.255.0.0")));
        assert!(match_addr_mask(ip("ff00:1234::"), ip("ff00:5678::"), ip("ffff::")));
        assert!(!match_addr_mask(ip("ff00:1234::"), ip("ff00:5678::"), ip("ffff:f000::")));
        // mixed families never match
        assert!(!match_addr_mask(ip("10.0.1.3"), ip("::1"), ip("255.255.0.0")));
        assert!(!match_addr_mask(ip("10.0.1.3"), ip("10.0.1.3"), ip("ffff::")));
    }

    #[test]
    fn gateway_of_an_interface() {
        let route = |dst: &str, gw: &str, index: u32, hint: Option<&str>| IpRoute {
            destination: ip(dst),
            netmask: ip(if dst.contains(':') { "::" } else { "0.0.0.0" }),
            gateway: ip(gw),
            source_hint: hint.map(ip),
            index,
        };
        let routes = [
            route("192.168.1.0", "0.0.0.0", 7, None),
            route("0.0.0.0", "192.168.1.1", 7, None),
            route("0.0.0.0", "10.0.0.1", 9, Some("10.0.0.2")),
            route("::", "fe80::1", 7, None),
        ];
        let iface = |addr: &str, index: u32| IpInterface {
            addr: ip(addr),
            netmask: ip("255.255.255.0"),
            scope_id: 0,
            index,
            name: String::new(),
            up: true,
            loopback: false,
            multicast: true,
            preferred: true,
        };
        assert_eq!(get_gateway(&iface("192.168.1.5", 7), &routes), Some(ip("192.168.1.1")));
        assert_eq!(get_gateway(&iface("192.168.1.5", 8), &routes), None, "another interface");
        assert_eq!(get_gateway(&iface("10.0.0.2", 9), &routes), Some(ip("10.0.0.1")));
        assert_eq!(
            get_gateway(&iface("10.0.0.3", 9), &routes),
            None,
            "the source hint names another address"
        );
        assert_eq!(get_gateway(&iface("2001:db8::5", 7), &routes), Some(ip("fe80::1")));
        assert_eq!(get_gateway(&iface("fe80::5", 7), &routes), None, "local IPv6 addresses never route out");
    }

    #[test]
    fn build_netmask_v4() {
        let cases = [
            (0, "0.0.0.0"),
            (1, "128.0.0.0"),
            (2, "192.0.0.0"),
            (3, "224.0.0.0"),
            (4, "240.0.0.0"),
            (5, "248.0.0.0"),
            (6, "252.0.0.0"),
            (7, "254.0.0.0"),
            (8, "255.0.0.0"),
            (9, "255.128.0.0"),
            (10, "255.192.0.0"),
            (11, "255.224.0.0"),
            (22, "255.255.252.0"),
            (23, "255.255.254.0"),
            (24, "255.255.255.0"),
            (25, "255.255.255.128"),
            (26, "255.255.255.192"),
            (27, "255.255.255.224"),
            (28, "255.255.255.240"),
            (29, "255.255.255.248"),
            (30, "255.255.255.252"),
            (31, "255.255.255.254"),
            (32, "255.255.255.255"),
        ];
        for (bits, mask) in cases {
            assert_eq!(build_netmask(bits, true), ip(mask), "{bits}");
        }
    }

    #[test]
    fn build_netmask_v6() {
        let cases = [
            (0, "::"),
            (1, "8000::"),
            (2, "c000::"),
            (3, "e000::"),
            (4, "f000::"),
            (5, "f800::"),
            (6, "fc00::"),
            (7, "fe00::"),
            (8, "ff00::"),
            (9, "ff80::"),
            (10, "ffc0::"),
            (11, "ffe0::"),
            (119, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:fe00"),
            (120, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ff00"),
            (121, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ff80"),
            (122, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffc0"),
            (123, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffe0"),
            (124, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:fff0"),
            (125, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:fff8"),
            (126, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffc"),
            (127, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:fffe"),
            (128, "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff"),
        ];
        for (bits, mask) in cases {
            assert_eq!(build_netmask(bits, false), ip(mask), "{bits}");
        }
    }
}
