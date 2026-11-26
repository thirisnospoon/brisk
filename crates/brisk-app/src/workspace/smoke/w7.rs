//! `BRISK_SMOKE_W7=1` (on an empty folder, with a scratch APPDATA: writes `a.py`, `sub/b.py`):
//! Local History (revisions on save, Put Label between them, a deleted file in the folder's
//! history and restored by Ctrl+Enter), multi-caret copy/paste spread over the carets, column
//! selection mode (Shift+Alt+Insert + Shift+arrows), and Replace with "In Selection" (on by
//! itself for a multi-line selection) and "Preserve case".

use crate::workspace::Workspace;
use brisk_keymap::Action;
use brisk_text::Selection;
use gpui::{Context, Window};

impl Workspace {
    pub(in crate::workspace) fn w7_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let a = self.root.join("a.py");
        let b = self.root.join("sub").join("b.py");
        let _ = std::fs::create_dir_all(b.parent().unwrap());
        if std::fs::write(&a, "x = 1\n").and_then(|_| std::fs::write(&b, "y = 2\n")).is_err() {
            eprintln!("smoke-w7: cannot write the files");
            return cx.quit();
        }
        let root = self.root.clone();
        self.reload_tree_for(&root, cx);

        // --- E1 Local History ---------------------------------------------------------------
        self.open_file(&a, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return cx.quit() };
        let append = |this: &mut Self, line: &str, window: &mut Window, cx: &mut Context<Self>| {
            ed.update(cx, |e, _| {
                let end = e.buffer.len_chars();
                e.buffer.set_caret(end, false);
                e.buffer.insert(line);
            });
            this.dispatch(Action::SaveAll, window, cx);
        };
        append(self, "x = 2\n", window, cx);
        self.dispatch(Action::PutLabel, window, cx);
        self.smoke_query("release 1");
        eprintln!("smoke-w7: put label popup {}", self.smoke_popup());
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-w7: status {:?}", self.status);
        append(self, "x = 3\n", window, cx);
        self.dispatch(Action::ShowLocalHistory, window, cx);
        eprintln!("smoke-w7: file history {}", self.smoke_popup());
        self.smoke_press("escape", window, cx);
        let gz = brisk_history::revisions(&a).len();
        eprintln!("smoke-w7: revisions of a.py {gz}");

        // Delete sub/b.py through the Project view's Delete, then restore it from the folder's history.
        self.start_file_op(crate::workspace::files_ui::FileOp::Delete(b.clone()), cx);
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-w7: after delete exists={}", b.exists());
        let root = self.root.clone();
        self.show_path_history(root, cx);
        let items: Vec<String> = self.popup.as_ref().map(|p| p.items.iter().map(|i| format!("{} | {}", i.label, i.detail)).collect()).unwrap_or_default();
        eprintln!("smoke-w7: folder history {items:?}");
        if let Some(p) = self.popup.as_mut() {
            p.selected = p.items.iter().position(|i| i.label.starts_with("sub/b.py")).unwrap_or(0);
        }
        self.smoke_press("ctrl-enter", window, cx);
        eprintln!("smoke-w7: restored exists={} text={:?} status {:?}", b.exists(), std::fs::read_to_string(&b).ok(), self.status);

        // --- E2 multi-caret copy / paste and column mode --------------------------------------
        self.open_file(&a, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return cx.quit() };
        ed.update(cx, |e, _| {
            e.buffer.set_text("aa\nbb\ncc\n");
            e.buffer.selections = vec![Selection::caret(4), Selection::caret(1), Selection::caret(7)];
        });
        self.smoke_press("ctrl-c", window, cx);
        let clip = cx.read_from_clipboard().and_then(|c| c.text());
        ed.update(cx, |e, _| e.buffer.selections = vec![Selection::caret(6), Selection::caret(0), Selection::caret(3)]);
        self.smoke_press("ctrl-v", window, cx);
        eprintln!("smoke-w7: multi-caret copy {clip:?} -> paste {:?}", ed.read(cx).buffer.text());
        ed.update(cx, |e, _| {
            e.buffer.set_text("abcd\nef\nghij\n");
            e.buffer.selections = vec![Selection::caret(1)];
        });
        self.smoke_press("alt-shift-insert shift-right shift-right shift-down shift-down", window, cx);
        let e = ed.read(cx);
        eprintln!("smoke-w7: column mode {} -> selections {:?} text {:?}", e.column_mode(), e.carets_for_smoke(), e.buffer.selected_text());
        self.smoke_press("alt-shift-insert", window, cx);
        eprintln!("smoke-w7: column mode off -> {}", ed.read(cx).column_mode());

        // --- E3 Replace: In Selection + Preserve case ----------------------------------------
        ed.update(cx, |e, _| {
            e.buffer.set_text("foo Foo FOO\nfoo bar\nfoo\n");
            // Lines 1-2 selected: "In Selection" turns on by itself.
            e.buffer.selections = vec![Selection { anchor: 0, head: 19, goal_col: None }];
        });
        self.smoke_press("ctrl-r", window, cx);
        self.smoke_type("foo", window, cx);
        self.smoke_press("tab", window, cx);
        self.smoke_type("bar", window, cx);
        self.smoke_press("alt-p", window, cx);
        let (n, _, q, open) = ed.read(cx).find_state();
        eprintln!("smoke-w7: replace bar open={open} query={q:?} matches in selection {n}");
        self.smoke_press("ctrl-alt-enter", window, cx);
        eprintln!("smoke-w7: replace all (in selection, preserve case) -> {:?}", ed.read(cx).buffer.text());
        self.smoke_press("escape", window, cx);
        // Regex groups: (\w+)@(\w+) -> $2.$1
        ed.update(cx, |e, _| {
            e.buffer.set_text("ann@x bob@y\n");
            e.buffer.selections = vec![Selection::caret(0)];
        });
        self.smoke_press("ctrl-r", window, cx);
        self.smoke_type(r"(\w+)@(\w+)", window, cx);
        self.smoke_press("alt-x tab", window, cx);
        self.smoke_type("$2.$1", window, cx);
        self.smoke_press("ctrl-alt-enter", window, cx);
        eprintln!("smoke-w7: regex replace -> {:?}", ed.read(cx).buffer.text());
        self.smoke_press("escape", window, cx);
        ed.update(cx, |e, _| e.buffer.mark_saved());
        self.w7_smoke2(window, cx);
    }
}
