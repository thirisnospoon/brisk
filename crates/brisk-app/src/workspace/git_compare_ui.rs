//! Compare with Branch… (a file against its version on another branch) and Show Diff with
//! Working Tree (a file of a commit against the local copy). The local side is the editor's
//! text when the file is open, so unsaved edits are compared too.

use super::git_ui::file_name;
use super::menu_ui::Entry;
use super::Workspace;
use crate::diff_view::DiffView;
use brisk_git::FileChange;
use gpui::{AppContext, Context, Pixels, Point, Window};
use std::path::Path;
use std::rc::Rc;

impl Workspace {
    /// The file as it is now: the open editor's text, or the disk.
    fn working_text(&self, rel: &str, cx: &Context<Self>) -> String {
        let Some(g) = self.git.as_ref() else { return String::new() };
        let abs = g.repo.abs(rel);
        self.editors()
            .find(|e| e.read(cx).path.as_deref().is_some_and(|p| g.repo.rel(p).as_deref() == Some(rel)))
            .map(|e| e.read(cx).text_string())
            .or_else(|| std::fs::read_to_string(&abs).ok().map(|s| s.replace("\r\n", "\n")))
            .unwrap_or_default()
    }

    /// `rev`'s version of the file (at `path_in_rev`) on the left, the local copy of `rel` on
    /// the right.
    pub(super) fn compare_with_rev(&mut self, rel: &str, rev: &str, path_in_rev: &str, label: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.git.as_ref().map(|g| g.repo.clone()) else { return };
        let left = match repo.file_at(rev, path_in_rev) {
            Ok(Some(t)) => t,
            Ok(None) => {
                self.show_toast(format!("{} does not exist in {label}", file_name(path_in_rev)), false, cx);
                String::new()
            }
            Err(e) => return self.show_toast(format!("Cannot read {path_in_rev} in {label}: {e}"), true, cx),
        };
        let abs = repo.abs(rel);
        let right = self.working_text(rel, cx);
        let title = format!("{} ({label} ↔ Local)", file_name(rel));
        let (label, rel) = (label.to_string(), rel.to_string());
        let view = cx.new(|cx| DiffView::new(title, Some(abs), label, &left, format!("Local: {rel}"), &right, cx));
        self.open_diff(view, window, cx);
    }

    /// Compare with Branch…: the branches (but the current one) in a menu at `pos`.
    pub(super) fn compare_with_branch_menu(&mut self, path: &Path, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_ref() else { return };
        let Some(rel) = g.repo.rel(path) else { return };
        let mut v = Vec::new();
        for b in g.branches.iter().filter(|b| !b.current) {
            let (name, rel) = (b.name.clone(), rel.clone());
            v.push(Entry::Item {
                label: if b.remote { format!("⇣ {name}") } else { format!("⎇ {name}") },
                shortcut: String::new(),
                enabled: true,
                run: Rc::new(move |this, window, cx| this.compare_with_rev(&rel, &name, &rel, &name, window, cx)),
            });
        }
        if v.is_empty() {
            return self.show_toast("There is no other branch to compare with".into(), false, cx);
        }
        self.open_menu(pos, v, cx);
    }

    /// Git > Compare with Branch… for the active editor's file.
    pub(super) fn compare_with_branch_action(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.active_editor().and_then(|e| e.read(cx).path.clone()) else {
            return self.show_toast("Open a file to compare it with a branch".into(), false, cx);
        };
        let pos = window.mouse_position();
        self.compare_with_branch_menu(&path, pos, cx);
    }

    /// Right click on a file of a commit (Git log details): its diffs and Open.
    pub(super) fn log_file_menu(&mut self, hash: String, f: FileChange, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let short = hash[..hash.len().min(8)].to_string();
        let exists = self.git.as_ref().is_some_and(|g| g.repo.abs(&f.path).exists());
        let v = vec![
            Entry::Item {
                label: "Show Diff".into(),
                shortcut: "Double click".into(),
                enabled: true,
                run: Rc::new({
                    let (hash, f) = (hash.clone(), f.clone());
                    move |this, window, cx| this.show_commit_diff(&hash, &f, window, cx)
                }),
            },
            Entry::Item {
                label: "Show Diff with Working Tree".into(),
                shortcut: String::new(),
                enabled: true,
                run: Rc::new({
                    let (hash, f) = (hash.clone(), f.clone());
                    move |this, window, cx| this.compare_with_rev(&f.path, &hash, &f.path, &short, window, cx)
                }),
            },
            Entry::Separator,
            Entry::Item {
                label: "Open".into(),
                shortcut: String::new(),
                enabled: exists,
                run: Rc::new(move |this, window, cx| {
                    if let Some(abs) = this.git.as_ref().map(|g| g.repo.abs(&f.path)) {
                        this.open_file(&abs, window, cx);
                    }
                }),
            },
        ];
        self.open_menu(pos, v, cx);
    }
}
