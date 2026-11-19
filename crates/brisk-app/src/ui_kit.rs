//! Shared controls: one look for text inputs and buttons (height, padding, radius, border,
//! placeholder position), so toolbars and dialogs line up.

use crate::theme::ui_px;
use gpui::{div, px, prelude::*, Div, Entity, Stateful, Styled};

use crate::editor::Editor;
use crate::theme::{ui_font, Theme};

/// Inputs and buttons in dialogs and tool window toolbars.
pub const CONTROL_H: f32 = 28.;
/// Inputs in thin bars (the editor's Find bar, the Git log filter).
pub const COMPACT_H: f32 = 26.;
pub const RADIUS: f32 = 5.;
/// Text size of one-line fields (typed text and placeholder), in the UI font.
pub const FIELD_TEXT: f32 = 13.;

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
                    .font_family(ui_font())
                    .text_size(ui_px(FIELD_TEXT))
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

/// Islands: the corner radius of a panel and the gap between panels.
pub const ISLAND_R: f32 = 8.;
pub const ISLAND_GAP: f32 = 6.;

/// Islands: makes `d` a rounded panel. GPUI clips children to a rectangle, so caps in the
/// frame color over the four corners round whatever the panel's content paints there; call
/// it after the content children (the caps must paint last).
pub fn island(d: Div) -> Div {
    let cap = |name: &'static str| crate::icons::icon(name, ISLAND_R, Theme::frame());
    d.relative()
        .rounded(px(ISLAND_R))
        .overflow_hidden()
        .child(div().absolute().top_0().left_0().child(cap("island-tl")))
        .child(div().absolute().top_0().right_0().child(cap("island-tr")))
        .child(div().absolute().bottom_0().left_0().child(cap("island-bl")))
        .child(div().absolute().bottom_0().right_0().child(cap("island-br")))
}

/// Height of a tool window's header row (Islands: 32, Classic: 34).
pub fn header_h() -> f32 {
    (if crate::theme::islands() { 32. } else { 34. }) * crate::theme::ui_scale()
}

/// A tool window's header row: one height, padding and text size in every tool window.
pub fn tool_header() -> Div {
    div().h(px(header_h())).flex_none().flex().items_center().gap(px(4.)).pl(px(12.)).pr(px(6.)).text_size(ui_px(13.))
}

/// A tool window's title in its header (13 px semibold).
pub fn tool_title(t: impl Into<gpui::SharedString>) -> Div {
    div().flex_none().pr(px(8.)).text_size(ui_px(13.)).font_weight(gpui::FontWeight::SEMIBOLD).text_color(Theme::text()).whitespace_nowrap().child(t.into())
}

/// An action in a tool window's header: a 24 px square, radius 4, hover and pressed states.
pub fn header_action(id: impl Into<gpui::ElementId>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .size(ui_px(24.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .cursor_pointer()
        .text_color(Theme::text_muted())
        .hover(|s| s.bg(Theme::hover_row()).text_color(Theme::text()))
        .active(|s| s.bg(Theme::press()))
}

/// An empty tool window or editor: the message centered; in Islands with a muted icon above.
pub fn empty_state(icon: &'static str, text: impl Into<gpui::SharedString>) -> Div {
    div()
        .flex_1()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .px(px(16.))
        .text_size(ui_px(13.))
        .text_color(Theme::text_muted())
        .when(crate::theme::islands(), |d| d.child(crate::icons::ui_icon(icon, 32., Theme::text_dim().opacity(0.7))))
        .child(div().max_w(px(460.)).text_center().child(text.into()))
}

/// Islands: a pill that just became active fades its highlight in (140 ms, ease-out). The
/// animation is keyed by `id`, which is only rendered while the pill is active, so it plays
/// on every switch and costs nothing afterwards.
pub fn pill_in<E: IntoElement + Styled + 'static>(e: E, id: impl Into<gpui::ElementId>, active: bool, color: gpui::Hsla) -> gpui::AnyElement {
    use gpui::{Animation, AnimationExt};
    if !(active && crate::theme::islands()) {
        return e.into_any_element();
    }
    e.with_animation(id.into(), Animation::new(std::time::Duration::from_millis(140)).with_easing(gpui::ease_out_quint()), move |d, t| {
        d.bg(color.opacity(color.a * t))
    })
    .into_any_element()
}

/// Popups, menus, dialogs and notices: in Islands, radius 10 and a soft two-layer shadow;
/// in Classic, the usual large shadow (the element keeps its own radius).
pub fn floating<E: Styled>(e: E) -> E {
    if !crate::theme::islands() {
        return e.shadow_lg();
    }
    let a = if crate::settings::is_light() { 0.5 } else { 1. };
    let layer = |alpha: f32, y: f32, blur: f32| gpui::BoxShadow {
        color: gpui::hsla(0., 0., 0., alpha * a),
        offset: gpui::point(px(0.), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.),
    };
    e.rounded(px(10.)).shadow(vec![layer(0.32, 12., 32.), layer(0.22, 2., 6.)])
}

/// A text button of the same height as the inputs; `primary` is filled with the accent.
pub fn button(id: &'static str, label: &'static str, enabled: bool, primary: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .h(ui_px(CONTROL_H))
        .px(px(12.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .border_1()
        .text_size(ui_px(12.5))
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
        .size(ui_px(CONTROL_H))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(RADIUS))
        .border_1()
        .border_color(Theme::divider())
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.bg(Theme::hover_row())).active(|s| s.bg(Theme::press())))
        .child(crate::icons::ui_icon(icon, 16., if enabled { Theme::text_muted() } else { Theme::text_dim().opacity(0.5) }))
}
