//! The search helper (D51, `docs/SEARCH_HELPER.md`) in the Search Sources dialog: "Get the
//! Search Helper (Jackett)..." while it is not installed; "Pick Sites...", "Open the Helper's
//! Page", "Update the Search Helper" and "Remove the Search Helper" once it is. Each step runs
//! behind the updater's progress dialog ([`crate::updater::run`]) on a worker thread, on
//! [`rookparcel_frontend::search::helper`].
//!
//! Get: the newest release from GitHub, the box that says what it is (a separate program by
//! its own authors under the GPL-2.0, from github.com/Jackett/Jackett, its size, where it goes,
//! that it runs only while searching), the download checked against the release's SHA-256,
//! the "Search helper (all)" source added, then the site picker. The picker lists the helper's
//! public sites with nothing ticked (a site ticked before stays ticked); sites that need
//! FlareSolverr are greyed and cannot be ticked.

use std::ptr::null;

use rookparcel_frontend::search::helper::{self, Indexer};
use rookparcel_frontend::{fmt, tr};
use windows_sys::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{COLOR_GRAYTEXT, GetSysColor};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use super::{Host, Sources, dlu, fill_sources, item, lv_add, lv_checked, lv_set_checked, set_text};
use crate::controls;
use crate::dialogs::{self, BUTTON, Class, DWLP_MSGRESULT, DWLP_USER, STATIC, Template};
use crate::updater;
use crate::util::{loword, send};

pub const ID_GET: u16 = 206;
pub const ID_SITES: u16 = 207;
pub const ID_PAGE: u16 = 208;
pub const ID_UPDATE: u16 = 209;
pub const ID_REMOVE: u16 = 210;

/// Tells the frontend where the profile folder is and which proxy to give the helper; whether
/// this window offers the helper at all.
pub fn configure(host: &mut dyn Host) -> bool {
    match host.helper() {
        Some((dir, proxy)) => {
            helper::configure(&dir, proxy);
            helper::available()
        }
        None => false,
    }
}

/// Shows the buttons that fit: Get while not installed, the other four once it is, none where
/// the helper is not offered.
pub unsafe fn show_buttons(dlg: HWND, host: &mut dyn Host) {
    let on = configure(host);
    let installed = on && helper::installed();
    unsafe {
        ShowWindow(item(dlg, ID_GET), if on && !installed { SW_SHOW } else { SW_HIDE });
        for id in [ID_SITES, ID_PAGE, ID_UPDATE, ID_REMOVE] {
            ShowWindow(item(dlg, id), if installed { SW_SHOW } else { SW_HIDE });
        }
    }
}

/// A helper button's click; false for any other id.
pub unsafe fn command(dlg: HWND, st: &mut Sources<'_>, id: u16) -> bool {
    unsafe {
        match id {
            ID_GET => get(dlg, st, false),
            ID_UPDATE => get(dlg, st, true),
            ID_SITES => pick_sites(dlg, st),
            ID_PAGE => open_page(dlg),
            ID_REMOVE => remove(dlg, st),
            _ => return false,
        }
        show_buttons(dlg, st.host);
        true
    }
}

fn failed(owner: HWND, e: &str) {
    dialogs::error(owner, &tr!(SearchHelperDownloadFailed, error = e));
}

