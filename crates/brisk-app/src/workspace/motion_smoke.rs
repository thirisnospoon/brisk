//! BRISK_SMOKE_MOTION=1 on an empty folder: smooth scrolling (wheel, Page Down / Page Up,
//! Ctrl+End / Ctrl+Home, Go to Line, an arrow key at the edge, smooth scrolling off), the
//! caret's fading blink (and how many frames an idle second costs) and the smooth caret
//! movement setting. Prints one line per check; `=hold` leaves the window open.
//! BRISK_SMOKE_PALETTE: a selection over three lines, find hits and a bracket match (all
//! rounded) and the caret block's indent guide; `=hold` leaves them on screen, `=caret` holds
//! with a plain caret at a bracket (bracket match and identifier occurrences).

use super::Workspace;
use crate::editor::Editor;
use gpui::{Context, Entity, Window};
use std::time::{Duration, Instant};

/// A long Python file with nested blocks (sticky lines use it too).
pub(super) fn long_python() -> String {
    let mut s = String::from("import os\n\n\n");
    for c in 0..12 {
        s.push_str(&format!("class Service{c}:\n    \"\"\"Service number {c}.\"\"\"\n\n    def __init__(self, name: str, size: int = {c}):\n        self.name = name\n        self.size = size\n\n"));
        for m in 0..6 {
            s.push_str(&format!("    def method_{m}(self, value, *args, **kwargs):\n        total = 0\n        for i in range(value):\n            if i % 3 == {}:\n", m % 3));
            for k in 0..8 {
                s.push_str(&format!("                total += i * {k}  # step {k}\n"));
            }
            s.push_str(&format!("        return f\"{{self.name}}: {{total}}\"\n\n"));
        }
        s.push('\n');
    }
    s
}

impl Workspace {
    pub(super) fn motion_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("motion.py");
        let _ = std::fs::write(&file, long_python());
        self.open_file(&file, window, cx);
        let hold = std::env::var("BRISK_SMOKE_MOTION").as_deref() == Ok("hold");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            pause(600).await;
            let Ok(Some(ed)) = this.update(cx, |t, _| t.active_editor().cloned()) else { return };
            let probe = |cx: &mut gpui::AsyncWindowContext, ed: &Entity<Editor>| ed.read_with(cx, |e, _| e.motion_probe()).ok();
            // Run `act`, then sample until the glide settles: (settle ms, frames, monotonic, scroll from, to).
            async fn settle(cx: &mut gpui::AsyncWindowContext, ed: &Entity<Editor>, bg: &gpui::BackgroundExecutor) -> (f32, u64, bool, f32) {
                let t0 = Instant::now();
                let p0 = ed.read_with(cx, |e, _| e.motion_probe()).ok();
                let (mut last, mut mono, mut dir) = (p0.map(|p| p.scroll_y).unwrap_or(0.0), true, 0.0f32);
                let paints0 = p0.map(|p| p.paints).unwrap_or(0);
                let mut paints = paints0;
                while t0.elapsed() < Duration::from_secs(2) {
                    bg.timer(Duration::from_millis(3)).await;
                    let Some(p) = ed.read_with(cx, |e, _| e.motion_probe()).ok() else { break };
                    let d = p.scroll_y - last;
                    if d != 0.0 {
                        if dir != 0.0 && d.signum() != dir {
                            mono = false;
                        }
                        dir = d.signum();
                    }
                    last = p.scroll_y;
                    paints = p.paints;
                    if p.target.is_none() {
                        break;
                    }
                }
                (t0.elapsed().as_secs_f32() * 1000.0, paints - paints0, mono, last)
            }
            let lh = probe(cx, &ed).map(|p| p.line_height).unwrap_or(20.0);

            // Wheel: three notches down in quick succession glide to one target.
            let _ = ed.update(cx, |e, cx| {
                for _ in 0..3 {
                    e.smooth_scroll(-3.0 * lh, cx);
                }
            });
            let target = probe(cx, &ed).and_then(|p| p.target);
            let (ms, frames, mono, end) = settle(cx, &ed, &bg).await;
            eprintln!("smoke-motion: wheel x3 -> target {target:?}, settled in {ms:.0} ms, {frames} frames, monotonic {mono}, end {end} ({} rows)", end / lh);

