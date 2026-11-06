//! The Pieces tab: a summary line, the piece map (`canvas::Kind::PieceMap`: one square a piece,
//! blue on disk, orange being downloaded, else green by how many peers have it; at most 160
//! points high, then it scrolls) over the list of pieces being downloaded, whose Blocks cells
//! are drawn (`canvas::Kind::BlockBar`: blue on disk, green being written, orange requested).

use std::any::Any;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSLayoutConstraint, NSRectFillList, NSScrollView, NSTextField, NSView};
use objc2_foundation::NSRect;
use rookparcel_frontend::app::drawn::{self, MAP_CELL};
use rookparcel_frontend::detail::{self, BlockKind, PIECE_BLOCKS_COLUMN, PIECE_COLUMNS, PieceShade, Tab};
use rookparcel_frontend::{lang, tr};

use super::tables::{self, Table};
use crate::app::peek;
use crate::canvas::{self, Kind, Painter, RPCanvas, rect};
use crate::table::{CellKind, CellValue};
use crate::ui::{self, controls::auto, layout};

/// The map's height before it scrolls.
const MAP_MAX: f64 = 160.0;

/// The map's value: one shade a piece.
#[derive(Default)]
pub struct Map {
    pub shades: Vec<PieceShade>,
}

/// The Blocks cell's value: the blocks and their runs of one kind.
pub struct Blocks {
    pub count: usize,
    pub runs: Vec<(BlockKind, usize, usize)>,
}

/// The four greens of the availability shades (none, one, two or three, four or more peers).
fn available(n: u8) -> Retained<NSColor> {
    let alpha = match n {
        0 => return NSColor::textBackgroundColor(),
        1 => 0.3,
        2 | 3 => 0.6,
        _ => 1.0,
    };
    NSColor::systemGreenColor().colorWithAlphaComponent(alpha)
}

/// Fills `rects` with `color` in one call.
fn fill_all(rects: &mut [NSRect], color: &NSColor) {
    if rects.is_empty() {
        return;
    }
    color.setFill();
    // SAFETY: the pointer and count are the slice's own, valid for the call.
    unsafe {
        NSRectFillList(std::ptr::NonNull::from(&mut rects[0]), rects.len() as isize);
    }
}

/// The map: each square's frame, then its colour, one fill per colour.
fn draw_map(p: &Painter, value: &dyn Any) {
    let Some(map) = value.downcast_ref::<Map>() else { return };
    let cols = drawn::map_columns(p.width());
    let mut frames = Vec::with_capacity(map.shades.len());
    // have, downloading, then the available shades 0..=4
    let mut by_color: [Vec<NSRect>; 7] = Default::default();
    for (i, s) in map.shades.iter().enumerate() {
        let (x, y) = drawn::map_square(i, cols);
        frames.push(rect(x, y, MAP_CELL, MAP_CELL));
        let slot = match s {
            PieceShade::Have => 0,
            PieceShade::Downloading => 1,
            PieceShade::Available(n) => 2 + (*n).min(4) as usize,
        };
        by_color[slot].push(rect(x + 1.0, y + 1.0, MAP_CELL - 2.0, MAP_CELL - 2.0));
    }
    fill_all(&mut frames, &NSColor::tertiaryLabelColor());
    for (slot, rects) in by_color.iter_mut().enumerate() {
        let color = match slot {
            0 => NSColor::systemBlueColor(),
            1 => NSColor::systemOrangeColor(),
            n => available((n - 2) as u8),
        };
        fill_all(rects, &color);
    }
}

/// The Blocks cell: the blocks left to right in a frame.
fn draw_blocks(p: &Painter, value: &dyn Any) {
    let Some(b) = value.downcast_ref::<Blocks>() else { return };
    let r = rect(1.0, 2.0, p.width() - 2.0, p.height() - 4.0);
    let inner = r.size.width - 2.0;
    if b.count == 0 || inner <= 0.0 || r.size.height <= 2.0 {
        return;
    }
    p.fill(r, &NSColor::textBackgroundColor());
    for &(kind, start, len) in &b.runs {
        let color = match kind {
            BlockKind::Free => continue,
            BlockKind::Requested => NSColor::systemOrangeColor(),
            BlockKind::Writing => NSColor::systemGreenColor(),
            BlockKind::Finished => NSColor::systemBlueColor(),
        };
        let x0 = r.origin.x + 1.0 + (start as f64 * inner / b.count as f64).floor();
        // at least a point, so one requested block among a thousand still shows
        let x1 = (r.origin.x + 1.0 + ((start + len) as f64 * inner / b.count as f64).floor()).max(x0 + 1.0);
        p.fill(rect(x0, r.origin.y + 1.0, x1 - x0, r.size.height - 2.0), &color);
    }
    p.frame(r, &NSColor::tertiaryLabelColor());
}

/// The tab's views.
pub struct Pieces {
    pub view: Retained<NSView>,
    summary: Retained<NSTextField>,
    map: Retained<RPCanvas>,
    map_scroll: Retained<NSScrollView>,
    /// The map's own height (all its rows) and the height it shows (at most [`MAP_MAX`]).
    map_height: Retained<NSLayoutConstraint>,
    shown_height: Retained<NSLayoutConstraint>,
    pub list: Table,
}

