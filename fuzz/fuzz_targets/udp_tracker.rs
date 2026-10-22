//! UDP tracker replies (BEP 15): connect, announce (IPv4 and IPv6 peers), scrape and error, as
//! `udp_decode` reads them, with the sizes each decoded reply must agree with.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::{UdpResponse, decode_udp_tracker_response};

fuzz_target!(|data: &[u8]| {
    for v6 in [false, true] {
        let Some(r) = decode_udp_tracker_response(data, v6) else { continue };
        assert!(data.len() >= 8);
        let txn = u32::from_be_bytes(data[4..8].try_into().unwrap());
        match r {
            UdpResponse::Connect { txn: t, .. } => {
                assert_eq!(t, txn);
                assert!(data.len() >= 16);
            }
            UdpResponse::Announce { txn: t, peers, .. } => {
                assert_eq!(t, txn);
                let size = if v6 { 18 } else { 6 };
                assert_eq!(peers.len(), (data.len() - 20) / size);
                assert!(peers.iter().all(|p| p.is_ipv6() == v6));
            }
            UdpResponse::Scrape { txn: t, entries } => {
                assert_eq!(t, txn);
                assert_eq!(entries.len(), (data.len() - 8) / 12);
            }
            UdpResponse::Error { txn: t, message } => {
                assert_eq!(t, txn);
                assert!(message.len() <= 3 * (data.len() - 8));
            }
        }
    }
});
