//! The General tab, as µTorrent 2.2.1's: the Downloaded and Availability bars (drawn,
//! `canvas::Kind::PieceBar`), then the Transfer and General sections in a grid of bold labels
//! and their values (`detail::general`, and the address trackers see, `network::general_row`).
//! A hint stands in for them while no torrent is selected.

use std::any::Any;

use objc2::rc::Retained;
use objc2::{MainThreadMarker, Message};
use objc2_app_kit::{
    NSBox, NSBoxType, NSColor, NSFont, NSGridCellPlacement, NSGridView, NSLayoutConstraintOrientation,
    NSScrollView, NSTextAlignment, NSTextField, NSView,
};
use objc2_foundation::{NSRange, NSString};
use rookparcel_frontend::app::drawn;
use rookparcel_frontend::{detail, tr};

use crate::app::peek;
use crate::canvas::{self, Kind, Painter, RPCanvas, rect};
use crate::ui::{self, controls::auto, layout};

/// A piece bar's value: each piece's strength (0..=1) and its colour.
pub struct PieceBar {
    pub pieces: Vec<f32>,
    /// Availability (green) rather than Downloaded (blue).
    pub green: bool,
}

/// One step a piece: the empty bar, its frame, then each run of columns of one strength in the
/// bar's colour at that strength (the Win32 client's `piece_bar`).
pub fn draw_piece_bar(p: &Painter, value: &dyn Any) {
    let Some(bar) = value.downcast_ref::<PieceBar>() else { return };
    let r = rect(0.0, 0.0, p.width(), p.height());
    p.fill(r, &NSColor::textBackgroundColor());
    let color = if bar.green { NSColor::systemGreenColor() } else { NSColor::systemBlueColor() };
    let inner = (p.width() - 2.0).max(0.0) as usize;
    for (col, len, v) in drawn::piece_bar_runs(bar.pieces.len(), inner, |i| bar.pieces[i]) {
        let c = color.colorWithAlphaComponent(f64::from(v));
        p.fill(rect(1.0 + col as f64, 1.0, len as f64, p.height() - 2.0), &c);
    }
    p.frame(r, &NSColor::tertiaryLabelColor());
}

/// The tab's views.
pub struct General {
    pub view: Retained<NSView>,
    hint: Retained<NSTextField>,
    scroll: Retained<NSScrollView>,
    bars: [(Retained<NSTextField>, Retained<RPCanvas>, Retained<NSTextField>); 2],
    titles: [Retained<NSTextField>; 2],
    /// (label, value) in `detail::General`'s order: transfer, status, general, hash, then the
    /// address trackers see.
    pairs: Vec<(Retained<NSTextField>, Retained<NSTextField>)>,
}

fn small(bold: bool) -> Retained<NSFont> {
    let size = NSFont::smallSystemFontSize();
    if bold { NSFont::boldSystemFontOfSize(size) } else { NSFont::systemFontOfSize(size) }
}

