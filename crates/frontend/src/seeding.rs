//! The seeding goal (µTorrent 2.2.1 Preferences > Queueing "Seeding Goal", and the per-torrent
//! "Override default settings" in Properties): once a queued torrent has seeded to a minimum
//! ratio and for a minimum time, it is stopped, or held to an upload rate.
//!
//! libtorrent's `share_ratio_limit` and `seed_time_limit` only rank seeds in the queue, so
//! this is the client's job, as in µTorrent and qBittorrent: [`SeedingGoal::check`] looks at
//! each status update and says what to do. Only torrents the queue manages are touched; a
//! force-started torrent seeds on, as in µTorrent.
//!
//! Defaults and the meaning of the two values follow µTorrent: a ratio of 150 % and no minimum
//! time; 0 switches a target off, and with both off there is no goal. With both set, both must
//! be reached ("minimum" ratio and "minimum" time). "Limit the upload rate to X kB/s [0: stop]"
//! off, or at 0, stops the torrent; at X it caps the torrent's upload rate instead.

use patorrent_core::{TorrentFlags, TorrentHandle, TorrentState, TorrentStatus};

use crate::prefs::Prefs;

/// One goal: share ratio in percent and seeding time in minutes; 0 = that target is off.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SeedGoal {
    pub ratio_pct: u32,
    pub minutes: u32,
}

impl Default for SeedGoal {
    fn default() -> SeedGoal {
        SeedGoal { ratio_pct: 150, minutes: 0 }
    }
}

impl SeedGoal {
    /// At least one target is set.
    pub fn is_set(&self) -> bool {
        self.ratio_pct > 0 || self.minutes > 0
    }

    /// Every target that is set has been reached.
    pub fn reached(&self, st: &TorrentStatus) -> bool {
        if !self.is_set() {
            return false;
        }
        let ratio_ok = self.ratio_pct == 0 || st.ratio() * 100.0 >= self.ratio_pct as f64;
        let time_ok = self.minutes == 0 || st.seeding_duration >= self.minutes as u64 * 60;
        ratio_ok && time_ok
    }

    /// A torrent's own goal, if it overrides the default (`torrent.<info-hash>.seed_ratio` and
    /// `.seed_time` in the preferences, beside its label).
    pub fn load_override(prefs: &Prefs, key: &str) -> Option<SeedGoal> {
        let ratio = prefs.get_i32(&format!("torrent.{key}.seed_ratio"))?;
        let minutes = prefs.get_i32(&format!("torrent.{key}.seed_time")).unwrap_or(0);
        Some(SeedGoal { ratio_pct: ratio.max(0) as u32, minutes: minutes.max(0) as u32 })
    }

    /// Sets (`Some`) or removes a torrent's own goal.
    pub fn store_override(prefs: &mut Prefs, key: &str, goal: Option<SeedGoal>) {
        let (r, t) = (format!("torrent.{key}.seed_ratio"), format!("torrent.{key}.seed_time"));
        match goal {
            Some(g) => {
                prefs.set(&r, g.ratio_pct);
                prefs.set(&t, g.minutes);
            }
            None => {
                prefs.remove(&r);
                prefs.remove(&t);
            }
        }
    }
}

/// What to do with a torrent that reached its goal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GoalAction {
    /// µTorrent's Stop: out of the queue and paused (it then shows as "Finished").
    Stop,
    /// Cap the torrent's upload rate, in bytes per second.
    LimitUpload(u32),
}

impl GoalAction {
    pub fn apply(self, t: &TorrentHandle) {
        match self {
            GoalAction::Stop => crate::actions::stop(t),
            GoalAction::LimitUpload(bytes) => t.set_upload_limit(bytes),
        }
    }
}

/// The default goal and what happens when a torrent reaches one (Preferences > Queueing).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SeedingGoal {
    pub goal: SeedGoal,
    /// "Limit the upload rate to" instead of stopping.
    pub limit_upload: bool,
    /// kB/s; 0 = stop.
    pub limit_kib: u32,
}

impl SeedingGoal {
    pub fn action(&self) -> GoalAction {
        if self.limit_upload && self.limit_kib > 0 {
            GoalAction::LimitUpload(self.limit_kib.saturating_mul(1024))
        } else {
            GoalAction::Stop
        }
    }