/// Get (or Update): the newest release, the confirm box, the download and unpack, the source;
/// after a first install, the site picker.
unsafe fn get(dlg: HWND, st: &mut Sources<'_>, update: bool) {
    unsafe {
        let proxy = st.host.proxy();
        let p = proxy.clone();
        let checked = updater::run(dlg, tr!(SearchHelperChecking), helper::release::PROJECT_URL, move |_| {
            helper::check(p)
        });
        let release = match checked {
            None => return,
            Some(Err(e)) => return failed(dlg, &e),
            Some(Ok(r)) => r,
        };
        let size = fmt::size(release.asset.size);
        let current = helper::state().map(|s| s.version).unwrap_or_default();
        let question = if update {
            if current == release.version {
                dialogs::info(dlg, &tr!(SearchHelperUpToDate, version = current));
                return;
            }
            tr!(SearchHelperUpdateConfirm, current = current, version = release.version, size = size)
        } else {
            let folder = helper::layout().map(|l| l.root.display().to_string()).unwrap_or_default();
            tr!(SearchHelperConfirm, version = release.version, size = size, folder = folder)
        };
        if !dialogs::ask(dlg, &question) {
            return;
        }
        let r = release.clone();
        let title = tr!(SearchHelperDownloading, version = release.version);
        let installed = updater::run(dlg, &title, &release.asset.name, move |progress| {
            helper::install(&r, proxy, progress)
        });
        let state = match installed {
            None => return,
            Some(Err(e)) => return failed(dlg, &e),
            Some(Ok(s)) => s,
        };
        let sources = helper::with_helper_source(&st.host.sources(), &state);
        st.host.store_sources(&sources);
        fill_sources(dlg, st, Some(sources.len().saturating_sub(1)));
        set_text(item(dlg, super::ID_SRC_STATUS), &tr!(SearchHelperInstalled, version = state.version));
        if !update {
            show_buttons(dlg, st.host);
            pick_sites(dlg, st);
        }
    }
}

/// Remove: the confirm box, the helper stopped and its folder deleted, its source taken out.
unsafe fn remove(dlg: HWND, st: &mut Sources<'_>) {
    unsafe {
        let folder = helper::layout().map(|l| l.root.display().to_string()).unwrap_or_default();
        if !dialogs::confirm(dlg, &tr!(SearchHelperRemoveConfirm, folder = folder)) {
            return;
        }
        match helper::remove() {
            Ok(state) => {
                if let Some(state) = state {
                    let sources = helper::without_helper_source(&st.host.sources(), &state);
                    st.host.store_sources(&sources);
                }
                fill_sources(dlg, st, None);
                set_text(item(dlg, super::ID_SRC_STATUS), tr!(SearchHelperRemoved));
            }
            Err(e) => dialogs::error(dlg, &e),
        }
    }
}

/// Open the Helper's Page: started first, its password on the clipboard and in a box, then
/// the browser.
unsafe fn open_page(dlg: HWND) {
    let page = updater::run(dlg, tr!(SearchHelperStarting), "Jackett", |_| helper::page());
    match page {
        None => {}
        Some(Err(e)) => dialogs::error(dlg, &tr!(SearchHelperFailed, error = e)),
        Some(Ok((url, password))) => {
            dialogs::set_clipboard_text(dlg, &password);
            dialogs::info(dlg, &tr!(SearchHelperPagePassword, password = password));
            crate::app::open_url(dlg, &url);
        }
    }
}

/// Pick Sites: the helper's public sites (it is started first), the picker, then the ticked
/// ones configured and the unticked ones removed.
unsafe fn pick_sites(dlg: HWND, st: &mut Sources<'_>) {
    unsafe {
        let listed = updater::run(dlg, tr!(SearchHelperStarting), "Jackett", |_| helper::list_sites());
        let list = match listed {
            None => return,
            Some(Err(e)) => return dialogs::error(dlg, &tr!(SearchHelperFailed, error = e)),
            Some(Ok(l)) => l,
        };
        let Some(ticked) = picker(dlg, &list) else { return };
        let count = ticked.len();
        let saved = updater::run(dlg, tr!(SearchHelperSaving), "Jackett", move |_| {
            Ok(helper::apply_sites(&list, &ticked))
        });
        match saved {
            None => {}
            Some(Ok(Err(errors))) => {
                dialogs::error(dlg, &tr!(SearchHelperSitesFailed, errors = errors.join("\n")));
            }
            Some(Err(e)) => dialogs::error(dlg, &e),
            Some(Ok(Ok(()))) => {
                set_text(item(dlg, super::ID_SRC_STATUS), &tr!(SearchHelperSitesDone, count = count));
            }
        }
        // the source list may show the helper's source as it is now
        fill_sources(dlg, st, None);
    }
}

