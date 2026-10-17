//! IP filter block lists (`ipfilter.dat` and P2P text): parsing, then the filter's invariants:
//! the exported ranges tile the address space, neighbours differ, and `access` agrees.
#![no_main]

use std::net::IpAddr;

use libfuzzer_sys::fuzz_target;
use patorrent_core::IpFilter;

fuzz_target!(|data: &[u8]| {
    let l = IpFilter::parse(data);
    let (v4, v6) = l.filter.export_filter();
    assert_eq!(v4.first().unwrap().first.to_bits(), 0);
    assert_eq!(v4.last().unwrap().last.to_bits(), u32::MAX);
    assert_eq!(v6.first().unwrap().first.to_bits(), 0);
    assert_eq!(v6.last().unwrap().last.to_bits(), u128::MAX);
    for w in v4.windows(2) {
        assert_eq!(w[0].last.to_bits() + 1, w[1].first.to_bits());
        assert_ne!(w[0].flags, w[1].flags);
    }
    for w in v6.windows(2) {
        assert_eq!(w[0].last.to_bits() + 1, w[1].first.to_bits());
        assert_ne!(w[0].flags, w[1].flags);
    }
    for r in &v4 {
        assert_eq!(l.filter.access(IpAddr::V4(r.first)), r.flags);
        assert_eq!(l.filter.access(IpAddr::V4(r.last)), r.flags);
    }
    // (IPv4-mapped IPv6 addresses are looked up in the IPv4 table)
    for a in v6.iter().flat_map(|r| [(r.first, r.flags), (r.last, r.flags)]) {
        if a.0.to_ipv4_mapped().is_none() {
            assert_eq!(l.filter.access(IpAddr::V6(a.0)), a.1);
        }
    }
    // every rule a block list adds blocks something
    assert_eq!(l.filter.is_empty(), l.rules == 0);
});
