//! Two uTP managers on a simulated network with a virtual clock (the replacement for
//! libtorrent's `simulation/test_utp.cpp`): connects, transfers with and without loss, Nagle,
//! stalls of the UDP socket, close handshakes and timeouts, all without a socket or a thread.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use super::header::{self, Header};
use super::socket::{ACK_STEPS, NEXT_SEQ_NR};
use super::{ReadOutcome, SendOutcome, SockId, State, UtpError, UtpIo, UtpManager, UtpSettings};

const A: &str = "10.0.0.1:6881";
const B: &str = "10.0.0.2:6881";
/// Virtual time per delivery round (one way).
const ROUND: Duration = Duration::from_millis(5);
const TICK: Duration = Duration::from_millis(100);

struct Net {
    /// Datagrams in flight: (from, to, data).
    queue: Vec<(SocketAddr, SocketAddr, Vec<u8>)>,
    /// Drop every n-th datagram (0: none).
    drop_every: usize,
    counter: usize,
    dropped: usize,
    sent: usize,
    /// Every send reports `WouldBlock`.
    blocked: bool,
    /// Sends larger than this report `MessageSize` (0: never).
    max_datagram: usize,
    /// Everything this side sends is lost.
    drop_from: Option<SocketAddr>,
}

impl Net {
    fn new() -> Net {
        Net {
            queue: Vec::new(),
            drop_every: 0,
            counter: 0,
            dropped: 0,
            sent: 0,
            blocked: false,
            max_datagram: 0,
            drop_from: None,
        }
    }
}

struct NetIo<'a> {
    net: &'a mut Net,
    from: SocketAddr,
}

impl UtpIo for NetIo<'_> {
    fn send(&mut self, to: SocketAddr, data: &[u8], dont_fragment: bool) -> SendOutcome {
        if self.net.blocked {
            return SendOutcome::WouldBlock;
        }
        // over the limit without DF the datagram is fragmented on the way, and a real network
        // delivers it as this one does
        if self.net.max_datagram > 0 && data.len() > self.net.max_datagram && dont_fragment {
            return SendOutcome::MessageSize;
        }
        self.net.sent += 1;
        self.net.counter += 1;
        if self.net.drop_every > 0 && self.net.counter % self.net.drop_every == 0
            || self.net.drop_from == Some(self.from)
        {
            self.net.dropped += 1;
            return SendOutcome::Sent;
        }
        self.net.queue.push((self.from, to, data.to_vec()));
        SendOutcome::Sent
    }
}

struct Sim {
    a: UtpManager,
    b: UtpManager,
    net: Net,
    now: Instant,
    last_tick: Instant,
}

