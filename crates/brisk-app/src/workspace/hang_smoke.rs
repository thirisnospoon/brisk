//! `BRISK_SMOKE_HANG=1` (on a scratch folder; no `.git` in it, or `=keepgit`): measures the
//! longest main-thread stall of each phase with the [`crate::stall`] watchdog and exits with
//! code 1 when one is over [`LIMIT_MS`]. With fewer than 20k files in the folder it first
//! generates a 51k-file tree (`app/`, `lib/`, `node_modules/`), a 100 MB one-line `big.json`
//! and a 20 MB `blob.bin` (while the window is open: that is also the watcher storm).
//! Phases: indexing, `git init` + first status with Commit and Git windows open, Find in
//! Files, opening the big files, then 2000 files created and deleted in the root (the storm
//! after `npm install` / `git checkout`). `=hold` keeps the window open.

use super::{Action, Workspace};
use crate::stall;
use gpui::{Context, Window};
use std::path::Path;
use std::time::{Duration, Instant};

pub const LIMIT_MS: u64 = 250;

fn count_files(root: &Path, cap: usize) -> usize {
    let mut n = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push(e.path());
            } else {
                n += 1;
                if n >= cap {
                    return n;
                }
            }
        }
    }
    n
}

fn generate(root: &Path) {
    for top in ["app", "node_modules", "lib"] {
        for d in 0..170 {
            let dir = root.join(top).join(format!("pkg{d:03}")).join("sub");
            let _ = std::fs::create_dir_all(&dir);
            for f in 0..100 {
                let ext = if top == "node_modules" { "js" } else { "py" };
                let _ = std::fs::write(
                    dir.join(format!("m{f:03}.{ext}")),
                    format!("def func_{d}_{f}(x):\n    return x + {f}  # needle\n\nclass C{d}_{f}:\n    pass\n"),
                );
            }
        }
    }
    let mut json = String::from("[");
    for i in 0..2_500_000 {
        if i > 0 {
            json.push(',');
        }
        json.push_str(&format!("{{\"k\": {i}, \"v\": \"xxxxxxxxxxxxxxxxxxxx\"}}"));
    }
    json.push(']');
    let _ = std::fs::write(root.join("big.json"), json);
    let mut x: u64 = 0x9e3779b97f4a7c15;
    let blob: Vec<u8> = (0..20_000_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    let _ = std::fs::write(root.join("blob.bin"), blob);
}

impl Workspace {
    pub(super) fn hang_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let arg = std::env::var("BRISK_SMOKE_HANG").unwrap_or_default();
        let hold = arg == "hold";
        let root = self.root.clone();
        let bg = cx.background_executor().clone();
        let t0 = Instant::now();
        cx.spawn_in(window, async move |this, cx| {
            let wait = |ms| bg.timer(Duration::from_millis(ms));
            let mut results: Vec<(&'static str, u64, String)> = Vec::new();
            let mut phase = |name: &'static str| {
                let (ms, what) = stall::worst();
                eprintln!("smoke-hang: {name}: worst stall {ms} ms{} ({:.1?})", if ms > 0 { format!(" during '{what}'") } else { String::new() }, t0.elapsed());
                results.push((name, ms, what));
                stall::reset();
            };
            stall::reset();
            let root2 = root.clone();
            let have = bg.spawn(async move { count_files(&root2, 20_000) }).await;
            if have < 20_000 {
                eprintln!("smoke-hang: generating 51k files in {}", root.display());
                let root2 = root.clone();
                bg.spawn(async move { generate(&root2) }).await;
                wait(3000).await; // let the disk poll see it
                phase("generate (watcher storm)");
            }
            // 1. Indexing.
            let _ = this.update(cx, |this, cx| this.reindex_files(cx));
            for _ in 0..1200 {
                wait(100).await;
                let n = this.update(cx, |this, _| this.files.as_ref().map_or(0, |f| f.files.len())).unwrap_or(usize::MAX);
                if n >= 20_000 {
                    break;
                }
            }
            let n = this.update(cx, |this, _| this.files.as_ref().map_or(0, |f| f.files.len())).unwrap_or(0);
            eprintln!("smoke-hang: indexed {n} files");
            phase("index");

            // 2. git init + first status, Commit and Git windows open.
            if arg != "keepgit" {
                let _ = std::fs::remove_dir_all(root.join(".git"));
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.dispatch(Action::ToggleCommitWindow, window, cx);
                if this.git.is_none() {
                    this.create_git_repo(cx);
                } else {
                    this.refresh_git(cx);
                }
            });
            for _ in 0..600 {
                wait(100).await;
                let done = this.update(cx, |this, _| this.git.as_ref().is_some_and(|g| g.loaded)).unwrap_or(true);
                if done {
                    break;
                }
            }
            let rows = this.update(cx, |this, _| this.git.as_ref().map_or(0, |g| g.status.untracked().count())).unwrap_or(0);
            eprintln!("smoke-hang: git status: {rows} unversioned files");
            wait(1500).await;
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::ToggleGitWindow, window, cx));
            wait(1500).await;
            phase("git init + status + Commit/Git windows");

            // 2b. Git Branches popup with 400 branches: open it and walk 60 rows down.
            let root2 = root.clone();
            let made = bg
                .spawn(async move {
                    let git = |args: &[&str]| {
                        let mut c = std::process::Command::new("git");
                        c.arg("-C").arg(&root2).args(args).env("GIT_TERMINAL_PROMPT", "0");
                        #[cfg(windows)]
                        {
                            use std::os::windows::process::CommandExt;
                            c.creation_flags(0x0800_0000);
                        }
                        c.output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    };
                    git(&["add", "app/pkg000"])?;
                    git(&["-c", "user.name=smoke", "-c", "user.email=smoke@example.invalid", "commit", "-qm", "first"])?;
                    let head = git(&["rev-parse", "HEAD"])?;
                    // Refs written directly: 400 `git branch` processes take a minute on Windows.
                    let heads = root2.join(".git/refs/heads/feature");
                    std::fs::create_dir_all(&heads).ok()?;
                    for i in 0..400 {
                        std::fs::write(heads.join(format!("topic-{i:03}")), format!("{head}\n")).ok()?;
                    }
                    Some(())
                })
                .await;
            let _ = this.update(cx, |this, cx| this.refresh_git(cx));
            for _ in 0..100 {
                wait(100).await;
                if this.update(cx, |this, _| this.git.as_ref().map_or(0, |g| g.branches.len())).unwrap_or(0) > 400 {
                    break;
                }
            }
            stall::reset();
            let _ = this.update_in(cx, |this, window, cx| this.dispatch(Action::Branches, window, cx));
            wait(500).await;
            for _ in 0..60 {
                let _ = this.update_in(cx, |this, window, cx| {
                    let ev = gpui::KeyDownEvent { keystroke: gpui::Keystroke::parse("down").unwrap(), is_held: false };
                    this.popup_key(&ev, window, cx);
                    cx.notify();
                });
                wait(16).await;
            }
            let shown = this.update(cx, |this, _| this.popup.as_ref().map(|p| (p.items.len(), p.selected))).ok().flatten();
            eprintln!("smoke-hang: branches popup (made: {}): items and selection {shown:?}", made.is_some());
            if hold {
                return; // leave the popup open for a screenshot
            }
            let _ = this.update_in(cx, |this, window, cx| {
                let ev = gpui::KeyDownEvent { keystroke: gpui::Keystroke::parse("escape").unwrap(), is_held: false };
                this.popup_key(&ev, window, cx);
            });
            phase("Git Branches popup, 400 branches, 60 × Down");

            // 3. Find in Files.
            let _ = this.update_in(cx, |this, window, cx| {
                this.find.set_query_for_smoke("needle");
                this.open_find(false, window, cx);
            });
            for _ in 0..600 {
                wait(100).await;
                if this.update(cx, |this, _| this.find.idle()).unwrap_or(true) {
                    break;
                }
            }
            let summary = this.update(cx, |this, _| this.find.summary()).unwrap_or_default();
            eprintln!("smoke-hang: find: {summary}");
            phase("find in files");

            // 4. Big files.
            for name in ["big.json", "blob.bin"] {
                let p = root.join(name);
                let _ = this.update_in(cx, |this, window, cx| this.open_file(&p, window, cx));
                wait(2500).await;
                let title = this.update(cx, |this, cx| this.active_editor().map(|e| e.read(cx).title())).ok().flatten();
                eprintln!("smoke-hang: opened {name} -> tab {title:?}");
                phase(if name == "big.json" { "open 100 MB one-line json" } else { "open 20 MB binary" });
            }

            // 5. Watcher storm: 2000 files appear and vanish in the project root.
            let root2 = root.clone();
            bg.spawn(async move {
                let d = root2.join("storm");
                let _ = std::fs::create_dir_all(&d);
                for i in 0..2000 {
                    let _ = std::fs::write(d.join(format!("s{i}.py")), "x = 1\n");
                    let _ = std::fs::write(root2.join(format!("storm_{}.tmp", i % 20)), "x");
                }
            })
            .await;
            wait(4000).await;
            let root2 = root.clone();
            bg.spawn(async move {
                let _ = std::fs::remove_dir_all(root2.join("storm"));
                for i in 0..20 {
                    let _ = std::fs::remove_file(root2.join(format!("storm_{i}.tmp")));
                }
            })
            .await;
            wait(4000).await;
            phase("watcher storm (2000 files)");

            let worst = results.iter().max_by_key(|r| r.1).cloned();
            let ok = worst.as_ref().is_none_or(|w| w.1 <= LIMIT_MS);
            if let Some((name, ms, what)) = &worst {
                eprintln!("smoke-hang: worst {ms} ms in '{name}' ('{what}'), limit {LIMIT_MS} ms -> {}", if ok { "OK" } else { "FAIL" });
            }
            if !hold {
                // Exit right away with the verdict: quitting first ends the process with 0.
                // Children die with the Job Object.
                std::process::exit(if ok { 0 } else { 1 });
            }
        })
        .detach();
    }
}
