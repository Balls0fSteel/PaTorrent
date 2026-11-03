//! Slowing down when the disk is busy (D48 part 4, `docs/DISKIO.md`): while a drive's writes
//! (with `disk_slow_downloads`) or reads (with `disk_slow_uploads`) take clearly longer than
//! its usual, as the disk threads time them, the download or upload is capped at half its rate,
//! and halved again every few seconds while the drive stays slow. Once it answers as usual the
//! cap grows 5% a second and goes after half a minute without a busy second. The caps share
//! the global class with keep ping low's (`auto_limit`): the lowest applies, and the user's
//! own limits are never changed.
//!
//! The drives also report themselves here when they are first seen, for the Logger.

use std::time::{Duration, Instant};

use super::Engine;
use super::peer_class::{DOWNLOAD, UPLOAD};
use crate::alert::Alert;

const STEP: Duration = Duration::from_secs(1);
/// Seconds after a cut before the next one: the drive's queue needs time to drain.
const HOLD_AFTER_CUT: u32 = 3;
/// Quiet seconds before a cap goes.
const RELEASE_SECS: u32 = 30;
/// The lowest cap, bytes a second.
const MIN_CAP: u32 = 50_000;

/// One direction's cap.
#[derive(Default)]
struct Controller {
    cap: Option<u32>,
    hold: u32,
    quiet: u32,
}

impl Controller {
    /// One second: `busy` says whether the drive this direction uses is slow, `rate` is the
    /// direction's rate over the last second.
    fn step(&mut self, busy: bool, rate: u32) {
        self.hold = self.hold.saturating_sub(1);
        if busy {
            self.quiet = 0;
            if self.hold == 0 {
                let from = self.cap.map_or(rate, |c| c.min(rate.max(MIN_CAP)));
                self.cap = Some((from / 2).max(MIN_CAP));
                self.hold = HOLD_AFTER_CUT;
            }
        } else if let Some(cap) = self.cap {
            self.quiet += 1;
            self.cap = if self.quiet >= RELEASE_SECS { None } else { Some(cap + cap / 20 + 1) };
        }
    }
}

#[derive(Default)]
pub(super) struct DiskLimit {
    ctl: [Controller; 2],
    next_step: Option<Instant>,
    totals: [u64; 2],
    /// What the Logger was last told per direction: busy or not.
    logged: [bool; 2],
}

impl DiskLimit {
    pub(super) fn caps(&self) -> [Option<u32>; 2] {
        [self.ctl[0].cap, self.ctl[1].cap]
    }
}

impl Engine {
    fn disk_log(&mut self, message: String) {
        self.alert(Alert::Log { id: None, message: format!("Disk I/O: {message}") });
    }

    /// Once a second, moves the caps by the drives' state; logs drives seen for the first time.
    pub(super) fn update_disk_limit(&mut self, now: Instant) {
        for d in self.disk.drives_found() {
            self.disk_log(d);
        }
        let next = *self.disk_limit.next_step.get_or_insert(now + STEP);
        if now < next {
            return;
        }
        let elapsed = now.duration_since(next - STEP).as_secs_f64().max(0.001);
        self.disk_limit.next_step = Some(now + STEP);
        let mut totals = [0; 2];
        totals[UPLOAD] = self.stat.total_upload();
        totals[DOWNLOAD] = self.stat.total_download();
        let enabled = {
            let mut e = [false; 2];
            e[UPLOAD] = self.settings.disk_slow_uploads;
            e[DOWNLOAD] = self.settings.disk_slow_downloads;
            e
        };
        let had = self.disk_limit.caps();
        let busy = if enabled.iter().any(|e| *e) { self.disk.busy_drives(now) } else { [None, None] };
        // the drives' writes slow the download, their reads the upload
        let mut drive = [None, None];
        drive[DOWNLOAD] = busy[0].clone();
        drive[UPLOAD] = busy[1].clone();
        let mut log = Vec::new();
        for (ch, name) in [(UPLOAD, "uploads"), (DOWNLOAD, "downloads")] {
            let rate =
                ((totals[ch] - self.disk_limit.totals[ch]) as f64 / elapsed).min(u32::MAX as f64) as u32;
            let c = &mut self.disk_limit.ctl[ch];
            if !enabled[ch] {
                *c = Controller::default();
            } else {
                c.step(drive[ch].is_some(), rate);
            }
            let slowed = c.cap.is_some();
            if slowed != self.disk_limit.logged[ch] {
                self.disk_limit.logged[ch] = slowed;
                log.push(match (&drive[ch], c.cap) {
                    (Some(d), Some(cap)) => format!("{d} is busy: {name} limited to {} kB/s", cap / 1000),
                    _ => format!("{name} no longer limited for the disk"),
                });
            }
        }
        self.disk_limit.totals = totals;
        for m in log {
            self.disk_log(m);
        }
        if self.disk_limit.caps() != had {
            let ping = self.ping_caps();
            self.apply_auto_caps(ping);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_busy_drive_halves_the_rate_then_lets_it_grow_back() {
        let mut c = Controller::default();
        c.step(false, 10_000_000);
        assert_eq!(c.cap, None, "an idle drive caps nothing");
        c.step(true, 10_000_000);
        assert_eq!(c.cap, Some(5_000_000));
        // held: the queue drains first
        c.step(true, 5_000_000);
        c.step(true, 5_000_000);
        assert_eq!(c.cap, Some(5_000_000));
        c.step(true, 5_000_000);
        assert_eq!(c.cap, Some(2_500_000));
        c.step(false, 2_500_000);
        assert_eq!(c.cap, Some(2_625_001));
        for _ in 0..RELEASE_SECS {
            c.step(false, 2_500_000);
        }
        assert_eq!(c.cap, None);
    }

    #[test]
    fn caps_stop_at_the_floor() {
        let mut c = Controller::default();
        for _ in 0..40 {
            c.step(true, 60_000);
        }
        assert_eq!(c.cap, Some(MIN_CAP));
    }
}