fn addr(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

impl Sim {
    fn new() -> Sim {
        Sim::with_settings(UtpSettings::default())
    }

    fn with_settings(settings: UtpSettings) -> Sim {
        let now = Instant::now();
        Sim {
            a: UtpManager::new(settings.clone(), now),
            b: UtpManager::new(settings, now),
            net: Net::new(),
            now,
            last_tick: now,
        }
    }

    fn side(&mut self, who: SocketAddr) -> (&mut UtpManager, NetIo<'_>) {
        let m = if who == addr(A) { &mut self.a } else { &mut self.b };
        (m, NetIo { net: &mut self.net, from: who })
    }

    /// One delivery round: everything in flight arrives `ROUND` later (a zero one-way delay
    /// would read as "no delay sample" to uTP), ACKs for it are sent, and the timers run once
    /// per `TICK`.
    fn step(&mut self) {
        self.now += ROUND;
        let batch = std::mem::take(&mut self.net.queue);
        for (from, to, data) in batch {
            let now = self.now;
            let (m, mut io) = self.side(to);
            m.incoming_packet(from, &data, &mut io, now);
        }
        for who in [addr(A), addr(B)] {
            let now = self.now;
            let (m, mut io) = self.side(who);
            m.socket_drained(&mut io, now);
        }
        if self.now.duration_since(self.last_tick) >= TICK {
            self.last_tick = self.now;
            for who in [addr(A), addr(B)] {
                let now = self.now;
                let (m, mut io) = self.side(who);
                m.tick(&mut io, now);
            }
        }
    }

    fn steps(&mut self, n: usize) {
        for _ in 0..n {
            self.step();
        }
    }

    /// Connects A to B and returns both socket ids once the connection is up on both sides.
    fn connect(&mut self) -> (SockId, SockId) {
        let now = self.now;
        let (a, mut io) = self.side(addr(A));
        let sa = a.connect(addr(B), 7, &mut io, now);
        assert_eq!(a.get(sa).unwrap().state(), State::SynSent);
        self.step(); // SYN arrives at B
        let accepted = self.b.take_accepted();
        assert_eq!(accepted.len(), 1);
        let sb = accepted[0];
        assert_eq!(self.b.get(sb).unwrap().state(), State::Connected);
        assert_eq!(self.b.get(sb).unwrap().remote(), addr(A));
        self.step(); // the SYN-ACK arrives at A
        assert_eq!(self.a.get(sa).unwrap().state(), State::Connected);
        let ready = self.a.take_ready();
        assert!(ready.contains(&(sa, 7)), "{ready:?}");
        (sa, sb)
    }

    /// `connect`, A's SYN carrying sequence number `seq` (its first data packet is `seq + 1`).
    fn connect_at(&mut self, seq: u16) -> (SockId, SockId) {
        NEXT_SEQ_NR.with(|s| s.set(Some(seq)));
        self.connect()
    }

    fn write(&mut self, who: SocketAddr, id: SockId, data: &[u8]) -> usize {
        let now = self.now;
        let (m, mut io) = self.side(who);
        m.write_some(id, data, &mut io, now).unwrap()
    }

    fn read_all(&mut self, who: SocketAddr, id: SockId, out: &mut Vec<u8>) -> ReadOutcome {
        let m = if who == addr(A) { &mut self.a } else { &mut self.b };
        loop {
            match m.read_some(id, out, usize::MAX) {
                ReadOutcome::Data(_) => continue,
                other => return other,
            }
        }
    }

    fn detach(&mut self, who: SocketAddr, id: SockId) {
        let now = self.now;
        let (m, mut io) = self.side(who);
        m.detach(id, &mut io, now);
    }

    /// Sends `data` from `from` to the other side and returns the rounds it took.
    fn transfer(
        &mut self,
        from: SocketAddr,
        sfrom: SockId,
        sto: SockId,
        data: &[u8],
        max_rounds: usize,
    ) -> usize {
        let to = if from == addr(A) { addr(B) } else { addr(A) };
        let mut sent = 0;
        let mut got = Vec::new();
        for round in 0..max_rounds {
            if sent < data.len() {
                sent += self.write(from, sfrom, &data[sent..]);
            }
            self.step();
            match self.read_all(to, sto, &mut got) {
                ReadOutcome::WouldBlock => {}
                ReadOutcome::Eof => panic!("eof after {} bytes", got.len()),
                ReadOutcome::Error(e) => panic!("{e} after {} bytes", got.len()),
                ReadOutcome::Data(_) => unreachable!(),
            }
            if got.len() == data.len() {
                assert_eq!(got, data);
                return round + 1;
            }
            assert!(got.len() < data.len(), "received more than sent");
        }
        panic!("transfer incomplete: {} of {} bytes after {max_rounds} rounds", got.len(), data.len());
    }
}

fn payload(len: usize) -> Vec<u8> {
    let mut x = 0x2545_f491u32;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            x as u8
        })
        .collect()
}

#[test]
fn connects_and_transfers() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    let data = payload(1 << 20);
    let rounds = sim.transfer(addr(A), sa, sb, &data, 10_000);
    // slow start opens the window within a few round trips; no loss, so no resends
    assert!(rounds < 200, "{rounds} rounds");
    assert_eq!(sim.net.dropped, 0);
    assert_eq!(sim.a.counters().packet_resend, 0);
    assert_eq!(sim.a.counters().timeout, 0);
    assert!(sim.a.counters().payload_pkts_out >= (1 << 20) / 1472);
    assert_eq!(sim.b.counters().payload_pkts_in, sim.a.counters().payload_pkts_out);
    assert!(sim.a.get(sa).unwrap().cwnd() > 1500);
}

#[test]
fn transfers_both_ways_at_once() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    let da = payload(300_000);
    let db = payload(200_000);
    let (mut sent_a, mut sent_b) = (0, 0);
    let (mut got_a, mut got_b) = (Vec::new(), Vec::new());
    for _ in 0..5000 {
        if sent_a < da.len() {
            sent_a += sim.write(addr(A), sa, &da[sent_a..]);
        }
        if sent_b < db.len() {
            sent_b += sim.write(addr(B), sb, &db[sent_b..]);
        }
        sim.step();
        assert!(matches!(sim.read_all(addr(B), sb, &mut got_b), ReadOutcome::WouldBlock));
        assert!(matches!(sim.read_all(addr(A), sa, &mut got_a), ReadOutcome::WouldBlock));
        if got_b.len() == da.len() && got_a.len() == db.len() {
            break;
        }
    }
    assert_eq!(got_b, da);
    assert_eq!(got_a, db);
}

#[test]
fn survives_packet_loss() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    sim.net.drop_every = 23;
    let data = payload(400_000);
    sim.transfer(addr(A), sa, sb, &data, 50_000);
    assert!(sim.net.dropped > 0);
    let c = sim.a.counters();
    assert!(c.packet_resend > 0, "{c:?}");
    assert!(c.packet_loss > 0 || c.timeout > 0, "{c:?}");
}

