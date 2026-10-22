//! The tray icon, as a StatusNotifierItem (the freedesktop.org / KDE tray protocol that KDE
//! Plasma, Xfce, LXQt, Cinnamon, MATE, waybar and GNOME with the AppIndicator extension show)
//! over GDBus, with its menu as `com.canonical.dbusmenu`: no library beyond GIO.
//!
//! The item lives at `/StatusNotifierItem` under the bus name
//! `org.kde.StatusNotifierItem-<pid>-1` and registers with `org.kde.StatusNotifierWatcher`
//! whenever a watcher is on the bus; while none is, there is no tray ([`Tray::shown`] is false)
//! and the window does not hide into it. Inside a Flatpak (`FLATPAK_ID` set), or when the bus
//! will not let it own that name, it registers the connection's unique name (`:1.42`) instead,
//! as Qt, KDE Frameworks and Electron do: the sandbox refuses owning `org.kde.*` names unless
//! the manifest grants a wildcard Flathub rejects ([`item_name`]). The icon is the application icon: by name
//! (`IconName`, with `IconThemePath` the folder `icons::setup` wrote it to) and as pixmaps for
//! hosts that draw those (`icons::app_pixmaps`). The tooltip shows the rates, a left click
//! shows or hides the window,
//! and the menu is the Win32 tray's: Show / Hide, Add Torrent, Add Torrent from URL, Start All,
//! Stop All, Exit.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use gtk::{gio, glib};
use patorrent_frontend::lang::{self, gtk_mnemonic};
use patorrent_frontend::tr;

use crate::app::APP_ID;

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/MenuBar";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const MENU_IFACE: &str = "com.canonical.dbusmenu";
const WATCHER: &str = "org.kde.StatusNotifierWatcher";

/// How owning the well-known item name went.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owning {
    /// Asked for, no answer yet.
    Pending,
    Owned,
    /// Refused (a Flatpak without `--own-name`, the name taken) or lost.
    Refused,
}

/// The bus name to register with the watcher: the unique name in a Flatpak or once the
/// well-known name was refused, the well-known one while owned, nothing while that is pending.
fn item_name<'a>(
    flatpak: bool,
    owning: Owning,
    well_known: &'a str,
    unique: Option<&'a str>,
) -> Option<&'a str> {
    match (flatpak, owning) {
        (true, _) | (false, Owning::Refused) => unique,
        (false, Owning::Owned) => Some(well_known),
        (false, Owning::Pending) => None,
    }
}

