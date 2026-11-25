//! A click on a change marker in the gutter, as in PyCharm: a popup under the change with
//! the lines as they are in HEAD (changed words marked) and Previous / Next Change, Rollback
//! Lines, Show Diff, Copy. Esc, a click elsewhere or an edit closes it. Ctrl+Alt+Z with the
//! caret in a change rolls just those lines back.

use super::{line_height, Editor, EditorEvent};
use crate::diff_view::{line_text, tool, Side};
use crate::settings::font_size;
use crate::theme::{mono_font, ui_font, ui_px, Theme};
use gpui::prelude::*;
use gpui::{div, px, ClipboardItem, MouseButton, Pixels, Point};
use std::ops::Range;

impl Editor {
    /// The change whose gutter marker is under `p`.
    pub(super) fn vcs_marker_at(&self, p: Point<Pixels>) -> Option<usize> {
        let l = self.layout.as_ref()?;
        if !self.show_gutter || self.vcs_base.is_none() || p.y < l.bounds.top() {
            return None;
        }
        // The marker bar: 31 px left of the text (see the gutter paint).
        let mx = l.text_left - px(31.);
        if p.x < mx - px(4.) || p.x > mx + px(8.) {
            return None;
        }
        let y = f32::from(p.y - l.bounds.top()) + self.scroll_y;
        let line = self.line_of_row((y / line_height()).floor().max(0.0) as usize);
        self.vcs_changes
            .iter()
            .position(|c| c.lines.contains(&line))
            .or_else(|| self.vcs_changes.iter().position(|c| c.lines.is_empty() && (c.lines.start == line || c.lines.start == line + 1)))
    }

    /// A window point on the marker of `line` (smokes).
    #[cfg(feature = "smoke")]
    pub fn vcs_marker_point(&self, line: usize) -> Option<Point<Pixels>> {
        let l = self.layout.as_ref()?;
        let y = l.bounds.top() + px((self.row_of(line) as f32 + 0.5) * line_height() - self.scroll_y);
        Some(gpui::point(l.text_left - px(30.), y))
    }

    /// Left click: open the popup of the change under the marker.
    pub(crate) fn vcs_marker_click(&mut self, p: Point<Pixels>, cx: &mut gpui::Context<Self>) -> bool {
        match self.vcs_marker_at(p) {
            Some(i) => {
                self.vcs_popup = Some((i, self.buffer.version()));
                cx.notify();
                true
            }
            None => {
                if self.vcs_popup.take().is_some() {
                    cx.notify();
                }
                false
            }
        }
    }

    /// The change the popup shows (smokes).
    pub fn vcs_popup_change(&self) -> Option<usize> {
        self.vcs_popup.filter(|p| p.1 == self.buffer.version()).map(|p| p.0)
    }

    /// Open the popup on change `i` (smokes; the marker click does the same).
    #[cfg(feature = "smoke")]
    pub fn open_vcs_popup(&mut self, i: usize, cx: &mut gpui::Context<Self>) {
        if i < self.vcs_changes.len() {
            self.vcs_popup = Some((i, self.buffer.version()));
            cx.notify();
        }
    }

    #[cfg(feature = "smoke")]
    pub fn vcs_change_count(&self) -> usize {
        self.vcs_changes.len()
    }

    /// The change with the caret in it (a deletion counts on the line below it).
    pub fn vcs_change_at_caret(&self) -> Option<usize> {
        let line = self.caret_line_col().0;
        self.vcs_changes.iter().position(|c| c.lines.contains(&line) || (c.lines.is_empty() && c.lines.start == line))
    }

    /// The base (HEAD) lines of change `i`, each with its line break.
    fn base_text(&self, i: usize) -> Option<String> {
        let c = self.vcs_changes.get(i)?;
        let base = self.vcs_base.as_ref()?;
        Some(base.split_inclusive('\n').skip(c.base.start).take(c.base.len()).collect())
    }

    /// Rollback Lines: put change `i` back as it is in HEAD (one undoable edit).
    pub fn rollback_change(&mut self, i: usize, cx: &mut gpui::Context<Self>) -> bool {
        // A long file is diffed after a pause in typing: roll back against the current text.
        self.vcs_now();
        let Some(c) = self.vcs_changes.get(i).cloned() else { return false };
        let Some(mut old) = self.base_text(i) else { return false };
        let n = self.buffer.len_lines();
        let start = self.buffer.line_start(c.lines.start.min(n));
        let end = if c.lines.end < n { self.buffer.line_start(c.lines.end) } else { self.buffer.len_chars() };
        // At the end of a file without a final newline the replaced text has none either.
        if end == self.buffer.len_chars() && !self.buffer.text().ends_with('\n') && old.ends_with('\n') {
            old.pop();
        } else if start == self.buffer.len_chars() && start > 0 && !old.is_empty() && !self.buffer.text().ends_with('\n') {
            old.insert(0, '\n');
            old.pop();
        }
        self.buffer.replace_ranges(vec![(start..end, old)]);
        self.vcs_popup = None;
        self.vcs_now();
        cx.notify();
        true
    }

