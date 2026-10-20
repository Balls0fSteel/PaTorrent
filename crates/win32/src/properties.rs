//! The Torrent Properties dialog (µTorrent 2.2.1): the General tab's Trackers, Bandwidth
//! Settings, Seeding Goal and Other Settings, for the selected torrents (the Advanced tab is
//! not built).
//!
//! The fields come from [`patorrent_frontend::properties`]. With several torrents selected the
//! check boxes have three states and fields whose values differ start blank or indeterminate;
//! what the user leaves that way stays as it is on every torrent, as in µTorrent. OK hands
//! back the [`Changes`] for the caller to apply to each torrent.

use std::mem::zeroed;
use std::ptr::null;

use patorrent_frontend::properties::{Changes, Field, FieldError, Fields, Properties, Tri};
use patorrent_frontend::tr;
use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::{
    BST_CHECKED, BST_INDETERMINATE, BST_UNCHECKED, EM_SETSEL, TCIF_TEXT, TCITEMW, TCM_INSERTITEMW,
};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BN_CLICKED, BS_AUTO3STATE, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_GROUPBOX,
    BS_PUSHBUTTON, DLGTEMPLATE, DialogBoxIndirectParamW, ES_AUTOHSCROLL, ES_AUTOVSCROLL, ES_MULTILINE,
    ES_NUMBER, ES_WANTRETURN, EndDialog, GetDlgItem, GetWindowLongPtrW, IDCANCEL, IDOK, SetWindowLongPtrW,
    SetWindowTextW, WM_COMMAND, WM_INITDIALOG, WS_BORDER, WS_CHILD, WS_CLIPSIBLINGS, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL,
};

use crate::dialogs::{self, BUTTON, Class, DWLP_USER, EDIT, STATIC, Template};
use crate::util::{hiword, loword, send, wide, window_text};

const ID_TABS: u16 = 100;
const ID_TRACKERS: u16 = 101;
const ID_UL: u16 = 102;
const ID_DL: u16 = 103;
const ID_SLOTS: u16 = 104;
const ID_OVERRIDE_SLOTS: u16 = 105;
const ID_OVERRIDE_GOAL: u16 = 106;
const ID_RATIO: u16 = 107;
const ID_MINUTES: u16 = 108;
const ID_INITIAL_SEEDING: u16 = 109;
const ID_DHT: u16 = 110;
const ID_PEX: u16 = 111;

struct State {
    /// "Properties - name", or how many torrents.
    title: String,
    props: Properties,
    changes: Option<Changes>,
    /// The last validation error (also shown in a message box outside tests).
    error: Option<FieldError>,
}

/// Shows the dialog; the changes on OK (possibly none), `None` on Cancel.
pub fn show(owner: HWND, title: &str, props: Properties) -> Option<Changes> {
    let multi = props.count > 1;
    let mut st = State { title: title.to_owned(), props, changes: None, error: None };
    let template = template(multi);
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            template.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(proc_),
            &mut st as *mut State as LPARAM,
        );
    }
    st.changes
}

