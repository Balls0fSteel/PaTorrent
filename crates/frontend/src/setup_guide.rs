//! Options > Setup Guide (D53): a few questions at the first start of a profile (a settings
//! file without keys, [`should_show`]) and from the Options menu afterwards (Ctrl+G, µTorrent
//! 2.2.1's entry). Each page is a question, a short text of its own and controls built from the
//! Preferences fields ([`pref_pages`]) where one exists, so their help texts (D55) come along:
//!
//! 1. where downloads go: the download folder and moving finished downloads;
//! 2. the router: modern or not sure, or older or the internet provider's basic box, whose
//!    small connection table wants fewer connections ([`OLDER_CONNECTIONS`] and the rest);
//!    Keep ping low (D49) beside it for games;
//! 3. the disks: smart disk I/O (D48: `check_gently`, `tune_for_drive`, the busy-disk
//!    throttles) in one box;
//! 4. this computer: starting with the system and the `.torrent` and `magnet:` associations,
//!    only where the toolkit offers them ([`Features::desktop`]; a package's associations are
//!    its own, [`Features::packaged`]).
//!
//! Nothing is written until Finish ([`Guide::apply`]), which, like Preferences, applies only
//! the answers the user changed; Skip keeps everything as it is. No speed test and no network
//! test: both would need servers the project does not run.

use std::collections::HashMap;

use patorrent_core::Settings;

use crate::lang::S;
use crate::options::{GuiOptions, Options};
use crate::pref_pages::{self, Control, Features, Field, Form, Item, Page};
use crate::prefs::Prefs;

/// The older router's numbers: connections in all (`connections_limit`, default 200), per new
/// torrent (`bt.max_peers`, default no limit) and new connection attempts a second
/// (`connection_speed`, default 30). Upload slots stay as they are. A starting point for the
/// owner to confirm.
pub const OLDER_CONNECTIONS: i32 = 100;
pub const OLDER_PER_TORRENT: u32 = 40;
pub const OLDER_CONNECTION_SPEED: i32 = 8;

/// The router question's field and its two answers.
pub const ROUTER: &str = "guide.router";
pub const MODERN: &str = "modern";
pub const OLDER: &str = "older";
/// The disks page's "Use smart disk I/O" box.
pub const SMART_DISK: &str = "guide.smart_disk";

/// The settings file key the guide leaves when it closes (Finish or Skip), so a profile whose
/// file had no keys yet does not ask again at its next start.
pub const SEEN_KEY: &str = "ui.setup_guide_seen";

/// Whether the guide opens at this start: the profile's settings file has no keys yet (the
/// default profile included). Ask before [`crate::profile::first_run_settings`], which writes
/// the first keys.
pub fn should_show(prefs: &Prefs) -> bool {
    prefs.entries().next().is_none()
}

/// Marks the guide as seen in `prefs` (see [`SEEN_KEY`]); the caller saves the file.
pub fn mark_seen(prefs: &mut Prefs) {
    prefs.set(SEEN_KEY, true);
}

/// The controls' values by field key, as [`pref_pages::Field::shown`] gives them: `true` /
/// `false` for a box, the text, the choice's value.
pub type Answers = HashMap<&'static str, String>;

/// The guide's pages and the values its controls were given.
pub struct Guide {
    /// The pages, in order; each page's own text is [`page_text`].
    pub pages: Vec<Page>,
    form: Form,
}

impl Guide {
    /// The pages for what the toolkit offers (`features.desktop` gives the last page).
    pub fn new(features: Features) -> Guide {
        Guide { pages: pages(features), form: Form::default() }
    }

    /// The values to write into the controls for `o`, remembered so Finish applies only what
    /// the user changed.
    pub fn write(&mut self, o: &Options) -> Vec<(&'static str, String)> {
        self.form.write(&self.pages, o)
    }

    /// Finish: `o` with the answers that differ from what [`Guide::write`] put into the
    /// controls applied (a key `answers` lacks counts as unchanged). On a value that cannot be
    /// stored nothing changes and the field's key and a message come back.
    pub fn apply(&self, answers: &Answers, o: &mut Options) -> Result<(), (&'static str, String)> {
        let now: HashMap<&'static str, String> =
            pref_pages::fields(&self.pages).map(|f| (f.key, f.shown(o))).collect();
        let new = self
            .form
            .read(&self.pages, o, |k| answers.get(k).or_else(|| now.get(k)).cloned().unwrap_or_default())?;
        *o = new;
        Ok(())
    }

