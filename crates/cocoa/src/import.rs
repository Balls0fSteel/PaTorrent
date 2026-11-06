//! File > Import from qBittorrent and µTorrent (GTK `import_from`): the client's usual folder
//! (or one picked in an open panel when it is not there), `import::begin` (refuse, pick, or ask),
//! the question with the folders the torrents would save to, then `Core::import` adds them with
//! their resume data, labels and pause marks, and its summary is shown. A remote window has no
//! import: the client's files are on this Mac, not the server (greyed, [`enabled`]).
//!
//! The open panel answers `ROOKPARCEL_TEST_PICK` and the question `ROOKPARCEL_TEST_ACCEPT`, as
//! the torrent dialogs' do (`dialogs`).

use std::path::PathBuf;

use objc2::MainThreadMarker;
use rookparcel_frontend::import::{self, Begin, Client};

use crate::app::{apply, peek, with_core};
use crate::dialogs;
use crate::ui;

/// Whether the import items are enabled: a local window's (not before the start).
pub fn enabled() -> bool {
    peek(|c| !c.is_remote()).unwrap_or(false)
}

/// File > Import from qBittorrent (`true`) or µTorrent.
pub fn import(qbittorrent: bool) {
    let client = if qbittorrent { Client::QBittorrent } else { Client::UTorrent };
    import_from(client, None);
}

fn import_from(client: Client, picked: Option<PathBuf>) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    if !enabled() {
        return;
    }
    let window = dialogs::main_window();
    match import::begin(client, picked.as_deref()) {
        Begin::Refuse(message) => dialogs::tell(None, ui::Style::Info, &message, "", mtm),
        Begin::PickFolder(title) => {
            let pick = ui::Pick { folders: true, message: title, ..ui::Pick::default() };
            dialogs::open_panel(
                window.as_deref(),
                pick,
                move |paths| {
                    if let Some(dir) = paths.into_iter().next() {
                        import_from(client, Some(dir));
                    }
                },
                mtm,
            );
        }
        Begin::Confirm(question, found) => {
            let (message, detail) = question.split_once("\n\n").unwrap_or((&question, ""));
            crate::profiles::confirm(window.as_deref(), ui::Style::Info, message, detail, move |ok| {
                if !ok {
                    return;
                }
                let summary = with_core(|c| c.import(client, found));
                // the Logger's lines and the torrents being added
                apply(with_core(|c| c.events()));
                if let Some(summary) = summary {
                    // once the question's sheet has gone
                    crate::mainq::on_main(move || {
                        if let Some(mtm) = MainThreadMarker::new() {
                            dialogs::tell(None, ui::Style::Info, &summary, "", mtm);
                        }
                    });
                }
            });
        }
    }
}
