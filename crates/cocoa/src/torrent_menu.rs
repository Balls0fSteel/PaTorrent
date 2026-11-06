//! The Torrent menu's and the list's context menu's commands on the selected torrents (GTK
//! `torrent_action`, `remove`, `open_folder`, `set_download_location`, `set_label`, `set_limit`,
//! `toggle_super_seeding`, `menu_state`), and Start All / Stop All of the Dock and status item.
//! The logic is `Core`'s; this asks the questions (Remove's sheet, the folder panel, New Label)
//! and gives the menus their check marks and greying ([`state`]).

use std::path::{Path, PathBuf};

use objc2::MainThreadMarker;
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSArray, NSString, NSURL};
use rookparcel_core::TorrentId;
use rookparcel_frontend::actions::LIMITS;
use rookparcel_frontend::app::TorrentAction;
use rookparcel_frontend::tr;

use crate::app::{apply, labels, peek, selected, ui, with_core};
use crate::commands::Command;
use crate::ui::{self as mui, Pick, Style};

/// Runs a torrent command on the selection (Start All and Stop All on every torrent).
pub fn run(cmd: Command) {
    use Command::*;
    let ids = selected();
    let action = |a: TorrentAction, ids: &[TorrentId]| {
        apply(with_core(|c| {
            c.torrent_action(a, ids);
            c.events()
        }))
    };
    match cmd {
        StartAll | StopAll => {
            let all = peek(|c| c.ids()).unwrap_or_default();
            action(if cmd == StartAll { TorrentAction::Start } else { TorrentAction::Stop }, &all);
        }
        _ if ids.is_empty() => {}
        ForceStart => action(TorrentAction::ForceStart, &ids),
        Start => action(TorrentAction::Start, &ids),
        Pause => action(TorrentAction::Pause, &ids),
        Stop => action(TorrentAction::Stop, &ids),
        QueueUp => action(TorrentAction::QueueUp, &ids),
        QueueDown => action(TorrentAction::QueueDown, &ids),
        QueueTop => action(TorrentAction::QueueTop, &ids),
        QueueBottom => action(TorrentAction::QueueBottom, &ids),
        Recheck => action(TorrentAction::Recheck, &ids),
        UpdateTracker => action(TorrentAction::Reannounce, &ids),
        OpenFolder => open_folder(&ids),
        Remove => remove(ids, false),
        RemoveAndDelete => remove(ids, true),
        SetLocation => set_location(ids),
        InitialSeeding => apply(with_core(|c| {
            c.toggle_super_seeding(&ids);
            c.events()
        })),
        DownLimit(i) | UpLimit(i) => {
            let Some(&kib) = LIMITS.get(usize::from(i)) else { return };
            let download = matches!(cmd, DownLimit(_));
            apply(with_core(|c| {
                c.set_limit(&ids, download, kib.saturating_mul(1024));
                c.events()
            }));
        }
        NewLabel => {
            crate::dialogs::input::ask(
                &mui::text(tr!(NewLabelTitle)),
                &mui::text(tr!(NewLabelPrompt)),
                "",
                move |l| {
                    let l = l.trim().to_owned();
                    if !l.is_empty() {
                        set_label(&ids, &l);
                    }
                },
            );
        }
        RemoveLabel => set_label(&ids, ""),
        Label(i) => {
            if let Some(l) = labels().get(usize::from(i)) {
                set_label(&ids, l);
            }
        }
        _ => {}
    }
}

