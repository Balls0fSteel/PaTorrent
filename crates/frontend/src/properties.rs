//! The Torrent Properties dialog's General tab (µTorrent 2.2.1): Trackers, Bandwidth Settings,
//! Seeding Goal and Other Settings, for one torrent or several at once. Toolkit-independent, so
//! the GTK dialog and the Web UI share it.
//!
//! [`Properties::load`] fills the fields from the selected torrents ([`TorrentProps`]). A value
//! that differs between them loads blank (text fields) or indeterminate ([`Tri::Mixed`], check
//! boxes), and a field the user leaves as it loaded is left alone on every torrent, as µTorrent
//! did with several torrents selected. [`Properties::changes`] validates the fields and returns
//! what changed ([`Changes`]); [`Changes::apply`] hands that to each torrent's handle and
//! [`Changes::store`] to the preferences, where the seeding goal override lives beside the label.
//!
//! The tracker box holds one URL per line, tiers separated by a blank line (BEP 12), as in
//! µTorrent; [`parse_trackers`] and [`trackers_text`] convert between that text and tiers.

use patorrent_core::{AnnounceEntry, TorrentFlags, TorrentHandle, TorrentStatus};

use crate::prefs::Prefs;
use crate::seeding::SeedGoal;
use crate::tr;

/// One torrent's current values, as the dialog loads them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TorrentProps {
    /// Tracker URLs per tier; `None` while unknown (the box then loads blank and is only
    /// applied if the user types into it).
    pub trackers: Option<Vec<Vec<String>>>,
    /// Bytes per second; 0 = unlimited.
    pub upload_limit: u32,
    pub download_limit: u32,
    /// Upload slots of its own (libtorrent `uploads_limit`).
    pub uploads_limit: Option<u32>,
    /// Its own seeding goal ("Override default settings").
    pub seed_goal: Option<SeedGoal>,
    pub super_seeding: bool,
    pub is_seed: bool,
    pub dht: bool,
    pub pex: bool,
}

impl TorrentProps {
    /// From the engine's status, the tracker list (`post_trackers`), and the preferences.
    pub fn new(st: &TorrentStatus, trackers: Option<&[AnnounceEntry]>, prefs: &Prefs) -> TorrentProps {
        TorrentProps {
            trackers: trackers.map(tiers_of),
            upload_limit: st.upload_limit,
            download_limit: st.download_limit,
            uploads_limit: st.uploads_limit,
            seed_goal: SeedGoal::load_override(prefs, &st.info_hashes.key_hex()),
            super_seeding: st.flags.contains(TorrentFlags::SUPER_SEEDING),
            is_seed: st.is_seeding,
            dht: !st.flags.contains(TorrentFlags::DISABLE_DHT),
            pex: !st.flags.contains(TorrentFlags::DISABLE_PEX),
        }
    }
}

/// What the dialog falls back to: Preferences > BitTorrent's upload slots per torrent (0 = no
/// limit) and Preferences > Queueing's seeding goal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Defaults {
    pub upload_slots: u32,
    pub seed_goal: SeedGoal,
}

/// A three-state check box.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tri {
    Off,
    On,
    /// Differs between the selected torrents.
    Mixed,
}

impl Tri {
    fn of(values: impl IntoIterator<Item = bool>) -> Tri {
        match same(values) {
            Some(true) => Tri::On,
            Some(false) => Tri::Off,
            None => Tri::Mixed,
        }
    }
}

/// The dialog's fields, as text and check boxes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fields {
    /// One URL per line, a blank line between tiers.
    pub trackers: String,
    /// "Maximum upload rate (kB/s): [0: unlimited]".
    pub upload_kib: String,
    pub download_kib: String,
    /// "Override default number of upload slots" and "Number of upload slots".
    pub override_slots: Tri,
    pub upload_slots: String,
    /// Seeding Goal: "Override default settings", "Minimum ratio (%)", "Minimum seeding time
    /// (minutes)".
    pub override_goal: Tri,
    pub goal_ratio: String,
    pub goal_minutes: String,
    /// Other Settings.
    pub initial_seeding: Tri,
    pub dht: Tri,
    pub pex: Tri,
}

