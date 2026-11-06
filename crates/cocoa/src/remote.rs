//! Connect to Remote (GTK `remote.rs`, Win32 `remote_ui`): the window drives a `rookparcel-nox`
//! elsewhere instead of an engine of its own.
//!
//! As in GTK, a remote window is a process of its own: File > Connect to Remote starts this
//! executable again with `--remote` (in this window's profile, whose folder keeps the
//! connections, [`connect`]). That process takes no library lock and listens to no second start
//! (a later local start still hands its torrents to the local window), and several servers can
//! be open at once. It asks where to connect ([`ask`]) before any other window: the host, port,
//! user and HTTPS from `remote.ini` (`remote::Form`; never the password), the plain-HTTP
//! question for another computer, the server certificate's check (`remote::probe`, the
//! fingerprint question, `remote::pin`), then `remote::connect`; the check and the connection
//! block for up to 15 s each, so they run on a thread of their own while the dialog waits,
//! greyed, with a spinner. Closing the dialog ends the process.
//!
//! Then the usual main window runs on the remote session ([`prepare`]: its own folder
//! `remote/<host>_<port>` in the settings folder, the server's settings and default folder,
//! the local language); `Core` gives the title naming the server, and the "disconnected" title
//! and message when the connection is lost (`Events::remote_lost`).
//!
//! Test aids for the VM, where nothing can be typed while its screen is locked (unset in normal
//! use): `ROOKPARCEL_TEST_PASSWORD` is put into the empty password field, and
//! `ROOKPARCEL_TEST_ACCEPT` presses Connect (and OK in the questions), as for the torrent
//! dialogs (`dialogs`).

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSApplication, NSButton, NSProgressIndicator, NSProgressIndicatorStyle, NSSecureTextField, NSTextField,
    NSView, NSWindow,
};
use rookparcel_core::Session;
use rookparcel_core::remote::{self as core_remote, ConnectError, Hello, Target};
use rookparcel_core::tls::Verify;
use rookparcel_frontend::app::{self as fapp, Dirs};
use rookparcel_frontend::options::Options;
use rookparcel_frontend::prefs::Prefs;
use rookparcel_frontend::remote::{self as connect_ui, Failure, Form, Probe, TIMEOUT};
use rookparcel_frontend::{profile, tr};

use crate::dialogs::{self, Observer, cmd};
use crate::paths;
use crate::ui::{self, layout};

const CONNECT: u8 = 1;
const CANCEL: u8 = 2;
/// The window closed.
const CLOSED: u8 = 3;
/// The certificate check finished (on its thread).
const PROBED: u8 = 4;
/// The connection finished (on its thread).
const CONNECTED: u8 = 5;

/// Whether this process is a remote window (`--remote` among its arguments).
pub fn requested(args: &[String]) -> bool {
    connect_ui::requested(args)
}

/// File > Connect to Remote: another process of this profile, in remote mode.
pub fn connect() {
    let mut args = profile::current().args();
    args.push(connect_ui::SWITCH.to_owned());
    if let Err(e) = crate::profiles::launch(&args, "remote") {
        if let Some(mtm) = MainThreadMarker::new() {
            dialogs::tell(None, ui::Style::Error, &tr!(ErrStartRookparcel, error = e), "", mtm);
        }
    }
}

/// A connected server.
pub struct Connected {
    pub session: Session,
    pub hello: Hello,
    /// `host:port`, as the title bar shows it.
    pub label: String,
}

/// The remote window's folders, preferences and options (GTK `build`'s remote part): the
/// server's engine settings and default folder are the window's; the language is the local
/// client's.
pub fn prepare(c: &Connected) -> (Dirs, Prefs, Options) {
    let dirs = paths::dirs().for_remote(&c.label);
    if let Err(e) = rookparcel_core::util::private_file::create_dir_all(&dirs.settings) {
        eprintln!("cannot create {}: {e}", dirs.settings.display());
    }
    let mut prefs = Prefs::load(dirs.settings.join("rookparcel.ini"));
    let base = Prefs::load(dirs.config.join("rookparcel.ini"));
    fapp::adopt_server(&mut prefs, &c.hello, &base);
    let (opts, _) = fapp::prepare_options(&mut prefs, &paths::downloads(), true);
    (dirs, prefs, opts)
}