/// Whether a command needing a selection is enabled now, and its check mark (Label ▸,
/// Bandwidth ▸, Initial Seeding), from `Core::menu_state`.
pub fn state(cmd: Command) -> (bool, Option<bool>) {
    use Command::*;
    let ids = selected();
    let Some(m) = peek(|c| c.menu_state(&ids)).flatten() else { return (false, None) };
    let limit = |i: u16, kib: u32| Some(LIMITS.get(usize::from(i)) == Some(&kib));
    match cmd {
        OpenFolder => (m.can_open_folder, None),
        SetLocation => (m.can_set_location, None),
        InitialSeeding => (m.can_super_seed, Some(m.super_seeding)),
        DownLimit(i) => (true, limit(i, m.download_kib)),
        UpLimit(i) => (true, limit(i, m.upload_kib)),
        RemoveLabel => (m.can_remove_label, None),
        Label(i) => (
            true,
            Some(m.label.is_some() && m.label.as_deref() == m.labels.get(usize::from(i)).map(String::as_str)),
        ),
        _ => (true, None),
    }
}

/// Open Containing Folder's text: one finished torrent's own folder, or the torrents'.
pub fn open_folder_title() -> &'static str {
    let ids = selected();
    match peek(|c| c.menu_state(&ids)).flatten() {
        Some(m) if m.single_done => tr!(MenuOpenFolder),
        _ => tr!(MenuOpenFolderMany),
    }
}

fn set_label(ids: &[TorrentId], label: &str) {
    apply(with_core(|c| {
        c.set_label(ids, label);
        c.events()
    }));
}

/// Shows the first torrent's file or folder in the Finder (or, before it exists, the nearest
/// folder that does). Nothing in a remote window: the files are on the server.
pub fn open_folder(ids: &[TorrentId]) {
    let Some(path) = peek(|c| c.folder_of(ids)).flatten() else { return };
    reveal(&path);
}

/// Shows `path` selected in the Finder, or opens its nearest existing folder.
pub fn reveal(path: &Path) {
    let ws = NSWorkspace::sharedWorkspace();
    let url = |p: &Path| NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()));
    if path.exists() {
        ws.activateFileViewerSelectingURLs(&NSArray::from_retained_slice(&[url(path)]));
        return;
    }
    if let Some(dir) = path.ancestors().skip(1).find(|p| p.is_dir()) {
        ws.openURL(&url(dir));
    }
}

/// Remove and Remove And Delete: Preferences' question as a sheet (`Core::remove_question`),
/// then `Core::remove`.
fn remove(ids: Vec<TorrentId>, with_data: bool) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let question = peek(|c| c.remove_question(&ids, with_data)).flatten();
    let go = move |ids: &[TorrentId]| {
        apply(with_core(|c| {
            c.remove(ids, with_data);
            c.events()
        }))
    };
    let Some((message, detail)) = question else {
        go(&ids);
        return;
    };
    let window = ui().map(|u| u.window.clone());
    mui::sheet(
        window.as_deref(),
        if with_data { Style::Warning } else { Style::Info },
        &message,
        &detail,
        &[tr!(BtnRemove), tr!(BtnCancel)],
        move |b| {
            if b == 0 {
                go(&ids);
            }
        },
        mtm,
    );
}

/// Advanced > Set Download Location: a folder from the open panel (a remote window's is typed:
/// a folder on the server), then the files move there.
fn set_location(ids: Vec<TorrentId>) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let Some((start, remote)) = peek(|c| (c.download_location_start(&ids), c.is_remote())) else { return };
    let start = start.unwrap_or_default();
    let move_to = move |dir: String| {
        apply(with_core(|c| {
            c.set_download_location(&ids, &dir);
            c.events()
        }));
        crate::detail::refresh();
    };
    if remote {
        let text = tr!(WebSetLocation);
        let (title, prompt) = text.split_once("\n\n").unwrap_or((text, text));
        crate::dialogs::input::ask(&mui::text(title), &mui::text(prompt), &start, move_to);
        return;
    }
    let pick = Pick {
        folders: true,
        start: Some(PathBuf::from(&start)).filter(|p| p.is_dir()),
        message: mui::text(tr!(PickLocation)),
        ..Pick::default()
    };
    let window = ui().map(|u| u.window.clone());
    mui::open_panel(
        window.as_deref(),
        pick,
        move |paths| {
            if let Some(p) = paths.first() {
                move_to(p.to_string_lossy().into_owned());
            }
        },
        mtm,
    );
}
