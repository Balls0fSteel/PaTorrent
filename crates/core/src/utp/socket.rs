// Ported from libtorrent-rasterbar 2.0.11: src/utp_stream.cpp, include/libtorrent/aux_/utp_stream.hpp
// (`utp_socket_impl`)
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: no asio handlers. Received bytes always land in the socket's own receive buffer and
// the application copies them out (`read_some`); bytes to send are handed in per call and
// what the window does not take stays with the caller (`write_some`). Instead of read, write
// and connect callbacks the socket marks itself ready and the engine looks at it after the
// wakeup. The `close_reason` extension is parsed but never sent (we have no close reasons yet).
// An incoming packet restarts the retransmission timer only if it acknowledges something or
// nothing is in flight (libtorrent restarts it on every packet, so a window lost whole was never
// resent while the peer kept sending data of its own).

//! One uTP connection: libtorrent's `utp_socket_impl`.

use std::collections::VecDeque;
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr};
use std::time::{Duration, Instant};

use super::packet_buffer::PacketBuffer;
use super::timestamp_history::TimestampHistory;
use super::{
    Ctx, INET_MIN_MTU, IPV4_HEADER, Packet, SendOutcome, UDP_HEADER, UTP_HEADER, compare_less_wrap, header,
};
use crate::util::random::random_below;
use crate::util::stat::SlidingAverage;

const ACK_MASK: u32 = 0xffff;
/// A packet with more duplicate ACKs than this is resent at once.
const DUP_ACK_LIMIT: u8 = 3;

const PKT_ACK: u8 = 1;
const PKT_FIN: u8 = 2;

/// libtorrent `utp_socket_impl::state_t`. The order matters: states from `ErrorWait` on are
/// closing states that lead to the socket being deleted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    /// Not yet connected.
    None = 0,
    /// Sent a SYN, not received any ACK.
    SynSent = 1,
    /// SYN-ACK received; sending and receiving data.
    Connected = 2,
    /// FIN sent, but not every packet up to it has been acknowledged. We might still be waiting
    /// for a FIN from the other end.
    FinSent = 3,
    /// Gracefully disconnected, or failed; `error` says which. Waiting for the upper layer to
    /// learn about it (and detach) before the state is deleted.
    ErrorWait = 4,
    /// Nothing refers to this socket any more; the manager deletes it.
    Deleting = 5,
}

/// Why a socket closed (libtorrent reports these as asio error codes).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UtpError {
    ConnectionAborted,
    ConnectionReset,
    TimedOut,
    /// Closed cleanly: the other end's FIN was received, or ours acknowledged.
    Eof,
    Send(std::io::ErrorKind),
}

impl fmt::Display for UtpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UtpError::ConnectionAborted => f.write_str("connection aborted"),
            UtpError::ConnectionReset => f.write_str("connection reset by peer"),
            UtpError::TimedOut => f.write_str("timed out"),
            UtpError::Eof => f.write_str("end of file"),
            UtpError::Send(k) => write!(f, "send failed: {k:?}"),
        }
    }
}

pub enum ReadOutcome {
    /// Copied this many bytes (never zero).
    Data(usize),
    /// Nothing to read right now.
    WouldBlock,
    /// The other end closed its side and everything it sent was consumed.
    Eof,
    Error(UtpError),
}

pub struct UtpSocket {
    /// The engine's handle for this socket (its peer slot).
    pub user: usize,
    /// The upper layer still refers to this socket.
    attached: bool,
    /// A write returned less than offered: the caller waits for room.
    wants_write: bool,

    /// A packet held back by Nagle, accruing more bytes before it is sent.
    nagle_packet: Option<Box<Packet>>,
    /// Packets received and not yet read by the application.
    receive_buffer: VecDeque<Box<Packet>>,
    error: Option<UtpError>,
    remote: SocketAddr,
    /// Packets received out of order, by sequence number.
    inbuf: PacketBuffer,
    /// Packets sent and not yet acknowledged, by sequence number.
    outbuf: PacketBuffer,
    /// When the last packet we sent times out, re-sends included.
    timeout: Instant,
    /// The last time the timestamp history stepped a minute.
    last_history_step: Instant,
    /// The next time a lost packet may halve the window.
    next_loss: Option<Instant>,
    /// The max number of bytes in flight: fixed point, shift right 16 for bytes.
    cwnd: i64,
    delay_hist: TimestampHistory,
    their_delay_hist: TimestampHistory,
    /// The window size the last time slow start ended; the threshold to leave it earlier next
    /// time.
    ssthres: i32,
    /// Bytes buffered in `inbuf`.
    buffered_incoming_bytes: i32,
    /// The timestamp difference of the last packet received, sent back in our next one.
    reply_micro: u32,
    /// The other end's advertised receive window; never more than this is in flight. At zero,
    /// one packet a second probes until it opens up again.
    adv_wnd: u32,
    /// Un-acked bytes sent, not counting packets that failed to send or were lost.
    bytes_in_flight: i32,
    /// The sum of the packets in `receive_buffer`.
    receive_buffer_size: i32,
    /// Max bytes to hold for the application.
    receive_buffer_capacity: i32,
    /// The last three corrected delay measurements; the lowest feeds the congestion
    /// controller, so one outlier can't close the window.
    delay_sample_hist: [u32; 3],
    delay_sample_idx: usize,
    in_packets: u32,
    out_packets: u32,
    send_delay: i32,
    recv_delay: i32,
    rtt: SlidingAverage<16>,
    send_id: u16,
    recv_id: u16,
    /// Everything up to this sequence number was received (what we ACK).
    ack_nr: u16,
    /// The next sequence number we send; the FIN's once we are closing.
    seq_nr: u16,
    /// Everything up to this sequence number was acknowledged by the other end.
    acked_seq_nr: u16,
    /// Each packet gets one chance of a fast resend: the next one eligible.
    fast_resend_seq_nr: u16,
    /// The last undersized packet sent; at most one is in flight (Nagle).
    nagle_seq_nr: u16,
    /// The sequence number of the FIN we received (valid with `in_eof`).
    in_eof_seq_nr: u16,
    /// The lowest sequence number whose loss cuts the window again.
    loss_seq_nr: u16,
    /// The largest packet we send, header included (mid-point of the search).
    mtu: u16,
    /// The largest packet known to get through unfragmented.
    mtu_floor: u16,
    /// The largest packet that might get through; `mtu_ceiling + 1` most likely does not.
    mtu_ceiling: u16,
    /// The sequence number of the outstanding MTU probe, or 0.
    mtu_seq: u16,
    duplicate_acks: u8,
    /// Timeouts in a row.
    num_timeouts: u8,
    state: State,
    in_eof: bool,
    out_eof: bool,
    nagle: bool,
    slow_start: bool,
    cwnd_full: bool,
    deferred_ack: bool,
    stalled: bool,
    /// A valid packet arrived after the connection was established: the remote address is not
    /// spoofed.
    confirmed: bool,
}

/// Diagnostics for the tests.
#[cfg(test)]
impl UtpSocket {
    /// The window size in bytes.
    pub fn cwnd(&self) -> i64 {
        self.cwnd >> 16
    }

    pub fn mtu(&self) -> u16 {
        self.mtu
    }
}

impl UtpSocket {
    /// The connection id this socket receives on.
    #[cfg(any(test, feature = "fuzzing"))]
    pub fn receive_id(&self) -> u16 {
        self.recv_id
    }

    pub(super) fn new(
        recv_id: u16,
        send_id: u16,
        now: Instant,
        settings: &super::UtpSettings,
        counters: &mut super::UtpCounters,
    ) -> UtpSocket {
        debug_assert!(recv_id == send_id.wrapping_add(1) || send_id == recv_id.wrapping_add(1));
        counters.num_by_state[State::None as usize] += 1;
        UtpSocket {
            user: 0,
            attached: true,
            wants_write: false,
            nagle_packet: None,
            receive_buffer: VecDeque::new(),
            error: None,
            remote: SocketAddr::new(Ipv4Addr::UNSPECIFIED.into(), 0),
            inbuf: PacketBuffer::default(),
            outbuf: PacketBuffer::default(),
            timeout: now + Duration::from_millis(settings.connect_timeout.max(0) as u64),
            last_history_step: now,
            next_loss: None,
            cwnd: (super::ETHERNET_MTU as i64) << 16,
            delay_hist: TimestampHistory::default(),
            their_delay_hist: TimestampHistory::default(),
            ssthres: 0,
            buffered_incoming_bytes: 0,
            reply_micro: 0,
            adv_wnd: super::ETHERNET_MTU as u32,
            bytes_in_flight: 0,
            receive_buffer_size: 0,
            receive_buffer_capacity: 1024 * 1024,
            delay_sample_hist: [u32::MAX; 3],
            delay_sample_idx: 0,
            in_packets: 0,
            out_packets: 0,
            send_delay: 0,
            recv_delay: 0,
            rtt: SlidingAverage::default(),
            send_id,
            recv_id,
            ack_nr: 0,
            seq_nr: 0,
            acked_seq_nr: 0,
            fast_resend_seq_nr: 0,
            nagle_seq_nr: 0,
            in_eof_seq_nr: 0,
            loss_seq_nr: 0,
            mtu: super::ETHERNET_MTU - IPV4_HEADER - UDP_HEADER - 8 - 24 - 36,
            mtu_floor: INET_MIN_MTU - IPV4_HEADER - UDP_HEADER,
            mtu_ceiling: super::ETHERNET_MTU - IPV4_HEADER - UDP_HEADER,
            mtu_seq: 0,
            duplicate_acks: 0,
            num_timeouts: 0,
            state: State::None,
            in_eof: false,
            out_eof: false,
            nagle: true,
            slow_start: true,
            cwnd_full: false,
            deferred_ack: false,
            stalled: false,
            confirmed: false,
        }
    }

    // --- Accessors ----------------------------------------------------------------------------

    pub fn state(&self) -> State {
        self.state
    }

    pub fn error(&self) -> Option<UtpError> {
        self.error
    }

