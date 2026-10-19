// Ported from libtorrent-rasterbar 2.0.11: src/ip_filter.cpp, include/libtorrent/ip_filter.hpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: addresses are integers (u32, u128, u16 for ports) in a BTreeMap instead of byte
// arrays in a std::set; add_rule rebuilds the affected span and keeps the map minimal. An
// IPv4-mapped IPv6 address is looked up in the IPv4 table. The ipfilter.dat / P2P parser is new.

//! IP and port filters (libtorrent `ip_filter`, `port_filter`) and a parser for block lists in
//! the eMule / µTorrent `ipfilter.dat` and PeerGuardian P2P text formats.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// An address (or port) type the filter maps ranges of.
trait Key: Copy + Ord {
    const ZERO: Self;
    const MAX: Self;
    fn plus_one(self) -> Self;
    fn minus_one(self) -> Self;
}

macro_rules! key {
    ($($t:ty),*) => {$(
        impl Key for $t {
            const ZERO: Self = 0;
            const MAX: Self = <$t>::MAX;
            fn plus_one(self) -> Self { self.wrapping_add(1) }
            fn minus_one(self) -> Self { self.wrapping_sub(1) }
        }
    )*};
}
key!(u16, u32, u128);

/// libtorrent `aux::filter_impl`: non-overlapping ranges covering the whole key space, each
/// starting at its key and ending where the next one starts.
#[derive(Clone, Debug, PartialEq, Eq)]
struct FilterImpl<K: Key> {
    access_list: BTreeMap<K, u32>,
}

impl<K: Key> Default for FilterImpl<K> {
    fn default() -> Self {
        // the entire range, not blocked
        FilterImpl { access_list: BTreeMap::from([(K::ZERO, 0)]) }
    }
}

impl<K: Key> FilterImpl<K> {
    fn is_empty(&self) -> bool {
        self.access_list.len() == 1 && self.access_list.get(&K::ZERO) == Some(&0)
    }

    /// Afterwards `access(x) == flags` for every `x` in `[first, last]`; neighbouring ranges
    /// with equal flags are merged.
    fn add_rule(&mut self, first: K, last: K, flags: u32) {
        debug_assert!(first <= last);
        let last_access = self.access(last);
        let doomed: Vec<K> = self.access_list.range(first..=last).map(|(k, _)| *k).collect();
        for k in doomed {
            self.access_list.remove(&k);
        }
        self.access_list.insert(first, flags);
        if last != K::MAX {
            self.access_list.entry(last.plus_one()).or_insert(last_access);
        }
        // merge with the range before ...
        if first != K::ZERO && self.access(first.minus_one()) == flags {
            self.access_list.remove(&first);
        }
        // ... and the one after
        if last != K::MAX && self.access_list.get(&last.plus_one()) == Some(&flags) {
            self.access_list.remove(&last.plus_one());
        }
    }

    fn access(&self, addr: K) -> u32 {
        self.access_list.range(..=addr).next_back().map_or(0, |(_, a)| *a)
    }

    /// `(first, last, flags)` per range, from low to high addresses.
    fn export_filter(&self) -> Vec<(K, K, u32)> {
        let mut ret = Vec::with_capacity(self.access_list.len());
        let mut it = self.access_list.iter().peekable();
        while let Some((&start, &access)) = it.next() {
            let last = it.peek().map_or(K::MAX, |(k, _)| k.minus_one());
            ret.push((start, last, access));
        }
        ret
    }
}

/// One range of an exported filter (libtorrent `ip_range`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpRange<A> {
    pub first: A,
    pub last: A,
    pub flags: u32,
}

/// A set of rules that puts every IP address in exactly one range with flags (libtorrent
/// `ip_filter`). A new filter allows everything.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IpFilter {
    filter4: FilterImpl<u32>,
    filter6: FilterImpl<u128>,
}

impl IpFilter {
    /// Addresses in a range with this flag are neither connected to nor accepted.
    pub const BLOCKED: u32 = 1;

