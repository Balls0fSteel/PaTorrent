//! The main window, µTorrent 2.2.1's layout in AppKit (docs/MACOS_GUI.md "Main window"):
//!
//! - an `NSToolbar` (customizable, autosaved; SF Symbols): Add Torrent, Add URL, Create |
//!   Remove | Start, Pause, Stop | Queue Up, Queue Down | RSS | flexible space | the search field,
//!   whose `searchMenuTemplate` is the search mode menu;
//! - an `NSSplitViewController`: the source-list sidebar, and a vertical split of the torrent
//!   table over an `NSTabView` of the 7 detail tabs;
//! - the status bar in the window's bottom content border.
//!
//! `RPWindowDelegate` is the window's, the toolbar's and the tab view's delegate. Slice B builds
//! the shell; slice C fills the sidebar, the list's cells and the status bar from `Core`, slice D
//! the tabs.

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSImageView, NSOutlineView, NSSearchField, NSSearchFieldCell, NSSplitViewController,
    NSSplitViewItem, NSStackView, NSTabView, NSTabViewDelegate, NSTabViewItem, NSTextField, NSToolbar,
    NSToolbarDelegate, NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSToolbarSpaceItemIdentifier,
    NSView, NSViewController, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString};
use rookparcel_frontend::detail::Tab;
use rookparcel_frontend::{lang, tr};

use crate::commands::Command;
use crate::menu;
use crate::table::{RPTableSource, RPTableView};
use crate::ui::{self, layout};

/// The status bar's height (the bottom content border).
pub const STATUS_HEIGHT: f64 = 24.0;

/// The toolbar's buttons, in their default order; `None` is a space.
const TOOLBAR: [Option<Command>; 14] = [
    Some(Command::AddTorrent),
    Some(Command::AddUrl),
    Some(Command::CreateTorrent),
    None,
    Some(Command::Remove),
    None,
    Some(Command::Start),
    Some(Command::Pause),
    Some(Command::Stop),
    None,
    Some(Command::QueueUp),
    Some(Command::QueueDown),
    None,
    Some(Command::Rss),
];

/// The search field's item.
const SEARCH_ID: &str = "rp.search";

/// A toolbar button's identifier.
fn item_id(cmd: Command) -> String {
    format!("rp.{}", cmd.tag())
}

/// A toolbar button's label and tooltip.
fn tool_text(cmd: Command) -> &'static str {
    match cmd {
        Command::AddTorrent => tr!(ToolAddTorrent),
        Command::AddUrl => tr!(ToolAddUrl),
        Command::CreateTorrent => tr!(ToolCreate),
        Command::Remove => tr!(ToolRemove),
        Command::Start => tr!(ToolStart),
        Command::Pause => tr!(ToolPause),
        Command::Stop => tr!(ToolStop),
        Command::QueueUp => tr!(ToolQueueUp),
        Command::QueueDown => tr!(ToolQueueDown),
        Command::Rss => tr!(ToolRss),
        _ => tr!(ToolPreferences),
    }
}

/// The status bar's parts.
pub struct Status {
    pub stack: Retained<NSStackView>,
    /// The update notice (hidden until there is one).
    pub update: Retained<NSTextField>,
    pub net_icon: Retained<NSImageView>,
    pub network: Retained<NSTextField>,
    pub dht: Retained<NSTextField>,
    pub down: Retained<NSTextField>,
    pub up: Retained<NSTextField>,
}

/// The main window's parts.
pub struct Ui {
    pub window: Retained<NSWindow>,
    pub delegate: Retained<RPWindowDelegate>,
    pub toolbar: Retained<NSToolbar>,
    pub search: Retained<NSSearchField>,
    pub split: Retained<NSSplitViewController>,
    pub sidebar_item: Retained<NSSplitViewItem>,
    pub content_split: Retained<NSSplitViewController>,
    pub details_item: Retained<NSSplitViewItem>,
    pub sidebar_view: Retained<NSOutlineView>,
    pub sidebar: Retained<RPTableSource>,
    pub list_view: Retained<RPTableView>,
    pub list: Retained<RPTableSource>,
    pub tabs: Retained<NSTabView>,
    pub status: Status,
}