            // Page Down: the view moves a page and the caret keeps its row on screen.
            let _ = this.update_in(cx, |t, w, cx| {
                if let Some(e) = t.active_editor().cloned() {
                    e.update(cx, |e, cx| e.go_to_line(30, cx));
                }
                let _ = w;
            });
            let _ = settle(cx, &ed, &bg).await;
            let before = probe(cx, &ed);
            let _ = this.update_in(cx, |t, w, cx| t.smoke_press("pagedown", w, cx));
            let mid = probe(cx, &ed);
            let (ms, frames, mono, end) = settle(cx, &ed, &bg).await;
            let after = probe(cx, &ed);
            eprintln!(
                "smoke-motion: pagedown -> glide {} , settled in {ms:.0} ms, {frames} frames, monotonic {mono}, scrolled {:.1} rows, caret row on screen {:.1} -> {:.1}",
                mid.and_then(|p| p.target).is_some(),
                (end - before.map(|p| p.scroll_y).unwrap_or(0.0)) / lh,
                before.map(|p| p.caret_screen_row).unwrap_or(-1.0),
                after.map(|p| p.caret_screen_row).unwrap_or(-1.0)
            );
            let _ = this.update_in(cx, |t, w, cx| t.smoke_press("pageup", w, cx));
            let (ms, _, mono, end) = settle(cx, &ed, &bg).await;
            eprintln!("smoke-motion: pageup -> settled in {ms:.0} ms, monotonic {mono}, back to {:.1} rows (was {:.1})", end / lh, before.map(|p| p.scroll_y / lh).unwrap_or(-1.0));

            // Held Page Down: presses faster than the glide keep one moving target.
            for _ in 0..4 {
                let _ = this.update_in(cx, |t, w, cx| t.smoke_press("pagedown", w, cx));
                pause(40).await;
            }
            let (_, _, mono, end) = settle(cx, &ed, &bg).await;
            eprintln!("smoke-motion: pagedown x4 held -> monotonic {mono}, at {:.1} rows, caret row on screen {:.1}", end / lh, probe(cx, &ed).map(|p| p.caret_screen_row).unwrap_or(-1.0));

            for (keys, what) in [("ctrl-end", "ctrl-end"), ("ctrl-home", "ctrl-home")] {
                let _ = this.update_in(cx, |t, w, cx| t.smoke_press(keys, w, cx));
                let gliding = probe(cx, &ed).and_then(|p| p.target);
                let (ms, frames, mono, end) = settle(cx, &ed, &bg).await;
                eprintln!("smoke-motion: {what} -> glide to {gliding:?}, settled in {ms:.0} ms, {frames} frames, monotonic {mono}, end {end}");
            }
            let _ = ed.update(cx, |e, cx| e.go_to_line(400, cx));
            let gliding = probe(cx, &ed).and_then(|p| p.target);
            let (ms, frames, mono, _) = settle(cx, &ed, &bg).await;
            eprintln!("smoke-motion: go to line 401 -> glide to {gliding:?}, settled in {ms:.0} ms, {frames} frames, monotonic {mono}, caret row on screen {:.1}", probe(cx, &ed).map(|p| p.caret_screen_row).unwrap_or(-1.0));

            // An arrow key at the bottom edge scrolls at once.
            let rows = probe(cx, &ed).map(|p| p.caret_screen_row).unwrap_or(0.0);
            let _ = this.update_in(cx, |t, w, cx| {
                for _ in 0..80 {
                    t.smoke_press("down", w, cx);
                }
            });
            let p = probe(cx, &ed);
            eprintln!("smoke-motion: 80 x down from screen row {rows:.1} -> no glide {}, caret row on screen {:.1}", p.is_some_and(|p| p.target.is_none()), p.map(|p| p.caret_screen_row).unwrap_or(-1.0));

