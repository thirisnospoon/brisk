//! Smoke check of editor features (BRISK_SMOKE_EDIT=<file>): keys go through the same
//! `on_key_down` path as real key presses, typed text through the platform input handler.
//! Use a scratch copy of a file: the check edits it.

use super::Workspace;
use crate::editor::Editor;
use gpui::{Context, Entity, EntityInputHandler, Focusable, KeyDownEvent, Keystroke, Window};
use std::path::PathBuf;

impl Workspace {
    fn smoke_press(&mut self, keys: &str, window: &mut Window, cx: &mut Context<Self>) {
        for k in keys.split(' ') {
            let ev = KeyDownEvent { keystroke: Keystroke::parse(k).expect("keystroke"), is_held: false };
            self.on_key_down(&ev, window, cx);
        }
    }

    fn smoke_type(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ed) = self.focused_editor(window, cx) {
            ed.update(cx, |e, cx| e.replace_text_in_range(None, text, window, cx));
        }
    }

    fn smoke_editor(&self) -> Option<Entity<Editor>> {
        self.active_editor().cloned()
    }

    pub(super) fn edit_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.open_file(&file, window, cx);
        let Some(ed) = self.smoke_editor() else {
            eprintln!("smoke-edit: cannot open {}", file.display());
            return;
        };
        let state = |ed: &Entity<Editor>, cx: &mut Context<Self>| {
            ed.update(cx, |e, cx| e.refresh_find(false, cx));
            let e = ed.read(cx);
            let (n, cur, q, open) = e.find_state();
            format!("bar={open} query={q:?} matches={n} current={cur:?} selected={:?}", e.buffer.selected_text())
        };
        let field_focused = |this: &Self, ed: &Entity<Editor>, window: &Window, cx: &mut Context<Self>| {
            let _ = this;
            ed.read(cx).find_field_focused(window, cx).is_some()
        };

        // --- Find ---
        self.smoke_press("ctrl-f", window, cx);
        eprintln!("smoke-edit: ctrl-f -> field focused={}", field_focused(self, &ed, window, cx));
        self.smoke_type("user", window, cx);
        eprintln!("smoke-edit: typed 'user' -> {}", state(&ed, cx));
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-edit: enter -> {}", state(&ed, cx));
        self.smoke_press("shift-enter", window, cx);
        eprintln!("smoke-edit: shift-enter -> {}", state(&ed, cx));
        self.smoke_press("alt-c", window, cx);
        eprintln!("smoke-edit: alt-c (match case) -> {}", state(&ed, cx));
        self.smoke_press("alt-c", window, cx);
        self.smoke_press("escape", window, cx);
        eprintln!("smoke-edit: escape -> {} editor focused={}", state(&ed, cx), ed.read(cx).focus_handle(cx).is_focused(window));
        self.smoke_press("f3", window, cx);
        eprintln!("smoke-edit: f3 after close -> {}", state(&ed, cx));

        // --- Replace ---
        self.smoke_press("ctrl-r", window, cx);
        self.smoke_press("tab", window, cx);
        self.smoke_type("member", window, cx);
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-edit: replace one -> {}", state(&ed, cx));
        self.smoke_press("ctrl-alt-enter", window, cx);
        let text = ed.read(cx).buffer.text();
        eprintln!("smoke-edit: replace all -> 'user' left={} 'member' now={}", text.matches("user").count(), text.matches("member").count());
        self.smoke_press("escape", window, cx);
        self.smoke_press("ctrl-z", window, cx);
        self.smoke_press("ctrl-z", window, cx);
        let text = ed.read(cx).buffer.text();
        eprintln!("smoke-edit: undo x2 -> 'user'={} 'member'={}", text.matches("user").count(), text.matches("member").count());

        // --- Regex with groups ---
        self.smoke_press("ctrl-r", window, cx);
        self.smoke_press("alt-x", window, cx);
        self.smoke_press("ctrl-a", window, cx);
        self.smoke_type(r"def (\w+)", window, cx);
        self.smoke_press("tab ctrl-a", window, cx);
        self.smoke_type("def renamed_$1", window, cx);
        eprintln!("smoke-edit: regex -> {}", state(&ed, cx));
        self.smoke_press("ctrl-alt-enter", window, cx);
        let text = ed.read(cx).buffer.text();
        let defs: Vec<&str> = text.lines().filter(|l| l.trim_start().starts_with("def ")).map(str::trim).collect();
        eprintln!("smoke-edit: regex replace all -> {defs:?}");
        self.smoke_press("tab ctrl-a", window, cx);
        self.smoke_type("(", window, cx);
        eprintln!("smoke-edit: '(' in field -> {} (no auto-pair, bad regex reported)", state(&ed, cx));
        self.smoke_press("escape", window, cx);
        self.scroll_smoke(file, window, cx);
    }

    /// Scrolling: keys, a click on the scrollbar track, dragging the thumb, auto-scroll while
    /// selecting past the bottom edge. Real mouse messages, so it runs asynchronously.
    fn scroll_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        use super::win_input::{post, Mouse};
        let long = file.with_file_name("long_lines.py");
        let mut text = String::new();
        for i in 0..400 {
            if i == 9 {
                text.push_str(&format!("x = '{}'
", "abcdefghij".repeat(50)));
            } else {
                text.push_str(&format!("line_{i} = {i}
"));
            }
        }
        let _ = std::fs::write(&long, text);
        self.open_file(&long, window, cx);
        let Some(ed) = self.smoke_editor() else { return };
        let scale = window.scale_factor();
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let pause = |ms| bg.timer(std::time::Duration::from_millis(ms));
            let st = |cx: &mut gpui::AsyncWindowContext| ed.read_with(cx, |e, _| e.scroll_state()).ok();
            let show = |label: &str, s: Option<(f32, f32, f32, f32, Option<gpui::Bounds<gpui::Pixels>>)>| {
                if let Some((x, y, mx, my, _)) = s {
                    eprintln!("smoke-edit: {label}: scroll x={x:.0}/{mx:.0} y={y:.0}/{my:.0}");
                }
            };
            pause(400).await;
            show("opened", st(cx));
            let press = |cx: &mut gpui::AsyncWindowContext, k: &'static str| {
                let _ = this.update_in(cx, |this, window, cx| this.smoke_press(k, window, cx));
            };
            press(cx, "ctrl-end");
            pause(100).await;
            show("ctrl-end", st(cx));
            press(cx, "ctrl-home");
            let _ = ed.update(cx, |e, cx| e.go_to_line(9, cx));
            press(cx, "end");
            pause(100).await;
            show("end of the long line", st(cx));
            press(cx, "home");
            pause(100).await;
            show("home", st(cx));

            let Some((_, _, _, _, Some(b))) = st(cx) else { return };
            let (right, top, h) = (f32::from(b.right()), f32::from(b.top()), f32::from(b.size.height));
            let sx = |v: f32| v * scale;
            // Click on the track at the middle: jump there.
            let (tx, ty) = (sx(right - 6.), sx(top + h / 2.));
            post(Mouse::Move, tx, ty, false);
            post(Mouse::Down, tx, ty, true);
            post(Mouse::Up, tx, ty, false);
            pause(200).await;
            show("click scrollbar middle", st(cx));
            // Drag the thumb to the bottom.
            post(Mouse::Down, tx, ty, true);
            for i in 1..=10 {
                post(Mouse::Move, tx, sx(top + h / 2. + (h / 2. - 4.) * i as f32 / 10.), true);
                pause(20).await;
            }
            post(Mouse::Up, tx, sx(top + h - 4.), false);
            pause(200).await;
            show("drag thumb to bottom", st(cx));
            press(cx, "ctrl-home");
            pause(150).await;
            // Select with the mouse and hold below the editor: auto-scroll.
            let (x0, y0) = (sx(f32::from(b.left()) + 120.), sx(top + 40.));
            post(Mouse::Move, x0, y0, false);
            post(Mouse::Down, x0, y0, true);
            pause(60).await;
            post(Mouse::Move, x0, sx(top + 80.), true);
            pause(60).await;
            post(Mouse::Move, x0, sx(top + h + 30.), true);
            pause(700).await;
            post(Mouse::Up, x0, sx(top + h + 30.), false);
            pause(150).await;
            show("selection drag held below", st(cx));
            let sel = ed.read_with(cx, |e, _| e.buffer.selected_text().lines().count()).unwrap_or(0);
            eprintln!("smoke-edit: selected lines after drag: {sel}");
            // Context menu: right click in the text, pick "Find…" with the mouse.
            let (rx, ry) = (f32::from(b.left()) + 150., top + 100.);
            super::win_input::right_click(sx(rx), sx(ry));
            pause(250).await;
            let labels = this
                .read_with(cx, |this, _| this.menu.as_ref().map(|m| m.entries.iter().map(|e| match e {
                    super::menu_ui::Entry::Item { label, enabled, .. } => format!("{label}{}", if *enabled { "" } else { " (off)" }),
                    super::menu_ui::Entry::Separator => "—".into(),
                }).collect::<Vec<_>>()))
                .ok()
                .flatten();
            eprintln!("smoke-edit: right click -> menu {labels:?}");
            if let Some(labels) = labels {
                // Rows: 5 px padding, 26 px items, 9 px separators.
                let mut y = ry + 5.;
                for l in &labels {
                    if l.starts_with("Find…") {
                        super::win_input::click(sx(rx + 60.), sx(y + 13.));
                        break;
                    }
                    y += if l == "—" { 9. } else { 26. };
                }
            }
            pause(250).await;
            let bar = ed.read_with(cx, |e, _| e.find_state().3).unwrap_or(false);
            let menu_open = this.read_with(cx, |this, _| this.menu.is_some()).unwrap_or(true);
            eprintln!("smoke-edit: clicked 'Find…' -> find bar open={bar}, menu open={menu_open}");
            super::win_input::right_click(sx(rx), sx(ry));
            pause(250).await;
            super::win_input::click(sx(5.), sx(top + h - 5.));
            pause(250).await;
            let menu_open = this.read_with(cx, |this, _| this.menu.is_some()).unwrap_or(true);
            eprintln!("smoke-edit: click outside the menu -> menu open={menu_open}");
            eprintln!("smoke-edit: done");
            let _ = cx.update(|_, cx| cx.quit());
        })
        .detach();
    }
}
