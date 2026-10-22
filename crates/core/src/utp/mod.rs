// Ported from libtorrent-rasterbar 2.0.11: src/utp_socket_manager.cpp,
// include/libtorrent/aux_/utp_socket_manager.hpp, include/libtorrent/aux_/packet_pool.hpp (the
// `packet` struct and the MTU constants), the `utp_header` and `compare_less_wrap` of
// utp_stream.hpp / utp_stream.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: the manager owns its sockets in a slab and sends through a `UtpIo` trait the reactor
// implements, so the protocol runs without sockets or clocks (tests, fuzzing). Instead of asio
// handlers, sockets with news for the upper layer are collected in a ready list the engine
// drains after each wakeup. Packets are plain heap buffers (no `packet_pool`).
// Security review (2026-10-22, docs/SECURITY-REVIEW.md): sockets are found through an index by
// receive id (libtorrent's multimap) rather than a scan of the slab; a SYN sent again for a
// connection we accepted is acknowledged again instead of making a second socket; a SYN that
// does not leave its new socket connected (a malformed extension) leaves no socket; a negative
// `connections_limit` (unlimited in the engine) bounds the sockets at
// `2 * UNLIMITED_CONNECTIONS` instead of refusing every SYN.

//! uTP (BEP 29): reliable, LEDBAT-congestion-controlled streams over the UDP listen socket.
//!
//! [`UtpManager`] demultiplexes datagrams by connection id, accepts incoming connections and
//! ticks the sockets' timers. Each [`UtpSocket`] is libtorrent's `utp_socket_impl`: the
//! sequence numbers, the send and reorder buffers, selective ACKs, Nagle, path MTU discovery,
//! the delay histories and the congestion window. The reactor feeds datagrams in and provides a
//! [`UtpIo`] to send with; the peer connection reads and writes through the manager.

pub(crate) mod packet_buffer;
mod socket;
#[cfg(test)]
mod tests;
pub(crate) mod timestamp_history;

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::time::Instant;

use crate::settings::Settings;
use crate::util::random::random_below;
pub use socket::{ReadOutcome, State, UtpError, UtpSocket};

// libtorrent packet_pool.hpp
pub(crate) const IPV4_HEADER: u16 = 20;
pub(crate) const IPV6_HEADER: u16 = 40;
pub(crate) const UDP_HEADER: u16 = 8;
/// The SOCKS5 UDP header without its address (libtorrent `TORRENT_SOCKS5_HEADER`).
pub(crate) const SOCKS5_HEADER: u16 = 6;
pub(crate) const UTP_HEADER: u16 = 20;
pub(crate) const ETHERNET_MTU: u16 = 1500;
pub(crate) const TEREDO_MTU: u16 = 1280;
pub(crate) const INET_MIN_MTU: u16 = 576;

/// Index of a socket in the manager.
pub type SockId = usize;

/// The connection count the SYN-flood bound assumes when `connections_limit` is negative
/// (unlimited): at most twice this many sockets accept new SYNs.
const UNLIMITED_CONNECTIONS: i64 = 10_000;

/// How a datagram send went, as the UDP socket owner saw it.
#[derive(Debug)]
pub enum SendOutcome {
    Sent,
    /// The socket's send buffer is full (libtorrent `would_block` / `try_again`): the uTP
    /// socket stalls until [`UtpManager::writable`].
    WouldBlock,
    /// The datagram is larger than the interface or path MTU (`EMSGSIZE`, `WSAEMSGSIZE`): an
    /// MTU probe that was too big.
    MessageSize,
    Failed(io::ErrorKind),
}

/// The UDP sockets the manager sends through.
pub trait UtpIo {
    /// Sends one datagram. With `dont_fragment` the DF bit is set (IPv4 only) so an oversized
    /// MTU probe is dropped rather than fragmented.
    fn send(&mut self, to: SocketAddr, data: &[u8], dont_fragment: bool) -> SendOutcome;
}

