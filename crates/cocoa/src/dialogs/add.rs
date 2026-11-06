//! Add New Torrent.
//!
//! Slice F fills this file (docs/MACOS_GUI.md); until then each entry point logs "not yet".

/// Not yet built (slice F).
pub fn open_files() {
    crate::app::not_yet("Add Torrent");
}

/// What an add (`Core::add_file`, `add_magnet`, `add_url`, `add_link`) asks of the GUI: the Add
/// New Torrent dialog for `AddStep::Dialog`, a message for `AlreadyInList` and `Error`, nothing
/// for the others. Slice C calls it for dropped files, slice G for opened files and links.
/// Not yet built (slice F).
pub fn handle(step: rookparcel_frontend::app::AddStep) {
    let _ = step;
    crate::app::not_yet("Add New Torrent for an added file or link");
}

/// `Events::magnet_ready` (open `Core::next_magnet_dialog`, one at a time) and `Events::adds`
/// (dialogs for downloaded `.torrent` files), from `app::apply`.
/// Not yet built (slice F).
pub fn from_events(magnet_ready: bool, adds: Vec<rookparcel_frontend::app::AddRequest>) {
    let _ = (magnet_ready, adds);
    crate::app::not_yet("the Add New Torrent dialog for a magnet or a downloaded .torrent");
}
