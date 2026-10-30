//! The update check (D32, D44): PaTorrent asks the project website which releases exist and,
//! when a newer one is out, asks once in a message box whether to open its download page, and
//! says so in the status bar until that notice is clicked, linking to the release's page on
//! GitHub. Nothing is downloaded or installed here: the user fetches the new version (the
//! installer upgrades in place, the zip is replaced by hand).
//!
//! * The website's `update.txt` ([`MANIFEST_URL`], written from the GitHub releases by
//!   `.github/workflows/pages.yml`, docs/WEBSITE.md) holds `key=value` lines: `stable=` the
//!   newest release, `prerelease=` the newest pre-release; `#` lines are comments, unknown keys
//!   are ignored and either key may be missing ([`Manifest`]).
//! * [`Checker`] decides when: [`FIRST_CHECK`] after the start, then every [`INTERVAL`], while
//!   Preferences > General "Check for updates automatically" is on; Help > Check for Updates
//!   asks at once and answers in a message box. The GUI runs [`fetch`] on a worker thread.
//! * The box is shown once per version: the newest version it was shown for is kept as
//!   `update.seen` in the GUI's settings ([`Checker::seen`]), so a later start or a later check
//!   finding the same version only shows the status bar's notice.
//! * A pre-release is offered when "Update to beta versions" is on, or when the running version
//!   is a pre-release itself: a release candidate's tester hears of the next candidate.
//! * A build with `PATORRENT_NO_UPDATE_CHECK` set has no check at all ([`AVAILABLE`]), for
//!   packages that a package manager updates; the GTK GUI also leaves it out in a Flatpak.
//! * For a hand test, `PATORRENT_UPDATE_URL` at run time names another `update.txt` (a local
//!   `http://127.0.0.1:8000/update.txt` with `stable=9.9.9`, say).

use std::cmp::Ordering;
use std::fmt;
use std::time::{Duration, Instant};

use patorrent_core::fetch::{FetchOptions, FetchProxy, http_get};

/// Where the releases are listed (the project website, D33).
pub const MANIFEST_URL: &str = "https://balls0fsteel.github.io/PaTorrent/update.txt";
/// The project website, for Help > PaTorrent Website.
pub const WEBSITE_URL: &str = "https://balls0fsteel.github.io/PaTorrent/";
const RELEASES_URL: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases");

/// Whether this build checks for updates at all.
pub const AVAILABLE: bool = option_env!("PATORRENT_NO_UPDATE_CHECK").is_none();

/// The first automatic check after the start, once the session is up.
pub const FIRST_CHECK: Duration = Duration::from_secs(10);
/// Between automatic checks, for a PaTorrent left running.
pub const INTERVAL: Duration = Duration::from_secs(12 * 60 * 60);

/// A release's version: `MAJOR.MINOR.PATCH` with an optional `-pre.release` suffix, ordered as
/// Semantic Versioning orders them (`1.0.0-rc.2` < `1.0.0-rc.10` < `1.0.0`). Displayed as
/// written.
#[derive(Clone, Debug)]
pub struct Version {
    text: String,
    core: [u64; 3],
    pre: Vec<String>,
}

impl Version {
    /// `1.0.0`, `1.0.0-rc.1`; `None` for anything else (a leading `v`, build metadata, empty
    /// identifiers).
    pub fn parse(s: &str) -> Option<Version> {
        let (core_text, pre_text) = match s.split_once('-') {
            Some((c, p)) => (c, Some(p)),
            None => (s, None),
        };
        let mut core = [0; 3];
        let mut parts = core_text.split('.');
        for n in &mut core {
            let p = parts.next()?;
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            *n = p.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        let mut pre = Vec::new();
        if let Some(p) = pre_text {
            for id in p.split('.') {
                if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
                    return None;
                }
                pre.push(id.to_owned());
            }
        }
        Some(Version { text: s.to_owned(), core, pre })
    }

