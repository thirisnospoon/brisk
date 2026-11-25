//! `BRISK_SMOKE_VCS=1` on an empty scratch folder: makes it a Git repository and checks the
//! VCS work of w5 phase by phase (V1: the diff viewer, V2: the merge window). Never run it on
//! a real project: it commits there. `BRISK_SMOKE_VCS_HOLD=<phase>` stops at that phase with
//! the window open (`diff`: the live side-by-side diff; `unified`: the unified one; `merge`:
//! the merge window as it opens; `merged`: after resolving, before Apply; `annotate`: a file
//! annotated; `previous`: Annotate Previous Revision's tab; `popup`: a change marker's popup;
//! `partial`: a diff with one block left out of the commit; `log`: a commit's menu in the log).

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
            let held = |p: &[&str]| std::env::var_os("BRISK_SMOKE_VCS_HOLD").is_none() || p.iter().any(|p| hold(p));
            if held(&["merge", "merged"]) {
                Self::smoke_vcs_merge(&this, cx, &bg, &root).await;
            }
            if held(&["annotate", "previous"]) {
                Self::smoke_vcs_annotate(&this, cx, &bg, &root).await;
            }
            if held(&["popup"]) {
                Self::smoke_vcs_hunks(&this, cx, &bg, &root).await;
            }
            if held(&["partial"]) {
                Self::smoke_vcs_partial(&this, cx, &bg, &root).await;
            }
            if held(&["log"]) {
                Self::smoke_vcs_log(&this, cx, &bg, &root).await;
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

    /// V6: the change marker popup, Rollback Lines from it and with Ctrl+Alt+Z, Show Diff.
    async fn smoke_vcs_hunks(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        let base: String = (1..=10)
            .map(|i| {
                format!(
                    "v{i} = compute({i}, 'x')
"
                )
            })
            .collect();
        let _ = std::fs::write(root.join("hunks.py"), &base);
        commit_as(root, "Alice", "2024-03-01T10:00:00", "Add hunks.py");
        // Line 2 changes a word, a line is added after 5, line 8 is deleted.
        let mut cur = base.replace("v2 = compute(2, 'x')", "v2 = compute(2, 'y')").replace(
            "v5 = compute(5, 'x')
",
            "v5 = compute(5, 'x')
added = True
",
        );
        cur = cur.replace(
            "v8 = compute(8, 'x')
",
            "",
        );
        let _ = std::fs::write(root.join("hunks.py"), &cur);
        let _ = this.update_in(cx, |this, window, cx| {
            this.refresh_git(cx);
            this.open_file(&root.join("hunks.py"), window, cx);
        });
        pause(bg, 1500).await;
        let _ = this.update_in(cx, |this, _, cx| {
            let Some(ed) = this.active_editor().cloned() else { return };
            eprintln!("smoke-vcs: hunks.py changes -> {}", ed.read(cx).vcs_change_count());
            // A click on the marker of line 2 opens its popup.
            let p = ed.read(cx).vcs_marker_point(1);
            let opened = p.is_some_and(|p| ed.update(cx, |e, cx| e.vcs_marker_click(p, cx)));
            eprintln!("smoke-vcs: marker click -> opened {opened}, change {:?}", ed.read(cx).vcs_popup_change());
        });
        if hold("popup") {
            return;
        }
        let _ = this.update_in(cx, |this, window, cx| {
            let Some(ed) = this.active_editor().cloned() else { return };
            // Rollback Lines in the popup.
            ed.update(cx, |e, cx| {
                e.rollback_change(0, cx);
            });
            eprintln!(
                "smoke-vcs: rollback -> line 2 {:?}, changes {}, popup {:?}",
                ed.read(cx).buffer.line(1),
                ed.read(cx).vcs_change_count(),
                ed.read(cx).vcs_popup_change()
            );
            // Ctrl+Alt+Z with the caret on the added line rolls just it back.
            ed.update(cx, |e, cx| e.go_to_line(5, cx));
            this.smoke_press("ctrl-alt-z", window, cx);
            eprintln!(
                "smoke-vcs: ctrl-alt-z on the added line -> line 6 {:?}, changes {}, confirm dialog {}",
                ed.read(cx).buffer.line(5),
                ed.read(cx).vcs_change_count(),
                this.popup.is_some()
            );
            // The deleted line: open its popup (old text), roll it back too; the file equals HEAD.
            ed.update(cx, |e, cx| e.open_vcs_popup(0, cx));
            ed.update(cx, |e, cx| {
                e.rollback_change(0, cx);
            });
            eprintln!("smoke-vcs: rollback the deletion -> equals HEAD {}", ed.read(cx).text_string() == base);
            // Show Diff from the popup: an edit, then the event opens the live diff.
            ed.update(cx, |e, cx| {
                let t = e.text_string().replace("v3 = ", "v3x = ");
                e.replace_all_text(&t, cx);
                e.open_vcs_popup(0, cx);
                cx.emit(crate::editor::EditorEvent::ShowVcsDiff);
            });
        });
        pause(bg, 1200).await;
        let _ = this.update_in(cx, |this, window, cx| {
            let tab = this.active_diff_view().map(|d| d.read(cx).title.clone());
            eprintln!("smoke-vcs: show diff from the popup -> {tab:?}");
            if let Some(i) = this.active {
                this.close_tab(i, window, cx);
            }
            this.dispatch(brisk_keymap::Action::SaveAll, window, cx);
        });
    }

    /// V6: partial commit by hunks: the second block of a file's diff is left out.
    async fn smoke_vcs_partial(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        let base: String = (1..=12).map(|i| format!("p{i} = {i}\n")).collect();
        let _ = std::fs::write(root.join("partial.py"), &base);
        commit_as(root, "Alice", "2024-04-01T10:00:00", "Add partial.py");
        let cur = base.replace("p2 = 2\n", "p2 = 20\n").replace("p10 = 10\n", "p10 = 100\n");
        let _ = std::fs::write(root.join("partial.py"), &cur);
        let _ = this.update(cx, |this, cx| this.refresh_git(cx));
        pause(bg, 1500).await;
        let change = brisk_git::FileChange { path: "partial.py".into(), old_path: None, status: brisk_git::FileStatus::Modified };
        let c2 = change.clone();
        let _ = this.update_in(cx, |this, window, cx| this.show_change_diff(&c2, window, cx));
        pause(bg, 1500).await;
        let _ = this.update_in(cx, |this, _, cx| {
            let Some(d) = this.active_diff_view() else { return eprintln!("smoke-vcs: partial: no diff tab") };
            eprintln!("smoke-vcs: partial diff -> {} blocks", d.read(cx).hunk_keys(cx).len());
            d.update(cx, |d, cx| d.toggle_hunk(1, cx));
            let partial = this.git.as_ref().is_some_and(|g| g.partial(&change));
            eprintln!("smoke-vcs: second block left out -> commit row partial {partial}");
        });
        if hold("partial") {
            return;
        }
        let _ = this.update_in(cx, |this, window, cx| {
            if let Some(g) = this.git.as_ref() {
                g.message.update(cx, |e, cx| e.replace_all_text("Partial commit", cx));
            }
            this.do_commit(false, window, cx);
        });
        pause(bg, 2500).await;
        let repo = brisk_git::Repo::discover(root);
        let head = repo.as_ref().and_then(|r| r.file_at("HEAD", "partial.py").ok().flatten()).unwrap_or_default();
        let disk = std::fs::read_to_string(root.join("partial.py")).unwrap_or_default();
        let modified = repo
            .and_then(|r| r.status().ok())
            .is_some_and(|st| st.changes.iter().any(|c| c.path == "partial.py" && c.status == brisk_git::FileStatus::Modified));
        eprintln!(
            "smoke-vcs: partial commit -> HEAD has only the first block {}, disk keeps both {}, still modified {modified}",
            head == base.replace("p2 = 2\n", "p2 = 20\n"),
            disk == cur
        );
        let _ = this.update_in(cx, |this, window, cx| {
            if let Some(i) = this.active {
                this.close_tab(i, window, cx);
            }
        });
    }

    /// V3: the log's commit menu: Copy Revision Number, Cherry-Pick, Revert Commit, Undo
    /// Commit, Reset Current Branch to Here.
    async fn smoke_vcs_log(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        let _ = this.update(cx, |this, cx| this.save_all(cx));
        sh(root, &["checkout", "-q", "-b", "pick"]);
        let _ = std::fs::write(
            root.join("pick.py"),
            "picked = True
",
        );
        sh(root, &["add", "pick.py"]);
        commit_as(root, "Bob", "2024-05-01T10:00:00", "Add pick.py");
        sh(root, &["checkout", "-q", "main"]);
        let Some(repo) = brisk_git::Repo::discover(root) else { return };
        let head = || sh(root, &["rev-parse", "HEAD"]).trim().to_string();
        let orig = head();
        let log = repo.log(100).unwrap_or_default();
        let find = |pred: &dyn Fn(&brisk_git::Commit) -> bool| log.iter().find(|c| pred(c)).cloned();
        let (Some(top), Some(pick)) = (find(&|c| c.hash == orig), find(&|c| c.subject == "Add pick.py")) else {
            return eprintln!("smoke-vcs: FAIL log commits not found");
        };
        let at = gpui::point(gpui::px(500.), gpui::px(400.));
        if hold("log") {
            // The Git log open, the menu of the other branch's commit over it.
            let _ = this.update(cx, |this, cx| {
                this.bottom = Some(super::BottomTool::Git);
                this.refresh_git(cx);
            });
            pause(bg, 2500).await;
            let _ = this.update(cx, |this, cx| {
                let i = this.git.as_ref().and_then(|g| g.log.iter().position(|c| c.hash == pick.hash));
                if let Some(i) = i {
                    this.select_log(i, cx);
                }
                this.log_commit_menu(pick.clone(), gpui::point(gpui::px(700.), gpui::px(560.)), cx);
            });
            return;
        }
        let _ = this.update_in(cx, |this, _, cx| {
            this.log_commit_menu(top.clone(), at, cx);
            eprintln!("smoke-vcs: log menu on HEAD -> {:?}", this.menu_labels());
            this.log_commit_menu(pick.clone(), at, cx);
            eprintln!("smoke-vcs: log menu on another branch's commit -> {:?}", this.menu_labels());
            this.menu = None;
            this.copy_revision(&pick.hash, cx);
            let clip = cx.read_from_clipboard().and_then(|c| c.text());
            eprintln!("smoke-vcs: copy revision number -> {}", clip.as_deref() == Some(pick.hash.as_str()));
            this.cherry_pick(pick.clone(), cx);
        });
        pause(bg, 2000).await;
        let message = |this: &WeakEntity<Self>, cx: &mut AsyncWindowContext| {
            this.update(cx, |this, cx| this.git.as_ref().map(|g| g.message.read(cx).text_string())).ok().flatten().unwrap_or_default()
        };
        let picked = head();
        eprintln!(
            "smoke-vcs: cherry-pick -> pick.py {}, new commit {}, message box kept {:?}",
            root.join("pick.py").exists(),
            picked != orig,
            message(this, cx)
        );
        let c = repo.log(1).ok().and_then(|l| l.into_iter().next());
        let _ = this.update(cx, |this, cx| {
            if let Some(c) = c {
                this.revert_commit(c, cx);
            }
        });
        pause(bg, 2000).await;
        let reverted = head();
        eprintln!("smoke-vcs: revert -> pick.py {}, new commit {}", root.join("pick.py").exists(), reverted != picked);
        let c = repo.log(1).ok().and_then(|l| l.into_iter().next());
        let _ = this.update(cx, |this, cx| {
            if let Some(c) = c {
                this.undo_commit(c, cx);
            }
        });
        pause(bg, 2000).await;
        eprintln!("smoke-vcs: undo commit -> HEAD back {}, message box {:?}", head() == picked, message(this, cx));
        let _ = this.update(cx, |this, cx| {
            this.open_reset(&top, cx);
            let labels: Vec<String> = this.popup.as_ref().map(|p| p.items.iter().map(|i| i.label.clone()).collect()).unwrap_or_default();
            eprintln!("smoke-vcs: reset popup -> {labels:?}");
            this.popup = None;
            this.run_reset(top.hash.clone(), brisk_git::ResetMode::Hard, cx);
        });
        pause(bg, 2000).await;
        eprintln!("smoke-vcs: hard reset to the first commit's place -> HEAD {}, pick.py {}", head() == orig, root.join("pick.py").exists());
        let _ = this.update(cx, |this, cx| {
            if let Some(g) = this.git.as_ref() {
                g.message.update(cx, |e, cx| e.replace_all_text("", cx));
            }
        });
    }

    /// V5: Annotate with Git Blame through a rename; the annotation menu; Annotate Previous
    /// Revision (twice, following the rename) in read-only tabs.
    async fn smoke_vcs_annotate(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        let _ = std::fs::write(
            root.join("old_name.py"),
            "a = 1
b = 1
c = 1
d = 1
e = 1
f = 1
",
        );
        commit_as(root, "Alice", "2024-02-01T10:00:00", "Add old_name.py");
        sh(root, &["mv", "old_name.py", "ann.py"]);
        let _ = std::fs::write(
            root.join("ann.py"),
            "a = 1
b = 2
c = 1
d = 1
e = 1
f = 1
",
        );
        commit_as(root, "Bob", "2024-02-02T10:00:00", "Rename to ann.py, b = 2");
        let _ = std::fs::write(
            root.join("ann.py"),
            "a = 1
b = 2
c = 3
d = 1
e = 1
f = 1
",
        );
        commit_as(root, "Carol", "2024-02-03T10:00:00", "c = 3");
        let _ = this.update_in(cx, |this, window, cx| {
            this.refresh_git(cx);
            this.open_file(&root.join("ann.py"), window, cx);
            if let Some(ed) = this.active_editor().cloned() {
                this.toggle_annotate(ed, cx);
            }
        });
        pause(bg, 1500).await;
        let labels = |ed: &gpui::Entity<crate::editor::Editor>, cx: &gpui::App| -> Vec<String> {
            (0..3).filter_map(|l| ed.read(cx).annotation_line(l)).map(|(_, label)| label).collect()
        };
        let _ = this.update_in(cx, |this, _, cx| {
            let Some(ed) = this.active_editor().cloned() else { return };
            eprintln!("smoke-vcs: annotate ann.py -> {:?}", labels(&ed, cx));
            // Right click on line 3's annotation (Carol): PyCharm's annotation menu.
            if let Some(p) = ed.read(cx).annotation_point(2) {
                this.open_gutter_menu(ed.clone(), p, cx);
                eprintln!("smoke-vcs: annotation menu -> {:?}", this.menu_labels());
                this.menu = None;
            }
        });
        if hold("annotate") {
            return;
        }
        // Previous revision of Carol's line: the file before her commit (Bob's), annotated.
        let _ = this.update_in(cx, |this, window, cx| {
            let Some(ed) = this.active_editor().cloned() else { return };
            if let Some(h) = ed.read(cx).annotation_hash(2) {
                this.annotate_previous(ed, h, window, cx);
            }
        });
        pause(bg, 1500).await;
        let _ = this.update_in(cx, |this, window, cx| {
            let Some(ed) = this.active_editor().cloned() else { return };
            let e = ed.read(cx);
            eprintln!(
                "smoke-vcs: previous revision tab '{}' {:?} -> line 3 {:?}, {:?}",
                e.title(),
                e.revision.as_ref().map(|r| r.0.clone()),
                e.buffer.line(2),
                labels(&ed, cx)
            );
            // Read-only: typing does nothing.
            ed.update(cx, |e, cx| {
                e.replace_all_text(
                    "changed
", cx,
                )
            });
            eprintln!("smoke-vcs: typing in it -> unchanged {}", ed.read(cx).buffer.line(2) == "c = 1");
            // Again on Bob's line 2: before the rename, as old_name.py.
            if let Some(h) = ed.read(cx).annotation_hash(1) {
                this.annotate_previous(ed, h, window, cx);
            }
        });
        pause(bg, 1500).await;
        let _ = this.update_in(cx, |this, window, cx| {
            let Some(ed) = this.active_editor().cloned() else { return };
            eprintln!(
                "smoke-vcs: and its previous revision '{}' {:?} -> {:?}",
                ed.read(cx).title(),
                ed.read(cx).revision.as_ref().map(|r| r.0.clone()),
                labels(&ed, cx)
            );
            if hold("previous") {
                return;
            }
            // Alice added the file: no previous revision.
            if let Some(h) = ed.read(cx).annotation_hash(0) {
                this.annotate_previous(ed, h, window, cx);
            }
        });
        pause(bg, 1000).await;
        let _ = this.update_in(cx, |this, _, _| {
            eprintln!("smoke-vcs: previous of the first commit -> toast {:?}", this.toast.as_ref().map(|t| t.0.clone()));
        });
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
        let _ = std::fs::write(
            root.join("merge2.py"),
            base.replace("f(1, 2)", "f(1, 20)").replace("\"base\"", "\"theirs\"")
                + "extra = 1
",
        );
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
            eprintln!(
                "smoke-vcs: merge window '{}' | '{}' -> {} changes, {} conflicts, current {:?}",
                v.title,
                v.theirs_label,
                v.model.changes(),
                v.model.unresolved_conflicts(),
                v.current
            );
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
                let t = format!(
                    "# header
{}",
                    e.text_string()
                );
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
            eprintln!(
                "smoke-vcs: typed in the conflict -> unresolved {}, conflicts {}",
                m.read(cx).model.unresolved(),
                m.read(cx).model.unresolved_conflicts()
            );
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
            eprintln!(
                "smoke-vcs: applied -> disk as expected {}, merge tab open {}, conflicts {:?}",
                disk == want,
                this.active_merge_view().is_some(),
                sh(root, &["diff", "--name-only", "--diff-filter=U"])
            );
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
