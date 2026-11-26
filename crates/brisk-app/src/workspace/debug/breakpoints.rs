//! Breakpoints: the stored list, the gutter, the debugger's copy and the Breakpoints dialog.

use super::*;

impl Workspace {
    // --- Breakpoints ------------------------------------------------------------------

    /// All breakpoints by file (open editors first-hand, the rest stored).
    pub(in crate::workspace) fn all_breakpoints(&self, cx: &gpui::App) -> Vec<(PathBuf, Vec<(usize, Breakpoint)>)> {
        let mut out: Vec<(PathBuf, Vec<(usize, Breakpoint)>)> = Vec::new();
        let mut open = Vec::new();
        for e in self.editors() {
            let e = e.read(cx);
            let Some(p) = e.path.clone() else { continue };
            open.push(p.clone());
            let list = e.breakpoint_list();
            if !list.is_empty() {
                out.push((p, list));
            }
        }
        for b in &self.debug.read(cx).stored {
            if open.contains(&b.path) {
                continue;
            }
            let bp = (b.line, b.breakpoint());
            match out.iter_mut().find(|(p, _)| *p == b.path) {
                Some((_, v)) => v.push(bp),
                None => out.push((b.path.clone(), vec![bp])),
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    pub(in crate::workspace) fn persist_breakpoints(&self, cx: &gpui::App) {
        let path = store_path();
        let mut all: serde_json::Map<String, Value> =
            std::fs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default();
        let mut list = Vec::new();
        for (p, bps) in self.all_breakpoints(cx) {
            let rel = p.strip_prefix(&self.root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            for (line, b) in bps {
                list.push(json!({"path": rel, "line": line, "condition": b.condition, "enabled": b.enabled}));
            }
        }
        let key = self.root.to_string_lossy().to_lowercase();
        if list.is_empty() {
            all.remove(&key);
        } else {
            all.insert(key, Value::Array(list));
        }
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if let Ok(s) = serde_json::to_string_pretty(&Value::Object(all)) {
            let _ = std::fs::write(path, s);
        }
    }

    /// A file was opened: its stored breakpoints move into the editor.
    pub(in crate::workspace) fn adopt_breakpoints(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(p) = ed.read(cx).path.clone() else { return };
        let mine: Vec<(usize, Breakpoint)> = self.debug.read(cx).stored.iter().filter(|b| b.path == p).map(|b| (b.line, b.breakpoint())).collect();
        if mine.is_empty() {
            return;
        }
        self.debug.as_mut(cx).stored.retain(|b| b.path != p);
        let muted = self.debug.read(cx).muted;
        ed.update(cx, |e, cx| {
            e.set_breakpoints(&mine);
            e.set_breakpoints_muted(muted);
            cx.notify();
        });
    }

    /// Mute Breakpoints: grey them in every editor and take them all out of a running session.
    pub(in crate::workspace) fn toggle_mute_breakpoints(&mut self, cx: &mut Context<Self>) {
        self.debug.as_mut(cx).muted = !self.debug.read(cx).muted;
        let muted = self.debug.read(cx).muted;
        for ed in self.editors().cloned().collect::<Vec<_>>() {
            ed.update(cx, |e, cx| {
                e.set_breakpoints_muted(muted);
                cx.notify();
            });
        }
        for (p, _) in self.all_breakpoints(cx) {
            self.send_breakpoints(&p, cx);
        }
        if self.debug.read(cx).session.as_ref().is_some_and(|s| s.state != State::Ended) {
            self.debug.update(cx, |d, cx| d.request("setExceptionBreakpoints", d.exception_args(), cx));
        }
        self.status = if muted { "Breakpoints muted".into() } else { "Breakpoints unmuted".into() };
        cx.notify();
    }

    /// An editor closes: keep its breakpoints.
    pub(in crate::workspace) fn stash_breakpoints(&mut self, ed: &Entity<Editor>, cx: &mut Context<Self>) {
        let Some(p) = ed.read(cx).path.clone() else { return };
        let list = ed.read(cx).breakpoint_list();
        self.debug.as_mut(cx).stored.retain(|b| b.path != p);
        self.debug.as_mut(cx).stored.extend(list.into_iter().map(|(line, b)| StoredBp { path: p.clone(), line, condition: b.condition, enabled: b.enabled }));
    }

    /// A breakpoint changed in an editor: save, and tell a running debugger.
    pub(in crate::workspace) fn breakpoints_changed(&mut self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        self.persist_breakpoints(cx);
        if let Some(p) = ed.read(cx).path.clone() {
            self.send_breakpoints(&p, cx);
        }
    }

    pub(super) fn source_breakpoints(&self, path: &Path, cx: &gpui::App) -> Vec<SourceBreakpoint> {
        let mut v: Vec<SourceBreakpoint> = self
            .all_breakpoints(cx)
            .into_iter()
            .filter(|(p, _)| p == path && !self.debug.read(cx).muted)
            .flat_map(|(_, l)| l.into_iter().filter(|(_, b)| b.enabled).map(|(line, b)| SourceBreakpoint { line, condition: b.condition }))
            .collect();
        if let Some((p, l)) = self.debug.read(cx).session.as_ref().and_then(|s| s.temp.clone()) {
            if p == path && !v.iter().any(|b| b.line == l) {
                v.push(SourceBreakpoint { line: l, condition: None });
            }
        }
        v
    }

    /// setBreakpoints for one file; the reply marks verified breakpoints in its editor.
    pub(super) fn send_breakpoints(&mut self, path: &Path, cx: &mut Context<Self>) {
        if !self.debug.read(cx).session.as_ref().is_some_and(|s| s.state != State::Ended) {
            return;
        }
        let bps = self.source_breakpoints(path, cx);
        self.debug.update(cx, |d, cx| d.set_breakpoints(path, &bps, cx));
    }

    /// Ctrl+F8 at the caret.
    pub(super) fn toggle_breakpoint_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        if !ed.read(cx).can_debug() {
            self.status = "Breakpoints are for Python files".into();
            return;
        }
        let line = ed.read(cx).caret_line_col().0;
        ed.update(cx, |e, cx| e.toggle_breakpoint(line, cx));
    }

    /// Right click on a breakpoint / Edit Breakpoint: the condition popup.
    pub(in crate::workspace) fn edit_breakpoint(&mut self, ed: Entity<Editor>, line: usize, cx: &mut Context<Self>) {
        let cond = ed.read(cx).breakpoint_list().into_iter().find(|(l, _)| *l == line).and_then(|(_, b)| b.condition).unwrap_or_default();
        self.debug_edit = Some((ed, line));
        self.open_popup(PopupKind::BreakpointCondition);
        if let Some(p) = self.popup.as_mut() {
            p.query = cond;
        }
        self.refresh_popup();
        cx.notify();
    }

    pub(in crate::workspace) fn condition_items(&self, q: &str) -> Vec<PopupItem> {
        let q = q.trim();
        let mut v = vec![PopupItem {
            label: if q.is_empty() { "Stop always (no condition)".into() } else { format!("Stop when: {q}") },
            detail: String::new(),
            hint: "Enter".into(),
            target: Target::BpCondition(q.to_string()),
        }];
        v.push(PopupItem { label: "Remove breakpoint".into(), detail: String::new(), hint: String::new(), target: Target::BpRemove });
        v
    }

    pub(in crate::workspace) fn pick_condition(&mut self, target: Target, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ed, line)) = self.debug_edit.take() else { return };
        match target {
            Target::BpCondition(c) => ed.update(cx, |e, cx| e.set_breakpoint_condition(line, Some(c), cx)),
            Target::BpRemove if ed.read(cx).breakpoint_list().iter().any(|(l, _)| *l == line) => {
                ed.update(cx, |e, cx| e.toggle_breakpoint(line, cx));
            }
            _ => {}
        }
        self.focus_editor(window, cx);
    }

    // --- View Breakpoints dialog -------------------------------------------------------

    pub(in crate::workspace) fn bp_rows(&self, cx: &gpui::App) -> Vec<BpRow> {
        let mut v = vec![BpRow::Raised, BpRow::Uncaught];
        v.extend(self.debug.read(cx).exc_classes.iter().cloned().enumerate().map(|(i, c)| BpRow::Class(i, c)));
        for (p, bps) in self.all_breakpoints(cx) {
            v.extend(bps.into_iter().map(|(l, b)| BpRow::Line(p.clone(), l, b)));
        }
        v
    }

    pub(in crate::workspace) fn open_bp_dialog(&mut self, cx: &mut Context<Self>) {
        // The first line breakpoint is selected, as in PyCharm.
        let sel = self.bp_rows(cx).iter().position(|r| matches!(r, BpRow::Line(..))).unwrap_or(0);
        self.debug.as_mut(cx).bp_dialog = Some(BpDialog { sel, editing: false });
        cx.notify();
    }

    pub(super) fn bp_editor(&self, path: &Path, cx: &gpui::App) -> Option<Entity<Editor>> {
        self.editors().find(|e| e.read(cx).path.as_deref() == Some(path)).cloned()
    }

    /// A stored (not open) breakpoint changed: save, tell the debugger.
    pub(super) fn stored_bp_changed(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.persist_breakpoints(cx);
        self.send_breakpoints(path, cx);
        cx.notify();
    }

    pub(in crate::workspace) fn bp_set_enabled(&mut self, path: &Path, line: usize, on: bool, cx: &mut Context<Self>) {
        match self.bp_editor(path, cx) {
            Some(ed) => ed.update(cx, |e, cx| e.set_breakpoint_enabled(line, on, cx)),
            None => {
                if let Some(b) = self.debug.as_mut(cx).stored.iter_mut().find(|b| b.path == path && b.line == line) {
                    b.enabled = on;
                }
                self.stored_bp_changed(path, cx);
            }
        }
    }

    pub(in crate::workspace) fn bp_set_condition(&mut self, path: &Path, line: usize, cond: Option<String>, cx: &mut Context<Self>) {
        let cond = cond.filter(|c| !c.trim().is_empty());
        match self.bp_editor(path, cx) {
            Some(ed) => ed.update(cx, |e, cx| e.set_breakpoint_condition(line, cond, cx)),
            None => {
                if let Some(b) = self.debug.as_mut(cx).stored.iter_mut().find(|b| b.path == path && b.line == line) {
                    b.condition = cond;
                }
                self.stored_bp_changed(path, cx);
            }
        }
    }

    pub(in crate::workspace) fn bp_remove(&mut self, path: &Path, line: usize, cx: &mut Context<Self>) {
        match self.bp_editor(path, cx) {
            Some(ed) => {
                if ed.read(cx).breakpoint_list().iter().any(|(l, _)| *l == line) {
                    ed.update(cx, |e, cx| e.toggle_breakpoint(line, cx));
                }
            }
            None => {
                self.debug.as_mut(cx).stored.retain(|b| !(b.path == path && b.line == line));
                self.stored_bp_changed(path, cx);
            }
        }
    }

    /// The Python Exception Breakpoint checkboxes.
    pub(in crate::workspace) fn set_exception_breakpoints(&mut self, raised: bool, uncaught: bool, cx: &mut Context<Self>) {
        self.debug.as_mut(cx).exc_raised = raised;
        self.debug.as_mut(cx).exc_uncaught = uncaught;
        save_exc(&self.root, raised, uncaught, &self.debug.read(cx).exc_classes);
        if self.debug.read(cx).session.as_ref().is_some_and(|s| s.state != State::Ended) {
            self.debug.update(cx, |d, cx| d.request("setExceptionBreakpoints", d.exception_args(), cx));
        }
        cx.notify();
    }

    /// A class exception breakpoint changed: save and tell a running debugger.
    pub(in crate::workspace) fn bp_update_class(&mut self, i: usize, f: impl FnOnce(&mut ExcClass), cx: &mut Context<Self>) {
        if let Some(c) = self.debug.as_mut(cx).exc_classes.get_mut(i) {
            f(c);
        }
        let (r, u) = (self.debug.read(cx).exc_raised, self.debug.read(cx).exc_uncaught);
        self.set_exception_breakpoints(r, u, cx);
    }

    /// "+ Exception": a new class row, its name field ready for typing.
    pub(in crate::workspace) fn bp_add_class(&mut self, cx: &mut Context<Self>) {
        self.bp_drop_empty_classes(cx);
        self.debug.as_mut(cx).exc_classes.push(ExcClass::default());
        let i = self.debug.read(cx).exc_classes.len() - 1;
        let sel = self.bp_rows(cx).iter().position(|r| matches!(r, BpRow::Class(j, _) if *j == i)).unwrap_or(0);
        if let Some(d) = self.debug.as_mut(cx).bp_dialog.as_mut() {
            d.sel = sel;
            d.editing = true;
        }
        cx.notify();
    }

    pub(in crate::workspace) fn bp_remove_class(&mut self, i: usize, cx: &mut Context<Self>) {
        if i < self.debug.read(cx).exc_classes.len() {
            self.debug.as_mut(cx).exc_classes.remove(i);
        }
        let (r, u) = (self.debug.read(cx).exc_raised, self.debug.read(cx).exc_uncaught);
        self.set_exception_breakpoints(r, u, cx);
    }

    /// A class row left without a name goes away.
    pub(super) fn bp_drop_empty_classes(&mut self, cx: &mut Context<Self>) {
        if self.debug.read(cx).exc_classes.iter().any(|c| c.name.trim().is_empty()) {
            self.debug.as_mut(cx).exc_classes.retain(|c| !c.name.trim().is_empty());
            let (r, u) = (self.debug.read(cx).exc_raised, self.debug.read(cx).exc_uncaught);
            self.set_exception_breakpoints(r, u, cx);
        }
    }

    /// Space / a click on the checkbox.
    pub(super) fn bp_toggle_row(&mut self, row: &BpRow, cx: &mut Context<Self>) {
        let (r, u) = (self.debug.read(cx).exc_raised, self.debug.read(cx).exc_uncaught);
        match row {
            BpRow::Raised => self.set_exception_breakpoints(!r, u, cx),
            BpRow::Uncaught => self.set_exception_breakpoints(r, !u, cx),
            BpRow::Class(i, c) => {
                let on = !c.enabled;
                self.bp_update_class(*i, |c| c.enabled = on, cx);
            }
            BpRow::Line(p, l, b) => self.bp_set_enabled(p, *l, !b.enabled, cx),
        }
    }

    /// Enter / a double click: the breakpoint's line in the editor.
    pub(super) fn bp_go_to(&mut self, row: &BpRow, window: &mut Window, cx: &mut Context<Self>) {
        let BpRow::Line(p, l, _) = row else { return };
        self.debug.as_mut(cx).bp_dialog = None;
        self.open_file(p, window, cx);
        if let Some(ed) = self.active_editor().cloned() {
            ed.update(cx, |e, cx| e.go_to_line(*l, cx));
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// Changes the open Breakpoints dialog's selection or editing state.
    pub(super) fn bp_dialog_set(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut BpDialog)) {
        if let Some(d) = self.debug.as_mut(cx).bp_dialog.as_mut() {
            f(d);
        }
    }

    pub(in crate::workspace) fn bp_dialog_key(&mut self, e: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let rows = self.bp_rows(cx);
        let Some((sel, editing)) = self.debug.as_mut(cx).bp_dialog.as_mut().map(|d| {
            d.sel = d.sel.min(rows.len().saturating_sub(1));
            (d.sel, d.editing)
        }) else {
            return;
        };
        let row = rows.get(sel).cloned();
        let k = &e.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            if k.key == "f8" && k.modifiers.control && k.modifiers.shift {
                self.debug.as_mut(cx).bp_dialog = None;
            }
            cx.notify();
            return;
        }
        let was_editing = editing;
        match k.key.as_str() {
            "escape" | "enter" if editing => self.bp_dialog_set(cx, |d| d.editing = false),
            "escape" => self.debug.as_mut(cx).bp_dialog = None,
            "up" => self.bp_dialog_set(cx, |d| {
                d.editing = false;
                d.sel = sel.saturating_sub(1);
            }),
            "down" => self.bp_dialog_set(cx, |d| {
                d.editing = false;
                d.sel = (sel + 1).min(rows.len().saturating_sub(1));
            }),
            "insert" | "+" if !editing => self.bp_add_class(cx),
            "space" if !editing => {
                if let Some(r) = row {
                    self.bp_toggle_row(&r, cx);
                }
            }
            "space" if matches!(row, Some(BpRow::Class(..))) => {}
            "space" => {
                if let Some(BpRow::Line(p, l, b)) = row {
                    let mut c = b.condition.unwrap_or_default();
                    c.push(' ');
                    self.bp_set_condition(&p, l, Some(c), cx);
                }
            }
            "enter" | "f4" => {
                if let Some(r) = row {
                    self.bp_go_to(&r, window, cx);
                }
            }
            "delete" => match row {
                Some(BpRow::Line(p, l, _)) => self.bp_remove(&p, l, cx),
                Some(BpRow::Class(i, _)) => self.bp_remove_class(i, cx),
                _ => {}
            },
            "backspace" if matches!(row, Some(BpRow::Class(..))) => {
                if let Some(BpRow::Class(i, _)) = row {
                    self.bp_dialog_set(cx, |d| d.editing = true);
                    self.bp_update_class(
                        i,
                        |c| {
                            c.name.pop();
                        },
                        cx,
                    );
                }
            }
            "backspace" => {
                if let Some(BpRow::Line(p, l, b)) = row {
                    let mut c = b.condition.unwrap_or_default();
                    c.pop();
                    self.bp_set_condition(&p, l, Some(c), cx);
                }
            }
            _ => {
                // Typing edits a class name (letters, digits, _ and .).
                if let (Some(BpRow::Class(i, _)), Some(t)) =
                    (&row, k.key_char.as_deref().filter(|t| t.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')))
                {
                    self.bp_dialog_set(cx, |d| d.editing = true);
                    let t = t.to_string();
                    self.bp_update_class(*i, |c| c.name.push_str(&t), cx);
                    cx.notify();
                    return;
                }
                // Typing edits the selected breakpoint's condition.
                if let (Some(BpRow::Line(p, l, b)), Some(t)) = (row, crate::keys::typed_text(k)) {
                    self.bp_dialog_set(cx, |d| d.editing = true);
                    let mut c = b.condition.unwrap_or_default();
                    c.push_str(t);
                    self.bp_set_condition(&p, l, Some(c), cx);
                }
            }
        }
        // Leaving a class name field: an empty one is dropped.
        if was_editing && !self.debug.read(cx).bp_dialog.as_ref().is_some_and(|d| d.editing) {
            self.bp_drop_empty_classes(cx);
        }
        cx.notify();
    }

    pub(in crate::workspace) fn render_bp_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let sel = self.debug.read(cx).bp_dialog.as_ref()?.sel;
        let editing = self.debug.read(cx).bp_dialog.as_ref().is_some_and(|d| d.editing);
        let rows = self.bp_rows(cx);
        let sel = sel.min(rows.len().saturating_sub(1));
        let check = |on: bool| {
            div()
                .size(ui_px(14.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(3.))
                .border_1()
                .border_color(if on { Theme::accent() } else { Theme::text_dim() })
                .when(on, |x| x.bg(Theme::accent()).child(crate::icons::ui_icon("check-bold", 10., gpui::white())))
        };
        let group = |label: &'static str| div().px(px(8.)).pt(px(6.)).pb(px(2.)).text_size(ui_px(11.)).text_color(Theme::text_dim()).child(label);
        let mut list: Vec<gpui::AnyElement> = vec![group("PYTHON EXCEPTION BREAKPOINTS").into_any_element()];
        let first_line = rows.iter().position(|r| matches!(r, BpRow::Line(..)));
        for (i, r) in rows.iter().enumerate() {
            if Some(i) == first_line {
                list.push(group("PYTHON LINE BREAKPOINTS").into_any_element());
            }
            let (on, label, detail) = match r {
                BpRow::Raised => (self.debug.read(cx).exc_raised, "On raised exceptions".to_string(), "even when handled".to_string()),
                BpRow::Uncaught => (self.debug.read(cx).exc_uncaught, "On uncaught exceptions".to_string(), String::new()),
                BpRow::Class(_, c) => (
                    c.enabled,
                    if c.name.is_empty() { "<class name>".to_string() } else { c.name.clone() },
                    match (c.raised, c.uncaught) {
                        (true, _) => "on raise".to_string(),
                        (false, true) => "on termination".to_string(),
                        _ => "never".to_string(),
                    },
                ),
                BpRow::Line(p, l, b) => {
                    let rel = p.strip_prefix(&self.root).unwrap_or(p).to_string_lossy().replace('\\', "/");
                    (b.enabled, format!("{rel}:{}", l + 1), b.condition.clone().map(|c| format!("if {c}")).unwrap_or_default())
                }
            };
            let row = r.clone();
            let row2 = r.clone();
            let selected = i == sel;
            list.push(
                div()
                    .id(("bp-row", i))
                    .h(ui_px(24.))
                    .px(px(8.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .when(selected, |x| x.bg(Theme::selection_row()))
                    .when(!selected, |x| x.hover(|s| s.bg(Theme::hover_row())))
                    .child(div().id(("bp-check", i)).child(check(on)).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.bp_toggle_row(&row, cx);
                    })))
                    .child(div().text_color(if on { Theme::text() } else { Theme::text_muted() }).whitespace_nowrap().child(label))
                    .child(div().text_color(Theme::text_dim()).whitespace_nowrap().overflow_hidden().child(detail))
                    .on_click(cx.listener(move |this, ev: &gpui::ClickEvent, w, cx| {
                        if let Some(d) = this.debug.as_mut(cx).bp_dialog.as_mut() {
                            d.sel = i;
                            d.editing = false;
                        }
                        this.bp_drop_empty_classes(cx);
                        if ev.click_count() >= 2 {
                            this.bp_go_to(&row2, w, cx);
                        }
                        cx.notify();
                    }))
                    .into_any_element(),
            );
        }
        let button = |id: &'static str, label: &'static str, primary: bool, on: bool| {
            div()
                .id(id)
                .px(px(14.))
                .h(ui_px(28.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .when(!on, |b| b.text_color(Theme::text_dim()).border_1().border_color(Theme::divider()))
                .when(on, |b| b.cursor_pointer().active(|s| s.bg(Theme::press())))
                .when(on && primary, |b| b.bg(Theme::accent()).text_color(gpui::white()))
                .when(on && !primary, |b| b.border_1().border_color(Theme::divider()).hover(|st| st.bg(Theme::hover_row())))
                .child(label)
        };
        let cur = rows.get(sel).cloned();
        let details: gpui::AnyElement = match &cur {
            Some(BpRow::Line(p, l, b)) => {
                let cond = b.condition.clone().unwrap_or_default();
                let (p2, l2) = (p.clone(), *l);
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(format!(
                        "{}:{}",
                        p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                        l + 1
                    )))
                    .child(div().id("bp-enabled").flex().items_center().gap(px(8.)).cursor_pointer().child(check(b.enabled)).child("Enabled").on_click(
                        cx.listener(move |this, _, _, cx| {
                            if let Some(BpRow::Line(p, l, b)) = this.bp_rows(cx).into_iter().find(|r| matches!(r, BpRow::Line(p, l, _) if *p == p2 && *l == l2))
                            {
                                this.bp_set_enabled(&p, l, !b.enabled, cx);
                            }
                        }),
                    ))
                    .child(div().text_color(Theme::text_muted()).child("Condition (type to edit):"))
                    .child(
                        div()
                            .id("bp-condition")
                            .h(ui_px(28.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .border_1()
                            .border_color(if editing { Theme::accent() } else { Theme::divider() })
                            .bg(Theme::editor_bg())
                            .font_family(mono_font())
                            .cursor_text()
                            .child(cond)
                            .when(editing, |x| x.child(div().w(px(1.5)).h(ui_px(15.)).bg(Theme::caret())))
                            .when(editing, |x| x.relative().child(crate::ui_kit::focus_ring(crate::ui_kit::RADIUS, 0.)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(d) = this.debug.as_mut(cx).bp_dialog.as_mut() {
                                    d.editing = true;
                                }
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .text_size(ui_px(12.))
                            .text_color(Theme::text_dim())
                            .child("A Python expression: the debugger stops only when it is true. Empty: always."),
                    )
                    .into_any_element()
            }
            Some(BpRow::Class(i, c)) => {
                let i = *i;
                let flag = |id: &'static str, on: bool, label: &'static str, f: fn(&mut ExcClass)| {
                    div()
                        .id(id)
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .cursor_pointer()
                        .child(check(on))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| this.bp_update_class(i, f, cx)))
                };
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Python exception class"))
                    .child(
                        div()
                            .id("bp-class-name")
                            .h(ui_px(28.))
                            .px(px(8.))
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .border_1()
                            .border_color(if editing { Theme::accent() } else { Theme::divider() })
                            .bg(Theme::editor_bg())
                            .font_family(mono_font())
                            .cursor_text()
                            .child(c.name.clone())
                            .when(editing, |x| x.child(div().w(px(1.5)).h(ui_px(15.)).bg(Theme::caret())))
                            .when(editing, |x| x.relative().child(crate::ui_kit::focus_ring(crate::ui_kit::RADIUS, 0.)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                if let Some(d) = this.debug.as_mut(cx).bp_dialog.as_mut() {
                                    d.editing = true;
                                }
                                cx.notify();
                            })),
                    )
                    .child(flag("bp-class-enabled", c.enabled, "Enabled", |c| c.enabled = !c.enabled))
                    .child(flag("bp-class-raise", c.raised, "On raise (even when handled)", |c| c.raised = !c.raised))
                    .child(flag("bp-class-uncaught", c.uncaught, "On termination (uncaught)", |c| c.uncaught = !c.uncaught))
                    .child(div().text_size(ui_px(12.)).text_color(Theme::text_dim()).child("A built-in name (ValueError) or the full name of an importable class (mypkg.errors.MyError); its subclasses stop too. A class defined in the script being run cannot be found by debugpy."))
                    .into_any_element()
            }
            Some(BpRow::Raised) => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("On raised exceptions"))
                .child(
                    div()
                        .text_color(Theme::text_muted())
                        .child("Stop wherever an exception is raised, even if a try block handles it (debugpy filter \"raised\")."),
                )
                .into_any_element(),
            Some(BpRow::Uncaught) => div()
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("On uncaught exceptions"))
                .child(div().text_color(Theme::text_muted()).child("Stop where an exception nothing handles is raised (debugpy filter \"uncaught\")."))
                .into_any_element(),
            None => div().into_any_element(),
        };
        let is_line = matches!(cur, Some(BpRow::Line(..)));
        let removable = is_line || matches!(cur, Some(BpRow::Class(..)));
        let (cur_rm, cur_go) = (cur.clone(), cur.clone());
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
                        .id("bp-dialog")
                        .w(px(760.))
                        .flex()
                        .flex_col()
                        .p(px(16.))
                        .gap(px(10.))
                        .bg(Theme::panel())
                        .border_1()
                        .border_color(Theme::divider())
                        .rounded(px(10.))
                        .map(crate::ui_kit::floating)
                        .text_size(ui_px(13.))
                        .text_color(Theme::text())
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Breakpoints"))
                        .child(
                            div()
                                .flex()
                                .h(ui_px(320.))
                                .gap(px(12.))
                                .child(
                                    div()
                                        .id("bp-list")
                                        .w(px(380.))
                                        .flex_shrink_0()
                                        .overflow_y_scroll()
                                        .border_1()
                                        .border_color(Theme::divider())
                                        .rounded(px(6.))
                                        .bg(Theme::editor_bg())
                                        .children(list),
                                )
                                .child(div().flex_1().min_w_0().child(details)),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_size(ui_px(12.))
                                        .text_color(Theme::text_dim())
                                        .child("Space: on/off · type: edit · Insert: + exception · Delete: remove · Enter: go to · Esc: close"),
                                )
                                .child(button("bp-add-class", "+ Exception", false, true).on_click(cx.listener(|this, _, _, cx| this.bp_add_class(cx))))
                                .child(button("bp-remove", "Remove", false, removable).on_click(cx.listener(move |this, _, _, cx| match cur_rm.clone() {
                                    Some(BpRow::Line(p, l, _)) => this.bp_remove(&p, l, cx),
                                    Some(BpRow::Class(i, _)) => this.bp_remove_class(i, cx),
                                    _ => {}
                                })))
                                .child(button("bp-goto", "Go to", false, is_line).on_click(cx.listener(move |this, _, w, cx| {
                                    if let Some(r) = cur_go.clone() {
                                        this.bp_go_to(&r, w, cx);
                                    }
                                })))
                                .child(button("bp-done", "Done", true, true).on_click(cx.listener(|this, _, _, cx| {
                                    this.debug.as_mut(cx).bp_dialog = None;
                                    this.bp_drop_empty_classes(cx);
                                    cx.notify();
                                }))),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl DebugModel {
    /// setBreakpoints for one file; the reply says which are verified (`DebugEvent::Verified`).
    pub(super) fn set_breakpoints(&mut self, path: &Path, bps: &[SourceBreakpoint], cx: &mut Context<Self>) {
        let Some(s) = self.session.as_ref().filter(|s| s.state != State::Ended) else { return };
        let rx = s.client.request("setBreakpoints", brisk_dap::set_breakpoints_args(path, bps));
        let path = path.to_path_buf();
        cx.spawn(async move |this, cx| {
            let Ok(Ok(v)) = rx.await else { return };
            // Replies list the temporary breakpoint last; editors only know the user's.
            let verified = brisk_dap::parse_breakpoints(&v);
            let _ = this.update(cx, |_, cx| cx.emit(DebugEvent::Verified(path, verified)));
        })
        .detach();
    }
}
