//! Code folding, as in PyCharm: Ctrl+- / Ctrl+= at the caret, Ctrl+Shift+- / Ctrl+Shift+=
//! for the whole file, ▸/▾ markers in the gutter, a "..." placeholder after the header line.
//! A collapsed region hides the lines after its header; folds follow edits and open when a
//! caret lands inside them.

use super::Editor;

#[derive(Default)]
pub struct Folds {
    /// Collapsed regions as char positions: (end of the header line, end of the last hidden line).
    regions: Vec<(usize, usize)>,
    /// Buffer version `regions` and `hidden` are up to date with.
    version: u64,
    /// Hidden lines, merged and sorted: (first, last) inclusive.
    hidden: Vec<(usize, usize)>,
    /// Foldable regions (first line, last line) for buffer version `.0`.
    pub(super) candidates: (Option<u64>, Vec<(usize, usize)>),
}

/// Regions from indentation, for files without a grammar: a line followed by more deeply
/// indented lines.
fn indent_regions(lines: &[String]) -> Vec<(usize, usize)> {
    let indent = |s: &str| -> Option<usize> { (!s.trim().is_empty()).then(|| s.chars().take_while(|c| c.is_whitespace()).count()) };
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let Some(base) = indent(l) else { continue };
        let mut last = i;
        for (j, m) in lines.iter().enumerate().skip(i + 1) {
            match indent(m) {
                Some(d) if d > base => last = j,
                Some(_) => break,
                None => {}
            }
        }
        if last > i {
            out.push((i, last));
        }
    }
    out
}

impl Editor {
    /// Bring folds up to date with edits and open those a caret moved into. Cheap when
    /// nothing changed.
    pub(super) fn sync_folds(&mut self) {
        let v = self.buffer.version();
        let mut changed = false;
        if self.folds.version != v {
            match self.buffer.changes_since(self.folds.version) {
                Some(ch) => {
                    for r in &mut self.folds.regions {
                        r.0 = brisk_text::map_through(r.0, &ch);
                        r.1 = brisk_text::map_through(r.1, &ch);
                    }
                }
                None => self.folds.regions.clear(),
            }
            self.folds.version = v;
            changed = true;
        }
        if self.folds.regions.is_empty() {
            if changed || !self.folds.hidden.is_empty() {
                self.folds.hidden.clear();
            }
            return;
        }
        // Normalize to whole lines; drop regions that collapsed to nothing.
        let b = &self.buffer;
        let len = b.len_chars();
        let mut regions: Vec<(usize, usize)> = self
            .folds
            .regions
            .iter()
            .filter_map(|&(s, e)| {
                let (h, _) = b.pos_to_line_col(s.min(len));
                let (last, _) = b.pos_to_line_col(e.min(len));
                (last > h).then(|| (b.line_end(h), b.line_end(last)))
            })
            .collect();
        // A caret inside a fold opens it.
        regions.retain(|&(s, e)| !b.selections.iter().any(|sel| sel.head > s && sel.head <= e));
        regions.sort();
        regions.dedup();
        if changed || regions != self.folds.regions {
            self.folds.regions = regions;
            self.rebuild_hidden();
        }
    }

    fn rebuild_hidden(&mut self) {
        let b = &self.buffer;
        let mut lines: Vec<(usize, usize)> = self
            .folds
            .regions
            .iter()
            .map(|&(s, e)| (b.pos_to_line_col(s).0 + 1, b.pos_to_line_col(e).0))
            .filter(|(a, z)| a <= z)
            .collect();
        lines.sort();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(lines.len());
        for (a, z) in lines {
            match merged.last_mut() {
                Some(m) if a <= m.1 + 1 => m.1 = m.1.max(z),
                _ => merged.push((a, z)),
            }
        }
        self.folds.hidden = merged;
    }

    /// The display row of a buffer line (a hidden line maps to the row after its header).
    pub fn row_of(&self, line: usize) -> usize {
        let mut hidden = 0;
        for &(a, z) in &self.folds.hidden {
            if a > line {
                break;
            }
            if line <= z {
                // Inside: only the hidden lines before it count.
                hidden += line - a;
                break;
            }
            hidden += z + 1 - a;
        }
        line - hidden
    }

    /// The buffer line shown on a display row.
    pub fn line_of_row(&self, row: usize) -> usize {
        let mut line = row;
        for &(a, z) in &self.folds.hidden {
            if a > line {
                break;
            }
            line += z + 1 - a;
        }
        line.min(self.buffer.len_lines().saturating_sub(1))
    }

    pub fn display_rows(&self) -> usize {
        let hidden: usize = self.folds.hidden.iter().map(|&(a, z)| z + 1 - a).sum();
        self.buffer.len_lines().saturating_sub(hidden).max(1)
    }

