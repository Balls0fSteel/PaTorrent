// Ported from libtorrent-rasterbar 2.0.11: src/natpmp.cpp (send_get_ip_address_request,
// send_map_request, on_reply's packet checks, from_result_code, pcp_error_category),
// include/libtorrent/natpmp.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the packets are built and read here, sans-IO; the mapping state machine that uses
// them lives in `engine::natpmp`.

//! NAT-PMP (RFC 6886) and PCP (RFC 6887) packets, as the gateway on the default route
//! exchanges them on UDP port 5351.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use crate::alert::Protocol;
use crate::net::ip_voter::is_local;

/// The port NAT-PMP and PCP servers listen on.
pub const PORT: u16 = 5351;
pub const VERSION_NATPMP: u8 = 0;
pub const VERSION_PCP: u8 = 2;
/// PCP opcodes (RFC 6887 section 19.2).
pub const OPCODE_MAP: u8 = 1;
/// A mapping lives an hour unless `natpmp_lease_duration` says otherwise; 0 deletes it.
pub const MAP_TTL: u32 = 3600;
/// 1100 octets is the largest PCP packet.
pub const MAX_PACKET: usize = 1100;

/// RFC 6887 section 7.4 result codes (libtorrent `errors::pcp_errors`); NAT-PMP results are
/// mapped onto them by [`result_message`].
pub const RESULT_SUCCESS: u16 = 0;
pub const RESULT_UNSUPP_VERSION: u16 = 1;

const RESULT_MESSAGES: [&str; 14] = [
    "success",
    "unsupported version",
    "not authorized",
    "malformed request",
    "unsupported opcode",
    "unsupported option",
    "malformed option",
    "network failure",
    "no resources",
    "unsupported protocol",
    "user exceeded quota",
    "cannot provide external",
    "address mismatch",
    "excessive remote peers",
];

/// libtorrent `natpmp::from_result_code` plus the message table: a few NAT-PMP result codes
/// mean something else in PCP.
pub fn result_message(version: u8, mut result: u16) -> &'static str {
    if version == VERSION_NATPMP {
        result = match result {
            3 => 7,
            4 => 8,
            5 => 4,
            r => r,
        };
    }
    RESULT_MESSAGES.get(result as usize).copied().unwrap_or("Unknown error")
}

pub fn version_name(version: u8) -> &'static str {
    if version == VERSION_NATPMP { "NAT-PMP" } else { "PCP" }
}

/// The NAT-PMP "public address" request (opcode 0). PCP routers report the external address in
/// their MAP responses instead.
pub fn external_address_request() -> [u8; 2] {
    [VERSION_NATPMP, 0]
}

/// A NAT-PMP map request (RFC 6886 section 3.3); `ttl` 0 deletes the mapping.
pub fn map_request_natpmp(protocol: Protocol, local_port: u16, external_port: u16, ttl: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.push(VERSION_NATPMP);
    out.push(if protocol == Protocol::Udp { 1 } else { 2 });
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    out.extend_from_slice(&local_port.to_be_bytes());
    out.extend_from_slice(&external_port.to_be_bytes());
    out.extend_from_slice(&ttl.to_be_bytes());
    out
}

/// IPv4 addresses travel v4-mapped in PCP.
fn v6(addr: IpAddr) -> Ipv6Addr {
    match addr {
        IpAddr::V4(a) => a.to_ipv6_mapped(),
        IpAddr::V6(a) => a,
    }
}

/// A PCP MAP request (RFC 6887 section 11.1). `local_addr` is the address the socket sends
/// from; `external_addr` is the suggested external address (`None` lets the server choose,
/// which libtorrent asks for when the local address is private).
pub fn map_request_pcp(
    local_addr: IpAddr,
    nonce: &[u8; 12],
    protocol: Protocol,
    local_port: u16,
    external_port: u16,
    external_addr: Option<IpAddr>,
    ttl: u32,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(60);
    out.push(VERSION_PCP);
    out.push(OPCODE_MAP);
    out.extend_from_slice(&0u16.to_be_bytes()); // reserved
    out.extend_from_slice(&ttl.to_be_bytes());
    out.extend_from_slice(&v6(local_addr).octets());
    out.extend_from_slice(nonce);
    // IANA protocol numbers
    out.push(if protocol == Protocol::Tcp { 6 } else { 17 });
    out.extend_from_slice(&[0; 3]); // reserved
    out.extend_from_slice(&local_port.to_be_bytes());
    out.extend_from_slice(&external_port.to_be_bytes());
    let suggested = match external_addr {
        Some(a) => v6(a),
        // a local address can't be the external one: let the server pick
        None if is_local(&local_addr) || local_addr.is_unspecified() => {
            if local_addr.is_ipv4() {
                Ipv4Addr::UNSPECIFIED.to_ipv6_mapped()
            } else {
                Ipv6Addr::UNSPECIFIED
            }
        }
        None => v6(local_addr),
    };
    out.extend_from_slice(&suggested.octets());
    out
}

