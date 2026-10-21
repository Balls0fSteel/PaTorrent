//! µTorrent 2.2.1's Options > Auto Shutdown: quit the client, put the computer on standby,
//! hibernate it or shut it down once the downloads, or everything, have completed.
//!
//! The choice lasts for this run only (µTorrent did not keep it either): every start is
//! Disabled. After it has acted, it is Disabled again.
//!
//! What still counts as work, per torrent ([`is_busy`]):
//!
//! * "Downloads complete": every started torrent (running, forced or queued) that has not
//!   finished downloading, including one that is checking, fetching its metadata, or held by the
//!   scheduler (session turned off, or Seeding only). A stopped or paused torrent, or one with an
//!   error, waits for the user and does not count. A torrent whose wanted files are all done is
//!   finished, even with skipped files left.
//! * "Everything completes": also every started torrent that has finished, that is every seed
//!   still seeding or queued to seed; such a torrent completes when the seeding goal stops it
//!   (Preferences > Queueing). A seed with no goal, or force-started, seeds on and holds the
//!   shutdown back, as in µTorrent.
//! * In both: a torrent whose files are being moved.
//!
//! [`AutoShutdown`] only acts after it has seen work while the mode was set, so choosing it with
//! nothing left to do waits for the next download rather than shutting down at once. When the
//! work is done it counts down ([`COUNTDOWN`]); the frontend shows the countdown with a Cancel
//! button, and new work during the countdown calls it off until that work is done too.

use std::time::{Duration, Instant};

use patorrent_core::{TorrentFlags, TorrentState, TorrentStatus};

use crate::tr;

/// What to do once the work is done.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Quit PaTorrent, as File > Exit.
    Quit,
    Standby,
    Hibernate,
    /// Shut down and power off the computer.
    ShutDown,
}

impl Action {
    /// The power actions need the OS; nox only quits.
    pub fn is_power(self) -> bool {
        self != Action::Quit
    }

    /// The Logger's word for it: "quitting", "standby", "hibernating", "shutting down".
    pub fn doing(self) -> &'static str {
        match self {
            Action::Quit => tr!(ShutdownWhatQuit),
            Action::Standby => tr!(ShutdownWhatStandby),
            Action::Hibernate => tr!(ShutdownWhatHibernate),
            Action::ShutDown => tr!(ShutdownWhatShutDown),
        }
    }
}

/// When the work is done.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum When {
    /// No torrent is downloading any more.
    Downloads,
    /// No torrent is downloading or seeding any more.
    Everything,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mode {
    pub action: Action,
    pub when: When,
}

impl Mode {
    pub const fn new(action: Action, when: When) -> Mode {
        Mode { action, when }
    }

    /// The submenu's items in order; `None` is Disabled.
    pub const MENU: [Option<Mode>; 9] = [
        None,
        Some(Mode::new(Action::Quit, When::Downloads)),
        Some(Mode::new(Action::Quit, When::Everything)),
        Some(Mode::new(Action::Standby, When::Downloads)),
        Some(Mode::new(Action::Standby, When::Everything)),
        Some(Mode::new(Action::Hibernate, When::Downloads)),
        Some(Mode::new(Action::Hibernate, When::Everything)),
        Some(Mode::new(Action::ShutDown, When::Downloads)),
        Some(Mode::new(Action::ShutDown, When::Everything)),
    ];

    /// The menu text (with its access key) of a [`Mode::MENU`] item.
    pub fn label(mode: Option<Mode>) -> &'static str {
        let Some(m) = mode else { return tr!(ShutdownDisabled) };
        match (m.action, m.when) {
            (Action::Quit, When::Downloads) => tr!(ShutdownQuitDownloads),
            (Action::Quit, When::Everything) => tr!(ShutdownQuitEverything),
            (Action::Standby, When::Downloads) => tr!(ShutdownStandbyDownloads),
            (Action::Standby, When::Everything) => tr!(ShutdownStandbyEverything),
            (Action::Hibernate, When::Downloads) => tr!(ShutdownHibernateDownloads),
            (Action::Hibernate, When::Everything) => tr!(ShutdownHibernateEverything),
            (Action::ShutDown, When::Downloads) => tr!(ShutdownShutDownDownloads),
            (Action::ShutDown, When::Everything) => tr!(ShutdownShutDownEverything),
        }
    }
}

