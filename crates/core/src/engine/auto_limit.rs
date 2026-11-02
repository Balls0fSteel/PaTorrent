//! Keep ping low (D49, `docs/LOWPING.md`): while `Settings::auto_rate_limit` is on, the line's
//! queue is measured with pings (`net::probe`) and the global class's limits are capped while it
//! grows past `auto_limit_delay_ms`, then raised again slowly. The user's own limits are never
//! changed: the global class gets the lower of the two.
//!
//! The queue is a target's round trip above its idle value: the median of its last five round
//! trips minus the lowest of the last three minutes. Each direction is cut, to 80% of its rate
//! over the last second (75% for the download, whose senders react more slowly), only when that
//! rate is at least half its cap, so a queue another program fills moves nothing. A cap grows
//! 5% a second while the queue stays under half the allowance and the rate uses it, and never
//! falls below 10 kB/s or 5% of the highest rate seen on this network.
//!
//! uTP peers measure the queue too, per direction (`UtpManager::shared_delays`): with at least
//! three busy ones, a direction's queue is the lower of the pings' and theirs, so a queue only
//! the other direction has does not cut this one. Without pings (none answer, or the system
//! gives none, as FreeBSD), the uTP peers' delays are the only signal.

use std::collections::VecDeque;
use std::net::IpAddr;
use std::time::{Duration, Instant};

use super::Engine;
use super::peer_class::{DOWNLOAD, PeerClassId, UPLOAD};
use crate::alert::Alert;
use crate::net::probe::{self, ProbeEvent, Prober};

/// How far back the idle round trip is looked for.
const BASELINE_WINDOW: Duration = Duration::from_secs(180);
/// Round trips the queue is the median of.
const RECENT: usize = 5;
/// A target whose last round trip is older than this tells nothing about the queue now.
const STALE: Duration = Duration::from_secs(3);
/// The controller's step.
const STEP: Duration = Duration::from_secs(1);
/// Seconds after a cut before the next one: the queue needs time to drain.
const HOLD_AFTER_CUT: u32 = 2;
/// Seconds of half rate when a new target is found, so its idle round trip is seen even while
/// the line is full.
const LEARN_SECS: u32 = 4;
/// The lowest cap: 10 kB/s, or 5% of the highest rate seen.
const MIN_CAP: u32 = 10_000;
/// The Logger hears of a cap change of more than 20%, at most once a minute.
const LOG_INTERVAL: Duration = Duration::from_secs(60);
/// uTP peers count for a direction while they measured its delay this recently.
const UTP_FRESH: Duration = Duration::from_secs(2);
/// uTP peers needed for their delays to say something about the line.
const UTP_PEERS: usize = 3;

/// A direction's queue in milliseconds from the pings' (both directions) and the uTP peers'
/// (this direction): the lower of the two when both measure, so the uTP peers tell which
/// direction a ping's queue is in; else whichever there is.
fn combine(probe: Option<f64>, utp: Option<f64>) -> Option<f64> {
    match (probe, utp) {
        (Some(p), Some(u)) => Some(p.min(u)),
        (p, u) => p.or(u),
    }
}

/// One target's round trips.
#[derive(Default)]
struct Target {
    /// The last [`BASELINE_WINDOW`] of round trips, for the idle value.
    window: VecDeque<(Instant, f64)>,
    recent: VecDeque<f64>,
}

impl Target {
    fn add(&mut self, now: Instant, ms: f64) {
        self.window.push_back((now, ms));
        while self.window.front().is_some_and(|&(t, _)| now.duration_since(t) > BASELINE_WINDOW) {
            self.window.pop_front();
        }
        self.recent.push_back(ms);
        if self.recent.len() > RECENT {
            self.recent.pop_front();
        }
    }

    /// The queue in milliseconds; `None` without a fresh round trip.
    fn queue(&self, now: Instant) -> Option<f64> {
        let &(last, _) = self.window.back()?;
        if now.duration_since(last) > STALE {
            return None;
        }
        let idle = self.window.iter().map(|&(_, ms)| ms).fold(f64::INFINITY, f64::min);
        Some((median(self.recent.iter().copied().collect()) - idle).max(0.0))
    }
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    match v.len() {
        0 => 0.0,
        n if n % 2 == 1 => v[n / 2],
        n => (v[n / 2 - 1] + v[n / 2]) / 2.0,
    }
}