pub struct Ivars {
    search: Retained<NSSearchField>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and RPWindowDelegate does not implement
    // Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "RPWindowDelegate"]
    #[ivars = Ivars]
    pub struct RPWindowDelegate;

    unsafe impl NSObjectProtocol for RPWindowDelegate {}

    unsafe impl NSWindowDelegate for RPWindowDelegate {
        /// Slice G: close and minimize into the status item (`ui.close_to_tray`,
        /// `ui.minimize_to_tray`).
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _sender: &NSWindow) -> bool {
            crate::mac::lifecycle::window_should_close()
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn did_miniaturize(&self, _n: &objc2_foundation::NSNotification) {
            crate::mac::lifecycle::did_miniaturize();
        }
    }

    unsafe impl NSToolbarDelegate for RPWindowDelegate {
        #[unsafe(method_id(toolbar:itemForItemIdentifier:willBeInsertedIntoToolbar:))]
        fn item_for(
            &self,
            _toolbar: &NSToolbar,
            id: &NSString,
            _inserted: bool,
        ) -> Option<Retained<NSToolbarItem>> {
            self.toolbar_item(id)
        }

        #[unsafe(method_id(toolbarDefaultItemIdentifiers:))]
        fn default_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            toolbar_ids(true)
        }

        #[unsafe(method_id(toolbarAllowedItemIdentifiers:))]
        fn allowed_ids(&self, _toolbar: &NSToolbar) -> Retained<NSArray<NSString>> {
            toolbar_ids(false)
        }
    }

    unsafe impl NSTabViewDelegate for RPWindowDelegate {
        #[unsafe(method(tabView:didSelectTabViewItem:))]
        fn tab_selected(&self, tabs: &NSTabView, item: Option<&NSTabViewItem>) {
            tab_selected(tabs, item);
        }
    }
);

fn tab_selected(tabs: &NSTabView, item: Option<&NSTabViewItem>) {
    let Some(item) = item else { return };
    let i = tabs.indexOfTabViewItem(item);
    if let Some(&tab) = usize::try_from(i).ok().and_then(|i| Tab::ALL.get(i)) {
        crate::detail::show_tab(tab);
    }
}

impl RPWindowDelegate {
    fn toolbar_item(&self, id: &NSString) -> Option<Retained<NSToolbarItem>> {
        let mtm = MainThreadMarker::from(self);
        let item = NSToolbarItem::initWithItemIdentifier(NSToolbarItem::alloc(mtm), id);
        let id = id.to_string();
        if id == SEARCH_ID {
            let label = ui::text(tr!(SearchCue));
            item.setLabel(&ui::ns(&label));
            item.setPaletteLabel(&ui::ns(&label));
            item.setView(Some(&self.ivars().search));
            return Some(item);
        }
        let cmd = TOOLBAR.iter().flatten().copied().find(|&c| item_id(c) == id)?;
        let text = ui::text(tool_text(cmd));
        item.setLabel(&ui::ns(&text));
        item.setPaletteLabel(&ui::ns(&text));
        item.setToolTip(Some(&ui::ns(&text)));
        item.setImage(crate::images::command(cmd, &text, mtm).as_deref());
        item.setBordered(true);
        ui::target::wire(&item, cmd, mtm);
        Some(item)
    }
}

/// The default toolbar (`default`), or every item the customization palette offers.
fn toolbar_ids(default: bool) -> Retained<NSArray<NSString>> {
    // SAFETY: AppKit's identifier constants.
    let (space, flexible) = unsafe { (NSToolbarSpaceItemIdentifier, NSToolbarFlexibleSpaceItemIdentifier) };
    let mut ids: Vec<Retained<NSString>> = Vec::new();
    for t in TOOLBAR {
        match t {
            Some(c) => ids.push(NSString::from_str(&item_id(c))),
            None if default => ids.push(space.retain()),
            None => {}
        }
    }
    if default {
        ids.push(flexible.retain());
    } else {
        ids.push(space.retain());
        ids.push(flexible.retain());
    }
    ids.push(NSString::from_str(SEARCH_ID));
    NSArray::from_retained_slice(&ids)
}