/// The countdown's text: what completed, then what happens in how long (`0:30`).
pub fn countdown_text(mode: Mode, secs: u32) -> String {
    let done = match mode.when {
        When::Downloads => tr!(ShutdownDownloadsDone),
        When::Everything => tr!(ShutdownEverythingDone),
    };
    let time = format!("{}:{:02}", secs / 60, secs % 60);
    let what = match mode.action {
        Action::Quit => tr!(ShutdownWillQuit, time),
        Action::Standby => tr!(ShutdownWillStandby, time),
        Action::Hibernate => tr!(ShutdownWillHibernate, time),
        Action::ShutDown => tr!(ShutdownWillShutDown, time),
    };
    format!("{done}\n{what}")
}

/// How long the countdown before the action runs.
pub const COUNTDOWN: Duration = Duration::from_secs(30);

/// Whether this torrent still has work that `when` waits for (see the module notes).
pub fn is_busy(st: &TorrentStatus, when: When) -> bool {
    if st.moving_storage {
        return true;
    }
    if !st.error.is_empty() {
        return false;
    }
    // running, forced, or queued: libtorrent's PAUSED alone, without AUTO_MANAGED, is the
    // user's Stop or Pause
    let started = !st.flags.contains(TorrentFlags::PAUSED) || st.flags.contains(TorrentFlags::AUTO_MANAGED);
    if !started {
        return false;
    }
    match when {
        When::Everything => true,
        When::Downloads => {
            !st.is_finished
                || matches!(
                    st.state,
                    TorrentState::CheckingFiles
                        | TorrentState::CheckingResumeData
                        | TorrentState::DownloadingMetadata
                )
        }
    }
}

/// Whether any of these torrents is busy.
pub fn any_busy<'a>(when: When, list: impl IntoIterator<Item = &'a TorrentStatus>) -> bool {
    list.into_iter().any(|st| is_busy(st, when))
}

/// What the frontend does after an [`AutoShutdown::update`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Step {
    /// Nothing to do (disabled, still working, or not armed yet).
    Wait,
    /// The work is done: show the countdown, this many seconds.
    Start(u32),
    /// The countdown goes on: this many seconds left.
    Tick(u32),
    /// New work came during the countdown: close it and wait again.
    Abort,
    /// The countdown has run out: do it. The mode is Disabled again.
    Act(Action),
}

/// The chosen mode and where it stands.
#[derive(Clone, Debug)]
pub struct AutoShutdown {
    mode: Option<Mode>,
    /// Work has been seen since the mode was chosen.
    armed: bool,
    deadline: Option<Instant>,
    countdown: Duration,
}

impl Default for AutoShutdown {
    fn default() -> AutoShutdown {
        AutoShutdown::new(COUNTDOWN)
    }
}

impl AutoShutdown {
    /// Disabled, with a countdown of this length (zero acts as soon as the work is done).
    pub fn new(countdown: Duration) -> AutoShutdown {
        AutoShutdown { mode: None, armed: false, deadline: None, countdown }
    }

    pub fn mode(&self) -> Option<Mode> {
        self.mode
    }

    /// A new choice (from the menu, or `None` from the countdown's Cancel): it waits for work
    /// again, and any countdown is over.
    pub fn set_mode(&mut self, mode: Option<Mode>) {
        self.mode = mode;
        self.armed = false;
        self.deadline = None;
    }

    pub fn counting_down(&self) -> bool {
        self.deadline.is_some()
    }

