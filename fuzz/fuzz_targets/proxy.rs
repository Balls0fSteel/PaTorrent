//! Proxy replies: arbitrary bytes from a SOCKS4, SOCKS5 (CONNECT or UDP ASSOCIATE, with or
//! without a password) or HTTP CONNECT "proxy", fed in pieces of arbitrary sizes. A handshake
//! never takes more than it said it wants, and nothing after it is over. Also SOCKS5 UDP
//! datagrams from the relay.
#![no_main]

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::{ProxyHandshake, ProxyStep, ProxyTarget, Socks5Command, udp_unwrap};

fuzz_target!(|data: &[u8]| {
    let Some((&sel, rest)) = data.split_first() else { return };
    let target = if sel & 0x10 != 0 {
        ProxyTarget::Host("tracker.example".into(), 80)
    } else {
        ProxyTarget::Addr("10.1.2.3:6881".parse().unwrap())
    };
    let creds = (sel & 0x20 != 0).then_some(("user", "pass"));
    let mut hs = match sel % 4 {
        0 => match ProxyHandshake::socks4(&target, "user") {
            Ok(h) => h,
            Err(_) => return,
        },
        1 => ProxyHandshake::socks5(Socks5Command::Connect, &target, creds),
        2 => ProxyHandshake::socks5(Socks5Command::UdpAssociate, &target, creds),
        _ => ProxyHandshake::http_connect(&target, creds, sel & 0x40 != 0),
    };
    let mut out = Vec::new();
    hs.start(&mut out);
    // chunk sizes come from the input too
    let mut input = rest;
    let mut step = (sel >> 6) as usize + 1;
    while !input.is_empty() {
        let want = hs.want();
        let n = step.min(input.len());
        match hs.feed(&input[..n], &mut out) {
            Ok((used, s)) => {
                // a reply may run into the next stage's: never past what was given
                assert!(used <= n, "took {used} of {n} (wanted {want})");
                if matches!(s, ProxyStep::Done(_)) {
                    assert!(hs.is_done() && hs.want() == 0);
                    break;
                }
                assert!(used == n, "an unfinished handshake takes all it is given up to its want");
                input = &input[used..];
            }
            Err(_) => {
                assert_eq!(hs.want(), 0);
                break;
            }
        }
        step = step % 7 + 1;
    }
    let _ = udp_unwrap(rest);
});
