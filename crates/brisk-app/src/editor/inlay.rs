//! Inlay hints from ty, as PyCharm draws them: inferred types after names (`x: list[int]`)
//! and parameter names before arguments (`f(a=1)`), inside the line but not part of the text.
//! Each hint is a chip: the UI font in `Theme::inlay_fg` on a rounded `Theme::inlay_bg`, so it
//! does not read as code. Useless hints are dropped (`Any`, `Unknown`, a type already visible
//! from a literal or a constructor call on the right), and long ones are shortened
//! (`tuple[Literal["bot"], …]`) with the full text in a tooltip. A line shows its hints only
//! while its text is what ty saw, so typing never leaves a hint at a wrong place.

use super::Editor;
use brisk_lsp::InlayHint;
use gpui::{Pixels, Point, TextRun};
use std::collections::HashMap;
use std::ops::Range;

/// Longer type hints are shortened.
const MAX_HINT: usize = 40;

/// Padding inside the chip, as text (one space of the UI font on each side).
const PAD: &str = " ";

#[derive(Clone, Debug, PartialEq)]
pub struct Hint {
    /// Byte offset in the line it stands before.
    byte: usize,
    /// Drawn text, with the padding.
    shown: String,
    /// The whole label when `shown` is shortened.
    full: Option<String>,
    /// The grey type after a typed colon (editor/type_hint.rs): drawn as code, no chip.
    ghost: bool,
}

impl Hint {
    fn new(byte: usize, label: &str) -> Self {
        let short = shorten(label);
        let full = (short != label).then(|| label.to_string());
        Hint { byte, shown: format!("{PAD}{short}{PAD}"), full, ghost: false }
    }

    /// The label as drawn, without the padding.
    #[cfg(feature = "smoke")]
    fn label(&self) -> &str {
        &self.shown[PAD.len()..self.shown.len() - PAD.len()]
    }
}

#[derive(Default)]
pub struct Inlays {
    /// Per line: the text ty saw, and its hints sorted by position.
    lines: HashMap<usize, (String, Vec<Hint>)>,
    /// The full text of a shortened hint under the mouse, and where to show it.
    pub(super) tip: Option<(Point<Pixels>, String)>,
    /// Per line: the text ty saw, and every type it inferred there (byte, label), the
    /// obvious ones too: typing `:` after a name offers them (editor/type_hint.rs).
    raw: HashMap<usize, (String, Vec<(usize, String)>)>,
}

impl Inlays {
    /// ty's type for the name ending at `byte` of `line`, while the line is still `text`.
    pub(super) fn raw_type(&self, line: usize, text: &str, byte: usize) -> Option<&str> {
        match self.raw.get(&line) {
            Some((t, types)) if t == text => types.iter().find(|(b, _)| *b == byte).map(|(_, l)| l.as_str()),
            _ => None,
        }
    }
}

impl Editor {
    /// Hints computed for buffer `version`; dropped when the text changed since.
    pub fn set_inlay_hints(&mut self, version: u64, hints: Vec<InlayHint>) -> bool {
        if version != self.buffer.version() {
            return false;
        }
        let mut lines: HashMap<usize, (String, Vec<Hint>)> = HashMap::new();
        let mut raw: HashMap<usize, (String, Vec<(usize, String)>)> = HashMap::new();
        for h in hints {
            if h.pos.line >= self.buffer.len_lines() {
                continue;
            }
            if !h.parameter {
                let r = raw.entry(h.pos.line).or_insert_with(|| (self.buffer.line(h.pos.line), Vec::new()));
                let byte = super::char_to_byte(&r.0, h.pos.col);
                r.1.push((byte, h.label.clone()));
            }
            let entry = lines.entry(h.pos.line).or_insert_with(|| (self.buffer.line(h.pos.line), Vec::new()));
            let byte = super::char_to_byte(&entry.0, h.pos.col);
            if h.parameter || useful_type(&h.label, &entry.0[byte.min(entry.0.len())..]) {
                entry.1.push(Hint::new(byte, &h.label));
            }
        }
        lines.retain(|_, (_, h)| !h.is_empty());
        for (_, hints) in lines.values_mut() {
            hints.sort_by_key(|h| h.byte);
        }
        let changed = lines != self.inlays.lines;
        self.inlays.lines = lines;
        self.inlays.raw = raw;
        changed
    }

