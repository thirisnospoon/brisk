//! Push Commits (Ctrl+Shift+K): the outgoing commits of the current branch with the files of
//! the selected one (double click: its diff), then Push. A branch without an upstream is
//! pushed as a new remote branch and starts tracking it.

use super::git_merge_ui::{button, dialog_frame, GitDialog};
use super::git_ui::{dir_name, file_name, fmt_time, status_color};
use super::Workspace;
use crate::theme::Theme;
use brisk_git::{FileChange, Outgoing};
use gpui::prelude::*;
use gpui::{div, px, Context, KeyDownEvent, Window};

pub struct PushDialog {
    pub outgoing: Option<Outgoing>,
    pub error: Option<String>,
    pub selected: usize,
    pub files: Vec<FileChange>,
}

impl Workspace {
    fn push_dialog(&mut self) -> Option<&mut PushDialog> {
        match self.git.as_mut()?.dialog.as_mut()? {
            GitDialog::Push(d) => Some(d),
            _ => None,
        }
    }

    pub(super) fn open_push_dialog(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        g.dialog = Some(GitDialog::Push(PushDialog { outgoing: None, error: None, selected: 0, files: Vec::new() }));
        let repo = g.repo.clone();
        let task = cx.background_executor().spawn(async move { repo.outgoing() });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(d) = this.push_dialog() else { return };
                match res {
                    Ok(o) => d.outgoing = Some(o),
                    Err(e) => d.error = Some(e.to_string()),
                }
                this.push_select(0, cx);
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn push_select(&mut self, i: usize, cx: &mut Context<Self>) {
        let repo = self.git.as_ref().map(|g| g.repo.clone());
        let Some(d) = self.push_dialog() else { return };
        let Some(o) = &d.outgoing else { return cx.notify() };
        if let Some(c) = o.commits.get(i.min(o.commits.len().saturating_sub(1))) {
            let hash = c.hash.clone();
            let sel = i.min(o.commits.len() - 1);
            d.selected = sel;
            // `git show --name-status` per row: off the UI thread; a stale answer is dropped.
            if let Some(repo) = repo {
                let task = cx.background_executor().spawn(async move { repo.commit_files(&hash).unwrap_or_default() });
                cx.spawn(async move |this, cx| {
                    let files = task.await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(d) = this.push_dialog().filter(|d| d.selected == sel) {
                            d.files = files;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        cx.notify();
    }

    pub(super) fn push_confirm(&mut self, cx: &mut Context<Self>) {
        let Some(o) = self.push_dialog().and_then(|d| d.outgoing.clone()) else { return };
        if let Some(g) = self.git.as_mut() {
            g.dialog = None;
        }
        self.git_op("Push", move |r| r.push_outgoing(&o), |_, _| {}, cx);
    }

    fn push_file_diff(&mut self, fi: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.push_dialog() else { return };
        let (Some(o), Some(f)) = (&d.outgoing, d.files.get(fi).cloned()) else { return };
        let Some(hash) = o.commits.get(d.selected).map(|c| c.hash.clone()) else { return };
        if let Some(g) = self.git.as_mut() {
            g.dialog = None;
        }
        self.show_commit_diff(&hash, &f, window, cx);
    }

    pub(super) fn push_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.push_dialog() else { return };
        let sel = d.selected;
        let k = &e.keystroke;
        match k.key.as_str() {
            "up" => self.push_select(sel.saturating_sub(1), cx),
            "down" => self.push_select(sel + 1, cx),
            "enter" => self.push_confirm(cx),
            "d" if k.modifiers.control => self.push_file_diff(0, window, cx),
            _ => {}
        }
    }

    pub(super) fn render_push_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let d = match self.git.as_ref()?.dialog.as_ref()? {
            GitDialog::Push(d) => d,
            _ => return None,
        };
        let title = match &d.outgoing {
            Some(o) => format!("Push Commits to {}", o.target),
            None => "Push Commits".to_string(),
        };
        let mut body = div().flex_1().min_h_0().flex().gap(px(10.));
        let info = match (&d.outgoing, &d.error) {
            (_, Some(e)) => div().text_color(Theme::error()).child(e.clone()),
            (None, None) => div().text_color(Theme::text_muted()).child("Loading…"),
            (Some(o), None) => {
                let rows = o.commits.iter().enumerate().map(|(i, c)| {
                    let on = i == d.selected;
                    div()
                        .id(("push-row", i))
                        .w_full()
                        .h(px(24.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .when(on, |x| x.bg(Theme::selection_row()))
                        .when(!on, |x| x.hover(|s| s.bg(Theme::hover_row())))
                        .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(c.subject.clone()))
                        .child(div().text_color(Theme::text_muted()).text_size(px(12.)).whitespace_nowrap().child(c.author.clone()))
                        .child(div().w(px(110.)).text_color(Theme::text_dim()).text_size(px(12.)).whitespace_nowrap().child(fmt_time(c.time)))
                        .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| this.push_select(i, cx)))
                });
                let files = d.files.iter().enumerate().map(|(i, f)| {
                    div()
                        .id(("push-file", i))
                        .w_full()
                        .h(px(22.))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .rounded(px(4.))
                        .cursor_pointer()
                        .hover(|s| s.bg(Theme::hover_row()))
                        .child(div().text_color(status_color(f.status)).whitespace_nowrap().child(file_name(&f.path).to_string()))
                        .child(div().text_color(Theme::text_dim()).text_size(px(12.)).whitespace_nowrap().child(dir_name(&f.path).to_string()))
                        .on_click(cx.listener(move |this, e: &gpui::ClickEvent, w, cx| {
                            if e.click_count() >= 2 {
                                this.push_file_diff(i, w, cx);
                            }
                        }))
                });
                body = body
                    .child(div().id("push-commits").flex_1().min_w_0().overflow_y_scroll().border_1().border_color(Theme::divider()).rounded(px(5.)).p(px(4.)).children(rows))
                    .child(div().id("push-files").w(px(300.)).overflow_y_scroll().border_1().border_color(Theme::divider()).rounded(px(5.)).p(px(4.)).children(files));
                let what = if o.commits.is_empty() {
                    "Nothing to push: the remote branch has all commits".to_string()
                } else {
                    format!("{} commit(s) from {}", o.commits.len(), o.branch)
                };
                div()
                    .flex()
                    .gap(px(8.))
                    .text_color(Theme::text_muted())
                    .child(what)
                    .when(o.new_branch, |x| x.child(div().px(px(6.)).rounded(px(3.)).bg(Theme::diff_added_bg()).text_color(Theme::text()).text_size(px(12.)).child("New branch, upstream will be set")))
            }
        };
        Some(
            dialog_frame("push-dialog", 900., &title)
                .h(px(480.))
                .child(div().pb(px(8.)).child(info))
                .child(body)
                .child(
                    div()
                        .pt(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().text_size(px(11.)).text_color(Theme::text_dim()).child("↑↓ commits · double click a file: diff · Enter Push · Esc cancel"))
                        .child(button("push-cancel", "Cancel", false).on_click(cx.listener(|this, _, w, cx| {
                            if let Some(g) = this.git.as_mut() {
                                g.dialog = None;
                            }
                            this.focus_editor(w, cx);
                            cx.notify();
                        })))
                        .when(d.outgoing.is_some(), |x| x.child(button("push-go", "Push", true).on_click(cx.listener(|this, _, _, cx| this.push_confirm(cx))))),
                )
                .into_any_element(),
        )
    }
}
