//! The search helper (D51, `docs/SEARCH_HELPER.md`) in the Search Sources window, as the
//! Windows GUI has it: "Get the Search Helper (Jackett)..." while it is not installed; "Pick
//! Sites...", "Open the Helper's Page", "Update the Search Helper" and "Remove the Search
//! Helper" once it is. Each step runs on GIO's thread pool behind a small progress window
//! ([`run_job`]), on [`patorrent_frontend::search::helper`]. Not offered in a Flatpak or in a
//! build without the update check ([`helper::available`]).

use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use patorrent_frontend::lang::gtk_mnemonic;
use patorrent_frontend::search::helper::{self, Indexer};
use patorrent_frontend::{fmt, tr};

use super::{Host, Manager, margins};

/// Tells the frontend where the profile folder is and which proxy to give the helper; whether
/// this window offers the helper at all.
pub fn configure(host: &dyn Host) -> bool {
    match host.helper() {
        Some((dir, proxy)) => {
            helper::configure(&dir, proxy);
            helper::available()
        }
        None => false,
    }
}

/// The helper's buttons of the Search Sources window.
pub struct Buttons {
    pub row: gtk::Box,
    get: gtk::Button,
    sites: gtk::Button,
    page: gtk::Button,
    update: gtk::Button,
    remove: gtk::Button,
}

impl Buttons {
    pub fn new() -> Buttons {
        let button = |t: &str| gtk::Button::with_mnemonic(&gtk_mnemonic(t));
        let b = Buttons {
            row: gtk::Box::new(gtk::Orientation::Horizontal, 6),
            get: button(tr!(SearchHelperGet)),
            sites: button(tr!(SearchHelperSites)),
            page: button(tr!(SearchHelperOpenPage)),
            update: button(tr!(SearchHelperUpdate)),
            remove: button(tr!(SearchHelperRemove)),
        };
        for w in [&b.get, &b.sites, &b.page, &b.update, &b.remove] {
            b.row.append(w);
        }
        b
    }

    /// Get while not installed, the other four once it is, none where it is not offered.
    pub fn show(&self, host: &dyn Host) {
        let on = configure(host);
        let installed = on && helper::installed();
        self.row.set_visible(on);
        self.get.set_visible(!installed);
        for w in [&self.sites, &self.page, &self.update, &self.remove] {
            w.set_visible(installed);
        }
    }

    /// Wires the clicks to the window `m`.
    pub fn connect(&self, m: &Rc<Manager>) {
        let w = Rc::downgrade(m);
        self.get.connect_clicked(move |_| {
            if let Some(m) = w.upgrade() {
                glib::spawn_future_local(get(m, false));
            }
        });
        let w = Rc::downgrade(m);
        self.update.connect_clicked(move |_| {
            if let Some(m) = w.upgrade() {
                glib::spawn_future_local(get(m, true));
            }
        });
        let w = Rc::downgrade(m);
        self.sites.connect_clicked(move |_| {
            if let Some(m) = w.upgrade() {
                glib::spawn_future_local(pick_sites(m));
            }
        });
        let w = Rc::downgrade(m);
        self.page.connect_clicked(move |_| {
            if let Some(m) = w.upgrade() {
                glib::spawn_future_local(open_page(m));
            }
        });
        let w = Rc::downgrade(m);
        self.remove.connect_clicked(move |_| {
            if let Some(m) = w.upgrade() {
                glib::spawn_future_local(remove(m));
            }
        });
    }
}

/// The progress window's state, shared with the worker.
#[derive(Default)]
struct Progress {
    got: u64,
    total: Option<u64>,
}