    /// The running PaTorrent's.
    pub fn current() -> Version {
        Version::parse(patorrent_core::VERSION).expect("the crate version is MAJOR.MINOR.PATCH")
    }

    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }

    /// The release's page on GitHub, where its files are.
    pub fn release_url(&self) -> String {
        format!("{RELEASES_URL}/tag/v{}", self.text)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Version) -> Ordering {
        self.core.cmp(&other.core).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            // a pre-release comes before its release
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let o = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(a), Ok(b)) => a.cmp(&b),
                        // numbers come before words
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => a.cmp(b),
                    };
                    if o != Ordering::Equal {
                        return o;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Version) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for Version {
    fn eq(&self, other: &Version) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Version {}

/// What `update.txt` lists.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// The newest release.
    pub stable: Option<Version>,
    /// The newest pre-release.
    pub prerelease: Option<Version>,
}

impl Manifest {
    /// Reads the `key=value` lines; a value that is no version, or a `stable` with a suffix or
    /// a `prerelease` without one, is left out.
    pub fn parse(text: &str) -> Manifest {
        let mut m = Manifest::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let v = Version::parse(value.trim());
            match key.trim() {
                "stable" => m.stable = v.filter(|v| !v.is_prerelease()),
                "prerelease" => m.prerelease = v.filter(Version::is_prerelease),
                _ => {}
            }
        }
        m
    }

    /// The newest version to offer to `current`, when one is newer: the release, and the
    /// pre-release with `beta` on or when `current` is a pre-release.
    pub fn newer(&self, current: &Version, beta: bool) -> Option<&Version> {
        let pre = self.prerelease.as_ref().filter(|_| beta || current.is_prerelease());
        [self.stable.as_ref(), pre].into_iter().flatten().filter(|v| *v > current).max()
    }
}

/// Fetches and reads `update.txt` (blocking: call it from a worker thread), through the
/// session's proxy as the RSS feeds go.
pub fn fetch(proxy: Option<FetchProxy>) -> Result<Manifest, String> {
    let opts = FetchOptions { proxy, max_size: 64 * 1024, ..FetchOptions::default() };
    let url = std::env::var("PATORRENT_UPDATE_URL").unwrap_or_else(|_| MANIFEST_URL.to_owned());
    let resp = http_get(&url, &opts).map_err(|e| e.to_string())?;
    if let Some(to) = resp.redirect() {
        return Err(format!("HTTP {} -> {to}", resp.status));
    }
    Ok(Manifest::parse(&String::from_utf8_lossy(&resp.body)))
}

/// Who asked for a check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// The timer: a newer version goes to the status bar and the Logger, a failure to the
    /// Logger only.
    Automatic,
    /// Help > Check for Updates: the answer goes to a message box.
    Manual,
}

/// A check's answer: the newer version, `None` when the running one is the newest.
pub type Answer = Result<Option<Version>, String>;

/// What [`Checker::finish`] makes of a check's result.
#[derive(Debug, PartialEq, Eq)]
pub struct Finished {
    /// Who asked.
    pub kind: Kind,
    pub answer: Answer,
    /// An automatic check found a version the box has not been shown for yet: show it once,
    /// and save [`Checker::seen`].
    pub announce: bool,
}

/// When the checks run, and the status bar's notice. The GUI calls [`Checker::due`] from its
/// timer and runs [`fetch`] on a thread when it says so (or for [`Checker::start_manual`]),
/// hands the result to [`Checker::finish`], shows the box when it says `announce`, and shows
/// the status bar's [`Checker::notice`] until it is clicked ([`Checker::dismiss`]).
#[derive(Debug)]
pub struct Checker {
    /// When the next automatic check is due; `None` while they are off.
    next: Option<Instant>,
    /// A check is on a worker thread; a manual request during an automatic one makes it
    /// manual.
    running: Option<Kind>,
    /// The newer version the status bar shows.
    notice: Option<Version>,
    /// The newest version a box was shown for (`update.seen`).
    seen: Option<Version>,
}

