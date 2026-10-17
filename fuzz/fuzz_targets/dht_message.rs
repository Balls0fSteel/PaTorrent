//! DHT (KRPC) messages as other nodes send them: queries we answer, and replies to our own
//! bootstrap lookup, fed through the whole node (DoS blocker, routing table, lookups, storage).
#![no_main]

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use patorrent_core::bencode::bdecode;
use patorrent_core::fuzzing::{DhtSettings, DhtState, DhtTracker};

fuzz_target!(|data: &[u8]| {
    let mut now = Instant::now();
    let router: SocketAddr = "8.8.8.8:6881".parse().unwrap();
    let settings = DhtSettings { block_ratelimit: 1000, ..DhtSettings::default() };
    let mut dht = DhtTracker::new(settings, DhtState::default(), now);
    dht.new_socket(true, None, now);
    dht.new_socket(false, None, now);
    dht.add_router_node(router);
    dht.start(now);
    // the transaction id of our bootstrap query, so replies can match it
    let tid = dht
        .take_outgoing()
        .first()
        .and_then(|p| bdecode(&p.data[..]).ok()?.root()?.dict_find_string_value("t").map(<[u8]>::to_vec))
        .unwrap_or_default();
    // Packets are split on 0xff bytes. A packet's first byte picks the sender; "1:t2:\0\0"
    // inside it is rewritten to the outstanding transaction id.
    for chunk in data.split(|&b| b == 0xff) {
        let Some((&who, body)) = chunk.split_first() else { continue };
        let mut body = body.to_vec();
        if tid.len() == 2 {
            if let Some(i) = body.windows(7).position(|w| w == b"1:t2:\0\0") {
                body[i + 5..i + 7].copy_from_slice(&tid);
            }
        }
        let from: SocketAddr = match who % 3 {
            0 => router,
            1 => SocketAddr::new([9, 9, who, 1].into(), 1000 + who as u16),
            _ => SocketAddr::new([0x2001, 0xdb8, 0, 0, 0, 0, 0, who as u16].into(), 6881),
        };
        dht.incoming_packet(from.is_ipv4(), from, &body, now);
        now += Duration::from_millis(300);
        dht.poll(now);
        let _ = dht.take_events();
        let _ = dht.take_outgoing();
    }
    let _ = dht.state();
    let _ = dht.num_nodes();
});
