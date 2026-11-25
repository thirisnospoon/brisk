//! Merge conflicts: the Conflicts dialog (Accept Yours / Accept Theirs / Merge…). Merge…
//! opens the merge window (`merge_view`) in a tab: Your version | Result | Changes from <branch>.

use crate::theme::ui_px;
use super::{Tab, Workspace};
use crate::merge_view::{MergeEvent, MergeView};
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, KeyDownEvent, Window};

pub struct ConflictsDialog {
    pub files: Vec<String>,
    pub selected: usize,
}

/// The Git dialogs that sit over the window.
pub enum GitDialog {
    Conflicts(ConflictsDialog),
    Stash(super::git_stash_ui::StashDialog),
    History(super::git_history_ui::HistoryDialog),
    Push(super::git_push_ui::PushDialog),
}

impl Workspace {
    fn git_dialog(&mut self) -> Option<&mut GitDialog> {
        self.git.as_mut()?.dialog.as_mut()
    }

    pub(super) fn git_dialog_open(&self) -> bool {
        self.git.as_ref().is_some_and(|g| g.dialog.is_some())
    }

    /// The Conflicts dialog with the repository's conflicted files (after merge, pull, rebase).
    /// The list is read off the UI thread.
    pub(super) fn open_conflicts(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.git.as_ref().map(|g| g.repo.clone()) else { return };
        let task = cx.background_executor().spawn(async move { repo.conflicts() });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                let files = match res {
                    Ok(files) => files,
                    Err(e) => return this.show_toast(format!("Cannot list the conflicts: {e}"), true, cx),
                };
                let Some(g) = this.git.as_mut() else { return };
                // Another dialog opened meanwhile: it stays.
                if g.dialog.as_ref().is_some_and(|d| !matches!(d, GitDialog::Conflicts(_))) {
                    return;
                }
                if files.is_empty() {
                    g.dialog = None;
                    this.show_toast("No conflicts to resolve".into(), false, cx);
                    return;
                }
                g.dialog = Some(GitDialog::Conflicts(ConflictsDialog { files, selected: 0 }));
                cx.notify();
            });
        })
        .detach();
    }

    fn selected_conflicts(&self) -> Vec<String> {
        match self.git.as_ref().and_then(|g| g.dialog.as_ref()) {
            Some(GitDialog::Conflicts(d)) => d.files.get(d.selected).cloned().into_iter().collect(),
            _ => Vec::new(),
        }
    }

    /// Accept Yours / Accept Theirs for the selected file.
    pub(super) fn conflicts_accept(&mut self, ours: bool, cx: &mut Context<Self>) {
        let files = self.selected_conflicts();
        if files.is_empty() {
            return;
        }
        let Some(task) = self.resolve_in_background("Resolve", cx, move |r| {
            r.accept(&files, ours).map(|_| format!("Resolved {} using {}", files.join(", "), if ours { "yours" } else { "theirs" }))
        }) else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let (res, left) = task.await;
            let _ = this.update(cx, |this, cx| this.finish_resolve(res, left, "Cannot resolve", cx));
        })
        .detach();
    }

    /// A resolve step off the UI thread (under the git busy guard), with the conflicts left
    /// after it. None when another git operation is running.
    fn resolve_in_background(
        &mut self,
        title: &'static str,
        cx: &mut Context<Self>,
        op: impl FnOnce(&brisk_git::Repo) -> brisk_git::Result<String> + Send + 'static,
    ) -> Option<gpui::Task<(brisk_git::Result<String>, Vec<String>)>> {
        let g = self.git.as_mut()?;
        if let Some(b) = g.busy {
            self.show_toast(format!("Wait for “{b}” to finish"), true, cx);
            return None;
        }
        g.busy = Some(title);
        let repo = g.repo.clone();
        Some(cx.background_executor().spawn(async move {
            let res = op(&repo);
            (res, repo.conflicts().unwrap_or_default())
        }))
    }

    /// Clears the busy guard and reports the outcome; true when it succeeded. A failure
    /// leaves the dialog (or the merge window) as it is.
    fn finish_resolve(&mut self, res: brisk_git::Result<String>, left: Vec<String>, err: &str, cx: &mut Context<Self>) -> bool {
        if let Some(g) = self.git.as_mut() {
            g.busy = None;
        }
        match res {
            Ok(msg) => {
                self.status = msg;
                self.after_resolve(left, cx);
                true
            }
            Err(e) => {
                self.show_toast(format!("{err}: {e}"), true, cx);
                false
            }
        }
    }

    /// Merge…: the merge window for the selected file, in a tab.
    pub(super) fn conflicts_merge(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(rel) = self.selected_conflicts().pop() {
            self.merge_file(rel, window, cx);
        }
    }

    /// The merge window for a conflicted file (also Show Diff on it in the Commit window).
    pub(super) fn merge_file(&mut self, rel: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.git.as_ref().map(|g| g.repo.clone()) else { return };
        // Base / yours / theirs (three `git show`s) and the merged branch's name off the UI thread.
        let r = rel.clone();
        let task = cx.background_executor().spawn(async move { (repo.conflict_versions(&r), repo.merge_source(), repo.abs(&r)) });
        cx.spawn_in(window, async move |this, cx| {
            let (res, source, abs) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                match res {
                    Ok(v) => {
                        let label = source.map_or_else(|| "Their version".to_string(), |b| format!("Changes from {b}"));
                        let view = cx.new(|cx| MergeView::new(rel, abs, label, v, cx));
                        this.open_merge(view, window, cx);
                    }
                    Err(e) => this.show_toast(format!("Cannot read the versions of {rel}: {e}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The merge window's tab (one per file); its Apply / Cancel come back here.
    pub(super) fn open_merge(&mut self, view: Entity<MergeView>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            g.dialog = None;
        }
        cx.subscribe_in(&view, window, |this, view, ev: &MergeEvent, window, cx| {
            let rel = view.read(cx).rel.clone();
            match ev {
                MergeEvent::Apply(text) => {
                    let text = text.clone();
                    let Some(task) = this.resolve_in_background("Merge", cx, move |r| r.resolve_with(&rel, &text).map(|_| format!("Merged {rel}"))) else {
                        return;
                    };
                    let view = view.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        let (res, left) = task.await;
                        let _ = this.update_in(cx, |this, window, cx| {
                            // A failed save keeps the merge window open with the user's work.
                            if this.finish_resolve(res, left, "Cannot save the merge", cx) {
                                if let Some(i) = this.tabs.iter().position(|t| matches!(t, Tab::Merge(m) if *m == view)) {
                                    this.close_tab(i, window, cx);
                                }
                            }
                        });
                    })
                    .detach();
                }
                MergeEvent::Cancel => {
                    if let Some(i) = this.tabs.iter().position(|t| matches!(t, Tab::Merge(m) if m == view)) {
                        this.close_tab(i, window, cx);
                    }
                    this.open_conflicts(cx);
                }
            }
        })
        .detach();
        let rel = view.read(cx).rel.clone();
        if let Some(i) = self.tabs.iter().position(|t| matches!(t, Tab::Merge(m) if m.read(cx).rel == rel)) {
            self.tabs[i] = Tab::Merge(view);
            self.active = Some(i);
        } else {
            self.add_tab(Tab::Merge(view));
        }
        self.focus_editor(window, cx);
        cx.notify();
    }

    /// Back to the Conflicts list of `files` (the conflicts left), or close it when none is.
    fn after_resolve(&mut self, files: Vec<String>, cx: &mut Context<Self>) {
        self.reload_editors(cx);
        self.refresh_git(cx);
        let Some(g) = self.git.as_mut() else { return };
        if files.is_empty() {
            g.dialog = None;
            self.show_toast("All conflicts are resolved. Commit to finish the merge.".into(), false, cx);
        } else {
            let selected = match &g.dialog {
                Some(GitDialog::Conflicts(d)) => d.selected.min(files.len() - 1),
                _ => 0,
            };
            g.dialog = Some(GitDialog::Conflicts(ConflictsDialog { files, selected }));
        }
        cx.notify();
    }

    fn close_git_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        g.dialog = None;
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn git_dialog_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let key = k.key.as_str();
        if key == "escape" {
            if self.stash_cancel_drop(cx) {
                return;
            }
            return self.close_git_dialog(window, cx);
        }
        match self.git_dialog() {
            Some(GitDialog::Conflicts(d)) => match key {
                "up" => d.selected = d.selected.saturating_sub(1),
                "down" => d.selected = (d.selected + 1).min(d.files.len().saturating_sub(1)),
                "enter" => self.conflicts_merge(window, cx),
                _ => {}
            },
            _ => self.stash_dialog_key(e, window, cx),
        }
        cx.notify();
    }

    pub(super) fn render_git_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let dialog = self.git.as_ref()?.dialog.as_ref()?;
        let body = match dialog {
            GitDialog::Conflicts(d) => self.render_conflicts(d, cx),
            _ => self.render_stash_dialog(cx)?,
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .flex()
                .justify_center()
                .items_start()
                .pt(px(50.))
                .occlude()
                .child(body)
                .into_any_element(),
        )
    }

    fn render_conflicts(&self, d: &ConflictsDialog, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = d.files.iter().enumerate().map(|(i, f)| {
            let on = i == d.selected;
            let (name, dir) = match f.rsplit_once('/') {
                Some((dir, name)) => (name.to_string(), dir.to_string()),
                None => (f.clone(), String::new()),
            };
            div()
                .id(("conflict-row", i))
                .w_full()
                .h(ui_px(26.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(on, |x| x.bg(Theme::selection_row()))
                .when(!on, |x| x.hover(|s| s.bg(Theme::hover_row())))
                .child(div().text_color(Theme::error()).child(name))
                .child(div().text_color(Theme::text_dim()).text_size(ui_px(12.)).child(dir))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, window, cx| {
                        if let Some(GitDialog::Conflicts(d)) = this.git_dialog() {
                            d.selected = i;
                        }
                        if e.click_count == 2 {
                            this.conflicts_merge(window, cx);
                        }
                        cx.notify();
                    }),
                )
        });
        let side = div()
            .w(px(150.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(button("cf-yours", "Accept Yours", false).on_click(cx.listener(|this, _, _, cx| this.conflicts_accept(true, cx))))
            .child(button("cf-theirs", "Accept Theirs", false).on_click(cx.listener(|this, _, _, cx| this.conflicts_accept(false, cx))))
            .child(button("cf-merge", "Merge…", true).on_click(cx.listener(|this, _, w, cx| this.conflicts_merge(w, cx))));
        dialog_frame("conflicts-dialog", 640., "Conflicts")
            .child(div().pb(px(8.)).text_color(Theme::text_muted()).child(format!("{} file(s) with merge conflicts", d.files.len())))
            .child(div().flex().gap(px(12.)).child(div().id("conflict-list").flex_1().max_h(ui_px(320.)).overflow_y_scroll().children(rows)).child(side))
            .child(
                div()
                    .pt(px(12.))
                    .flex()
                    .items_center()
                    .child(div().flex_1().text_size(ui_px(11.)).text_color(Theme::text_dim()).child("↑↓ select · Enter or double click: Merge… · Esc close"))
                    .child(button("cf-close", "Close", false).on_click(cx.listener(|this, _, w, cx| this.close_git_dialog(w, cx)))),
            )
            .into_any_element()
    }

}

/// A dialog button (primary: filled with the accent color).
pub(super) fn button(id: &'static str, label: &'static str, primary: bool) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .px(px(14.))
        .h(ui_px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .cursor_pointer()
        .when(primary, |b| b.bg(Theme::accent()).text_color(gpui::white()))
        .when(!primary, |b| b.border_1().border_color(Theme::divider()).hover(|s| s.bg(Theme::hover_row())))
        .active(|s| s.bg(Theme::press()))
        .child(label)
}

/// The panel of a modal dialog with its title.
pub(super) fn dialog_frame(id: &'static str, w: f32, title: &str) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w(px(w))
        .max_w(gpui::relative(0.95))
        .flex()
        .flex_col()
        .p(px(16.))
        .bg(Theme::panel())
        .border_1()
        .border_color(Theme::divider())
        .rounded(px(10.))
        .map(crate::ui_kit::floating)
        .text_size(ui_px(13.))
        .text_color(Theme::text())
        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).pb(px(10.)).child(title.to_string()))
}
