//! The status bar, the window title and the tray tip (GTK `update_status`, `sync_title`,
//! `show_update_notice`, `ask_open_release`; texts only).

use super::Core;
use crate::icons::{StateIcon, network_icon};
use crate::update::Version;
use crate::{fmt, lang, profile, tr};

/// The status bar's parts, as GTK's: the network part (with the scheduler's note), DHT, down,
/// up.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusBar {
    /// "Network OK", "No incoming connections", "Port N in use"; the scheduler's note after a
    /// `|` while it holds the session back.
    pub net: String,
    /// The network part's tooltip (the external address, D54).
    pub net_tip: String,
    /// The network indicator before it.
    pub net_icon: StateIcon,
    /// Another program holds the listen port: a click on the network part opens Preferences >
    /// Connection.
    pub port_taken: bool,
    pub dht: String,
    pub down: String,
    pub up: String,
    /// The update notice at the start (its text and the release page), until clicked
    /// ([`Core::dismiss_update_notice`]).
    pub update: Option<(String, String)>,
}

/// A newer version to offer in a dialog: OK opens [`UpdateOffer::url`].
#[derive(Clone, Debug)]
pub struct UpdateOffer {
    pub version: Version,
    /// The running one.
    pub current: Version,
}

impl UpdateOffer {
    /// The dialog's heading (Check for Updates, without its access key).
    pub fn message(&self) -> String {
        lang::plain(tr!(MenuCheckUpdates))
    }

    /// "Rookparcel {v} is available. Open its download page?"
    pub fn detail(&self) -> String {
        tr!(UpdateNewer, version = self.version, current = self.current)
    }

    /// The release's page.
    pub fn url(&self) -> String {
        self.version.release_url()
    }
}

impl Core {
    /// The status bar's texts now.
    pub fn status_bar(&self) -> StatusBar {
        let s = &self.stats;
        let net = match self.listen.text() {
            Some(t) => t,
            None if s.has_incoming_connections => tr!(NetOk).to_owned(),
            None => tr!(NetNoIncoming).to_owned(),
        };
        let net = crate::network::with_status(s, net);
        let [down, up] = crate::status::bar_rates(s);
        StatusBar {
            net: match self.scheduler.status_text() {
                Some(sched) => format!("{net} | {sched}"),
                None => net,
            },
            net_tip: crate::network::tooltip(s),
            net_icon: network_icon(s.has_incoming_connections),
            port_taken: self.listen.port().is_some(),
            dht: tr!(BarDht, count = s.dht_nodes),
            down,
            up,
            update: self.updates.notice().map(|v| (tr!(UpdateNotice, version = v), v.release_url())),
        }
    }

    /// The main window's title: with "Show current speed in the title bar" µTorrent's
    /// `D:… U:… - Rookparcel …`, else the name; a remote window's names its server, or says
    /// it is disconnected.
    pub fn title(&self) -> String {
        if let Some(r) = &self.remote {
            return match self.remote_lost {
                false => tr!(TitleRemote, version = profile::version(), label = r.label),
                true => tr!(TitleDisconnected, version = profile::version(), label = r.label),
            };
        }
        if self.title_speed {
            fmt::title_speed(self.stats.download_rate, self.stats.upload_rate, &profile::name_build())
        } else {
            profile::name_build()
        }
    }

    /// The tray (status item) icon's tooltip.
    pub fn tray_tip(&self) -> String {
        let s = &self.stats;
        tr!(
            TrayTip,
            version = profile::version(),
            down = fmt::speed_always(s.download_rate),
            up = fmt::speed_always(s.upload_rate)
        )
    }
}