const XML: &str = r#"<node>
 <interface name="org.kde.StatusNotifierItem">
  <property name="Category" type="s" access="read"/>
  <property name="Id" type="s" access="read"/>
  <property name="Title" type="s" access="read"/>
  <property name="Status" type="s" access="read"/>
  <property name="WindowId" type="i" access="read"/>
  <property name="IconThemePath" type="s" access="read"/>
  <property name="IconName" type="s" access="read"/>
  <property name="IconPixmap" type="a(iiay)" access="read"/>
  <property name="OverlayIconName" type="s" access="read"/>
  <property name="OverlayIconPixmap" type="a(iiay)" access="read"/>
  <property name="AttentionIconName" type="s" access="read"/>
  <property name="AttentionIconPixmap" type="a(iiay)" access="read"/>
  <property name="AttentionMovieName" type="s" access="read"/>
  <property name="ToolTip" type="(sa(iiay)ss)" access="read"/>
  <property name="ItemIsMenu" type="b" access="read"/>
  <property name="Menu" type="o" access="read"/>
  <method name="ContextMenu"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
  <method name="Activate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
  <method name="SecondaryActivate"><arg name="x" type="i" direction="in"/><arg name="y" type="i" direction="in"/></method>
  <method name="Scroll"><arg name="delta" type="i" direction="in"/><arg name="orientation" type="s" direction="in"/></method>
  <signal name="NewTitle"/>
  <signal name="NewIcon"/>
  <signal name="NewAttentionIcon"/>
  <signal name="NewOverlayIcon"/>
  <signal name="NewToolTip"/>
  <signal name="NewStatus"><arg name="status" type="s"/></signal>
 </interface>
 <interface name="com.canonical.dbusmenu">
  <property name="Version" type="u" access="read"/>
  <property name="TextDirection" type="s" access="read"/>
  <property name="Status" type="s" access="read"/>
  <property name="IconThemePath" type="as" access="read"/>
  <method name="GetLayout">
   <arg name="parentId" type="i" direction="in"/>
   <arg name="recursionDepth" type="i" direction="in"/>
   <arg name="propertyNames" type="as" direction="in"/>
   <arg name="revision" type="u" direction="out"/>
   <arg name="layout" type="(ia{sv}av)" direction="out"/>
  </method>
  <method name="GetGroupProperties">
   <arg name="ids" type="ai" direction="in"/>
   <arg name="propertyNames" type="as" direction="in"/>
   <arg name="properties" type="a(ia{sv})" direction="out"/>
  </method>
  <method name="GetProperty">
   <arg name="id" type="i" direction="in"/>
   <arg name="name" type="s" direction="in"/>
   <arg name="value" type="v" direction="out"/>
  </method>
  <method name="Event">
   <arg name="id" type="i" direction="in"/>
   <arg name="eventId" type="s" direction="in"/>
   <arg name="data" type="v" direction="in"/>
   <arg name="timestamp" type="u" direction="in"/>
  </method>
  <method name="EventGroup">
   <arg name="events" type="a(isvu)" direction="in"/>
   <arg name="idErrors" type="ai" direction="out"/>
  </method>
  <method name="AboutToShow">
   <arg name="id" type="i" direction="in"/>
   <arg name="needUpdate" type="b" direction="out"/>
  </method>
  <method name="AboutToShowGroup">
   <arg name="ids" type="ai" direction="in"/>
   <arg name="updatesNeeded" type="ai" direction="out"/>
   <arg name="idErrors" type="ai" direction="out"/>
  </method>
  <signal name="ItemsPropertiesUpdated">
   <arg name="updatedProps" type="a(ia{sv})"/>
   <arg name="removedProps" type="a(ias)"/>
  </signal>
  <signal name="LayoutUpdated"><arg name="revision" type="u"/><arg name="parent" type="i"/></signal>
  <signal name="ItemActivationRequested"><arg name="id" type="i"/><arg name="timestamp" type="u"/></signal>
 </interface>
</node>"#;

/// A menu entry the owner acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Show the window, or hide it into the tray.
    Toggle,
    AddTorrent,
    AddUrl,
    StartAll,
    StopAll,
    Exit,
}

/// The menu: item ids from 1 (0 is the root), `None` a separator.
const MENU: [Option<Command>; 9] = [
    Some(Command::Toggle),
    None,
    Some(Command::AddTorrent),
    Some(Command::AddUrl),
    None,
    Some(Command::StartAll),
    Some(Command::StopAll),
    None,
    Some(Command::Exit),
];

/// What the tray asks of its owner.
pub struct Hooks {
    /// A left click on the icon.
    pub activate: Box<dyn Fn()>,
    pub command: Box<dyn Fn(Command)>,
    /// Whether the window is on screen (the menu says Hide then).
    pub window_visible: Box<dyn Fn() -> bool>,
    /// The tray came (true) or went (false): a watcher took the item or left the bus.
    pub shown: Box<dyn Fn(bool)>,
}

pub struct Tray {
    connection: gio::DBusConnection,
    /// The well-known name, `org.kde.StatusNotifierItem-<pid>-1`.
    name: String,
    /// In a Flatpak: the well-known name is not asked for at all.
    flatpak: bool,
    hooks: Hooks,
    tooltip: RefCell<(String, String)>,
    pixmaps: Vec<(i32, i32, Vec<u8>)>,
    revision: Cell<u32>,
    owning: Cell<Owning>,
    watcher: Cell<bool>,
    shown: Cell<bool>,
}

