//! Code-insight popups drawn over the editor: completion list, Quick Documentation and
//! Parameter Info. The workspace asks ty and hands the results here.

use super::{Editor, EditorEvent, LINE_HEIGHT};
use crate::theme::{Theme, MONO_FONT};
use brisk_lsp::{CompletionItem, DocBlock, Signature};
use gpui::prelude::*;
use gpui::{point, px, Context, HighlightStyle, MouseButton, Pixels, Point, StyledText};
use std::time::Duration;

/// Rows the completion list shows at once.
const VISIBLE: usize = 12;
const ROW_H: f32 = 22.0;
/// Mouse rest before documentation shows, like PyCharm's "Show quick documentation on hover".
const HOVER_DELAY: Duration = Duration::from_millis(500);

pub struct Completion {
    pub items: Vec<CompletionItem>,
    /// Indices into `items` that match the typed prefix.
    pub shown: Vec<usize>,
    pub selected: usize,
    pub top: usize,
    /// Char position where the completed word starts.
    pub start: usize,
}

#[derive(Clone, Copy)]
pub enum Anchor {
    Caret,
    /// Mouse hover: the char position and the mouse point (window coordinates).
    Mouse(usize, Point<Pixels>),
}

pub struct DocPopup {
    pub blocks: Vec<DocBlock>,
    pub anchor: Anchor,
}

#[derive(Default)]
pub struct Insight {
    pub completion: Option<Completion>,
    pub doc: Option<DocPopup>,
    pub signature: Option<Signature>,
    /// Mouse-hover documentation timer generation.
    hover_gen: u64,
    /// Char position of the word the mouse rests on.
    hover_word: Option<usize>,
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// PyCharm-style matching: prefix (any case) first, then camel-hump / subsequence matches.
fn matches(label: &str, prefix: &str) -> Option<u8> {
    if prefix.is_empty() {
        return Some(1);
    }
    if label.starts_with(prefix) {
        return Some(0);
    }
    if label.to_lowercase().starts_with(&prefix.to_lowercase()) {
        return Some(1);
    }
    let mut it = prefix.chars().map(|c| c.to_ascii_lowercase()).peekable();
    let first = label.chars().next().map(|c| c.to_ascii_lowercase());
    if first != prefix.chars().next().map(|c| c.to_ascii_lowercase()) {
        return None;
    }
    for c in label.chars() {
        if it.peek() == Some(&c.to_ascii_lowercase()) {
            it.next();
        }
    }
    it.peek().is_none().then_some(2)
}

/// One-letter kind badge and its color, like PyCharm's completion icons.
fn kind_badge(kind: u32) -> (&'static str, u32) {
    match kind {
        2 | 3 | 4 => ("f", 0xc77dbb),
        5 | 10 => ("p", 0xcf8e6d),
        6 => ("v", 0x6aab73),
        7 | 22 | 25 => ("c", 0x56a8f5),
        9 => ("m", 0x8c8c8c),
        14 => ("k", 0xcf8e6d),
        21 => ("C", 0x6aab73),
        _ => ("·", 0x8c8c8c),
    }
}

impl Editor {
    /// Start of the identifier before the caret.
    pub fn word_start_before_caret(&self) -> usize {
        let caret = self.buffer.primary().head;
        let rope = self.buffer.rope();
        let mut s = caret;
        while s > 0 && is_ident(rope.char(s - 1)) {
            s -= 1;
        }
        s
    }

    fn typed_prefix(&self, start: usize) -> Option<String> {
        let caret = self.buffer.primary().head;
        if caret < start || self.buffer.selections.len() > 1 {
            return None;
        }
        let p: String = self.buffer.rope().slice(start..caret).to_string();
        p.chars().all(is_ident).then_some(p)
    }

    /// Completion results for the word starting at `start`.
    pub fn show_completion(&mut self, items: Vec<CompletionItem>, start: usize, cx: &mut Context<Self>) {
        self.insight.completion = Some(Completion { items, shown: Vec::new(), selected: 0, top: 0, start });
        self.refilter_completion();
        cx.notify();
    }

    pub fn completion_open(&self) -> bool {
        self.insight.completion.is_some()
    }

