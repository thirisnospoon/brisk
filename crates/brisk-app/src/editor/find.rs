//! Find / Replace in the current file (Ctrl+F / Ctrl+R), PyCharm's search bar above the editor.
//!
//! The query and replacement fields are small editors, so selection, clipboard and undo work in
//! them as anywhere else. Matching is shared with Find in Files (`brisk_search::Query`).

use super::{line_height, Editor};
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, Keystroke, Subscription, Window};
use std::ops::Range;

/// More matches than this are not counted or highlighted ("10000+").
const MAX_MATCHES: usize = 10_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindOpts {
    pub query: String,
    pub case_sensitive: bool,
    pub whole_words: bool,
    pub regex: bool,
    /// Replace keeps the found text's case: FOO → BAR, Foo → Bar, foo → bar.
    pub preserve_case: bool,
    /// Search only inside [`FindBar::scope`].
    pub in_selection: bool,
}

pub struct FindBar {
    pub query: Entity<Editor>,
    pub replace: Option<Entity<Editor>>,
    pub opts: FindOpts,
    /// Char ranges in the editor's buffer, in order.
    pub matches: Vec<Range<usize>>,
    pub current: Option<usize>,
    pub error: Option<String>,
    /// Buffer version and options the matches were computed for.
    searched: Option<(u64, FindOpts)>,
    /// Where the caret was when the bar opened: typing searches forward from here.
    origin: usize,
    /// "In Selection": the searched range and the buffer version it is in.
    scope: Option<(u64, Range<usize>)>,
    _subs: Vec<Subscription>,
}

fn field(text: &str, window: &mut Window, cx: &mut Context<Editor>) -> Entity<Editor> {
    let _ = window;
    cx.new(|cx| {
        let mut e = Editor::scratch(text, cx);
        e.buffer.auto_pairs = false;
        e.is_field = true;
        e.buffer.select_all();
        e
    })
}

impl Editor {
    /// The query or replacement field has the keyboard.
    pub fn find_field_focused(&self, window: &Window, cx: &gpui::App) -> Option<Entity<Editor>> {
        let f = self.find.as_ref()?;
        std::iter::once(&f.query).chain(f.replace.as_ref()).find(|e| e.read(cx).focus.is_focused(window)).cloned()
    }