    /// The index of the page holding field `key` (to show the page of a refused value).
    pub fn page_of(&self, key: &str) -> Option<usize> {
        self.pages.iter().position(|p| p.items.iter().any(|i| matches!(i, Item::Field(f) if f.key == key)))
    }
}

/// A page's own short text, shown under its title above the controls.
pub fn page_text(page: &Page) -> S {
    match page.key {
        "guide.downloads" => S::GuideDownloadsText,
        "guide.router" => S::GuideRouterText,
        "guide.disks" => S::GuideDisksText,
        _ => S::GuideComputerText,
    }
}

/// "Step 2 of 4" for page `index` (from 0) of `count`.
pub fn step_text(index: usize, count: usize) -> String {
    crate::tr!(GuideStep, step = index + 1, count)
}

/// Whether `choice` should be drawn as option buttons rather than a drop-down: the guide's
/// questions have two or three answers.
pub fn as_options(choice: &Control, o: &Options) -> Option<Vec<(String, String)>> {
    match choice {
        Control::Choice { choices, .. } => Some(choices(o)).filter(|c| c.len() <= 3),
        _ => None,
    }
}

/// The Preferences field `key` (every page, everything a toolkit can offer).
fn pref_field(key: &str) -> Field {
    let all = Features { tray: true, desktop: true, packaged: false, updates: true, alternate_rows: true };
    *pref_pages::fields(&pref_pages::pages(all))
        .find(|f| f.key == key)
        .unwrap_or_else(|| panic!("no Preferences field {key}"))
}

/// Defaults an empty settings file gives: the modern answer goes back to these.
fn default_gui() -> GuiOptions {
    GuiOptions::load(&Prefs::default(), "")
}

fn router_choices(_: &Options) -> Vec<(String, String)> {
    vec![
        (MODERN.to_owned(), crate::tr!(GuideRouterModern).to_owned()),
        (OLDER.to_owned(), crate::tr!(GuideRouterOlder).to_owned()),
    ]
}

/// "older" while all three numbers are the older router's, else "modern".
fn router_get(o: &Options) -> String {
    let older = o.settings.connections_limit == OLDER_CONNECTIONS
        && o.gui.max_peers_per_torrent == OLDER_PER_TORRENT
        && o.settings.connection_speed == OLDER_CONNECTION_SPEED;
    (if older { OLDER } else { MODERN }).to_owned()
}

/// The older router's numbers, or the defaults for the modern one (only set when the answer
/// changed, so a modern answer undoes an older one and leaves other numbers alone).
fn router_set(o: &mut Options, v: String) {
    if v == OLDER {
        o.settings.connections_limit = OLDER_CONNECTIONS;
        o.gui.max_peers_per_torrent = OLDER_PER_TORRENT;
        o.settings.connection_speed = OLDER_CONNECTION_SPEED;
    } else {
        let d = Settings::default();
        o.settings.connections_limit = d.connections_limit;
        o.settings.connection_speed = d.connection_speed;
        o.gui.max_peers_per_torrent = default_gui().max_peers_per_torrent;
    }
}

/// Smart disk I/O: ticked while gentle checks, per-drive tuning and both busy-disk throttles
/// are all on.
fn smart_get(o: &Options) -> bool {
    let s = &o.settings;
    s.check_gently && s.tune_for_drive && s.disk_slow_downloads && s.disk_slow_uploads
}

fn smart_set(o: &mut Options, on: bool) {
    let s = &mut o.settings;
    s.check_gently = on;
    s.tune_for_drive = on;
    s.disk_slow_downloads = on;
    s.disk_slow_uploads = on;
}

fn router_note() -> String {
    crate::tr!(
        GuideRouterNote,
        total = OLDER_CONNECTIONS,
        per_torrent = OLDER_PER_TORRENT,
        per_second = OLDER_CONNECTION_SPEED
    )
}

fn disks_note() -> String {
    crate::tr!(GuideDisksNote).to_owned()
}

