//! Show Local History: the revisions of a file (or of every file under a folder, deleted ones
//! too) with the project's labels between them; Enter shows the diff with the current text,
//! Ctrl+Enter reverts to the revision (one Ctrl+Z step) or restores a deleted file.
//! Put Label: a named point in time of the project.

use super::{PopupItem, PopupKind, Target, Workspace};
use crate::diff_view::DiffView;
use crate::local_history::{self, Label};
use gpui::prelude::*;
use gpui::{Context, Window};
use std::path::{Path, PathBuf};

/// Rows a folder's history shows.
const FOLDER_ROWS: usize = 500;

impl Workspace {
    /// The active editor's file.
    pub(super) fn show_local_history(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.active_editor().and_then(|e| e.read(cx).path.clone()) else {
            self.status = "Local History: open a file first".into();
            return;
        };
        self.show_path_history(path, cx);
    }

    /// A file's revisions, or a folder's changes (Project view → Local History → Show History).
    pub(super) fn show_path_history(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let labels = local_history::labels_for(&path);
        let items = if path.is_dir() { self.folder_history_items(&path, &labels) } else { self.file_history_items(&path, &labels, cx) };
        if items.is_empty() {
            self.status = format!("Local History: no revisions of {} yet (they are kept on save)", self.rel(&path));
            return;
        }
        self.structure = items;
        self.open_popup(PopupKind::LocalHistory);
    }

    fn file_history_items(&self, path: &Path, labels: &[local_history::UserLabel], cx: &Context<Self>) -> Vec<PopupItem> {
        let revs = local_history::revisions(path);
        let current = self.current_text(path, cx);
        let mut items = Vec::new();
        let mut labels = labels.iter().peekable();
        for r in &revs {
            let text = r.text();
            let summary = local_history::change_summary(&text, &current);
            // A label goes above the newest revision not after it: that is the text it marks.
            while let Some(l) = labels.next_if(|l| l.at >= r.at) {
                items.push(PopupItem {
                    label: format!("Label: {}  —  {}", l.name, local_history::age(l.at)),
                    detail: summary.clone(),
                    hint: String::new(),
                    target: Target::History(path.to_path_buf(), r.at),
                });
            }
            items.push(PopupItem {
                label: format!("{}  —  {}", r.age(), r.label.title()),
                detail: summary,
                hint: String::new(),
                target: Target::History(path.to_path_buf(), r.at),
            });
        }
        items
    }

    fn folder_history_items(&self, dir: &Path, labels: &[local_history::UserLabel]) -> Vec<PopupItem> {
        let changes = local_history::history_under(dir, FOLDER_ROWS);
        let mut items = Vec::new();
        let mut labels = labels.iter().peekable();
        for c in &changes {
            while let Some(l) = labels.next_if(|l| l.at >= c.rev.at) {
                items.push(PopupItem {
                    label: format!("Label: {}  —  {}", l.name, local_history::age(l.at)),
                    detail: String::new(),
                    hint: String::new(),
                    target: Target::History(c.path.clone(), c.rev.at),
                });
            }
            let gone = !c.path.exists();
            items.push(PopupItem {
                label: format!("{}  —  {}", self.rel(&c.path), c.rev.label.title()),
                detail: if gone { format!("{} · deleted, Ctrl+Enter restores", c.rev.age()) } else { c.rev.age() },
                hint: String::new(),
                target: Target::History(c.path.clone(), c.rev.at),
            });
        }
        items
    }

    /// A restored file shows up in the Project view: its nearest loaded folder reloads.
    pub(super) fn reload_tree_for(&mut self, path: &Path, cx: &mut Context<Self>) {
        let mut cur = path.parent();
        while let Some(c) = cur {
            if let Some(id) = self.tree.find(c) {
                self.tree.reload(id);
                break;
            }
            cur = c.parent();
        }
        self.reindex_files(cx);
    }

    /// The text in an open editor, else on disk ('\n' line ends); "" for a deleted file.
    fn current_text(&self, path: &Path, cx: &Context<Self>) -> String {
        let open = self.editors().find(|e| e.read(cx).path.as_deref() == Some(path)).map(|e| e.read(cx).buffer.text());
        open.unwrap_or_else(|| std::fs::read_to_string(path).unwrap_or_default().replace("\r\n", "\n"))
    }

    fn history_revision(path: &PathBuf, at: u64) -> Option<local_history::Revision> {
        local_history::revisions(path).into_iter().find(|r| r.at == at)
    }

    /// Enter: the revision against the current text.
    pub(super) fn history_diff(&mut self, path: PathBuf, at: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rev) = Self::history_revision(&path, at) else { return };
        let current = self.current_text(&path, cx);
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let title = format!("{name} (Local History ↔ Current)");
        let left = format!("{} — {}", rev.age(), rev.label.title());
        let right = if path.exists() { "Current".to_string() } else { "Current (deleted)".to_string() };
        let text = rev.text();
        let view = cx.new(|cx| DiffView::new(title, Some(path.clone()), left, &text, right, &current, cx));
        self.open_diff(view, window, cx);
    }

    /// Ctrl+Enter: put the revision's text into the editor (the current text is kept first);
    /// a deleted file is written back to disk and opened.
    pub(super) fn history_revert(&mut self, path: PathBuf, at: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rev) = Self::history_revision(&path, at) else { return };
        let text = rev.text();
        let open = |this: &Self, cx: &Context<Self>| this.editors().find(|e| e.read(cx).path.as_ref() == Some(&path)).cloned();
        if !path.exists() && open(self, cx).is_none() {
            let r = path.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| std::fs::write(&path, &text));
            if let Err(e) = r {
                self.status = format!("Could not restore {}: {e}", self.rel(&path));
                return;
            }
            self.reload_tree_for(&path, cx);
            self.open_file(&path, window, cx);
            self.status = format!("Restored {} from Local History ({})", self.rel(&path), rev.age());
            return;
        }
        if open(self, cx).is_none() {
            self.open_file(&path, window, cx);
        }
        let Some(ed) = open(self, cx) else { return };
        let current = ed.read(cx).buffer.text();
        local_history::record(&path, &current, Label::BeforeRevert);
        ed.update(cx, |e, cx| e.replace_all_text(&text, cx));
        self.status = format!("Reverted to the revision from {} (Ctrl+Z undoes)", rev.age());
        self.focus_editor(window, cx);
    }

    /// Put Label: ask for the name.
    pub(super) fn start_put_label(&mut self) {
        self.open_popup(PopupKind::PutLabel);
    }

    pub(super) fn put_label_items(&self, q: &str) -> Vec<PopupItem> {
        let name = q.trim();
        if name.is_empty() {
            return Vec::new();
        }
        vec![PopupItem { label: format!("Put label '{name}'"), detail: "on the whole project, now".into(), hint: "Enter".into(), target: Target::PutLabel(name.to_string()) }]
    }

    pub(super) fn put_label(&mut self, name: String, cx: &mut Context<Self>) {
        // The open editors' text is the state the label marks: keep it first.
        for ed in self.editors() {
            let e = ed.read(cx);
            if let Some(p) = e.path.as_deref() {
                local_history::record(p, &e.buffer.text(), Label::Saved);
            }
        }
        self.status = match local_history::put_label(&self.root, &name) {
            Ok(()) => format!("Local History: label '{name}' put"),
            Err(e) => format!("Local History: could not put the label: {e}"),
        };
        cx.notify();
    }
}
