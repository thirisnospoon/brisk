//! `BRISK_SMOKE_COMPLETE=1`: completion while typing and middle click, with whatever ty there
//! is: none (`BRISK_TY` set to a missing file and no ty anywhere) gives words of the file,
//! keywords and builtins; the bundled `lib\ty.exe` (or one in the interpreter) gives ty's list.
//! The middle click is a real mouse message on `compute_total` in the last line.

use super::Workspace;
use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use std::time::{Duration, Instant};

const SRC: &str = "def compute_total(items):\n    return sum(items)\n\n\nclass Order:\n    def __init__(self):\n        self.total_price = 0\n\n\norder = Order()\nvalue = compute_total([1, 2])\n";

impl Workspace {
    pub(super) fn complete_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        let file = self.root.join("comp.py");
        if let Err(e) = std::fs::write(&file, SRC) {
            eprintln!("smoke-complete: cannot write {}: {e}", file.display());
            return;
        }
        self.open_file(&file, window, cx);
        let bg = cx.background_executor().clone();
        let scale = window.scale_factor();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            let wait = async |this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, f: &dyn Fn(&Self, &gpui::App) -> bool| {
                for _ in 0..200 {
                    if this.update(cx, |this, cx| f(this, cx)).unwrap_or(true) {
                        return true;
                    }
                    pause(50).await;
                }
                false
            };
            use super::lsp_ui::TyState;
            wait(&this, cx, &|this, _| this.lsp.state != TyState::Unknown && this.lsp.state != TyState::Starting).await;
            let _ = this.update(cx, |this, _| {
                eprintln!("smoke-complete: ty {:?} after {:.1?}", this.lsp.state, t0.elapsed());
                eprintln!("smoke-complete: status {:?}", this.status);
                eprintln!("smoke-complete: toast {:?}", this.toast);
            });
            // Type key by key at the end of the file; each case starts a new line.
            for (typed, accept) in [("co", false), ("re", false), ("order.", false), ("val", true)] {
                let _ = this.update(cx, |this, cx| {
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| {
                            while e.close_insight() {}
                            let end = e.buffer.len_chars();
                            e.buffer.set_caret(end, false);
                            e.type_for_smoke("\n", cx);
                        });
                    }
                });
                for ch in typed.chars() {
                    pause(60).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(ed) = this.active_editor().cloned() {
                            ed.update(cx, |e, cx| e.type_for_smoke(&ch.to_string(), cx));
                        }
                    });
                }
                let open = wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| e.read(cx).completion_open())).await;
                pause(300).await;
                let _ = this.update(cx, |this, cx| {
                    let Some(ed) = this.active_editor().cloned() else { return };
                    eprintln!("smoke-complete: {typed:?} -> open {open}, {}", ed.read(cx).insight_summary());
                    if accept && open {
                        ed.update(cx, |e, cx| e.completion_key("enter", cx));
                        let t = ed.read(cx).text_string();
                        eprintln!("smoke-complete: Enter -> last line {:?}", t.lines().last());
                    }
                });
            }
            // Middle click on `compute_total` in `value = compute_total([1, 2])`.
            let line = this
                .update(cx, |this, cx| {
                    let ed = this.active_editor().cloned()?;
                    let line = ed.read(cx).text_string().lines().position(|l| l.starts_with("value = compute_total"))?;
                    ed.update(cx, |e, cx| {
                        while e.close_insight() {}
                        e.select_on_line(line, 0..0, cx);
                    });
                    Some(line)
                })
                .ok()
                .flatten()
                .unwrap_or(10);
            pause(300).await;
            let p = this.update(cx, |this, cx| this.active_editor().and_then(|e| e.read(cx).char_point(line, 10))).ok().flatten();
            let Some(p) = p else {
                eprintln!("smoke-complete: line 11 is not on screen");
                return;
            };
            let (x, y) = (f32::from(p.x) * scale, f32::from(p.y) * scale);
            super::win_input::post(super::win_input::Mouse::Move, x, y, false);
            pause(150).await;
            super::win_input::middle_click(x, y);
            wait(&this, cx, &|this, cx| this.active_editor().is_some_and(|e| e.read(cx).caret_line_col().0 != line) || this.status.contains("eclaration")).await;
            pause(200).await;
            let _ = this.update(cx, |this, cx| {
                let caret = this.active_editor().map(|e| e.read(cx).caret_line_col());
                eprintln!("smoke-complete: middle click on compute_total -> caret {caret:?}, status {:?}", this.status);
                eprintln!("smoke-complete: done in {:.1?}", t0.elapsed());
                if std::env::var_os("BRISK_SMOKE_COMPLETE_HOLD").is_none() {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}