            // Idle: the caret fades out and in; frames are drawn only during the fades.
            pause(300).await;
            let paints0 = probe(cx, &ed).map(|p| p.paints).unwrap_or(0);
            let t0 = Instant::now();
            let (mut lo, mut hi, mut mid_values) = (1.0f32, 0.0f32, 0usize);
            while t0.elapsed() < Duration::from_millis(2000) {
                pause(8).await;
                if let Some(p) = probe(cx, &ed) {
                    lo = lo.min(p.alpha);
                    hi = hi.max(p.alpha);
                    if p.alpha > 0.02 && p.alpha < 0.98 {
                        mid_values += 1;
                    }
                }
            }
            let paints = probe(cx, &ed).map(|p| p.paints).unwrap_or(0) - paints0;
            eprintln!("smoke-motion: idle 2 s -> caret opacity {lo:.2}..{hi:.2}, {mid_values} samples mid-fade, {paints} frames ({:.0}/s)", paints as f32 / 2.0);
            let _ = this.update_in(cx, |t, w, cx| t.smoke_type("x", w, cx));
            pause(30).await;
            eprintln!("smoke-motion: typing -> caret opacity {:.2}", probe(cx, &ed).map(|p| p.alpha).unwrap_or(-1.0));

            // Smooth caret movement (off by default): End slides the caret.
            let mut s = crate::settings::current();
            eprintln!("smoke-motion: defaults -> smooth scrolling {}, caret blinking {}, smooth caret {}", s.smooth_scrolling, s.caret_blinking, s.smooth_caret);
            for slide in [false, true] {
                s.smooth_caret = slide;
                crate::settings::apply(&s);
                let _ = this.update_in(cx, |t, w, cx| t.smoke_press("home", w, cx));
                pause(150).await;
                let x0 = probe(cx, &ed).map(|p| p.caret_x).unwrap_or(0.0);
                let _ = this.update_in(cx, |t, w, cx| t.smoke_press("end", w, cx));
                let mut xs = Vec::new();
                let t0 = Instant::now();
                while t0.elapsed() < Duration::from_millis(150) {
                    pause(3).await;
                    if let Some(p) = probe(cx, &ed) {
                        if xs.last() != Some(&p.caret_x) {
                            xs.push(p.caret_x);
                        }
                    }
                }
                let x1 = xs.last().copied().unwrap_or(x0);
                let between = xs.iter().filter(|&&x| x > x0 + 0.5 && x < x1 - 0.5).count();
                eprintln!("smoke-motion: smooth caret {slide} -> end: x {x0:.0} -> {x1:.0}, {between} positions in between");
            }

            // Smooth scrolling off: the wheel jumps.
            s.smooth_caret = false;
            s.smooth_scrolling = false;
            crate::settings::apply(&s);
            let y0 = probe(cx, &ed).map(|p| p.scroll_y).unwrap_or(0.0);
            let _ = ed.update(cx, |e, cx| e.smooth_scroll(-3.0 * lh, cx));
            let p = probe(cx, &ed);
            eprintln!("smoke-motion: smooth scrolling off -> wheel moved {:.1} rows at once, glide {}", p.map(|p| p.scroll_y - y0).unwrap_or(0.0) / lh, p.is_some_and(|p| p.target.is_some()));
            crate::settings::apply(&crate::settings::Settings::default());
            eprintln!("smoke-motion: done");
            if !hold {
                let _ = this.update(cx, |_, cx| cx.quit());
            }
        })
        .detach();
    }
}