    pub fn remote(&self) -> SocketAddr {
        self.remote
    }

    pub fn matches(&self, ep: &SocketAddr, id: u16) -> bool {
        self.recv_id == id && self.remote == *ep
    }

    /// Deletable from the manager's point of view: the upper layer let go and the other end no
    /// longer expects the socket to be alive. While stalled the manager still points at it
    /// from its writable queue, so it waits for that.
    pub fn should_delete(&self) -> bool {
        (self.state >= State::ErrorWait || self.state == State::None) && !self.attached && !self.stalled
    }

    fn set_state(&mut self, ctx: &mut Ctx<'_>, s: State) {
        if s == self.state {
            return;
        }
        ctx.counters.num_by_state[self.state as usize] -= 1;
        self.state = s;
        ctx.counters.num_by_state[self.state as usize] += 1;
    }

    /// Something the upper layer should look at happened (data, EOF, an error, a completed
    /// connect, room to write).
    fn mark_ready(&mut self, ctx: &mut Ctx<'_>) {
        ctx.mark_ready();
    }

    fn can_write(&self) -> bool {
        self.state == State::Connected
            && !self.stalled
            && !self.out_eof
            && (self.bytes_in_flight == 0
                || (self.bytes_in_flight as i64 + self.mtu as i64)
                    <= (self.cwnd >> 16).min(self.adv_wnd as i64))
    }

    /// Wakes a writer that was told to wait, once the window has room again.
    fn maybe_wake_writer(&mut self, ctx: &mut Ctx<'_>) {
        if self.wants_write && (self.can_write() || self.error.is_some()) {
            self.wants_write = false;
            self.mark_ready(ctx);
        }
    }

    // --- Application side ---------------------------------------------------------------------

    /// libtorrent `utp_socket_impl::read_some`: moves received bytes to the application.
    pub fn read_some(&mut self, out: &mut Vec<u8>, max: usize) -> ReadOutcome {
        if let Some(e) = self.error {
            if self.receive_buffer_size <= 0 {
                return if e == UtpError::Eof { ReadOutcome::Eof } else { ReadOutcome::Error(e) };
            }
        }
        let mut total = 0usize;
        while total < max {
            let Some(p) = self.receive_buffer.front_mut() else { break };
            let hs = p.header_size as usize;
            let avail = p.size as usize - hs;
            let n = avail.min(max - total);
            out.extend_from_slice(&p.buf[hs..hs + n]);
            p.header_size += n as u16;
            self.receive_buffer_size -= n as i32;
            total += n;
            if p.header_size == p.size {
                self.receive_buffer.pop_front();
            }
        }
        debug_assert!(self.receive_buffer_size >= 0);
        if total > 0 {
            return ReadOutcome::Data(total);
        }
        // if we have passed all data up to the application, and we're at EOF, say so
        if self.receive_buffer.is_empty() && self.in_eof && self.in_eof_seq_nr == self.ack_nr {
            return ReadOutcome::Eof;
        }
        match self.error {
            Some(UtpError::Eof) => ReadOutcome::Eof,
            Some(e) => ReadOutcome::Error(e),
            None => ReadOutcome::WouldBlock,
        }
    }

    /// libtorrent `utp_socket_impl::write_some`: sends what the window allows of `ctx.wdata`.
    /// `ctx.written` says how much was taken.
    pub fn write_some(&mut self, ctx: &mut Ctx<'_>) -> Result<(), UtpError> {
        if let Some(e) = self.error {
            return Err(e);
        }
        if self.out_eof || self.state == State::FinSent {
            // we can't send more data after closing the socket (libtorrent `broken_pipe`)
            return Err(UtpError::Eof);
        }
        if self.state != State::Connected {
            self.wants_write = true;
            return Ok(());
        }
        // try to write if the congestion window allows it
        while self.send_pkt(ctx, 0) {}
        if let Some(e) = self.error {
            return Err(e);
        }
        if ctx.write_buffer_size() > 0 {
            self.wants_write = true;
        }
        Ok(())
    }

    /// libtorrent `utp_stream::do_connect`.
    pub(super) fn do_connect(&mut self, ctx: &mut Ctx<'_>, ep: SocketAddr, mtu: u16) {
        self.init_mtu(mtu);
        self.remote = ep;
        if self.test_socket_state(ctx) {
            return;
        }
        self.send_syn(ctx);
    }

    /// libtorrent `utp_socket_impl::close`: no more data will be written; the FIN goes out once
    /// everything queued is sent.
    fn close(&mut self, ctx: &mut Ctx<'_>) {
        // the FIN packet is supposed to be last, so if we still have user data waiting to be
        // sent, we have to defer sending the FIN
        self.out_eof = true;
        if self.nagle_packet.is_none()
            && ctx.write_buffer_size() == 0
            && self.state == State::Connected
            && self.outbuf.at(self.seq_nr as u32).is_none()
        {
            self.send_fin(ctx);
        }
    }

    /// libtorrent `destroy` + `detach`: the upper layer is gone.
    pub(super) fn destroy(&mut self, ctx: &mut Ctx<'_>) {
        if !self.attached {
            return;
        }
        self.close(ctx);
        self.attached = false;
        self.wants_write = false;
        if matches!(self.state, State::ErrorWait | State::None | State::SynSent) {
            self.set_state(ctx, State::Deleting);
        }
    }

    fn fail(&mut self, ctx: &mut Ctx<'_>, e: UtpError) {
        self.error = Some(e);
        self.set_state(ctx, State::ErrorWait);
        self.test_socket_state(ctx);
    }

    /// libtorrent `test_socket_state`: in an error state, tell the upper layer and, once it
    /// has let go, move on to deletion. Returns true if the socket is closed.
    fn test_socket_state(&mut self, ctx: &mut Ctx<'_>) -> bool {
        if self.error.is_none() {
            return false;
        }
        debug_assert!(matches!(self.state, State::ErrorWait | State::Deleting));
        self.wants_write = false;
        if self.attached {
            self.mark_ready(ctx);
        } else if self.state == State::ErrorWait {
            self.set_state(ctx, State::Deleting);
        }
        true
    }

    // --- MTU ----------------------------------------------------------------------------------

    pub(super) fn init_mtu(&mut self, mtu: u16) {
        // set the ceiling to what we found out from the interface
        self.mtu_ceiling = mtu;
        // start in the middle of the PMTU search space
        self.mtu = (self.mtu_ceiling + self.mtu_floor) / 2;
        if self.mtu > self.mtu_ceiling {
            self.mtu = self.mtu_ceiling;
        }
        if self.mtu_floor > mtu {
            self.mtu_floor = mtu;
        }
        // if the window size is smaller than one packet size, set it to one
        if (self.cwnd >> 16) < self.mtu as i64 {
            self.cwnd = (self.mtu as i64) << 16;
        }
    }

    fn update_mtu_limits(&mut self) {
        if self.mtu_floor > self.mtu_ceiling {
            // This is the case where we drop an MTU probe once we're in steady state. Assume
            // the probe was lost by chance, and don't decrement the ceiling. We're still
            // restarting the Path MTU discovery, so if the MTU did in fact change, we'll be
            // notified again, when not in steady state.
            self.mtu_ceiling = self.mtu_floor;
            // the path MTU may have changed. Perform another search; don't start all the way
            // from start, just half way down.
            self.mtu_floor = ((INET_MIN_MTU - IPV4_HEADER - UDP_HEADER) + self.mtu_ceiling) / 2;
        }
        self.mtu = (self.mtu_floor + self.mtu_ceiling) / 2;
        if (self.cwnd >> 16) < self.mtu as i64 {
            self.cwnd = (self.mtu as i64) << 16;
        }
        // clear the mtu probe sequence number since it was either dropped or acked
        self.mtu_seq = 0;
    }

    // --- Sending ------------------------------------------------------------------------------

    fn send_syn(&mut self, ctx: &mut Ctx<'_>) {
        self.seq_nr = random_below(0x1_0000) as u16;
        self.acked_seq_nr = self.seq_nr.wrapping_sub(1);
        self.loss_seq_nr = self.acked_seq_nr;
        self.ack_nr = 0;
        self.fast_resend_seq_nr = self.seq_nr;

        let mut p = Box::new(Packet::new(header::SIZE));
        p.size = UTP_HEADER;
        p.header_size = UTP_HEADER;
        let h = header::Header {
            type_: header::ST_SYN,
            version: 1,
            extension: header::EXT_NONE,
            // using recv_id here is intentional! This is an odd thing in uTP. The syn packet is
            // sent with the connection ID that it expects to receive the syn ack on. All
            // subsequent connection IDs will be this plus one.
            connection_id: self.recv_id,
            timestamp_microseconds: ctx.micros(),
            timestamp_difference_microseconds: self.reply_micro,
            wnd_size: 0,
            seq_nr: self.seq_nr,
            ack_nr: 0,
        };
        h.write(&mut p.buf[..header::SIZE]);
        p.send_time = ctx.now;
        match ctx.io.send(self.remote, &p.buf[..header::SIZE], false) {
            SendOutcome::WouldBlock => {
                if !self.stalled {
                    self.stalled = true;
                    ctx.subscribe_writable();
                }
                p.need_resend = true;
            }
            SendOutcome::MessageSize => {
                self.fail(ctx, UtpError::Send(std::io::ErrorKind::InvalidInput));
                return;
            }
            SendOutcome::Failed(kind) => {
                self.fail(ctx, UtpError::Send(kind));
                return;
            }
            SendOutcome::Sent => p.num_transmissions += 1,
        }
        debug_assert!(self.outbuf.at(self.seq_nr as u32).is_none());
        self.outbuf.insert(self.seq_nr as u32, p);
        self.seq_nr = self.seq_nr.wrapping_add(1);
        debug_assert!(self.error.is_none());
        self.set_state(ctx, State::SynSent);
    }