/// The uTP part of `settings_pack` (libtorrent's defaults in `Default`).
#[derive(Clone, Debug, PartialEq)]
pub struct UtpSettings {
    /// Microseconds (libtorrent `utp_target_delay` is in milliseconds).
    pub target_delay: i32,
    pub gain_factor: i32,
    /// Milliseconds.
    pub min_timeout: i32,
    pub syn_resends: i32,
    pub fin_resends: i32,
    pub num_resends: i32,
    /// Milliseconds.
    pub connect_timeout: i32,
    /// Percent of the window kept on loss.
    pub loss_multiplier: i32,
    /// Milliseconds between window cuts.
    pub cwnd_reduce_timer: i32,
    pub enable_incoming: bool,
    pub connections_limit: i32,
    /// Datagrams go through a SOCKS5 proxy, whose UDP header takes room.
    pub socks5: bool,
}

impl Default for UtpSettings {
    fn default() -> UtpSettings {
        UtpSettings {
            target_delay: 100 * 1000,
            gain_factor: 3000,
            min_timeout: 500,
            syn_resends: 2,
            fin_resends: 2,
            num_resends: 3,
            connect_timeout: 3000,
            loss_multiplier: 50,
            cwnd_reduce_timer: 100,
            enable_incoming: true,
            connections_limit: 200,
            socks5: false,
        }
    }
}

impl From<&Settings> for UtpSettings {
    fn from(s: &Settings) -> UtpSettings {
        UtpSettings {
            target_delay: s.utp_target_delay.saturating_mul(1000),
            gain_factor: s.utp_gain_factor,
            min_timeout: s.utp_min_timeout,
            syn_resends: s.utp_syn_resends,
            fin_resends: s.utp_fin_resends,
            num_resends: s.utp_num_resends,
            connect_timeout: s.utp_connect_timeout,
            loss_multiplier: s.utp_loss_multiplier,
            cwnd_reduce_timer: s.utp_cwnd_reduce_timer,
            enable_incoming: s.enable_incoming_utp,
            connections_limit: s.connections_limit,
            socks5: s.proxy_type.is_socks5(),
        }
    }
}

/// libtorrent's `counters::utp_*` and `num_utp_*`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UtpCounters {
    pub packet_loss: u64,
    pub timeout: u64,
    pub packets_in: u64,
    pub packets_out: u64,
    pub fast_retransmit: u64,
    pub packet_resend: u64,
    pub samples_above_target: u64,
    pub samples_below_target: u64,
    pub payload_pkts_in: u64,
    pub payload_pkts_out: u64,
    pub invalid_pkts_in: u64,
    pub redundant_pkts_in: u64,
    /// Sockets per state, indexed by `State as usize`.
    pub num_by_state: [i32; 6],
}

/// A packet we sent and wait to have acknowledged, or one we received and hold for the
/// application (libtorrent `aux::packet`).
pub struct Packet {
    /// The last time this packet was sent.
    pub send_time: Instant,
    /// Bytes used in `buf`.
    pub size: u16,
    /// Offset of the payload in `buf`; for received packets also the cursor of what the
    /// application has consumed.
    pub header_size: u16,
    /// Times this packet has been sent.
    pub num_transmissions: u8,
    /// Must be sent again (every outstanding packet is marked on a timeout).
    pub need_resend: bool,
    /// Sent with the DF bit set.
    pub mtu_probe: bool,
    /// `buf.len()` is libtorrent's `allocated`.
    pub buf: Vec<u8>,
}

impl Packet {
    pub fn new(allocate: usize) -> Packet {
        Packet {
            // overwritten before use; `Instant::now()` would make this layer depend on a clock
            send_time: epoch(),
            size: 0,
            header_size: 0,
            num_transmissions: 0,
            need_resend: false,
            mtu_probe: false,
            buf: vec![0; allocate],
        }
    }

    pub fn allocated(&self) -> u16 {
        self.buf.len() as u16
    }

    pub fn payload_len(&self) -> i32 {
        self.size as i32 - self.header_size as i32
    }
}

