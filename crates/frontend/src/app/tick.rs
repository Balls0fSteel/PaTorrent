//! The 1 s timer (GTK `build`'s timer body): status requests, the scheduler, seeding goals,
//! auto-load, RSS, Auto Shutdown and the update check.

use std::path::PathBuf;
use std::time::Instant;

use rookparcel_core::{AddTorrentParams, TorrentFlags, TorrentSource, TorrentStatus};

use super::{Core, Events, Power, UpdateOffer, Wakeup};
use crate::auto_shutdown::{Action, Mode as AutoMode, Step};
use crate::detail::Tab;
use crate::options::Options;
use crate::seeding::{GoalAction, SeedGoal};
use crate::update::{Kind, Manifest, Version};
use crate::{fmt, torrent_folders, tr};

impl Core {
    /// The timer's body, once a second: asks the engine for the torrents' and the session's
    /// state and for what the detail tab on screen shows, puts the scheduler's hour into
    /// effect, loads new `.torrent` files from the auto-load folder, refreshes the feeds that
    /// are due, steps Auto Shutdown and starts an automatic update check when one is due.
    pub fn tick(&mut self, now: Instant) -> Events {
        self.handle.post_torrent_updates();
        self.handle.post_session_stats();
        self.request_detail();
        self.run_scheduler(None);
        self.autoload(now);
        self.rss_timer();
        let list = &self.list;
        let step = self.auto_shutdown.check(list.ids().filter_map(|id| list.get(id)), now);
        if self.updates.due(now) {
            self.start_update_check();
        }
        self.auto_shutdown_step(step);
        self.take_events()
    }

    /// The results a worker woke the GUI for ([`super::Wakeup`]).
    pub fn on_wake(&mut self, w: Wakeup) -> Events {
        match w {
            Wakeup::Alerts => self.drain(),
            Wakeup::Rss => self.rss_collect(),
            Wakeup::Update => {
                let result = self.update_result.lock().unwrap_or_else(|e| e.into_inner()).take();
                if let Some(r) = result {
                    self.update_done(r);
                }
            }
            Wakeup::Fetched => self.fetched_done(),
        }
        self.take_events()
    }

    /// Asks the engine for what the detail tab on screen needs.
    pub(super) fn request_detail(&mut self) {
        let Some(id) = self.detail.id else { return };
        let t = self.handle.torrent(id);
        match self.tab {
            Tab::General | Tab::Pieces => t.post_piece_info(),
            Tab::Trackers => t.post_trackers(),
            Tab::Peers => t.post_peer_info(),
            Tab::Files => {
                if self.files_requested != Some(id) {
                    self.files_requested = Some(id);
                    t.post_file_list();
                }
                t.post_file_progress();
            }
            Tab::Speed | Tab::Logger => {}
        }
    }

    /// Puts the scheduler's mode for this hour into effect. `applied`: the options just applied
    /// (their settings go to the engine through the scheduler, even if the mode stays).
    pub(super) fn run_scheduler(&mut self, applied: Option<&Options>) {
        let Some((now, offset)) = (self.clock)() else {
            if let Some(o) = applied {
                self.handle.apply_settings(o.settings.clone());
            }
            return;
        };
        if applied.is_none() && self.schedule.mode_at(now, offset) == self.scheduler.mode() {
            return;
        }
        let loaded;
        let o = match applied {
            Some(o) => o,
            None => {
                loaded = self.options();
                &loaded
            }
        };
        let old = self.scheduler.mode();
        let Some(c) = self.scheduler.update(&self.schedule, &o.settings, now, offset, applied.is_some())
        else {
            return;
        };
        if let Some(s) = c.settings {
            self.handle.apply_settings(s);
        }
        match c.pause_session {
            Some(true) => self.handle.pause(),
            Some(false) => self.handle.resume(),
            None => {}
        }
        if let Some(on) = c.upload_mode {
            for id in self.list.ids() {
                let t = self.handle.torrent(id);
                if on {
                    t.set_flags(TorrentFlags::UPLOAD_MODE);
                } else if !self.awaiting.holds(id) {
                    // a magnet waiting for the Add dialog stays in upload mode until its OK
                    t.unset_flags(TorrentFlags::UPLOAD_MODE);
                }
            }
        }
        if c.mode != old {
            self.log(tr!(LogScheduler, mode = c.mode.label()));
            self.ev.status = true;
        }
    }