    /// The next shown line after `line`.
    pub fn next_shown_line(&self, line: usize) -> usize {
        let mut l = line + 1;
        for &(a, z) in &self.folds.hidden {
            if a <= l && l <= z {
                l = z + 1;
            }
        }
        l
    }

    /// Whether this line is the header of a collapsed region.
    pub fn is_folded_header(&self, line: usize) -> bool {
        self.folds.hidden.iter().any(|&(a, _)| a == line + 1)
    }

    /// Foldable regions (cached per buffer version).
    pub(super) fn fold_candidates(&mut self) -> &[(usize, usize)] {
        let v = self.buffer.version();
        if self.folds.candidates.0 != Some(v) {
            self.reparse();
            let c = match &self.syntax {
                Some(s) => s.fold_ranges(),
                None if self.buffer.len_lines() <= 50_000 => {
                    let lines: Vec<String> = (0..self.buffer.len_lines()).map(|l| self.buffer.line(l)).collect();
                    indent_regions(&lines)
                }
                None => Vec::new(),
            };
            self.folds.candidates = (Some(v), c);
        }
        &self.folds.candidates.1
    }

    /// Lines that start a foldable region (gutter markers).
    /// In a long file the markers come from `schedule_folds` (off the UI thread, after a
    /// pause in typing) and may be a moment stale; folding commands still use fresh regions.
    pub(super) fn fold_headers(&mut self) -> Vec<usize> {
        if !self.lazy_folds() || self.folds.candidates.0.is_none() {
            self.fold_candidates();
        }
        let mut h: Vec<usize> = self.folds.candidates.1.iter().map(|r| r.0).collect();
        h.dedup();
        h
    }

    fn collapse_lines(&mut self, first: usize, last: usize) {
        let b = &self.buffer;
        let r = (b.line_end(first), b.line_end(last));
        if !self.folds.regions.contains(&r) {
            self.folds.regions.push(r);
            self.folds.regions.sort();
        }
        // Carets inside move to the header so the fold stays closed.
        let head_ok = |p: usize| p <= r.0 || p > r.1;
        for s in &mut self.buffer.selections {
            if !head_ok(s.head) || !head_ok(s.anchor) {
                *s = brisk_text::Selection::caret(r.0);
            }
        }
        self.rebuild_hidden();
    }

    /// Open the fold whose header is `line`. True if there was one.
    fn expand_header(&mut self, line: usize) -> bool {
        let b = &self.buffer;
        let n = self.folds.regions.len();
        let end = b.line_end(line);
        self.folds.regions.retain(|r| r.0 != end);
        let opened = self.folds.regions.len() != n;
        if opened {
            self.rebuild_hidden();
        }
        opened
    }

    /// Gutter marker click: collapse or expand the region starting at `line`.
    pub fn toggle_fold(&mut self, line: usize) {
        self.sync_folds();
        if self.expand_header(line) {
            return;
        }
        if let Some(&(a, z)) = self.fold_candidates().iter().find(|r| r.0 == line) {
            self.collapse_lines(a, z);
        }
    }

    /// Ctrl+-: the innermost open region around the caret.
    pub fn collapse_at_caret(&mut self) {
        self.sync_folds();
        let (line, _) = self.caret_line_col();
        let folded: Vec<usize> = self.folds.regions.iter().map(|r| self.buffer.pos_to_line_col(r.0).0).collect();
        let pick = self
            .fold_candidates()
            .iter()
            .filter(|r| r.0 <= line && line <= r.1 && !folded.contains(&r.0))
            .min_by_key(|r| r.1 - r.0)
            .copied();
        if let Some((a, z)) = pick {
            self.collapse_lines(a, z);
        }
    }

    /// Ctrl+=: the fold on the caret's line.
    pub fn expand_at_caret(&mut self) {
        self.sync_folds();
        let (line, _) = self.caret_line_col();
        self.expand_header(line);
    }

    pub fn collapse_all(&mut self) {
        self.sync_folds();
        let all = self.fold_candidates().to_vec();
        for (a, z) in all {
            self.collapse_lines(a, z);
        }
    }

    /// On opening a Python file, as PyCharm does by default: fold the first group of imports
    /// (two or more lines) down to its first line.
    pub fn collapse_imports(&mut self) {
        if !self.is_python() {
            return;
        }
        let is_import = |l: &str| l.starts_with("import ") || l.starts_with("from ");
        let candidates = self.fold_candidates().to_vec();
        let group = candidates.into_iter().find(|&(a, z)| {
            (a..=z).all(|i| {
                let l = self.buffer.line(i);
                is_import(&l) || l.trim().is_empty() || l.starts_with([' ', '\t', ')']) || l.trim_start().starts_with('#')
            }) && is_import(&self.buffer.line(a))
        });
        if let Some((a, z)) = group {
            self.collapse_lines(a, z);
        }
    }