/// Which field a validation error is about (so the dialog can focus it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Field {
    Trackers,
    UploadRate,
    DownloadRate,
    UploadSlots,
    GoalRatio,
    GoalMinutes,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FieldError {
    pub field: Field,
    pub message: String,
}

impl std::fmt::Display for FieldError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The dialog's model.
#[derive(Clone, Debug)]
pub struct Properties {
    pub fields: Fields,
    loaded: Fields,
    /// How many torrents the dialog edits.
    pub count: usize,
    /// "Initial Seeding" is for seeds only (µTorrent greys it out otherwise).
    pub can_initial_seed: bool,
    defaults: Defaults,
}

/// A change to the seeding goal override.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GoalChange {
    /// Back to the default goal.
    Default,
    /// A goal of its own; `None` keeps each torrent's current value of that target (the field
    /// was left blank because the torrents differed).
    Own { ratio_pct: Option<u32>, minutes: Option<u32> },
}

/// What the user changed; `None` fields stay as they are on every torrent.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub trackers: Option<Vec<Vec<String>>>,
    /// Bytes per second; 0 = unlimited.
    pub upload_limit: Option<u32>,
    pub download_limit: Option<u32>,
    /// 0 = no limit.
    pub max_uploads: Option<u32>,
    pub seed_goal: Option<GoalChange>,
    pub super_seeding: Option<bool>,
    pub dht: Option<bool>,
    pub pex: Option<bool>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        *self == Changes::default()
    }

    /// The engine side, for one torrent.
    pub fn apply(&self, t: &TorrentHandle) {
        if let Some(tiers) = &self.trackers {
            t.replace_trackers(announce_entries(tiers));
        }
        if let Some(l) = self.upload_limit {
            t.set_upload_limit(l);
        }
        if let Some(l) = self.download_limit {
            t.set_download_limit(l);
        }
        if let Some(n) = self.max_uploads {
            t.set_max_uploads(n);
        }
        let mut set = TorrentFlags::empty();
        let mut unset = TorrentFlags::empty();
        match self.super_seeding {
            Some(true) => set |= TorrentFlags::SUPER_SEEDING,
            Some(false) => unset |= TorrentFlags::SUPER_SEEDING,
            None => {}
        }
        // the switches are "disable" flags in the engine
        for (on, flag) in [(self.dht, TorrentFlags::DISABLE_DHT), (self.pex, TorrentFlags::DISABLE_PEX)] {
            match on {
                Some(true) => unset |= flag,
                Some(false) => set |= flag,
                None => {}
            }
        }
        if !set.is_empty() {
            t.set_flags(set);
        }
        if !unset.is_empty() {
            t.unset_flags(unset);
        }
    }

    /// The preferences side, for the torrent whose info-hash is `key` (hex).
    pub fn store(&self, prefs: &mut Prefs, key: &str, defaults: &Defaults) {
        match self.seed_goal {
            None => {}
            Some(GoalChange::Default) => SeedGoal::store_override(prefs, key, None),
            Some(GoalChange::Own { ratio_pct, minutes }) => {
                let current = SeedGoal::load_override(prefs, key).unwrap_or(defaults.seed_goal);
                let goal = SeedGoal {
                    ratio_pct: ratio_pct.unwrap_or(current.ratio_pct),
                    minutes: minutes.unwrap_or(current.minutes),
                };
                SeedGoal::store_override(prefs, key, Some(goal));
            }
        }
    }
}

/// `Some(v)` when every value is `v` (and there is at least one).
fn same<T: PartialEq>(values: impl IntoIterator<Item = T>) -> Option<T> {
    let mut it = values.into_iter();
    let first = it.next()?;
    it.all(|v| v == first).then_some(first)
}

fn text_of<T: ToString>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