    /// Preferences > Queueing's seeding goal (or the torrent's own, from Properties) for a
    /// finished torrent.
    pub(super) fn seeding_goal(&mut self, st: &TorrentStatus) {
        if !st.is_finished {
            return;
        }
        let own = SeedGoal::load_override(&self.prefs, &st.info_hashes.key_hex());
        let Some(action) = self.seeding_goal.check(st, own) else { return };
        action.apply(&self.handle.torrent(st.id));
        let what = match action {
            GoalAction::Stop => tr!(SeedingGoalStopped).to_owned(),
            GoalAction::LimitUpload(b) => tr!(SeedingGoalLimited, rate = fmt::speed_always(b)),
        };
        self.log(tr!(LogSeedingGoal, name = st.name, what));
    }

    /// Preferences > Directories "Automatically load .torrents from": the new files in the
    /// folder are added without the Add New Torrent dialog and renamed to `.torrent.loaded` (or
    /// deleted) once the engine has them.
    fn autoload(&mut self, now: Instant) {
        let Some(dir) = self.torrent_folders.loads_from().map(PathBuf::from) else { return };
        // scanned and read on a thread of its own: the folder may be on a slow share
        let Some(scan) = self.autoload.poll(&dir, now) else { return };
        if let Some(e) = scan.error {
            self.log(tr!(LogAutoloadError, error = e));
        }
        for (path, read) in scan.files {
            let (bytes, ti) = match read {
                Ok(r) => r,
                Err(e) => {
                    self.log(tr!(FoldersNotLoaded, path = path.display(), error = e));
                    continue;
                }
            };
            let key = ti.info_hashes().key_hex();
            if self.in_list(&key).is_some() {
                let line =
                    torrent_folders::loaded(ti.name(), &key, &path, self.torrent_folders.autoload_delete);
                self.log(tr!(LogAlreadyInList, line));
                continue;
            }
            self.torrent_files.loading(&ti, path);
            let mut p = AddTorrentParams::new(TorrentSource::TorrentFile(bytes), self.save_path.clone());
            self.gui_options().apply_to_new(&mut p);
            self.add_to_session(p);
        }
    }

    // ---- Auto Shutdown ------------------------------------------------------------------------

    /// Options > Auto Shutdown's mode (`None`: Disabled), for the menu's check marks.
    pub fn auto_shutdown_mode(&self) -> Option<AutoMode> {
        self.auto_shutdown.mode()
    }

    /// Picks an Auto Shutdown mode from the menu.
    pub fn set_auto_shutdown(&mut self, mode: Option<AutoMode>) {
        self.auto_shutdown.set_mode(mode);
    }

    /// The countdown's Cancel: Auto Shutdown is Disabled again.
    pub fn cancel_auto_shutdown(&mut self) {
        self.auto_shutdown.set_mode(None);
        self.log(tr!(LogShutdownCancelled));
    }

    /// A power action the GUI tried failed (or `ROOKPARCEL_POWER_DRY_RUN` said what it would
    /// have done): the Logger gets it, and the message comes back to show.
    pub fn power_failed(&mut self, error: &str) -> Events {
        self.log(tr!(LogShutdown, what = error.replace('\n', " ")));
        self.ev.errors.push((error.to_owned(), String::new()));
        self.take_events()
    }