    pub fn new() -> IpFilter {
        IpFilter::default()
    }

    /// True if no rule blocks anything.
    pub fn is_empty(&self) -> bool {
        self.filter4.is_empty() && self.filter6.is_empty()
    }

    /// Marks `[first, last]` with `flags` (0 = allowed, [`IpFilter::BLOCKED`]). Overlapping
    /// rules: the last one added wins. Both ends must be of the same family (and `first <=
    /// last`); otherwise the rule is ignored.
    pub fn add_rule(&mut self, first: IpAddr, last: IpAddr, flags: u32) {
        match (first, last) {
            (IpAddr::V4(a), IpAddr::V4(b)) if a <= b => {
                self.filter4.add_rule(a.to_bits(), b.to_bits(), flags)
            }
            (IpAddr::V6(a), IpAddr::V6(b)) if a <= b => {
                self.filter6.add_rule(a.to_bits(), b.to_bits(), flags)
            }
            _ => debug_assert!(false, "bad ip_filter rule {first} - {last}"),
        }
    }

    /// The flags of the range `addr` falls in. O(log n).
    pub fn access(&self, addr: IpAddr) -> u32 {
        match addr {
            IpAddr::V4(a) => self.filter4.access(a.to_bits()),
            IpAddr::V6(a) => match a.to_ipv4_mapped() {
                Some(a) => self.filter4.access(a.to_bits()),
                None => self.filter6.access(a.to_bits()),
            },
        }
    }

    pub fn is_blocked(&self, addr: IpAddr) -> bool {
        self.access(addr) & IpFilter::BLOCKED != 0
    }

    /// The filter in the fewest ranges, low to high, for IPv4 and IPv6.
    pub fn export_filter(&self) -> (Vec<IpRange<Ipv4Addr>>, Vec<IpRange<Ipv6Addr>>) {
        let v4 = self.filter4.export_filter().into_iter();
        let v6 = self.filter6.export_filter().into_iter();
        (
            v4.map(|(a, b, flags)| IpRange {
                first: Ipv4Addr::from_bits(a),
                last: Ipv4Addr::from_bits(b),
                flags,
            })
            .collect(),
            v6.map(|(a, b, flags)| IpRange {
                first: Ipv6Addr::from_bits(a),
                last: Ipv6Addr::from_bits(b),
                flags,
            })
            .collect(),
        )
    }

    /// Reads a block list in the `ipfilter.dat` or P2P format (see [`IpFilter::parse`]),
    /// gzip-compressed or not (lists are often published as `.gz`; the file's first bytes
    /// decide, not its name). A list may inflate to at most [`IpFilter::MAX_LIST_SIZE`].
    pub fn load(path: impl AsRef<std::path::Path>) -> std::io::Result<IpFilterLoad> {
        let data = std::fs::read(path)?;
        if crate::util::gzip::is_gzip(&data) {
            let plain = crate::util::gzip::inflate_gzip(&data, IpFilter::MAX_LIST_SIZE)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            return Ok(IpFilter::parse(&plain));
        }
        Ok(IpFilter::parse(&data))
    }

    /// The largest block list [`IpFilter::load`] inflates (the big public lists are some
    /// tens of MB as text).
    pub const MAX_LIST_SIZE: usize = 512 << 20;

    /// Parses a block list. Each line is one of
    ///
    /// * eMule / µTorrent `ipfilter.dat`: `first - last [, level [, description]]`, where a
    ///   level of 127 or less blocks the range and a higher one leaves it alone (no level
    ///   blocks); octets may carry leading zeros (`001.002.003.004`), and a lone address is a
    ///   range of one;
    /// * PeerGuardian P2P: `description:first-last`.
    ///
    /// Blank lines and `#` or `//` comments are skipped; other lines that fit neither format
    /// are counted in [`IpFilterLoad::invalid`] and skipped.
    pub fn parse(data: &[u8]) -> IpFilterLoad {
        let mut out = IpFilterLoad { filter: IpFilter::new(), rules: 0, invalid: 0 };
        let data = data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data);
        for line in data.split(|&b| b == b'\n') {
            let line = String::from_utf8_lossy(line);
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
                continue;
            }
            match parse_dat_line(line).or_else(|| parse_p2p_line(line)) {
                Some(Rule::Block(a, b)) => {
                    out.filter.add_rule(a, b, IpFilter::BLOCKED);
                    out.rules += 1;
                }
                Some(Rule::Allow) => {}
                None => out.invalid += 1,
            }
        }
        out
    }
}

