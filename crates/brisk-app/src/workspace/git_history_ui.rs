//! Git > Show History (a file) and Show History for Selection (lines of it): the commits
//! that touched it, newest first, with the selected commit's patch on the right. Enter or a
//! double click opens the whole file's diff for that commit.

use super::git_merge_ui::{button, dialog_frame, GitDialog};
use super::git_ui::{file_name, fmt_time};
use super::menu_ui::Entry;
use super::Workspace;
use crate::editor::Editor;
use crate::theme::ui_px;
use crate::theme::Theme;
use brisk_git::{Commit, FileChange, FileStatus};
use gpui::prelude::*;
use gpui::{div, px, Context, Entity, KeyDownEvent, Window};
use std::path::Path;
use std::rc::Rc;

const LIMIT: usize = 500;
/// Patch lines shown in the preview (the full diff is one Enter away).
const PATCH_LINES: usize = 3000;

pub struct HistoryDialog {
    pub rel: String,
    /// 1-based lines for Show History for Selection.
    pub lines: Option<(usize, usize)>,
    pub commits: Vec<Commit>,
    /// `git log -L` gives the patches with the commits.
    line_patches: Vec<String>,
    pub selected: usize,
    pub patch: String,
    pub loading: bool,
}

impl Workspace {
    fn history(&mut self) -> Option<&mut HistoryDialog> {
        match self.git.as_mut()?.dialog.as_mut()? {
            GitDialog::History(d) => Some(d),
            _ => None,
        }
    }

    /// The editor's selected lines (1-based), when the selection spans text.
    fn selected_lines(ed: &Entity<Editor>, cx: &Context<Self>) -> Option<(usize, usize)> {
        let b = &ed.read(cx).buffer;
        let r = b.primary().range();
        if r.is_empty() {
            return None;
        }
        let (l0, _) = b.pos_to_line_col(r.start);
        let (l1, c1) = b.pos_to_line_col(r.end);
        // A selection that ends at the start of a line does not include that line.
        let l1 = if c1 == 0 && l1 > l0 { l1 - 1 } else { l1 };
        Some((l0 + 1, l1 + 1))
    }

    /// The Git entries of the tree and editor context menus.
    pub(super) fn git_menu_entries(&self, path: &Path, editor: Option<&Entity<Editor>>, cx: &Context<Self>) -> Vec<Entry> {
        let Some(g) = self.git.as_ref() else { return Vec::new() };
        if g.repo.rel(path).is_none() || path.is_dir() {
            return Vec::new();
        }
        let mut v = vec![Entry::Separator];
        let p = path.to_path_buf();
        v.push(Entry::Item {
            label: "Git: Show History".into(),
            shortcut: String::new(),
            enabled: true,
            run: Rc::new(move |this, _, cx| this.show_history(&p, None, cx)),
        });
        let p = path.to_path_buf();
        v.push(Entry::Item {
            label: "Git: Compare with Branch…".into(),
            shortcut: String::new(),
            enabled: g.branches.iter().any(|b| !b.current),
            run: Rc::new(move |this, window, cx| {
                let pos = window.mouse_position();
                this.compare_with_branch_menu(&p, pos, cx)
            }),
        });
        if let Some(lines) = editor.and_then(|ed| Self::selected_lines(ed, cx)) {
            let p = path.to_path_buf();
            v.push(Entry::Item {
                label: format!("Git: Show History for Selection (lines {}–{})", lines.0, lines.1),
                shortcut: String::new(),
                enabled: true,
                run: Rc::new(move |this, _, cx| this.show_history(&p, Some(lines), cx)),
            });
        }
        if editor.is_some() {
            v.push(self.action_entry("Git: Annotate with Git Blame", brisk_keymap::Action::Annotate, true, true));
        }
        v
    }

    /// Git > Show History: the active editor's file (its selection, if any).
    pub(super) fn show_file_history_action(&mut self, cx: &mut Context<Self>) {
        let Some(ed) = self.active_editor().cloned() else {
            return self.show_toast("Open a file to see its history".into(), false, cx);
        };
        let Some(path) = ed.read(cx).path.clone() else { return };
        let lines = Self::selected_lines(&ed, cx);
        self.show_history(&path, lines, cx);
    }

