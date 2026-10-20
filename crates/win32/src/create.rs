//! File → Create New Torrent: µTorrent 2.2.1's dialog. The files are listed and hashed on a
//! worker thread that posts its progress back; Cancel stops it through the progress callback.

use std::mem::{size_of, zeroed};
use std::path::{Path, PathBuf};
use std::ptr::null;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use patorrent_frontend::create::{self, CreateOptions, Created, FORMATS, PIECE_SIZES, TorrentFormat};
use patorrent_frontend::{lang, tr};
use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree, CoUninitialize,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::Dialogs::{
    GetOpenFileNameW, GetSaveFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_HIDEREADONLY,
    OFN_OVERWRITEPROMPT, OFN_PATHMUSTEXIST, OPENFILENAMEW,
};
use windows_sys::Win32::UI::Controls::{BST_CHECKED, BST_UNCHECKED, PBM_SETPOS, PBM_SETRANGE32, PBS_SMOOTH};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::Shell::{
    BFFM_INITIALIZED, BFFM_SETSELECTIONW, BIF_NEWDIALOGSTYLE, BIF_RETURNONLYFSDIRS, BROWSEINFOW,
    SHBrowseForFolderW, SHGetPathFromIDListW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_GROUPBOX, BS_PUSHBUTTON, CB_ADDSTRING,
    CB_GETCURSEL, CB_SETCURSEL, CBN_SELCHANGE, CBS_DROPDOWNLIST, DLGTEMPLATE, DialogBoxIndirectParamW,
    ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE, ES_WANTRETURN, EndDialog, GetDlgItem, GetWindowLongPtrW,
    IDCANCEL, IDOK, PostMessageW, SetWindowLongPtrW, SetWindowTextW, WM_APP, WM_CLOSE, WM_COMMAND,
    WM_INITDIALOG, WS_BORDER, WS_CHILD, WS_TABSTOP, WS_VISIBLE, WS_VSCROLL,
};

use crate::dialogs::{self, BUTTON, COMBOBOX, Class, DWLP_USER, EDIT, STATIC, Template};
use crate::util::{hiword, loword, send, wide, window_text};

/// `wParam` = pieces done, `lParam` = pieces in all.
const WM_APP_PROGRESS: u32 = WM_APP + 20;
/// `lParam` = `Box<Result<Created, String>>`.
const WM_APP_DONE: u32 = WM_APP + 21;

/// Static control style (in `System::SystemServices`, a feature we do not otherwise need).
const SS_RIGHT: u32 = 2;

const ID_SOURCE: u16 = 100;
const ID_ADD_FILE: u16 = 101;
const ID_ADD_DIR: u16 = 102;
const ID_SKIP: u16 = 103;
const ID_TRACKERS: u16 = 104;
const ID_WEB_SEEDS: u16 = 105;
const ID_COMMENT: u16 = 106;
const ID_PIECE: u16 = 107;
const ID_SEED: u16 = 108;
const ID_PRIVATE: u16 = 109;
const ID_ORDER: u16 = 110;
const ID_PROGRESS: u16 = 111;
const ID_PERCENT: u16 = 112;
const ID_FORMAT: u16 = 113;

/// Every input, disabled while hashing.
const INPUTS: [u16; 13] = [
    ID_SOURCE,
    ID_ADD_FILE,
    ID_ADD_DIR,
    ID_SKIP,
    ID_TRACKERS,
    ID_WEB_SEEDS,
    ID_COMMENT,
    ID_PIECE,
    ID_FORMAT,
    ID_SEED,
    ID_PRIVATE,
    ID_ORDER,
    IDOK as u16,
];

/// What the dialog remembers between uses (kept in the preferences by the caller).
#[derive(Clone, Debug, Default)]
pub struct Remembered {
    /// The Trackers box text.
    pub trackers: String,
    /// Index into [`PIECE_SIZES`].
    pub piece: usize,
    pub seed: bool,
    pub private: bool,
    pub preserve_order: bool,
    /// The BitTorrent version (v1, hybrid, v2).
    pub format: TorrentFormat,
}

/// A torrent written to disk.
pub struct Done {
    pub created: Created,
    /// The save path to seed it from, when "Start seeding" was on.
    pub seed_from: Option<PathBuf>,
}

