//! Side-by-side diff viewer tab (PyCharm's "Show Diff", Ctrl+D). Read-only; virtualized rows.

use crate::theme::{Theme, MONO_FONT};
use brisk_git::{AlignedRow, RowKind};
use gpui::prelude::*;
use gpui::{div, px, uniform_list, App, Context, FocusHandle, Focusable, SharedString, UniformListScrollHandle, Window};
use std::path::PathBuf;

const ROW_H: f32 = 20.0;

pub struct DiffView {
    pub title: String,
    /// The working-tree file, for F4 / "open source".
    pub path: Option<PathBuf>,
    left_label: String,
    right_label: String,
    left: Vec<String>,
    right: Vec<String>,
    rows: Vec<AlignedRow>,
    /// First row of each change block.
    blocks: Vec<usize>,
    current: usize,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
}

impl DiffView {
    pub fn new(title: String, path: Option<PathBuf>, left_label: String, left: &str, right_label: String, right: &str, cx: &mut Context<Self>) -> Self {
        let rows = brisk_git::align(left, right);
        let mut blocks = Vec::new();
        for (i, r) in rows.iter().enumerate() {
            if r.kind != RowKind::Equal && (i == 0 || rows[i - 1].kind == RowKind::Equal) {
                blocks.push(i);
            }
        }
        let split = |s: &str| s.lines().map(|l| l.replace('\t', "    ")).collect::<Vec<_>>();
        let scroll = UniformListScrollHandle::new();
        if let Some(&first) = blocks.first() {
            scroll.scroll_to_item(first.saturating_sub(3), gpui::ScrollStrategy::Top);
        }
        Self { title, path, left_label, right_label, left: split(left), right: split(right), rows, blocks, current: 0, scroll, focus: cx.focus_handle() }
    }

    /// F7 / Shift+F7 in PyCharm's diff: jump between differences.
    pub fn jump(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.blocks.is_empty() {
            return;
        }
        let n = self.blocks.len();
        self.current = if forward { (self.current + 1) % n } else { (self.current + n - 1) % n };
        self.scroll.scroll_to_item(self.blocks[self.current].saturating_sub(3), gpui::ScrollStrategy::Top);
        cx.notify();
    }
}

impl Focusable for DiffView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn side(num: Option<usize>, text: Option<&String>, bg: gpui::Hsla) -> gpui::Div {
    div()
        .flex_1()
        .min_w_0()
        .h(px(ROW_H))
        .flex()
        .items_center()
        .bg(bg)
        .child(
            div()
                .w(px(48.))
                .flex_shrink_0()
                .pr(px(10.))
                .flex()
                .justify_end()
                .text_color(Theme::gutter_fg())
                .child(num.map(|n| (n + 1).to_string()).unwrap_or_default()),
        )
        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(SharedString::from(text.cloned().unwrap_or_default())))
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let n = self.rows.len();
        let diffs = self.blocks.len();
        let btn = |id: &'static str, glyph: &'static str| {
            div().id(id).px(px(6.)).rounded(px(4.)).cursor_pointer().hover(|s| s.bg(Theme::hover_row())).child(glyph)
        };
        div()
            .track_focus(&self.focus)
            .key_context("DiffView")
            .size_full()
            .flex()
            .flex_col()
            .bg(Theme::editor_bg())
            .child(
                div()
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(10.))
                    .bg(Theme::panel())
                    .border_b_1()
                    .border_color(Theme::divider())
                    .text_size(px(12.))
                    .text_color(Theme::text_muted())
                    .child(btn("diff-prev", "↑").on_click(cx.listener(|this, _, _, cx| this.jump(false, cx))))
                    .child(btn("diff-next", "↓").on_click(cx.listener(|this, _, _, cx| this.jump(true, cx))))
                    .child(if diffs == 0 { "Contents are identical".to_string() } else { format!("{diffs} difference{}", if diffs == 1 { "" } else { "s" }) }),
            )
            .child(
                div()
                    .flex()
                    .h(px(24.))
                    .items_center()
                    .text_size(px(12.))
                    .text_color(Theme::text_muted())
                    .border_b_1()
                    .border_color(Theme::divider())
                    .child(div().flex_1().px(px(12.)).child(self.left_label.clone()))
                    .child(div().flex_1().px(px(12.)).child(self.right_label.clone())),
            )
            .child(
                uniform_list(
                    "diff-rows",
                    n,
                    cx.processor(|this, range: std::ops::Range<usize>, _, _| {
                        range
                            .map(|i| {
                                let r = &this.rows[i];
                                let (lbg, rbg) = match r.kind {
                                    RowKind::Equal => (Theme::editor_bg(), Theme::editor_bg()),
                                    RowKind::Added => (Theme::diff_filler_bg(), Theme::diff_added_bg()),
                                    RowKind::Deleted => (Theme::diff_deleted_bg(), Theme::diff_filler_bg()),
                                    RowKind::Modified => (
                                        if r.left.is_some() { Theme::diff_modified_bg() } else { Theme::diff_filler_bg() },
                                        if r.right.is_some() { Theme::diff_modified_bg() } else { Theme::diff_filler_bg() },
                                    ),
                                };
                                div()
                                    .flex()
                                    .w_full()
                                    .child(side(r.left, r.left.and_then(|l| this.left.get(l)), lbg))
                                    .child(div().w(px(1.)).h(px(ROW_H)).bg(Theme::divider()))
                                    .child(side(r.right, r.right.and_then(|l| this.right.get(l)), rbg))
                            })
                            .collect()
                    }),
                )
                .track_scroll(self.scroll.clone())
                .flex_1()
                .font_family(MONO_FONT)
                .text_size(px(13.))
                .text_color(Theme::text()),
            )
    }
}
