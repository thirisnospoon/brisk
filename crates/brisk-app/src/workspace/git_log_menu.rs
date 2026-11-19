//! Right click on a commit in the Git log: PyCharm's commit menu (lite). Copy Revision Number
//! (Ctrl+Alt+Shift+C), Cherry-Pick, Checkout Revision, Reset Current Branch to Here…
//! (Soft / Mixed / Hard / Keep), Revert Commit, Undo Commit.

use super::git_ui::Pending;
use super::menu_ui::Entry;
use super::{Popup, PopupItem, PopupKind, Target, Workspace};
use brisk_git::{Commit, ResetMode};
use gpui::{ClipboardItem, Context, Pixels, Point};
use std::rc::Rc;

impl Workspace {
    pub(super) fn log_commit_menu(&mut self, c: Commit, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let is_head = c.refs.iter().any(|r| r == "HEAD" || r.starts_with("HEAD -> "));
        let shortcut = |s: &str| s.to_string();
        let item = |label: &str, sc: String, enabled: bool, run: Rc<dyn Fn(&mut Workspace, &mut gpui::Window, &mut Context<Workspace>)>| Entry::Item { label: label.into(), shortcut: sc, enabled, run };
        let (c1, c2, c3, c4, c5, c6) = (c.clone(), c.clone(), c.clone(), c.clone(), c.clone(), c.clone());
        let entries = vec![
            item("Copy Revision Number", shortcut("Ctrl+Alt+Shift+C"), true, Rc::new(move |this, _, cx| this.copy_revision(&c1.hash, cx))),
            Entry::Separator,
            item("Cherry-Pick", String::new(), !is_head, Rc::new(move |this, _, cx| this.cherry_pick(c2.clone(), cx))),
            item("Checkout Revision", String::new(), true, Rc::new(move |this, _, cx| {
                this.save_all(cx);
                let h = c3.hash.clone();
                this.git_op("Checkout Revision", move |r| r.checkout_revision(&h), |_, _| {}, cx);
            })),
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

    /// Put `text` into the commit message; give back what was there.
    fn swap_message(&mut self, text: &str, cx: &mut Context<Self>) -> String {
        let Some(g) = self.git.as_ref() else { return String::new() };
        let old = g.message.read(cx).text_string();
        g.message.update(cx, |e, cx| e.replace_all_text(text, cx));
        old
    }

    /// Cherry-Pick. On conflicts the commit message waits in the Commit window (PyCharm);
    /// on success the message there is put back.
    pub(super) fn cherry_pick(&mut self, c: Commit, cx: &mut Context<Self>) {
        self.save_all(cx);
        let old = self.swap_message(&c.subject, cx);
        let h = c.hash.clone();
        self.git_op("Cherry-Pick", move |r| r.cherry_pick(&h), move |this, cx| {
            this.swap_message(&old, cx);
        }, cx);
    }

    pub(super) fn revert_commit(&mut self, c: Commit, cx: &mut Context<Self>) {
        self.save_all(cx);
        let old = self.swap_message(&format!("Revert \"{}\"\n\nThis reverts commit {}.", c.subject, c.hash), cx);
        let h = c.hash.clone();
        self.git_op("Revert Commit", move |r| r.revert_commit(&h), move |this, cx| {
            this.swap_message(&old, cx);
        }, cx);
    }

    /// Undo Commit: its changes become local changes, its message goes back to the Commit window.
    pub(super) fn undo_commit(&mut self, c: Commit, cx: &mut Context<Self>) {
        let h = c.hash.clone();
        self.git_op("Undo Commit", move |r| r.undo_commit(&h), move |this, cx| {
            this.swap_message(&c.subject, cx);
        }, cx);
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