#[test]
fn heavy_loss_both_ways_still_completes() {
    // every 13th datagram in either direction, ACKs included (at every 7th, a packet and its
    // resends are all lost now and then, which is a dead connection by uTP's rules)
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    sim.net.drop_every = 13;
    let data = payload(150_000);
    sim.transfer(addr(A), sa, sb, &data, 200_000);
    let data = payload(100_000);
    sim.transfer(addr(B), sb, sa, &data, 200_000);
    assert!(sim.a.counters().packet_loss > 0 || sim.a.counters().timeout > 0);
}

#[test]
fn small_writes_are_nagled_behind_an_unacked_packet() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    // nothing in flight: the first small write goes out at once
    assert_eq!(sim.write(addr(A), sa, b"hello"), 5);
    let before = sim.net.sent;
    // one undersized packet is in flight now: the next two writes are held in the Nagle packet
    assert_eq!(sim.write(addr(A), sa, b" uTP"), 4);
    assert_eq!(sim.write(addr(A), sa, b" world"), 6);
    assert_eq!(sim.net.sent, before);
    let mut got = Vec::new();
    sim.step();
    sim.read_all(addr(B), sb, &mut got);
    assert_eq!(got, b"hello");
    // the ACK releases the Nagle packet with both pieces in it
    sim.step();
    sim.step();
    sim.read_all(addr(B), sb, &mut got);
    assert_eq!(got, b"hello uTP world");
}

#[test]
fn a_stalled_udp_socket_resumes_when_writable() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    sim.net.blocked = true;
    let data = payload(50_000);
    let n = sim.write(addr(A), sa, &data);
    assert!(n > 0, "the first packet is built and queued for the resend path");
    assert!(sim.net.queue.is_empty());
    sim.net.blocked = false;
    let now = sim.now;
    let (a, mut io) = sim.side(addr(A));
    a.writable(&mut io, now);
    assert!(!sim.net.queue.is_empty(), "the stalled packet goes out once writable");
    let mut sent = n;
    let mut got = Vec::new();
    for _ in 0..2000 {
        if sent < data.len() {
            sent += sim.write(addr(A), sa, &data[sent..]);
        }
        sim.step();
        sim.read_all(addr(B), sb, &mut got);
        if got.len() == data.len() {
            break;
        }
    }
    assert_eq!(got, data);
}

#[test]
fn close_handshake_deletes_both_sockets() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    let data = payload(20_000);
    sim.transfer(addr(A), sa, sb, &data, 1000);
    // A is done: its FIN goes out after the data, and B reads EOF once it has everything
    sim.detach(addr(A), sa);
    assert_eq!(sim.a.get(sa).unwrap().state(), State::FinSent);
    let mut got = Vec::new();
    let mut eof = false;
    for _ in 0..10 {
        sim.step();
        if matches!(sim.read_all(addr(B), sb, &mut got), ReadOutcome::Eof) {
            eof = true;
            break;
        }
    }
    assert!(eof);
    assert!(sim.b.take_ready().contains(&(sb, 0)));
    // B's FIN answers; once both are acknowledged the sockets are gone
    sim.detach(addr(B), sb);
    for _ in 0..100 {
        sim.step();
        if sim.a.num_sockets() == 0 && sim.b.num_sockets() == 0 {
            break;
        }
    }
    assert_eq!(sim.a.num_sockets(), 0);
    assert_eq!(sim.b.num_sockets(), 0);
    assert_eq!(sim.a.counters().num_by_state, [0; 6]);
    assert_eq!(sim.b.counters().num_by_state, [0; 6]);
}

#[test]
fn a_remote_fin_is_read_as_eof_and_writes_fail_after_our_own() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    sim.detach(addr(B), sb);
    sim.step();
    let mut got = Vec::new();
    assert!(matches!(sim.read_all(addr(A), sa, &mut got), ReadOutcome::Eof));
    sim.detach(addr(A), sa);
    let now = sim.now;
    let (a, mut io) = sim.side(addr(A));
    assert!(matches!(a.write_some(sa, b"late", &mut io, now), Err(UtpError::Eof)));
}

#[test]
fn a_connect_nobody_answers_times_out() {
    let mut sim = Sim::new();
    let now = sim.now;
    let start = now;
    let (a, mut io) = sim.side(addr(A));
    let sa = a.connect(addr("10.0.0.9:1"), 1, &mut io, now);
    // the SYN times out after utp_connect_timeout (3 s); a socket that never heard from the
    // other end fails on its first timeout, so the SYN is not resent
    for _ in 0..(10_000 / 5) {
        sim.step();
        if sim.a.get(sa).unwrap().state() == State::ErrorWait {
            break;
        }
    }
    let s = sim.a.get(sa).unwrap();
    assert_eq!(s.state(), State::ErrorWait);
    assert_eq!(s.error(), Some(UtpError::TimedOut));
    let waited = sim.now.duration_since(start);
    assert!(waited >= Duration::from_secs(3) && waited < Duration::from_millis(3200), "{waited:?}");
    assert!(sim.a.take_ready().contains(&(sa, 1)));
    assert_eq!(sim.a.counters().timeout, 1);
    sim.detach(addr(A), sa);
    sim.step();
    sim.steps(20);
    assert_eq!(sim.a.num_sockets(), 0);
}