struct State {
    remembered: Remembered,
    cancel: Arc<AtomicBool>,
    busy: bool,
    /// Closed while hashing: end once the worker has stopped.
    closing: bool,
    file: PathBuf,
    seed_from: Option<PathBuf>,
    done: Option<Done>,
}

/// Shows the dialog; returns the torrent it made, if any. `remembered` is updated from the
/// fields when a torrent is created.
pub fn show(owner: HWND, remembered: &mut Remembered) -> Option<Done> {
    let mut st = State {
        remembered: remembered.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        busy: false,
        closing: false,
        file: PathBuf::new(),
        seed_from: None,
        done: None,
    };
    let template = template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(proc_),
            &mut st as *mut State as LPARAM,
        );
    }
    if st.done.is_some() {
        *remembered = st.remembered;
    }
    st.done
}

fn template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    const TAB: u32 = C | WS_TABSTOP;
    const LINE: u32 = TAB | WS_BORDER | ES_AUTOHSCROLL as u32;
    const BOX: u32 = TAB | WS_BORDER | WS_VSCROLL | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32;
    const PUSH: u32 = TAB | BS_PUSHBUTTON as u32;
    const CHECK: u32 = TAB | BS_AUTOCHECKBOX as u32;
    const GROUP: u32 = C | BS_GROUPBOX as u32;
    const LABEL: u32 = C | SS_RIGHT;
    let mut t = Template::new(300, 318);
    t.item(BUTTON, 0xffff, GROUP, [7, 4, 286, 50], tr!(CreateSource));
    t.item(EDIT, ID_SOURCE, LINE, [14, 17, 272, 13], "");
    t.item(BUTTON, ID_ADD_FILE, PUSH, [126, 35, 78, 14], tr!(CreateAddFile));
    t.item(BUTTON, ID_ADD_DIR, PUSH, [208, 35, 78, 14], tr!(CreateAddDir));
    t.item(BUTTON, 0xffff, GROUP, [7, 58, 286, 32], tr!(CreateSkip));
    t.item(EDIT, ID_SKIP, LINE, [14, 71, 272, 13], "");
    t.item(BUTTON, 0xffff, GROUP, [7, 94, 286, 136], tr!(CreateProps));
    t.item(STATIC, 0xffff, LABEL, [12, 108, 48, 9], tr!(CreateTrackers));
    t.item(EDIT, ID_TRACKERS, BOX, [64, 106, 222, 50], "");
    t.item(STATIC, 0xffff, LABEL, [12, 163, 48, 9], tr!(CreateWebSeeds));
    t.item(EDIT, ID_WEB_SEEDS, BOX, [64, 161, 222, 26], "");
    t.item(STATIC, 0xffff, LABEL, [12, 194, 48, 9], tr!(CreateComment));
    t.item(EDIT, ID_COMMENT, LINE, [64, 192, 222, 13], "");
    t.item(STATIC, 0xffff, LABEL, [12, 212, 48, 9], tr!(CreatePieceSize));
    t.item(COMBOBOX, ID_PIECE, TAB | WS_VSCROLL | CBS_DROPDOWNLIST as u32, [64, 210, 90, 160], "");
    // BEP 52 (not in µTorrent 2.2.1): v1, hybrid or v2
    t.item(STATIC, 0xffff, LABEL, [158, 212, 40, 9], tr!(CreateVersion));
    t.item(COMBOBOX, ID_FORMAT, TAB | WS_VSCROLL | CBS_DROPDOWNLIST as u32, [202, 210, 84, 60], "");
    // two rows, so long translations fit
    t.item(BUTTON, 0xffff, GROUP, [7, 234, 286, 40], tr!(CreateOther));
    t.item(BUTTON, ID_SEED, CHECK, [14, 246, 130, 10], tr!(CreateSeed));
    t.item(BUTTON, ID_PRIVATE, CHECK, [150, 246, 136, 10], tr!(CreatePrivate));
    t.item(BUTTON, ID_ORDER, CHECK, [14, 259, 272, 10], tr!(CreatePreserveOrder));
    // the progress on its own row, so the buttons can take a long translation
    t.item(Class::Name("msctls_progress32"), ID_PROGRESS, C | PBS_SMOOTH, [7, 280, 248, 10], "");
    t.item(STATIC, ID_PERCENT, C, [259, 280, 34, 9], "");
    t.item(BUTTON, IDOK as u16, TAB | BS_DEFPUSHBUTTON as u32, [139, 297, 106, 14], tr!(CreateSave));
    t.item(BUTTON, IDCANCEL as u16, PUSH, [249, 297, 44, 14], tr!(BtnClose));
    t.finish()
}

