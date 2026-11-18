//! `BRISK_SMOKE_BIG=<file.py>`: open a large file, then type into it key by key and print how
//! long each keystroke takes to handle and to paint (time to the next frame), plus how long
//! ruff and ty take to report on it.

use super::Workspace;
use gpui::{Context, Window};
use std::path::PathBuf;
use std::time::{Duration, Instant};

impl Workspace {
    pub(super) fn big_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let t0 = Instant::now();
        self.open_file(&file, window, cx);
        eprintln!("smoke-big: opened {} in {:.0?}", file.display(), t0.elapsed());
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            // Let ty start and the first checks run.
            pause(6000).await;
            let _ = this.update(cx, |this, cx| {
                let Some(ed) = this.active_editor().cloned() else { return };
                let e = ed.read(cx);
                eprintln!("smoke-big: after 6s: {} lines, ty {:?}, {} diagnostics, status {:?}", e.buffer.len_lines(), this.lsp.state, e.diagnostics.len(), this.status);
                let mid = e.buffer.len_lines() / 2;
                ed.update(cx, |e, cx| e.go_to_line(mid, cx));
            });
            pause(500).await;
            let mut handle = Vec::new();
            let mut frame = Vec::new();
            for ch in "value = compute_total(items)\n".chars() {
                let t = Instant::now();
                let _ = this.update_in(cx, |this, window, cx| {
                    if let Some(ed) = this.active_editor().cloned() {
                        ed.update(cx, |e, cx| e.type_for_smoke(&ch.to_string(), cx));
                    }
                    window.refresh();
                });
                handle.push(t.elapsed());
                // The next frame: an update that runs after the window drew again.
                let _ = this.update_in(cx, |_, window, _| window.refresh());
                pause(1).await;
                frame.push(t.elapsed());
                pause(60).await;
            }
            let stats = |v: &[Duration]| {
                let mut v = v.to_vec();
                v.sort();
                format!("median {:.1?}, max {:.1?}", v[v.len() / 2], v[v.len() - 1])
            };
            eprintln!("smoke-big: typing {} keys: handle {}, to next frame {}", handle.len(), stats(&handle), stats(&frame));
            pause(4000).await;
            let _ = this.update(cx, |this, cx| {
                let Some(ed) = this.active_editor() else { return };
                eprintln!("smoke-big: 4s after typing: {} diagnostics, inlay lines {}, status {:?}", ed.read(cx).diagnostics.len(), ed.read(cx).inlay_line_count(), this.status);
                eprintln!("smoke-big: done in {:.1?}", t0.elapsed());
                if std::env::var_os("BRISK_SMOKE_BIG_HOLD").is_none() {
                    cx.quit();
                }
            });
        })
        .detach();
    }
}
