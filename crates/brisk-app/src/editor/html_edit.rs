//! Tag editing in HTML, Vue, Svelte, JSX and XML files (brisk_syntax::html): `>` adds the
//! closing tag, `</` completes the open one, editing a tag's name renames its pair, and Tab
//! expands an Emmet abbreviation.

use super::Editor;
use brisk_syntax::html;
use brisk_text::Selection;
use gpui::Context;
use std::ops::Range;

/// Files bigger than this skip the per-key tag scan.
const MAX_SCAN: usize = 1 << 20;

/// A tag name being edited and its pair, in chars, taken before the edit.
pub(super) struct TagPair {
    name: Range<usize>,
    partner: Range<usize>,
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.')
}

impl Editor {
    /// A file with markup tags.
    pub(super) fn html_like(&self) -> bool {
        !self.is_field
            && self
                .path
                .as_ref()
                .and_then(|p| p.extension())
                .and_then(|e| e.to_str())
                .is_some_and(|e| matches!(e.to_ascii_lowercase().as_str(), "html" | "htm" | "xhtml" | "vue" | "svelte" | "jsx" | "tsx" | "xml" | "svg"))
    }

    fn jsx(&self) -> bool {
        self.path.as_ref().and_then(|p| p.extension()).is_some_and(|e| e == "jsx" || e == "tsx")
    }

    /// One plain caret in a markup file of a reasonable size.
    fn tag_editing(&self) -> bool {
        self.html_like() && self.buffer.selections.len() == 1 && self.buffer.primary().is_empty() && self.buffer.len_chars() < MAX_SCAN
    }

    /// Before an edit at the caret: the tag name the caret is in and its pair.
    pub(super) fn tag_pair_before_edit(&self) -> Option<TagPair> {
        if !self.tag_editing() {
            return None;
        }
        let rope = self.buffer.rope();
        let text = rope.to_string();
        let at = rope.char_to_byte(self.buffer.primary().head);
        let (me, other) = html::paired_name(&text, at)?;
        let c = |b: usize| rope.byte_to_char(b);
        Some(TagPair { name: c(me.start)..c(me.end), partner: c(other.start)..c(other.end) })
    }

    /// After the edit: the name around the caret is copied to the pair.
    pub(super) fn mirror_tag_pair(&mut self, pair: Option<TagPair>) {
        let Some(p) = pair else { return };
        if self.buffer.selections.len() != 1 {
            return;
        }
        let rope = self.buffer.rope();
        let caret = self.buffer.primary().head;
        let (mut s, mut e) = (caret, caret);
        while s > 0 && is_name_char(rope.char(s - 1)) {
            s -= 1;
        }
        while e < rope.len_chars() && is_name_char(rope.char(e)) {
            e += 1;
        }
        // The edit stayed in the name (a space or `>` typed ends it: the pair is kept).
        if s == e || s != p.name.start || s > p.name.end {
            return;
        }
        let name = rope.slice(s..e).to_string();
        let delta = (e - s) as isize - p.name.len() as isize;
        let partner = if p.partner.start > p.name.start {
            (p.partner.start as isize + delta) as usize..(p.partner.end as isize + delta) as usize
        } else {
            p.partner.clone()
        };
        if partner.end > rope.len_chars() || rope.slice(partner.clone()) == name.as_str() {
            return;
        }
        self.buffer.replace_ranges(vec![(partner, name)]);
    }

    /// After a char typed: `>` closes the tag, `/` after `<` completes the open one.
    pub(super) fn after_tag_char(&mut self, c: char) {
        if !matches!(c, '>' | '/') || !self.tag_editing() {
            return;
        }
        let rope = self.buffer.rope();
        let text = rope.to_string();
        let caret = self.buffer.primary().head;
        let at = rope.char_to_byte(caret);
        if c == '>' {
            if let Some(close) = html::close_tag_after(&text, at, self.jsx()) {
                // The caret stays between the tags.
                self.buffer.replace_ranges(vec![(caret..caret, close)]);
                self.buffer.selections = vec![Selection::caret(caret)];
            }
        } else if let Some(rest) = html::close_open_tag(&text, at) {
            // `</` with the closing `>` already there (auto-paired): only the name.
            let rest = if text[at..].starts_with('>') { rest.trim_end_matches('>').to_string() } else { rest };
            let n = rest.chars().count();
            self.buffer.replace_ranges(vec![(caret..caret, rest)]);
            let end = if text[at..].starts_with('>') { caret + n + 1 } else { caret + n };
            self.buffer.selections = vec![Selection::caret(end)];
        }
    }

    /// Tab after an Emmet abbreviation: expands it; false when there is none.
    pub(super) fn expand_emmet(&mut self, cx: &mut Context<Self>) -> bool {
        if !self.tag_editing() {
            return false;
        }
        let caret = self.buffer.primary().head;
        let (line, col) = self.buffer.pos_to_line_col(caret);
        let line_text = self.buffer.line(line);
        let prefix: String = line_text.chars().take(col).collect();
        let Some(abbr) = html::emmet_abbr(&prefix) else { return false };
        // In JSX a bare word is code more often than a tag: only abbreviations with an operator.
        let jsx = self.jsx();
        if jsx && !abbr.contains(['>', '+', '*', '.', '#', '[', '{']) {
            return false;
        }
        let indent: String = line_text.chars().take_while(|c| *c == ' ' || *c == '\t').collect();
        let Some((out, caret_byte)) = html::expand(abbr, &indent, &self.buffer.indent_unit.clone(), jsx) else { return false };
        let start = caret - abbr.chars().count();
        let to = start + out[..caret_byte].chars().count();
        self.buffer.replace_ranges(vec![(start..caret, out)]);
        self.buffer.selections = vec![Selection::caret(to)];
        self.ensure_caret_visible();
        cx.notify();
        true
    }
}
