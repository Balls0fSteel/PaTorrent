//! The detail tabs under the torrent list, as in the Win32 client: General, Trackers, Peers,
//! Pieces, Files, Speed and Logger, drawn from the frontend's [`Detail`] for the selected
//! torrent. Trackers, Peers, Files and Logger are [`Table`]s; General is a grid of labels
//! under two drawn bars; Pieces is the piece map over the list of pieces being downloaded;
//! Speed is the 600-second graph.
//!
//! The widgets read the data through a [`Peek`] at the application state when they draw, so
//! only the tab on screen costs anything.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;
use patorrent_core::TorrentStatus;
use patorrent_frontend::detail::{
    self, BlockKind, Detail, PIECE_BLOCKS_COLUMN, PIECE_COLUMNS, PRIORITIES, PieceShade, SPEED_SAMPLES, Tab,
};
use patorrent_frontend::fmt;
use patorrent_frontend::lang::{self, gtk_mnemonic};
use patorrent_frontend::tr;

use crate::canvas::{self, Canvas, faded, fill, frame, rgb};
use crate::table::{ContextMenu, Table};

/// Runs the callback on the detail data and the selected torrent's status (with whether the
/// user paused it), unless the application state is busy.
pub type Peek = fn(&mut Visit<'_>);

/// What a [`Peek`] runs: the data and the selected torrent's status with its Pause flag.
pub type Visit<'a> = dyn FnMut(&Detail, Option<(&TorrentStatus, bool)>) + 'a;

fn with<R>(peek: Peek, f: impl FnOnce(&Detail, Option<(&TorrentStatus, bool)>) -> R) -> Option<R> {
    let mut f = Some(f);
    let mut out = None;
    peek(&mut |d, st| {
        if let Some(f) = f.take() {
            out = Some(f(d, st));
        }
    });
    out
}

const BAR_BLUE: u32 = 0x3c78d8;
const BAR_GREEN: u32 = 0x4bb543;
const BLOCK_REQUESTED: u32 = 0xe8a000;
const DOWN_COLOR: u32 = 0x2aa52a;
const UP_COLOR: u32 = 0xd23c3c;

/// The piece map's squares: padding, side, gap.
const MAP_PAD: i32 = 4;
const MAP_CELL: i32 = 9;
const MAP_GAP: i32 = 1;

pub struct Details {
    pub notebook: gtk::Notebook,
    peek: Peek,
    general: General,
    trackers: Table,
    peers: Table,
    files: Table,
    logger: Table,
    logger_scroll: gtk::ScrolledWindow,
    pieces_summary: gtk::Label,
    piece_map: Canvas,
    piece_list: Table,
    /// Pieces the map was last sized for.
    map_pieces: std::cell::Cell<usize>,
    speed: Canvas,
}

/// The General tab: a hint while nothing is selected, else the bars and the label grid.
struct General {
    stack: gtk::Stack,
    bars: [(gtk::Label, Canvas, gtk::Label); 2],
    transfer_title: gtk::Label,
    general_title: gtk::Label,
    /// (label, value) pairs in [`detail::General`]'s order: transfer, status, general, hash.
    pairs: Vec<(gtk::Label, gtk::Label)>,
}

impl Details {
    /// `on_switch` gets the tab the user picks; `files_menu` opens the Files tab's menu.
    pub fn new(
        peek: Peek,
        tab: Tab,
        on_switch: impl Fn(Tab) + 'static,
        files_menu: ContextMenu,
    ) -> Rc<Details> {
        let notebook = gtk::Notebook::new();
        notebook.set_scrollable(true);

        let general = General::new(peek);
        let text = |tab: Tab| -> crate::table::CellText {
            Rc::new(move |row, col| with(peek, |d, _| d.cell(tab, row, col)).unwrap_or_default())
        };
        let trackers = Table::new(Tab::Trackers.columns(), text(Tab::Trackers), None, None);
        let peers = Table::new(Tab::Peers.columns(), text(Tab::Peers), None, None);
        let files = Table::new(Tab::Files.columns(), text(Tab::Files), None, Some(files_menu));
        let logger = Table::new(Tab::Logger.columns(), text(Tab::Logger), None, None);
        logger.view.set_show_column_separators(false);
        let blocks: crate::table::CellPaint = Rc::new(move |w, s, width, height, row| {
            with(peek, |d, _| {
                if let Some(p) = d.partial.get(row) {
                    block_bar(w, s, (1.0, 2.0, width - 2.0, height - 4.0), p);
                }
            });
        });
        let piece_list =
            Table::new(&PIECE_COLUMNS, text(Tab::Pieces), Some((PIECE_BLOCKS_COLUMN, blocks)), None);

        let piece_map = Canvas::new(move |w, s, width, _| {
            with(peek, |d, _| piece_map(w, s, width, d));
        });
        piece_map.set_height_for_width(move |width| {
            let n = with(peek, |d, _| d.have.as_ref().map_or(0, |h| h.len())).unwrap_or(0);
            map_height(n, width)
        });
        let pieces_summary = gtk::Label::new(None);
        pieces_summary.set_xalign(0.0);
        pieces_summary.set_ellipsize(gtk::pango::EllipsizeMode::End);
        pieces_summary.set_margin_start(8);
        pieces_summary.set_margin_top(4);
        pieces_summary.set_margin_bottom(4);
        let map_scroll = gtk::ScrolledWindow::builder()
            .child(&piece_map)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(160)
            .build();
        let pieces_split = gtk::Paned::new(gtk::Orientation::Vertical);
        pieces_split.set_start_child(Some(&map_scroll));
        pieces_split.set_end_child(Some(&scrolled(&piece_list.view)));
        pieces_split.set_shrink_start_child(true);
        pieces_split.set_resize_start_child(false);
        pieces_split.set_vexpand(true);
        let pieces_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        pieces_page.append(&pieces_summary);
        pieces_page.append(&pieces_split);

        let speed = Canvas::new(move |w, s, width, height| {
            with(peek, |d, _| speed_graph(w, s, width, height, d));
        });
        speed.set_hexpand(true);
        speed.set_vexpand(true);

        let logger_scroll = scrolled(&logger.view);
        for t in Tab::ALL {
            let page: gtk::Widget = match t {
                Tab::General => general.stack.clone().upcast(),
                Tab::Trackers => scrolled(&trackers.view).upcast(),
                Tab::Peers => scrolled(&peers.view).upcast(),
                Tab::Pieces => pieces_page.clone().upcast(),
                Tab::Files => scrolled(&files.view).upcast(),
                Tab::Speed => speed.clone().upcast(),
                Tab::Logger => logger_scroll.clone().upcast(),
            };
            let label = gtk::Label::with_mnemonic(&gtk_mnemonic(lang::text(t.title())));
            notebook.append_page(&page, Some(&label));
        }
        notebook.set_current_page(Some(tab.index() as u32));
        // after the switch (`switch-page` runs before it)
        notebook.connect_page_notify(move |nb| {
            if let Some(&t) = nb.current_page().and_then(|n| Tab::ALL.get(n as usize)) {
                on_switch(t);
            }
        });

        Rc::new(Details {
            notebook,
            peek,
            general,
            trackers,
            peers,
            files,
            logger,
            logger_scroll,
            pieces_summary,
            piece_map,
            piece_list,
            map_pieces: std::cell::Cell::new(0),
            speed,
        })
    }

    pub fn tab(&self) -> Tab {
        self.notebook.current_page().and_then(|n| Tab::ALL.get(n as usize).copied()).unwrap_or(Tab::General)
    }

    /// Another torrent is shown: the tables' selections and scroll positions start over.
    pub fn torrent_changed(&self) {
        for t in [&self.trackers, &self.peers, &self.files, &self.piece_list] {
            t.unselect_all();
            t.model.set_len(0);
        }
    }

    /// The Files tab's selected rows.
    pub fn selected_files(&self) -> Vec<usize> {
        self.files.selected()
    }

    /// Redraws the tab on screen from the data.
    pub fn refresh(&self) {
        let peek = self.peek;
        match self.tab() {
            Tab::General => self.general.refresh(peek),
            Tab::Trackers => self.trackers.update(rows(peek, Tab::Trackers)),
            Tab::Peers => self.peers.update(rows(peek, Tab::Peers)),
            Tab::Files => self.files.update(rows(peek, Tab::Files)),
            Tab::Pieces => {
                let (summary, n) = with(peek, |d, st| {
                    let summary = match (&d.have, st) {
                        (Some(_), _) => d.pieces_summary(),
                        (None, Some(_)) => tr!(PiecesWaiting).to_owned(),
                        (None, None) => tr!(PiecesSelectTorrent).to_owned(),
                    };
                    (summary, d.have.as_ref().map_or(0, |h| h.len()))
                })
                .unwrap_or_default();
                set_text(&self.pieces_summary, &summary);
                if self.map_pieces.replace(n) != n {
                    self.piece_map.queue_resize();
                }
                self.piece_map.queue_draw();
                self.piece_list.update(rows(peek, Tab::Pieces));
            }
            Tab::Speed => self.speed.queue_draw(),
            Tab::Logger => {
                let adj = self.logger_scroll.vadjustment();
                let at_end = adj.value() + adj.page_size() >= adj.upper() - 4.0;
                let before = self.logger.model.n_items() as usize;
                let n = rows(peek, Tab::Logger);
                self.logger.update(n);
                if at_end && n > before {
                    self.logger.scroll_to_end();
                }
            }
        }
    }
}

fn rows(peek: Peek, tab: Tab) -> usize {
    with(peek, |d, _| d.rows(tab)).unwrap_or(0)
}

fn scrolled(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder().child(child).hexpand(true).vexpand(true).build()
}

fn set_text(label: &gtk::Label, text: &str) {
    if label.text() != text {
        label.set_text(text);
    }
}

fn bold(label: &gtk::Label) {
    let attrs = gtk::pango::AttrList::new();
    attrs.insert(gtk::pango::AttrInt::new_weight(gtk::pango::Weight::Bold));
    label.set_attributes(Some(&attrs));
}

fn label(bold_text: bool) -> gtk::Label {
    let l = gtk::Label::new(None);
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_single_line_mode(true);
    if bold_text {
        bold(&l);
    }
    l
}

/// A section heading: its bold title and a line to the right.
fn section(title: &gtk::Label) -> gtk::Box {
    title.set_ellipsize(gtk::pango::EllipsizeMode::None);
    let line = gtk::Separator::new(gtk::Orientation::Horizontal);
    line.set_hexpand(true);
    line.set_valign(gtk::Align::Center);
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    b.append(title);
    b.append(&line);
    b.set_margin_top(8);
    b
}

impl General {
    fn new(peek: Peek) -> General {
        let hint = gtk::Label::new(Some(tr!(GenSelectTorrent)));
        hint.set_xalign(0.0);
        hint.set_yalign(0.0);
        hint.set_margin_start(8);
        hint.set_margin_top(8);
        hint.add_css_class("dim-label");

        let grid = gtk::Grid::new();
        grid.set_column_spacing(8);
        grid.set_row_spacing(2);
        grid.set_margin_start(8);
        grid.set_margin_end(8);
        grid.set_margin_top(8);
        grid.set_margin_bottom(8);
        // six columns: three pairs of a bold label and its value
        let bars: [(gtk::Label, Canvas, gtk::Label); 2] = std::array::from_fn(|i| {
            let canvas = Canvas::new(move |w, s, width, height| {
                with(peek, |d, st| {
                    if let Some((st, _)) = st {
                        general_bar(w, s, (0.0, 0.0, width, height), i, d, st);
                    }
                });
            });
            canvas.set_size_request(-1, 12);
            canvas.set_hexpand(true);
            canvas.set_valign(gtk::Align::Center);
            let value = label(false);
            value.set_xalign(1.0);
            value.set_width_chars(8);
            (label(false), canvas, value)
        });
        for (i, (l, c, v)) in bars.iter().enumerate() {
            let row = i as i32;
            grid.attach(l, 0, row, 1, 1);
            grid.attach(c, 1, row, 4, 1);
            grid.attach(v, 5, row, 1, 1);
        }
        let mut pairs = Vec::new();
        let mut pair = |grid: &gtk::Grid, col: i32, row: i32, span: i32| {
            let (l, v) = (label(true), label(false));
            v.set_hexpand(true);
            v.set_selectable(true);
            v.set_can_focus(false);
            grid.attach(&l, col, row, 1, 1);
            grid.attach(&v, col + 1, row, span, 1);
            pairs.push((l, v));
        };
        let transfer_title = label(true);
        grid.attach(&section(&transfer_title), 0, 2, 6, 1);
        for row in 0..4 {
            for col in 0..3 {
                pair(&grid, col * 2, 3 + row, 1);
            }
        }
        pair(&grid, 0, 7, 5);
        let general_title = label(true);
        grid.attach(&section(&general_title), 0, 8, 6, 1);
        for row in 0..3 {
            pair(&grid, 0, 9 + row, 3);
            pair(&grid, 4, 9 + row, 1);
        }
        pair(&grid, 0, 12, 5);

        let info =
            gtk::ScrolledWindow::builder().child(&grid).hscrollbar_policy(gtk::PolicyType::Never).build();
        let stack = gtk::Stack::new();
        stack.add_named(&hint, Some("hint"));
        stack.add_named(&info, Some("info"));
        stack.set_visible_child_name("hint");
        General { stack, bars, transfer_title, general_title, pairs }
    }

    fn refresh(&self, peek: Peek) {
        let g = with(peek, |_, st| st.map(|(st, paused)| detail::general(st, paused))).flatten();
        let Some(g) = g else {
            self.stack.set_visible_child_name("hint");
            return;
        };
        self.stack.set_visible_child_name("info");
        set_text(&self.transfer_title, tr!(GenTransfer));
        set_text(&self.general_title, tr!(GenGeneral));
        for ((l, c, v), (label, value)) in self.bars.iter().zip(&g.bars) {
            set_text(l, label);
            set_text(v, value);
            c.queue_draw();
        }
        let texts =
            g.transfer.iter().flatten().chain([&g.status]).chain(g.general.iter().flatten()).chain([&g.hash]);
        for ((l, v), (label, value)) in self.pairs.iter().zip(texts) {
            set_text(l, label);
            set_text(v, value);
        }
    }
}

/// The General tab's Downloaded (`which` 0: the pieces we have) or Availability bar (1: how many
/// peers have each piece, ours full).
fn general_bar(
    w: &gtk::Widget,
    s: &gtk::Snapshot,
    rect: (f32, f32, f32, f32),
    which: usize,
    d: &Detail,
    st: &TorrentStatus,
) {
    if which == 0 {
        match &d.have {
            Some(h) => canvas::piece_bar(w, s, rect, h.len(), BAR_BLUE, |p| if h.get(p) { 1.0 } else { 0.0 }),
            None => {
                // before the first piece reply: the progress alone
                let done = st.progress_ppm as usize;
                canvas::piece_bar(w, s, rect, 1000, BAR_BLUE, |p| if p * 1000 < done { 1.0 } else { 0.0 })
            }
        }
    } else {
        let (av, have) = (&d.availability, d.have.as_ref());
        canvas::piece_bar(w, s, rect, av.len(), BAR_GREEN, |p| {
            if have.is_some_and(|h| h.get(p)) { 1.0 } else { (av[p] as f32 / 3.0).min(1.0) }
        });
    }
}

/// Columns of squares in a map `width` wide.
fn map_columns(width: i32) -> usize {
    ((width - 2 * MAP_PAD) / (MAP_CELL + MAP_GAP)).max(1) as usize
}

/// The map's height for `pieces` at `width`.
fn map_height(pieces: usize, width: i32) -> i32 {
    if pieces == 0 {
        return 0;
    }
    let rows = pieces.div_ceil(map_columns(width)) as i32;
    2 * MAP_PAD + rows * (MAP_CELL + MAP_GAP)
}

/// One square per piece: blue on disk, orange being downloaded, else green by availability.
fn piece_map(w: &gtk::Widget, s: &gtk::Snapshot, width: f32, d: &Detail) {
    let Some(have) = &d.have else { return };
    let cols = map_columns(width as i32);
    let border = faded(w, 0.45);
    let (cell, step) = (MAP_CELL as f32, (MAP_CELL + MAP_GAP) as f32);
    let shades = [rgb(0xffffff), rgb(0xd8e8d0), rgb(0xa8d29a), rgb(0x6fb85c)];
    let (blue, orange) = (rgb(BAR_BLUE), rgb(BLOCK_REQUESTED));
    for i in 0..have.len() {
        let x = MAP_PAD as f32 + (i % cols) as f32 * step;
        let y = MAP_PAD as f32 + (i / cols) as f32 * step;
        let color = match d.piece_shade(i) {
            PieceShade::Have => &blue,
            PieceShade::Downloading => &orange,
            PieceShade::Available(0) => &shades[0],
            PieceShade::Available(1) => &shades[1],
            PieceShade::Available(2 | 3) => &shades[2],
            PieceShade::Available(_) => &shades[3],
        };
        // the frame is the square's background: two nodes per piece, not five
        fill(s, x, y, cell, cell, &border);
        fill(s, x + 1.0, y + 1.0, cell - 2.0, cell - 2.0, color);
    }
}

/// The Blocks cell: the blocks left to right, blue on disk, green being written, orange
/// requested, white free, in a frame.
fn block_bar(
    w: &gtk::Widget,
    s: &gtk::Snapshot,
    (x, y, width, height): (f32, f32, f32, f32),
    p: &patorrent_core::PartialPiece,
) {
    let n = p.blocks.len();
    let inner = width - 2.0;
    if n == 0 || inner <= 0.0 || height <= 2.0 {
        return;
    }
    fill(s, x, y, width, height, &rgb(0xffffff));
    for (kind, start, len) in detail::block_runs(p) {
        let color = match kind {
            BlockKind::Free => continue,
            BlockKind::Requested => BLOCK_REQUESTED,
            BlockKind::Writing => BAR_GREEN,
            BlockKind::Finished => BAR_BLUE,
        };
        let x0 = x + 1.0 + (start as f32 * inner / n as f32).floor();
        // at least a pixel, so one requested block among a thousand still shows
        let x1 = (x + 1.0 + ((start + len) as f32 * inner / n as f32).floor()).max(x0 + 1.0);
        fill(s, x0, y + 1.0, x1 - x0, height - 2.0, &rgb(color));
    }
    frame(s, x, y, width, height, &faded(w, 0.45));
}

/// The session's download and upload rates over the last ten minutes, newest at the right.
fn speed_graph(w: &gtk::Widget, s: &gtk::Snapshot, width: f32, height: f32, d: &Detail) {
    let (pad, legend, axis, line) = (8.0, 22.0, 76.0, 16.0);
    let (left, top) = (pad + axis, pad + legend);
    let (pw, ph) = (width - left - pad, height - top - pad);
    if pw < 20.0 || ph < 20.0 {
        return;
    }
    let fg = w.color();
    let scale = detail::speed_scale(&d.speed);

    // legend
    let (down, up) = d.speed.back().copied().unwrap_or((0, 0));
    let mut x = left;
    for (text, color) in [
        (tr!(SpeedDownload, rate = fmt::speed_always(down)), DOWN_COLOR),
        (tr!(SpeedUpload, rate = fmt::speed_always(up)), UP_COLOR),
    ] {
        fill(s, x, pad + 3.0, 10.0, 10.0, &rgb(color));
        let tw = canvas::text(w, s, &text, (x + 14.0, pad, line), false, &fg);
        x += (14.0 + tw + 24.0).max(200.0);
    }

    // grid and scale
    let grid = faded(w, 0.15);
    for i in 0..=4 {
        let y = top + ph * i as f32 / 4.0;
        if i > 0 && i < 4 {
            fill(s, left + 1.0, y.floor(), pw - 2.0, 1.0, &grid);
        }
        if i < 4 {
            let v = scale * (4 - i) as u64 / 4;
            canvas::text(w, s, &fmt::speed_always(v as u32), (left - 4.0, y - line / 2.0, line), true, &fg);
        }
    }
    frame(s, left, top, pw, ph, &faded(w, 0.45));

    // the two rates
    let n = d.speed.len();
    if n < 2 {
        return;
    }
    let cr = s.append_cairo(&gtk::graphene::Rect::new(left, top, pw, ph));
    cr.set_line_width(2.0);
    let step = ((pw - 2.0) / (SPEED_SAMPLES - 1) as f32).max(1.0) as f64;
    for (series, color) in [(0, DOWN_COLOR), (1, UP_COLOR)] {
        let c = rgb(color);
        cr.set_source_rgb(c.red().into(), c.green().into(), c.blue().into());
        for (i, &(a, b)) in d.speed.iter().enumerate() {
            let v = if series == 0 { a } else { b } as f64;
            let x = ((left + pw - 1.0) as f64 - (n - 1 - i) as f64 * step).max((left + 1.0) as f64);
            let y = (top + ph - 1.0) as f64 - v / scale as f64 * (ph - 2.0) as f64;
            if i == 0 {
                cr.move_to(x, y);
            } else {
                cr.line_to(x, y);
            }
        }
        let _ = cr.stroke();
    }
}

/// The Files tab's menu: the priorities (`win.file-prio-N`, N into [`PRIORITIES`]).
pub fn files_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    for (i, (text, _)) in PRIORITIES.iter().enumerate() {
        menu.append(Some(&gtk_mnemonic(lang::text(*text))), Some(&format!("win.file-prio-{i}")));
    }
    menu
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_piece_map_grows_a_row_at_a_time() {
        assert_eq!(map_height(0, 500), 0);
        // (500 - 8) / 10 = 49 squares a row
        assert_eq!(map_columns(500), 49);
        assert_eq!(map_height(49, 500), 8 + 10);
        assert_eq!(map_height(50, 500), 8 + 20);
        // never fewer than one column
        assert_eq!(map_columns(5), 1);
        assert_eq!(map_height(3, 5), 8 + 30);
    }
}