// ---- the site picker ------------------------------------------------------------------------

const ID_HINT: u16 = 400;
const ID_LIST: u16 = 401;

struct Picker<'a> {
    list: &'a [Indexer],
    /// Filling the list: its notifications are not the user's.
    loading: bool,
    ticked: Option<Vec<String>>,
}

/// The site picker over `owner`: the ids ticked on OK, `None` on Cancel.
pub fn picker(owner: HWND, list: &[Indexer]) -> Option<Vec<String>> {
    let mut st = Picker { list, loading: false, ticked: None };
    let t = picker_template();
    unsafe {
        DialogBoxIndirectParamW(
            GetModuleHandleW(null()),
            t.as_ptr() as *const DLGTEMPLATE,
            owner,
            Some(picker_proc),
            &mut st as *mut Picker<'_> as LPARAM,
        );
    }
    st.ticked
}

fn picker_template() -> Vec<u16> {
    const C: u32 = WS_CHILD | WS_VISIBLE;
    const TAB: u32 = C | WS_TABSTOP;
    let mut t = Template::new(400, 260);
    t.item(STATIC, ID_HINT, C | super::SS_NOPREFIX, [7, 7, 386, 18], tr!(SearchHelperSitesHint));
    let list = TAB | WS_BORDER | LVS_REPORT | LVS_SHOWSELALWAYS | LVS_SINGLESEL;
    t.item(Class::Name("SysListView32"), ID_LIST, list, [7, 28, 386, 206], "");
    t.item(BUTTON, IDOK as u16, TAB | BS_DEFPUSHBUTTON as u32, [289, 240, 50, 14], tr!(BtnOk));
    t.item(BUTTON, IDCANCEL as u16, TAB | BS_PUSHBUTTON as u32, [343, 240, 50, 14], tr!(BtnCancel));
    t.finish()
}

