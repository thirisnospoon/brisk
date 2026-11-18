//! Shared controls: one look for text inputs and buttons (height, padding, radius, border,
//! placeholder position), so toolbars and dialogs line up.

use gpui::{div, px, prelude::*, Div, Entity, Stateful};

use crate::editor::Editor;
use crate::theme::{mono_font, Theme};

/// Inputs and buttons in dialogs and tool window toolbars.
pub const CONTROL_H: f32 = 28.;
/// Inputs in thin bars (the editor's Find bar, the Git log filter).
pub const COMPACT_H: f32 = 26.;
pub const RADIUS: f32 = 5.;

/// The frame every text input shares: border (accent when focused, red when the value is
/// wrong), background, radius and a vertically centered row.
pub fn input_frame(focused: bool, bad: bool, h: f32) -> Div {
    div()
        .relative()
        .flex_shrink_0()
        .h(px(h))
        .flex()
        .items_center()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(if bad { Theme::error() } else if focused { Theme::accent() } else { Theme::divider() })
        .bg(Theme::bg())
        .overflow_hidden()
}

/// A one-line editor inside a box of height `h` (its parent's inner height): the editor's
/// line is centered, and the placeholder sits exactly where typed text goes (same font,
/// size and left offset as the editor), so nothing shifts when typing starts.
pub fn editor_slot(e: &Entity<Editor>, placeholder: &'static str, show_placeholder: bool, h: f32) -> Div {
    let lh = crate::editor::line_height();
    let top = ((h - lh) / 2.).floor().max(0.);
    div()
        .relative()
        .flex_1()
        .min_w_0()
        .h_full()
        .child(div().absolute().left_0().right_0().top(px(top)).h(px(lh)).child(e.clone()))
        .when(show_placeholder, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(top))
                    .left(px(8.))
                    .h(px(lh))
                    .flex()
                    .items_center()
                    .whitespace_nowrap()
                    .font_family(mono_font())
                    .text_size(px(crate::settings::font_size()))
                    .text_color(Theme::text_dim())
                    .child(placeholder),
            )
        })
}

/// A one-line editor field: `input_frame` around `editor_slot`.
pub fn editor_field(e: &Entity<Editor>, placeholder: &'static str, focused: bool, bad: bool, h: f32, cx: &gpui::App) -> Div {
    let empty = e.read(cx).buffer.len_chars() == 0;
    input_frame(focused, bad, h).child(editor_slot(e, placeholder, empty && !focused, h - 2.))
}

/// A text button of the same height as the inputs; `primary` is filled with the accent.
pub fn button(id: &'static str, label: &'static str, enabled: bool, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .h(px(CONTROL_H))
        .px(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .border_1()
        .text_size(px(12.5))
        .when(primary && enabled, |d| d.bg(Theme::accent()).border_color(Theme::accent()).text_color(gpui::white()))
        .when(!(primary && enabled), |d| d.border_color(Theme::divider()))
        .when(!enabled, |d| d.text_color(Theme::text_dim()))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.opacity(0.9)).active(|s| s.bg(Theme::press())))
        .child(label)
}

/// A square icon button of the same height as the inputs.
pub fn icon_button(id: &'static str, icon: &'static str, enabled: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .size(px(CONTROL_H))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(Theme::divider())
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(Theme::hover_row())).active(|s| s.bg(Theme::press())))
        .child(crate::icons::icon(icon, 16., if enabled { Theme::text_muted() } else { Theme::text_dim().opacity(0.5) }))
}