/// A label of the grid: bold for the names; values truncate and can be selected (copied).
fn label(bold: bool, mtm: MainThreadMarker) -> Retained<NSTextField> {
    let l = ui::truncating_label("", mtm);
    l.setFont(Some(&small(bold)));
    if bold {
        l.setContentCompressionResistancePriority_forOrientation(
            750.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
    } else {
        l.setSelectable(true);
        // a long save path gives way, not the grid
        l.setContentCompressionResistancePriority_forOrientation(
            200.0,
            NSLayoutConstraintOrientation::Horizontal,
        );
        layout::stretch(&l);
    }
    l
}

/// A section heading: its bold title and a line to the right.
fn section(title: &NSTextField, mtm: MainThreadMarker) -> Retained<NSView> {
    let line = NSBox::new(mtm);
    line.setBoxType(NSBoxType::Separator);
    auto(&line);
    layout::stretch(&line);
    let s = layout::hstack(&[title, &line], mtm);
    Retained::into_super(s)
}

impl General {
    pub fn new(mtm: MainThreadMarker) -> General {
        let hint = ui::note(&ui::text(tr!(GenSelectTorrent)), mtm);

        // six columns: three pairs of a bold label and its value
        let mut rows: Vec<Vec<Option<Retained<NSView>>>> = Vec::new();
        let mut merges: Vec<(usize, usize, usize)> = Vec::new();
        let view = some;
        let bars: [(Retained<NSTextField>, Retained<RPCanvas>, Retained<NSTextField>); 2] =
            std::array::from_fn(|i| {
                let c = RPCanvas::new(
                    Kind::PieceBar,
                    Box::new(PieceBar { pieces: Vec::new(), green: i == 1 }),
                    mtm,
                );
                layout::height(&c, 12.0);
                layout::min_width(&c, 100.0);
                let v = label(false, mtm);
                v.setAlignment(NSTextAlignment::Right);
                layout::min_width(&v, 56.0);
                (label(true, mtm), c, v)
            });
        for (i, (l, c, v)) in bars.iter().enumerate() {
            rows.push(vec![view(l), view(c), None, None, None, view(v)]);
            merges.push((i, 1, 4));
        }
        let titles: [Retained<NSTextField>; 2] = std::array::from_fn(|_| label(true, mtm));
        let mut pairs = Vec::new();
        let pair = |pairs: &mut Vec<(Retained<NSTextField>, Retained<NSTextField>)>| {
            let p = (label(true, mtm), label(false, mtm));
            pairs.push(p.clone());
            p
        };
        rows.push(vec![Some(section(&titles[0], mtm)), None, None, None, None, None]);
        merges.push((rows.len() - 1, 0, 6));
        for _ in 0..4 {
            let mut row = Vec::new();
            for _ in 0..3 {
                let (l, v) = pair(&mut pairs);
                row.push(view(&l));
                row.push(view(&v));
            }
            rows.push(row);
        }
        let mut wide = |rows: &mut Vec<Vec<Option<Retained<NSView>>>>, pairs: &mut Vec<_>| {
            let (l, v) = pair(pairs);
            rows.push(vec![view(&l), view(&v), None, None, None, None]);
            merges.push((rows.len() - 1, 1, 5));
        };
        // the status line
        wide(&mut rows, &mut pairs);
        rows.push(vec![Some(section(&titles[1], mtm)), None, None, None, None, None]);
        let heading = rows.len() - 1;
        for _ in 0..3 {
            let (l, v) = pair(&mut pairs);
            let (l2, v2) = pair(&mut pairs);
            rows.push(vec![view(&l), view(&v), None, None, view(&l2), view(&v2)]);
        }
        let general_rows = (heading + 1)..rows.len();
        // the hash, then "Your address as trackers see it" (D54)
        wide(&mut rows, &mut pairs);
        wide(&mut rows, &mut pairs);
        merges.push((heading, 0, 6));
        for r in general_rows {
            merges.push((r, 1, 3));
        }

        let refs: Vec<Vec<Option<&NSView>>> =
            rows.iter().map(|r| r.iter().map(|c| c.as_deref()).collect()).collect();
        let grid: Retained<NSGridView> = layout::grid(&refs, mtm);
        grid.setRowSpacing(3.0);
        for (row, col, len) in merges {
            grid.mergeCellsInHorizontalRange_verticalRange(NSRange::new(col, len), NSRange::new(row, 1));
        }
        // the three value columns share the width, as µTorrent's three pairs do
        for i in 0..grid.numberOfColumns() {
            grid.columnAtIndex(i).setXPlacement(NSGridCellPlacement::Fill);
        }
        let first = &pairs[0].1;
        layout::activate(&[
            pairs[1].1.widthAnchor().constraintEqualToAnchor(&first.widthAnchor()),
            pairs[2].1.widthAnchor().constraintEqualToAnchor(&first.widthAnchor()),
        ]);

        // the grid scrolls when the pane is low; a stack (flipped) keeps it at the top
        let doc = layout::vstack(&[&grid], mtm);
        layout::insets(&doc, 8.0, 8.0, 8.0, 8.0);
        let scroll = NSScrollView::new(mtm);
        auto(&scroll);
        scroll.setHasVerticalScroller(true);
        scroll.setAutohidesScrollers(true);
        scroll.setDrawsBackground(false);
        scroll.setDocumentView(Some(&doc));
        let clip = scroll.contentView();
        layout::activate(&[
            doc.topAnchor().constraintEqualToAnchor(&clip.topAnchor()),
            doc.leadingAnchor().constraintEqualToAnchor(&clip.leadingAnchor()),
            doc.widthAnchor().constraintEqualToAnchor(&clip.widthAnchor()),
            grid.trailingAnchor().constraintEqualToAnchor_constant(&doc.trailingAnchor(), -8.0),
        ]);

        let view = NSView::new(mtm);
        view.addSubview(&scroll);
        view.addSubview(&hint);
        layout::pin(&scroll, &view, 0.0);
        layout::activate(&[
            hint.topAnchor().constraintEqualToAnchor_constant(&view.topAnchor(), 12.0),
            hint.leadingAnchor().constraintEqualToAnchor_constant(&view.leadingAnchor(), 12.0),
            hint.trailingAnchor().constraintLessThanOrEqualToAnchor_constant(&view.trailingAnchor(), -12.0),
        ]);
        scroll.setHidden(true);
        General { view, hint, scroll, bars, titles, pairs }
    }

    /// Shows the selected torrent (or the hint).
    pub fn refresh(&self) {
        let data = peek(|c| {
            let (st, paused) = c.detail_torrent()?;
            let d = c.detail();
            let g = detail::general(st, paused);
            let downloaded: Vec<f32> = match &d.have {
                Some(h) => (0..h.len()).map(|p| if h.get(p) { 1.0 } else { 0.0 }).collect(),
                // before the first piece reply: the progress alone
                None => {
                    let done = st.progress_ppm as usize;
                    (0..1000).map(|p| if p * 1000 < done { 1.0 } else { 0.0 }).collect()
                }
            };
            let have = d.have.as_ref();
            let available: Vec<f32> = d
                .availability
                .iter()
                .enumerate()
                .map(|(p, &n)| if have.is_some_and(|h| h.get(p)) { 1.0 } else { (n as f32 / 3.0).min(1.0) })
                .collect();
            Some((g, [downloaded, available]))
        })
        .flatten();
        let Some((g, pieces)) = data else {
            self.scroll.setHidden(true);
            self.hint.setHidden(false);
            return;
        };
        self.hint.setHidden(true);
        self.scroll.setHidden(false);
        set(&self.titles[0], &ui::text(tr!(GenTransfer)));
        set(&self.titles[1], &ui::text(tr!(GenGeneral)));
        for (((l, c, v), (label, value)), (i, pieces)) in
            self.bars.iter().zip(&g.bars).zip(pieces.into_iter().enumerate())
        {
            set(l, &ui::text(label));
            set(v, value);
            c.set_accessibility(&ui::text(label), value);
            c.set_value(Box::new(PieceBar { pieces, green: i == 1 }));
        }
        let external = rookparcel_frontend::network::general_row();
        let texts = g
            .transfer
            .iter()
            .flatten()
            .chain([&g.status])
            .chain(g.general.iter().flatten())
            .chain([&g.hash])
            .map(|(l, v)| (*l, v.clone()))
            .chain([external]);
        for ((l, v), (label, value)) in self.pairs.iter().zip(texts) {
            set(l, &ui::text(label));
            set(v, &value);
            ui::tooltip(v, &value);
        }
    }
}

/// Sets a label's text when it changed (a selection in a value stays).
fn set(l: &NSTextField, text: &str) {
    if l.stringValue().to_string() != text {
        l.setStringValue(&NSString::from_str(text));
    }
}

/// A grid cell.
fn some(v: &NSView) -> Option<Retained<NSView>> {
    Some(v.retain())
}

/// Registers the tab's painter.
pub fn register() {
    canvas::register(Kind::PieceBar, draw_piece_bar, None);
}