unsafe extern "system" fn picker_proc(dlg: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> isize {
    unsafe {
        if msg == WM_INITDIALOG {
            SetWindowLongPtrW(dlg, DWLP_USER, lp);
            picker_init(dlg, &mut *(lp as *mut Picker<'_>));
            crate::fit::fit_texts(dlg, |_| Some(0));
            return 1;
        }
        let st = GetWindowLongPtrW(dlg, DWLP_USER) as *mut Picker<'_>;
        if st.is_null() {
            return 0;
        }
        let st = &mut *st;
        match msg {
            WM_COMMAND if loword(wp) == IDOK as u16 => {
                let lv = item(dlg, ID_LIST);
                let ticked = st
                    .list
                    .iter()
                    .enumerate()
                    .filter(|&(row, i)| !i.needs_flaresolverr && lv_checked(lv, row))
                    .map(|(_, i)| i.id.clone())
                    .collect();
                st.ticked = Some(ticked);
                EndDialog(dlg, IDOK as isize);
                1
            }
            WM_COMMAND if loword(wp) == IDCANCEL as u16 => {
                EndDialog(dlg, IDCANCEL as isize);
                1
            }
            WM_NOTIFY => {
                let nm = &*(lp as *const NMHDR);
                if nm.idFrom as u16 != ID_LIST {
                    return 0;
                }
                match nm.code {
                    // a FlareSolverr site's box cannot be ticked
                    LVN_ITEMCHANGING if !st.loading => {
                        let lv = &*(lp as *const NMLISTVIEW);
                        let to_on = lv.uChanged & LVIF_STATE != 0
                            && lv.uNewState & LVIS_STATEIMAGEMASK == 2 << 12
                            && lv.uOldState & LVIS_STATEIMAGEMASK != 0;
                        let refuse = to_on
                            && usize::try_from(lv.iItem)
                                .ok()
                                .and_then(|r| st.list.get(r))
                                .is_some_and(|i| i.needs_flaresolverr);
                        SetWindowLongPtrW(dlg, DWLP_MSGRESULT, refuse as isize);
                        1
                    }
                    NM_CUSTOMDRAW => {
                        let cd = &mut *(lp as *mut NMLVCUSTOMDRAW);
                        let r = match cd.nmcd.dwDrawStage {
                            CDDS_PREPAINT => CDRF_NOTIFYITEMDRAW as isize,
                            CDDS_ITEMPREPAINT => {
                                if st.list.get(cd.nmcd.dwItemSpec).is_some_and(|i| i.needs_flaresolverr) {
                                    cd.clrText = GetSysColor(COLOR_GRAYTEXT);
                                }
                                CDRF_DODEFAULT as isize
                            }
                            _ => CDRF_DODEFAULT as isize,
                        };
                        SetWindowLongPtrW(dlg, DWLP_MSGRESULT, r);
                        1
                    }
                    _ => 0,
                }
            }
            _ => 0,
        }
    }
}

unsafe fn picker_init(dlg: HWND, st: &mut Picker<'_>) {
    unsafe {
        set_text(dlg, tr!(SearchHelperSitesTitle));
        let lv = item(dlg, ID_LIST);
        let cols = [
            (tr!(ColName), dlu(dlg, 130, 0).0, false),
            (tr!(SearchHelperColLanguage), dlu(dlg, 50, 0).0, false),
            (tr!(SearchHelperColType), dlu(dlg, 50, 0).0, false),
            (tr!(SearchHelperColNote), dlu(dlg, 140, 0).0, false),
        ];
        controls::set_columns(lv, &cols);
        let ex = LVS_EX_FULLROWSELECT | LVS_EX_LABELTIP | LVS_EX_DOUBLEBUFFER | LVS_EX_CHECKBOXES;
        send(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, ex as usize, ex as isize);
        st.loading = true;
        for (row, i) in st.list.iter().enumerate() {
            let note = if i.needs_flaresolverr { tr!(SearchHelperNeedsFlaresolverr) } else { "" };
            lv_add(lv, row, &[&i.name, &i.language, &i.kind, note]);
            // nothing ticked for the user: only what was ticked before
            lv_set_checked(lv, row, i.configured && !i.needs_flaresolverr);
        }
        st.loading = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(id: &str, configured: bool, cf: bool) -> Indexer {
        Indexer {
            id: id.into(),
            name: id.to_uppercase(),
            kind: "public".into(),
            language: "en-US".into(),
            configured,
            needs_flaresolverr: cf,
            ..Indexer::default()
        }
    }

    /// The picker as the user meets it: nothing ticked but what was before, a FlareSolverr site
    /// refusing its tick, OK handing back the ticked ids.
    #[test]
    fn the_picker_ticks_nothing_and_refuses_flaresolverr_sites() {
        let list = [site("open", false, false), site("before", true, false), site("cf", true, true)];
        let mut st = Picker { list: &list, loading: false, ticked: None };
        let t = picker_template();
        unsafe {
            let dlg = CreateDialogIndirectParamW(
                GetModuleHandleW(null()),
                t.as_ptr() as *const DLGTEMPLATE,
                std::ptr::null_mut(),
                Some(picker_proc),
                &mut st as *mut Picker<'_> as LPARAM,
            );
            assert!(!dlg.is_null());
            let lv = item(dlg, ID_LIST);
            assert_eq!(send(lv, LVM_GETITEMCOUNT, 0, 0), 3);
            assert!(!lv_checked(lv, 0), "nothing ticked for the user");
            assert!(lv_checked(lv, 1), "a site ticked before stays");
            assert!(!lv_checked(lv, 2), "a FlareSolverr site is never ticked");
            lv_set_checked(lv, 2, true);
            assert!(!lv_checked(lv, 2), "and cannot be");
            lv_set_checked(lv, 0, true);
            send(dlg, WM_COMMAND, IDOK as usize, 0);
            DestroyWindow(dlg);
        }
        assert_eq!(st.ticked, Some(vec!["open".to_owned(), "before".to_owned()]));
    }
}