/// A fixed point in time every `Packet::new` can use before its first send.
fn epoch() -> Instant {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<Instant> = OnceLock::new();
    *EPOCH.get_or_init(Instant::now)
}

/// Compares `lhs < rhs` on wrapping counters (sequence numbers under `mask`, 32-bit
/// timestamps under `0xffffffff`): the shorter way round decides.
pub fn compare_less_wrap(lhs: u32, rhs: u32, mask: u32) -> bool {
    // distance walking from lhs to rhs, downwards
    let dist_down = lhs.wrapping_sub(rhs) & mask;
    // distance walking from lhs to rhs, upwards
    let dist_up = rhs.wrapping_sub(lhs) & mask;
    // if the distance walking up is shorter, lhs is less than rhs. If the distance walking
    // down is shorter, then rhs is less than lhs
    dist_up < dist_down
}

/// The uTP header (BEP 29), 20 bytes:
///
/// ```text
/// 0       4       8               16              24              32
/// +-------+-------+---------------+---------------+---------------+
/// | type  | ver   | extension     | connection_id                 |
/// +-------+-------+---------------+---------------+---------------+
/// | timestamp_microseconds                                        |
/// +---------------+---------------+---------------+---------------+
/// | timestamp_difference_microseconds                             |
/// +---------------+---------------+---------------+---------------+
/// | wnd_size                                                      |
/// +---------------+---------------+---------------+---------------+
/// | seq_nr                        | ack_nr                        |
/// +---------------+---------------+---------------+---------------+
/// ```
pub mod header {
    pub const SIZE: usize = 20;

    // packet types (libtorrent `utp_socket_state_t`)
    pub const ST_DATA: u8 = 0;
    pub const ST_FIN: u8 = 1;
    pub const ST_STATE: u8 = 2;
    pub const ST_RESET: u8 = 3;
    pub const ST_SYN: u8 = 4;
    pub const NUM_TYPES: u8 = 5;

    // extension headers; 2 is skipped because a deprecated extension used it in the wild
    pub const EXT_NONE: u8 = 0;
    pub const EXT_SACK: u8 = 1;
    pub const EXT_CLOSE_REASON: u8 = 3;

    /// The fixed fields of an incoming packet.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Header {
        pub type_: u8,
        pub version: u8,
        pub extension: u8,
        pub connection_id: u16,
        pub timestamp_microseconds: u32,
        pub timestamp_difference_microseconds: u32,
        pub wnd_size: u32,
        pub seq_nr: u16,
        pub ack_nr: u16,
    }

    impl Header {
        pub fn parse(b: &[u8]) -> Option<Header> {
            if b.len() < SIZE {
                return None;
            }
            Some(Header {
                type_: b[0] >> 4,
                version: b[0] & 0xf,
                extension: b[1],
                connection_id: u16::from_be_bytes([b[2], b[3]]),
                timestamp_microseconds: u32::from_be_bytes([b[4], b[5], b[6], b[7]]),
                timestamp_difference_microseconds: u32::from_be_bytes([b[8], b[9], b[10], b[11]]),
                wnd_size: u32::from_be_bytes([b[12], b[13], b[14], b[15]]),
                seq_nr: u16::from_be_bytes([b[16], b[17]]),
                ack_nr: u16::from_be_bytes([b[18], b[19]]),
            })
        }

        pub fn write(&self, b: &mut [u8]) {
            b[0] = (self.type_ << 4) | (self.version & 0xf);
            b[1] = self.extension;
            b[2..4].copy_from_slice(&self.connection_id.to_be_bytes());
            b[4..8].copy_from_slice(&self.timestamp_microseconds.to_be_bytes());
            b[8..12].copy_from_slice(&self.timestamp_difference_microseconds.to_be_bytes());
            b[12..16].copy_from_slice(&self.wnd_size.to_be_bytes());
            b[16..18].copy_from_slice(&self.seq_nr.to_be_bytes());
            b[18..20].copy_from_slice(&self.ack_nr.to_be_bytes());
        }
    }

    // in-place field access on a stored packet
    pub fn set_type(b: &mut [u8], t: u8) {
        b[0] = (t << 4) | 1;
    }
    pub fn extension(b: &[u8]) -> u8 {
        b[1]
    }
    pub fn set_extension(b: &mut [u8], e: u8) {
        b[1] = e;
    }
    pub fn seq_nr(b: &[u8]) -> u16 {
        u16::from_be_bytes([b[16], b[17]])
    }
    pub fn ack_nr(b: &[u8]) -> u16 {
        u16::from_be_bytes([b[18], b[19]])
    }
    pub fn set_ack_nr(b: &mut [u8], v: u16) {
        b[18..20].copy_from_slice(&v.to_be_bytes());
    }
    pub fn set_timestamp(b: &mut [u8], v: u32) {
        b[4..8].copy_from_slice(&v.to_be_bytes());
    }
    pub fn set_timestamp_difference(b: &mut [u8], v: u32) {
        b[8..12].copy_from_slice(&v.to_be_bytes());
    }
    pub fn set_wnd_size(b: &mut [u8], v: u32) {
        b[12..16].copy_from_slice(&v.to_be_bytes());
    }
}