/// A parsed block list.
#[derive(Clone, Debug, Default)]
pub struct IpFilterLoad {
    pub filter: IpFilter,
    /// Blocking rules added.
    pub rules: usize,
    /// Lines that were neither a rule, a comment nor blank.
    pub invalid: usize,
}

enum Rule {
    Block(IpAddr, IpAddr),
    /// An `ipfilter.dat` line with a level above 127.
    Allow,
}

/// `first - last , level , description`.
fn parse_dat_line(line: &str) -> Option<Rule> {
    let mut fields = line.splitn(3, ',');
    let range = fields.next()?;
    let (first, last) = parse_range(range, true)?;
    if let Some(level) = fields.next() {
        let level = level.trim();
        // a non-numeric level is not this format
        let level: u32 = if level.is_empty() { 0 } else { level.parse().ok()? };
        if level > 127 {
            return Some(Rule::Allow);
        }
    }
    Some(Rule::Block(first, last))
}

/// `description:first-last`. The description may itself hold colons (and IPv6 addresses do),
/// so the first colon after which a valid range follows splits the line.
fn parse_p2p_line(line: &str) -> Option<Rule> {
    line.match_indices(':')
        .take(64)
        .find_map(|(i, _)| parse_range(&line[i + 1..], false))
        .map(|(a, b)| Rule::Block(a, b))
}

/// `first - last`, or (with `single`) a lone address. Both ends of one family, `first <= last`.
fn parse_range(s: &str, single: bool) -> Option<(IpAddr, IpAddr)> {
    let s = s.trim();
    let (first, last) = match s.split_once('-') {
        Some((a, b)) => (parse_addr(a)?, parse_addr(b)?),
        None if !single => return None,
        None => {
            let a = parse_addr(s)?;
            (a, a)
        }
    };
    let ok = match (first, last) {
        (IpAddr::V4(a), IpAddr::V4(b)) => a <= b,
        (IpAddr::V6(a), IpAddr::V6(b)) => a <= b,
        _ => false,
    };
    ok.then_some((first, last))
}

/// An address, allowing the zero-padded IPv4 octets of `ipfilter.dat` (`001.002.003.004`).
fn parse_addr(s: &str) -> Option<IpAddr> {
    let s = s.trim();
    if s.contains(':') {
        return s.parse::<Ipv6Addr>().ok().map(IpAddr::V6);
    }
    let mut octets = [0u8; 4];
    let mut parts = s.split('.');
    for o in &mut octets {
        let p = parts.next()?;
        if p.is_empty() || p.len() > 3 || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        *o = p.parse().ok()?;
    }
    parts.next().is_none().then_some(IpAddr::V4(Ipv4Addr::from(octets)))
}

/// The ports a peer may have (libtorrent `port_filter`). A new filter allows every port.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PortFilter {
    filter: FilterImpl<u16>,
}

impl PortFilter {
    /// Peers listening on a port in a range with this flag are not connected to.
    pub const BLOCKED: u32 = 1;

    pub fn new() -> PortFilter {
        PortFilter::default()
    }

    /// Sets `flags` on the ports `[first, last]`, replacing earlier rules for them.
    pub fn add_rule(&mut self, first: u16, last: u16, flags: u32) {
        if first <= last {
            self.filter.add_rule(first, last, flags);
        }
    }

    pub fn access(&self, port: u16) -> u32 {
        self.filter.access(port)
    }

    pub fn is_blocked(&self, port: u16) -> bool {
        self.access(port) & PortFilter::BLOCKED != 0
    }

