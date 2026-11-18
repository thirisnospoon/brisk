//! Inlay hints from ty, as PyCharm draws them: inferred types after names (`x: list[int]`)
//! and parameter names before arguments (`f(a=1)`), dimmed inside the line but not part of
//! the text. A line shows its hints only while its text is what ty saw, so typing never
//! leaves a hint at a wrong place; fresh hints follow the next check.

use super::Editor;
use brisk_lsp::InlayHint;
use gpui::TextRun;
use std::collections::HashMap;

#[derive(Default)]
pub struct Inlays {
    /// Per line: the text ty saw, and the hints as (byte offset, label).
    lines: HashMap<usize, (String, Vec<(usize, String)>)>,
}

impl Editor {
    /// Hints computed for buffer `version`; dropped when the text changed since.
    pub fn set_inlay_hints(&mut self, version: u64, hints: Vec<InlayHint>) -> bool {
        if version != self.buffer.version() {
            return false;
        }
        let mut lines: HashMap<usize, (String, Vec<(usize, String)>)> = HashMap::new();
        for h in hints {
            if h.pos.line >= self.buffer.len_lines() {
                continue;
            }
            let entry = lines.entry(h.pos.line).or_insert_with(|| (self.buffer.line(h.pos.line), Vec::new()));
            let byte = super::char_to_byte(&entry.0, h.pos.col);
            entry.1.push((byte, h.label));
        }
        for (_, hints) in lines.values_mut() {
            hints.sort_by_key(|h| h.0);
        }
        let changed = lines != self.inlays.lines;
        self.inlays.lines = lines;
        changed
    }

    /// Hints on `line` when its text is still `text`.
    fn hints_on(&self, line: usize, text: &str) -> &[(usize, String)] {
        match self.inlays.lines.get(&line) {
            Some((t, h)) if t == text => h,
            _ => &[],
        }
    }

    /// The line as drawn: hint labels spliced in before their positions, each in a run made
    /// by `hint_run(len)`; the text's own runs are split around them.
    pub(super) fn line_with_inlays(&self, line: usize, text: String, runs: Vec<TextRun>, hint_run: impl Fn(usize) -> TextRun) -> (String, Vec<TextRun>) {
        let hints = self.hints_on(line, &text);
        if hints.is_empty() {
            return (text, runs);
        }
        let mut src: std::collections::VecDeque<TextRun> = runs.into();
        let mut out_text = String::with_capacity(text.len() + hints.iter().map(|h| h.1.len()).sum::<usize>());
        let mut out_runs = Vec::new();
        let mut take = |n: usize, out: &mut Vec<TextRun>| {
            let mut left = n;
            while left > 0 {
                let Some(mut r) = src.pop_front() else { break };
                if r.len > left {
                    let mut rest = r.clone();
                    rest.len = r.len - left;
                    r.len = left;
                    src.push_front(rest);
                }
                left -= r.len;
                out.push(r);
            }
        };
        let mut at = 0;
        for (byte, label) in hints {
            let byte = (*byte).min(text.len());
            take(byte - at, &mut out_runs);
            out_text.push_str(&text[at..byte]);
            out_text.push_str(label);
            out_runs.push(hint_run(label.len()));
            at = byte;
        }
        take(text.len() - at, &mut out_runs);
        out_text.push_str(&text[at..]);
        (out_text, out_runs)
    }

    /// Byte offset in the drawn line for byte `byte` of the text.
    pub(super) fn display_byte(&self, line: usize, text: &str, byte: usize) -> usize {
        display_byte(self.hints_on(line, text), byte)
    }

    /// Text byte for a byte of the drawn line.
    pub(super) fn text_byte(&self, line: usize, text: &str, shown: usize) -> usize {
        text_byte(self.hints_on(line, text), text.len(), shown)
    }

    /// Hints shown on `line`, as text: for smokes.
    pub fn inlay_labels(&self, line: usize) -> Vec<String> {
        let text = self.buffer.line(line);
        self.hints_on(line, &text).iter().map(|h| h.1.clone()).collect()
    }
}

/// Hints before `byte` shift it; a hint right at it is drawn after the caret.
fn display_byte(hints: &[(usize, String)], byte: usize) -> usize {
    byte + hints.iter().filter(|h| h.0 < byte).map(|h| h.1.len()).sum::<usize>()
}

/// A point inside a hint lands where the hint stands.
fn text_byte(hints: &[(usize, String)], len: usize, shown: usize) -> usize {
    let mut shift = 0;
    for (byte, label) in hints {
        if shown <= byte + shift {
            break;
        }
        if shown < byte + shift + label.len() {
            return *byte;
        }
        shift += label.len();
    }
    shown.saturating_sub(shift).min(len)
}

#[cfg(test)]
mod tests {
    use super::{display_byte, text_byte};

    #[test]
    fn offsets() {
        // "x = f(1)" drawn as "x: int = f(a=1)".
        let h = vec![(1, ": int".to_string()), (6, "a=".to_string())];
        assert_eq!([0, 1, 2, 6, 7, 8].map(|b| display_byte(&h, b)), [0, 1, 7, 11, 14, 15]);
        // Clicks: before the hint, inside it, after it, inside "a=", on "1", at the end.
        assert_eq!([1, 3, 6, 12, 13, 15].map(|d| text_byte(&h, 8, d)), [1, 1, 1, 6, 6, 8]);
    }
}