/// Teredo tunnels (2001:0::/32) carry a smaller MTU (libtorrent `is_teredo`).
fn is_teredo(addr: &IpAddr) -> bool {
    match addr {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            s[0] == 0x2001 && s[1] == 0
        }
        IpAddr::V4(_) => false,
    }
}

/// What a socket call needs from the manager: the settings, the counters, the way out and the
/// per-wakeup bookkeeping (libtorrent's `utp_socket_manager&` plus the clock).
pub struct Ctx<'a> {
    pub id: SockId,
    pub settings: &'a UtpSettings,
    pub counters: &'a mut UtpCounters,
    pub io: &'a mut dyn UtpIo,
    pub now: Instant,
    pub epoch: Instant,
    deferred_ack: &'a mut Option<SockId>,
    stalled: &'a mut Vec<SockId>,
    ready: &'a mut Vec<SockId>,
    restrict_mtu: &'a mut [u16; 3],
    mtu_idx: &'a mut usize,
    /// The application's bytes to send, and how far they were consumed (libtorrent's
    /// `m_write_buffer`, which lives only for the call here).
    pub wdata: &'a [u8],
    pub wpos: usize,
    /// Bytes copied into packets this call (libtorrent `m_written`).
    pub written: usize,
}

impl Ctx<'_> {
    /// Microseconds of `now` on the manager's clock, wrapped to 32 bits.
    pub fn micros(&self) -> u32 {
        micros(self.epoch, self.now)
    }

    pub fn write_buffer_size(&self) -> usize {
        self.wdata.len() - self.wpos
    }

    pub fn defer_ack(&mut self) {
        debug_assert!(self.deferred_ack.is_none() || *self.deferred_ack == Some(self.id));
        *self.deferred_ack = Some(self.id);
    }

    pub fn cancel_deferred_ack(&mut self) {
        if *self.deferred_ack == Some(self.id) {
            *self.deferred_ack = None;
        }
    }

    pub fn subscribe_writable(&mut self) {
        debug_assert!(!self.stalled.contains(&self.id));
        self.stalled.push(self.id);
    }

    pub fn mark_ready(&mut self) {
        if !self.ready.contains(&self.id) {
            self.ready.push(self.id);
        }
    }

    /// libtorrent `restrict_mtu(int)`: remembers a packet size the network silently dropped.
    pub fn restrict_mtu(&mut self, mtu: u16) {
        self.restrict_mtu[*self.mtu_idx] = mtu;
        *self.mtu_idx = (*self.mtu_idx + 1) % self.restrict_mtu.len();
    }
}

pub fn micros(epoch: Instant, now: Instant) -> u32 {
    now.saturating_duration_since(epoch).as_micros() as u32
}

