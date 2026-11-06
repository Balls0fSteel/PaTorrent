//! The geometry of the detail tabs' drawn views, for a GUI that draws them itself (the native
//! macOS one; GTK and Win32 keep their own copies): the General tab's piece bars, the Pieces
//! tab's map and the Speed graph's points. Only arithmetic: the toolkit draws.

use std::collections::VecDeque;

use crate::detail::SPEED_SAMPLES;

/// The piece map's squares: padding, side, gap (points).
pub const MAP_PAD: f64 = 4.0;
pub const MAP_CELL: f64 = 9.0;
pub const MAP_GAP: f64 = 1.0;
/// The distance from one square to the next.
pub const MAP_STEP: f64 = MAP_CELL + MAP_GAP;

/// Squares in a row of a map `width` wide (at least one).
pub fn map_columns(width: f64) -> usize {
    (((width - 2.0 * MAP_PAD) / MAP_STEP).floor() as i64).max(1) as usize
}

/// The map's height for `pieces` squares at `width`.
pub fn map_height(pieces: usize, width: f64) -> f64 {
    if pieces == 0 {
        return 0.0;
    }
    let rows = pieces.div_ceil(map_columns(width));
    2.0 * MAP_PAD + rows as f64 * MAP_STEP
}

/// The top-left corner of piece `i`'s square in a map of `cols` columns.
pub fn map_square(i: usize, cols: usize) -> (f64, f64) {
    let cols = cols.max(1);
    (MAP_PAD + (i % cols) as f64 * MAP_STEP, MAP_PAD + (i / cols) as f64 * MAP_STEP)
}

/// A piece bar `columns` points wide over `pieces` pieces, as runs `(first column, columns,
/// strength)`: each column's strength is the mean of `value` (0..=1) over its pieces, and
/// neighbours within 0.02 of a run's first share it (the Win32 client's `piece_bar`). Runs of
/// strength 0 are left out.
pub fn piece_bar_runs(
    pieces: usize,
    columns: usize,
    value: impl Fn(usize) -> f32,
) -> Vec<(usize, usize, f32)> {
    let mut out = Vec::new();
    if pieces == 0 || columns == 0 {
        return out;
    }
    let step = |col: usize| {
        let a = col * pieces / columns;
        let b = ((col + 1) * pieces / columns).max(a + 1).min(pieces);
        (a..b).map(&value).sum::<f32>() / (b - a) as f32
    };
    let mut col = 0;
    let mut next = step(0);
    while col < columns {
        let v = next;
        let mut end = col + 1;
        loop {
            if end >= columns {
                break;
            }
            next = step(end);
            if (next - v).abs() > 0.02 {
                break;
            }
            end += 1;
        }
        if v > 0.0 {
            out.push((col, end - col, v.min(1.0)));
        }
        col = end;
    }
    out
}

/// The Speed graph's lines in a plot `width` × `height` (top-left origin) scaled to `scale`
/// bytes per second: the download and the upload line's points, the newest sample at the right
/// edge, one sample per `(width - 2) / 599` points (at least one), none for fewer than two
/// samples.
pub fn speed_lines(
    speed: &VecDeque<(u32, u32)>,
    scale: u64,
    width: f64,
    height: f64,
) -> [Vec<(f64, f64)>; 2] {
    let n = speed.len();
    if n < 2 || scale == 0 {
        return [Vec::new(), Vec::new()];
    }
    let step = ((width - 2.0) / (SPEED_SAMPLES - 1) as f64).max(1.0);
    let point = |i: usize, v: u32| {
        let x = ((width - 1.0) - (n - 1 - i) as f64 * step).max(1.0);
        let y = (height - 1.0) - f64::from(v) / scale as f64 * (height - 2.0);
        (x, y)
    };
    [
        speed.iter().enumerate().map(|(i, &(d, _))| point(i, d)).collect(),
        speed.iter().enumerate().map(|(i, &(_, u))| point(i, u)).collect(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_piece_map_grows_a_row_at_a_time() {
        assert_eq!(map_height(0, 500.0), 0.0);
        // (500 - 8) / 10 = 49 squares a row
        assert_eq!(map_columns(500.0), 49);
        assert_eq!(map_height(49, 500.0), 8.0 + 10.0);
        assert_eq!(map_height(50, 500.0), 8.0 + 20.0);
        assert_eq!(map_square(50, 49), (4.0 + 10.0, 4.0 + 10.0));
        // never fewer than one column
        assert_eq!(map_columns(5.0), 1);
        assert_eq!(map_height(3, 5.0), 8.0 + 30.0);
    }

    #[test]
    fn piece_bars_merge_columns_of_the_same_strength() {
        // 4 pieces, the middle two done, over 8 columns: two columns a piece
        let have = [false, true, true, false];
        assert_eq!(piece_bar_runs(4, 8, |p| if have[p] { 1.0 } else { 0.0 }), vec![(2, 4, 1.0)]);
        // 1,000 pieces in 10 columns: a column's strength is its pieces' mean
        let runs = piece_bar_runs(1000, 10, |p| if p < 250 { 1.0 } else { 0.0 });
        assert_eq!(runs, vec![(0, 2, 1.0), (2, 1, 0.5)]);
        // more columns than pieces: each column shows one piece
        assert_eq!(piece_bar_runs(2, 5, |p| p as f32), vec![(3, 2, 1.0)]);
        assert!(piece_bar_runs(0, 10, |_| 1.0).is_empty());
        assert!(piece_bar_runs(10, 0, |_| 1.0).is_empty());
        // availability over 1 is full strength
        assert_eq!(piece_bar_runs(1, 3, |_| 4.0), vec![(0, 3, 1.0)]);
    }

    #[test]
    fn speed_lines_end_at_the_right_edge() {
        let mut s = VecDeque::new();
        assert_eq!(speed_lines(&s, 10_240, 601.0, 102.0), [Vec::new(), Vec::new()]);
        s.push_back((0, 10_240));
        s.push_back((5_120, 0));
        let [down, up] = speed_lines(&s, 10_240, 601.0, 102.0);
        // 599 points over 599 gaps: one point a sample, the newest at x = 600
        assert_eq!(down, vec![(599.0, 101.0), (600.0, 51.0)]);
        assert_eq!(up, vec![(599.0, 1.0), (600.0, 101.0)]);
        // a narrow plot still steps a point a sample, clamped at the left edge
        let full: VecDeque<(u32, u32)> = (0..SPEED_SAMPLES as u32).map(|i| (i, i)).collect();
        let [down, _] = speed_lines(&full, 10_240, 100.0, 50.0);
        assert_eq!(down.len(), SPEED_SAMPLES);
        assert_eq!(down[0].0, 1.0);
        assert_eq!(down[SPEED_SAMPLES - 1].0, 99.0);
    }
}
