//! Text buffer: rope storage, multi-caret selections, undo/redo, PyCharm-style editing ops.
//!
//! Positions are **char indices** into the rope. The UI converts to (line, column) for display.

pub mod encoding;

use ropey::Rope;
pub use ropey::Rope as TextRope;
use std::ops::Range;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }
}

/// One selection. `anchor == head` means a plain caret. `goal_col` keeps the column for Up/Down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
    pub goal_col: Option<usize>,
}

impl Selection {
    pub fn caret(pos: usize) -> Self {
        Self { anchor: pos, head: pos, goal_col: None }
    }
    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone, Debug)]
struct EditRecord {
    start: usize,
    removed: String,
    inserted: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditKind {
    Typing,
    Deleting,
    Other,
}

#[derive(Clone, Debug)]
struct Transaction {
    edits: Vec<EditRecord>, // in application order
    before: Vec<Selection>,
    after: Vec<Selection>,
    kind: EditKind,
    at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Movement {
    Left,
    Right,
    Up,
    Down,
    WordLeft,
    WordRight,
    LineStart,
    LineEnd,
    DocStart,
    DocEnd,
    PageUp(usize),
    PageDown(usize),
}

/// Undo steps kept per buffer (typing is grouped, so a step is usually a word or more).
/// Each step holds the removed and inserted text, so an endless session of large pastes or
/// reformats would otherwise grow without bound.
pub const UNDO_LIMIT: usize = 1000;

pub struct Buffer {
    rope: Rope,
    pub line_ending: LineEnding,
    pub selections: Vec<Selection>,
    pub indent_unit: String,
    pub line_comment: Option<String>,
    /// Typing `(` inserts `()` and so on. Off in search fields.
    pub auto_pairs: bool,
    undo: Vec<Transaction>,
    redo: Vec<Transaction>,
    version: u64,
    saved_version: u64,
    group_window: Duration,
    /// Recent edits as (version after, start, removed chars, inserted chars), in application
    /// order, so positions kept outside the buffer (folds, bookmarks) can follow them.
    changes: std::collections::VecDeque<(u64, usize, usize, usize)>,
    /// Changes after this version are all in `changes`.
    changes_floor: u64,
}

/// Move a char position through a list of edits from [`Buffer::changes_since`]. A position
/// inside a removed range goes to its start.
pub fn map_through(mut pos: usize, changes: &[(usize, usize, usize)]) -> usize {
    for &(start, removed, inserted) in changes {
        if pos >= start + removed {
            pos = pos - removed + inserted;
        } else if pos > start {
            pos = start;
        }
    }
    pos
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        let line_ending = if text.contains("\r\n") { LineEnding::CrLf } else { LineEnding::Lf };
        // Normalize to LF internally; restore on save.
        let normalized = if line_ending == LineEnding::CrLf { text.replace("\r\n", "\n") } else { text.to_string() };
        Self {
            rope: Rope::from_str(&normalized),
            line_ending,
            selections: vec![Selection::caret(0)],
            indent_unit: "    ".into(),
            line_comment: Some("#".into()),
            auto_pairs: true,
            undo: Vec::new(),
            redo: Vec::new(),
            version: 0,
            saved_version: 0,
            group_window: Duration::from_millis(1000),
            changes: Default::default(),
            changes_floor: 0,
        }
    }

    // --- Queries -----------------------------------------------------------

    pub fn rope(&self) -> &Rope {
        &self.rope
    }
    pub fn text(&self) -> String {
        self.rope.to_string()
    }
    /// Text with the original line endings, for saving.
    pub fn text_for_save(&self) -> String {
        let t = self.rope.to_string();
        match self.line_ending {
            LineEnding::Lf => t,
            LineEnding::CrLf => t.replace('\n', "\r\n"),
        }
    }
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }
    pub fn version(&self) -> u64 {
        self.version
    }
    pub fn is_dirty(&self) -> bool {
        self.version != self.saved_version
    }
    pub fn mark_saved(&mut self) {
        self.saved_version = self.version;
    }

