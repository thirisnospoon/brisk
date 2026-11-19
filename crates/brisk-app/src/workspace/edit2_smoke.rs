//! Smoke check of Stage 2 editor features (BRISK_SMOKE_EDIT2=1, on a scratch project):
//! caret marks, folding, and the rest as they land. Actions go through `dispatch`, the
//! same path as key presses; gutter clicks are real Windows mouse messages.

use super::Workspace;
use brisk_keymap::Action;
use gpui::{Context, Window};

const SRC: &str = "import os
import sys


class Repo:
    def __init__(self):
        self.items = [
            1,
            2,
        ]

    def add(self, item):
        self.items.append(item)
        return item


def main():
    repo = Repo()
    repo.add(os.getcwd())
    print(sys.argv, repo)
";

impl Workspace {
    pub(super) fn edit2_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("stage2.py");
        let _ = std::fs::write(&file, SRC);
        self.open_file(&file, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        let folds = |cx: &mut Context<Self>| ed.read(cx).fold_state();

        // --- Caret marks ---
        ed.update(cx, |e, cx| {
            e.go_to_line(17, cx);
            e.buffer.set_caret(e.buffer.line_col_to_pos(17, 6), false);
        });
        let (br, occ) = ed.update(cx, |e, _| e.caret_marks_for_smoke());
        eprintln!("smoke-edit2: caret on 'repo' -> occurrences={occ} brackets={br:?}");
        ed.update(cx, |e, _| e.buffer.set_caret(e.buffer.line_col_to_pos(17, 15), false));
        let (br, _) = ed.update(cx, |e, _| e.caret_marks_for_smoke());
        eprintln!("smoke-edit2: caret before '(' of Repo() -> brackets={br:?}");
        ed.update(cx, |e, _| e.buffer.set_caret(e.buffer.line_col_to_pos(6, 22), false));
        let (br, _) = ed.update(cx, |e, _| e.caret_marks_for_smoke());
        eprintln!("smoke-edit2: caret after '[' -> brackets={br:?}");

        // --- Folding ---
        self.dispatch(Action::CollapseAll, window, cx);
        eprintln!("smoke-edit2: collapse all -> {:?}", folds(cx));
        self.dispatch(Action::ExpandAll, window, cx);
        eprintln!("smoke-edit2: expand all -> {:?}", folds(cx));
        ed.update(cx, |e, _| e.buffer.set_caret(e.buffer.line_col_to_pos(11, 8), false));
        self.dispatch(Action::CollapseRegion, window, cx);
        eprintln!("smoke-edit2: ctrl-minus on 'def add' -> {:?}", folds(cx));
        self.dispatch(Action::MoveDown, window, cx);
        eprintln!("smoke-edit2: down from the header -> caret line {}", ed.read(cx).caret_line_col().0);
        self.dispatch(Action::MoveUp, window, cx);
        eprintln!("smoke-edit2: up -> caret line {}", ed.read(cx).caret_line_col().0);
        ed.update(cx, |e, _| {
            e.buffer.set_caret(0, false);
            e.buffer.insert("# top\n");
        });
        self.dispatch(Action::MoveDown, window, cx);
        eprintln!("smoke-edit2: line typed above -> {:?}", folds(cx));
        ed.update(cx, |e, cx| e.go_to_line(13, cx));
        eprintln!("smoke-edit2: go to a hidden line -> {:?}", folds(cx));
        self.dispatch(Action::Undo, window, cx);

        // Gutter marker click (real mouse messages, after a repaint).
        let bg = cx.background_executor().clone();
        let scale = window.scale_factor();
        cx.spawn_in(window, async move |this, cx| {
            let wait = |ms| bg.timer(std::time::Duration::from_millis(ms));
            wait(300).await;
            let p = ed.read_with(cx, |e, _| e.fold_marker_point(4)).ok().flatten();
            if let Some(p) = p {
                let (x, y) = (f32::from(p.x) * scale, f32::from(p.y) * scale);
                super::win_input::post(super::win_input::Mouse::Move, x, y, false);
                wait(150).await;
                super::win_input::click(x, y);
                wait(250).await;
                eprintln!("smoke-edit2: click the marker of 'class Repo' -> {:?}", ed.read_with(cx, |e, _| e.fold_state()).ok());
                super::win_input::click(x, y);
                wait(250).await;
                eprintln!("smoke-edit2: click it again -> {:?}", ed.read_with(cx, |e, _| e.fold_state()).ok());
            } else {
                eprintln!("smoke-edit2: no fold marker point for line 4");
            }
            let _ = this.update_in(cx, |this, window, cx| this.edit2_more(window, cx));
        })
        .detach();
    }