/// The line's queue from the round trips of one or more targets (the fallback hosts each have
/// their own idle value; their queues' median is the line's).
#[derive(Default)]
pub(super) struct QueueMeter {
    targets: Vec<Target>,
}

impl QueueMeter {
    pub fn reset(&mut self, targets: usize) {
        self.targets = (0..targets.max(1)).map(|_| Target::default()).collect();
    }

    pub fn add(&mut self, now: Instant, target: usize, ms: f64) {
        if let Some(t) = self.targets.get_mut(target) {
            t.add(now, ms);
        }
    }

    pub fn queue(&self, now: Instant) -> Option<f64> {
        let queues: Vec<f64> = self.targets.iter().filter_map(|t| t.queue(now)).collect();
        (!queues.is_empty()).then(|| median(queues))
    }
}

/// One direction's cap.
#[derive(Clone, Debug)]
pub(super) struct Controller {
    /// What a cut keeps of the rate.
    factor: f64,
    /// Bytes per second; `None`: no cap.
    pub cap: Option<u32>,
    /// The highest rate seen on this network.
    peak: u32,
    /// Seconds before the controller acts again.
    hold: u32,
    /// The queue at the last cut, until it drains: while it shrinks, the cut is working and
    /// another would only sink the cap under the line.
    cut_queue: Option<f64>,
}

impl Controller {
    pub fn new(factor: f64) -> Controller {
        Controller { factor, cap: None, peak: 0, hold: 0, cut_queue: None }
    }

    fn floor(&self) -> u32 {
        MIN_CAP.max(self.peak / 20)
    }

    /// A new network: forget its highest rate.
    pub fn forget(&mut self) {
        self.peak = 0;
    }

    /// Half the rate for a few seconds, so the idle round trip shows (a new target).
    pub fn learn(&mut self, rate: u32) {
        if rate > self.floor() {
            self.cap = Some(self.floor().max(rate / 2));
            self.hold = LEARN_SECS;
        }
    }

    /// One second: `queue` the line's queue in milliseconds (`None`: unknown), `rate` our bytes
    /// in the last second, `limit` the user's own limit (0: none). Returns the cap.
    pub fn step(&mut self, queue: Option<f64>, allowance: f64, rate: u32, limit: u32) -> Option<u32> {
        self.peak = self.peak.max(rate);
        if self.hold > 0 {
            self.hold -= 1;
            return self.cap;
        }
        let Some(queue) = queue else { return self.cap };
        let ceiling = if limit > 0 { limit } else { u32::MAX };
        let current = self.cap.unwrap_or(ceiling).min(ceiling);
        let reference = if current == u32::MAX { self.peak } else { current };
        let floor = self.floor();
        if queue > allowance {
            // the queue is ours only while we move about as much as we may
            let draining = self.cut_queue.is_some_and(|q| queue < q * 0.9);
            if rate as f64 >= reference as f64 * 0.5 && rate > floor && !draining {
                self.cap = Some(floor.max((rate as f64 * self.factor) as u32));
                self.hold = HOLD_AFTER_CUT;
                self.cut_queue = Some(queue);
            }
        } else if queue < allowance / 2.0 {
            self.cut_queue = None;
            if let Some(cap) = self.cap {
                if rate as f64 >= cap as f64 * 0.75 {
                    let next = (cap as f64 * 1.05) as u32;
                    self.cap = (next < ceiling).then_some(next.max(cap + 1));
                }
            }
        }
        self.cap
    }
}

/// The running feature: the probes, the meter and a controller per direction.
pub(super) struct AutoLimit {
    prober: Option<Prober>,
    meter: QueueMeter,
    /// Per channel ([`UPLOAD`], [`DOWNLOAD`]).
    ctl: [Controller; 2],
    target: Option<IpAddr>,
    next_step: Instant,
    /// The session's byte totals at the last step, per channel.
    totals: [u64; 2],
    /// The last rate per channel, bytes per second.
    rate: [u32; 2],
    /// Per channel, the cap last logged and when.
    logged: [(Option<u32>, Option<Instant>); 2],
    /// The Logger heard that the uTP peers' delays are in use.
    utp_logged: bool,
}