impl Pieces {
    pub fn new(mtm: MainThreadMarker) -> Pieces {
        let summary = ui::truncating_label("", mtm);
        let map = RPCanvas::new(Kind::PieceMap, Box::new(Map::default()), mtm);
        map.set_accessibility(&ui::text(lang::text(Tab::Pieces.title())), "");
        let map_scroll = NSScrollView::new(mtm);
        auto(&map_scroll);
        map_scroll.setHasVerticalScroller(true);
        map_scroll.setAutohidesScrollers(true);
        map_scroll.setDrawsBackground(false);
        map_scroll.setDocumentView(Some(&map));
        let clip = map_scroll.contentView();
        let map_height = map.heightAnchor().constraintEqualToConstant(0.0);
        let shown_height = map_scroll.heightAnchor().constraintEqualToConstant(0.0);
        // below the split view's holding priority: a low pane squeezes the map, not the list
        // above it
        shown_height.setPriority(200.0);
        layout::activate(&[
            map.topAnchor().constraintEqualToAnchor(&clip.topAnchor()),
            map.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()),
            map.widthAnchor().constraintEqualToAnchor(&clip.widthAnchor()),
            map_height.clone(),
            shown_height.clone(),
        ]);

        let mut specs = tables::specs(&PIECE_COLUMNS);
        specs[PIECE_BLOCKS_COLUMN].kind = CellKind::Drawn(Kind::BlockBar);
        let mut hooks = tables::hooks(Tab::Pieces);
        hooks.cell = Box::new(|row, col, _dark| {
            peek(|c| {
                let d = c.detail();
                let Some(p) = d.partial.get(row as usize) else { return CellValue::Empty };
                if col == PIECE_BLOCKS_COLUMN {
                    let text = format!("{} / {}", p.finished(), p.blocks.len());
                    CellValue::Drawn(
                        Box::new(Blocks { count: p.blocks.len(), runs: detail::block_runs(p) }),
                        text,
                    )
                } else {
                    CellValue::Text(d.cell(Tab::Pieces, row as usize, col))
                }
            })
            .unwrap_or(CellValue::Empty)
        });
        let list = Table::new(Tab::Pieces, specs, hooks, mtm);

        let view = NSView::new(mtm);
        for v in [&*summary as &NSView, &map_scroll, &list.scroll] {
            view.addSubview(v);
        }
        layout::activate(&[
            summary.topAnchor().constraintEqualToAnchor_constant(&view.topAnchor(), 6.0),
            summary.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), 8.0),
            summary.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&view.trailingAnchor(), -8.0),
            map_scroll.topAnchor().constraintEqualToAnchor_constant(&summary.bottomAnchor(), 4.0),
            map_scroll.leadingAnchor().constraintEqualToAnchor(&view.leadingAnchor()),
            map_scroll.trailingAnchor().constraintEqualToAnchor(&view.trailingAnchor()),
            list.scroll.topAnchor().constraintEqualToAnchor_constant(&map_scroll.bottomAnchor(), 4.0),
            list.scroll.leadingAnchor().constraintEqualToAnchor(&view.leadingAnchor()),
            list.scroll.trailingAnchor().constraintEqualToAnchor(&view.trailingAnchor()),
            list.scroll.bottomAnchor().constraintEqualToAnchor(&view.bottomAnchor()),
        ]);
        Pieces { view, summary, map, map_scroll, map_height, shown_height, list }
    }

    /// The summary, the map (sized for the pane's width) and the list.
    pub fn refresh(&self) {
        let Some((summary, shades)) = peek(|c| {
            let d = c.detail();
            let summary = match (&d.have, d.id) {
                (Some(_), _) => d.pieces_summary(),
                (None, Some(_)) => ui::text(tr!(PiecesWaiting)),
                (None, None) => ui::text(tr!(PiecesSelectTorrent)),
            };
            let n = d.have.as_ref().map_or(0, |h| h.len());
            (summary, (0..n).map(|i| d.piece_shade(i)).collect::<Vec<_>>())
        }) else {
            return;
        };
        if self.summary.stringValue().to_string() != summary {
            self.summary.setStringValue(&ui::ns(&summary));
        }
        let width = self.map_scroll.contentSize().width;
        let height = drawn::map_height(shades.len(), if width > 0.0 { width } else { 300.0 });
        if self.map_height.constant() != height {
            self.map_height.setConstant(height);
            self.shown_height.setConstant(height.min(MAP_MAX));
        }
        self.map.set_accessibility(&ui::text(lang::text(Tab::Pieces.title())), &summary);
        self.map.set_value(Box::new(Map { shades }));
        self.list.refresh();
    }
}

/// Registers the tab's painters.
pub fn register() {
    canvas::register(Kind::PieceMap, draw_map, None);
    canvas::register(Kind::BlockBar, draw_blocks, None);
}