impl Properties {
    pub fn load(items: &[TorrentProps], defaults: Defaults) -> Properties {
        let default_slots = (defaults.upload_slots > 0).then_some(defaults.upload_slots);
        let kib = |b: u32| b.div_ceil(1024);
        let trackers = match same(items.iter().map(|t| t.trackers.as_ref())) {
            Some(Some(tiers)) => trackers_text(tiers),
            _ => String::new(),
        };
        let goal = |t: &TorrentProps| t.seed_goal.unwrap_or(defaults.seed_goal);
        let fields = Fields {
            trackers,
            upload_kib: text_of(same(items.iter().map(|t| kib(t.upload_limit)))),
            download_kib: text_of(same(items.iter().map(|t| kib(t.download_limit)))),
            override_slots: Tri::of(items.iter().map(|t| t.uploads_limit != default_slots)),
            upload_slots: text_of(same(items.iter().map(|t| t.uploads_limit.unwrap_or(0)))),
            override_goal: Tri::of(items.iter().map(|t| t.seed_goal.is_some())),
            goal_ratio: text_of(same(items.iter().map(|t| goal(t).ratio_pct))),
            goal_minutes: text_of(same(items.iter().map(|t| goal(t).minutes))),
            initial_seeding: Tri::of(items.iter().map(|t| t.super_seeding)),
            dht: Tri::of(items.iter().map(|t| t.dht)),
            pex: Tri::of(items.iter().map(|t| t.pex)),
        };
        Properties {
            loaded: fields.clone(),
            fields,
            count: items.len(),
            can_initial_seed: !items.is_empty() && items.iter().all(|t| t.is_seed),
            defaults,
        }
    }

    /// The fields as they were loaded.
    pub fn loaded(&self) -> &Fields {
        &self.loaded
    }

    /// Validates the fields; what the user changed, or the first field that is wrong.
    pub fn changes(&self) -> Result<Changes, FieldError> {
        let (f, l) = (&self.fields, &self.loaded);
        let mut c = Changes::default();
        if f.trackers != l.trackers {
            c.trackers = Some(parse_trackers(&f.trackers).map_err(|m| err(Field::Trackers, m))?);
        }
        let rate = |text: &str, loaded: &str, field: Field| -> Result<Option<u32>, FieldError> {
            if text == loaded || text.trim().is_empty() {
                return Ok(None);
            }
            let kib = number(text, field, u32::MAX / 1024)?;
            Ok(Some(kib * 1024))
        };
        c.upload_limit = rate(&f.upload_kib, &l.upload_kib, Field::UploadRate)?;
        c.download_limit = rate(&f.download_kib, &l.download_kib, Field::DownloadRate)?;

        let slots_edited = f.upload_slots != l.upload_slots && !f.upload_slots.trim().is_empty();
        c.max_uploads = match f.override_slots {
            Tri::Off if l.override_slots != Tri::Off => Some(self.defaults.upload_slots),
            Tri::On if slots_edited => Some(number(&f.upload_slots, Field::UploadSlots, 1 << 24)?),
            Tri::On if l.override_slots != Tri::On => {
                // switched on with a blank (mixed) number: there is nothing to take it from
                Some(number(&f.upload_slots, Field::UploadSlots, 1 << 24)?)
            }
            _ => None,
        };

        let edited = |text: &str, loaded: &str| text != loaded && !text.trim().is_empty();
        let target = |text: &str, field: Field| -> Result<Option<u32>, FieldError> {
            if text.trim().is_empty() { Ok(None) } else { number(text, field, 1_000_000).map(Some) }
        };
        c.seed_goal = match f.override_goal {
            Tri::Off if l.override_goal != Tri::Off => Some(GoalChange::Default),
            Tri::On
                if l.override_goal != Tri::On
                    || edited(&f.goal_ratio, &l.goal_ratio)
                    || edited(&f.goal_minutes, &l.goal_minutes) =>
            {
                Some(GoalChange::Own {
                    ratio_pct: target(&f.goal_ratio, Field::GoalRatio)?,
                    minutes: target(&f.goal_minutes, Field::GoalMinutes)?,
                })
            }
            _ => None,
        };

        let switch = |now: Tri, then: Tri| (now != then && now != Tri::Mixed).then_some(now == Tri::On);
        if self.can_initial_seed {
            c.super_seeding = switch(f.initial_seeding, l.initial_seeding);
        }
        c.dht = switch(f.dht, l.dht);
        c.pex = switch(f.pex, l.pex);
        Ok(c)
    }
}