    /// The UDP socket is writable again after a stall (libtorrent `writable`).
    pub(super) fn writable(&mut self, ctx: &mut Ctx<'_>) {
        debug_assert!(self.stalled);
        self.stalled = false;
        if self.should_delete() {
            return;
        }
        // if the socket stalled while sending an ack then there will be a pending deferred
        // ack. make sure it gets sent out
        if !self.deferred_ack || self.send_pkt(ctx, PKT_ACK) {
            while self.send_pkt(ctx, 0) {}
        }
        // try to flush the last part of the send buffer before sending a FIN
        if self.out_eof && self.nagle_packet.is_some() {
            self.send_pkt(ctx, 0);
        }
        // this handles the case where send_fin() was called while stalled
        if self.out_eof
            && self.nagle_packet.is_none()
            && ctx.write_buffer_size() == 0
            && self.state == State::Connected
            && self.outbuf.at(self.seq_nr as u32).is_none()
        {
            self.send_fin(ctx);
        }
        self.maybe_wake_writer(ctx);
    }

    fn send_fin(&mut self, ctx: &mut Ctx<'_>) {
        debug_assert!(ctx.write_buffer_size() == 0);
        debug_assert!(self.out_eof);
        debug_assert!(self.nagle_packet.is_none());
        self.send_pkt(ctx, PKT_FIN);
        // unless there was an error, we're now in FIN-SENT state
        if self.error.is_none() {
            self.set_state(ctx, State::FinSent);
        } else {
            self.set_state(ctx, State::ErrorWait);
            self.test_socket_state(ctx);
        }
    }

    fn send_reset(&mut self, ctx: &mut Ctx<'_>, ack_nr: u16) {
        let h = header::Header {
            type_: header::ST_RESET,
            version: 1,
            extension: header::EXT_NONE,
            connection_id: self.send_id,
            timestamp_microseconds: ctx.micros(),
            timestamp_difference_microseconds: self.reply_micro,
            wnd_size: 0,
            seq_nr: random_below(0x1_0000) as u16,
            ack_nr,
        };
        let mut b = [0u8; header::SIZE];
        h.write(&mut b);
        // ignore errors here
        let _ = ctx.io.send(self.remote, &b, false);
    }

    pub(super) fn send_deferred_ack(&mut self, ctx: &mut Ctx<'_>) {
        debug_assert!(self.deferred_ack);
        if !self.deferred_ack {
            return;
        }
        self.deferred_ack = false;
        self.send_pkt(ctx, PKT_ACK);
    }

    fn defer_ack(&mut self, ctx: &mut Ctx<'_>) {
        if self.deferred_ack {
            return;
        }
        self.deferred_ack = true;
        ctx.defer_ack();
    }

    /// Copies `dst.len()` bytes of the application's data into a packet.
    fn write_payload(ctx: &mut Ctx<'_>, dst: &mut [u8]) {
        let n = dst.len();
        debug_assert!(ctx.write_buffer_size() >= n);
        dst.copy_from_slice(&ctx.wdata[ctx.wpos..ctx.wpos + n]);
        ctx.wpos += n;
        ctx.written += n;
    }

    /// The selective-ACK bitfield over the reorder buffer, `size` bytes.
    fn write_sack(&self, buf: &mut [u8]) {
        debug_assert!(!self.inbuf.is_empty());
        let mut ack_nr = (self.ack_nr as u32 + 2) & ACK_MASK;
        for b in buf.iter_mut() {
            *b = 0;
            for i in 0..8 {
                if self.inbuf.at(ack_nr).is_some() {
                    *b |= 1 << i;
                }
                ack_nr = (ack_nr + 1) & ACK_MASK;
            }
        }
    }

    fn remove_sack_header(p: &mut Packet) {
        debug_assert_eq!(header::extension(&p.buf), header::EXT_SACK);
        let next_ext = p.buf[header::SIZE];
        let sack_size = p.buf[header::SIZE + 1] as usize;
        header::set_extension(&mut p.buf, next_ext);
        debug_assert!(next_ext == header::EXT_NONE || next_ext == header::EXT_CLOSE_REASON);
        let removed = sack_size + 2;
        debug_assert!(p.size >= p.header_size);
        debug_assert!(p.header_size as usize >= header::SIZE + removed);
        p.buf.copy_within(header::SIZE + removed..p.size as usize, header::SIZE);
        p.header_size -= removed as u16;
        p.size -= removed as u16;
    }

