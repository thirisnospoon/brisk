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
    }

    fn edit2_rest(&mut self, window: &mut Window, cx: &mut Context<Self>) {
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
        self.dispatch(Action::CloseAllTabs, window, cx);
        eprintln!("smoke-edit2: close all -> {:?}", self.panes_for_smoke(cx));
        eprintln!("smoke-edit2: done");
        cx.quit();
    }
}