/// The end of `app::start` in a remote window: the status item and the window's own start
/// (`mac::lifecycle`), not the opening of files and second starts, which are the local
/// window's.
pub fn started(mtm: MainThreadMarker) {
    crate::mac::status_item::start(mtm);
    crate::mac::lifecycle::started(mtm);
}

type Done = Box<dyn FnOnce(Connected)>;

/// Where the connection stands while a thread works on it: the target, its label and whether it
/// is HTTPS.
type Pending = (Target, String, bool);

/// What connecting gave.
type Outcome = Result<(Session, Hello), ConnectError>;

struct Dialog {
    id: u16,
    window: Retained<NSWindow>,
    host: Retained<NSTextField>,
    port: Retained<NSTextField>,
    user: Retained<NSTextField>,
    password: Retained<NSSecureTextField>,
    https: Retained<NSButton>,
    connect: Retained<NSButton>,
    spinner: Retained<NSProgressIndicator>,
    /// `remote.ini`.
    prefs: RefCell<Prefs>,
    done: RefCell<Option<Done>>,
    pending: RefCell<Option<Pending>>,
    probed: Arc<Mutex<Option<Result<Probe, String>>>>,
    result: Arc<Mutex<Option<Outcome>>>,
    /// Connected: the window closes without ending the application.
    finished: Cell<bool>,
    closed: Cell<bool>,
    observer: RefCell<Option<Observer>>,
}

/// Shows the Connect to Remote dialog, the application's only window, and connects when asked
/// until it works (`done` gets the server; the dialog closes after it returns) or the user
/// closes it (the application ends).
pub fn ask(done: impl FnOnce(Connected) + 'static) {
    let Some(mtm) = MainThreadMarker::new() else { return };
    let prefs = Prefs::load(paths::data_dir().join(connect_ui::FILE_NAME));
    let form = Form::load(&prefs);
    let id = dialogs::new_id();
    let host = ui::field(&form.host, "", mtm);
    layout::min_width(&host, 220.0);
    layout::stretch(&host);
    let port = ui::field(&form.port, "", mtm);
    layout::width(&port, 70.0);
    let user = ui::field(&form.user, "", mtm);
    let test_password = std::env::var("ROOKPARCEL_TEST_PASSWORD").unwrap_or_default();
    let password = ui::secure_field(&test_password, mtm);
    let https = ui::check(&ui::text(tr!(RemoteHttps)), form.https, cmd(id, dialogs::NOTHING), mtm);
    let caption = |s: &str| ui::label_tr(s, mtm);
    let (c_host, c_port, c_user, c_password) = (
        caption(tr!(RemoteHost)),
        caption(tr!(RemotePort)),
        caption(tr!(RemoteUser)),
        caption(tr!(RemotePassword)),
    );
    let grid = layout::grid(
        &[
            vec![Some(&*c_host), Some(&*host), Some(&*c_port), Some(&*port)],
            vec![Some(&*c_user), Some(&*user), None, None],
            vec![Some(&*c_password), Some(&*password), None, None],
            vec![None, Some(&*https), None, None],
        ],
        mtm,
    );
    layout::label_column(&grid);
    let note = ui::note(&ui::text(tr!(RemoteNote)), mtm);
    note.setPreferredMaxLayoutWidth(400.0);
    let spinner = NSProgressIndicator::new(mtm);
    spinner.setStyle(NSProgressIndicatorStyle::Spinning);
    spinner.setDisplayedWhenStopped(false);
    ui::auto(&spinner);
    layout::width(&spinner, 16.0);
    layout::height(&spinner, 16.0);
    let cancel = ui::button_tr(tr!(BtnCancel), cmd(id, CANCEL), mtm);
    let connect = ui::button_tr(tr!(RemoteConnect), cmd(id, CONNECT), mtm);
    let buttons = dialogs::buttons(&[&spinner], &cancel, &connect, mtm);
    let views: [&NSView; 3] = [&grid, &note, &buttons];
    let root = dialogs::column(&views, mtm);
    root.setSpacing(14.0);
    let window = dialogs::window(&ui::text(tr!(RemoteTitle)), &root, None, false, mtm);
    // the first empty field gets the focus
    let first: &NSView = if form.host.is_empty() { &host } else { &password };
    dialogs::focus_first(&window, first);
    let d = Rc::new(Dialog {
        id,
        window: window.clone(),
        host,
        port,
        user,
        password,
        https,
        connect,
        spinner,
        prefs: RefCell::new(prefs),
        done: RefCell::new(Some(Box::new(done))),
        pending: RefCell::new(None),
        probed: Arc::new(Mutex::new(None)),
        result: Arc::new(Mutex::new(None)),
        finished: Cell::new(false),
        closed: Cell::new(false),
        observer: RefCell::new(None),
    });
    *d.observer.borrow_mut() = Some(dialogs::on_close(&window, id, CLOSED));
    let handler = d.clone();
    dialogs::set_handler(id, move |act| handler.act(act));
    dialogs::test_accept(id, CONNECT);
    window.center();
    window.makeKeyAndOrderFront(None);
    crate::mac::lifecycle::activate(mtm);
}