    pub(super) fn show_history(&mut self, path: &Path, lines: Option<(usize, usize)>, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        let Some(rel) = g.repo.rel(path) else {
            return self.show_toast(format!("{} is not in the repository", path.display()), true, cx);
        };
        g.dialog = Some(GitDialog::History(HistoryDialog {
            rel: rel.clone(),
            lines,
            commits: Vec::new(),
            line_patches: Vec::new(),
            selected: 0,
            patch: String::new(),
            loading: true,
        }));
        let repo = g.repo.clone();
        let task = cx.background_executor().spawn(async move {
            match lines {
                Some((a, b)) => repo.line_log(&rel, a, b, LIMIT).map(|v| v.into_iter().unzip()),
                None => repo.file_log(&rel, LIMIT).map(|c| (c, Vec::new())),
            }
        });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                let Some(d) = this.history() else { return };
                d.loading = false;
                match res {
                    Ok((commits, patches)) => {
                        d.commits = commits;
                        d.line_patches = patches;
                        this.history_select(0, cx);
                    }
                    Err(e) => this.show_toast(format!("Cannot read the history: {e}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn history_select(&mut self, i: usize, cx: &mut Context<Self>) {
        let repo = self.git.as_ref().map(|g| g.repo.clone());
        let Some(d) = self.history() else { return };
        let Some(c) = d.commits.get(i.min(d.commits.len().saturating_sub(1))) else { return };
        d.selected = i.min(d.commits.len() - 1);
        match (d.line_patches.get(d.selected), repo) {
            (Some(p), _) => d.patch = p.clone(),
            (None, Some(repo)) => {
                // `git log --follow` + `git show` per row: off the UI thread, so holding ↓ in a
                // long history scrolls instead of freezing. A stale answer (the selection moved
                // on meanwhile) is dropped.
                let (hash, rel, sel) = (c.hash.clone(), d.rel.clone(), d.selected);
                d.patch = "Loading…".into();
                let task = cx.background_executor().spawn(async move {
                    let path = repo.path_in_commit(&hash, &rel);
                    repo.file_patch(&hash, &path).unwrap_or_else(|e| e.to_string())
                });
                cx.spawn(async move |this, cx| {
                    let patch = task.await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(d) = this.history() {
                            if d.selected == sel {
                                d.patch = patch;
                                cx.notify();
                            }
                        }
                    });
                })
                .detach();
            }
            _ => d.patch = String::new(),
        }
        cx.notify();
    }

    /// The whole file's diff for the selected commit (the dialog closes so the tab shows).
    pub(super) fn history_open_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.history() else { return };
        let Some(c) = d.commits.get(d.selected) else { return };
        let (hash, rel) = (c.hash.clone(), d.rel.clone());
        let first = c.parents.is_empty();
        let Some(g) = self.git.as_mut() else { return };
        let repo = g.repo.clone();
        g.dialog = None;
        let status = if first { FileStatus::Added } else { FileStatus::Modified };
        // The file's name in that commit (`git log --follow`) off the UI thread.
        let h = hash.clone();
        let task = cx.background_executor().spawn(async move { repo.path_in_commit(&h, &rel) });
        cx.spawn_in(window, async move |this, cx| {
            let path = task.await;
            let _ = this.update_in(cx, |this, window, cx| this.show_commit_diff(&hash, &FileChange { path, old_path: None, status }, window, cx));
        })
        .detach();
        cx.notify();
    }

