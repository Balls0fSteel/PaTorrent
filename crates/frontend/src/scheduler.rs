//! µTorrent 2.2.1's weekly scheduler: a 7 × 24 table of hours, each one Full speed, Limited,
//! Turn off or Seeding only, plus the Limited rates and "Disable DHT when turning off".
//!
//! [`Schedule`] is what Preferences > Scheduler edits; [`Scheduler`] follows the clock and says
//! what to change in the session when the hour's mode changes. Toolkit-independent: the Win32
//! GUI and nox both drive it from their own timers.
//!
//! What each mode does here:
//!
//! * Full speed: the configured settings, unchanged.
//! * Limited: the global upload / download limits become the scheduler's Limited rates.
//! * Turn off: the session is paused (every torrent stops without losing its own state);
//!   with "Disable DHT when turning off" the DHT is switched off too.
//! * Seeding only: every torrent goes into libtorrent's upload mode (it uploads but requests
//!   nothing), finished ones seed as before.

use patorrent_core::{AddTorrentParams, Settings, TorrentFlags};

use crate::prefs::Prefs;

pub const DAYS: usize = 7;
pub const HOURS: usize = 24;
pub const CELLS: usize = DAYS * HOURS;

/// The days as the table's rows, Monday first like µTorrent's.
pub const DAY_NAMES: [&str; DAYS] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// One hour of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    #[default]
    Full,
    Limited,
    Off,
    SeedOnly,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Full, Mode::Limited, Mode::Off, Mode::SeedOnly];

    /// The legend's text.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Full => "Full speed",
            Mode::Limited => "Limited",
            Mode::Off => "Turn off",
            Mode::SeedOnly => "Seeding only",
        }
    }

    /// One character per cell in `sched.table`.
    pub fn code(self) -> char {
        match self {
            Mode::Full => '0',
            Mode::Off => '1',
            Mode::Limited => '2',
            Mode::SeedOnly => '3',
        }
    }

    pub fn from_code(c: char) -> Option<Mode> {
        Some(match c {
            '0' => Mode::Full,
            '1' => Mode::Off,
            '2' => Mode::Limited,
            '3' => Mode::SeedOnly,
            _ => return None,
        })
    }

    /// The next mode a click on a cell gives (Full → Limited → Turn off → Seeding only).
    pub fn next(self) -> Mode {
        match self {
            Mode::Full => Mode::Limited,
            Mode::Limited => Mode::Off,
            Mode::Off => Mode::SeedOnly,
            Mode::SeedOnly => Mode::Full,
        }
    }
}

/// Preferences > Scheduler (preferences file keys in brackets).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Schedule {
    /// "Enable Scheduler" (`sched.enable`).
    pub enabled: bool,
    /// Monday 00:00 first, then by hour and day (`sched.table`, one [`Mode::code`] per hour).
    pub table: [Mode; CELLS],
    /// Limited upload / download rate in kB/s, 0 = no limit (`sched.ul_rate`, `sched.dl_rate`).
    pub limited_up_kib: u32,
    pub limited_down_kib: u32,
    /// "Disable DHT when turning off" (`sched.dis_dht`, on by default as in µTorrent).
    pub disable_dht: bool,
}

impl Default for Schedule {
    fn default() -> Schedule {
        Schedule {
            enabled: false,
            table: [Mode::Full; CELLS],
            limited_up_kib: 0,
            limited_down_kib: 0,
            disable_dht: true,
        }
    }
}

impl Schedule {
    pub fn load(prefs: &Prefs) -> Schedule {
        let d = Schedule::default();
        let kib = |k: &str| prefs.get_i32(k).unwrap_or(0).max(0) as u32;
        Schedule {
            enabled: prefs.get_bool("sched.enable", d.enabled),
            table: prefs.get("sched.table").and_then(parse_table).unwrap_or(d.table),
            limited_up_kib: kib("sched.ul_rate"),
            limited_down_kib: kib("sched.dl_rate"),
            disable_dht: prefs.get_bool("sched.dis_dht", d.disable_dht),
        }
    }

    pub fn store(&self, prefs: &mut Prefs) {
        prefs.set_bool("sched.enable", self.enabled);
        prefs.set("sched.table", self.table_text());
        prefs.set("sched.ul_rate", self.limited_up_kib);
        prefs.set("sched.dl_rate", self.limited_down_kib);
        prefs.set_bool("sched.dis_dht", self.disable_dht);
    }

    /// The table as `sched.table` stores it: 168 [`Mode::code`] characters.
    pub fn table_text(&self) -> String {
        self.table.iter().map(|m| m.code()).collect()
    }

    pub fn get(&self, day: usize, hour: usize) -> Mode {
        self.table[day * HOURS + hour]
    }

