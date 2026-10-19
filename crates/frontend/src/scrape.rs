//! Swarm counts for torrents that do not announce. µTorrent 2.2.1 showed the tracker's counts
//! of stopped and queued torrents too, in the Seeds and Peers columns as "0 (123)"; the engine
//! learns them only from announces (and the scrape after an announce without them), so
//! [`Scrapes`] asks the trackers of paused torrents for them now and then.
//!
//! The rules:
//!
//! * Only torrents that are paused (stopped, paused or queued) and have trackers are scraped;
//!   running torrents get their counts from their own announces.
//! * Each is scraped every [`ScrapeConfig::interval`], never more often than the longest
//!   announce interval its trackers asked for (`TorrentStatus::announce_interval`, known once a
//!   tracker answered an announce in this session), plus a fixed per-torrent stagger of up to an
//!   eighth of that, so torrents scraped together drift apart.
//! * A torrent whose counts are unknown (restored at startup, added stopped) is due at once; one
//!   stopped after running keeps its announce's counts for an interval first.
//! * Due torrents are collected and asked for in one [`SessionHandle::scrape_trackers`]
//!   command, so the engine sends their UDP scrapes in requests of up to 74 info-hashes per
//!   tracker; the next round comes [`ScrapeConfig::round`] later, or [`ScrapeConfig::check`]
//!   later when nothing was due (so a torrent added stopped gets its counts within seconds). At most [`ScrapeConfig::max_per_round`] go in
//!   a round, the longest-waiting first; the rest wait for the next round, which spreads the
//!   first pass over a big library and keeps HTTP trackers (one request per torrent) from
//!   getting a flood.
//!
//! Replies need no handling here: the engine puts the counts into the torrent's status
//! (`num_complete` / `num_incomplete`), which reaches the list with the next status update. The
//! frontends leave scrape alerts out of their Logger, as there is one per torrent and interval.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use patorrent_core::{SessionHandle, TorrentFlags, TorrentId, TorrentStatus};

/// How often [`Scrapes`] asks, and how much at once.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScrapeConfig {
    /// Time between two scrapes of one torrent, at least (the trackers' own announce interval
    /// raises it).
    pub interval: Duration,
    /// Due torrents are collected and asked for together this often.
    pub round: Duration,
    /// After a round that found nothing due, the next one comes this soon.
    pub check: Duration,
    /// Torrents asked for in one round, at most.
    pub max_per_round: usize,
}

impl Default for ScrapeConfig {
    /// 30 minutes (the usual tracker announce interval), rounds of 30 s (5 s while nothing is
    /// due), 200 torrents a round:
    /// 24,000 scrapes an hour at most, so libraries past about 12,000 paused torrents are
    /// scraped less often than every 30 minutes.
    fn default() -> ScrapeConfig {
        ScrapeConfig {
            interval: Duration::from_secs(30 * 60),
            round: Duration::from_secs(30),
            check: Duration::from_secs(5),
            max_per_round: 200,
        }
    }
}

/// The periodic scrape of paused torrents (see the module documentation).
#[derive(Debug, Default)]
pub struct Scrapes {
    config: ScrapeConfig,
    /// When each paused torrent with trackers is next scraped.
    due: HashMap<TorrentId, Instant>,
    next_round: Option<Instant>,
}

impl Scrapes {
    pub fn new(config: ScrapeConfig) -> Scrapes {
        Scrapes { config, due: HashMap::new(), next_round: None }
    }

    /// Whether this torrent is scraped by us: it does not announce (paused, which includes
    /// queued torrents) and has trackers.
    pub fn wanted(st: &TorrentStatus) -> bool {
        st.flags.contains(TorrentFlags::PAUSED) && st.num_trackers > 0
    }

    /// The time from one scrape of this torrent to its next.
    pub fn interval(&self, st: &TorrentStatus) -> Duration {
        let base = self.config.interval.max(Duration::from_secs(st.announce_interval.unwrap_or(0) as u64));
        let mut h = std::collections::hash_map::DefaultHasher::new();
        st.id.hash(&mut h);
        let eighth = base.as_millis() as u64 / 8;
        base + Duration::from_millis(if eighth == 0 { 0 } else { h.finish() % eighth })
    }