/// libtorrent `utp_socket_manager`.
pub struct UtpManager {
    sockets: Vec<Option<Box<UtpSocket>>>,
    free: Vec<SockId>,
    /// Sockets by the connection id they receive on (libtorrent's `m_utp_sockets` multimap):
    /// a packet for a socket other than the last one is found without a scan of the slab.
    by_recv_id: HashMap<u16, Vec<SockId>>,
    settings: UtpSettings,
    counters: UtpCounters,
    /// This socket still needs to send an ACK. Once the UDP socket is drained, or a packet for
    /// another socket arrives, the ACK goes out.
    deferred_ack: Option<SockId>,
    /// Sockets that got `WouldBlock` from the UDP socket; told when it is writable again.
    stalled: Vec<SockId>,
    /// Sockets with news for the upper layer (received data, EOF, errors, a completed connect,
    /// room to write).
    ready: Vec<SockId>,
    /// The last socket a packet arrived for; most packets are for the same one.
    last_socket: Option<SockId>,
    /// Incoming connections not yet handed to the engine.
    accepted: Vec<SockId>,
    restrict_mtu: [u16; 3],
    mtu_idx: usize,
    epoch: Instant,
}

impl UtpManager {
    pub fn new(settings: UtpSettings, now: Instant) -> UtpManager {
        UtpManager {
            sockets: Vec::new(),
            free: Vec::new(),
            by_recv_id: HashMap::new(),
            settings,
            counters: UtpCounters::default(),
            deferred_ack: None,
            stalled: Vec::new(),
            ready: Vec::new(),
            last_socket: None,
            accepted: Vec::new(),
            restrict_mtu: [u16::MAX; 3],
            mtu_idx: 0,
            epoch: now,
        }
    }

    pub fn set_settings(&mut self, settings: UtpSettings) {
        self.settings = settings;
    }

    pub fn counters(&self) -> &UtpCounters {
        &self.counters
    }

    pub fn num_sockets(&self) -> usize {
        self.sockets.len() - self.free.len()
    }

    pub fn get(&self, id: SockId) -> Option<&UtpSocket> {
        self.sockets.get(id).and_then(|s| s.as_deref())
    }

    pub fn get_mut(&mut self, id: SockId) -> Option<&mut UtpSocket> {
        self.sockets.get_mut(id).and_then(|s| s.as_deref_mut())
    }

