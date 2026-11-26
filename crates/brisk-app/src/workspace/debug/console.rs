//! The Debug console: evaluating the input line and completion.

use super::*;

impl Workspace {
    /// Enter in the console's input line: evaluate in the selected frame.
    pub(super) fn evaluate_input(&mut self, cx: &mut Context<Self>) {
        let expr = std::mem::take(&mut self.debug.as_mut(cx).input);
        if expr.trim().is_empty() {
            return;
        }
        self.debug.as_mut(cx).completion = None;
        self.debug.as_mut(cx).history.retain(|h| *h != expr);
        self.debug.as_mut(cx).history.push(expr.clone());
        let n = self.debug.read(cx).history.len().saturating_sub(100);
        self.debug.as_mut(cx).history.drain(..n);
        self.debug.as_mut(cx).history_pos = usize::MAX;
        let _ = std::fs::create_dir_all(brisk_db::config_dir());
        let _ = std::fs::write(history_path(), serde_json::to_string(&self.debug.read(cx).history).unwrap_or_default());
        self.debug.as_mut(cx).print(&format!(">>> {expr}\n"), Line::Input);
        let frame = self.debug.read(cx).frames.get(self.debug.read(cx).frame).map(|f| f.id);
        let Some(s) = self.debug.read(cx).session.as_ref().filter(|s| s.state != State::Ended) else {
            self.debug.as_mut(cx).info("The debugger is not running");
            return;
        };
        let mut args = json!({"expression": expr, "context": "repl"});
        if let Some(f) = frame {
            args["frameId"] = json!(f);
        }
        let rx = s.client.request("evaluate", args);
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(Ok(v)) => {
                        let (r, _) = brisk_dap::parse_evaluate(&v);
                        if !r.is_empty() {
                            this.debug.as_mut(cx).print(&format!("{r}\n"), Line::Result);
                        }
                    }
                    Ok(Err(e)) => this.debug.as_mut(cx).print(&format!("{e}\n"), Line::Err),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Tab in the console: ask debugpy what the name at the end of the input can be.
    pub(super) fn complete_input(&mut self, cx: &mut Context<Self>) {
        let Some(frame) = self.debug.read(cx).frames.get(self.debug.read(cx).frame).map(|f| f.id) else {
            self.status = "Completion works while the program is paused".into();
            return;
        };
        let Some(s) = self.debug.read(cx).session.as_ref().filter(|s| s.state != State::Ended) else { return };
        let text = self.debug.read(cx).input.clone();
        let rx = s.client.request("completions", json!({"text": text, "column": text.chars().count() + 1, "frameId": frame}));
        cx.spawn(async move |this, cx| {
            let res = rx.await;
            let _ = this.update(cx, |this, cx| {
                if this.debug.read(cx).input != text {
                    return;
                }
                let mut items: Vec<(String, String)> = match res {
                    Ok(Ok(v)) => v
                        .get("targets")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .filter_map(|t| Some((t.get("label")?.as_str()?.to_string(), t.get("type").and_then(Value::as_str).unwrap_or("").to_string())))
                                .collect()
                        })
                        .unwrap_or_default(),
                    _ => Vec::new(),
                };
                order_completions(&mut items);
                let start = completion_start(&text);
                match items.len() {
                    0 => this.status = "No completions".into(),
                    1 => {
                        this.debug.as_mut(cx).input.truncate(start);
                        this.debug.as_mut(cx).input.push_str(&items[0].0);
                    }
                    _ => {
                        let typed = &text[start..];
                        let p = common_prefix(items.iter().map(|i| i.0.as_str()));
                        if p.len() > typed.len() && p.starts_with(typed) {
                            this.debug.as_mut(cx).input.truncate(start);
                            this.debug.as_mut(cx).input.push_str(&p);
                        }
                        this.debug.as_mut(cx).completion = Some(ConsoleCompletion { items, start, sel: 0 });
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }
}

impl DebugModel {
    /// Keys while the completion list is open; false lets the input line have the key.
    pub(super) fn completion_key(&mut self, key: &str) -> bool {
        let Some(c) = self.completion.as_mut() else { return false };
        let n = c.shown(&self.input).len();
        match key {
            "up" => c.sel = c.sel.checked_sub(1).unwrap_or(n.saturating_sub(1)),
            "down" => c.sel = if c.sel + 1 < n { c.sel + 1 } else { 0 },
            "pageup" => c.sel = c.sel.saturating_sub(8),
            "pagedown" => c.sel = (c.sel + 8).min(n.saturating_sub(1)),
            "escape" => self.completion = None,
            "enter" | "tab" => {
                let start = c.start;
                if let Some(l) = c.shown(&self.input).get(c.sel).map(|i| i.0.clone()) {
                    self.input.truncate(start);
                    self.input.push_str(&l);
                }
                self.completion = None;
            }
            _ => return false,
        }
        true
    }

    /// After the input changed: the list follows what is typed, or closes.
    pub(super) fn refilter_completion(&mut self) {
        let Some(c) = self.completion.as_mut() else { return };
        let input = &self.input;
        let start_ok = input.len() >= c.start && input.get(c.start..).is_some_and(|t| t.chars().all(|ch| ch.is_alphanumeric() || ch == '_'));
        let n = if start_ok { c.shown(input).len() } else { 0 };
        if n == 0 {
            self.completion = None;
        } else {
            c.sel = c.sel.min(n - 1);
        }
    }
}