    pub fn is_empty(&self) -> bool {
        self.filter.is_empty()
    }
}

#[cfg(test)]
mod tests {
    // Ported from libtorrent-rasterbar 2.0.11: test/test_ip_filter.cpp
    use super::*;

    fn addr(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn a_gzipped_list_loads_like_a_plain_one() {
        // a comment, an ipfilter.dat line and a P2P line, through Python's gzip.compress:
        // "# a list\n001.002.003.000 - 001.002.003.255 , 000 , test\nsome range:5.6.7.8-5.6.7.9\n"
        let gz: [u8; 82] = [
            31, 139, 8, 0, 0, 0, 0, 0, 2, 255, 83, 86, 72, 84, 200, 201, 44, 46, 225, 50, 48, 48, 212, 51,
            48, 48, 2, 98, 99, 32, 54, 80, 208, 85, 64, 22, 49, 50, 53, 85, 208, 81, 0, 137, 235, 40, 148,
            164, 2, 149, 23, 231, 231, 166, 42, 20, 37, 230, 165, 167, 90, 153, 234, 153, 233, 153, 235, 89,
            232, 66, 104, 75, 46, 0, 33, 222, 82, 75, 83, 0, 0, 0,
        ];
        let dir = std::env::temp_dir().join(format!("patorrent-ipfilter-gz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("level1.gz");
        std::fs::write(&path, gz).unwrap();
        let l = IpFilter::load(&path).unwrap();
        assert_eq!((l.rules, l.invalid), (2, 0));
        assert_eq!(l.filter.access(addr("1.2.3.4")), IpFilter::BLOCKED);
        assert_eq!(l.filter.access(addr("5.6.7.9")), IpFilter::BLOCKED);
        assert_eq!(l.filter.access(addr("5.6.7.10")), 0);
        // a damaged one is an error, not an empty filter
        std::fs::write(&path, &gz[..50]).unwrap();
        let Err(e) = IpFilter::load(&path) else { panic!("a truncated list loaded") };
        assert_eq!(e.kind(), std::io::ErrorKind::InvalidData);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn r4(a: &str, b: &str, flags: u32) -> IpRange<Ipv4Addr> {
        IpRange { first: a.parse().unwrap(), last: b.parse().unwrap(), flags }
    }

    fn r6(a: &str, b: &str, flags: u32) -> IpRange<Ipv6Addr> {
        IpRange { first: a.parse().unwrap(), last: b.parse().unwrap(), flags }
    }

    /// libtorrent `test_rules_invariant`.
    fn rules_invariant<A: Copy + Into<IpAddr> + PartialEq + std::fmt::Debug>(
        r: &[IpRange<A>],
        f: &IpFilter,
        lowest: A,
        highest: A,
        plus_one: impl Fn(A) -> A,
    ) {
        assert!(!r.is_empty());
        assert_eq!(r[0].first, lowest);
        assert_eq!(r.last().unwrap().last, highest);
        for w in r.windows(2) {
            assert_eq!(f.access(w[0].last.into()), w[0].flags);
            assert_eq!(f.access(w[1].first.into()), w[1].flags);
            assert_eq!(plus_one(w[0].last), w[1].first);
        }
    }

    fn invariant4(r: &[IpRange<Ipv4Addr>], f: &IpFilter) {
        rules_invariant(r, f, Ipv4Addr::UNSPECIFIED, Ipv4Addr::BROADCAST, |a| {
            Ipv4Addr::from_bits(a.to_bits() + 1)
        });
    }

    fn expected1() -> Vec<IpRange<Ipv4Addr>> {
        vec![
            r4("0.0.0.0", "0.255.255.255", 0),
            r4("1.0.0.0", "3.0.0.0", IpFilter::BLOCKED),
            r4("3.0.0.1", "255.255.255.255", 0),
        ]
    }

    #[test]
    fn session_get_ip_filter() {
        assert_eq!(IpFilter::new().export_filter().0.len(), 1);
    }

    #[test]
    fn joining_ranges_at_end() {
        let mut f = IpFilter::new();
        f.add_rule(addr("1.0.0.0"), addr("2.0.0.0"), IpFilter::BLOCKED);
        f.add_rule(addr("2.0.0.1"), addr("3.0.0.0"), IpFilter::BLOCKED);
        let range = f.export_filter().0;
        invariant4(&range, &f);
        assert_eq!(range, expected1());
    }

    #[test]
    fn joining_ranges_at_start() {
        let mut f = IpFilter::new();
        f.add_rule(addr("2.0.0.1"), addr("3.0.0.0"), IpFilter::BLOCKED);
        f.add_rule(addr("1.0.0.0"), addr("2.0.0.0"), IpFilter::BLOCKED);
        let range = f.export_filter().0;
        invariant4(&range, &f);
        assert_eq!(range, expected1());
    }

    #[test]
    fn joining_overlapping_ranges_at_start() {
        let mut f = IpFilter::new();
        f.add_rule(addr("2.0.0.1"), addr("3.0.0.0"), IpFilter::BLOCKED);
        f.add_rule(addr("1.0.0.0"), addr("2.4.0.0"), IpFilter::BLOCKED);
        let range = f.export_filter().0;
        invariant4(&range, &f);
        assert_eq!(range, expected1());
    }

    #[test]
    fn joining_overlapping_ranges_at_end() {
        let mut f = IpFilter::new();
        f.add_rule(addr("1.0.0.0"), addr("2.4.0.0"), IpFilter::BLOCKED);
        f.add_rule(addr("2.0.0.1"), addr("3.0.0.0"), IpFilter::BLOCKED);
        let range = f.export_filter().0;
        invariant4(&range, &f);
        assert_eq!(range, expected1());
    }

    fn four_ranges() -> IpFilter {
        let mut f = IpFilter::new();
        f.add_rule(addr("1.0.0.0"), addr("2.0.0.0"), IpFilter::BLOCKED);
        f.add_rule(addr("3.0.0.0"), addr("4.0.0.0"), IpFilter::BLOCKED);
        f.add_rule(addr("5.0.0.0"), addr("6.0.0.0"), IpFilter::BLOCKED);
        f.add_rule(addr("7.0.0.0"), addr("8.0.0.0"), IpFilter::BLOCKED);
        f
    }

    #[test]
    fn joining_multiple_overlapping_ranges_1() {
        let mut f = four_ranges();
        f.add_rule(addr("1.0.1.0"), addr("9.0.0.0"), IpFilter::BLOCKED);
        let range = f.export_filter().0;
        invariant4(&range, &f);
        let expected = vec![
            r4("0.0.0.0", "0.255.255.255", 0),
            r4("1.0.0.0", "9.0.0.0", IpFilter::BLOCKED),
            r4("9.0.0.1", "255.255.255.255", 0),
        ];
        assert_eq!(range, expected);
    }

    #[test]
    fn joining_multiple_overlapping_ranges_2() {
        let mut f = four_ranges();
        f.add_rule(addr("0.0.1.0"), addr("7.0.4.0"), IpFilter::BLOCKED);
        let range = f.export_filter().0;
        invariant4(&range, &f);
        let expected = vec![
            r4("0.0.0.0", "0.0.0.255", 0),
            r4("0.0.1.0", "8.0.0.0", IpFilter::BLOCKED),
            r4("8.0.0.1", "255.255.255.255", 0),
        ];
        assert_eq!(range, expected);
    }

    #[test]
    fn ipv6() {
        let expected2 = vec![
            r6("::0", "0:ffff:ffff:ffff:ffff:ffff:ffff:ffff", 0),
            r6("1::", "3::", IpFilter::BLOCKED),
            r6("3::1", "ffff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", 0),
        ];
        let mut f = IpFilter::new();
        f.add_rule(addr("2::1"), addr("3::"), IpFilter::BLOCKED);
        f.add_rule(addr("1::"), addr("2::"), IpFilter::BLOCKED);
        let range = f.export_filter().1;
        rules_invariant(&range, &f, Ipv6Addr::UNSPECIFIED, Ipv6Addr::from_bits(u128::MAX), |a| {
            Ipv6Addr::from_bits(a.to_bits() + 1)
        });
        assert_eq!(range, expected2);
    }

    #[test]
    fn default_empty() {
        let mut f = IpFilter::new();
        assert!(f.is_empty());
        f.add_rule(addr("1::"), addr("2::"), IpFilter::BLOCKED);
        assert!(!f.is_empty());

        let mut f = IpFilter::new();
        f.add_rule(addr("0.0.1.0"), addr("7.0.4.0"), IpFilter::BLOCKED);
        assert!(!f.is_empty());

        let mut f = IpFilter::new();
        f.add_rule(addr("0.0.1.0"), addr("7.0.4.0"), 0);
        assert!(f.is_empty());
    }

    #[test]
    fn port_filter() {
        let mut pf = PortFilter::new();
        // a new port filter allows any port
        assert_eq!(pf.access(0), 0);
        assert_eq!(pf.access(65535), 0);
        assert_eq!(pf.access(6881), 0);

        // block port 100 - 300
        pf.add_rule(100, 300, PortFilter::BLOCKED);

        assert_eq!(pf.access(0), 0);
        assert_eq!(pf.access(99), 0);
        assert_eq!(pf.access(100), PortFilter::BLOCKED);
        assert_eq!(pf.access(150), PortFilter::BLOCKED);
        assert_eq!(pf.access(300), PortFilter::BLOCKED);
        assert_eq!(pf.access(301), 0);
        assert_eq!(pf.access(6881), 0);
        assert_eq!(pf.access(65535), 0);
    }

    // --- PaTorrent's own tests ---------------------------------------------------------------

    #[test]
    fn edges_of_the_address_space() {
        let mut f = IpFilter::new();
        f.add_rule(addr("0.0.0.0"), addr("0.0.0.0"), IpFilter::BLOCKED);
        f.add_rule(addr("255.255.255.255"), addr("255.255.255.255"), IpFilter::BLOCKED);
        let range = f.export_filter().0;
        invariant4(&range, &f);
        assert_eq!(
            range,
            [
                r4("0.0.0.0", "0.0.0.0", 1),
                r4("0.0.0.1", "255.255.255.254", 0),
                r4("255.255.255.255", "255.255.255.255", 1)
            ]
        );
        f.add_rule(addr("0.0.0.0"), addr("255.255.255.255"), 0);
        assert!(f.is_empty());
        f.add_rule(addr("0.0.0.0"), addr("255.255.255.255"), IpFilter::BLOCKED);
        assert_eq!(f.export_filter().0, [r4("0.0.0.0", "255.255.255.255", 1)]);
    }

    #[test]
    fn a_hole_punched_into_a_range_and_filled_again() {
        let mut f = IpFilter::new();
        f.add_rule(addr("10.0.0.0"), addr("10.255.255.255"), IpFilter::BLOCKED);
        f.add_rule(addr("10.1.0.0"), addr("10.1.255.255"), 0);
        assert!(f.is_blocked(addr("10.0.255.255")));
        assert!(!f.is_blocked(addr("10.1.2.3")));
        assert!(f.is_blocked(addr("10.2.0.0")));
        assert_eq!(f.export_filter().0.len(), 5);
        f.add_rule(addr("10.1.0.0"), addr("10.1.255.255"), IpFilter::BLOCKED);
        assert_eq!(f.export_filter().0.len(), 3);
    }

    #[test]
    fn random_rules_match_a_naive_model() {
        // a 256-address space through the IPv4 table's low byte
        let mut x = 0x2545_f491u32;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x
        };
        for _ in 0..200 {
            let mut f = IpFilter::new();
            let mut model = [0u32; 256];
            for _ in 0..12 {
                let (a, b) = ((next() % 256) as u8, (next() % 256) as u8);
                let (a, b) = (a.min(b), a.max(b));
                let flags = next() % 3;
                f.add_rule(addr(&format!("0.0.0.{a}")), addr(&format!("0.0.0.{b}")), flags);
                model[a as usize..=b as usize].fill(flags);
            }
            for (i, m) in model.iter().enumerate() {
                assert_eq!(f.access(addr(&format!("0.0.0.{i}"))), *m);
            }
            let ranges = f.export_filter().0;
            invariant4(&ranges, &f);
            assert!(ranges.windows(2).all(|w| w[0].flags != w[1].flags), "minimal");
        }
    }

    #[test]
    fn mapped_ipv6_addresses_use_the_ipv4_rules() {
        let mut f = IpFilter::new();
        f.add_rule(addr("1.2.3.0"), addr("1.2.3.255"), IpFilter::BLOCKED);
        assert!(f.is_blocked(addr("::ffff:1.2.3.4")));
        assert!(!f.is_blocked(addr("::1.2.3.4")));
    }

    #[test]
    fn parse_ipfilter_dat() {
        let text = b"\xef\xbb\xbf# comment\r\n\
            // another comment\r\n\
            001.002.004.000 - 001.002.004.255 , 000 , Some organisation\r\n\
            \r\n\
            002.000.000.000 - 002.000.000.010 , 127 , level 127 blocks\n\
            003.000.000.000 - 003.000.000.010 , 200 , level 200 does not\n\
            4.0.0.1-4.0.0.2\n\
            5.5.5.5\n\
            2001:db8:: - 2001:db8::ffff , 0 , IPv6\n\
            garbage line\n\
            9.0.0.10 - 9.0.0.1 , 0 , backwards\n\
            1.2.3.4 - 1::2 , 0 , mixed\n\
            256.0.0.0 - 256.0.0.1 , 0 , bad octet\n\
            6.0.0.0 - 6.0.0.1 , abc , bad level\n";
        let l = IpFilter::parse(text);
        assert_eq!(l.rules, 5);
        assert_eq!(l.invalid, 5);
        let f = &l.filter;
        assert!(f.is_blocked(addr("1.2.4.0")) && f.is_blocked(addr("1.2.4.255")));
        assert!(!f.is_blocked(addr("1.2.5.0")) && !f.is_blocked(addr("1.2.3.255")));
        assert!(f.is_blocked(addr("2.0.0.5")));
        assert!(!f.is_blocked(addr("3.0.0.5")));
        assert!(f.is_blocked(addr("4.0.0.2")) && !f.is_blocked(addr("4.0.0.3")));
        assert!(f.is_blocked(addr("5.5.5.5")) && !f.is_blocked(addr("5.5.5.6")));
        assert!(f.is_blocked(addr("2001:db8::1")) && !f.is_blocked(addr("2001:db8::1:0")));
        assert!(!f.is_blocked(addr("9.0.0.5")) && !f.is_blocked(addr("6.0.0.0")));
    }

    #[test]
    fn parse_p2p() {
        let text = b"Bad Corp:1.2.3.0-1.2.3.255\n\
            Name: with colons:10.0.0.0-10.0.0.255\n\
            v6 range:2001:db8::-2001:db8::ff\n\
            no range here:\n\
            :7.7.7.7-7.7.7.8\n";
        let l = IpFilter::parse(text);
        assert_eq!((l.rules, l.invalid), (4, 1));
        let f = &l.filter;
        assert!(f.is_blocked(addr("1.2.3.200")));
        assert!(f.is_blocked(addr("10.0.0.7")));
        assert!(f.is_blocked(addr("2001:db8::80")) && !f.is_blocked(addr("2001:db8::100")));
        assert!(f.is_blocked(addr("7.7.7.8")));
    }

    #[test]
    fn parse_garbage_does_not_panic() {
        for s in [&b""[..], b"\n\n\n", b"-", b",,,", b":::-:::", b"\xff\xfe\x00", b"1.2.3.4-", b"-1.2.3.4"] {
            let l = IpFilter::parse(s);
            assert_eq!(l.rules, 0);
        }
    }
}
