//! The status bar's texts, icon and tooltips from `Core::status_bar` (GTK `update_status`,
//! `show_update_notice`): the update notice (a link: the release page opens and the notice goes
//! away), the network part (icon and text; while another program holds the port, a click opens
//! Preferences > Connection), DHT, down and up.

use objc2_app_kit::{NSButton, NSTextField, NSWorkspace};
use objc2_foundation::{NSString, NSURL};

use crate::app::{apply, peek, with_core};
use crate::commands::Command;
use crate::images;
use crate::window::Ui;

fn set_label(l: &NSTextField, text: &str) {
    if l.stringValue().to_string() != text {
        l.setStringValue(&NSString::from_str(text));
    }
}

fn set_title(b: &NSButton, text: &str) {
    if b.title().to_string() != text {
        b.setTitle(&NSString::from_str(text));
    }
}

fn set_tip(b: &NSButton, tip: &str) {
    if b.toolTip().map(|t| t.to_string()).unwrap_or_default() != tip {
        crate::ui::tooltip(b, tip);
    }
}

/// Shows `Core::status_bar` now (each second, and when the update notice comes or goes).
pub fn update(ui: &Ui) {
    let Some(s) = peek(|c| c.status_bar()) else { return };
    let st = &ui.status;
    set_title(&st.network, &s.net);
    set_tip(&st.network, &s.net_tip);
    st.network.setImage(Some(&images::state(s.net_icon, images::is_dark(&st.network))));
    set_label(&st.dht, &s.dht);
    set_label(&st.down, &s.down);
    set_label(&st.up, &s.up);
    match &s.update {
        Some((text, url)) => {
            set_title(&st.update, text);
            set_tip(&st.update, url);
            st.update.setHidden(false);
        }
        None => st.update.setHidden(true),
    }
}

/// The status bar's clicks: [`Command::UpdateNotice`] and [`Command::NetworkStatus`].
pub fn run(cmd: Command) {
    match cmd {
        Command::UpdateNotice => {
            let url = peek(|c| c.status_bar().update.map(|(_, url)| url)).flatten();
            if let Some(url) = url.and_then(|u| NSURL::URLWithString(&NSString::from_str(&u))) {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
            apply(with_core(|c| {
                c.dismiss_update_notice();
                c.events()
            }));
        }
        Command::NetworkStatus if peek(|c| c.status_bar().port_taken) == Some(true) => {
            crate::prefs::open_at("connection");
        }
        _ => {}
    }
}
