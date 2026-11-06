//! SF Symbols names for the toolbar, the menus (macOS 26 and later only, where the system's own
//! menus carry symbols), the detail tabs and the Preferences pages (D61). Only names from the SF
//! Symbols 2 set, which macOS 11 has (convention 5); `images::symbol` falls back to the D27 PNGs
//! when one is missing all the same.
//!
//! Pure Rust, compiled and tested on every system.

use rookparcel_frontend::detail::Tab;
use rookparcel_frontend::lang::S;

use crate::commands::Command;

/// The symbol of a command's toolbar item or menu item.
pub fn command(cmd: Command) -> Option<&'static str> {
    use Command::*;
    Some(match cmd {
        AddTorrent => "plus",
        AddUrl => "link",
        CreateTorrent => "square.and.pencil",
        Remove => "xmark",
        RemoveAndDelete => "trash",
        Start | ForceStart | StartAll => "play.fill",
        Pause => "pause.fill",
        Stop | StopAll => "stop.fill",
        QueueUp => "arrow.up",
        QueueDown => "arrow.down",
        QueueTop => "arrow.up.to.line",
        QueueBottom => "arrow.down.to.line",
        Rss => "dot.radiowaves.up.forward",
        Preferences => "gearshape",
        OpenFolder | ProfileFolder => "folder",
        Recheck => "arrow.clockwise",
        UpdateTracker => "antenna.radiowaves.left.and.right",
        Properties => "info.circle",
        SetLocation => "folder.badge.gearshape",
        NewLabel => "tag",
        RemoveLabel => "tag.slash",
        Find => "magnifyingglass",
        Website => "globe",
        Notices => "doc.text",
        CheckUpdates => "arrow.down.circle",
        ImportQbt | ImportUt => "square.and.arrow.down",
        ConnectRemote => "network",
        NewProfile => "person.badge.plus",
        About => "info.circle",
        _ => return None,
    })
}

/// A detail tab's symbol.
pub fn tab(t: Tab) -> &'static str {
    match t {
        Tab::General => "info.circle",
        Tab::Trackers => "antenna.radiowaves.left.and.right",
        Tab::Peers => "person.2",
        Tab::Pieces => "square.grid.3x3",
        Tab::Files => "doc.on.doc",
        Tab::Speed => "speedometer",
        Tab::Logger => "list.bullet.rectangle",
    }
}

/// A Preferences page's symbol, by its title.
pub fn page(title: S) -> &'static str {
    match title {
        S::PageGeneral => "gearshape",
        S::PageUi => "paintbrush",
        S::PageDirectories => "folder",
        S::PageConnection => "network",
        S::PageBandwidth => "speedometer",
        S::PageBitTorrent => "arrow.up.arrow.down",
        S::PageQueueing => "list.number",
        S::PageScheduler => "calendar",
        S::PageWebUi => "globe",
        S::PageAdvanced => "slider.horizontal.3",
        S::PageDiskCache | S::PageDiskIo => "internaldrive",
        _ => "gearshape.2",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The names are SF Symbols 2's: a typo would only show at run time, as the PNG fallback.
    #[test]
    fn names_look_like_symbols() {
        let names = crate::commands::ALL
            .iter()
            .filter_map(|&c| command(c))
            .chain(Tab::ALL.iter().map(|&t| tab(t)))
            .chain([page(S::PageGeneral), page(S::PageAdvanced)]);
        for n in names {
            assert!(
                !n.is_empty() && n.chars().all(|c| c.is_ascii_lowercase() || c == '.' || c.is_ascii_digit()),
                "{n}"
            );
        }
    }
}
