//! `BRISK_SMOKE_GITX=1` on an empty scratch folder: makes it a Git repository with a short
//! history (two authors) and checks Annotate with Git Blame, and the other Git features
//! added with it. Never run it on a real project: it commits there.

use super::Workspace;
use brisk_keymap::Action;
use gpui::{AsyncWindowContext, Context, WeakEntity, Window};
use std::path::Path;
use std::time::Duration;

/// Run git in `dir` for the smoke setup; prints failures.
pub(super) fn sh(dir: &Path, args: &[&str]) -> String {
    let mut c = std::process::Command::new("git");
    c.arg("-C").arg(dir).args(args).env("GIT_TERMINAL_PROMPT", "0");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    match c.output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => {
            eprintln!("smoke-gitx: git {args:?} failed: {}", String::from_utf8_lossy(&o.stderr).trim());
            String::new()
        }
        Err(e) => {
            eprintln!("smoke-gitx: cannot run git: {e}");
            String::new()
        }
    }
}

/// Commit everything as `who`, with a fixed date so ages differ.
pub(super) fn commit_as(dir: &Path, who: &str, date: &str, msg: &str) {
    sh(dir, &["add", "-A"]);
    let author = format!("{who} <{}@example.com>", who.to_lowercase());
    let mut c = std::process::Command::new("git");
    c.arg("-C").arg(dir).args(["commit", "-q", "-m", msg, "--author", &author, "--date", date]).env("GIT_COMMITTER_DATE", date);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let _ = c.output();
}

/// `BRISK_SMOKE_GITX_HOLD=<phase>`: stop at that phase and keep the window open.
fn hold(phase: &str) -> bool {
    std::env::var("BRISK_SMOKE_GITX_HOLD").is_ok_and(|v| v == phase)
}

async fn pause(bg: &gpui::BackgroundExecutor, ms: u64) {
    bg.timer(Duration::from_millis(ms)).await;
}

