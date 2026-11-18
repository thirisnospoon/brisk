//! Caret marks, as in PyCharm: the bracket next to the caret and its partner, and, once the
//! caret has rested for `OCCURRENCE_DELAY`, the other usages of the identifier under it (also
//! as marks on the scrollbar). In Python the usages come from the syntax tree's scopes (a
//! local `x` in one function does not light up `x` in another); elsewhere, and for attribute
//! names, from the same identifier text. No ty needed.

use super::Editor;
use gpui::Context;
use std::ops::Range;
use std::time::{Duration, Instant};

/// Whole-file occurrence marks only for files up to this size (bytes).
const STRIPE_LIMIT: usize = 2 * 1024 * 1024;

/// How long the caret rests before its usages are highlighted.
const OCCURRENCE_DELAY: Duration = Duration::from_millis(300);

#[derive(Default)]
pub struct CaretMarks {
    /// Char ranges of the two matching brackets.
    pub brackets: Vec<Range<usize>>,
    /// Char ranges of the identifier occurrences in the visible part.
    pub occurrences: Vec<Range<usize>>,
    /// Lines of all occurrences (scrollbar marks).
    pub occurrence_lines: Vec<usize>,
}

/// Usages of the identifier at the caret, computed once per caret position and text version.
pub struct Occurrences {
    /// (caret, buffer version) they are for.
    key: (usize, u64),
    since: Instant,
    ready: bool,
    /// Char ranges, sorted.
    all: Vec<Range<usize>>,
    lines: Vec<usize>,
}

