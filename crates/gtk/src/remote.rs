//! Remote mode (PLAN.md Phase 7 slice 5, as the Win32 client's `remote_ui`): the window drives
//! a `patorrent-nox` elsewhere instead of an engine of its own. `patorrent-gtk --remote` (File >
//! Connect to Remote... starts one) asks where to connect, has the user check a self-signed
//! server's certificate, and runs the usual window on a remote session
//! (`patorrent_core::remote`): the same models, dialogs and automations, the server's torrents.
//!
//! A remote window is a process of its own outside the single instance (`NON_UNIQUE`), so a
//! second local start still hands its torrents to the local window, and several servers can be
//! open at once. What it keeps on disk (its settings, labels, RSS feeds) lives in a folder per
//! server, `remote/<host>_<port>` in the settings folder ([`crate::paths::settings_dir`]); the
//! connection details and the accepted certificates are in `remote.ini` beside the local
//! `patorrent.ini`, never the password (`patorrent_frontend::remote`). Folders on this computer
//! mean nothing to the server: the folder pickers for torrents are off (Save In and Set Download
//! Location are typed), and so are Open Containing Folder, seeding a torrent just created here
//! and the local IP filter.
//!
//! Checking a certificate and connecting block for up to 15 s each, so they run on GIO's
//! thread pool while the dialog waits, greyed, with a spinner.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::OnceLock;

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_core::Session;
use patorrent_core::remote::{self, ConnectError, Hello, Target};
use patorrent_core::tls::Verify;
use patorrent_frontend::lang::gtk_mnemonic;
use patorrent_frontend::prefs::Prefs;
use patorrent_frontend::remote::{self as connect, Failure, Form, Probe, TIMEOUT};
use patorrent_frontend::tr;

use crate::paths;

/// The command-line switch that makes a remote window.
pub const SWITCH: &str = "--remote";

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

/// Started with `--remote`: this process is a remote window.
pub fn requested() -> bool {
    std::env::args_os().skip(1).any(|a| a == SWITCH)
}

/// Given the session once connected; the dialog closes after it returns, so the application
/// always has a window.
type Connected = Box<dyn FnOnce(Session)>;

/// What the dialog's handlers share. It holds no window, so the widgets' closures can keep it
/// without a cycle; the window's close handler keeps it alive.
struct Dialog {
    window: glib::WeakRef<gtk::Window>,
    host: gtk::Entry,
    port: gtk::Entry,
    user: gtk::Entry,
    password: gtk::PasswordEntry,
    https: gtk::CheckButton,
    connect: gtk::Button,
    spinner: gtk::Spinner,
    /// `remote.ini`.
    prefs: RefCell<Prefs>,
    connected: RefCell<Option<Connected>>,
}

