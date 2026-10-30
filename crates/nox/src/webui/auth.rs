//! Web UI credentials and logins: PBKDF2-HMAC-SHA256 password hashes (RFC 8018), session ids,
//! and the failed-login ban list.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use patorrent_core::crypto::Sha256;
use patorrent_core::util::random::crypto_random_bytes;

/// PBKDF2 iterations for new hashes (OWASP's 2023 figure for PBKDF2-HMAC-SHA256 is 600,000;
/// this is what a small NAS answers in well under a second).
pub const DEFAULT_ITERATIONS: u32 = 100_000;

const BLOCK: usize = 64;

/// HMAC-SHA256 (RFC 2104).
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut k = [0u8; BLOCK];
    if key.len() > BLOCK {
        k[..32].copy_from_slice(&Sha256::digest(key).0);
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut ipad = [0x36u8; BLOCK];
    let mut opad = [0x5cu8; BLOCK];
    for i in 0..BLOCK {
        ipad[i] ^= k[i];
        opad[i] ^= k[i];
    }
    let mut inner = Sha256::default();
    inner.update(&ipad).update(msg);
    let inner = inner.finalize();
    let mut outer = Sha256::default();
    outer.update(&opad).update(&inner.0);
    outer.finalize().0
}

/// PBKDF2 with HMAC-SHA256 (RFC 8018 section 5.2).
pub fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    for (block, chunk) in out.chunks_mut(32).enumerate() {
        let mut msg = salt.to_vec();
        msg.extend_from_slice(&(block as u32 + 1).to_be_bytes());
        let mut u = hmac_sha256(password, &msg);
        let mut t = u;
        for _ in 1..iterations.max(1) {
            u = hmac_sha256(password, &u);
            for (a, b) in t.iter_mut().zip(u) {
                *a ^= b;
            }
        }
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
}

/// Compares in time that depends only on the lengths.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(s.get(i..i + 2)?, 16).ok()).collect()
}

/// `n` random bytes from the OS RNG, as hex.
pub fn random_hex(n: usize) -> String {
    let mut b = vec![0u8; n];
    crypto_random_bytes(&mut b);
    hex(&b)
}

/// A stored password: `pbkdf2-sha256:<iterations>:<salt hex>:<hash hex>`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasswordHash {
    iterations: u32,
    salt: Vec<u8>,
    hash: Vec<u8>,
}

impl PasswordHash {
    pub fn new(password: &str, iterations: u32) -> PasswordHash {
        let mut salt = vec![0u8; 16];
        crypto_random_bytes(&mut salt);
        let mut hash = vec![0u8; 32];
        pbkdf2_sha256(password.as_bytes(), &salt, iterations, &mut hash);
        PasswordHash { iterations, salt, hash }
    }

    pub fn verify(&self, password: &str) -> bool {
        let mut h = vec![0u8; self.hash.len()];
        pbkdf2_sha256(password.as_bytes(), &self.salt, self.iterations, &mut h);
        constant_time_eq(&h, &self.hash)
    }

    pub fn parse(s: &str) -> Option<PasswordHash> {
        let mut it = s.split(':');
        if it.next()? != "pbkdf2-sha256" {
            return None;
        }
        let iterations = it.next()?.parse().ok().filter(|&n| n > 0)?;
        let salt = unhex(it.next()?)?;
        let hash = unhex(it.next()?)?;
        (it.next().is_none() && !hash.is_empty() && hash.len() <= 64).then_some(PasswordHash {
            iterations,
            salt,
            hash,
        })
    }
}

impl std::fmt::Display for PasswordHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "pbkdf2-sha256:{}:{}:{}", self.iterations, hex(&self.salt), hex(&self.hash))
    }
}

/// Failed logins per client (qBittorrent `web_ui_max_auth_fail_count` /
/// `web_ui_ban_duration`). An IPv4 client is its address; an IPv6 client is its /64, since a
/// host given a /64 can pick any address in it (privacy addresses, or on purpose) and would
/// otherwise get a fresh count with each one. qBittorrent bans per address; the /64 is ours
/// (security review, 2026-10-22).
#[derive(Default)]
pub struct BanList {
    entries: HashMap<IpAddr, (u32, Option<Instant>)>,
}

/// What a client's failures are counted under: an IPv4 address (IPv4-mapped IPv6 included) as
/// is, an IPv6 address with its last 64 bits cleared.
pub(crate) fn ban_key(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V6(v6) => IpAddr::V6((u128::from(v6) & !u128::from(u64::MAX)).into()),
        v4 => v4,
    }
}

impl BanList {
    pub fn is_banned(&mut self, ip: IpAddr, now: Instant) -> bool {
        let ip = ban_key(ip);
        match self.entries.get(&ip) {
            Some((_, Some(until))) if *until > now => true,
            Some((_, Some(_))) => {
                self.entries.remove(&ip);
                false
            }
            _ => false,
        }
    }