    /// The timer's Auto Shutdown step: the countdown opens when the work is done and closes
    /// when time is up (then it acts) or new work comes.
    fn auto_shutdown_step(&mut self, step: Step) {
        match step {
            Step::Wait => return,
            Step::Start(secs) => {
                let Some(mode) = self.auto_shutdown.mode() else { return };
                self.log(tr!(LogShutdownDone, what = mode.action.doing(), secs));
            }
            Step::Tick(_) => {}
            Step::Abort => self.log(tr!(LogShutdownNewWork)),
            Step::Act(action) => self.auto_shutdown_act(action),
        }
        self.ev.auto_shutdown = Some(step);
    }

    /// Quit as File > Exit does (state saved on the way out); Shut Down after that; Standby and
    /// Hibernate once the engine has written the resume data (`ResumeDataSaved`).
    fn auto_shutdown_act(&mut self, action: Action) {
        self.log(tr!(LogShutdown, what = action.doing()));
        match Power::of(action) {
            Some(p @ (Power::Standby | Power::Hibernate)) => {
                self.save_prefs();
                self.suspend_pending = Some(p);
                self.handle.save_resume_data();
            }
            p => {
                self.ev.power_after_exit = p;
                self.ev.quit = true;
            }
        }
    }

    // ---- the update check ---------------------------------------------------------------------

    /// Help > Check for Updates: starts a check unless one is on its way (which then answers
    /// in a dialog too).
    pub fn check_for_updates(&mut self) {
        if self.updates.start_manual() {
            self.start_update_check();
        }
    }

    /// The newer version the status bar's notice shows (until clicked).
    pub fn update_notice(&self) -> Option<&Version> {
        self.updates.notice()
    }

    /// The notice was clicked (its page opens): it goes away.
    pub fn dismiss_update_notice(&mut self) {
        self.updates.dismiss();
        self.ev.update_notice = true;
    }

    /// Whether automatic checks are possible in this GUI at all ([`super::Start::updates`]).
    pub fn updates_here(&self) -> bool {
        self.updates_here
    }

    /// Asks the website for the releases on a worker thread, through the proxy the feeds use;
    /// the answer comes back through [`Wakeup::Update`].
    fn start_update_check(&mut self) {
        let proxy = self.proxy();
        let (slot, wake) = (self.update_result.clone(), self.wake.clone());
        std::thread::spawn(move || {
            let result = crate::update::fetch(proxy);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
            wake(Wakeup::Update);
        });
    }

    /// A check's result: an automatic check's newer version goes to the status bar (until
    /// clicked) and to the Logger, and once per version into a dialog (D44); its failure goes
    /// to the Logger only. Help > Check for Updates answers in a dialog. The newest version a
    /// dialog was shown for is kept as `update.seen`.
    pub(super) fn update_done(&mut self, result: Result<Manifest, String>) {
        let current = Version::current();
        let f = self.updates.finish(result, &current, self.update_beta);
        match (&f.kind, &f.answer) {
            (Kind::Automatic, Ok(Some(v))) => {
                self.log(tr!(LogUpdateAvailable, version = v, url = v.release_url()))
            }
            (Kind::Automatic, Err(e)) => self.log(tr!(LogUpdateFailed, error = e)),
            _ => {}
        }
        if let Some(seen) = self.updates.seen() {
            let seen = seen.to_string();
            self.prefs.set("update.seen", seen);
        }
        match (f.kind, f.answer) {
            (Kind::Automatic, Ok(Some(v))) => {
                self.ev.update_notice = true;
                if f.announce {
                    self.ev.update = Some(UpdateOffer { version: v, current });
                }
            }
            (Kind::Automatic, Ok(None)) => self.ev.update_notice = true,
            (Kind::Automatic, Err(_)) => {}
            (Kind::Manual, Ok(Some(v))) => self.ev.update = Some(UpdateOffer { version: v, current }),
            (Kind::Manual, Ok(None)) => self.ev.infos.push(tr!(UpdateNewest, version = current)),
            (Kind::Manual, Err(e)) => self.ev.errors.push((tr!(UpdateFailed, error = e), String::new())),
        }
    }
}