fn err(field: Field, message: impl Into<String>) -> FieldError {
    FieldError { field, message: message.into() }
}

fn number(text: &str, field: Field, max: u32) -> Result<u32, FieldError> {
    match text.trim().parse::<u32>() {
        Ok(v) if v <= max => Ok(v),
        _ => {
            let message = match field {
                Field::UploadRate => tr!(PropsBadUploadRate, max),
                Field::DownloadRate => tr!(PropsBadDownloadRate, max),
                Field::UploadSlots => tr!(PropsBadUploadSlots, max),
                Field::GoalRatio => tr!(PropsBadRatio, max),
                Field::GoalMinutes => tr!(PropsBadSeedTime, max),
                Field::Trackers => tr!(PropsBadNumber, max),
            };
            Err(err(field, message))
        }
    }
}

/// Tracker URLs grouped by tier, from a tracker list sorted by tier (as `post_trackers` gives).
pub fn tiers_of(entries: &[AnnounceEntry]) -> Vec<Vec<String>> {
    let mut tiers: Vec<(u32, Vec<String>)> = Vec::new();
    for e in entries {
        match tiers.last_mut() {
            Some((tier, urls)) if *tier == e.tier => urls.push(e.url.clone()),
            _ => tiers.push((e.tier, vec![e.url.clone()])),
        }
    }
    tiers.into_iter().map(|(_, urls)| urls).collect()
}

/// Tiers as the engine takes them: tier numbers by position.
pub fn announce_entries(tiers: &[Vec<String>]) -> Vec<AnnounceEntry> {
    tiers
        .iter()
        .enumerate()
        .flat_map(|(tier, urls)| urls.iter().map(move |u| AnnounceEntry::new(u.clone(), tier as u32)))
        .collect()
}

/// The tracker box's text: one URL per line, a blank line between tiers (`\n` line ends).
pub fn trackers_text(tiers: &[Vec<String>]) -> String {
    tiers.iter().filter(|t| !t.is_empty()).map(|t| t.join("\n")).collect::<Vec<_>>().join("\n\n")
}

/// Reads the tracker box: tiers separated by blank lines, each line a `http://`, `https://`
/// or `udp://` URL. Repeated URLs are dropped (the first one counts); the error names the
/// first line that is not a tracker URL.
pub fn parse_trackers(text: &str) -> Result<Vec<Vec<String>>, String> {
    let mut tiers: Vec<Vec<String>> = vec![Vec::new()];
    let mut seen = std::collections::HashSet::new();
    for line in text.lines().map(str::trim) {
        if line.is_empty() {
            if !tiers.last().is_some_and(Vec::is_empty) {
                tiers.push(Vec::new());
            }
            continue;
        }
        if !is_tracker_url(line) {
            return Err(tr!(PropsBadTracker, line));
        }
        if seen.insert(line.to_owned()) {
            tiers.last_mut().expect("never empty").push(line.to_owned());
        }
    }
    tiers.retain(|t| !t.is_empty());
    Ok(tiers)
}