    /// Re-match the list against what is typed now; closes it when nothing matches.
    pub(super) fn refilter_completion(&mut self) {
        let Some(c) = &self.insight.completion else { return };
        let Some(prefix) = self.typed_prefix(c.start) else {
            self.insight.completion = None;
            return;
        };
        let mut ranked: Vec<(u8, usize)> = c.items.iter().enumerate().filter_map(|(i, it)| matches(&it.label, &prefix).map(|r| (r, i))).collect();
        ranked.sort_by_key(|&(r, i)| (r, i));
        let shown: Vec<usize> = ranked.into_iter().map(|(_, i)| i).collect();
        // A single exact match needs no list.
        let exact_only = shown.len() == 1 && c.items[shown[0]].label == prefix;
        if shown.is_empty() || exact_only {
            self.insight.completion = None;
            return;
        }
        let c = self.insight.completion.as_mut().unwrap();
        c.shown = shown;
        c.selected = 0;
        c.top = 0;
    }

    /// Keys while the completion list is open. Returns true when the list used the key.
    pub fn completion_key(&mut self, key: &str, cx: &mut Context<Self>) -> bool {
        let Some(c) = self.insight.completion.as_mut() else { return false };
        let n = c.shown.len();
        match key {
            "up" => c.selected = if c.selected == 0 { n - 1 } else { c.selected - 1 },
            "down" => c.selected = (c.selected + 1) % n,
            "pageup" => c.selected = c.selected.saturating_sub(VISIBLE),
            "pagedown" => c.selected = (c.selected + VISIBLE).min(n - 1),
            "enter" | "tab" => {
                self.accept_completion(key == "tab", cx);
                return true;
            }
            "escape" => self.insight.completion = None,
            _ => return false,
        }
        if let Some(c) = self.insight.completion.as_mut() {
            if c.selected < c.top {
                c.top = c.selected;
            } else if c.selected >= c.top + VISIBLE {
                c.top = c.selected + 1 - VISIBLE;
            }
        }
        cx.notify();
        true
    }

    /// Insert the selected item. Tab replaces the rest of the identifier too (PyCharm).
    /// Functions get `()` with the caret inside, unless a `(` already follows.
    pub fn accept_completion(&mut self, replace: bool, cx: &mut Context<Self>) {
        let Some(c) = self.insight.completion.take() else { return };
        let Some(item) = c.shown.get(c.selected).map(|&i| c.items[i].clone()) else { return };
        let caret = self.buffer.primary().head;
        let rope = self.buffer.rope();
        let mut end = caret;
        if replace {
            while end < rope.len_chars() && is_ident(rope.char(end)) {
                end += 1;
            }
        }
        let next = (end < rope.len_chars()).then(|| rope.char(end));
        let (line, _) = self.buffer.pos_to_line_col(caret);
        let line_text = self.buffer.line(line);
        let in_import = matches!(line_text.trim_start().split_whitespace().next(), Some("import" | "from"));
        let call = matches!(item.kind, 2 | 3 | 4) && next != Some('(') && !in_import;
        let text = if call { format!("{}()", item.insert) } else { item.insert.clone() };
        self.buffer.replace_ranges(vec![(c.start..end, text)]);
        if call {
            let p = c.start + item.insert.chars().count() + 1;
            self.buffer.set_caret(p, false);
            cx.emit(EditorEvent::Typed('('));
        }
        self.ensure_caret_visible();
        cx.notify();
    }

    /// Smoke tests: type like the keyboard does (auto-pairs, events).
    pub fn type_for_smoke(&mut self, text: &str, cx: &mut Context<Self>) {
        self.insight.doc = None;
        for c in text.chars() {
            if c == '\n' {
                self.buffer.newline();
                self.insight.completion = None;
            } else {
                self.buffer.type_char(c);
                self.refilter_completion();
                cx.emit(EditorEvent::Typed(c));
            }
        }
        cx.notify();
    }

    /// Smoke tests: what the code-insight popups show.
    pub fn insight_summary(&self) -> String {
        let c = self.insight.completion.as_ref().map(|c| c.shown.iter().take(5).map(|&i| c.items[i].label.clone()).collect::<Vec<_>>());
        let d = self.insight.doc.as_ref().map(|d| d.blocks.len());
        let s = self.insight.signature.as_ref().map(|s| (s.label.clone(), s.active));
        format!("completion {c:?}, doc blocks {d:?}, signature {s:?}")
    }

