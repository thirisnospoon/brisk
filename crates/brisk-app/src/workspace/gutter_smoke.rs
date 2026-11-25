//! `BRISK_SMOKE_GUTTER=1` (on an empty folder: writes `gutter.py`): the editor gutter with
//! real Windows mouse messages (WM_MOUSEMOVE) and the cursor style the editor then gives its
//! hitbox (GPUI applies it only while the real pointer is over the window, so GetCursor
//! cannot see it from posted messages). Over a line number: the hand and a faint
//! breakpoint; over a breakpoint: the hand and its tooltip (line, condition); over ▶ and a
//! fold marker: the hand; over the text: the I-beam. `BRISK_SMOKE_GUTTER_HOLD=preview|tip`
//! stops in that state for a screenshot.

use super::win_input::{post, Mouse};
use super::Workspace;
use crate::editor::breakpoints::Breakpoint;
use gpui::{Context, Window};
use std::time::Duration;

impl Workspace {
    pub(super) fn gutter_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("gutter.py");
        let src = "import os\n\n\ndef helper(x):\n    if x > 1:\n        return os.sep\n    return x\n\n\nif __name__ == \"__main__\":\n    helper(2)\n";
        if let Err(e) = std::fs::write(&file, src) {
            eprintln!("smoke-gutter: cannot write {}: {e}", file.display());
            return;
        }
        self.open_file(&file, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        // A breakpoint with a condition on `return os.sep` (line 6).
        ed.update(cx, |e, _| e.set_breakpoints(&[(5, Breakpoint::new(Some("x > 1".into())))]));
        let scale = window.scale_factor();
        let hold = std::env::var("BRISK_SMOKE_GUTTER_HOLD").unwrap_or_default();
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            pause(700).await;
            let pts = ed
                .read_with(cx, |e, _| {
                    (e.breakpoint_point(0), e.breakpoint_point(5), e.run_icon_point(9), e.fold_marker_point(3), e.char_point(4, 6), e.scroll_state().4)
                })
                .ok();
            let Some((Some(empty), Some(bp), run, fold, Some(text), Some(b))) = pts else {
                eprintln!("smoke-gutter: no layout");
                return;
            };
            let mut fails = 0;
            let mut step = async |label: &str, (x, y): (f32, f32), want: &str, cx: &mut gpui::AsyncWindowContext| {
                post(Mouse::Move, x * scale, y * scale, false);
                pause(200).await;
                let (cur, st) = ed
                    .read_with(cx, |e, _| {
                        let cur = match e.pointer_style() {
                            gpui::CursorStyle::PointingHand => "hand",
                            gpui::CursorStyle::Arrow => "arrow",
                            gpui::CursorStyle::IBeam => "ibeam",
                            _ => "other",
                        };
                        (cur, Some(e.gutter_state_for_smoke()))
                    })
                    .unwrap_or(("none", None));
                let ok = cur == want;
                if !ok {
                    fails += 1;
                }
                eprintln!("smoke-gutter: {label}: cursor {cur} (want {want}{}), state {st:?}", if ok { "" } else { ", FAIL" });
            };
            step("text", (f32::from(text.x), f32::from(text.y) + 4.), "ibeam", cx).await;
            step("line number 1 (empty)", empty, "hand", cx).await;
            if hold == "preview" {
                pause(500).await;
                let _ = ed.update(cx, |e, cx| {
                    e.hold_gutter_hover_for_smoke(empty);
                    cx.notify();
                });
                eprintln!("smoke-gutter: holding");
                return;
            }
            step("breakpoint, line 6", bp, "hand", cx).await;
            if hold == "tip" {
                pause(500).await;
                let _ = ed.update(cx, |e, cx| {
                    e.hold_gutter_hover_for_smoke(bp);
                    cx.notify();
                });
                eprintln!("smoke-gutter: holding");
                return;
            }
            match run {
                Some(p) => step("run mark, line 10", p, "hand", cx).await,
                None => eprintln!("smoke-gutter: no run mark on line 10, FAIL"),
            }
            match fold {
                Some(p) => step("fold marker, line 4", (f32::from(p.x), f32::from(p.y)), "hand", cx).await,
                None => eprintln!("smoke-gutter: no fold marker on line 4, FAIL"),
            }
            // The bookmark column: nothing to click there.
            step("gutter, bookmark column", (f32::from(b.left()) + 9., empty.1), "arrow", cx).await;
            step("text again", (f32::from(text.x), f32::from(text.y) + 4.), "ibeam", cx).await;
            eprintln!("smoke-gutter: done, {fails} failed");
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