impl RPWindowDelegate {
    pub fn new(search: Retained<NSSearchField>, mtm: MainThreadMarker) -> Retained<RPWindowDelegate> {
        let this = RPWindowDelegate::alloc(mtm).set_ivars(Ivars { search });
        // SAFETY: NSObject's `init` on a fresh object with its ivars set.
        unsafe { msg_send![super(this), init] }
    }
}

/// The search field with its mode menu (filter the names, a source, all sources, Manage
/// Sources...).
fn search_field(mtm: MainThreadMarker) -> Retained<NSSearchField> {
    let f = ui::search_field(&ui::text(tr!(SearchCue)), Command::Search, mtm);
    layout::activate(&[
        f.widthAnchor().constraintGreaterThanOrEqualToConstant(140.0),
        f.widthAnchor().constraintLessThanOrEqualToConstant(260.0),
    ]);
    let template = menu::build(
        "",
        &[
            menu::Item::Cmd { cmd: Command::SearchNames, title: ui::text(tr!(SearchCue)), key: None },
            menu::Item::Sep,
            menu::Item::Cmd {
                cmd: Command::SearchAllSources,
                title: ui::text(tr!(SearchAllSources)),
                key: None,
            },
            menu::Item::Sep,
            menu::Item::Cmd { cmd: Command::ManageSources, title: ui::text(tr!(SearchManage)), key: None },
        ],
        mtm,
    );
    if let Some(cell) = f.cell().and_then(|c| c.downcast::<NSSearchFieldCell>().ok()) {
        cell.setSearchMenuTemplate(Some(&template));
    }
    f
}

/// A view controller showing `view`.
fn controller(view: &NSView, mtm: MainThreadMarker) -> Retained<NSViewController> {
    let vc = NSViewController::new(mtm);
    vc.setView(view);
    vc
}

/// The 7 detail tabs, empty (slice D fills them).
fn tabs(delegate: &RPWindowDelegate, mtm: MainThreadMarker) -> Retained<NSTabView> {
    let tabs = NSTabView::new(mtm);
    ui::controls::auto(&tabs);
    for t in Tab::ALL {
        let item = NSTabViewItem::new();
        item.setLabel(&ui::nstext(lang::text(t.title())));
        item.setView(Some(&NSView::new(mtm)));
        tabs.addTabViewItem(&item);
    }
    tabs.setDelegate(Some(ProtocolObject::from_ref(delegate)));
    tabs
}

/// The status bar's stack, in the bottom content border.
fn status_bar(mtm: MainThreadMarker) -> Status {
    let label = |s: &str| {
        let l = ui::truncating_label(s, mtm);
        l.setFont(Some(&objc2_app_kit::NSFont::systemFontOfSize(
            objc2_app_kit::NSFont::smallSystemFontSize(),
        )));
        l
    };
    let update = label("");
    update.setHidden(true);
    update.setTextColor(Some(&objc2_app_kit::NSColor::linkColor()));
    let net_icon = NSImageView::new(mtm);
    ui::controls::auto(&net_icon);
    layout::width(&net_icon, 16.0);
    layout::height(&net_icon, 16.0);
    let (network, dht, down, up) = (label(""), label(""), label(""), label(""));
    let spacer = layout::spacer(mtm);
    let stack = layout::hstack(&[&update, &spacer, &net_icon, &network, &dht, &down, &up], mtm);
    stack.setSpacing(16.0);
    layout::insets(&stack, 0.0, 12.0, 0.0, 12.0);
    Status { stack, update, net_icon, network, dht, down, up }
}

