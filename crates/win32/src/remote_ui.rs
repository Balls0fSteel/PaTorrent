//! Remote mode (PLAN.md Phase 6): the window drives a `patorrent-nox` elsewhere instead of an
//! engine of its own. `patorrent.exe --remote` (File > Connect to Remote... starts one) asks
//! where to connect, has the user check a self-signed server's certificate, and runs the usual
//! window on a remote session (`patorrent_core::remote`): the same models, dialogs and
//! automations, the server's torrents.
//!
//! What such a window keeps on disk (its settings, labels, RSS feeds) lives in a folder of its
//! own per server (`remote\<host>_<port>` in the settings folder), so it never mixes with the
//! local client's. The connection details and the accepted certificates are in `remote.ini`
//! beside `patorrent.ini`; the password is not stored. Folders on this computer mean nothing
//! to the server: the folder pickers for torrents are off, and so are Open Containing Folder,
//! seeding a torrent just created here, and the local IP filter.

use std::path::PathBuf;
use std::ptr::{null, null_mut};
use std::sync::OnceLock;

use patorrent_core::Session;
use patorrent_core::remote::{self, Hello, Target};
use patorrent_core::tls::Verify;
use patorrent_frontend::prefs::Prefs;
use patorrent_frontend::remote::{self as connect, Failure, Form, Probe, TIMEOUT};
use patorrent_frontend::tr;
use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, DLGTEMPLATE,
    DialogBoxIndirectParamW, ES_AUTOHSCROLL, ES_NUMBER, ES_PASSWORD, EndDialog, GetDlgItem,
    GetWindowLongPtrW, IDCANCEL, IDOK, SetWindowLongPtrW, SetWindowTextW, WM_COMMAND, WM_INITDIALOG,
    WS_BORDER, WS_CHILD, WS_TABSTOP, WS_VISIBLE,
};

use crate::dialogs::{self, BUTTON, DWLP_USER, EDIT, STATIC, Template};
use crate::util::{loword, send, wide, window_text};

/// The server this window is connected to.
pub struct Remote {
    /// `host:port`, as the title bar shows it.
    pub label: String,
    pub hello: Hello,
}

static REMOTE: OnceLock<Remote> = OnceLock::new();

/// The server, in remote mode.
pub fn get() -> Option<&'static Remote> {
    REMOTE.get()
}

pub fn is_remote() -> bool {
    REMOTE.get().is_some()
}

/// The folder a remote window keeps its files in, inside the settings folder `base`.
pub fn data_dir(base: &std::path::Path, label: &str) -> PathBuf {
    connect::data_dir(base, label)
}

/// Asks where to connect and connects, until it works or the user cancels (`None`). On success
/// the window is in remote mode ([`get`]).
pub fn connect_interactive() -> Option<Session> {
    let mut prefs = Prefs::load(crate::app::base_data_dir().join(connect::FILE_NAME));
    let mut form = Form::load(&prefs);
    loop {
        form = dialog(form)?;
        form.store(&mut prefs);
        let _ = prefs.save();
        let Some((mut target, name)) = form.target() else {
            dialogs::error(null_mut(), tr!(RemoteBadAddress));
            continue;
        };
        if form.https {
            match verify(&mut prefs, &target, &name) {
                Some(v) => target.tls = Some(v),
                None => continue,
            }
        }
        match remote::connect(&target, TIMEOUT) {
            Ok((session, hello)) => {
                let _ = REMOTE.set(Remote { label: name, hello });
                return Some(session);
            }
            Err(e) => {
                // a changed certificate is asked about again at the next attempt, after the
                // user has had a look
                let failure = Failure::new(&e, &name, form.https);
                failure.apply(&mut prefs, &name);
                let _ = prefs.save();
                dialogs::error(null_mut(), &failure.text);
            }
        }
    }
}

/// How to check the server's certificate: one trusted by Windows, or one the user accepted
/// (asked here when it is new or has changed). `None` when the user declined or the server
/// cannot be reached.
fn verify(prefs: &mut Prefs, target: &Target, name: &str) -> Option<Verify> {
    if let Some(pin) = connect::pinned(prefs, name) {
        return Some(Verify::Pinned(pin));
    }
    match connect::probe(target, name) {
        Err(text) => {
            dialogs::error(null_mut(), &text);
            None
        }
        // a certificate Windows trusts for this name needs no question
        Ok(Probe::Trusted) => Some(Verify::System),
        Ok(Probe::Unknown(presented)) => {
            if !dialogs::confirm(null_mut(), &tr!(RemoteTrust, name, fingerprint = presented)) {
                return None;
            }
            connect::pin(prefs, name, presented);
            let _ = prefs.save();
            Some(Verify::Pinned(presented))
        }
    }
}

// --- the dialog ------------------------------------------------------------------------------

const ID_HOST: u16 = 100;
const ID_PORT: u16 = 101;
const ID_USER: u16 = 102;
const ID_PASSWORD: u16 = 103;
const ID_HTTPS: u16 = 104;

