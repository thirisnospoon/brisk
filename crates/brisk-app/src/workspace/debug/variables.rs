//! Threads, frames, variables and watches: loading, editing, copying and viewing values.

use super::*;

impl Workspace {
    /// The thread list for the chooser above Frames.
    pub(super) fn load_threads(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.debug.read(cx).session else { return };
        let rx = s.client.request("threads", json!({}));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(v)) = rx.await else { return };
            let threads: Vec<(i64, String)> = v
                .get("threads")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|t| Some((t.get("id")?.as_i64()?, t.get("name").and_then(Value::as_str).unwrap_or("").to_string()))).collect())
                .unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                if this.debug.read(cx).paused_thread().is_some() {
                    this.debug.as_mut(cx).threads = threads;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Another thread in the chooser: its stack and variables; steps and Resume go to it.
    pub(in crate::workspace) fn select_thread(&mut self, id: i64, window: &mut Window, cx: &mut Context<Self>) {
        {
            let mut d = self.debug.as_mut(cx);
            d.threads_open = false;
            let Some(s) = d.session.as_mut().filter(|s| matches!(s.state, State::Paused(_))) else { return };
            s.state = State::Paused(id);
            d.frames.clear();
        }
        self.load_frames(id, window, cx);
        cx.notify();
    }

    pub(super) fn render_threads(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let d = self.debug.read(cx);
        let cur = d.paused_thread()?;
        if d.threads.len() < 2 {
            return None;
        }
        let name = |id: i64| d.threads.iter().find(|t| t.0 == id).map(|t| t.1.clone()).unwrap_or_else(|| format!("Thread {id}"));
        let head = div()
            .id("dbg-thread")
            .h(ui_px(24.))
            .px(px(8.))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .border_b_1()
            .border_color(Theme::divider())
            .hover(|s| s.bg(Theme::hover_row()))
            .child(div().text_color(Theme::text_dim()).child("Thread"))
            .child(div().flex_1().child(name(cur)))
            .child(crate::icons::ui_icon(if d.threads_open { "chevron-up" } else { "chevron-down" }, 12., Theme::text_dim()))
            .on_click(cx.listener(|this, _, _, cx| {
                this.debug.as_mut(cx).threads_open = !this.debug.read(cx).threads_open;
                cx.notify();
            }));
        let list = d.threads.iter().map(|(id, n)| {
            let id = *id;
            div()
                .id(("dbg-thread-row", id as u64))
                .h(ui_px(22.))
                .pl(px(20.))
                .flex()
                .items_center()
                .cursor_pointer()
                .when(id == cur, |x| x.bg(Theme::selection_row()))
                .when(id != cur, |x| x.hover(|s| s.bg(Theme::hover_row())))
                .child(n.clone())
                .on_click(cx.listener(move |this, _, w, cx| this.select_thread(id, w, cx)))
        });
        Some(div().flex().flex_col().child(head).when(d.threads_open, |x| x.children(list).child(div().h(ui_px(1.)).bg(Theme::divider()))).into_any_element())
    }

    pub(super) fn load_frames(&mut self, thread: i64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(s) = &self.debug.read(cx).session else { return };
        let rx = s.client.request("stackTrace", json!({"threadId": thread, "startFrame": 0, "levels": 100}));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(v)) = rx.await else { return };
            let frames = brisk_dap::parse_frames(&v);
            let _ = this.update_in(cx, |this, window, cx| {
                this.debug.as_mut(cx).frames = frames;
                this.select_frame(0, window, cx);
            });
        })
        .detach();
    }

    /// Show a frame: its line in the editor, its variables.
    pub(in crate::workspace) fn select_frame(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.debug.read(cx).frames.get(i).cloned() else { return };
        self.debug.as_mut(cx).frame = i;
        self.clear_exec_marks(cx);
        if let Some(p) = f.path.clone().filter(|p| p.is_file()) {
            let focus_back = self.debug.read(cx).focus.is_focused(window);
            self.open_file(&p, window, cx);
            if let Some(ed) = self.active_editor().cloned() {
                ed.update(cx, |e, cx| {
                    e.exec_line = Some(f.line);
                    e.go_to_line(f.line, cx);
                });
            }
            self.debug.as_mut(cx).marked.push(p);
            if focus_back {
                window.focus(&self.debug.read(cx).focus.clone());
            }
        }
        self.debug.as_mut(cx).idle_watches();
        self.evaluate_watches(cx);
        self.refresh_data_views(cx);
        let Some(s) = &self.debug.read(cx).session else { return };
        let rx = s.client.request("scopes", json!({"frameId": f.id}));
        let client = s.client.clone();
        let frame_path = f.path.clone();
        let frame_line = f.line;
        cx.spawn(async move |this, cx| {
            let Ok(Ok(v)) = rx.await else { return };
            let scopes = brisk_dap::parse_scopes(&v);
            // The first scope (locals) opens right away, like PyCharm's flat list.
            let mut first = match scopes.first() {
                Some(s) => client
                    .request("variables", json!({"variablesReference": s.reference}))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .map(|v| brisk_dap::parse_variables(&v)),
                None => None,
            };
            // Values just returned by calls first.
            if let Some(v) = first.as_mut() {
                v.sort_by_key(|v| return_label(&v.name).is_none());
            }
            let _ = this.update(cx, |this, cx| {
                let mut rows = Vec::new();
                for (k, s) in scopes.into_iter().enumerate() {
                    let open = k == 0 && first.is_some();
                    let parent = s.reference;
                    rows.push(VarRow { depth: 0, var: s, expanded: open, kind: RowKind::Scope, parent: 0 });
                    if open {
                        rows.extend(first.clone().unwrap_or_default().into_iter().map(|v| VarRow::var(1, v, parent)));
                    }
                }
                let n = this.debug.read(cx).watch_len();
                this.debug.as_mut(cx).vars.truncate(n);
                this.debug.as_mut(cx).vars.extend(rows);
                if let (Some(p), Some(vars)) = (frame_path.as_ref(), first.as_ref()) {
                    if let Some(ed) = this.editors().find(|e| e.read(cx).path.as_ref() == Some(p)).cloned() {
                        let lines: Vec<String> =
                            (0..=frame_line.min(ed.read(cx).buffer.len_lines().saturating_sub(1))).map(|l| ed.read(cx).buffer.line(l)).collect();
                        let values = inline_values(&lines, frame_line, vars);
                        ed.update(cx, |e, cx| {
                            e.set_inline_values(values);
                            cx.notify();
                        });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Expand or collapse a variable row.
    pub(in crate::workspace) fn toggle_var(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(row) = self.debug.read(cx).vars.get(i).cloned() else { return };
        self.debug.as_mut(cx).sel = Some(i);
        if row.var.reference <= 0 {
            return;
        }
        if row.expanded {
            let end =
                (i + 1..self.debug.read(cx).vars.len()).find(|&j| self.debug.read(cx).vars[j].depth <= row.depth).unwrap_or(self.debug.read(cx).vars.len());
            self.debug.as_mut(cx).vars.drain(i + 1..end);
            self.debug.as_mut(cx).vars[i].expanded = false;
            cx.notify();
            return;
        }
        let Some(s) = &self.debug.read(cx).session else { return };
        let rx = s.client.request("variables", json!({"variablesReference": row.var.reference}));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(v)) = rx.await else { return };
            let kids = brisk_dap::parse_variables(&v);
            let _ = this.update(cx, |this, cx| {
                if this.debug.read(cx).vars.get(i).map(|r| r.var.reference) != Some(row.var.reference) {
                    return;
                }
                this.debug.as_mut(cx).vars[i].expanded = true;
                let at = i + 1;
                for (k, v) in kids.into_iter().enumerate() {
                    this.debug.as_mut(cx).vars.insert(at + k, VarRow::var(row.depth + 1, v, row.var.reference));
                }
                cx.notify();
            });
        })
        .detach();
    }

    // --- Value on hover ---------------------------------------------------------------

    /// Mouse rest on a name while paused: `name = {type} value` and its first children, in
    /// the editor's hover popup. False when the debugger is not paused here (docs instead).
    pub(in crate::workspace) fn debug_hover(&mut self, ed: &Entity<Editor>, pos: usize, at: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) -> bool {
        let frame = self.debug.read(cx).frames.get(self.debug.read(cx).frame).map(|f| f.id);
        let live = self.debug.read(cx).session.as_ref().filter(|s| matches!(s.state, State::Paused(_)));
        let (Some(s), Some(frame)) = (live, frame) else { return false };
        let e = ed.read(cx);
        if !e.is_python() || pos >= e.buffer.len_chars() {
            return false;
        }
        let (line, col) = e.buffer.pos_to_line_col(pos);
        let Some(expr) = hover_expression(&e.buffer.line(line), col) else { return false };
        let client = s.client.clone();
        let ed = ed.clone();
        cx.spawn(async move |this, cx| {
            let res = client.request("evaluate", json!({"expression": expr, "frameId": frame, "context": "hover"})).await;
            let Ok(Ok(v)) = res else {
                // Not a value here (a module, an unknown name): the documentation.
                let _ = this.update(cx, |this, cx| this.quick_doc(ed, Some((pos, at)), cx));
                return;
            };
            let (value, reference) = brisk_dap::parse_evaluate(&v);
            let ty = v.get("type").and_then(Value::as_str).unwrap_or("");
            let mut text = if ty.is_empty() { format!("{expr} = {value}") } else { format!("{expr} = {{{ty}}} {value}") };
            if reference > 0 {
                if let Ok(Ok(kids)) = client.request("variables", json!({"variablesReference": reference})).await {
                    // debugpy's "special variables" / "function variables" groups are left out.
                    let kids: Vec<Variable> = brisk_dap::parse_variables(&kids).into_iter().filter(|k| !k.name.contains(' ')).collect();
                    for k in kids.iter().take(30) {
                        let v: String = k.value.chars().take(120).collect();
                        text.push_str(&if k.type_name.is_empty() {
                            format!("\n    {} = {v}", k.name)
                        } else {
                            format!("\n    {} = {{{}}} {v}", k.name, k.type_name)
                        });
                    }
                    if kids.len() > 30 {
                        text.push_str(&format!("\n    … {} more", kids.len() - 30));
                    }
                }
            }
            let md = format!("```\n{text}\n```");
            let _ = ed.update(cx, |e, cx| e.show_doc(&md, crate::editor::Anchor::Mouse(pos, at), cx));
        })
        .detach();
        true
    }

    // --- Watches, Set Value, Copy Value ---------------------------------------------------

    /// Evaluate every watch in the selected frame (on each stop and after changes).
    pub(super) fn evaluate_watches(&mut self, cx: &mut Context<Self>) {
        let frame = self.debug.read(cx).frames.get(self.debug.read(cx).frame).map(|f| f.id);
        let live = self.debug.read(cx).session.as_ref().filter(|s| matches!(s.state, State::Paused(_)));
        let (Some(s), Some(frame)) = (live, frame) else {
            let rows = self.debug.read(cx).watch_section(None);
            self.debug.as_mut(cx).set_watch_rows(rows);
            return;
        };
        let client = s.client.clone();
        let exprs = self.debug.read(cx).watches.clone();
        cx.spawn(async move |this, cx| {
            let mut vals = Vec::new();
            for e in &exprs {
                let r = client.request("evaluate", json!({"expression": e, "frameId": frame, "context": "watch"})).await;
                vals.push(match r {
                    Ok(Ok(v)) => {
                        let (value, reference) = brisk_dap::parse_evaluate(&v);
                        let type_name = v.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                        Ok(Variable { name: e.clone(), value, type_name, reference, evaluate_name: e.clone() })
                    }
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("no reply".into()),
                });
            }
            let _ = this.update(cx, |this, cx| {
                // Stale: the watches or the frame changed meanwhile.
                if this.debug.read(cx).watches != exprs || this.debug.read(cx).frames.get(this.debug.read(cx).frame).map(|f| f.id) != Some(frame) {
                    return;
                }
                let rows = this.debug.read(cx).watch_section(Some(vals));
                this.debug.as_mut(cx).set_watch_rows(rows);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn watches_changed(&mut self, cx: &mut Context<Self>) {
        save_watches(&self.root, &self.debug.read(cx).watches);
        self.evaluate_watches(cx);
        cx.notify();
    }

    pub(in crate::workspace) fn add_watch(&mut self, expr: String, cx: &mut Context<Self>) {
        let expr = expr.trim().to_string();
        if expr.is_empty() {
            return;
        }
        if !self.debug.read(cx).watches.contains(&expr) {
            self.debug.as_mut(cx).watches.push(expr);
        }
        self.watches_changed(cx);
    }

    pub(in crate::workspace) fn remove_watch(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.debug.read(cx).watches.len() {
            self.debug.as_mut(cx).watches.remove(i);
            self.watches_changed(cx);
        }
    }

    pub(super) fn edit_watch(&mut self, i: usize, expr: String, cx: &mut Context<Self>) {
        let expr = expr.trim().to_string();
        if expr.is_empty() {
            return self.remove_watch(i, cx);
        }
        let found = self.debug.as_mut(cx).watches.get_mut(i).map(|w| *w = expr).is_some();
        if found {
            self.watches_changed(cx);
        }
    }

    pub(in crate::workspace) fn commit_edit(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.debug.as_mut(cx).edit.take() else { return };
        match ed.target {
            EditTarget::NewWatch => self.add_watch(ed.text, cx),
            EditTarget::Watch(w) => self.edit_watch(w, ed.text, cx),
            EditTarget::Value(i) => self.set_value(i, ed.text, cx),
        }
        cx.notify();
    }

    /// Set Value (DAP setVariable) of a variable row.
    pub(in crate::workspace) fn set_value(&mut self, i: usize, value: String, cx: &mut Context<Self>) {
        let Some(row) = self.debug.read(cx).vars.get(i).cloned().filter(|r| r.kind == RowKind::Var && r.parent > 0) else { return };
        let Some(s) = self.debug.read(cx).session.as_ref().filter(|s| matches!(s.state, State::Paused(_))) else { return };
        let rx = s.client.request("setVariable", json!({"variablesReference": row.parent, "name": row.var.name, "value": value}));
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(Ok(v)) => {
                        let at = if this.debug.read(cx).vars.get(i).is_some_and(|r| r.parent == row.parent && r.var.name == row.var.name) {
                            Some(i)
                        } else {
                            this.debug.read(cx).vars.iter().position(|r| r.parent == row.parent && r.var.name == row.var.name)
                        };
                        if let Some(at) = at {
                            if this.debug.read(cx).vars[at].expanded {
                                // Collapse: the children are of the old value.
                                this.toggle_var(at, cx);
                            }
                            let r = &mut this.debug.as_mut(cx).vars[at];
                            r.var.value = v.get("value").and_then(Value::as_str).unwrap_or("").to_string();
                            if let Some(t) = v.get("type").and_then(Value::as_str) {
                                r.var.type_name = t.to_string();
                            }
                            r.var.reference = v.get("variablesReference").and_then(Value::as_i64).unwrap_or(0);
                        }
                        this.evaluate_watches(cx);
                    }
                    Ok(Err(e)) => {
                        this.status = format!("Cannot set value: {e}");
                        this.debug.as_mut(cx).info(format!("Cannot set {}: {e}", row.var.name));
                    }
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workspace) fn copy_value(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(r) = self.debug.read(cx).vars.get(i) else { return };
        if matches!(r.kind, RowKind::Scope | RowKind::WatchHeader) {
            return;
        }
        let (name, shown) = (r.var.name.clone(), r.var.value.clone());
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(shown));
        self.status = format!("Copied the value of {name}");
        // The tree shows debugpy's short repr: replace it with the whole value when it comes.
        self.full_value(i, cx, move |this, full, cx| {
            if let Ok(full) = full {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(full));
                this.status = format!("Copied the value of {name}");
            }
        });
    }

    /// The whole value of a row (str() of a string, repr() of anything else), not the
    /// tree's truncated repr: evaluated in the program, hex-encoded so no repr mangles it.
    pub(super) fn full_value(&mut self, i: usize, cx: &mut Context<Self>, done: impl FnOnce(&mut Self, Result<String, String>, &mut Context<Self>) + 'static) {
        let Some(expr) = self.debug.read(cx).row_expression(i) else { return };
        let Some(s) = self.debug.read(cx).session.as_ref().filter(|s| matches!(s.state, State::Paused(_))) else { return };
        let mut args = json!({"expression": full_value_expression(&expr), "context": "clipboard"});
        if let Some(f) = self.debug.read(cx).frames.get(self.debug.read(cx).frame) {
            args["frameId"] = json!(f.id);
        }
        let rx = s.client.request("evaluate", args);
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                let v = match res {
                    Ok(Ok(v)) => decode_hex_repr(v.get("result").and_then(Value::as_str).unwrap_or("")).ok_or_else(|| "unexpected reply".to_string()),
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err("no reply".into()),
                };
                done(this, v, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(in crate::workspace) fn copy_as_expression(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(expr) = self.debug.read(cx).row_expression(i) else { return };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(expr.clone()));
        self.status = format!("Copied {expr}");
    }

    pub(in crate::workspace) fn add_row_to_watches(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(expr) = self.debug.read(cx).row_expression(i) else { return };
        if !self.debug.read(cx).watches.contains(&expr) {
            self.add_watch(expr, cx);
        }
    }

    /// View: the whole value in an editor tab (a long string, a big repr).
    pub(in crate::workspace) fn view_value(&mut self, i: usize, cx: &mut Context<Self>) {
        let Some(expr) = self.debug.read(cx).row_expression(i) else { return };
        self.full_value(i, cx, move |this, full, cx| match full {
            Ok(text) => {
                let ed = cx.new(|cx| {
                    let mut e = Editor::scratch(&text, cx);
                    e.title_override = Some(format!("{expr} (value)"));
                    e
                });
                this.add_tab(Tab::Editor(ed));
            }
            Err(e) => this.status = format!("Cannot view {expr}: {e}"),
        });
    }

    /// View as DataFrame / View as Array: the row's value in a table tab, paged from the program.
    pub(in crate::workspace) fn view_as_data(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(expr) = self.debug.read(cx).row_expression(i) else { return };
        let Some(s) = self.debug.read(cx).session.as_ref().filter(|s| matches!(s.state, State::Paused(_))) else { return };
        let client = s.client.clone();
        let frame = self.debug.read(cx).frames.get(self.debug.read(cx).frame).map(|f| f.id);
        // One tab per expression: View again shows it, evaluated anew.
        let existing = self.all_tabs().find_map(|t| match t {
            Tab::Data(v) if v.read(cx).expr == expr => Some(v.clone()),
            _ => None,
        });
        let v = match existing {
            Some(v) => {
                v.update(cx, |v, cx| v.refresh(client, frame, cx));
                v
            }
            None => {
                let v = cx.new(|cx| DataView::new(expr, client, frame, cx));
                self.add_tab(Tab::Data(v.clone()));
                v
            }
        };
        if let Some(at) = self.tabs.iter().position(|t| matches!(t, Tab::Data(x) if *x == v)) {
            self.active = Some(at);
        }
        window.focus(&gpui::Focusable::focus_handle(v.read(cx), cx));
        cx.notify();
    }

    pub(super) fn data_views(&self) -> Vec<Entity<DataView>> {
        self.all_tabs()
            .filter_map(|t| match t {
                Tab::Data(v) => Some(v.clone()),
                _ => None,
            })
            .collect()
    }

    /// A stop or another frame: open tables evaluate their expressions there again.
    pub(super) fn refresh_data_views(&mut self, cx: &mut Context<Self>) {
        let Some(s) = self.debug.read(cx).session.as_ref().filter(|s| matches!(s.state, State::Paused(_))) else { return };
        let client = s.client.clone();
        let frame = self.debug.read(cx).frames.get(self.debug.read(cx).frame).map(|f| f.id);
        for v in self.data_views() {
            v.update(cx, |v, cx| v.refresh(client.clone(), frame, cx));
        }
    }

    pub(super) fn stale_data_views(&mut self, why: &str, cx: &mut Context<Self>) {
        for v in self.data_views() {
            v.update(cx, |v, cx| v.set_stale(why, cx));
        }
    }

    /// Right click in the variables tree.
    pub(super) fn var_menu(&mut self, i: usize, pos: gpui::Point<gpui::Pixels>, cx: &mut Context<Self>) {
        use crate::workspace::menu_ui::Entry;
        use std::rc::Rc;
        let Some(r) = self.debug.read(cx).vars.get(i).cloned() else { return };
        if matches!(r.kind, RowKind::Scope | RowKind::WatchHeader) {
            return;
        }
        let paused = self.debug.read(cx).paused_thread().is_some();
        let has_expr = self.debug.read(cx).row_expression(i).is_some();
        let item = |label: &str, shortcut: &str, enabled: bool, run: crate::workspace::menu_ui::MenuFn| Entry::Item {
            label: label.into(),
            shortcut: shortcut.into(),
            enabled,
            run,
        };
        let mut v = Vec::new();
        if let Some(label) = viewable(&r.var.type_name) {
            v.push(item(label, "", paused && has_expr, Rc::new(move |this, w, cx| this.view_as_data(i, w, cx))));
            v.push(Entry::Separator);
        }
        v.push(item(
            "Set Value…",
            "F2",
            paused && r.kind == RowKind::Var && r.parent > 0,
            Rc::new(move |this, _, cx| {
                this.debug.as_mut(cx).begin_edit(i);
                cx.notify();
            }),
        ));
        v.push(item("Copy Value", "Ctrl+C", true, Rc::new(move |this, _, cx| this.copy_value(i, cx))));
        v.push(item("Copy as Expression", "", has_expr, Rc::new(move |this, _, cx| this.copy_as_expression(i, cx))));
        v.push(item("View Value", "", paused && has_expr, Rc::new(move |this, _, cx| this.view_value(i, cx))));
        v.push(Entry::Separator);
        if let RowKind::Watch(w) = r.kind {
            v.push(item("Remove Watch", "Delete", true, Rc::new(move |this, _, cx| this.remove_watch(w, cx))));
        } else {
            v.push(item("Add to Watches", "", has_expr, Rc::new(move |this, _, cx| this.add_row_to_watches(i, cx))));
        }
        self.open_menu(pos, v, cx);
    }

    /// Keys in the variables tree (the Debugger tab has focus).
    pub(super) fn vars_key(&mut self, k: &gpui::Keystroke, cx: &mut Context<Self>) -> bool {
        let ctrl = k.modifiers.control || k.modifiers.platform;
        if self.debug.read(cx).edit.is_some() {
            let edit_text = |this: &mut Self, cx: &mut Context<Self>, f: &dyn Fn(&mut String)| {
                if let Some(ed) = this.debug.as_mut(cx).edit.as_mut() {
                    f(&mut ed.text);
                }
            };
            match k.key.as_str() {
                "enter" => self.commit_edit(cx),
                "escape" => self.debug.as_mut(cx).edit = None,
                "backspace" => edit_text(self, cx, &|t| {
                    t.pop();
                }),
                // gpui gives Space no key_char.
                "space" if !ctrl => edit_text(self, cx, &|t| t.push(' ')),
                "v" if ctrl => {
                    if let Some(c) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        edit_text(self, cx, &|t| t.push_str(c.lines().next().unwrap_or("")));
                    }
                }
                _ if ctrl || k.modifiers.alt => return false,
                _ => match k.key_char.as_deref() {
                    Some(s) if !s.chars().any(char::is_control) => edit_text(self, cx, &|t| t.push_str(s)),
                    _ => return false,
                },
            }
            cx.notify();
            return true;
        }
        let n = self.debug.read(cx).vars.len();
        let sel = self.debug.read(cx).sel.filter(|&s| s < n);
        let before = self.debug.read(cx).sel;
        match (k.key.as_str(), ctrl) {
            ("c", true) => {
                if let Some(s) = sel {
                    self.copy_value(s, cx);
                }
            }
            (_, true) => return false,
            ("down", _) if n > 0 => self.debug.as_mut(cx).sel = Some(sel.map_or(0, |s| (s + 1).min(n - 1))),
            ("up", _) if n > 0 => self.debug.as_mut(cx).sel = Some(sel.map_or(0, |s| s.saturating_sub(1))),
            ("right", _) => {
                if let Some(s) = sel.filter(|&s| !self.debug.read(cx).vars[s].expanded) {
                    self.toggle_var(s, cx);
                }
            }
            ("left", _) => {
                if let Some(s) = sel {
                    let r = &self.debug.read(cx).vars[s];
                    if r.expanded && r.var.reference > 0 {
                        self.toggle_var(s, cx);
                    } else if let Some(p) = (0..s).rev().find(|&j| self.debug.read(cx).vars[j].depth < r.depth) {
                        self.debug.as_mut(cx).sel = Some(p);
                    }
                }
            }
            ("enter", _) => {
                if let Some(s) = sel {
                    if self.debug.read(cx).vars[s].var.reference > 0 {
                        self.toggle_var(s, cx);
                    } else {
                        self.debug.as_mut(cx).begin_edit(s);
                    }
                }
            }
            ("f2", _) => {
                if let Some(s) = sel {
                    self.debug.as_mut(cx).begin_edit(s);
                }
            }
            ("insert", _) => self.debug.as_mut(cx).edit = Some(VarEdit { target: EditTarget::NewWatch, text: String::new() }),
            ("delete", _) => {
                if let Some(RowKind::Watch(w)) = sel.map(|s| self.debug.read(cx).vars[s].kind) {
                    self.remove_watch(w, cx);
                }
            }
            _ => return false,
        }
        if self.debug.read(cx).sel != before {
            if let Some(s) = self.debug.read(cx).sel {
                self.debug.read(cx).vars_scroll.scroll_to_item(s, gpui::ScrollStrategy::Top);
            }
        }
        cx.notify();
        true
    }
}

impl DebugModel {
    /// F2 / double click: edit a watch's expression, or set a variable's value.
    pub(in crate::workspace) fn begin_edit(&mut self, i: usize) {
        let Some(r) = self.vars.get(i) else { return };
        self.sel = Some(i);
        self.edit = match r.kind {
            RowKind::Watch(w) => Some(VarEdit { target: EditTarget::Watch(w), text: self.watches.get(w).cloned().unwrap_or_default() }),
            RowKind::WatchHeader => Some(VarEdit { target: EditTarget::NewWatch, text: String::new() }),
            RowKind::Var if r.parent > 0 && self.paused_thread().is_some() => Some(VarEdit { target: EditTarget::Value(i), text: r.var.value.clone() }),
            _ => None,
        };
    }

    /// The expression of a row of the variables tree: a watch's text, or debugpy's evaluateName.
    pub(in crate::workspace) fn row_expression(&self, i: usize) -> Option<String> {
        let r = self.vars.get(i)?;
        match r.kind {
            RowKind::Watch(w) => self.watches.get(w).cloned(),
            RowKind::Var if !r.var.evaluate_name.is_empty() => Some(r.var.evaluate_name.clone()),
            RowKind::Var if r.depth == 1 => Some(r.var.name.clone()),
            _ => None,
        }
    }
}
