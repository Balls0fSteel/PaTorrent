//! The main window's layout as the native macOS GUI keeps it in `rookparcel.ini` (not in AppKit's
//! autosave, which writes `~/Library/Preferences` even in portable mode): the View toggles and
//! split sizes under the Win32 GUI's keys (`ui.toolbar`, `ui.tree`, `ui.details`, `ui.status`,
//! `ui.tree_width`, `ui.detail_height`, `ui.tab`), and what only the Mac has under `ui.mac.*`:
//! the window frame (AppKit's own frame string), the toolbar's items and its display mode.

use crate::detail::Tab;
use crate::prefs::Prefs;

const TOOLBAR: &str = "ui.toolbar";
const SIDEBAR: &str = "ui.tree";
const DETAILS: &str = "ui.details";
const STATUS: &str = "ui.status";
const SIDEBAR_WIDTH: &str = "ui.tree_width";
const DETAIL_HEIGHT: &str = "ui.detail_height";
const TAB: &str = "ui.tab";
const FRAME: &str = "ui.mac.frame";
const TOOLBAR_ITEMS: &str = "ui.mac.toolbar";
const TOOLBAR_MODE: &str = "ui.mac.toolbar_mode";

/// The main window's layout.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub toolbar: bool,
    pub sidebar: bool,
    pub details: bool,
    pub status: bool,
    /// The sidebar's width in points (`None`: the default).
    pub sidebar_width: Option<f64>,
    /// The detail tabs' height in points.
    pub detail_height: Option<f64>,
    pub tab: Tab,
    /// The window's frame as AppKit writes it (`NSWindow.stringWithSavedFrame`).
    pub frame: Option<String>,
    /// The toolbar's item identifiers in order (`None`: the default toolbar).
    pub toolbar_items: Option<Vec<String>>,
    /// The toolbar's display mode (`NSToolbarDisplayMode`'s value).
    pub toolbar_mode: Option<usize>,
}

impl Default for Layout {
    fn default() -> Layout {
        Layout {
            toolbar: true,
            sidebar: true,
            details: true,
            status: true,
            sidebar_width: None,
            detail_height: None,
            tab: Tab::General,
            frame: None,
            toolbar_items: None,
            toolbar_mode: None,
        }
    }
}

/// A size kept in whole points; a size of nothing is no size.
fn size(prefs: &Prefs, key: &str) -> Option<f64> {
    prefs.get_i32(key).filter(|&v| v > 0).map(f64::from)
}

impl Layout {
    pub fn load(prefs: &Prefs) -> Layout {
        Layout {
            toolbar: prefs.get_bool(TOOLBAR, true),
            sidebar: prefs.get_bool(SIDEBAR, true),
            details: prefs.get_bool(DETAILS, true),
            status: prefs.get_bool(STATUS, true),
            sidebar_width: size(prefs, SIDEBAR_WIDTH),
            detail_height: size(prefs, DETAIL_HEIGHT),
            tab: prefs.get(TAB).and_then(Tab::from_key).unwrap_or(Tab::General),
            frame: prefs.get(FRAME).filter(|f| !f.trim().is_empty()).map(str::to_owned),
            toolbar_items: prefs
                .get(TOOLBAR_ITEMS)
                .map(|s| s.split(',').map(str::trim).filter(|i| !i.is_empty()).map(str::to_owned).collect()),
            toolbar_mode: prefs.get(TOOLBAR_MODE).and_then(|m| m.trim().parse().ok()),
        }
    }

    /// Writes the layout; sizes and the frame stay as they were when this has none (a collapsed
    /// sidebar keeps its last width).
    pub fn store(&self, prefs: &mut Prefs) {
        prefs.set_bool(TOOLBAR, self.toolbar);
        prefs.set_bool(SIDEBAR, self.sidebar);
        prefs.set_bool(DETAILS, self.details);
        prefs.set_bool(STATUS, self.status);
        if let Some(w) = self.sidebar_width.filter(|w| *w >= 1.0) {
            prefs.set(SIDEBAR_WIDTH, w.round() as i32);
        }
        if let Some(h) = self.detail_height.filter(|h| *h >= 1.0) {
            prefs.set(DETAIL_HEIGHT, h.round() as i32);
        }
        prefs.set(TAB, self.tab.key());
        if let Some(f) = &self.frame {
            prefs.set(FRAME, f);
        }
        match &self.toolbar_items {
            Some(items) => prefs.set(TOOLBAR_ITEMS, items.join(",")),
            None => prefs.remove(TOOLBAR_ITEMS),
        }
        if let Some(m) = self.toolbar_mode {
            prefs.set(TOOLBAR_MODE, m);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_without_keys() {
        assert_eq!(Layout::load(&Prefs::default()), Layout::default());
    }

    #[test]
    fn round_trip_with_the_win32_keys() {
        let l = Layout {
            toolbar: false,
            sidebar: false,
            details: true,
            status: false,
            sidebar_width: Some(181.4),
            detail_height: Some(230.0),
            tab: Tab::Files,
            frame: Some("100 120 1000 640 0 0 1440 875 ".into()),
            toolbar_items: Some(vec![
                "rp.393216".into(),
                "NSToolbarFlexibleSpaceItem".into(),
                "rp.search".into(),
            ]),
            toolbar_mode: Some(2),
        };
        let mut p = Prefs::default();
        l.store(&mut p);
        assert_eq!(p.get("ui.toolbar"), Some("0"));
        assert_eq!(p.get("ui.tree"), Some("0"));
        assert_eq!(p.get("ui.details"), Some("1"));
        assert_eq!(p.get("ui.status"), Some("0"));
        assert_eq!(p.get("ui.tree_width"), Some("181"));
        assert_eq!(p.get("ui.detail_height"), Some("230"));
        assert_eq!(p.get("ui.tab"), Some("Files"));
        assert_eq!(p.get("ui.mac.toolbar"), Some("rp.393216,NSToolbarFlexibleSpaceItem,rp.search"));
        let back = Layout::load(&p);
        assert_eq!(back, Layout { sidebar_width: Some(181.0), ..l });
    }

    #[test]
    fn missing_sizes_keep_the_old_ones() {
        let mut p = Prefs::default();
        p.set("ui.tree_width", 200);
        p.set("ui.mac.toolbar", "a,b");
        Layout { sidebar_width: None, toolbar_items: None, ..Layout::default() }.store(&mut p);
        assert_eq!(p.get("ui.tree_width"), Some("200"));
        // the default toolbar again
        assert_eq!(p.get("ui.mac.toolbar"), None);
        p.set("ui.detail_height", "x");
        p.set("ui.tab", "Nope");
        let l = Layout::load(&p);
        assert_eq!((l.detail_height, l.tab), (None, Tab::General));
    }
}
