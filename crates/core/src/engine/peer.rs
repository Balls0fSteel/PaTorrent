// Ported from libtorrent-rasterbar 2.0.11: src/peer_connection.cpp (connection state, request
// queue, timeouts, statistics), src/receive_buffer.cpp, src/chained_buffer.cpp
// Copyright (c) 2003-2020, Arvid Norberg and contributors. BSD-3-Clause, see THIRD-PARTY-NOTICES.md.
// Changes: one struct owning a mio socket plus flat receive and send buffers; the message
// handling that needs the torrent lives in `engine::wire` as methods on the engine.

//! One BitTorrent peer connection.

use std::collections::{HashSet, VecDeque};
use std::io::{self, Read, Write};
use std::net::SocketAddr;
use std::time::Instant;

use mio::net::TcpStream;

use crate::proto::peer_wire::ExtHandshake;
use crate::proto::ut_metadata::PeerMetadata;
use crate::status::PeerSource;
use crate::types::TorrentId;
use crate::util::bitfield::Bitfield;
use crate::util::stat::Stat;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PeerState {
    /// Outgoing TCP connect in progress.
    Connecting,
    /// Waiting for the remote handshake.
    Handshake,
    Active,
}

/// An outstanding request we sent.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Request {
    pub piece: u32,
    pub block: u32,
}

pub(crate) struct PeerConn {
    pub id: u64,
    pub sock: TcpStream,
    pub addr: SocketAddr,
    pub outgoing: bool,
    pub source: PeerSource,
    pub torrent: Option<TorrentId>,
    pub state: PeerState,

    recv: Vec<u8>,
    rpos: usize,
    send: Vec<u8>,
    spos: usize,
    /// Stream offset of `send[0]` (bytes ever queued before it).
    send_base: u64,
    /// Stream offset where each queued piece payload ends, with its size, so upload payload is
    /// counted when it actually leaves.
    payload_marks: VecDeque<(u64, u32)>,

    pub am_choking: bool,
    pub am_interested: bool,
    pub peer_choking: bool,
    pub peer_interested: bool,
    pub peer_has: Bitfield,
    /// HAVE_ALL, or a full bitfield.
    pub peer_seed: bool,
    /// Bitfield / have-all / have-none seen (only one is allowed, first).
    pub got_bitfield: bool,
    pub supports_fast: bool,
    pub supports_extensions: bool,
    pub peer_id: [u8; 20],
    pub client: String,
    pub ext: Option<ExtHandshake>,
    /// BEP 9 metadata exchange with this peer.
    pub meta: PeerMetadata,

    pub requests: Vec<Request>,
    /// Requests from the peer not yet read from disk.
    pub upload_queue: VecDeque<(u32, u32, u32)>,
    /// Disk reads in flight for this peer.
    pub reads_in_flight: HashSet<(u32, u32, u32)>,
    /// BEP 6: pieces the peer lets us request while it chokes us.
    pub allowed_fast: Vec<u32>,
    /// BEP 6: pieces we let the peer request while we choke it.
    pub fast_set: Vec<u32>,

    pub stat: Stat,
    pub connected_at: Instant,
    pub last_receive: Instant,
    pub last_sent: Instant,
    pub last_piece: Instant,
    pub last_unchoked: Option<Instant>,
    pub last_optimistic: Option<Instant>,
    pub optimistic: bool,
    pub snubbed: bool,
    pub hashfails: u32,
    /// Stopped reading for lack of quota or disk space; retried by the bandwidth tick.
    pub read_blocked: bool,
    /// Bytes are waiting in the send buffer and the socket said WouldBlock.
    pub write_blocked: bool,
    /// Set once we decided to close; the engine removes the peer after the current event.
    pub closing: Option<(String, bool)>,
}

pub(crate) enum ReadOutcome {
    /// Read this many bytes (maybe zero if no quota).
    Data(usize),
    /// The socket has no more data now.
    WouldBlock(usize),
    Closed,
    Error(io::Error),
}

impl PeerConn {
    pub fn new(
        id: u64,
        sock: TcpStream,
        addr: SocketAddr,
        outgoing: bool,
        source: PeerSource,
        now: Instant,
    ) -> PeerConn {
        PeerConn {
            id,
            sock,
            addr,
            outgoing,
            source,
            torrent: None,
            state: if outgoing { PeerState::Connecting } else { PeerState::Handshake },
            recv: Vec::new(),
            rpos: 0,
            send: Vec::new(),
            spos: 0,
            send_base: 0,
            payload_marks: VecDeque::new(),
            am_choking: true,
            am_interested: false,
            peer_choking: true,
            peer_interested: false,
            peer_has: Bitfield::default(),
            peer_seed: false,
            got_bitfield: false,
            supports_fast: false,
            supports_extensions: false,
            peer_id: [0; 20],
            client: String::new(),
            ext: None,
            meta: PeerMetadata::default(),
            requests: Vec::new(),
            upload_queue: VecDeque::new(),
            reads_in_flight: HashSet::new(),
            allowed_fast: Vec::new(),
            fast_set: Vec::new(),
            stat: Stat::default(),
            connected_at: now,
            last_receive: now,
            last_sent: now,
            last_piece: now,
            last_unchoked: None,
            last_optimistic: None,
            optimistic: false,
            snubbed: false,
            hashfails: 0,
            read_blocked: false,
            write_blocked: false,
            closing: None,
        }
    }