    /// Sends a packet, pulling payload from `ctx.wdata` (if any). With `PKT_ACK` a packet goes
    /// out regardless of payload; `PKT_FIN` makes it a FIN. Returns true if there is more room
    /// for payload in the window (call again), false if not. `seq_nr` only advances for packets
    /// with payload: not for ST_STATE or ST_FIN.
    fn send_pkt(&mut self, ctx: &mut Ctx<'_>, flags: u8) -> bool {
        if self.stalled {
            if flags & PKT_ACK != 0 {
                self.defer_ack(ctx);
            }
            return false;
        }
        // out_eof means we're trying to close the write side of this socket, we need to flush
        // all payload before we can send the FIN packet, so don't store any payload in the
        // nagle packet
        let force = flags & PKT_ACK != 0 || flags & PKT_FIN != 0 || self.out_eof;
        // when we want to close the outgoing stream, we need to send the remaining nagle
        // packet even though it won't fill a packet.
        let force_flush_nagle = self.out_eof && ctx.write_buffer_size() > 0;

        // first see if we need to resend any packets
        let mut i = (self.acked_seq_nr as u32 + 1) & ACK_MASK;
        while i != self.seq_nr as u32 {
            let needs = self.outbuf.at(i).is_some_and(|p| p.need_resend);
            if needs {
                if !self.resend_packet(ctx, i, false) {
                    // we couldn't resend the packet. It probably doesn't fit in our cwnd. If
                    // force is set, we need to continue to send our packet anyway, if we don't
                    // have force set, we might as well return
                    if !force {
                        return false;
                    }
                    // resend_packet might have failed
                    if matches!(self.state, State::ErrorWait | State::Deleting) || self.stalled {
                        return false;
                    }
                    break;
                }
                // don't fast-resend this packet
                if self.fast_resend_seq_nr as u32 == i {
                    self.fast_resend_seq_nr = self.fast_resend_seq_nr.wrapping_add(1);
                }
            }
            i = (i + 1) & ACK_MASK;
        }

        // MTU DISCOVERY: under these conditions, the next packet we send should be an MTU probe.
        // MTU probes get to use the mid-point packet size, whereas other packets use a
        // conservative packet size of the largest known to work. The reason for the cwnd
        // condition is to make sure the probe is surrounded by non-probes, to be able to
        // distinguish a loss of the probe vs. just loss in general.
        let mtu_probe =
            self.mtu_seq == 0 && self.seq_nr != 0 && (self.cwnd >> 16) > self.mtu_floor as i64 * 3;
        // for non MTU-probes, use the conservative packet size
        let effective_mtu = (if mtu_probe { self.mtu } else { self.mtu_floor }) as usize;

        let mut sack = 0usize;
        if !self.inbuf.is_empty() {
            let max_sack_size = effective_mtu - header::SIZE - 2; // 2 for sack padding/header
            // the SACK bitfield should ideally fit all the pieces we have successfully received
            sack = (self.inbuf.span() as usize).div_ceil(8).min(max_sack_size);
        }
        let header_size = header::SIZE + if sack > 0 { sack + 2 } else { 0 };

        // once we're in fin-sent mode, the write buffer should not be re-filled although, we
        // may re-send packets, but those live in outbuf
        debug_assert!(self.state != State::FinSent || ctx.write_buffer_size() == 0);

        let nagle_size = self.nagle_packet.as_ref().map_or(0, |p| p.payload_len() as usize);
        let mut payload_size = (ctx.write_buffer_size() + nagle_size).min(effective_mtu - header_size);
        // we cannot include any payload in FIN packets
        debug_assert!(flags & PKT_FIN == 0 || payload_size == 0);

        // if we have one MSS worth of data, make sure it fits in our congestion window and the
        // advertised receive window from the other end.
        if self.bytes_in_flight as i64 + payload_size as i64 > (self.cwnd >> 16).min(self.adv_wnd as i64) {
            // we can't fit a full packet of payload in the cwnd, but if we're sending an ACK,
            // we can send a packet without payload
            if flags & PKT_ACK != 0 {
                payload_size = 0;
            }
            // we're constrained by the window size
            self.cwnd_full = true;
            if !force {
                return false;
            }
        }

        // if we don't have any data to send, or can't send any data and we don't have any
        // data to force, don't send a packet
        if payload_size == 0 && !force {
            return false;
        }

        let mut p: Box<Packet>;
        // payload size being zero means we're just sending a force. For efficiency, pick up the
        // nagle packet if there's room. Note that if there is a nagle packet, payload_size will
        // include its size, so we won't take the first branch here
        if self.nagle_packet.is_none() || (payload_size == 0 && force && self.cwnd_full) {
            debug_assert!(!(force_flush_nagle && self.nagle_packet.is_some()));
            p = Box::new(Packet::new(effective_mtu));
            if payload_size > 0 {
                ctx.counters.payload_pkts_out += 1;
            }
            p.size = (header_size + payload_size) as u16;
            p.header_size = header_size as u16;
            let h = header::Header {
                type_: if payload_size > 0 { header::ST_DATA } else { header::ST_STATE },
                version: 1,
                extension: if sack > 0 { header::EXT_SACK } else { header::EXT_NONE },
                connection_id: self.send_id,
                timestamp_microseconds: 0,
                timestamp_difference_microseconds: 0,
                wnd_size: 0,
                // seq_nr is ignored for ST_STATE packets, so it doesn't matter that we say this
                // is a sequence number we haven't actually sent yet
                seq_nr: self.seq_nr,
                ack_nr: 0,
            };
            h.write(&mut p.buf[..header::SIZE]);
            Self::write_payload(ctx, &mut p.buf[header_size..header_size + payload_size]);
        } else {
            debug_assert!(self.state != State::FinSent);
            // pick up the nagle packet and keep adding bytes to it
            p = self.nagle_packet.take().expect("checked");
            debug_assert_eq!(header::seq_nr(&p.buf), self.seq_nr);
            // if the packet has a selective ack header, we'll need to update it
            if header::extension(&p.buf) == header::EXT_SACK {
                sack = p.buf[header::SIZE + 1] as usize;
                // if we no longer have any out-of-order packets waiting to be delivered,
                // there's no selective ack to be sent.
                if self.inbuf.is_empty() {
                    Self::remove_sack_header(&mut p);
                    sack = 0;
                }
            } else {
                sack = 0;
            }
            let size_left = (p.allocated() as usize - p.size as usize)
                .min(ctx.write_buffer_size())
                .min(effective_mtu.saturating_sub(p.size as usize));
            if size_left > 0 {
                let start = p.size as usize;
                Self::write_payload(ctx, &mut p.buf[start..start + size_left]);
                p.size += size_left as u16;
            }
            // did we fill up the whole mtu? if we didn't, we may still send it if there's no
            // undersized packet currently in flight
            if self.bytes_in_flight > 0
                && (p.size as usize) < (p.allocated() as usize).min(effective_mtu)
                && !force
                && self.nagle
                && compare_less_wrap(self.acked_seq_nr as u32, self.nagle_seq_nr as u32, ACK_MASK)
            {
                // the packet is still not a full MSS, so put it back into the nagle packet
                self.nagle_packet = Some(p);
                return false;
            }
        }

        if sack > 0 {
            let at = header::SIZE;
            p.buf[at] = header::EXT_NONE;
            p.buf[at + 1] = sack as u8; // bytes for SACK bitfield
            let mut bits = vec![0u8; sack];
            self.write_sack(&mut bits);
            p.buf[at + 2..at + 2 + sack].copy_from_slice(&bits);
            debug_assert!(at + 2 + sack <= p.header_size as usize);
        }

        if self.bytes_in_flight > 0
            && (p.size as usize) < (p.allocated() as usize).min(effective_mtu)
            && !force_flush_nagle
            && !force
            && self.nagle
            && compare_less_wrap(self.acked_seq_nr as u32, self.nagle_seq_nr as u32, ACK_MASK)
        {
            // this is nagle. If we don't have a full packet worth of payload to send AND we
            // have at least one outstanding undersized packet, hold off. Once the outstanding
            // packet is acked, we'll send this payload
            debug_assert!(self.nagle_packet.is_none());
            debug_assert_eq!(header::seq_nr(&p.buf), self.seq_nr);
            self.nagle_packet = Some(p);
            return false;
        }

        // for ST_STATE packets, payload size is 0. Such packets do not have unique sequence
        // numbers and should never be used as mtu probes
        if (mtu_probe || p.mtu_probe) && p.size >= self.mtu_floor {
            p.mtu_probe = true;
            self.mtu_seq = self.seq_nr;
        } else {
            p.mtu_probe = false;
        }

        header::set_timestamp_difference(&mut p.buf, self.reply_micro);
        let wnd =
            (self.receive_buffer_capacity - self.buffered_incoming_bytes - self.receive_buffer_size).max(0);
        header::set_wnd_size(&mut p.buf, wnd as u32);
        header::set_ack_nr(&mut p.buf, self.ack_nr);
        // if this is a FIN packet, override the type
        if flags & PKT_FIN != 0 {
            header::set_type(&mut p.buf, header::ST_FIN);
        }
        // fill in the timestamp as late as possible
        p.send_time = ctx.now;
        header::set_timestamp(&mut p.buf, ctx.micros());

        let mut outcome = ctx.io.send(self.remote, &p.buf[..p.size as usize], p.mtu_probe);
        if matches!(outcome, SendOutcome::MessageSize) {
            // if we fail even though this is not a probe, we're screwed since we'd have to
            // repacketize
            debug_assert!(p.mtu_probe);
            self.mtu_ceiling = p.size - 1;
            self.update_mtu_limits();
            // resend the packet immediately without it being an MTU probe
            p.mtu_probe = false;
            self.mtu_seq = 0;
            outcome = ctx.io.send(self.remote, &p.buf[..p.size as usize], false);
        }
        match outcome {
            SendOutcome::WouldBlock => {
                debug_assert!(!self.stalled);
                self.stalled = true;
                ctx.subscribe_writable();
                // If this is an ack then defer it to when the socket becomes writable again
                if p.size == p.header_size && flags & PKT_ACK != 0 {
                    self.defer_ack(ctx);
                }
            }
            SendOutcome::MessageSize | SendOutcome::Failed(_) => {
                let kind = match outcome {
                    SendOutcome::Failed(k) => k,
                    _ => std::io::ErrorKind::InvalidInput,
                };
                self.fail(ctx, UtpError::Send(kind));
                return false;
            }
            SendOutcome::Sent => {
                self.out_packets += 1;
                ctx.counters.packets_out += 1;
                p.num_transmissions += 1;
                // Only reset the timeout for the initial packet
                if self.bytes_in_flight == 0 {
                    self.timeout = ctx.now + Duration::from_millis(self.packet_timeout(ctx) as u64);
                }
                // Any queued up deferred ack is now redundant
                if self.deferred_ack {
                    self.deferred_ack = false;
                    ctx.cancel_deferred_ack();
                }
            }
        }

        // if we have payload, we need to save the packet until it's acked and progress seq_nr
        if p.size > p.header_size {
            // if we're sending a payload packet, there should not be a nagle packet waiting for
            // more data
            debug_assert!(self.nagle_packet.is_none());
            debug_assert_eq!(header::seq_nr(&p.buf), self.seq_nr);
            // 0 is a special sequence number, since it's also used as "uninitialized". we never
            // send an mtu probe for sequence number 0
            debug_assert!(p.mtu_probe == (self.seq_nr == self.mtu_seq) || self.seq_nr == 0);
            // If we're stalled we'll need to resend
            if self.stalled {
                p.need_resend = true;
            }
            // If this packet is undersized then note the sequence number so we never have more
            // than one undersized packet in flight at once
            if (p.size as usize) < (p.allocated() as usize).min(effective_mtu) {
                self.nagle_seq_nr = self.seq_nr;
            }
            let new_in_flight = p.payload_len();
            if let Some(old) = self.outbuf.insert(self.seq_nr as u32, p) {
                if !old.need_resend {
                    self.bytes_in_flight -= old.payload_len();
                }
            }
            // we shouldn't be sending payload at sequence numbers past the FIN
            debug_assert!(self.state != State::FinSent);
            self.seq_nr = self.seq_nr.wrapping_add(1);
            if !self.stalled {
                self.bytes_in_flight += new_in_flight;
            }
        } else if flags & PKT_FIN != 0 {
            debug_assert_eq!(payload_size, 0);
            // If we're stalled we'll need to resend
            if self.stalled {
                p.need_resend = true;
            }
            if let Some(old) = self.outbuf.insert(self.seq_nr as u32, p) {
                if !old.need_resend {
                    self.bytes_in_flight -= old.payload_len();
                }
            }
        } else {
            debug_assert_eq!(header::seq_nr(&p.buf), self.seq_nr);
            // this is a re-entrant call, so we have to be careful only making it if we're not
            // already sending a FIN
            if self.out_eof
                && ctx.write_buffer_size() == 0
                && self.nagle_packet.is_none()
                && self.state == State::Connected
            {
                self.send_fin(ctx);
            }
        }

        // if the socket is stalled, always return false, don't try to write more packets.
        // We'll keep writing once the underlying UDP socket becomes writable
        ctx.write_buffer_size() > 0 && !self.cwnd_full && !self.stalled
    }

    /// Sends the packet at `seq` of `outbuf` again. Returns false if it could not go out.
    fn resend_packet(&mut self, ctx: &mut Ctx<'_>, seq: u32, fast_resend: bool) -> bool {
        let Some(mut p) = self.outbuf.remove(seq) else { return false };
        let r = self.resend_packet_impl(ctx, &mut p, fast_resend);
        self.outbuf.insert(seq, p);
        r
    }

    fn resend_packet_impl(&mut self, ctx: &mut Ctx<'_>, p: &mut Packet, fast_resend: bool) -> bool {
        // for fast re-sends the packet hasn't been marked as needing resending
        debug_assert!(p.need_resend || fast_resend);
        if self.error.is_some() {
            return false;
        }
        if self.stalled {
            return false;
        }
        if self.acked_seq_nr.wrapping_add(1) == self.mtu_seq && self.mtu_seq != 0 {
            self.mtu_seq = 0;
            p.mtu_probe = false;
            // we got multiple acks for the packet before our probe, assume it was dropped
            // because it was too big
            self.mtu_ceiling = p.size - 1;
            self.update_mtu_limits();
        }
        // we can only resend the packet if there's enough space in our congestion window
        // since we can't re-packetize, some packets that are larger than the congestion window
        // must be allowed through but only if we don't have any outstanding bytes
        let window_size_left = (self.cwnd >> 16).min(self.adv_wnd as i64) - self.bytes_in_flight as i64;
        if !fast_resend && p.payload_len() as i64 > window_size_left && self.bytes_in_flight > 0 {
            self.cwnd_full = true;
            return false;
        }
        // the fast-resend path does not check for too many resends, that's only in the
        // time-out path
        debug_assert!(fast_resend || (p.num_transmissions as i32) < ctx.settings.num_resends + 1);
        debug_assert!(p.payload_len() >= 0);
        if p.need_resend {
            self.bytes_in_flight += p.payload_len();
        }
        ctx.counters.packet_resend += 1;
        if fast_resend {
            ctx.counters.fast_retransmit += 1;
        }
        p.need_resend = false;
        // update packet header
        header::set_timestamp_difference(&mut p.buf, self.reply_micro);
        p.send_time = ctx.now;
        header::set_timestamp(&mut p.buf, ctx.micros());
        // if the packet has a selective ack header, we'll need to update it
        if header::extension(&p.buf) == header::EXT_SACK && header::ack_nr(&p.buf) != self.ack_nr {
            let sack_size = p.buf[header::SIZE + 1] as usize;
            if !self.inbuf.is_empty() {
                // update the sack header
                let mut bits = vec![0u8; sack_size];
                self.write_sack(&mut bits);
                p.buf[header::SIZE + 2..header::SIZE + 2 + sack_size].copy_from_slice(&bits);
                debug_assert!(header::SIZE + 2 + sack_size <= p.header_size as usize);
            } else {
                Self::remove_sack_header(p);
            }
        }
        header::set_ack_nr(&mut p.buf, self.ack_nr);
        match ctx.io.send(self.remote, &p.buf[..p.size as usize], false) {
            SendOutcome::WouldBlock => {
                debug_assert!(!self.stalled);
                self.stalled = true;
                ctx.subscribe_writable();
                p.need_resend = true;
                self.bytes_in_flight -= p.payload_len();
            }
            SendOutcome::MessageSize => {
                self.fail(ctx, UtpError::Send(std::io::ErrorKind::InvalidInput));
                return false;
            }
            SendOutcome::Failed(k) => {
                self.fail(ctx, UtpError::Send(k));
                return false;
            }
            SendOutcome::Sent => {
                ctx.counters.packets_out += 1;
                self.out_packets += 1;
                p.num_transmissions += 1;
            }
        }
        !self.stalled
    }