impl Default for Occurrences {
    fn default() -> Self {
        Self { key: (usize::MAX, u64::MAX), since: Instant::now(), ready: false, all: Vec::new(), lines: Vec::new() }
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    /// Prepaint: restart the delay when the caret or the text changed; compute the usages
    /// once it has passed. Uses `text_cache` (fresh after `reparse`).
    pub(super) fn update_occurrences(&mut self, cx: &mut Context<Self>) {
        let b = &self.buffer;
        let single = b.selections.len() == 1 && b.primary().is_empty() && self.read_error.is_none();
        let key = if single { (b.primary().head, b.version()) } else { (usize::MAX, b.version()) };
        if key != self.occ.key {
            self.occ = Occurrences { key, since: Instant::now(), ..Default::default() };
            if single {
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(OCCURRENCE_DELAY).await;
                    let _ = this.update(cx, |e, cx| {
                        if e.occ.key == key {
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            return;
        }
        if self.occ.ready || !single || self.occ.since.elapsed() < OCCURRENCE_DELAY || self.text_cache.0 != b.version() {
            return;
        }
        self.fill_occurrences();
    }

    /// Compute the usages for the current caret now (also for smokes, without the delay).
    pub(super) fn fill_occurrences(&mut self) {
        let head = self.buffer.primary().head;
        self.occ.key = (head, self.buffer.version());
        self.occ.ready = true;
        let all = self.occurrences_at(head);
        let rope = self.buffer.rope();
        let mut lines: Vec<usize> = all.iter().map(|r| rope.char_to_line(r.start)).collect();
        lines.dedup();
        self.occ.all = all;
        self.occ.lines = if self.text_cache.1.len() <= STRIPE_LIMIT { lines } else { Vec::new() };
    }

    /// Usages (char ranges) of the identifier at char `head`, or the one just before it.
    fn occurrences_at(&self, head: usize) -> Vec<Range<usize>> {
        let Some(s) = &self.syntax else { return Vec::new() };
        let text = &self.text_cache.1;
        let rope = self.buffer.rope();
        let to_chars = |r: Range<usize>| rope.byte_to_char(r.start)..rope.byte_to_char(r.end);
        let hb = rope.char_to_byte(head);
        if let Some(refs) = s.name_refs(text, hb) {
            return refs.ranges.into_iter().map(to_chars).collect();
        }
        let before = (head > 0).then(|| rope.char_to_byte(head - 1));
        let Some(word) = s.identifier_at(hb).or_else(|| before.and_then(|p| s.identifier_at(p))) else {
            return Vec::new();
        };
        let name = &text[word.clone()];
        if name.is_empty() || text.len() > STRIPE_LIMIT {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut from = 0;
        while let Some(i) = text[from..].find(name) {
            let st = from + i;
            let en = st + name.len();
            from = en;
            let left_ok = !text[..st].chars().next_back().is_some_and(is_word);
            let right_ok = !text[en..].chars().next().is_some_and(is_word);
            if left_ok && right_ok && s.is_identifier(st..en) {
                out.push(to_chars(st..en));
            }
        }
        out
    }

    /// The variable at the caret and its usages in this file, from the syntax tree (Python):
    /// Rename and Find Usages without ty.
    pub fn name_refs_at_caret(&mut self) -> Option<brisk_syntax::NameRefs> {
        self.reparse();
        let head = self.buffer.primary().head;
        let hb = self.buffer.rope().char_to_byte(head);
        let text = self.text().to_string();
        self.syntax.as_ref()?.name_refs(&text, hb)
    }

    /// Smokes: how many usages are highlighted now (none until the caret has rested).
    pub fn shown_occurrences(&self) -> usize {
        self.caret_marks(0..self.text_cache.1.len()).occurrences.len()
    }

    /// Marks for the visible byte range.
    pub(super) fn caret_marks(&self, bytes: Range<usize>) -> CaretMarks {
        let mut out = CaretMarks::default();
        let b = &self.buffer;
        if b.selections.len() != 1 || !b.primary().is_empty() || self.read_error.is_some() {
            return out;
        }
        let text = &self.text_cache.1;
        if self.text_cache.0 != b.version() {
            return out;
        }
        let rope = b.rope();
        let head = b.primary().head;
        let hb = rope.char_to_byte(head);

        // Brackets: the one after the caret, else the one before it.
        let before = (head > 0).then(|| rope.char_to_byte(head - 1));
        let pair = match &self.syntax {
            Some(s) => s.bracket_pair(hb).or_else(|| before.and_then(|p| s.bracket_pair(p))),
            None => scan_pair(text, hb).or_else(|| before.and_then(|p| scan_pair(text, p))),
        };
        if let Some((a, z)) = pair {
            out.brackets = vec![rope.byte_to_char(a.start)..rope.byte_to_char(a.end), rope.byte_to_char(z.start)..rope.byte_to_char(z.end)];
        }

        // Usages, once the caret has rested (see `update_occurrences`).
        if self.occ.ready && self.occ.key == (head, b.version()) && !self.occ.all.is_empty() {
            let lo = rope.byte_to_char(floor_char(text, bytes.start.min(text.len())));
            let hi = rope.byte_to_char(floor_char(text, bytes.end.min(text.len())));
            out.occurrences = self.occ.all.iter().filter(|r| r.end >= lo && r.start <= hi).cloned().collect();
            out.occurrence_lines = self.occ.lines.clone();
        }
        out
    }
}

fn floor_char(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Bracket matching without a syntax tree: a plain scan, at most 20 000 bytes away.
fn scan_pair(text: &str, at: usize) -> Option<(Range<usize>, Range<usize>)> {
    let bytes = text.as_bytes();
    let c = *bytes.get(at)?;
    let (open, close, forward) = match c {
        b'(' => (b'(', b')', true),
        b'[' => (b'[', b']', true),
        b'{' => (b'{', b'}', true),
        b')' => (b'(', b')', false),
        b']' => (b'[', b']', false),
        b'}' => (b'{', b'}', false),
        _ => return None,
    };
    let mut depth = 0i32;
    if forward {
        for (i, &x) in bytes.iter().enumerate().skip(at).take(20_000) {
            if x == open {
                depth += 1;
            } else if x == close {
                depth -= 1;
                if depth == 0 {
                    return Some((at..at + 1, i..i + 1));
                }
            }
        }
    } else {
        let start = at.saturating_sub(20_000);
        for i in (start..=at).rev() {
            let x = bytes[i];
            if x == close {
                depth += 1;
            } else if x == open {
                depth -= 1;
                if depth == 0 {
                    return Some((at..at + 1, i..i + 1));
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    #[test]
    fn scan_pair_plain_text() {
        let t = "a(b[c]d)e";
        assert_eq!(super::scan_pair(t, 1), Some((1..2, 7..8)));
        assert_eq!(super::scan_pair(t, 7), Some((7..8, 1..2)));
        assert_eq!(super::scan_pair(t, 0), None);
    }
}