    pub fn close(&mut self, reason: impl Into<String>, failed: bool) {
        if self.closing.is_none() {
            self.closing = Some((reason.into(), failed));
        }
    }

    /// Unprocessed received bytes.
    pub fn received(&self) -> &[u8] {
        &self.recv[self.rpos..]
    }

    pub fn consume(&mut self, n: usize) {
        self.rpos += n;
        if self.rpos == self.recv.len() {
            self.recv.clear();
            self.rpos = 0;
        }
    }

    /// Reads at most `max` bytes from the socket into the receive buffer.
    pub fn read_some(&mut self, max: usize) -> ReadOutcome {
        const CHUNK: usize = 64 * 1024;
        if self.rpos > 0 && self.rpos * 2 > self.recv.len() {
            self.recv.drain(..self.rpos);
            self.rpos = 0;
        }
        let mut total = 0;
        while total < max {
            let want = (max - total).min(CHUNK);
            let start = self.recv.len();
            self.recv.resize(start + want, 0);
            match self.sock.read(&mut self.recv[start..]) {
                Ok(0) => {
                    self.recv.truncate(start);
                    return if total > 0 { ReadOutcome::Data(total) } else { ReadOutcome::Closed };
                }
                Ok(n) => {
                    self.recv.truncate(start + n);
                    total += n;
                    if n < want {
                        // Short read: probably drained. mio requires reading until WouldBlock,
                        // so loop once more unless the quota is used up.
                        continue;
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    self.recv.truncate(start);
                    return ReadOutcome::WouldBlock(total);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => self.recv.truncate(start),
                Err(e) => {
                    self.recv.truncate(start);
                    return ReadOutcome::Error(e);
                }
            }
        }
        ReadOutcome::Data(total)
    }

    /// The send buffer, for the codec to append to.
    pub fn out(&mut self) -> &mut Vec<u8> {
        &mut self.send
    }

    /// Marks that the last `len` bytes appended were piece payload.
    pub fn mark_payload(&mut self, len: u32) {
        self.payload_marks.push_back((self.send_base + self.send.len() as u64, len));
    }

    pub fn send_buffer_len(&self) -> usize {
        self.send.len() - self.spos
    }

    /// Writes up to `max` buffered bytes. Returns (raw bytes, payload bytes) written and
    /// whether the socket would block. Errors close the connection.
    pub fn write_some(&mut self, max: usize) -> io::Result<(usize, usize, bool)> {
        let mut total = 0;
        let mut would_block = false;
        while self.spos < self.send.len() && total < max {
            // `max` is `usize::MAX` without a rate limit: saturate rather than overflow.
            let end = self.send.len().min(self.spos.saturating_add(max - total));
            match self.sock.write(&self.send[self.spos..end]) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "connection closed")),
                Ok(n) => {
                    self.spos += n;
                    total += n;
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    would_block = true;
                    break;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        let sent_pos = self.send_base + self.spos as u64;
        let mut payload = 0usize;
        while let Some(&(end, len)) = self.payload_marks.front() {
            if end > sent_pos {
                break;
            }
            payload += len as usize;
            self.payload_marks.pop_front();
        }
        if self.spos == self.send.len() || (self.spos > 64 * 1024 && self.spos * 2 > self.send.len()) {
            self.send_base += self.spos as u64;
            self.send.drain(..self.spos);
            self.spos = 0;
        }
        self.write_blocked = would_block;
        Ok((total, payload, would_block))
    }

    pub fn is_seed(&self) -> bool {
        self.peer_seed || (!self.peer_has.is_empty() && self.peer_has.all_set())
    }

    /// How many requests to keep outstanding (libtorrent `peer_connection::desired_queue_size`).
    pub fn desired_queue(&self, queue_time: u32, max: usize, block: u32) -> usize {
        if self.snubbed {
            return 1;
        }
        let rate = self.stat.download_payload.rate() as u64;
        let want = (rate * queue_time as u64 / block as u64) as usize;
        let peer_max = self.ext.as_ref().and_then(|e| e.reqq).map_or(250, |q| q as usize);
        want.clamp(4, max.min(peer_max).max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A rate-limited write leaves bytes behind; the next, unlimited write (`usize::MAX`) used to
    /// overflow `spos + max` (debug: panic; release: an out-of-range slice and an abort). Seen on
    /// the FreeBSD CI runner, where loopback writes block more often.
    #[test]
    fn unlimited_write_after_a_partial_one() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let out = std::net::TcpStream::connect(addr).unwrap();
        let (mut remote, _) = l.accept().unwrap();
        out.set_nonblocking(true).unwrap();
        let sock = TcpStream::from_std(out);
        let mut p = PeerConn::new(1, sock, addr, true, PeerSource::TRACKER, Instant::now());
        p.send.extend_from_slice(&[7u8; 100]);
        assert_eq!(p.write_some(10).unwrap().0, 10);
        assert_eq!(p.send_buffer_len(), 90);
        assert_eq!(p.write_some(usize::MAX).unwrap().0, 90);
        let mut got = [0u8; 100];
        remote.read_exact(&mut got).unwrap();
        assert!(got.iter().all(|&b| b == 7));
    }
}
