//! The main window, µTorrent 2.2.1's layout in AppKit (docs/MACOS_GUI.md "Main window"):
//!
//! - an `NSToolbar` (customizable; SF Symbols): Add Torrent, Add URL, Create |
//!   Remove | Start, Pause, Stop | Queue Up, Queue Down | RSS | flexible space | the search field,
//!   whose `searchMenuTemplate` is the search mode menu;
//! - an `NSSplitViewController`: the source-list sidebar, and a vertical split of the torrent
//!   table over an `NSTabView` of the 7 detail tabs;
//! - the status bar in the window's bottom content border.
//!
//! `RPWindowDelegate` is the window's, the toolbar's and the tab view's delegate, and takes the
//! `.torrent` files dropped on the window (the window passes its dragging messages to its
//! delegate). Slice B builds the shell; slice C fills the sidebar, the list and the status bar
//! from `Core`, slice D the tabs.
//!
//! The layout (View toggles, split sizes, the detail tab, the frame, the toolbar's items and
//! mode) is kept in `rookparcel.ini` through `frontend::app::Layout`, never in AppKit's
//! autosave (frame and toolbar autosave write `~/Library/Preferences` even in portable mode).

use std::path::PathBuf;

use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSButton, NSCellImagePosition, NSColor, NSDragOperation, NSDraggingDestination,
    NSDraggingInfo, NSFont, NSLineBreakMode, NSOutlineView, NSPasteboardTypeFileURL, NSSearchField,
    NSSearchFieldCell, NSSplitViewController, NSSplitViewItem, NSStackView, NSTabView, NSTabViewDelegate,
    NSTabViewItem, NSTextField, NSToolbar, NSToolbarDelegate, NSToolbarDisplayMode,
    NSToolbarFlexibleSpaceItemIdentifier, NSToolbarItem, NSToolbarSpaceItemIdentifier, NSView,
    NSViewController, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{NSArray, NSPoint, NSRect, NSRectEdge, NSSize, NSString, NSURL};
use rookparcel_frontend::app::Layout;
use rookparcel_frontend::detail::Tab;
use rookparcel_frontend::{lang, tr};

use crate::commands::Command;
use crate::menu;
use crate::table::{RPTableSource, RPTableView};
use crate::ui::{self, layout};

/// The status bar's height (the bottom content border).
pub const STATUS_HEIGHT: f64 = 24.0;

/// The sidebar's width until the user moves it (room for "Downloading (1234)").
const SIDEBAR_WIDTH: f64 = 180.0;
/// The detail tabs' height until the user moves the split.
const DETAIL_HEIGHT: f64 = 220.0;

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
    /// The update notice, a link (hidden until there is one).
    pub update: Retained<NSButton>,
    /// The network icon and text (a click leads to the port while another program has it).
    pub network: Retained<NSButton>,
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
    /// What holds the torrent list's scroll view (slice H puts the feed items' table beside it,
    /// one of the two shown).
    pub list_box: Retained<NSView>,
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

        #[unsafe(method(windowDidDeminiaturize:))]
        fn did_deminiaturize(&self, _n: &objc2_foundation::NSNotification) {
            crate::mac::lifecycle::did_deminiaturize();
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

    // the window passes the dragging messages it gets to its delegate
    unsafe impl NSDraggingDestination for RPWindowDelegate {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            if dropped_torrents(info).is_empty() { NSDragOperation::None } else { NSDragOperation::Copy }
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let paths = dropped_torrents(info);
            let any = !paths.is_empty();
            if any {
                // once the drag is over: adding may open dialogs
                crate::mainq::on_main(move || crate::list::add_files(paths));
            }
            any
        }
    }
);

