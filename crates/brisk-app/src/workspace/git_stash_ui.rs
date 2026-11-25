//! Git > Stash Changes… (a message, keep untracked files) and Unstash Changes… (the stash
//! list with Apply / Pop / Drop and the files of the selected stash; a file opens its diff).

use super::git_merge_ui::{button, dialog_frame, GitDialog};
use super::git_ui::{dir_name, file_name, fmt_time, status_color};
use super::Workspace;
use crate::diff_view::DiffView;
use crate::theme::ui_px;
use crate::theme::Theme;
use brisk_git::{FileChange, FileStatus, StashEntry};
use gpui::prelude::*;
use gpui::{div, px, Context, KeyDownEvent, Window};

pub enum StashDialog {
    Push {
        message: String,
        untracked: bool,
    },
    /// `confirm_drop`: Drop was asked for; the dialog shows its confirmation (Enter / Esc).
    List {
        entries: Vec<StashEntry>,
        selected: usize,
        files: Vec<FileChange>,
        confirm_drop: bool,
    },
}

impl Workspace {
    fn stash_dialog(&mut self) -> Option<&mut StashDialog> {
        match self.git.as_mut()?.dialog.as_mut()? {
            GitDialog::Stash(d) => Some(d),
            _ => None,
        }
    }

    fn set_stash_dialog(&mut self, d: Option<StashDialog>, cx: &mut Context<Self>) {
        if let Some(g) = self.git.as_mut() {
            g.dialog = d.map(GitDialog::Stash);
        }
        cx.notify();
    }

    pub(super) fn git_more_action(&mut self, action: brisk_keymap::Action, window: &mut Window, cx: &mut Context<Self>) {
        use brisk_keymap::Action as A;
        if self.git.is_none() {
            return self.show_toast("Version control is not enabled for this project".into(), true, cx);
        }
        match action {
            A::StashChanges => self.open_stash_push(cx),
            A::UnstashChanges => self.open_unstash(cx),
            A::ShowFileHistory => self.show_file_history_action(cx),
            A::CompareWithBranch => self.compare_with_branch_action(window, cx),
            _ => {}
        }
    }

    /// Stash Changes…: nothing to stash → a toast, like PyCharm.
    pub(super) fn open_stash_push(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_ref() else { return };
        if g.status.changes.is_empty() {
            return self.show_toast("No local changes to stash".into(), false, cx);
        }
        // Only unversioned files changed: without them the stash would be empty.
        let untracked = g.status.changes.iter().all(|c| c.status == FileStatus::Untracked);
        self.set_stash_dialog(Some(StashDialog::Push { message: String::new(), untracked }), cx);
    }

    /// Unstash Changes…: the stash list, the newest selected.
    pub(super) fn open_unstash(&mut self, cx: &mut Context<Self>) {
        self.open_unstash_at(0, cx);
    }

    /// The stash list with row `keep` selected. The git calls run off the UI thread.
    fn open_unstash_at(&mut self, keep: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.git.as_ref().map(|g| g.repo.clone()) else { return };
        let task = cx.background_executor().spawn(async move {
            let entries = repo.stash_list().unwrap_or_default();
            let keep = keep.min(entries.len().saturating_sub(1));
            let files = entries.get(keep).map(|e| repo.stash_files(&e.name).unwrap_or_default()).unwrap_or_default();
            (entries, keep, files)
        });
        cx.spawn(async move |this, cx| {
            let (entries, selected, files) = task.await;
            let _ = this.update(cx, |this, cx| {
                if entries.is_empty() {
                    this.set_stash_dialog(None, cx);
                    return this.show_toast("There are no stashes".into(), false, cx);
                }
                this.set_stash_dialog(Some(StashDialog::List { entries, selected, files, confirm_drop: false }), cx);
            });
        })
        .detach();
    }

