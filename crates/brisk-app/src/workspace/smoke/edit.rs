//! Smoke check of editor features (BRISK_SMOKE_EDIT=<file>): keys go through the same
//! `on_key_down` path as real key presses, typed text through the platform input handler.
//! Use a scratch copy of a file: the check edits it.

use crate::editor::Editor;
use crate::workspace::Workspace;
use gpui::{Context, Entity, EntityInputHandler, Focusable, KeyDownEvent, Keystroke, Window};
use std::path::PathBuf;

impl Workspace {
    pub(in crate::workspace) fn smoke_press(&mut self, keys: &str, window: &mut Window, cx: &mut Context<Self>) {
        for k in keys.split(' ') {
            let ev = KeyDownEvent { keystroke: Keystroke::parse(k).expect("keystroke"), is_held: false };
            self.on_key_down(&ev, window, cx);
        }
    }

    pub(in crate::workspace) fn smoke_type(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ed) = self.focused_editor(window, cx) {
            ed.update(cx, |e, cx| e.replace_text_in_range(None, text, window, cx));
        }
    }

    fn smoke_editor(&self) -> Option<Entity<Editor>> {
        self.active_editor().cloned()
    }

    pub(in crate::workspace) fn edit_smoke(&mut self, file: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
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
        use crate::workspace::smoke::win_input::{post, Mouse};
        let long = file.with_file_name("long_lines.py");
        let mut text = String::new();
        for i in 0..400 {
            if i == 9 {
                text.push_str(&format!(
                    "x = '{}'
",
                    "abcdefghij".repeat(50)
                ));
            } else {
                text.push_str(&format!(
                    "line_{i} = {i}
"
                ));
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
            crate::workspace::smoke::win_input::right_click(sx(rx), sx(ry));
            pause(250).await;
            let labels = this
                .read_with(cx, |this, _| {
                    this.menu.as_ref().map(|m| {
                        m.entries
                            .iter()
                            .map(|e| match e {
                                crate::workspace::menu_ui::Entry::Item { label, enabled, .. } => format!("{label}{}", if *enabled { "" } else { " (off)" }),
                                crate::workspace::menu_ui::Entry::Separator => "—".into(),
                                crate::workspace::menu_ui::Entry::Submenu { label, .. } => format!("{label} ›"),
                            })
                            .collect::<Vec<_>>()
                    })
                })
                .ok()
                .flatten();
            eprintln!("smoke-edit: right click -> menu {labels:?}");
            if let Some(labels) = labels {
                // Rows: 5 px padding, 26 px items, 9 px separators.
                let mut y = ry + 5.;
                for l in &labels {
                    if l.starts_with("Find…") {
                        crate::workspace::smoke::win_input::click(sx(rx + 60.), sx(y + 13.));
                        break;
                    }
                    y += if l == "—" { 9. } else { 26. };
                }
            }
            pause(250).await;
            let bar = ed.read_with(cx, |e, _| e.find_state().3).unwrap_or(false);
            let menu_open = this.read_with(cx, |this, _| this.menu.is_some()).unwrap_or(true);
            eprintln!("smoke-edit: clicked 'Find…' -> find bar open={bar}, menu open={menu_open}");
            crate::workspace::smoke::win_input::right_click(sx(rx), sx(ry));
            pause(250).await;
            crate::workspace::smoke::win_input::click(sx(5.), sx(top + h - 5.));
            pause(250).await;
            let menu_open = this.read_with(cx, |this, _| this.menu.is_some()).unwrap_or(true);
            eprintln!("smoke-edit: click outside the menu -> menu open={menu_open}");
            // A Python rename / move first asks ty for the import updates and finishes later:
            // wait (bounded) for each such step before the next.
            let mut pending = this.update_in(cx, |this, window, cx| this.files_smoke(0, window, cx)).unwrap_or(false);
            for stage in 1..=2 {
                let mut waited = 0;
                while pending && waited < 100 {
                    pause(100).await;
                    waited += 1;
                    pending = this.read_with(cx, |this, _| this.file_op.is_some() || this.lsp.moved.is_some()).unwrap_or(false);
                }
                if pending {
                    eprintln!("smoke-edit: FAIL file operation still pending after {} ms", waited * 100);
                }
                pending = this.update_in(cx, |this, window, cx| this.files_smoke(stage, window, cx)).unwrap_or(false);
            }
            // Right click on a Project view row (scan down the panel until a menu opens).
            let mut y = 50.;
            while y < 260. {
                crate::workspace::smoke::win_input::right_click(sx(120.), sx(y));
                pause(120).await;
                let got = this.read_with(cx, |this, _| this.menu.as_ref().map(|m| (this.tree_selected, m.entries.len()))).ok().flatten();
                if let Some((sel, n)) = got {
                    let name = this.read_with(cx, |this, _| sel.map(|i| this.tree.nodes[i].name.clone())).ok().flatten();
                    eprintln!("smoke-edit: right click on tree row at y={y} -> menu with {n} entries for {name:?}");
                    let _ = this.update(cx, |this, cx| {
                        this.menu = None;
                        cx.notify();
                    });
                    break;
                }
                y += 8.;
            }
            eprintln!("smoke-edit: done");
            let _ = cx.update(|_, cx| cx.quit());
        })
        .detach();
    }

    pub(in crate::workspace) fn smoke_query(&mut self, text: &str) {
        if let Some(p) = self.popup.as_mut() {
            p.query = text.into();
            p.selected = 0;
        }
        self.refresh_popup();
    }

    pub(in crate::workspace) fn smoke_popup(&self) -> String {
        match &self.popup {
            None => "closed".into(),
            Some(p) => format!("{:?} {:?}", p.kind, p.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()),
        }
    }

    fn smoke_select(&mut self, p: &std::path::Path, window: &mut Window) {
        self.left = Some(crate::workspace::LeftTool::Project);
        self.tree_selected = self.tree.reveal(p);
        window.focus(&self.tree_focus);
    }

    /// New / rename / copy / move / delete through the Project view's keys, in stages: 0 ends
    /// with the rename and 1 with the move, which may finish later (ty is asked about imports
    /// first). Returns whether the caller should wait for a file operation before the next.
    fn files_smoke(&mut self, stage: u8, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let root = self.root.clone();
        let exists = |p: &str| root.join(p).exists();
        match stage {
            0 => {}
            1 => {
                self.files_smoke_after_rename(window, cx);
                return self.file_op.is_some();
            }
            _ => {
                self.files_smoke_after_move(window, cx);
                return false;
            }
        }
        self.smoke_select(&root.join("models.py"), window);
        self.smoke_press("alt-insert", window, cx);
        // File, Scratch File, Directory, Python Package, Python File.
        self.smoke_press("down down down down down enter", window, cx);
        self.smoke_query("pkg/helper");
        eprintln!("smoke-edit: new python file popup -> {}", self.smoke_popup());
        self.smoke_press("enter", window, cx);
        let tab = self.active_editor().and_then(|e| e.read(cx).path.clone());
        eprintln!(
            "smoke-edit: created pkg/helper.py={} open tab={:?}",
            exists("pkg/helper.py"),
            tab.as_ref().map(|p| p.strip_prefix(&root).unwrap_or(p).to_path_buf())
        );
        if let Some(e) = self.active_editor().cloned() {
            e.update(cx, |e, cx| e.replace_text_in_range(None, "x = 1", window, cx));
        }
        self.smoke_select(&root.join("pkg/helper.py"), window);
        self.smoke_press("shift-f6", window, cx);
        eprintln!("smoke-edit: rename popup -> {}", self.smoke_popup());
        self.smoke_query("util.py");
        self.smoke_press("enter", window, cx);
        self.file_op.is_some()
    }

    fn files_smoke_after_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let exists = |p: &str| root.join(p).exists();
        let tab = self.active_editor().and_then(|e| e.read(cx).path.clone());
        let saved = std::fs::read_to_string(root.join("pkg/util.py")).unwrap_or_default();
        eprintln!(
            "smoke-edit: renamed -> helper.py={} util.py={} (content {saved:?}) tab now {:?}",
            exists("pkg/helper.py"),
            exists("pkg/util.py"),
            tab.as_ref().map(|p| p.strip_prefix(&root).unwrap_or(p).to_path_buf())
        );
        self.smoke_select(&root.join("pkg/util.py"), window);
        self.smoke_press("f5", window, cx);
        self.smoke_query("pkg/util.py");
        eprintln!("smoke-edit: copy onto itself -> {}", self.smoke_popup());
        self.smoke_query("pkg/util_copy.py");
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-edit: copied -> util_copy.py={}", exists("pkg/util_copy.py"));
        self.smoke_select(&root.join("pkg/util_copy.py"), window);
        self.smoke_press("f6", window, cx);
        self.smoke_query(".");
        self.smoke_press("enter", window, cx);
    }

    fn files_smoke_after_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        let exists = |p: &str| root.join(p).exists();
        eprintln!("smoke-edit: moved to root -> pkg/util_copy.py={} util_copy.py={}", exists("pkg/util_copy.py"), exists("util_copy.py"));
        self.smoke_select(&root.join("util_copy.py"), window);
        self.smoke_press("delete", window, cx);
        eprintln!("smoke-edit: delete popup -> {}", self.smoke_popup());
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-edit: deleted -> util_copy.py={} tabs={}", exists("util_copy.py"), self.tabs.len());
        let rows: Vec<String> = self.tree.rows().to_vec().iter().map(|r| format!("{}{}", "  ".repeat(r.depth), self.tree.nodes[r.id].name)).collect();
        eprintln!("smoke-edit: tree now {rows:?}");
    }
}
