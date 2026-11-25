//! Bookmarks in an open file (they follow edits) and method navigation (Alt+Up / Alt+Down).

use super::Editor;

#[derive(Default)]
pub struct Bookmarks {
    /// (char position of the line start, mnemonic 0-9 / A-Z).
    marks: Vec<(usize, Option<char>)>,
    version: u64,
}

impl Editor {
    /// Follow edits; one bookmark per line.
    pub(super) fn sync_bookmarks(&mut self) {
        let v = self.buffer.version();
        if self.bookmarks.version == v {
            return;
        }
        let b = &self.buffer;
        let changes = b.changes_since(self.bookmarks.version);
        let mut marks: Vec<(usize, Option<char>)> = self
            .bookmarks
            .marks
            .iter()
            .filter_map(|&(p, m)| {
                let p = brisk_text::map_through(p, changes.as_deref()?).min(b.len_chars());
                Some((b.line_start(b.pos_to_line_col(p).0), m))
            })
            .collect();
        marks.sort_by_key(|m| m.0);
        marks.dedup_by_key(|m| m.0);
        self.bookmarks = Bookmarks { marks, version: v };
    }

    /// Bookmarks as (line, mnemonic), by line.
    pub fn bookmark_lines(&mut self) -> Vec<(usize, Option<char>)> {
        self.sync_bookmarks();
        self.bookmarks.marks.iter().map(|&(p, m)| (self.buffer.pos_to_line_col(p).0, m)).collect()
    }

    /// The same without `&mut` (listings): edits since the last sync are applied on the fly.
    pub fn bookmarks_snapshot(&self) -> Vec<(usize, Option<char>)> {
        self.bookmark_lines_now()
    }

    pub(super) fn bookmark_lines_now(&self) -> Vec<(usize, Option<char>)> {
        let b = &self.buffer;
        let changes = (self.bookmarks.version != b.version()).then(|| b.changes_since(self.bookmarks.version)).flatten();
        let mut v: Vec<(usize, Option<char>)> = self
            .bookmarks
            .marks
            .iter()
            .map(|&(p, m)| {
                let p = changes.as_deref().map_or(p, |c| brisk_text::map_through(p, c)).min(b.len_chars());
                (b.pos_to_line_col(p).0, m)
            })
            .collect();
        v.dedup_by_key(|x| x.0);
        v
    }

    pub fn set_bookmarks(&mut self, v: &[(usize, Option<char>)]) {
        let n = self.buffer.len_lines();
        let mut marks: Vec<(usize, Option<char>)> = v.iter().filter(|(l, _)| *l < n).map(|&(l, m)| (self.buffer.line_start(l), m)).collect();
        marks.sort_by_key(|m| m.0);
        marks.dedup_by_key(|m| m.0);
        self.bookmarks = Bookmarks { marks, version: self.buffer.version() };
    }

    /// F11 (`mnemonic` None) and Ctrl+F11: add a bookmark on the caret line, or remove it.
    /// With a mnemonic, an existing bookmark on the line takes it. True if one is there now.
    pub fn toggle_bookmark(&mut self, mnemonic: Option<char>) -> bool {
        self.sync_bookmarks();
        let (line, _) = self.caret_line_col();
        let at = self.buffer.line_start(line);
        match self.bookmarks.marks.iter().position(|m| m.0 == at) {
            Some(i) if mnemonic.is_none() || self.bookmarks.marks[i].1 == mnemonic => {
                self.bookmarks.marks.remove(i);
                false
            }
            Some(i) => {
                self.bookmarks.marks[i].1 = mnemonic;
                true
            }
            None => {
                self.bookmarks.marks.push((at, mnemonic));
                self.bookmarks.marks.sort_by_key(|m| m.0);
                true
            }
        }
    }

    /// Remove a mnemonic from this file (it moved to another bookmark).
    pub fn clear_mnemonic(&mut self, c: char) {
        for m in &mut self.bookmarks.marks {
            if m.1 == Some(c) {
                m.1 = None;
            }
        }
    }

    pub fn remove_bookmark_at(&mut self, line: usize) {
        self.sync_bookmarks();
        let at = self.buffer.line_start(line.min(self.buffer.len_lines().saturating_sub(1)));
        self.bookmarks.marks.retain(|m| m.0 != at);
    }

    /// Alt+Down / Alt+Up: the caret goes to the name of the next / previous class or function.
    pub fn goto_method(&mut self, forward: bool, cx: &mut gpui::Context<Self>) -> bool {
        let items = self.outline_cached();
        let (line, col) = self.caret_line_col();
        let here = (line, col);
        let target = if forward {
            items.iter().map(|o| (o.line, self.buffer.pos_to_line_col(self.byte_to_pos(o.byte)).1)).filter(|&p| p > here).min()
        } else {
            items.iter().map(|o| (o.line, self.buffer.pos_to_line_col(self.byte_to_pos(o.byte)).1)).filter(|&p| p < here).max()
        };
        let Some((l, c)) = target else { return false };
        let pos = self.buffer.line_col_to_pos(l, c);
        self.buffer.set_caret(pos, false);
        self.ensure_caret_visible();
        cx.notify();
        true
    }

    fn byte_to_pos(&self, byte: usize) -> usize {
        let rope = self.buffer.rope();
        rope.byte_to_char(byte.min(rope.len_bytes()))
    }
}
