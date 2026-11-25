//! Ctrl+J (Insert Live Template) and Ctrl+Alt+T (Surround With): the chooser popups. The
//! expansion itself lives in `editor/templates.rs`; Tab after an abbreviation expands it there.

use super::problems_ui::Intent;
use super::{PopupItem, PopupKind, Target, Workspace};
use crate::editor::templates::{SURROUNDERS, TEMPLATES};
use brisk_keymap::Action;
use gpui::{Context, Window};
use std::time::Duration;

impl Workspace {
    pub(super) fn template_action(&mut self, action: Action, window: &mut Window, cx: &mut Context<Self>) {
        let title = if action == Action::SurroundWith { "Surround With" } else { "Insert Live Template" };
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        if !ed.read(cx).is_python() {
            self.status = format!("{title}: Brisk has Python templates only");
            cx.notify();
            return;
        }
        let item = |label: String, detail: String, hint: String, intent: Intent| PopupItem { label, detail, hint, target: Target::Intention(Box::new(intent)) };
        let items: Vec<PopupItem> = if action == Action::SurroundWith {
            SURROUNDERS.iter().enumerate().map(|(i, s)| item(s.0.to_string(), String::new(), (i + 1).to_string(), Intent::Surround(i))).collect()
        } else {
            // An abbreviation before the caret expands at once, like Tab.
            if ed.update(cx, |e, cx| e.expand_live_template(cx)) {
                return;
            }
            TEMPLATES
                .iter()
                .filter(|t| t.key != "ifmain")
                .map(|t| item(t.key.to_string(), t.desc.to_string(), String::new(), Intent::Template(t.key)))
                .collect()
        };
        self.problems.intents = items;
        self.problems.intents_title = Some(title);
        self.open_popup(PopupKind::Intentions);
        cx.notify();
    }

