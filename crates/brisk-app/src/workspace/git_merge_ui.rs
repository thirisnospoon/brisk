//! Merge conflicts: the Conflicts dialog (Accept Yours / Accept Theirs / Merge…) and the
//! three-pane Merge Revisions dialog (yours | result | theirs) with per-chunk >> and <<.

use crate::theme::ui_px;
use super::Workspace;
use crate::theme::Theme;
use brisk_git::Chunk;
use gpui::prelude::*;
use gpui::{div, px, Context, KeyDownEvent, Window};

/// What a changed chunk of the result holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    /// Not decided: the result shows the base lines.
    Unresolved,
    Base,
    Ours,
    Theirs,
    OursThenTheirs,
    TheirsThenOurs,
}

pub struct ConflictsDialog {
    pub files: Vec<String>,
    pub selected: usize,
}

pub struct MergeDialog {
    pub rel: String,
    pub chunks: Vec<Chunk>,
    pub picks: Vec<Pick>,
    /// The file ended with a newline on our side.
    final_newline: bool,
    /// Keyboard: the current changed chunk (index into `chunks`).
    pub current: Option<usize>,
}

/// The Git dialogs that sit over the window.
pub enum GitDialog {
    Conflicts(ConflictsDialog),
    Merge(MergeDialog),
    Stash(super::git_stash_ui::StashDialog),
    History(super::git_history_ui::HistoryDialog),
    Push(super::git_push_ui::PushDialog),
}

impl MergeDialog {
    pub fn new(rel: String, v: brisk_git::ConflictVersions) -> Self {
        let ours = v.ours.unwrap_or_default();
        let chunks = brisk_git::merge3(&v.base.unwrap_or_default(), &ours, &v.theirs.unwrap_or_default());
        // Non-conflicting changes are applied right away (PyCharm does the same).
        let picks = chunks
            .iter()
            .map(|c| match c {
                Chunk::Same(_) => Pick::Base,
                Chunk::Change { base, ours, theirs } if ours == theirs => Pick::Ours,
                Chunk::Change { base, ours, .. } if ours == base => Pick::Theirs,
                Chunk::Change { base, theirs, .. } if theirs == base => Pick::Ours,
                _ => Pick::Unresolved,
            })
            .collect();
        let mut d = MergeDialog { rel, chunks, picks, final_newline: ours.is_empty() || ours.ends_with('\n'), current: None };
        d.current = d.next_conflict(None, true);
        d
    }

    pub fn unresolved(&self) -> usize {
        self.picks.iter().filter(|p| **p == Pick::Unresolved).count()
    }

    fn next_conflict(&self, from: Option<usize>, forward: bool) -> Option<usize> {
        let changed = |i: &usize| matches!(self.chunks[*i], Chunk::Change { .. });
        let n = self.chunks.len();
        if forward {
            let start = from.map_or(0, |f| f + 1);
            (start..n).find(|i| changed(i) && self.picks[*i] == Pick::Unresolved).or_else(|| (start..n).find(changed))
        } else {
            let end = from.unwrap_or(n);
            (0..end).rev().find(changed)
        }
    }

    /// >> (`ours`) or << on a chunk: take that side; the other side's arrow then appends.
    pub fn apply(&mut self, i: usize, ours: bool) {
        let Some(p) = self.picks.get_mut(i) else { return };
        *p = match (*p, ours) {
            (Pick::Unresolved | Pick::Base, true) => Pick::Ours,
            (Pick::Unresolved | Pick::Base, false) => Pick::Theirs,
            (Pick::Theirs, true) => Pick::TheirsThenOurs,
            (Pick::Ours, false) => Pick::OursThenTheirs,
            (p, _) => p,
        };
    }

    /// × on a chunk: keep the base lines (ignore both changes).
    pub fn ignore(&mut self, i: usize) {
        if let Some(p) = self.picks.get_mut(i) {
            *p = Pick::Base;
        }
    }

    /// Accept Left / Accept Right for every changed chunk.
    pub fn accept_all(&mut self, ours: bool) {
        for (c, p) in self.chunks.iter().zip(self.picks.iter_mut()) {
            if let Chunk::Change { .. } = c {
                *p = if ours { Pick::Ours } else { Pick::Theirs };
            }
        }
    }