fn template() -> Vec<u16> {
    const TAB: u32 = WS_CHILD | WS_VISIBLE | WS_TABSTOP;
    const FIELD: u32 = TAB | WS_BORDER | ES_AUTOHSCROLL as u32;
    let mut t = Template::new(250, 118);
    t.item(STATIC, 0, WS_CHILD | WS_VISIBLE, [7, 9, 50, 9], tr!(RemoteHost));
    t.item(EDIT, ID_HOST, FIELD, [60, 7, 118, 13], "");
    t.item(STATIC, 0, WS_CHILD | WS_VISIBLE, [184, 9, 18, 9], tr!(RemotePort));
    t.item(EDIT, ID_PORT, FIELD | ES_NUMBER as u32, [204, 7, 39, 13], "");
    t.item(STATIC, 0, WS_CHILD | WS_VISIBLE, [7, 27, 50, 9], tr!(RemoteUser));
    t.item(EDIT, ID_USER, FIELD, [60, 25, 118, 13], "");
    t.item(STATIC, 0, WS_CHILD | WS_VISIBLE, [7, 45, 50, 9], tr!(RemotePassword));
    t.item(EDIT, ID_PASSWORD, FIELD | ES_PASSWORD as u32, [60, 43, 118, 13], "");
    t.item(BUTTON, ID_HTTPS, TAB | BS_AUTOCHECKBOX as u32, [60, 61, 183, 10], tr!(RemoteHttps));
    t.item(STATIC, 0, WS_CHILD | WS_VISIBLE, [7, 76, 236, 18], tr!(RemoteNote));
    t.item(BUTTON, IDOK as u16, TAB | BS_DEFPUSHBUTTON as u32, [139, 98, 50, 14], tr!(RemoteConnect));
    t.item(BUTTON, IDCANCEL as u16, TAB | BS_PUSHBUTTON as u32, [193, 98, 50, 14], tr!(BtnCancel));
    t.finish()
}

struct State {
    form: Form,
    ok: bool,
}

/// The connect dialog. `None` on Cancel.
fn dialog(form: Form) -> Option<Form> {
    let mut state = State { form, ok: false };
    let t = template();
    // SAFETY: the template is a valid in-memory dialog; `state` outlives the modal loop.
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            t.as_ptr() as *const DLGTEMPLATE,
            null_mut(),
            Some(proc_),
            &mut state as *mut State as LPARAM,
        );
    }
    state.ok.then_some(state.form)
}

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    // SAFETY: plain dialog-procedure calls; DWLP_USER holds the `State` passed in WM_INITDIALOG,
    // which lives until DialogBoxIndirectParamW returns.
    unsafe {
        let set = |id: u16, s: &str| {
            let w = wide(s);
            SetWindowTextW(GetDlgItem(dlg, id as i32), w.as_ptr());
        };
        let get = |id: u16| window_text(GetDlgItem(dlg, id as i32));
        match msg {
            WM_INITDIALOG => {
                SetWindowLongPtrW(dlg, DWLP_USER, lp);
                let st = &*(lp as *const State);
                let title = wide(tr!(RemoteTitle));
                SetWindowTextW(dlg, title.as_ptr());
                set(ID_HOST, &st.form.host);
                set(ID_PORT, &st.form.port);
                set(ID_USER, &st.form.user);
                send(GetDlgItem(dlg, ID_HTTPS as i32), BM_SETCHECK, usize::from(st.form.https), 0);
                crate::fit::fit_texts(dlg, |_| Some(0));
                // the first empty field gets the focus
                let first = if st.form.host.is_empty() { ID_HOST } else { ID_PASSWORD };
                SetFocus(GetDlgItem(dlg, first as i32));
                0
            }
            WM_COMMAND => {
                let id = loword(wp) as i32;
                if id == IDOK || id == IDCANCEL {
                    let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut State;
                    if id == IDOK && !st.is_null() {
                        (*st).form = Form {
                            host: get(ID_HOST).trim().to_owned(),
                            port: get(ID_PORT).trim().to_owned(),
                            user: get(ID_USER),
                            password: get(ID_PASSWORD),
                            https: send(GetDlgItem(dlg, ID_HTTPS as i32), BM_GETCHECK, 0, 0) == 1,
                        };
                        (*st).ok = true;
                    }
                    EndDialog(dlg, id as isize);
                    return 1;
                }
                0
            }
            _ => 0,
        }
    }
}

/// Starts another PaTorrent window in remote mode (File > Connect to Remote...).
pub fn start_remote_window(owner: HWND) {
    if let Err(e) = std::process::Command::new(crate::util::exe_path()).arg("--remote").spawn() {
        dialogs::error(owner, &tr!(ErrStartPaTorrent, error = e));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folders_and_the_template() {
        let base = std::path::Path::new(r"C:\PaTorrent");
        assert_eq!(data_dir(base, "nas.lan:8080"), base.join("remote").join("nas.lan_8080"));
        // the template is well formed: its item count is the twelve controls
        let t = template();
        assert_eq!(t[4], 12);
    }
}
