//! Annotate with Git Blame (the gutter menu, Find Action) and its commit diffs.

use super::menu_ui::Entry;
use super::Workspace;
use crate::editor::annotate::Annotation;
use crate::editor::Editor;
use brisk_git::{FileChange, FileStatus};
use gpui::{Context, Entity, Pixels, Point, Window};
use std::rc::Rc;

impl Workspace {
    /// Right click on the line numbers: PyCharm's gutter menu.
    pub(super) fn open_gutter_menu(&mut self, ed: Entity<Editor>, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let shown = ed.read(cx).annotation_shown();
        let tracked = self.annotate_target(&ed, cx).is_some();
        let label = if shown { "Close Annotations" } else { "Annotate with Git Blame" };
        let shortcut = self.resolver.keymap().shortcut_label(brisk_keymap::Action::Annotate).unwrap_or_default();
        let entries = vec![Entry::Item {
            label: label.into(),
            shortcut,
            enabled: shown || tracked,
            run: Rc::new(move |this, _, cx| this.toggle_annotate(ed.clone(), cx)),
        }];
        self.open_menu(pos, entries, cx);
    }

    /// The repository-relative path of an editor's file, if it is in the repository.
    fn annotate_target(&self, ed: &Entity<Editor>, cx: &gpui::App) -> Option<String> {
        let g = self.git.as_ref()?;
        let path = ed.read(cx).path.clone()?;
        let rel = g.repo.rel(&path)?;
        (self.file_status(&path) != Some(FileStatus::Untracked)).then_some(rel)
    }

    /// Annotate with Git Blame / Close Annotations.
    pub(super) fn toggle_annotate(&mut self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        if ed.read(cx).annotation_shown() {
            ed.update(cx, |e, cx| e.set_annotation(None, cx));
            return;
        }
        if self.git.is_none() {
            return self.show_toast("Version control is not enabled for this project".into(), true, cx);
        }
        if self.annotate_target(&ed, cx).is_none() {
            return self.show_toast("Annotate: the file is not under Git".into(), true, cx);
        }
        self.annotate(ed, cx);
    }

    /// (Re)compute the blame of an editor's text in the background.
    pub(super) fn annotate(&mut self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        let Some(rel) = self.annotate_target(&ed, cx) else { return };
        let Some(repo) = self.git.as_ref().map(|g| g.repo.clone()) else { return };
        let (text, version) = {
            let e = ed.read(cx);
            (e.buffer.text(), e.buffer.version())
        };
        let task = cx.background_executor().spawn(async move { repo.blame(&rel, Some(&text)) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| match res {
                Ok(lines) => ed.update(cx, |e, cx| {
                    let mut a = Annotation::from_blame(lines);
                    a.version = version;
                    e.set_annotation(Some(a), cx);
                }),
                Err(err) => this.show_toast(format!("Annotate failed: {err}"), true, cx),
            });
        })
        .detach();
    }

    /// After a save or a new HEAD: refresh open annotations that went stale.
    pub(super) fn refresh_annotations(&mut self, head_changed: bool, cx: &mut Context<Self>) {
        let eds: Vec<Entity<Editor>> = self
            .editors()
            .filter(|e| {
                let e = e.read(cx);
                e.annotation.as_ref().is_some_and(|a| head_changed || (a.version != e.buffer.version() && !e.buffer.is_dirty()))
            })
            .cloned()
            .collect();
        for e in eds {
            self.annotate(e, cx);
        }
    }

    /// A click on an annotation: that commit's diff of the file (PyCharm: Show Diff).
    pub(super) fn show_annotation_commit(&mut self, ed: Entity<Editor>, hash: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rel) = self.annotate_target(&ed, cx) else { return };
        let Some(repo) = self.git.as_ref().map(|g| g.repo.clone()) else { return };
        // The file may have had another name in that commit (`git log --follow`): off the UI thread.
        let h = hash.clone();
        let task = cx.background_executor().spawn(async move {
            let path = repo.path_in_commit(&h, &rel);
            repo.commit_files(&h)
                .ok()
                .and_then(|v| v.into_iter().find(|c| c.path == path))
                .unwrap_or(FileChange { path, old_path: None, status: FileStatus::Modified })
        });
        cx.spawn_in(window, async move |this, cx| {
            let change = task.await;
            let _ = this.update_in(cx, |this, window, cx| this.show_commit_diff(&hash, &change, window, cx));
        })
        .detach();
    }
}