    /// Looks at every torrent (the whole list, not just the ones in the last status update).
    pub fn check<'a>(&mut self, list: impl IntoIterator<Item = &'a TorrentStatus>, now: Instant) -> Step {
        let Some(m) = self.mode else { return Step::Wait };
        self.update(any_busy(m.when, list), now)
    }

    /// One step, with `busy` already worked out (for frontends that know of more work, such
    /// as moves they have started themselves).
    pub fn update(&mut self, busy: bool, now: Instant) -> Step {
        let Some(m) = self.mode else { return Step::Wait };
        if busy {
            self.armed = true;
            return if self.deadline.take().is_some() { Step::Abort } else { Step::Wait };
        }
        if !self.armed {
            return Step::Wait;
        }
        let deadline = match self.deadline {
            Some(d) => d,
            None => {
                let d = now + self.countdown;
                self.deadline = Some(d);
                if now < d {
                    return Step::Start(secs_left(d, now));
                }
                d
            }
        };
        if now >= deadline {
            self.set_mode(None);
            return Step::Act(m.action);
        }
        Step::Tick(secs_left(deadline, now))
    }
}

/// Whole seconds left, rounded up (a countdown shows 30 first and 1 last).
fn secs_left(deadline: Instant, now: Instant) -> u32 {
    let left = deadline.saturating_duration_since(now);
    (left.as_millis().div_ceil(1000)) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(flags: TorrentFlags, state: TorrentState, finished: bool) -> TorrentStatus {
        TorrentStatus { flags, state, is_finished: finished, ..TorrentStatus::default() }
    }

    const AM: TorrentFlags = TorrentFlags::AUTO_MANAGED;
    const P: TorrentFlags = TorrentFlags::PAUSED;

    #[test]
    fn what_counts_as_work() {
        use TorrentState::*;
        use When::*;
        let none = TorrentFlags::empty();
        // (status, busy for Downloads, busy for Everything)
        let cases = [
            (st(AM, Downloading, false), true, true),
            (st(none, Downloading, false), true, true), // [F] Downloading
            (st(AM | P, Downloading, false), true, true), // Queued
            (st(AM, DownloadingMetadata, false), true, true), // magnet
            (st(AM, CheckingFiles, true), true, true),  // checking a seed
            (st(AM | P, CheckingResumeData, false), true, true), // Queued for checking
            (st(P, Downloading, false), false, false),  // Stopped / Paused
            (st(P, CheckingFiles, false), false, false), // stopped while checking
            (st(AM, Seeding, true), false, true),       // Seeding
            (st(none, Seeding, true), false, true),     // [F] Seeding
            (st(AM | P, Seeding, true), false, true),   // Queued Seed
            (st(P, Seeding, true), false, false),       // Finished
            (st(AM, Downloading, true), false, true),   // skipped files left
        ];
        for (i, (s, dl, all)) in cases.iter().enumerate() {
            assert_eq!(is_busy(s, Downloads), *dl, "case {i} downloads: {s:?}");
            assert_eq!(is_busy(s, Everything), *all, "case {i} everything: {s:?}");
        }
        let error = TorrentStatus { error: "disk full".into(), ..st(AM, Downloading, false) };
        assert!(!is_busy(&error, Downloads) && !is_busy(&error, Everything));
        let moving = TorrentStatus { moving_storage: true, ..st(P, Seeding, true) };
        assert!(is_busy(&moving, Downloads) && is_busy(&moving, Everything));
        // upload mode (the scheduler's Seeding only) leaves the download unfinished
        let held = st(AM | TorrentFlags::UPLOAD_MODE, Downloading, false);
        assert!(is_busy(&held, Downloads));

        let list = [st(AM, Seeding, true), st(P, Downloading, false)];
        assert!(!any_busy(Downloads, &list));
        assert!(any_busy(Everything, &list));
        assert!(!any_busy(Everything, &[]));
    }

    #[test]
    fn the_menu_lists_every_mode_once() {
        let labels: Vec<&str> = Mode::MENU.iter().map(|&m| Mode::label(m)).collect();
        assert_eq!(labels[0], "&Disabled");
        assert_eq!(labels[1], "&Quit When Downloads Complete");
        assert_eq!(labels[8], "Shut Down When Everything &Completes");
        let mut keys: Vec<char> = labels
            .iter()
            .map(|l| l.chars().skip_while(|&c| c != '&').nth(1).unwrap().to_ascii_lowercase())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), 9, "access keys are distinct: {labels:?}");
        assert!(!Action::Quit.is_power() && Action::ShutDown.is_power());
    }

    #[test]
    fn it_waits_for_work_then_counts_down_and_acts_once() {
        let t0 = Instant::now();
        let s = |secs: u64| t0 + Duration::from_secs(secs);
        let mut a = AutoShutdown::default();
        assert_eq!(a.update(false, t0), Step::Wait, "disabled");
        assert_eq!(a.update(true, t0), Step::Wait);

        a.set_mode(Some(Mode::new(Action::ShutDown, When::Downloads)));
        // chosen with nothing to do: no shutdown until something has run
        assert_eq!(a.update(false, s(1)), Step::Wait);
        assert_eq!(a.update(false, s(100)), Step::Wait);
        assert!(!a.counting_down());
        assert_eq!(a.update(true, s(101)), Step::Wait);
        assert_eq!(a.update(true, s(102)), Step::Wait);
        assert_eq!(a.update(false, s(110)), Step::Start(30));
        assert!(a.counting_down());
        assert_eq!(a.update(false, s(111)), Step::Tick(29));
        assert_eq!(a.update(false, s(110) + Duration::from_millis(29_500)), Step::Tick(1));
        assert_eq!(a.update(false, s(140)), Step::Act(Action::ShutDown));
        // done: disabled again, nothing more
        assert_eq!(a.mode(), None);
        assert!(!a.counting_down());
        assert_eq!(a.update(true, s(141)), Step::Wait);
        assert_eq!(a.update(false, s(200)), Step::Wait);
    }

    #[test]
    fn new_work_calls_the_countdown_off_and_cancel_disables() {
        let t0 = Instant::now();
        let s = |secs: u64| t0 + Duration::from_secs(secs);
        let mut a = AutoShutdown::default();
        a.set_mode(Some(Mode::new(Action::Quit, When::Everything)));
        a.update(true, s(0));
        assert_eq!(a.update(false, s(1)), Step::Start(30));
        assert_eq!(a.update(true, s(5)), Step::Abort, "an RSS download arrived");
        assert_eq!(a.update(true, s(6)), Step::Wait);
        assert_eq!(a.update(false, s(50)), Step::Start(30), "a full countdown again");
        assert_eq!(a.update(false, s(79)), Step::Tick(1));
        // Cancel in the countdown
        a.set_mode(None);
        assert_eq!(a.update(false, s(90)), Step::Wait);
        assert_eq!(a.mode(), None);

        // choosing again re-arms from scratch
        a.set_mode(Some(Mode::new(Action::Quit, When::Downloads)));
        assert_eq!(a.update(false, s(91)), Step::Wait);
    }

    #[test]
    fn check_reads_the_whole_list_for_the_mode() {
        let t0 = Instant::now();
        let seeding = [st(AM, TorrentState::Seeding, true)];
        let downloading = [st(AM, TorrentState::Downloading, false)];
        let mut a = AutoShutdown::new(Duration::ZERO);
        assert_eq!(a.check(&downloading, t0), Step::Wait, "disabled");
        a.set_mode(Some(Mode::new(Action::Quit, When::Downloads)));
        assert_eq!(a.check(&downloading, t0), Step::Wait);
        // no countdown (nox): acts on the first look with the downloads done
        assert_eq!(a.check(&seeding, t0), Step::Act(Action::Quit));

        let mut a = AutoShutdown::new(Duration::ZERO);
        a.set_mode(Some(Mode::new(Action::Quit, When::Everything)));
        assert_eq!(a.check(&downloading, t0), Step::Wait);
        assert_eq!(a.check(&seeding, t0), Step::Wait, "still seeding");
        assert_eq!(a.check(&[], t0), Step::Act(Action::Quit));
    }
}