impl AutoLimit {
    /// The caps while they are below the user's limits (0: none), per channel.
    pub fn caps(&self, limits: [u32; 2]) -> [u32; 2] {
        let mut out = [0; 2];
        for ch in [UPLOAD, DOWNLOAD] {
            if let Some(cap) = self.ctl[ch].cap {
                if limits[ch] == 0 || cap < limits[ch] {
                    out[ch] = cap;
                }
            }
        }
        out
    }
}

fn kb(bytes: u32) -> String {
    format!("{} kB/s", bytes.div_ceil(1000))
}

impl Engine {
    fn auto_log(&mut self, message: String) {
        self.alert(Alert::Log { id: None, message: format!("Keep ping low: {message}") });
    }

    /// The session's totals per channel, protocol included.
    fn totals(&self) -> [u64; 2] {
        let mut t = [0; 2];
        t[UPLOAD] = self.stat.total_upload();
        t[DOWNLOAD] = self.stat.total_download();
        t
    }

    /// Starts or stops the feature to match the setting, reads the probes and, once a second,
    /// moves the caps (the tick).
    pub(super) fn update_auto_limit(&mut self, now: Instant) {
        if !self.settings.auto_rate_limit {
            if self.auto_limit.take().is_some() {
                self.apply_auto_caps([None, None]);
                self.auto_log("off".into());
            }
            return;
        }
        if self.auto_limit.is_none() {
            let hosts = probe::parse_hosts(&self.settings.auto_limit_probe_hosts);
            let prober = Prober::start(hosts);
            if prober.is_none() {
                self.auto_log(probe::UNAVAILABLE.into());
            }
            self.auto_limit = Some(AutoLimit {
                prober,
                meter: QueueMeter::default(),
                ctl: [Controller::new(0.8), Controller::new(0.75)],
                target: None,
                next_step: now + STEP,
                totals: self.totals(),
                rate: [0; 2],
                logged: [(None, None); 2],
                utp_logged: false,
            });
        }
        let Some(mut a) = self.auto_limit.take() else { return };
        let events: Vec<ProbeEvent> = a.prober.as_ref().map(|p| p.events().collect()).unwrap_or_default();
        let mut log = Vec::new();
        for e in events {
            match e {
                ProbeEvent::Target { addr, hop, fallback } => {
                    let hosts = probe::parse_hosts(&self.settings.auto_limit_probe_hosts);
                    a.meter.reset(if fallback { hosts.len() } else { 1 });
                    log.push(match (addr, hop) {
                        (Some(addr), Some(hop)) => {
                            format!("pinging the first router past the line, {addr} (hop {hop})")
                        }
                        _ if fallback => {
                            let list: Vec<String> = hosts.iter().map(IpAddr::to_string).collect();
                            format!("no router past the line answers; pinging {}", list.join(", "))
                        }
                        _ => "nothing answers pings; only the uTP peers' delays until the network changes"
                            .into(),
                    });
                    if addr != a.target || addr.is_none() {
                        // a new network: its rates and idle round trip are learned afresh
                        for ch in [UPLOAD, DOWNLOAD] {
                            a.ctl[ch].forget();
                            a.ctl[ch].learn(a.rate[ch]);
                        }
                    }
                    a.target = addr;
                }
                ProbeEvent::Rtt { target, ms } => a.meter.add(now, target, ms),
            }
        }
        if now >= a.next_step {
            let elapsed = now.duration_since(a.next_step - STEP).as_secs_f64().max(0.001);
            a.next_step = now + STEP;
            let totals = self.totals();
            let probe = a.meter.queue(now);
            let utp = self.utp_queues(now);
            if !a.utp_logged && utp.iter().any(Option::is_some) {
                a.utp_logged = true;
                log.push("uTP peers' delays in use".into());
            }
            let allowance = self.settings.auto_limit_delay_ms.max(1) as f64;
            let limits = self.user_limits();
            for ch in [UPLOAD, DOWNLOAD] {
                a.rate[ch] = ((totals[ch] - a.totals[ch]) as f64 / elapsed).min(u32::MAX as f64) as u32;
                a.ctl[ch].step(combine(probe, utp[ch]), allowance, a.rate[ch], limits[ch]);
            }
            a.totals = totals;
        }
        let caps = a.caps(self.user_limits());
        for (ch, name) in [(UPLOAD, "upload"), (DOWNLOAD, "download")] {
            let cap = (caps[ch] > 0).then_some(caps[ch]);
            let (last, at) = a.logged[ch];
            let big = match (last, cap) {
                (Some(l), Some(c)) => (l as f64 - c as f64).abs() > l as f64 * 0.2,
                (l, c) => l != c,
            };
            if big && at.is_none_or(|at| now.duration_since(at) >= LOG_INTERVAL) {
                a.logged[ch] = (cap, Some(now));
                log.push(match cap {
                    Some(c) => format!("{name} limited to {}", kb(c)),
                    None => format!("{name} no longer limited"),
                });
            }
        }
        self.auto_limit = Some(a);
        self.apply_auto_caps([caps[UPLOAD], caps[DOWNLOAD]].map(|c| (c > 0).then_some(c)));
        for m in log {
            self.auto_log(m);
        }
    }

