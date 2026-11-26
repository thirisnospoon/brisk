//! Right click on a commit in the Git log: PyCharm's commit menu (lite). Copy Revision Number
//! (Ctrl+Alt+Shift+C), Cherry-Pick, Checkout Revision, Reset Current Branch to Here…
//! (Soft / Mixed / Hard / Keep), Revert Commit, Undo Commit.

use super::git_ui::Pending;
use super::menu_ui::{Entry, MenuFn};
use super::{Popup, PopupItem, PopupKind, Target, Workspace};
use brisk_git::{Commit, ResetMode};
use gpui::{ClipboardItem, Context, Pixels, Point};
use std::rc::Rc;

impl Workspace {
    pub(super) fn log_commit_menu(&mut self, c: Commit, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let is_head = c.refs.iter().any(|r| r == "HEAD" || r.starts_with("HEAD -> "));
        let shortcut = |s: &str| s.to_string();
        let item = |label: &str, sc: String, enabled: bool, run: MenuFn| Entry::Item { label: label.into(), shortcut: sc, enabled, run };
        let (c1, c2, c3, c4, c5, c6) = (c.clone(), c.clone(), c.clone(), c.clone(), c.clone(), c.clone());
        let entries = vec![
            item("Copy Revision Number", shortcut("Ctrl+Alt+Shift+C"), true, Rc::new(move |this, _, cx| this.copy_revision(&c1.hash, cx))),
            Entry::Separator,
            item("Cherry-Pick", String::new(), !is_head, Rc::new(move |this, _, cx| this.cherry_pick(c2.clone(), cx))),
            item(
                "Checkout Revision",
                String::new(),
                true,
                Rc::new(move |this, _, cx| {
                    this.save_all(cx);
                    let h = c3.hash.clone();
                    this.git_op("Checkout Revision", move |r| r.checkout_revision(&h), |_, _| {}, cx);
                }),
            ),
            Entry::Separator,
            item("Reset Current Branch to Here…", String::new(), true, Rc::new(move |this, _, cx| this.open_reset(&c4, cx))),
            item("Revert Commit", String::new(), true, Rc::new(move |this, _, cx| this.revert_commit(c5.clone(), cx))),
            item("Undo Commit", String::new(), is_head && !c.parents.is_empty(), Rc::new(move |this, _, cx| this.undo_commit(c6.clone(), cx))),
        ];
        self.open_menu(pos, entries, cx);
    }

    pub(super) fn copy_revision(&mut self, hash: &str, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(hash.to_string()));
        self.show_toast(format!("Copied revision number {}", &hash[..hash.len().min(8)]), false, cx);
    }

    /// Put `text` into the commit message. With `keep_draft` a non-empty draft stays, with
    /// `text` after it.
    fn put_message(&mut self, text: &str, keep_draft: bool, cx: &mut Context<Self>) {
        let Some(message) = self.git(cx).map(|g| g.message.clone()) else { return };
        let draft = message.read(cx).text_string();
        let draft = draft.trim_end();
        let new = if !keep_draft || draft.is_empty() {
            text.to_string()
        } else if draft.contains(text.trim()) {
            return;
        } else {
            format!("{draft}\n\n{text}")
        };
        message.update(cx, |e, cx| e.replace_all_text(&new, cx));
    }

    /// Cherry-Pick / Revert. The draft in the Commit window is left alone, except when the
    /// command stops on conflicts: the commit that finishes it needs `message` (PyCharm).
    fn history_command(
        &mut self,
        title: &'static str,
        pending: &'static str,
        message: String,
        run: impl FnOnce(&brisk_git::Repo) -> brisk_git::Result<String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.save_all(cx);
        self.git_op_with(
            title,
            move |r| {
                let res = run(r);
                let stopped = res.is_err() && r.pending_operation() == Some(pending);
                (res, stopped)
            },
            move |this, _, stopped, cx| {
                if stopped {
                    this.put_message(&message, false, cx);
                }
            },
            cx,
        );
    }

    pub(super) fn cherry_pick(&mut self, c: Commit, cx: &mut Context<Self>) {
        let h = c.hash.clone();
        self.history_command("Cherry-Pick", "cherry-pick", c.subject.clone(), move |r| r.cherry_pick(&h), cx);
    }

    pub(super) fn revert_commit(&mut self, c: Commit, cx: &mut Context<Self>) {
        let h = c.hash.clone();
        let message = format!("Revert \"{}\"\n\nThis reverts commit {}.", c.subject, c.hash);
        self.history_command("Revert Commit", "revert", message, move |r| r.revert_commit(&h), cx);
    }

    /// Undo Commit: its changes become local changes, its full message goes back to the
    /// Commit window (after the draft, if there is one).
    pub(super) fn undo_commit(&mut self, c: Commit, cx: &mut Context<Self>) {
        let h = c.hash.clone();
        let subject = c.subject.clone();
        self.git_op_with(
            "Undo Commit",
            move |r| {
                let message = r.commit_message(&h).unwrap_or(subject);
                (r.undo_commit(&h), message)
            },
            move |this, ok, message, cx| {
                if ok {
                    this.put_message(&message, true, cx);
                }
            },
            cx,
        );
    }

    /// Reset Current Branch to Here…: the four modes with what each does (Mixed first chosen).
    pub(super) fn open_reset(&mut self, c: &Commit, _cx: &mut Context<Self>) {
        let short = c.short().to_string();
        let mut items: Vec<PopupItem> = ResetMode::ALL
            .into_iter()
            .map(|m| PopupItem {
                label: format!("{} reset to {short}", m.label()),
                detail: m.detail().into(),
                hint: String::new(),
                target: Target::Confirm(Pending::Reset(c.hash.clone(), m)),
            })
            .collect();
        items.push(PopupItem { label: "Cancel".into(), detail: String::new(), hint: "Esc".into(), target: Target::Cancel });
        self.popup = Some(Popup { kind: PopupKind::Confirm, query: String::new(), selected: 1, items });
    }

    pub(super) fn run_reset(&mut self, hash: String, mode: ResetMode, cx: &mut Context<Self>) {
        self.save_all(cx);
        self.git_op("Reset", move |r| r.reset_to(&hash, mode), |_, _| {}, cx);
    }
}
