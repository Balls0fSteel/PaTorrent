//! The PatinaTorrent icons in GTK, from the PNGs `patorrent_frontend::icons` embeds (GDK
//! decodes them):
//!
//! - the application icon is a themed icon named [`APP_ID`], as the desktop entries name it.
//!   [`setup`] writes it as a small icon theme folder under `$XDG_CACHE_HOME/patorrent/icons`
//!   and adds that folder to the display's icon theme, so the windows, the About box and the
//!   tray (its `IconThemePath`) find it whether or not it is installed; [`install_theme_icons`]
//!   also puts it into `$XDG_DATA_HOME/icons/hicolor` when desktop integration writes the
//!   desktop entries (`integration`). The AppImage and the Flatpak bring their own copy.
//! - the tray's pixmaps ([`app_pixmaps`]), for hosts that draw the item's pixels.
//! - the state icons of the torrent list, the Trackers tab and the status bar
//!   ([`state_texture`]), the toolbar images ([`toolbar_image`]) and the symbols of the category
//!   list, the detail tabs and the Preferences pages ([`ui_image`]): the artwork's light
//!   palette, or its dark one while the GTK theme is dark ([`setup`] follows the theme). The
//!   menus stay text only: GTK 4's popover menus show an icon only in a row of buttons.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use gtk::prelude::*;
use gtk::{gdk, glib};
use patorrent_frontend::icons::{self, APP_SIZES, Icon, StateIcon, UiIcon};

use crate::app::APP_ID;

/// The state icons' size in the list and the status bar, in logical pixels.
pub const STATE_PX: i32 = 16;

thread_local! {
    static TEXTURES: RefCell<HashMap<StateIcon, gdk::Texture>> = RefCell::default();
    /// Whether the GTK theme is dark: the icons use the artwork's dark palette.
    static DARK: Cell<bool> = const { Cell::new(false) };
    /// The toolbar's and the navigation's images, to repaint when the theme turns dark or light.
    static THEMED: RefCell<Vec<(Art, glib::WeakRef<gtk::Image>)>> = RefCell::default();
    /// The folder [`setup`] wrote the application icon into.
    static THEME_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

fn texture(png: &'static [u8]) -> Option<gdk::Texture> {
    gdk::Texture::from_bytes(&glib::Bytes::from_static(png)).ok()
}

/// A state icon, decoded once per theme. It is the 32 px export, which GTK draws at
/// [`STATE_PX`] logical pixels: sharp at a scale of 2.
pub fn state_texture(icon: StateIcon) -> Option<gdk::Texture> {
    TEXTURES.with(|t| {
        if let Some(tex) = t.borrow().get(&icon) {
            return Some(tex.clone());
        }
        let tex = texture(icon.themed_png(32, DARK.get()))?;
        t.borrow_mut().insert(icon, tex.clone());
        Some(tex)
    })
}

/// An image for a state icon, at [`STATE_PX`].
pub fn state_image() -> gtk::Image {
    let image = gtk::Image::new();
    image.set_pixel_size(STATE_PX);
    image
}

/// Shows `icon` in `image` (nothing when it already does).
pub fn set_state(image: &gtk::Image, icon: StateIcon) {
    let Some(tex) = state_texture(icon) else { return };
    let shown = image.paintable().and_downcast::<gdk::Texture>();
    if shown.as_ref() != Some(&tex) {
        image.set_paintable(Some(&tex));
    }
}

/// What a theme-following image shows.
#[derive(Clone, Copy)]
enum Art {
    Toolbar(Icon),
    Ui(UiIcon),
}

impl Art {
    /// The 32 px export, in the dark palette with `dark`.
    fn png(self, dark: bool) -> &'static [u8] {
        match self {
            Art::Toolbar(icon) => icon.png(32, dark),
            Art::Ui(icon) => icon.themed_png(32, dark),
        }
    }
}

/// `art` drawn at [`STATE_PX`] logical pixels, as the state icons are, in the theme's palette
/// (kept so when the theme changes).
fn themed_image(art: Art) -> gtk::Image {
    let image = gtk::Image::new();
    image.set_pixel_size(STATE_PX);
    if let Some(tex) = texture(art.png(DARK.get())) {
        image.set_paintable(Some(&tex));
    }
    THEMED.with(|t| t.borrow_mut().push((art, image.downgrade())));
    image
}

/// A toolbar button's image.
pub fn toolbar_image(icon: Icon) -> gtk::Image {
    themed_image(Art::Toolbar(icon))
}

/// A navigation symbol's image: a category list row's, a detail tab's, a Preferences page's.
pub fn ui_image(icon: UiIcon) -> gtk::Image {
    themed_image(Art::Ui(icon))
}

/// `label` after `icon`'s image: a navigation row's or a tab's content.
pub fn with_icon(icon: UiIcon, label: &gtk::Label) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    row.append(&ui_image(icon));
    row.append(label);
    row
}