unsafe fn item(dlg: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(dlg, id as i32) }
}

unsafe fn set_text(hwnd: HWND, s: &str) {
    let w = wide(s);
    unsafe { SetWindowTextW(hwnd, w.as_ptr()) };
}

unsafe fn checked(dlg: HWND, id: u16) -> bool {
    unsafe { send(item(dlg, id), BM_GETCHECK, 0, 0) == BST_CHECKED as isize }
}

unsafe fn set_check(dlg: HWND, id: u16, on: bool) {
    unsafe { send(item(dlg, id), BM_SETCHECK, if on { BST_CHECKED } else { BST_UNCHECKED } as usize, 0) };
}

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            init(dlg, &(*(lp as *const State)).remembered);
            crate::fit::fit_texts(dlg, |_| Some(0));
            return 1;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut State;
        if st.is_null() {
            return 0;
        }
        let st = &mut *st;
        match msg {
            WM_COMMAND => {
                match loword(wp) {
                    ID_ADD_FILE => {
                        if let Some(p) = pick_file(dlg) {
                            set_text(item(dlg, ID_SOURCE), &p);
                        }
                    }
                    ID_ADD_DIR => {
                        if let Some(p) = pick_folder(dlg, tr!(CreatePickDir)) {
                            set_text(item(dlg, ID_SOURCE), &p);
                        }
                    }
                    ID_FORMAT if hiword(wp) == CBN_SELCHANGE as u16 => sync_order(dlg),
                    id if id == IDOK as u16 && !st.busy => start(dlg, st),
                    id if id == IDCANCEL as u16 => close(dlg, st),
                    _ => return 0,
                }
                1
            }
            WM_CLOSE => {
                close(dlg, st);
                1
            }
            WM_APP_PROGRESS => {
                let (done, total) = (wp as u32, (lp as u32).max(1));
                let bar = item(dlg, ID_PROGRESS);
                send(bar, PBM_SETRANGE32, 0, total as isize);
                send(bar, PBM_SETPOS, done as usize, 0);
                set_text(item(dlg, ID_PERCENT), &format!("{:.1} %", done as f64 * 100.0 / total as f64));
                1
            }
            WM_APP_DONE => {
                let result = *Box::from_raw(lp as *mut Result<Created, String>);
                finished(dlg, st, result);
                1
            }
            _ => 0,
        }
    }
}

