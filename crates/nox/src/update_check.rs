//! `run`'s update check (D32, D43): as the GUIs do, nox asks the project website which
//! releases exist ([`update::fetch`]: `update.txt` over HTTPS, through the session's proxy,
//! nothing about this installation sent) [`update::FIRST_CHECK`] after the start and then every
//! [`update::INTERVAL`], on a worker thread. A newer version is one log line (stdout, and the
//! Web UI's Logger and status bar); a failed check is one line on stderr, once per check.
//! `--no-update-check` or the Web UI's Preferences > General "Check for updates
//! automatically" (`update.check`) turns it off. nox never downloads or installs anything.

use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use patorrent_core::fetch::FetchProxy;
use patorrent_frontend::tr;
use patorrent_frontend::update::{self, Checker, Manifest, Version};

/// What fetches `update.txt`: [`update::fetch`], a fake one in the tests.
pub type Fetch = fn(Option<FetchProxy>) -> Result<Manifest, String>;

/// A finished check, or the check turned off.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A newer version is out.
    Newer(Version),
    /// The running version is the newest.
    Newest,
    Failed(String),
    /// The check was turned off (the Web UI's preferences): a shown version goes.
    Off,
}

impl Outcome {
    /// The log line, if any: the newer version with its release page, or the failure.
    pub fn line(&self) -> Option<String> {
        match self {
            Outcome::Newer(v) => Some(tr!(LogUpdateAvailable, version = v, url = v.release_url())),
            Outcome::Failed(e) => Some(tr!(LogUpdateFailed, error = e)),
            Outcome::Newest | Outcome::Off => None,
        }
    }

    /// What the Web UI's status bar shows now; `None` for a failure, which leaves it as it was.
    pub fn shown(&self) -> Option<Option<&Version>> {
        match self {
            Outcome::Newer(v) => Some(Some(v)),
            Outcome::Newest | Outcome::Off => Some(None),
            Outcome::Failed(_) => None,
        }
    }
}

/// The schedule ([`Checker`]) and the worker thread's answer. The main loop calls
/// [`UpdateCheck::poll`] every turn; the worker wakes it when it is done.
pub struct UpdateCheck {
    checker: Checker,
    current: Version,
    on: bool,
    fetch: Fetch,
    done_tx: Sender<Result<Manifest, String>>,
    done_rx: Receiver<Result<Manifest, String>>,
    wake: Sender<()>,
}

impl UpdateCheck {
    /// The check for the running version, through the website.
    pub fn new(on: bool, now: Instant, wake: Sender<()>) -> UpdateCheck {
        UpdateCheck::with(on, now, wake, Version::current(), update::fetch)
    }

    pub fn with(on: bool, now: Instant, wake: Sender<()>, current: Version, fetch: Fetch) -> UpdateCheck {
        let (done_tx, done_rx) = mpsc::channel();
        // a daemon has no box to show once, so nothing is "seen"
        UpdateCheck { checker: Checker::new(on, now, None), current, on, fetch, done_tx, done_rx, wake }
    }