/// The General tab's layout (dialog units); `multi` gives the check boxes a third state.
fn template(multi: bool) -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    const TAB: u32 = C | WS_TABSTOP;
    const NUMBER: u32 = TAB | WS_BORDER | ES_AUTOHSCROLL as u32 | ES_NUMBER as u32;
    const GROUP: u32 = C | BS_GROUPBOX as u32;
    let check = TAB | if multi { BS_AUTO3STATE } else { BS_AUTOCHECKBOX } as u32;
    let tracker_box = TAB | WS_BORDER | WS_VSCROLL | (ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN) as u32;
    let mut t = Template::new(300, 324);
    t.item(BUTTON, 0xffff, GROUP, [14, 24, 272, 78], tr!(PropsTrackers));
    t.item(EDIT, ID_TRACKERS, tracker_box, [21, 36, 258, 60], "");

    t.item(BUTTON, 0xffff, GROUP, [14, 106, 272, 74], tr!(PropsBandwidth));
    t.item(STATIC, 0xffff, C, [21, 121, 200, 9], tr!(PropsUpRate));
    t.item(EDIT, ID_UL, NUMBER, [230, 119, 49, 13], "");
    t.item(STATIC, 0xffff, C, [21, 137, 200, 9], tr!(PropsDownRate));
    t.item(EDIT, ID_DL, NUMBER, [230, 135, 49, 13], "");
    t.item(STATIC, 0xffff, C, [21, 153, 200, 9], tr!(PropsSlots));
    t.item(EDIT, ID_SLOTS, NUMBER, [230, 151, 49, 13], "");
    t.item(BUTTON, ID_OVERRIDE_SLOTS, check, [21, 166, 200, 10], tr!(PropsOverrideSlots));

    t.item(BUTTON, 0xffff, GROUP, [14, 184, 272, 58], tr!(PropsSeedingGoal));
    t.item(BUTTON, ID_OVERRIDE_GOAL, check, [21, 196, 200, 10], tr!(PropsOverrideGoal));
    t.item(STATIC, 0xffff, C, [21, 212, 200, 9], tr!(PropsRatio));
    t.item(EDIT, ID_RATIO, NUMBER, [230, 210, 49, 13], "");
    t.item(STATIC, 0xffff, C, [21, 228, 200, 9], tr!(PropsSeedTime));
    t.item(EDIT, ID_MINUTES, NUMBER, [230, 226, 49, 13], "");

    t.item(BUTTON, 0xffff, GROUP, [14, 246, 272, 44], tr!(PropsOther));
    t.item(BUTTON, ID_INITIAL_SEEDING, check, [21, 258, 120, 10], tr!(PropsInitialSeeding));
    t.item(BUTTON, ID_DHT, check, [21, 272, 120, 10], tr!(PropsDht));
    t.item(BUTTON, ID_PEX, check, [150, 258, 120, 10], tr!(PropsPex));

    // last, so it lies under the page's controls (and does not take their clicks)
    t.item(Class::Name("SysTabControl32"), ID_TABS, C | WS_CLIPSIBLINGS, [7, 7, 286, 290], "");
    t.item(BUTTON, IDOK as u16, TAB | BS_DEFPUSHBUTTON as u32, [189, 303, 50, 14], tr!(BtnOk));
    t.item(BUTTON, IDCANCEL as u16, TAB | BS_PUSHBUTTON as u32, [243, 303, 50, 14], tr!(BtnCancel));
    t.finish()
}

unsafe fn item(dlg: HWND, id: u16) -> HWND {
    unsafe { GetDlgItem(dlg, id as i32) }
}

unsafe fn set_text(hwnd: HWND, s: &str) {
    let w = wide(s);
    unsafe { SetWindowTextW(hwnd, w.as_ptr()) };
}

unsafe fn get_tri(dlg: HWND, id: u16) -> Tri {
    match unsafe { send(item(dlg, id), BM_GETCHECK, 0, 0) } as u32 {
        BST_CHECKED => Tri::On,
        BST_INDETERMINATE => Tri::Mixed,
        _ => Tri::Off,
    }
}

unsafe fn set_tri(dlg: HWND, id: u16, v: Tri) {
    let state = match v {
        Tri::On => BST_CHECKED,
        Tri::Off => BST_UNCHECKED,
        Tri::Mixed => BST_INDETERMINATE,
    };
    unsafe { send(item(dlg, id), BM_SETCHECK, state as usize, 0) };
}

/// The tracker box shows Windows line ends.
fn to_edit(s: &str) -> String {
    s.replace('\n', "\r\n")
}

unsafe extern "system" fn proc_(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            init(dlg, &*(lp as *const State));
            return 1;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut State;
        if st.is_null() || msg != WM_COMMAND {
            return 0;
        }
        command(dlg, &mut *st, loword(wp), hiword(wp) as u32) as isize
    }
}