    /// The torrents to scrape now, longest waiting first. Call it on every status tick with
    /// every torrent's latest status; it looks at them once per round.
    pub fn due<'a>(
        &mut self,
        torrents: impl IntoIterator<Item = &'a TorrentStatus>,
        now: Instant,
    ) -> Vec<TorrentId> {
        if self.next_round.is_some_and(|t| now < t) {
            return Vec::new();
        }
        let mut due = HashMap::with_capacity(self.due.len());
        let mut ready = Vec::new();
        for st in torrents.into_iter().filter(|st| Self::wanted(st)) {
            let interval = self.interval(st);
            let at = self.due.get(&st.id).copied().unwrap_or_else(|| {
                // counts from a recent announce (or scrape) are fresh for an interval
                if st.num_complete.is_some() || st.num_incomplete.is_some() { now + interval } else { now }
            });
            if at <= now {
                ready.push((at, st.id, interval));
            }
            due.insert(st.id, at);
        }
        ready.sort_unstable_by_key(|&(at, id, _)| (at, id));
        ready.truncate(self.config.max_per_round);
        for &(_, id, interval) in &ready {
            due.insert(id, now + interval);
        }
        self.due = due;
        self.next_round = Some(now + if ready.is_empty() { self.config.check } else { self.config.round });
        ready.into_iter().map(|(_, id, _)| id).collect()
    }

    /// [`Scrapes::due`], asked of the session in one command.
    pub fn run<'a>(
        &mut self,
        torrents: impl IntoIterator<Item = &'a TorrentStatus>,
        now: Instant,
        session: &SessionHandle,
    ) {
        session.scrape_trackers(self.due(torrents, now));
    }

    /// How many torrents wait for a scrape (tests and diagnostics).
    pub fn len(&self) -> usize {
        self.due.len()
    }

    pub fn is_empty(&self) -> bool {
        self.due.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::columns::{Column, RowExtra};

    fn st(raw: u64, flags: TorrentFlags, trackers: u32) -> TorrentStatus {
        TorrentStatus {
            id: TorrentId::from_raw(raw),
            flags,
            num_trackers: trackers,
            ..TorrentStatus::default()
        }
    }

    const STOPPED: TorrentFlags = TorrentFlags::PAUSED;
    const QUEUED: TorrentFlags =
        TorrentFlags::from_bits_truncate(TorrentFlags::PAUSED.bits() | TorrentFlags::AUTO_MANAGED.bits());

    fn config() -> ScrapeConfig {
        ScrapeConfig {
            interval: Duration::from_secs(1800),
            round: Duration::from_secs(30),
            check: Duration::from_secs(5),
            max_per_round: 3,
        }
    }

    fn ids(v: &[u64]) -> Vec<TorrentId> {
        v.iter().map(|&r| TorrentId::from_raw(r)).collect()
    }

    #[test]
    fn paused_torrents_with_trackers_are_scraped_once_per_interval() {
        let mut s = Scrapes::new(config());
        let t0 = Instant::now();
        let list = [
            st(1, STOPPED, 1),
            st(2, QUEUED, 2),
            st(3, TorrentFlags::AUTO_MANAGED, 1), // running
            st(4, STOPPED, 0),                    // no trackers
        ];
        let mut got = s.due(&list, t0);
        got.sort();
        assert_eq!(got, ids(&[1, 2]));
        assert_eq!(s.len(), 2);
        // within the round nothing happens, and in the next ones they are not due yet
        assert!(s.due(&list, t0 + Duration::from_secs(1)).is_empty());
        assert!(s.due(&list, t0 + Duration::from_secs(30)).is_empty());
        assert!(s.due(&list, t0 + Duration::from_secs(1799)).is_empty());
        // due again after the interval and its stagger (at most an eighth more)
        let mut later = s.due(&list, t0 + Duration::from_secs(1800 + 1800 / 8 + 1));
        later.sort();
        assert_eq!(later, ids(&[1, 2]));
        let i = s.interval(&list[0]);
        assert!(i >= Duration::from_secs(1800) && i < Duration::from_secs(1800 + 225), "{i:?}");
    }

    #[test]
    fn a_round_takes_the_longest_waiting_first_and_the_rest_follow() {
        let mut s = Scrapes::new(config());
        let t0 = Instant::now();
        let list: Vec<_> = (1..=7).map(|r| st(r, STOPPED, 1)).collect();
        let first = s.due(&list, t0);
        assert_eq!(first.len(), 3);
        let second = s.due(&list, t0 + Duration::from_secs(30));
        assert_eq!(second.len(), 3);
        let third = s.due(&list, t0 + Duration::from_secs(60));
        assert_eq!(third.len(), 1);
        let mut all: Vec<_> = first.iter().chain(&second).chain(&third).copied().collect();
        all.sort();
        assert_eq!(all, ids(&[1, 2, 3, 4, 5, 6, 7]), "each once");
        assert!(s.due(&list, t0 + Duration::from_secs(90)).is_empty());
    }

    #[test]
    fn the_trackers_announce_interval_is_a_floor() {
        let mut s = Scrapes::new(config());
        let t0 = Instant::now();
        // a private tracker that wants an hour between announces
        let mut slow = st(1, STOPPED, 1);
        slow.announce_interval = Some(3600);
        // one that asked for less than our interval changes nothing
        let mut quick = st(2, STOPPED, 1);
        quick.announce_interval = Some(600);
        let list = [slow, quick];
        assert_eq!(s.due(&list, t0).len(), 2);
        assert_eq!(s.due(&list, t0 + Duration::from_secs(1800 + 226)), ids(&[2]));
        assert!(s.due(&list, t0 + Duration::from_secs(3599)).is_empty());
        // (the quick one may be due again by then too)
        assert!(s.due(&list, t0 + Duration::from_secs(3600 + 451)).contains(&TorrentId::from_raw(1)));
        assert!(s.interval(&list[0]) >= Duration::from_secs(3600));
    }

    #[test]
    fn known_counts_wait_an_interval_and_running_torrents_are_forgotten() {
        let mut s = Scrapes::new(config());
        let t0 = Instant::now();
        // stopped after running: its announce's counts are fresh
        let mut stopped = st(1, STOPPED, 1);
        stopped.num_complete = Some(5);
        assert!(s.due([&stopped], t0).is_empty());
        assert_eq!(s.len(), 1);
        // started: forgotten; stopped again later: waits an interval from then
        let running = st(1, TorrentFlags::empty(), 1);
        assert!(s.due([&running], t0 + Duration::from_secs(30)).is_empty());
        assert!(s.is_empty());
        let t1 = t0 + Duration::from_secs(60);
        assert!(s.due([&stopped], t1).is_empty());
        // a torrent added stopped: picked up at the next check, not a whole round later
        let added = st(2, STOPPED, 1);
        assert!(s.due([&stopped, &added], t1 + Duration::from_secs(4)).is_empty(), "within the check");
        assert_eq!(s.due([&stopped, &added], t1 + Duration::from_secs(5)), ids(&[2]));
        assert!(s.due([&stopped], t1 + Duration::from_secs(1799)).is_empty());
        assert_eq!(s.due([&stopped], t1 + Duration::from_secs(1800 + 226)), ids(&[1]));
        // removed torrents are dropped
        assert!(s.due([], t1 + Duration::from_secs(4000)).is_empty());
        assert!(s.is_empty());
    }

    /// Through the mock engine: stopped and queued torrents get their counts in the Seeds and
    /// Peers columns, a running one and one without trackers are left alone.
    #[test]
    fn the_mock_engine_fills_the_seeds_and_peers_columns() {
        use patorrent_core::{AddTorrentParams, Alert, MockConfig, Session, Settings, TorrentSource};

        // no download slots, so the auto-managed torrent stays queued
        let settings =
            Settings { tick_interval: 50, active_downloads: 0, active_limit: 0, ..Settings::default() };
        let s = Session::start_mock(settings, MockConfig::default());
        let h = s.handle();
        let flags = [STOPPED, QUEUED, TorrentFlags::empty(), STOPPED];
        for (i, f) in flags.iter().enumerate() {
            let mut p = AddTorrentParams::new(
                TorrentSource::Magnet(format!("magnet:?xt=urn:btih:{:040x}", 0xab00 + i)),
                "/dl",
            );
            p.flags = *f;
            h.add_torrent(p);
        }
        let mut list = crate::model::TorrentList::new();
        let mut added = Vec::new();
        let mut alerts = Vec::new();
        let mut replies = Vec::new();
        let mut scrapes = Scrapes::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut asked = false;
        loop {
            assert!(
                Instant::now() < deadline,
                "timed out: {added:?} {replies:?} {asked} {:?}",
                added
                    .iter()
                    .map(|id| list.get(*id).map(|st| (st.flags, st.num_trackers)))
                    .collect::<Vec<_>>()
            );
            std::thread::sleep(Duration::from_millis(20));
            h.post_torrent_updates();
            h.pop_alerts(&mut alerts);
            for a in alerts.drain(..) {
                match a {
                    Alert::TorrentAdded { id, .. } => {
                        added.push(id);
                        if added.len() == 4 {
                            h.torrent(added[3]).replace_trackers(Vec::new());
                        }
                    }
                    Alert::StateUpdate(v) => list.update(v),
                    Alert::ScrapeReply { id, complete, incomplete, .. } => {
                        replies.push((id, complete, incomplete))
                    }
                    _ => {}
                }
            }
            let ready = added.len() == 4
                && added.iter().all(|id| list.get(*id).is_some())
                && list.get(added[3]).is_some_and(|st| st.num_trackers == 0);
            if ready && !asked {
                let rows: Vec<_> = list.ids().filter_map(|id| list.get(id)).cloned().collect();
                scrapes.run(&rows, Instant::now(), &h);
                asked = true;
            }
            let done = replies.len() == 2
                && replies
                    .iter()
                    .all(|(id, c, _)| list.get(*id).is_some_and(|st| st.num_complete == Some(*c)));
            if asked && done {
                break;
            }
        }
        let mut scraped: Vec<_> = replies.iter().map(|r| r.0).collect();
        scraped.sort();
        let mut want = vec![added[0], added[1]];
        want.sort();
        assert_eq!(scraped, want, "the stopped and the queued torrent only");
        for (id, complete, incomplete) in replies {
            let st = list.get(id).unwrap();
            assert_eq!(Column::Seeds.text(st, RowExtra::default()), format!("0 ({complete})"));
            assert_eq!(Column::Peers.text(st, RowExtra::default()), format!("0 ({incomplete})"));
        }
        s.shutdown();
    }
}