    /// Follows the switch (`on`, with `beta` versions offered), hands back a finished check,
    /// and starts the next one when it is due, through the proxy `proxy` gives then.
    pub fn poll(
        &mut self,
        on: bool,
        beta: bool,
        now: Instant,
        proxy: impl FnOnce() -> Option<FetchProxy>,
    ) -> Option<Outcome> {
        let mut out = None;
        if on != self.on {
            self.on = on;
            self.checker.set_automatic(on, now);
            if !on {
                out = Some(Outcome::Off);
            }
        }
        if let Ok(result) = self.done_rx.try_recv() {
            let answer = self.checker.finish(result, &self.current, beta).answer;
            // one on its way when the check was turned off answers into the void
            if self.on {
                out = Some(match answer {
                    Ok(Some(v)) => Outcome::Newer(v),
                    Ok(None) => Outcome::Newest,
                    Err(e) => Outcome::Failed(e),
                });
            }
        }
        if self.checker.due(now) {
            let (fetch, done, wake, proxy) = (self.fetch, self.done_tx.clone(), self.wake.clone(), proxy());
            std::thread::spawn(move || {
                let _ = done.send(fetch(proxy));
                let _ = wake.send(());
            });
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use update::{FIRST_CHECK, INTERVAL};

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn newer(_: Option<FetchProxy>) -> Result<Manifest, String> {
        Ok(Manifest::parse("stable=1.0.1\nprerelease=1.1.0-rc.1\n"))
    }

    fn failing(_: Option<FetchProxy>) -> Result<Manifest, String> {
        Err("no route to host".into())
    }

    static NEVER_CALLS: AtomicUsize = AtomicUsize::new(0);
    fn never(_: Option<FetchProxy>) -> Result<Manifest, String> {
        NEVER_CALLS.fetch_add(1, Ordering::SeqCst);
        Err("must not be called".into())
    }

    /// Polls at `at` (starting a check when due), waits for the worker's wake-up, and polls
    /// again for its answer.
    fn check(u: &mut UpdateCheck, wake: &Receiver<()>, at: Instant, beta: bool) -> Option<Outcome> {
        assert_eq!(u.poll(true, beta, at, || None), None, "a check starts, nothing to say yet");
        wake.recv_timeout(Duration::from_secs(10)).expect("the worker wakes the loop");
        u.poll(true, beta, at, || None)
    }

    #[test]
    fn a_newer_version_once_per_check_on_the_schedule() {
        if !update::AVAILABLE {
            return;
        }
        let t0 = Instant::now();
        let (tx, wake) = mpsc::channel();
        let mut u = UpdateCheck::with(true, t0, tx, v("1.0.0"), newer);
        // nothing before the first check is due
        assert_eq!(u.poll(true, false, t0, || panic!("no check yet")), None);
        let out = check(&mut u, &wake, t0 + FIRST_CHECK, false).unwrap();
        assert_eq!(out, Outcome::Newer(v("1.0.1")));
        assert_eq!(
            out.line().unwrap(),
            "PaTorrent 1.0.1 is available: https://github.com/Balls0fSteel/PaTorrent/releases/tag/v1.0.1"
        );
        assert_eq!(out.shown(), Some(Some(&v("1.0.1"))));
        // quiet until the interval is over, then again (beta on: the release candidate)
        assert_eq!(u.poll(true, true, t0 + FIRST_CHECK + INTERVAL / 2, || panic!("not due")), None);
        let out = check(&mut u, &wake, t0 + FIRST_CHECK + INTERVAL, true).unwrap();
        assert_eq!(out, Outcome::Newer(v("1.1.0-rc.1")));
    }

    #[test]
    fn a_failure_is_one_line_and_keeps_what_is_shown() {
        if !update::AVAILABLE {
            return;
        }
        let t0 = Instant::now();
        let (tx, wake) = mpsc::channel();
        let mut u = UpdateCheck::with(true, t0, tx, v("1.0.0"), failing);
        let out = check(&mut u, &wake, t0 + FIRST_CHECK, false).unwrap();
        assert_eq!(out.line().unwrap(), "Update check failed: no route to host");
        assert_eq!(out.shown(), None);
        // no retry storm: the next attempt is a whole interval away
        for s in [1, 60, 3600] {
            assert_eq!(
                u.poll(true, false, t0 + FIRST_CHECK + Duration::from_secs(s), || panic!("retry")),
                None
            );
        }
        // the newest version: nothing logged, the status bar's link goes
        let (tx, wake) = mpsc::channel();
        let mut u = UpdateCheck::with(true, t0, tx, v("9.0.0"), newer);
        let out = check(&mut u, &wake, t0 + FIRST_CHECK, false).unwrap();
        assert_eq!((out.line(), out.shown()), (None, Some(None)));
    }

    #[test]
    fn off_never_fetches_and_turning_it_off_hides_the_version() {
        let t0 = Instant::now();
        let (tx, _wake) = mpsc::channel();
        // --no-update-check (or update.check off at start)
        let mut u = UpdateCheck::with(false, t0, tx.clone(), v("1.0.0"), never);
        for s in [0, 10, 3600, 86_400] {
            assert_eq!(u.poll(false, false, t0 + Duration::from_secs(s), || None), None);
        }
        assert_eq!(NEVER_CALLS.load(Ordering::SeqCst), 0);
        // on at start, off in the Web UI before the first check
        let mut u = UpdateCheck::with(true, t0, tx, v("1.0.0"), never);
        assert_eq!(u.poll(false, false, t0 + Duration::from_secs(1), || None), Some(Outcome::Off));
        assert_eq!(u.poll(false, false, t0 + FIRST_CHECK + INTERVAL, || None), None);
        assert_eq!(NEVER_CALLS.load(Ordering::SeqCst), 0);
    }
}