#[test]
fn a_dead_peer_times_out_mid_transfer() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    let data = payload(10_000);
    sim.transfer(addr(A), sa, sb, &data, 1000);
    // B vanishes: everything A sends is dropped
    sim.net.drop_every = 1;
    sim.write(addr(A), sa, &data);
    for _ in 0..(120_000 / 5) {
        sim.step();
        if sim.a.get(sa).unwrap().state() == State::ErrorWait {
            break;
        }
    }
    assert_eq!(sim.a.get(sa).unwrap().error(), Some(UtpError::TimedOut));
    // utp_num_resends (3) timeouts, each doubling: 0.5, 1.5, 2.5, 4.5 s; well under two minutes
    assert!(sim.a.counters().timeout >= 3);
}

#[test]
fn a_window_lost_whole_is_resent_while_the_peer_keeps_sending() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    // open the window
    sim.transfer(addr(A), sa, sb, &payload(100_000), 10_000);
    // A's next packets are lost, all of them (as when the receiver's UDP buffer overflows),
    // and nothing follows that would bring duplicate ACKs
    sim.net.drop_from = Some(addr(A));
    let data = payload(10_000);
    assert_eq!(sim.write(addr(A), sa, &data), data.len(), "all of it in flight");
    sim.step();
    sim.net.drop_from = None;
    // B has nothing to ACK, but goes on sending a small message every 100 ms (HAVEs, say).
    // Those must not keep pushing A's retransmission timer out.
    let (mut got, mut at_a) = (Vec::new(), Vec::new());
    for round in 0..400 {
        if round % 20 == 0 {
            sim.write(addr(B), sb, b"have");
        }
        sim.step();
        assert!(matches!(sim.read_all(addr(B), sb, &mut got), ReadOutcome::WouldBlock));
        assert!(matches!(sim.read_all(addr(A), sa, &mut at_a), ReadOutcome::WouldBlock));
        if got.len() == data.len() {
            break;
        }
    }
    assert!(got.len() == data.len(), "{} of {} bytes after 2 s", got.len(), data.len());
    assert_eq!(got, data);
    assert!(sim.a.counters().timeout >= 1, "{:?}", sim.a.counters());
    assert_eq!(sim.a.get(sa).unwrap().state(), State::Connected);
}

#[test]
fn a_reset_fails_the_socket() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    let recv_id = sim.a.get(sa).unwrap().receive_id();
    let seq = {
        // B's next sequence number, from its point of view
        let _ = sb;
        0u16
    };
    let h = Header {
        type_: header::ST_RESET,
        version: 1,
        extension: 0,
        connection_id: recv_id,
        timestamp_microseconds: 1,
        timestamp_difference_microseconds: 0,
        wnd_size: 0,
        seq_nr: seq,
        ack_nr: 0,
    };
    let mut pkt = [0u8; 20];
    h.write(&mut pkt);
    // the ack_nr of a reset must not be ahead of what we sent; 0 is behind for any random seq
    // number except a handful, in which case the reset is ignored and the socket stays up
    let now = sim.now;
    let (a, mut io) = sim.side(addr(A));
    assert!(a.incoming_packet(addr(B), &pkt, &mut io, now));
    let s = sim.a.get(sa).unwrap();
    assert!(
        s.state() == State::ErrorWait && s.error() == Some(UtpError::ConnectionReset)
            || s.state() == State::Connected
    );
}

#[test]
fn packets_that_are_not_utp_are_declined() {
    let mut sim = Sim::new();
    let now = sim.now;
    let (b, mut io) = sim.side(addr(B));
    assert!(!b.incoming_packet(
        addr(A),
        b"d1:ad2:id20:abcdefghijabcdefghije1:q4:ping1:t2:aa1:y1:qe",
        &mut io,
        now
    ));
    assert!(!b.incoming_packet(addr(A), &[0u8; 19], &mut io, now));
    // version 2 is not ours
    let mut pkt = [0u8; 20];
    Header {
        type_: header::ST_SYN,
        version: 2,
        extension: 0,
        connection_id: 1,
        timestamp_microseconds: 1,
        timestamp_difference_microseconds: 0,
        wnd_size: 0,
        seq_nr: 1,
        ack_nr: 0,
    }
    .write(&mut pkt);
    assert!(!b.incoming_packet(addr(A), &pkt, &mut io, now));
    // a data packet for a connection we don't have
    pkt[0] = (header::ST_DATA << 4) | 1;
    assert!(!b.incoming_packet(addr(A), &pkt, &mut io, now));
    assert_eq!(b.num_sockets(), 0);
    assert!(b.take_accepted().is_empty());
}

