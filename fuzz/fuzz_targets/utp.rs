//! uTP (`core::utp`): the input as datagrams into a manager with no sockets (SYN handling), into
//! a socket waiting for its SYN-ACK and into an accepted connection, with the connection id
//! patched in so the packets reach the socket's parser (header, extensions, selective ACKs,
//! sequence-number arithmetic), then the timers. Since the security review (2026-10-22) the
//! unpatched datagrams also go, one by one, to a manager with a small connection limit, which
//! must stay within its SYN-flood bound and answer every datagram with at most one; and the
//! accepted connection is detached halfway, so the rest reaches a closing socket nobody reads.
#![no_main]

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use libfuzzer_sys::fuzz_target;
use patorrent_core::fuzzing::{UtpIo, UtpManager, UtpSendOutcome, UtpSettings};

#[derive(Default)]
struct Sink {
    sent: usize,
}

impl UtpIo for Sink {
    fn send(&mut self, _: SocketAddr, _: &[u8], _: bool) -> UtpSendOutcome {
        self.sent += 1;
        UtpSendOutcome::Sent
    }
}

/// Splits the input into datagrams: each starts with a length byte.
fn datagrams(mut data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    while let Some((&len, rest)) = data.split_first() {
        let n = (len as usize).min(rest.len());
        out.push(&rest[..n]);
        data = &rest[n..];
    }
    out
}

fuzz_target!(|data: &[u8]| {
    let now = Instant::now();
    let from: SocketAddr = "10.0.0.2:6881".parse().unwrap();
    let mut io = Sink::default();

    // 1. an unknown connection: only a SYN makes a socket
    let mut m = UtpManager::new(UtpSettings::default(), now);
    m.incoming_packet(from, data, &mut io, now);
    m.socket_drained(&mut io, now);
    for id in m.take_accepted() {
        let mut out = Vec::new();
        let _ = m.read_some(id, &mut out, 1 << 16);
        let _ = m.write_some(id, b"hello", &mut io, now);
    }

    // 1b. every datagram, unpatched, from one of two addresses, into a manager that holds at
    // most 2 * 2 + 1 sockets and whose sockets have nothing of their own to send: no more
    // sockets than that, and no more than one datagram out for each one in
    let mut m = UtpManager::new(UtpSettings { connections_limit: 2, ..UtpSettings::default() }, now);
    let mut flood = Sink::default();
    let other: SocketAddr = "10.0.0.3:6881".parse().unwrap();
    let dgrams = datagrams(data);
    for (i, d) in dgrams.iter().enumerate() {
        let src = if i % 3 == 2 { other } else { from };
        m.incoming_packet(src, d, &mut flood, now);
        m.socket_drained(&mut flood, now);
        assert!(m.num_sockets() <= 5, "{} sockets", m.num_sockets());
        assert!(flood.sent <= i + 1, "{} datagrams out for {} in", flood.sent, i + 1);
    }
    for id in m.take_accepted() {
        let mut out = Vec::new();
        let _ = m.read_some(id, &mut out, 1 << 16);
    }

    // 2. a socket waiting for its SYN-ACK, then one that was connected by a SYN
    let mut m = UtpManager::new(UtpSettings::default(), now);
    let connecting = m.connect(from, 1, &mut io, now);
    let mut syn = [0u8; 20];
    syn[0] = (4 << 4) | 1; // ST_SYN, version 1
    syn[2..4].copy_from_slice(&0x1234u16.to_be_bytes());
    syn[4..8].copy_from_slice(&1u32.to_be_bytes());
    syn[16..18].copy_from_slice(&7u16.to_be_bytes());
    m.incoming_packet(from, &syn, &mut io, now);
    m.socket_drained(&mut io, now);
    let accepted = m.take_accepted().first().copied();
    let ids: Vec<(usize, u16)> = [Some(connecting), accepted]
        .into_iter()
        .flatten()
        .filter_map(|id| m.get(id).map(|s| (id, s.receive_id())))
        .collect();
    let mut t = now;
    let half = dgrams.len() / 2;
    for (i, d) in dgrams.iter().enumerate() {
        if i == half && half > 0 {
            if let Some(id) = accepted {
                m.detach(id, &mut io, t);
            }
        }
        let (id, recv_id) = ids[i % ids.len()];
        let mut pkt = d.to_vec();
        if pkt.len() >= 4 && pkt[0] & 1 == 1 {
            // keep the version nibble, point the connection id at the socket
            pkt[2..4].copy_from_slice(&recv_id.to_be_bytes());
        }
        m.incoming_packet(from, &pkt, &mut io, t);
        m.socket_drained(&mut io, t);
        let mut out = Vec::new();
        let _ = m.read_some(id, &mut out, 1 << 16);
        let _ = m.write_some(id, d, &mut io, t);
        for _ in m.take_ready() {}
        t += Duration::from_millis(d.first().copied().unwrap_or(0) as u64 * 10);
        m.tick(&mut io, t);
    }
    for &(id, _) in &ids {
        m.detach(id, &mut io, t);
    }
    for _ in 0..10 {
        t += Duration::from_secs(10);
        m.tick(&mut io, t);
    }
    // every socket was let go: within the linger (60 s) and the timeouts they are gone
    assert_eq!(m.num_sockets(), 0, "sockets left after the upper layer let go");
});