    fn move_vcs_popup(&mut self, forward: bool, cx: &mut gpui::Context<Self>) {
        let Some(i) = self.vcs_popup_change() else { return };
        let n = self.vcs_changes.len();
        let j = if forward { (i + 1) % n.max(1) } else { (i + n - 1) % n.max(1) };
        if let Some(c) = self.vcs_changes.get(j) {
            let line = c.lines.start;
            self.go_to_line(line, cx);
            self.vcs_popup = Some((j, self.buffer.version()));
        }
    }

    /// The popup under its change, while the text is as it was when it opened.
    pub(super) fn render_vcs_popup(&mut self, cx: &mut gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let i = self.vcs_popup_change()?;
        let l = self.layout.as_ref()?;
        let c = self.vcs_changes.get(i)?.clone();
        let lh = line_height();
        let y = (self.row_of(c.lines.end.max(c.lines.start)) as f32) * lh - self.scroll_y + 2.;
        let x = f32::from(l.text_left - l.bounds.left()) - 36.;
        let old = self.base_text(i).unwrap_or_default();
        let old_lines: Vec<String> = old.split_inclusive('\n').map(|s| s.trim_end_matches(['\n', '\r']).replace('\t', "    ")).collect();
        let new_lines: Vec<String> = (c.lines.clone()).map(|l| self.buffer.line(l).replace('\t', "    ")).collect();
        // Changed words of the old lines against the new ones.
        let words: Vec<Vec<Range<usize>>> = if !old_lines.is_empty() && !new_lines.is_empty() && old_lines.len() <= 40 {
            let (a, b) = (old_lines.join("\n"), new_lines.join("\n"));
            let (ranges, _) = brisk_git::inline_diff(&a, &b);
            let mut out = Vec::new();
            let mut at = 0;
            for line in &old_lines {
                let end = at + line.len();
                out.push(
                    ranges
                        .iter()
                        .filter_map(|r| {
                            let (s, e) = (r.start.max(at), r.end.min(end));
                            (s < e).then(|| s - at..e - at)
                        })
                        .collect(),
                );
                at = end + 1;
            }
            out
        } else {
            vec![Vec::new(); old_lines.len()]
        };
        // The old lines with the file's syntax colors.
        let side = Side::new(&old, self.syntax.as_ref().map(|s| s.lang()));
        let n = self.vcs_changes.len();
        let kind = match c.kind {
            brisk_git::LineChangeKind::Added => "Added lines",
            brisk_git::LineChangeKind::Deleted => "Deleted lines",
            brisk_git::LineChangeKind::Modified => "Modified lines",
        };
        let old_box = (!old_lines.is_empty()).then(|| {
            div()
                .max_h(px(lh * 12.))
                .overflow_hidden()
                .border_t_1()
                .border_color(Theme::divider())
                .bg(Theme::diff_deleted_bg())
                .py(px(2.))
                .font_family(mono_font())
                .text_size(px(font_size()))
                .children(words.into_iter().enumerate().map(|(k, w)| {
                    let (t, colors) = side.line(Some(k));
                    div().h(px(lh)).px(px(10.)).flex().items_center().whitespace_nowrap().child(line_text(t, colors, &w, Theme::diff_deleted_word_bg()))
                }))
        });
        Some(
            div()
                .absolute()
                .left(px(x.max(0.)))
                .top(px(y))
                .min_w(ui_px(320.))
                .max_w(ui_px(760.))
                .rounded(px(6.))
                .overflow_hidden()
                .border_1()
                .border_color(Theme::divider())
                .bg(Theme::panel())
                .shadow_md()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .h(ui_px(30.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(2.))
                        .font_family(ui_font())
                        .text_size(ui_px(12.))
                        .text_color(Theme::text_muted())
                        .child(tool("vcs-prev", "arrow-up", "Previous Change", false).on_click(cx.listener(|this, _, _, cx| this.move_vcs_popup(false, cx))))
                        .child(tool("vcs-next", "arrow-down", "Next Change", false).on_click(cx.listener(|this, _, _, cx| this.move_vcs_popup(true, cx))))
                        .child(tool("vcs-rollback", "undo-2", "Rollback Lines (Ctrl+Alt+Z)", false).on_click(cx.listener(move |this, _, _, cx| {
                            this.rollback_change(i, cx);
                        })))
                        .child(tool("vcs-diff", "columns-2", "Show Diff", false).on_click(cx.listener(|this, _, _, cx| {
                            this.vcs_popup = None;
                            cx.emit(EditorEvent::ShowVcsDiff);
                            cx.notify();
                        })))
                        .child(tool("vcs-copy", "copy", "Copy the Lines as They Were", false).on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(old.clone()));
                        })))
                        .child(div().flex_1().px(px(8.)).child(format!("{kind} · {} of {n}", i + 1)))
                        .child(tool("vcs-close", "x", "Close (Esc)", false).on_click(cx.listener(|this, _, _, cx| {
                            this.vcs_popup = None;
                            cx.notify();
                        }))),
                )
                .children(old_box)
                .into_any_element(),
        )
    }
}