    /// Hints on `line` when its text is still `text` and hints are turned on in Settings.
    fn hints_on(&self, line: usize, text: &str) -> std::borrow::Cow<'_, [Hint]> {
        let hints: &[Hint] = match self.inlays.lines.get(&line) {
            Some((t, h)) if t == text && brisk_settings::inlay_hints() => h,
            _ => &[],
        };
        // The typed-colon ghost joins them, in its place.
        match self.live_type_ghost().filter(|g| g.line == line && g.text == text) {
            Some(g) => {
                let mut all = hints.to_vec();
                let at = all.iter().position(|h| h.byte > g.byte).unwrap_or(all.len());
                all.insert(at, Hint { byte: g.byte, shown: g.insert.clone(), full: None, ghost: true });
                std::borrow::Cow::Owned(all)
            }
            None => std::borrow::Cow::Borrowed(hints),
        }
    }

    /// The line as drawn: hints spliced in before their positions, each in a run made by
    /// `hint_run(len)`; the text's own runs are split around them. Also returns the byte
    /// ranges of the hints in the drawn line (their chips).
    pub(super) fn line_with_inlays(
        &self,
        line: usize,
        text: String,
        runs: Vec<TextRun>,
        hint_run: impl Fn(usize, bool) -> TextRun,
    ) -> (String, Vec<TextRun>, Vec<Range<usize>>) {
        let hints = self.hints_on(line, &text);
        if hints.is_empty() {
            return (text, runs, Vec::new());
        }
        let mut src: std::collections::VecDeque<TextRun> = runs.into();
        let mut out_text = String::with_capacity(text.len() + hints.iter().map(|h| h.shown.len()).sum::<usize>());
        let mut out_runs = Vec::new();
        let mut chips = Vec::new();
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
        for h in hints.iter() {
            let byte = h.byte.min(text.len());
            take(byte - at, &mut out_runs);
            out_text.push_str(&text[at..byte]);
            if !h.ghost {
                chips.push(out_text.len()..out_text.len() + h.shown.len());
            }
            out_text.push_str(&h.shown);
            out_runs.push(hint_run(h.shown.len(), h.ghost));
            at = byte;
        }
        take(text.len() - at, &mut out_runs);
        out_text.push_str(&text[at..]);
        (out_text, out_runs, chips)
    }

    /// Byte offset in the drawn line for byte `byte` of the text.
    pub(super) fn display_byte(&self, line: usize, text: &str, byte: usize) -> usize {
        display_byte(&self.hints_on(line, text), byte)
    }

    /// Text byte for a byte of the drawn line.
    pub(super) fn text_byte(&self, line: usize, text: &str, shown: usize) -> usize {
        text_byte(&self.hints_on(line, text), text.len(), shown)
    }

    /// Mouse move: the full text of a shortened hint under `p` becomes a tooltip. True when
    /// the tooltip changed.
    pub(super) fn hover_inlay(&mut self, p: Point<Pixels>) -> bool {
        let tip = (|| {
            let l = self.layout.as_ref()?;
            let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
            let line = self.line_of_row((y / super::line_height()).floor().max(0.0) as usize);
            let text = self.buffer.line(line);
            let hints = self.hints_on(line, &text);
            if hints.iter().all(|h| h.full.is_none()) {
                return None;
            }
            let shown = l.shaped(line)?.index_for_x(p.x - l.text_x)?;
            Some((p, hint_at(&hints, shown)?.full.clone()?))
        })();
        let changed = tip.as_ref().map(|t| &t.1) != self.inlays.tip.as_ref().map(|t| &t.1);
        self.inlays.tip = tip;
        changed
    }

    pub(super) fn render_inlay_tip(&self) -> Option<gpui::AnyElement> {
        use crate::theme::Theme;
        use gpui::{px, IntoElement, ParentElement, Styled};
        let (at, text) = self.inlays.tip.as_ref()?;
        let rel = *at - self.layout.as_ref()?.bounds.origin;
        Some(
            gpui::div()
                .absolute()
                .left(rel.x.max(px(4.)))
                .top(rel.y + px(super::line_height()))
                .max_w(px(640.))
                .px(px(8.))
                .py(px(4.))
                .rounded(px(6.))
                .border_1()
                .border_color(Theme::divider())
                .bg(Theme::panel())
                .shadow_md()
                .text_size(px(12.5))
                .text_color(Theme::text())
                .child(text.clone())
                .into_any_element(),
        )
    }

    /// Lines that have hints: for smokes.
    #[cfg(feature = "smoke")]
    pub fn inlay_line_count(&self) -> usize {
        self.inlays.lines.len()
    }

    /// Hints shown on `line`, as text (shortened ones as drawn): for smokes.
    #[cfg(feature = "smoke")]
    pub fn inlay_labels(&self, line: usize) -> Vec<String> {
        let text = self.buffer.line(line);
        self.hints_on(line, &text).iter().map(|h| h.label().to_string()).collect()
    }
}

