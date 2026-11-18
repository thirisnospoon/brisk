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
            if std::env::var_os("BRISK_SMOKE_GITX_HOLD").is_some() {
                return;
            }
            this.dispatch(Action::Annotate, window, cx);
            eprintln!("smoke-gitx: annotate again (close) -> shown={:?}", this.active_editor().map(|e| e.read(cx).annotation_shown()));
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
