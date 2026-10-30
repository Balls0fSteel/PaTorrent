//! nox's update check in the Web UI (D32, D43): the daemon's `run` asks the project website
//! for the releases (`main.rs`, `update_check.rs`) and hands the answer here; the page shows a
//! newer version as a link in its status bar (`hello` and `tick` carry it as `update`) and the
//! Logger gets the line. Preferences > General > Privacy turns the check off (`update.check`)
//! or on to beta versions (`update.beta`). Hosted (D40), the desktop client checks for itself:
//! nothing is set or sent here.

use patorrent_core::fetch::FetchProxy;
use patorrent_frontend::update::Version;

use super::json::Json;
use super::{State, WebUi, unix_now};

impl WebUi {
    /// Preferences > General > Privacy: (check for updates automatically, offer beta
    /// versions). Hosted, never on: the host checks.
    pub fn update_options(&self) -> (bool, bool) {
        let st = self.shared.lock();
        (st.prefs.check_updates && !st.hosted, st.prefs.update_beta)
    }

    /// The proxy the update check goes through: the engine's, as the RSS feeds use it.
    pub fn update_proxy(&self) -> Option<FetchProxy> {
        FetchProxy::from_settings(&self.shared.lock().settings)
    }

    /// The newer version for the status bar (`None`: the running one is the newest, or the
    /// check was turned off). Hosted, nothing.
    pub fn set_update(&self, newer: Option<&Version>) {
        let mut st = self.shared.lock();
        if !st.hosted {
            st.update = newer.cloned();
        }
    }

    /// A line for the Logger (the daemon's own, such as the update check's).
    pub fn log_line(&self, line: String) {
        self.shared.lock().push_log(None, line, unix_now());
    }
}

/// `hello`'s and `tick`'s `update`: the newer version and its release page, or `None` (left
/// out) while there is none, and always hosted.
pub(crate) fn json(st: &State) -> Option<Json> {
    let v = st.update.as_ref().filter(|_| !st.hosted)?;
    Some(Json::obj().with("version", v.to_string()).with("url", v.release_url()))
}