unsafe fn init(dlg: HWND, st: &State) {
    unsafe {
        set_text(dlg, &st.title);
        let mut text = wide(tr!(TabGeneral));
        let mut tab: TCITEMW = zeroed();
        tab.mask = TCIF_TEXT;
        tab.pszText = text.as_mut_ptr();
        send(item(dlg, ID_TABS), TCM_INSERTITEMW, 0, &tab as *const _ as LPARAM);
        show_fields(dlg, &st.props.fields);
        EnableWindow(item(dlg, ID_INITIAL_SEEDING), st.props.can_initial_seed as i32);
        update_enabled(dlg);
    }
}

unsafe fn show_fields(dlg: HWND, f: &Fields) {
    unsafe {
        set_text(item(dlg, ID_TRACKERS), &to_edit(&f.trackers));
        for (id, text) in [
            (ID_UL, &f.upload_kib),
            (ID_DL, &f.download_kib),
            (ID_SLOTS, &f.upload_slots),
            (ID_RATIO, &f.goal_ratio),
            (ID_MINUTES, &f.goal_minutes),
        ] {
            set_text(item(dlg, id), text);
        }
        for (id, v) in [
            (ID_OVERRIDE_SLOTS, f.override_slots),
            (ID_OVERRIDE_GOAL, f.override_goal),
            (ID_INITIAL_SEEDING, f.initial_seeding),
            (ID_DHT, f.dht),
            (ID_PEX, f.pex),
        ] {
            set_tri(dlg, id, v);
        }
    }
}

unsafe fn read_fields(dlg: HWND) -> Fields {
    unsafe {
        let text = |id| window_text(item(dlg, id));
        Fields {
            trackers: text(ID_TRACKERS).replace("\r\n", "\n"),
            upload_kib: text(ID_UL),
            download_kib: text(ID_DL),
            override_slots: get_tri(dlg, ID_OVERRIDE_SLOTS),
            upload_slots: text(ID_SLOTS),
            override_goal: get_tri(dlg, ID_OVERRIDE_GOAL),
            goal_ratio: text(ID_RATIO),
            goal_minutes: text(ID_MINUTES),
            initial_seeding: get_tri(dlg, ID_INITIAL_SEEDING),
            dht: get_tri(dlg, ID_DHT),
            pex: get_tri(dlg, ID_PEX),
        }
    }
}

/// The slot count and the goal's values are edited only while their override is not off.
unsafe fn update_enabled(dlg: HWND) {
    unsafe {
        let slots = get_tri(dlg, ID_OVERRIDE_SLOTS) != Tri::Off;
        EnableWindow(item(dlg, ID_SLOTS), slots as i32);
        let goal = get_tri(dlg, ID_OVERRIDE_GOAL) != Tri::Off;
        EnableWindow(item(dlg, ID_RATIO), goal as i32);
        EnableWindow(item(dlg, ID_MINUTES), goal as i32);
    }
}

unsafe fn command(dlg: HWND, st: &mut State, id: u16, code: u32) -> bool {
    unsafe {
        match id {
            ID_OVERRIDE_SLOTS | ID_OVERRIDE_GOAL if code == BN_CLICKED => update_enabled(dlg),
            _ if id == IDOK as u16 => {
                st.props.fields = read_fields(dlg);
                match st.props.changes() {
                    Ok(c) => {
                        st.error = None;
                        st.changes = Some(c);
                        EndDialog(dlg, IDOK as isize);
                    }
                    Err(e) => report(dlg, st, e),
                }
            }
            _ if id == IDCANCEL as u16 => {
                EndDialog(dlg, IDCANCEL as isize);
            }
            _ => return false,
        }
        true
    }
}

/// Tells the user what is wrong and puts the caret in that field.
unsafe fn report(dlg: HWND, st: &mut State, e: FieldError) {
    let id = match e.field {
        Field::Trackers => ID_TRACKERS,
        Field::UploadRate => ID_UL,
        Field::DownloadRate => ID_DL,
        Field::UploadSlots => ID_SLOTS,
        Field::GoalRatio => ID_RATIO,
        Field::GoalMinutes => ID_MINUTES,
    };
    if !cfg!(test) {
        dialogs::error(dlg, &e.message);
    }
    st.error = Some(e);
    unsafe {
        let edit = item(dlg, id);
        SetFocus(edit);
        send(edit, EM_SETSEL, 0, -1);
    }
}