    /// The lines of chunk `i` in the result.
    pub fn result_lines(&self, i: usize) -> Vec<String> {
        match &self.chunks[i] {
            Chunk::Same(v) => v.clone(),
            Chunk::Change { base, ours, theirs } => match self.picks[i] {
                Pick::Unresolved | Pick::Base => base.clone(),
                Pick::Ours => ours.clone(),
                Pick::Theirs => theirs.clone(),
                Pick::OursThenTheirs => ours.iter().chain(theirs).cloned().collect(),
                Pick::TheirsThenOurs => theirs.iter().chain(ours).cloned().collect(),
            },
        }
    }

    pub fn result_text(&self) -> String {
        let lines: Vec<String> = (0..self.chunks.len()).flat_map(|i| self.result_lines(i)).collect();
        let mut s = lines.join("\n");
        if self.final_newline && !s.is_empty() {
            s.push('\n');
        }
        s
    }
}

impl Workspace {
    fn git_dialog(&mut self) -> Option<&mut GitDialog> {
        self.git.as_mut()?.dialog.as_mut()
    }

    pub(super) fn git_dialog_open(&self) -> bool {
        self.git.as_ref().is_some_and(|g| g.dialog.is_some())
    }

    /// The Conflicts dialog with the repository's conflicted files (after merge, pull, rebase).
    pub(super) fn open_conflicts(&mut self, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_mut() else { return };
        let files = g.repo.conflicts().unwrap_or_default();
        if files.is_empty() {
            g.dialog = None;
            self.show_toast("No conflicts to resolve".into(), false, cx);
            return;
        }
        g.dialog = Some(GitDialog::Conflicts(ConflictsDialog { files, selected: 0 }));
        cx.notify();
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
        let Some(g) = self.git.as_ref() else { return };
        if files.is_empty() {
            return;
        }
        match g.repo.accept(&files, ours) {
            Ok(()) => self.status = format!("Resolved {} using {}", files.join(", "), if ours { "yours" } else { "theirs" }),
            Err(e) => self.show_toast(format!("Cannot resolve: {e}"), true, cx),
        }
        self.after_resolve(cx);
    }