impl Dialog {
    fn act(self: &Rc<Self>, act: u8) {
        match act {
            CONNECT => self.start(),
            CANCEL => self.window.close(),
            CLOSED => self.closed(),
            PROBED => self.probed(),
            CONNECTED => self.connected(),
            _ => {}
        }
    }

    fn form(&self) -> Form {
        Form {
            host: ui::value(&self.host).trim().to_owned(),
            port: ui::value(&self.port).trim().to_owned(),
            user: ui::value(&self.user),
            password: ui::value(&self.password),
            https: ui::checked(&self.https),
        }
    }

    /// Greys the fields and spins while a check or the connection runs.
    fn set_busy(&self, busy: bool) {
        self.host.setEnabled(!busy);
        self.port.setEnabled(!busy);
        self.user.setEnabled(!busy);
        self.password.setEnabled(!busy);
        self.https.setEnabled(!busy);
        self.connect.setEnabled(!busy);
        // SAFETY: the sender may be nil; the indicator is on the main thread.
        unsafe {
            if busy {
                self.spinner.startAnimation(None);
            } else {
                self.spinner.stopAnimation(None);
            }
        }
    }

    fn save(&self) {
        if let Err(e) = self.prefs.borrow().save() {
            eprintln!("cannot save {}: {e}", connect_ui::FILE_NAME);
        }
    }