impl Checker {
    /// With automatic checks `on` (and [`AVAILABLE`]), the first [`FIRST_CHECK`] from `now`;
    /// `seen` is the stored `update.seen`, the newest version already announced in a box.
    pub fn new(on: bool, now: Instant, seen: Option<Version>) -> Checker {
        Checker { next: (on && AVAILABLE).then(|| now + FIRST_CHECK), running: None, notice: None, seen }
    }

    /// Preferences changed: turning the automatic check on checks soon, off stops it (a check
    /// on its way still answers).
    pub fn set_automatic(&mut self, on: bool, now: Instant) {
        match (on && AVAILABLE, self.next) {
            (true, None) => self.next = Some(now + FIRST_CHECK),
            (false, _) => self.next = None,
            (true, Some(_)) => {}
        }
    }

    /// Whether an automatic check should start now; if so it counts as started.
    pub fn due(&mut self, now: Instant) -> bool {
        match self.next {
            Some(at) if now >= at && self.running.is_none() => {
                self.next = Some(now + INTERVAL);
                self.running = Some(Kind::Automatic);
                true
            }
            _ => false,
        }
    }

    /// Help > Check for Updates: `true` to start a check; `false` when one is already on its
    /// way, which now answers in the message box.
    pub fn start_manual(&mut self) -> bool {
        let start = self.running.is_none();
        self.running = Some(Kind::Manual);
        start
    }

    /// A check's result: who asked, and the answer for `current` with `beta`. An automatic
    /// check that finds a newer version puts it in the status bar, and says `announce` when no
    /// box has been shown for it (or a newer one) yet; one that finds nothing newer takes the
    /// notice down. A version shown in either box counts as seen.
    pub fn finish(&mut self, result: Result<Manifest, String>, current: &Version, beta: bool) -> Finished {
        let kind = self.running.take().unwrap_or(Kind::Automatic);
        let answer = result.map(|m| m.newer(current, beta).cloned());
        let mut announce = false;
        match (kind, &answer) {
            (Kind::Automatic, Ok(Some(v))) => {
                self.notice = Some(v.clone());
                announce = self.seen.as_ref().is_none_or(|s| v > s);
                self.mark_seen(v);
            }
            (Kind::Automatic, Ok(None)) => self.notice = None,
            (Kind::Manual, Ok(Some(v))) => self.mark_seen(v),
            _ => {}
        }
        Finished { kind, answer, announce }
    }

    fn mark_seen(&mut self, v: &Version) {
        if self.seen.as_ref().is_none_or(|s| v > s) {
            self.seen = Some(v.clone());
        }
    }

    /// The newest version a box was shown for, to store as `update.seen`.
    pub fn seen(&self) -> Option<&Version> {
        self.seen.as_ref()
    }

    /// The newer version the status bar shows now.
    pub fn notice(&self) -> Option<&Version> {
        self.notice.as_ref()
    }

    /// Hides the notice now (it was clicked).
    pub fn dismiss(&mut self) {
        self.notice = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    #[test]
    fn versions_parse_and_print_as_written() {
        assert_eq!(v("1.0.0").to_string(), "1.0.0");
        assert_eq!(v("1.0.0-rc.1").to_string(), "1.0.0-rc.1");
        assert!(v("1.0.0-rc.1").is_prerelease() && !v("1.0.0").is_prerelease());
        assert_eq!(v("12.34.56-alpha-2.x").core, [12, 34, 56]);
        for bad in [
            "",
            "1",
            "1.0",
            "1.0.0.0",
            "v1.0.0",
            "1.0.0-",
            "1.0.0-rc..1",
            "1.0.0+build",
            "1.a.0",
            " 1.0.0",
            "1.0.0-rc_1",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad}");
        }
        let current = Version::current();
        assert_eq!(current.to_string(), patorrent_core::VERSION);
        assert_eq!(
            v("1.0.0-rc.1").release_url(),
            "https://github.com/Balls0fSteel/PaTorrent/releases/tag/v1.0.0-rc.1"
        );
    }

