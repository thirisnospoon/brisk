//! Show Whitespaces (View › Active Editor, and Settings › Editor › General › Appearance as the
//! default), as in PyCharm: a small dot in the middle of each space and an arrow across each tab.
//! Where the whitespace is on a line is cached per line for the current text version; a frame
//! only places the marks.

use super::Editor;
use std::collections::HashMap;
use std::rc::Rc;

/// More cached lines than this and the cache starts over (scrolling through a huge file).
const MAX_LINES: usize = 4000;

#[derive(Default)]
pub(super) struct Whitespace {
    /// Set by View › Active Editor › Show Whitespaces for this editor; None follows Settings.
    pub(super) shown: Option<bool>,
    version: u64,
    /// Line → its spaces and tabs: (column, is a tab).
    lines: HashMap<usize, Rc<[(u32, bool)]>>,
}

/// The spaces and tabs of a line: (column in chars, is a tab).
pub(super) fn scan(line: &str) -> Vec<(u32, bool)> {
    line.trim_end_matches(['\n', '\r']).chars().enumerate().filter(|(_, c)| *c == ' ' || *c == '\t').map(|(i, c)| (i as u32, c == '\t')).collect()
}

impl Editor {
    pub(super) fn whitespace_shown(&self) -> bool {
        self.whitespace.shown.unwrap_or_else(crate::settings::show_whitespaces)
    }

    /// View › Active Editor › Show Whitespaces: flip it for this editor.
    pub(super) fn toggle_whitespace(&mut self) {
        self.whitespace.shown = Some(!self.whitespace_shown());
    }

    /// Before a frame: fill the cache for the shown lines not in it yet (`height`: the view's).
    pub(super) fn update_whitespace(&mut self, height: f32) {
        if !self.whitespace_shown() {
            return;
        }
        let n = self.buffer.len_lines();
        let first = ((self.scroll_y / super::line_height()).floor() as usize).min(self.display_rows().saturating_sub(1));
        let visible = (height / super::line_height()).ceil() as usize + 1;
        let mut rows = Vec::with_capacity(visible);
        let mut next = self.line_of_row(first);
        while rows.len() < visible && next < n {
            rows.push(next);
            next = self.next_shown_line(next);
        }
        let v = self.buffer.version();
        if self.whitespace.version != v || self.whitespace.lines.len() > MAX_LINES {
            self.whitespace.version = v;
            self.whitespace.lines.clear();
        }
        for l in rows {
            if !self.whitespace.lines.contains_key(&l) {
                let cols: Rc<[(u32, bool)]> = scan(&self.buffer.line(l)).into();
                self.whitespace.lines.insert(l, cols);
            }
        }
    }

    /// The cached spaces and tabs of a line (empty when not shown or not cached).
    pub(super) fn whitespace_of(&self, line: usize) -> Option<Rc<[(u32, bool)]>> {
        self.whitespace.lines.get(&line).cloned()
    }

    /// Smoke: (shown, cached lines, spaces and tabs on this line).
    pub fn whitespace_for_smoke(&self, line: usize) -> (bool, usize, usize, usize) {
        let cols = self.whitespace_of(line);
        let (s, t) = cols.map_or((0, 0), |c| (c.iter().filter(|x| !x.1).count(), c.iter().filter(|x| x.1).count()));
        (self.whitespace_shown(), self.whitespace.lines.len(), s, t)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn spaces_and_tabs() {
        assert_eq!(super::scan("\tx = 1  \n"), vec![(0, true), (2, false), (4, false), (6, false), (7, false)]);
        assert!(super::scan("abc").is_empty());
    }
}
