//! Move Statement Up / Down (Ctrl+Shift+↑/↓), Unwrap / Remove (Ctrl+Shift+Delete) and
//! inserting generated class members, on Python by the syntax tree; other files move lines.

use super::Editor;
use brisk_syntax::{ClassInfo, Lang, Unwrap};
use brisk_text::Selection;

impl Editor {
    pub(super) fn python_tree(&mut self) -> Option<&brisk_syntax::Tree> {
        self.reparse();
        let s = self.syntax.as_ref().filter(|s| s.lang() == Lang::Python && !s.is_stale())?;
        s.tree()
    }

    /// The whole statement(s) under the carets swap with the neighbouring statement of the
    /// same block, as PyCharm does; at the block's edge nothing moves.
    pub(super) fn move_statement(&mut self, up: bool) {
        let p = self.buffer.primary();
        let r = p.range();
        let (first, _) = self.buffer.pos_to_line_col(r.start);
        let (mut last, last_col) = self.buffer.pos_to_line_col(r.end);
        if last > first && last_col == 0 {
            last -= 1;
        }
        let Some(tree) = self.python_tree() else {
            self.buffer.move_lines(up);
            return;
        };
        let Some((target, Some(other))) = brisk_syntax::statement_move(tree, first, last, up) else { return };
        let b = &self.buffer;
        let lines = |rows: std::ops::Range<usize>| rows.map(|l| b.line(l)).collect::<Vec<_>>();
        // [upper][gap][lower] becomes [lower][gap][upper].
        let (upper, lower) = if up { (other, target.clone()) } else { (target.clone(), other) };
        let gap = lines(upper.end..lower.start);
        let (top, bottom) = (lines(lower.clone()), lines(upper.clone()));
        let start = b.line_start(upper.start);
        let end = b.line_end(lower.end - 1);
        let moved_to = if up { start } else { start + top.iter().chain(&gap).map(|l| l.chars().count() + 1).sum::<usize>() };
        let shift = |pos: usize| pos - b.line_start(target.start) + moved_to;
        let sel = Selection { anchor: shift(p.anchor), head: shift(p.head), goal_col: p.goal_col };
        let text = top.into_iter().chain(gap).chain(bottom).collect::<Vec<_>>().join("\n");
        self.buffer.replace_ranges(vec![(start..end, text)]);
        self.buffer.selections = vec![sel];
    }

    /// Unwrap / Remove choices around the caret (Python), innermost first.
    pub fn unwrap_options(&mut self) -> Vec<Unwrap> {
        let head = self.buffer.primary().head;
        let byte = self.buffer.rope().char_to_byte(head);
        let text = self.text().to_string();
        self.python_tree().map(|t| brisk_syntax::unwrap_options(t, &text, byte)).unwrap_or_default()
    }

    /// The Python class around the caret (Generate, Override Methods).
    pub fn class_at_caret(&mut self) -> Option<ClassInfo> {
        let byte = self.buffer.rope().char_to_byte(self.buffer.primary().head);
        let text = self.text().to_string();
        brisk_syntax::class_at(self.python_tree()?, &text, byte)
    }

    /// Insert a generated member (unindented `code`) into the class at the caret: after the
    /// member under the caret, else at the end, or in place of a lone `pass`. One undo step;
    /// the caret goes to the end of the new body's first line.
    pub fn insert_member(&mut self, code: &str, cx: &mut gpui::Context<Self>) {
        let Some(c) = self.class_at_caret() else { return };
        let body: Vec<String> = code.lines().map(|l| if l.is_empty() { String::new() } else { format!("{}{l}", c.indent) }).collect();
        let b = &self.buffer;
        let (edit, first_row) = match c.pass_row {
            Some(r) => ((b.line_start(r)..b.line_end(r), body.join("\n")), r),
            None => {
                let at = b.line_end(c.insert_after);
                ((at..at, format!("\n\n{}", body.join("\n"))), c.insert_after + 2)
            }
        };
        self.buffer.replace_ranges(vec![edit]);
        let def = code.lines().position(|l| l.starts_with("def ") || l.starts_with("async def ")).unwrap_or(0);
        let row = (first_row + def + 1).min(self.buffer.len_lines() - 1);
        self.buffer.set_caret(self.buffer.line_end(row), false);
        cx.notify();
    }

    /// Apply one Unwrap / Remove choice (one undo step); the caret goes to its first line.
    pub fn apply_unwrap(&mut self, u: &Unwrap, cx: &mut gpui::Context<Self>) {
        let b = &self.buffer;
        if u.rows.end > b.len_lines() || u.rows.is_empty() {
            return;
        }
        let start = b.line_start(u.rows.start);
        let edit = match &u.text {
            Some(t) => (start..b.line_end(u.rows.end - 1), t.clone()),
            // Delete the rows with their line break.
            None if u.rows.end < b.len_lines() => (start..b.line_start(u.rows.end), String::new()),
            None => (start.saturating_sub(1)..b.len_chars(), String::new()),
        };
        self.buffer.replace_ranges(vec![edit]);
        let line = u.rows.start.min(self.buffer.len_lines() - 1);
        let ls = self.buffer.line_start(line);
        let indent = self.buffer.line(line).chars().take_while(|c| c.is_whitespace()).count();
        self.buffer.set_caret(ls + indent, false);
        cx.notify();
    }
}
