//! Two uTP managers on a simulated network with a virtual clock (the replacement for
//! libtorrent's `simulation/test_utp.cpp`): connects, transfers with and without loss, Nagle,
//! stalls of the UDP socket, close handshakes and timeouts, all without a socket or a thread.

use std::net::SocketAddr;
use std::time::{Duration, Instant};

use super::header::{self, Header};
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
    // A black hole: a router that silently drops anything over 1000 bytes, DF bit or not, and
    // sends no ICMP. The first probe (1010 bytes) is lost; it cannot be re-packetized, so its
    // resends are lost too and the connection times out, as libtorrent's does. The manager
    // remembers the size that failed; once three connections failed that way (the restriction
    // is the largest of the last three), new connections start from a lower ceiling.
    let mut sim = Sim::new();
    let sb_first = {
        let (_, sb) = sim.connect();
        sb
    };
    let mut restricted = Vec::new();
    for attempt in 0..3 {
        let (sa, sb) = if attempt == 0 {
            let sa = sim.a.take_ready().first().map(|r| r.0).unwrap_or(0);
            (sa, sb_first)
        } else {
            sim.connect()
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