/// Whether a type hint says more than the line already does. `rest` is the line from the
/// hint's position on (`= value` for an assignment).
fn useful_type(label: &str, rest: &str) -> bool {
    let ty = label.trim_start_matches(':').trim();
    // `<special-form 'Literal[...]'>` on a type alias says nothing a reader needs.
    if ty.is_empty() || ty == "Any" || ty.contains("Unknown") || ty.contains("@Todo") || ty.starts_with("<special-form") {
        return false;
    }
    let Some(value) = rest.trim_start().strip_prefix('=').filter(|v| !v.starts_with('=')) else { return true };
    let v = value.trim_start();
    let word_end = v.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.')).unwrap_or(v.len());
    let (word, after) = v.split_at(word_end);
    let first = v.chars().next().unwrap_or(' ');
    // Literals: numbers, strings (also with prefixes), lists, dicts, sets, True / False / None.
    let string_prefix =
        !word.is_empty() && word.len() <= 2 && word.chars().all(|c| "rbfuRBFU".contains(c)) && (after.starts_with('"') || after.starts_with('\''));
    if first.is_ascii_digit()
        || (first == '-' && v[1..].starts_with(|c: char| c.is_ascii_digit()))
        || matches!(first, '"' | '\'' | '[' | '{')
        || string_prefix
        || matches!(word, "True" | "False" | "None")
        || (first == '(' && ty.starts_with("tuple["))
    {
        return false;
    }
    // A class or module itself: `Integer = int` is `<class 'int'>`.
    if (ty.starts_with("<class '") || ty.starts_with("<module '")) && !word.is_empty() && after.trim().is_empty() {
        return false;
    }
    // A constructor call: `Foo(...)` or `models.Foo(...)` typed `Foo` / `Foo[...]`.
    if after.starts_with('(') {
        let class = word.rsplit('.').next().unwrap_or(word);
        let base = ty.split('[').next().unwrap_or(ty);
        if class.starts_with(|c: char| c.is_uppercase()) && base.rsplit('.').next() == Some(class) {
            return false;
        }
    }
    true
}

