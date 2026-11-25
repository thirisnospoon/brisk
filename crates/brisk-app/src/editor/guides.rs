//! Indent guides as in PyCharm: one at each enclosing block's indentation level. A line that
//! continues a bracketed expression from an earlier line (arguments, a tuple over several
//! lines) has no guides of its own: it takes those of the line the expression starts on.
//! Blank lines inside a block keep the guides going. The guide of the block holding the caret
//! is drawn brighter.

use super::Editor;

/// How far up / down the active block is searched.
const SCAN: usize = 3000;

#[derive(Default)]
pub(super) struct Guides {
    /// (buffer version, caret line) the active guide was found for.
    key: Option<(u64, usize)>,
    /// The caret's block: (guide column, first line, last line).
    pub(super) active: Option<(usize, usize, usize)>,
}

fn leading(t: &str) -> usize {
    t.chars().take_while(|c| *c == ' ').count()
}

impl Editor {
    fn blank(&self, line: usize) -> bool {
        self.buffer.line(line).trim().is_empty()
    }

    /// The indentation that counts for guides on a non-blank line: its own, or for a line
    /// that continues a bracketed expression, that of the line where the expression starts.
    fn statement_indent(&self, line: usize) -> usize {
        let b = &self.buffer;
        let t = b.line(line);
        let ws = leading(&t);
        let Some(syn) = self.syntax.as_ref().filter(|s| s.tree().is_some() && !s.is_stale()) else { return ws };
        let rope = b.rope();
        let start = rope.char_to_byte(b.line_start(line));
        let first = start + t.char_indices().nth(ws).map_or(t.len(), |(i, _)| i);
        match syn.continuation_start(first, start) {
            Some(open) if open < rope.len_bytes() => leading(&b.line(rope.byte_to_line(open))),
            _ => ws,
        }
    }

    /// Columns of indentation with guides on `line` (guides at 0, unit, 2·unit… below it).
    pub(super) fn guide_indent(&self, line: usize) -> usize {
        if !self.blank(line) {
            return self.statement_indent(line);
        }
        let n = self.buffer.len_lines();
        let prev = (line.saturating_sub(200)..line).rev().find(|&l| !self.blank(l));
        let next = (line + 1..(line + 200).min(n)).find(|&l| !self.blank(l));
        match (prev, next) {
            (Some(p), Some(q)) => self.statement_indent(p).min(self.statement_indent(q)),
            _ => 0,
        }
    }

    /// Before laying out a frame: the caret's block (cached per text version and caret line).
    pub(super) fn update_guides(&mut self) {
        let line = self.caret_line_col().0;
        let key = Some((self.buffer.version(), line));
        if self.guides.key == key {
            return;
        }
        self.guides.key = key;
        self.guides.active = self.find_active_guide(line);
    }

    fn find_active_guide(&self, line: usize) -> Option<(usize, usize, usize)> {
        let ind = self.guide_indent(line);
        if ind == 0 {
            return None;
        }
        // The block's header: the nearest line above that is indented less.
        let header =
            (line.saturating_sub(SCAN)..line).rev().find(|&l| !self.blank(l) && leading(&self.buffer.line(l)) < ind && self.statement_indent(l) < ind)?;
        let col = self.statement_indent(header);
        let n = self.buffer.len_lines();
        let mut last = line;
        for l in line + 1..(line + SCAN).min(n) {
            if self.blank(l) {
                continue;
            }
            if leading(&self.buffer.line(l)) <= col && self.statement_indent(l) <= col {
                break;
            }
            last = l;
        }
        Some((col, header + 1, last))
    }
}