    /// Ctrl+F (`replace` false) / Ctrl+R: open the bar, prefilled with a one-line selection.
    pub fn open_find(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let seed = self.search_seed();
        // A selection over several lines: search in it (PyCharm turns "In Selection" on).
        let sel = self.buffer.primary().range();
        let multiline = !sel.is_empty() && self.buffer.pos_to_line_col(sel.start).0 != self.buffer.pos_to_line_col(sel.end).0;
        let scope = multiline.then(|| (self.buffer.version(), sel.clone()));
        if self.find.is_none() {
            let mut opts = self.last_find.clone();
            if let Some(s) = &seed {
                opts.query = s.clone();
            }
            opts.in_selection = multiline;
            let query = field(&opts.query, window, cx);
            let subs = vec![cx.observe(&query, |this, _, cx| this.find_query_changed(cx))];
            let origin = self.buffer.selections.first().map(|s| s.range().start).unwrap_or(0);
            self.find = Some(FindBar { query, replace: None, opts, matches: Vec::new(), current: None, error: None, searched: None, origin, scope, _subs: subs });
        } else if multiline {
            let f = self.find.as_mut().unwrap();
            f.opts.in_selection = true;
            f.scope = scope;
            f.searched = None;
        } else if let Some(s) = seed {
            let f = self.find.as_ref().unwrap();
            f.query.update(cx, |q, cx| {
                q.buffer.set_text(&s);
                q.buffer.select_all();
                cx.notify();
            });
        }
        let f = self.find.as_mut().unwrap();
        if replace && f.replace.is_none() {
            let r = field(&self.last_replace, window, cx);
            let f = self.find.as_mut().unwrap();
            f._subs.push(cx.observe(&r, |this, r, cx| {
                this.last_replace = r.read(cx).buffer.text();
                cx.notify();
            }));
            f.replace = Some(r);
        }
        let f = self.find.as_ref().unwrap();
        // Ctrl+F focuses the query (all selected); Ctrl+R in an open bar goes to the replacement.
        let target = if replace && f.query.read(cx).focus.is_focused(window) { f.replace.clone().unwrap() } else { f.query.clone() };
        target.update(cx, |e, _| e.buffer.select_all());
        window.focus(&target.read(cx).focus);
        self.refresh_find(true, cx);
        cx.notify();
    }

    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(f) = self.find.take() {
            self.last_find = f.opts;
        }
        window.focus(&self.focus);
        cx.notify();
    }

    fn find_query_changed(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.find.as_mut() else { return };
        let q = f.query.read(cx).buffer.text();
        if q != f.opts.query {
            f.opts.query = q;
            self.refresh_find(true, cx);
            cx.notify();
        }
    }

    /// Recompute matches when the text or the options changed. With `jump`, select the first
    /// match after the bar's starting point (incremental search).
    pub fn refresh_find(&mut self, mut jump: bool, cx: &mut Context<Self>) {
        let version = self.buffer.version();
        let Some(f) = self.find.as_mut() else { return };
        // The query field may have changed since the last look (typing, paste, undo).
        let typed = f.query.read(cx).buffer.text();
        if typed != f.opts.query {
            f.opts.query = typed;
            jump = true;
        }
        // The "In Selection" range follows edits (replacements inside it move its end).
        if let Some((v, r)) = f.scope.clone() {
            if v != version {
                f.scope = self.buffer.changes_since(v).map(|ch| (version, brisk_text::map_through(r.start, &ch)..brisk_text::map_through(r.end, &ch)));
                if f.scope.is_none() {
                    // Too many edits to follow the range: drop "In Selection" (the toggle shows
                    // it) rather than quietly searching, and replacing in, the whole file.
                    f.opts.in_selection = false;
                }
            }
        }
        let f = self.find.as_ref().unwrap();
        if !jump && f.searched.as_ref().is_some_and(|(v, o)| *v == version && *o == f.opts) {
            return;
        }
        let scope = f.scope.as_ref().filter(|_| f.opts.in_selection).map(|(_, r)| r.clone());
        let opts = f.opts.clone();
        let text = self.text().to_string();
        let (matches, error) = if opts.query.is_empty() {
            (Vec::new(), None)
        } else {
            let o = brisk_search::Options { query: opts.query.clone(), case_sensitive: opts.case_sensitive, whole_words: opts.whole_words, regex: opts.regex, file_mask: String::new() };
            match brisk_search::Query::new(&o) {
                Ok(q) => {
                    let rope = self.buffer.rope();
                    let all = q.ranges_in(&text, byte_scope(rope, scope.as_ref()), MAX_MATCHES);
                    (all.into_iter().map(|r| rope.byte_to_char(r.start)..rope.byte_to_char(r.end)).collect(), None)
                }
                Err(e) => (Vec::new(), Some(e.trim_start_matches("error: ").to_string())),
            }
        };
        let f = self.find.as_mut().unwrap();
        f.searched = Some((version, opts));
        f.error = error;
        f.matches = matches;
        if jump {
            let origin = f.origin;
            f.current = f.matches.iter().position(|m| m.start >= origin).or(if f.matches.is_empty() { None } else { Some(0) });
            if let Some(i) = f.current {
                let m = f.matches[i].clone();
                self.select_match(m);
            }
        } else {
            // Keep "current" on the match that is selected, if any.
            let sel = self.buffer.primary().range();
            f.current = f.matches.iter().position(|m| *m == sel);
        }
        let _ = cx;
    }

    fn select_match(&mut self, m: Range<usize>) {
        self.buffer.selections = vec![brisk_text::Selection { anchor: m.start, head: m.end, goal_col: None }];
        let vh = self.layout.as_ref().map(|l| f32::from(l.bounds.size.height)).unwrap_or(600.0);
        self.refresh_wrap();
        let y = self.caret_row() as f32 * line_height();
        if y < self.scroll_y || y + line_height() > self.scroll_y + vh {
            self.center_caret();
        }
        self.ensure_caret_visible_x();
    }

    /// F3 / Shift+F3 / Enter / Shift+Enter. Without an open bar, repeats the last search.
    pub fn find_next(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_none() {
            if self.last_find.query.is_empty() {
                return self.open_find(false, window, cx);
            }
            // Search without showing the bar, like PyCharm's F3 after Esc.
            let opts = self.last_find.clone();
            let query = field(&opts.query, window, cx);
            self.find = Some(FindBar { query, replace: None, opts, matches: Vec::new(), current: None, error: None, searched: None, origin: 0, scope: None, _subs: Vec::new() });
            self.refresh_find(false, cx);
            self.step_match(forward);
            self.last_find = self.find.take().unwrap().opts;
            cx.notify();
            return;
        }
        self.refresh_find(false, cx);
        self.step_match(forward);
        cx.notify();
    }

    fn step_match(&mut self, forward: bool) {
        let Some(f) = self.find.as_mut() else { return };
        if f.matches.is_empty() {
            return;
        }
        let n = f.matches.len();
        let sel = self.buffer.primary().range();
        let i = match f.current.filter(|&i| f.matches[i] == sel) {
            Some(i) => if forward { (i + 1) % n } else { (i + n - 1) % n },
            None => {
                let at = if forward { sel.end } else { sel.start };
                if forward {
                    f.matches.iter().position(|m| m.start >= at).unwrap_or(0)
                } else {
                    f.matches.iter().rposition(|m| m.end <= at).unwrap_or(n - 1)
                }
            }
        };
        f.current = Some(i);
        let m = f.matches[i].clone();
        self.select_match(m);
    }

    /// Replace the selected match and go to the next one.
    pub fn replace_current(&mut self, cx: &mut Context<Self>) {
        self.refresh_find(false, cx);
        let Some(f) = self.find.as_ref() else { return };
        let sel = self.buffer.primary().range();
        let Some(i) = f.matches.iter().position(|m| *m == sel) else {
            self.step_match(true);
            cx.notify();
            return;
        };
        let start = f.matches[i].start;
        let with = self.replacement_for(i, cx);
        self.buffer.replace_ranges(vec![(sel, with.clone())]);
        let after = start + with.chars().count();
        self.buffer.set_caret(after, false);
        self.refresh_find(false, cx);
        if let Some(f) = self.find.as_mut() {
            f.origin = after;
        }
        self.step_match(true);
        cx.notify();
    }

    /// Replace every match in scope as one undoable step: all of them, not only the
    /// [`MAX_MATCHES`] that are highlighted. The query is compiled and the text copied once.
    pub fn replace_all(&mut self, cx: &mut Context<Self>) -> usize {
        self.refresh_find(false, cx);
        let Some(f) = self.find.as_ref() else { return 0 };
        if f.opts.query.is_empty() {
            return 0;
        }
        let with = f.replace.as_ref().map(|r| r.read(cx).buffer.text()).unwrap_or_default();
        let opts = f.opts.clone();
        let scope = f.scope.as_ref().filter(|_| opts.in_selection).map(|(_, r)| r.clone());
        let o = brisk_search::Options { query: opts.query.clone(), case_sensitive: opts.case_sensitive, whole_words: opts.whole_words, regex: opts.regex, file_mask: String::new() };
        let Ok(q) = brisk_search::Query::new(&o) else { return 0 };
        let text = self.text().to_string();
        let rope = self.buffer.rope();
        let edits: Vec<(Range<usize>, String)> = q
            .ranges_in(&text, byte_scope(rope, scope.as_ref()), usize::MAX)
            .into_iter()
            .map(|r| {
                let w = q.expand(&text, r.clone(), &with);
                let w = if opts.preserve_case { preserve_case(&text[r.clone()], &w) } else { w };
                (rope.byte_to_char(r.start)..rope.byte_to_char(r.end), w)
            })
            .collect();
        let n = edits.len();
        if !edits.is_empty() {
            self.buffer.replace_ranges(edits);
            self.refresh_find(false, cx);
        }
        cx.notify();
        n
    }

    fn replacement_for(&mut self, i: usize, cx: &mut Context<Self>) -> String {
        let with = self.find.as_ref().and_then(|f| f.replace.as_ref()).map(|r| r.read(cx).buffer.text()).unwrap_or_default();
        let f = self.find.as_ref().unwrap();
        let opts = f.opts.clone();
        let m = f.matches[i].clone();
        let found = self.buffer.slice(m.clone());
        let keep_case = |w: String| if opts.preserve_case { preserve_case(&found, &w) } else { w };
        if !opts.regex {
            return keep_case(with);
        }
        let o = brisk_search::Options { query: opts.query.clone(), case_sensitive: opts.case_sensitive, whole_words: opts.whole_words, regex: true, file_mask: String::new() };
        let Ok(q) = brisk_search::Query::new(&o) else { return keep_case(with) };
        let text = self.text().to_string();
        let rope = self.buffer.rope();
        keep_case(q.expand(&text, rope.char_to_byte(m.start)..rope.char_to_byte(m.end), &with))
    }

    /// What Replace would put in place of the current match (regex groups expanded, case kept),
    /// when that differs from the typed replacement: shown next to the field, like PyCharm.
    fn replace_preview(&mut self, cx: &mut Context<Self>) -> Option<String> {
        let f = self.find.as_ref()?;
        let typed = f.replace.as_ref()?.read(cx).buffer.text();
        if !(f.opts.regex || f.opts.preserve_case) {
            return None;
        }
        let i = f.current?;
        let out = self.replacement_for(i, cx);
        (out != typed).then_some(out)
    }

    pub fn toggle_find_opt(&mut self, which: char, cx: &mut Context<Self>) {
        let Some(f) = self.find.as_mut() else { return };
        match which {
            'c' => f.opts.case_sensitive = !f.opts.case_sensitive,
            'w' => f.opts.whole_words = !f.opts.whole_words,
            'x' => f.opts.regex = !f.opts.regex,
            'p' => f.opts.preserve_case = !f.opts.preserve_case,
            'e' => {
                f.opts.in_selection = !f.opts.in_selection;
                // Turning it on takes the current selection (a selected match is not a scope).
                let sel = self.buffer.primary().range();
                if f.opts.in_selection && !sel.is_empty() && !f.matches.contains(&sel) {
                    f.scope = Some((self.buffer.version(), sel));
                }
                if f.opts.in_selection && f.scope.is_none() {
                    f.opts.in_selection = false;
                }
                f.searched = None;
            }
            _ => return,
        }
        self.refresh_find(true, cx);
        cx.notify();
    }

    /// Keys typed in the query / replacement field that the bar handles. False: let the field
    /// edit as usual.
    pub fn find_key(&mut self, k: &Keystroke, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let m = &k.modifiers;
        let Some(f) = self.find.as_ref() else { return false };
        let in_replace = f.replace.as_ref().is_some_and(|r| r.read(cx).focus.is_focused(window));
        match (k.key.as_str(), m.control, m.shift, m.alt) {
            ("escape", false, false, false) => self.close_find(window, cx),
            ("enter", false, false, false) if in_replace => self.replace_current(cx),
            ("enter", true, false, true) if in_replace => {
                self.replace_all(cx);
            }
            ("enter", false, sh, false) => self.find_next(!sh, window, cx),
            ("f3", false, sh, false) => self.find_next(!sh, window, cx),
            ("down", false, false, false) if !in_replace => self.find_next(true, window, cx),
            ("up", false, false, false) if !in_replace => self.find_next(false, window, cx),
            ("tab", false, _, false) => {
                let f = self.find.as_ref().unwrap();
                let to = if in_replace { Some(f.query.clone()) } else { f.replace.clone() };
                if let Some(to) = to {
                    to.update(cx, |e, _| e.buffer.select_all());
                    window.focus(&to.read(cx).focus);
                }
            }
            ("f", true, false, false) => self.open_find(false, window, cx),
            ("r", true, false, false) => self.open_find(true, window, cx),
            ("c" | "w" | "x" | "p" | "e", false, false, true) => self.toggle_find_opt(k.key.chars().next().unwrap(), cx),
            _ => return false,
        }
        cx.notify();
        true
    }

    /// "3/17", "0 results", "10000+".
    fn find_count(&self) -> String {
        let Some(f) = &self.find else { return String::new() };
        if f.opts.query.is_empty() {
            return String::new();
        }
        let n = f.matches.len();
        let total = if n >= MAX_MATCHES { format!("{n}+") } else { n.to_string() };
        match (n, f.current) {
            (0, _) => "0 results".into(),
            (_, Some(i)) => format!("{}/{total}", i + 1),
            _ => format!("{total} results"),
        }
    }

    pub(super) fn render_find_bar(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        self.refresh_find(false, cx);
        let f = self.find.as_ref()?;
        let none = !f.opts.query.is_empty() && f.matches.is_empty();
        let count = self.find_count();
        let input = |e: Entity<Editor>, bad: bool| {
            let h = crate::ui_kit::COMPACT_H;
            crate::ui_kit::input_frame(false, bad, h).w(px(320.)).child(crate::ui_kit::editor_slot(&e, "", false, h - 2.))
        };
        let toggle = |id: &'static str, label: &'static str, on: bool, tip: char, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(px(6.))
                .h(px(22.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_size(px(12.))
                .when(on, |d| d.bg(Theme::selection_row()).text_color(Theme::text()))
                .when(!on, |d| d.text_color(Theme::text_muted()).hover(|s| s.bg(Theme::hover_row())))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_find_opt(tip, cx)))
                .child(label)
        };
        let button = |id: &'static str, label: &'static str, cx: &mut Context<Self>, act: fn(&mut Editor, &mut Window, &mut Context<Editor>)| {
            div()
                .id(id)
                .px(px(8.))
                .h(px(24.))
                .flex()
                .items_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(Theme::text())
                .hover(|s| s.bg(Theme::hover_row()))
                .on_click(cx.listener(move |this, _, window, cx| act(this, window, cx)))
                .child(crate::icons::glyph(label, 14., Theme::text()))
        };
        let row1 = div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(div().w(px(16.)).flex().items_center().child(crate::icons::icon("search", 14., Theme::text_muted())))
            .child(input(f.query.clone(), none || f.error.is_some()))
            .child(toggle("find-case", "Cc", f.opts.case_sensitive, 'c', cx))
            .child(toggle("find-words", "W", f.opts.whole_words, 'w', cx))
            .child(toggle("find-regex", ".*", f.opts.regex, 'x', cx))
            .child(toggle("find-in-sel", "In selection", f.opts.in_selection, 'e', cx))
            .child(
                div()
                    .min_w(px(80.))
                    .px(px(6.))
                    .text_size(px(12.))
                    .text_color(if none || f.error.is_some() { Theme::error() } else { Theme::text_muted() })
                    .child(f.error.clone().unwrap_or(count)),
            )
            .child(button("find-prev", "↑", cx, |e, w, cx| e.find_next(false, w, cx)))
            .child(button("find-next", "↓", cx, |e, w, cx| e.find_next(true, w, cx)))
            .child(div().flex_1())
            .child(button("find-close", "✕", cx, |e, w, cx| e.close_find(w, cx)));
        let preserve = f.opts.preserve_case;
        let has_replace = f.replace.is_some();
        let preview = if has_replace { self.replace_preview(cx) } else { None };
        let f = self.find.as_ref()?;
        let row2 = f.replace.clone().map(|r| {
            div()
                .flex()
                .items_center()
                .gap(px(4.))
                .child(div().w(px(16.)))
                .child(input(r, false))
                .child(toggle("find-preserve", "Preserve case", preserve, 'p', cx))
                .children(preview.map(|p| div().max_w(px(220.)).overflow_hidden().text_ellipsis().whitespace_nowrap().text_size(px(12.)).text_color(Theme::text_muted()).child(format!("→ {p}"))))
                .child(button("find-replace", "Replace", cx, |e, _, cx| e.replace_current(cx)))
                .child(button("find-replace-all", "Replace All", cx, |e, _, cx| {
                    e.replace_all(cx);
                }))
        });
        Some(
            div()
                .id("find-bar")
                .flex()
                .flex_col()
                .gap(px(4.))
                .px(px(8.))
                .py(px(5.))
                .bg(Theme::panel())
                .border_b_1()
                .border_color(Theme::divider())
                .text_size(px(13.))
                .text_color(Theme::text())
                .cursor(gpui::CursorStyle::Arrow)
                .child(row1)
                .children(row2)
                .into_any_element(),
        )
    }

    /// Matches on lines `first..last` for painting: (char range, is current).
    pub(super) fn visible_matches(&self, lo: usize, hi: usize) -> Vec<(Range<usize>, bool)> {
        let Some(f) = &self.find else { return Vec::new() };
        let start = f.matches.partition_point(|m| m.end < lo);
        f.matches[start..].iter().enumerate().take_while(|(_, m)| m.start <= hi).map(|(j, m)| (m.clone(), f.current == Some(start + j))).collect()
    }
}