#[test]
fn incoming_connections_can_be_switched_off() {
    let mut sim = Sim::with_settings(UtpSettings { enable_incoming: false, ..UtpSettings::default() });
    let now = sim.now;
    let (a, mut io) = sim.side(addr(A));
    let sa = a.connect(addr(B), 0, &mut io, now);
    sim.step();
    assert!(sim.b.take_accepted().is_empty());
    assert_eq!(sim.b.num_sockets(), 0);
    assert_eq!(sim.a.get(sa).unwrap().state(), State::SynSent);
}

#[test]
fn an_oversized_mtu_probe_lowers_the_ceiling() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    // the interface takes at most 1200-byte datagrams
    sim.net.max_datagram = 1200;
    let data = payload(600_000);
    sim.transfer(addr(A), sa, sb, &data, 20_000);
    let s = sim.a.get(sa).unwrap();
    assert!(s.mtu() <= 1200, "mtu {}", s.mtu());
}

#[test]
fn a_path_that_drops_big_packets_without_fragmenting_kills_the_connection() {
    black_hole(None);
}

#[test]
fn a_black_hole_kills_the_connection_the_same_way_across_the_sequence_number_wrap() {
    // the MTU search runs while the receiver holds everything behind the lost first probe;
    // from 0xffc0 the probes from 980 bytes on are sent past the wrap
    black_hole(Some(0xffc0));
}

/// A black hole: a router that silently drops anything over 1000 bytes, DF bit or not, and
/// sends no ICMP. The first probe (1010 bytes) is lost; it cannot be re-packetized, so its
/// resends are lost too and the connection times out, as libtorrent's does. Meanwhile the
/// search goes on with the packets behind it (778 to 994 bytes get through) and ends on a lost
/// 1001-byte probe. The manager remembers the size that failed; once three connections failed
/// that way (the restriction is the largest of the last three), new connections start from a
/// lower ceiling. `seq` pins each connection's initial sequence number.
fn black_hole(seq: Option<u16>) {
    let mut sim = Sim::new();
    let mut restricted = Vec::new();
    for attempt in 0..3 {
        let (sa, sb) = match seq {
            Some(seq) => sim.connect_at(seq),
            None => sim.connect(),
        };
        let data = payload(400_000);
        let mut sent = 0;
        let mut got = Vec::new();
        for _ in 0..40_000 {
            if sent < data.len() {
                if let Ok(n) = {
                    let now = sim.now;
                    let (a, mut io) = sim.side(addr(A));
                    a.write_some(sa, &data[sent..], &mut io, now)
                } {
                    sent += n;
                }
            }
            sim.net.queue.retain(|(_, _, d)| d.len() <= 1000);
            sim.step();
            sim.read_all(addr(B), sb, &mut got);
            if sim.a.get(sa).unwrap().error().is_some() {
                break;
            }
        }
        let s = sim.a.get(sa).unwrap();
        assert_eq!(s.error(), Some(UtpError::TimedOut), "attempt {attempt}");
        assert!(s.mtu() > 1000, "the probe that failed: {}", s.mtu());
        restricted.push(sim.a.restricted_mtu());
        sim.detach(addr(A), sa);
        sim.detach(addr(B), sb);
        sim.steps(30);
    }
    assert_eq!(restricted[0], u16::MAX, "one failure does not restrict yet");
    assert!(restricted[2] <= 1472 && restricted[2] > 1000, "{restricted:?}");
    assert!(sim.a.mtu_for_dest(&addr(B).ip()) < 1472);
    assert!(sim.a.counters().packet_resend >= 2);
}

#[test]
fn packets_past_the_sequence_number_wrap_wait_behind_a_hole() {
    // One packet lost just before the sequence numbers wrap, with a full window behind it: the
    // receiver's reorder buffer holds everything after the hole until the resend fills it.
    // libtorrent's packet buffer compares raw sequence numbers, so a packet past the wrap and
    // past the buffer's capacity found the slot of one held from before the wrap, was dropped
    // as a duplicate and had to be sent again.
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect_at(0xfe00);
    let data = payload(1 << 20);
    let (mut sent, mut got, mut lost) = (0, Vec::new(), false);
    for _ in 0..10_000 {
        if sent < data.len() {
            sent += sim.write(addr(A), sa, &data[sent..]);
        }
        if !lost {
            let hole = sim.net.queue.iter().position(|(from, _, d)| {
                *from == addr(A)
                    && Header::parse(d).is_some_and(|h| h.type_ == header::ST_DATA && h.seq_nr == 0xfff0)
            });
            if let Some(i) = hole {
                sim.net.queue.remove(i);
                lost = true;
            }
        }
        sim.step();
        sim.read_all(addr(B), sb, &mut got);
        if got.len() == data.len() {
            break;
        }
    }
    assert!(lost, "never sent 0xfff0");
    assert_eq!(got.len(), data.len());
    assert!(got == data);
    let c = sim.a.counters();
    assert_eq!((c.packet_resend, c.timeout), (1, 0), "only the lost packet is sent again: {c:?}");
}

