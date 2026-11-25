//! `BRISK_SMOKE_MOUSE=1` (on an empty folder: writes `mouse.py`, 400 lines): the vertical
//! scrollbar and middle-button scrolling with real Windows mouse messages (WM_MOUSEMOVE,
//! WM_LBUTTONDOWN/UP, WM_MBUTTONDOWN/UP). Prints the hovered scrollbar, a track click (one
//! page), a thumb drag, then a middle press with a small move (no scrolling), a move 100 px
//! down (scrolls, anchor shown), the release (stops), and a middle click without moving on
//! `target()` (Go to Declaration). `BRISK_SMOKE_MOUSE_HOLD=hover|drag|middle` stops in that
//! state for a screenshot.

use super::win_input::{post, Mouse};
use super::Workspace;
use gpui::{Context, Window};
use std::time::Duration;

impl Workspace {
    pub(super) fn mouse_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("mouse.py");
        let mut src = String::from("def target():\n    return 1\n\n\n");
        for i in 4..400 {
            src.push_str(&if i % 10 == 0 { format!("value_{i} = target()\n") } else { format!("line_{i} = {i}\n") });
        }
        if let Err(e) = std::fs::write(&file, src) {
            eprintln!("smoke-mouse: cannot write {}: {e}", file.display());
            return;
        }
        self.open_file(&file, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        let scale = window.scale_factor();
        let hold = std::env::var("BRISK_SMOKE_MOUSE_HOLD").unwrap_or_default();
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            pause(500).await;
            let st = |cx: &mut gpui::AsyncWindowContext| ed.read_with(cx, |e, _| (e.scroll_state(), e.mouse_state_for_smoke(), e.caret_line_col())).ok();
            let show = |label: &str, cx: &mut gpui::AsyncWindowContext| {
                if let Some(((_, y, _, my, _), (hover, hot, thumb, middle), caret)) = st(cx) {
                    eprintln!(
                        "smoke-mouse: {label}: scroll y={y:.0}/{my:.0}, bar hover {hover:?}, hot {hot}, thumb {thumb:?}, middle {middle:?}, caret {caret:?}"
                    );
                }
            };
            let Some(((_, _, _, _, Some(b)), (_, _, Some((top, th)), _), _)) = st(cx) else {
                eprintln!("smoke-mouse: no layout or no scrollbar");
                return;
            };
            let (left, right, btop, h) = (f32::from(b.left()), f32::from(b.right()), f32::from(b.top()), f32::from(b.size.height));
            let s = |v: f32| v * scale;
            let bar_x = s(right - 5.);

            // A: hover over the thumb, then off it.
            post(Mouse::Move, s(left + 200.), s(btop + 100.), false);
            pause(80).await;
            post(Mouse::Move, bar_x, s(btop + top + th / 2.), false);
            pause(150).await;
            show("hover thumb", cx);
            if hold == "hover" {
                eprintln!("smoke-mouse: holding");
                return;
            }
            post(Mouse::Move, s(left + 200.), s(btop + 100.), false);
            pause(150).await;
            show("hover text", cx);
            // Click on the track below the thumb: one page down.
            let ty = s(btop + (top + th + h) / 2.);
            post(Mouse::Move, bar_x, ty, false);
            post(Mouse::Down, bar_x, ty, true);
            post(Mouse::Up, bar_x, ty, false);
            pause(150).await;
            show("click track below thumb", cx);
            // Drag the thumb down by 100 px.
            let Some((_, (_, _, Some((top, th)), _), _)) = st(cx) else { return };
            let gy = btop + top + th / 2.;
            post(Mouse::Move, bar_x, s(gy), false);
            post(Mouse::Down, bar_x, s(gy), true);
            for i in 1..=10 {
                post(Mouse::Move, bar_x, s(gy + 10. * i as f32), true);
                pause(20).await;
            }
            pause(100).await;
            show("dragging thumb +100 px", cx);
            if hold == "drag" {
                eprintln!("smoke-mouse: holding");
                return;
            }
            post(Mouse::Up, bar_x, s(gy + 100.), false);
            pause(150).await;
            show("released thumb", cx);

            // B: middle button held in the text.
            let _ = this.update_in(cx, |this, window, cx| this.smoke_press("ctrl-home", window, cx));
            pause(150).await;
            let (mx, my) = (left + 300., btop + h / 2.);
            post(Mouse::Move, s(mx), s(my), false);
            pause(50).await;
            post(Mouse::MiddleDown, s(mx), s(my), false);
            pause(50).await;
            post(Mouse::MiddleMove, s(mx + 2.), s(my + 3.), false);
            pause(300).await;
            show("middle held, moved 3 px", cx);
            for i in 1..=10 {
                post(Mouse::MiddleMove, s(mx), s(my + 10. * i as f32), false);
                pause(15).await;
            }
            pause(500).await;
            show("middle held, 100 px below", cx);
            if hold == "middle" {
                eprintln!("smoke-mouse: holding");
                return;
            }
            post(Mouse::MiddleUp, s(mx), s(my + 100.), false);
            pause(150).await;
            let y1 = st(cx).map(|v| v.0 .1).unwrap_or(0.);
            pause(300).await;
            let y2 = st(cx).map(|v| v.0 .1).unwrap_or(-1.);
            eprintln!("smoke-mouse: released middle: scroll stays {} ({y1:.0} -> {y2:.0})", y1 == y2);
            show("after release", cx);

            // A middle click without moving on `target()`: Go to Declaration.
            let p = this
                .update(cx, |this, cx| {
                    let e = this.active_editor()?.read(cx);
                    let line = (0..e.buffer.len_lines()).find(|&l| e.char_point(l, 0).is_some() && e.buffer.line(l).contains("= target()"))?;
                    e.char_point(line, e.buffer.line(line).find("target")? + 2).map(|p| (line, p))
                })
                .ok()
                .flatten();
            if let Some((line, p)) = p {
                let (x, y) = (f32::from(p.x) * scale, f32::from(p.y) * scale);
                super::win_input::middle_click(x, y);
                pause(500).await;
                show(&format!("middle click on target() in line {line}"), cx);
            }
            eprintln!("smoke-mouse: done");
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
