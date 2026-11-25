//! `BRISK_SMOKE_MEMBUDGET=1` (on a scratch copy of a project: writes `mb_flood.py` and
//! `mb_tabs/`): runs a script printing 200k lines, opens and closes 50 tabs, runs Find in
//! Files, then prints the working set, private bytes and live child processes and exits with
//! code 1 above the budget (`=<MB>` sets it; `=hold` keeps the window open instead of exiting).

use super::{Action, Workspace};
use gpui::{Context, Window};
use std::time::{Duration, Instant};

/// Default budget for the working set after the run. Measured on the demo project with the
/// terminal and ty running: ~110 MB idle, ~170 MB after the 200k-line run with the 20k-line
/// Run console cap. The old 100k-line cap ended at ~380 MB, so 250 MB catches a lost cap
/// (or a leak of a similar size) and leaves room for GPU driver and font caches.
const DEFAULT_BUDGET_MB: u64 = 250;
const LINES: usize = 200_000;
const TABS: usize = 50;

fn mb(b: u64) -> u64 {
    b / (1024 * 1024)
}

impl Workspace {
    pub(super) fn membudget_smoke(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let arg = std::env::var("BRISK_SMOKE_MEMBUDGET").unwrap_or_default();
        let hold = arg == "hold";
        let budget = arg.parse::<u64>().ok().filter(|&n| n > 1).unwrap_or(DEFAULT_BUDGET_MB);
        let root = self.root.clone();
        let flood = root.join("mb_flood.py");
        let _ =
            std::fs::write(&flood, format!("for i in range({LINES}):\n    print(f'line {{i:06d}} the quick brown fox jumps over the lazy dog 0123456789')\n"));
        let dir = root.join("mb_tabs");
        let _ = std::fs::create_dir_all(&dir);
        let files: Vec<_> = (0..TABS)
            .map(|i| {
                let p = dir.join(format!("tab_{i:02}.py"));
                let body: String = (0..400).map(|n| format!("def f_{i}_{n}(x):\n    return x * {n}  # needle\n\n")).collect();
                let _ = std::fs::write(&p, body);
                p
            })
            .collect();
        let (ws0, _) = brisk_proc::memory().unwrap_or_default();
        eprintln!("smoke-membudget: start, working set {} MB", mb(ws0));
        let t0 = Instant::now();
        let bg = cx.background_executor().clone();
        cx.spawn_in(window, async move |this, cx| {
            let wait = |ms| bg.timer(Duration::from_millis(ms));
            // 1. The flood run, as Ctrl+Shift+F10 on the file.
            for _ in 0..100 {
                let ready = this.update(cx, |this, _| this.runs.interpreter(&this.root).is_some() || this.runs.all.is_some()).unwrap_or(true);
                if ready {
                    break;
                }
                wait(100).await;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.open_file(&flood, window, cx);
                this.dispatch(Action::RunContext, window, cx);
            });
            for _ in 0..1200 {
                wait(100).await;
                let done = this.update(cx, |this, cx| this.runs.panel.as_ref().is_some_and(|p| !p.read(cx).is_running())).unwrap_or(true);
                if done {
                    break;
                }
            }
            eprintln!("smoke-membudget: run finished after {:.1?}, working set {} MB", t0.elapsed(), mb(brisk_proc::memory().unwrap_or_default().0));

            // 2. Open and close 50 tabs.
            for f in &files {
                let _ = this.update_in(cx, |this, window, cx| this.open_file(f, window, cx));
                wait(20).await;
            }
            let peak_tabs = brisk_proc::memory().unwrap_or_default().0;
            let _ = this.update_in(cx, |this, window, cx| {
                let eds: Vec<_> = this.editors().filter(|e| e.read(cx).path.as_ref().is_some_and(|p| p.starts_with(&dir))).cloned().collect();
                for e in eds {
                    this.close_editor(&e, window, cx);
                }
            });
            wait(500).await;
            eprintln!("smoke-membudget: {TABS} tabs opened ({} MB) and closed ({} MB)", mb(peak_tabs), mb(brisk_proc::memory().unwrap_or_default().0));

            // 3. Find in Files (over the 50 new files too: index them first).
            let _ = this.update(cx, |this, cx| this.reindex_files(cx));
            for _ in 0..100 {
                wait(100).await;
                let n = this.update(cx, |this, _| this.files.as_ref().map_or(0, |f| f.files.len())).unwrap_or(usize::MAX);
                if n >= TABS {
                    break;
                }
            }
            let _ = this.update_in(cx, |this, window, cx| {
                this.find.set_query_for_smoke("needle");
                this.open_find(false, window, cx);
            });
            for _ in 0..240 {
                wait(250).await;
                let done = this.update(cx, |this, _| this.files.is_some() && this.find.idle()).unwrap_or(true);
                if done {
                    break;
                }
            }
            let summary = this.update(cx, |this, _| this.find.summary()).unwrap_or_default();
            eprintln!("smoke-membudget: find: {summary}");

            // 4. Settle and report.
            wait(2000).await;
            let (ws, private) = brisk_proc::memory().unwrap_or_default();
            let kids = brisk_proc::live_children().map_or("n/a".to_string(), |n| n.to_string());
            let ok = mb(ws) <= budget;
            eprintln!(
                "smoke-membudget: working set {} MB, private {} MB, budget {budget} MB, live children {kids}, total {:.1?} -> {}",
                mb(ws),
                mb(private),
                t0.elapsed(),
                if ok { "OK" } else { "FAIL" }
            );
            if !hold {
                // Exit right away with the verdict: quitting first ends the process with 0.
                // Children die with the Job Object.
                std::process::exit(if ok { 0 } else { 1 });
            }
        })
        .detach();
    }
}
