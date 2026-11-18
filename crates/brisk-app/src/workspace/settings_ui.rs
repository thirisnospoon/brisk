//! Settings dialog (Ctrl+Alt+S): theme, editor font, font size, line spacing, inlay hints; font zoom
//! (Alt+Shift+. / Alt+Shift+, / Ctrl+wheel) and the quick theme switch.

use super::Workspace;
use crate::settings::{self, Settings};
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
        self.settings_dialog = Some(SettingsDialog { orig: s.clone(), draft: s, fonts });
    }

    /// Change the draft and show it at once.
    pub(super) fn settings_edit(&mut self, f: impl FnOnce(&mut Settings), window: &mut Window) {
        let Some(d) = self.settings_dialog.as_mut() else { return };
        f(&mut d.draft);
        d.draft.font_size = d.draft.font_size.clamp(settings::MIN_FONT_SIZE, settings::MAX_FONT_SIZE);
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
        match e.keystroke.key.as_str() {
            "escape" => self.settings_close(false, window, cx),
            "enter" => self.settings_close(true, window, cx),
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn render_settings(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let d = self.settings_dialog.as_ref()?;
        let s = d.draft.clone();
        let chip = |id: (&'static str, usize), on: bool, label: String| {
            div()
                .id(id)
                .px(px(10.))
                .h(px(26.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .border_1()
                .border_color(if on { Theme::accent() } else { Theme::divider() })
                .when(on, |x| x.bg(Theme::selection_row()))
                .when(!on, |x| x.hover(|st| st.bg(Theme::hover_row())))
                .child(label)
        };
        let row = |label: &'static str, value: gpui::AnyElement| {
            div().min_h(px(36.)).flex().items_center().gap(px(12.)).child(div().w(px(110.)).flex_shrink_0().text_color(Theme::text_muted()).child(label)).child(value)
        };
        let stepper = |id: &'static str, value: String, step: f32, size: bool, cx: &mut Context<Self>| {
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(chip((id, 0), false, "−".into()).on_click(cx.listener(move |this, _, w, cx| {
                    this.settings_edit(|s| if size { s.font_size -= step } else { s.line_spacing -= step }, w);
                    cx.notify();
                })))
                .child(div().w(px(44.)).flex().justify_center().child(value))
                .child(chip((id, 1), false, "+".into()).on_click(cx.listener(move |this, _, w, cx| {
                    this.settings_edit(|s| if size { s.font_size += step } else { s.line_spacing += step }, w);
                    cx.notify();
                })))
                .into_any_element()
        };
        let theme = div()
            .flex()
            .gap(px(6.))
            .child(chip(("set-theme", 0), !s.light, "Dark".into()).on_click(cx.listener(|this, _, w, cx| {
                this.settings_edit(|s| s.light = false, w);
                cx.notify();
            })))
            .child(chip(("set-theme", 1), s.light, "Light".into()).on_click(cx.listener(|this, _, w, cx| {
                this.settings_edit(|s| s.light = true, w);
                cx.notify();
            })))
            .into_any_element();
        let fonts = div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .children(d.fonts.iter().enumerate().map(|(i, f)| {
                let name = f.clone();
                chip(("set-font", i), f.eq_ignore_ascii_case(&s.editor_font), f.clone()).font_family(f.clone()).on_click(cx.listener(move |this, _, w, cx| {
                    let n = name.clone();
                    this.settings_edit(|s| s.editor_font = n, w);
                    cx.notify();
                }))
            }))
            .into_any_element();
        let size = stepper("set-size", format!("{}", s.font_size), 1.0, true, cx);
        let spacing = stepper("set-spacing", format!("{:.1}", s.line_spacing), 0.1, false, cx);
        let on = s.inlay_hints;
        let inlay = div()
            .id("set-inlay")
            .flex()
            .items_center()
            .gap(px(8.))
            .cursor_pointer()
            .child(
                div()
                    .size(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if on { Theme::accent() } else { Theme::divider() })
                    .when(on, |x| x.bg(Theme::accent()).text_color(gpui::white()).text_size(px(11.)).child("✓")),
            )
            .child("Show inlay hints")
            .child(div().text_color(Theme::text_dim()).child("(inferred types, parameter names)"))
            .on_click(cx.listener(|this, _, w, cx| {
                this.settings_edit(|s| s.inlay_hints = !s.inlay_hints, w);
                cx.notify();
            }))
            .into_any_element();
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
            .child(div().flex().child(div().text_color(Theme::syntax(brisk_syntax::Highlight::Keyword)).child("def ")).child(div().text_color(Theme::syntax(brisk_syntax::Highlight::Function)).child("greet")).child(div().text_color(Theme::text()).child("(name):")))
            .child(div().flex().child(div().text_color(Theme::syntax(brisk_syntax::Highlight::Keyword)).child("    return ")).child(div().text_color(Theme::syntax(brisk_syntax::Highlight::String)).child("f\"Hello, {name}!\"")).child(div().text_color(Theme::syntax(brisk_syntax::Highlight::Comment)).child("  # 0O1lI")));
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px(px(14.))
                .h(px(28.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .cursor_pointer()
                .when(primary, |b| b.bg(Theme::accent()).text_color(gpui::white()))
                .when(!primary, |b| b.border_1().border_color(Theme::divider()).hover(|st| st.bg(Theme::hover_row())))
                .child(label)
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
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .p(px(16.))
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .shadow_lg()
                        .text_size(px(13.))
                        .text_color(Theme::text())
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).pb(px(8.)).child("Settings"))
                        .child(div().text_size(px(11.)).text_color(Theme::text_dim()).pb(px(2.)).child("APPEARANCE"))
                        .child(row("Theme", theme))
                        .child(div().text_size(px(11.)).text_color(Theme::text_dim()).pt(px(8.)).pb(px(2.)).child("EDITOR › FONT"))
                        .child(row("Font", fonts))
                        .child(row("Size", size))
                        .child(row("Line spacing", spacing))
                        .child(preview)
                        .child(div().text_size(px(11.)).text_color(Theme::text_dim()).pt(px(8.)).pb(px(2.)).child("EDITOR › APPEARANCE"))
                        .child(row("Inlay hints", inlay))
                        .child(
                            div()
                                .pt(px(12.))
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(div().flex_1().text_size(px(11.)).text_color(Theme::text_dim()).child("Alt+Shift+. / Alt+Shift+, or Ctrl+wheel: zoom all editors"))
                                .child(button("set-cancel", "Cancel", false).on_click(cx.listener(|this, _, w, cx| this.settings_close(false, w, cx))))
                                .child(button("set-ok", "OK", true).on_click(cx.listener(|this, _, w, cx| this.settings_close(true, w, cx)))),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Smoke hook (BRISK_SMOKE_SETTINGS): the dialog, live preview, Cancel, OK, zoom.
    pub(super) fn settings_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let show = || {
            let s = settings::current();
            format!("light={} font={} size={} spacing={} line_height={}", s.light, s.editor_font, settings::font_size(), s.line_spacing, settings::line_height())
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
        self.smoke_press("escape", window, cx);
        eprintln!("smoke-settings: escape -> inlay hints {}", settings::inlay_hints());
        self.dispatch(Action::Settings, window, cx);
        let font = self.settings_dialog.as_ref().and_then(|d| d.fonts.last().cloned()).unwrap_or_default();
        self.settings_edit(|s| {
            s.light = true;
            s.font_size = 15.0;
            s.line_spacing = 1.2;
            s.editor_font = font;
        }, window);
        self.smoke_press("enter", window, cx);
        let saved = std::fs::read_to_string(brisk_db::config_dir().join("settings.json")).unwrap_or_default();
        eprintln!("smoke-settings: OK -> {} saved={}", show(), saved.replace(['\n', ' '], ""));
        self.smoke_press("alt-shift-. alt-shift-.", window, cx);
        eprintln!("smoke-settings: alt-shift-. x2 -> size {}", settings::font_size());
        self.smoke_press("alt-shift-,", window, cx);
        eprintln!("smoke-settings: alt-shift-, -> size {}", settings::font_size());
        self.dispatch(Action::ResetFontSize, window, cx);
        eprintln!("smoke-settings: reset -> size {}", settings::font_size());
        // .editorconfig: indent from the config, trailing spaces and final newline on save.
        let dir = self.root.join("ec_smoke");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(self.root.join(".editorconfig"), "root = true\n[*]\ninsert_final_newline = true\ntrim_trailing_whitespace = true\n[ec_smoke/*.py]\nindent_size = 2\nend_of_line = crlf\n");
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
            for line in ["y = 3\n", "z = 4\n"] {
                ed.update(cx, |e, _| {
                    let end = e.buffer.len_chars();
                    e.buffer.set_caret(end, false);
                    e.buffer.insert(line);
                });
                self.dispatch(Action::SaveAll, window, cx);
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