    /// The selected commit's version against the local file (Show Diff with Working Tree).
    pub(super) fn history_compare_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.history() else { return };
        let Some(c) = d.commits.get(d.selected) else { return };
        let (hash, rel) = (c.hash.clone(), d.rel.clone());
        let Some(g) = self.git.as_mut() else { return };
        let repo = g.repo.clone();
        g.dialog = None;
        let short = hash[..hash.len().min(8)].to_string();
        let (h, r) = (hash.clone(), rel.clone());
        let task = cx.background_executor().spawn(async move { repo.path_in_commit(&h, &r) });
        cx.spawn_in(window, async move |this, cx| {
            let path = task.await;
            let _ = this.update_in(cx, |this, window, cx| this.compare_with_rev(&rel, &hash, &path, &short, window, cx));
        })
        .detach();
        cx.notify();
    }

    pub(super) fn history_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.history() else { return self.push_key(e, window, cx) };
        let sel = d.selected;
        match e.keystroke.key.as_str() {
            "up" => self.history_select(sel.saturating_sub(1), cx),
            "down" => self.history_select(sel + 1, cx),
            "pageup" => self.history_select(sel.saturating_sub(15), cx),
            "pagedown" => self.history_select(sel + 15, cx),
            "enter" => self.history_open_diff(window, cx),
            "d" if e.keystroke.modifiers.control => self.history_open_diff(window, cx),
            _ => {}
        }
    }

    pub(super) fn render_history_dialog(&mut self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let d = match self.git.as_ref()?.dialog.as_ref()? {
            GitDialog::History(d) => d,
            _ => return self.render_push_dialog(cx),
        };
        let rows = d.commits.iter().enumerate().map(|(i, c)| {
            let on = i == d.selected;
            div()
                .id(("hist-row", i))
                .w_full()
                .h(ui_px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(8.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(on, |x| x.bg(Theme::selection_row()))
                .when(!on, |x| x.hover(|s| s.bg(Theme::hover_row())))
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(c.subject.clone()))
                .child(div().text_color(Theme::text_muted()).text_size(ui_px(12.)).whitespace_nowrap().child(c.author.clone()))
                .child(div().w(px(90.)).text_color(Theme::text_dim()).text_size(ui_px(12.)).whitespace_nowrap().child(fmt_time(c.time)))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, e: &gpui::MouseDownEvent, w, cx| {
                        this.history_select(i, cx);
                        if e.click_count == 2 {
                            this.history_open_diff(w, cx);
                        }
                    }),
                )
        });
        let patch_lines = d
            .patch
            .lines()
            .filter(|l| !(l.starts_with("diff --git") || l.starts_with("index ") || l.starts_with("--- ") || l.starts_with("+++ ")))
            .take(PATCH_LINES)
            .enumerate()
            .map(|(i, l)| {
                let (bg, fg) = if l.starts_with('+') {
                    (Some(Theme::diff_added_bg()), Theme::text())
                } else if l.starts_with('-') {
                    (Some(Theme::diff_deleted_bg()), Theme::text())
                } else if l.starts_with("@@") {
                    (None, Theme::accent())
                } else {
                    (None, Theme::text_muted())
                };
                div().id(("hist-patch", i)).px(px(6.)).h(ui_px(19.)).whitespace_nowrap().when_some(bg, |x, c| x.bg(c)).text_color(fg).child(if l.is_empty() {
                    " ".to_string()
                } else {
                    l.to_string()
                })
            });
        let selected = d.commits.get(d.selected);
        let what = match d.lines {
            Some((a, b)) => format!("History for {} lines {a}–{b}", file_name(&d.rel)),
            None => format!("History for {}", file_name(&d.rel)),
        };
        let info = if d.loading {
            "Loading…".to_string()
        } else if d.commits.is_empty() {
            "No commits: the file is not committed yet".to_string()
        } else {
            format!("{} commit(s) · {}", d.commits.len(), d.rel)
        };
        Some(
            dialog_frame("history-dialog", 1100., &what)
                .h(ui_px(620.))
                .child(div().pb(px(8.)).text_color(Theme::text_muted()).child(info))
                .child(
                    div().flex_1().min_h_0().flex().gap(px(10.)).child(div().id("hist-list").w(px(430.)).overflow_y_scroll().children(rows)).child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .children(selected.map(|c| {
                                div().pb(px(6.)).text_size(ui_px(12.)).text_color(Theme::text_muted()).child(format!(
                                    "{} · {} <{}> · {}",
                                    c.short(),
                                    c.author,
                                    c.email,
                                    fmt_time(c.time)
                                ))
                            }))
                            .child(
                                div()
                                    .id("hist-patch")
                                    .flex_1()
                                    .min_h_0()
                                    .overflow_scroll()
                                    .border_1()
                                    .border_color(Theme::divider())
                                    .bg(Theme::editor_bg())
                                    .font_family(brisk_settings::editor_font())
                                    .text_size(ui_px(12.5))
                                    .children(patch_lines),
                            ),
                    ),
                )
                .child(
                    div()
                        .pt(px(10.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .flex_1()
                                .text_size(ui_px(11.))
                                .text_color(Theme::text_dim())
                                .child("↑↓ select · Enter, Ctrl+D or double click: full diff · Esc close"),
                        )
                        .child(button("hist-local", "Compare with Local", false).on_click(cx.listener(|this, _, w, cx| this.history_compare_local(w, cx))))
                        .child(button("hist-diff", "Show Diff", false).on_click(cx.listener(|this, _, w, cx| this.history_open_diff(w, cx))))
                        .child(button("hist-close", "Close", true).on_click(cx.listener(|this, _, w, cx| {
                            if let Some(g) = this.git.as_mut() {
                                g.dialog = None;
                            }
                            this.focus_editor(w, cx);
                            cx.notify();
                        }))),
                )
                .into_any_element(),
        )
    }
}