/// Shows the Connect to Remote dialog, a window of `app`'s, and connects when asked until it
/// works (`connected` gets the session) or the user closes it (and the application quits, with
/// no other window).
pub fn connect(app: &gtk::Application, connected: impl FnOnce(Session) + 'static) {
    let prefs = Prefs::load(paths::config_dir().join(connect::FILE_NAME));
    let form = Form::load(&prefs);
    let window = gtk::Window::builder()
        .application(app)
        .title(tr!(RemoteTitle))
        .default_width(420)
        .resizable(false)
        .build();
    let entry = |text: &str| {
        let e = gtk::Entry::new();
        e.set_text(text);
        e.set_activates_default(true);
        e
    };
    let password = gtk::PasswordEntry::new();
    password.set_show_peek_icon(true);
    password.set_activates_default(true);
    let d = Rc::new(Dialog {
        window: window.downgrade(),
        host: entry(&form.host),
        port: entry(&form.port),
        user: entry(&form.user),
        password,
        https: gtk::CheckButton::with_mnemonic(&gtk_mnemonic(tr!(RemoteHttps))),
        connect: gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(RemoteConnect))),
        spinner: gtk::Spinner::new(),
        prefs: RefCell::new(prefs),
        connected: RefCell::new(Some(Box::new(connected))),
    });
    d.host.set_hexpand(true);
    d.port.set_width_chars(6);
    d.port.set_max_width_chars(6);
    d.port.set_input_purpose(gtk::InputPurpose::Digits);
    d.https.set_active(form.https);

    let grid = gtk::Grid::new();
    grid.set_row_spacing(6);
    grid.set_column_spacing(8);
    let caption = |text: &str, field: &gtk::Widget| {
        let l = gtk::Label::with_mnemonic(&gtk_mnemonic(text));
        l.set_xalign(0.0);
        l.set_mnemonic_widget(Some(field));
        l
    };
    grid.attach(&caption(tr!(RemoteHost), d.host.upcast_ref()), 0, 0, 1, 1);
    grid.attach(&d.host, 1, 0, 1, 1);
    grid.attach(&caption(tr!(RemotePort), d.port.upcast_ref()), 2, 0, 1, 1);
    grid.attach(&d.port, 3, 0, 1, 1);
    grid.attach(&caption(tr!(RemoteUser), d.user.upcast_ref()), 0, 1, 1, 1);
    grid.attach(&d.user, 1, 1, 1, 1);
    grid.attach(&caption(tr!(RemotePassword), d.password.upcast_ref()), 0, 2, 1, 1);
    grid.attach(&d.password, 1, 2, 1, 1);
    grid.attach(&d.https, 1, 3, 3, 1);
    let note = gtk::Label::new(Some(tr!(RemoteNote)));
    note.set_xalign(0.0);
    note.set_wrap(true);
    note.set_max_width_chars(50);
    note.add_css_class("dim-label");

    let cancel = gtk::Button::with_mnemonic(&gtk_mnemonic(tr!(BtnCancel)));
    d.connect.add_css_class("suggested-action");
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    buttons.append(&d.spinner);
    buttons.append(&cancel);
    buttons.append(&d.connect);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 10);
    body.set_margin_top(12);
    body.set_margin_bottom(12);
    body.set_margin_start(12);
    body.set_margin_end(12);
    body.append(&grid);
    body.append(&note);
    body.append(&buttons);
    window.set_child(Some(&body));
    window.set_default_widget(Some(&d.connect));
    let keys = gtk::ShortcutController::new();
    keys.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("Escape"),
        Some(gtk::NamedAction::new("window.close")),
    ));
    window.add_controller(keys);

    let w = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(w) = w.upgrade() {
            w.close();
        }
    });
    let weak = Rc::downgrade(&d);
    d.connect.connect_clicked(move |_| {
        if let Some(d) = weak.upgrade() {
            d.start();
        }
    });
    // the handler keeps the shared state as long as the window is open
    let keep = d.clone();
    window.connect_close_request(move |_| {
        keep.connected.borrow_mut().take();
        glib::Propagation::Proceed
    });
    window.present();
    // the first empty field gets the focus
    let first: &gtk::Widget =
        if form.host.is_empty() { d.host.upcast_ref() } else { d.password.upcast_ref() };
    first.grab_focus();
}

impl Dialog {
    fn form(&self) -> Form {
        Form {
            host: self.host.text().trim().to_owned(),
            port: self.port.text().trim().to_owned(),
            user: self.user.text().to_string(),
            password: self.password.text().to_string(),
            https: self.https.is_active(),
        }
    }

    /// Greys the fields and spins while a check or the connection runs.
    fn set_busy(&self, busy: bool) {
        for w in [
            self.host.upcast_ref::<gtk::Widget>(),
            self.port.upcast_ref(),
            self.user.upcast_ref(),
            self.password.upcast_ref(),
            self.https.upcast_ref(),
            self.connect.upcast_ref(),
        ] {
            w.set_sensitive(!busy);
        }
        self.spinner.set_spinning(busy);
    }

    fn save(&self) {
        if let Err(e) = self.prefs.borrow().save() {
            eprintln!("cannot save {}: {e}", connect::FILE_NAME);
        }
    }

    /// A message over the dialog.
    fn alert(&self, text: &str) {
        let Some(window) = self.window.upgrade() else { return };
        gtk::AlertDialog::builder()
            .message(tr!(RemoteTitle))
            .detail(text)
            .modal(true)
            .build()
            .show(Some(&window));
    }