/// A reply from the gateway.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Response {
    /// The server does not speak this version (PCP → fall back to NAT-PMP).
    UnsupportedVersion { version: u8 },
    /// A NAT-PMP "public address" response.
    ExternalAddress(Ipv4Addr),
    Map {
        version: u8,
        protocol: Protocol,
        /// PCP only: matches the request's nonce.
        nonce: Option<[u8; 12]>,
        private_port: u16,
        public_port: u16,
        lifetime: u32,
        /// PCP only: the assigned external address.
        external_addr: Option<IpAddr>,
        result: u16,
    },
}

/// Why a datagram is ignored (each is a `log` line in libtorrent's `on_reply`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    Size,
    Version,
    Opcode,
}

/// Reads a reply the way libtorrent's `natpmp::on_reply` does, size checks included.
pub fn parse_response(buf: &[u8]) -> Result<Response, Invalid> {
    if buf.len() < 4 {
        return Err(Invalid::Size);
    }
    let version = buf[0];
    if version != VERSION_NATPMP && version != VERSION_PCP {
        return Err(Invalid::Version);
    }
    let pcp = version == VERSION_PCP;
    let cmd = if pcp { buf[1] & 0x7f } else { buf[1] };
    let result = if pcp { buf[3] as u16 } else { u16::from_be_bytes([buf[2], buf[3]]) };
    if result == RESULT_UNSUPP_VERSION {
        return Ok(Response::UnsupportedVersion { version });
    }
    if (!pcp && buf.len() < 12) || (pcp && buf.len() < 24) {
        return Err(Invalid::Size);
    }
    let mut in_ = 4;
    let mut lifetime = 0;
    if pcp {
        lifetime = u32::from_be_bytes(buf[4..8].try_into().expect("4 bytes"));
        in_ = 8;
    }
    // seconds since start of epoch: not used
    in_ += 4;
    if pcp {
        in_ += 12; // reserved
    }
    if !pcp && cmd == 128 {
        // public IP request response
        let a: [u8; 4] = buf[in_..in_ + 4].try_into().expect("4 bytes");
        return Ok(Response::ExternalAddress(Ipv4Addr::from(a)));
    }
    if (!pcp && buf.len() != 16) || (pcp && buf.len() != 60) {
        return Err(Invalid::Size);
    }
    let mut nonce = None;
    let mut protocol = Protocol::Tcp;
    if pcp {
        if cmd != OPCODE_MAP {
            return Err(Invalid::Opcode);
        }
        nonce = Some(buf[in_..in_ + 12].try_into().expect("12 bytes"));
        in_ += 12;
        protocol = if buf[in_] == 6 { Protocol::Tcp } else { Protocol::Udp };
        in_ += 4; // protocol and reserved
    }
    let private_port = u16::from_be_bytes([buf[in_], buf[in_ + 1]]);
    let public_port = u16::from_be_bytes([buf[in_ + 2], buf[in_ + 3]]);
    in_ += 4;
    let mut external_addr = None;
    if pcp {
        let a: [u8; 16] = buf[in_..in_ + 16].try_into().expect("16 bytes");
        let a = Ipv6Addr::from(a);
        external_addr = Some(a.to_ipv4_mapped().map_or(IpAddr::V6(a), IpAddr::V4));
    } else {
        lifetime = u32::from_be_bytes(buf[in_..in_ + 4].try_into().expect("4 bytes"));
        protocol = match cmd {
            129 => Protocol::Udp,
            130 => Protocol::Tcp,
            _ => return Err(Invalid::Opcode),
        };
    }
    Ok(Response::Map { version, protocol, nonce, private_port, public_port, lifetime, external_addr, result })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_laid_out_as_the_rfcs_say() {
        assert_eq!(external_address_request(), [0, 0]);
        let r = map_request_natpmp(Protocol::Tcp, 6881, 6881, 3600);
        assert_eq!(r, [0, 2, 0, 0, 0x1a, 0xe1, 0x1a, 0xe1, 0, 0, 0x0e, 0x10]);
        let r = map_request_natpmp(Protocol::Udp, 1, 2, 0);
        assert_eq!(r[1], 1);
        assert_eq!(&r[8..], [0, 0, 0, 0]);

        let nonce = [7u8; 12];
        let r =
            map_request_pcp("192.168.1.5".parse().unwrap(), &nonce, Protocol::Udp, 6881, 6882, None, 3600);
        assert_eq!(r.len(), 60);
        assert_eq!(&r[..8], [2, 1, 0, 0, 0, 0, 0x0e, 0x10]);
        let mapped = "::ffff:192.168.1.5".parse::<Ipv6Addr>().unwrap().octets();
        assert_eq!(&r[8..24], &mapped);
        assert_eq!(&r[24..36], &nonce);
        assert_eq!(&r[36..40], [17, 0, 0, 0]);
        assert_eq!(&r[40..44], [0x1a, 0xe1, 0x1a, 0xe2]);
        // a private local address asks for "any" external address, v4-mapped
        assert_eq!(&r[44..60], &Ipv4Addr::UNSPECIFIED.to_ipv6_mapped().octets());
        // a public one suggests itself; an explicit one is used as is
        let r = map_request_pcp("8.8.8.8".parse().unwrap(), &nonce, Protocol::Tcp, 1, 1, None, 1);
        assert_eq!(&r[44..60], &"::ffff:8.8.8.8".parse::<Ipv6Addr>().unwrap().octets());
        let ext: IpAddr = "1.2.3.4".parse().unwrap();
        let r = map_request_pcp("10.0.0.1".parse().unwrap(), &nonce, Protocol::Tcp, 1, 1, Some(ext), 1);
        assert_eq!(&r[44..60], &"::ffff:1.2.3.4".parse::<Ipv6Addr>().unwrap().octets());
        assert_eq!(r[36], 6);
    }

    #[test]
    fn natpmp_responses() {
        // public address
        let mut p = vec![0, 128, 0, 0, 0, 0, 0, 1];
        p.extend_from_slice(&[203, 0, 113, 9]);
        assert_eq!(parse_response(&p), Ok(Response::ExternalAddress("203.0.113.9".parse().unwrap())));
        // tcp mapping
        let mut p = vec![0, 130, 0, 0, 0, 0, 0, 1];
        p.extend_from_slice(&[0x1a, 0xe1, 0x1a, 0xe2, 0, 0, 0x0e, 0x10]);
        assert_eq!(
            parse_response(&p),
            Ok(Response::Map {
                version: 0,
                protocol: Protocol::Tcp,
                nonce: None,
                private_port: 6881,
                public_port: 6882,
                lifetime: 3600,
                external_addr: None,
                result: 0,
            })
        );
        p[1] = 129;
        assert!(matches!(parse_response(&p), Ok(Response::Map { protocol: Protocol::Udp, .. })));
        p[1] = 131;
        assert_eq!(parse_response(&p), Err(Invalid::Opcode));
        p[1] = 130;
        p[3] = 3; // network failure
        assert!(matches!(parse_response(&p), Ok(Response::Map { result: 3, .. })));
        assert_eq!(result_message(0, 3), "network failure");
        assert_eq!(result_message(0, 4), "no resources");
        assert_eq!(result_message(0, 5), "unsupported opcode");
        assert_eq!(result_message(2, 3), "malformed request");
        assert_eq!(result_message(2, 99), "Unknown error");
        // unsupported version comes before the size checks
        assert_eq!(parse_response(&[0, 0, 0, 1]), Ok(Response::UnsupportedVersion { version: 0 }));
        assert_eq!(parse_response(&[0, 130, 0, 0, 0, 0, 0, 1, 1, 2]), Err(Invalid::Size));
        assert_eq!(parse_response(&[0, 130, 0, 0, 0, 0, 0, 1, 1, 2, 3, 4, 5]), Err(Invalid::Size));
        assert_eq!(parse_response(&[1, 0, 0, 0]), Err(Invalid::Version));
        assert_eq!(parse_response(&[0, 0, 0]), Err(Invalid::Size));
    }

    #[test]
    fn pcp_responses() {
        let mut p = vec![2, 0x81, 0, 0]; // version, MAP | response bit, reserved, success
        p.extend_from_slice(&3600u32.to_be_bytes());
        p.extend_from_slice(&[0, 0, 0, 1]); // epoch
        p.extend_from_slice(&[0; 12]); // reserved
        p.extend_from_slice(&[9; 12]); // nonce
        p.extend_from_slice(&[17, 0, 0, 0]);
        p.extend_from_slice(&[0x1a, 0xe1, 0x1a, 0xe3]);
        p.extend_from_slice(&"::ffff:198.51.100.7".parse::<Ipv6Addr>().unwrap().octets());
        assert_eq!(p.len(), 60);
        assert_eq!(
            parse_response(&p),
            Ok(Response::Map {
                version: 2,
                protocol: Protocol::Udp,
                nonce: Some([9; 12]),
                private_port: 6881,
                public_port: 6883,
                lifetime: 3600,
                external_addr: Some("198.51.100.7".parse().unwrap()),
                result: 0,
            })
        );
        p[3] = 1;
        assert_eq!(parse_response(&p), Ok(Response::UnsupportedVersion { version: 2 }));
        p[3] = 0;
        p[1] = 0x80; // ANNOUNCE
        assert_eq!(parse_response(&p), Err(Invalid::Opcode));
        p[1] = 0x81;
        p.pop();
        assert_eq!(parse_response(&p), Err(Invalid::Size));
        assert_eq!(parse_response(&[2, 0x81, 0, 0, 0, 0, 0, 0]), Err(Invalid::Size));
    }
}