/// Runs `job` on GIO's thread pool behind a modal progress window over `parent` (title,
/// `name` above the bar, Cancel): its result, `None` when cancelled.
pub async fn run_job<T: Send + 'static>(
    parent: &gtk::Window,
    title: &str,
    name: &str,
    job: impl FnOnce(&mut dyn FnMut(u64, Option<u64>) -> bool) -> Result<T, String> + Send + 'static,
) -> Option<Result<T, String>> {
    let window = gtk::Window::builder()
        .title(title)
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .deletable(false)
        .default_width(420)
        .build();
    let label = gtk::Label::new(Some(name));
    label.set_xalign(0.0);
    label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    let bar = gtk::ProgressBar::new();
    bar.set_show_text(true);
    let cancel = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnCancel)));
    cancel.set_halign(gtk::Align::End);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    margins(&body, 12);
    body.append(&label);
    body.append(&bar);
    body.append(&cancel);
    window.set_child(Some(&body));

    let stop = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(Progress::default()));
    let s = stop.clone();
    cancel.connect_clicked(move |b| {
        s.store(true, Ordering::Relaxed);
        b.set_sensitive(false);
    });
    let (p, b) = (progress.clone(), bar.downgrade());
    let timer = glib::timeout_add_local(Duration::from_millis(100), move || {
        let Some(bar) = b.upgrade() else { return glib::ControlFlow::Break };
        let pr = p.lock().map(|g| (g.got, g.total)).unwrap_or((0, None));
        match pr {
            (got, Some(total)) if total > 0 => {
                bar.set_fraction((got as f64 / total as f64).min(1.0));
                bar.set_text(Some(&format!("{} / {}", fmt::size(got), fmt::size(total))));
            }
            _ => {
                bar.pulse();
                bar.set_text(None);
            }
        }
        glib::ControlFlow::Continue
    });
    window.present();
    let (p, s) = (progress.clone(), stop.clone());
    let answer = gio::spawn_blocking(move || {
        job(&mut |got, total| {
            if let Ok(mut g) = p.lock() {
                g.got = got;
                g.total = total;
            }
            !s.load(Ordering::Relaxed)
        })
    })
    .await;
    timer.remove();
    window.destroy();
    if stop.load(Ordering::Relaxed) {
        return None;
    }
    Some(answer.unwrap_or_else(|_| Err("the worker stopped".to_owned())))
}

async fn message(parent: &gtk::Window, text: &str) {
    let d = gtk::AlertDialog::builder().message(text).modal(true).build();
    let _ = d.choose_future(Some(parent)).await;
}

async fn ask(parent: &gtk::Window, text: &str, yes: &str) -> bool {
    let d = gtk::AlertDialog::builder()
        .message(text)
        .buttons([tr!(BtnCancel), yes])
        .cancel_button(0)
        .default_button(1)
        .modal(true)
        .build();
    d.choose_future(Some(parent)).await == Ok(1)
}

fn say(m: &Manager, text: &str) {
    m.status.set_text(text);
    m.status.remove_css_class("error");
}

/// Get (or Update): the newest release, the confirm box, the download, the source; after a
/// first install, the site picker.
async fn get(m: Rc<Manager>, update: bool) {
    let Some(window) = m.window.upgrade() else { return };
    let proxy = m.host.proxy();
    let p = proxy.clone();
    let checked =
        run_job(&window, tr!(SearchHelperChecking), helper::release::PROJECT_URL, move |_| helper::check(p))
            .await;
    let release = match checked {
        None => return,
        Some(Err(e)) => return message(&window, &tr!(SearchHelperDownloadFailed, error = e)).await,
        Some(Ok(r)) => r,
    };
    let size = fmt::size(release.asset.size);
    let current = helper::state().map(|s| s.version).unwrap_or_default();
    let question = if update {
        if current == release.version {
            return message(&window, &tr!(SearchHelperUpToDate, version = current)).await;
        }
        tr!(SearchHelperUpdateConfirm, current = current, version = release.version, size = size)
    } else {
        let folder = helper::layout().map(|l| l.root.display().to_string()).unwrap_or_default();
        tr!(SearchHelperConfirm, version = release.version, size = size, folder = folder)
    };
    if !ask(&window, &question, tr!(BtnOk)).await {
        return;
    }
    let r = release.clone();
    let title = tr!(SearchHelperDownloading, version = release.version);
    let installed =
        run_job(&window, &title, &release.asset.name, move |progress| helper::install(&r, proxy, progress))
            .await;
    let state = match installed {
        None => return,
        Some(Err(e)) => return message(&window, &tr!(SearchHelperDownloadFailed, error = e)).await,
        Some(Ok(s)) => s,
    };
    let sources = helper::with_helper_source(&m.host.sources(), &state);
    super::store(m.host.as_ref(), &sources);
    m.fill(Some(sources.len().saturating_sub(1)));
    m.helper.show(m.host.as_ref());
    say(&m, &tr!(SearchHelperInstalled, version = state.version));
    if !update {
        pick_sites(m).await;
    }
}