    /// What to do with this torrent now, given its own goal (`own`, else the default): `None`
    /// unless it is a queued torrent seeding past its goal that has not been dealt with yet.
    pub fn check(&self, st: &TorrentStatus, own: Option<SeedGoal>) -> Option<GoalAction> {
        let managed =
            st.flags.contains(TorrentFlags::AUTO_MANAGED) && !st.flags.contains(TorrentFlags::PAUSED);
        let checking = matches!(st.state, TorrentState::CheckingFiles | TorrentState::CheckingResumeData);
        if !managed || checking || !st.is_finished || !st.error.is_empty() || st.moving_storage {
            return None;
        }
        if !own.unwrap_or(self.goal).reached(st) {
            return None;
        }
        match self.action() {
            GoalAction::LimitUpload(b) if st.upload_limit > 0 && st.upload_limit <= b => None,
            a => Some(a),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed(up: u64, size: u64, seeded_secs: u64) -> TorrentStatus {
        TorrentStatus {
            flags: TorrentFlags::AUTO_MANAGED,
            state: TorrentState::Seeding,
            is_finished: true,
            is_seeding: true,
            all_time_upload: up,
            all_time_download: size,
            total_wanted_done: size,
            seeding_duration: seeded_secs,
            ..Default::default()
        }
    }

    #[test]
    fn goals_are_reached_when_every_target_set_is() {
        let g = SeedGoal::default();
        assert_eq!((g.ratio_pct, g.minutes), (150, 0));
        assert!(!g.reached(&seed(140, 100, 0)));
        assert!(g.reached(&seed(150, 100, 0)));
        let both = SeedGoal { ratio_pct: 100, minutes: 10 };
        assert!(!both.reached(&seed(200, 100, 599)), "the time is not up yet");
        assert!(both.reached(&seed(200, 100, 600)));
        let time_only = SeedGoal { ratio_pct: 0, minutes: 1 };
        assert!(time_only.reached(&seed(0, 100, 60)));
        assert!(!SeedGoal { ratio_pct: 0, minutes: 0 }.reached(&seed(1000, 1, 1_000_000)), "no goal");
    }

    #[test]
    fn only_queued_seeds_past_their_goal_are_stopped_or_limited() {
        let stop = SeedingGoal::default();
        let st = seed(300, 100, 10);
        assert_eq!(stop.check(&st, None), Some(GoalAction::Stop));
        // a torrent's own goal wins over the default
        assert_eq!(stop.check(&st, Some(SeedGoal { ratio_pct: 400, minutes: 0 })), None);
        assert_eq!(stop.check(&st, Some(SeedGoal { ratio_pct: 0, minutes: 0 })), None, "seed forever");
        // forced, stopped, still downloading or checking: left alone
        let forced = TorrentStatus { flags: TorrentFlags::empty(), ..st.clone() };
        assert_eq!(stop.check(&forced, None), None);
        let stopped =
            TorrentStatus { flags: TorrentFlags::PAUSED | TorrentFlags::AUTO_MANAGED, ..st.clone() };
        assert_eq!(stop.check(&stopped, None), None);
        let unfinished = TorrentStatus { is_finished: false, state: TorrentState::Downloading, ..st.clone() };
        assert_eq!(stop.check(&unfinished, None), None);
        let checking = TorrentStatus { state: TorrentState::CheckingFiles, ..st.clone() };
        assert_eq!(stop.check(&checking, None), None);

        // "Limit the upload rate to 0" stops too; another rate caps it once
        let zero = SeedingGoal { limit_upload: true, limit_kib: 0, ..stop };
        assert_eq!(zero.check(&st, None), Some(GoalAction::Stop));
        let cap = SeedingGoal { limit_upload: true, limit_kib: 5, ..stop };
        assert_eq!(cap.check(&st, None), Some(GoalAction::LimitUpload(5120)));
        let capped = TorrentStatus { upload_limit: 5120, ..st.clone() };
        assert_eq!(cap.check(&capped, None), None);
        let lower = TorrentStatus { upload_limit: 1024, ..st.clone() };
        assert_eq!(cap.check(&lower, None), None, "a lower limit of its own stays");
        let higher = TorrentStatus { upload_limit: 50 * 1024, ..st };
        assert_eq!(cap.check(&higher, None), Some(GoalAction::LimitUpload(5120)));
    }

    #[test]
    fn overrides_live_in_the_preferences() {
        let mut prefs = Prefs::default();
        assert_eq!(SeedGoal::load_override(&prefs, "ab"), None);
        SeedGoal::store_override(&mut prefs, "ab", Some(SeedGoal { ratio_pct: 200, minutes: 30 }));
        assert!(prefs.to_text().contains("torrent.ab.seed_ratio=200\n"));
        assert_eq!(SeedGoal::load_override(&prefs, "ab"), Some(SeedGoal { ratio_pct: 200, minutes: 30 }));
        SeedGoal::store_override(&mut prefs, "ab", None);
        assert_eq!(SeedGoal::load_override(&prefs, "ab"), None);
        assert!(!prefs.to_text().contains("torrent.ab"));
    }
}