/// A long label cut after the first element of its outer brackets
/// (`: tuple[Literal["bot"], …]`), or at `MAX_HINT` characters.
fn shorten(label: &str) -> String {
    if label.chars().count() <= MAX_HINT {
        return label.to_string();
    }
    let mut depth = 0i32;
    let mut close = ']';
    for (i, c) in label.char_indices() {
        match c {
            '[' | '(' | '{' => {
                if depth == 0 {
                    close = match c {
                        '[' => ']',
                        '(' => ')',
                        _ => '}',
                    };
                }
                depth += 1;
            }
            ']' | ')' | '}' => depth -= 1,
            ',' if depth == 1 => {
                let s = format!("{}, …{close}", &label[..i]);
                if s.chars().count() <= MAX_HINT + 4 {
                    return s;
                }
                break;
            }
            _ => {}
        }
    }
    let cut: String = label.chars().take(MAX_HINT - 1).collect();
    format!("{cut}…")
}

/// The hint drawn at byte `shown` of the drawn line.
fn hint_at(hints: &[Hint], shown: usize) -> Option<&Hint> {
    let mut shift = 0;
    for h in hints {
        let start = h.byte + shift;
        if shown >= start && shown < start + h.shown.len() {
            return Some(h);
        }
        shift += h.shown.len();
    }
    None
}

/// Hints before `byte` shift it; a hint right at it is drawn after the caret.
fn display_byte(hints: &[Hint], byte: usize) -> usize {
    byte + hints.iter().filter(|h| h.byte < byte).map(|h| h.shown.len()).sum::<usize>()
}

/// A point inside a hint lands where the hint stands.
fn text_byte(hints: &[Hint], len: usize, shown: usize) -> usize {
    let mut shift = 0;
    for h in hints {
        if shown <= h.byte + shift {
            break;
        }
        if shown < h.byte + shift + h.shown.len() {
            return h.byte;
        }
        shift += h.shown.len();
    }
    shown.saturating_sub(shift).min(len)
}

#[cfg(test)]
mod tests {
    use super::{display_byte, shorten, text_byte, useful_type, Hint};

    #[test]
    fn offsets() {
        // "x = f(1)" drawn as "x : int  = f( a= 1)" (each hint has a space on both sides).
        let h = vec![Hint::new(1, ": int"), Hint::new(6, "a=")];
        assert_eq!([0, 1, 2, 6, 7, 8].map(|b| display_byte(&h, b)), [0, 1, 9, 13, 18, 19]);
        // Clicks: before the hint, inside it, after it, inside "a=", on "1", at the end.
        assert_eq!([1, 4, 8, 14, 17, 19].map(|d| text_byte(&h, 8, d)), [1, 1, 1, 6, 6, 8]);
    }

    #[test]
    fn useless_types_are_dropped() {
        for (label, rest) in [
            (": Any", " = db.Column(db.Integer)"),
            (": Unknown", " = thing"),
            (": int", " = 1"),
            (": float", " = -2.5"),
            (": str", " = \"bot\""),
            (": str", " = f'{x}'"),
            (": list[int]", " = [1, 2]"),
            (": dict[str, int]", " = {}"),
            (": bool", " = True"),
            (": tuple[Literal[\"bot\"], Literal[\"manager\"]]", " = (\"bot\", \"manager\")"),
            (": Repo", " = Repo()"),
            (": Session", " = orm.Session(engine)"),
            (": <class 'int'>", " = int"),
            (": <special-form 'Literal[\"bot\"]'>", " = Literal[\"bot\"]"),
        ] {
            assert!(!useful_type(label, rest), "{label} {rest}");
        }
        for (label, rest) in [(": int", " = len(x)"), (": Repo", " = make()"), (": int", " == 1"), (": list[str]", " = names.copy()")] {
            assert!(useful_type(label, rest), "{label} {rest}");
        }
    }

    #[test]
    fn long_hints_are_shortened() {
        assert_eq!(shorten(": tuple[Literal[\"bot\"], Literal[\"manager\"], Literal[\"both\"]]"), ": tuple[Literal[\"bot\"], …]");
        assert_eq!(shorten(": int"), ": int");
        assert_eq!(shorten(&format!(": {}", "A".repeat(60))).chars().count(), 40);
    }
}