/// The dialog's title: "Properties - name" for one torrent.
pub fn title(names: &[String]) -> String {
    match names {
        [one] => tr!(PropsTitle, name = one),
        _ => tr!(PropsTitleMany, count = names.len()),
    }
}

#[cfg(test)]
mod tests {
    //! The dialog built from its template, hidden and modeless, driven through its messages.

    use patorrent_frontend::properties::{Defaults, GoalChange, TorrentProps};
    use patorrent_frontend::seeding::SeedGoal;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::IsWindowEnabled;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateDialogIndirectParamW, DestroyWindow, SendMessageW,
    };

    use std::ptr::null_mut;

    use super::*;

    struct Fixture {
        dlg: HWND,
        st: Box<State>,
    }

    impl Fixture {
        fn new(items: &[TorrentProps]) -> Fixture {
            let defaults = Defaults { upload_slots: 4, seed_goal: SeedGoal::default() };
            let props = Properties::load(items, defaults);
            let multi = props.count > 1;
            let mut st =
                Box::new(State { title: "Properties - t".into(), props, changes: None, error: None });
            let template = template(multi);
            let dlg = unsafe {
                CreateDialogIndirectParamW(
                    GetModuleHandleW(null()),
                    template.as_ptr() as *const DLGTEMPLATE,
                    null_mut(),
                    Some(proc_),
                    &mut *st as *mut State as LPARAM,
                )
            };
            assert!(!dlg.is_null(), "the template does not load");
            Fixture { dlg, st }
        }

        fn text(&self, id: u16) -> String {
            unsafe { window_text(item(self.dlg, id)) }
        }

        fn set(&self, id: u16, s: &str) {
            unsafe { set_text(item(self.dlg, id), s) };
        }

        /// A click on a check box: the next state as the button would cycle it (three-state:
        /// off, on, mixed), then the notification.
        fn click(&self, id: u16) {
            let multi = self.st.props.count > 1;
            let next = match self.tri(id) {
                Tri::Off => Tri::On,
                Tri::On if multi => Tri::Mixed,
                _ => Tri::Off,
            };
            unsafe {
                set_tri(self.dlg, id, next);
                let wp = (BN_CLICKED as usize) << 16 | id as usize;
                SendMessageW(self.dlg, WM_COMMAND, wp, item(self.dlg, id) as LPARAM);
            }
        }

        fn ok(&self) {
            unsafe { SendMessageW(self.dlg, WM_COMMAND, IDOK as usize, 0) };
        }

        fn tri(&self, id: u16) -> Tri {
            unsafe { get_tri(self.dlg, id) }
        }

        fn enabled(&self, id: u16) -> bool {
            unsafe { IsWindowEnabled(item(self.dlg, id)) != 0 }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { DestroyWindow(self.dlg) };
        }
    }

    fn torrent() -> TorrentProps {
        TorrentProps {
            trackers: Some(vec![
                vec!["udp://a:1/announce".into(), "udp://b:1/announce".into()],
                vec!["http://c/announce".into()],
            ]),
            upload_limit: 20 * 1024,
            download_limit: 0,
            uploads_limit: Some(4),
            seed_goal: None,
            super_seeding: false,
            is_seed: false,
            dht: true,
            pex: false,
        }
    }

    #[test]
    fn shows_one_torrent() {
        let f = Fixture::new(&[torrent()]);
        assert_eq!(f.text(ID_TRACKERS), "udp://a:1/announce\r\nudp://b:1/announce\r\n\r\nhttp://c/announce");
        assert_eq!((f.text(ID_UL), f.text(ID_DL), f.text(ID_SLOTS)), ("20".into(), "0".into(), "4".into()));
        assert_eq!((f.text(ID_RATIO), f.text(ID_MINUTES)), ("150".into(), "0".into()));
        assert_eq!(f.tri(ID_OVERRIDE_SLOTS), Tri::Off);
        assert_eq!((f.tri(ID_DHT), f.tri(ID_PEX)), (Tri::On, Tri::Off));
        assert!(!f.enabled(ID_SLOTS) && !f.enabled(ID_RATIO), "overrides off: greyed");
        assert!(!f.enabled(ID_INITIAL_SEEDING), "not a seed");
        f.click(ID_OVERRIDE_SLOTS);
        assert!(f.enabled(ID_SLOTS));
        f.click(ID_OVERRIDE_SLOTS);
        assert!(!f.enabled(ID_SLOTS));
    }

    #[test]
    fn ok_hands_back_what_changed() {
        let mut f = Fixture::new(&[torrent()]);
        f.ok();
        assert!(f.st.changes.take().expect("OK without changes").is_empty());

        f.set(ID_TRACKERS, "udp://a:1/announce\r\n\r\nudp://z:2/announce\r\n");
        f.set(ID_DL, "300");
        f.click(ID_OVERRIDE_GOAL);
        f.set(ID_RATIO, "0");
        f.set(ID_MINUTES, "120");
        f.click(ID_PEX);
        f.ok();
        let c = f.st.changes.take().expect("OK gives the changes");
        assert_eq!(
            c.trackers,
            Some(vec![vec!["udp://a:1/announce".to_owned()], vec!["udp://z:2/announce".to_owned()]])
        );
        assert_eq!((c.download_limit, c.upload_limit), (Some(300 * 1024), None));
        assert_eq!(c.seed_goal, Some(GoalChange::Own { ratio_pct: Some(0), minutes: Some(120) }));
        assert_eq!((c.pex, c.dht, c.max_uploads), (Some(true), None, None));
    }

    #[test]
    fn a_bad_tracker_keeps_the_dialog_open() {
        let mut f = Fixture::new(&[torrent()]);
        f.set(ID_TRACKERS, "udp://a:1/announce\r\nnot-a-tracker");
        f.ok();
        assert!(f.st.changes.is_none());
        let e = f.st.error.take().expect("an error");
        assert_eq!(e.field, Field::Trackers);
        assert!(e.message.contains("not-a-tracker"), "{}", e.message);
        f.set(ID_TRACKERS, "");
        f.ok();
        assert_eq!(f.st.changes.take().unwrap().trackers, Some(Vec::new()), "an empty box removes them all");
    }

    #[test]
    fn several_torrents_start_mixed() {
        let a = torrent();
        let mut b = torrent();
        b.upload_limit = 0;
        b.pex = true;
        b.uploads_limit = Some(9);
        b.trackers = None;
        let mut f = Fixture::new(&[a, b]);
        assert_eq!(f.text(ID_TRACKERS), "");
        assert_eq!((f.text(ID_UL), f.text(ID_DL)), (String::new(), "0".into()));
        assert_eq!(
            (f.tri(ID_PEX), f.tri(ID_DHT), f.tri(ID_OVERRIDE_SLOTS)),
            (Tri::Mixed, Tri::On, Tri::Mixed)
        );
        assert!(f.enabled(ID_SLOTS), "a mixed override can be edited");
        f.ok();
        assert!(f.st.changes.take().unwrap().is_empty(), "untouched fields stay as they are");
        // three-state boxes cycle mixed -> off -> on
        f.click(ID_PEX);
        f.set(ID_UL, "5");
        f.ok();
        let c = f.st.changes.take().unwrap();
        assert_eq!((c.pex, c.upload_limit, c.trackers), (Some(false), Some(5120), None));
        f.click(ID_PEX);
        assert_eq!(f.tri(ID_PEX), Tri::On);
        f.ok();
        assert_eq!(f.st.changes.take().unwrap().pex, Some(true));
    }

    #[test]
    fn titles() {
        assert_eq!(title(&["Debian".into()]), "Properties - Debian");
        assert_eq!(title(&["a".into(), "b".into()]), "Properties - 2 torrents");
    }
}