    pub fn expand_all(&mut self) {
        self.folds.regions.clear();
        self.folds.hidden.clear();
    }

    /// After Up/Down: carets that landed on hidden lines jump over the fold.
    pub(super) fn skip_folded_lines(&mut self, down: bool) {
        if self.folds.hidden.is_empty() {
            return;
        }
        let hidden = self.folds.hidden.clone();
        let n = self.buffer.len_lines();
        for i in 0..self.buffer.selections.len() {
            let s = self.buffer.selections[i];
            let (line, col) = self.buffer.pos_to_line_col(s.head);
            let Some(&(a, z)) = hidden.iter().find(|&&(a, z)| a <= line && line <= z) else { continue };
            let target = if down && z + 1 < n { z + 1 } else { a - 1 };
            let col = s.goal_col.unwrap_or(col).min(self.buffer.line_len(target));
            let pos = self.buffer.line_col_to_pos(target, col);
            let sel = &mut self.buffer.selections[i];
            sel.head = pos;
            if s.is_empty() {
                sel.anchor = pos;
            }
        }
    }

    /// Smoke checks: (collapsed regions as line pairs, display rows).
    pub fn fold_state(&self) -> (Vec<(usize, usize)>, usize) {
        let b = &self.buffer;
        (self.folds.regions.iter().map(|&(s, e)| (b.pos_to_line_col(s).0, b.pos_to_line_col(e).0)).collect(), self.display_rows())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn indent_regions() {
        let lines: Vec<String> = ["a:", "  b", "", "  c:", "    d", "e"].iter().map(|s| s.to_string()).collect();
        assert_eq!(super::indent_regions(&lines), vec![(0, 4), (3, 4)]);
    }
}

impl Editor {
    /// The pointer is on a fold marker in the gutter (the hand cursor).
    pub(super) fn fold_marker_at(&mut self, p: gpui::Point<gpui::Pixels>) -> bool {
        let Some(l) = self.layout.as_ref() else { return false };
        if !self.show_gutter || p.x >= l.text_left || p.x < l.text_left - gpui::px(28.) {
            return false;
        }
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / super::line_height()).floor().max(0.0) as usize);
        self.is_folded_header(line) || self.fold_headers().binary_search(&line).is_ok()
    }

    /// A click on a fold marker in the gutter, or on the "..." of a collapsed header.
    pub(super) fn fold_click(&mut self, p: gpui::Point<gpui::Pixels>) -> bool {
        let Some(l) = self.layout.as_ref() else { return false };
        if !self.show_gutter {
            return false;
        }
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / super::line_height()).floor().max(0.0) as usize);
        let gutter_right = l.text_left - gpui::px(8.);
        if p.x < l.text_left && p.x >= gutter_right - gpui::px(20.) {
            let header = self.is_folded_header(line) || self.fold_headers().binary_search(&line).is_ok();
            if header {
                self.toggle_fold(line);
                return true;
            }
            return false;
        }
        if self.is_folded_header(line) {
            let Some(l) = self.layout.as_ref() else { return false };
            let text = self.buffer.line(line);
            let end_x = match l.shaped(line) {
                Some(s) => l.text_x + s.x_for_index(self.display_byte(line, &text, text.len() + 1) - 1),
                None => l.text_x + l.char_w * text.chars().count() as f32,
            };
            if p.x >= end_x + l.char_w * 0.5 && p.x <= end_x + l.char_w * 5. {
                self.expand_header(line);
                return true;
            }
        }
        false
    }
}

impl Editor {
    /// Smoke checks: the window point of a line's fold marker, if the line is on screen.
    pub fn fold_marker_point(&self, line: usize) -> Option<gpui::Point<gpui::Pixels>> {
        let l = self.layout.as_ref()?;
        l.rows.binary_search(&line).ok()?;
        let y = l.bounds.top() + gpui::px(self.row_of(line) as f32 * super::line_height() - self.scroll_y + super::line_height() / 2.);
        Some(gpui::point(l.text_left - gpui::px(17.), y))
    }

    /// Smoke checks: brackets and occurrences around the caret, as (line, column) pairs.
    pub fn caret_marks_for_smoke(&mut self) -> (Vec<(usize, usize)>, usize) {
        self.reparse();
        self.fill_occurrences();
        let m = self.caret_marks(0..self.text_cache.1.len());
        (m.brackets.iter().map(|r| self.buffer.pos_to_line_col(r.start)).collect(), m.occurrences.len())
    }
}