    #[test]
    fn semver_precedence() {
        // the example from the Semantic Versioning specification, item 11
        let ordered = [
            "1.0.0-alpha",
            "1.0.0-alpha.1",
            "1.0.0-alpha.beta",
            "1.0.0-beta",
            "1.0.0-beta.2",
            "1.0.0-beta.11",
            "1.0.0-rc.1",
            "1.0.0-rc.2",
            "1.0.0-rc.10",
            "1.0.0",
            "1.0.1",
            "1.1.0-rc.1",
            "1.1.0",
            "2.0.0",
            "10.0.0",
        ];
        for pair in ordered.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
        assert_eq!(v("1.0.0"), v("1.0.0"));
    }

    #[test]
    fn manifest() {
        let m = Manifest::parse(
            "# PaTorrent update information\r\nstable=1.0.0\r\n prerelease = 1.1.0-rc.2 \r\nfuture=x\r\n\r\nnoise\r\n",
        );
        assert_eq!(m, Manifest { stable: Some(v("1.0.0")), prerelease: Some(v("1.1.0-rc.2")) });
        assert_eq!(Manifest::parse("# no releases yet\n"), Manifest::default());
        assert_eq!(Manifest::parse("<html>404</html>"), Manifest::default());
        // the wrong kind of version for its key, or none
        assert_eq!(Manifest::parse("stable=1.0.0-rc.1\nprerelease=1.0.0\n"), Manifest::default());
        assert_eq!(Manifest::parse("stable=v1.0.0\nprerelease=\n"), Manifest::default());
    }

    #[test]
    fn what_is_offered() {
        let both = Manifest { stable: Some(v("1.0.0")), prerelease: Some(v("1.1.0-rc.1")) };
        // a release user: the release only, unless beta versions are on
        assert_eq!(both.newer(&v("0.9.0"), false), Some(&v("1.0.0")));
        assert_eq!(both.newer(&v("0.9.0"), true), Some(&v("1.1.0-rc.1")));
        assert_eq!(both.newer(&v("1.0.0"), false), None);
        assert_eq!(both.newer(&v("1.0.0"), true), Some(&v("1.1.0-rc.1")));
        // a release candidate's tester hears of the next candidate and of the release
        assert_eq!(both.newer(&v("1.0.0-rc.1"), false), Some(&v("1.1.0-rc.1")));
        let older_pre = Manifest { stable: Some(v("1.0.0")), prerelease: Some(v("1.0.0-rc.2")) };
        assert_eq!(older_pre.newer(&v("1.0.0-rc.1"), false), Some(&v("1.0.0")));
        assert_eq!(older_pre.newer(&v("1.0.0"), true), None);
        let pre_only = Manifest { stable: None, prerelease: Some(v("1.0.0-rc.2")) };
        assert_eq!(pre_only.newer(&v("1.0.0-rc.1"), false), Some(&v("1.0.0-rc.2")));
        assert_eq!(pre_only.newer(&v("1.0.0-rc.2"), false), None);
        assert_eq!(Manifest::default().newer(&v("1.0.0"), true), None);
    }

    fn newer(s: &str) -> Result<Manifest, String> {
        Ok(Manifest { stable: Some(v(s)), prerelease: None })
    }