/// Whether `settings` describe a dark theme: one asked for (`gtk-application-prefer-dark-theme`)
/// or one named so (`Adwaita-dark`, `Breeze-Dark`, ...).
fn is_dark(settings: &gtk::Settings) -> bool {
    settings.is_gtk_application_prefer_dark_theme()
        || settings.gtk_theme_name().is_some_and(|n| n.to_lowercase().contains("dark"))
}

/// Switches the icons to the palette for `settings`' theme. State icons follow at their next
/// redraw (the list and the status bar redraw every refresh); the themed images are set here.
fn follow_theme(settings: &gtk::Settings) {
    let dark = is_dark(settings);
    if DARK.replace(dark) == dark {
        return;
    }
    TEXTURES.with(|t| t.borrow_mut().clear());
    THEMED.with(|t| {
        t.borrow_mut().retain(|(art, image)| {
            let Some(image) = image.upgrade() else { return false };
            if let Some(tex) = texture(art.png(dark)) {
                image.set_paintable(Some(&tex));
            }
            true
        })
    });
}

/// Writes `path` unless it already holds `data`.
fn write_if_changed(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if std::fs::read(path).is_ok_and(|old| old == data) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, data)
}

/// Writes the application icon under `base` (an icon theme base folder, such as
/// `$XDG_DATA_HOME/icons`) as `hicolor/<n>x<n>/apps/<APP_ID>.png` at every exported size.
/// With `index`, also a `hicolor/index.theme` naming those folders, for a folder that is only
/// ours (never `$XDG_DATA_HOME/icons`, where it would hide the system's hicolor index).
pub fn install_theme_icons(base: &Path, index: bool) -> Result<(), String> {
    let theme = base.join("hicolor");
    let fail = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let mut dirs = Vec::new();
    for size in APP_SIZES {
        let Some(png) = icons::app_png(size) else { continue };
        let dir = format!("{size}x{size}/apps");
        let path = theme.join(&dir).join(format!("{APP_ID}.png"));
        write_if_changed(&path, png).map_err(|e| fail(&path, e))?;
        dirs.push((dir, size));
    }
    if index {
        let mut text = String::from("[Icon Theme]\nName=Hicolor\nComment=Fallback icon theme\nDirectories=");
        text += &dirs.iter().map(|(d, _)| d.as_str()).collect::<Vec<_>>().join(",");
        text += "\n";
        for (dir, size) in &dirs {
            text += &format!("\n[{dir}]\nSize={size}\nContext=Applications\nType=Threshold\n");
        }
        let path = theme.join("index.theme");
        write_if_changed(&path, text.as_bytes()).map_err(|e| fail(&path, e))?;
    }
    Ok(())
}

/// Makes the application icon findable by name on `display` and the default icon of every
/// window, and makes the other icons follow the display's theme (light or dark). Once, at
/// startup.
pub fn setup(display: &gdk::Display) {
    let settings = gtk::Settings::for_display(display);
    follow_theme(&settings);
    settings.connect_gtk_theme_name_notify(follow_theme);
    settings.connect_gtk_application_prefer_dark_theme_notify(follow_theme);
    let dir = glib::user_cache_dir().join("patorrent").join("icons");
    if let Err(e) = install_theme_icons(&dir, true) {
        eprintln!("icons: {e}");
    } else {
        gtk::IconTheme::for_display(display).add_search_path(&dir);
        THEME_DIR.with(|d| *d.borrow_mut() = Some(dir));
    }
    gtk::Window::set_default_icon_name(APP_ID);
}

/// The folder holding the application icon as a theme (the tray's `IconThemePath`), when
/// [`setup`] could write it.
#[cfg(not(target_os = "macos"))]
pub fn theme_dir() -> Option<PathBuf> {
    THEME_DIR.with(|d| d.borrow().clone())
}

/// The application icon as StatusNotifierItem pixmaps: (width, height, ARGB32 in network byte
/// order, straight alpha), at the exported sizes from 16 to 64 pixels.
#[cfg(not(target_os = "macos"))]
pub fn app_pixmaps() -> Vec<(i32, i32, Vec<u8>)> {
    APP_SIZES
        .iter()
        .filter(|&&s| s <= 64)
        .filter_map(|&s| {
            let tex = texture(icons::app_png(s)?)?;
            let mut down = gdk::TextureDownloader::new(&tex);
            // A, R, G, B in memory order: what the protocol asks for
            down.set_format(gdk::MemoryFormat::A8r8g8b8);
            let (bytes, stride) = down.download_bytes();
            let (w, h) = (tex.width() as usize, tex.height() as usize);
            let mut out = Vec::with_capacity(w * h * 4);
            for row in bytes.chunks(stride).take(h) {
                out.extend_from_slice(row.get(..w * 4)?);
            }
            Some((w as i32, h as i32, out))
        })
        .collect()
}
