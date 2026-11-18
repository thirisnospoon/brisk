//! `BRISK_SMOKE_SCOPE=1` (on an empty folder: writes `scope.py`): the imports folded on
//! opening, Collapse All / Expand All, usages of the name under the caret highlighted after
//! the caret rests (not at once), and, without ty, Find Usages (file only) and in-place
//! Rename of a local; Rename of a module-level name says it needs ty. With ty the same steps
//! go to ty.

use super::Workspace;
use brisk_keymap::Action;
use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use std::time::{Duration, Instant};

const SRC: &str = "import os
import sys
from collections import defaultdict


total = 0


def add(items, total=1):
    for x in items:
        total += x
    return total


def other():
    return total + len(os.sep) + len(sys.argv) + len(defaultdict(int))
";

impl Workspace {
    pub(super) fn scope_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        let file = self.root.join("scope.py");
        if let Err(e) = std::fs::write(&file, SRC) {
            eprintln!("smoke-scope: cannot write {}: {e}", file.display());
            return;
        }
        self.open_file(&file, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        eprintln!("smoke-scope: on open, folds {:?}", ed.read(cx).fold_state());
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            let wait = async |this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, f: &dyn Fn(&Self, &gpui::App) -> bool| {
                for _ in 0..100 {
                    if this.update(cx, |this, cx| f(this, cx)).unwrap_or(true) {
                        return true;
                    }
                    pause(50).await;
                }
                false
            };
            use super::lsp_ui::TyState;
            wait(&this, cx, &|this, _| this.lsp.state != TyState::Unknown && this.lsp.state != TyState::Starting).await;
            let ty = this.update(cx, |this, _| this.lsp.ty.is_some()).unwrap_or(false);
            eprintln!("smoke-scope: ty {ty}");

            // Collapse All / Expand All (Ctrl+Shift+- / Ctrl+Shift+=).
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch(Action::CollapseAll, window, cx);
                let all = ed.read(cx).fold_state();
                this.dispatch(Action::ExpandAll, window, cx);
                eprintln!("smoke-scope: collapse all {all:?}, expand all {:?}", ed.read(cx).fold_state());
            });

            // Usages under the caret: the parameter `total` of add (line 8).
            let _ = this.update(cx, |_, cx| ed.update(cx, |e, cx| e.select_on_line(8, 16..16, cx)));
            pause(100).await;
            let early = this.update(cx, |_, cx| ed.read(cx).shown_occurrences()).unwrap_or(99);
            pause(400).await;
            let late = this.update(cx, |_, cx| ed.read(cx).shown_occurrences()).unwrap_or(0);
            eprintln!("smoke-scope: usages of local 'total' after 100 ms {early}, after 500 ms {late}");
            let _ = this.update(cx, |_, cx| ed.update(cx, |e, cx| e.select_on_line(5, 0..0, cx)));
            pause(500).await;
            let module = this.update(cx, |_, cx| ed.read(cx).shown_occurrences()).unwrap_or(0);
            eprintln!("smoke-scope: usages of module 'total' after 500 ms {module}");

            // Find Usages (Alt+F7) on the module-level `total`.
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::FindUsages, window, cx));
            wait(&this, cx, &|this, _| this.status.contains("usage")).await;
            let _ = this.update(cx, |this, _| eprintln!("smoke-scope: Find Usages -> {:?}, {:?}", this.status, this.find_window_rows()));

            // Rename (Shift+F6) of the module-level name, then of the local parameter.
            let _ = this.update_in(cx, |this, window, cx| {
                ed.update(cx, |e, cx| e.select_on_line(5, 1..1, cx));
                this.dispatch(Action::Rename, window, cx);
            });
            pause(500).await;
            let _ = this.update_in(cx, |this, window, cx| {
                eprintln!("smoke-scope: Rename module 'total' -> status {:?}, popup {}", this.status, this.popup.is_some());
                this.popup = None;
                ed.update(cx, |e, cx| e.select_on_line(10, 9..9, cx));
                this.dispatch(Action::Rename, window, cx);
            });
            pause(500).await;
            let _ = this.update_in(cx, |this, window, cx| {
                let n = ed.read(cx).buffer.selections.len();
                eprintln!("smoke-scope: Rename local 'total' -> {n} selection(s), popup {}, status {:?}", this.popup.is_some(), this.status);
                if !ty {
                    ed.update(cx, |e, cx| e.type_for_smoke("acc", cx));
                    this.smoke_press("escape", window, cx);
                    let t = ed.read(cx).text_string();
                    eprintln!("smoke-scope: after typing -> {:?}", t.lines().collect::<Vec<_>>());
                }
                eprintln!("smoke-scope: done in {:.1?}", t0.elapsed());
                if std::env::var_os("BRISK_SMOKE_SCOPE_HOLD").is_none() {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}