    /// Runs `f` on a socket with the manager's context. The socket is taken out of the slab
    /// for the call, so it must not look up other sockets.
    fn with_socket<R>(
        &mut self,
        id: SockId,
        io: &mut dyn UtpIo,
        now: Instant,
        wdata: &[u8],
        f: impl FnOnce(&mut UtpSocket, &mut Ctx<'_>) -> R,
    ) -> Option<(R, usize)> {
        let mut s = self.sockets.get_mut(id)?.take()?;
        let mut ctx = Ctx {
            id,
            settings: &self.settings,
            counters: &mut self.counters,
            io,
            now,
            epoch: self.epoch,
            deferred_ack: &mut self.deferred_ack,
            stalled: &mut self.stalled,
            ready: &mut self.ready,
            restrict_mtu: &mut self.restrict_mtu,
            mtu_idx: &mut self.mtu_idx,
            wdata,
            wpos: 0,
            written: 0,
        };
        let r = f(&mut s, &mut ctx);
        let written = ctx.written;
        self.sockets[id] = Some(s);
        Some((r, written))
    }

    fn insert(&mut self, s: UtpSocket) -> SockId {
        let recv_id = s.receive_id();
        let id = match self.free.pop() {
            Some(i) => {
                self.sockets[i] = Some(Box::new(s));
                i
            }
            None => {
                self.sockets.push(Some(Box::new(s)));
                self.sockets.len() - 1
            }
        };
        self.by_recv_id.entry(recv_id).or_default().push(id);
        id
    }

    /// The socket receiving on `id` from `from`.
    fn find(&self, from: &SocketAddr, id: u16) -> Option<SockId> {
        let ids = self.by_recv_id.get(&id)?;
        ids.iter().copied().find(|&i| self.get(i).is_some_and(|s| s.matches(from, id)))
    }

    /// libtorrent `remove_socket`.
    fn remove(&mut self, id: SockId) {
        if let Some(s) = self.sockets.get_mut(id).and_then(Option::take) {
            self.counters.num_by_state[s.state() as usize] -= 1;
            self.free.push(id);
            let recv_id = s.receive_id();
            if let Some(ids) = self.by_recv_id.get_mut(&recv_id) {
                ids.retain(|&i| i != id);
                if ids.is_empty() {
                    self.by_recv_id.remove(&recv_id);
                }
            }
        }
        if self.last_socket == Some(id) {
            self.last_socket = None;
        }
        if self.deferred_ack == Some(id) {
            self.deferred_ack = None;
        }
        self.stalled.retain(|&s| s != id);
        self.ready.retain(|&s| s != id);
        self.accepted.retain(|&s| s != id);
    }

    /// libtorrent `restrict_mtu()`: the largest size the network is known to pass.
    pub fn restricted_mtu(&self) -> u16 {
        *self.restrict_mtu.iter().max().expect("three entries")
    }

    /// libtorrent `mtu_for_dest`: the UDP payload that fits one link-layer frame. Through a
    /// SOCKS5 proxy (assumed to be reached over IPv4) its header and the address in it take room.
    pub fn mtu_for_dest(&self, addr: &IpAddr) -> u16 {
        let mut mtu = if is_teredo(addr) { TEREDO_MTU } else { ETHERNET_MTU };
        mtu -= UDP_HEADER;
        if self.settings.socks5 {
            mtu -= IPV4_HEADER + SOCKS5_HEADER;
            mtu -= if addr.is_ipv4() { 4 } else { 16 };
        } else {
            mtu -= if addr.is_ipv4() { IPV4_HEADER } else { IPV6_HEADER };
        }
        mtu.min(self.restricted_mtu())
    }

    /// Opens an outgoing connection to `to` (libtorrent `new_utp_socket` + `utp_stream::connect`):
    /// the SYN goes out now. `user` is the engine's handle for the socket (its peer slot).
    pub fn connect(&mut self, to: SocketAddr, user: usize, io: &mut dyn UtpIo, now: Instant) -> SockId {
        let send_id = random_below(0x1_0000) as u16;
        let recv_id = send_id.wrapping_sub(1);
        let mut s = UtpSocket::new(recv_id, send_id, now, &self.settings, &mut self.counters);
        s.user = user;
        let mtu = self.mtu_for_dest(&to.ip());
        let id = self.insert(s);
        let _ = self.with_socket(id, io, now, &[], |s, ctx| s.do_connect(ctx, to, mtu));
        id
    }

    /// Feeds a datagram from the UDP listen socket. Returns false if it is not a uTP packet
    /// (so the DHT or the trackers can have it).
    pub fn incoming_packet(
        &mut self,
        from: SocketAddr,
        buf: &[u8],
        io: &mut dyn UtpIo,
        now: Instant,
    ) -> bool {
        let Some(h) = header::Header::parse(buf) else { return false };
        if h.version != 1 {
            return false;
        }
        // parse out connection ID and look for existing connections. If found, forward to the
        // socket. First test to see if it's the same socket as last time: in most cases it is.
        let id = h.connection_id;
        if let Some(last) = self.last_socket {
            if self.get(last).is_some_and(|s| s.matches(&from, id)) {
                return self
                    .with_socket(last, io, now, &[], |s, ctx| s.incoming_packet(ctx, buf, from))
                    .is_some_and(|r| r.0);
            }
        }
        // We send the deferred ACK when the socket is drained as well, so as long as the
        // incoming packets go to the last socket we can defer the ACK more. However, if we
        // receive a packet for another socket, we have to trigger the ACK in case the new
        // socket also wants to defer an ACK.
        if let Some(d) = self.deferred_ack.take() {
            let _ = self.with_socket(d, io, now, &[], |s, ctx| s.send_deferred_ack(ctx));
        }
        if let Some(i) = self.find(&from, id) {
            let ret = self
                .with_socket(i, io, now, &[], |s, ctx| s.incoming_packet(ctx, buf, from))
                .is_some_and(|r| r.0);
            if ret {
                self.last_socket = Some(i);
            }
            return ret;
        }
        if !self.settings.enable_incoming {
            return false;
        }
        // if not found, see if it's a SYN packet, if it is, create a new socket
        if h.type_ == header::ST_SYN {
            // A SYN for a connection we accepted from this address: our SYN-ACK was lost and the
            // other end sends its SYN again. libtorrent makes a second socket (and a second
            // incoming connection) for it; this one is acknowledged again instead. Such a socket
            // sends with the SYN's id and receives on the next one; an outgoing socket of ours
            // has them the other way round, so it never matches.
            let again = self.by_recv_id.get(&id.wrapping_add(1)).and_then(|ids| {
                ids.iter().copied().find(|&i| self.get(i).is_some_and(|s| s.accepted_from(&from, id)))
            });
            if let Some(i) = again {
                let _ = self.with_socket(i, io, now, &[], |s, ctx| s.syn_again(ctx, &h));
                return true;
            }
            // possible SYN flood. Just ignore. A negative limit is unlimited to the engine; the
            // sockets stay bounded here.
            let limit = match self.settings.connections_limit {
                l if l < 0 => UNLIMITED_CONNECTIONS,
                l => l as i64,
            };
            if self.num_sockets() as i64 > limit * 2 {
                return false;
            }
            // create the new socket with this ID: the SYN carries the id we send with, and we
            // receive on the next one (libtorrent `m_new_connection`)
            let s = UtpSocket::new(id.wrapping_add(1), id, now, &self.settings, &mut self.counters);
            let mtu = self.mtu_for_dest(&from.ip());
            let i = self.insert(s);
            let ret = self
                .with_socket(i, io, now, &[], |s, ctx| {
                    s.init_mtu(mtu);
                    s.incoming_packet(ctx, buf, from)
                })
                .is_some_and(|r| r.0);
            // a SYN the socket rejected after all (a malformed extension chain returns early,
            // leaving the socket unconnected) leaves no socket: libtorrent hands such a socket
            // to the session in its `none` state
            if !ret || self.get(i).is_none_or(|s| s.state() != State::Connected) {
                self.remove(i);
                return false;
            }
            self.last_socket = Some(i);
            self.accepted.push(i);
            return true;
        }
        // an ST_RESET for an unknown connection is ignored, as is anything else
        false
    }

    /// Incoming connections since the last call (libtorrent's `incoming_utp_callback_t`).
    pub fn take_accepted(&mut self) -> Vec<SockId> {
        std::mem::take(&mut self.accepted)
    }

    /// Sockets with news for the upper layer since the last call, with their `user` handles.
    pub fn take_ready(&mut self) -> Vec<(SockId, usize)> {
        let ready = std::mem::take(&mut self.ready);
        ready.into_iter().filter_map(|id| self.get(id).map(|s| (id, s.user))).collect()
    }

    /// The UDP socket became writable again after a `WouldBlock` (libtorrent `writable`).
    pub fn writable(&mut self, io: &mut dyn UtpIo, now: Instant) {
        let stalled = std::mem::take(&mut self.stalled);
        for id in stalled {
            let _ = self.with_socket(id, io, now, &[], |s, ctx| s.writable(ctx));
        }
    }

    /// The UDP socket has no more datagrams this wakeup (libtorrent `socket_drained`): the
    /// deferred ACK goes out, so ACKs coalesce over the packets of one wakeup at least.
    pub fn socket_drained(&mut self, io: &mut dyn UtpIo, now: Instant) {
        if let Some(d) = self.deferred_ack.take() {
            let _ = self.with_socket(d, io, now, &[], |s, ctx| s.send_deferred_ack(ctx));
        }
    }

    /// libtorrent `utp_socket_manager::tick`: deletes finished sockets and runs the timers.
    pub fn tick(&mut self, io: &mut dyn UtpIo, now: Instant) {
        for id in 0..self.sockets.len() {
            let Some(s) = self.get(id) else { continue };
            if s.should_delete() {
                self.remove(id);
                continue;
            }
            let _ = self.with_socket(id, io, now, &[], |s, ctx| s.tick(ctx));
        }
    }

    pub fn set_user(&mut self, id: SockId, user: usize) {
        if let Some(s) = self.get_mut(id) {
            s.user = user;
        }
    }

    /// Copies up to `max` received bytes into `out` (libtorrent `utp_stream::read_some`).
    pub fn read_some(&mut self, id: SockId, out: &mut Vec<u8>, max: usize) -> ReadOutcome {
        match self.get_mut(id) {
            Some(s) => s.read_some(out, max),
            None => ReadOutcome::Error(UtpError::ConnectionAborted),
        }
    }

    /// Sends as much of `data` as the congestion window allows, returning how much was taken
    /// (libtorrent `utp_stream::write_some`, always non-blocking).
    pub fn write_some(
        &mut self,
        id: SockId,
        data: &[u8],
        io: &mut dyn UtpIo,
        now: Instant,
    ) -> Result<usize, UtpError> {
        match self.with_socket(id, io, now, data, |s, ctx| s.write_some(ctx)) {
            Some((Ok(()), written)) => Ok(written),
            Some((Err(e), _)) => Err(e),
            None => Err(UtpError::ConnectionAborted),
        }
    }

    /// The upper layer is done with the socket (libtorrent `utp_stream::~utp_stream`: `destroy`
    /// then `detach`): a connected socket sends its FIN and lives on until it is acknowledged;
    /// one that never connected, or already failed, is deleted at the next tick.
    pub fn detach(&mut self, id: SockId, io: &mut dyn UtpIo, now: Instant) {
        let _ = self.with_socket(id, io, now, &[], |s, ctx| s.destroy(ctx));
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;

    // libtorrent test/test_utp.cpp `compare_less_wrap`
    #[test]
    fn compare_less_wrap_cases() {
        assert!(compare_less_wrap(1, 2, 0xffff));
        assert!(!compare_less_wrap(2, 1, 0xffff));
        assert!(compare_less_wrap(100, 200, 0xffff));
        assert!(!compare_less_wrap(200, 100, 0xffff));
        assert!(compare_less_wrap(0xfff0, 0x000f, 0xffff)); // wrap
        assert!(!compare_less_wrap(0xfff0, 0xff00, 0xffff));
    }

    #[test]
    fn header_round_trip() {
        let h = header::Header {
            type_: header::ST_DATA,
            version: 1,
            extension: header::EXT_SACK,
            connection_id: 0xbeef,
            timestamp_microseconds: 0x0102_0304,
            timestamp_difference_microseconds: 0x0a0b_0c0d,
            wnd_size: 1_000_000,
            seq_nr: 0x1234,
            ack_nr: 0xfedc,
        };
        let mut b = [0u8; 20];
        h.write(&mut b);
        assert_eq!(header::Header::parse(&b), Some(h));
        assert_eq!(b[0], 0x01);
        assert_eq!(header::seq_nr(&b), 0x1234);
        header::set_ack_nr(&mut b, 7);
        assert_eq!(header::ack_nr(&b), 7);
        assert!(header::Header::parse(&b[..19]).is_none());
    }

    #[test]
    fn mtu_for_destinations() {
        let m = UtpManager::new(UtpSettings::default(), Instant::now());
        assert_eq!(m.mtu_for_dest(&"1.2.3.4".parse().unwrap()), 1472);
        assert_eq!(m.mtu_for_dest(&"2a00::1".parse().unwrap()), 1452);
        assert_eq!(m.mtu_for_dest(&"2001:0:4136:e378:8000:63bf:3fff:fdd2".parse().unwrap()), 1232);
    }
}
