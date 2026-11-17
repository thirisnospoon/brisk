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

    /// The Stage 2 checks after folding (filled in as features land).
    fn edit2_more(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        eprintln!("smoke-edit2: done");
        cx.quit();
    }
}
