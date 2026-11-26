//! Soft wraps, as in PyCharm (View › Active Editor › Soft-Wrap; on by default for the files in
//! Settings › Editor › General › Soft Wraps): a line longer than the text area continues on
//! the next rows, broken after a space when there is one, the continuation rows indented like
//! the line ("Use original line's indent for wrapped parts").
//!
//! The break points are cached per line and rebuilt only when the text, the width or the folds
//! change; lines that fit cost a length lookup. With soft wraps off nothing here runs.

use super::{Editor, Selection};
use std::collections::HashMap;

#[derive(Default)]
pub(super) struct Wrap {
    /// Soft-Wrap is on for this editor.
    pub on: bool,
    /// Text area width in columns (0: not laid out yet).
    cols: usize,
    /// (buffer version, columns, folds, indent options) the cache is for.
    key: Option<(u64, usize, u64, bool, usize)>,
    /// Lines that wrap: the columns where their 2nd, 3rd… rows start.
    breaks: HashMap<usize, Vec<usize>>,
    /// Continuation indent (columns) of the lines that wrap.
    indents: HashMap<usize, usize>,
    /// Display row of each line's first row; one more entry: the total. Hidden (folded) lines
    /// take no rows.
    first_row: Vec<usize>,
    /// Long lines' break points and indents by the hash of their text, for the columns and
    /// indent options in `memo_key`.
    memo: HashMap<u64, (Vec<usize>, usize)>,
    memo_key: Option<(usize, bool, usize)>,
}

/// Where the rows of a line of `len` chars break, `width` columns per row, the continuation
/// rows `indent` columns in: after the last space that fits, else at the width.
pub(super) fn break_line(text: &[char], width: usize, indent: usize) -> Vec<usize> {
    let mut out = Vec::new();
    let width = width.max(8);
    let indent = indent.min(width / 2);
    let mut start = 0;
    let mut cap = width;
    while text.len() - start > cap {
        let end = start + cap;
        // The last space in the row (keep it on this row, the next one starts after it).
        let brk = (start + 1..=end).rev().find(|&i| text[i - 1] == ' ' && text[i] != ' ').unwrap_or(end);
        out.push(brk);
        start = brk;
        cap = width - indent;
    }
    out
}

impl Editor {
    pub fn soft_wrap(&self) -> bool {
        self.wrap.on
    }

    /// View › Active Editor › Soft-Wrap.
    pub fn toggle_soft_wrap(&mut self) {
        self.wrap.on = !self.wrap.on;
        self.wrap.key = None;
        self.scroll_x = 0.0;
    }

    /// Smoke checks: rows, the caret, its row, scrolling, the first wrapped line's breaks.
    #[cfg(feature = "smoke")]
    pub fn wrap_smoke(&self) -> String {
        let (l, c) = self.caret_line_col();
        let vh = self.layout.as_ref().map(|l| f32::from(l.bounds.size.height)).unwrap_or(0.0);
        let first = (0..self.buffer.len_lines()).find(|&l| !self.wrap_breaks(l).is_empty());
        format!(
            "on={} active={} cols={} rows={} lines={} caret=({l},{c}) caret_row={} scroll_y={:.0} vh={vh:.0} lh={:.1} scroll_x={} breaks{:?}={:?}",
            self.wrap.on,
            self.wrap_active(),
            self.wrap.cols,
            self.display_rows(),
            self.buffer.len_lines(),
            self.caret_row(),
            self.scroll_y,
            super::line_height(),
            self.scroll_x,
            first,
            first.map(|f| self.wrap_breaks(f).to_vec()),
        )
    }

    /// Smoke checks: what a click at display row `row`, `cols` columns into the text, hits.
    #[cfg(feature = "smoke")]
    pub fn wrap_click(&self, row: usize, cols: f32) -> Option<(usize, usize)> {
        let l = self.layout.as_ref()?;
        let y = l.bounds.top() + gpui::px((row as f32 + 0.5) * super::line_height() - self.scroll_y);
        let x = l.text_left + l.char_w * cols;
        self.pos_for_point(gpui::point(x, y)).map(|p| self.buffer.pos_to_line_col(p))
    }

    pub(super) fn wrap_active(&self) -> bool {
        self.wrap.on && self.wrap.cols > 0 && !self.is_field && self.wrap.first_row.len() == self.buffer.len_lines() + 1
    }

    /// The layout's width changed (or text, folds): bring the break points up to date.
    pub(super) fn sync_wrap(&mut self, cols: usize) {
        if !self.wrap.on || self.is_field {
            return;
        }
        if cols != self.wrap.cols {
            self.wrap.cols = cols;
        }
        self.refresh_wrap();
    }

