//! Column selection mode (Shift+Alt+Insert), like PyCharm's: while it is on, Shift+arrows
//! and a plain mouse drag select a rectangle — one selection per line between the anchor
//! and the head columns (a line shorter than the block gets an empty caret at its end).

use super::Editor;
use brisk_keymap::Action as A;
use brisk_text::Selection;

impl Editor {
    pub fn column_mode(&self) -> bool {
        self.column_mode
    }

    pub(super) fn toggle_column_mode(&mut self) {
        self.column_mode = !self.column_mode;
        self.column_block = None;
    }

    /// Shift+arrows in column mode: grow or shrink the block. Returns false for other actions.
    pub(super) fn column_key(&mut self, action: A) -> bool {
        if !self.column_mode || !matches!(action, A::SelectLeft | A::SelectRight | A::SelectUp | A::SelectDown) {
            return false;
        }
        let b = &self.buffer;
        let ((al, ac), (hl, hc)) = self.column_block.unwrap_or_else(|| {
            let lc = b.pos_to_line_col(b.primary().head);
            (lc, lc)
        });
        let last = b.len_lines().saturating_sub(1);
        let (hl, hc) = match action {
            A::SelectLeft => (hl, hc.saturating_sub(1)),
            A::SelectRight => (hl, hc + 1),
            A::SelectUp => (hl.saturating_sub(1), hc),
            _ => ((hl + 1).min(last), hc),
        };
        self.set_column_block((al, ac), (hl, hc));
        true
    }

    /// One selection per line from line `al` to `hl`, columns `ac` to `hc` (clamped to each line).
    pub(super) fn set_column_block(&mut self, (al, ac): (usize, usize), (hl, hc): (usize, usize)) {
        self.column_block = Some(((al, ac), (hl, hc)));
        let (l0, l1) = (al.min(hl), al.max(hl));
        let sels: Vec<Selection> = (l0..=l1)
            .map(|l| {
                let len = self.buffer.line_len(l);
                let a = self.buffer.line_col_to_pos(l, ac.min(len));
                let h = self.buffer.line_col_to_pos(l, hc.min(len));
                Selection { anchor: a, head: h, goal_col: None }
            })
            .collect();
        if !sels.is_empty() {
            self.buffer.selections = sels;
        }
    }
}