/// `with` in the case of `found` (Replace with "Preserve case"): all upper, capitalized or all
/// lower; mixed case leaves the replacement as typed.
/// The char range `scope` as bytes of the rope's text; the whole text without one.
fn byte_scope(rope: &brisk_text::TextRope, scope: Option<&Range<usize>>) -> Range<usize> {
    let len = rope.len_chars();
    scope.map_or(0..rope.len_bytes(), |s| rope.char_to_byte(s.start.min(len))..rope.char_to_byte(s.end.min(len)))
}

pub fn preserve_case(found: &str, with: &str) -> String {
    let letters: Vec<char> = found.chars().filter(|c| c.is_alphabetic()).collect();
    if letters.is_empty() {
        return with.to_string();
    }
    let upper = |c: &char| c.is_uppercase();
    let lower = |c: &char| c.is_lowercase();
    if letters.len() > 1 && letters.iter().all(upper) {
        return with.to_uppercase();
    }
    if letters.iter().all(lower) {
        return with.to_lowercase();
    }
    if upper(&letters[0]) && letters[1..].iter().all(lower) {
        let mut cs = with.chars();
        return match cs.next() {
            Some(c) => c.to_uppercase().chain(cs.flat_map(|c| c.to_lowercase())).collect(),
            None => String::new(),
        };
    }
    with.to_string()
}

#[cfg(test)]
mod tests {
    use super::preserve_case;

    #[test]
    fn keeps_case() {
        assert_eq!(preserve_case("FOO", "bar"), "BAR");
        assert_eq!(preserve_case("Foo", "bar"), "Bar");
        assert_eq!(preserve_case("Foo", "BAR"), "Bar");
        assert_eq!(preserve_case("foo", "Bar"), "bar");
        assert_eq!(preserve_case("fooBar", "bazQux"), "bazQux");
        assert_eq!(preserve_case("F", "bar"), "Bar");
        assert_eq!(preserve_case("42", "x"), "x");
    }
}

impl Editor {
    /// Smoke checks: (match count, current index, query, bar open).
    pub fn find_state(&self) -> (usize, Option<usize>, String, bool) {
        match &self.find {
            Some(f) => (f.matches.len(), f.current, format!("{}{}", f.opts.query, f.error.as_ref().map(|e| format!(" [error: {e}]")).unwrap_or_default()), true),
            None => (0, None, self.last_find.query.clone(), false),
        }
    }
}
