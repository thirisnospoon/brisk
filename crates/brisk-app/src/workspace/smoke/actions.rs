//! BRISK_SMOKE_ACTIONS=1 on an empty folder: keys and menu entries that used to end in "not
//! implemented yet". Enter and F4 in the Project view, Alt+Insert there with no selection and
//! File › New…, Copy Path (Ctrl+Shift+C) in the editor, Reload All from Disk (Ctrl+Alt+Y), and
//! the Python tool windows in Tools. Keys go through `on_key_down`. Exits with 1 on a failure.

use crate::workspace::Workspace;
use gpui::{ClipboardItem, Context, Window};

fn check(failed: &mut bool, what: &str, ok: bool, got: String) {
    eprintln!("smoke-actions: {what}: {} ({got})", if ok { "ok" } else { "FAIL" });
    *failed |= !ok;
}

impl Workspace {
    pub(in crate::workspace) fn actions_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut failed = false;
        let a = self.root.join("a.py");
        let _ = std::fs::write(&a, "a = 1\n");
        let _ = std::fs::write(self.root.join("b.py"), "b = 1\n");
        self.tree.reload(0);
        let open_names = |this: &Workspace, cx: &gpui::App| -> Vec<String> {
            this.editors().filter_map(|e| e.read(cx).path.as_ref()?.file_name().map(|n| n.to_string_lossy().into_owned())).collect()
        };

        // Enter and F4 on a file row open it.
        for key in ["enter", "f4"] {
            let tabs: Vec<usize> = (0..self.tabs.len()).collect();
            for i in tabs.into_iter().rev() {
                self.close_tab(i, window, cx);
            }
            self.tree_selected = self.tree.reveal(&a);
            window.focus(&self.tree_focus);
            self.smoke_press(key, window, cx);
            let open = open_names(self, cx);
            check(
                &mut failed,
                &format!("{key} on a.py in the Project view"),
                open.contains(&"a.py".to_string()),
                format!("open={open:?}, status={:?}", self.status),
            );
        }

        // Alt+Insert in the Project view with nothing selected, and File › New….
        self.tree_selected = None;
        window.focus(&self.tree_focus);
        self.menu = None;
        self.smoke_press("alt-insert", window, cx);
        let labels = self.menu.as_ref().map(|m| m.entries.iter().flat_map(|e| e.labels()).collect::<Vec<_>>()).unwrap_or_default();
        check(&mut failed, "Alt+Insert, no selection", labels.iter().any(|l| l == "New Python File…"), format!("menu={labels:?}, status={:?}", self.status));
        self.menu = None;
        self.open_file(&a, window, cx);
        self.dispatch(brisk_keymap::Action::NewElement, window, cx);
        let labels = self.menu.as_ref().map(|m| m.entries.iter().flat_map(|e| e.labels()).collect::<Vec<_>>()).unwrap_or_default();
        check(&mut failed, "File › New…", labels.iter().any(|l| l == "New Python File…"), format!("menu={labels:?}, status={:?}", self.status));
        self.menu = None;

        // Rename / Copy / Move / Delete keys in the Project view come from the keymap.
        let b = self.root.join("b.py");
        for (key, want) in [("shift-f6", "Rename"), ("f5", "Copy"), ("f6", "Move"), ("delete", "Delete")] {
            self.tree_selected = self.tree.reveal(&b);
            window.focus(&self.tree_focus);
            self.smoke_press(key, window, cx);
            let op = self.file_op.as_ref().map(|o| format!("{o:?}")).unwrap_or_default();
            check(
                &mut failed,
                &format!("{key} on b.py in the Project view"),
                op.starts_with(want) && self.popup.is_some(),
                format!("op={op}, status={:?}", self.status),
            );
            self.smoke_press("escape", window, cx);
            self.file_op = None;
        }
        self.tree_selected = Some(0);
        window.focus(&self.tree_focus);
        self.smoke_press("shift-f6", window, cx);
        check(&mut failed, "Shift+F6 on the project root", self.popup.is_none() && self.status.contains("project root"), format!("status={:?}", self.status));

        // Copy Path in the editor.
        cx.write_to_clipboard(ClipboardItem::new_string(String::new()));
        self.focus_editor(window, cx);
        self.smoke_press("ctrl-shift-c", window, cx);
        let clip = cx.read_from_clipboard().and_then(|c| c.text()).unwrap_or_default();
        check(&mut failed, "Ctrl+Shift+C in the editor", clip == a.to_string_lossy(), format!("clipboard={clip:?}, status={:?}", self.status));

        // Reload All from Disk: an outside edit shows in the editor without waiting for the poll.
        let _ = std::fs::write(&a, "a = 2\n");
        self.smoke_press("ctrl-alt-y", window, cx);
        let ed = self.active_editor().cloned();
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let mut failed = failed;
            // Well under the 1.5 s disk poll.
            bg.timer(std::time::Duration::from_millis(400)).await;
            let text = ed.and_then(|e| e.read_with(cx, |e, _| e.buffer.text()).ok()).unwrap_or_default();
            let _ = this.update(cx, |this, cx| {
                check(
                    &mut failed,
                    "Ctrl+Alt+Y",
                    text == "a = 2\n" && this.status == "Reloaded all files from disk",
                    format!("editor={text:?}, status={:?}", this.status),
                );

                // Tools lists the Python tool windows.
                let menu = this.main_menu_entries();
                let tools: Vec<String> = menu.iter().flat_map(|e| e.labels()).skip_while(|l| l != "Tools:").collect();
                check(
                    &mut failed,
                    "Tools menu",
                    tools.iter().any(|l| l == "Python Console") && tools.iter().any(|l| l == "Python Packages"),
                    format!("{tools:?}"),
                );
                this.menu = None;
                this.tree_selected = this.tree.reveal(&this.root.join("b.py"));
                cx.notify();
            });

            // A real right click below the last row of the Project view: the root's menu.
            bg.timer(std::time::Duration::from_millis(300)).await;
            let at = cx.update(|window, _| window.scale_factor()).ok().zip(this.update(cx, |this, _| this.tree_scroll.0.borrow().base_handle.bounds()).ok());
            if let Some((scale, list)) = at {
                let (x, y) = (f32::from(list.left()) + 40., f32::from(list.bottom()) - 12.);
                crate::workspace::smoke::win_input::right_click(x * scale, y * scale);
            }
            bg.timer(std::time::Duration::from_millis(300)).await;
            let _ = this.update(cx, |this, cx| {
                let labels = this.menu.as_ref().map(|m| m.entries.iter().flat_map(|e| e.labels()).collect::<Vec<_>>()).unwrap_or_default();
                let rename = this.menu.as_ref().and_then(|m| {
                    m.entries.iter().find_map(|e| match e {
                        crate::workspace::menu_ui::Entry::Item { label, shortcut, .. } if label == "Rename…" => Some(shortcut.clone()),
                        _ => None,
                    })
                });
                check(
                    &mut failed,
                    "right click below the rows",
                    this.tree_selected == Some(0) && labels.iter().any(|l| l == "Reload from Disk"),
                    format!("selected={:?}, menu={labels:?}", this.tree_selected),
                );
                check(&mut failed, "Rename label from the keymap", rename.as_deref() == Some("Shift+F6"), format!("{rename:?}"));
                eprintln!("smoke-actions: done");
                if failed {
                    std::process::exit(1);
                }
                cx.quit();
            });
        })
        .detach();
    }
}