    /// Counts a failure; true when this one got the client banned.
    pub fn failed(&mut self, ip: IpAddr, max_fails: u32, ban: Duration, now: Instant) -> bool {
        let ip = ban_key(ip);
        if self.entries.len() > 10_000 {
            // Bounded: forget addresses that are not banned.
            self.entries.retain(|_, (_, until)| until.is_some_and(|u| u > now));
        }
        let e = self.entries.entry(ip).or_insert((0, None));
        e.0 += 1;
        if max_fails > 0 && e.0 >= max_fails {
            e.1 = Some(now + ban);
            return true;
        }
        false
    }

    pub fn succeeded(&mut self, ip: IpAddr) {
        self.entries.remove(&ban_key(ip));
    }
}

/// The shortest API key the Preferences take (`webui.api_key`).
pub const API_KEY_MIN_LEN: usize = 16;

/// The token of an `Authorization: Bearer <token>` header (the scheme in any case).
pub fn bearer_token(authorization: &str) -> Option<&str> {
    let (scheme, token) = authorization.trim().split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

/// The SHA-256 of an API key: what the last key that verified is remembered as, so the
/// PBKDF2 hash runs once per key rather than per request.
pub fn key_digest(key: &str) -> [u8; 32] {
    Sha256::digest(key.as_bytes()).0
}

/// One entry of the subnet whitelist (`webui.bypass_auth_subnets`): an address and the length
/// of its network prefix. An IPv4-mapped IPv6 subnet is kept as its IPv4 one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Subnet {
    addr: IpAddr,
    prefix: u8,
}

impl Subnet {
    /// `a.b.c.d/n`, `x:y::z/n`, or a bare address (the whole prefix).
    pub fn parse(s: &str) -> Option<Subnet> {
        let s = s.trim();
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => {
                if p.is_empty() || !p.bytes().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                (a.trim().parse::<IpAddr>().ok()?, Some(p.parse::<u8>().ok()?))
            }
            None => (s.parse::<IpAddr>().ok()?, None),
        };
        let max = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = prefix.unwrap_or(max);
        if prefix > max {
            return None;
        }
        Some(match addr {
            IpAddr::V6(v6) if prefix >= 96 && v6.to_ipv4_mapped().is_some() => {
                Subnet { addr: IpAddr::V4(v6.to_ipv4_mapped().expect("checked")), prefix: prefix - 96 }
            }
            _ => Subnet { addr, prefix },
        })
    }

    /// Whether `ip` (IPv4-mapped counting as IPv4) is in this subnet.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.addr, ip.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.prefix)).unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(self.prefix)).unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// The entries of a `;`- or `,`-separated subnet list, blanks skipped; the first entry that is
/// not a subnet or an address is the error.
pub fn parse_subnets(list: &str) -> Result<Vec<Subnet>, String> {
    list.split([';', ',', '\n'])
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|e| Subnet::parse(e).ok_or_else(|| e.to_owned()))
        .collect()
}