    /// Line text without the trailing newline.
    pub fn line(&self, line: usize) -> String {
        let mut s = self.rope.line(line).to_string();
        if s.ends_with('\n') {
            s.pop();
        }
        s
    }
    pub fn line_start(&self, line: usize) -> usize {
        self.rope.line_to_char(line)
    }
    pub fn line_len(&self, line: usize) -> usize {
        let l = self.rope.line(line);
        let n = l.len_chars();
        if n > 0 && l.char(n - 1) == '\n' {
            n - 1
        } else {
            n
        }
    }
    pub fn line_end(&self, line: usize) -> usize {
        self.line_start(line) + self.line_len(line)
    }
    pub fn pos_to_line_col(&self, pos: usize) -> (usize, usize) {
        let pos = pos.min(self.len_chars());
        let line = self.rope.char_to_line(pos);
        (line, pos - self.rope.line_to_char(line))
    }
    pub fn line_col_to_pos(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.len_lines().saturating_sub(1));
        self.line_start(line) + col.min(self.line_len(line))
    }
    pub fn slice(&self, r: Range<usize>) -> String {
        self.rope.slice(r).to_string()
    }
    pub fn primary(&self) -> Selection {
        *self.selections.last().unwrap()
    }
    fn char_at(&self, pos: usize) -> Option<char> {
        (pos < self.len_chars()).then(|| self.rope.char(pos))
    }

    /// Selected text, joined by newlines for multiple carets in text order (like PyCharm copy).
    pub fn selected_text(&self) -> String {
        let mut sels: Vec<&Selection> = self.selections.iter().filter(|s| !s.is_empty()).collect();
        sels.sort_by_key(|s| s.range().start);
        let parts: Vec<String> = sels.iter().map(|s| self.slice(s.range())).collect();
        parts.join("\n")
    }

    /// The lines of all carets (each once, in text order), for copy/cut with no selection.
    pub fn caret_lines(&self) -> Vec<usize> {
        let mut lines: Vec<usize> = self.selections.iter().map(|s| self.pos_to_line_col(s.head).0).collect();
        lines.sort_unstable();
        lines.dedup();
        lines
    }

    // --- Low-level edit machinery -------------------------------------------

    /// Apply non-overlapping edits (given in original coordinates, any order).
    /// `after` computes the new selections from a position-mapping function.
    fn apply(
        &mut self,
        mut edits: Vec<(Range<usize>, String)>,
        kind: EditKind,
        after: impl FnOnce(&dyn Fn(usize, bool) -> usize, &Self) -> Vec<Selection>,
    ) {
        // Clamp to the text and merge overlaps: carets close together (Ctrl+Delete with carets
        // at 1 and 3 in `abcd`, Backspace in `()` with carets on both sides) produce
        // overlapping ranges, and removing those from the rope one by one panics.
        let len = self.len_chars();
        for (r, _) in edits.iter_mut() {
            r.end = r.end.min(len);
            r.start = r.start.min(r.end);
        }
        edits.sort_by_key(|(r, _)| r.start);
        let mut merged: Vec<(Range<usize>, String)> = Vec::with_capacity(edits.len());
        for (r, s) in edits {
            match merged.last_mut() {
                Some((pr, ps)) if r.start < pr.end => {
                    pr.end = pr.end.max(r.end);
                    ps.push_str(&s);
                }
                _ => merged.push((r, s)),
            }
        }
        let mut edits = merged;
        edits.retain(|(r, s)| !(r.is_empty() && s.is_empty()));
        if edits.is_empty() {
            return;
        }
        let before = self.selections.clone();

        // Deltas for mapping old -> new positions.
        let deltas: Vec<(Range<usize>, usize)> =
            edits.iter().map(|(r, s)| (r.clone(), s.chars().count())).collect();
        let map = move |pos: usize, right_bias: bool| -> usize {
            let mut shift: isize = 0;
            for (r, ins) in &deltas {
                if pos < r.start {
                    break;
                }
                let new_start = (r.start as isize + shift) as usize;
                if pos == r.start {
                    // Pure insertion at pos: right bias moves past it. A replacement starting at
                    // pos leaves pos at its start.
                    return if r.is_empty() && right_bias { new_start + ins } else { new_start };
                }
                if pos < r.end {
                    // Inside a replaced range.
                    return if right_bias { new_start + ins } else { new_start };
                }
                shift += *ins as isize - r.len() as isize;
            }
            (pos as isize + shift) as usize
        };

        let mut records = Vec::with_capacity(edits.len());
        for (r, s) in edits.iter().rev() {
            let removed = self.rope.slice(r.clone()).to_string();
            self.rope.remove(r.clone());
            self.rope.insert(r.start, s);
            self.log_change(r.start, r.len(), s.chars().count());
            records.push(EditRecord { start: r.start, removed, inserted: s.clone() });
        }

        let mut sels = after(&map, self);
        normalize(&mut sels, self.len_chars());
        self.selections = sels;
        self.version += 1;
        self.redo.clear();

        let now = Instant::now();
        if let Some(last) = self.undo.last_mut() {
            if kind != EditKind::Other && last.kind == kind && now.duration_since(last.at) < self.group_window {
                last.edits.extend(records);
                last.after = self.selections.clone();
                last.at = now;
                return;
            }
        }
        self.undo.push(Transaction { edits: records, before, after: self.selections.clone(), kind, at: now });
        if self.undo.len() > UNDO_LIMIT {
            self.undo.drain(..self.undo.len() - UNDO_LIMIT);
        }
    }

    /// Break typing grouping (e.g. after caret movement).
    pub fn break_undo_group(&mut self) {
        if let Some(t) = self.undo.last_mut() {
            t.kind = EditKind::Other;
        }
    }

    /// Record one rope edit (tagged with the version it produces).
    fn log_change(&mut self, start: usize, removed: usize, inserted: usize) {
        self.changes.push_back((self.version + 1, start, removed, inserted));
        while self.changes.len() > 4096 {
            if let Some((v, ..)) = self.changes.pop_front() {
                // Changes of version v may now be incomplete.
                self.changes_floor = self.changes_floor.max(v);
            }
        }
    }

    /// The edits made after `version`, in order; None when they are no longer all known.
    pub fn changes_since(&self, version: u64) -> Option<Vec<(usize, usize, usize)>> {
        if version < self.changes_floor {
            return None;
        }
        Some(self.changes.iter().filter(|c| c.0 > version).map(|&(_, s, r, i)| (s, r, i)).collect())
    }

    pub fn undo(&mut self) -> bool {
        let Some(t) = self.undo.pop() else { return false };
        for e in t.edits.iter().rev() {
            let end = e.start + e.inserted.chars().count();
            self.rope.remove(e.start..end);
            self.rope.insert(e.start, &e.removed);
            self.log_change(e.start, end - e.start, e.removed.chars().count());
        }
        self.selections = t.before.clone();
        self.version += 1;
        self.redo.push(t);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(mut t) = self.redo.pop() else { return false };
        for e in t.edits.iter() {
            let end = e.start + e.removed.chars().count();
            self.rope.remove(e.start..end);
            self.rope.insert(e.start, &e.inserted);
            self.log_change(e.start, end - e.start, e.inserted.chars().count());
        }
        self.selections = t.after.clone();
        self.version += 1;
        t.kind = EditKind::Other;
        self.undo.push(t);
        true
    }

    /// Replace the whole text (e.g. reload from disk or formatter output) as one undoable step.
    pub fn set_text(&mut self, text: &str) {
        let len = self.len_chars();
        let caret = self.primary().head;
        self.apply(vec![(0..len, text.to_string())], EditKind::Other, |_, b| {
            vec![Selection::caret(caret.min(b.len_chars()))]
        });
    }

    /// Apply non-overlapping replacements (char ranges in the current text) as one undoable
    /// step, keeping the carets where they were relative to the text — quick fixes.
    pub fn replace_ranges(&mut self, edits: Vec<(Range<usize>, String)>) {
        let len = self.len_chars();
        let edits = edits.into_iter().map(|(r, s)| (r.start.min(len)..r.end.min(len).max(r.start.min(len)), s)).collect();
        let sels = self.selections.clone();
        self.apply(edits, EditKind::Other, |map, _| sels.iter().map(|s| Selection { anchor: map(s.anchor, false), head: map(s.head, false), goal_col: None }).collect());
    }

    // --- Typing --------------------------------------------------------------

    pub fn insert(&mut self, text: &str) {
        let n = text.chars().count();
        let edits: Vec<_> = self.selections.iter().map(|s| (s.range(), text.to_string())).collect();
        let starts: Vec<usize> = self.selections.iter().map(|s| s.range().start).collect();
        let kind = if text.contains('\n') { EditKind::Other } else { EditKind::Typing };
        self.apply(edits, kind, |map, _| {
            starts.iter().map(|&st| Selection::caret(map(st, false) + n)).collect()
        });
    }

    /// Paste with as many carets as pieces: the i-th caret in text order gets `texts[i]`
    /// (PyCharm spreads a multi-caret copy over the same number of carets). Falls back to
    /// [`Self::insert`] of the joined text when the counts differ.
    pub fn insert_each(&mut self, texts: &[String]) {
        if texts.len() != self.selections.len() {
            return self.insert(&texts.join("\n"));
        }
        let mut order: Vec<usize> = (0..self.selections.len()).collect();
        order.sort_by_key(|&i| self.selections[i].range().start);
        // The i-th selection in text order gets texts[i]; the selections keep their own order.
        let mut piece = vec![String::new(); texts.len()];
        for (k, &i) in order.iter().enumerate() {
            piece[i] = texts[k].clone();
        }
        let edits: Vec<_> = self.selections.iter().zip(&piece).map(|(s, t)| (s.range(), t.clone())).collect();
        let ends: Vec<(usize, usize)> = self.selections.iter().zip(&piece).map(|(s, t)| (s.range().start, t.chars().count())).collect();
        self.apply(edits, EditKind::Other, |map, _| ends.iter().map(|&(st, n)| Selection::caret(map(st, false) + n)).collect());
    }

    /// Type a char with PyCharm-like auto-pairing for brackets and quotes.
    pub fn type_char(&mut self, c: char) {
        let closing = match c {
            '(' => Some(')'),
            '[' => Some(']'),
            '{' => Some('}'),
            '"' => Some('"'),
            '\'' => Some('\''),
            _ => None,
        };
        let closing = closing.filter(|_| self.auto_pairs);
        // Overtype a closing char that's already there.
        if self.auto_pairs
            && matches!(c, ')' | ']' | '}' | '"' | '\'')
            && self.selections.iter().all(|s| s.is_empty() && self.char_at(s.head) == Some(c))
        {
            self.move_carets(Movement::Right, false);
            return;
        }
        if let Some(close) = closing {
            if self.selections.iter().all(|s| s.is_empty()) {
                let next_ok = self.selections.iter().all(|s| {
                    self.char_at(s.head).map_or(true, |n| n.is_whitespace() || ")]},:".contains(n))
                });
                let quote_ok = !matches!(c, '"' | '\'')
                    || self.selections.iter().all(|s| {
                        s.head == 0
                            || self.char_at(s.head - 1).map_or(true, |p| !p.is_alphanumeric() && p != c)
                    });
                if next_ok && quote_ok {
                    let pair: String = [c, close].iter().collect();
                    let starts: Vec<usize> = self.selections.iter().map(|s| s.head).collect();
                    let edits = starts.iter().map(|&p| (p..p, pair.clone())).collect();
                    self.apply(edits, EditKind::Typing, |map, _| {
                        starts.iter().map(|&p| Selection::caret(map(p, false) + 1)).collect()
                    });
                    return;
                }
            } else if self.selections.iter().all(|s| !s.is_empty()) {
                // Wrap selection.
                let edits: Vec<_> = self
                    .selections
                    .iter()
                    .map(|s| (s.range(), format!("{c}{}{close}", self.slice(s.range()))))
                    .collect();
                let ranges: Vec<_> = self.selections.iter().map(|s| s.range()).collect();
                self.apply(edits, EditKind::Other, |map, _| {
                    ranges
                        .iter()
                        .map(|r| {
                            let st = map(r.start, false) + 1;
                            Selection { anchor: st, head: st + r.len(), goal_col: None }
                        })
                        .collect()
                });
                return;
            }
        }
        let mut buf = [0u8; 4];
        self.insert(c.encode_utf8(&mut buf));
    }

    fn leading_ws(&self, line: usize) -> String {
        self.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').collect()
    }

    /// Enter with smart indent: keep indentation, indent after ':' or an open bracket,
    /// and split `(|)` into three lines.
    pub fn newline(&mut self) {
        let mut edits = Vec::new();
        let mut carets = Vec::new();
        for s in &self.selections {
            let r = s.range();
            let (line, col) = self.pos_to_line_col(r.start);
            let text = self.line(line);
            let before: String = text.chars().take(col).collect();
            let base = self.leading_ws(line);
            let trimmed = before.trim_end();
            let opens = trimmed.ends_with(':') || trimmed.ends_with(['(', '[', '{']);
            let mut indent = base.clone();
            if opens {
                indent.push_str(&self.indent_unit);
            }
            let prev = before.chars().last();
            let next = self.char_at(r.end);
            let between_pair = matches!((prev, next), (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}')));
            let mut ins = format!("\n{indent}");
            let caret_off = ins.chars().count();
            if between_pair {
                ins.push('\n');
                ins.push_str(&base);
            }
            edits.push((r.clone(), ins));
            carets.push((r.start, caret_off));
        }
        self.apply(edits, EditKind::Other, |map, _| {
            carets.iter().map(|&(st, off)| Selection::caret(map(st, false) + off)).collect()
        });
    }

    /// Shift+Enter: new line after the current one, regardless of caret column.
    pub fn start_new_line(&mut self, before: bool) {
        let mut edits = Vec::new();
        let mut targets = Vec::new();
        for s in &self.selections {
            let (line, _) = self.pos_to_line_col(s.head);
            let indent = self.leading_ws(line);
            if before {
                let p = self.line_start(line);
                edits.push((p..p, format!("{indent}\n")));
                targets.push((p, indent.chars().count(), false));
            } else {
                let p = self.line_end(line);
                let t = self.line(line);
                let extra = if t.trim_end().ends_with(':') { self.indent_unit.clone() } else { String::new() };
                let ins = format!("\n{indent}{extra}");
                let n = ins.chars().count();
                edits.push((p..p, ins));
                targets.push((p, n, true));
            }
        }
        self.apply(edits, EditKind::Other, |map, _| {
            targets
                .iter()
                .map(|&(p, n, after)| Selection::caret(if after { map(p, false) + n } else { map(p, false) + n }))
                .collect()
        });
    }

    pub fn backspace(&mut self) {
        if self.selections.iter().any(|s| !s.is_empty()) {
            return self.delete_selections();
        }
        let mut edits = Vec::new();
        for s in &self.selections {
            let p = s.head;
            if p == 0 {
                continue;
            }
            let (line, col) = self.pos_to_line_col(p);
            let before: String = self.line(line).chars().take(col).collect();
            // In leading whitespace: delete back to previous indent stop (PyCharm smart backspace).
            if col > 0 && before.chars().all(|c| c == ' ') {
                let unit = self.indent_unit.chars().count().max(1);
                let target = (col - 1) / unit * unit;
                edits.push((p - (col - target)..p, String::new()));
                continue;
            }
            // Delete an empty auto-pair together: (|)
            let prev = self.rope.char(p - 1);
            let next = self.char_at(p);
            let pair = matches!((prev, next), ('(', Some(')')) | ('[', Some(']')) | ('{', Some('}')) | ('"', Some('"')) | ('\'', Some('\'')));
            edits.push((p - 1..if pair { p + 1 } else { p }, String::new()));
        }
        let starts: Vec<usize> = edits.iter().map(|(r, _): &(Range<usize>, String)| r.start).collect();
        let carets: Vec<usize> = if starts.is_empty() { vec![] } else { starts.clone() };
        let fallback: Vec<usize> = self.selections.iter().map(|s| s.head).collect();
        self.apply(edits, EditKind::Deleting, |map, _| {
            let src = if carets.is_empty() { &fallback } else { &carets };
            src.iter().map(|&p| Selection::caret(map(p, false))).collect()
        });
    }

    pub fn delete_forward(&mut self) {
        if self.selections.iter().any(|s| !s.is_empty()) {
            return self.delete_selections();
        }
        let len = self.len_chars();
        let edits: Vec<_> =
            self.selections.iter().filter(|s| s.head < len).map(|s| (s.head..s.head + 1, String::new())).collect();
        let starts: Vec<usize> = self.selections.iter().map(|s| s.head).collect();
        self.apply(edits, EditKind::Deleting, |map, _| {
            starts.iter().map(|&p| Selection::caret(map(p, false))).collect()
        });
    }

    pub fn delete_word(&mut self, forward: bool) {
        if self.selections.iter().any(|s| !s.is_empty()) {
            return self.delete_selections();
        }
        let ranges: Vec<Range<usize>> = self
            .selections
            .iter()
            .map(|s| {
                let p = s.head;
                if forward {
                    p..self.word_boundary(p, true)
                } else {
                    self.word_boundary(p, false)..p
                }
            })
            .collect();
        let edits = ranges.iter().map(|r| (r.clone(), String::new())).collect();
        self.apply(edits, EditKind::Deleting, |map, _| {
            ranges.iter().map(|r| Selection::caret(map(r.start, false))).collect()
        });
    }

    fn delete_selections(&mut self) {
        let ranges: Vec<_> = self.selections.iter().map(|s| s.range()).collect();
        let edits = ranges.iter().map(|r| (r.clone(), String::new())).collect();
        self.apply(edits, EditKind::Other, |map, _| {
            ranges.iter().map(|r| Selection::caret(map(r.start, false))).collect()
        });
    }

    // --- Line operations -----------------------------------------------------

    fn selected_lines(&self, s: &Selection) -> Range<usize> {
        let r = s.range();
        let first = self.rope.char_to_line(r.start);
        let mut last = self.rope.char_to_line(r.end);
        if last > first && r.end == self.line_start(last) {
            last -= 1; // selection ends at column 0: don't include that line
        }
        first..last + 1
    }

    /// Ctrl+D: duplicate line, or the selection.
    pub fn duplicate(&mut self) {
        let mut edits = Vec::new();
        let mut info = Vec::new();
        for s in &self.selections {
            if s.is_empty() {
                let (line, col) = self.pos_to_line_col(s.head);
                let end = self.line_end(line);
                let ins = format!("\n{}", self.line(line));
                info.push((end, Some(col), 0usize));
                edits.push((end..end, ins));
            } else {
                let r = s.range();
                let text = self.slice(r.clone());
                info.push((r.end, None, text.chars().count()));
                edits.push((r.end..r.end, text));
            }
        }
        self.apply(edits, EditKind::Other, |map, b| {
            info.iter()
                .map(|&(p, col, n)| match col {
                    Some(col) => {
                        let np = map(p, false) + 1; // start of duplicated line
                        let (line, _) = b.pos_to_line_col(np);
                        Selection::caret(b.line_col_to_pos(line, col))
                    }
                    None => {
                        let st = map(p, false);
                        Selection { anchor: st, head: st + n, goal_col: None }
                    }
                })
                .collect()
        });
    }

    /// Ctrl+Y: delete the line(s) under the carets.
    pub fn delete_lines(&mut self) {
        let mut lines: Vec<Range<usize>> = self.selections.iter().map(|s| self.selected_lines(s)).collect();
        lines.sort_by_key(|r| r.start);
        lines.dedup_by(|b, a| {
            if b.start <= a.end {
                a.end = a.end.max(b.end);
                true
            } else {
                false
            }
        });
        let n_lines = self.len_lines();
        let edits: Vec<_> = lines
            .iter()
            .map(|l| {
                let start = self.line_start(l.start);
                let end = if l.end < n_lines { self.line_start(l.end) } else { self.len_chars() };
                // Last line: also eat the preceding newline.
                let start = if l.end >= n_lines && start > 0 { start - 1 } else { start };
                (start..end, String::new())
            })
            .collect();
        let starts: Vec<usize> = edits.iter().map(|(r, _)| r.start).collect();
        self.apply(edits, EditKind::Other, |map, b| {
            starts
                .iter()
                .map(|&p| {
                    let np = map(p, false).min(b.len_chars());
                    let (line, _) = b.pos_to_line_col(np);
                    let ws = b.leading_ws(line).chars().count();
                    Selection::caret(b.line_start(line) + ws)
                })
                .collect()
        });
    }

    /// Alt+Shift+Up/Down. Operates on the primary selection's lines.
    pub fn move_lines(&mut self, up: bool) {
        let s = self.primary();
        let lines = self.selected_lines(&s);
        let n_lines = self.len_lines();
        if (up && lines.start == 0) || (!up && lines.end >= n_lines) {
            return;
        }
        // Make sure the last line has a trailing newline to simplify the swap.
        let block_start = self.line_start(lines.start);
        let block_end = self.line_end(lines.end - 1);
        let block = self.slice(block_start..block_end);
        let (swap_line, range) = if up {
            let l = lines.start - 1;
            (l, self.line_start(l)..block_end)
        } else {
            let l = lines.end;
            (l, block_start..self.line_end(l))
        };
        let other = self.line(swap_line);
        let replacement = if up { format!("{block}\n{other}") } else { format!("{other}\n{block}") };
        let other_len = other.chars().count() as isize + 1;
        let delta: isize = if up { -other_len } else { other_len };
        let (a, h) = (s.anchor as isize + delta, s.head as isize + delta);
        self.apply(vec![(range, replacement)], EditKind::Other, |_, _| {
            vec![Selection { anchor: a as usize, head: h as usize, goal_col: s.goal_col }]
        });
    }

    /// Ctrl+/: toggle line comment on selected lines, PyCharm style (comment at column 0).
    pub fn toggle_line_comment(&mut self) {
        let Some(prefix) = self.line_comment.clone() else { return };
        let mut all_lines: Vec<usize> = Vec::new();
        for s in &self.selections {
            all_lines.extend(self.selected_lines(s));
        }
        all_lines.sort();
        all_lines.dedup();
        let non_empty: Vec<usize> = all_lines.iter().copied().filter(|&l| !self.line(l).trim().is_empty()).collect();
        let targets = if non_empty.is_empty() { all_lines.clone() } else { non_empty };
        let all_commented = targets.iter().all(|&l| self.line(l).trim_start().starts_with(&prefix));
        let plen = prefix.chars().count();
        let mut edits = Vec::new();
        for &l in &targets {
            let start = self.line_start(l);
            if all_commented {
                let text = self.line(l);
                let ws = text.chars().take_while(|c| c.is_whitespace()).count();
                let after: String = text.chars().skip(ws + plen).take(1).collect();
                let extra = if after == " " { 1 } else { 0 };
                edits.push((start + ws..start + ws + plen + extra, String::new()));
            } else {
                edits.push((start..start, format!("{prefix} ")));
            }
        }
        let single_caret = self.selections.len() == 1 && self.primary().is_empty();
        let sels = self.selections.clone();
        self.apply(edits, EditKind::Other, |map, b| {
            if single_caret {
                // PyCharm moves the caret to the next line after commenting.
                let (line, col) = b.pos_to_line_col(map(sels[0].head, true));
                let next = (line + 1).min(b.len_lines().saturating_sub(1));
                vec![Selection::caret(b.line_col_to_pos(next, col))]
            } else {
                sels.iter()
                    .map(|s| Selection { anchor: map(s.anchor, true), head: map(s.head, true), goal_col: None })
                    .collect()
            }
        });
    }

    /// Tab: indent selected lines, or insert indent at caret.
    pub fn indent(&mut self) {
        if self.selections.iter().all(|s| s.is_empty()) {
            let unit = self.indent_unit.clone();
            return self.insert(&unit);
        }
        self.reindent(true);
    }

    /// Shift+Tab.
    pub fn unindent(&mut self) {
        self.reindent(false);
    }

    fn reindent(&mut self, add: bool) {
        let mut lines: Vec<usize> = Vec::new();
        for s in &self.selections {
            lines.extend(self.selected_lines(s));
        }
        lines.sort();
        lines.dedup();
        let unit = self.indent_unit.clone();
        let ulen = unit.chars().count();
        let mut edits = Vec::new();
        for l in lines {
            let start = self.line_start(l);
            if add {
                if !self.line(l).trim().is_empty() {
                    edits.push((start..start, unit.clone()));
                }
            } else {
                let ws = self.line(l).chars().take(ulen).take_while(|c| *c == ' ').count();
                let ws = if ws == 0 && self.line(l).starts_with('\t') { 1 } else { ws };
                if ws > 0 {
                    edits.push((start..start + ws, String::new()));
                }
            }
        }
        let sels = self.selections.clone();
        self.apply(edits, EditKind::Other, |map, _| {
            sels.iter()
                .map(|s| {
                    let (a_bias, h_bias) = (s.anchor > s.head, s.head >= s.anchor);
                    Selection { anchor: map(s.anchor, a_bias), head: map(s.head, h_bias), goal_col: None }
                })
                .collect()
        });
    }

    /// Ctrl+Shift+J.
    pub fn join_lines(&mut self) {
        let s = self.primary();
        let (line, _) = self.pos_to_line_col(s.head);
        if line + 1 >= self.len_lines() {
            return;
        }
        let end = self.line_end(line);
        let next_ws = self.leading_ws(line + 1).chars().count();
        let next_start = self.line_start(line + 1);
        let cur_trailing = self.line(line).chars().rev().take_while(|c| c.is_whitespace()).count();
        let r = end - cur_trailing..next_start + next_ws;
        let next_empty = self.line(line + 1).trim().is_empty();
        let sep = if next_empty { "" } else { " " };
        let at = r.start;
        self.apply(vec![(r, sep.into())], EditKind::Other, |_, _| vec![Selection::caret(at)]);
    }

    /// Ctrl+Shift+U.
    pub fn toggle_case(&mut self) {
        let ranges: Vec<_> = self.selections.iter().filter(|s| !s.is_empty()).map(|s| s.range()).collect();
        let edits: Vec<_> = ranges
            .iter()
            .map(|r| {
                let t = self.slice(r.clone());
                let up = t.chars().any(|c| c.is_lowercase());
                (r.clone(), if up { t.to_uppercase() } else { t.to_lowercase() })
            })
            .collect();
        let sels = self.selections.clone();
        self.apply(edits, EditKind::Other, |_, _| sels);
    }

    // --- Movement and selection ----------------------------------------------

    fn is_word_char(c: char) -> bool {
        c.is_alphanumeric() || c == '_'
    }

    /// PyCharm-style word boundary (camel humps off): stops at word/punctuation/whitespace transitions.
    pub fn word_boundary(&self, pos: usize, forward: bool) -> usize {
        let len = self.len_chars();
        let class = |c: char| {
            if c == '\n' {
                0
            } else if c.is_whitespace() {
                1
            } else if Self::is_word_char(c) {
                2
            } else {
                3
            }
        };
        if forward {
            if pos >= len {
                return len;
            }
            let mut p = pos;
            let start_class = class(self.rope.char(p));
            if start_class == 0 {
                return p + 1;
            }
            while p < len && class(self.rope.char(p)) == start_class {
                p += 1;
            }
            if start_class != 1 {
                while p < len && class(self.rope.char(p)) == 1 {
                    p += 1;
                }
            }
            p
        } else {
            if pos == 0 {
                return 0;
            }
            let mut p = pos;
            while p > 0 && class(self.rope.char(p - 1)) == 1 {
                p -= 1;
            }
            if p == 0 {
                return 0;
            }
            let c = class(self.rope.char(p - 1));
            if c == 0 {
                return if p == pos { p - 1 } else { p };
            }
            while p > 0 && class(self.rope.char(p - 1)) == c {
                p -= 1;
            }
            p
        }
    }

    fn move_one(&self, s: Selection, m: Movement, extend: bool) -> Selection {
        let len = self.len_chars();
        // Collapsing a selection with Left/Right goes to its edge, like PyCharm.
        if !extend && !s.is_empty() {
            match m {
                Movement::Left => return Selection::caret(s.range().start),
                Movement::Right => return Selection::caret(s.range().end),
                _ => {}
            }
        }
        let (line, col) = self.pos_to_line_col(s.head);
        let mut goal = None;
        let head = match m {
            Movement::Left => s.head.saturating_sub(1),
            Movement::Right => (s.head + 1).min(len),
            Movement::Up | Movement::Down | Movement::PageUp(_) | Movement::PageDown(_) => {
                let g = s.goal_col.unwrap_or(col);
                goal = Some(g);
                let target = match m {
                    Movement::Up => line.checked_sub(1),
                    Movement::Down => (line + 1 < self.len_lines()).then_some(line + 1),
                    Movement::PageUp(n) => Some(line.saturating_sub(n)),
                    Movement::PageDown(n) => Some((line + n).min(self.len_lines() - 1)),
                    _ => unreachable!(),
                };
                match target {
                    Some(t) => self.line_col_to_pos(t, g),
                    None if matches!(m, Movement::Up) => 0,
                    None => len,
                }
            }
            Movement::WordLeft => self.word_boundary(s.head, false),
            Movement::WordRight => self.word_boundary(s.head, true),
            Movement::LineStart => {
                // Smart Home: toggle between first non-space and column 0.
                let ws = self.leading_ws(line).chars().count();
                self.line_start(line) + if col == ws { 0 } else { ws }
            }
            Movement::LineEnd => self.line_end(line),
            Movement::DocStart => 0,
            Movement::DocEnd => len,
        };
        Selection { anchor: if extend { s.anchor } else { head }, head, goal_col: goal }
    }

    pub fn move_carets(&mut self, m: Movement, extend: bool) {
        let mut sels: Vec<_> = self.selections.iter().map(|&s| self.move_one(s, m, extend)).collect();
        normalize(&mut sels, self.len_chars());
        self.selections = sels;
        self.break_undo_group();
    }

    pub fn set_caret(&mut self, pos: usize, extend: bool) {
        let pos = pos.min(self.len_chars());
        let p = self.primary();
        self.selections = vec![Selection { anchor: if extend { p.anchor } else { pos }, head: pos, goal_col: None }];
        self.break_undo_group();
    }

    /// Alt+click: add a caret.
    pub fn add_caret(&mut self, pos: usize) {
        let len = self.len_chars();
        self.selections.push(Selection::caret(pos.min(len)));
        normalize(&mut self.selections, len);
    }

    pub fn select_all(&mut self) {
        self.selections = vec![Selection { anchor: 0, head: self.len_chars(), goal_col: None }];
    }

    /// Escape: collapse to the primary caret.
    pub fn collapse_selections(&mut self) {
        let p = self.primary();
        self.selections = vec![Selection::caret(p.head)];
    }

    pub fn word_range_at(&self, pos: usize) -> Range<usize> {
        let len = self.len_chars();
        let mut a = pos.min(len);
        let mut b = a;
        while a > 0 && Self::is_word_char(self.rope.char(a - 1)) {
            a -= 1;
        }
        while b < len && Self::is_word_char(self.rope.char(b)) {
            b += 1;
        }
        a..b
    }

    /// Alt+J: select word, then add the next occurrence of the selected text.
    pub fn add_next_occurrence(&mut self) {
        let p = self.primary();
        if p.is_empty() {
            let r = self.word_range_at(p.head);
            if r.is_empty() {
                return;
            }
            let last = self.selections.len() - 1;
            self.selections[last] = Selection { anchor: r.start, head: r.end, goal_col: None };
            return;
        }
        let needle = self.slice(p.range());
        let text = self.text();
        // Search in char space via byte offsets.
        let from_byte = self.rope.char_to_byte(p.range().end);
        let found = text[from_byte..]
            .find(&needle)
            .map(|i| from_byte + i)
            .or_else(|| text[..from_byte].find(&needle));
        if let Some(b) = found {
            let st = self.rope.byte_to_char(b);
            let sel = Selection { anchor: st, head: st + needle.chars().count(), goal_col: None };
            if !self.selections.iter().any(|s| s.range() == sel.range()) {
                self.selections.push(sel);
            }
        }
    }

    /// Ctrl+Alt+Shift+J.
    pub fn select_all_occurrences(&mut self) {
        let p = self.primary();
        let r = if p.is_empty() { self.word_range_at(p.head) } else { p.range() };
        if r.is_empty() {
            return;
        }
        let needle = self.slice(r);
        let text = self.text();
        let n = needle.chars().count();
        let mut sels: Vec<Selection> = text
            .match_indices(&needle)
            .map(|(b, _)| {
                let st = self.rope.byte_to_char(b);
                Selection { anchor: st, head: st + n, goal_col: None }
            })
            .collect();
        if !sels.is_empty() {
            normalize(&mut sels, self.len_chars());
            self.selections = sels;
        }
    }
}

/// Sort selections and merge overlapping ones; keep at least one caret.
fn normalize(sels: &mut Vec<Selection>, len: usize) {
    for s in sels.iter_mut() {
        s.anchor = s.anchor.min(len);
        s.head = s.head.min(len);
    }
    if sels.is_empty() {
        sels.push(Selection::caret(0));
        return;
    }
    if sels.len() == 1 {
        return;
    }
    sels.sort_by_key(|s| s.range().start);
    let mut out: Vec<Selection> = Vec::with_capacity(sels.len());
    for s in sels.drain(..) {
        if let Some(last) = out.last_mut() {
            let (lr, sr) = (last.range(), s.range());
            if sr.start < lr.end || (sr.start == lr.end && (s.is_empty() || last.is_empty()) && sr.start == lr.start) {
                let merged = lr.start.min(sr.start)..lr.end.max(sr.end);
                *last = Selection { anchor: merged.start, head: merged.end, goal_col: None };
                continue;
            }
        }
        out.push(s);
    }
    *sels = out;
}

#[cfg(test)]
mod tests;
