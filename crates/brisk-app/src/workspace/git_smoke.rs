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
            if !["annotate", "merge", "stash", "history"].iter().any(|p| hold(p)) {
                Self::smoke_log(&this, cx, &bg).await;
            }
            if !["annotate", "merge", "stash", "history", "log", "logmenu"].iter().any(|p| hold(p)) {
                Self::smoke_compare(&this, cx, &bg, &root).await;
            }
            if !["annotate", "merge", "stash", "history", "log", "logmenu"].iter().any(|p| hold(p)) {
                Self::smoke_push(&this, cx, &bg, &root).await;
            }
            if !["annotate", "merge", "stash", "history", "log", "logmenu", "push"].iter().any(|p| hold(p)) {
                Self::smoke_branches(&this, cx, &bg, &root).await;
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

    /// Click filter chip `chip` (0 Branch, 1 User) with real mouse messages, then the menu
    /// item whose label contains `item`.
    async fn smoke_pick(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, chip: usize, item: &str) {
        use super::menu_ui::Entry;
        let rows = |e: &Entry| if matches!(e, Entry::Separator) { 9.0 } else { 26.0 };
        let _ = this.update_in(cx, |_, window, _| {
            let scale = window.scale_factor();
            match super::git_log_filter::CHIP_BOUNDS.with(|c| c.borrow()[chip]) {
                Some(b) => {
                    let c = b.center();
                    super::win_input::click(f32::from(c.x) * scale, f32::from(c.y) * scale);
                }
                None => eprintln!("smoke-gitx: chip {chip} was not painted"),
            }
        });
        pause(bg, 500).await;
        let _ = this.update_in(cx, |this, window, _| {
            let Some(m) = this.menu.as_ref() else {
                return eprintln!("smoke-gitx: chip {chip} click opened no menu");
            };
            let labels: Vec<String> = m.entries.iter().filter_map(|e| match e {
                Entry::Item { label, .. } => Some(label.trim().to_string()),
                _ => None,
            }).collect();
            eprintln!("smoke-gitx: chip {chip} menu (click) -> {labels:?}");
            let Some(i) = m.entries.iter().position(|e| matches!(e, Entry::Item { label, .. } if label.contains(item))) else { return };
            // The menu's own layout (menu_ui): 5 px padding, flipped up near the bottom edge.
            let h: f32 = m.entries.iter().map(rows).sum::<f32>() + 10.0;
            let vp = window.viewport_size();
            let (mut x, mut y) = (f32::from(m.pos.x), f32::from(m.pos.y));
            if x + 280.0 > f32::from(vp.width) {
                x = (x - 280.0).max(0.0);
            }
            if y + h > f32::from(vp.height) {
                y = (y - h).max(0.0);
            }
            let top: f32 = m.entries[..i].iter().map(rows).sum();
            let scale = window.scale_factor();
            super::win_input::click((x + 60.0) * scale, (y + 5.0 + top + 13.0) * scale);
        });
        pause(bg, 1300).await;
    }

    async fn smoke_log(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor) {
        fn subjects(this: &Workspace) -> Vec<String> {
            this.git.as_ref().map(|g| g.log.iter().map(|c| c.subject.clone()).collect()).unwrap_or_default()
        }
        let _ = this.update_in(cx, |this, window, cx| {
            if this.bottom != Some(super::BottomTool::Git) {
                this.dispatch(Action::ToggleGitWindow, window, cx);
            }
            this.refresh_git(cx);
        });
        pause(bg, 1200).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: log -> {} commits, selected {:?}", subjects(this).len(), this.git.as_ref().and_then(|g| g.log_selected));
            // Text filter: typed into the field, the log reloads after a pause.
            if let Some(f) = this.git.as_ref().map(|g| g.filter.field.clone()) {
                window.focus(&gpui::Focusable::focus_handle(f.read(cx), cx));
            }
            this.smoke_type("feature", window, cx);
        });
        pause(bg, 1200).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: log text 'feature' -> {:?}", subjects(this));
            // Esc clears the text; User and Branch filters.
            this.smoke_press("escape", window, cx);
        });
        pause(bg, 1200).await;
        // Real Windows clicks: the User chip, then "Bob" in its menu.
        Self::smoke_pick(this, cx, bg, 1, "Bob").await;
        let _ = this.update(cx, |this, _| eprintln!("smoke-gitx: log user Bob (clicks) -> {:?}", subjects(this)));
        Self::smoke_pick(this, cx, bg, 1, "All users").await;
        Self::smoke_pick(this, cx, bg, 0, "feature").await;
        let _ = this.update(cx, |this, _| {
            let q = this.git.as_ref().map(|g| g.filter.query.clone()).unwrap_or_default();
            eprintln!("smoke-gitx: log branch feature (clicks) -> {:?}, query {:?} / {:?}", subjects(this), q.branch, q.author);
        });
        if hold("log") {
            return;
        }
        if hold("logmenu") {
            // The User popup left open by a real click (no item matches), for a screenshot.
            Self::smoke_pick(this, cx, bg, 1, "<keep open>").await;
            return;
        }
        let _ = this.update_in(cx, |this, window, cx| {
            this.clear_log_filters(window, cx);
        });
        pause(bg, 1200).await;
        let _ = this.update(cx, |this, _| eprintln!("smoke-gitx: log filters cleared -> {} commits", subjects(this).len()));
    }

    async fn smoke_compare(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        fn diff(this: &Workspace, cx: &gpui::App) -> String {
            match this.active.and_then(|i| this.tabs.get(i)) {
                Some(super::Tab::Diff(d)) => {
                    let d = d.read(cx);
                    format!("{:?} {:?}", d.title, d.summary())
                }
                _ => "no diff tab".into(),
            }
        }
        let file = root.join("other.py");
        let _ = this.update_in(cx, |this, window, cx| {
            this.open_file(&file, window, cx);
            if let Some(ed) = this.active_editor().cloned() {
                let labels: Vec<String> = this.git_menu_entries(&file, Some(&ed), cx).iter().filter_map(|e| match e {
                    super::menu_ui::Entry::Item { label, .. } => Some(label.clone()),
                    _ => None,
                }).collect();
                eprintln!("smoke-gitx: editor menu git entries -> {labels:?}");
            }
            // Compare with Branch… → the branch menu, then "feature" from it.
            this.dispatch(Action::CompareWithBranch, window, cx);
            eprintln!("smoke-gitx: compare with branch menu -> {:?}", this.menu_labels());
            this.menu = None;
            this.compare_with_rev("other.py", "feature", "other.py", "feature", window, cx);
            eprintln!("smoke-gitx: compare with feature -> {}", diff(this, cx));
            // The root commit's file: no "hash^" on the left.
            let root_commit = sh(&this.root, &["rev-list", "--max-parents=0", "HEAD"]).trim().to_string();
            let f = brisk_git::FileChange { path: "blame.py".into(), old_path: None, status: brisk_git::FileStatus::Added };
            this.show_commit_diff(&root_commit, &f, window, cx);
            eprintln!("smoke-gitx: root commit diff -> {}", diff(this, cx));
            // Log details: right click on a file.
            this.log_file_menu(root_commit.clone(), f.clone(), gpui::point(gpui::px(400.), gpui::px(400.)), cx);
            eprintln!("smoke-gitx: log file menu -> {:?}", this.menu_labels());
            this.menu = None;
            this.compare_with_rev("blame.py", &root_commit, "blame.py", &root_commit[..8], window, cx);
            eprintln!("smoke-gitx: diff with working tree -> {}", diff(this, cx));
        });
        // Rollback in the Commit window: the checked files (other.py and blame.py changed).
        let _ = std::fs::write(root.join("blame.py"), "z = 0\na = 1\nb = 20\nc = 3\nd = 4\n");
        let _ = this.update(cx, |this, cx| this.refresh_git(cx));
        pause(bg, 1200).await;
        let _ = this.update_in(cx, |this, window, cx| {
            this.dispatch(Action::SaveAll, window, cx);
            if this.left != Some(super::LeftTool::Commit) {
                this.dispatch(Action::ToggleCommitWindow, window, cx);
            }
            window.focus(&this.commit_focus);
            eprintln!("smoke-gitx: before rollback -> {:?}", sh(&this.root, &["status", "--porcelain"]));
            // Space unchecks the first row: only the other checked file is rolled back.
            this.smoke_press("space", window, cx);
            this.dispatch(Action::RollbackChanges, window, cx);
            let items: Vec<String> = this.popup.as_ref().map(|p| p.items.iter().map(|i| format!("{} | {}", i.label, i.detail)).collect()).unwrap_or_default();
            eprintln!("smoke-gitx: rollback confirm -> {items:?}");
            this.smoke_press("enter", window, cx);
        });
        pause(bg, 1500).await;
        eprintln!("smoke-gitx: after rollback -> {:?}", sh(root, &["status", "--porcelain"]));
    }

    /// Push into a bare repository next to the scratch folder, then Fetch and Update Project
    /// bring in a commit pushed from a second clone.
    async fn smoke_push(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        fn dialog(this: &Workspace) -> String {
            match this.git.as_ref().and_then(|g| g.dialog.as_ref()) {
                Some(super::git_merge_ui::GitDialog::Push(d)) => match (&d.outgoing, &d.error) {
                    (Some(o), _) => {
                        let subjects: Vec<&str> = o.commits.iter().map(|c| c.subject.as_str()).collect();
                        let files: Vec<&str> = d.files.iter().map(|f| f.path.as_str()).collect();
                        format!("to {} new={} commits {subjects:?}, files of the first {files:?}", o.target, o.new_branch)
                    }
                    (None, e) => format!("error {e:?}"),
                },
                _ => "no push dialog".into(),
            }
        }
        fn toast(this: &Workspace) -> String {
            this.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
        }
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let origin = root.with_file_name(format!("{name}-origin.git"));
        let other = root.with_file_name(format!("{name}-other"));
        let _ = std::fs::remove_dir_all(&origin);
        let _ = std::fs::remove_dir_all(&other);
        let origin_s = origin.to_string_lossy().into_owned();
        sh(root, &["init", "-q", "--bare", &origin_s]);
        let _ = this.update_in(cx, |this, window, cx| {
            this.dispatch(Action::Push, window, cx);
        });
        pause(bg, 1000).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: push without a remote -> {}", dialog(this));
            this.smoke_press("escape", window, cx);
        });
        sh(root, &["remote", "add", "origin", &origin_s]);
        let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::Push, window, cx));
        pause(bg, 1000).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: push dialog (new branch) -> {}", dialog(this));
            this.smoke_press("enter", window, cx);
        });
        pause(bg, 2000).await;
        let _ = this.update(cx, |this, _| eprintln!("smoke-gitx: pushed -> toast {:?}, origin has {} commits", toast(this), sh(&this.root, &["rev-list", "--count", "origin/main"]).trim()));
        let _ = std::fs::write(root.join("pushme.py"), "p = 1\n");
        sh(root, &["add", "pushme.py"]);
        commit_as(root, "Alice", "2025-08-01T10:00:00", "Add pushme.py");
        let _ = this.update_in(cx, |this, window, cx| {
            this.refresh_git(cx);
            this.dispatch(Action::Push, window, cx);
        });
        pause(bg, 1200).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: push dialog (1 commit) -> {}", dialog(this));
            if hold("push") {
                return;
            }
            this.smoke_press("enter", window, cx);
        });
        if hold("push") {
            return;
        }
        pause(bg, 2000).await;
        let _ = this.update(cx, |this, _| eprintln!("smoke-gitx: pushed again -> toast {:?}", toast(this)));
        // A teammate pushes: Fetch reports it, Update Project merges it in.
        sh(root, &["clone", "-q", "-b", "main", &origin_s, &other.to_string_lossy()]);
        sh(&other, &["config", "user.name", "Carol"]);
        sh(&other, &["config", "user.email", "carol@example.com"]);
        let _ = std::fs::write(other.join("remote.py"), "r = 1\n");
        sh(&other, &["add", "remote.py"]);
        sh(&other, &["commit", "-q", "-m", "Add remote.py"]);
        sh(&other, &["push", "-q"]);
        let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::Fetch, window, cx));
        pause(bg, 2000).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: fetch -> toast {:?}", toast(this));
            this.dispatch(Action::UpdateProject, window, cx);
        });
        pause(bg, 2500).await;
        let _ = this.update(cx, |this, _| eprintln!("smoke-gitx: update project -> toast {:?}, remote.py exists {}", toast(this), this.root.join("remote.py").exists()));
        let _ = std::fs::remove_dir_all(&other);
    }

    /// Branches popup: → on a branch, then one of its actions (by label).
    fn smoke_branch_op(&mut self, branch: &str, op: &str, window: &mut Window, cx: &mut Context<Self>) -> Vec<String> {
        self.dispatch(Action::Branches, window, cx);
        let labels = |this: &Self| this.popup.as_ref().map(|p| p.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()).unwrap_or_default();
        if let Some(p) = self.popup.as_mut() {
            p.selected = p.items.iter().position(|i| i.label == branch).unwrap_or(0);
        }
        self.smoke_press("right", window, cx);
        let ops = labels(self);
        if let Some(p) = self.popup.as_mut() {
            if let Some(i) = p.items.iter().position(|i| i.label.contains(op)) {
                p.selected = i;
                self.smoke_press("enter", window, cx);
            }
        }
        ops
    }

    async fn smoke_branches(this: &WeakEntity<Self>, cx: &mut AsyncWindowContext, bg: &gpui::BackgroundExecutor, root: &Path) {
        fn toast(this: &Workspace) -> String {
            this.toast.as_ref().map(|t| t.0.clone()).unwrap_or_default()
        }
        // topic: one commit ahead (merges as a fast-forward); spike: never merged.
        for (b, f) in [("topic", "topic.py"), ("spike", "spike.py")] {
            sh(root, &["switch", "-q", "-c", b]);
            let _ = std::fs::write(root.join(f), "t = 1\n");
            sh(root, &["add", f]);
            commit_as(root, "Bob", "2025-09-01T10:00:00", &format!("Work on {b}"));
            sh(root, &["switch", "-q", "main"]);
        }
        let _ = this.update(cx, |this, cx| this.refresh_git(cx));
        pause(bg, 1200).await;
        let _ = this.update_in(cx, |this, window, cx| {
            let ops = this.smoke_branch_op("topic", "Merge", window, cx);
            eprintln!("smoke-gitx: branch topic actions -> {ops:?}");
        });
        pause(bg, 1800).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: merge topic -> toast {:?}, topic.py {}", toast(this), this.root.join("topic.py").exists());
            this.smoke_branch_op("spike", "Delete", window, cx);
            let confirm: Vec<String> = this.popup.as_ref().map(|p| p.items.iter().map(|i| format!("{} | {}", i.label, i.detail)).collect()).unwrap_or_default();
            eprintln!("smoke-gitx: delete unmerged spike -> {confirm:?}, selected {:?}", this.popup.as_ref().map(|p| p.selected));
            // Up to Force Delete, Enter.
            this.smoke_press("up enter", window, cx);
        });
        pause(bg, 1500).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: force delete spike -> toast {:?}", toast(this));
            this.smoke_branch_op("topic", "Delete", window, cx);
        });
        pause(bg, 1500).await;
        let _ = this.update_in(cx, |this, window, cx| {
            eprintln!("smoke-gitx: delete merged topic -> toast {:?}, branches {:?}", toast(this), sh(&this.root, &["branch", "--format=%(refname:short)"]).lines().collect::<Vec<_>>());
            let ops = this.smoke_branch_op("main", "Show in Log", window, cx);
            let q = this.git.as_ref().map(|g| g.filter.query.branch.clone());
            eprintln!("smoke-gitx: current branch actions -> {ops:?}, log branch filter {q:?}");
        });
        pause(bg, 800).await;
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
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}