    fn stash_select(&mut self, i: usize, cx: &mut Context<Self>) {
        let repo = self.git.as_ref().map(|g| g.repo.clone());
        if let (Some(repo), Some(StashDialog::List { entries, selected, files, confirm_drop })) = (repo, self.stash_dialog()) {
            *confirm_drop = false;
            let i = i.min(entries.len().saturating_sub(1));
            if *selected != i || files.is_empty() {
                *selected = i;
                files.clear();
                let name = entries[i].name.clone();
                let task = cx.background_executor().spawn(async move { repo.stash_files(&name).unwrap_or_default() });
                cx.spawn(async move |this, cx| {
                    let got = task.await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(StashDialog::List { selected, files, .. }) = this.stash_dialog() {
                            if *selected == i {
                                *files = got;
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    /// Create the stash: open files are saved first so their edits go into it.
    pub(super) fn stash_push(&mut self, cx: &mut Context<Self>) {
        let Some(StashDialog::Push { message, untracked }) = self.stash_dialog() else { return };
        let (message, untracked) = (message.trim().to_string(), *untracked);
        self.save_all(cx);
        self.set_stash_dialog(None, cx);
        self.git_op("Stash", move |r| r.stash_push(&message, untracked).map(|_| "Changes stashed".to_string()), |_, _| {}, cx);
    }

    fn selected_stash(&mut self) -> Option<String> {
        match self.stash_dialog()? {
            StashDialog::List { entries, selected, .. } => entries.get(*selected).map(|e| e.name.clone()),
            _ => None,
        }
    }

    /// Apply / Pop: the dialog closes; a conflict opens the Conflicts dialog through refresh_git.
    pub(super) fn stash_apply(&mut self, pop: bool, cx: &mut Context<Self>) {
        let Some(name) = self.selected_stash() else { return };
        self.save_all(cx);
        self.set_stash_dialog(None, cx);
        let title = if pop { "Pop Stash" } else { "Apply Stash" };
        self.git_op(title, move |r| r.stash_apply(&name, pop).map(|_| format!("{name} {}", if pop { "popped" } else { "applied" })), |_, _| {}, cx);
    }

    /// Drop asks first (it cannot be undone), like Rollback.
    pub(super) fn stash_drop(&mut self, cx: &mut Context<Self>) {
        if let Some(StashDialog::List { entries, confirm_drop, .. }) = self.stash_dialog() {
            *confirm_drop = !entries.is_empty();
        }
        cx.notify();
    }

    /// Esc on the Drop confirmation cancels just that; true when there was one.
    pub(super) fn stash_cancel_drop(&mut self, cx: &mut Context<Self>) -> bool {
        match self.stash_dialog() {
            Some(StashDialog::List { confirm_drop, .. }) if *confirm_drop => {
                *confirm_drop = false;
                cx.notify();
                true
            }
            _ => false,
        }
    }

    /// The confirmed Drop, through `git_op` (one git operation at a time). The stash is
    /// addressed by its commit: `stash@{N}` may have shifted since the list was read.
    fn stash_drop_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(StashDialog::List { entries, selected, confirm_drop, .. }) = self.stash_dialog() else { return };
        *confirm_drop = false;
        let Some(e) = entries.get(*selected).cloned() else { return };
        // The list stays open on the next stash (or closes when none is left).
        let keep = *selected;
        let name = e.name.clone();
        self.git_op("Drop Stash", move |r| r.stash_drop(&e.hash).map(|_| format!("Dropped {name}")), move |this, cx| this.open_unstash_at(keep, cx), cx);
    }

    /// The diff of one file of the selected stash: the commit it was made on vs the stash.
    pub(super) fn stash_file_diff(&mut self, fi: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.selected_stash() else { return };
        let Some(StashDialog::List { files, .. }) = self.stash_dialog() else { return };
        let Some(c) = files.get(fi).cloned() else { return };
        let Some(repo) = self.git.as_ref().map(|g| g.repo.clone()) else { return };
        let abs = repo.abs(&c.path);
        let title = format!("{} ({name})", file_name(&c.path));
        // The tab shows behind the modal otherwise.
        self.set_stash_dialog(None, cx);
        let n = name.clone();
        let task = cx.background_executor().spawn(async move { repo.stash_file_versions(&n, &c) });
        cx.spawn_in(window, async move |this, cx| {
            let (left, right) = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let view = cx.new(|cx| DiffView::new(title, Some(abs), format!("{name}^"), &left, name.clone(), &right, cx));
                this.open_diff(view, window, cx);
            });
        })
        .detach();
    }

    pub(super) fn stash_dialog_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let m = &k.modifiers;
        let key = k.key.as_str();
        match self.stash_dialog() {
            Some(StashDialog::Push { message, untracked }) => match key {
                "enter" => self.stash_push(cx),
                "backspace" => {
                    if m.control {
                        let t = message.trim_end().len();
                        let cut = message[..t].rfind(' ').map(|i| i + 1).unwrap_or(0);
                        message.truncate(cut);
                    } else {
                        message.pop();
                    }
                }
                "u" if m.alt => *untracked = !*untracked,
                "v" if m.control => {
                    if let Some(t) = cx.read_from_clipboard().and_then(|c| c.text()) {
                        message.push_str(t.lines().next().unwrap_or(""));
                    }
                }
                _ => {
                    if !(m.control || m.alt || m.platform) {
                        if let Some(ch) = &k.key_char {
                            if !ch.chars().any(|c| c.is_control()) {
                                message.push_str(ch);
                            }
                        }
                    }
                }
            },
            Some(StashDialog::List { confirm_drop: true, .. }) => {
                if key == "enter" {
                    self.stash_drop_confirmed(cx);
                }
            }
            Some(StashDialog::List { selected, .. }) => {
                let sel = *selected;
                match key {
                    "up" => self.stash_select(sel.saturating_sub(1), cx),
                    "down" => self.stash_select(sel + 1, cx),
                    "enter" => self.stash_apply(m.shift, cx),
                    "delete" => self.stash_drop(cx),
                    "d" if m.control => self.stash_file_diff(0, window, cx),
                    _ => {}
                }
            }
            None => self.history_key(e, window, cx),
        }
        cx.notify();
    }

    pub(super) fn render_stash_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let d = match self.git.as_ref()?.dialog.as_ref()? {
            GitDialog::Stash(d) => d,
            _ => return self.render_history_dialog(cx),
        };
        Some(match d {
            StashDialog::Push { message, untracked } => self.render_stash_push(message, *untracked, cx),
            StashDialog::List { entries, selected, files, confirm_drop } => self.render_unstash(entries, *selected, files, *confirm_drop, cx),
        })
    }

    fn render_stash_push(&self, message: &str, untracked: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let branch = self.git.as_ref().and_then(|g| g.status.branch.clone()).unwrap_or_else(|| "HEAD".into());
        let n = self.git.as_ref().map(|g| g.status.changes.len()).unwrap_or(0);
        let field = div()
            .id("stash-message")
            .h(ui_px(30.))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(5.))
            .border_1()
            .border_color(Theme::accent())
            .bg(Theme::editor_bg())
            .when(message.is_empty(), |x| x.child(div().text_color(Theme::text_dim()).child(format!("WIP on {branch}"))))
            .when(!message.is_empty(), |x| x.child(message.to_string()))
            .child(div().w(px(1.5)).h(ui_px(16.)).bg(Theme::accent()));
        let check = div()
            .id("stash-untracked")
            .pt(px(10.))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .child(
                div()
                    .size(ui_px(14.))
                    .rounded(px(3.))
                    .border_1()
                    .border_color(if untracked { Theme::accent() } else { Theme::divider() })
                    .when(untracked, |x| x.bg(Theme::accent()))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(ui_px(10.))
                    .text_color(gpui::white())
                    .when(untracked, |x| x.child(crate::icons::ui_icon("check-bold", 10., gpui::white()))),
            )
            .child("Include untracked files")
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if let Some(StashDialog::Push { untracked, .. }) = this.stash_dialog() {
                        *untracked = !*untracked;
                    }
                    cx.notify();
                }),
            );
        dialog_frame("stash-dialog", 480., "Stash Changes")
            .child(div().pb(px(6.)).text_color(Theme::text_muted()).child(format!("{n} changed file(s) on {branch} go to a new stash. Message:")))
            .child(field)
            .child(check)
            .child(
                div()
                    .pt(px(14.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().text_size(ui_px(11.)).text_color(Theme::text_dim()).child("Enter create · Alt+U untracked · Esc cancel"))
                    .child(button("stash-cancel", "Cancel", false).on_click(cx.listener(|this, _, w, cx| {
                        this.set_stash_dialog(None, cx);
                        this.focus_editor(w, cx);
                    })))
                    .child(button("stash-create", "Create Stash", true).on_click(cx.listener(|this, _, _, cx| this.stash_push(cx)))),
            )
            .into_any_element()
    }

    fn render_unstash(&self, entries: &[StashEntry], selected: usize, files: &[FileChange], confirm_drop: bool, cx: &mut Context<Self>) -> gpui::AnyElement {
        let rows = entries.iter().enumerate().map(|(i, e)| {
            let on = i == selected;
            div()
                .id(("stash-row", i))
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
                .child(div().text_color(Theme::text_dim()).text_size(ui_px(12.)).child(e.name.clone()))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(e.message.clone()))
                .child(div().text_color(Theme::text_dim()).text_size(ui_px(12.)).child(fmt_time(e.time)))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        this.stash_select(i, cx);
                        if e.click_count == 2 {
                            this.stash_apply(false, cx);
                        }
                    }),
                )
        });
        let file_rows = files.iter().enumerate().map(|(i, c)| {
            div()
                .id(("stash-file", i))
                .w_full()
                .h(ui_px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(4.))
                .cursor_pointer()
                .hover(|s| s.bg(Theme::hover_row()))
                .child(div().text_color(status_color(c.status)).child(file_name(&c.path).to_string()))
                .child(div().text_color(Theme::text_dim()).text_size(ui_px(12.)).child(dir_name(&c.path).to_string()))
                .when(c.status == FileStatus::Added, |x| x.child(div().text_color(Theme::text_dim()).text_size(ui_px(11.)).child("new")))
                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, w, cx| this.stash_file_diff(i, w, cx)))
        });
        let side = div()
            .w(px(120.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(button("st-apply", "Apply", true).on_click(cx.listener(|this, _, _, cx| this.stash_apply(false, cx))))
            .child(button("st-pop", "Pop", false).on_click(cx.listener(|this, _, _, cx| this.stash_apply(true, cx))))
            .child(button("st-drop", "Drop", false).on_click(cx.listener(|this, _, _, cx| this.stash_drop(cx))));
        let branch = self.git.as_ref().and_then(|g| g.status.branch.clone()).unwrap_or_else(|| "HEAD".into());
        dialog_frame("unstash-dialog", 720., "Unstash Changes")
            .child(div().pb(px(8.)).text_color(Theme::text_muted()).child(format!("{} stash(es) · changes go to {branch}", entries.len())))
            .child(div().flex().gap(px(12.)).child(div().id("stash-list").flex_1().max_h(ui_px(220.)).overflow_y_scroll().children(rows)).child(side))
            .child(
                div()
                    .pt(px(12.))
                    .pb(px(4.))
                    .text_color(Theme::text_muted())
                    .child(format!("Files in {} (click: diff)", entries.get(selected).map(|e| e.name.as_str()).unwrap_or(""))),
            )
            .child(
                div()
                    .id("stash-files")
                    .max_h(ui_px(200.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(Theme::divider())
                    .rounded(px(5.))
                    .p(px(4.))
                    .children(file_rows),
            )
            .when(confirm_drop, |x| {
                let what = entries.get(selected).map(|e| format!("{} “{}”", e.name, e.message)).unwrap_or_default();
                x.child(
                    div()
                        .mt(px(12.))
                        .p(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(5.))
                        .border_1()
                        .border_color(Theme::divider())
                        .child(div().flex_1().min_w_0().overflow_hidden().child(format!("Drop {what}? This cannot be undone.")))
                        .child(button("st-drop-no", "Cancel", false).on_click(cx.listener(|this, _, _, cx| {
                            this.stash_cancel_drop(cx);
                        })))
                        .child(button("st-drop-yes", "Drop", true).on_click(cx.listener(|this, _, _, cx| this.stash_drop_confirmed(cx)))),
                )
            })
            .child(
                div()
                    .pt(px(12.))
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(ui_px(11.))
                            .text_color(Theme::text_dim())
                            .child("↑↓ select · Enter Apply · Shift+Enter Pop · Delete Drop · Ctrl+D diff · Esc close"),
                    )
                    .child(button("st-close", "Close", false).on_click(cx.listener(|this, _, w, cx| {
                        this.set_stash_dialog(None, cx);
                        this.focus_editor(w, cx);
                    }))),
            )
            .into_any_element()
    }
}