    /// Rebuild the cache if the text or the folds changed since (after an edit, before rows
    /// are asked for).
    pub(super) fn refresh_wrap(&mut self) {
        if !self.wrap.on || self.wrap.cols == 0 {
            return;
        }
        let folds: u64 =
            self.folds.hidden.iter().fold(self.folds.hidden.len() as u64, |h, &(a, z)| h.wrapping_mul(31).wrapping_add((a as u64) << 20 ^ z as u64));
        let indent_on = brisk_settings::soft_wrap_indent();
        let extra = brisk_settings::soft_wrap_extra_indent();
        let key = (self.buffer.version(), self.wrap.cols, folds, indent_on, extra);
        if self.wrap.key == Some(key) {
            return;
        }
        self.scroll_x = 0.0;
        let n = self.buffer.len_lines();
        let cols = self.wrap.cols;
        // Break points of the long lines, by their text: an edit recomputes only the lines it
        // touched; the rest is one pass over the text (lines that fit cost a length check).
        let memo_key = (cols, indent_on, extra);
        if self.wrap.memo_key != Some(memo_key) {
            self.wrap.memo.clear();
            self.wrap.memo_key = Some(memo_key);
        }
        self.text();
        let text = &self.text_cache.1;
        let mut old = std::mem::take(&mut self.wrap.memo);
        let mut memo = HashMap::new();
        let mut breaks = HashMap::new();
        let mut indents = HashMap::new();
        let mut first_row = Vec::with_capacity(n + 1);
        let mut row = 0usize;
        let mut hidden = self.folds.hidden.iter().peekable();
        for (l, line) in text.split('\n').enumerate() {
            first_row.push(row);
            while hidden.peek().is_some_and(|&&(_, z)| z < l) {
                hidden.next();
            }
            if hidden.peek().is_some_and(|&&(a, z)| a <= l && l <= z) {
                continue;
            }
            row += 1;
            let line = line.strip_suffix('\r').unwrap_or(line);
            if line.len() <= cols || line.chars().count() <= cols {
                continue;
            }
            let h = line.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3));
            let entry = match old.remove(&h).or_else(|| memo.get(&h).cloned()) {
                Some(e) => e,
                None => {
                    let chars: Vec<char> = line.chars().collect();
                    let ws = if indent_on { chars.iter().take_while(|c| **c == ' ' || **c == '\t').count() + extra } else { 0 };
                    (break_line(&chars, cols, ws), ws.min(cols.max(8) / 2))
                }
            };
            if !entry.0.is_empty() {
                row += entry.0.len();
                indents.insert(l, entry.1);
                breaks.insert(l, entry.0.clone());
            }
            memo.insert(h, entry);
        }
        first_row.push(row);
        self.wrap.memo = memo;
        self.wrap.breaks = breaks;
        self.wrap.indents = indents;
        self.wrap.first_row = first_row;
        self.wrap.key = Some(key);
    }

    pub(super) fn wrap_cols(&self) -> usize {
        self.wrap.cols
    }

    /// A click on a wrapped line: the column under `x` on display row `row`.
    pub(super) fn wrap_pos_for_point(&self, l: &super::LayoutCache, line: usize, row: usize, x: gpui::Pixels) -> usize {
        let sub = row.saturating_sub(self.row_of(line)).min(self.wrap_breaks(line).len());
        let (ts, te) = self.wrap_segment(line, sub);
        let last = sub == self.wrap_breaks(line).len();
        let max = if last { te } else { te.saturating_sub(1).max(ts) };
        let text = self.buffer.line(line);
        let col = match l.shaped(line) {
            Some(shaped) => {
                let x0 = shaped.x_for_index(self.display_byte(line, &text, super::char_to_byte(&text, ts)));
                let lead = if sub > 0 { l.char_w * self.wrap_indent(line) as f32 } else { gpui::px(0.) };
                let local = (x - l.text_x - lead).max(gpui::px(0.)) + x0;
                super::byte_to_char(&text, self.text_byte(line, &text, shaped.closest_index_for_x(local)))
            }
            None => ts + (f32::from(x - l.text_x) / f32::from(l.char_w)).round().max(0.0) as usize,
        };
        self.buffer.line_col_to_pos(line, col.clamp(ts, max))
    }

    pub(super) fn wrap_first_row(&self, line: usize) -> usize {
        self.wrap.first_row[line.min(self.wrap.first_row.len() - 1)]
    }

    pub(super) fn wrap_line_of_row(&self, row: usize) -> usize {
        let n = self.buffer.len_lines();
        let i = self.wrap.first_row[..n].partition_point(|&r| r <= row);
        i.saturating_sub(1).min(n.saturating_sub(1))
    }

    pub(super) fn wrap_total_rows(&self) -> usize {
        self.wrap.first_row.last().copied().unwrap_or(1).max(1)
    }

    /// The columns where the rows of `line` after the first start (empty: one row).
    pub(super) fn wrap_breaks(&self, line: usize) -> &[usize] {
        if !self.wrap_active() {
            return &[];
        }
        self.wrap.breaks.get(&line).map(Vec::as_slice).unwrap_or(&[])
    }

    pub(super) fn wrap_indent(&self, line: usize) -> usize {
        self.wrap.indents.get(&line).copied().unwrap_or(0)
    }

    /// The row of `line` that shows column `col` (0: the first).
    pub(super) fn wrap_sub(&self, line: usize, col: usize) -> usize {
        self.wrap_breaks(line).partition_point(|&b| b <= col)
    }

    /// The columns row `sub` of `line` shows: start..end (end is the next row's start).
    pub(super) fn wrap_segment(&self, line: usize, sub: usize) -> (usize, usize) {
        let b = self.wrap_breaks(line);
        let start = if sub == 0 { 0 } else { b.get(sub - 1).copied().unwrap_or(0) };
        let end = b.get(sub).copied().unwrap_or_else(|| self.buffer.line_len(line));
        (start, end)
    }

    /// The display row of the caret (its line's row plus the wrapped rows before it).
    pub(super) fn caret_row(&self) -> usize {
        let (line, col) = self.caret_line_col();
        self.row_of(line) + self.wrap_sub(line, col)
    }

    /// Up / Down by display rows when lines wrap. False when soft wraps are off.
    pub(super) fn wrap_move_vertical(&mut self, up: bool, extend: bool) -> bool {
        if !self.wrap_active() {
            return false;
        }
        let sels: Vec<Selection> = self.buffer.selections.clone();
        let mut out = Vec::with_capacity(sels.len());
        for s in sels {
            let (line, col) = self.buffer.pos_to_line_col(s.head);
            let sub = self.wrap_sub(line, col);
            let (seg, _) = self.wrap_segment(line, sub);
            let lead = if sub > 0 { self.wrap_indent(line) } else { 0 };
            // The goal is a column on screen (within the row), kept across moves.
            let goal = s.goal_col.unwrap_or(col - seg + lead);
            let row = self.row_of(line) + sub;
            let target_row = if up { row.checked_sub(1) } else { Some(row + 1).filter(|&r| r < self.display_rows()) };
            let head = match target_row {
                None if up => 0,
                None => self.buffer.len_chars(),
                Some(r) => {
                    let tl = self.line_of_row(r);
                    let tsub = r.saturating_sub(self.row_of(tl)).min(self.wrap_breaks(tl).len());
                    let (ts, te) = self.wrap_segment(tl, tsub);
                    let tlead = if tsub > 0 { self.wrap_indent(tl) } else { 0 };
                    let last = tsub == self.wrap_breaks(tl).len();
                    // On a row that continues, its end is the next row's start: stop before it.
                    let max = if last { te } else { te.saturating_sub(1).max(ts) };
                    let c = (ts + goal.saturating_sub(tlead)).min(max);
                    self.buffer.line_col_to_pos(tl, c)
                }
            };
            out.push(Selection { anchor: if extend { s.anchor } else { head }, head, goal_col: Some(goal) });
        }
        self.buffer.selections = out;
        true
    }

    /// Home / End on a wrapped row: first to the row's start / end, then (pressed again, or on
    /// the line's first / last row) the line's (Smart Home stays the line's).
    pub(super) fn wrap_move_home_end(&mut self, end: bool, extend: bool) -> bool {
        if !self.wrap_active() {
            return false;
        }
        let mut any = false;
        let sels: Vec<Selection> = self.buffer.selections.clone();
        let mut out = Vec::with_capacity(sels.len());
        for s in sels {
            let (line, col) = self.buffer.pos_to_line_col(s.head);
            let rows = self.wrap_breaks(line).len() + 1;
            let sub = self.wrap_sub(line, col);
            let (ts, te) = self.wrap_segment(line, sub);
            let target = if end {
                let row_end = te.saturating_sub(1).max(ts);
                (sub + 1 < rows && col != row_end).then_some(row_end)
            } else {
                (sub > 0 && col != ts).then_some(ts)
            };
            match target {
                Some(c) => {
                    any = true;
                    let head = self.buffer.line_col_to_pos(line, c);
                    out.push(Selection { anchor: if extend { s.anchor } else { head }, head, goal_col: None });
                }
                None => out.push(s),
            }
        }
        if !any {
            return false;
        }
        self.buffer.selections = out;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::break_line;

    #[test]
    fn breaks_after_spaces() {
        let t: Vec<char> = "aaaa bbbb cccc dddd".chars().collect();
        // 10 columns: "aaaa bbbb " | "cccc dddd".
        assert_eq!(break_line(&t, 10, 0), vec![10]);
        let t: Vec<char> = "x".repeat(25).chars().collect();
        assert_eq!(break_line(&t, 10, 0), vec![10, 20]);
        // Continuation rows 2 columns in hold 8 columns.
        let t: Vec<char> = "  aaaaaa bbbbbb cccccc".chars().collect();
        assert_eq!(break_line(&t, 10, 2), vec![9, 16]);
        assert!(break_line(&"short".chars().collect::<Vec<_>>(), 10, 0).is_empty());
    }
}