#[test]
fn ready_is_reported_when_the_window_opens() {
    let mut sim = Sim::new();
    let (sa, sb) = sim.connect();
    let _ = sim.a.take_ready();
    let data = payload(200_000);
    let n = sim.write(addr(A), sa, &data);
    assert!(n < data.len(), "the first window is one packet in slow start: {n}");
    assert!(sim.a.take_ready().is_empty());
    sim.step(); // data at B, which ACKs
    sim.step(); // ACK at A: room to write again
    assert!(sim.a.take_ready().contains(&(sa, 7)));
    let _ = sb;
}

// --- Security review (2026-10-22): hostile peers driven packet by packet ----------------------

/// Records what a manager sends.
#[derive(Default)]
struct Capture {
    sent: Vec<(SocketAddr, Vec<u8>)>,
}

impl UtpIo for Capture {
    fn send(&mut self, to: SocketAddr, data: &[u8], _dont_fragment: bool) -> SendOutcome {
        self.sent.push((to, data.to_vec()));
        SendOutcome::Sent
    }
}

impl Capture {
    fn last(&self) -> Header {
        Header::parse(&self.sent.last().expect("something sent").1).expect("a uTP header")
    }
}

/// A peer we drive by hand.
const P: &str = "10.0.0.9:7000";
/// The connection id P's SYN carries: P receives on it, we on the next one.
const P_ID: u16 = 0x2000;
const P_SEQ: u16 = 500;
/// Our side's first sequence number (pinned).
const B_SEQ: u16 = 1000;

fn raw(type_: u8, extension: u8, connection_id: u16, seq_nr: u16, ack_nr: u16, rest: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; header::SIZE];
    Header {
        type_,
        version: 1,
        extension,
        connection_id,
        timestamp_microseconds: 1,
        timestamp_difference_microseconds: 0,
        wnd_size: 1 << 20,
        seq_nr,
        ack_nr,
    }
    .write(&mut b);
    b.extend_from_slice(rest);
    b
}

/// P's SYN.
fn syn() -> Vec<u8> {
    raw(header::ST_SYN, 0, P_ID, P_SEQ, 0, &[])
}

/// A data packet from P, acknowledging what our side sent before its first packet.
fn data(seq_nr: u16, payload: &[u8]) -> Vec<u8> {
    raw(header::ST_DATA, 0, P_ID.wrapping_add(1), seq_nr, B_SEQ.wrapping_sub(1), payload)
}

/// A manager that accepted a connection from P.
fn accept_raw(settings: UtpSettings) -> (UtpManager, Capture, SockId, Instant) {
    let now = Instant::now();
    let mut m = UtpManager::new(settings, now);
    let mut io = Capture::default();
    NEXT_SEQ_NR.with(|s| s.set(Some(B_SEQ)));
    assert!(m.incoming_packet(addr(P), &syn(), &mut io, now));
    m.socket_drained(&mut io, now);
    let accepted = m.take_accepted();
    assert_eq!(accepted.len(), 1);
    let h = io.last();
    assert_eq!((h.type_, h.connection_id, h.ack_nr), (header::ST_STATE, P_ID, P_SEQ));
    (m, io, accepted[0], now)
}

fn read_everything(m: &mut UtpManager, id: SockId) -> (Vec<u8>, ReadOutcome) {
    let mut out = Vec::new();
    loop {
        match m.read_some(id, &mut out, usize::MAX) {
            ReadOutcome::Data(_) => continue,
            other => return (out, other),
        }
    }
}

#[test]
fn one_byte_packets_take_one_byte_each_in_the_receive_buffer() {
    // Each received packet used to be kept as its own heap packet until read: 1-byte packets
    // filling the 1 MiB receive window cost about 100 bytes each, 100 MiB per connection.
    let (mut m, mut io, sb, now) = accept_raw(UtpSettings::default());
    for i in 1..=4000u16 {
        assert!(m.incoming_packet(addr(P), &data(P_SEQ + i, &[i as u8]), &mut io, now));
    }
    // empty data packets, out of order and then in order, take no room at all (libtorrent
    // drops them; a reordered one used to be queued for the reader)
    assert!(m.incoming_packet(addr(P), &data(P_SEQ + 4002, &[]), &mut io, now));
    assert!(m.incoming_packet(addr(P), &data(P_SEQ + 4001, &[]), &mut io, now));
    m.socket_drained(&mut io, now);
    assert_eq!(io.last().ack_nr, P_SEQ + 4002, "the empty packets are acknowledged");
    let allocated = m.get(sb).unwrap().receive_buffer_allocated();
    assert!((4000..=8192).contains(&allocated), "{allocated} bytes for 4000 received");
    let (got, end) = read_everything(&mut m, sb);
    assert!(matches!(end, ReadOutcome::WouldBlock));
    assert_eq!(got, (1..=4000u16).map(|i| i as u8).collect::<Vec<_>>());
}