/// Whether `ip` is in one of the list's subnets (entries that do not parse match nothing).
pub fn subnets_contain(list: &str, ip: IpAddr) -> bool {
    list.split([';', ',', '\n']).filter_map(Subnet::parse).any(|s| s.contains(ip))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnets_parse_and_match() {
        let ip = |s: &str| -> IpAddr { s.parse().unwrap() };
        let any4 = Subnet::parse("0.0.0.0/0").unwrap();
        assert!(any4.contains(ip("1.2.3.4")) && any4.contains(ip("255.255.255.255")));
        assert!(any4.contains(ip("::ffff:10.0.0.1")));
        assert!(!any4.contains(ip("::1")));
        let host = Subnet::parse("192.168.1.10/32").unwrap();
        assert!(host.contains(ip("192.168.1.10")) && !host.contains(ip("192.168.1.11")));
        assert_eq!(Subnet::parse(" 192.168.1.10 "), Some(host));
        let lan = Subnet::parse("192.168.1.0/24").unwrap();
        assert!(lan.contains(ip("192.168.1.0")) && lan.contains(ip("192.168.1.255")));
        assert!(!lan.contains(ip("192.168.0.255")) && !lan.contains(ip("192.168.2.0")));
        assert!(lan.contains(ip("::ffff:192.168.1.7")));
        // host bits in the entry are ignored, as qBittorrent's parser does
        assert!(Subnet::parse("10.1.2.3/8").unwrap().contains(ip("10.200.0.1")));
        let v6 = Subnet::parse("2001:db8::/32").unwrap();
        assert!(v6.contains(ip("2001:db8:ffff::1")) && !v6.contains(ip("2001:db9::1")));
        assert!(!v6.contains(ip("10.0.0.1")));
        assert!(Subnet::parse("::/0").unwrap().contains(ip("fe80::1")));
        assert!(Subnet::parse("::1").unwrap().contains(ip("::1")));
        // a mapped subnet is its IPv4 one
        assert_eq!(Subnet::parse("::ffff:10.0.0.0/104"), Subnet::parse("10.0.0.0/8"));
        for bad in [
            "",
            "10.0.0.0/33",
            "::/129",
            "10.0.0/8",
            "10.0.0.0/",
            "10.0.0.0/-1",
            "host",
            "1.2.3.4/8/8",
            "1.2.3.4/+8",
        ] {
            assert_eq!(Subnet::parse(bad), None, "{bad}");
        }
        assert_eq!(parse_subnets(" 10.0.0.0/8; ;fd00::/8 , 127.0.0.1").map(|v| v.len()), Ok(3));
        assert_eq!(parse_subnets("10.0.0.0/8;nope;also bad"), Err("nope".to_owned()));
        assert_eq!(parse_subnets(""), Ok(Vec::new()));
        assert!(subnets_contain("garbage; 127.0.0.0/8", ip("127.0.0.5")));
        assert!(!subnets_contain("garbage", ip("127.0.0.5")));
        assert!(!subnets_contain("", ip("127.0.0.5")));
    }

    #[test]
    fn bearer_tokens() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer  abc "), Some("abc"));
        assert_eq!(bearer_token("Basic YTpi"), None);
        assert_eq!(bearer_token("Bearer "), None);
        assert_eq!(bearer_token("Bearer"), None);
    }

    fn unhex_str(s: &str) -> Vec<u8> {
        unhex(&s.replace(' ', "")).unwrap()
    }

    #[test]
    fn hmac_rfc4231() {
        // test case 2
        assert_eq!(
            hmac_sha256(b"Jefe", b"what do ya want for nothing?").to_vec(),
            unhex_str("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
        // test case 6: a key longer than the block
        assert_eq!(
            hmac_sha256(&[0xaa; 131], b"Test Using Larger Than Block-Size Key - Hash Key First").to_vec(),
            unhex_str("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54")
        );
    }

    #[test]
    fn pbkdf2_rfc7914() {
        let mut out = [0u8; 64];
        pbkdf2_sha256(b"passwd", b"salt", 1, &mut out);
        assert_eq!(
            out.to_vec(),
            unhex_str(
                "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc\
                 49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"
            )
        );
    }

    #[test]
    fn password_hashes() {
        let h = PasswordHash::new("secret", 10);
        assert!(h.verify("secret"));
        assert!(!h.verify("Secret"));
        let s = h.to_string();
        assert!(s.starts_with("pbkdf2-sha256:10:"));
        assert!(!s.contains("secret"));
        assert_eq!(PasswordHash::parse(&s), Some(h));
        for bad in ["", "pbkdf2-sha256:0:00:00", "md5:1:00:00", "pbkdf2-sha256:1:0:00", "pbkdf2-sha256:1:00:"]
        {
            assert_eq!(PasswordHash::parse(bad), None, "{bad}");
        }
        assert_ne!(PasswordHash::new("secret", 10), PasswordHash::new("secret", 10)); // salted
    }

    #[test]
    fn bans_after_max_failures() {
        let mut b = BanList::default();
        let ip: IpAddr = "10.0.0.1".parse().unwrap();
        let t = Instant::now();
        let ban = Duration::from_secs(60);
        assert!(!b.failed(ip, 3, ban, t));
        assert!(!b.failed(ip, 3, ban, t));
        b.succeeded(ip);
        assert!(!b.failed(ip, 3, ban, t));
        assert!(!b.failed(ip, 3, ban, t));
        assert!(b.failed(ip, 3, ban, t));
        assert!(b.is_banned(ip, t + Duration::from_secs(59)));
        assert!(!b.is_banned("10.0.0.2".parse().unwrap(), t));
        assert!(!b.is_banned(ip, t + Duration::from_secs(61)));
    }

    /// Security review 2026-10-22: an IPv6 client moving within its /64 keeps its count and its
    /// ban; the next /64 and IPv4 neighbours are separate clients, and an IPv4-mapped address
    /// is its IPv4 address.
    #[test]
    fn an_ipv6_client_is_counted_and_banned_by_its_64() {
        let mut b = BanList::default();
        let t = Instant::now();
        let ban = Duration::from_secs(60);
        let addr = |s: &str| -> IpAddr { s.parse().unwrap() };
        assert!(!b.failed(addr("2001:db8:1:2::1"), 3, ban, t));
        assert!(!b.failed(addr("2001:db8:1:2:aaaa:bbbb:cccc:dddd"), 3, ban, t));
        assert!(b.failed(addr("2001:db8:1:2:ffff:ffff:ffff:ffff"), 3, ban, t));
        assert!(b.is_banned(addr("2001:db8:1:2::1234"), t));
        assert!(!b.is_banned(addr("2001:db8:1:3::1"), t));
        // a login from inside the /64 clears its count
        assert!(!b.failed(addr("2001:db8:1:3::1"), 3, ban, t));
        b.succeeded(addr("2001:db8:1:3::2"));
        assert!(!b.failed(addr("2001:db8:1:3::3"), 2, ban, t));
        // IPv4 stays per address; a mapped address counts as its IPv4 address
        assert!(!b.failed(addr("10.0.0.1"), 2, ban, t));
        assert!(b.failed(addr("::ffff:10.0.0.1"), 2, ban, t));
        assert!(b.is_banned(addr("10.0.0.1"), t));
        assert!(!b.is_banned(addr("10.0.0.2"), t));
    }
}
