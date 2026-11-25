//! Actions on one branch (→ on a row of the Branches popup, or a right click on a branch in
//! the Git log): Checkout, Merge into Current, Show in Log, Delete. Delete never touches the
//! current branch and asks before force-deleting a branch that is not merged.

use super::git_ui::Pending;
use super::menu_ui::Entry;
use super::{BottomTool, Popup, PopupItem, PopupKind, Target, Workspace};
use brisk_git::Branch;
use gpui::{Context, Pixels, Point};
use std::rc::Rc;

#[derive(Clone, Debug)]
pub enum BranchOp {
    Checkout(Branch),
    Merge(String),
    ShowInLog(String),
    Delete(String),
    ForceDelete(String),
}

impl Workspace {
    fn branch_ops(&self, b: &Branch) -> Vec<(String, BranchOp)> {
        let cur = self.branch_label();
        let mut v = Vec::new();
        if !b.current {
            v.push((format!("Checkout {}", b.name), BranchOp::Checkout(b.clone())));
            v.push((format!("Merge '{}' into '{cur}'", b.name), BranchOp::Merge(b.name.clone())));
        }
        v.push(("Show in Log".to_string(), BranchOp::ShowInLog(b.name.clone())));
        if !b.current && !b.remote {
            v.push((format!("Delete {}", b.name), BranchOp::Delete(b.name.clone())));
        }
        v
    }

    /// → on a branch in the Branches popup: its actions as the next popup.
    pub(super) fn open_branch_actions(&mut self, b: &Branch) {
        let items = self
            .branch_ops(b)
            .into_iter()
            .map(|(label, op)| PopupItem { label, detail: String::new(), hint: String::new(), target: Target::Confirm(Pending::Branch(op)) })
            .collect();
        self.popup = Some(Popup { kind: PopupKind::Confirm, query: String::new(), selected: 0, items });
    }

    /// Right click on a branch in the Git log's branch list.
    pub(super) fn branch_menu(&mut self, b: &Branch, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let entries = self
            .branch_ops(b)
            .into_iter()
            .map(|(label, op)| Entry::Item {
                label,
                shortcut: String::new(),
                enabled: true,
                run: Rc::new(move |this, _, cx| this.run_branch_op(op.clone(), cx)),
            })
            .collect();
        self.open_menu(pos, entries, cx);
    }

    pub(super) fn run_branch_op(&mut self, op: BranchOp, cx: &mut Context<Self>) {
        let Some(g) = self.git.as_ref() else { return };
        match op {
            BranchOp::Checkout(b) => self.checkout(b, cx),
            BranchOp::Merge(name) => {
                self.save_all(cx);
                // A conflict fails the merge; refresh_git then opens the Conflicts dialog.
                self.git_op("Merge", move |r| r.merge_into_current(&name), |_, _| {}, cx);
            }
            BranchOp::ShowInLog(name) => {
                self.bottom = Some(BottomTool::Git);
                self.set_log_branch(Some(name), cx);
            }
            BranchOp::Delete(name) => {
                // `git branch --merged` and `git rev-list --count` off the UI thread.
                let repo = g.repo.clone();
                let task = cx.background_executor().spawn({
                    let name = name.clone();
                    async move {
                        if repo.is_merged(&name) {
                            None
                        } else {
                            Some(repo.git_count(&format!("HEAD..{name}")))
                        }
                    }
                });
                cx.spawn(async move |this, cx| {
                    let unmerged = task.await;
                    let _ = this.update(cx, |this, cx| {
                        let Some(n) = unmerged else {
                            return this.git_op(
                                "Delete Branch",
                                move |r| r.delete_branch(&name, false).map(|_| format!("Deleted branch {name}")),
                                |_, _| {},
                                cx,
                            );
                        };
                        let cur = this.branch_label();
                        let detail = format!("not merged into {cur}: {n} commit(s) will be lost");
                        let items = vec![
                            PopupItem {
                                label: format!("Force Delete {name}"),
                                detail,
                                hint: "Enter".into(),
                                target: Target::Confirm(Pending::Branch(BranchOp::ForceDelete(name))),
                            },
                            PopupItem { label: "Cancel".into(), detail: String::new(), hint: "Esc".into(), target: Target::Cancel },
                        ];
                        this.popup = Some(Popup { kind: PopupKind::Confirm, query: String::new(), selected: 1, items });
                        cx.notify();
                    });
                })
                .detach();
            }
            BranchOp::ForceDelete(name) => {
                self.git_op(
                    "Delete Branch",
                    move |r| r.delete_branch(&name, true).map(|_| format!("Deleted branch {name} (it was not merged)")),
                    |_, _| {},
                    cx,
                );
            }
        }
        cx.notify();
    }
}