impl Workspace {
    /// BRISK_SMOKE_STICKY=1 on an empty folder: sticky lines inside a method (class and def),
    /// on the def line itself (class only), pushed up at the end of the method, a click on the
    /// class line (it lands where it was shown) and the setting off. `=hold` stays with two
    /// sticky lines and the mouse over the second (for a screenshot).
    pub(super) fn sticky_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("sticky.py");
        let text = long_python();
        let _ = std::fs::write(&file, &text);
        self.open_file(&file, window, cx);
        let lines: Vec<&str> = text.lines().collect();
        let class3 = lines.iter().position(|l| l.starts_with("class Service3")).unwrap_or(0);
        let def2 = class3 + lines[class3..].iter().position(|l| l.starts_with("    def method_2")).unwrap_or(0);
        let ret2 = def2 + lines[def2..].iter().position(|l| l.trim_start().starts_with("return")).unwrap_or(0);
        let hold = std::env::var("BRISK_SMOKE_STICKY").as_deref() == Ok("hold");
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            let pause = |ms| bg.timer(Duration::from_millis(ms));
            pause(800).await;
            let Ok(Some(ed)) = this.update(cx, |t, _| t.active_editor().cloned()) else { return };
            let lh = ed.read_with(cx, |e, _| e.motion_probe().line_height).unwrap_or(20.0);
            let at = async |cx: &mut gpui::AsyncWindowContext, ed: &Entity<Editor>, top_line: f32| {
                let _ = ed.update(cx, |e, cx| e.set_scroll_for_smoke(top_line * lh, cx));
                bg.timer(Duration::from_millis(120)).await;
                ed.read_with(cx, |e, _| e.sticky_shown().iter().map(|&(l, y)| (l + 1, (y / lh * 100.0).round() / 100.0)).collect::<Vec<_>>()).unwrap_or_default()
            };
            eprintln!("smoke-sticky: class Service3 at line {}, def method_2 at {}, its return at {}", class3 + 1, def2 + 1, ret2 + 1);
            let s = at(cx, &ed, (def2 + 3) as f32).await;
            eprintln!("smoke-sticky: top inside method_2 -> {s:?} (line, row)");
            let s = at(cx, &ed, def2 as f32).await;
            eprintln!("smoke-sticky: def line at the top -> {s:?}");
            let s = at(cx, &ed, ret2 as f32 - 0.5).await;
            eprintln!("smoke-sticky: method's last line under the panel -> {s:?} (the def pushed up)");
            let s = at(cx, &ed, 0.0).await;
            eprintln!("smoke-sticky: at the top of the file -> {s:?}");
            let s = at(cx, &ed, (def2 + 5) as f32).await;
            if hold {
                // Hover the def line for the screenshot.
                let _ = ed.update(cx, |e, cx| e.hover_sticky_for_smoke(1, cx));
                eprintln!("smoke-sticky: holding with {s:?}");
                return;
            }
            let _ = ed.update(cx, |e, cx| e.click_sticky_for_smoke(0, cx));
            let (gliding, caret) = ed.read_with(cx, |e, _| (e.motion_probe().target.is_some(), e.caret_line_col())).unwrap_or_default();
            pause(300).await;
            let top = ed.read_with(cx, |e, _| e.line_top_for_smoke(class3) / lh).unwrap_or(-1.0);
            eprintln!("smoke-sticky: click on the class line -> caret {:?}, glide {gliding}, class line drawn at row {top:.2}", (caret.0 + 1, caret.1));
            let mut st = crate::settings::current();
            st.sticky_lines = false;
            crate::settings::apply(&st);
            let s = at(cx, &ed, (def2 + 3) as f32).await;
            eprintln!("smoke-sticky: setting off -> {s:?}");
            crate::settings::apply(&crate::settings::Settings::default());
            eprintln!("smoke-sticky: done");
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }

    pub(super) fn palette_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("palette.py");
        let text = long_python();
        let _ = std::fs::write(&file, &text);
        self.open_file(&file, window, cx);
        let anchor = text.find("value, *args").unwrap_or(0);
        let head = text.find("range(value)").map_or(0, |i| i + 5);
        let mode = std::env::var("BRISK_SMOKE_PALETTE").unwrap_or_default();
        // `=caret`: a plain caret at the bracket instead (bracket match and occurrences show).
        let anchor = if mode == "caret" { head } else { anchor };
        let hold = mode == "hold" || mode == "caret";
        cx.spawn_in(window, async move |this, cx| {
            let bg = cx.background_executor().clone();
            bg.timer(Duration::from_millis(800)).await;
            let Ok(Some(ed)) = this.update(cx, |t, _| t.active_editor().cloned()) else { return };
            let _ = cx.update(|window, cx| ed.update(cx, |e, cx| e.find_for_smoke("total", window, cx)));
            bg.timer(Duration::from_millis(300)).await;
            let _ = cx.update(|window, cx| ed.update(cx, |e, cx| e.select_for_smoke(anchor, head, window, cx)));
            bg.timer(Duration::from_millis(300)).await;
            let (hits, guide, marks) = ed.update(cx, |e, _| (e.find_hits_for_smoke(), e.active_guide_for_smoke(), e.caret_marks_for_smoke())).unwrap_or_default();
            eprintln!("smoke-palette: find hits {hits}, caret block guide {guide:?} (column, lines), caret marks {marks:?}");
            if hold {
                eprintln!("smoke-palette: holding");
                return;
            }
            eprintln!("smoke-palette: done");
            let _ = this.update(cx, |_, cx| cx.quit());
        })
        .detach();
    }
}
