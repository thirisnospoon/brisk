//! Settings dialog (Ctrl+Alt+S): theme, smooth scrolling, the caret, sticky lines, editor font, font
//! size, line spacing, inlay hints, editor tabs, actions on save; font zoom
//! (Alt+Shift+. / Alt+Shift+, / Ctrl+wheel) and the quick theme switch.

use super::Workspace;
use crate::settings::{self, Settings};
use crate::theme::ui_px;
use crate::theme::Theme;
use brisk_keymap::Action;
use gpui::prelude::*;
use gpui::{div, px, Context, Window};

pub struct SettingsDialog {
    /// The settings when the dialog opened (Cancel goes back to them).
    orig: Settings,
    pub draft: Settings,
    /// Installed monospaced fonts we offer.
    fonts: Vec<String>,
    /// The control with the keyboard focus (Tab / Shift+Tab, or the last one clicked), as
    /// an index into `controls()`; it wears the focus ring.
    kb: Option<usize>,
    /// Typing goes to the "Soft-wrap these files" field.
    pub mask_edit: bool,
}

/// A Settings control, in Tab order; steppers carry their step (-1 / +1).
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum SetCtl {
    Theme(bool),
    Style(bool),
    UiSize(i8),
    Smooth,
    Blink,
    Slide,
    Sticky,
    Font(usize),
    Size(i8),
    Spacing(i8),
    Inlay,
    Animations,
    Whitespace,
    TabRows(bool),
    TabLimit(i8),
    WrapFiles,
    WrapIndent,
    WrapExtra(i8),
    CommentCol,
    CommentSpace,
    Reformat,
    Imports,
    Cancel,
    Ok,
}

impl SettingsDialog {
    /// Every control, in the order Tab visits them (top to bottom, left to right).
    fn controls(&self) -> Vec<SetCtl> {
        use SetCtl::*;
        let mut v = vec![Theme(false), Theme(true), Style(false), Style(true), UiSize(-1), UiSize(1), Animations, Smooth, Blink, Slide, Sticky];
        v.extend((0..self.fonts.len()).map(Font));
        v.extend([Size(-1), Size(1), Spacing(-1), Spacing(1), Inlay, Whitespace, TabRows(false), TabRows(true), TabLimit(-1), TabLimit(1)]);
        v.extend([WrapFiles, WrapIndent, WrapExtra(-1), WrapExtra(1), CommentCol, CommentSpace]);
        v.extend([Reformat, Imports, Cancel, Ok]);
        v
    }

    fn focused(&self) -> Option<SetCtl> {
        self.kb.and_then(|i| self.controls().get(i).copied())
    }
}