    /// A message over the dialog.
    fn alert(&self, text: &str) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        ui::sheet(
            Some(&self.window),
            ui::Style::Warning,
            &ui::text(tr!(RemoteTitle)),
            text,
            &[],
            |_| {},
            mtm,
        );
    }

    /// Connect: the plain-HTTP question, the certificate check, then the connection.
    fn start(self: &Rc<Self>) {
        if !self.connect.isEnabled() {
            return;
        }
        let form = self.form();
        form.store(&mut self.prefs.borrow_mut());
        self.save();
        let Some((mut target, name)) = form.target() else {
            self.alert(tr!(RemoteBadAddress));
            return;
        };
        self.set_busy(true);
        let https = form.https;
        if form.needs_plaintext_consent(&target) {
            let d = self.clone();
            let text = tr!(RemotePlaintext, name);
            crate::profiles::confirm(
                Some(&self.window),
                ui::Style::Warning,
                &ui::text(tr!(RemoteTitle)),
                &text,
                move |ok| {
                    if !ok {
                        return d.set_busy(false);
                    }
                    target.allow_plaintext = true;
                    d.verify(target, name, https);
                },
            );
            return;
        }
        self.verify(target, name, https);
    }

    /// How to check the server's certificate: one trusted by the system, or one the user
    /// accepted (asked when it is new or has changed, once the check has run on its thread).
    fn verify(self: &Rc<Self>, mut target: Target, name: String, https: bool) {
        if !https {
            return self.connect_to(target, name, https);
        }
        if let Some(pin) = connect_ui::pinned(&self.prefs.borrow(), &name) {
            target.tls = Some(Verify::Pinned(pin));
            return self.connect_to(target, name, https);
        }
        let (t, n, shared, id) = (target.clone(), name.clone(), self.probed.clone(), self.id);
        *self.pending.borrow_mut() = Some((target, name, https));
        std::thread::spawn(move || {
            let r = connect_ui::probe(&t, &n);
            if let Ok(mut s) = shared.lock() {
                *s = Some(r);
            }
            crate::mainq::on_main(move || dialogs::act(id, PROBED));
        });
    }

    /// The certificate check's outcome.
    fn probed(self: &Rc<Self>) {
        let probe = self.probed.lock().ok().and_then(|mut s| s.take());
        let pending = self.pending.borrow_mut().take();
        let (Some(probe), Some((mut target, name, https))) = (probe, pending) else { return };
        if self.closed.get() {
            return;
        }
        match probe {
            Err(text) => {
                self.set_busy(false);
                self.alert(&text);
            }
            Ok(Probe::Trusted) => {
                target.tls = Some(Verify::System);
                self.connect_to(target, name, https);
            }
            Ok(Probe::Unknown(presented)) => {
                let d = self.clone();
                let text = tr!(RemoteTrust, name, fingerprint = presented);
                crate::profiles::confirm(
                    Some(&self.window),
                    ui::Style::Warning,
                    &ui::text(tr!(RemoteTitle)),
                    &text,
                    move |ok| {
                        if !ok {
                            return d.set_busy(false);
                        }
                        connect_ui::pin(&mut d.prefs.borrow_mut(), &name, presented);
                        d.save();
                        target.tls = Some(Verify::Pinned(presented));
                        d.connect_to(target, name, https);
                    },
                );
            }
        }
    }

    /// Connects on a thread of its own.
    fn connect_to(&self, target: Target, name: String, https: bool) {
        let (t, shared, id) = (target.clone(), self.result.clone(), self.id);
        *self.pending.borrow_mut() = Some((target, name, https));
        std::thread::spawn(move || {
            let r = core_remote::connect(&t, TIMEOUT);
            if let Ok(mut s) = shared.lock() {
                *s = Some(r);
            }
            crate::mainq::on_main(move || dialogs::act(id, CONNECTED));
        });
    }

    /// The connection's outcome: the main window on success, else what went wrong.
    fn connected(&self) {
        let result = self.result.lock().ok().and_then(|mut s| s.take());
        let pending = self.pending.borrow_mut().take();
        let (Some(result), Some((_, name, https))) = (result, pending) else { return };
        match result {
            Ok((session, hello)) => {
                let done = self.done.borrow_mut().take();
                // closed while connecting: nobody wants the window any more
                let Some(done) = done.filter(|_| !self.closed.get()) else { return session.shutdown() };
                self.finished.set(true);
                eprintln!("connected to {name} (rookparcel {})", hello.server_version);
                done(Connected { session, hello, label: name });
                self.window.close();
            }
            Err(e) => {
                // a changed certificate is asked about again at the next attempt
                let failure = Failure::new(&e, &name, https);
                failure.apply(&mut self.prefs.borrow_mut(), &name);
                self.save();
                self.set_busy(false);
                self.alert(&failure.text);
            }
        }
    }

    /// The window closed: connected, the main window goes on; otherwise the application ends.
    fn closed(&self) {
        if let Some(o) = self.observer.borrow_mut().take() {
            dialogs::forget(&o);
        }
        dialogs::retire(self.id);
        if self.finished.get() {
            return;
        }
        self.closed.set(true);
        self.done.borrow_mut().take();
        // after the window's close has finished
        crate::mainq::on_main(|| {
            if let Some(mtm) = MainThreadMarker::new() {
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        });
    }
}