/// The `.torrent` files a drag carries.
fn dropped_torrents(info: &ProtocolObject<dyn NSDraggingInfo>) -> Vec<PathBuf> {
    let Some(items) = info.draggingPasteboard().pasteboardItems() else { return Vec::new() };
    // SAFETY: AppKit's pasteboard type constant.
    let file_url = unsafe { NSPasteboardTypeFileURL };
    items
        .iter()
        .filter_map(|i| i.stringForType(file_url))
        .filter_map(|s| NSURL::URLWithString(&s))
        .filter_map(|u| u.path().map(|p| PathBuf::from(p.to_string())))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("torrent")))
        .collect()
}

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
    // slice I: the hint and the menu follow the kept mode and the sources
    let f = ui::search_field(&crate::search::cue(), Command::Search, mtm);
    layout::activate(&[
        f.widthAnchor().constraintGreaterThanOrEqualToConstant(140.0),
        f.widthAnchor().constraintLessThanOrEqualToConstant(260.0),
    ]);
    let template = menu::build("", &crate::search::menu_items(), mtm);
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
    let small = || NSFont::systemFontOfSize(NSFont::smallSystemFontSize());
    let label = |s: &str| {
        let l = ui::truncating_label(s, mtm);
        l.setFont(Some(&small()));
        l
    };
    // borderless buttons that look like the labels beside them
    let link = |cmd: Command| {
        let b = ui::button("", cmd, mtm);
        b.setBordered(false);
        b.setFont(Some(&small()));
        b.setLineBreakMode(NSLineBreakMode::ByTruncatingTail);
        b
    };
    let update = link(Command::UpdateNotice);
    update.setContentTintColor(Some(&NSColor::linkColor()));
    update.setHidden(true);
    let network = link(Command::NetworkStatus);
    network.setImagePosition(NSCellImagePosition::ImageLeading);
    let (dht, down, up) = (label(""), label(""), label(""));
    let spacer = layout::spacer(mtm);
    let stack = layout::hstack(&[&update, &spacer, &network, &dht, &down, &up], mtm);
    stack.setSpacing(16.0);
    layout::insets(&stack, 0.0, 12.0, 0.0, 12.0);
    Status { stack, update, network, dht, down, up }
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
    // the frame is kept in the prefs (`Layout`): no frame autosave, no state restoration
    window.setRestorable(false);
    // SAFETY: AppKit's pasteboard type constant; the delegate takes the drops.
    window.registerForDraggedTypes(&NSArray::from_slice(&[unsafe { NSPasteboardTypeFileURL }]));

    let search = search_field(mtm);
    let delegate = RPWindowDelegate::new(search.clone(), mtm);
    window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &NSString::from_str("RookparcelMain"));
    toolbar.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    toolbar.setAllowsUserCustomization(true);
    // kept in the prefs (`Layout`), not in ~/Library/Preferences
    toolbar.setAutosavesConfiguration(false);
    // µTorrent's toolbar is icons only (Customize Toolbar can add the labels)
    toolbar.setDisplayMode(objc2_app_kit::NSToolbarDisplayMode::IconOnly);
    window.setToolbar(Some(&toolbar));

    // the sidebar beside a vertical split of the list over the tabs
    let tabs = tabs(&delegate, mtm);
    let content_split = NSSplitViewController::new(mtm);
    content_split.splitView().setVertical(false);
    let list_box = NSView::new(mtm);
    list_box.addSubview(&list.0);
    layout::pin(&list.0, &list_box, 0.0);
    let list_item = NSSplitViewItem::splitViewItemWithViewController(&controller(&list_box, mtm));
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
    // the list has the keys (Return, ⌫, ⌘A) from the start, as µTorrent's
    let first: &NSView = &list.1;
    window.setInitialFirstResponder(Some(first));
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
        list_box,
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

    /// Puts back the kept layout, before the window shows: the frame, the View toggles, the
    /// toolbar's items and mode, the detail tab. The split sizes follow in [`Ui::apply_sizes`].
    pub fn apply_layout(&self, l: &Layout) {
        match &l.frame {
            Some(f) => self.window.setFrameFromString(&NSString::from_str(f)),
            None => self.window.center(),
        }
        self.toolbar.setVisible(l.toolbar);
        if let Some(items) = &l.toolbar_items {
            let allowed: Vec<String> = toolbar_ids(false).iter().map(|s| s.to_string()).collect();
            while self.toolbar.items().count() > 0 {
                self.toolbar.removeItemAtIndex(0);
            }
            for (i, id) in items.iter().filter(|id| allowed.contains(id)).enumerate() {
                self.toolbar.insertItemWithItemIdentifier_atIndex(&NSString::from_str(id), i as isize);
            }
        }
        if let Some(m) = l.toolbar_mode {
            self.toolbar.setDisplayMode(NSToolbarDisplayMode(m));
        }
        self.sidebar_item.setCollapsed(!l.sidebar);
        self.details_item.setCollapsed(!l.details);
        self.set_status_visible(l.status);
        self.tabs.selectTabViewItemAtIndex(l.tab.index() as isize);
    }

    /// The split sizes, once the window is laid out.
    pub fn apply_sizes(&self, l: &Layout) {
        self.window.layoutIfNeeded();
        if l.sidebar {
            let w = l.sidebar_width.unwrap_or(SIDEBAR_WIDTH);
            self.split.splitView().setPosition_ofDividerAtIndex(w, 0);
        }
        if l.details {
            let h = l.detail_height.unwrap_or(DETAIL_HEIGHT);
            let sv = self.content_split.splitView();
            let at = sv.frame().size.height - sv.dividerThickness() - h;
            if at > 0.0 {
                sv.setPosition_ofDividerAtIndex(at, 0);
            }
        }
    }

    /// The layout now, to keep (`Layout::store`).
    pub fn layout(&self) -> Layout {
        let mtm = MainThreadMarker::from(&*self.window);
        let sidebar = !self.sidebar_item.isCollapsed();
        let details = !self.details_item.isCollapsed();
        let items: Vec<String> =
            self.toolbar.items().iter().map(|i| i.itemIdentifier().to_string()).collect();
        let default: Vec<String> = toolbar_ids(true).iter().map(|s| s.to_string()).collect();
        let full_screen = self.window.styleMask().contains(NSWindowStyleMask::FullScreen);
        let tab = self
            .tabs
            .selectedTabViewItem()
            .and_then(|i| usize::try_from(self.tabs.indexOfTabViewItem(&i)).ok())
            .and_then(|i| Tab::ALL.get(i).copied())
            .unwrap_or(Tab::General);
        Layout {
            toolbar: self.toolbar.isVisible(),
            sidebar,
            details,
            status: self.status_visible(),
            sidebar_width: sidebar.then(|| self.sidebar_item.viewController(mtm).view().frame().size.width),
            // the split's lower pane, as `apply_sizes` sets it: the tab view controller's own view
            // is 16 points taller than the pane, which made the details grow at every start
            detail_height: details.then(|| {
                let panes = self.content_split.splitView().subviews();
                match panes.iter().nth(1) {
                    Some(pane) => pane.frame().size.height,
                    None => self.details_item.viewController(mtm).view().frame().size.height,
                }
            }),
            tab,
            // a full-screen frame is not the window's own
            frame: (!full_screen).then(|| self.window.stringWithSavedFrame().to_string()),
            toolbar_items: (items != default).then_some(items),
            toolbar_mode: Some(self.toolbar.displayMode().0),
        }
    }
}
