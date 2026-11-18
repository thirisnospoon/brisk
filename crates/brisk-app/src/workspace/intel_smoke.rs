//! `BRISK_SMOKE_INTEL=1`: code intelligence end to end with ty and ruff. Writes `intel.py` in
//! the project, then checks Alt+Enter auto-import (and that one Undo takes it back),
//! Optimize Imports, auto-import completion and the documentation preview beside the list.
//! With `BRISK_SMOKE_INTEL_HOLD=1` it stops with the completion list and its preview open.

use super::{Target, Workspace};
use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use std::time::{Duration, Instant};

const SRC: &str = "import sys\nimport os\n\nx = defaultdict(list)\nprint(os.sep, x)\n";

impl Workspace {
    pub(super) fn intel_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        let file = self.root.join("intel.py");
        if let Err(e) = std::fs::write(&file, SRC) {
            eprintln!("smoke-intel: cannot write {}: {e}", file.display());
            return;
        }
        self.open_file(&file, window, cx);
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            let wait = async |this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, f: &dyn Fn(&Self, &gpui::App) -> bool| {
                for _ in 0..600 {
                    if this.update(cx, |this, cx| f(this, cx)).unwrap_or(true) {
                        return;
                    }
                    pause(50).await;
                }
            };
            let text = |this: &Self, cx: &gpui::App| this.active_editor().map(|e| e.read(cx).text_string()).unwrap_or_default();
            // ty running and the file checked (ruff's F821 stands for ty's unresolved-reference).
            wait(&this, cx, &|this, _| this.lsp.state != super::lsp_ui::TyState::Unknown && this.lsp.state != super::lsp_ui::TyState::Starting).await;
            eprintln!("smoke-intel: ty ready after {:.1?}", t0.elapsed());
            wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| e.read(cx).diagnostics.iter().any(|d| d.code == "F821" || d.code == "unresolved-reference"))).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-intel: ty {:?} after {:.1?}", this.lsp.state, t0.elapsed());
                if let Some(ed) = this.active_editor().cloned() {
                    for d in &ed.read(cx).diagnostics {
                        eprintln!("smoke-intel: {:?} {}:{} {}", d.severity, d.start.line + 1, d.start.col + 1, d.title());
                    }
                    // Alt+Enter on `defaultdict`.
                    ed.update(cx, |e, cx| e.select_on_line(3, 6..6, cx));
                }
                this.problems.intents.clear();
                this.show_intentions(window, cx);
            });
            wait(&this, cx, &|this, _| !this.problems.intents.is_empty()).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let labels: Vec<String> = this.problems.intents.iter().map(|i| format!("{} [{}]", i.label, i.detail)).collect();
                eprintln!("smoke-intel: Alt+Enter -> {labels:?}");
                let pick = this.problems.intents.iter().find(|i| i.label.contains("collections")).map(|i| i.target.clone());
                this.popup = None;
                if let Some(Target::Intention(i)) = pick {
                    this.apply_intent(*i, window, cx);
                }
                eprintln!("smoke-intel: status {:?}", this.status);
                let t = text(this, cx);
                eprintln!("smoke-intel: after import {:?}", t.lines().take(4).collect::<Vec<_>>());
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, _| e.buffer.undo());
                    eprintln!("smoke-intel: one undo restores the file: {}", text(this, cx) == SRC);
                    ed.update(cx, |e, _| e.buffer.redo());
                }
                // Optimize Imports drops `import sys` and sorts.
                this.dispatch(brisk_keymap::Action::OptimizeImports, window, cx);
                eprintln!("smoke-intel: Ctrl+Alt+O -> {:?}", this.status);
                eprintln!("smoke-intel: after optimize {:?}", text(this, cx).lines().take(4).collect::<Vec<_>>());
                this.dispatch(brisk_keymap::Action::OptimizeImports, window, cx);
                eprintln!("smoke-intel: Ctrl+Alt+O again -> {:?}", this.status);
                // Auto-import completion at the end of the file.
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, cx| {
                        let end = e.buffer.len_chars();
                        e.buffer.set_caret(end, false);
                        e.type_for_smoke("y = ", cx);
                        e.close_insight();
                    });
                }
            });
            // Key by key, as typing does: each keystroke is its own event for the workspace.
            for ch in "OrderedD".chars() {
                pause(40).await;
                let _ = this.update(cx, |this, cx| {
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.type_for_smoke(&ch.to_string(), cx));
                    }
                });
            }
            wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| e.read(cx).completion_open())).await;
            pause(200).await;
            let _ = this.update_in(cx, |this, _, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                eprintln!("smoke-intel: after 'OrderedD' -> {}", ed.read(cx).insight_summary());
                let before = text(this, cx);
                ed.update(cx, |e, cx| e.completion_key("enter", cx));
                let after = text(this, cx);
                eprintln!("smoke-intel: accepted -> {:?} ... {:?}", after.lines().take(3).collect::<Vec<_>>(), after.lines().last());
                eprintln!("smoke-intel: caret {:?}", ed.read(cx).caret_line_col());
                ed.update(cx, |e, _| e.buffer.undo());
                eprintln!("smoke-intel: one undo removes word and import: {}", text(this, cx) == before);
                ed.update(cx, |e, _| e.buffer.redo());
                // Documentation preview of the selected item.
                ed.update(cx, |e, cx| e.type_for_smoke("\nos.", cx));
            });
            pause(300).await;
            for ch in "pa".chars() {
                let _ = this.update(cx, |this, cx| {
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.type_for_smoke(&ch.to_string(), cx));
                    }
                });
                pause(40).await;
            }
            wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| e.read(cx).completion_open())).await;
            pause(200).await;
            let _ = this.update(cx, |this, cx| {
                if let Some(ed) = this.active_editor() {
                    eprintln!("smoke-intel: after 'os.pa' -> {}", ed.read(cx).insight_summary());
                }
                cx.notify();
            });
            if std::env::var_os("BRISK_SMOKE_INTEL_HOLD").is_some() {
                eprintln!("smoke-intel: holding");
                return;
            }
            let _ = this.update(cx, |this, cx| {
                if let Some(ed) = this.active_editor().cloned() {
                    ed.update(cx, |e, _| e.close_insight());
                }
                eprintln!("smoke-intel: done in {:.1?}", t0.elapsed());
                cx.quit();
            });
        })
        .detach();
    }
}