    fn experienced_loss(&mut self, ctx: &mut Ctx<'_>, seq_nr: u32) {
        // the window size could go below one MMS here, if it does, we'll get a timeout in about
        // one second
        ctx.counters.packet_loss += 1;
        // since loss often comes in bursts, we only cut the window in half once per RTT. This
        // is implemented by limiting which packets can cause us to cut the window size. The
        // first packet that's lost will update the limit to the last sequence number we sent.
        // i.e. only packet sent after this loss can cause another window size cut. The +1 is to
        // turn the comparison into less than or equal to. If we experience loss of the same
        // packet again, ignore it.
        if compare_less_wrap(seq_nr, self.loss_seq_nr as u32 + 1, ACK_MASK) {
            return;
        }
        // don't reduce cwnd more than once every 100ms
        if self.next_loss.is_some_and(|t| t >= ctx.now) {
            return;
        }
        self.next_loss = Some(ctx.now + Duration::from_millis(ctx.settings.cwnd_reduce_timer.max(0) as u64));
        // cut window size in 2
        self.cwnd = (self.cwnd * ctx.settings.loss_multiplier as i64 / 100).max((self.mtu as i64) << 16);
        self.loss_seq_nr = self.seq_nr;
        // if we happen to be in slow-start mode, we need to leave it. Note that we set ssthres
        // to the window size _after_ reducing it. Next slow start should end before we over
        // shoot.
        if self.slow_start {
            self.ssthres = (self.cwnd >> 16) as i32;
            self.slow_start = false;
        }
    }

    fn maybe_inc_acked_seq_nr(&mut self) {
        let mut incremented = false;
        // don't pass seq_nr, since we move into sequence numbers that haven't been sent yet, and
        // aren't supposed to be in outbuf. If the slot in outbuf is 0, it means the packet has
        // been ACKed and removed from the send buffer. Once we're in the fin_sent state
        // acked_seq_nr can equal seq_nr, but shouldn't reach seq_nr + 1
        let limit = if self.state == State::FinSent { self.seq_nr.wrapping_add(1) } else { self.seq_nr };
        while self.acked_seq_nr.wrapping_add(1) != limit
            && self.outbuf.at(self.acked_seq_nr.wrapping_add(1) as u32).is_none()
        {
            // increment the fast resend sequence number
            if self.fast_resend_seq_nr == self.acked_seq_nr {
                self.fast_resend_seq_nr = self.fast_resend_seq_nr.wrapping_add(1);
            }
            self.acked_seq_nr = self.acked_seq_nr.wrapping_add(1);
            incremented = true;
        }
        if !incremented {
            return;
        }
        // update loss seq number if it's less than the packet that was just acked. If loss seq
        // nr is greater, it suggests that we're still in a window that has experienced loss
        if compare_less_wrap(self.loss_seq_nr as u32, self.acked_seq_nr as u32, ACK_MASK) {
            self.loss_seq_nr = self.acked_seq_nr;
        }
        self.duplicate_acks = 0;
    }

    /// A packet of ours was acknowledged. Returns the RTT in microseconds.
    fn ack_packet(&mut self, p: &Packet, receive_time: Instant, seq_nr: u16) -> u32 {
        if !p.need_resend {
            debug_assert!(self.bytes_in_flight >= p.payload_len());
            self.bytes_in_flight -= p.payload_len();
        }
        if seq_nr == self.mtu_seq && self.mtu_seq != 0 {
            debug_assert!(p.mtu_probe);
            // our mtu probe was acked!
            self.mtu_floor = self.mtu_floor.max(p.size);
            self.update_mtu_limits();
        }
        // increment the acked sequence number counter
        self.maybe_inc_acked_seq_nr();
        let rtt = match receive_time.checked_duration_since(p.send_time) {
            Some(d) => d.as_micros() as u32,
            // this means our clock is not monotonic. Just assume the RTT was 100 ms
            None => 100_000,
        };
        self.rtt.add_sample((rtt / 1000) as i64);
        rtt
    }

    /// libtorrent `parse_sack`: returns (min RTT, acked bytes).
    fn parse_sack(&mut self, ctx: &mut Ctx<'_>, packet_ack: u16, bits: &[u8]) -> (u32, i32) {
        if bits.is_empty() {
            return (0, 0);
        }
        // this is the sequence number the current bit represents
        let mut ack_nr = (packet_ack as u32 + 2) & ACK_MASK;
        let mut resend = [0u16; 5];
        let mut num_to_resend = 0usize;
        let mut acked_bytes = 0i32;
        let mut min_rtt = u32::MAX;

        // this was implicitly lost
        if !compare_less_wrap((packet_ack as u32 + 1) & ACK_MASK, self.fast_resend_seq_nr as u32, ACK_MASK) {
            resend[num_to_resend] = packet_ack.wrapping_add(1);
            num_to_resend += 1;
        }

        // for each byte
        'outer: for &bitfield in bits {
            let mut mask = 1u8;
            // for each bit
            for _ in 0..8 {
                if mask & bitfield != 0 {
                    // this bit was set, ack_nr was received
                    match self.outbuf.remove(ack_nr) {
                        Some(p) => {
                            acked_bytes += p.payload_len();
                            // each ACKed packet counts as a duplicate ack
                            min_rtt = min_rtt.min(self.ack_packet(&p, ctx.now, ack_nr as u16));
                        }
                        None => {
                            // this packet might have been acked by a previous selective ack
                            self.maybe_inc_acked_seq_nr();
                        }
                    }
                } else if !compare_less_wrap(ack_nr, self.fast_resend_seq_nr as u32, ACK_MASK)
                    && num_to_resend < resend.len()
                {
                    resend[num_to_resend] = ack_nr as u16;
                    num_to_resend += 1;
                }
                mask <<= 1;
                ack_nr = (ack_nr + 1) & ACK_MASK;
                // we haven't sent packets past this point. if there are any more bits set, we
                // have to ignore them anyway
                if ack_nr == self.seq_nr as u32 {
                    break 'outer;
                }
            }
        }

        if self.outbuf.is_empty() {
            self.duplicate_acks = 0;
        }

