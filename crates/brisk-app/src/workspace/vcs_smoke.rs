//! `BRISK_SMOKE_VCS=1` on an empty scratch folder: makes it a Git repository and checks the
//! VCS work of w5 phase by phase (V1: the diff viewer, V2: the merge window). Never run it on
//! a real project: it commits there. `BRISK_SMOKE_VCS_HOLD=<phase>` stops at that phase with
//! the window open (`diff`: the live side-by-side diff; `unified`: the unified one; `merge`:
//! the merge window as it opens; `merged`: after resolving, before Apply).

use super::git_smoke::{commit_as, sh};
use super::{Tab, Workspace};
use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use std::path::Path;
use std::time::Duration;

fn hold(phase: &str) -> bool {
    std::env::var("BRISK_SMOKE_VCS_HOLD").is_ok_and(|v| v == phase)
}

async fn pause(bg: &gpui::BackgroundExecutor, ms: u64) {
    bg.timer(Duration::from_millis(ms)).await;
}

/// 60 lines of Python; `v` = 1 is the committed version, 2 the working copy:
/// line 10 changes one word, a line is inserted after 40, line 50 is deleted.
fn sample(v: u8) -> String {
    let mut out = String::new();
    for i in 1..=60 {
        if v == 2 && i == 50 {
            continue;
        }
        if i == 10 {
            out.push_str(if v == 1 { "total = compute(bar, 1)\n" } else { "total = compute(baz, 1)\n" });
        } else {
            out.push_str(&format!("value_{i} = {i}  # line {i}\n"));
        }
        if v == 2 && i == 40 {
            out.push_str("inserted = True\n");
        }
    }
    out
}

