//! Show Local History: the revisions of the current file; Enter shows the diff with the
//! current text, Ctrl+Enter reverts to the revision (one Ctrl+Z step).

use super::{PopupItem, PopupKind, Target, Workspace};
use crate::diff_view::DiffView;
use crate::local_history::{self, Label};
use gpui::prelude::*;
use gpui::{Context, Window};
use std::path::PathBuf;

impl Workspace {
    pub(super) fn show_local_history(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else {
            self.status = "Local History: open a file first".into();
            return;
        };
        let (path, current) = {
            let e = ed.read(cx);
            (e.path.clone(), e.buffer.text())
        };
        let Some(path) = path else { return };
        let revs = local_history::revisions(&path);
        if revs.is_empty() {
            self.status = "Local History: no revisions yet (they are kept on save)".into();
            return;
        }
        self.structure = revs
            .iter()
            .map(|r| PopupItem {
                label: format!("{}  —  {}", r.age(), r.label.title()),
                detail: local_history::change_summary(&r.text(), &current),
                hint: String::new(),
                target: Target::History(path.clone(), r.at),
            })
            .collect();
        self.open_popup(PopupKind::LocalHistory);
    }

    fn history_revision(path: &PathBuf, at: u64) -> Option<local_history::Revision> {
        local_history::revisions(path).into_iter().find(|r| r.at == at)
    }

    /// Enter: the revision against the current text.
    pub(super) fn history_diff(&mut self, path: PathBuf, at: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rev) = Self::history_revision(&path, at) else { return };
        let current = self.editors().find(|e| e.read(cx).path.as_ref() == Some(&path)).map(|e| e.read(cx).buffer.text());
        let current = current.unwrap_or_else(|| std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n"));
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let title = format!("{name} (Local History ↔ Current)");
        let left = format!("{} — {}", rev.age(), rev.label.title());
        let text = rev.text();
        let view = cx.new(|cx| DiffView::new(title, Some(path.clone()), left, &text, "Current".into(), &current, cx));
        self.open_diff(view, window, cx);
    }

    /// Ctrl+Enter: put the revision's text into the editor (the current text is kept first).
    pub(super) fn history_revert(&mut self, path: PathBuf, at: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rev) = Self::history_revision(&path, at) else { return };
        let Some(ed) = self.editors().find(|e| e.read(cx).path.as_ref() == Some(&path)).cloned() else { return };
        let current = ed.read(cx).buffer.text();
        local_history::record(&path, &current, Label::BeforeRevert);
        let text = rev.text();
        ed.update(cx, |e, cx| e.replace_all_text(&text, cx));
        self.status = format!("Reverted to the revision from {} (Ctrl+Z undoes)", rev.age());
        self.focus_editor(window, cx);
    }
}