        // now, scan the bits in reverse, and count the number of ACKed packets. Only lost
        // packets followed by 'dup_ack_limit' packets may be resent. Start with the sequence
        // number represented by the last bit in the SACK bitmask
        let mut last_resend = (packet_ack as u32 + 1 + bits.len() as u32 * 8) & ACK_MASK;
        // the number of acked packets past the fast re-send sequence number this is used to
        // determine if we should trigger more fast re-sends
        let mut dups = 0u8;
        'rev: for &bitfield in bits.iter().rev() {
            let mut mask = 0x80u8;
            for _ in 0..8 {
                if mask & bitfield != 0 {
                    dups += 1;
                }
                if dups > DUP_ACK_LIMIT {
                    break 'rev;
                }
                last_resend = last_resend.wrapping_sub(1) & ACK_MASK;
                mask >>= 1;
            }
        }

        // we did not get enough packets acked in this message to warrant a resend
        if dups <= DUP_ACK_LIMIT {
            num_to_resend = 0;
        }

        // now we need to (likely) prune the tail of the resend list, since all "unacked"
        // packets that weren't followed by an acked one, don't count
        while num_to_resend > 0 && !compare_less_wrap(resend[num_to_resend - 1] as u32, last_resend, ACK_MASK)
        {
            num_to_resend -= 1;
        }

        let mut cut_cwnd = true;
        // we received more than dup_ack_limit ACKs in this SACK message. trigger fast re-send.
        // This is not an equal check because 3 identical ACKS are only 2 duplicates
        for &pkt_seq in &resend[..num_to_resend] {
            if self.outbuf.at(pkt_seq as u32).is_none() {
                continue;
            }
            // don't cut cwnd if the packet we lost was the MTU probe; the logic to handle a
            // lost MTU probe is in resend_packet()
            if cut_cwnd && (pkt_seq != self.mtu_seq || self.mtu_seq == 0) {
                self.experienced_loss(ctx, pkt_seq as u32);
                cut_cwnd = false;
            }
            if self.resend_packet(ctx, pkt_seq as u32, true) {
                self.duplicate_acks = 0;
                self.fast_resend_seq_nr = pkt_seq.wrapping_add(1);
            }
            if self.stalled {
                break;
            }
        }
        (min_rtt, acked_bytes)
    }

    // --- Receiving ----------------------------------------------------------------------------

    /// Stores received payload for the application (libtorrent `incoming`, with no user
    /// buffer to copy into).
    fn incoming(&mut self, ctx: &mut Ctx<'_>, buf: &[u8], p: Option<Box<Packet>>) {
        let p = match p {
            Some(p) => p,
            None => {
                if buf.is_empty() {
                    return;
                }
                let mut p = Box::new(Packet::new(buf.len()));
                p.size = buf.len() as u16;
                p.header_size = 0;
                p.buf.copy_from_slice(buf);
                p
            }
        };
        // save this packet until the client issues another read
        self.receive_buffer_size += p.payload_len();
        self.receive_buffer.push_back(p);
        self.mark_ready(ctx);
    }

    /// Returns true if the packet was consumed (dropped or already had), false if it was data
    /// to go on with.
    fn consume_incoming_data(&mut self, ctx: &mut Ctx<'_>, ph: &header::Header, payload: &[u8]) -> bool {
        if ph.type_ != header::ST_DATA {
            return false;
        }
        if self.in_eof && self.ack_nr == self.in_eof_seq_nr {
            // What?! We've already received a FIN and everything up to it has been acked.
            // Ignore this packet
            return true;
        }
        if self.receive_buffer_size >= self.receive_buffer_capacity - self.buffered_incoming_bytes {
            // the number of queued up bytes, waiting for the upper layer, exceeds the
            // advertised receive window, start ignoring more data packets
            return false;
        }
        let payload_size = payload.len() as i32;
        if ph.seq_nr == self.ack_nr.wrapping_add(1) {
            debug_assert!(self.inbuf.at(self.ack_nr as u32).is_none());
            if self.buffered_incoming_bytes + self.receive_buffer_size + payload_size
                > self.receive_buffer_capacity
            {
                // other end is not honoring our advertised window, dropping packet
                return true;
            }
            // we received a packet in order
            self.incoming(ctx, payload, None);
            self.ack_nr = self.ack_nr.wrapping_add(1);
            // If this packet was previously in the reorder buffer it would have been acked
            // when ack_nr-1 was acked.
            debug_assert!(self.inbuf.at(self.ack_nr as u32).is_none());
            loop {
                let next_ack_nr = self.ack_nr.wrapping_add(1);
                let Some(p) = self.inbuf.remove(next_ack_nr as u32) else { break };
                debug_assert!(p.size >= p.header_size);
                self.buffered_incoming_bytes -= p.payload_len();
                self.incoming(ctx, &[], Some(p));
                self.ack_nr = next_ack_nr;
            }
        } else {
            // this packet was received out of order. Stick it in the reorder buffer until it
            // can be delivered in order. Have we already received this packet and passed it on
            // to the client?
            if !compare_less_wrap(self.ack_nr as u32, ph.seq_nr as u32, ACK_MASK) {
                return true;
            }
            // do we already have this packet? If so, just ignore it
            if self.inbuf.at(ph.seq_nr as u32).is_some() {
                return true;
            }
            if self.buffered_incoming_bytes + self.receive_buffer_size + payload_size
                > self.receive_buffer_capacity
            {
                // other end is not honoring our advertised window, dropping packet
                return true;
            }
            // we don't need to save the packet header, just the payload
            let mut p = Box::new(Packet::new(payload.len()));
            p.size = payload.len() as u16;
            p.header_size = 0;
            p.buf.copy_from_slice(payload);
            self.buffered_incoming_bytes += p.size as i32;
            self.inbuf.insert(ph.seq_nr as u32, p);
        }
        false
    }

    /// libtorrent `packet_timeout`: milliseconds a packet sent now would have before it times
    /// out, from the RTT estimate.
    fn packet_timeout(&self, ctx: &Ctx<'_>) -> i32 {
        // SYN packets have a bit longer timeout, since we don't have an RTT estimate yet, make
        // a conservative guess
        if self.state == State::None {
            return 3000;
        }
        // avoid overflow by simply capping based on number of timeouts as well
        if self.num_timeouts >= 7 {
            return 60000;
        }
        let mut timeout =
            (ctx.settings.min_timeout as i64).max(self.rtt.mean() + self.rtt.avg_deviation() * 2);
        if self.num_timeouts > 0 {
            timeout += (1i64 << (self.num_timeouts as i64 - 1)) * 1000;
        }
        // timeouts over 1 minute are capped
        timeout.min(60000) as i32
    }

    /// Feeds a datagram for this socket. Returns false if it is not a valid uTP packet.
    pub(super) fn incoming_packet(&mut self, ctx: &mut Ctx<'_>, buf: &[u8], ep: SocketAddr) -> bool {
        let receive_time = ctx.now;
        ctx.counters.packets_in += 1;
        let Some(ph) = header::Header::parse(buf) else {
            ctx.counters.invalid_pkts_in += 1;
            return false;
        };
        if ph.version != 1 {
            ctx.counters.invalid_pkts_in += 1;
            return false;
        }
        // SYN packets have special (reverse) connection ids
        if ph.type_ != header::ST_SYN && ph.connection_id != self.recv_id {
            ctx.counters.invalid_pkts_in += 1;
            return false;
        }
        if ph.type_ >= header::NUM_TYPES {
            ctx.counters.invalid_pkts_in += 1;
            return false;
        }
        if self.state == State::None && ph.type_ == header::ST_SYN {
            self.remote = ep;
        }
        if self.state != State::None && ph.type_ == header::ST_SYN {
            ctx.counters.invalid_pkts_in += 1;
            return true;
        }

        let mut step = false;
        if receive_time.saturating_duration_since(self.last_history_step) > Duration::from_secs(60) {
            step = true;
            self.last_history_step = receive_time;
        }

        // this is the difference between their send time and our receive time. 0 means no
        // sample yet
        let mut their_delay = 0u32;
        if ph.timestamp_microseconds != 0 {
            let timestamp = ctx.micros();
            self.reply_micro = timestamp.wrapping_sub(ph.timestamp_microseconds);
            let prev_base =
                if self.their_delay_hist.initialized() { self.their_delay_hist.base() } else { 0 };
            their_delay = self.their_delay_hist.add_sample(self.reply_micro, step);
            let base_change = self.their_delay_hist.base().wrapping_sub(prev_base) as i32;
            if prev_base != 0 && base_change < 0 && base_change > -10000 && self.delay_hist.initialized() {
                // their base delay went down. This is caused by clock drift. To compensate,
                // adjust our base delay upwards. Don't adjust more than 10 ms. If the change is
                // that big, something is probably wrong
                self.delay_hist.adjust_base(-base_change);
            }
        }

        let state_or_fin = ph.type_ == header::ST_STATE || ph.type_ == header::ST_FIN;

        // Is this ACK valid? If the other end is ACKing a packet that hasn't been sent yet just
        // ignore it. A 3rd party could easily inject a packet like this in a stream, don't
        // sever it because of it. Since seq_nr is the sequence number of the next packet we'll
        // send (and seq_nr-1 was the last packet we sent), if the ACK we got is greater than
        // the last packet we sent something is wrong. If our state is None, this packet must
        // be a syn packet and the ack_nr should be ignored. Note that when we send a FIN, we
        // don't increment seq_nr
        let cmp_seq_nr =
            if matches!(self.state, State::SynSent | State::FinSent | State::Deleting) && state_or_fin {
                self.seq_nr
            } else {
                self.seq_nr.wrapping_sub(1)
            };
        if (self.state != State::None || ph.type_ != header::ST_SYN)
            && (compare_less_wrap(cmp_seq_nr as u32, ph.ack_nr as u32, ACK_MASK)
                || compare_less_wrap(
                    ph.ack_nr as u32,
                    (self.acked_seq_nr as u32).wrapping_sub(DUP_ACK_LIMIT as u32) & ACK_MASK,
                    ACK_MASK,
                ))
        {
            ctx.counters.redundant_pkts_in += 1;
            return true;
        }

        // if the socket is closing, always ignore any packet with a higher sequence number than
        // the FIN sequence number. ST_STATE messages always include the next seqnr, so it's
        // acceptable to receive the same seq_nr as the EOF as long as it's a STATE or FIN packet
        if self.in_eof
            && compare_less_wrap(self.in_eof_seq_nr as u32, ph.seq_nr as u32, ACK_MASK)
            && !(self.in_eof_seq_nr == ph.seq_nr && state_or_fin)
        {
            return true;
        }

        if ph.type_ == header::ST_DATA {
            ctx.counters.payload_pkts_in += 1;
        }

        // the number of packets that'll fit in the reorder buffer
        let max_packets_reorder = (self.receive_buffer_capacity / 1100).max(16) as u32;
        if self.state != State::None
            && self.state != State::SynSent
            && compare_less_wrap(
                (self.ack_nr as u32 + max_packets_reorder) & ACK_MASK,
                ph.seq_nr as u32,
                ACK_MASK,
            )
        {
            // this is too far out to fit in our reorder buffer. Drop it. This is either an
            // attack to try to break the connection or a seriously damaged connection that lost
            // a lot of packets. Neither is very likely, and it should be OK to drop the
            // timestamp information.
            ctx.counters.redundant_pkts_in += 1;
            return true;
        }

        if ph.type_ == header::ST_RESET {
            if compare_less_wrap(cmp_seq_nr as u32, ph.ack_nr as u32, ACK_MASK) {
                // invalid RESET packet (ignored)
                return true;
            }
            self.fail(ctx, UtpError::ConnectionReset);
            return true;
        }

        self.in_packets += 1;

        // the test for INT_MAX here is a work-around for a bug in uTorrent where it's sometimes
        // sent as INT_MAX when it is in fact uninitialized
        let sample = if ph.timestamp_difference_microseconds == i32::MAX as u32 {
            0
        } else {
            ph.timestamp_difference_microseconds
        };
        if sample != 0 {
            let delay = self.delay_hist.add_sample(sample, step);
            self.delay_sample_hist[self.delay_sample_idx] = delay;
            self.delay_sample_idx = (self.delay_sample_idx + 1) % self.delay_sample_hist.len();
        }

        let mut acked_bytes = 0i32;
        debug_assert!(self.bytes_in_flight >= 0);
        let prev_bytes_in_flight = self.bytes_in_flight;
        let prev_acked_seq_nr = self.acked_seq_nr;
        self.adv_wnd = ph.wnd_size;

        // if we get an ack for the same sequence number as was last ACKed, and we have
        // outstanding packets, it counts as a duplicate ack. The reason to not count ST_DATA
        // packets as duplicate ACKs is because we may be receiving a stream of those regardless
        // of our outgoing traffic, which makes their ACK number not indicative of a dropped
        // packet
        if ph.ack_nr == self.acked_seq_nr && !self.outbuf.is_empty() && ph.type_ == header::ST_STATE {
            self.duplicate_acks = self.duplicate_acks.saturating_add(1);
        }

        let mut min_rtt = u32::MAX;
        // has this packet already been ACKed? If the ACK we just got is less than the max
        // ACKed sequence number, it doesn't tell us anything. So, only act on it if the ACK is
        // greater than the last acked sequence number
        if self.state != State::None
            && compare_less_wrap(self.acked_seq_nr as u32, ph.ack_nr as u32, ACK_MASK)
        {
            let next_ack_nr = ph.ack_nr;
            let mut ack_nr = self.acked_seq_nr.wrapping_add(1);
            while ack_nr != next_ack_nr.wrapping_add(1) {
                if self.fast_resend_seq_nr == ack_nr {
                    self.fast_resend_seq_nr = self.fast_resend_seq_nr.wrapping_add(1);
                }
                if let Some(p) = self.outbuf.remove(ack_nr as u32) {
                    acked_bytes += p.payload_len();
                    let rtt = self.ack_packet(&p, receive_time, ack_nr);
                    min_rtt = min_rtt.min(rtt);
                }
                ack_nr = ack_nr.wrapping_add(1);
            }
            self.maybe_inc_acked_seq_nr();
            if self.outbuf.is_empty() {
                self.duplicate_acks = 0;
            }
        }

        // look for extended headers
        let mut ptr = header::SIZE;
        let size = buf.len();
        let mut extension = ph.extension;
        while extension != 0 {
            // invalid packet. It says it has an extension header but the packet is too short
            if ptr + 2 > size {
                ctx.counters.invalid_pkts_in += 1;
                return true;
            }
            let next_extension = buf[ptr];
            let len = buf[ptr + 1] as usize;
            ptr += 2;
            if ptr + len > size {
                ctx.counters.invalid_pkts_in += 1;
                return true;
            }
            match extension {
                header::EXT_SACK => {
                    let (rtt, acked) = self.parse_sack(ctx, ph.ack_nr, &buf[ptr..ptr + len]);
                    min_rtt = min_rtt.min(rtt);
                    acked_bytes = acked;
                }
                // the close reason is read and dropped: we report none of our own yet
                header::EXT_CLOSE_REASON => {}
                _ => {}
            }
            ptr += len;
            extension = next_extension;
        }

        // this is a valid incoming packet, update the timeout timer. Do this after processing
        // sacks/acks as that can effect packet_timeout()
        // Unlike libtorrent, only while nothing is in flight or when the packet acknowledged
        // some of it (RFC 6298 5.3): the peer's own data packets do not prove that ours arrive.
        // Restarting the timer on every packet, a window lost whole (UDP receive buffer
        // overflow) was never resent for as long as the peer kept sending, e.g. HAVEs every
        // few hundred ms: no timeout, and no duplicate ACKs since nothing arrived to ACK.
        let progress = self.acked_seq_nr != prev_acked_seq_nr || self.bytes_in_flight < prev_bytes_in_flight;
        if progress || self.bytes_in_flight == 0 {
            self.num_timeouts = 0;
            self.timeout = receive_time + Duration::from_millis(self.packet_timeout(ctx) as u64);
        }

        if self.duplicate_acks >= DUP_ACK_LIMIT
            && self.acked_seq_nr.wrapping_add(1) == self.fast_resend_seq_nr
        {
            // LOSS: resend the lost packet
            let lost = self.fast_resend_seq_nr;
            // don't fast-resend this again
            self.fast_resend_seq_nr = self.fast_resend_seq_nr.wrapping_add(1);
            if let Some(p) = self.outbuf.at(lost as u32) {
                // don't consider a lost probe as proper loss, it doesn't necessarily signal
                // congestion
                let probe = p.mtu_probe;
                if !probe {
                    self.experienced_loss(ctx, self.fast_resend_seq_nr as u32);
                }
                self.resend_packet(ctx, lost as u32, true);
            }
        }

        // ptr points to the payload of the packet
        let payload = &buf[ptr..];

        if ph.type_ == header::ST_FIN {
            // We ignore duplicate FIN packets, but we still need to ACK them.
            if ph.seq_nr == self.ack_nr.wrapping_add(1) || ph.seq_nr == self.ack_nr {
                // The FIN arrived in order, nothing else is in the reorder buffer.
                self.ack_nr = ph.seq_nr;
            }
            if !self.in_eof {
                self.in_eof = true;
                // even though invalid, tolerate FIN packets with payload
                self.in_eof_seq_nr = if payload.is_empty() { ph.seq_nr } else { ph.seq_nr.wrapping_add(1) };
            }
            self.defer_ack(ctx);
            if self.ack_nr == self.in_eof_seq_nr && self.receive_buffer.is_empty() {
                self.mark_ready(ctx);
            }
            return true;
        }

        // the send operation in parse_sack() may have set the socket to an error state, in
        // which case we shouldn't continue
        if matches!(self.state, State::ErrorWait | State::Deleting) {
            return true;
        }

        match self.state {
            State::None => {
                if ph.type_ == header::ST_SYN {
                    // if we're in state_none, the only thing we accept are SYN packets.
                    self.set_state(ctx, State::Connected);
                    self.remote = ep;
                    self.ack_nr = ph.seq_nr;
                    self.seq_nr = random_below(0x1_0000) as u16;
                    self.acked_seq_nr = self.seq_nr.wrapping_sub(1);
                    self.loss_seq_nr = self.acked_seq_nr;
                    self.fast_resend_seq_nr = self.seq_nr;
                    if self.send_id != ph.connection_id {
                        return false;
                    }
                    debug_assert_eq!(self.recv_id, self.send_id.wrapping_add(1));
                    self.defer_ack(ctx);
                }
                // anything else is ignored
            }
            State::SynSent | State::Connected => {
                if self.state == State::SynSent {
                    // just wait for an ack to our SYN, ignore everything else
                    if ph.ack_nr != self.seq_nr.wrapping_sub(1) {
                        return true;
                    }
                    debug_assert!(self.error.is_none());
                    self.set_state(ctx, State::Connected);
                    // only progress our ack_nr on ST_DATA messages since our ack_nr is
                    // uninitialized at this point we still need to set it to something
                    // regardless
                    self.ack_nr =
                        if ph.type_ == header::ST_DATA { ph.seq_nr } else { ph.seq_nr.wrapping_sub(1) };
                    // notify the client that the socket connected
                    self.mark_ready(ctx);
                }
                // the lowest seen RTT can be used to clamp the delay within reasonable bounds.
                // The one-way delay is never higher than the round-trip time.
                if sample != 0 && acked_bytes > 0 && prev_bytes_in_flight > 0 {
                    // only use the minimum from the last 3 delay measurements
                    let mut delay = *self.delay_sample_hist.iter().min().expect("three samples");
                    // it's impossible for delay to be more than the RTT, so make sure to clamp
                    // it as a sanity check
                    if delay > min_rtt {
                        delay = min_rtt;
                    }
                    self.do_ledbat(ctx, acked_bytes, delay as i32, prev_bytes_in_flight);
                    self.send_delay = delay as i32;
                }
                self.recv_delay = their_delay.min(min_rtt) as i32;
                self.consume_incoming_data(ctx, &ph, payload);
                // the parameter to send_pkt tells it if we're acking data. If we are, we'll
                // send an ACK regardless of if we have any space left in our send window or
                // not. If we just got an ACK (i.e. ST_STATE) we're not ACKing anything. If we
                // just received a FIN packet, we need to ack that as well
                let has_ack = matches!(ph.type_, header::ST_DATA | header::ST_FIN | header::ST_SYN);
                let prev_out_packets = self.out_packets;
                // the connection is connected and this packet made it past all the checks. We
                // can now assume the other end is not spoofing its IP.
                if ph.type_ != header::ST_SYN {
                    self.confirmed = true;
                }
                // try to send more data as long as we can (pending user data from an earlier
                // write is the caller's; here only the Nagle packet and resends go out)
                while self.send_pkt(ctx, 0) {}
                if has_ack && prev_out_packets == self.out_packets {
                    // we need to ack some data we received, and we didn't end up sending any
                    // payload packets in the loop above (because out_packets would have been
                    // incremented in that case). This means we need to send an ack. Don't do it
                    // right away, because we may still receive more packets. Defer the ack to
                    // send as few acks as possible
                    self.defer_ack(ctx);
                }
                if matches!(self.state, State::ErrorWait | State::Deleting) {
                    return true;
                }
                self.maybe_wake_writer(ctx);
                debug_assert!(!compare_less_wrap(self.seq_nr as u32, self.acked_seq_nr as u32, ACK_MASK));
            }
            State::FinSent => {
                // There are two ways we can end up in this state:
                //
                // 1. If the socket has been explicitly closed on our side, in which case in_eof
                //    is false.
                //
                // 2. If we received a FIN from the remote side, in which case in_eof is true.
                //    If this is the case, we don't come here until everything up to the FIN
                //    has been received.
                //
                // At this point seq_nr - 1 is the FIN sequence number. We can receive both
                // ST_DATA and ST_STATE here, because after we have closed our end of the
                // socket, the remote end might have data in the pipeline. We don't really care
                // about the data, but we do have to ack it. Or rather, we have to ack the FIN
                // that will come after the data.
                //
                // Case 1: if we are here because the local endpoint was closed, we need to
                // first wait for all of our messages to be acked (acked_seq_nr == seq_nr - 1,
                // the ST_FIN we sent). After that has happened we need to wait for the remote
                // side to send its ST_FIN message. When we receive that we send an ST_STATE
                // back to ack, and wait for a sufficient period. During this wait we keep
                // acking incoming ST_FINs. This is all handled at the top of this function.
                //
                // Case 2: if we are here because we received a ST_FIN message, and then sent
                // our own ST_FIN to ack that, we need to wait for our ST_FIN to be acked. After
                // that has happened we know the remote side has all our data, and we can
                // gracefully shut down.

                // we should still ack any incoming data to prevent potential timeouts/resends
                // at the other end
                if ph.type_ == header::ST_DATA {
                    self.defer_ack(ctx);
                }
                if self.consume_incoming_data(ctx, &ph, payload) {
                    return true;
                }
                // we don't increment seq_nr when sending a FIN, so we actually need to wait for
                // acked_seq_nr to reach seq_nr before the FIN is considered ACKed
                if self.acked_seq_nr == self.seq_nr {
                    // When this happens we know that the remote side has received all of our
                    // packets.
                    self.error = Some(UtpError::Eof);
                    if !self.attached {
                        // close initiated here, delete socket
                        self.set_state(ctx, State::Deleting);
                    } else {
                        self.set_state(ctx, State::ErrorWait);
                    }
                    self.test_socket_state(ctx);
                }
            }
            State::ErrorWait | State::Deleting => {
                // respond with a reset
                self.send_reset(ctx, ph.seq_nr);
            }
        }
        true
    }

    /// The LEDBAT controller (RFC 6817 / BEP 29): grows or shrinks the window from the one-way
    /// delay's distance to the target.
    fn do_ledbat(&mut self, ctx: &mut Ctx<'_>, acked_bytes: i32, delay: i32, in_flight: i32) {
        // the portion of the in-flight bytes that were acked. This is used to make the gain
        // factor be scaled by the rtt. The formula is applied once per rtt, or on every ACK
        // scaled by the number of ACKs per rtt
        debug_assert!(in_flight > 0);
        debug_assert!(acked_bytes > 0);
        let target_delay = ctx.settings.target_delay.max(1);
        // true if the upper layer is pushing enough data down the socket to be limited by the
        // cwnd. If this is not the case, we should not adjust cwnd.
        let cwnd_saturated =
            (self.bytes_in_flight + acked_bytes + self.mtu as i32) as i64 > (self.cwnd >> 16);
        // all of these are fixed points with 16 bits fraction portion
        let window_factor = ((acked_bytes as i64) << 16) / in_flight as i64;
        let delay_factor = (((target_delay - delay) as i64) << 16) / target_delay as i64;
        if delay >= target_delay {
            if self.slow_start {
                self.ssthres = ((self.cwnd >> 16) / 2) as i32;
                self.slow_start = false;
            }
            ctx.counters.samples_above_target += 1;
        } else {
            ctx.counters.samples_below_target += 1;
        }
        let linear_gain = ((window_factor * delay_factor) >> 16) * ctx.settings.gain_factor as i64;
        // if the user is not saturating the link (i.e. not filling the congestion window),
        // don't adjust it at all.
        let mut scaled_gain = if cwnd_saturated {
            let exponential_gain = (acked_bytes as i64) << 16;
            if self.slow_start {
                // mimic TCP slow-start by adding the number of acked bytes to cwnd
                if self.ssthres != 0 && ((self.cwnd + exponential_gain) >> 16) > self.ssthres as i64 {
                    // if we would exceed the slow start threshold by growing the cwnd
                    // exponentially, don't do it, and leave slow-start mode. This make us avoid
                    // causing more delay and/or packet loss by being too aggressive
                    self.slow_start = false;
                    linear_gain
                } else {
                    exponential_gain.max(linear_gain)
                }
            } else {
                linear_gain
            }
        } else {
            0
        };
        // make sure we don't wrap the cwnd
        if scaled_gain >= i64::MAX - self.cwnd {
            scaled_gain = i64::MAX - self.cwnd - 1;
        }
        // don't drop below 1*MSS. This behavior is from rfc6817 (LEDBAT). This differs from
        // BEP 29 which allows cwnd to drop to 0, however this way avoids needing to wait until
        // the next timeout to resume sending.
        if (self.cwnd + scaled_gain) >> 16 < self.mtu as i64 {
            self.cwnd = (self.mtu as i64) << 16;
        } else {
            self.cwnd += scaled_gain;
        }
        debug_assert!((self.cwnd >> 16) >= self.mtu as i64);
        let window_size_left =
            (self.cwnd >> 16).min(self.adv_wnd as i64) - in_flight as i64 + acked_bytes as i64;
        if window_size_left >= self.mtu as i64 {
            self.cwnd_full = false;
        }
    }

    /// libtorrent `utp_socket_impl::tick`: timeouts.
    pub(super) fn tick(&mut self, ctx: &mut Ctx<'_>) {
        debug_assert!(
            self.outbuf.at(self.acked_seq_nr.wrapping_add(1) as u32).is_some()
                || (self.seq_nr.wrapping_sub(self.acked_seq_nr) as u32 & ACK_MASK) <= 1
        );
        // if we're already in an error state, we're just waiting for the client to perform an
        // operation so that we can communicate the error. No need to do anything else with this
        // socket
        if matches!(self.state, State::ErrorWait | State::Deleting) {
            return;
        }
        if ctx.now <= self.timeout {
            return;
        }
        // TIMEOUT!
        let mut ignore_loss = false;
        if self.acked_seq_nr.wrapping_add(1) == self.mtu_seq
            && self.seq_nr.wrapping_sub(1) == self.mtu_seq
            && self.mtu_seq != 0
        {
            // we timed out, and the only outstanding packet we had was the probe. Assume it was
            // dropped because it was too big
            self.mtu_ceiling = self.mtu - 1;
            self.update_mtu_limits();
            ignore_loss = true;
        }
        if !self.outbuf.is_empty() {
            // num_timeouts is used to update the connection timeout, and if we lose this packet
            // because it's an MTU-probe, don't change the timeout
            if !ignore_loss {
                self.num_timeouts = self.num_timeouts.saturating_add(1);
            }
            ctx.counters.timeout += 1;
        }
        // a socket that has not been confirmed to actually have a live remote end (the IP may
        // have been spoofed) fail on the first timeout. If we had heard anything from this
        // peer, it would have been confirmed.
        if self.num_timeouts as i32 > ctx.settings.num_resends || (self.num_timeouts > 0 && !self.confirmed) {
            // the connection is dead
            self.fail(ctx, UtpError::TimedOut);
            return;
        }
        if !ignore_loss {
            // set cwnd to 1 MSS
            if self.bytes_in_flight == 0 && (self.cwnd >> 16) >= self.mtu as i64 {
                // this is just a timeout because this direction of the stream is idle. Don't
                // reset the cwnd, just decay it
                self.cwnd = (self.cwnd * 2 / 3).max((self.mtu as i64) << 16);
            } else {
                // we timed out because a packet was not ACKed or because the cwnd was made
                // smaller than one packet
                self.cwnd = (self.mtu as i64) << 16;
            }
            debug_assert!(self.cwnd >= 0);
            self.timeout = ctx.now + Duration::from_millis(self.packet_timeout(ctx) as u64);
            // since we've already timed out now, don't count loss that we might detect for
            // packets that just timed out
            self.loss_seq_nr = self.seq_nr;
            // when we time out, the cwnd is reset to 1 MSS, which means we need to ramp it up
            // quickly again. enter slow start mode. This time we're very likely to have an
            // ssthres set, which will make us leave slow start before inducing more delay or
            // loss.
            self.slow_start = true;
        }
        // we dropped all packets, that includes the mtu probe
        self.mtu_seq = 0;
        // we need to go one past seq_nr to cover the case where we just sent a SYN packet and
        // then adjusted for the uTorrent sequence number reuse
        let mut i = self.acked_seq_nr as u32 & ACK_MASK;
        let end = (self.seq_nr as u32 + 1) & ACK_MASK;
        while i != end {
            if let Some(p) = self.outbuf.at_mut(i) {
                if !p.need_resend {
                    p.need_resend = true;
                    let n = p.payload_len();
                    debug_assert!(self.bytes_in_flight >= n);
                    self.bytes_in_flight -= n;
                }
            }
            i = (i + 1) & ACK_MASK;
        }
        debug_assert_eq!(self.bytes_in_flight, 0);
        self.bytes_in_flight = 0;

        // if we have a packet that needs re-sending, resend it
        let next = self.acked_seq_nr.wrapping_add(1) as u32;
        if let Some(p) = self.outbuf.at(next) {
            let (transmissions, size) = (p.num_transmissions as i32, p.size);
            if transmissions >= ctx.settings.num_resends
                || (self.state == State::SynSent && transmissions >= ctx.settings.syn_resends)
                || (self.state == State::FinSent && transmissions >= ctx.settings.fin_resends)
            {
                // failed sends in a row. Socket timed out.
                if size > self.mtu_floor {
                    // the packet that caused the connection to fail was an mtu probe (note that
                    // the mtu_probe field won't be set at this point because it's cleared when
                    // the packet is re-sent). This suggests that perhaps our network throws
                    // away oversized packets without fragmenting them. Tell the socket manager
                    // to be more conservative about mtu ceiling in the future
                    ctx.restrict_mtu(self.mtu);
                }
                // the connection is dead
                self.fail(ctx, UtpError::TimedOut);
                return;
            }
            // don't fast-resend this packet
            if self.fast_resend_seq_nr == self.acked_seq_nr.wrapping_add(1) {
                self.fast_resend_seq_nr = self.fast_resend_seq_nr.wrapping_add(1);
            }
            // the packet timed out, resend it
            self.resend_packet(ctx, next, false);
            if matches!(self.state, State::ErrorWait | State::Deleting) {
                return;
            }
        } else if self.state < State::FinSent {
            self.send_pkt(ctx, 0);
            if matches!(self.state, State::ErrorWait | State::Deleting) {
                return;
            }
        } else if self.state == State::FinSent {
            // the connection is dead
            self.fail(ctx, UtpError::Eof);
            return;
        }
        self.maybe_wake_writer(ctx);
    }
}
