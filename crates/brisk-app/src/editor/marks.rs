//! Caret marks, as in PyCharm: the bracket next to the caret and its partner, and, once the
//! caret has rested for `OCCURRENCE_DELAY`, the other usages of the identifier under it (also
//! as marks on the scrollbar). In Python the usages come from the syntax tree's scopes (a
//! local `x` in one function does not light up `x` in another); elsewhere, and for attribute
//! names, from the same identifier text. No ty needed.

use super::Editor;
use gpui::Context;
use std::ops::Range;
use std::time::Duration;

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
    ready: bool,
    /// Char ranges, sorted.
    all: Vec<Range<usize>>,
    lines: Vec<usize>,
}

impl Default for Occurrences {
    fn default() -> Self {
        Self { key: (usize::MAX, u64::MAX), ready: false, all: Vec::new(), lines: Vec::new() }
    }
}

/// Usages (char ranges, sorted) of the identifier at char `head`, and their lines for the
/// scrollbar. Pure, so a large file runs it on a background thread.
fn find_usages(lang: brisk_syntax::Lang, tree: &brisk_syntax::Tree, text: &str, rope: &brisk_text::TextRope, head: usize) -> (Vec<Range<usize>>, Vec<usize>) {
    let hb = rope.char_to_byte(head);
    let all: Vec<_> = brisk_syntax::occurrences_of(lang, tree, text, hb, STRIPE_LIMIT).into_iter().map(|r| rope.byte_to_char(r.start)..rope.byte_to_char(r.end)).collect();
    let mut lines: Vec<usize> = if text.len() <= STRIPE_LIMIT { all.iter().map(|r| rope.char_to_line(r.start)).collect() } else { Vec::new() };
    lines.dedup();
    (all, lines)
}

impl Editor {
    /// Prepaint: when the caret or the text changed, forget the usages and start the delay;
    /// once it has passed, compute them (on a background thread in a long file).
    pub(super) fn update_occurrences(&mut self, cx: &mut Context<Self>) {
        let b = &self.buffer;
        let single = b.selections.len() == 1 && b.primary().is_empty() && self.read_error.is_none();
        let key = if single { (b.primary().head, b.version()) } else { (usize::MAX, b.version()) };
        if key == self.occ.key {
            return;
        }
        self.occ = Occurrences { key, ..Default::default() };
        if !single {
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(OCCURRENCE_DELAY).await;
            let job = this
                .update(cx, |e, cx| {
                    if e.occ.key != key {
                        return None;
                    }
                    e.reparse();
                    if e.buffer.len_lines() < super::big::LAZY_FOLD_LINES {
                        e.fill_occurrences();
                        cx.notify();
                        return None;
                    }
                    let s = e.syntax.as_ref()?;
                    let (lang, tree, text, rope) = (s.lang(), s.tree()?.clone(), e.text_cache.1.clone(), e.buffer.rope().clone());
                    Some(cx.background_executor().spawn(async move { find_usages(lang, &tree, &text, &rope, key.0) }))
                })
                .ok()
                .flatten();
            let Some(job) = job else { return };
            let (all, lines) = job.await;
            let _ = this.update(cx, |e, cx| {
                if e.occ.key == key {
                    (e.occ.ready, e.occ.all, e.occ.lines) = (true, all, lines);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Compute the usages for the current caret now (also for smokes, without the delay).
    pub(super) fn fill_occurrences(&mut self) {
        let head = self.buffer.primary().head;
        self.occ.key = (head, self.buffer.version());
        self.occ.ready = true;
        self.text();
        let Some(tree) = self.syntax.as_ref().and_then(|s| s.tree()) else {
            (self.occ.all, self.occ.lines) = (Vec::new(), Vec::new());
            return;
        };
        let lang = self.syntax.as_ref().map(|s| s.lang()).unwrap_or(brisk_syntax::Lang::Python);
        (self.occ.all, self.occ.lines) = find_usages(lang, tree, &self.text_cache.1, self.buffer.rope(), head);
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