/// Builds the main window around the sidebar outline and the torrent table (made by the
/// caller from its hooks).
pub fn build(
    sidebar: (Retained<objc2_app_kit::NSScrollView>, Retained<NSOutlineView>, Retained<RPTableSource>),
    list: (Retained<objc2_app_kit::NSScrollView>, Retained<RPTableView>, Retained<RPTableSource>),
    mtm: MainThreadMarker,
) -> Ui {
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;
    let frame = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(1000.0, 640.0));
    // SAFETY: a plain titled window; it is released only when we drop it (`setReleasedWhenClosed`).
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    // SAFETY: we keep the window in `Ui`; closing only hides it.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(&rookparcel_frontend::profile::name_build()));
    window.setContentMinSize(NSSize::new(560.0, 320.0));
    window.setFrameAutosaveName(&NSString::from_str("Main"));

    let search = search_field(mtm);
    let delegate = RPWindowDelegate::new(search.clone(), mtm);
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &NSString::from_str("RookparcelMain"));
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    toolbar.setAllowsUserCustomization(true);
    toolbar.setAutosavesConfiguration(true);
    // µTorrent's toolbar is icons only (Customize Toolbar can add the labels)
    toolbar.setDisplayMode(objc2_app_kit::NSToolbarDisplayMode::IconOnly);
    window.setToolbar(Some(&toolbar));

    // the sidebar beside a vertical split of the list over the tabs
    let tabs = tabs(&delegate, mtm);
    let content_split = NSSplitViewController::new(mtm);
    content_split.splitView().setVertical(false);
    let list_item = NSSplitViewItem::splitViewItemWithViewController(&controller(&list.0, mtm));
    list_item.setMinimumThickness(80.0);
    let details_item = NSSplitViewItem::splitViewItemWithViewController(&controller(&tabs, mtm));
    details_item.setMinimumThickness(120.0);
    details_item.setCanCollapse(true);
    content_split.addSplitViewItem(&list_item);
    content_split.addSplitViewItem(&details_item);

    let split = NSSplitViewController::new(mtm);
    split.splitView().setVertical(true);
    let sidebar_item = NSSplitViewItem::sidebarWithViewController(&controller(&sidebar.0, mtm));
    sidebar_item.setMinimumThickness(140.0);
    sidebar_item.setMaximumThickness(320.0);
    split.addSplitViewItem(&sidebar_item);
    split.addSplitViewItem(&NSSplitViewItem::splitViewItemWithViewController(&content_split));

    // the window's content: the splits over the status bar
    let content = NSView::new(mtm);
    let split_view = split.view();
    ui::controls::auto(&split_view);
    content.addSubview(&split_view);
    let status = status_bar(mtm);
    content.addSubview(&status.stack);
    layout::activate(&[
        split_view.topAnchor().constraintEqualToAnchor(&content.topAnchor()),
        split_view.leadingAnchor().constraintEqualToAnchor(&content.leadingAnchor()),
        split_view.trailingAnchor().constraintEqualToAnchor(&content.trailingAnchor()),
        split_view.bottomAnchor().constraintEqualToAnchor(&status.stack.topAnchor()),
        status.stack.leadingAnchor().constraintEqualToAnchor(&content.leadingAnchor()),
        status.stack.trailingAnchor().constraintEqualToAnchor(&content.trailingAnchor()),
        status.stack.bottomAnchor().constraintEqualToAnchor(&content.bottomAnchor()),
        status.stack.heightAnchor().constraintEqualToConstant(STATUS_HEIGHT),
    ]);
    window.setContentView(Some(&content));
    window.setAutorecalculatesContentBorderThickness_forEdge(false, NSRectEdge::MinY);
    window.setContentBorderThickness_forEdge(STATUS_HEIGHT, NSRectEdge::MinY);

    Ui {
        window,
        delegate,
        toolbar,
        search,
        split,
        sidebar_item,
        content_split,
        details_item,
        sidebar_view: sidebar.1,
        sidebar: sidebar.2,
        list_view: list.1,
        list: list.2,
        tabs,
        status,
    }
}

impl Ui {
    /// Shows or hides the status bar (and its border).
    pub fn set_status_visible(&self, on: bool) {
        self.status.stack.setHidden(!on);
        self.window.setContentBorderThickness_forEdge(if on { STATUS_HEIGHT } else { 0.0 }, NSRectEdge::MinY);
        for c in self.status.stack.constraints().iter() {
            if c.firstAttribute() == objc2_app_kit::NSLayoutAttribute::Height {
                c.setConstant(if on { STATUS_HEIGHT } else { 0.0 });
            }
        }
    }

    pub fn status_visible(&self) -> bool {
        !self.status.stack.isHidden()
    }
}