    pub fn set(&mut self, day: usize, hour: usize, mode: Mode) {
        self.table[day * HOURS + hour] = mode;
    }

    /// The mode in force at `unix` (seconds since 1970 UTC) in a zone `utc_offset` seconds east
    /// of UTC; Full speed while the scheduler is off.
    pub fn mode_at(&self, unix: i64, utc_offset: i32) -> Mode {
        if !self.enabled {
            return Mode::Full;
        }
        let (day, hour) = day_hour(unix, utc_offset);
        self.get(day, hour)
    }

    /// The engine settings for `mode`, starting from the configured ones.
    pub fn settings_for(&self, mode: Mode, base: &Settings) -> Settings {
        let mut s = base.clone();
        match mode {
            Mode::Limited => {
                s.upload_rate_limit = self.limited_up_kib.saturating_mul(1024);
                s.download_rate_limit = self.limited_down_kib.saturating_mul(1024);
            }
            Mode::Off if self.disable_dht => s.enable_dht = false,
            _ => {}
        }
        s
    }
}

/// `sched.table` text back to a table; `None` unless it has exactly 168 valid codes.
pub fn parse_table(text: &str) -> Option<[Mode; CELLS]> {
    let mut t = [Mode::Full; CELLS];
    let mut n = 0;
    for c in text.trim().chars() {
        *t.get_mut(n)? = Mode::from_code(c)?;
        n += 1;
    }
    (n == CELLS).then_some(t)
}

/// The table row (0 = Monday) and hour of local time.
pub fn day_hour(unix: i64, utc_offset: i32) -> (usize, usize) {
    let t = unix + utc_offset as i64;
    let days = t.div_euclid(86_400);
    // 1970-01-01 was a Thursday (row 3).
    ((days + 3).rem_euclid(7) as usize, (t.rem_euclid(86_400) / 3600) as usize)
}

/// What a frontend has to do to the session when the mode changes; `None` = leave it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Change {
    /// The scheduler's mode now.
    pub mode: Mode,
    /// Apply these settings.
    pub settings: Option<Settings>,
    /// Pause (`Some(true)`) or resume (`Some(false)`) the session.
    pub pause_session: Option<bool>,
    /// Set (`Some(true)`) or clear (`Some(false)`) [`TorrentFlags::UPLOAD_MODE`] on every torrent.
    pub upload_mode: Option<bool>,
}

/// Follows the clock: remembers the mode last put into effect.
#[derive(Clone, Debug, Default)]
pub struct Scheduler {
    applied: Mode,
}

impl Scheduler {
    pub fn new() -> Scheduler {
        Scheduler::default()
    }

    /// The mode last put into effect (Full speed until the first change).
    pub fn mode(&self) -> Mode {
        self.applied
    }

    /// Checks the clock. Returns the change to make when the mode differs from the one in
    /// effect, or always with `force` (the configured settings or the schedule just changed:
    /// the settings are recomputed then even if the mode stays).
    pub fn update(
        &mut self,
        schedule: &Schedule,
        base: &Settings,
        unix: i64,
        utc_offset: i32,
        force: bool,
    ) -> Option<Change> {
        let mode = schedule.mode_at(unix, utc_offset);
        let old = self.applied;
        if mode == old && !force {
            return None;
        }
        self.applied = mode;
        let flip = |was: bool, now: bool| (was != now).then_some(now);
        Some(Change {
            mode,
            settings: Some(schedule.settings_for(mode, base)),
            pause_session: flip(old == Mode::Off, mode == Mode::Off),
            upload_mode: flip(old == Mode::SeedOnly, mode == Mode::SeedOnly),
        })
    }

    /// Puts a torrent being added into upload mode during Seeding only hours.
    pub fn apply_to_new(&self, p: &mut AddTorrentParams) {
        if self.applied == Mode::SeedOnly {
            p.flags |= TorrentFlags::UPLOAD_MODE;
        }
    }