impl Tray {
    /// Puts the item on the session bus; `None` without one.
    pub fn start(hooks: Hooks) -> Option<Rc<Tray>> {
        let connection = gio::bus_get_sync(gio::BusType::Session, None::<&gio::Cancellable>).ok()?;
        let node = gio::DBusNodeInfo::for_xml(XML).ok()?;
        let (item, menu) = (node.lookup_interface(ITEM_IFACE)?, node.lookup_interface(MENU_IFACE)?);
        let tray = Rc::new(Tray {
            name: format!("org.kde.StatusNotifierItem-{}-1", std::process::id()),
            connection: connection.clone(),
            flatpak: std::env::var_os("FLATPAK_ID").is_some(),
            hooks,
            tooltip: RefCell::new(("PaTorrent".into(), String::new())),
            pixmaps: crate::icons::app_pixmaps(),
            revision: Cell::new(1),
            owning: Cell::new(Owning::Pending),
            watcher: Cell::new(false),
            shown: Cell::new(false),
        });
        for (path, info) in [(ITEM_PATH, &item), (MENU_PATH, &menu)] {
            let (calls, props) = (Rc::downgrade(&tray), Rc::downgrade(&tray));
            let registered = connection
                .register_object(path, info)
                .method_call(move |_, _, _, iface, method, params, invocation| match calls.upgrade() {
                    Some(t) => t.call(iface.unwrap_or_default(), method, &params, invocation),
                    None => invocation.return_value(None),
                })
                .property(move |_, _, _, iface, name| {
                    props.upgrade().map_or_else(|| "".to_variant(), |t| t.property(iface, name))
                })
                .build();
            if let Err(e) = registered {
                eprintln!("tray: cannot export {path}: {e}");
                return None;
            }
        }
        if !tray.flatpak {
            let (got, lost) = (Rc::downgrade(&tray), Rc::downgrade(&tray));
            // not queued: a refusal falls back to the unique name for good, so the item is
            // never registered under two names
            gio::bus_own_name_on_connection(
                &connection,
                &tray.name,
                gio::BusNameOwnerFlags::DO_NOT_QUEUE,
                move |_, _| {
                    if let Some(t) = got.upgrade() {
                        if t.owning.get() == Owning::Pending {
                            t.owning.set(Owning::Owned);
                            t.register();
                        }
                    }
                },
                move |_, _| {
                    if let Some(t) = lost.upgrade() {
                        if t.owning.replace(Owning::Refused) != Owning::Refused {
                            eprintln!("tray: cannot own {}; registering the unique bus name", t.name);
                            t.set_shown(false);
                            t.register();
                        }
                    }
                },
            );
        }
        let (came, went) = (Rc::downgrade(&tray), Rc::downgrade(&tray));
        gio::bus_watch_name_on_connection(
            &connection,
            WATCHER,
            gio::BusNameWatcherFlags::NONE,
            move |_, _, _| {
                if let Some(t) = came.upgrade() {
                    t.watcher.set(true);
                    t.register();
                }
            },
            move |_, _| {
                if let Some(t) = went.upgrade() {
                    t.watcher.set(false);
                    t.set_shown(false);
                }
            },
        );
        Some(tray)
    }

    /// A watcher has the item: the tray is on screen (as far as the desktop shows one).
    pub fn shown(&self) -> bool {
        self.shown.get()
    }

    fn set_shown(&self, on: bool) {
        if self.shown.replace(on) != on {
            (self.hooks.shown)(on);
        }
    }