impl Workspace {
    pub(super) fn gitx_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let root = self.root.clone();
        if root.join(".git").exists() && !root.join("blame.py").exists() {
            eprintln!("smoke-gitx: {} is already a repository; run it on an empty folder", root.display());
            cx.quit();
            return;
        }
        // History: Alice writes three lines, Bob changes the second one later.
        sh(&root, &["init", "-q", "-b", "main"]);
        sh(&root, &["config", "user.name", "Smoke"]);
        sh(&root, &["config", "user.email", "smoke@example.com"]);
        sh(&root, &["config", "core.autocrlf", "false"]);
        let _ = std::fs::write(root.join("blame.py"), "a = 1\nb = 2\nc = 3\n");
        commit_as(&root, "Alice", "2024-01-10T10:00:00", "Add blame.py");
        let _ = std::fs::write(root.join("blame.py"), "a = 1\nb = 20\nc = 3\n");
        commit_as(&root, "Bob", "2025-06-01T10:00:00", "Bump b to 20");
        if let Some(repo) = brisk_git::Repo::discover(&root) {
            self.git = Some(super::git_ui::Git::new(repo, cx));
            self.refresh_git(cx);
        }
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            Self::smoke_annotate(&this, cx, &bg, &root).await;
            if !hold("annotate") {
                Self::smoke_conflicts(&this, cx, &bg, &root).await;
            }
            if !hold("annotate") && !hold("merge") {
                Self::smoke_stash(&this, cx, &bg, &root).await;
            }
            if !hold("annotate") && !hold("merge") && !hold("stash") {
                Self::smoke_history(&this, cx, &bg, &root).await;
            }
            let _ = this.update(cx, |_, cx| {
                eprintln!("smoke-gitx: done");
                if std::env::var_os("BRISK_SMOKE_GITX_HOLD").is_none() {
                    cx.quit();
                }
            });
        })
        .detach();
    }

    async fn smoke_annotate(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        let show = |this: &mut Self, cx: &mut Context<Self>| {
            let Some(ed) = this.active_editor().cloned() else { return String::from("no editor") };
            let e = ed.read(cx);
            (0..e.buffer.len_lines()).map(|l| format!("{}: {:?}", l + 1, e.annotation_line(l))).collect::<Vec<_>>().join(" | ")
        };
        let _ = this.update_in(cx, |this, window, cx| {
            this.open_file(&root.join("blame.py"), window, cx);
            this.dispatch(Action::Annotate, window, cx);
        });
        pause(bg, 1500).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: annotate -> {}", show(this, cx));
            let Some(ed) = this.active_editor().cloned() else { return };
            // The gutter menu offers to close them now.
            this.open_gutter_menu(ed.clone(), gpui::point(gpui::px(30.), gpui::px(120.)), cx);
            eprintln!("smoke-gitx: gutter menu -> {:?}", this.menu_labels());
            this.menu = None;
            // A click on Bob's line opens that commit's diff of the file.
            let hash = ed.read(cx).annotation.as_ref().and_then(|a| a.lines.get(1).cloned().flatten()).map(|l| l.hash);
            if let Some(h) = hash {
                this.show_annotation_commit(ed.clone(), h, window, cx);
                let tab = this.active.and_then(|i| match this.tabs.get(i) {
                    Some(super::Tab::Diff(d)) => Some(d.read(cx).title.clone()),
                    _ => None,
                });
                eprintln!("smoke-gitx: click on line 2 -> diff tab {tab:?}");
            }
            // Edit and save: the new line is not committed, the others shift down.
            this.open_file(&root.join("blame.py"), window, cx);
            ed.update(cx, |e, _| {
                e.buffer.set_caret(0, false);
                e.buffer.insert("z = 0\n");
            });
            this.dispatch(Action::SaveAll, window, cx);
        });
        pause(bg, 2000).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: after edit + save -> {}", show(this, cx));
            if hold("annotate") {
                return;
            }
            this.dispatch(Action::Annotate, window, cx);
            eprintln!("smoke-gitx: annotate again (close) -> shown={:?}", this.active_editor().map(|e| e.read(cx).annotation_shown()));
        });
    }

    async fn smoke_conflicts(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        // Two files changed on both branches: merge.py (one conflict plus changes on one side
        // only) and other.py (resolved with Accept Theirs).
        let _ = this.update_in(cx, |this, window, cx| {
            // Close the editors of the annotate phase so nothing unsaved stays around.
            this.dispatch(Action::SaveAll, window, cx);
        });
        let _ = std::fs::write(root.join("merge.py"), "import os\nx = 1\ny = 2\nz = 3\n");
        let _ = std::fs::write(root.join("other.py"), "v = 'base'\n");
        commit_as(root, "Alice", "2025-07-01T10:00:00", "Add merge.py");
        sh(root, &["switch", "-q", "-c", "feature"]);
        let _ = std::fs::write(root.join("merge.py"), "import os\nx = 1\ny = 'theirs'\nz = 3\nw = 4\n");
        let _ = std::fs::write(root.join("other.py"), "v = 'theirs'\n");
        commit_as(root, "Bob", "2025-07-02T10:00:00", "Feature change");
        sh(root, &["switch", "-q", "main"]);
        let _ = std::fs::write(root.join("merge.py"), "import sys\nx = 1\ny = 'ours'\nz = 3\n");
        let _ = std::fs::write(root.join("other.py"), "v = 'ours'\n");
        commit_as(root, "Alice", "2025-07-03T10:00:00", "Main change");
        // Like Git > Merge: it stops with conflicts.
        sh(root, &["merge", "--no-edit", "feature"]);
        let _ = this.update(cx, |this, cx| this.refresh_git(cx));
        pause(bg, 1500).await;
        let _ = this.update_in(cx, |this, window, cx| {
            let files = match this.git.as_ref().and_then(|g| g.dialog.as_ref()) {
                Some(super::git_merge_ui::GitDialog::Conflicts(d)) => d.files.clone(),
                _ => Vec::new(),
            };
            eprintln!("smoke-gitx: conflicts dialog opened by itself -> {files:?}");
            // Enter: Merge… on merge.py.
            this.smoke_press("enter", window, cx);
            if let Some(super::git_merge_ui::GitDialog::Merge(d)) = this.git.as_ref().and_then(|g| g.dialog.as_ref()) {
                eprintln!("smoke-gitx: merge dialog {} -> {} chunk(s), {} unresolved, result now {:?}", d.rel, d.chunks.len(), d.unresolved(), d.result_text());
            }
            if hold("merge") {
                return;
            }
            // Ctrl+Enter refuses while a conflict is left; Alt+→ takes ours, Alt+← appends theirs.
            this.smoke_press("ctrl-enter", window, cx);
            eprintln!("smoke-gitx: apply with a conflict left -> toast {:?}", this.toast.as_ref().map(|t| t.0.clone()));
            this.smoke_press("alt-right alt-left", window, cx);
            if let Some(super::git_merge_ui::GitDialog::Merge(d)) = this.git.as_ref().and_then(|g| g.dialog.as_ref()) {
                eprintln!("smoke-gitx: >> then << -> {:?}", d.result_text());
            }
            this.smoke_press("ctrl-enter", window, cx);
            eprintln!("smoke-gitx: merge.py on disk -> {:?}", std::fs::read_to_string(this.root.join("merge.py")).unwrap_or_default());
            let left = match this.git.as_ref().and_then(|g| g.dialog.as_ref()) {
                Some(super::git_merge_ui::GitDialog::Conflicts(d)) => d.files.clone(),
                _ => Vec::new(),
            };
            eprintln!("smoke-gitx: back in the list -> {left:?}");
            this.conflicts_accept(false, cx);
            eprintln!("smoke-gitx: accept theirs -> other.py {:?}, dialog open {}", std::fs::read_to_string(this.root.join("other.py")).unwrap_or_default(), this.git_dialog_open());
        });
        eprintln!("smoke-gitx: git status -> {:?}", sh(root, &["status", "--porcelain"]));
        sh(root, &["commit", "-q", "--no-edit"]);
        pause(bg, 500).await;
    }

    async fn smoke_stash(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        use super::git_stash_ui::StashDialog;
        fn dialog(this: &Workspace) -> Option<&StashDialog> {
            match this.git.as_ref()?.dialog.as_ref()? {
                super::git_merge_ui::GitDialog::Stash(d) => Some(d),
                _ => None,
            }
        }
        // A tracked change and an untracked file.
        let _ = std::fs::write(root.join("other.py"), "v = 'stashed'\n");
        let _ = std::fs::write(root.join("note.py"), "note = 1\n");
        let _ = this.update(cx, |this, cx| this.refresh_git(cx));
        pause(bg, 1200).await;
        let _ = this.update_in(cx, |this, window, cx| {
            this.dispatch(Action::StashChanges, window, cx);
            // Typed message, Alt+U includes untracked files, Enter creates the stash.
            for ch in "wip 1".chars() {
                let keystroke = gpui::Keystroke { modifiers: gpui::Modifiers::default(), key: ch.to_string(), key_char: Some(ch.to_string()) };
                this.on_key_down(&gpui::KeyDownEvent { keystroke, is_held: false }, window, cx);
            }
            this.smoke_press("backspace alt-u", window, cx);
            if let Some(StashDialog::Push { message, untracked }) = dialog(this) {
                eprintln!("smoke-gitx: stash dialog -> message {message:?}, untracked {untracked}");
            }
            this.smoke_press("enter", window, cx);
        });
        pause(bg, 1500).await;
        eprintln!(
            "smoke-gitx: after stash -> other.py {:?}, note.py exists {}, list {:?}",
            std::fs::read_to_string(root.join("other.py")).unwrap_or_default(),
            root.join("note.py").exists(),
            sh(root, &["stash", "list"])
        );
        let _ = this.update_in(cx, |this, window, cx| {
            this.dispatch(Action::UnstashChanges, window, cx);
            if let Some(StashDialog::List { entries, files, .. }) = dialog(this) {
                let names: Vec<String> = entries.iter().map(|e| format!("{} {}", e.name, e.message)).collect();
                let paths: Vec<String> = files.iter().map(|f| format!("{:?} {}", f.status, f.path)).collect();
                eprintln!("smoke-gitx: unstash dialog -> {names:?}, files {paths:?}");
            }
        });
        if hold("stash") {
            return;
        }
        let _ = this.update_in(cx, |this, window, cx| {
            // A file row opens its diff (stash^ vs stash) and closes the dialog.
            this.stash_file_diff(0, window, cx);
            eprintln!("smoke-gitx: file diff -> dialog open {}, tabs {}", this.git_dialog_open(), this.tabs.len());
            // Apply keeps the stash.
            this.dispatch(Action::UnstashChanges, window, cx);
            this.smoke_press("enter", window, cx);
        });
        pause(bg, 1500).await;
        eprintln!(
            "smoke-gitx: after apply -> other.py {:?}, note.py exists {}, list {:?}",
            std::fs::read_to_string(root.join("other.py")).unwrap_or_default(),
            root.join("note.py").exists(),
            sh(root, &["stash", "list"])
        );
        let _ = this.update_in(cx, |this, window, cx| {
            this.dispatch(Action::UnstashChanges, window, cx);
            this.smoke_press("delete", window, cx);
            eprintln!("smoke-gitx: drop -> dialog open {}, toast {:?}, list {:?}", this.git_dialog_open(), this.toast.as_ref().map(|t| t.0.clone()), sh(&this.root, &["stash", "list"]));
        });
        pause(bg, 300).await;
    }

    async fn smoke_history(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        fn dialog(this: &Workspace) -> Option<&super::git_history_ui::HistoryDialog> {
            match this.git.as_ref()?.dialog.as_ref()? {
                super::git_merge_ui::GitDialog::History(d) => Some(d),
                _ => None,
            }
        }
        fn show(this: &Workspace) -> String {
            match dialog(this) {
                Some(d) => {
                    let subjects: Vec<&str> = d.commits.iter().map(|c| c.subject.as_str()).collect();
                    let changed: Vec<&str> = d.patch.lines().filter(|l| (l.starts_with('+') || l.starts_with('-')) && !l.starts_with("+++") && !l.starts_with("---")).collect();
                    format!("{subjects:?}, selected {} patch {changed:?}", d.selected)
                }
                None => "no dialog".into(),
            }
        }
        let file = root.join("blame.py");
        let _ = this.update_in(cx, |this, window, cx| {
            this.open_file(&file, window, cx);
            // Tree context menu of a file: the Git entries.
            let labels: Vec<String> = this.git_menu_entries(&file, None, cx).iter().filter_map(|e| match e {
                super::menu_ui::Entry::Item { label, .. } => Some(label.clone()),
                _ => None,
            }).collect();
            eprintln!("smoke-gitx: tree menu git entries -> {labels:?}");
            this.dispatch(Action::ShowFileHistory, window, cx);
        });
        pause(bg, 1200).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: history -> {}", show(this));
            this.smoke_press("down", window, cx);
            eprintln!("smoke-gitx: history down -> {}", show(this));
        });
        if hold("history") {
            return;
        }
        let _ = this.update_in(cx, |this, window, cx| {
            let tabs = this.tabs.len();
            this.smoke_press("ctrl-d", window, cx);
            let tab = this.active.and_then(|i| match this.tabs.get(i) {
                Some(super::Tab::Diff(d)) => Some(d.read(cx).title.clone()),
                _ => None,
            });
            eprintln!("smoke-gitx: ctrl-d -> dialog open {}, tabs {tabs} -> {}, active diff {tab:?}", this.git_dialog_open(), this.tabs.len());
            // Show History for Selection: line 1 ("a = 1") only changed in the first commit.
            this.open_file(&file, window, cx);
            if let Some(ed) = this.active_editor().cloned() {
                ed.update(cx, |e, cx| e.select_on_line(0, 0..5, cx));
                let labels: Vec<String> = this.git_menu_entries(&file, Some(&ed), cx).iter().filter_map(|e| match e {
                    super::menu_ui::Entry::Item { label, .. } => Some(label.clone()),
                    _ => None,
                }).collect();
                eprintln!("smoke-gitx: editor menu git entries -> {labels:?}");
            }
            this.dispatch(Action::ShowFileHistory, window, cx);
        });
        pause(bg, 1200).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: selection history {:?} -> {}", dialog(this).and_then(|d| d.lines), show(this));
            this.smoke_press("escape", window, cx);
        });
    }

    /// Labels of the open context menu (smoke tests).
    pub(super) fn menu_labels(&self) -> Vec<String> {
        self.menu
            .as_ref()
            .map(|m| {
                m.entries
                    .iter()
                    .filter_map(|e| match e {
                        super::menu_ui::Entry::Item { label, enabled, .. } => Some(format!("{label}{}", if *enabled { "" } else { " (disabled)" })),
                        super::menu_ui::Entry::Separator => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