#[test]
fn a_detached_socket_stores_nothing_and_goes_away_although_the_peer_keeps_sending() {
    // Once nothing of ours is in flight every packet restarts the retransmission timer, so a
    // peer that kept sending data without acknowledging our FIN kept a socket the engine had
    // let go alive for ever, buffering up to 1 MiB that nobody would read.
    let (mut m, mut io, sb, mut now) = accept_raw(UtpSettings::default());
    let mut seq = P_SEQ + 1;
    assert!(m.incoming_packet(addr(P), &data(seq, b"handshake"), &mut io, now));
    m.socket_drained(&mut io, now);
    m.detach(sb, &mut io, now);
    assert_eq!(m.get(sb).unwrap().state(), State::FinSent);
    assert_eq!(io.last().type_, header::ST_FIN);
    let mut gone_after = None;
    for second in 1..=70u64 {
        now += Duration::from_secs(1);
        seq += 1;
        m.incoming_packet(addr(P), &data(seq, &[7; 1000]), &mut io, now);
        m.socket_drained(&mut io, now);
        m.tick(&mut io, now);
        match m.get(sb) {
            Some(s) => {
                assert_eq!(s.receive_buffer_allocated(), 0, "second {second}");
                if second < 60 {
                    assert_eq!(s.state(), State::FinSent, "second {second}");
                }
            }
            None => {
                gone_after.get_or_insert(second);
            }
        }
    }
    let gone = gone_after.expect("the socket is deleted");
    assert!((60..=63).contains(&gone), "deleted after {gone} s");
    assert_eq!(m.num_sockets(), 0);
}

#[test]
fn a_selective_ack_on_a_syn_does_not_walk_the_sequence_space() {
    // A new socket has sent nothing (sequence number 0, acked 0); parsing a selective ACK
    // stepped its acked sequence number through all 65,536 values for every spoofed SYN.
    let mut sack = vec![header::EXT_NONE, 255];
    sack.extend_from_slice(&[0xff; 255]);
    let pkt = raw(header::ST_SYN, header::EXT_SACK, P_ID, P_SEQ, 0x1234, &sack);
    let now = Instant::now();
    let mut m = UtpManager::new(UtpSettings::default(), now);
    let mut io = Capture::default();
    ACK_STEPS.with(|c| c.set(0));
    assert!(m.incoming_packet(addr(P), &pkt, &mut io, now));
    let steps = ACK_STEPS.with(|c| c.get());
    assert!(steps < 100, "{steps} steps for one SYN");
    assert_eq!(m.take_accepted().len(), 1);
}

#[test]
fn a_syn_with_a_broken_extension_chain_leaves_no_socket() {
    // The extension parser returns early on a malformed chain; the SYN's socket was then
    // handed to the engine unconnected.
    let now = Instant::now();
    let mut m = UtpManager::new(UtpSettings::default(), now);
    let mut io = Capture::default();
    for rest in [&[][..], &[0][..], &[0, 10, 1, 2, 3][..]] {
        let pkt = raw(header::ST_SYN, header::EXT_SACK, P_ID, P_SEQ, 0, rest);
        assert!(!m.incoming_packet(addr(P), &pkt, &mut io, now));
        m.socket_drained(&mut io, now);
    }
    assert_eq!(m.num_sockets(), 0);
    assert!(m.take_accepted().is_empty());
    assert!(io.sent.is_empty(), "nothing is sent for them");
}