/// Remove: the confirm box, the helper stopped and its folder deleted, its source taken out.
async fn remove(m: Rc<Manager>) {
    let Some(window) = m.window.upgrade() else { return };
    let folder = helper::layout().map(|l| l.root.display().to_string()).unwrap_or_default();
    if !ask(&window, &tr!(SearchHelperRemoveConfirm, folder = folder), tr!(SearchHelperRemove)).await {
        return;
    }
    let removed = run_job(&window, tr!(SearchHelperRemove), &folder, |_| helper::remove()).await;
    match removed {
        None => {}
        Some(Err(e)) => message(&window, &e).await,
        Some(Ok(state)) => {
            if let Some(state) = state {
                super::store(m.host.as_ref(), &helper::without_helper_source(&m.host.sources(), &state));
            }
            m.fill(None);
            say(&m, tr!(SearchHelperRemoved));
        }
    }
    m.helper.show(m.host.as_ref());
}

/// Open the Helper's Page: started first, its password on the clipboard and in a box, then
/// the browser.
async fn open_page(m: Rc<Manager>) {
    let Some(window) = m.window.upgrade() else { return };
    match run_job(&window, tr!(SearchHelperStarting), "Jackett", |_| helper::page()).await {
        None => {}
        Some(Err(e)) => message(&window, &tr!(SearchHelperFailed, error = e)).await,
        Some(Ok((url, password))) => {
            if let Some(display) = gdk::Display::default() {
                display.clipboard().set_text(&password);
            }
            message(&window, &tr!(SearchHelperPagePassword, password = password)).await;
            gtk::UriLauncher::new(&url).launch(Some(&window), None::<&gio::Cancellable>, |r| {
                if let Err(e) = r {
                    eprintln!("cannot open the helper's page: {e}");
                }
            });
        }
    }
}

/// Pick Sites: the helper's public sites (it is started first), the picker, then the ticked
/// ones configured and the unticked ones removed.
async fn pick_sites(m: Rc<Manager>) {
    let Some(window) = m.window.upgrade() else { return };
    let list = match run_job(&window, tr!(SearchHelperStarting), "Jackett", |_| helper::list_sites()).await {
        None => return,
        Some(Err(e)) => return message(&window, &tr!(SearchHelperFailed, error = e)).await,
        Some(Ok(l)) => l,
    };
    let Some(ticked) = picker(&window, list.clone()).await else { return };
    let count = ticked.len();
    let saved = run_job(&window, tr!(SearchHelperSaving), "Jackett", move |_| {
        Ok(helper::apply_sites(&list, &ticked))
    })
    .await;
    match saved {
        None => {}
        Some(Ok(Err(errors))) => {
            message(&window, &tr!(SearchHelperSitesFailed, errors = errors.join("\n"))).await;
        }
        Some(Err(e)) => message(&window, &e).await,
        Some(Ok(Ok(()))) => say(&m, &tr!(SearchHelperSitesDone, count = count)),
    }
}

/// The ids a picker's rows start ticked with: only those ticked before (configured), never a
/// FlareSolverr one.
pub fn initially_ticked(list: &[Indexer]) -> Vec<bool> {
    list.iter().map(|i| i.configured && !i.needs_flaresolverr).collect()
}

