//! Middle click on a name goes to its declaration, as Ctrl+Click does (PyCharm's default
//! mouse shortcut for Go to Declaration or Usages). The press arms middle-button scrolling
//! (middle_scroll.rs); the declaration opens on release if the mouse did not move.

use super::Editor;
use gpui::{Context, MouseDownEvent, Window};

impl Editor {
    pub(super) fn on_middle_down(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus);
        self.caret_tip = None;
        self.hover = None;
        self.insight.completion = None;
        self.insight.doc = None;
        self.insight.signature = None;
        self.middle_scroll_start(e.position, window, cx);
    }

    /// Without ty: where the name at `pos` is declared in this file (a `def`, a `class`, or its
    /// first assignment), unless `pos` is already there.
    pub fn local_declaration(&self, pos: usize) -> Option<(usize, usize)> {
        let r = self.buffer.word_range_at(pos);
        let name = self.buffer.rope().slice(r.clone()).to_string();
        if name.is_empty() || !name.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_') {
            return None;
        }
        let (here, _) = self.buffer.pos_to_line_col(r.start);
        let text = self.text_string();
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        let mut assigned = None;
        for (i, line) in text.lines().enumerate() {
            let t = line.trim_start();
            let indent = line.chars().count() - t.chars().count();
            for kw in ["def ", "async def ", "class "] {
                if let Some(rest) = t.strip_prefix(kw) {
                    if rest.strip_prefix(name.as_str()).is_some_and(|r| !r.starts_with(ident)) {
                        return (i != here).then(|| (i, indent + kw.len()));
                    }
                }
            }
            if assigned.is_none() {
                if let Some(rest) = t.strip_prefix(name.as_str()) {
                    let rest = rest.trim_start();
                    if !rest.starts_with(ident) && (rest.starts_with('=') && !rest.starts_with("==") || rest.starts_with(':')) {
                        assigned = Some((i, indent));
                    }
                }
            }
        }
        assigned.filter(|&(i, _)| i != here)
    }

    /// Smoke checks: the window point in the middle of the character at (`line`, `col`), if
    /// the line is on screen.
    pub fn char_point(&self, line: usize, col: usize) -> Option<gpui::Point<gpui::Pixels>> {
        let l = self.layout.as_ref()?;
        let shaped = l.shaped(line)?;
        let text = self.buffer.line(line);
        let x0 = shaped.x_for_index(self.display_byte(line, &text, super::char_to_byte(&text, col)));
        let x1 = shaped.x_for_index(self.display_byte(line, &text, super::char_to_byte(&text, col + 1)));
        let y = l.bounds.top() + gpui::px(self.row_of(line) as f32 * super::line_height() - self.scroll_y + super::line_height() / 2.);
        Some(gpui::point(l.text_x + (x0 + x1) / 2., y))
    }
}