    fn register(self: &Rc<Self>) {
        if !self.watcher.get() {
            return;
        }
        let unique = self.connection.unique_name();
        let Some(name) = item_name(self.flatpak, self.owning.get(), &self.name, unique.as_deref()) else {
            return;
        };
        let me = Rc::downgrade(self);
        self.connection.call(
            Some(WATCHER),
            "/StatusNotifierWatcher",
            WATCHER,
            "RegisterStatusNotifierItem",
            Some(&(name,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            -1,
            None::<&gio::Cancellable>,
            move |r| {
                if let Some(t) = me.upgrade() {
                    if let Err(e) = &r {
                        eprintln!("tray: the watcher refused the item: {e}");
                    }
                    t.set_shown(r.is_ok());
                }
            },
        );
    }

    /// The tooltip: its first line is the title, the rest the text under it.
    pub fn set_tooltip(&self, text: &str) {
        let (title, body) = text.split_once('\n').unwrap_or((text, ""));
        let new = (title.to_owned(), body.to_owned());
        if *self.tooltip.borrow() != new {
            *self.tooltip.borrow_mut() = new;
            self.emit(ITEM_PATH, ITEM_IFACE, "NewToolTip", None);
        }
    }

    /// The window was shown or hidden: the menu's first entry follows.
    pub fn menu_changed(&self) {
        let rev = self.revision.get() + 1;
        self.revision.set(rev);
        self.emit(MENU_PATH, MENU_IFACE, "LayoutUpdated", Some(&(rev, 0i32).to_variant()));
    }

    fn emit(&self, path: &str, iface: &str, signal: &str, args: Option<&glib::Variant>) {
        if let Err(e) = self.connection.emit_signal(None, path, iface, signal, args) {
            eprintln!("tray: {signal}: {e}");
        }
    }

    fn property(&self, iface: &str, name: &str) -> glib::Variant {
        if iface == MENU_IFACE {
            return match name {
                "Version" => 3u32.to_variant(),
                "TextDirection" => if lang::right_to_left() { "rtl" } else { "ltr" }.to_variant(),
                "Status" => "normal".to_variant(),
                _ => Vec::<String>::new().to_variant(),
            };
        }
        let none: Vec<(i32, i32, Vec<u8>)> = Vec::new();
        match name {
            "Category" => "ApplicationStatus".to_variant(),
            "Id" => APP_ID.to_variant(),
            "Title" => "PaTorrent".to_variant(),
            "Status" => "Active".to_variant(),
            "WindowId" => 0i32.to_variant(),
            "IconPixmap" => self.pixmaps.to_variant(),
            "OverlayIconPixmap" | "AttentionIconPixmap" => none.to_variant(),
            "ToolTip" => {
                let (title, body) = self.tooltip.borrow().clone();
                ("", none, title, body).to_variant()
            }
            "ItemIsMenu" => false.to_variant(),
            "Menu" => glib::variant::ObjectPath::try_from(MENU_PATH).expect("a valid path").to_variant(),
            "IconName" if crate::icons::theme_dir().is_some() => APP_ID.to_variant(),
            "IconThemePath" => crate::icons::theme_dir()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default()
                .to_variant(),
            // the overlay and attention names: none
            _ => "".to_variant(),
        }
    }

    fn call(&self, iface: &str, method: &str, params: &glib::Variant, invocation: gio::DBusMethodInvocation) {
        // the reply goes first; what the owner does with a click comes after it
        if iface == ITEM_IFACE {
            invocation.return_value(None);
            if matches!(method, "Activate" | "SecondaryActivate") {
                (self.hooks.activate)();
            }
            return;
        }
        let mut clicked = Vec::new();
        let reply = match method {
            "GetLayout" => {
                let (parent, _depth, _names) =
                    params.get::<(i32, i32, Vec<String>)>().unwrap_or((0, -1, Vec::new()));
                let layout = if parent == 0 { self.root() } else { self.node(parent) };
                Some(glib::Variant::tuple_from_iter([self.revision.get().to_variant(), layout]))
            }
            "GetGroupProperties" => {
                let (ids, _names) = params.get::<(Vec<i32>, Vec<String>)>().unwrap_or_default();
                let ids: Vec<i32> = if ids.is_empty() { (1..=MENU.len() as i32).collect() } else { ids };
                let items = ids
                    .into_iter()
                    .filter(|&id| id >= 0 && id as usize <= MENU.len())
                    .map(|id| glib::Variant::tuple_from_iter([id.to_variant(), self.props(id).to_variant()]));
                let ty = glib::VariantTy::new("(ia{sv})").expect("a valid type");
                Some(glib::Variant::tuple_from_iter([glib::Variant::array_from_iter_with_type(ty, items)]))
            }
            "GetProperty" => {
                let (id, name) = params.get::<(i32, String)>().unwrap_or_default();
                let value = self.props(id).remove(&name).unwrap_or_else(|| "".to_variant());
                Some(glib::Variant::tuple_from_iter([glib::Variant::from_variant(&value)]))
            }
            "Event" => {
                if let Some((id, event, _, _)) = params.get::<(i32, String, glib::Variant, u32)>() {
                    clicked.push((id, event));
                }
                None
            }
            "EventGroup" => {
                let events = params.get::<(Vec<(i32, String, glib::Variant, u32)>,)>().map(|e| e.0);
                clicked.extend(events.unwrap_or_default().into_iter().map(|(id, event, _, _)| (id, event)));
                Some((Vec::<i32>::new(),).to_variant())
            }
            "AboutToShow" => Some((false,).to_variant()),
            "AboutToShowGroup" => Some((Vec::<i32>::new(), Vec::<i32>::new()).to_variant()),
            _ => None,
        };
        invocation.return_value(reply.as_ref());
        for (id, event) in clicked {
            self.event(id, &event);
        }
    }

    /// A clicked entry's command.
    fn event(&self, id: i32, event: &str) {
        if event != "clicked" {
            return;
        }
        let Some(Some(command)) = usize::try_from(id - 1).ok().and_then(|i| MENU.get(i)).copied() else {
            return;
        };
        (self.hooks.command)(command);
    }

    /// The properties of entry `id` (0: the root).
    fn props(&self, id: i32) -> HashMap<String, glib::Variant> {
        let mut p = HashMap::new();
        if id == 0 {
            p.insert("children-display".to_owned(), "submenu".to_variant());
            return p;
        }
        match usize::try_from(id - 1).ok().and_then(|i| MENU.get(i)).copied().flatten() {
            None => {
                p.insert("type".to_owned(), "separator".to_variant());
            }
            Some(c) => {
                let text = match c {
                    Command::Toggle if (self.hooks.window_visible)() => tr!(MenuHide),
                    Command::Toggle => tr!(MenuShow),
                    Command::AddTorrent => tr!(MenuAddTorrent),
                    Command::AddUrl => tr!(MenuAddUrl),
                    Command::StartAll => tr!(MenuStartAll),
                    Command::StopAll => tr!(MenuStopAll),
                    Command::Exit => tr!(MenuExit),
                };
                p.insert("label".to_owned(), gtk_mnemonic(text).to_variant());
                p.insert("enabled".to_owned(), true.to_variant());
                p.insert("visible".to_owned(), true.to_variant());
            }
        }
        p
    }

    /// `(ia{sv}av)` for entry `id`, without children.
    fn node(&self, id: i32) -> glib::Variant {
        let children = glib::Variant::array_from_iter_with_type(
            glib::VariantTy::VARIANT,
            std::iter::empty::<glib::Variant>(),
        );
        glib::Variant::tuple_from_iter([id.to_variant(), self.props(id).to_variant(), children])
    }

    fn root(&self) -> glib::Variant {
        let children = (1..=MENU.len() as i32).map(|id| glib::Variant::from_variant(&self.node(id)));
        let children = glib::Variant::array_from_iter_with_type(glib::VariantTy::VARIANT, children);
        glib::Variant::tuple_from_iter([0i32.to_variant(), self.props(0).to_variant(), children])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registered_name() {
        let (wk, unique) = ("org.kde.StatusNotifierItem-7-1", Some(":1.42"));
        // in a Flatpak the unique name, whatever owning did (it is never asked for)
        for owning in [Owning::Pending, Owning::Owned, Owning::Refused] {
            assert_eq!(item_name(true, owning, wk, unique), Some(":1.42"));
        }
        assert_eq!(item_name(false, Owning::Pending, wk, unique), None);
        assert_eq!(item_name(false, Owning::Owned, wk, unique), Some(wk));
        assert_eq!(item_name(false, Owning::Refused, wk, unique), Some(":1.42"));
        // a connection without a unique name (not a message bus): nothing to register
        assert_eq!(item_name(true, Owning::Pending, wk, None), None);
    }
}