/// The site picker over `parent`: name, language, type and a note per public site, a check
/// box each (nothing ticked for the user; FlareSolverr ones greyed and not tickable). The ids
/// ticked on OK, `None` on Cancel.
async fn picker(parent: &gtk::Window, list: Vec<Indexer>) -> Option<Vec<String>> {
    let window = gtk::Window::builder()
        .title(tr!(SearchHelperSitesTitle))
        .transient_for(parent)
        .modal(true)
        .default_width(620)
        .default_height(480)
        .build();
    let hint = gtk::Label::new(Some(tr!(SearchHelperSitesHint)));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    let rows = gtk::ListBox::new();
    rows.set_selection_mode(gtk::SelectionMode::None);
    let mut checks = Vec::new();
    for (i, on) in list.iter().zip(initially_ticked(&list)) {
        let check = gtk::CheckButton::new();
        check.set_active(on);
        check.set_sensitive(!i.needs_flaresolverr);
        let name = gtk::Label::new(Some(&i.name));
        name.set_xalign(0.0);
        name.set_hexpand(true);
        name.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let detail = if i.needs_flaresolverr {
            format!("{} · {} · {}", i.language, i.kind, tr!(SearchHelperNeedsFlaresolverr))
        } else {
            format!("{} · {}", i.language, i.kind)
        };
        let info = gtk::Label::new(Some(&detail));
        info.set_xalign(0.0);
        info.add_css_class("dim-label");
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.set_hexpand(true);
        text.append(&name);
        text.append(&info);
        let line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        margins(&line, 4);
        line.append(&check);
        line.append(&text);
        if i.needs_flaresolverr {
            line.set_sensitive(false);
            line.set_tooltip_text(Some(tr!(SearchHelperNeedsFlaresolverr)));
        }
        rows.append(&line);
        checks.push(check);
    }
    let scroll = gtk::ScrolledWindow::builder().child(&rows).vexpand(true).has_frame(true).build();
    let ok = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnOk)));
    ok.add_css_class("suggested-action");
    let cancel = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnCancel)));
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&ok);
    buttons.append(&cancel);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    margins(&body, 12);
    body.append(&hint);
    body.append(&scroll);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&ok));

    let (tx, rx) = futures_channel();
    let tx = Rc::new(std::cell::RefCell::new(Some(tx)));
    let send = {
        let tx = tx.clone();
        move |v: Option<Vec<String>>| {
            if let Some(tx) = tx.borrow_mut().take() {
                let _ = tx.send(v);
            }
        }
    };
    {
        let (send, w, ids) = (
            send.clone(),
            window.downgrade(),
            list.iter().map(|i| (i.id.clone(), i.needs_flaresolverr)).collect::<Vec<_>>(),
        );
        ok.connect_clicked(move |_| {
            let ticked = ids
                .iter()
                .zip(&checks)
                .filter(|((_, cf), c)| !cf && c.is_active())
                .map(|((id, _), _)| id.clone())
                .collect();
            send(Some(ticked));
            if let Some(w) = w.upgrade() {
                w.close();
            }
        });
    }
    {
        let w = window.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(w) = w.upgrade() {
                w.close();
            }
        });
    }
    window.connect_close_request(move |_| {
        send(None);
        glib::Propagation::Proceed
    });
    window.present();
    rx.await.ok().flatten()
}

/// A one-shot channel for a window's answer (glib's oneshot: `futures_channel` is not a
/// dependency of ours).
fn futures_channel<T>() -> (OneshotSender<T>, impl std::future::Future<Output = Result<T, ()>>) {
    let slot: Rc<std::cell::RefCell<(Option<T>, Option<std::task::Waker>)>> = Rc::default();
    let rx_slot = slot.clone();
    let rx = std::future::poll_fn(move |cx| {
        let mut s = rx_slot.borrow_mut();
        match s.0.take() {
            Some(v) => std::task::Poll::Ready(Ok(v)),
            None if Rc::strong_count(&rx_slot) == 1 => std::task::Poll::Ready(Err(())),
            None => {
                s.1 = Some(cx.waker().clone());
                std::task::Poll::Pending
            }
        }
    });
    (OneshotSender(slot), rx)
}

/// The sending half of [`futures_channel`]; dropping it unsent ends the wait with an error.
struct OneshotSender<T>(Rc<std::cell::RefCell<(Option<T>, Option<std::task::Waker>)>>);

impl<T> OneshotSender<T> {
    fn send(self, v: T) -> Result<(), ()> {
        let waker = {
            let mut s = self.0.borrow_mut();
            s.0 = Some(v);
            s.1.take()
        };
        if let Some(w) = waker {
            w.wake();
        }
        Ok(())
    }
}

impl<T> Drop for OneshotSender<T> {
    fn drop(&mut self) {
        let waker = self.0.borrow_mut().1.take();
        if let Some(w) = waker {
            w.wake();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_ticked_for_the_user() {
        let site = |configured, cf| Indexer { configured, needs_flaresolverr: cf, ..Indexer::default() };
        assert_eq!(
            initially_ticked(&[site(false, false), site(true, false), site(true, true)]),
            [false, true, false]
        );
    }
}