    /// Connect: the certificate check, then the connection, off the main loop.
    fn start(self: &Rc<Self>) {
        let form = self.form();
        form.store(&mut self.prefs.borrow_mut());
        self.save();
        let Some((target, name)) = form.target() else {
            self.alert(tr!(RemoteBadAddress));
            return;
        };
        self.set_busy(true);
        let d = self.clone();
        glib::spawn_future_local(async move {
            let mut target = target;
            if form.https {
                match d.verify(&target, &name).await {
                    Some(v) => target.tls = Some(v),
                    None => return d.set_busy(false),
                }
            }
            let t = target.clone();
            match gio::spawn_blocking(move || remote::connect(&t, TIMEOUT)).await {
                Ok(result) => d.finish(result, name, form.https),
                // a panic on the pool's thread: the connection is not there
                Err(_) => d.set_busy(false),
            }
        });
    }

    /// How to check the server's certificate: one trusted by the system, or one the user
    /// accepted (asked here when it is new or has changed). `None` when the user declined or
    /// the server cannot be reached over HTTPS.
    async fn verify(&self, target: &Target, name: &str) -> Option<Verify> {
        if let Some(pin) = connect::pinned(&self.prefs.borrow(), name) {
            return Some(Verify::Pinned(pin));
        }
        let (t, n) = (target.clone(), name.to_owned());
        match gio::spawn_blocking(move || connect::probe(&t, &n)).await.ok()? {
            Err(text) => {
                self.alert(&text);
                None
            }
            Ok(Probe::Trusted) => Some(Verify::System),
            Ok(Probe::Unknown(presented)) => {
                let window = self.window.upgrade()?;
                let ask = gtk::AlertDialog::builder()
                    .message(tr!(RemoteTitle))
                    .detail(tr!(RemoteTrust, name, fingerprint = presented))
                    .buttons([tr!(BtnCancel), tr!(BtnOk)])
                    .cancel_button(0)
                    .default_button(0)
                    .modal(true)
                    .build();
                if ask.choose_future(Some(&window)).await != Ok(1) {
                    return None;
                }
                connect::pin(&mut self.prefs.borrow_mut(), name, presented);
                self.save();
                Some(Verify::Pinned(presented))
            }
        }
    }

    /// The connection's outcome: the main window on success, else what went wrong.
    fn finish(&self, result: Result<(Session, Hello), ConnectError>, name: String, https: bool) {
        match result {
            Ok((session, hello)) => {
                // closed while connecting: nobody wants the window any more
                let (Some(window), Some(connected)) =
                    (self.window.upgrade(), self.connected.borrow_mut().take())
                else {
                    return session.shutdown();
                };
                let _ = REMOTE.set(Remote { label: name, hello });
                connected(session);
                window.close();
            }
            Err(e) => {
                // a changed certificate is asked about again at the next attempt, after the
                // user has had a look
                let failure = Failure::new(&e, &name, https);
                failure.apply(&mut self.prefs.borrow_mut(), &name);
                self.save();
                self.set_busy(false);
                self.alert(&failure.text);
            }
        }
    }
}

/// Starts another PaTorrent window in remote mode (File > Connect to Remote...): this executable
/// again (inside a Flatpak's sandbox too), or the AppImage file, whose mount goes away with this
/// process.
pub fn start_remote_window(parent: &impl IsA<gtk::Window>) {
    let program = std::env::var_os("APPIMAGE").map(PathBuf::from).or_else(|| std::env::current_exe().ok());
    let started = match program {
        Some(p) => std::process::Command::new(p).arg(SWITCH).spawn(),
        None => Err(std::io::Error::other("the program's path is not known")),
    };
    match started {
        // reaped when it ends, so it leaves no zombie behind
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(e) => gtk::AlertDialog::builder()
            .message(tr!(ErrStartPaTorrent, error = e))
            .modal(true)
            .build()
            .show(Some(parent)),
    }
}
