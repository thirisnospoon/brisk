//! Soft rounded fills for selections, search hits, occurrences and bracket matches. A fill that
//! spans several rows is drawn as one shape: only its outer (convex) corners are rounded, so the
//! rows join without notches. Edges are snapped to device pixels so the anti-aliased curves stay
//! clean.

use gpui::{fill, point, px, Bounds, Corners, Hsla, PaintQuad, Pixels};

/// Selection corners, as in VS Code / Fleet.
pub(super) const SELECTION: f32 = 3.;
/// Search hits, occurrences and bracket matches: small rounded boxes.
pub(super) const MARK: f32 = 3.;

/// One row of a fill: (top, bottom, left, right) in logical pixels.
pub(super) type Row = (f32, f32, f32, f32);

fn snap(v: f32, scale: f32) -> f32 {
    (v * scale).round() / scale
}

/// Corner radii [top_left, top_right, bottom_right, bottom_left] for each row of a shape whose
/// rows touch vertically: a corner is rounded unless the neighbor row covers it.
pub(super) fn radii(rows: &[Row], r: f32) -> Vec<[f32; 4]> {
    let covers =
        |n: Option<&Row>, x: f32, left: bool| n.is_some_and(|&(_, _, a, b)| if left { a <= x + 0.5 && b > x + 0.5 } else { a < x - 0.5 && b >= x - 0.5 });
    rows.iter()
        .enumerate()
        .map(|(i, row)| {
            let &(top, bottom, x0, x1) = row;
            let above = i.checked_sub(1).and_then(|j| rows.get(j)).filter(|p| (p.1 - top).abs() < 0.5);
            let below = rows.get(i + 1).filter(|n| (n.0 - bottom).abs() < 0.5);
            let max = (r).min((x1 - x0) / 2.).min((bottom - top) / 2.).max(0.);
            let c = |covered: bool| if covered { 0. } else { max };
            [c(covers(above, x0, true)), c(covers(above, x1, false)), c(covers(below, x1, false)), c(covers(below, x0, true))]
        })
        .collect()
}

/// Quads for `rows` (in order, top to bottom) with rounded outer corners.
pub(super) fn push(rows: &[Row], r: f32, scale: f32, color: Hsla, out: &mut Vec<PaintQuad>) {
    let rows: Vec<Row> = rows.iter().map(|&(t, b, a, z)| (snap(t, scale), snap(b, scale), snap(a, scale), snap(z, scale))).filter(|r| r.3 > r.2).collect();
    for (&(t, b, a, z), k) in rows.iter().zip(radii(&rows, r)) {
        let corners = Corners { top_left: px(k[0]), top_right: px(k[1]), bottom_right: px(k[2]), bottom_left: px(k[3]) };
        out.push(fill(Bounds::from_corners(point(px(a), px(t)), point(px(z), px(b))), color).corner_radii(corners));
    }
}

/// A row from pixel values.
pub(super) fn row(top: Pixels, bottom: Pixels, left: Pixels, right: Pixels) -> Row {
    (f32::from(top), f32::from(bottom), f32::from(left), f32::from(right))
}

impl super::Editor {
    /// Smoke: open the find bar with `query` (its hits are drawn rounded).
    #[cfg(feature = "smoke")]
    pub fn find_for_smoke(&mut self, query: &str, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        self.open_find(false, window, cx);
        if let Some(f) = &self.find {
            f.query.update(cx, |q, cx| {
                q.buffer.set_text(query);
                cx.notify();
            });
        }
        self.refresh_find(false, cx);
    }

    /// Smoke: one selection from `anchor` to `head` (chars), the editor focused so the caret,
    /// its bracket match and the active indent guide show.
    #[cfg(feature = "smoke")]
    pub fn select_for_smoke(&mut self, anchor: usize, head: usize, window: &mut gpui::Window, cx: &mut gpui::Context<Self>) {
        self.buffer.selections = vec![brisk_text::Selection { anchor, head, goal_col: None }];
        window.focus(&self.focus);
        cx.notify();
    }

    /// Smoke: the caret's block guide as (column, first line, last line), 1-based lines.
    #[cfg(feature = "smoke")]
    pub fn active_guide_for_smoke(&self) -> Option<(usize, usize, usize)> {
        self.guides.active.map(|(c, a, z)| (c, a + 1, z + 1))
    }

    /// Smoke: the find hits' count.
    #[cfg(feature = "smoke")]
    pub fn find_hits_for_smoke(&self) -> usize {
        self.find.as_ref().map_or(0, |f| f.matches.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outer_corners_only() {
        // A selection from the middle of line 1 to the middle of line 3.
        let rows = [(0., 20., 50., 200.), (20., 40., 10., 180.), (40., 60., 10., 90.)];
        let k = radii(&rows, 3.);
        assert_eq!(k[0], [3., 3., 3., 0.], "row 1: bottom-right is exposed, bottom-left lies on row 2");
        assert_eq!(k[1], [3., 0., 3., 0.], "row 2 sticks out left above and right below");
        assert_eq!(k[2], [0., 0., 3., 3.]);
        // One row: all four corners, at most half its width.
        assert_eq!(radii(&[(0., 20., 0., 4.)], 3.), vec![[2.; 4]]);
        // Rows that do not touch (a fold between them) are separate shapes.
        assert_eq!(radii(&[(0., 20., 0., 50.), (40., 60., 0., 50.)], 3.), vec![[3.; 4], [3.; 4]]);
    }
}