fn is_tracker_url(s: &str) -> bool {
    let Some((scheme, rest)) = s.split_once("://") else { return false };
    let host = rest.split(['/', '?']).next().unwrap_or_default();
    ["http", "https", "udp"].iter().any(|k| scheme.eq_ignore_ascii_case(k))
        && !host.is_empty()
        && !s.contains(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use patorrent_core::{InfoHashes, Sha1Hash};

    use super::*;

    #[test]
    fn tracker_text_and_tiers() {
        let text = " http://a/announce \r\nudp://b:80\r\n\r\n\r\nhttps://c/x?k=1\n\nudp://b:80\n";
        let tiers = parse_trackers(text).unwrap();
        assert_eq!(
            tiers,
            [
                vec!["http://a/announce".to_owned(), "udp://b:80".to_owned()],
                vec!["https://c/x?k=1".to_owned()]
            ]
        );
        assert_eq!(trackers_text(&tiers), "http://a/announce\nudp://b:80\n\nhttps://c/x?k=1");
        assert_eq!(parse_trackers(&trackers_text(&tiers)).unwrap(), tiers);
        assert_eq!(parse_trackers("").unwrap(), Vec::<Vec<String>>::new());
        assert_eq!(parse_trackers("\n\n  \n").unwrap(), Vec::<Vec<String>>::new());
        for bad in ["tracker.example.com", "ftp://x/announce", "http://", "udp://a b"] {
            assert!(parse_trackers(bad).unwrap_err().contains(bad), "{bad}");
        }
        assert!(parse_trackers("HTTP://UPPER/announce").is_ok());

        let entries = announce_entries(&tiers);
        assert_eq!(entries.iter().map(|e| e.tier).collect::<Vec<_>>(), [0, 0, 1]);
        assert_eq!(tiers_of(&entries), tiers);
        // tiers numbered with gaps (as a .torrent may have them) group the same way
        let gapped = [AnnounceEntry::new("udp://x:1", 3), AnnounceEntry::new("udp://y:1", 7)];
        assert_eq!(tiers_of(&gapped).len(), 2);
    }

    fn one() -> TorrentProps {
        TorrentProps {
            trackers: Some(vec![vec!["udp://t:1/announce".into()]]),
            upload_limit: 10 * 1024,
            download_limit: 0,
            uploads_limit: Some(4),
            seed_goal: None,
            super_seeding: false,
            is_seed: true,
            dht: true,
            pex: true,
        }
    }

    const DEFAULTS: Defaults =
        Defaults { upload_slots: 4, seed_goal: SeedGoal { ratio_pct: 150, minutes: 0 } };

    #[test]
    fn one_torrent_loads_and_changes() {
        let mut p = Properties::load(&[one()], DEFAULTS);
        let f = &p.fields;
        assert_eq!(f.trackers, "udp://t:1/announce");
        assert_eq!((f.upload_kib.as_str(), f.download_kib.as_str()), ("10", "0"));
        assert_eq!((f.override_slots, f.upload_slots.as_str()), (Tri::Off, "4"), "4 is the default");
        assert_eq!((f.override_goal, f.goal_ratio.as_str(), f.goal_minutes.as_str()), (Tri::Off, "150", "0"));
        assert_eq!((f.initial_seeding, f.dht, f.pex), (Tri::Off, Tri::On, Tri::On));
        assert!(p.can_initial_seed);
        assert!(p.changes().unwrap().is_empty(), "nothing changed");

        p.fields.trackers = "udp://t:1/announce\n\nhttp://u/announce".into();
        p.fields.upload_kib = "0".into();
        p.fields.download_kib = " 250 ".into();
        p.fields.override_slots = Tri::On;
        p.fields.upload_slots = "2".into();
        p.fields.override_goal = Tri::On;
        p.fields.goal_ratio = "300".into();
        p.fields.initial_seeding = Tri::On;
        p.fields.pex = Tri::Off;
        let c = p.changes().unwrap();
        assert_eq!(
            c,
            Changes {
                trackers: Some(vec![vec!["udp://t:1/announce".into()], vec!["http://u/announce".into()]]),
                upload_limit: Some(0),
                download_limit: Some(250 * 1024),
                max_uploads: Some(2),
                seed_goal: Some(GoalChange::Own { ratio_pct: Some(300), minutes: Some(0) }),
                super_seeding: Some(true),
                dht: None,
                pex: Some(false),
            }
        );

        // the goal goes to the preferences
        let mut prefs = Prefs::default();
        c.store(&mut prefs, "ab", &DEFAULTS);
        assert_eq!(SeedGoal::load_override(&prefs, "ab"), Some(SeedGoal { ratio_pct: 300, minutes: 0 }));
        let back = Changes { seed_goal: Some(GoalChange::Default), ..Changes::default() };
        back.store(&mut prefs, "ab", &DEFAULTS);
        assert_eq!(SeedGoal::load_override(&prefs, "ab"), None);

        // clearing the tracker box removes every tracker; bad input names its field
        p.fields.trackers = String::new();
        assert_eq!(p.changes().unwrap().trackers, Some(Vec::new()));
        p.fields.trackers = "not a url".into();
        assert_eq!(p.changes().unwrap_err().field, Field::Trackers);
        p.fields.trackers = p.loaded().trackers.clone();
        p.fields.download_kib = "-1".into();
        assert_eq!(p.changes().unwrap_err().field, Field::DownloadRate);
        p.fields.download_kib = "4194304".into();
        assert_eq!(p.changes().unwrap_err().field, Field::DownloadRate, "past u32 bytes per second");
        p.fields.download_kib = String::new();
        p.fields.goal_minutes = "x".into();
        assert_eq!(p.changes().unwrap_err().field, Field::GoalMinutes);
    }

    #[test]
    fn override_off_goes_back_to_the_defaults() {
        let mut t = one();
        t.uploads_limit = Some(9);
        t.seed_goal = Some(SeedGoal { ratio_pct: 0, minutes: 60 });
        let mut p = Properties::load(&[t], DEFAULTS);
        assert_eq!((p.fields.override_slots, p.fields.upload_slots.as_str()), (Tri::On, "9"));
        assert_eq!(
            (p.fields.override_goal, p.fields.goal_ratio.as_str(), p.fields.goal_minutes.as_str()),
            (Tri::On, "0", "60")
        );
        p.fields.override_slots = Tri::Off;
        p.fields.override_goal = Tri::Off;
        let c = p.changes().unwrap();
        assert_eq!(c.max_uploads, Some(4));
        assert_eq!(c.seed_goal, Some(GoalChange::Default));
        // with no default the torrent gets no limit of its own
        let p2 = Properties { defaults: Defaults { upload_slots: 0, ..DEFAULTS }, ..p };
        assert_eq!(p2.changes().unwrap().max_uploads, Some(0));
    }

    #[test]
    fn several_torrents_leave_differing_fields_alone() {
        let a = one();
        let mut b = one();
        b.trackers = Some(vec![vec!["http://other/announce".into()]]);
        b.upload_limit = 0;
        b.uploads_limit = Some(7);
        b.seed_goal = Some(SeedGoal { ratio_pct: 200, minutes: 5 });
        b.is_seed = false;
        b.pex = false;
        let mut p = Properties::load(&[a, b], DEFAULTS);
        assert_eq!(p.count, 2);
        let f = &p.fields;
        assert_eq!(f.trackers, "", "lists differ");
        assert_eq!((f.upload_kib.as_str(), f.download_kib.as_str()), ("", "0"));
        assert_eq!((f.override_slots, f.upload_slots.as_str()), (Tri::Mixed, ""));
        assert_eq!((f.override_goal, f.goal_ratio.as_str(), f.goal_minutes.as_str()), (Tri::Mixed, "", ""));
        assert_eq!((f.dht, f.pex), (Tri::On, Tri::Mixed));
        assert!(!p.can_initial_seed, "one is not a seed");
        assert!(p.changes().unwrap().is_empty());

        // only what is touched applies to both
        p.fields.override_goal = Tri::On;
        p.fields.goal_minutes = "30".into();
        p.fields.pex = Tri::On;
        p.fields.initial_seeding = Tri::On;
        let c = p.changes().unwrap();
        assert_eq!(c.seed_goal, Some(GoalChange::Own { ratio_pct: None, minutes: Some(30) }));
        assert_eq!((c.pex, c.dht, c.super_seeding), (Some(true), None, None));
        assert_eq!((c.trackers.as_ref(), c.upload_limit, c.max_uploads), (None, None, None));
        // each keeps its own ratio (the default for the one without a goal)
        let mut prefs = Prefs::default();
        SeedGoal::store_override(&mut prefs, "b", Some(SeedGoal { ratio_pct: 200, minutes: 5 }));
        c.store(&mut prefs, "a", &DEFAULTS);
        c.store(&mut prefs, "b", &DEFAULTS);
        assert_eq!(SeedGoal::load_override(&prefs, "a"), Some(SeedGoal { ratio_pct: 150, minutes: 30 }));
        assert_eq!(SeedGoal::load_override(&prefs, "b"), Some(SeedGoal { ratio_pct: 200, minutes: 30 }));

        // switching the slot override on with a blank count asks for one
        p.fields.override_slots = Tri::On;
        assert_eq!(p.changes().unwrap_err().field, Field::UploadSlots);
        p.fields.upload_slots = "3".into();
        assert_eq!(p.changes().unwrap().max_uploads, Some(3));
        // typing trackers replaces both lists
        p.fields.trackers = "udp://new:1".into();
        assert_eq!(p.changes().unwrap().trackers, Some(vec![vec!["udp://new:1".to_owned()]]));
    }

    #[test]
    fn props_come_from_the_status_and_the_preferences() {
        let ih = InfoHashes::v1(Sha1Hash([0xab; 20]));
        let st = TorrentStatus {
            info_hashes: ih,
            upload_limit: 2048,
            uploads_limit: Some(3),
            flags: TorrentFlags::DISABLE_PEX | TorrentFlags::SUPER_SEEDING,
            is_seeding: true,
            ..Default::default()
        };
        let mut prefs = Prefs::default();
        SeedGoal::store_override(&mut prefs, &ih.key_hex(), Some(SeedGoal { ratio_pct: 50, minutes: 1 }));
        let trackers = [AnnounceEntry::new("udp://a:1", 0), AnnounceEntry::new("udp://b:1", 1)];
        let t = TorrentProps::new(&st, Some(&trackers), &prefs);
        assert_eq!(t.trackers, Some(vec![vec!["udp://a:1".to_owned()], vec!["udp://b:1".to_owned()]]));
        assert_eq!((t.upload_limit, t.uploads_limit), (2048, Some(3)));
        assert_eq!(t.seed_goal, Some(SeedGoal { ratio_pct: 50, minutes: 1 }));
        assert!(t.super_seeding && t.is_seed && t.dht && !t.pex);
        assert_eq!(TorrentProps::new(&st, None, &Prefs::default()).trackers, None);
    }

    #[test]
    fn changes_reach_the_engine() {
        use patorrent_core::{AddTorrentParams, Alert, MockConfig, Session, Settings, TorrentSource};
        use std::time::{Duration, Instant};

        let s =
            Session::start_mock(Settings { tick_interval: 50, ..Settings::default() }, MockConfig::default());
        let h = s.handle();
        let magnet = "magnet:?xt=urn:btih:00112233445566778899aabbccddeeff00112233&dn=p".to_owned();
        h.add_torrent(AddTorrentParams::new(TorrentSource::Magnet(magnet), "/dl"));
        let mut alerts = Vec::new();
        let mut id = None;
        let deadline = Instant::now() + Duration::from_secs(10);
        let c = Changes {
            trackers: Some(vec![vec!["udp://x:1".into()], vec!["udp://y:1".into(), "udp://z:1".into()]]),
            upload_limit: Some(5 * 1024),
            download_limit: Some(0),
            max_uploads: Some(2),
            dht: Some(false),
            pex: Some(false),
            ..Changes::default()
        };
        let mut applied = false;
        let mut status_ok = false;
        let mut trackers_ok = false;
        while !(status_ok && trackers_ok) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            h.post_torrent_updates();
            h.pop_alerts(&mut alerts);
            for a in alerts.drain(..) {
                match a {
                    Alert::TorrentAdded { id: x, .. } => id = Some(x),
                    Alert::StateUpdate(v) if applied => {
                        status_ok |= v.iter().any(|st| {
                            Some(st.id) == id
                                && st.upload_limit == 5120
                                && st.uploads_limit == Some(2)
                                && st.flags.contains(TorrentFlags::DISABLE_DHT | TorrentFlags::DISABLE_PEX)
                        });
                    }
                    Alert::TrackerList { trackers, .. } if applied => {
                        trackers_ok = tiers_of(&trackers) == *c.trackers.as_ref().unwrap();
                    }
                    _ => {}
                }
            }
            if let (Some(id), false) = (id, applied) {
                c.apply(&h.torrent(id));
                h.torrent(id).post_trackers();
                applied = true;
            }
        }
        assert!(status_ok && trackers_ok, "status {status_ok}, trackers {trackers_ok}");
    }
}