impl Workspace {
    pub(super) fn settings_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        match action {
            Action::Settings => self.open_settings(window),
            Action::IncreaseFontSize | Action::DecreaseFontSize | Action::ResetFontSize => {
                let size = settings::zoom(match action {
                    Action::IncreaseFontSize => Some(1.0),
                    Action::DecreaseFontSize => Some(-1.0),
                    _ => None,
                });
                self.status = format!("Editor font size: {size}");
                window.refresh();
            }
            Action::ZoomInIde | Action::ZoomOutIde | Action::ResetIdeZoom => {
                let z = settings::zoom_ide(match action {
                    Action::ZoomInIde => Some(0.1),
                    Action::ZoomOutIde => Some(-0.1),
                    _ => None,
                });
                self.status = format!("IDE zoom: {:.0}%", z * 100.);
                window.refresh();
            }
            Action::QuickSwitchTheme => {
                let mut s = settings::current();
                s.light = !s.light;
                settings::apply(&s);
                let _ = settings::save(&s);
                self.status = format!("Theme: {}", if s.light { "Light" } else { "Dark" });
                window.refresh();
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn open_settings(&mut self, window: &mut Window) {
        let s = settings::current();
        let installed = window.text_system().all_font_names();
        let mut fonts: Vec<String> =
            settings::FONT_CHOICES.iter().filter(|f| installed.iter().any(|i| i.eq_ignore_ascii_case(f))).map(|f| f.to_string()).collect();
        if !fonts.iter().any(|f| f.eq_ignore_ascii_case(&s.editor_font)) {
            fonts.insert(0, s.editor_font.clone());
        }
        self.settings_dialog = Some(SettingsDialog { orig: s.clone(), draft: s, fonts, kb: None, mask_edit: false });
    }

    /// Change the draft and show it at once.
    pub(super) fn settings_edit(&mut self, f: impl FnOnce(&mut Settings), window: &mut Window) {
        let Some(d) = self.settings_dialog.as_mut() else { return };
        f(&mut d.draft);
        d.draft.font_size = d.draft.font_size.clamp(settings::MIN_FONT_SIZE, settings::MAX_FONT_SIZE);
        d.draft.ui_font_size = d.draft.ui_font_size.clamp(settings::MIN_UI_FONT_SIZE, settings::MAX_UI_FONT_SIZE);
        d.draft.line_spacing = (d.draft.line_spacing.clamp(0.8, 2.0) * 10.0).round() / 10.0;
        settings::apply(&d.draft);
        window.refresh();
    }

    pub(super) fn settings_close(&mut self, ok: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.settings_dialog.take() else { return };
        if ok {
            settings::apply(&d.draft);
            if let Err(e) = settings::save(&d.draft) {
                self.status = format!("Cannot save settings: {e}");
            }
        } else {
            settings::apply(&d.orig);
        }
        window.refresh();
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn settings_key(&mut self, e: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        match k.key.as_str() {
            "escape" => self.settings_close(false, window, cx),
            // Enter is OK, unless Cancel has the focus.
            "enter" => {
                let cancel = self.settings_dialog.as_ref().is_some_and(|d| d.focused() == Some(SetCtl::Cancel));
                self.settings_close(!cancel, window, cx)
            }
            // The masks field: a plain one-line text box.
            "backspace" if self.settings_dialog.as_ref().is_some_and(|d| d.mask_edit) => {
                self.settings_edit(
                    |s| {
                        s.soft_wrap_files.pop();
                    },
                    window,
                );
            }
            // Tab / Shift+Tab move the focus over the controls, Space presses the focused one.
            "tab" => {
                if let Some(d) = self.settings_dialog.as_mut() {
                    let n = d.controls().len();
                    d.kb = Some(match (d.kb, k.modifiers.shift) {
                        (None, false) => 0,
                        (None, true) => n - 1,
                        (Some(i), false) => (i + 1) % n,
                        (Some(i), true) => (i + n - 1) % n,
                    });
                    d.mask_edit = d.focused() == Some(SetCtl::WrapFiles);
                }
            }
            _ if self.settings_dialog.as_ref().is_some_and(|d| d.mask_edit)
                && !k.modifiers.control
                && !k.modifiers.alt
                && k.key_char.as_ref().is_some_and(|t| t.chars().all(|c| !c.is_control())) =>
            {
                let t = k.key_char.clone().unwrap_or_default();
                self.settings_edit(|s| s.soft_wrap_files.push_str(&t), window);
            }
            "space" => {
                if let Some(c) = self.settings_dialog.as_ref().and_then(|d| d.focused()) {
                    self.settings_press(c, window, cx);
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// Press control `c` (a click, or Space when it has the focus).
    fn settings_press(&mut self, c: SetCtl, window: &mut Window, cx: &mut Context<Self>) {
        // Typing goes to the masks field while it is the one pressed.
        if let Some(d) = self.settings_dialog.as_mut() {
            d.mask_edit = c == SetCtl::WrapFiles;
        }
        match c {
            SetCtl::WrapFiles => {}
            SetCtl::WrapIndent => self.settings_edit(|s| s.soft_wrap_indent = !s.soft_wrap_indent, window),
            SetCtl::WrapExtra(d) => self.settings_edit(|s| s.soft_wrap_extra_indent = (s.soft_wrap_extra_indent as i32 + d as i32).clamp(0, 16) as u32, window),
            SetCtl::CommentCol => self.settings_edit(|s| s.line_comment_first_column = !s.line_comment_first_column, window),
            SetCtl::CommentSpace => self.settings_edit(|s| s.line_comment_space = !s.line_comment_space, window),
            SetCtl::Theme(light) => self.settings_edit(|s| s.light = light, window),
            SetCtl::Style(islands) => self.settings_edit(|s| s.islands = islands, window),
            SetCtl::UiSize(d) => self.settings_edit(|s| s.ui_font_size += d as f32, window),
            SetCtl::Font(i) => {
                let Some(n) = self.settings_dialog.as_ref().and_then(|d| d.fonts.get(i).cloned()) else { return };
                self.settings_edit(|s| s.editor_font = n, window)
            }
            SetCtl::Size(d) => self.settings_edit(|s| s.font_size += d as f32, window),
            SetCtl::Spacing(d) => self.settings_edit(|s| s.line_spacing += d as f32 * 0.1, window),
            SetCtl::Smooth => self.settings_edit(|s| s.smooth_scrolling = !s.smooth_scrolling, window),
            SetCtl::Blink => self.settings_edit(|s| s.caret_blinking = !s.caret_blinking, window),
            SetCtl::Slide => self.settings_edit(|s| s.smooth_caret = !s.smooth_caret, window),
            SetCtl::Sticky => self.settings_edit(|s| s.sticky_lines = !s.sticky_lines, window),
            SetCtl::Whitespace => self.settings_edit(|s| s.show_whitespaces = !s.show_whitespaces, window),
            SetCtl::Inlay => self.settings_edit(|s| s.inlay_hints = !s.inlay_hints, window),
            SetCtl::Animations => self.settings_edit(|s| s.animations = !s.animations, window),
            SetCtl::Reformat => self.settings_edit(|s| s.reformat_on_save = !s.reformat_on_save, window),
            SetCtl::Imports => self.settings_edit(|s| s.optimize_imports_on_save = !s.optimize_imports_on_save, window),
            SetCtl::TabRows(rows) => self.settings_edit(|s| s.tab_rows = rows, window),
            SetCtl::TabLimit(d) => self.settings_edit(|s| s.tab_limit = (s.tab_limit as f32 + d as f32).clamp(1., 100.) as u32, window),
            SetCtl::Cancel => return self.settings_close(false, window, cx),
            SetCtl::Ok => return self.settings_close(true, window, cx),
        }
        cx.notify();
    }

    /// A click on control `c`: it takes the focus (its ring shows) and is pressed.
    fn settings_click(c: SetCtl, cx: &mut Context<Self>) -> Box<dyn Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static> {
        Box::new(cx.listener(move |this, _: &gpui::ClickEvent, w, cx| {
            if let Some(d) = this.settings_dialog.as_mut() {
                d.kb = d.controls().iter().position(|x| *x == c);
            }
            this.settings_press(c, w, cx);
        }))
    }

    pub(super) fn render_settings(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let d = self.settings_dialog.as_ref()?;
        let s = d.draft.clone();
        let focus = d.focused();
        let chip = |id: (&'static str, usize), on: bool, label: String, c: SetCtl| {
            div()
                .id(id)
                .relative()
                .px(px(10.))
                .h(ui_px(26.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .border_1()
                .border_color(if on { Theme::accent() } else { Theme::divider() })
                .when(on, |x| x.bg(Theme::selection_row()))
                .when(!on, |x| x.hover(|st| st.bg(Theme::hover_row())).active(|st| st.bg(Theme::press())))
                .child(label)
                .when(focus == Some(c), |x| x.child(crate::ui_kit::focus_ring(5., 0.)))
        };
        let row = |label: &'static str, value: gpui::AnyElement| {
            // The value takes the rest of the width, so a wrapping value (the fonts) grows the
            // row instead of running into the next one.
            // Rows do not shrink in the scrolling body: a shrunk row lets its value spill over
            // the next one.
            div()
                .flex_shrink_0()
                .min_h(ui_px(36.))
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().w(px(110.)).flex_shrink_0().text_color(Theme::text_muted()).child(label))
                .child(div().flex_1().min_w_0().py(px(4.)).child(value))
        };
        // − value +
        let stepper = |id: &'static str, value: String, ctl: fn(i8) -> SetCtl, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(chip((id, 0), false, "−".into(), ctl(-1)).on_click(Self::settings_click(ctl(-1), cx)))
                .child(div().w(px(44.)).flex().justify_center().child(value))
                .child(chip((id, 1), false, "+".into(), ctl(1)).on_click(Self::settings_click(ctl(1), cx)))
                .into_any_element()
        };
        // Two or more choices, one of them on.
        let choices = |id: &'static str, items: Vec<(String, bool, SetCtl)>, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(6.))
                .children(items.into_iter().enumerate().map(|(i, (label, on, c))| chip((id, i), on, label, c).on_click(Self::settings_click(c, cx))))
        };
        let theme =
            choices("set-theme", vec![("Dark".into(), !s.light, SetCtl::Theme(false)), ("Light".into(), s.light, SetCtl::Theme(true))], cx).into_any_element();
        // UI style: the setting; BRISK_DESIGN, when it names a style, wins (and says so).
        let forced = crate::theme::islands_override();
        let style = choices("set-style", vec![("Classic".into(), !s.islands, SetCtl::Style(false)), ("Islands".into(), s.islands, SetCtl::Style(true))], cx)
            .when_some(forced, |d, on| {
                d.child(
                    div()
                        .pl(px(6.))
                        .text_size(ui_px(11.5))
                        .text_color(Theme::text_dim())
                        .child(format!("BRISK_DESIGN sets {} for this run", if on { "Islands" } else { "Classic" })),
                )
            })
            .into_any_element();
        let fonts = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .children(d.fonts.iter().enumerate().map(|(i, f)| {
                chip(("set-font", i), f.eq_ignore_ascii_case(&s.editor_font), f.clone(), SetCtl::Font(i))
                    .font_family(f.clone())
                    .on_click(Self::settings_click(SetCtl::Font(i), cx))
            }))
            .into_any_element();
        let size = stepper("set-size", format!("{}", s.font_size), SetCtl::Size, cx);
        let spacing = stepper("set-spacing", format!("{:.1}", s.line_spacing), SetCtl::Spacing, cx);
        let ui_size = stepper("set-ui-size", format!("{}", s.ui_font_size), SetCtl::UiSize, cx);
        let tab_limit = stepper("set-tab-limit", format!("{}", s.tab_limit), SetCtl::TabLimit, cx);
        let tab_rows = choices(
            "set-tab-rows",
            vec![("One row".into(), !s.tab_rows, SetCtl::TabRows(false)), ("Multiple rows".into(), s.tab_rows, SetCtl::TabRows(true))],
            cx,
        )
        .into_any_element();
        // A checkbox; filled with the accent when on, so its focus ring sits outside.
        let checkbox = |id: &'static str, on: bool, label: &'static str, note: &'static str, c: SetCtl, cx: &mut Context<Self>| {
            div()
                .id(id)
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .child(
                    div()
                        .relative()
                        .size(ui_px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(3.))
                        .border_1()
                        .border_color(if on { Theme::accent() } else { Theme::divider() })
                        .when(on, |x| x.bg(Theme::accent()).child(crate::icons::ui_icon("check-bold", 12., gpui::white())))
                        .when(focus == Some(c), |x| x.child(crate::ui_kit::focus_ring(3., if on { 3. } else { 0. }))),
                )
                .child(label)
                .when(!note.is_empty(), |x| x.child(div().text_color(Theme::text_dim()).child(note)))
                .on_click(Self::settings_click(c, cx))
                .into_any_element()
        };
        let inlay = checkbox("set-inlay", s.inlay_hints, "Show inlay hints", "(inferred types, parameter names)", SetCtl::Inlay, cx);
        let reformat = checkbox("set-reformat", s.reformat_on_save, "Reformat code", "(ruff format)", SetCtl::Reformat, cx);
        let imports = checkbox("set-imports", s.optimize_imports_on_save, "Optimize imports", "(ruff: unused, sorted)", SetCtl::Imports, cx);
        let animations = checkbox("set-anim", s.animations, "Animate popups, dialogs and tool windows", "(off: they appear at once)", SetCtl::Animations, cx);
        let smooth = checkbox("set-smooth", s.smooth_scrolling, "Enable smooth scrolling", "", SetCtl::Smooth, cx);
        let caret = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .py(px(4.))
            .child(checkbox("set-blink", s.caret_blinking, "Caret blinking", "", SetCtl::Blink, cx))
            .child(checkbox("set-slide", s.smooth_caret, "Smooth caret movement", "(the caret slides to its new place)", SetCtl::Slide, cx))
            .into_any_element();
        let whitespace = checkbox("set-ws", s.show_whitespaces, "Show whitespaces", "(· for spaces, → for tabs)", SetCtl::Whitespace, cx);
        let sticky = checkbox("set-sticky", s.sticky_lines, "Show sticky lines", "(enclosing class and def headers stay on top)", SetCtl::Sticky, cx);
        let editing = d.mask_edit;
        let masks = div()
            .id("set-wrap-files")
            .relative()
            .flex_1()
            .h(ui_px(26.))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(4.))
            .border_1()
            .border_color(if editing { Theme::accent() } else { Theme::divider() })
            .bg(Theme::editor_bg())
            .cursor_text()
            .child(s.soft_wrap_files.clone())
            .when(editing, |x| x.child(div().w(px(1.5)).h(ui_px(15.)).bg(Theme::caret())))
            .when(focus == Some(SetCtl::WrapFiles) && !editing, |x| x.child(crate::ui_kit::focus_ring(4., 0.)))
            .on_click(Self::settings_click(SetCtl::WrapFiles, cx))
            .into_any_element();
        let wrap_indent = checkbox("set-wrap-indent", s.soft_wrap_indent, "Use original line's indent for wrapped parts", "", SetCtl::WrapIndent, cx);
        let wrap_extra = stepper("set-wrap-extra", format!("+{}", s.soft_wrap_extra_indent), SetCtl::WrapExtra, cx);
        let comment_col = checkbox("set-comment-col", s.line_comment_first_column, "Line comment at first column", "", SetCtl::CommentCol, cx);
        let comment_space = checkbox("set-comment-space", s.line_comment_space, "Add a space at line comment start", "", SetCtl::CommentSpace, cx);
        let preview = div()
            .mt(px(6.))
            .p(px(10.))
            .rounded(px(6.))
            .bg(Theme::editor_bg())
            .border_1()
            .border_color(Theme::divider())
            .font_family(s.editor_font.clone())
            .text_size(px(s.font_size))
            .line_height(px(settings::line_height()))
            .child(
                div()
                    .flex()
                    .child(div().text_color(Theme::syntax(brisk_syntax::Highlight::Keyword)).child("def "))
                    .child(div().text_color(Theme::syntax(brisk_syntax::Highlight::Function)).child("greet"))
                    .child(div().text_color(Theme::text()).child("(name):")),
            )
            .child(
                div()
                    .flex()
                    .child(div().text_color(Theme::syntax(brisk_syntax::Highlight::Keyword)).child("    return "))
                    .child(div().text_color(Theme::syntax(brisk_syntax::Highlight::String)).child("f\"Hello, {name}!\""))
                    .child(div().text_color(Theme::syntax(brisk_syntax::Highlight::Comment)).child("  # 0O1lI")),
            );
        let button = |id: &'static str, label: &'static str, primary: bool, c: SetCtl, cx: &mut Context<Self>| {
            crate::ui_kit::button(id, label, true, primary)
                .text_size(ui_px(13.))
                .px(px(14.))
                .when(focus == Some(c), |b| b.child(crate::ui_kit::focus_ring(crate::ui_kit::RADIUS, if primary { 3. } else { 0. })))
                .on_click(Self::settings_click(c, cx))
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(70.))
                .occlude()
                .child(
                    div()
                        .id("settings-dialog")
                        .w(px(620.))
                        .max_h(gpui::relative(0.86))
                        .flex()
                        .flex_col()
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .map(crate::ui_kit::floating)
                        .text_size(ui_px(13.))
                        .text_color(Theme::text())
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).px(px(16.)).pt(px(16.)).pb(px(8.)).child("Settings"))
                        .child(
                            div()
                                .id("settings-body")
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .px(px(16.))
                                .child(div().text_size(ui_px(11.)).text_color(Theme::text_dim()).pb(px(2.)).child("APPEARANCE"))
                                .child(row("Theme", theme))
                                .child(row("UI style", style))
                                .child(row("UI font size", ui_size))
                                .child(row("Animations", animations))
                                .child(div().text_size(ui_px(11.)).text_color(Theme::text_dim()).pt(px(8.)).pb(px(2.)).child("EDITOR › GENERAL"))
                                .child(row("Scrolling", smooth))
                                .child(row("Caret", caret))
                                .child(row("Sticky lines", sticky))
                                .child(div().text_size(ui_px(11.)).text_color(Theme::text_dim()).pt(px(8.)).pb(px(2.)).child("EDITOR › FONT"))
                                .child(row("Font", fonts))
                                .child(row("Size", size))
                                .child(row("Line spacing", spacing))
                                .child(preview)
                                .child(div().text_size(ui_px(11.)).text_color(Theme::text_dim()).pt(px(8.)).pb(px(2.)).child("EDITOR › APPEARANCE"))
                                .child(row("Inlay hints", inlay))
                                .child(row("Whitespaces", whitespace))
                                .child(div().text_size(ui_px(11.)).text_color(Theme::text_dim()).pt(px(8.)).pb(px(2.)).child("EDITOR › EDITOR TABS"))
                                .child(row("Show tabs in", tab_rows))
                                .child(row("Tab limit", tab_limit))
                                .child(div().text_size(ui_px(11.)).text_color(Theme::text_dim()).pt(px(8.)).pb(px(2.)).child("EDITOR › GENERAL › SOFT WRAPS"))
                                .child(row("Soft-wrap files", masks))
                                .child(row("", div().flex().items_center().gap(px(12.)).child(wrap_indent).child(wrap_extra).into_any_element()))
                                .child(
                                    div()
                                        .text_size(ui_px(11.))
                                        .text_color(Theme::text_dim())
                                        .pt(px(8.))
                                        .pb(px(2.))
                                        .child("EDITOR › CODE STYLE › PYTHON › CODE GENERATION"),
                                )
                                .child(row("Comment code", comment_col))
                                .child(row("", comment_space))
                                .child(div().text_size(ui_px(11.)).text_color(Theme::text_dim()).pt(px(8.)).pb(px(2.)).child("TOOLS › ACTIONS ON SAVE"))
                                .child(row("", reformat))
                                .child(row("", imports)),
                        )
                        .child(
                            div()
                                .p(px(16.))
                                .pt(px(12.))
                                .border_t_1()
                                .border_color(Theme::divider())
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_size(ui_px(11.))
                                        .text_color(Theme::text_dim())
                                        .child("Alt+Shift+. / Alt+Shift+, or Ctrl+wheel: zoom all editors"),
                                )
                                .child(button("set-cancel", "Cancel", false, SetCtl::Cancel, cx))
                                .child(button("set-ok", "OK", true, SetCtl::Ok, cx)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Smoke hook (BRISK_SMOKE_SETTINGS): the dialog, live preview, Cancel, OK, zoom.
    #[cfg(feature = "smoke")]
    pub(super) fn settings_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let show = || {
            let s = settings::current();
            format!(
                "light={} font={} size={} spacing={} line_height={}",
                s.light,
                s.editor_font,
                settings::font_size(),
                s.line_spacing,
                settings::line_height()
            )
        };
        eprintln!("smoke-settings: start -> {}", show());
        self.dispatch(Action::Settings, window, cx);
        if std::env::var_os("BRISK_SMOKE_SETTINGS_HOLD").is_some() {
            eprintln!("smoke-settings: holding with the dialog open");
            return;
        }
        eprintln!("smoke-settings: dialog open={} fonts={:?}", self.settings_dialog.is_some(), self.settings_dialog.as_ref().map(|d| d.fonts.clone()));
        self.settings_edit(|s| s.light = true, window);
        self.settings_edit(|s| s.font_size += 2.0, window);
        eprintln!("smoke-settings: light + size+2 (live) -> {} text color {:?}", show(), Theme::editor_bg());
        self.smoke_press("escape", window, cx);
        eprintln!("smoke-settings: escape -> open={} {}", self.settings_dialog.is_some(), show());
        self.dispatch(Action::Settings, window, cx);
        self.settings_edit(|s| s.inlay_hints = false, window);
        eprintln!("smoke-settings: inlay hints off (live) -> {}", settings::inlay_hints());
        self.settings_edit(|s| s.islands = true, window);
        eprintln!("smoke-settings: ui style islands (live) -> islands={}", crate::theme::islands());
        self.settings_edit(|s| (s.smooth_scrolling, s.caret_blinking, s.smooth_caret) = (false, false, true), window);
        eprintln!(
            "smoke-settings: smooth scrolling / caret blinking / smooth caret (live) -> {} {} {}",
            settings::smooth_scrolling(),
            settings::caret_blinking(),
            settings::smooth_caret()
        );
        self.smoke_press("escape", window, cx);
        eprintln!(
            "smoke-settings: escape -> inlay hints {} islands={} smooth scrolling {} caret blinking {} smooth caret {}",
            settings::inlay_hints(),
            crate::theme::islands(),
            settings::smooth_scrolling(),
            settings::caret_blinking(),
            settings::smooth_caret()
        );
        // Keyboard focus: Tab / Shift+Tab move the ring, Space presses the focused control,
        // Enter on Cancel cancels.
        self.dispatch(Action::Settings, window, cx);
        let light0 = settings::current().light;
        self.smoke_press("tab tab space", window, cx);
        let (at, light) = (self.settings_dialog.as_ref().and_then(|d| d.focused()), settings::current().light);
        self.smoke_press("shift-tab", window, cx);
        let back = self.settings_dialog.as_ref().and_then(|d| d.focused());
        self.smoke_press("shift-tab shift-tab", window, cx);
        let wrapped = self.settings_dialog.as_ref().and_then(|d| d.focused());
        self.smoke_press("enter", window, cx);
        eprintln!(
            "smoke-settings: tab tab space -> {at:?} light {light0}->{light}; shift-tab -> {back:?}; twice more -> {wrapped:?}; Cancel+Enter -> open={} light={}",
            self.settings_dialog.is_some(),
            settings::current().light
        );
        self.dispatch(Action::Settings, window, cx);
        let font = self.settings_dialog.as_ref().and_then(|d| d.fonts.last().cloned()).unwrap_or_default();
        self.settings_edit(
            |s| {
                s.light = true;
                s.font_size = 15.0;
                s.line_spacing = 1.2;
                s.editor_font = font;
            },
            window,
        );
        self.smoke_press("enter", window, cx);
        let saved = std::fs::read_to_string(brisk_db::config_dir().join("settings.json")).unwrap_or_default();
        eprintln!("smoke-settings: OK -> {} saved={}", show(), saved.replace(['\n', ' '], ""));
        self.smoke_press("alt-shift-. alt-shift-.", window, cx);
        eprintln!("smoke-settings: alt-shift-. x2 -> size {}", settings::font_size());
        self.smoke_press("alt-shift-,", window, cx);
        eprintln!("smoke-settings: alt-shift-, -> size {}", settings::font_size());
        // The zoom survives editing another setting and Cancel.
        let zoomed = settings::font_size();
        self.dispatch(Action::Settings, window, cx);
        self.settings_edit(|s| s.inlay_hints = !s.inlay_hints, window);
        self.smoke_press("escape", window, cx);
        let kept = settings::font_size();
        eprintln!("smoke-settings: zoom {zoomed} after an edit and Cancel -> {kept}{}", if kept == zoomed { "" } else { " FAIL" });
        self.dispatch(Action::ResetFontSize, window, cx);
        eprintln!("smoke-settings: reset -> size {}", settings::font_size());
        // .editorconfig: indent from the config, trailing spaces and final newline on save.
        let dir = self.root.join("ec_smoke");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(
            self.root.join(".editorconfig"),
            "root = true\n[*]\ninsert_final_newline = true\ntrim_trailing_whitespace = true\n[ec_smoke/*.py]\nindent_size = 2\nend_of_line = crlf\n",
        );
        let file = dir.join("ec.py");
        let _ = std::fs::write(&file, "def f():\n        return 1   \nx = 2   ");
        let detect = self.root.join("tabs.mk");
        let _ = std::fs::write(&detect, "all:\n\tcc a\n\tcc b\n");
        self.open_file(&detect, window, cx);
        let tab_label = self.active_editor().map(|e| e.read(cx).indent_label());
        self.open_file(&file, window, cx);
        if let Some(ed) = self.active_editor().cloned() {
            eprintln!("smoke-settings: editorconfig -> indent {:?}, tabs.mk detected {:?}", ed.read(cx).indent_label(), tab_label);
            ed.update(cx, |e, _| {
                let p = e.buffer.line_col_to_pos(2, 0);
                e.buffer.set_caret(p, false);
            });
            self.dispatch(Action::EditorTab, window, cx);
            ed.update(cx, |e, _| e.buffer.set_caret(0, false));
            self.dispatch(Action::SaveAll, window, cx);
            let disk = std::fs::read_to_string(&file).unwrap_or_default();
            eprintln!("smoke-settings: tab + save -> {disk:?}");
        }
        let _ = std::fs::remove_file(self.root.join(".editorconfig"));

        // Local History: revisions on open and save, diff, revert.
        if let Some(ed) = self.active_editor().cloned() {
            // Saves record their revision on the local-history thread: wait for both (the
            // UI thread may block here, the recording does not run on it).
            let wait_revisions = |want: usize| {
                let t0 = std::time::Instant::now();
                while crate::local_history::revisions(&file).len() < want && t0.elapsed() < std::time::Duration::from_secs(5) {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                crate::local_history::revisions(&file).len()
            };
            let before = wait_revisions(2);
            for line in ["y = 3\n", "z = 4\n"] {
                ed.update(cx, |e, _| {
                    let end = e.buffer.len_chars();
                    e.buffer.set_caret(end, false);
                    e.buffer.insert(line);
                });
                self.dispatch(Action::SaveAll, window, cx);
            }
            if wait_revisions(before + 2) < before + 2 {
                eprintln!("smoke-history: FAIL the two saves were not recorded within 5 s");
            }
            self.dispatch(Action::ShowLocalHistory, window, cx);
            let n = self.popup.as_ref().map(|p| p.items.len()).unwrap_or(0);
            eprintln!("smoke-history: popup -> {} ({n} revisions)", self.smoke_popup());
            let details: Vec<String> = self.popup.as_ref().map(|p| p.items.iter().map(|i| i.detail.clone()).collect()).unwrap_or_default();
            eprintln!("smoke-history: vs now -> {details:?}");
            self.smoke_press("down enter", window, cx);
            let tab = self.active.and_then(|i| match self.tabs.get(i) {
                Some(super::Tab::Diff(d)) => Some(d.read(cx).title.clone()),
                _ => None,
            });
            eprintln!("smoke-history: enter -> diff tab {tab:?}");
            self.open_file(&file, window, cx);
            self.dispatch(Action::ShowLocalHistory, window, cx);
            let last = n.saturating_sub(1);
            if let Some(p) = self.popup.as_mut() {
                p.selected = last;
            }
            self.smoke_press("ctrl-enter", window, cx);
            eprintln!("smoke-history: ctrl-enter on the oldest -> editor {:?} status {:?}", ed.read(cx).buffer.text(), self.status);
            self.dispatch(Action::Undo, window, cx);
            eprintln!("smoke-history: undo -> {:?}, revisions now {}", ed.read(cx).buffer.text(), crate::local_history::revisions(&file).len());
        }
        let back = settings::load();
        eprintln!("smoke-settings: reloaded from disk -> light={} size={}", back.light, back.font_size);
        settings::apply(&Settings::default());
        let _ = settings::save(&Settings::default());
        eprintln!("smoke-settings: done");
        cx.quit();
    }
}