#[test]
fn a_syn_sent_again_is_acknowledged_again_without_a_second_socket() {
    // libtorrent makes a new socket, and a second incoming connection, for every copy of a SYN
    // whose SYN-ACK was lost (libutp resends its SYN).
    let (mut m, mut io, sb, now) = accept_raw(UtpSettings::default());
    let sent = io.sent.len();
    assert!(m.incoming_packet(addr(P), &syn(), &mut io, now));
    m.socket_drained(&mut io, now);
    assert_eq!(m.num_sockets(), 1);
    assert!(m.take_accepted().is_empty());
    assert_eq!(io.sent.len(), sent + 1);
    let h = io.last();
    assert_eq!((h.type_, h.connection_id, h.ack_nr), (header::ST_STATE, P_ID, P_SEQ));
    // once the peer sent anything else, a SYN with that id is ignored: nothing goes out
    assert!(m.incoming_packet(addr(P), &data(P_SEQ + 1, b"x"), &mut io, now));
    m.socket_drained(&mut io, now);
    let sent = io.sent.len();
    assert!(m.incoming_packet(addr(P), &syn(), &mut io, now));
    m.socket_drained(&mut io, now);
    assert_eq!(io.sent.len(), sent);
    assert_eq!(m.num_sockets(), 1);
    assert_eq!(m.get(sb).unwrap().state(), State::Connected);
    // the same SYN from another address is another connection
    let mut other = io;
    assert!(m.incoming_packet(addr("10.0.0.10:7000"), &syn(), &mut other, now));
    assert_eq!(m.take_accepted().len(), 1);
}

#[test]
fn an_unlimited_connections_limit_still_accepts_and_bounds_syns() {
    // A negative `connections_limit` is unlimited to the engine; here it refused every SYN.
    let settings = UtpSettings { connections_limit: -1, ..UtpSettings::default() };
    let (m, _, _, _) = accept_raw(settings);
    assert_eq!(m.num_sockets(), 1);
    // with a limit of 2, at most 5 sockets (more than twice the limit refuses)
    let now = Instant::now();
    let mut m = UtpManager::new(UtpSettings { connections_limit: 2, ..UtpSettings::default() }, now);
    let mut io = Capture::default();
    for i in 0..20u16 {
        let pkt = raw(header::ST_SYN, 0, i * 4, P_SEQ, 0, &[]);
        m.incoming_packet(addr(P), &pkt, &mut io, now);
        m.socket_drained(&mut io, now);
    }
    assert_eq!(m.num_sockets(), 5);
}

#[test]
fn sockets_are_found_by_id_after_their_slots_are_reused() {
    let mut sim = Sim::new();
    let (sa1, sb1) = sim.connect();
    let (sa2, sb2) = sim.connect();
    sim.detach(addr(A), sa1);
    sim.detach(addr(B), sb1);
    sim.steps(50);
    assert_eq!((sim.a.num_sockets(), sim.b.num_sockets()), (1, 1));
    let (sa3, sb3) = sim.connect();
    assert_eq!((sa3, sb3), (sa1, sb1), "the freed slots are reused");
    let data = payload(100_000);
    sim.transfer(addr(A), sa2, sb2, &data, 10_000);
    // (the accepting side learns the other end's window from its first packet after the SYN)
    sim.transfer(addr(A), sa3, sb3, &data, 10_000);
    sim.transfer(addr(B), sb3, sa3, &data, 10_000);
}

#[test]
fn no_more_than_a_quarter_of_the_sequence_space_is_in_flight() {
    // A receiver advertising a huge window, with a congestion window grown to match, while no
    // acknowledgement comes back: without a bound, sequence numbers in flight passed half the
    // 16-bit space and the wrapping comparisons turned around.
    let mut sim = Sim::new();
    let (sa, _sb) = sim.connect();
    sim.net.drop_from = Some(addr(B));
    sim.a.get_mut(sa).unwrap().force_windows(1 << 30, u32::MAX);
    let data = payload(32 << 20);
    let taken = sim.write(addr(A), sa, &data);
    let s = sim.a.get(sa).unwrap();
    assert_eq!(s.outstanding(), 0x4000);
    assert!(taken < data.len());
    // the window is shut until acknowledgements come
    assert_eq!(sim.write(addr(A), sa, &data[taken..]), 0);
}

#[test]
fn an_extreme_target_delay_and_gain_do_not_overflow_the_window_arithmetic() {
    // With a 1 µs target delay and the largest gain factor (both settings), a delay of a few
    // hundred milliseconds overflowed the 64-bit LEDBAT gain (a panic in a debug build).
    let mut sim =
        Sim::with_settings(UtpSettings { target_delay: 1, gain_factor: i32::MAX, ..UtpSettings::default() });
    let (sa, sb) = sim.connect();
    let data = payload(6_000_000);
    let (mut sent, mut got) = (0, Vec::new());
    for round in 0..300 {
        if sent < data.len() {
            sent += sim.write(addr(A), sa, &data[sent..]);
        }
        // the first rounds set the base delay; then every packet waits 200 ms more (a round
        // trip stays under the 500 ms minimum timeout, so the ACKs reach the controller)
        if round >= 4 {
            sim.now += Duration::from_millis(200);
        }
        sim.step();
        sim.read_all(addr(B), sb, &mut got);
        if got.len() == data.len() {
            break;
        }
    }
    assert!(got.len() > 10_000, "{} bytes", got.len());
    assert_eq!(&got[..], &data[..got.len()]);
    let s = sim.a.get(sa).unwrap();
    assert!(s.cwnd() >= s.mtu() as i64);
}