/// The pages for `features`.
pub fn pages(features: Features) -> Vec<Page> {
    let field = |key| Item::Field(pref_field(key));
    let mut pages = vec![
        Page {
            key: "guide.downloads",
            title: S::GuidePageDownloads,
            parent: None,
            items: vec![field("dir.download"), field("dir.move_completed"), field("dir.completed")],
        },
        Page {
            key: "guide.router",
            title: S::GuidePageRouter,
            parent: None,
            items: vec![
                Item::Field(Field {
                    key: ROUTER,
                    label: Some(S::GuideRouter),
                    control: Control::Choice { choices: router_choices, get: router_get, set: router_set },
                    indent: false,
                    beside: false,
                    needs: &[],
                    fixed: false,
                    help: None,
                }),
                Item::Note(router_note),
                field("auto_rate_limit"),
            ],
        },
        Page {
            key: "guide.disks",
            title: S::GuidePageDisks,
            parent: None,
            items: vec![
                Item::Field(Field {
                    key: SMART_DISK,
                    label: Some(S::GuideSmartDisk),
                    control: Control::Check { get: smart_get, set: smart_set },
                    indent: false,
                    beside: false,
                    needs: &[],
                    fixed: false,
                    help: None,
                }),
                Item::Note(disks_note),
            ],
        },
    ];
    if features.desktop {
        let mut items = vec![field("assoc.autostart")];
        // a package's associations are its own (Preferences shows them greyed)
        if !features.packaged {
            items.extend([field("assoc.torrent"), field("assoc.magnet")]);
        }
        pages.push(Page { key: "guide.computer", title: S::GuidePageComputer, parent: None, items });
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::H;

    const ALL: Features =
        Features { tray: true, desktop: true, packaged: false, updates: true, alternate_rows: true };

    fn opts() -> Options {
        Options::load(&Prefs::default(), "/dl")
    }

    /// The guide opened on `o` and finished with `answers` (the rest as written).
    fn finish(o: &Options, answers: &[(&'static str, &str)]) -> Result<Options, (&'static str, String)> {
        let mut g = Guide::new(ALL);
        let mut answered: Answers = g.write(o).into_iter().collect();
        for (k, v) in answers {
            assert!(answered.contains_key(k), "no control {k}");
            answered.insert(k, (*v).to_owned());
        }
        let mut new = o.clone();
        g.apply(&answered, &mut new).map(|()| new)
    }

    #[test]
    fn the_pages_and_their_help() {
        let pages = pages(ALL);
        let keys: Vec<&str> = pages.iter().map(|p| p.key).collect();
        assert_eq!(keys, ["guide.downloads", "guide.router", "guide.disks", "guide.computer"]);
        let texts: Vec<S> = pages.iter().map(page_text).collect();
        assert_eq!(
            texts,
            [S::GuideDownloadsText, S::GuideRouterText, S::GuideDisksText, S::GuideComputerText]
        );
        let mut seen = std::collections::HashSet::new();
        for f in pref_pages::fields(&pages) {
            assert!(seen.insert(f.key), "{} twice", f.key);
            let h = f.help_key();
            assert!(h.is_some_and(|h| !h.english().is_empty()), "{} has no help text", f.key);
        }
        assert_eq!(
            pref_pages::fields(&pages).find(|f| f.key == ROUTER).unwrap().help_key(),
            Some(H::GuideRouter)
        );
        assert_eq!(
            pref_pages::fields(&pages).find(|f| f.key == SMART_DISK).unwrap().help_key(),
            Some(H::GuideSmartDisk)
        );
        // every control a field needs is on its page
        for p in &pages {
            for i in &p.items {
                if let Item::Field(f) = i {
                    for need in f.needs {
                        assert!(
                            p.items.iter().any(|j| matches!(j, Item::Field(g) if g.key == *need)),
                            "{need}"
                        );
                    }
                }
            }
        }
        assert_eq!(step_text(1, 4), "Step 2 of 4");
        let router = pref_pages::fields(&pages).find(|f| f.key == ROUTER).unwrap();
        assert_eq!(as_options(&router.control, &opts()).map(|c| c.len()), Some(2));
        assert!(router_note().contains("100") && router_note().contains('8'));
    }

    #[test]
    fn the_last_page_only_where_offered() {
        let none = pages(Features { desktop: false, ..ALL });
        assert_eq!(none.len(), 3);
        let packaged = pages(Features { packaged: true, ..ALL });
        let keys: Vec<&str> = pref_pages::fields(&packaged[3..]).map(|f| f.key).collect();
        assert_eq!(keys, ["assoc.autostart"]);
        let keys: Vec<&str> = pref_pages::fields(&pages(ALL)[3..]).map(|f| f.key).collect();
        assert_eq!(keys, ["assoc.autostart", "assoc.torrent", "assoc.magnet"]);
    }

    #[test]
    fn skip_and_untouched_pages_change_nothing() {
        let o = opts();
        // Skip: apply is never called; Finish with nothing changed, or with no answers at all
        assert_eq!(finish(&o, &[]).unwrap(), o);
        let g = {
            let mut g = Guide::new(ALL);
            g.write(&o);
            g
        };
        let mut same = o.clone();
        g.apply(&Answers::new(), &mut same).unwrap();
        assert_eq!(same, o);
        // custom numbers survive an untouched router page
        let mut custom = opts();
        custom.settings.connections_limit = 500;
        custom.settings.check_gently = false;
        assert_eq!(finish(&custom, &[]).unwrap(), custom);
    }

    #[test]
    fn where_downloads_go() {
        let o = opts();
        let new = finish(
            &o,
            &[("dir.download", r"D:\dl"), ("dir.move_completed", "true"), ("dir.completed", r"E:\done")],
        )
        .unwrap();
        assert_eq!(new.gui.download_dir, r"D:\dl");
        assert!(new.gui.move_completed);
        assert_eq!(new.gui.completed_dir, r"E:\done");
        assert_eq!(new.settings, o.settings);
        // moving without a folder is refused, naming the field
        let e = finish(&o, &[("dir.move_completed", "true")]).unwrap_err();
        assert_eq!(e.0, "dir.completed");
        assert_eq!(Guide::new(ALL).page_of(e.0), Some(0));
        assert_eq!(finish(&o, &[("dir.download", " ")]).unwrap_err().0, "dir.download");
    }

    #[test]
    fn the_older_router_lowers_the_connections() {
        let o = opts();
        assert_eq!(router_get(&o), MODERN);
        let older = finish(&o, &[(ROUTER, OLDER)]).unwrap();
        assert_eq!(older.settings.connections_limit, OLDER_CONNECTIONS);
        assert_eq!(older.settings.connection_speed, OLDER_CONNECTION_SPEED);
        assert_eq!(older.gui.max_peers_per_torrent, OLDER_PER_TORRENT);
        assert_eq!(older.gui.upload_slots_per_torrent, o.gui.upload_slots_per_torrent);
        assert_eq!((older.settings.connections_limit, older.settings.connection_speed), (100, 8));
        assert_eq!(older.gui.max_peers_per_torrent, 40);
        assert_eq!(router_get(&older), OLDER);
        // the numbers land in the settings file
        let mut prefs = Prefs::default();
        older.store(&mut prefs);
        let back = Options::load(&prefs, "/dl");
        assert_eq!(back.settings.connections_limit, 100);
        assert_eq!(back.settings.connection_speed, 8);
        assert_eq!(back.gui.max_peers_per_torrent, 40);
        // the modern answer keeps the defaults, and undoes the older one
        assert_eq!(finish(&o, &[(ROUTER, MODERN)]).unwrap(), o);
        assert_eq!(finish(&older, &[(ROUTER, MODERN)]).unwrap(), o);
        // keep ping low on the same page
        assert!(finish(&o, &[("auto_rate_limit", "true")]).unwrap().settings.auto_rate_limit);
    }

    #[test]
    fn smart_disk_io() {
        let o = opts();
        assert!(!smart_get(&o), "the busy-disk throttles are off by default");
        let on = finish(&o, &[(SMART_DISK, "true")]).unwrap();
        let s = &on.settings;
        assert!(s.check_gently && s.tune_for_drive && s.disk_slow_downloads && s.disk_slow_uploads);
        assert_eq!(on.gui, o.gui);
        let off = finish(&on, &[(SMART_DISK, "false")]).unwrap();
        let s = &off.settings;
        assert!(!s.check_gently && !s.tune_for_drive && !s.disk_slow_downloads && !s.disk_slow_uploads);
        // untouched: gentle checks and tuning stay on as they are by default
        let same = finish(&o, &[(SMART_DISK, "false")]).unwrap();
        assert!(same.settings.check_gently && same.settings.tune_for_drive);
    }

    #[test]
    fn this_computer() {
        let o = opts();
        let new =
            finish(&o, &[("assoc.autostart", "true"), ("assoc.torrent", "true"), ("assoc.magnet", "true")])
                .unwrap();
        assert!(new.integration.autostart && new.integration.torrent && new.integration.magnet);
        assert!(!new.integration.minimized);
        assert_eq!((new.settings.clone(), new.gui.clone()), (o.settings.clone(), o.gui.clone()));
    }

    #[test]
    fn shown_at_a_first_start_only() {
        let mut prefs = Prefs::default();
        assert!(should_show(&prefs));
        mark_seen(&mut prefs);
        assert!(!should_show(&prefs));
        let mut prefs = Prefs::default();
        prefs.set("net.port", 6881);
        assert!(!should_show(&prefs));
    }
}