    /// Merge…: the three-pane dialog for the selected file.
    pub(super) fn conflicts_merge(&mut self, cx: &mut Context<Self>) {
        let Some(rel) = self.selected_conflicts().pop() else { return };
        let Some(repo) = self.git.as_ref().map(|g| g.repo.clone()) else { return };
        // Base / yours / theirs (three `git show`s) off the UI thread.
        let r = rel.clone();
        let task = cx.background_executor().spawn(async move { repo.conflict_versions(&r) });
        cx.spawn(async move |this, cx| {
            let res = task.await;
            let _ = this.update(cx, |this, cx| {
                match res {
                    Ok(v) => {
                        if let Some(g) = this.git.as_mut() {
                            g.dialog = Some(GitDialog::Merge(MergeDialog::new(rel, v)));
                        }
                    }
                    Err(e) => this.show_toast(format!("Cannot read the versions of {rel}: {e}"), true, cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Merge dialog Apply: write the result, `git add` it, back to the Conflicts list.
    pub(super) fn merge_apply(&mut self, cx: &mut Context<Self>) {
        let Some(GitDialog::Merge(d)) = self.git_dialog() else { return };
        let (rel, text, left) = (d.rel.clone(), d.result_text(), d.unresolved());
        if left > 0 {
            return self.show_toast(format!("{left} conflict(s) are not resolved yet: use » or « on each red chunk"), true, cx);
        }
        let Some(g) = self.git.as_ref() else { return };
        match g.repo.resolve_with(&rel, &text) {
            Ok(()) => self.status = format!("Merged {rel}"),
            Err(e) => return self.show_toast(format!("Cannot save the merge: {e}"), true, cx),
        }
        self.after_resolve(cx);
    }

    /// Back to the Conflicts list, or close it when nothing is left.
    fn after_resolve(&mut self, cx: &mut Context<Self>) {
        self.reload_editors(cx);
        self.refresh_git(cx);
        let Some(g) = self.git.as_mut() else { return };
        let files = g.repo.conflicts().unwrap_or_default();
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
        // Cancel in Merge goes back to the list.
        if matches!(g.dialog, Some(GitDialog::Merge(_))) {
            let files = g.repo.conflicts().unwrap_or_default();
            if !files.is_empty() {
                g.dialog = Some(GitDialog::Conflicts(ConflictsDialog { files, selected: 0 }));
                cx.notify();
                return;
            }
        }
        g.dialog = None;
        self.focus_editor(window, cx);
        cx.notify();
    }

    pub(super) fn git_dialog_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        let key = k.key.as_str();
        if key == "escape" {
            return self.close_git_dialog(window, cx);
        }
        match self.git_dialog() {
            Some(GitDialog::Conflicts(d)) => match key {
                "up" => d.selected = d.selected.saturating_sub(1),
                "down" => d.selected = (d.selected + 1).min(d.files.len().saturating_sub(1)),
                "enter" => self.conflicts_merge(cx),
                _ => {}
            },
            Some(GitDialog::Merge(d)) => match key {
                // F7 / Shift+F7 or ↓/↑: next / previous change; Alt+→ / Alt+← accept a side.
                "f7" | "down" | "up" => d.current = d.next_conflict(d.current, !(k.modifiers.shift || key == "up")).or(d.current),
                "right" if k.modifiers.alt => {
                    if let Some(i) = d.current {
                        d.apply(i, true);
                    }
                }
                "left" if k.modifiers.alt => {
                    if let Some(i) = d.current {
                        d.apply(i, false);
                    }
                }
                "enter" if k.modifiers.control => self.merge_apply(cx),
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
            GitDialog::Merge(d) => self.render_merge(d, cx),
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
                    cx.listener(move |this, e: &gpui::MouseDownEvent, _, cx| {
                        if let Some(GitDialog::Conflicts(d)) = this.git_dialog() {
                            d.selected = i;
                        }
                        if e.click_count == 2 {
                            this.conflicts_merge(cx);
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
            .child(button("cf-merge", "Merge…", true).on_click(cx.listener(|this, _, _, cx| this.conflicts_merge(cx))));
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

    fn render_merge(&self, d: &MergeDialog, cx: &mut Context<Self>) -> gpui::AnyElement {
        let mono = crate::settings::editor_font();
        let lh = px(20.);
        let col = |lines: &[String], rows: usize, bg: Option<gpui::Hsla>| {
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .when_some(bg, |x, c| x.bg(c))
                .children(lines.iter().map(|l| div().h(lh).px(px(6.)).whitespace_nowrap().child(if l.is_empty() { " ".to_string() } else { l.clone() })))
                .children((lines.len()..rows).map(|_| div().h(lh)))
        };
        let arrow = |id: (&'static str, usize), glyph: &'static str, cx: &mut Context<Self>, act: fn(&mut MergeDialog, usize), i: usize| {
            div()
                .id(id)
                .w(px(26.))
                .h(lh)
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(4.))
                .cursor_pointer()
                .text_color(Theme::accent())
                .hover(|s| s.bg(Theme::hover_row()))
                .active(|s| s.bg(Theme::press()))
                .child(glyph)
                .on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(GitDialog::Merge(d)) = this.git_dialog() {
                        act(d, i);
                        d.current = Some(i);
                    }
                    cx.notify();
                }))
        };
        let mut body: Vec<gpui::AnyElement> = Vec::new();
        for (i, c) in d.chunks.iter().enumerate() {
            let result = d.result_lines(i);
            match c {
                Chunk::Same(lines) => {
                    body.push(
                        div()
                            .flex()
                            .text_color(Theme::text())
                            .child(col(lines, lines.len(), None))
                            .child(div().w(px(26.)))
                            .child(col(lines, lines.len(), None))
                            .child(div().w(px(26.)))
                            .child(col(lines, lines.len(), None))
                            .into_any_element(),
                    );
                }
                Chunk::Change { ours, theirs, base } => {
                    let pick = d.picks[i];
                    let conflict = c.is_conflict();
                    let rows = ours.len().max(theirs.len()).max(result.len()).max(1);
                    let side_bg = |changed: bool| {
                        if conflict {
                            Some(Theme::diff_deleted_bg())
                        } else if changed {
                            Some(Theme::diff_modified_bg())
                        } else {
                            None
                        }
                    };
                    let res_bg = match pick {
                        Pick::Unresolved => Theme::error().opacity(0.25),
                        Pick::Base => Theme::diff_filler_bg(),
                        _ => Theme::diff_added_bg(),
                    };
                    let used_ours = matches!(pick, Pick::Ours | Pick::OursThenTheirs | Pick::TheirsThenOurs);
                    let used_theirs = matches!(pick, Pick::Theirs | Pick::OursThenTheirs | Pick::TheirsThenOurs);
                    let current = d.current == Some(i);
                    body.push(
                        div()
                            .flex()
                            .text_color(Theme::text())
                            .when(current, |x| x.border_1().border_color(Theme::accent()))
                            .child(col(ours, rows, side_bg(ours != base)).when(used_ours, |x| x.opacity(0.6)))
                            .child(
                                div()
                                    .w(px(26.))
                                    .flex()
                                    .flex_col()
                                    .when(!used_ours && ours != base, |x| x.child(arrow(("mg-ours", i), "»", cx, |d, i| d.apply(i, true), i))),
                            )
                            .child(col(&result, rows, Some(res_bg)))
                            .child(
                                div()
                                    .w(px(26.))
                                    .flex()
                                    .flex_col()
                                    .when(!used_theirs && theirs != base, |x| x.child(arrow(("mg-theirs", i), "«", cx, |d, i| d.apply(i, false), i)))
                                    .when(pick != Pick::Base, |x| x.child(arrow(("mg-ignore", i), "×", cx, |d, i| d.ignore(i), i))),
                            )
                            .child(col(theirs, rows, side_bg(theirs != base)).when(used_theirs, |x| x.opacity(0.6)))
                            .into_any_element(),
                    );
                }
            }
        }
        let left = d.unresolved();
        let changes = d.chunks.iter().filter(|c| matches!(c, Chunk::Change { .. })).count();
        let header = |t: &'static str| div().flex_1().px(px(6.)).text_color(Theme::text_muted()).child(t);
        dialog_frame("merge-dialog", 1100., &format!("Merge Revisions for {}", d.rel))
            .h(ui_px(620.))
            .child(
                div()
                    .flex()
                    .pb(px(4.))
                    .child(header("Your version"))
                    .child(div().w(px(26.)))
                    .child(header("Result"))
                    .child(div().w(px(26.)))
                    .child(header("Changes from the other branch")),
            )
            .child(
                div()
                    .id("merge-body")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(Theme::divider())
                    .bg(Theme::editor_bg())
                    .font_family(mono)
                    .text_size(ui_px(13.))
                    .children(body),
            )
            .child(
                div()
                    .pt(px(10.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().text_color(if left > 0 { Theme::error() } else { Theme::file_added() }).child(if left > 0 {
                        format!("{changes} change(s), {left} conflict(s) left")
                    } else {
                        format!("{changes} change(s), all resolved")
                    }))
                    .child(div().flex_1().text_size(ui_px(11.)).text_color(Theme::text_dim()).child("» / « take a side (again: append the other) · × keep base · ↑↓ / F7 changes · Alt+→ / Alt+← · Ctrl+Enter Apply"))
                    .child(button("mg-left", "Accept Left", false).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(GitDialog::Merge(d)) = this.git_dialog() {
                            d.accept_all(true);
                        }
                        cx.notify();
                    })))
                    .child(button("mg-right", "Accept Right", false).on_click(cx.listener(|this, _, _, cx| {
                        if let Some(GitDialog::Merge(d)) = this.git_dialog() {
                            d.accept_all(false);
                        }
                        cx.notify();
                    })))
                    .child(button("mg-cancel", "Cancel", false).on_click(cx.listener(|this, _, w, cx| this.close_git_dialog(w, cx))))
                    .child(button("mg-apply", "Apply", true).on_click(cx.listener(|this, _, _, cx| this.merge_apply(cx)))),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_and_result() {
        let v = brisk_git::ConflictVersions { base: Some("a\nx\nb\nc\n".into()), ours: Some("a\nx\nB1\nc\nd\n".into()), theirs: Some("A\nx\nB2\nc\n".into()) };
        let mut d = MergeDialog::new("x.py".into(), v);
        // A (theirs only) and d (ours only) apply by themselves; b is a conflict.
        assert_eq!(d.unresolved(), 1);
        let i = d.current.expect("the conflict is current");
        assert_eq!(d.result_text(), "A\nx\nb\nc\nd\n");
        d.apply(i, true);
        assert_eq!(d.result_text(), "A\nx\nB1\nc\nd\n");
        d.apply(i, false);
        assert_eq!(d.result_text(), "A\nx\nB1\nB2\nc\nd\n");
        d.ignore(i);
        assert_eq!(d.result_text(), "A\nx\nb\nc\nd\n");
        d.accept_all(false);
        assert_eq!(d.result_text(), "A\nx\nB2\nc\n");
        assert_eq!(d.unresolved(), 0);
    }
}
