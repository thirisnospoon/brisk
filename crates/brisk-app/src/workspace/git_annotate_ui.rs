//! Annotate with Git Blame (the gutter menu, Find Action) and its commit diffs. A right click
//! on an annotation gives PyCharm's menu: Annotate Previous Revision (the file as it was
//! before that commit, annotated, in a read-only tab), Show Diff, Copy Revision Number,
//! Close Annotations.

use super::menu_ui::Entry;
use super::{Tab, Workspace};
use crate::editor::annotate::Annotation;
use crate::editor::{Editor, EditorEvent};
use brisk_git::{BlameLine, FileChange, FileStatus};
use gpui::prelude::*;
use gpui::{ClipboardItem, Context, Entity, Pixels, Point, Window};
use std::rc::Rc;

impl Workspace {
    /// Right click on the line numbers: PyCharm's gutter menu; on an annotation, its own menu.
    pub(super) fn open_gutter_menu(&mut self, ed: Entity<Editor>, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let shown = ed.read(cx).annotation_shown();
        let tracked = self.annotate_target(&ed, cx).is_some();
        let label = if shown { "Close Annotations" } else { "Annotate with Git Blame" };
        let shortcut = self.resolver.keymap().shortcut_label(brisk_keymap::Action::Annotate).unwrap_or_default();
        let hash = ed.read(cx).annotation_at(pos).and_then(|l| ed.read(cx).annotation_hash(l));
        let mut entries = Vec::new();
        if let Some(h) = hash {
            let (e1, h1) = (ed.clone(), h.clone());
            entries.push(Entry::Item {
                label: "Annotate Previous Revision".into(),
                shortcut: String::new(),
                enabled: true,
                run: Rc::new(move |this, window, cx| this.annotate_previous(e1.clone(), h1.clone(), window, cx)),
            });
            let (e2, h2) = (ed.clone(), h.clone());
            entries.push(Entry::Item {
                label: "Show Diff".into(),
                shortcut: String::new(),
                enabled: true,
                run: Rc::new(move |this, window, cx| this.show_annotation_commit(e2.clone(), h2.clone(), window, cx)),
            });
            entries.push(Entry::Item {
                label: "Copy Revision Number".into(),
                shortcut: String::new(),
                enabled: true,
                run: Rc::new(move |this, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(h.clone()));
                    this.status = format!("Copied {}", &h[..h.len().min(8)]);
                    cx.notify();
                }),
            });
            entries.push(Entry::Separator);
        }
        entries.push(Entry::Item {
            label: label.into(),
            shortcut,
            enabled: shown || tracked,
            run: Rc::new(move |this, _, cx| this.toggle_annotate(ed.clone(), cx)),
        });
        self.open_menu(pos, entries, cx);
    }

    /// The repository-relative path of an editor's file, if it is in the repository (for a
    /// file at a revision: its path there).
    fn annotate_target(&self, ed: &Entity<Editor>, cx: &gpui::App) -> Option<String> {
        let g = self.git(cx)?;
        let e = ed.read(cx);
        if let Some((rel, _)) = &e.revision {
            return Some(rel.clone());
        }
        let path = e.path.clone()?;
        let rel = g.repo.rel(&path)?;
        (self.file_status(&path, cx) != Some(FileStatus::Untracked)).then_some(rel)
    }

    /// Annotate with Git Blame / Close Annotations.
    pub(super) fn toggle_annotate(&mut self, ed: Entity<Editor>, cx: &mut Context<Self>) {
        if ed.read(cx).annotation_shown() {
            ed.update(cx, |e, cx| e.set_annotation(None, cx));
            return;
        }
        if self.git(cx).is_none() {
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
        let Some(repo) = self.git(cx).map(|g| g.repo.clone()) else { return };
        let (text, version, rev) = {
            let e = ed.read(cx);
            (e.buffer.text(), e.buffer.version(), e.revision.as_ref().map(|r| r.1.clone()))
        };
        let task = cx.background_executor().spawn(async move {
            match rev {
                Some(rev) => repo.blame_at(&rev, &rel),
                None => repo.blame(&rel, Some(&text)),
            }
        });
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
                e.revision.is_none() && e.annotation.as_ref().is_some_and(|a| head_changed || (a.version != e.buffer.version() && !e.buffer.is_dirty()))
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
        let Some(repo) = self.git(cx).map(|g| g.repo.clone()) else { return };
        let from = ed.read(cx).revision.as_ref().map(|r| r.1.clone());
        // The file may have had another name in that commit (`git log --follow`): off the UI thread.
        let h = hash.clone();
        let task = cx.background_executor().spawn(async move {
            let path = repo.path_at(from.as_deref(), &h, &rel);
            repo.commit_files(&h).ok().and_then(|v| v.into_iter().find(|c| c.path == path)).unwrap_or(FileChange {
                path,
                old_path: None,
                status: FileStatus::Modified,
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let change = task.await;
            let _ = this.update_in(cx, |this, window, cx| this.show_commit_diff(&hash, &change, window, cx));
        })
        .detach();
    }

    /// Annotate Previous Revision: the file as it was before `hash` changed this line,
    /// annotated, in a read-only tab.
    pub(super) fn annotate_previous(&mut self, ed: Entity<Editor>, hash: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rel) = self.annotate_target(&ed, cx) else { return };
        let Some(repo) = self.git(cx).map(|g| g.repo.clone()) else { return };
        let from = ed.read(cx).revision.as_ref().map(|r| r.1.clone());
        let task = cx.background_executor().spawn(async move {
            let (parent, path) = repo
                .previous_revision(&hash, &rel, from.as_deref())
                .ok_or_else(|| "The file was added in this commit: it has no previous revision".to_string())?;
            let text =
                repo.file_at(&parent, &path).map_err(|e| e.to_string())?.ok_or_else(|| format!("{path} is not in {}", &parent[..8.min(parent.len())]))?;
            let blame = repo.blame_at(&parent, &path).map_err(|e| e.to_string())?;
            Ok::<_, String>((parent, path, text, blame))
        });
        cx.spawn_in(window, async move |this, cx| {
            let res = task.await;
            let _ = this.update_in(cx, |this, window, cx| match res {
                Ok((parent, path, text, blame)) => this.open_revision(path, parent, &text, blame, window, cx),
                Err(e) => this.show_toast(format!("Annotate Previous Revision: {e}"), true, cx),
            });
        })
        .detach();
    }

    /// A read-only, annotated tab with a file at a revision.
    pub(super) fn open_revision(&mut self, rel: String, rev: String, text: &str, blame: Vec<BlameLine>, window: &mut Window, cx: &mut Context<Self>) {
        let key = (rel.clone(), rev.clone());
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Editor(e) if e.read(cx).revision.as_ref() == Some(&key))) {
            self.active = Some(i);
        } else {
            let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            let title = format!("{name} ({})", &rev[..8.min(rev.len())]);
            let lang = brisk_syntax::Lang::from_path(std::path::Path::new(&rel));
            let ed = cx.new(|cx| {
                let mut e = Editor::scratch(text, cx);
                e.set_language(lang);
                e.buffer.read_only = true;
                e.show_gutter = true;
                e.title_override = Some(title);
                e.revision = Some(key);
                let mut a = Annotation::from_blame(blame);
                a.version = e.buffer.version();
                e.set_annotation(Some(a), cx);
                e
            });
            cx.subscribe_in(&ed, window, |this: &mut Self, ed, ev: &EditorEvent, window, cx| match ev {
                EditorEvent::GutterMenu(at) => this.open_gutter_menu(ed.clone(), *at, cx),
                EditorEvent::ShowCommit(hash) => this.show_annotation_commit(ed.clone(), hash.clone(), window, cx),
                _ => {}
            })
            .detach();
            self.add_tab(Tab::Editor(ed));
        }
        self.focus_editor(window, cx);
        cx.notify();
    }
}