    /// Per channel, the queue the busy uTP peers share, in milliseconds (`None` with fewer
    /// than [`UTP_PEERS`] of them).
    fn utp_queues(&self, now: Instant) -> [Option<f64>; 2] {
        let since = now.checked_sub(UTP_FRESH).unwrap_or(now);
        // the manager's order is upload, download: the channels'
        let delays = self.utp.shared_delays(since);
        let mut out = [None; 2];
        out[UPLOAD] = (delays[0].1 >= UTP_PEERS).then(|| delays[0].0 as f64 / 1000.0);
        out[DOWNLOAD] = (delays[1].1 >= UTP_PEERS).then(|| delays[1].0 as f64 / 1000.0);
        out
    }

    /// The session's own limits per channel.
    fn user_limits(&self) -> [u32; 2] {
        let mut l = [0; 2];
        l[UPLOAD] = self.settings.upload_rate_limit;
        l[DOWNLOAD] = self.settings.download_rate_limit;
        l
    }

    /// Sets the global class's limits to the lower of the user's and the caps.
    fn apply_auto_caps(&mut self, caps: [Option<u32>; 2]) {
        let limits = self.user_limits();
        for ch in [UPLOAD, DOWNLOAD] {
            let want = match caps[ch] {
                Some(c) if limits[ch] == 0 || c < limits[ch] => c,
                _ => limits[ch],
            };
            if self.class_channel(PeerClassId::GLOBAL, ch).is_some_and(|b| b.limit != want) {
                self.set_rate_limit(PeerClassId::GLOBAL, ch, want);
            }
        }
    }

    /// The network changed: search for the router again.
    pub(super) fn auto_limit_network_changed(&mut self) {
        if let Some(p) = self.auto_limit.as_ref().and_then(|a| a.prober.as_ref()) {
            p.rediscover();
        }
    }

