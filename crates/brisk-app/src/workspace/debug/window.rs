//! The Debug window: keys and drawing of the Debugger and Console tabs.

use super::*;

impl Workspace {
    /// Keys while the Debug window has focus: the console's input line.
    pub(in crate::workspace) fn debug_key(&mut self, e: &gpui::KeyDownEvent, window: &Window, cx: &mut Context<Self>) -> bool {
        if !self.debug.focus.is_focused(window) {
            return false;
        }
        let k = &e.keystroke;
        if self.debug.tab == DebugTab::Debugger {
            return self.vars_key(k, cx);
        }
        let m = k.modifiers;
        let paste = ((m.control || m.platform) && !m.alt && !m.shift && k.key == "v") || (m.shift && !m.control && k.key == "insert");
        if paste {
            // A one-line input: the clipboard's first line, as the Run dialog's fields take it.
            if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                self.debug.input.push_str(t.lines().next().unwrap_or(""));
                self.refilter_completion();
            }
            cx.notify();
            return true;
        }
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return false;
        }
        if self.completion_key(k.key.as_str()) {
            cx.notify();
            return true;
        }
        match k.key.as_str() {
            "tab" if !k.modifiers.shift => self.complete_input(cx),
            "enter" => self.evaluate_input(cx),
            "backspace" => {
                self.debug.input.pop();
            }
            "space" => self.debug.input.push(' '),
            "escape" => self.debug.input.clear(),
            "up" => {
                let pos = self.debug.history_pos.min(self.debug.history.len());
                if pos > 0 {
                    self.debug.history_pos = pos - 1;
                    self.debug.input = self.debug.history[pos - 1].clone();
                }
            }
            "down" => {
                if self.debug.history_pos.saturating_add(1) < self.debug.history.len() {
                    self.debug.history_pos += 1;
                    self.debug.input = self.debug.history[self.debug.history_pos].clone();
                } else {
                    self.debug.history_pos = self.debug.history.len();
                    self.debug.input.clear();
                }
            }
            _ => match crate::keys::typed_text(k) {
                Some(t) => self.debug.input.push_str(t),
                _ => return false,
            },
        }
        self.refilter_completion();
        cx.notify();
        true
    }

    // --- Debug window -------------------------------------------------------------------

    pub(in crate::workspace) fn render_debug(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let d = &self.debug;
        let state = d.state();
        let paused = matches!(state, Some(State::Paused(_)));
        let live = d.is_live();
        let running = state == Some(State::Running) && !d.pending;
        let can_rerun = d.last.is_some() || d.last_attach.is_some();
        let title = match (&d.session, state) {
            _ if d.pending => format!("{} — starting debugpy…", d.last.as_ref().map(|c| c.name.as_str()).unwrap_or("")),
            (Some(s), Some(State::Paused(_))) => {
                let at = d
                    .frames
                    .get(d.frame)
                    .map(|f| {
                        format!(
                            " at {}:{}",
                            f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                            f.line + 1
                        )
                    })
                    .unwrap_or_default();
                format!("{} — paused{at}", s.name)
            }
            (Some(s), Some(State::Running)) => format!("{} — running", s.name),
            (Some(s), Some(State::Starting)) => format!("{} — starting debugpy…", s.name),
            (Some(s), _) => format!("{} — finished", s.name),
            (None, _) => "No debug session. Shift+F9 debugs the selected configuration, Ctrl+Shift+F9 the current file.".into(),
        };
        let tool = |id: &'static str, icon: &'static str, tip: &'static str, on: bool, color: gpui::Hsla, action: Action, cx: &mut Context<Self>| {
            div()
                .id(id)
                .size(ui_px(24.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .when(on, |x| x.cursor_pointer().hover(|s| s.bg(Theme::hover_row())).active(|s| s.bg(Theme::press())))
                .child(crate::icons::tool_icon(icon, 16., if on { color } else { Theme::text_dim().opacity(0.5) }))
                .tooltip(move |_, cx| cx.new(|_| crate::workspace::Tip(tip.into())).into())
                .on_click(cx.listener(move |this, _, w, cx| {
                    if on {
                        this.dispatch(action, w, cx);
                    }
                }))
        };
        let green = Theme::run_icon();
        let plain = Theme::text_muted();
        let toolbar = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .pl(px(12.))
            .pr(px(6.))
            .h(px(crate::ui_kit::header_h()))
            .border_b_1()
            .border_color(Theme::divider())
            .child(crate::ui_kit::tool_title("Debug"))
            .child(tool("dbg-rerun", "rerun", "Rerun Debug Session", can_rerun, green, Action::RerunDebug, cx))
            .child(tool("dbg-resume", "resume", "Resume Program  F9", paused && !d.pending, green, Action::Resume, cx))
            .child(tool("dbg-pause", "pause", "Pause Program", running, plain, Action::PauseProgram, cx))
            .child(tool("dbg-stop", "stop", "Stop  Ctrl+F2", live, Theme::error(), Action::Stop, cx))
            .child(div().w(px(1.)).h(ui_px(16.)).mx(px(4.)).bg(Theme::divider()))
            .child(tool("dbg-over", "step-over", "Step Over  F8", paused, plain, Action::StepOver, cx))
            .child(tool("dbg-into", "step-into", "Step Into  F7", paused, plain, Action::StepInto, cx))
            .child(tool("dbg-out", "step-out", "Step Out  Shift+F8", paused, plain, Action::StepOut, cx))
            .child(tool("dbg-cursor", "run-to-cursor", "Run to Cursor  Alt+F9", paused, plain, Action::RunToCursor, cx))
            .child(tool("dbg-eval", "evaluate", "Evaluate Expression  Alt+F8", paused, plain, Action::EvaluateExpression, cx))
            .child(div().w(px(1.)).h(ui_px(16.)).mx(px(4.)).bg(Theme::divider()))
            .child(tool("dbg-bps", "breakpoints", "View Breakpoints  Ctrl+Shift+F8", true, Theme::breakpoint(), Action::ViewBreakpoints, cx))
            .child(
                // Mute Breakpoints: a crossed-out dot, pressed while on.
                div()
                    .id("dbg-mute")
                    .size(ui_px(24.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .cursor_pointer()
                    .when(d.muted, |x| x.bg(Theme::selection_row()))
                    .when(!d.muted, |x| x.hover(|s| s.bg(Theme::hover_row())))
                    .active(|s| s.bg(Theme::press()))
                    .child(crate::icons::tool_icon("mute-breakpoints", 16., if d.muted { Theme::text() } else { plain }))
                    .tooltip(|_, cx| cx.new(|_| crate::workspace::Tip("Mute Breakpoints".into())).into())
                    .on_click(cx.listener(|this, _, w, cx| this.dispatch(Action::MuteBreakpoints, w, cx))),
            )
            .child(div().pl(px(10.)).text_size(ui_px(12.)).text_color(Theme::text_muted()).child(title));
        let tab = |id: &'static str, label: &'static str, t: DebugTab, cur: DebugTab, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px(px(10.))
                .h(ui_px(26.))
                .flex()
                .items_center()
                .cursor_pointer()
                .text_color(if t == cur { Theme::text() } else { Theme::text_muted() })
                .when(t == cur, |x| x.border_b_2().border_color(Theme::accent()))
                .child(label)
                .on_click(cx.listener(move |this, _, w, cx| {
                    this.debug.tab = t;
                    w.focus(&this.debug.focus.clone());
                    cx.notify();
                }))
        };
        let tabs = div().flex().px(px(6.)).child(tab("dbg-tab-dbg", "Debugger", DebugTab::Debugger, d.tab, cx)).child(tab(
            "dbg-tab-con",
            "Console",
            DebugTab::Console,
            d.tab,
            cx,
        ));
        let body: gpui::AnyElement = match d.tab {
            DebugTab::Debugger => self.render_debugger_tab(window, cx),
            DebugTab::Console => self.render_console_tab(window, cx),
        };
        div()
            .id("debug-window")
            .track_focus(&self.debug.focus)
            .size_full()
            .flex()
            .flex_col()
            .text_size(ui_px(13.))
            .child(toolbar)
            .child(tabs)
            .child(div().flex_1().min_h_0().border_t_1().border_color(Theme::divider()).child(body))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, w, cx| {
                    w.focus(&this.debug.focus.clone());
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    /// One row of the Variables / Watches list (only the rows in view are built).
    pub(super) fn debug_var_row(&self, i: usize, focused: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let d = &self.debug;
        let Some(r) = d.vars.get(i) else { return div().into_any_element() };
        let paused = d.paused_thread().is_some();
        let edit = d.edit.clone();
        let input = Self::var_input;
        let arrow = crate::icons::tree_arrow(r.var.reference > 0, r.expanded, Theme::text_dim());
        let header = matches!(r.kind, RowKind::Scope | RowKind::WatchHeader);
        let watch = matches!(r.kind, RowKind::Watch(_));
        let editing_name = matches!((r.kind, edit.as_ref().map(|e| e.target)), (RowKind::Watch(w), Some(EditTarget::Watch(x))) if w == x);
        let editing_value = edit.as_ref().is_some_and(|e| e.target == EditTarget::Value(i));
        let error = r.var.value.starts_with("<error: ") && watch;
        let returned = r.return_label();
        let name_color = if header {
            Theme::text_muted()
        } else if watch {
            Theme::accent()
        } else if returned.is_some() {
            Theme::syntax(brisk_syntax::Highlight::Function)
        } else {
            Theme::syntax(brisk_syntax::Highlight::Property)
        };
        let value: gpui::AnyElement = if editing_value {
            input(&edit.as_ref().map(|e| e.text.clone()).unwrap_or_default(), focused).into_any_element()
        } else if watch && r.var.value.is_empty() && !paused {
            div().text_color(Theme::text_dim()).child("not available: the program is not paused").into_any_element()
        } else {
            {
                // One line: a DataFrame's or array's repr spans lines, and a row must not.
                let flat: String = if r.var.value.contains('\n') { r.var.value.split_whitespace().collect::<Vec<_>>().join(" ") } else { r.var.value.clone() };
                div().text_color(if error { Theme::error() } else { Theme::text() }).child(flat.chars().take(300).collect::<String>()).into_any_element()
            }
        };
        div()
            .id(("dbg-var", i))
            .w_full()
            .h(ui_px(21.))
            .pl(px(8. + 16. * r.depth as f32))
            .pr(px(8.))
            .flex()
            .items_center()
            .gap(px(4.))
            .whitespace_nowrap()
            .overflow_hidden()
            .when(r.var.reference > 0, |x| x.cursor_pointer())
            .when(d.sel == Some(i), |x| x.bg(if focused { Theme::selection_row() } else { Theme::hover_row() }))
            .when(d.sel != Some(i), |x| x.hover(|s| s.bg(Theme::hover_row())))
            .child(arrow)
            .child(if editing_name {
                input(&edit.as_ref().map(|e| e.text.clone()).unwrap_or_default(), focused).into_any_element()
            } else {
                div().text_color(name_color).child(returned.clone().unwrap_or_else(|| r.var.name.clone())).into_any_element()
            })
            .when(!header, |x| {
                x.child(div().text_color(Theme::text_dim()).child("="))
                    .when(!r.var.type_name.is_empty(), |x| x.child(div().text_color(Theme::text_dim()).child(format!("{{{}}}", r.var.type_name))))
                    .child(div().flex_1().min_w_0().overflow_hidden().child(value))
            })
            // PyCharm's "...View as DataFrame" link at the end of a frame or array row.
            .when_some(viewable(&r.var.type_name).filter(|_| paused && !header), |x, label| {
                x.child(
                    div()
                        .id(("dbg-view-data", i))
                        .flex_shrink_0()
                        .ml(px(8.))
                        .font_family(crate::theme::ui_font())
                        .text_size(px(12.))
                        .text_color(Theme::accent())
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .child(label)
                        .on_click(cx.listener(move |this, _, w, cx| {
                            cx.stop_propagation();
                            this.debug.sel = Some(i);
                            this.view_as_data(i, w, cx);
                        })),
                )
            })
            .on_mouse_down(
                gpui::MouseButton::Right,
                cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                    this.debug.sel = Some(i);
                    this.var_menu(i, e.position, cx);
                }),
            )
            .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, _, cx| {
                if this.debug.edit.as_ref().is_some_and(|e| e.target != EditTarget::Value(i)) {
                    this.commit_edit(cx);
                }
                if ev.click_count() >= 2 {
                    this.begin_edit(i);
                } else {
                    this.toggle_var(i, cx);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    /// The in-place editor of a watch name, a value or a new watch.
    pub(super) fn var_input(text: &str, focused: bool) -> gpui::Div {
        let caret = || div().w(px(1.5)).h(ui_px(14.)).bg(Theme::caret());
        div()
            .flex()
            .items_center()
            .px(px(3.))
            .border_1()
            .border_color(Theme::accent())
            .rounded(px(3.))
            .child(text.to_string())
            .when(focused, |x| x.child(caret()))
    }

    /// The "+ new watch" row while it is being typed.
    pub(super) fn new_watch_row(text: &str, focused: bool) -> gpui::AnyElement {
        div()
            .id("dbg-new-watch")
            .h(ui_px(21.))
            .pl(px(8. + 16.))
            .flex()
            .items_center()
            .gap(px(4.))
            .child(div().w(px(12.)).text_color(Theme::text_dim()).child("+"))
            .child(Self::var_input(text, focused))
            .into_any_element()
    }

    pub(super) fn render_debugger_tab(&mut self, _window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let threads = self.render_threads(cx);
        let d = &self.debug;
        let frames = div()
            .id("dbg-frames")
            .w(px(300.))
            .h_full()
            .flex_shrink_0()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(Theme::divider())
            .children(threads)
            .when(d.frames.is_empty(), |x| {
                x.child(div().p(px(8.)).text_color(Theme::text_dim()).child(if d.paused_thread().is_some() {
                    "Loading…"
                } else {
                    "Frames appear when the program is paused"
                }))
            })
            .children(d.frames.iter().enumerate().map(|(i, f)| {
                let file = f.path.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let mine = f.path.as_ref().is_some_and(|p| p.starts_with(&self.root));
                div()
                    .id(("dbg-frame", i))
                    .h(ui_px(22.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .cursor_pointer()
                    .when(i == d.frame, |x| x.bg(Theme::selection_row()))
                    .when(i != d.frame, |x| x.hover(|s| s.bg(Theme::hover_row())))
                    .text_color(if mine { Theme::text() } else { Theme::text_muted() })
                    .child(f.name.clone())
                    .child(div().text_color(Theme::text_dim()).text_size(ui_px(12.)).child(format!("{file}:{}", f.line + 1)))
                    .on_click(cx.listener(move |this, _, w, cx| {
                        this.select_frame(i, w, cx);
                        cx.notify();
                    }))
            }));
        let paused = d.paused_thread().is_some();
        let edit = d.edit.clone();
        // The new watch being typed shows as a row before the first scope.
        let new_watch = edit.as_ref().filter(|e| e.target == EditTarget::NewWatch).map(|e| (d.watch_len().min(d.vars.len()), e.text.clone()));
        let n_rows = d.vars.len() + new_watch.is_some() as usize;
        let sel = d.sel;
        let sel_kind = sel.and_then(|s| d.vars.get(s)).map(|r| (r.kind, r.parent));
        let can_remove = matches!(sel_kind, Some((RowKind::Watch(_), _)));
        let can_set = matches!(sel_kind, Some((RowKind::Var, p)) if p > 0) && paused;
        let can_copy = matches!(sel_kind, Some((RowKind::Var | RowKind::Watch(_), _)));
        let mini =
            |id: &'static str, label: &'static str, tip: &'static str, on: bool, f: fn(&mut Workspace, &mut Context<Workspace>), cx: &mut Context<Self>| {
                div()
                    .id(id)
                    .px(px(6.))
                    .h(ui_px(20.))
                    .flex()
                    .items_center()
                    .rounded(px(4.))
                    .text_size(ui_px(12.))
                    .text_color(if on { Theme::text() } else { Theme::text_dim() })
                    .when(on, |x| x.cursor_pointer().hover(|s| s.bg(Theme::hover_row())).active(|s| s.bg(Theme::press())))
                    .child(label)
                    .tooltip(move |_, cx| cx.new(|_| crate::workspace::Tip(tip.into())).into())
                    .on_click(cx.listener(move |this, _, w, cx| {
                        if on {
                            f(this, cx);
                            w.focus(&this.debug.focus.clone());
                            cx.notify();
                        }
                    }))
            };
        let bar = div()
            .flex()
            .items_center()
            .gap(px(2.))
            .px(px(4.))
            .h(ui_px(24.))
            .border_b_1()
            .border_color(Theme::divider())
            .child(mini(
                "dbg-watch-add",
                "+ Watch",
                "New Watch  Insert",
                true,
                |t, _| t.debug.edit = Some(VarEdit { target: EditTarget::NewWatch, text: String::new() }),
                cx,
            ))
            .child(mini(
                "dbg-watch-remove",
                "− Watch",
                "Remove Watch  Delete",
                can_remove,
                |t, cx| {
                    if let Some(RowKind::Watch(w)) = t.debug.sel.and_then(|s| t.debug.vars.get(s)).map(|r| r.kind) {
                        t.remove_watch(w, cx);
                    }
                },
                cx,
            ))
            .child(mini(
                "dbg-set-value",
                "Set Value",
                "Set Value  F2",
                can_set,
                |t, _| {
                    if let Some(s) = t.debug.sel {
                        t.begin_edit(s);
                    }
                },
                cx,
            ))
            .child(mini(
                "dbg-copy-value",
                "Copy Value",
                "Copy Value  Ctrl+C",
                can_copy,
                |t, cx| {
                    if let Some(s) = t.debug.sel {
                        t.copy_value(s, cx);
                    }
                },
                cx,
            ));
        let vars = div().id("dbg-vars").flex_1().min_w_0().h_full().flex().flex_col().child(bar).child(if n_rows == 0 {
            div()
                .flex_1()
                .min_h_0()
                .font_family(mono_font())
                .text_size(ui_px(12.5))
                .child(div().p(px(8.)).text_color(Theme::text_dim()).child(if paused {
                    "Loading…"
                } else {
                    "Variables appear when the program is paused. Insert adds a watch."
                }))
                .into_any_element()
        } else {
            div()
                .id("dbg-vars-list")
                .relative()
                .flex_1()
                .min_h_0()
                .font_family(mono_font())
                .text_size(ui_px(12.5))
                .child(
                    uniform_list(
                        "dbg-vars-rows",
                        n_rows,
                        cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                            let focused = this.debug.focus.is_focused(window);
                            let nw = this
                                .debug
                                .edit
                                .as_ref()
                                .filter(|e| e.target == EditTarget::NewWatch)
                                .map(|e| (this.debug.watch_len().min(this.debug.vars.len()), e.text.clone()));
                            range
                                .map(|j| match &nw {
                                    Some((at, text)) if j == *at => Self::new_watch_row(text, focused),
                                    Some((at, _)) if j > *at => this.debug_var_row(j - 1, focused, cx),
                                    _ => this.debug_var_row(j, focused, cx),
                                })
                                .collect()
                        }),
                    )
                    .size_full()
                    .track_scroll(d.vars_scroll.clone()),
                )
                .child(crate::scrollbar::scrollbar(crate::scrollbar::Source::list(&d.vars_scroll), d.vars_bar.clone()))
                .into_any_element()
        });
        let banner = d.exception.clone().map(|e| {
            div()
                .flex_shrink_0()
                .px(px(8.))
                .py(px(3.))
                .flex()
                .items_center()
                .gap(px(6.))
                .bg(Theme::warning_bg())
                .border_b_1()
                .border_color(Theme::divider())
                .text_color(Theme::error())
                .child(div().font_weight(gpui::FontWeight::BOLD).child("Exception"))
                .child(div().whitespace_nowrap().overflow_hidden().child(e))
        });
        div().size_full().flex().flex_col().children(banner).child(div().flex_1().min_h_0().flex().child(frames).child(vars)).into_any_element()
    }

    /// One Debug Console line (the list builds only the lines in view, links included).
    pub(super) fn debug_console_line(&self, ln: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some((t, k)) = self.debug.console.get(ln) else { return div().into_any_element() };
        let color = match k {
            Line::Out => Theme::text(),
            Line::Err => Theme::error(),
            Line::Info => Theme::text_muted(),
            Line::Input => Theme::accent(),
            Line::Result => Theme::syntax(brisk_syntax::Highlight::String),
        };
        let line = t.trim_end_matches('\n');
        let chars: Vec<char> = line.chars().collect();
        let mut pieces: Vec<gpui::AnyElement> = Vec::new();
        let mut at = 0;
        if matches!(k, Line::Out | Line::Err) {
            for (j, l) in brisk_run::find_links(line).into_iter().enumerate() {
                let Some((path, n)) = self.console_link(&l.path, l.line) else { continue };
                if l.cols.start > at {
                    pieces.push(div().child(chars[at..l.cols.start].iter().collect::<String>()).into_any_element());
                }
                pieces.push(
                    div()
                        .id(("dbg-link", ln * 64 + j))
                        .text_color(Theme::accent())
                        .underline()
                        .cursor_pointer()
                        .child(chars[l.cols.clone()].iter().collect::<String>())
                        .on_click(cx.listener(move |this, _, w, cx| this.open_console_link(&path, n, w, cx)))
                        .into_any_element(),
                );
                at = l.cols.end;
            }
        }
        if at < chars.len() || pieces.is_empty() {
            pieces.push(div().child(chars[at.min(chars.len())..].iter().collect::<String>()).into_any_element());
        }
        div().px(px(8.)).min_h(ui_px(17.)).flex().text_color(color).whitespace_nowrap().children(pieces).into_any_element()
    }

    pub(super) fn render_console_tab(&mut self, window: &Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        self.debug.sync_console();
        let d = &self.debug;
        let focused = d.focus.is_focused(window);
        let can_eval = d.paused_thread().is_some();
        let lines = div()
            .id("dbg-console")
            .flex_1()
            .min_h_0()
            .py(px(4.))
            .font_family(mono_font())
            .text_size(ui_px(12.5))
            .child(list(d.console_out.clone(), cx.processor(|this, ln: usize, _window, cx| this.debug_console_line(ln, cx))).size_full());
        let input = div()
            .h(ui_px(26.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .border_t_1()
            .border_color(Theme::divider())
            .font_family(mono_font())
            .text_size(ui_px(12.5))
            .child(div().text_color(Theme::accent()).child(">>>"))
            .child(if d.input.is_empty() && !focused {
                div()
                    .text_color(Theme::text_dim())
                    .child(if can_eval {
                        "Evaluate an expression in the selected frame (Alt+F8)"
                    } else {
                        "Expressions can be evaluated while the program is paused"
                    })
                    .into_any_element()
            } else {
                div()
                    .flex()
                    .items_center()
                    .child(d.input.clone())
                    .when(focused, |x| x.child(div().w(px(1.5)).h(ui_px(15.)).bg(Theme::caret())))
                    .into_any_element()
            });
        let popup = d.completion.as_ref().map(|c| {
            let shown = c.shown(&d.input);
            let first = c.sel.saturating_sub(7);
            let left = 8. + 30. + 7.5 * d.input[..c.start.min(d.input.len())].chars().count() as f32;
            div()
                .absolute()
                .bottom(px(28.))
                .left(px(left))
                .min_w(px(220.))
                .max_w(px(480.))
                .py(px(2.))
                .bg(Theme::panel())
                .border_1()
                .border_color(Theme::divider())
                .rounded(px(4.))
                .shadow_md()
                .font_family(mono_font())
                .text_size(ui_px(12.5))
                .occlude()
                .children(shown.into_iter().enumerate().skip(first).take(8).map(|(j, (label, kind))| {
                    div()
                        .h(ui_px(20.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .when(j == c.sel, |x| x.bg(Theme::selection_row()))
                        .child(div().flex_1().whitespace_nowrap().child(label.clone()))
                        .child(div().text_color(Theme::text_dim()).text_size(ui_px(11.)).child(kind.clone()))
                }))
        });
        div().size_full().relative().flex().flex_col().child(lines).child(input).children(popup).into_any_element()
    }

    // --- Smoke --------------------------------------------------------------------------
}
