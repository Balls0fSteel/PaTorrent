//! Auto Layout helpers (convention 6: no fixed frames). Stacks and grids size themselves to
//! their content, so a longer translation or a right-to-left language fits by itself; the
//! constraint helpers pin a view to another's edges or give it a size.

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSGridCell, NSGridView, NSLayoutAttribute, NSLayoutConstraint, NSLayoutPriority, NSStackView,
    NSStackViewDistribution, NSUserInterfaceLayoutOrientation, NSView,
};
use objc2_foundation::{NSArray, NSEdgeInsets};

use super::controls::auto;

/// Spacing between related controls (Apple's 8 points).
pub const SPACING: f64 = 8.0;
/// A window's or a group's margin (Apple's 20 points).
pub const MARGIN: f64 = 20.0;

fn stack(
    views: &[&NSView],
    orientation: NSUserInterfaceLayoutOrientation,
    mtm: MainThreadMarker,
) -> Retained<NSStackView> {
    let s = NSStackView::stackViewWithViews(&NSArray::from_slice(views), mtm);
    s.setOrientation(orientation);
    s.setSpacing(SPACING);
    auto(&s);
    s
}

/// Views one under the other, leading edges aligned.
pub fn vstack(views: &[&NSView], mtm: MainThreadMarker) -> Retained<NSStackView> {
    let s = stack(views, NSUserInterfaceLayoutOrientation::Vertical, mtm);
    s.setAlignment(NSLayoutAttribute::Leading);
    s
}

/// Views side by side, centred vertically (in the reading direction: right to left in Arabic,
/// Persian and Hebrew).
pub fn hstack(views: &[&NSView], mtm: MainThreadMarker) -> Retained<NSStackView> {
    let s = stack(views, NSUserInterfaceLayoutOrientation::Horizontal, mtm);
    s.setAlignment(NSLayoutAttribute::CenterY);
    s
}

/// A stack whose views share the space equally.
pub fn fill_equally(s: &NSStackView) {
    s.setDistribution(NSStackViewDistribution::FillEqually);
}

/// The stack's inner margins.
pub fn insets(s: &NSStackView, top: f64, leading: f64, bottom: f64, trailing: f64) {
    s.setEdgeInsets(NSEdgeInsets { top, left: leading, bottom, right: trailing });
}

/// A grid of rows; `None` leaves a cell empty. Labels go in the first column, right-aligned by
/// [`label_column`], as Mac forms have them.
pub fn grid(rows: &[Vec<Option<&NSView>>], mtm: MainThreadMarker) -> Retained<NSGridView> {
    let empty = NSGridCell::emptyContentView(mtm);
    let rows: Vec<Retained<NSArray<NSView>>> = rows
        .iter()
        .map(|r| {
            let cells: Vec<&NSView> = r.iter().map(|c| c.unwrap_or(&empty)).collect();
            NSArray::from_slice(&cells)
        })
        .collect();
    let rows: Vec<&NSArray<NSView>> = rows.iter().map(|r| &**r).collect();
    let g = NSGridView::gridViewWithViews(&NSArray::from_slice(&rows), mtm);
    g.setRowSpacing(SPACING);
    g.setColumnSpacing(SPACING);
    auto(&g);
    g
}

/// The grid's first column right-aligned (the labels of a form).
pub fn label_column(g: &NSGridView) {
    if g.numberOfColumns() > 0 {
        g.columnAtIndex(0).setXPlacement(objc2_app_kit::NSGridCellPlacement::Trailing);
    }
}

/// Pins `view`'s four edges to `to`'s, `inset` points inside.
pub fn pin(view: &NSView, to: &NSView, inset: f64) {
    activate(&[
        view.topAnchor().constraintEqualToAnchor_constant(&to.topAnchor(), inset),
        view.bottomAnchor().constraintEqualToAnchor_constant(&to.bottomAnchor(), -inset),
        view.leadingAnchor().constraintEqualToAnchor_constant(&to.leadingAnchor(), inset),
        view.trailingAnchor().constraintEqualToAnchor_constant(&to.trailingAnchor(), -inset),
    ]);
}

/// Turns the constraints on.
pub fn activate(constraints: &[Retained<NSLayoutConstraint>]) {
    let refs: Vec<&NSLayoutConstraint> = constraints.iter().map(|c| &**c).collect();
    NSLayoutConstraint::activateConstraints(&NSArray::from_slice(&refs));
}

/// A width of at least `w` points.
pub fn min_width(view: &NSView, w: f64) {
    activate(&[view.widthAnchor().constraintGreaterThanOrEqualToConstant(w)]);
}

/// A fixed width.
pub fn width(view: &NSView, w: f64) {
    activate(&[view.widthAnchor().constraintEqualToConstant(w)]);
}

/// A fixed height.
pub fn height(view: &NSView, h: f64) {
    activate(&[view.heightAnchor().constraintEqualToConstant(h)]);
}

/// A view that stretches: it gives way first when its stack or grid grows.
pub fn stretch(view: &NSView) {
    let low: NSLayoutPriority = 1.0;
    view.setContentHuggingPriority_forOrientation(
        low,
        objc2_app_kit::NSLayoutConstraintOrientation::Horizontal,
    );
}

/// An empty view that takes the spare room in a stack.
pub fn spacer(mtm: MainThreadMarker) -> Retained<NSView> {
    let v = NSView::new(mtm);
    auto(&v);
    stretch(&v);
    v
}
