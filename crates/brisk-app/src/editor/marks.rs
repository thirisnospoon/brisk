//! Caret marks, as in PyCharm: the bracket next to the caret and its partner, and the other
//! occurrences of the identifier under the caret (also as marks on the scrollbar).

use super::Editor;
use std::ops::Range;

/// Whole-file occurrence marks only for files up to this size (bytes).
const STRIPE_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Default)]
pub struct CaretMarks {
    /// Char ranges of the two matching brackets.
    pub brackets: Vec<Range<usize>>,
    /// Char ranges of the identifier occurrences in the visible part.
    pub occurrences: Vec<Range<usize>>,
    /// Lines of all occurrences (scrollbar marks).
    pub occurrence_lines: Vec<usize>,
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Editor {
    /// Marks for the visible byte range. Uses `text_cache` (fresh after `reparse`).
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
        let to_chars = |r: Range<usize>| rope.byte_to_char(r.start)..rope.byte_to_char(r.end);

        // Brackets: the one after the caret, else the one before it.
        let before = (head > 0).then(|| rope.char_to_byte(head - 1));
        let pair = match &self.syntax {
            Some(s) => s.bracket_pair(hb).or_else(|| before.and_then(|p| s.bracket_pair(p))),
            None => scan_pair(text, hb).or_else(|| before.and_then(|p| scan_pair(text, p))),
        };
        if let Some((a, z)) = pair {
            out.brackets = vec![to_chars(a), to_chars(z)];
        }

        // Occurrences of the identifier under the caret (not in strings or comments).
        let Some(s) = &self.syntax else { return out };
        let Some(word) = s.identifier_at(hb).or_else(|| before.and_then(|p| s.identifier_at(p))) else {
            return out;
        };
        let name = &text[word.clone()];
        if name.is_empty() {
            return out;
        }
        let find_all = |range: Range<usize>, mut f: Box<dyn FnMut(Range<usize>) + '_>| {
            let hay = &text[range.clone()];
            let mut from = 0;
            while let Some(i) = hay[from..].find(name) {
                let st = range.start + from + i;
                let en = st + name.len();
                from += i + name.len();
                let left_ok = !text[..st].chars().next_back().is_some_and(is_word);
                let right_ok = !text[en..].chars().next().is_some_and(is_word);
                if left_ok && right_ok && s.is_identifier(st..en) {
                    f(st..en);
                }
            }
        };
        let lo = floor_char(text, bytes.start.min(text.len()));
        let hi = floor_char(text, bytes.end.min(text.len()));
        let mut occ = Vec::new();
        find_all(lo..hi, Box::new(|r| occ.push(r)));
        out.occurrences = occ.into_iter().map(to_chars).collect();
        if text.len() <= STRIPE_LIMIT {
            let mut lines = Vec::new();
            find_all(0..text.len(), Box::new(|r| lines.push(rope.byte_to_line(r.start))));
            lines.dedup();
            out.occurrence_lines = lines;
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
