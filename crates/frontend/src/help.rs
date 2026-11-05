//! Help for settings and controls (D55): which help text ([`lang::H`]) a control or an Advanced
//! row shows, and where the wiki will be. The texts themselves are a language table
//! (`lang/help_en.rs`); every toolkit shows them three ways: a tooltip on hover, the Preferences
//! description pane for the focused or hovered control, and F1 on the focused control.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::lang::{self, H};
use crate::pref_pages::{self, Features};

/// The wiki page for a help text, once the wiki exists: `{lang}` is the current language's code
/// and `{anchor}` the key's anchor ([`H::anchor`]), such as
/// `https://example.org/wiki/{lang}/preferences#{anchor}`. `None` until then: F1 shows the
/// tooltip only. THE ONE PLACE THE WIKI'S ADDRESS GOES.
pub const WIKI_URL: Option<&str> = None;

/// The wiki page for `h` ([`WIKI_URL`]), `None` while there is no wiki.
pub fn wiki_url(h: H) -> Option<String> {
    WIKI_URL.map(|t| t.replace("{lang}", lang::current()).replace("{anchor}", &h.anchor()))
}

/// The help text of the Advanced table's row `name` (an engine setting, `settings::TABLE`): its
/// own (`Adv` and the name in CamelCase: `connection_speed` is `AdvConnectionSpeed`), else the
/// text of the Preferences field that edits the same setting under its name
/// (`connections_limit` shows the Bandwidth page's `PrefMaxConnections`).
pub fn for_setting(name: &str) -> Option<H> {
    H::from_key(&adv_key(name)).or_else(|| page_settings().get(name).copied())
}

/// `Adv` and `name` in CamelCase, split at `_` and `.`.
fn adv_key(name: &str) -> String {
    let mut out = String::from("Adv");
    for part in name.split(['_', '.']).filter(|p| !p.is_empty()) {
        let mut chars = part.chars();
        if let Some(first) = chars.next() {
            out.push(first.to_ascii_uppercase());
            out.extend(chars);
        }
    }
    out
}

/// The Preferences fields named like an engine setting, with their help keys.
fn page_settings() -> &'static HashMap<&'static str, H> {
    static MAP: OnceLock<HashMap<&'static str, H>> = OnceLock::new();
    MAP.get_or_init(|| {
        let all =
            Features { tray: true, desktop: true, packaged: false, updates: true, alternate_rows: true };
        let pages = pref_pages::pages(all);
        pref_pages::fields(&pages).filter_map(|f| f.help_key().map(|h| (f.key, h))).collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advanced_rows_find_their_text() {
        assert_eq!(adv_key("connection_speed"), "AdvConnectionSpeed");
        assert_eq!(adv_key("bt.transp_disposition"), "AdvBtTranspDisposition");
        assert_eq!(for_setting("connection_speed"), Some(H::AdvConnectionSpeed));
        // a setting a page edits under its own name shows the page's text
        assert_eq!(for_setting("connections_limit"), Some(H::PrefMaxConnections));
        assert_eq!(for_setting("no_such_setting"), None);
        assert_eq!(wiki_url(H::PrefDht).is_some(), WIKI_URL.is_some());
    }

    /// `docs/HELP_TEXTS.md`, the reviewable list of every help text with where its claim was
    /// read, is generated from the table: `ROOKPARCEL_WRITE_LNG=1` rewrites it, as it does
    /// `en.lng`.
    #[test]
    fn the_help_texts_document_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/HELP_TEXTS.md");
        let want = lang::help_texts_md();
        if std::env::var_os("ROOKPARCEL_WRITE_LNG").is_some() {
            std::fs::write(&path, &want).unwrap();
        }
        let have = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        assert!(have == want, "{} is stale: run the tests with ROOKPARCEL_WRITE_LNG=1", path.display());
    }
}