impl Workspace {
    pub(super) fn vcs_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        if root.join(".git").exists() && !root.join("diffme.py").exists() {
            eprintln!("smoke-vcs: {} is already a repository; run it on an empty folder", root.display());
            cx.quit();
            return;
        }
        sh(&root, &["init", "-q", "-b", "main"]);
        sh(&root, &["config", "user.name", "Smoke"]);
        sh(&root, &["config", "user.email", "smoke@example.com"]);
        sh(&root, &["config", "core.autocrlf", "false"]);
        let _ = std::fs::write(root.join("diffme.py"), sample(1));
        commit_as(&root, "Alice", "2024-01-10T10:00:00", "Add diffme.py");
        let _ = std::fs::write(root.join("diffme.py"), sample(2));
        if let Some(repo) = brisk_git::Repo::discover(&root) {
            self.git = Some(super::git_ui::Git::new(repo, cx));
            self.refresh_git(cx);
        }
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            Self::smoke_vcs_diff(&this, cx, &bg, &root).await;
            if std::env::var_os("BRISK_SMOKE_VCS_HOLD").is_none() || hold("merge") || hold("merged") {
                Self::smoke_vcs_merge(&this, cx, &bg, &root).await;
            }
            let _ = this.update(cx, |_, cx| {
                eprintln!("smoke-vcs: done");
                if std::env::var_os("BRISK_SMOKE_VCS_HOLD").is_none() {
                    cx.quit();
                }
            });
        })
        .detach();
    }

    fn active_diff_view(&self) -> Option<gpui::Entity<crate::diff_view::DiffView>> {
        match self.active.and_then(|i| self.tabs.get(i)) {
            Some(Tab::Diff(d)) => Some(d.clone()),
            _ => None,
        }
    }

    fn active_merge_view(&self) -> Option<gpui::Entity<crate::merge_view::MergeView>> {
        match self.active.and_then(|i| self.tabs.get(i)) {
            Some(Tab::Merge(m)) => Some(m.clone()),
            _ => None,
        }
    }

    /// V2: a merge with a simple conflict (different words of one line), a real one and a
    /// change on one side; typing in the result, then Apply.
    async fn smoke_vcs_merge(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        let _ = this.update_in(cx, |this, window, cx| this.dispatch(brisk_keymap::Action::SaveAll, window, cx));
        sh(root, &["add", "-A"]);
        commit_as(root, "Alice", "2024-01-11T10:00:00", "Diff phase");
        let base = "def f(a, b):
    return a + b
x = f(1, 2)
sep = 0
name = \"base\"
tail = 0
";
        let _ = std::fs::write(root.join("merge2.py"), base);
        commit_as(root, "Alice", "2024-01-12T10:00:00", "Add merge2.py");
        sh(root, &["switch", "-q", "-c", "feature"]);
        let _ = std::fs::write(root.join("merge2.py"), base.replace("f(1, 2)", "f(1, 20)").replace("\"base\"", "\"theirs\"") + "extra = 1
");
        commit_as(root, "Bob", "2024-01-13T10:00:00", "Feature");
        sh(root, &["switch", "-q", "main"]);
        let _ = std::fs::write(root.join("merge2.py"), base.replace("f(1, 2)", "f(10, 2)").replace("\"base\"", "\"ours\""));
        commit_as(root, "Alice", "2024-01-14T10:00:00", "Main");
        sh(root, &["merge", "--no-edit", "feature"]);
        let _ = this.update(cx, |this, cx| this.refresh_git(cx));
        pause(bg, 1500).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-vcs: merge stopped -> conflicts dialog {}", this.git_dialog_open());
            // Show Diff on the conflicted file in the Commit window opens the merge window.
            let c = brisk_git::FileChange { path: "merge2.py".into(), old_path: None, status: brisk_git::FileStatus::Conflict };
            this.show_change_diff(&c, window, cx);
        });
        pause(bg, 1200).await;
        let _ = this.update_in(cx, |this, _, cx| {
            let Some(m) = this.active_merge_view() else {
                eprintln!("smoke-vcs: FAIL no merge tab");
                return;
            };
            let v = m.read(cx);
            eprintln!("smoke-vcs: merge window '{}' | '{}' -> {} changes, {} conflicts, current {:?}", v.title, v.theirs_label, v.model.changes(), v.model.unresolved_conflicts(), v.current);
        });
        if hold("merge") {
            return;
        }
        let _ = this.update_in(cx, |this, _, cx| {
            let Some(m) = this.active_merge_view() else { return };
            let n = m.update(cx, |v, cx| v.resolve_simple(cx));
            eprintln!("smoke-vcs: resolve simple conflicts -> {n}, conflicts left {}", m.read(cx).model.unresolved_conflicts());
            // Typing: a header line on top (the changes below move down), and the last conflict by hand.
            let ed = m.read(cx).editor();
            ed.update(cx, |e, cx| {
                let t = format!("# header
{}", e.text_string());
                e.replace_all_text(&t, cx);
            });
        });
        pause(bg, 200).await;
        let _ = this.update_in(cx, |this, _, cx| {
            let Some(m) = this.active_merge_view() else { return };
            let ed = m.read(cx).editor();
            ed.update(cx, |e, cx| {
                let t = e.text_string().replace("name = \"base\"", "name = \"mine\"");
                e.replace_all_text(&t, cx);
            });
        });
        pause(bg, 200).await;
        let _ = this.update_in(cx, |this, _, cx| {
            let Some(m) = this.active_merge_view() else { return };
            eprintln!("smoke-vcs: typed in the conflict -> unresolved {}, conflicts {}", m.read(cx).model.unresolved(), m.read(cx).model.unresolved_conflicts());
            m.update(cx, |v, cx| v.apply_non_conflicting(None, cx));
            eprintln!("smoke-vcs: non-conflicting applied -> unresolved {}, result {:?}", m.read(cx).model.unresolved(), m.read(cx).text());
        });
        if hold("merged") {
            return;
        }
        let _ = this.update_in(cx, |this, _, cx| {
            if let Some(m) = this.active_merge_view() {
                m.update(cx, |v, cx| v.press_apply(cx));
            }
        });
        pause(bg, 800).await;
        let _ = this.update_in(cx, |this, _, _| {
            let disk = std::fs::read_to_string(root.join("merge2.py")).unwrap_or_default();
            let want = "# header
def f(a, b):
    return a + b
x = f(10, 20)
sep = 0
name = \"mine\"
tail = 0
extra = 1
";
            eprintln!("smoke-vcs: applied -> disk as expected {}, merge tab open {}, conflicts {:?}", disk == want, this.active_merge_view().is_some(), sh(root, &["diff", "--name-only", "--diff-filter=U"]));
            if disk != want {
                eprintln!("smoke-vcs: FAIL disk {disk:?}");
            }
        });
    }

    /// V1: word highlights, unified and collapsed views, the live (editable) right side.
    async fn smoke_vcs_diff(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        // Fixed starting options, whatever an earlier run left in diff.json.
        let _ = this.update_in(cx, |this, window, cx| {
            let c = brisk_git::FileChange { path: "diffme.py".into(), old_path: None, status: brisk_git::FileStatus::Modified };
            this.show_change_diff(&c, window, cx);
        });
        pause(bg, 1500).await;
        let _ = this.update_in(cx, |this, _, cx| {
            let Some(d) = this.active_diff_view() else {
                eprintln!("smoke-vcs: FAIL no diff tab");
                return;
            };
            d.update(cx, |d, cx| {
                d.set_unified(false, cx);
                d.set_collapse(true, cx);
            });
            let (l, r, n) = d.read(cx).summary();
            let live = d.read(cx).live_editor().is_some();
            let owned = d.read(cx).owned_editor().is_some();
            eprintln!("smoke-vcs: diff '{l}' | '{r}' -> {n} differences, live {live}, owned editor {owned}");
            let words = d.update(cx, |d, _| d.words_of_change(0));
            eprintln!("smoke-vcs: changed words of the first modified line -> {words:?}");
            eprintln!("smoke-vcs: side-by-side collapsed (unified, collapse, rows, folded) -> {:?}", d.read(cx).layout_state());
        });
        if hold("diff") {
            return;
        }
        let _ = this.update_in(cx, |this, _, cx| {
            let Some(d) = this.active_diff_view() else { return };
            d.update(cx, |d, cx| d.set_unified(true, cx));
            eprintln!("smoke-vcs: unified collapsed -> {:?}", d.read(cx).layout_state());
            d.update(cx, |d, cx| d.set_collapse(false, cx));
            eprintln!("smoke-vcs: unified expanded -> {:?}", d.read(cx).layout_state());
            d.update(cx, |d, cx| {
                d.set_collapse(true, cx);
                d.set_unified(false, cx);
            });
        });
        if hold("unified") {
            let _ = this.update_in(cx, |this, _, cx| {
                if let Some(d) = this.active_diff_view() {
                    d.update(cx, |d, cx| d.set_unified(true, cx));
                }
            });
            return;
        }
        // Live: an edit in the right side re-aligns the diff; closing the tab saves the file.
        let _ = this.update_in(cx, |this, _, cx| {
            let Some(d) = this.active_diff_view() else { return };
            let Some(ed) = d.read(cx).live_editor() else {
                eprintln!("smoke-vcs: FAIL the working-copy diff is not editable");
                return;
            };
            ed.update(cx, |e, cx| {
                let t = e.text_string().replace("value_5 = 5", "value_5 = 555");
                e.replace_all_text(&t, cx);
            });
        });
        pause(bg, 300).await;
        let _ = this.update_in(cx, |this, window, cx| {
            let Some(d) = this.active_diff_view() else { return };
            eprintln!("smoke-vcs: after editing line 5 -> {} differences", d.read(cx).summary().2);
            if let Some(i) = this.active {
                this.close_tab(i, window, cx);
            }
            let disk = std::fs::read_to_string(root.join("diffme.py")).unwrap_or_default();
            eprintln!("smoke-vcs: closed the diff -> saved to disk {}", disk.contains("value_5 = 555"));
        });
    }
}