    /// The Stage 2 checks after folding.
    fn edit2_more(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Reformat and Optimize Imports need ruff: wait for interpreter discovery.
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            for _ in 0..100 {
                if this.read_with(cx, |this, _| this.runs.all.is_some()).unwrap_or(true) {
                    break;
                }
                bg.timer(std::time::Duration::from_millis(100)).await;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.format_smoke(window, cx);
                this.edit2_rest(window, cx);
            });
        })
        .detach();
    }

    fn format_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let file = self.root.join("messy.py");
        let _ = std::fs::write(&file, "import sys\nimport os\nimport json\ndef f( a,b ):\n  return {'a':a,\n'b':b}\nx=f(1,2)\ny = [ 1,2 ]\nprint(os.getcwd(),x,y)\n");
        self.open_file(&file, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        self.dispatch(Action::OptimizeImports, window, cx);
        eprintln!("smoke-edit2: optimize imports -> {:?} | status {:?}", ed.read(cx).text_string().lines().take(2).collect::<Vec<_>>(), self.status);
        // Select only the "y = [ 1,2 ]" line: the rest stays as it is.
        ed.update(cx, |e, _| {
            let l = e.text_string().lines().position(|l| l.starts_with("y =")).unwrap_or(0);
            let (a, z) = (e.buffer.line_start(l), e.buffer.line_end(l));
            e.buffer.selections = vec![brisk_text::Selection { anchor: a, head: z, goal_col: None }];
        });
        self.dispatch(Action::ReformatCode, window, cx);
        let t = ed.read(cx).text_string();
        eprintln!("smoke-edit2: reformat selection -> y line {:?}, x line {:?}", t.lines().find(|l| l.starts_with("y")), t.lines().find(|l| l.starts_with("x")));
        ed.update(cx, |e, _| e.buffer.set_caret(0, false));
        self.dispatch(Action::ReformatCode, window, cx);
        eprintln!("smoke-edit2: reformat file -> {:?} | status {:?}", ed.read(cx).text_string(), self.status);
        self.dispatch(Action::Undo, window, cx);
        eprintln!("smoke-edit2: undo -> back to the selection format: {}", ed.read(cx).text_string().contains("x=f(1,2)"));
        // Actions on Save: Optimize imports + Reformat code on Ctrl+S.
        let mut s = crate::settings::current();
        (s.reformat_on_save, s.optimize_imports_on_save) = (true, true);
        crate::settings::apply(&s);
        ed.update(cx, |e, cx| e.replace_all_text("import sys\nimport os\nx=os.sep\n", cx));
        self.dispatch(Action::SaveAll, window, cx);
        let disk = std::fs::read_to_string(&file).unwrap_or_default();
        eprintln!("smoke-edit2: actions on save -> disk {disk:?}, dirty {}", ed.read(cx).buffer.is_dirty());
        (s.reformat_on_save, s.optimize_imports_on_save) = (false, false);
        crate::settings::apply(&s);
        // Code Cleanup: every safe ruff fix (the unused import goes).
        ed.update(cx, |e, cx| e.replace_all_text("import sys\nx = 1\n", cx));
        self.dispatch(Action::CodeCleanup, window, cx);
        eprintln!("smoke-edit2: code cleanup -> {:?} | status {:?}", ed.read(cx).text_string(), self.status);
    }

    fn edit2_rest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // --- Method navigation, File Structure, bookmarks ---
        let file = self.root.join("stage2.py");
        self.open_file(&file, window, cx);
        let Some(ed) = self.active_editor().cloned() else { return };
        ed.update(cx, |e, _| e.buffer.set_caret(0, false));
        let mut seen = Vec::new();
        for a in [Action::NextMethod, Action::NextMethod, Action::NextMethod, Action::PreviousMethod] {
            self.dispatch(a, window, cx);
            seen.push(ed.read(cx).caret_line_col());
        }
        eprintln!("smoke-edit2: alt-down x3, alt-up -> {seen:?}");
        self.dispatch(Action::FileStructurePopup, window, cx);
        let sel = self.popup.as_ref().map(|p| p.selected);
        eprintln!("smoke-edit2: file structure -> {} selected={sel:?}", self.smoke_popup());
        self.smoke_query("add");
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-edit2: 'add' + enter -> caret {:?}", ed.read(cx).caret_line_col());
        self.dispatch(Action::ToggleBookmark, window, cx);
        ed.update(cx, |e, _| e.buffer.set_caret(e.buffer.line_start(16), false));
        self.dispatch(Action::ToggleBookmarkWithMnemonic, window, cx);
        self.smoke_query("3");
        eprintln!("smoke-edit2: ctrl-f11 popup -> {}", self.smoke_popup());
        self.smoke_press("enter", window, cx);
        eprintln!("smoke-edit2: bookmarks in the editor -> {:?}", ed.update(cx, |e, _| e.bookmark_lines()));
        ed.update(cx, |e, _| e.buffer.set_caret(0, false));
        self.smoke_press("ctrl-3", window, cx);
        eprintln!("smoke-edit2: ctrl-3 -> caret line {}", ed.read(cx).caret_line_col().0);
        ed.update(cx, |e, _| {
            e.buffer.set_caret(0, false);
            e.buffer.insert("# shifted
");
        });
        self.dispatch(Action::ShowBookmarks, window, cx);
        eprintln!("smoke-edit2: after inserting a line, shift-f11 -> {}", self.smoke_popup());
        self.popup = None;
        self.dispatch(Action::Undo, window, cx);
        self.dispatch(Action::CloseTab, window, cx);
        eprintln!("smoke-edit2: tab closed, stored -> {:?}", self.all_bookmarks(cx).iter().map(|b| (b.line, b.mnemonic)).collect::<Vec<_>>());
        self.open_file(&file, window, cx);
        let back = self.active_editor().map(|e| e.update(cx, |e, _| e.bookmark_lines()));
        eprintln!("smoke-edit2: reopened -> {back:?}, saved file has project: {}", std::fs::read_to_string(brisk_db::config_dir().join("bookmarks.json")).map(|s| s.contains("stage2.py")).unwrap_or(false));

        // --- Extend / Shrink Selection, Clone Caret, Paste from History ---
        if let Some(ed) = self.active_editor().cloned() {
            ed.update(cx, |e, _| e.buffer.set_caret(e.buffer.line_col_to_pos(19, 15), false));
            let mut grown = Vec::new();
            for _ in 0..5 {
                self.dispatch(Action::ExtendSelection, window, cx);
                grown.push(ed.read(cx).buffer.selected_text());
            }
            eprintln!("smoke-edit2: ctrl-w x5 -> {grown:?}");
            let mut shrunk = Vec::new();
            for _ in 0..2 {
                self.dispatch(Action::ShrinkSelection, window, cx);
                shrunk.push(ed.read(cx).buffer.selected_text());
            }
            eprintln!("smoke-edit2: ctrl-shift-w x2 -> {shrunk:?}");

            ed.update(cx, |e, _| e.buffer.set_caret(e.buffer.line_col_to_pos(12, 8), false));
            let mods = |control: bool| gpui::ModifiersChangedEvent { modifiers: gpui::Modifiers { control, ..Default::default() }, ..Default::default() };
            self.on_modifiers(&mods(true), window, cx);
            self.on_modifiers(&mods(false), window, cx);
            self.on_modifiers(&mods(true), window, cx);
            self.smoke_press("ctrl-up ctrl-up", window, cx);
            self.on_modifiers(&mods(false), window, cx);
            eprintln!("smoke-edit2: ctrl, ctrl+up x2 -> {:?}", ed.read(cx).carets_for_smoke());
            self.on_modifiers(&mods(true), window, cx);
            self.smoke_press("ctrl-up", window, cx);
            self.on_modifiers(&mods(false), window, cx);
            eprintln!("smoke-edit2: single ctrl+up (no clone) -> carets {}", ed.read(cx).carets_for_smoke().len());
            self.dispatch(Action::CloneCaretBelow, window, cx);
            eprintln!("smoke-edit2: clone caret below -> {:?}", ed.read(cx).carets_for_smoke());

            for (l, a, b) in [(18usize, 4usize, 8usize), (5, 8, 16)] {
                ed.update(cx, |e, _| {
                    e.buffer.set_caret(e.buffer.line_col_to_pos(l, a), false);
                    e.buffer.set_caret(e.buffer.line_col_to_pos(l, b), true);
                });
                self.dispatch(Action::Copy, window, cx);
            }
            ed.update(cx, |e, _| e.buffer.set_caret(e.buffer.line_start(20.min(e.buffer.len_lines() - 1)), false));
            self.dispatch(Action::PasteFromHistory, window, cx);
            eprintln!("smoke-edit2: paste from history -> {}", self.smoke_popup());
            self.smoke_press("down enter", window, cx);
            let last = ed.read(cx).buffer.text();
            eprintln!("smoke-edit2: picked the 2nd entry -> file ends with {:?}", &last[last.len().saturating_sub(12)..]);
            self.dispatch(Action::Undo, window, cx);
            eprintln!("smoke-edit2: undo -> back to source: {}", ed.read(cx).buffer.text() == SRC);
        }

        // --- Tabs and split ---
        let third = self.root.join("third.py");
        let _ = std::fs::write(&third, "x = 3
");
        self.open_file(&third, window, cx);
        eprintln!("smoke-edit2: tabs -> {}", self.panes_for_smoke(cx));
        self.dispatch(Action::SplitVertically, window, cx);
        eprintln!("smoke-edit2: split right -> {}", self.panes_for_smoke(cx));
        self.open_file(&self.root.join("stage2.py"), window, cx);
        eprintln!("smoke-edit2: open a file of the other group -> {}", self.panes_for_smoke(cx));
        self.dispatch(Action::MoveToOppositeGroup, window, cx);
        eprintln!("smoke-edit2: move to opposite group -> {}", self.panes_for_smoke(cx));
        self.dispatch(Action::NextSplitter, window, cx);
        eprintln!("smoke-edit2: next splitter -> {}", self.panes_for_smoke(cx));
        let editors = self.editors().count();
        eprintln!("smoke-edit2: editors in both groups -> {editors}");
        self.tab_menu(false, 0, gpui::point(gpui::px(300.), gpui::px(60.)), window, cx);
        let labels: Vec<String> = self.menu.as_ref().map(|m| m.entries.iter().filter_map(|e| match e {
            super::menu_ui::Entry::Item { label, enabled, .. } => Some(format!("{label}{}", if *enabled { "" } else { " (off)" })),
            super::menu_ui::Entry::Separator => None,
            super::menu_ui::Entry::Submenu { label, .. } => Some(format!("{label} ›")),
        }).collect()).unwrap_or_default();
        self.menu = None;
        eprintln!("smoke-edit2: tab menu -> {labels:?}");
        self.dispatch(Action::CloseOtherTabs, window, cx);
        eprintln!("smoke-edit2: close other tabs -> {}", self.panes_for_smoke(cx));
        self.dispatch(Action::ReopenClosedTab, window, cx);
        eprintln!("smoke-edit2: reopen closed tab -> {}", self.panes_for_smoke(cx));
        self.dispatch(Action::Unsplit, window, cx);
        eprintln!("smoke-edit2: unsplit -> {}", self.panes_for_smoke(cx));
        self.dispatch(Action::SplitHorizontally, window, cx);
        eprintln!("smoke-edit2: split down -> {}", self.panes_for_smoke(cx));
        self.dispatch(Action::CloseTab, window, cx);
        eprintln!("smoke-edit2: close the only tab of a group -> {}", self.panes_for_smoke(cx));
        // Pin: the pinned tab moves first and survives Close Other / Close All.
        for f in ["stage2.py", "messy.py"] {
            self.open_file(&self.root.join(f), window, cx);
        }
        self.dispatch(Action::PinTab, window, cx);
        eprintln!("smoke-edit2: pin messy.py -> {} pinned={}", self.panes_for_smoke(cx), self.pinned_count());
        self.dispatch(Action::CloseAllTabs, window, cx);
        eprintln!("smoke-edit2: close all keeps the pinned tab -> {}", self.panes_for_smoke(cx));
        self.dispatch(Action::PinTab, window, cx);
        self.dispatch(Action::CloseAllTabs, window, cx);
        eprintln!("smoke-edit2: unpinned, close all -> {:?}", self.panes_for_smoke(cx));
        eprintln!("smoke-edit2: done");
        cx.quit();
    }
}