    #[test]
    fn automatic_checks_and_the_notice() {
        let t0 = Instant::now();
        let current = v("1.0.0");
        let mut c = Checker::new(true, t0, None);
        if !AVAILABLE {
            assert!(!c.due(t0 + FIRST_CHECK));
            return;
        }
        assert!(!c.due(t0));
        assert!(c.due(t0 + FIRST_CHECK));
        // one at a time
        assert!(!c.due(t0 + FIRST_CHECK + INTERVAL));
        // the first newer version: the box once, and the notice until it is clicked
        assert_eq!(
            c.finish(newer("1.0.1"), &current, false),
            Finished { kind: Kind::Automatic, answer: Ok(Some(v("1.0.1"))), announce: true }
        );
        assert_eq!(c.notice(), Some(&v("1.0.1")));
        assert_eq!(c.seen(), Some(&v("1.0.1")));
        // the next one an interval after the last started; the same version is no news
        assert!(!c.due(t0 + FIRST_CHECK + INTERVAL - Duration::from_secs(1)));
        let t2 = t0 + FIRST_CHECK + INTERVAL;
        assert!(c.due(t2));
        let f = c.finish(newer("1.0.1"), &current, false);
        assert!(!f.announce && f.answer == Ok(Some(v("1.0.1"))));
        assert_eq!(c.notice(), Some(&v("1.0.1")), "the notice stays");
        c.dismiss();
        assert_eq!(c.notice(), None);
        // a newer one still: the box again, and the notice is back
        assert!(c.due(t2 + INTERVAL));
        assert!(c.finish(newer("1.0.2"), &current, false).announce);
        assert_eq!((c.notice(), c.seen()), (Some(&v("1.0.2")), Some(&v("1.0.2"))));
        // a failure leaves the notice; nothing newer takes it down
        assert!(c.due(t2 + INTERVAL * 2));
        assert_eq!(
            c.finish(Err("timed out".into()), &current, false),
            Finished { kind: Kind::Automatic, answer: Err("timed out".into()), announce: false }
        );
        assert_eq!(c.notice(), Some(&v("1.0.2")));
        assert!(c.due(t2 + INTERVAL * 3));
        assert_eq!(
            c.finish(Ok(Manifest::default()), &current, false),
            Finished { kind: Kind::Automatic, answer: Ok(None), announce: false }
        );
        assert_eq!(c.notice(), None);
        assert_eq!(c.seen(), Some(&v("1.0.2")), "seen is never forgotten");
    }

    #[test]
    fn the_box_is_shown_once_per_version_across_starts() {
        let t0 = Instant::now();
        let current = v("1.0.0");
        if !AVAILABLE {
            return;
        }
        // the next start, with update.seen stored: the notice only
        let mut c = Checker::new(true, t0, Some(v("1.0.1")));
        assert!(c.due(t0 + FIRST_CHECK));
        let f = c.finish(newer("1.0.1"), &current, false);
        assert!(!f.announce);
        assert_eq!(c.notice(), Some(&v("1.0.1")));
        // an older stored version does not cover a newer release
        let mut c = Checker::new(true, t0, Some(v("1.0.0-rc.1")));
        assert!(c.due(t0 + FIRST_CHECK));
        assert!(c.finish(newer("1.0.1"), &current, false).announce);
    }

    #[test]
    fn manual_checks_and_switching_off() {
        let t0 = Instant::now();
        let current = v("1.0.0");
        let mut c = Checker::new(false, t0, None);
        assert!(!c.due(t0 + INTERVAL * 2), "off");
        assert!(c.start_manual());
        assert!(!c.start_manual(), "one is on its way");
        assert_eq!(
            c.finish(newer("1.0.1"), &current, false),
            Finished { kind: Kind::Manual, answer: Ok(Some(v("1.0.1"))), announce: false }
        );
        assert_eq!(c.notice(), None, "a manual check answers in its message box");
        assert_eq!(c.seen(), Some(&v("1.0.1")), "and that box counts as seen");
        if !AVAILABLE {
            return;
        }
        // switched on: soon; an automatic check on its way becomes manual
        c.set_automatic(true, t0);
        assert!(c.due(t0 + FIRST_CHECK));
        assert!(!c.start_manual());
        assert_eq!(c.finish(Ok(Manifest::default()), &current, false).kind, Kind::Manual);
        // the version the manual box showed is no news to the automatic check
        c.set_automatic(true, t0 + FIRST_CHECK);
        assert!(!c.due(t0 + FIRST_CHECK * 2), "switching on again keeps the schedule");
        assert!(c.due(t0 + FIRST_CHECK + INTERVAL));
        assert!(!c.finish(newer("1.0.1"), &current, false).announce);
        assert_eq!(c.notice(), Some(&v("1.0.1")));
        c.set_automatic(false, t0);
        assert!(!c.due(t0 + INTERVAL * 3));
    }
}