    /// The status bar's note while the scheduler holds the session back.
    pub fn status_text(&self) -> Option<&'static str> {
        match self.applied {
            Mode::Full => None,
            Mode::Limited => Some("Scheduler: limited"),
            Mode::Off => Some("Scheduler: turned off"),
            Mode::SeedOnly => Some("Scheduler: seeding only"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-19 00:00 UTC is a Monday.
    const MONDAY: i64 = 1_792_368_000;

    #[test]
    fn days_and_hours_follow_local_time() {
        assert_eq!(day_hour(0, 0), (3, 0)); // Thursday 1970-01-01
        assert_eq!(day_hour(MONDAY, 0), (0, 0));
        assert_eq!(day_hour(MONDAY + 13 * 3600 + 59 * 60, 0), (0, 13));
        // Sunday 23:00 in UTC is Monday 01:00 in UTC+2.
        assert_eq!(day_hour(MONDAY - 3600, 2 * 3600), (0, 1));
        assert_eq!(day_hour(MONDAY + 3600, -2 * 3600), (6, 23));
        assert_eq!(day_hour(-1, 0), (2, 23)); // Wednesday 1969-12-31
    }

    #[test]
    fn the_table_round_trips_through_the_preferences() {
        let mut s =
            Schedule { enabled: true, limited_up_kib: 20, limited_down_kib: 300, ..Default::default() };
        s.set(0, 9, Mode::Limited);
        s.set(4, 23, Mode::Off);
        s.set(6, 0, Mode::SeedOnly);
        let mut prefs = Prefs::default();
        s.store(&mut prefs);
        assert_eq!(prefs.get("sched.table").map(str::len), Some(CELLS));
        assert_eq!(Schedule::load(&prefs), s);
        assert_eq!(Schedule::load(&Prefs::default()), Schedule::default());
    }

    #[test]
    fn a_bad_table_falls_back_to_full_speed() {
        assert!(parse_table(&"0".repeat(CELLS - 1)).is_none());
        assert!(parse_table(&"0".repeat(CELLS + 1)).is_none());
        assert!(parse_table(&format!("{}x", "0".repeat(CELLS - 1))).is_none());
        let mut prefs = Prefs::default();
        prefs.set("sched.table", "0123");
        assert_eq!(Schedule::load(&prefs).table, [Mode::Full; CELLS]);
    }

    #[test]
    fn clicks_cycle_through_the_modes() {
        let mut m = Mode::Full;
        let mut seen = vec![];
        for _ in 0..4 {
            seen.push(m);
            m = m.next();
        }
        assert_eq!(seen, Mode::ALL);
        assert_eq!(m, Mode::Full);
        for m in Mode::ALL {
            assert_eq!(Mode::from_code(m.code()), Some(m));
        }
    }

    #[test]
    fn modes_change_the_settings() {
        let base = Settings { upload_rate_limit: 5000, download_rate_limit: 0, ..Default::default() };
        let s = Schedule { limited_up_kib: 10, limited_down_kib: 100, ..Default::default() };
        assert_eq!(s.settings_for(Mode::Full, &base), base);
        assert_eq!(s.settings_for(Mode::SeedOnly, &base), base);
        let l = s.settings_for(Mode::Limited, &base);
        assert_eq!((l.upload_rate_limit, l.download_rate_limit), (10 * 1024, 100 * 1024));
        assert!(!s.settings_for(Mode::Off, &base).enable_dht);
        let keep = Schedule { disable_dht: false, ..s };
        assert!(keep.settings_for(Mode::Off, &base).enable_dht);
    }

    #[test]
    fn the_scheduler_acts_only_when_the_hour_changes_mode() {
        let base = Settings::default();
        let mut s = Schedule { enabled: true, ..Default::default() };
        s.set(0, 1, Mode::Off);
        s.set(0, 2, Mode::SeedOnly);
        s.set(0, 3, Mode::Limited);
        let mut sch = Scheduler::new();
        assert_eq!(sch.update(&s, &base, MONDAY, 0, false), None);
        let c = sch.update(&s, &base, MONDAY + 3600, 0, false).expect("off");
        assert_eq!((c.mode, c.pause_session, c.upload_mode), (Mode::Off, Some(true), None));
        assert!(!c.settings.expect("settings").enable_dht);
        assert_eq!(sch.update(&s, &base, MONDAY + 3600 + 1800, 0, false), None);
        let c = sch.update(&s, &base, MONDAY + 2 * 3600, 0, false).expect("seeding");
        assert_eq!((c.mode, c.pause_session, c.upload_mode), (Mode::SeedOnly, Some(false), Some(true)));
        let mut p = AddTorrentParams::new(patorrent_core::TorrentSource::Magnet(String::new()), "");
        sch.apply_to_new(&mut p);
        assert!(p.flags.contains(TorrentFlags::UPLOAD_MODE));
        let c = sch.update(&s, &base, MONDAY + 3 * 3600, 0, false).expect("limited");
        assert_eq!((c.mode, c.pause_session, c.upload_mode), (Mode::Limited, None, Some(false)));
        // An Apply in the same hour recomputes the settings without touching the rest.
        let c = sch.update(&s, &base, MONDAY + 3 * 3600, 0, true).expect("forced");
        assert_eq!((c.mode, c.pause_session, c.upload_mode), (Mode::Limited, None, None));
        assert!(c.settings.is_some());
        // Switching the scheduler off returns to full speed.
        s.enabled = false;
        let c = sch.update(&s, &base, MONDAY + 3 * 3600, 0, true).expect("off");
        assert_eq!(c.mode, Mode::Full);
        assert_eq!(c.settings, Some(base));
        assert_eq!(sch.status_text(), None);
    }
}