unsafe fn init(dlg: HWND, r: &Remembered) {
    unsafe {
        set_text(dlg, tr!(CreateTitle));
        set_text(item(dlg, ID_TRACKERS), &r.trackers);
        let combo = item(dlg, ID_PIECE);
        for bytes in PIECE_SIZES {
            let w = wide(&create::piece_size_text(bytes));
            send(combo, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        send(combo, CB_SETCURSEL, r.piece.min(PIECE_SIZES.len() - 1), 0);
        let formats = item(dlg, ID_FORMAT);
        for (label, _) in FORMATS {
            let w = wide(lang::text(label));
            send(formats, CB_ADDSTRING, 0, w.as_ptr() as LPARAM);
        }
        send(formats, CB_SETCURSEL, r.format.index(), 0);
        set_check(dlg, ID_SEED, r.seed);
        set_check(dlg, ID_PRIVATE, r.private);
        set_check(dlg, ID_ORDER, r.preserve_order);
        sync_order(dlg);
    }
}

/// The chosen version.
unsafe fn format(dlg: HWND) -> TorrentFormat {
    unsafe {
        let i = send(item(dlg, ID_FORMAT), CB_GETCURSEL, 0, 0).max(0) as usize;
        FORMATS[i.min(FORMATS.len() - 1)].1
    }
}

/// "Preserve file order" applies to v1 only (v2 lays the files out sorted).
unsafe fn sync_order(dlg: HWND) {
    unsafe {
        EnableWindow(item(dlg, ID_ORDER), format(dlg).allows_any_order() as i32);
        // a remote server has not got the files: the torrent cannot be seeded from here
        if crate::remote_ui::is_remote() {
            set_check(dlg, ID_SEED, false);
            EnableWindow(item(dlg, ID_SEED), 0);
        }
    }
}

/// "Create and save as...": checks the source, asks where to save, then hashes.
unsafe fn start(dlg: HWND, st: &mut State) {
    unsafe {
        let Some((opts, seed_from)) = collect(dlg, st) else { return };
        let Some(file) = save_as(dlg, &opts.source) else { return };
        hash(dlg, st, opts, seed_from, file);
    }
}

/// Reads and checks the fields (and remembers them); `None` after telling the user what is wrong.
unsafe fn collect(dlg: HWND, st: &mut State) -> Option<(CreateOptions, Option<PathBuf>)> {
    unsafe {
        let source = PathBuf::from(window_text(item(dlg, ID_SOURCE)).trim().trim_matches('"'));
        if source.as_os_str().is_empty() {
            dialogs::error(dlg, tr!(CreateNoSource));
            SetFocus(item(dlg, ID_SOURCE));
            return None;
        }
        if let Err(e) = std::fs::metadata(&source) {
            dialogs::error(dlg, &format!("{}:\n{e}", source.display()));
            SetFocus(item(dlg, ID_SOURCE));
            return None;
        }
        let piece =
            (send(item(dlg, ID_PIECE), CB_GETCURSEL, 0, 0).max(0) as usize).min(PIECE_SIZES.len() - 1);
        let opts = CreateOptions {
            source: source.clone(),
            skip: window_text(item(dlg, ID_SKIP)),
            trackers: window_text(item(dlg, ID_TRACKERS)),
            web_seeds: window_text(item(dlg, ID_WEB_SEEDS)),
            comment: window_text(item(dlg, ID_COMMENT)),
            piece_size: PIECE_SIZES[piece],
            private: checked(dlg, ID_PRIVATE),
            preserve_order: checked(dlg, ID_ORDER),
            format: format(dlg),
        };
        let seed = checked(dlg, ID_SEED);
        st.remembered = Remembered {
            trackers: opts.trackers.clone(),
            piece,
            seed,
            private: opts.private,
            preserve_order: opts.preserve_order,
            format: opts.format,
        };
        let seed_from = if seed {
            match create::seed_save_path(&source) {
                Ok(p) => Some(p),
                Err(e) => {
                    dialogs::error(dlg, &format!("{}:\n{e}", source.display()));
                    return None;
                }
            }
        } else {
            None
        };
        Some((opts, seed_from))
    }
}

/// Hashes on a worker thread and writes `file`; ends in `WM_APP_DONE`.
unsafe fn hash(dlg: HWND, st: &mut State, opts: CreateOptions, seed_from: Option<PathBuf>, file: PathBuf) {
    unsafe {
        st.file = file.clone();
        st.seed_from = seed_from;
        st.cancel.store(false, Ordering::Relaxed);
        set_busy(dlg, st, true);
        let cancel = st.cancel.clone();
        let target = dlg as isize;
        std::thread::spawn(move || {
            let mut shown = u32::MAX;
            let result = create::build(&opts, &mut |done, total| {
                // about 1000 updates at most
                let step = done as u64 * 1000 / total.max(1) as u64;
                if step as u32 != shown {
                    shown = step as u32;
                    PostMessageW(target as HWND, WM_APP_PROGRESS, done as usize, total as isize);
                }
                !cancel.load(Ordering::Relaxed)
            })
            .and_then(|c| match std::fs::write(&file, &c.bytes) {
                Ok(()) => Ok(c),
                Err(e) => Err(format!("{}: {e}", file.display())),
            });
            let boxed = Box::into_raw(Box::new(result));
            if PostMessageW(target as HWND, WM_APP_DONE, 0, boxed as LPARAM) == 0 {
                drop(Box::from_raw(boxed));
            }
        });
    }
}

unsafe fn set_busy(dlg: HWND, st: &mut State, busy: bool) {
    unsafe {
        st.busy = busy;
        for id in INPUTS {
            EnableWindow(item(dlg, id), (!busy) as i32);
        }
        if !busy {
            sync_order(dlg);
        }
        set_text(item(dlg, IDCANCEL as u16), if busy { tr!(BtnCancel) } else { tr!(BtnClose) });
        if busy {
            send(item(dlg, ID_PROGRESS), PBM_SETPOS, 0, 0);
            set_text(item(dlg, ID_PERCENT), "0.0 %");
            SetFocus(item(dlg, IDCANCEL as u16));
        }
    }
}

/// Cancel, Esc or the close box: stops hashing first, then closes.
unsafe fn close(dlg: HWND, st: &mut State) {
    unsafe {
        if st.busy {
            st.cancel.store(true, Ordering::Relaxed);
            st.closing = true;
        } else {
            EndDialog(dlg, IDCANCEL as isize);
        }
    }
}

unsafe fn finished(dlg: HWND, st: &mut State, result: Result<Created, String>) {
    unsafe {
        set_busy(dlg, st, false);
        match result {
            Ok(created) => {
                st.done = Some(Done { created, seed_from: st.seed_from.take() });
                EndDialog(dlg, IDOK as isize);
            }
            Err(e) => {
                if st.closing {
                    EndDialog(dlg, IDCANCEL as isize);
                    return;
                }
                send(item(dlg, ID_PROGRESS), PBM_SETPOS, 0, 0);
                set_text(item(dlg, ID_PERCENT), "");
                if e != "cancelled" {
                    dialogs::error(dlg, &tr!(CreateFailed, error = e));
                }
                SetFocus(item(dlg, IDOK as u16));
            }
        }
    }
}

/// "Add file".
pub(crate) unsafe fn pick_file(owner: HWND) -> Option<String> {
    let filter = wide(&dialogs::all_files_filter());
    let title = wide(tr!(PickFile));
    let mut buf = vec![0u16; 32 * 1024];
    unsafe {
        let mut ofn: OPENFILENAMEW = zeroed();
        ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrTitle = title.as_ptr();
        ofn.Flags = OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_HIDEREADONLY;
        if GetOpenFileNameW(&mut ofn) == 0 {
            return None;
        }
    }
    Some(crate::util::from_wide(&buf))
}

/// "Add directory": the shell's folder browser.
pub(crate) unsafe fn pick_folder(owner: HWND, title: &str) -> Option<String> {
    unsafe { pick_folder_at(owner, title, None) }
}

/// The shell's folder browser, opened on `start` when it is given.
pub(crate) unsafe fn pick_folder_at(owner: HWND, title: &str, start: Option<&str>) -> Option<String> {
    unsafe extern "system" fn select_start(hwnd: HWND, msg: u32, _: LPARAM, start: LPARAM) -> i32 {
        if msg == BFFM_INITIALIZED && start != 0 {
            unsafe { send(hwnd, BFFM_SETSELECTIONW, 1, start) };
        }
        0
    }
    let title = wide(title);
    let start = start.filter(|s| !s.trim().is_empty()).map(wide);
    unsafe {
        // BIF_NEWDIALOGSTYLE needs an apartment on this thread
        let com = CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32) >= 0;
        let mut bi: BROWSEINFOW = zeroed();
        bi.hwndOwner = owner;
        bi.lpszTitle = title.as_ptr();
        bi.ulFlags = BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE;
        if let Some(start) = &start {
            bi.lpfn = Some(select_start);
            bi.lParam = start.as_ptr() as LPARAM;
        }
        let pidl = SHBrowseForFolderW(&bi);
        let mut out = None;
        if !pidl.is_null() {
            let mut buf = vec![0u16; 32 * 1024];
            if SHGetPathFromIDListW(pidl, buf.as_mut_ptr()) != 0 {
                out = Some(crate::util::from_wide(&buf));
            }
            CoTaskMemFree(pidl as *const _);
        }
        if com {
            CoUninitialize();
        }
        out.filter(|s| !s.is_empty())
    }
}

/// The save dialog, starting next to the source with `<name>.torrent`.
unsafe fn save_as(owner: HWND, source: &Path) -> Option<PathBuf> {
    let filter = wide(&dialogs::torrent_filter());
    let title = wide(tr!(CreatePickSave));
    let ext = wide("torrent");
    let dir = create::seed_save_path(source).ok().map(|d| wide(&d.to_string_lossy()));
    let mut buf = vec![0u16; 32 * 1024];
    for (i, c) in create::default_file_name(source).encode_utf16().take(buf.len() - 1).enumerate() {
        buf[i] = c;
    }
    unsafe {
        let mut ofn: OPENFILENAMEW = zeroed();
        ofn.lStructSize = size_of::<OPENFILENAMEW>() as u32;
        ofn.hwndOwner = owner;
        ofn.lpstrFilter = filter.as_ptr();
        ofn.lpstrFile = buf.as_mut_ptr();
        ofn.nMaxFile = buf.len() as u32;
        ofn.lpstrTitle = title.as_ptr();
        ofn.lpstrDefExt = ext.as_ptr();
        ofn.lpstrInitialDir = dir.as_ref().map_or(null(), |d| d.as_ptr());
        ofn.Flags = OFN_EXPLORER | OFN_OVERWRITEPROMPT | OFN_PATHMUSTEXIST | OFN_HIDEREADONLY;
        if GetSaveFileNameW(&mut ofn) == 0 {
            return None;
        }
    }
    Some(PathBuf::from(crate::util::from_wide(&buf)))
}

#[cfg(test)]
mod tests {
    //! The dialog built from its template, hidden and modeless, driven through its messages
    //! (the save dialog is skipped: `hash` gets the output path directly).

    use std::time::{Duration, Instant};

    use patorrent_core::metainfo::TorrentInfo;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CB_GETCOUNT, CreateDialogIndirectParamW, DestroyWindow, DispatchMessageW, MSG, PM_REMOVE,
        PeekMessageW, SendMessageW, TranslateMessage,
    };

    use super::*;

    struct Fixture {
        dir: PathBuf,
        dlg: HWND,
        st: Box<State>,
    }

    impl Fixture {
        fn new(name: &str, remembered: Remembered) -> Fixture {
            let dir =
                std::env::temp_dir().join(format!("patorrent-win32-create-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let mut st = Box::new(State {
                remembered,
                cancel: Arc::new(AtomicBool::new(false)),
                busy: false,
                closing: false,
                file: PathBuf::new(),
                seed_from: None,
                done: None,
            });
            let template = template();
            let dlg = unsafe {
                CreateDialogIndirectParamW(
                    GetModuleHandleW(null()),
                    template.as_ptr() as *const DLGTEMPLATE,
                    std::ptr::null_mut(),
                    Some(proc_),
                    &mut *st as *mut State as LPARAM,
                )
            };
            assert!(!dlg.is_null(), "the template does not load");
            Fixture { dir, dlg, st }
        }

        /// Runs this thread's messages until `f` holds.
        fn pump(&self, secs: u64, f: impl Fn(&State) -> bool) {
            let deadline = Instant::now() + Duration::from_secs(secs);
            while !f(&self.st) {
                assert!(Instant::now() < deadline, "timed out");
                unsafe {
                    let mut msg: MSG = zeroed();
                    while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                        TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.dlg) };
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn creates_and_reports_seeding() {
        let mut f = Fixture::new("ok", Remembered { piece: 2, seed: true, ..Remembered::default() });
        let dlg = f.dlg;
        unsafe {
            for id in INPUTS.into_iter().chain([ID_PROGRESS, ID_PERCENT, IDCANCEL as u16]) {
                assert!(!item(dlg, id).is_null(), "control {id}");
            }
            assert_eq!(send(item(dlg, ID_PIECE), CB_GETCOUNT, 0, 0), PIECE_SIZES.len() as isize);
            assert_eq!(send(item(dlg, ID_PIECE), CB_GETCURSEL, 0, 0), 2);
            assert!(checked(dlg, ID_SEED) && !checked(dlg, ID_PRIVATE) && !checked(dlg, ID_ORDER));
        }
        let src = f.dir.join("Folder");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.bin"), vec![1u8; 200_000]).unwrap();
        std::fs::write(src.join("sub").join("b.bin"), vec![2u8; 30_000]).unwrap();
        std::fs::write(src.join("skip.nfo"), b"x").unwrap();
        let out = f.dir.join("out.torrent");
        unsafe {
            set_text(item(dlg, ID_SOURCE), &src.to_string_lossy());
            set_text(item(dlg, ID_SKIP), "*.nfo");
            set_text(item(dlg, ID_TRACKERS), "http://a/announce\r\n\r\nhttp://b/announce");
            set_text(item(dlg, ID_COMMENT), "made in a test");
            set_check(dlg, ID_PRIVATE, true);
            let (opts, seed_from) = collect(dlg, &mut f.st).expect("valid fields");
            hash(dlg, &mut f.st, opts, seed_from, out.clone());
            assert!(f.st.busy);
            assert_eq!(window_text(item(dlg, IDCANCEL as u16)), "Cancel");
        }
        f.pump(20, |st| st.done.is_some());
        let done = f.st.done.as_ref().unwrap();
        assert_eq!(done.seed_from.as_deref(), Some(f.dir.as_path()));
        let ti = TorrentInfo::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(ti.info_hash(), done.created.info_hash);
        assert_eq!(ti.name(), "Folder");
        // a hybrid (the default version): two files, each padded to a piece
        let fs = ti.files();
        assert_eq!((0..fs.num_files()).filter(|&i| !fs.pad_file_at(i)).count(), 2);
        assert!(ti.v1() && ti.v2());
        assert_eq!(f.st.remembered.format, TorrentFormat::Hybrid);
        assert_eq!(ti.piece_length(), 32 << 10);
        assert!(ti.is_private());
        assert_eq!(ti.comment(), "made in a test");
        assert_eq!(ti.trackers().len(), 2);
        assert_eq!(f.st.remembered.piece, 2);
        assert!(f.st.remembered.private);
        assert!(f.st.remembered.trackers.contains("http://b/announce"));
        unsafe { assert_eq!(window_text(item(dlg, ID_PERCENT)), "100.0 %") };
    }

    #[test]
    fn the_version_choice_greys_out_preserve_order_and_is_remembered() {
        let mut f = Fixture::new("format", Remembered { format: TorrentFormat::V1, ..Remembered::default() });
        let dlg = f.dlg;
        let src = f.dir.join("one.bin");
        std::fs::write(&src, vec![5u8; 50_000]).unwrap();
        unsafe {
            assert_eq!(send(item(dlg, ID_FORMAT), CB_GETCOUNT, 0, 0), FORMATS.len() as isize);
            assert_eq!(format(dlg), TorrentFormat::V1);
            assert!(IsWindowEnabled(item(dlg, ID_ORDER)) != 0, "v1 may keep the listing order");
            // pick "v2 only" as the user would
            send(item(dlg, ID_FORMAT), CB_SETCURSEL, TorrentFormat::V2.index(), 0);
            let wp = ((CBN_SELCHANGE as usize) << 16) | ID_FORMAT as usize;
            SendMessageW(dlg, WM_COMMAND, wp, item(dlg, ID_FORMAT) as LPARAM);
            assert!(IsWindowEnabled(item(dlg, ID_ORDER)) == 0, "v2 sorts the files");
            set_text(item(dlg, ID_SOURCE), &src.to_string_lossy());
            let (opts, _) = collect(dlg, &mut f.st).unwrap();
            assert_eq!(opts.format, TorrentFormat::V2);
            assert_eq!(f.st.remembered.format, TorrentFormat::V2);
        }
    }

    #[test]
    fn cancel_stops_hashing_and_closes() {
        let mut f = Fixture::new("cancel", Remembered { piece: 1, ..Remembered::default() });
        let dlg = f.dlg;
        let src = f.dir.join("big.bin");
        // 64 MiB in 16 KiB pieces: still hashing when Cancel arrives
        std::fs::write(&src, vec![0u8; 64 << 20]).unwrap();
        let out = f.dir.join("never.torrent");
        unsafe {
            set_text(item(dlg, ID_SOURCE), &src.to_string_lossy());
            let (opts, seed_from) = collect(dlg, &mut f.st).unwrap();
            hash(dlg, &mut f.st, opts, seed_from, out.clone());
            SendMessageW(dlg, WM_COMMAND, IDCANCEL as usize, 0);
            assert!(f.st.cancel.load(Ordering::Relaxed) && f.st.closing);
        }
        f.pump(20, |st| !st.busy);
        assert!(f.st.done.is_none());
        assert!(!out.exists());
    }
}