    /// The caps for the session stats (0: none).
    pub(super) fn auto_caps(&self) -> [u32; 2] {
        self.auto_limit.as_ref().map_or([0; 2], |a| a.caps(self.user_limits()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line of `capacity` bytes per second with a deep buffer: the queue grows while more is
    /// offered than it carries.
    struct Line {
        capacity: f64,
        buffered: f64,
    }

    impl Line {
        /// Offers `want` bytes for a second; returns what got through and the queue in ms.
        fn second(&mut self, want: f64) -> (f64, f64) {
            self.buffered = (self.buffered + want - self.capacity).max(0.0);
            // a 2 MB buffer, as bloated modems have
            self.buffered = self.buffered.min(2_000_000.0);
            (want.min(self.capacity + self.buffered), self.buffered / self.capacity * 1000.0)
        }
    }

    #[test]
    fn the_cap_settles_under_the_line() {
        let mut line = Line { capacity: 1_000_000.0, buffered: 0.0 };
        let mut c = Controller::new(0.8);
        let mut queue = 0.0;
        let mut caps = Vec::new();
        for _ in 0..300 {
            // the peers would take 5 MB/s
            let want = c.cap.map_or(5_000_000.0, |c| c as f64);
            let (_, q) = line.second(want);
            c.step(Some(queue), 15.0, want.min(5_000_000.0) as u32, 0);
            queue = q;
            caps.push(c.cap);
        }
        let last: Vec<u32> = caps[200..].iter().map(|c| c.expect("capped")).collect();
        let (lo, hi) = (*last.iter().min().unwrap(), *last.iter().max().unwrap());
        // around the line's rate (probing a little above it before each cut), never far under
        assert!(lo >= 600_000 && hi <= 1_150_000, "{lo}..{hi}");
        let mean = last.iter().map(|&c| c as f64).sum::<f64>() / last.len() as f64;
        assert!((800_000.0..=1_050_000.0).contains(&mean), "{mean}");
    }

    #[test]
    fn someone_elses_queue_moves_nothing() {
        let mut c = Controller::new(0.8);
        // a high rate seen once, then little sent while the queue is high
        c.step(Some(0.0), 15.0, 1_000_000, 0);
        for _ in 0..30 {
            assert_eq!(c.step(Some(200.0), 15.0, 50_000, 0), None);
        }
    }

    #[test]
    fn floors_and_the_users_limit() {
        let mut c = Controller::new(0.8);
        c.step(Some(0.0), 15.0, 2_000_000, 0);
        // cut after cut: never under 5% of the peak
        for _ in 0..60 {
            let rate = c.cap.unwrap_or(2_000_000);
            c.step(Some(500.0), 15.0, rate, 0);
        }
        assert_eq!(c.cap, Some(100_000));
        // growing back past the user's limit drops the cap
        for _ in 0..200 {
            let rate = c.cap.unwrap_or(300_000);
            c.step(Some(0.0), 15.0, rate, 300_000);
        }
        assert_eq!(c.cap, None);
        // a tiny peak: the 10 kB/s floor
        let mut c = Controller::new(0.8);
        c.step(Some(0.0), 15.0, 30_000, 0);
        for _ in 0..30 {
            let rate = c.cap.unwrap_or(30_000);
            c.step(Some(500.0), 15.0, rate, 0);
        }
        assert_eq!(c.cap, Some(MIN_CAP));
    }

    #[test]
    fn unknown_queue_holds_and_learning_halves() {
        let mut c = Controller::new(0.75);
        c.step(Some(0.0), 15.0, 400_000, 0);
        c.learn(400_000);
        assert_eq!(c.cap, Some(200_000));
        for _ in 0..LEARN_SECS {
            assert_eq!(c.step(Some(500.0), 15.0, 200_000, 0), Some(200_000));
        }
        assert_eq!(c.step(None, 15.0, 200_000, 0), Some(200_000));
        assert_eq!(c.step(Some(0.0), 15.0, 200_000, 0), Some(210_000));
    }

    #[test]
    fn utp_delays_tell_the_directions_apart() {
        // the pings see a queue, the uTP peers place it in the download
        assert_eq!(combine(Some(80.0), Some(2.0)), Some(2.0));
        assert_eq!(combine(Some(80.0), Some(70.0)), Some(70.0));
        // the pings say the line is idle: a single peer's path does not overrule them
        assert_eq!(combine(Some(1.0), Some(40.0)), Some(1.0));
        // one signal alone
        assert_eq!(combine(None, Some(40.0)), Some(40.0));
        assert_eq!(combine(Some(40.0), None), Some(40.0));
        assert_eq!(combine(None, None), None);
    }

    #[test]
    fn the_meter_takes_the_median_above_the_idle_round_trip() {
        let t0 = Instant::now();
        let mut m = QueueMeter::default();
        m.reset(1);
        assert_eq!(m.queue(t0), None);
        for (i, ms) in [10.0, 12.0, 11.0, 90.0, 40.0, 42.0, 41.0].into_iter().enumerate() {
            m.add(t0 + Duration::from_millis(250 * i as u64), 0, ms);
        }
        let now = t0 + Duration::from_millis(1500);
        // the last five: 11, 90, 40, 42, 41; median 41, idle 10
        assert_eq!(m.queue(now), Some(31.0));
        assert_eq!(m.queue(now + Duration::from_secs(5)), None);
        // the idle value ages out after three minutes
        let later = t0 + Duration::from_secs(200);
        m.add(later, 0, 41.0);
        assert_eq!(m.queue(later), Some(0.0));
        // several hosts: each against its own idle value
        m.reset(2);
        m.add(t0, 0, 10.0);
        m.add(t0, 1, 30.0);
        m.add(t0 + STEP, 0, 20.0);
        m.add(t0 + STEP, 1, 40.0);
        assert_eq!(m.queue(t0 + STEP), Some(5.0));
    }
}