    /// Ctrl+Shift+Delete (Unwrap/Remove...): the enclosing statements to unwrap or remove.
    pub(super) fn unwrap_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ed) = self.focused_editor(window, cx).or_else(|| self.active_editor().cloned()) else { return };
        let options = ed.update(cx, |e, _| e.unwrap_options());
        if options.is_empty() {
            self.problems.intents.clear();
            self.status = "Nothing here to unwrap or remove".into();
            cx.notify();
            return;
        }
        self.problems.intents = options
            .into_iter()
            .map(|u| PopupItem { label: u.label.clone(), detail: String::new(), hint: String::new(), target: Target::Intention(Box::new(Intent::Unwrap(u))) })
            .collect();
        self.problems.intents_title = Some("Choose the statement to unwrap/remove");
        self.open_popup(PopupKind::Intentions);
        cx.notify();
    }

    /// `BRISK_SMOKE_TEMPLATES=1`: writes `tpl.py`, `tpl.toml`, `tpl.yaml`, `tpl.json` and
    /// `tpl.css` in the project and presses real keys: abbreviations + Tab, Ctrl+J, Ctrl+Alt+T,
    /// Ctrl+/ and Ctrl+Shift+/. Prints each result.
    pub(super) fn templates_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let files = [
            ("tpl.py", "x = 1\nprint(x)\n"),
            ("tpl.toml", "[tool]\nname = \"a\"\n"),
            ("tpl.yaml", "a: 1\nb: 2\n"),
            ("tpl.json", "{\n  \"a\": 1\n}\n"),
            ("tpl.css", "body {\n  color: red;\n}\n"),
        ];
        for (n, t) in files {
            if let Err(e) = std::fs::write(self.root.join(n), t) {
                eprintln!("smoke-tpl: cannot write {n}: {e}");
                return;
            }
        }
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            let text = |this: &Self, cx: &gpui::App| this.active_editor().map(|e| e.read(cx).text_string()).unwrap_or_default();
            let sel = |this: &Self, cx: &gpui::App| {
                this.active_editor()
                    .map(|e| {
                        let e = e.read(cx);
                        let r = e.buffer.primary().range();
                        e.buffer.rope().slice(r).to_string()
                    })
                    .unwrap_or_default()
            };
            let open = |name: &'static str| {
                move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| {
                    let p = this.root.join(name);
                    this.open_file(&p, window, cx);
                    this.focus_editor(window, cx);
                }
            };
            // Python: abbreviation + Tab at the end of the file, with ty's completion racing it.
            for (abbr, line) in [("main", 2usize), ("def", 4), ("for", 6), ("try", 8)] {
                let _ = this.update_in(cx, |this, window, cx| {
                    open("tpl.py")(this, window, cx);
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| {
                            let end = e.buffer.len_chars();
                            e.buffer.set_caret(end, false);
                            e.type_for_smoke(abbr, cx);
                        });
                    }
                    let _ = line;
                });
                pause(400).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    this.smoke_press("tab", window, cx);
                    let t = text(this, cx);
                    let tail: Vec<&str> = t.lines().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
                    eprintln!("smoke-tpl: {abbr}+Tab -> {tail:?}, selected {:?}", sel(this, cx));
                    // Leave the body valid: replace the placeholder, end the block.
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| {
                            let end = e.buffer.len_chars();
                            e.buffer.set_caret(end, false);
                            e.type_for_smoke("\n", cx);
                            e.close_insight();
                        });
                        let _ = ed.update(cx, |e, _| {
                            let t = e.text_string();
                            let n = t.trim_end_matches([' ', '\t']).len();
                            let n = t[..n].chars().count();
                            let len = e.buffer.len_chars();
                            e.buffer.replace_ranges(vec![(n..len, String::new())]);
                        });
                    }
                });
            }
            // Fields: def + Tab, then type into each field and Tab on; Shift+Tab goes back.
            let _ = this.update_in(cx, |this, window, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                ed.update(cx, |e, cx| {
                    let end = e.buffer.len_chars();
                    e.buffer.set_caret(end, false);
                    e.type_for_smoke("\ndef", cx);
                    e.close_insight();
                });
                this.smoke_press("tab", window, cx);
                let mut steps = Vec::new();
                for (typed, key) in [("go", "tab"), ("", "shift-tab"), ("", "tab"), ("x, y", "tab"), ("return x + y", "tab")] {
                    if !typed.is_empty() {
                        ed.update(cx, |e, cx| {
                            e.type_for_smoke(typed, cx);
                            e.close_insight();
                        });
                    }
                    this.smoke_press(key, window, cx);
                    steps.push(format!("{typed:?}+{key} -> sel {:?}", sel(this, cx)));
                }
                let t = text(this, cx);
                let tail: Vec<&str> = t.lines().rev().take(2).collect::<Vec<_>>().into_iter().rev().collect();
                let (l, c) = ed.read(cx).caret_line_col();
                eprintln!("smoke-tpl: fields {steps:?}");
                eprintln!("smoke-tpl: fields result {tail:?}, caret {}:{}, template still active {}", l + 1, c + 1, ed.read(cx).template_active());
            });
            // Ctrl+J lists the templates; Ctrl+Alt+T on the first two lines, then "if / else".
            let _ = this.update_in(cx, |this, window, cx| {
                this.smoke_press("ctrl-j", window, cx);
                let labels: Vec<String> = this.problems.intents.iter().map(|i| i.label.clone()).collect();
                eprintln!("smoke-tpl: Ctrl+J -> {:?} {labels:?}", this.problems.intents_title);
                this.popup = None;
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| {
                        e.buffer.set_caret(0, false);
                        let p = e.buffer.line_start(1) + 3;
                        e.buffer.set_caret(p, true);
                        cx.notify();
                    });
                }
                this.smoke_press("ctrl-alt-t", window, cx);
                let labels: Vec<String> = this.problems.intents.iter().map(|i| format!("{} {}", i.hint, i.label)).collect();
                eprintln!("smoke-tpl: Ctrl+Alt+T -> {:?} {labels:?}", this.problems.intents_title);
                let pick = this.problems.intents.iter().find(|i| i.label == "if / else").map(|i| i.target.clone());
                this.popup = None;
                if let Some(super::Target::Intention(i)) = pick {
                    this.apply_intent(*i, window, cx);
                }
                eprintln!("smoke-tpl: surrounded -> {:?}, selected {:?}", text(this, cx).lines().take(6).collect::<Vec<_>>(), sel(this, cx));
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, _| e.buffer.undo());
                    eprintln!("smoke-tpl: one undo -> {:?}", text(this, cx).lines().take(2).collect::<Vec<_>>());
                }
                // Ctrl+/ twice on line 1 of each file, Ctrl+Shift+/ in Python and CSS.
                for name in ["tpl.py", "tpl.toml", "tpl.yaml", "tpl.json", "tpl.css"] {
                    open(name)(this, window, cx);
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, _| e.buffer.set_caret(0, false));
                    }
                    this.smoke_press("ctrl-/", window, cx);
                    let once = text(this, cx).lines().next().unwrap_or("").to_string();
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, _| e.buffer.set_caret(0, false));
                    }
                    this.smoke_press("ctrl-/", window, cx);
                    let twice = text(this, cx).lines().next().unwrap_or("").to_string();
                    eprintln!("smoke-tpl: {name} Ctrl+/ -> {once:?}, again -> {twice:?}");
                }
                for name in ["tpl.css", "tpl.py"] {
                    open(name)(this, window, cx);
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| {
                            e.buffer.set_caret(0, false);
                            let p = e.buffer.line_start(0) + e.buffer.line_len(0);
                            e.buffer.set_caret(p, true);
                            cx.notify();
                        });
                    }
                    this.smoke_press("ctrl-shift-/", window, cx);
                    let once = text(this, cx).lines().next().unwrap_or("").to_string();
                    this.smoke_press("ctrl-shift-/", window, cx);
                    let twice = text(this, cx).lines().next().unwrap_or("").to_string();
                    eprintln!("smoke-tpl: {name} Ctrl+Shift+/ -> {once:?}, again -> {twice:?}");
                }
                eprintln!("smoke-tpl: done");
                if std::env::var_os("BRISK_SMOKE_TEMPLATES_HOLD").is_none() {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}
