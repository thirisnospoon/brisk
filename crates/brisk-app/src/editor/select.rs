//! Extend / Shrink Selection (by the syntax tree), Clone Caret Above / Below and the
//! clipboard history behind Paste from History.

use super::Editor;
use brisk_text::Selection;
use std::sync::Mutex;

/// Copied texts, newest first (shared by all editors, like PyCharm).
static CLIPBOARD_HISTORY: Mutex<Vec<String>> = Mutex::new(Vec::new());
const HISTORY_MAX: usize = 30;

pub fn remember_clip(text: &str) {
    if text.is_empty() {
        return;
    }
    let Ok(mut h) = CLIPBOARD_HISTORY.lock() else { return };
    h.retain(|t| t != text);
    h.insert(0, text.to_string());
    h.truncate(HISTORY_MAX);
}

pub fn clipboard_history() -> Vec<String> {
    CLIPBOARD_HISTORY.lock().map(|h| h.clone()).unwrap_or_default()
}

/// Selections before each Extend Selection, so Shrink can go back.
#[derive(Default)]
pub struct SelStack {
    version: u64,
    stack: Vec<Vec<Selection>>,
}

impl Editor {
    /// Ctrl+W: word, then the enclosing syntax nodes, then whole lines, then the file.
    pub(super) fn extend_selection(&mut self) {
        self.reparse();
        let b = &self.buffer;
        let p = b.primary();
        let r = p.range();
        let word = b.word_range_at(p.head);
        let next = if p.is_empty() && !word.is_empty() {
            Some(word)
        } else {
            let rope = b.rope();
            let bytes = rope.char_to_byte(r.start)..rope.char_to_byte(r.end);
            let node = self.syntax.as_ref().and_then(|s| {
                let mut cur = bytes.clone();
                // Skip nodes that cover the same characters (e.g. an expression statement
                // around its expression).
                loop {
                    let n = s.enclosing_range(cur)?;
                    let c = rope.byte_to_char(n.start)..rope.byte_to_char(n.end);
                    if c != r {
                        return Some(c);
                    }
                    cur = n;
                }
            });
            let (l1, _) = b.pos_to_line_col(r.start);
            let (l2, _) = b.pos_to_line_col(r.end);
            let lines = b.line_start(l1)..b.line_end(l2);
            match node {
                Some(n) => Some(n),
                None if r != lines && r.start >= lines.start && r.end <= lines.end => Some(lines),
                None if r != (0..b.len_chars()) => Some(0..b.len_chars()),
                None => None,
            }
        };
        let Some(n) = next else { return };
        if self.sel_stack.version != self.buffer.version() {
            self.sel_stack = SelStack { version: self.buffer.version(), stack: Vec::new() };
        }
        self.sel_stack.stack.push(self.buffer.selections.clone());
        self.buffer.selections = vec![Selection { anchor: n.start, head: n.end, goal_col: None }];
    }

    /// Ctrl+Shift+W: back to the selection before the last Extend Selection.
    pub(super) fn shrink_selection(&mut self) {
        let fresh = self.sel_stack.version == self.buffer.version();
        match self.sel_stack.stack.pop() {
            Some(prev) if fresh => self.buffer.selections = prev,
            _ => {
                self.sel_stack.stack.clear();
                let p = self.buffer.primary();
                if !p.is_empty() {
                    self.buffer.selections = vec![Selection::caret(p.head)];
                }
            }
        }
    }

    /// Ctrl, Ctrl+Up / Ctrl, Ctrl+Down: a new caret above the topmost / below the lowest one.
    pub(super) fn clone_caret(&mut self, above: bool) {
        let b = &mut self.buffer;
        let edge = if above { b.selections.iter().min_by_key(|s| s.head) } else { b.selections.iter().max_by_key(|s| s.head) };
        let Some(&s) = edge else { return };
        let (line, col) = b.pos_to_line_col(s.head);
        let goal = s.goal_col.unwrap_or(col);
        let target = if above { line.checked_sub(1) } else { (line + 1 < b.len_lines()).then_some(line + 1) };
        let Some(t) = target else { return };
        let pos = b.line_col_to_pos(t, goal);
        b.add_caret(pos);
        for s in &mut b.selections {
            if s.head == pos && s.is_empty() {
                s.goal_col = Some(goal);
            }
        }
    }

    /// Paste from History: insert this text at every caret.
    pub fn paste_text(&mut self, text: &str, cx: &mut gpui::Context<Self>) {
        remember_clip(text);
        self.buffer.paste_python(&text.replace("\r\n", "\n"));
        self.ensure_caret_visible();
        cx.notify();
    }

    pub fn carets_for_smoke(&self) -> Vec<(usize, usize, usize, usize)> {
        let b = &self.buffer;
        let mut v: Vec<_> = b
            .selections
            .iter()
            .map(|s| {
                let (a, c) = b.pos_to_line_col(s.range().start);
                let (d, e) = b.pos_to_line_col(s.range().end);
                (a, c, d, e)
            })
            .collect();
        v.sort();
        v
    }
}