    pub fn signature_open(&self) -> bool {
        self.insight.signature.is_some()
    }

    pub fn show_doc(&mut self, markdown: &str, anchor: Anchor, cx: &mut Context<Self>) {
        if let Anchor::Mouse(p, _) = anchor {
            // The mouse moved on, or a list is in the way.
            if self.insight.hover_word != Some(self.buffer.word_range_at(p).start) || self.insight.completion.is_some() {
                return;
            }
        }
        let blocks = brisk_lsp::doc_blocks(markdown);
        self.insight.doc = (!blocks.is_empty()).then_some(DocPopup { blocks, anchor });
        cx.notify();
    }

    pub fn show_signature(&mut self, sig: Option<Signature>, cx: &mut Context<Self>) {
        self.insight.signature = sig;
        cx.notify();
    }

    /// Escape: close the topmost code-insight popup. Returns false when none was open.
    pub fn close_insight(&mut self) -> bool {
        if self.insight.completion.take().is_some() || self.insight.doc.take().is_some() || self.insight.signature.take().is_some() {
            return true;
        }
        false
    }

    /// Called from mouse move when no problem tooltip is shown: after a rest on a word,
    /// ask the workspace for its documentation.
    pub(super) fn hover_for_doc(&mut self, pos: Option<usize>, at: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.is_python() {
            return;
        }
        let word = pos.filter(|&p| p < self.buffer.len_chars() && is_ident(self.buffer.rope().char(p))).map(|p| self.buffer.word_range_at(p).start);
        if word == self.insight.hover_word {
            return;
        }
        self.insight.hover_word = word;
        self.insight.hover_gen += 1;
        if matches!(self.insight.doc.as_ref().map(|d| d.anchor), Some(Anchor::Mouse(..))) {
            self.insight.doc = None;
            cx.notify();
        }
        let (Some(p), my) = (pos, self.insight.hover_gen) else { return };
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let _ = this.update(cx, |e, cx| {
                if e.insight.hover_gen == my {
                    cx.emit(EditorEvent::HoverDoc(p, at));
                }
            });
        })
        .detach();
    }

    pub(super) fn mouse_left_text(&mut self, cx: &mut Context<Self>) {
        if self.insight.hover_word.take().is_some() {
            self.insight.hover_gen += 1;
            if matches!(self.insight.doc.as_ref().map(|d| d.anchor), Some(Anchor::Mouse(..))) {
                self.insight.doc = None;
                cx.notify();
            }
        }
    }

    /// Window point of a char position (approximate x from the monospace advance).
    fn point_of(&self, pos: usize) -> Option<Point<Pixels>> {
        let l = self.layout.as_ref()?;
        let (line, col) = self.buffer.pos_to_line_col(pos);
        let y = l.bounds.top() + px(line as f32 * LINE_HEIGHT - self.scroll_y);
        Some(point(l.text_x + l.char_w * col as f32, y))
    }

    pub(super) fn render_insight(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        let mut out = Vec::new();
        let Some(l) = self.layout.as_ref() else { return out };
        let origin = l.bounds.origin;
        let caret = self.buffer.primary().head;
        // Parameter info sits above the caret line.
        if let Some(sig) = &self.insight.signature {
            if let Some(p) = self.point_of(caret) {
                let rel = p - origin;
                let mut hl = Vec::new();
                if let Some(r) = sig.active.and_then(|a| sig.params.get(a)) {
                    let b = |c: usize| sig.label.char_indices().nth(c).map(|(b, _)| b).unwrap_or(sig.label.len());
                    hl.push((b(r.start)..b(r.end), HighlightStyle { font_weight: Some(gpui::FontWeight::BOLD), color: Some(Theme::text()), ..Default::default() }));
                }
                out.push(
                    gpui::div()
                        .absolute()
                        .left(rel.x.max(px(4.)) - px(12.))
                        .top((rel.y - px(LINE_HEIGHT + 8.)).max(px(0.)))
                        .px(px(8.))
                        .py(px(3.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(Theme::divider())
                        .bg(Theme::panel())
                        .shadow_md()
                        .font_family(MONO_FONT)
                        .text_size(px(12.5))
                        .text_color(Theme::text_muted())
                        .child(StyledText::new(sig.label.clone()).with_highlights(hl))
                        .into_any_element(),
                );
            }
        }
        if let Some(c) = &self.insight.completion {
            if let Some(p) = self.point_of(c.start) {
                let rel = p - origin + point(px(-22.), px(LINE_HEIGHT + 2.));
                let rows = c.shown.iter().enumerate().skip(c.top).take(VISIBLE).map(|(i, &ix)| {
                    let it = &c.items[ix];
                    let (badge, color) = kind_badge(it.kind);
                    gpui::div()
                        .id(("completion", i))
                        .h(px(ROW_H))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .px(px(6.))
                        .when(i == c.selected, |d| d.bg(Theme::selection_row()))
                        .child(gpui::div().w(px(14.)).text_color(gpui::rgb(color)).child(badge))
                        .child(gpui::div().text_color(Theme::text()).whitespace_nowrap().child(it.label.clone()))
                        .child(gpui::div().flex_1().overflow_hidden().whitespace_nowrap().text_right().text_color(Theme::text_dim()).text_size(px(12.)).child(it.detail.clone()))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |e, ev: &gpui::MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            if let Some(c) = e.insight.completion.as_mut() {
                                c.selected = i;
                            }
                            if ev.click_count >= 2 {
                                e.accept_completion(false, cx);
                            }
                            cx.notify();
                        }))
                });
                out.push(
                    gpui::div()
                        .id("completion-list")
                        .absolute()
                        .left(rel.x.max(px(0.)))
                        .top(rel.y)
                        .w(px(440.))
                        .py(px(2.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(Theme::divider())
                        .bg(Theme::panel())
                        .shadow_lg()
                        .text_size(px(13.))
                        .font_family(MONO_FONT)
                        .cursor(gpui::CursorStyle::Arrow)
                        .children(rows)
                        .into_any_element(),
                );
            }
        }
        if let Some(d) = &self.insight.doc {
            let at = match d.anchor {
                Anchor::Caret => self.point_of(caret),
                Anchor::Mouse(pos, m) => self.point_of(pos).map(|p| point(m.x.min(p.x + px(400.)), p.y)),
            };
            if let Some(p) = at {
                let rel = p - origin + point(px(0.), px(LINE_HEIGHT + 2.));
                let blocks = d.blocks.iter().map(|b| match b {
                    DocBlock::Code(code) => gpui::div()
                        .px(px(10.))
                        .py(px(6.))
                        .font_family(MONO_FONT)
                        .text_size(px(12.5))
                        .text_color(Theme::text())
                        .children(code.lines().map(|l| gpui::div().whitespace_nowrap().child(if l.is_empty() { " ".to_string() } else { l.to_string() })))
                        .into_any_element(),
                    DocBlock::Text(t) => gpui::div()
                        .px(px(10.))
                        .py(px(6.))
                        .text_size(px(13.))
                        .text_color(Theme::text())
                        .children(t.split("\n\n").map(|para| gpui::div().pb(px(4.)).child(para.replace('\n', " "))))
                        .into_any_element(),
                    DocBlock::Rule => gpui::div().h(px(1.)).bg(Theme::divider()).into_any_element(),
                });
                out.push(
                    gpui::div()
                        .id("quick-doc")
                        .absolute()
                        .left(rel.x.max(px(4.)))
                        .top(rel.y)
                        .max_w(px(620.))
                        .max_h(px(360.))
                        .overflow_y_scroll()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(Theme::divider())
                        .bg(Theme::panel())
                        .shadow_lg()
                        .cursor(gpui::CursorStyle::Arrow)
                        .on_mouse_move(|_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                        .children(blocks)
                        .into_any_element(),
                );
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::matches;

    #[test]
    fn completion_matching() {
        assert_eq!(matches("path", "pa"), Some(0));
        assert_eq!(matches("PathLike", "pa"), Some(1));
        assert_eq!(matches("getcwd", "gcw"), Some(2));
        assert_eq!(matches("environ", "gcw"), None);
        assert_eq!(matches("anything", ""), Some(1));
    }
}
